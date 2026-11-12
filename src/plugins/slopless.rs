//! Slopless: метки ИИ-исполнителей и ИИ-треков по публичному API
//! slopless.art. На исполнителя — один запрос `GET /api/artist/{id}`: в
//! ответе доля ИИ-треков и их список. Токен Яндекса туда не уходит.
//! Подробности — `docs/api/stage-7.5.md`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{may_write, read_limited, stale, unix_now};
use crate::api::Track;
use crate::net::NetError;
use crate::storage::StorageError;

pub const API_BASE: &str = "https://slopless.art/api";
/// Страница проекта: ссылка в настройках.
pub const SITE_URL: &str = "https://slopless.art";
/// Больше не читаем: у самых больших ИИ-исполнителей ответ около 1 МБ.
const RESPONSE_LIMIT: usize = 8 * 1024 * 1024;
/// Столько номеров ИИ-треков исполнителя храним.
const MAX_AI_TRACKS: usize = 20_000;
/// Сколько живёт ответ в кэше.
const FRESH: Duration = Duration::from_secs(7 * 24 * 3600);
/// Не спрашивать того же исполнителя после ошибки сети раньше.
const RETRY_AFTER: Duration = Duration::from_secs(60);
/// Запросов одновременно.
pub const PARALLEL: usize = 4;
/// Порог из описания API: трек считается ИИ при оценке от 0,3.
pub const TRACK_THRESHOLD: f64 = 0.3;
const CACHE_FILE: &str = "slopless.json";

/// Что известно об исполнителе.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verdict {
    /// Не меньше половины треков — ИИ.
    pub ai: bool,
    /// Номера ИИ-треков исполнителя.
    pub tracks: HashSet<String>,
    /// Исполнителя нет в базе Slopless.
    #[serde(default)]
    pub unknown: bool,
    pub at: u64,
}

/// Ошибка запроса. Тексты — для окна.
#[derive(Debug, Clone, thiserror::Error)]
pub enum SlopError {
    #[error("{0}")]
    Net(String),
    #[error("slopless.art ответил кодом {0}")]
    Status(u16),
    #[error("slopless.art прислал непонятный ответ")]
    Broken,
}

impl From<NetError> for SlopError {
    fn from(err: NetError) -> Self {
        Self::Net(err.to_string())
    }
}

/// Число из ответа: схема разрешает и число, и строку.
fn number(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

fn id(value: Option<&Value>) -> Option<String> {
    let id = match value? {
        Value::Number(number) => number.as_u64()?.to_string(),
        Value::String(text) => text.trim().to_owned(),
        _ => return None,
    };
    crate::api::plausible_id(&id).then_some(id)
}

/// Разбирает ответ `GET /api/artist/{id}`.
pub fn parse_artist(body: &[u8]) -> Result<Verdict, SlopError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| SlopError::Broken)?;
    let ai_count = number(value.get("aiTracks")).ok_or(SlopError::Broken)?;
    let total = number(value.get("totalTracks")).ok_or(SlopError::Broken)?;
    let listed = |key: &str, scored: bool| -> Vec<String> {
        value
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|item| {
                !scored || number(item.get("score")).is_none_or(|score| score >= TRACK_THRESHOLD)
            })
            .filter_map(|item| id(item.get("id")))
            .collect()
    };
    let tracks: HashSet<String> = listed("aiTrackList", true)
        .into_iter()
        .chain(listed("yandexAiTrackList", false))
        .take(MAX_AI_TRACKS)
        .collect();
    Ok(Verdict {
        ai: ai_count > 0.0 && ai_count * 2.0 >= total,
        tracks,
        unknown: false,
        at: unix_now(),
    })
}

/// Спрашивает о исполнителе `artist`. Нет в базе (404) — «не знаем».
pub async fn fetch_artist(http: reqwest::Client, artist: String) -> Result<Verdict, SlopError> {
    #[cfg(debug_assertions)]
    if let Some(name) = super::test_hook() {
        return test_verdict(&name, &artist);
    }
    if !crate::api::plausible_id(&artist) {
        return Ok(unknown());
    }
    let response = http
        .get(format!("{API_BASE}/artist/{artist}"))
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(NetError::from)?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(unknown());
    }
    if !status.is_success() {
        return Err(SlopError::Status(status.as_u16()));
    }
    let body = read_limited(response, RESPONSE_LIMIT)
        .await?
        .ok_or(SlopError::Broken)?;
    parse_artist(&body)
}

