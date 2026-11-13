//! Прослушивания на диске: `<кэш ОС>/history.json`. Только метаданные
//! (id трека и альбома, название, исполнители, адрес обложки, длительность,
//! откуда играли, день и время), аудио сюда не попадает. Нужны, чтобы
//! вкладка «История» после перезапуска показывала то, что играло в
//! Plastinka, даже если сервер (`music-history`) этого ещё не показывает.
//!
//! Храним последние [`MAX_AGE_DAYS`] дней, не больше
//! [`history::LOCAL_LIMIT`] записей. Битый, чужой или слишком большой файл —
//! пустая история, не ошибка. Запись атомарная: временный файл и
//! переименование. С тестовыми хуками (`PLASTINKA_TEST_TOKEN`) файл не
//! читается и не пишется.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::history::{self, LocalPlay};
use crate::api::{ArtistRef, HistoryContext, Track, Warning};
use crate::storage::AppDirs;

const VERSION: u32 = 1;
const FILE_NAME: &str = "history.json";
const DAY: u64 = 24 * 60 * 60;
/// Сколько дней храним.
pub const MAX_AGE_DAYS: u64 = 30;
/// Файл больше — не наш (тысяча записей столько не занимает).
const MAX_FILE: u64 = 4 * 1024 * 1024;

/// Номер последнего запроса на запись и номер записанного: запись из пула
/// потоков может прийти не по порядку, старый снимок новый не затирает.
static NEXT: AtomicU64 = AtomicU64::new(0);
static WRITTEN: Mutex<u64> = Mutex::new(0);

#[derive(Serialize, Deserialize)]
struct File {
    v: u32,
    #[serde(default)]
    plays: Vec<Stored>,
}

/// Запись о прослушивании. Поля короткие: файл перечитывается при каждом
/// запуске.
#[derive(Serialize, Deserialize)]
struct Stored {
    id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    album: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    album_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(default)]
    artists: Vec<StoredArtist>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cover: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ms: Option<u64>,
    #[serde(default)]
    available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    warning: Option<String>,
    ctx: StoredContext,
    /// День (UTC): год, месяц, число.
    date: (i32, u32, u32),
    /// Когда зазвучал, секунды Unix.
    at: u64,
}

#[derive(Serialize, Deserialize)]
struct StoredArtist {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    name: String,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum StoredContext {
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
    Liked,
    Other {
        title: String,
    },
}

impl From<&HistoryContext> for StoredContext {
    fn from(context: &HistoryContext) -> Self {
        match context.clone() {
            HistoryContext::Album { id, title } => Self::Album { id, title },
            HistoryContext::Artist { id, name } => Self::Artist { id, name },
            HistoryContext::Playlist { owner, kind, title } => {
                Self::Playlist { owner, kind, title }
            }
            HistoryContext::Wave => Self::Wave,
            HistoryContext::Liked => Self::Liked,
            HistoryContext::Other(title) => Self::Other { title },
        }
    }
}

impl From<StoredContext> for HistoryContext {
    fn from(context: StoredContext) -> Self {
        match context {
            StoredContext::Album { id, title } => Self::Album { id, title },
            StoredContext::Artist { id, name } => Self::Artist { id, name },
            StoredContext::Playlist { owner, kind, title } => Self::Playlist { owner, kind, title },
            StoredContext::Wave => Self::Wave,
            StoredContext::Liked => Self::Liked,
            StoredContext::Other { title } => Self::Other(title),
        }
    }
}

impl From<&LocalPlay> for Stored {
    fn from(play: &LocalPlay) -> Self {
        let track = &play.track;
        Self {
            id: track.id.clone(),
            album: track.album_id.clone(),
            album_title: track.album_title.clone(),
            title: track.title.clone(),
            version: track.version.clone(),
            artists: track
                .artists
                .iter()
                .map(|artist| StoredArtist {
                    id: artist.id.clone(),
                    name: artist.name.clone(),
                })
                .collect(),
            cover: track.cover_uri.clone(),
            ms: track
                .duration
                .and_then(|duration| u64::try_from(duration.as_millis()).ok()),
            available: track.available,
            warning: match track.warning {
                Warning::None => None,
                Warning::Explicit => Some("explicit".to_owned()),
                Warning::Clean => Some("clean".to_owned()),
            },
            ctx: StoredContext::from(&play.context),
            date: play.date,
            at: play.at,
        }
    }
}

impl Stored {
    /// Обратно в прослушивание. Без id или с невозможной датой — `None`.
    fn into_play(self) -> Option<LocalPlay> {
        let (_, month, day) = self.date;
        if self.id.trim().is_empty() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
            return None;
        }
        let track = Track {
            id: self.id,
            title: self.title,
            version: self.version,
            artists: self
                .artists
                .into_iter()
                .map(|artist| ArtistRef {
                    id: artist.id,
                    name: artist.name,
                })
                .collect(),
            album_id: self.album,
            album_title: self.album_title,
            duration: self.ms.map(Duration::from_millis),
            available: self.available,
            cover_uri: self.cover,
            warning: match self.warning.as_deref() {
                Some("explicit") => Warning::Explicit,
                Some("clean") => Warning::Clean,
                _ => Warning::None,
            },
            loudness: None,
            lyrics: None,
        };
        Some(LocalPlay {
            track,
            context: self.ctx.into(),
            date: self.date,
            at: self.at,
        })
    }
}

