//! «Моя волна»: сессия rotor, порции треков и обратная связь.
//!
//! Путь сессионный: `POST /rotor/session/new`, затем
//! `POST /rotor/session/{radioSessionId}/tracks` и
//! `POST /rotor/session/{radioSessionId}/feedback`. Разбор мягкий: непонятный
//! элемент `sequence[]` пропускается, недоступные треки отбрасываются.
//! Подробности и источники — `docs/api/stage-8.md`. Звук отсюда не берётся,
//! только метаданные треков.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::StatusCode;
use reqwest::header::AUTHORIZATION;
use serde_json::{Value, json};

use super::tracks::{Track, parse_track};
use super::{API_BASE, ApiError, MUSIC_CLIENT, auth_header, check_status};
use crate::net::NetError;

/// Станция «Моя волна».
const WAVE_SEED: &str = "user:onyourwave";

/// Сколько последних треков отправлять в `queue` при догрузке.
pub const QUEUE_LEN: usize = 5;

/// Цвета трека (`derivedColors`), только корректные `#RRGGBB` или `#RRGGBBAA`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WaveColors {
    pub average: Option<String>,
    pub accent: Option<String>,
}

/// Трек из волны.
#[derive(Debug, Clone, PartialEq)]
pub struct WaveTrack {
    pub track: Track,
    /// Трек уже в «Мне нравится».
    pub liked: bool,
    /// Оттенок трека (`trackParameters.hue`), градусы.
    pub hue: Option<f64>,
    pub colors: Option<WaveColors>,
    /// Порция, в которой пришёл трек: с ней уходят его события.
    pub batch_id: String,
}

impl WaveTrack {
    /// Идентификатор для `queue` и событий: `id:albumId` или просто `id`.
    pub fn queue_id(&self) -> String {
        queue_id(&self.track)
    }
}

/// `id:albumId` трека, без альбома — `id`.
pub fn queue_id(track: &Track) -> String {
    match &track.album_id {
        Some(album) => format!("{}:{album}", track.id),
        None => track.id.clone(),
    }
}

/// Порция треков волны.
#[derive(Debug, Clone, PartialEq)]
pub struct WaveBatch {
    /// `radioSessionId`: безопасная строка, идёт в адрес запросов.
    pub session: String,
    pub batch_id: String,
    /// Значение поля `from` в событиях (`wave.idForFrom`), если пришло.
    pub from: Option<String>,
    pub tracks: Vec<WaveTrack>,
}

/// Ответ на запрос следующей порции.
#[derive(Debug, Clone, PartialEq)]
pub enum WaveTracks {
    Batch(WaveBatch),
    /// Сервер не знает сессию (`unknownSession`): нужна новая.
    Expired,
}

/// Событие обратной связи. `track_id` — `id:albumId`.
#[derive(Debug, Clone, PartialEq)]
pub enum WaveEvent {
    RadioStarted,
    TrackStarted { track_id: String },
    TrackFinished { track_id: String, played: Duration },
    Skipped { track_id: String, played: Duration },
}

/// Запускает «Мою волну» и возвращает первую порцию.
pub async fn start_wave(http: &reqwest::Client, token: &str) -> Result<WaveBatch, ApiError> {
    let body = json!({
        "seeds": [WAVE_SEED],
        "queue": [],
        "includeTracksInResponse": true,
        "includeWaveModel": true,
        "interactive": true,
    });
    let (status, text) = send(http, token, "rotor/session/new", &body, Call::New).await?;
    match parse_batch(status, &text, None)? {
        WaveTracks::Batch(batch) => Ok(batch),
        WaveTracks::Expired => Err(ApiError::InvalidResponse),
    }
}

