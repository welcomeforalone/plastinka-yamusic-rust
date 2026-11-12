//! История прослушиваний с сервера: `GET /music-history?fullModelsCount=N`.
//! Ответ — вкладки по дням (`historyTabs[]`), в каждой группы «контекст +
//! треки». Полные модели приходят только для первых N треков, остальные
//! догружаем по id пачками `POST /tracks`. Спека — раздел «История»
//! `docs/api/stage-9.md`; формат пересказан по справке MarshalX, **вживую не
//! проверен**. Разбор мягкий: непонятный элемент пропускается.

use std::time::{SystemTime, UNIX_EPOCH};

use reqwest::StatusCode;
use reqwest::header::AUTHORIZATION;
use serde_json::Value;

use super::tracks::{Track, clean, fetch_tracks, id_text, numeric_id, parse_track};
use super::{API_BASE, ApiError, MUSIC_CLIENT, auth_header, check_status};
use crate::net::NetError;

/// Сколько полных моделей просим в самом ответе.
const FULL_MODELS: u32 = 50;
/// Сколько дней и треков берём: длинная история окно не перегружает.
const MAX_DAYS: usize = 30;
const MAX_TRACKS: usize = 300;

/// Дата: год, месяц, день.
pub type Ymd = (i32, u32, u32);

/// Откуда играли.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryContext {
    Album {
        id: String,
        title: String,
    },
    Artist {
        id: String,
        name: String,
    },
    Playlist {
        owner: String,
        kind: String,
        title: String,
    },
    Wave,
    /// Незнакомый вид: только подпись.
    Other(String),
}

impl HistoryContext {
    /// Подпись над группой треков.
    pub fn label(&self) -> String {
        match self {
            Self::Album { title, .. } => format!("Альбом «{title}»"),
            Self::Artist { name, .. } => format!("Исполнитель {name}"),
            Self::Playlist { title, .. } => format!("Плейлист «{title}»"),
            Self::Wave => "Моя волна".to_owned(),
            Self::Other(title) => title.clone(),
        }
    }
}

/// Группа: контекст и треки, которые из него играли.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryItem {
    pub context: HistoryContext,
    pub tracks: Vec<Track>,
}

/// День истории.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryDay {
    pub date: Option<Ymd>,
    pub items: Vec<HistoryItem>,
}

/// Трек из ответа: полная модель или только идентификатор.
#[derive(Debug, Clone, PartialEq)]
enum Slot {
    Full(Track),
    Id(String),
}

struct RawItem {
    context: HistoryContext,
    tracks: Vec<Slot>,
}

struct RawDay {
    date: Option<Ymd>,
    items: Vec<RawItem>,
}

/// История с подгруженными треками.
pub async fn fetch_history(
    http: &reqwest::Client,
    token: &str,
) -> Result<Vec<HistoryDay>, ApiError> {
    let (status, body) = request(http, token).await?;
    let days = parse_history(status, &body)?;
    // Треки без полной модели — одним набором id.
    let missing: Vec<String> = days
        .iter()
        .flat_map(|day| &day.items)
        .flat_map(|item| &item.tracks)
        .filter_map(|slot| match slot {
            Slot::Id(id) => Some(id.clone()),
            Slot::Full(_) => None,
        })
        .collect();
    let loaded = if missing.is_empty() {
        Vec::new()
    } else {
        fetch_tracks(http, token, &missing, |_| {}).await?
    };
    Ok(resolve(days, &loaded))
}

async fn request(http: &reqwest::Client, token: &str) -> Result<(StatusCode, String), ApiError> {
    #[cfg(debug_assertions)]
    if let Some(reply) = test_hook::reply(token).await? {
        return Ok(reply);
    }
    let response = http
        .get(format!(
            "{API_BASE}/music-history?fullModelsCount={FULL_MODELS}"
        ))
        .header(AUTHORIZATION, auth_header(token)?)
        .header("X-Yandex-Music-Client", MUSIC_CLIENT)
        .send()
        .await
        .map_err(NetError::from)?;
    let status = response.status();
    let body = response.text().await.map_err(NetError::from)?;
    Ok((status, body))
}

