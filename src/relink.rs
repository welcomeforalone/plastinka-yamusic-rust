//! Копия недоступного трека: тот же трек в другом релизе (сборник,
//! переиздание, сингл), найденный поиском по Яндексу.
//!
//! Сервис помечает часть треков `available: false` (права, регион аккаунта).
//! Для такого трека окно ищет доступный трек с тем же исполнителем,
//! названием, версией и длительностью (±3 с) и играет его вместо
//! оригинала. Это не плагин: работает всегда. Лайк, история, Discord и
//! системный плеер остаются за оригиналом.
//!
//! На диск пишутся только номера треков и время поиска (метаданные,
//! правило 1). Формат и крайние случаи — `docs/api/relink.md`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::api::{self, Found, SearchKind, Track};
use crate::plugins::fckcensor::{normalize, same_artist, write_atomic};
use crate::plugins::{may_write, unix_now};
use crate::storage::{AppDirs, StorageError};

/// Сколько помнить найденную копию.
pub const FOUND_FRESH: Duration = Duration::from_secs(30 * 24 * 3600);
/// Сколько помнить, что копии нет.
pub const MISSING_FRESH: Duration = Duration::from_secs(7 * 24 * 3600);
/// После сетевой ошибки или ответа 5xx не искать этот трек раньше.
pub const RETRY_AFTER: Duration = Duration::from_secs(60);
/// Сколько запросов идёт одновременно.
pub const PARALLEL: usize = 2;
/// Сколько ждать ответ поиска.
pub const SEARCH_TIMEOUT: Duration = Duration::from_secs(10);
/// Разница длительностей, при которой трек ещё считается тем же.
const DURATION_SLACK: Duration = Duration::from_secs(3);
/// Файл кэша в каталоге кэша приложения.
const CACHE_FILE: &str = "relink.json";

// ------------------------------------------------------------ тестовый хук

/// Отладочный хук `PLASTINKA_TEST_UNAVAILABLE=<id>,<id>`: эти треки считаются
/// `available: false`. В релизе его нет.
#[cfg(debug_assertions)]
fn hook_ids() -> &'static [String] {
    static IDS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    IDS.get_or_init(|| {
        std::env::var("PLASTINKA_TEST_UNAVAILABLE")
            .map(|value| {
                value
                    .split(',')
                    .map(|id| id.trim().to_owned())
                    .filter(|id| !id.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    })
}

/// Хук объявил трек недоступным.
#[cfg(debug_assertions)]
pub fn forced_unavailable(id: &str) -> bool {
    hook_ids().iter().any(|hooked| hooked == id)
}

#[cfg(not(debug_assertions))]
pub fn forced_unavailable(_id: &str) -> bool {
    false
}

/// Хук задан: кэш копий не читаем и не пишем, чтобы проверка не оставила следов.
#[cfg(debug_assertions)]
pub fn hook_active() -> bool {
    !hook_ids().is_empty()
}

#[cfg(not(debug_assertions))]
pub fn hook_active() -> bool {
    false
}

// ------------------------------------------------------------ подбор

/// Версия для сравнения: нет версии — пустая строка.
fn version_key(track: &Track) -> String {
    track.version.as_deref().map(normalize).unwrap_or_default()
}

/// Копия трека `original` среди `candidates`: доступный трек с другим
/// номером, тот же исполнитель, название и версия равны после `normalize`,
/// длительности известны у обоих и отличаются не больше чем на 3 с. Из
/// подходящих — сначала с той же пометкой `contentWarning`, затем с
/// ближайшей длительностью.
pub fn pick_copy<'a>(original: &Track, candidates: &'a [Track]) -> Option<&'a Track> {
    let title = normalize(original.title.as_deref()?);
    if title.is_empty() {
        return None;
    }
    let version = version_key(original);
    let length = original.duration?;
    candidates
        .iter()
        .filter(|candidate| {
            candidate.available
                && candidate.id != original.id
                && same_artist(&original.artists, &candidate.artists)
                && candidate
                    .title
                    .as_deref()
                    .is_some_and(|other| normalize(other) == title)
                && version_key(candidate) == version
        })
        .filter_map(|candidate| {
            let gap = length.abs_diff(candidate.duration?);
            (gap <= DURATION_SLACK).then_some((
                candidate.warning != original.warning,
                gap,
                candidate,
            ))
        })
        .min_by_key(|(other_warning, gap, _)| (*other_warning, *gap))
        .map(|(_, _, candidate)| candidate)
}