fn unknown() -> Verdict {
    Verdict {
        ai: false,
        tracks: HashSet::new(),
        unknown: true,
        at: unix_now(),
    }
}

/// Ответы в кэше: исполнитель → ответ.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Cache {
    #[serde(default)]
    pub artists: HashMap<String, Verdict>,
}

fn cache_path() -> Option<PathBuf> {
    super::cache_dir().map(|dir| dir.join(CACHE_FILE))
}

pub async fn load_cache() -> Cache {
    #[cfg(debug_assertions)]
    if super::test_hook().is_some() {
        return Cache::default();
    }
    let Some(path) = cache_path() else {
        return Cache::default();
    };
    crate::storage::blocking(move || {
        let text = std::fs::read_to_string(&path).map_err(|_| StorageError::Interrupted)?;
        let mut cache: Cache =
            serde_json::from_str(&text).map_err(|_| StorageError::Interrupted)?;
        cache
            .artists
            .retain(|id, verdict| crate::api::plausible_id(id) && !stale(verdict.at, FRESH));
        Ok(cache)
    })
    .await
    .unwrap_or_default()
}

pub async fn save_cache(cache: Cache) {
    if !may_write() {
        return;
    }
    let Some(path) = cache_path() else {
        return;
    };
    let _ = crate::storage::blocking(move || {
        let text = serde_json::to_string(&cache).map_err(|_| StorageError::Interrupted)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|_| StorageError::Interrupted)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, text).map_err(|_| StorageError::Interrupted)?;
        std::fs::rename(&tmp, &path).map_err(|_| StorageError::Interrupted)
    })
    .await;
}

/// Метка трека или исполнителя.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Label {
    /// Ещё не знаем.
    Pending,
    Human,
    Ai,
}

/// Slopless в окне: ответы, запросы в полёте, очередь.
#[derive(Debug, Default)]
pub struct Slop {
    pub cache: Cache,
    in_flight: HashSet<String>,
    queue: VecDeque<String>,
    failed: HashMap<String, Instant>,
    /// Кэш изменился, его надо записать.
    pub dirty: bool,
    /// Треки, которым уже ставили «Не нравится» в этом сеансе.
    pub disliked: HashSet<String>,
}

impl Slop {
    /// Исполнитель — ИИ.
    pub fn artist(&self, id: &str) -> Label {
        match self.cache.artists.get(id) {
            Some(verdict) if verdict.ai => Label::Ai,
            Some(_) => Label::Human,
            None => Label::Pending,
        }
    }

    /// Трек — ИИ: он в списке ИИ-треков исполнителя или исполнитель ИИ.
    /// Пока хоть об одном исполнителе ничего не знаем — `Pending`.
    pub fn track(&self, track: &Track) -> Label {
        let mut pending = false;
        for artist in &track.artists {
            let Some(id) = &artist.id else { continue };
            match self.cache.artists.get(id) {
                Some(verdict) if verdict.ai || verdict.tracks.contains(&track.id) => {
                    return Label::Ai;
                }
                Some(_) => {}
                None => pending = true,
            }
        }
        if pending {
            Label::Pending
        } else {
            Label::Human
        }
    }

    /// Поставить исполнителей в очередь, если о них ещё не спрашивали.
    /// Новые — вперёд: они на экране сейчас.
    pub fn want<'a>(&mut self, artists: impl IntoIterator<Item = &'a str>, now: Instant) {
        for id in artists {
            let known = self.cache.artists.contains_key(id)
                || self.in_flight.contains(id)
                || self.queue.iter().any(|queued| queued == id)
                || self
                    .failed
                    .get(id)
                    .is_some_and(|at| now.saturating_duration_since(*at) < RETRY_AFTER);
            if !known && crate::api::plausible_id(id) {
                self.queue.push_front(id.to_owned());
            }
        }
        // Очередь не растёт без конца, если долго листать.
        self.queue.truncate(200);
    }

    /// Следующие запросы, не больше [`PARALLEL`] сразу.
    pub fn next_batch(&mut self) -> Vec<String> {
        let mut batch = Vec::new();
        while self.in_flight.len() < PARALLEL {
            let Some(id) = self.queue.pop_front() else {
                break;
            };
            self.in_flight.insert(id.clone());
            batch.push(id);
        }
        batch
    }

    /// Пришёл ответ о исполнителе.
    pub fn finished(&mut self, artist: String, result: Result<Verdict, SlopError>, now: Instant) {
        self.in_flight.remove(&artist);
        match result {
            Ok(verdict) => {
                self.failed.remove(&artist);
                self.cache.artists.insert(artist, verdict);
                self.dirty = true;
            }
            Err(_) => {
                self.failed.insert(artist, now);
            }
        }
    }

    /// Идут ли запросы.
    pub fn busy(&self) -> bool {
        !self.in_flight.is_empty() || !self.queue.is_empty()
    }
}