/// Подставляет догруженные треки; не найденные и недоступные (без названия)
/// отбрасываются, пустые группы и дни — тоже.
fn resolve(days: Vec<RawDay>, loaded: &[Track]) -> Vec<HistoryDay> {
    let find = |id: &str| loaded.iter().find(|track| track.id == id).cloned();
    days.into_iter()
        .filter_map(|day| {
            let items: Vec<HistoryItem> = day
                .items
                .into_iter()
                .filter_map(|item| {
                    let tracks: Vec<Track> = item
                        .tracks
                        .into_iter()
                        .filter_map(|slot| match slot {
                            Slot::Full(track) => Some(track),
                            Slot::Id(id) => find(&id),
                        })
                        .filter(|track| track.title.is_some())
                        .collect();
                    (!tracks.is_empty()).then_some(HistoryItem {
                        context: item.context,
                        tracks,
                    })
                })
                .collect();
            (!items.is_empty()).then_some(HistoryDay {
                date: day.date,
                items,
            })
        })
        .collect()
}

fn parse_history(status: StatusCode, body: &str) -> Result<Vec<RawDay>, ApiError> {
    check_status(status)?;
    let envelope: Value = serde_json::from_str(body).map_err(|_| ApiError::InvalidResponse)?;
    let tabs = envelope
        .get("result")
        .and_then(|result| result.get("historyTabs"))
        .and_then(Value::as_array)
        .ok_or(ApiError::InvalidResponse)?;
    let mut budget = MAX_TRACKS;
    let mut days = Vec::new();
    for tab in tabs.iter().take(MAX_DAYS) {
        if budget == 0 {
            break;
        }
        let date = tab.get("date").and_then(Value::as_str).and_then(parse_date);
        let mut items = Vec::new();
        for item in tab
            .get("items")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            let Some(context) = item.get("context").and_then(parse_context) else {
                continue;
            };
            let tracks: Vec<Slot> = item
                .get("tracks")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .filter_map(parse_slot)
                .take(budget)
                .collect();
            budget = budget.saturating_sub(tracks.len());
            if !tracks.is_empty() {
                items.push(RawItem { context, tracks });
            }
        }
        if !items.is_empty() {
            days.push(RawDay { date, items });
        }
    }
    Ok(days)
}

/// `{type, data:{itemId, fullModel}}` у трека.
fn parse_slot(value: &Value) -> Option<Slot> {
    let data = value.get("data")?;
    if let Some(track) = data.get("fullModel").and_then(parse_track) {
        return Some(Slot::Full(track));
    }
    let id = data.get("itemId")?.get("trackId").and_then(id_text)?;
    Some(Slot::Id(id))
}

/// `{type, data:{itemId, fullModel}}` у контекста.
fn parse_context(value: &Value) -> Option<HistoryContext> {
    let kind = value.get("type").and_then(Value::as_str)?;
    let data = value.get("data");
    let item_id = data.and_then(|data| data.get("itemId"));
    let full = data.and_then(|data| data.get("fullModel"));
    let text = |source: Option<&Value>, key: &str| {
        source
            .and_then(|value| value.get(key))
            .and_then(Value::as_str)
            .and_then(clean)
    };
    // Идентификатор лежит в `itemId` (строкой или объектом `{id}`) или в
    // самой модели.
    let single_id = || {
        item_id
            .and_then(|id| id.get("id").or(Some(id)).and_then(numeric_id))
            .or_else(|| full.and_then(|full| full.get("id")).and_then(numeric_id))
    };
    Some(match kind {
        "album" => HistoryContext::Album {
            id: single_id()?,
            title: text(full, "title").unwrap_or_else(|| "Альбом".to_owned()),
        },
        "artist" => HistoryContext::Artist {
            id: single_id()?,
            name: text(full, "name").unwrap_or_else(|| "Исполнитель".to_owned()),
        },
        "playlist" => {
            let owner = item_id
                .and_then(|id| id.get("uid"))
                .and_then(numeric_id)
                .or_else(|| {
                    full.and_then(|full| full.get("owner"))
                        .and_then(|owner| owner.get("uid"))
                        .and_then(numeric_id)
                })?;
            let kind = item_id
                .and_then(|id| id.get("kind"))
                .and_then(numeric_id)
                .or_else(|| full.and_then(|full| full.get("kind")).and_then(numeric_id))?;
            HistoryContext::Playlist {
                owner,
                kind,
                title: text(full, "title").unwrap_or_else(|| "Плейлист".to_owned()),
            }
        }
        "wave" => HistoryContext::Wave,
        _ => HistoryContext::Other(
            text(full, "title")
                .or_else(|| text(full, "name"))
                .unwrap_or_else(|| "Подборка".to_owned()),
        ),
    })
}

/// `2026-11-12` (после даты может идти время) → год, месяц, день.
pub fn parse_date(text: &str) -> Option<Ymd> {
    let date = text.get(..10)?;
    let mut parts = date.split('-');
    let year: i32 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next()?.parse().ok()?;
    let day: u32 = parts.next()?.parse().ok()?;
    ((1..=12).contains(&month) && (1..=31).contains(&day) && parts.next().is_none())
        .then_some((year, month, day))
}