/// Следующая порция. `queue` — `id:albumId` последних треков сессии, текущий
/// включительно; отправляются не больше [`QUEUE_LEN`] последних.
pub async fn fetch_wave_tracks(
    http: &reqwest::Client,
    token: &str,
    session: &str,
    queue: &[String],
) -> Result<WaveTracks, ApiError> {
    if !safe_session(session) {
        return Err(ApiError::InvalidResponse);
    }
    let skip = queue.len().saturating_sub(QUEUE_LEN);
    let queue: Vec<&String> = queue
        .iter()
        .skip(skip)
        .filter(|id| safe_queue_id(id))
        .collect();
    let body = json!({ "queue": queue, "feedbacks": [] });
    let path = format!("rotor/session/{session}/tracks");
    let (status, text) = send(http, token, &path, &body, Call::Tracks).await?;
    parse_batch(status, &text, Some(session))
}

/// Отправляет событие. Ошибку окно не показывает: воспроизведение от
/// обратной связи не зависит.
pub async fn send_wave_feedback(
    http: &reqwest::Client,
    token: &str,
    session: &str,
    batch_id: &str,
    from: Option<&str>,
    event: &WaveEvent,
) -> Result<(), ApiError> {
    if !safe_session(session) {
        return Err(ApiError::InvalidResponse);
    }
    let body = feedback_body(batch_id, from, event, SystemTime::now());
    let path = format!("rotor/session/{session}/feedback");
    let (status, _) = send(http, token, &path, &body, Call::Feedback).await?;
    check_status(status)
}

/// Что за запрос уходит: нужно только отладочному хуку.
enum Call {
    New,
    Tracks,
    Feedback,
}

#[cfg_attr(not(debug_assertions), allow(unused_variables))]
async fn send(
    http: &reqwest::Client,
    token: &str,
    path: &str,
    body: &Value,
    call: Call,
) -> Result<(StatusCode, String), ApiError> {
    #[cfg(debug_assertions)]
    if let Some(reply) = test_hook::reply(token, &call, body).await? {
        return Ok(reply);
    }
    let response = http
        .post(format!("{API_BASE}/{path}"))
        .header(AUTHORIZATION, auth_header(token)?)
        .header("X-Yandex-Music-Client", MUSIC_CLIENT)
        .json(body)
        .send()
        .await
        .map_err(NetError::from)?;
    let status = response.status();
    let text = response.text().await.map_err(NetError::from)?;
    Ok((status, text))
}

/// Тело события. `batchId` — рядом с `event`, `from` — внутри него,
/// `totalPlayedSeconds` — только у `trackFinished` и `skip`.
fn feedback_body(batch_id: &str, from: Option<&str>, event: &WaveEvent, now: SystemTime) -> Value {
    let (kind, track_id, played) = match event {
        WaveEvent::RadioStarted => ("radioStarted", None, None),
        WaveEvent::TrackStarted { track_id } => ("trackStarted", Some(track_id), None),
        WaveEvent::TrackFinished { track_id, played } => {
            ("trackFinished", Some(track_id), Some(played))
        }
        WaveEvent::Skipped { track_id, played } => ("skip", Some(track_id), Some(played)),
    };
    let mut inner = json!({ "type": kind, "timestamp": rfc3339_utc(now) });
    if let Some(object) = inner.as_object_mut() {
        if let Some(from) = from {
            object.insert("from".to_owned(), json!(from));
        }
        if let Some(track_id) = track_id {
            object.insert("trackId".to_owned(), json!(track_id));
        }
        if let Some(played) = played {
            object.insert("totalPlayedSeconds".to_owned(), json!(played.as_secs_f64()));
        }
    }
    json!({ "event": inner, "batchId": batch_id })
}

/// Время в формате RFC 3339, UTC, с миллисекундами: `2026-11-06T12:00:00.123Z`.
/// Своя маленькая функция: `chrono` и `time` в проект не подключены.
fn rfc3339_utc(time: SystemTime) -> String {
    let since = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let (secs, millis) = (since.as_secs(), since.subsec_millis());
    let (year, month, day) = civil_from_days(secs / 86_400);
    let rest = secs % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        rest / 3600,
        rest / 60 % 60,
        rest % 60
    )
}

/// Дата по числу дней с 1970-01-01 (григорианский календарь).
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let day_of_era = z % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