#[cfg(debug_assertions)]
fn test_verdict(name: &str, artist: &str) -> Result<Verdict, SlopError> {
    match name {
        "offline" => return Err(SlopError::Net(NetError::Offline.to_string())),
        "broken" => return Err(SlopError::Broken),
        _ => {}
    }
    // В примерах: «Оркестр» (1001) — ИИ-исполнитель, у «Tame Impala»
    // (853465) один ИИ-трек, «Солистка» (5) — тоже ИИ.
    let body = match artist {
        "1001" | "5" => r#"{"aiTracks":9,"totalTracks":10,"aiTrackList":[]}"#,
        "853465" => {
            r#"{"aiTracks":"1","totalTracks":"40","aiTrackList":[{"id":500001,"title":"x","score":0.9}]}"#
        }
        "41052" => r#"{"aiTracks":0,"totalTracks":300,"aiTrackList":[]}"#,
        _ => return Ok(unknown()),
    };
    parse_artist(body.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{ArtistRef, Warning};

    fn track(id: &str, artists: &[&str]) -> Track {
        Track {
            id: id.to_owned(),
            title: None,
            version: None,
            artists: artists
                .iter()
                .map(|id| ArtistRef {
                    id: Some((*id).to_owned()),
                    name: "x".to_owned(),
                })
                .collect(),
            album_id: None,
            album_title: None,
            duration: None,
            available: true,
            cover_uri: None,
            warning: Warning::None,
            loudness: None,
            lyrics: None,
        }
    }

    #[test]
    fn artist_answer_is_parsed() {
        let verdict = parse_artist(
            br#"{"id":24092397,"name":"VANTOR","aiTracks":4176,"totalTracks":6914,
                 "aiTrackList":[{"id":149592201,"score":0.99},{"id":"7","score":"0.1"},{"id":-1}],
                 "yandexAiTracks":0,"yandexAiTrackList":[{"id":8,"status":1}]}"#,
        )
        .unwrap();
        assert!(verdict.ai);
        let mut tracks: Vec<_> = verdict.tracks.into_iter().collect();
        tracks.sort();
        assert_eq!(tracks, ["149592201", "8"]);
        let human = parse_artist(br#"{"aiTracks":"0","totalTracks":"0"}"#).unwrap();
        assert!(!human.ai);
        assert!(matches!(parse_artist(b"{}"), Err(SlopError::Broken)));
        assert!(matches!(parse_artist(b"<html>"), Err(SlopError::Broken)));
    }

    #[test]
    fn labels_and_queue() {
        let now = Instant::now();
        let mut slop = Slop::default();
        let song = track("10", &["1", "2"]);
        assert_eq!(slop.track(&song), Label::Pending);
        slop.want(["1", "2", "1", "bad"], now);
        let batch = slop.next_batch();
        assert_eq!(batch.len(), 2);
        assert!(slop.next_batch().is_empty(), "оба уже в полёте");
        slop.finished(
            "1".to_owned(),
            Ok(parse_artist(br#"{"aiTracks":0,"totalTracks":5}"#).unwrap()),
            now,
        );
        assert_eq!(slop.track(&song), Label::Pending, "о втором ещё не знаем");
        slop.finished(
            "2".to_owned(),
            Ok(parse_artist(
                br#"{"aiTracks":1,"totalTracks":50,"aiTrackList":[{"id":10,"score":0.5}]}"#,
            )
            .unwrap()),
            now,
        );
        assert_eq!(slop.track(&song), Label::Ai, "трек в списке ИИ-треков");
        assert_eq!(slop.artist("2"), Label::Human);
        // После ошибки сети не спрашиваем сразу снова.
        slop.want(["3"], now);
        let _ = slop.next_batch();
        slop.finished("3".to_owned(), Err(SlopError::Broken), now);
        slop.want(["3"], now);
        assert!(slop.next_batch().is_empty());
        slop.want(["3"], now + RETRY_AFTER + Duration::from_secs(1));
        assert_eq!(slop.next_batch(), ["3"]);
        // Трек без исполнителей с номерами — не ИИ.
        assert_eq!(slop.track(&track("11", &[])), Label::Human);
    }
}