/// Номер дня от 1970-01-01 (григорианский календарь).
fn days_from_civil((year, month, day): Ymd) -> i64 {
    let y = i64::from(year) - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (i64::from(month) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Дата по номеру дня.
fn civil_from_days(days: i64) -> Ymd {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let month = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    let year = i32::try_from(yoe + era * 400 + i64::from(month <= 2)).unwrap_or(1970);
    (year, month, day)
}

/// Сегодняшняя дата по UTC (часового пояса без внешних крейтов не знаем).
pub fn today_utc() -> Ymd {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    civil_from_days(i64::try_from(secs / 86_400).unwrap_or(0))
}

/// Подпись дня: «Сегодня», «Вчера», «12 ноября» (с годом, если он не
/// нынешний). Без даты — «Раньше».
pub fn day_label(date: Option<Ymd>, today: Ymd) -> String {
    const MONTHS: [&str; 12] = [
        "января",
        "февраля",
        "марта",
        "апреля",
        "мая",
        "июня",
        "июля",
        "августа",
        "сентября",
        "октября",
        "ноября",
        "декабря",
    ];
    let Some((year, month, day)) = date else {
        return "Раньше".to_owned();
    };
    match days_from_civil(today) - days_from_civil((year, month, day)) {
        0 => "Сегодня".to_owned(),
        1 => "Вчера".to_owned(),
        _ => {
            let name = usize::try_from(month - 1)
                .ok()
                .and_then(|index| MONTHS.get(index))
                .copied()
                .unwrap_or("");
            if year == today.0 {
                format!("{day} {name}")
            } else {
                format!("{day} {name} {year}")
            }
        }
    }
}

/// Дата `days` дней назад от `today`: для готовых ответов хука.
#[cfg(any(test, debug_assertions))]
fn days_ago(today: Ymd, days: i64) -> Ymd {
    civil_from_days(days_from_civil(today) - days)
}

/// Готовые ответы для отладочного хука `PLASTINKA_TEST_HISTORY`.
#[cfg(any(test, debug_assertions))]
mod fixtures {
    use serde_json::{Value, json};

    use super::{Ymd, days_ago};

    #[cfg(debug_assertions)]
    pub const NAMES: &[&str] = &["sample", "empty", "error", "slow"];

    /// Полная модель трека и его запись в группе.
    fn full(id: u64, title: &str, artist: (u64, &str), album: u64) -> Value {
        let model = json!({
            "id": id.to_string(), "realId": id.to_string(), "title": title,
            "available": true, "durationMs": (id % 300 + 60) * 1000,
            "coverUri": format!("avatars.example/{album}/%%"),
            "artists": [{"id": artist.0, "name": artist.1}],
            "albums": [{"id": album, "title": format!("Альбом {album}")}],
        });
        json!({"type": "track", "data": {
            "itemId": {"trackId": id.to_string(), "albumId": album.to_string()},
            "fullModel": model}})
    }

    fn bare(id: &str, album: &str) -> Value {
        json!({"type": "track", "data": {"itemId": {"trackId": id, "albumId": album}}})
    }

    /// Ответ `sample` на день `today`: сегодня, вчера и неделей раньше.
    pub fn sample(today: Ymd) -> Value {
        let date = |days: i64| {
            let (y, m, d) = days_ago(today, days);
            format!("{y:04}-{m:02}-{d:02}")
        };
        json!({"result": {"historyTabs": [
            {"date": date(0), "items": [
                {"context": {"type": "album", "data": {
                    "itemId": {"id": "10101"},
                    "fullModel": {"id": 10101, "title": "Камнем по голове"}}},
                 "tracks": [
                    full(29_238_706, "Дурак и молния", (41052, "Король и Шут"), 10101),
                    full(900_001, "Прыгну со скалы", (41052, "Король и Шут"), 10101),
                 ]},
                {"context": {"type": "wave", "data": {
                    "itemId": {"seeds": ["user:onyourwave"]}, "fullModel": {}}},
                 "tracks": [
                    // Без полной модели: догружается по id (`PLASTINKA_TEST_LIKES`).
                    bare("7019818", "777"),
                    full(700_001, "Волновой трек", (853_465, "Tame Impala"), 3),
                 ]},
            ]},
            {"date": date(1), "items": [
                {"context": {"type": "playlist", "data": {
                    "itemId": {"uid": "1000", "kind": "3"},
                    "fullModel": {"title": "Утренний", "kind": 3, "owner": {"uid": 1000}}}},
                 "tracks": [full(900_002, "Утро", (1001, "Оркестр"), 55)]},
                {"context": {"type": "artist", "data": {
                    "itemId": {"id": "853465"}, "fullModel": {"id": 853_465, "name": "Tame Impala"}}},
                 "tracks": [
                    full(900_003, "Let It Happen", (853_465, "Tame Impala"), 66),
                    full(900_004, "The Less I Know the Better", (853_465, "Tame Impala"), 66),
                 ]},
            ]},
            {"date": date(8), "items": [
                {"context": {"type": "album", "data": {
                    "itemId": {"id": "777"}, "fullModel": {"id": 777, "title": "Старый альбом"}}},
                 "tracks": [full(900_005, "Давно", (5, "Солистка"), 777)]},
            ]},
        ]}})
    }

    pub fn empty() -> Value {
        json!({"result": {"historyTabs": []}})
    }
}

/// Хук `PLASTINKA_TEST_HISTORY`: готовые ответы вместо сервиса.
#[cfg(debug_assertions)]
mod test_hook {
    use reqwest::StatusCode;

    use super::{ApiError, fixtures, today_utc};

    pub async fn reply(token: &str) -> Result<Option<(StatusCode, String)>, ApiError> {
        let Some(name) = crate::api::test_hook("PLASTINKA_TEST_HISTORY", token)? else {
            return Ok(None);
        };
        if name == "slow" {
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        }
        Ok(Some(match name.as_str() {
            "sample" | "slow" => (StatusCode::OK, fixtures::sample(today_utc()).to_string()),
            "empty" => (StatusCode::OK, fixtures::empty().to_string()),
            "error" => (StatusCode::SERVICE_UNAVAILABLE, String::new()),
            _ => {
                return Err(crate::api::unknown_fixture(
                    "PLASTINKA_TEST_HISTORY",
                    fixtures::NAMES,
                ));
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TODAY: Ymd = (2026, 11, 12);

    fn parsed(name: &str) -> Vec<RawDay> {
        let body = match name {
            "empty" => fixtures::empty(),
            _ => fixtures::sample(TODAY),
        };
        parse_history(StatusCode::OK, &body.to_string()).unwrap()
    }

    #[test]
    fn sample_is_grouped_by_day_and_context() {
        let days = parsed("sample");
        assert_eq!(days.len(), 3);
        assert_eq!(days.first().and_then(|day| day.date), Some(TODAY));
        let first = days.first().unwrap();
        assert_eq!(first.items.len(), 2);
        assert!(matches!(
            first.items.first().map(|item| &item.context),
            Some(HistoryContext::Album { id, title }) if id == "10101" && title == "Камнем по голове"
        ));
        assert_eq!(
            first.items.get(1).map(|item| &item.context),
            Some(&HistoryContext::Wave)
        );
        // Трек без полной модели остаётся идентификатором.
        let wave = first.items.get(1).unwrap();
        assert!(matches!(wave.tracks.first(), Some(Slot::Id(id)) if id == "7019818"));
        assert!(matches!(wave.tracks.get(1), Some(Slot::Full(_))));
        let second = days.get(1).unwrap();
        assert!(matches!(
            second.items.first().map(|item| &item.context),
            Some(HistoryContext::Playlist { owner, kind, title })
                if owner == "1000" && kind == "3" && title == "Утренний"
        ));
        assert!(matches!(
            second.items.get(1).map(|item| &item.context),
            Some(HistoryContext::Artist { id, name }) if id == "853465" && name == "Tame Impala"
        ));
    }

    #[test]
    fn missing_tracks_are_filled_in_and_unavailable_dropped() {
        let days = parsed("sample");
        let mut found = parse_track(&serde_json::json!({
            "id": "7019818", "title": "Sempre libera", "available": true,
            "artists": [{"id": 5, "name": "Солистка"}], "albums": [{"id": 777}]
        }))
        .unwrap();
        found.title = Some("Sempre libera".to_owned());
        let full = resolve(days, &[found]);
        let wave = full.first().unwrap().items.get(1).unwrap();
        assert_eq!(wave.tracks.len(), 2);
        assert_eq!(
            wave.tracks.first().map(|track| track.id.as_str()),
            Some("7019818")
        );
        // Без догруженного трека остаётся только полная модель.
        let without = resolve(parsed("sample"), &[]);
        let wave = without.first().unwrap().items.get(1).unwrap();
        assert_eq!(wave.tracks.len(), 1);
    }

    #[test]
    fn empty_history_and_garbage() {
        assert!(parsed("empty").is_empty());
        for body in [
            "не json",
            "{}",
            r#"{"result":{}}"#,
            r#"{"result":{"historyTabs":5}}"#,
        ] {
            assert!(matches!(
                parse_history(StatusCode::OK, body),
                Err(ApiError::InvalidResponse)
            ));
        }
        // Мусор внутри: элементы пропускаются, остальное живёт.
        let body = r#"{"result":{"historyTabs":[
            "мусор",
            {"date":"плохая","items":[
                5,
                {"context":{"type":"album","data":{}},"tracks":[]},
                {"context":{"type":"wave"},"tracks":[
                    {"data":{}}, {"data":{"itemId":{"trackId":"42"}}}, 7]}
            ]}
        ]}}"#;
        let days = parse_history(StatusCode::OK, body).unwrap();
        assert_eq!(days.len(), 1);
        assert_eq!(days.first().and_then(|day| day.date), None);
        let item = days.first().and_then(|day| day.items.first()).unwrap();
        assert_eq!(item.context, HistoryContext::Wave);
        assert!(matches!(item.tracks.as_slice(), [Slot::Id(id)] if id == "42"));
    }

    #[test]
    fn errors_are_mapped() {
        assert!(matches!(
            parse_history(StatusCode::SERVICE_UNAVAILABLE, ""),
            Err(ApiError::Server(503))
        ));
        assert!(matches!(
            parse_history(StatusCode::UNAUTHORIZED, ""),
            Err(ApiError::Unauthorized)
        ));
    }

    #[test]
    fn unknown_context_gets_a_title() {
        let value = serde_json::json!({"type": "various", "data": {"fullModel": {"title": "Подборка дня"}}});
        assert_eq!(
            parse_context(&value),
            Some(HistoryContext::Other("Подборка дня".to_owned()))
        );
        // Альбом без идентификатора — не контекст.
        assert_eq!(parse_context(&serde_json::json!({"type": "album"})), None);
        assert_eq!(parse_context(&serde_json::json!({"data": {}})), None);
    }

    #[test]
    fn labels_of_contexts() {
        let album = HistoryContext::Album {
            id: "1".into(),
            title: "Т".into(),
        };
        assert_eq!(album.label(), "Альбом «Т»");
        assert_eq!(HistoryContext::Wave.label(), "Моя волна");
        let playlist = HistoryContext::Playlist {
            owner: "1".into(),
            kind: "2".into(),
            title: "П".into(),
        };
        assert_eq!(playlist.label(), "Плейлист «П»");
        let artist = HistoryContext::Artist {
            id: "1".into(),
            name: "И".into(),
        };
        assert_eq!(artist.label(), "Исполнитель И");
    }

    #[test]
    fn dates_are_parsed_and_labelled() {
        assert_eq!(parse_date("2026-11-12"), Some((2026, 11, 12)));
        assert_eq!(
            parse_date("2026-11-12T10:00:00+03:00"),
            Some((2026, 11, 12))
        );
        assert_eq!(parse_date("2026-13-01"), None);
        assert_eq!(parse_date("вчера"), None);
        assert_eq!(parse_date("2026-11"), None);
        assert_eq!(day_label(Some(TODAY), TODAY), "Сегодня");
        assert_eq!(day_label(Some((2026, 11, 11)), TODAY), "Вчера");
        assert_eq!(day_label(Some((2026, 11, 4)), TODAY), "4 ноября");
        assert_eq!(day_label(Some((2025, 12, 31)), TODAY), "31 декабря 2025");
        assert_eq!(day_label(None, TODAY), "Раньше");
        // Граница месяца и года.
        assert_eq!(day_label(Some((2026, 2, 28)), (2026, 3, 1)), "Вчера");
        assert_eq!(day_label(Some((2025, 12, 31)), (2026, 1, 1)), "Вчера");
        assert_eq!(days_ago((2026, 3, 1), 1), (2026, 2, 28));
        assert_eq!(civil_from_days(days_from_civil(TODAY)), TODAY);
    }

    #[test]
    fn budget_limits_tracks() {
        let mut tracks = Vec::new();
        for n in 0..(MAX_TRACKS + 20) {
            tracks.push(serde_json::json!({"data": {"itemId": {"trackId": n.to_string()}}}));
        }
        let body = serde_json::json!({"result": {"historyTabs": [
            {"date": "2026-11-12", "items": [
                {"context": {"type": "wave"}, "tracks": tracks}]}]}});
        let days = parse_history(StatusCode::OK, &body.to_string()).unwrap();
        let count: usize = days
            .iter()
            .flat_map(|day| &day.items)
            .map(|item| item.tracks.len())
            .sum();
        assert_eq!(count, MAX_TRACKS);
    }
}
