//! Кэш текстов на диске: `<кэш ОС>/lyrics/<источник>-<id трека>.json`. Только
//! текст (метаданные), аудио сюда не попадает. Положительный результат живёт
//! долго, «текста нет» — неделю (текст могут добавить). Битый, слишком большой
//! или устаревший файл — промах, не ошибка; сбой записи показу не мешает.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{Line, Lyrics, Outcome, Source};
use crate::storage::AppDirs;

const VERSION: u32 = 1;
const DAY: u64 = 24 * 60 * 60;
/// Сколько живёт найденный текст.
const FOUND_TTL: u64 = 365 * DAY;
/// Сколько живёт «инструментал».
const INSTRUMENTAL_TTL: u64 = 30 * DAY;
/// Сколько живёт «текста нет».
const MISSING_TTL: u64 = 7 * DAY;
/// Файл больше — не наш (текст песни столько не занимает).
const MAX_FILE: u64 = 1024 * 1024;

/// Каталог кэша текстов; `None` — каталог ОС не определён (кэша нет).
#[derive(Debug, Clone, Default)]
pub struct Cache {
    dir: Option<PathBuf>,
    /// Каталог создан тестом и удаляется вместе с кэшем.
    #[cfg(test)]
    temp: bool,
}

#[derive(Serialize, Deserialize)]
struct File {
    v: u32,
    /// Когда записано, секунды Unix.
    saved: u64,
    /// `found`, `missing` или `instrumental`.
    kind: String,
    #[serde(default)]
    synced: Vec<(u64, String)>,
    #[serde(default)]
    plain: Option<String>,
}

impl Cache {
    /// Кэш в каталоге кэша ОС. С тестовыми хуками кэша нет: ничего не
    /// читаем и не пишем в каталог настоящего пользователя.
    pub fn system() -> Self {
        if crate::storage::test_hooks_active() {
            return Self::default();
        }
        Self {
            dir: AppDirs::discover()
                .ok()
                .map(|dirs| dirs.cache_dir().join("lyrics")),
            #[cfg(test)]
            temp: false,
        }
    }

    /// Кэш во временном каталоге теста: каталог удаляется вместе с кэшем.
    #[cfg(test)]
    pub fn in_dir(dir: PathBuf) -> Self {
        Self {
            dir: Some(dir),
            temp: true,
        }
    }

    /// Что известно про трек от источника. `now` — секунды Unix.
    pub fn get(&self, source: Source, track_id: &str, now: u64) -> Option<Outcome> {
        let path = self.path(source, track_id)?;
        let meta = std::fs::metadata(&path).ok()?;
        if meta.len() > MAX_FILE {
            return None;
        }
        let file: File = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
        if file.v != VERSION {
            return None;
        }
        let ttl = match file.kind.as_str() {
            "found" => FOUND_TTL,
            "instrumental" => INSTRUMENTAL_TTL,
            "missing" => MISSING_TTL,
            _ => return None,
        };
        // Время «из будущего» — часы сбились или файл подделан: промах.
        if file.saved > now.saturating_add(DAY) || now.saturating_sub(file.saved) > ttl {
            return None;
        }
        match file.kind.as_str() {
            "missing" => Some(Outcome::Missing),
            "instrumental" => Some(Outcome::Instrumental),
            _ => {
                let synced: Vec<Line> = file
                    .synced
                    .into_iter()
                    .map(|(time_ms, text)| Line { time_ms, text })
                    .collect();
                let synced = (!synced.is_empty()).then_some(synced);
                let plain = file.plain.filter(|plain| !plain.trim().is_empty());
                if synced.is_none() && plain.is_none() {
                    return None;
                }
                Some(Outcome::Found(Lyrics {
                    source,
                    synced,
                    plain,
                }))
            }
        }
    }

    /// Запоминает результат. Любой сбой молча игнорируется.
    pub fn put(&self, source: Source, track_id: &str, outcome: &Outcome, now: u64) {
        let Some(path) = self.path(source, track_id) else {
            return;
        };
        let mut file = File {
            v: VERSION,
            saved: now,
            kind: String::new(),
            synced: Vec::new(),
            plain: None,
        };
        match outcome {
            Outcome::Missing => "missing".clone_into(&mut file.kind),
            Outcome::Instrumental => "instrumental".clone_into(&mut file.kind),
            Outcome::Found(lyrics) => {
                "found".clone_into(&mut file.kind);
                file.synced = lyrics
                    .synced
                    .iter()
                    .flatten()
                    .map(|line| (line.time_ms, line.text.clone()))
                    .collect();
                file.plain.clone_from(&lyrics.plain);
            }
        }
        if let Ok(data) = serde_json::to_vec(&file) {
            let _ = write_atomically(&path, &data);
        }
    }

