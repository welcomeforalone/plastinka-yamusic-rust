//! Обложки: загрузка с `avatars.yandex.net` и кэш картинок на диске.
//!
//! Правило 1 разрешает кэшировать обложки: здесь только картинки, аудио сюда
//! не попадает. Грузим только то, что видно, не больше [`MAX_IN_FLIGHT`]
//! запросов сразу. Подробности — `docs/ui/stage-5.md`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use iced::Task;
use iced::widget::image;
use reqwest::Url;

use super::Message;
use crate::net::NetError;

/// Размер, который подставляется в шаблон адреса вместо `%%`.
const SIZE: &str = "100x100";
/// Сколько обложек грузить одновременно.
const MAX_IN_FLIGHT: usize = 6;
/// Больше картинка 100×100 не бывает; всё крупнее — не обложка.
const MAX_BYTES: usize = 2 * 1024 * 1024;
/// Откуда можно брать обложки.
const ALLOWED_HOSTS: [&str; 2] = ["avatars.yandex.net", "avatars.mds.yandex.net"];

enum Entry {
    Loading,
    Ready(image::Handle),
    Failed,
}

/// Обложки, которые уже есть или грузятся. Ключ — шаблон `cover_uri`.
pub struct Covers {
    cache_dir: Option<PathBuf>,
    entries: HashMap<String, Entry>,
    /// Что видно сейчас и ещё не загружено, по порядку.
    wanted: Vec<String>,
}

/// Состояние обложки для отрисовки.
pub enum Art<'a> {
    Ready(&'a image::Handle),
    Loading,
    Missing,
}

impl Covers {
    pub fn new(cache_dir: Option<PathBuf>) -> Self {
        Self {
            cache_dir: cache_dir.map(|dir| dir.join("covers")),
            entries: HashMap::new(),
            wanted: Vec::new(),
        }
    }

    pub fn art(&self, uri: Option<&str>) -> Art<'_> {
        let Some(uri) = uri.filter(|uri| cover_url(uri).is_some()) else {
            return Art::Missing;
        };
        match self.entries.get(uri) {
            Some(Entry::Ready(handle)) => Art::Ready(handle),
            Some(Entry::Failed) => Art::Missing,
            Some(Entry::Loading) | None => Art::Loading,
        }
    }

    /// Задаёт, что видно сейчас, и запускает загрузку того, чего ещё нет.
    /// Невидимое из очереди выкидывается.
    pub fn show<'a>(
        &mut self,
        visible: impl IntoIterator<Item = &'a str>,
        http: Option<&reqwest::Client>,
    ) -> Task<Message> {
        let mut seen = HashSet::new();
        self.wanted = visible
            .into_iter()
            .filter(|uri| cover_url(uri).is_some())
            .filter(|uri| !self.entries.contains_key(*uri) && seen.insert(*uri))
            .map(str::to_owned)
            .collect();
        self.pump(http)
    }

    /// Загрузка закончилась.
    pub fn finished(
        &mut self,
        uri: String,
        bytes: Option<Vec<u8>>,
        http: Option<&reqwest::Client>,
    ) -> Task<Message> {
        let entry = match bytes {
            Some(bytes) => Entry::Ready(image::Handle::from_bytes(bytes)),
            None => Entry::Failed,
        };
        self.entries.insert(uri, entry);
        self.pump(http)
    }

    fn pump(&mut self, http: Option<&reqwest::Client>) -> Task<Message> {
        let Some(http) = http else {
            return Task::none();
        };
        let in_flight = self
            .entries
            .values()
            .filter(|entry| matches!(entry, Entry::Loading))
            .count();
        let free = MAX_IN_FLIGHT
            .saturating_sub(in_flight)
            .min(self.wanted.len());
        let tasks: Vec<_> = self
            .wanted
            .drain(..free)
            .map(|uri| {
                self.entries.insert(uri.clone(), Entry::Loading);
                let http = http.clone();
                let cache_dir = self.cache_dir.clone();
                Task::perform(
                    async move {
                        let bytes = load(&http, cache_dir.as_deref(), &uri).await;
                        (uri, bytes)
                    },
                    |(uri, bytes)| Message::CoverLoaded(uri, bytes),
                )
            })
            .collect();
        Task::batch(tasks)
    }
}