/// Секунды Unix сейчас.
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// Файл в каталоге кэша ОС. В тестах, с тестовыми хуками и без каталога —
/// `None`.
pub fn system_path() -> Option<PathBuf> {
    // Unit-тесты не трогают файл пользователя.
    if cfg!(test) || crate::storage::test_hooks_active() {
        return None;
    }
    AppDirs::discover()
        .ok()
        .map(|dirs| dirs.cache_dir().join(FILE_NAME))
}

/// Свежие записи: не старше [`MAX_AGE_DAYS`] и не «из будущего» (больше
/// суток вперёд — часы сбились или файл подделан); не больше
/// [`history::LOCAL_LIMIT`], остаются последние.
fn keep_fresh(mut plays: Vec<LocalPlay>, now: u64) -> Vec<LocalPlay> {
    let oldest = now.saturating_sub(MAX_AGE_DAYS * DAY);
    let newest = now.saturating_add(DAY);
    plays.retain(|play| play.at >= oldest && play.at <= newest);
    let extra = plays.len().saturating_sub(history::LOCAL_LIMIT);
    plays.drain(..extra);
    plays
}

/// Прослушивания из файла, от старых к новым. Нет файла, битый, чужая
/// версия — пусто; непонятная запись пропускается.
pub fn load(path: &Path, now: u64) -> Vec<LocalPlay> {
    let fits = std::fs::metadata(path).is_ok_and(|meta| meta.len() <= MAX_FILE);
    if !fits {
        return Vec::new();
    }
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Vec::new();
    };
    if value.get("v").and_then(serde_json::Value::as_u64) != Some(u64::from(VERSION)) {
        return Vec::new();
    }
    let plays = value
        .get("plays")
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|item| serde_json::from_value::<Stored>(item.clone()).ok())
        .filter_map(Stored::into_play)
        .collect();
    keep_fresh(plays, now)
}

/// Записывает прослушивания через временный файл. Ошибка — текстом, без
/// путей к аудио (их нет) и без токена.
pub fn save(path: &Path, plays: Vec<LocalPlay>, now: u64) -> Result<(), String> {
    let file = File {
        v: VERSION,
        plays: keep_fresh(plays, now).iter().map(Stored::from).collect(),
    };
    let text = serde_json::to_vec(&file).map_err(|err| err.to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|err| err.to_string())?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|err| err.to_string())?;
    std::fs::rename(&tmp, path).map_err(|err| err.to_string())
}

/// Номер для [`save_in_background`]: берётся в момент снимка.
pub fn next_serial() -> u64 {
    NEXT.fetch_add(1, Ordering::Relaxed).wrapping_add(1)
}

/// Сохранение в фоне. Снимок старше уже записанного не пишется. Сбой — одна
/// строка в stderr, показу не мешает.
pub async fn save_in_background(path: PathBuf, plays: Vec<LocalPlay>, serial: u64) {
    let work = move || {
        let mut written = WRITTEN.lock().unwrap_or_else(PoisonError::into_inner);
        if serial <= *written {
            return Ok(());
        }
        save(&path, plays, unix_now())?;
        *written = serial;
        Ok::<(), String>(())
    };
    let result = tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| "запись прервалась".to_owned())
        .and_then(|result| result);
    if let Err(err) = result {
        use std::io::Write as _;
        let _ = writeln!(
            std::io::stderr(),
            "Предупреждение: не удалось сохранить историю прослушиваний: {err}"
        );
    }
}