/// Итог одного поиска.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    /// Копия найдена.
    Found(Box<Track>),
    /// Поиск удался, копии нет.
    Missing,
    /// Сеть, 4xx/5xx, тайм-аут, битый ответ: не кэшируем.
    Failed,
}

/// Ищет копию: один запрос «первый исполнитель + название», страница 0.
pub async fn find_copy(http: &reqwest::Client, token: &str, original: &Track) -> Lookup {
    let Some(title) = original.title.as_deref() else {
        return Lookup::Missing;
    };
    let artist = original
        .artists
        .first()
        .map(|artist| artist.name.as_str())
        .unwrap_or_default();
    let query = format!("{artist} {title}");
    let search = api::search(http, token, &query, SearchKind::Tracks, 0);
    match tokio::time::timeout(SEARCH_TIMEOUT, search).await {
        Ok(Ok(result)) => {
            let Found::Tracks(page) = result.found else {
                return Lookup::Missing;
            };
            pick_copy(original, &page.items).map_or(Lookup::Missing, |track| {
                Lookup::Found(Box::new(track.clone()))
            })
        }
        _ => Lookup::Failed,
    }
}

// ------------------------------------------------------------ кэш

/// Копия на диске: только номера.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyRef {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album_id: Option<String>,
}

/// Запись кэша: копия или «нет копии», и когда искали (секунды Unix).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    #[serde(default)]
    pub copy: Option<CopyRef>,
    pub at: u64,
}

impl Entry {
    /// Запись ещё свежая на момент `now` (секунды Unix).
    pub fn fresh(&self, now: u64) -> bool {
        let max = if self.copy.is_some() {
            FOUND_FRESH
        } else {
            MISSING_FRESH
        };
        now.saturating_sub(self.at) <= max.as_secs()
    }
}

/// Кэш копий: номер оригинала → запись.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cache {
    #[serde(default)]
    pub tracks: HashMap<String, Entry>,
}

impl Cache {
    /// Свежая запись о треке: `Some(None)` — копии нет.
    pub fn get(&self, id: &str, now: u64) -> Option<Option<&CopyRef>> {
        let entry = self.tracks.get(id).filter(|entry| entry.fresh(now))?;
        Some(entry.copy.as_ref())
    }

    fn insert(&mut self, id: String, copy: Option<CopyRef>, now: u64) {
        self.tracks.insert(id, Entry { copy, at: now });
    }
}

/// Разбор файла кэша: битый файл — пустой кэш, неправдоподобные номера и
/// устаревшие записи отбрасываются.
pub fn parse_cache(text: &str, now: u64) -> Cache {
    let Ok(mut cache) = serde_json::from_str::<Cache>(text) else {
        return Cache::default();
    };
    cache.tracks.retain(|id, entry| {
        api::plausible_id(id)
            && entry.copy.as_ref().is_none_or(|copy| {
                api::plausible_id(&copy.id)
                    && copy.id != *id
                    && copy.album_id.as_deref().is_none_or(api::plausible_id)
            })
            && entry.fresh(now)
    });
    cache
}

fn cache_path() -> Option<PathBuf> {
    AppDirs::discover()
        .ok()
        .map(|dirs| dirs.cache_dir().join(CACHE_FILE))
}

/// Кэш с диска. Нет файла, хук или ошибка чтения — пустой.
pub async fn load() -> Cache {
    if hook_active() {
        return Cache::default();
    }
    let Some(path) = cache_path() else {
        return Cache::default();
    };
    crate::storage::blocking(move || {
        let text = std::fs::read_to_string(&path).map_err(|_| StorageError::Interrupted)?;
        Ok(parse_cache(&text, unix_now()))
    })
    .await
    .unwrap_or_default()
}