/// `radioSessionId` идёт в адрес: буквы, цифры, `-`, `_`, `.`, до 128 знаков.
fn safe_session(session: &str) -> bool {
    (1..=128).contains(&session.len())
        && session
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// `id` или `id:albumId`, цифры и UUID.
fn safe_queue_id(id: &str) -> bool {
    (1..=80).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() || matches!(b, b':' | b'-'))
}

/// Короткий текст из видимых символов ASCII (для `batchId` и `idForFrom`).
fn plain_text(value: Option<&Value>) -> Option<String> {
    let text = value?.as_str()?;
    ((1..=128).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_graphic()))
        .then(|| text.to_owned())
}

/// `#RRGGBB` или `#RRGGBBAA`.
fn hex_color(value: Option<&Value>) -> Option<String> {
    let text = value?.as_str()?;
    let digits = text.strip_prefix('#')?;
    (matches!(digits.len(), 6 | 8) && digits.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| text.to_owned())
}

fn parse_colors(value: Option<&Value>) -> Option<WaveColors> {
    let object = value?;
    let colors = WaveColors {
        average: hex_color(object.get("average")),
        accent: hex_color(object.get("accent")),
    };
    (colors != WaveColors::default()).then_some(colors)
}

/// Разбор ответа `session/new` и `…/tracks`. `fallback_session` — сессия
/// запроса: в ответе на догрузку `radioSessionId` может не прийти.
fn parse_batch(
    status: StatusCode,
    body: &str,
    fallback_session: Option<&str>,
) -> Result<WaveTracks, ApiError> {
    check_status(status)?;
    let envelope: Value = serde_json::from_str(body).map_err(|_| ApiError::InvalidResponse)?;
    let result = envelope.get("result").ok_or(ApiError::InvalidResponse)?;
    if result.get("unknownSession").and_then(Value::as_bool) == Some(true) {
        return Ok(WaveTracks::Expired);
    }
    let session = match result.get("radioSessionId") {
        Some(value) => value
            .as_str()
            .filter(|session| safe_session(session))
            .map(str::to_owned),
        None => fallback_session.map(str::to_owned),
    }
    .ok_or(ApiError::InvalidResponse)?;
    let batch_id = plain_text(result.get("batchId")).ok_or(ApiError::InvalidResponse)?;
    let sequence = result
        .get("sequence")
        .and_then(Value::as_array)
        .ok_or(ApiError::InvalidResponse)?;
    let tracks = sequence
        .iter()
        .filter_map(|item| parse_item(item, &batch_id))
        .collect();
    Ok(WaveTracks::Batch(WaveBatch {
        session,
        batch_id,
        from: plain_text(result.get("wave").and_then(|wave| wave.get("idForFrom"))),
        tracks,
    }))
}

/// Элемент `sequence[]`. Не трек, непонятный или недоступный — `None`.
fn parse_item(item: &Value, batch_id: &str) -> Option<WaveTrack> {
    if item
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind != "track")
    {
        return None;
    }
    let track = parse_track(item.get("track")?)?;
    if !track.available {
        return None;
    }
    Some(WaveTrack {
        liked: item.get("liked").and_then(Value::as_bool) == Some(true),
        hue: item
            .get("trackParameters")
            .and_then(|parameters| parameters.get("hue"))
            .and_then(Value::as_f64)
            .filter(|hue| hue.is_finite()),
        colors: parse_colors(item.get("track").and_then(|t| t.get("derivedColors"))),
        batch_id: batch_id.to_owned(),
        track,
    })
}

/// Готовые ответы сервиса для отладочного хука и тестов.
#[cfg(any(test, debug_assertions))]
mod fixtures {
    use reqwest::StatusCode;
    use serde_json::{Value, json};

    pub const NAMES: &[&str] = &[
        "sample",
        "endless",
        "empty",
        "expired",
        "new-503",
        "new-401",
        "tracks-503",
        "feedback-503",
        "colors",
        "slow",
    ];

    pub const SESSION: &str = "test-session-1";
    pub const FROM: &str = "mobile-radio-user-onyourwave";