    /// Путь файла. Идентификатор должен быть безопасным для имени файла.
    fn path(&self, source: Source, track_id: &str) -> Option<PathBuf> {
        let safe = (1..=40).contains(&track_id.len())
            && track_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
        let dir = self.dir.as_ref()?;
        safe.then(|| dir.join(format!("{}-{track_id}.json", source.key())))
    }
}

#[cfg(test)]
impl Drop for Cache {
    fn drop(&mut self) {
        if let (true, Some(dir)) = (self.temp, &self.dir) {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// Запись через временный файл рядом: половинки файла в кэше не останется.
fn write_atomically(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("plastinka-lyrics-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn found() -> Outcome {
        Outcome::Found(Lyrics {
            source: Source::Yandex,
            synced: Some(vec![
                Line {
                    time_ms: 1000,
                    text: "строка".to_owned(),
                },
                Line {
                    time_ms: 2000,
                    text: String::new(),
                },
            ]),
            plain: Some("текст".to_owned()),
        })
    }

    #[test]
    fn found_roundtrip() {
        let cache = Cache::in_dir(dir("roundtrip"));
        cache.put(Source::Yandex, "123", &found(), 1000);
        assert_eq!(cache.get(Source::Yandex, "123", 2000), Some(found()));
        // Другой источник и другой трек — отдельные записи.
        assert_eq!(cache.get(Source::LrcLib, "123", 2000), None);
        assert_eq!(cache.get(Source::Yandex, "124", 2000), None);
    }

    #[test]
    fn negative_result_expires() {
        let cache = Cache::in_dir(dir("missing"));
        cache.put(Source::LrcLib, "1", &Outcome::Missing, 1000);
        assert_eq!(
            cache.get(Source::LrcLib, "1", 1000 + MISSING_TTL),
            Some(Outcome::Missing)
        );
        assert_eq!(cache.get(Source::LrcLib, "1", 1000 + MISSING_TTL + 1), None);
    }

    #[test]
    fn instrumental_and_found_live_longer() {
        let cache = Cache::in_dir(dir("long"));
        cache.put(Source::LrcLib, "1", &Outcome::Instrumental, 1000);
        cache.put(Source::Yandex, "1", &found(), 1000);
        let later = 1000 + 20 * DAY;
        assert_eq!(
            cache.get(Source::LrcLib, "1", later),
            Some(Outcome::Instrumental)
        );
        assert_eq!(cache.get(Source::Yandex, "1", later), Some(found()));
        assert_eq!(cache.get(Source::LrcLib, "1", 1000 + 31 * DAY), None);
    }

    #[test]
    fn broken_file_is_a_miss() {
        let base = dir("broken");
        let cache = Cache::in_dir(base.clone());
        std::fs::create_dir_all(&base).unwrap();
        for (name, content) in [
            ("yandex-1.json", "не json".to_owned()),
            (
                "yandex-2.json",
                r#"{"v":99,"saved":1,"kind":"missing"}"#.to_owned(),
            ),
            (
                "yandex-3.json",
                r#"{"v":1,"saved":1,"kind":"что-то"}"#.to_owned(),
            ),
            (
                "yandex-4.json",
                r#"{"v":1,"saved":1,"kind":"found"}"#.to_owned(),
            ),
            ("yandex-5.json", "x".repeat(2 * 1024 * 1024)),
        ] {
            std::fs::write(base.join(name), content).unwrap();
        }
        for id in ["1", "2", "3", "4", "5"] {
            assert_eq!(cache.get(Source::Yandex, id, 10), None, "{id}");
        }
    }

    #[test]
    fn future_timestamp_is_a_miss() {
        let cache = Cache::in_dir(dir("future"));
        cache.put(Source::Yandex, "1", &Outcome::Missing, 10 * DAY);
        assert_eq!(cache.get(Source::Yandex, "1", 1000), None);
    }

    #[test]
    fn unsafe_ids_and_no_dir_do_nothing() {
        let cache = Cache::in_dir(dir("unsafe"));
        cache.put(Source::Yandex, "../x", &Outcome::Missing, 1);
        assert_eq!(cache.get(Source::Yandex, "../x", 1), None);
        let none = Cache::default();
        none.put(Source::Yandex, "1", &Outcome::Missing, 1);
        assert_eq!(none.get(Source::Yandex, "1", 1), None);
    }
}