/// Запись кэша в фоне. Ошибки молча: это только ускорение.
pub async fn save(cache: Cache) {
    if !may_write() || hook_active() {
        return;
    }
    let Some(path) = cache_path() else {
        return;
    };
    let _ = crate::storage::blocking(move || {
        let text = serde_json::to_string(&cache).map_err(|_| StorageError::Interrupted)?;
        write_atomic(&path, &text)
    })
    .await;
}

// ------------------------------------------------------------ состояние

/// Что окно знает о копиях: кэш, найденные треки целиком (до перезапуска),
/// очередь поиска.
#[derive(Debug, Default)]
pub struct Relinks {
    cache: Cache,
    /// Найденные в этом сеансе треки-копии: громкость, альбом, обложка.
    tracks: HashMap<String, Track>,
    /// Поиск этих оригиналов уже идёт.
    pending: HashSet<String>,
    /// Не искать этот трек раньше (после ошибки).
    retry: HashMap<String, Instant>,
    /// Кэш с диска прочитан: до этого не ищем и не пишем.
    pub ready: bool,
    /// Кэш изменился: пора записать.
    pub dirty: bool,
    /// Запись на диск идёт.
    pub saving: bool,
}

impl Relinks {
    /// Кэш с диска пришёл: свежие записи не затирают найденное в этом сеансе.
    pub fn merge_disk(&mut self, disk: Cache) {
        for (id, entry) in disk.tracks {
            self.cache.tracks.entry(id).or_insert(entry);
        }
        self.ready = true;
    }

    pub fn cache(&self) -> &Cache {
        &self.cache
    }

    /// Найденная копия трека `id`.
    pub fn copy_of(&self, id: &str) -> Option<&CopyRef> {
        self.cache
            .tracks
            .get(id)
            .and_then(|entry| entry.copy.as_ref())
    }

    /// Трек-копия целиком, если он найден в этом сеансе.
    pub fn track(&self, copy_id: &str) -> Option<&Track> {
        self.tracks.get(copy_id)
    }