    /// Какой запрос подменяем; `n` у `Tracks` — сколько догрузок уже было.
    pub enum Call {
        New,
        Tracks(u32),
        Feedback,
    }

    pub fn reply(name: &str, call: &Call) -> Option<(StatusCode, String)> {
        if !NAMES.contains(&name) {
            return None;
        }
        let colors = name == "colors";
        let ok = |value: Value| (StatusCode::OK, value.to_string());
        Some(match call {
            Call::New => match name {
                "new-503" => (StatusCode::SERVICE_UNAVAILABLE, String::new()),
                "new-401" => (StatusCode::UNAUTHORIZED, r#"{"status":401}"#.to_owned()),
                "empty" => ok(envelope(0, true, colors)),
                _ => ok(envelope(0, false, colors)),
            },
            Call::Tracks(n) => match name {
                "tracks-503" => (StatusCode::SERVICE_UNAVAILABLE, String::new()),
                "expired" if *n == 0 => ok(json!({ "result": { "unknownSession": true } })),
                "empty" => ok(envelope(u64::from(*n) + 1, true, colors)),
                "endless" | "expired" => ok(envelope(u64::from(*n) + 1, false, colors)),
                // Две догрузки, потом волна «иссякает».
                _ => ok(envelope(u64::from(*n) + 1, *n >= 2, colors)),
            },
            Call::Feedback => match name {
                "feedback-503" => (StatusCode::SERVICE_UNAVAILABLE, String::new()),
                _ => ok(json!({ "result": "ok" })),
            },
        })
    }

    fn envelope(batch: u64, empty: bool, colors: bool) -> Value {
        let sequence: Vec<Value> = if empty {
            Vec::new()
        } else {
            items(batch, colors)
        };
        json!({ "result": {
            "radioSessionId": SESSION,
            "batchId": format!("test-batch-{batch}"),
            "sequence": sequence,
            "wave": { "name": "Моя волна", "description": "Подборка", "idForFrom": FROM,
                      "seeds": ["user:onyourwave"], "stationId": "user:onyourwave" },
            "pumpkin": false,
            "terminated": false,
        }})
    }

    const ARTISTS: [(u64, &str); 4] = [
        (41052, "Король и Шут"),
        (853_465, "Tame Impala"),
        (1001, "Оркестр"),
        (5, "Солистка"),
    ];

    /// Акцент трека из набора: у разных треков цвета разные.
    pub fn accent_color(id: u64) -> &'static str {
        const COLORS: [&str; 4] = ["#3A7BD5", "#D5533A", "#3AD56B", "#B03AD5"];
        usize::try_from(id % 7 % 4)
            .ok()
            .and_then(|index| COLORS.get(index))
            .copied()
            .unwrap_or("#3A7BD5")
    }

    fn artist(index: u64) -> (u64, &'static str) {
        usize::try_from(index)
            .ok()
            .and_then(|index| ARTISTS.get(index))
            .copied()
            .unwrap_or((41052, "Король и Шут"))
    }

    fn item(id: u64, title: &str, artist: (u64, &str), album: u64, with_colors: bool) -> Value {
        let mut track = json!({
            "id": id.to_string(),
            "realId": id.to_string(),
            "title": title,
            // Недоступен трек «2» и каждый седьмой из сгенерированных.
            "available": id != 2 && !((700_010..800_000).contains(&id) && id.is_multiple_of(7)),
            "durationMs": (id % 300 + 60) * 1000,
            "coverUri": format!("avatars.example/{album}/%%"),
            "artists": [{ "id": artist.0, "name": artist.1 }],
            "albums": [{ "id": album, "title": format!("Альбом {album}") }],
        });
        if with_colors
            && id.is_multiple_of(2)
            && let Some(object) = track.as_object_mut()
        {
            object.insert(
                "derivedColors".to_owned(),
                json!({ "average": "#2A3F5F", "waveText": "#FFFFFF",
                        "miniPlayer": "#101820", "accent": accent_color(id) }),
            );
        }
        json!({
            "type": "track",
            "liked": id.is_multiple_of(3),
            "trackParameters": { "bpm": 120, "energy": 0.5, "hue": (id * 97 % 360) as f64,
                                 "userCollectionHue": 0 },
            "track": track,
        })
    }