/// Адрес картинки из шаблона, только на разрешённых хостах.
fn cover_url(template: &str) -> Option<Url> {
    if !template.contains("%%") {
        return None;
    }
    let url = Url::parse(&format!("https://{}", template.replace("%%", SIZE))).ok()?;
    let trusted = url
        .host_str()
        .is_some_and(|host| ALLOWED_HOSTS.contains(&host));
    (trusted && url.username().is_empty() && url.password().is_none()).then_some(url)
}

/// Имя файла в кэше: шаблон, где всё кроме букв, цифр и дефиса заменено на `_`.
fn cache_name(template: &str) -> String {
    let safe: String = template
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .take(160)
        .collect();
    // Замена символов склеивает разные адреса, хеш их разводит.
    format!("{safe}-{:016x}-{SIZE}.img", fnv1a(template))
}

/// FNV-1a, 64 бита: стабилен между запусками, в отличие от `DefaultHasher`.
fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// Похоже ли на картинку, которую умеем показать (JPEG, PNG, WebP).
fn looks_like_image(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xFF, 0xD8, 0xFF])
        || bytes.starts_with(b"\x89PNG")
        || (bytes.get(..4) == Some(b"RIFF") && bytes.get(8..12) == Some(b"WEBP"))
}

/// Картинка из кэша или из сети. Ошибки не важны: тогда остаётся заглушка.
async fn load(http: &reqwest::Client, cache_dir: Option<&Path>, template: &str) -> Option<Vec<u8>> {
    let path = cache_dir.map(|dir| dir.join(cache_name(template)));
    if let Some(path) = path.clone() {
        let cached = tokio::task::spawn_blocking(move || std::fs::read(path))
            .await
            .ok()
            .and_then(Result::ok);
        if let Some(bytes) = cached.filter(|bytes| looks_like_image(bytes)) {
            return Some(bytes);
        }
    }
    let bytes = download(http, template).await.ok()?;
    if !looks_like_image(&bytes) {
        return None;
    }
    if let Some(path) = path {
        let data = bytes.clone();
        // Сбой записи в кэш показу не мешает.
        let _ = tokio::task::spawn_blocking(move || write_atomically(&path, &data)).await;
    }
    Some(bytes)
}

async fn download(http: &reqwest::Client, template: &str) -> Result<Vec<u8>, NetError> {
    let url = cover_url(template).ok_or(NetError::Other("неверный адрес обложки".to_owned()))?;
    let mut response = http.get(url).send().await?;
    if !response.status().is_success() {
        return Err(NetError::Other(format!(
            "код {}",
            response.status().as_u16()
        )));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len() + chunk.len() > MAX_BYTES {
            return Err(NetError::Other("слишком большая обложка".to_owned()));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Запись через временный файл рядом: половинка картинки в кэше не останется.
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

    #[test]
    fn cover_url_is_built_only_for_yandex_hosts() {
        let url = cover_url("avatars.yandex.net/get-music-content/38044/abc/%%").unwrap();
        assert_eq!(
            url.as_str(),
            "https://avatars.yandex.net/get-music-content/38044/abc/100x100"
        );
        for bad in [
            "avatars.example/abc/%%",
            "avatars.yandex.net.evil.example/%%",
            "user@avatars.yandex.net/%%",
            "avatars.yandex.net/no-size",
            "",
        ] {
            assert!(cover_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn cache_name_is_safe() {
        let name = cache_name("avatars.yandex.net/get-music-content/38044/a..b/%%");
        assert!(name.starts_with("avatars_yandex_net_get-music-content_38044_a__b___-"));
        assert!(name.ends_with("-100x100.img"));
        let evil = cache_name("../../evil");
        assert!(evil.starts_with("______evil-"));
        assert!(!evil.contains('/') && !evil.contains(".."));
        // Адреса, которые замена символов склеивает, дают разные имена.
        assert_ne!(cache_name("a/b"), cache_name("a.b"));
        assert_eq!(cache_name("a/b"), cache_name("a/b"));
    }

    #[test]
    fn images_are_recognized() {
        assert!(looks_like_image(&[0xFF, 0xD8, 0xFF, 0xE0]));
        assert!(looks_like_image(b"\x89PNG\r\n"));
        assert!(looks_like_image(b"RIFF\0\0\0\0WEBPVP8 "));
        assert!(!looks_like_image(b"<html>"));
        assert!(!looks_like_image(b""));
    }
}