    /// Какие из `wanted` искать сейчас: недоступные, без свежей записи, не в
    /// работе и не в паузе после ошибки; всего в работе не больше
    /// [`PARALLEL`]. Они сразу отмечаются «в работе».
    pub fn next_batch<'a>(
        &mut self,
        wanted: impl IntoIterator<Item = &'a Track>,
        now: Instant,
        secs: u64,
    ) -> Vec<Track> {
        let mut batch = Vec::new();
        if !self.ready {
            return batch;
        }
        for track in wanted {
            if self.pending.len() >= PARALLEL {
                break;
            }
            let searchable = !track.available && track.title.is_some() && !track.artists.is_empty();
            if !searchable
                || self.pending.contains(&track.id)
                || self.cache.get(&track.id, secs).is_some()
                || self.retry.get(&track.id).is_some_and(|at| *at > now)
            {
                continue;
            }
            self.retry.remove(&track.id);
            self.pending.insert(track.id.clone());
            batch.push(track.clone());
        }
        batch
    }

    /// Поиск оригинала `id` закончился.
    pub fn finished(&mut self, id: &str, lookup: Lookup, now: Instant, secs: u64) {
        self.pending.remove(id);
        match lookup {
            Lookup::Found(track) => {
                let copy = CopyRef {
                    id: track.id.clone(),
                    album_id: track.album_id.clone(),
                };
                self.cache.insert(id.to_owned(), Some(copy), secs);
                self.tracks.insert(track.id.clone(), *track);
                self.dirty = true;
            }
            Lookup::Missing => {
                self.cache.insert(id.to_owned(), None, secs);
                self.dirty = true;
            }
            Lookup::Failed => {
                self.retry.insert(id.to_owned(), now + RETRY_AFTER);
            }
        }
    }

    /// Идёт ли хоть один поиск.
    pub fn busy(&self) -> bool {
        !self.pending.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{ArtistRef, Warning};

    fn track(id: &str, title: &str, artist: &str, seconds: u64) -> Track {
        Track {
            id: id.to_owned(),
            title: Some(title.to_owned()),
            version: None,
            artists: vec![ArtistRef {
                id: Some("7".to_owned()),
                name: artist.to_owned(),
            }],
            album_id: Some("100".to_owned()),
            album_title: Some("Сборник".to_owned()),
            duration: Some(Duration::from_secs(seconds)),
            available: true,
            cover_uri: None,
            warning: Warning::None,
            loudness: None,
            lyrics: None,
        }
    }

    fn gone(id: &str) -> Track {
        Track {
            available: false,
            ..track(id, "Песня", "Группа", 200)
        }
    }

    #[test]
    fn picks_the_same_track_from_another_release() {
        let original = gone("1");
        let list = [track("2", "песня", "Группа", 201)];
        assert_eq!(
            pick_copy(&original, &list).map(|t| t.id.as_str()),
            Some("2")
        );
    }

    #[test]
    fn rejects_other_artist_title_version_and_length() {
        let original = gone("1");
        let mut other_artist = track("2", "Песня", "Другая группа", 200);
        other_artist.artists[0].id = Some("8".to_owned());
        let prefix = track("3", "Песня (live)", "Группа", 200);
        let mut version = track("4", "Песня", "Группа", 200);
        version.version = Some("Live".to_owned());
        let far = track("5", "Песня", "Группа", 204);
        let unknown = Track {
            duration: None,
            ..track("6", "Песня", "Группа", 200)
        };
        let list = [other_artist, prefix, version, far, unknown];
        assert!(pick_copy(&original, &list).is_none());
        // Ровно 3 секунды — ещё подходит.
        assert!(pick_copy(&original, &[track("7", "Песня", "Группа", 203)]).is_some());
    }

    #[test]
    fn needs_known_length_title_and_available_candidate() {
        let original = gone("1");
        let mut unavailable = track("2", "Песня", "Группа", 200);
        unavailable.available = false;
        assert!(pick_copy(&original, &[unavailable]).is_none());
        // Тот же номер — это сам оригинал.
        let same = track("1", "Песня", "Группа", 200);
        assert!(pick_copy(&original, &[same]).is_none());
        let no_length = Track {
            duration: None,
            ..gone("1")
        };
        assert!(pick_copy(&no_length, &[track("2", "Песня", "Группа", 200)]).is_none());
        let no_title = Track {
            title: None,
            ..gone("1")
        };
        assert!(pick_copy(&no_title, &[track("2", "Песня", "Группа", 200)]).is_none());
    }

    #[test]
    fn version_must_match_and_missing_version_is_empty() {
        let mut original = gone("1");
        original.version = Some("Remastered".to_owned());
        let plain = track("2", "Песня", "Группа", 200);
        let mut remastered = track("3", "Песня", "Группа", 200);
        remastered.version = Some("remastered".to_owned());
        assert_eq!(
            pick_copy(&original, &[plain, remastered]).map(|t| t.id.as_str()),
            Some("3")
        );
    }

    #[test]
    fn prefers_same_warning_then_closest_length() {
        let mut original = gone("1");
        original.warning = Warning::Explicit;
        let near = track("2", "Песня", "Группа", 200);
        let mut explicit_far = track("3", "Песня", "Группа", 202);
        explicit_far.warning = Warning::Explicit;
        let mut explicit_near = track("4", "Песня", "Группа", 201);
        explicit_near.warning = Warning::Explicit;
        assert_eq!(
            pick_copy(&original, &[near.clone(), explicit_far.clone()]).map(|t| t.id.as_str()),
            Some("3")
        );
        assert_eq!(
            pick_copy(&original, &[near, explicit_far, explicit_near]).map(|t| t.id.as_str()),
            Some("4")
        );
    }

    #[test]
    fn cache_freshness_is_30_and_7_days() {
        let day = 24 * 3600;
        let found = Entry {
            copy: Some(CopyRef {
                id: "2".to_owned(),
                album_id: None,
            }),
            at: 1_000_000,
        };
        let missing = Entry {
            copy: None,
            at: 1_000_000,
        };
        assert!(found.fresh(1_000_000 + 30 * day));
        assert!(!found.fresh(1_000_000 + 30 * day + 1));
        assert!(missing.fresh(1_000_000 + 7 * day));
        assert!(!missing.fresh(1_000_000 + 7 * day + 1));
    }

    #[test]
    fn broken_cache_file_is_empty_and_junk_is_dropped() {
        assert_eq!(parse_cache("не json", 10), Cache::default());
        assert_eq!(parse_cache("[1,2]", 10), Cache::default());
        let text = r#"{"tracks":{
            "1":{"copy":{"id":"2","album_id":"3"},"at":10},
            "x":{"copy":null,"at":10},
            "4":{"copy":{"id":"../5"},"at":10},
            "6":{"copy":{"id":"7","album_id":"a"},"at":10},
            "8":{"copy":{"id":"8"},"at":10},
            "9":{"copy":null,"at":1},
            "10":{"copy":null,"at":10}
        }}"#;
        let day = 24 * 3600;
        let cache = parse_cache(text, 10 + 8 * day);
        // «9» и «10» — устаревшие «нет копии» (старше 7 дней), «1» ещё свежая.
        assert_eq!(cache.tracks.keys().collect::<Vec<_>>(), ["1"]);
        let cache = parse_cache(text, 10 + day);
        let mut ids: Vec<&str> = cache.tracks.keys().map(String::as_str).collect();
        ids.sort_unstable();
        // «9» записана секундой раньше, но через сутки ещё свежая.
        assert_eq!(ids, ["1", "10", "9"]);
    }

    #[test]
    fn batch_respects_parallel_limit_and_cache() {
        let mut relinks = Relinks::default();
        relinks.merge_disk(Cache::default());
        let now = Instant::now();
        let tracks = [gone("1"), gone("2"), gone("3"), track("4", "Я", "Х", 100)];
        let first = relinks.next_batch(&tracks, now, 100);
        assert_eq!(first.len(), PARALLEL);
        // Те же треки повторно не просим.
        assert!(relinks.next_batch(&tracks, now, 100).is_empty());
        relinks.finished("1", Lookup::Missing, now, 100);
        let next = relinks.next_batch(&tracks, now, 100);
        assert_eq!(
            next.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            ["3"]
        );
        // «1» теперь в кэше: не просим.
        assert!(relinks.cache().get("1", 100).is_some());
    }

    #[test]
    fn failure_pauses_the_track_for_a_minute() {
        let mut relinks = Relinks::default();
        relinks.merge_disk(Cache::default());
        let now = Instant::now();
        let tracks = [gone("1")];
        assert_eq!(relinks.next_batch(&tracks, now, 100).len(), 1);
        relinks.finished("1", Lookup::Failed, now, 100);
        assert!(!relinks.dirty);
        assert!(relinks.cache().get("1", 100).is_none());
        assert!(relinks.next_batch(&tracks, now, 100).is_empty());
        let later = now + RETRY_AFTER + Duration::from_secs(1);
        assert_eq!(relinks.next_batch(&tracks, later, 100).len(), 1);
    }

    #[test]
    fn found_copy_is_remembered_with_its_track() {
        let mut relinks = Relinks::default();
        relinks.merge_disk(Cache::default());
        let now = Instant::now();
        relinks.finished(
            "1",
            Lookup::Found(Box::new(track("2", "Песня", "Группа", 200))),
            now,
            100,
        );
        assert!(relinks.dirty);
        assert_eq!(
            relinks.copy_of("1"),
            Some(&CopyRef {
                id: "2".to_owned(),
                album_id: Some("100".to_owned()),
            })
        );
        assert_eq!(
            relinks.track("2").and_then(|t| t.album_title.as_deref()),
            Some("Сборник")
        );
        assert!(relinks.copy_of("9").is_none());
    }

    #[test]
    fn nothing_is_searched_before_the_disk_cache_is_read() {
        let mut relinks = Relinks::default();
        assert!(
            relinks
                .next_batch(&[gone("1")], Instant::now(), 100)
                .is_empty()
        );
    }
}