    /// Первая порция — треки из `PLASTINKA_TEST_LIKES`/`CATALOG` sample, чтобы
    /// сработали плагины. Следующие — сгенерированные.
    fn items(batch: u64, colors: bool) -> Vec<Value> {
        if batch == 0 {
            return vec![
                item(29_238_706, "Дурак и молния", artist(0), 10_101, colors),
                item(7_019_818, "Sempre libera", artist(3), 777, colors),
                item(2, "Снят с каталога", artist(2), 777, colors),
                item(700_001, "Волновой трек", artist(1), 3, colors),
                item(700_002, "Волновой трек", artist(0), 3, colors),
                json!({ "type": "ad", "track": { "id": "1" } }),
                json!("мусор"),
            ];
        }
        (0..5)
            .map(|n| {
                let id = 700_000 + batch * 10 + n;
                let artist = artist(id % 4);
                item(id, &format!("Волновой трек {id}"), artist, 3, colors)
            })
            .collect()
    }
}

/// Отладочный хук `PLASTINKA_TEST_ROTOR`: вместо запросов к сервису готовые
/// ответы. Условия — у `api::test_hook`. События обратной связи копятся в
/// памяти (для тестов), никуда не пишутся и не выводятся.
#[cfg(debug_assertions)]
mod test_hook {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};

    use reqwest::StatusCode;

    use super::{Call, fixtures};
    use crate::api::ApiError;

    const VAR: &str = "PLASTINKA_TEST_ROTOR";

    static TRACKS_CALLS: AtomicU32 = AtomicU32::new(0);
    static EVENTS: Mutex<Vec<String>> = Mutex::new(Vec::new());

    pub async fn reply(
        token: &str,
        call: &Call,
        body: &serde_json::Value,
    ) -> Result<Option<(StatusCode, String)>, ApiError> {
        let Some(name) = crate::api::test_hook(VAR, token)? else {
            return Ok(None);
        };
        if name == "slow" {
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        }
        let fixture_call = match call {
            Call::New => fixtures::Call::New,
            Call::Tracks => fixtures::Call::Tracks(TRACKS_CALLS.fetch_add(1, Ordering::Relaxed)),
            Call::Feedback => {
                if let Ok(mut events) = EVENTS.lock() {
                    events.push(body.to_string());
                }
                fixtures::Call::Feedback
            }
        };
        fixtures::reply(&name, &fixture_call)
            .map(Some)
            .ok_or_else(|| crate::api::unknown_fixture(VAR, fixtures::NAMES))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(name: &str, call: &fixtures::Call) -> Result<WaveTracks, ApiError> {
        let (status, body) = fixtures::reply(name, call).unwrap();
        parse_batch(status, &body, Some(fixtures::SESSION))
    }

    fn batch(name: &str, call: &fixtures::Call) -> WaveBatch {
        match parse(name, call).unwrap() {
            WaveTracks::Batch(batch) => batch,
            WaveTracks::Expired => panic!("ожидалась порция"),
        }
    }

    #[test]
    fn nested_track_liked_and_unavailable_are_handled() {
        let wave = batch("sample", &fixtures::Call::New);
        assert_eq!(wave.session, fixtures::SESSION);
        assert_eq!(wave.batch_id, "test-batch-0");
        assert_eq!(wave.from.as_deref(), Some(fixtures::FROM));
        // Недоступный «2», реклама и мусор отброшены.
        let ids: Vec<&str> = wave.tracks.iter().map(|t| t.track.id.as_str()).collect();
        assert_eq!(ids, ["29238706", "7019818", "700001", "700002"]);
        let first = wave.tracks.first().unwrap();
        assert_eq!(first.track.title.as_deref(), Some("Дурак и молния"));
        assert_eq!(first.track.album_id.as_deref(), Some("10101"));
        assert_eq!(first.queue_id(), "29238706:10101");
        assert_eq!(first.hue, Some((29_238_706_u64 * 97 % 360) as f64));
        assert!(wave.tracks.iter().all(|t| t.batch_id == "test-batch-0"));
        // liked у id, кратных 3: из четырёх это 700002.
        assert_eq!(
            wave.tracks.iter().map(|t| t.liked).collect::<Vec<_>>(),
            [false, false, false, true]
        );
        assert_eq!(first.colors, None);
    }

    #[test]
    fn colors_are_read_and_validated() {
        let wave = batch("colors", &fixtures::Call::New);
        let colored: Vec<_> = wave.tracks.iter().filter(|t| t.colors.is_some()).collect();
        assert!(!colored.is_empty() && colored.len() < wave.tracks.len());
        let colors = colored.first().unwrap().colors.clone().unwrap();
        let first_id = colored.first().unwrap().track.id.parse::<u64>().unwrap();
        assert_eq!(
            colors.accent.as_deref(),
            Some(fixtures::accent_color(first_id))
        );
        assert_eq!(colors.average.as_deref(), Some("#2A3F5F"));
        for bad in ["red", "#12345", "#GGGGGG", "rgba(1,2,3,1)"] {
            let value = json!(bad);
            assert_eq!(hex_color(Some(&value)), None, "{bad}");
        }
    }

    #[test]
    fn broken_responses_are_errors() {
        let ok = StatusCode::OK;
        for body in [
            "",
            "<html>",
            r#"{"result":{}}"#,
            // Нет radioSessionId (и запасной не задан).
            r#"{"result":{"batchId":"b","sequence":[]}}"#,
            // Небезопасный radioSessionId.
            r#"{"result":{"radioSessionId":"../x?y","batchId":"b","sequence":[]}}"#,
            r#"{"result":{"radioSessionId":"a b","batchId":"b","sequence":[]}}"#,
            // Нет batchId и sequence.
            r#"{"result":{"radioSessionId":"s","sequence":[]}}"#,
            r#"{"result":{"radioSessionId":"s","batchId":"b"}}"#,
        ] {
            assert!(
                matches!(parse_batch(ok, body, None), Err(ApiError::InvalidResponse)),
                "{body}"
            );
        }
    }

    #[test]
    fn session_falls_back_to_request_session() {
        let body = r#"{"result":{"batchId":"b","sequence":[]}}"#;
        let WaveTracks::Batch(wave) = parse_batch(StatusCode::OK, body, Some("sess-1")).unwrap()
        else {
            panic!("ожидалась порция");
        };
        assert_eq!(wave.session, "sess-1");
        assert!(wave.tracks.is_empty());
    }

    #[test]
    fn unknown_session_means_expired() {
        assert_eq!(
            parse("expired", &fixtures::Call::Tracks(0)).unwrap(),
            WaveTracks::Expired
        );
        assert!(matches!(
            parse("expired", &fixtures::Call::Tracks(1)).unwrap(),
            WaveTracks::Batch(_)
        ));
    }

    #[test]
    fn errors_are_mapped() {
        for (name, expected) in [("new-503", "Server(503)"), ("new-401", "Unauthorized")] {
            let error = parse(name, &fixtures::Call::New).unwrap_err();
            assert_eq!(format!("{error:?}"), expected);
        }
        let error = parse("tracks-503", &fixtures::Call::Tracks(0)).unwrap_err();
        assert_eq!(format!("{error:?}"), "Server(503)");
        assert!(matches!(
            parse_batch(StatusCode::FORBIDDEN, "", None),
            Err(ApiError::Unauthorized)
        ));
        assert!(matches!(
            parse_batch(StatusCode::BAD_REQUEST, "", None),
            Err(ApiError::UnexpectedStatus(400))
        ));
    }

    #[test]
    fn feedback_body_has_exact_keys() {
        let now = UNIX_EPOCH + Duration::from_millis(1_700_000_000_123);
        let started = feedback_body(
            "b1",
            Some("from-x"),
            &WaveEvent::TrackStarted {
                track_id: "1:2".to_owned(),
            },
            now,
        );
        assert_eq!(
            started,
            json!({"batchId":"b1","event":{"type":"trackStarted",
                "timestamp":"2023-11-14T22:13:20.123Z","from":"from-x","trackId":"1:2"}})
        );
        let skip = feedback_body(
            "b1",
            None,
            &WaveEvent::Skipped {
                track_id: "1:2".to_owned(),
                played: Duration::from_millis(12_500),
            },
            now,
        );
        assert_eq!(
            skip,
            json!({"batchId":"b1","event":{"type":"skip",
                "timestamp":"2023-11-14T22:13:20.123Z","trackId":"1:2",
                "totalPlayedSeconds":12.5}})
        );
        let finished = feedback_body(
            "b1",
            None,
            &WaveEvent::TrackFinished {
                track_id: "1".to_owned(),
                played: Duration::from_secs(100),
            },
            now,
        );
        assert_eq!(finished["event"]["type"], "trackFinished");
        assert_eq!(finished["event"]["totalPlayedSeconds"], 100.0);
        let radio = feedback_body("b1", Some("f"), &WaveEvent::RadioStarted, now);
        assert_eq!(radio["event"]["type"], "radioStarted");
        assert!(radio["event"].get("trackId").is_none());
        assert!(radio["event"].get("totalPlayedSeconds").is_none());
        assert!(
            started["event"].get("totalPlayedSeconds").is_none(),
            "у trackStarted секунд нет"
        );
    }

    #[test]
    fn timestamp_format() {
        let at = |secs: u64| rfc3339_utc(UNIX_EPOCH + Duration::from_secs(secs));
        assert_eq!(at(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(at(951_782_400), "2000-02-29T00:00:00.000Z");
        assert_eq!(at(1_709_251_200 - 1), "2024-02-29T23:59:59.000Z");
        assert_eq!(at(1_709_251_200), "2024-03-01T00:00:00.000Z");
        assert_eq!(at(1_700_000_000), "2023-11-14T22:13:20.000Z");
        assert_eq!(at(4_102_444_800), "2100-01-01T00:00:00.000Z");
    }

    #[test]
    fn unsafe_ids_are_rejected() {
        assert!(safe_session("s_1-a.B"));
        for bad in ["", "a/b", "a?b", "a b", "../x", &"a".repeat(129)] {
            assert!(!safe_session(bad), "{bad}");
        }
        assert!(safe_queue_id("29238706:10101"));
        assert!(safe_queue_id("0f6c8a5e-1b2c-4d3e-9f00-123456789abc"));
        assert!(!safe_queue_id("1/../2"));
        assert!(!safe_queue_id(""));
    }

    #[test]
    fn hook_fixtures_behave() {
        assert!(fixtures::reply("нет такого", &fixtures::Call::New).is_none());
        for name in fixtures::NAMES {
            assert!(
                fixtures::reply(name, &fixtures::Call::New).is_some(),
                "{name}"
            );
        }
        // empty: пустой старт.
        assert!(batch("empty", &fixtures::Call::New).tracks.is_empty());
        // sample иссякает после двух догрузок, endless — нет.
        assert!(
            !batch("sample", &fixtures::Call::Tracks(1))
                .tracks
                .is_empty()
        );
        assert!(
            batch("sample", &fixtures::Call::Tracks(2))
                .tracks
                .is_empty()
        );
        assert!(
            !batch("endless", &fixtures::Call::Tracks(50))
                .tracks
                .is_empty()
        );
        // Догруженные треки не повторяют первую порцию.
        let first = batch("endless", &fixtures::Call::New);
        let next = batch("endless", &fixtures::Call::Tracks(0));
        assert!(
            next.tracks
                .iter()
                .all(|t| first.tracks.iter().all(|f| f.track.id != t.track.id))
        );
        assert_eq!(
            fixtures::reply("feedback-503", &fixtures::Call::Feedback)
                .unwrap()
                .0,
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            fixtures::reply("sample", &fixtures::Call::Feedback)
                .unwrap()
                .0,
            StatusCode::OK
        );
    }
}
