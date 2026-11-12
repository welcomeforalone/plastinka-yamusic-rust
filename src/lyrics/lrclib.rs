//! Клиент LRCLib (`lrclib.net`): запасной источник текстов. Запросы без
//! авторизации; User-Agent — общий клиент приложения (`Plastinka/<версия>`),
//! без личных данных. Лимит сервиса около 90 запросов в минуту: на 429 не
//! повторяем, показываем «текст недоступен». Спека — раздел 2
//! `docs/api/stage-9.md`.

use std::time::Duration;

use reqwest::StatusCode;
use serde_json::Value;

use super::{LyricsError, Outcome, Request, lrc};
use crate::net::{self, NetError};

const BASE: &str = "https://lrclib.net/api";
/// Предел ответа: `search` отдаёт до 20 записей с полными текстами.
const MAX_BYTES: usize = 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(15);
/// Допуск по длительности: так же сверяет и сам сервис.
const DURATION_TOLERANCE: f64 = 2.0;
/// Длительность в запросе: сервис принимает от 1 до 3600 секунд.
const DURATION_RANGE: std::ops::RangeInclusive<u64> = 1..=3600;

/// Одна запись ответа.
#[derive(Debug, Clone, PartialEq)]
struct Record {
    duration: Option<f64>,
    instrumental: bool,
    plain: Option<String>,
    synced: Option<String>,
}

/// Ищет текст: точное совпадение `/get`, при 404 — `/search`.
pub async fn fetch(http: &reqwest::Client, request: &Request) -> Result<Outcome, LyricsError> {
    // LRCLib требует название и исполнителя: без них спрашивать нечего.
    if request.title.trim().is_empty() || request.artist.trim().is_empty() {
        return Ok(Outcome::Missing);
    }
    let mut query = vec![
        ("track_name", request.title.clone()),
        ("artist_name", request.artist.clone()),
    ];
    if let Some(album) = request.album.as_deref().filter(|album| !album.is_empty()) {
        query.push(("album_name", album.to_owned()));
    }
    if let Some(seconds) = request
        .duration
        .map(|duration| duration.as_secs_f64().round() as u64)
        && DURATION_RANGE.contains(&seconds)
    {
        query.push(("duration", seconds.to_string()));
    }
    let (status, body) = send(http, "get", &query).await?;
    match status {
        StatusCode::NOT_FOUND => {}
        status if status.is_success() => {
            let value: Value =
                serde_json::from_str(&body).map_err(|_| LyricsError::bad_answer())?;
            return Ok(outcome(parse_record(&value)));
        }
        status => return Err(status_error(status)),
    }

    // Точного совпадения нет: ищем по названию и исполнителю (альбом
    // сужает поиск слишком сильно).
    query.retain(|(key, _)| matches!(*key, "track_name" | "artist_name"));
    let (status, body) = send(http, "search", &query).await?;
    if !status.is_success() {
        return Err(status_error(status));
    }
    let value: Value = serde_json::from_str(&body).map_err(|_| LyricsError::bad_answer())?;
    let records: Vec<Record> = value
        .as_array()
        .map(|items| items.iter().filter_map(parse_record).collect())
        .unwrap_or_default();
    Ok(outcome(pick(&records, request.duration).cloned()))
}

async fn send(
    http: &reqwest::Client,
    path: &str,
    query: &[(&str, String)],
) -> Result<(StatusCode, String), LyricsError> {
    let url = reqwest::Url::parse_with_params(
        &format!("{BASE}/{path}"),
        query.iter().map(|(key, value)| (*key, value.as_str())),
    )
    .map_err(|_| LyricsError::bad_answer())?;
    let response = http
        .get(url)
        .timeout(TIMEOUT)
        .send()
        .await
        .map_err(|err| LyricsError::from(NetError::from(err)))?;
    let status = response.status();
    let body = net::read_limited(response, MAX_BYTES).await?;
    Ok((status, String::from_utf8_lossy(&body).into_owned()))
}

fn status_error(status: StatusCode) -> LyricsError {
    if status == StatusCode::TOO_MANY_REQUESTS {
        LyricsError::Unavailable(
            "Текст недоступен: сервис LRCLib просит подождать. Повторите позже".to_owned(),
        )
    } else if status.is_server_error() {
        LyricsError::Unavailable("Сервис текстов LRCLib временно недоступен".to_owned())
    } else {
        LyricsError::Unavailable(format!(
            "Сервис текстов LRCLib ответил неожиданно (код {})",
            status.as_u16()
        ))
    }
}

/// Запись из ответа. Нет ни одного известного поля — не запись.
fn parse_record(value: &Value) -> Option<Record> {
    let object = value.as_object()?;
    let text = |key: &str| {
        object
            .get(key)
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .map(str::to_owned)
    };
    Some(Record {
        duration: object.get("duration").and_then(Value::as_f64),
        instrumental: object.get("instrumental").and_then(Value::as_bool) == Some(true),
        plain: text("plainLyrics"),
        synced: text("syncedLyrics"),
    })
}