/// Загрузка в фоне: пусто, если файла нет или пути нет.
pub async fn load_in_background(path: Option<PathBuf>) -> Vec<LocalPlay> {
    let Some(path) = path else {
        return Vec::new();
    };
    tokio::task::spawn_blocking(move || load(&path, unix_now()))
        .await
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("plastinka-history-{name}-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn file(&self) -> PathBuf {
            self.0.join(FILE_NAME)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn play(id: &str, context: HistoryContext, at: u64) -> LocalPlay {
        LocalPlay {
            track: Track {
                id: id.to_owned(),
                title: Some(format!("Трек {id}")),
                version: Some("Remastered".to_owned()),
                artists: vec![ArtistRef {
                    id: Some("7".to_owned()),
                    name: "Исполнитель".to_owned(),
                }],
                album_id: Some("55".to_owned()),
                album_title: Some("Альбом".to_owned()),
                duration: Some(Duration::from_millis(201_500)),
                available: true,
                cover_uri: Some("avatars.example/55/%%".to_owned()),
                warning: Warning::Explicit,
                loudness: None,
                lyrics: None,
            },
            context,
            date: (2027, 1, 15),
            at,
        }
    }

    #[test]
    fn plays_survive_save_and_load() {
        let dir = TempDir::new("roundtrip");
        let plays = vec![
            play(
                "1",
                HistoryContext::Album {
                    id: "55".into(),
                    title: "Альбом".into(),
                },
                NOW - 100,
            ),
            play(
                "2",
                HistoryContext::Playlist {
                    owner: "9".into(),
                    kind: "3".into(),
                    title: "П".into(),
                },
                NOW - 50,
            ),
            play(
                "3",
                HistoryContext::Artist {
                    id: "7".into(),
                    name: "И".into(),
                },
                NOW - 40,
            ),
            play("4", HistoryContext::Wave, NOW - 30),
            play("5", HistoryContext::Liked, NOW - 20),
            play("6", HistoryContext::Other("Поиск".into()), NOW - 10),
        ];
        save(&dir.file(), plays.clone(), NOW).unwrap();
        assert_eq!(load(&dir.file(), NOW), plays);
        // Временного файла не остаётся.
        assert!(!dir.file().with_extension("json.tmp").exists());
        // Повторная запись заменяет файл целиком.
        save(&dir.file(), plays[..2].to_vec(), NOW).unwrap();
        assert_eq!(load(&dir.file(), NOW).len(), 2);
    }

    #[test]
    fn missing_or_broken_file_gives_empty_history() {
        let dir = TempDir::new("broken");
        assert!(load(&dir.file(), NOW).is_empty());
        for body in [
            "не json",
            "",
            "{}",
            r#"{"v":2,"plays":[]}"#,
            r#"{"v":1,"plays":5}"#,
            "[1,2,3]",
        ] {
            std::fs::write(dir.file(), body).unwrap();
            assert!(load(&dir.file(), NOW).is_empty(), "{body}");
        }
        // Битая запись пропускается, остальные живут.
        let good =
            serde_json::to_value(Stored::from(&play("1", HistoryContext::Wave, NOW))).unwrap();
        let body = serde_json::json!({"v": 1, "plays": [
            "мусор",
            {"id": "2"},
            {"id": "", "ctx": {"type": "wave"}, "date": [2027, 1, 15], "at": NOW},
            {"id": "3", "ctx": {"type": "wave"}, "date": [2027, 13, 15], "at": NOW},
            {"id": "4", "ctx": {"type": "чужой"}, "date": [2027, 1, 15], "at": NOW},
            good,
        ]});
        std::fs::write(dir.file(), body.to_string()).unwrap();
        let loaded = load(&dir.file(), NOW);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded.first().map(|play| play.track.id.as_str()), Some("1"));
        // Слишком большой файл не читается.
        std::fs::write(
            dir.file(),
            vec![b' '; usize::try_from(MAX_FILE).unwrap() + 1],
        )
        .unwrap();
        assert!(load(&dir.file(), NOW).is_empty());
    }

    #[test]
    fn old_future_and_extra_plays_are_trimmed() {
        let dir = TempDir::new("trim");
        let plays = vec![
            play("old", HistoryContext::Wave, NOW - MAX_AGE_DAYS * DAY - 1),
            play("edge", HistoryContext::Wave, NOW - MAX_AGE_DAYS * DAY),
            play("fresh", HistoryContext::Wave, NOW - 5),
            play("future", HistoryContext::Wave, NOW + 2 * DAY),
        ];
        save(&dir.file(), plays, NOW).unwrap();
        let ids: Vec<String> = load(&dir.file(), NOW)
            .into_iter()
            .map(|play| play.track.id)
            .collect();
        assert_eq!(ids, ["edge", "fresh"]);
        // Через месяц и «edge» устаревает: обрезка и при чтении.
        let later: Vec<String> = load(&dir.file(), NOW + DAY)
            .into_iter()
            .map(|play| play.track.id)
            .collect();
        assert_eq!(later, ["fresh"]);
        // Лимит записей: остаются последние.
        let many: Vec<LocalPlay> = (0..history::LOCAL_LIMIT + 5)
            .map(|n| play(&n.to_string(), HistoryContext::Wave, NOW))
            .collect();
        save(&dir.file(), many, NOW).unwrap();
        let loaded = load(&dir.file(), NOW);
        assert_eq!(loaded.len(), history::LOCAL_LIMIT);
        assert_eq!(loaded.first().map(|play| play.track.id.as_str()), Some("5"));
    }

    #[tokio::test]
    async fn stale_snapshot_does_not_overwrite_a_newer_one() {
        let dir = TempDir::new("order");
        // Фоновая запись берёт настоящее время.
        let now = unix_now();
        let older = next_serial();
        let newer = next_serial();
        save_in_background(
            dir.file(),
            vec![play("new", HistoryContext::Wave, now)],
            newer,
        )
        .await;
        save_in_background(
            dir.file(),
            vec![play("old", HistoryContext::Wave, now)],
            older,
        )
        .await;
        let ids: Vec<String> = load(&dir.file(), now)
            .into_iter()
            .map(|play| play.track.id)
            .collect();
        assert_eq!(ids, ["new"]);
    }
}