/// Запись из `search`: длительность в пределах допуска, текст с метками
/// предпочтительнее, затем ближайшая по длительности. Без нашей длительности
/// сверять нечем: берётся первая подходящая с текстом.
fn pick(records: &[Record], duration: Option<Duration>) -> Option<&Record> {
    let ours = duration.map(|duration| duration.as_secs_f64());
    let distance = |record: &Record| match (ours, record.duration) {
        (Some(ours), Some(theirs)) => Some((ours - theirs).abs()),
        _ => None,
    };
    records
        .iter()
        .filter(|record| record.instrumental || record.plain.is_some() || record.synced.is_some())
        .filter(|record| match (ours, distance(record)) {
            (Some(_), Some(delta)) => delta <= DURATION_TOLERANCE,
            // Сверить не с чем: длительности нет у нас или у записи.
            (Some(_), None) => false,
            (None, _) => true,
        })
        .min_by(|a, b| {
            let rank = |record: &&Record| {
                (
                    record.synced.is_none(),
                    distance(record).map_or(0, |delta| (delta * 1000.0) as u64),
                )
            };
            rank(a).cmp(&rank(b))
        })
}

fn outcome(record: Option<Record>) -> Outcome {
    let Some(record) = record else {
        return Outcome::Missing;
    };
    if record.instrumental {
        return Outcome::Instrumental;
    }
    let synced = record
        .synced
        .as_deref()
        .map(lrc::parse)
        .filter(|lines| !lines.is_empty());
    if synced.is_none() && record.plain.is_none() {
        return Outcome::Missing;
    }
    Outcome::Found(super::Lyrics {
        source: super::Source::LrcLib,
        synced,
        plain: record.plain,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(duration: f64, synced: bool, plain: bool) -> Record {
        Record {
            duration: Some(duration),
            instrumental: false,
            plain: plain.then(|| "текст".to_owned()),
            synced: synced.then(|| "[00:01.00] текст".to_owned()),
        }
    }

    #[test]
    fn record_is_parsed() {
        let value: Value = serde_json::from_str(
            r#"{"id":1,"trackName":"a","artistName":"b","albumName":"c","duration":233.5,
                "instrumental":false,"plainLyrics":"x\ny","syncedLyrics":"[00:01.00] x"}"#,
        )
        .unwrap();
        let record = parse_record(&value).unwrap();
        assert_eq!(record.duration, Some(233.5));
        assert_eq!(record.plain.as_deref(), Some("x\ny"));
        assert!(record.synced.is_some());
        assert!(parse_record(&Value::Null).is_none());
    }

    #[test]
    fn outcome_kinds() {
        assert_eq!(outcome(None), Outcome::Missing);
        let instrumental = Record {
            instrumental: true,
            ..record(100.0, false, false)
        };
        assert_eq!(outcome(Some(instrumental)), Outcome::Instrumental);
        // Оба текста null и не инструментал: текста нет.
        assert_eq!(outcome(Some(record(100.0, false, false))), Outcome::Missing);
        let Outcome::Found(found) = outcome(Some(record(100.0, true, true))) else {
            return;
        };
        assert_eq!(found.synced.map(|lines| lines.len()), Some(1));
        assert_eq!(found.plain.as_deref(), Some("текст"));
    }

    #[test]
    fn synced_without_marks_falls_back_to_plain() {
        let broken = Record {
            synced: Some("просто текст".to_owned()),
            ..record(100.0, false, true)
        };
        let Outcome::Found(found) = outcome(Some(broken)) else {
            return;
        };
        assert!(found.synced.is_none());
        assert!(found.plain.is_some());
    }

    #[test]
    fn pick_respects_tolerance_and_prefers_synced() {
        let records = vec![
            record(200.0, true, true),
            record(231.0, false, true),
            record(233.5, false, true),
            record(234.0, true, true),
        ];
        let ours = Some(Duration::from_secs(233));
        // 200 и 231 вне допуска; из 233,5 и 234 с метками — вторая.
        let picked = pick(&records, ours).unwrap();
        assert_eq!(picked.duration, Some(234.0));
        // Никто не подходит.
        assert!(pick(&records, Some(Duration::from_secs(10))).is_none());
    }

    #[test]
    fn pick_prefers_closest_among_equal_kind() {
        let records = vec![record(234.0, true, false), record(233.2, true, false)];
        let picked = pick(&records, Some(Duration::from_secs(233))).unwrap();
        assert_eq!(picked.duration, Some(233.2));
    }

    #[test]
    fn pick_without_our_duration_takes_text() {
        let empty = record(100.0, false, false);
        let records = vec![empty, record(150.0, false, true)];
        assert_eq!(pick(&records, None).unwrap().duration, Some(150.0));
    }

    #[test]
    fn rate_limit_has_clear_text() {
        let LyricsError::Unavailable(text) = status_error(StatusCode::TOO_MANY_REQUESTS) else {
            return;
        };
        assert!(text.contains("Текст недоступен"));
    }
}
