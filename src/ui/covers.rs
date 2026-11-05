//! Обложки: загрузка с `avatars.yandex.net` и кэш картинок на диске.
//!
//! Правило 1 разрешает кэшировать обложки: здесь только картинки, аудио сюда
//! не попадает. Грузим только то, что видно, не больше [`MAX_IN_FLIGHT`]
//! запросов сразу. Подробности — `docs/ui/stage-5.md`.
//!
//! Для полноэкранного плеера — обложка текущего трека крупнее и её размытая
//! затемнённая копия для фона. Размытие считается на процессоре один раз, в
//! фоновом потоке, по маленькой копии картинки: при растяжке на всё окно она
//! и так выходит мягкой. Подробности — `docs/ui/stage-6.5.md`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use iced::Task;
use iced::widget::image;
use reqwest::Url;

use super::{Message, anim};
use crate::net::NetError;
use iced::time::Instant;

/// Размер, который подставляется в шаблон адреса вместо `%%`.
const SIZE: &str = "100x100";
/// Размер обложки в полноэкранном плеере.
const BIG_SIZE: &str = "600x600";
/// Сторона маленькой копии для фона, пикселей, и сила размытия на ней.
const BACKDROP_SIDE: u32 = 48;
const BACKDROP_SIGMA: f32 = 3.0;
/// Яркость фона: доля от исходной, из 256.
const BACKDROP_BRIGHTNESS: u16 = 120;
/// Пределы для большой обложки при распаковке: файл до 2 МБ может
/// развернуться в огромную картинку. Больше — не обложка.
const BIG_MAX_SIDE: u32 = 3000;
const BIG_MAX_ALLOC: u64 = 64 * 1024 * 1024;
/// Сторона большой обложки, которую отдаём окну.
const BIG_SIDE: u32 = 600;
/// Сколько больших обложек держать в памяти.
const BIG_KEEP: usize = 4;
/// Сколько обложек грузить одновременно.
const MAX_IN_FLIGHT: usize = 6;
/// Больше картинка 100×100 не бывает; всё крупнее — не обложка.
const MAX_BYTES: usize = 2 * 1024 * 1024;
/// Откуда можно брать обложки.
const ALLOWED_HOSTS: [&str; 2] = ["avatars.yandex.net", "avatars.mds.yandex.net"];

enum Entry {
    Loading,
    /// Картинка и когда она пришла: свежая обложка проявляется плавно.
    Ready(image::Handle, Instant),
    Failed,
}

/// Обложки, которые уже есть или грузятся. Ключ — шаблон `cover_uri`.
pub struct Covers {
    cache_dir: Option<PathBuf>,
    entries: HashMap<String, Entry>,
    /// Что видно сейчас и ещё не загружено, по порядку.
    wanted: Vec<String>,
    /// Когда пришла последняя обложка: пока она проявляется, нужны кадры.
    last_ready: Option<Instant>,
    /// Большие обложки с фоном, свежие в конце.
    big: Vec<(String, BigEntry)>,
}

enum BigEntry {
    Loading,
    Ready(Arc<Big>, Instant),
    Failed,
}

/// Обложка для полноэкранного плеера и размытый фон из неё.
#[derive(Debug)]
pub struct Big {
    pub cover: image::Handle,
    pub backdrop: image::Handle,
}

/// Состояние обложки для отрисовки.
pub enum Art<'a> {
    /// Картинка и её непрозрачность (свежая проявляется).
    Ready(&'a image::Handle, f32),
    Loading,
    Missing,
}

impl Covers {
    pub fn new(cache_dir: Option<PathBuf>) -> Self {
        Self {
            cache_dir: cache_dir.map(|dir| dir.join("covers")),
            entries: HashMap::new(),
            wanted: Vec::new(),
            last_ready: None,
            big: Vec::new(),
        }
    }

    /// Большая обложка по шаблону `uri` и её непрозрачность (свежая
    /// проявляется). `None` — ещё грузится или не вышло.
    pub fn big(&self, uri: Option<&str>, now: Instant) -> Option<(&Big, f32)> {
        let uri = uri?;
        self.big.iter().find_map(|(key, entry)| match entry {
            BigEntry::Ready(big, at) if key == uri => {
                let age = now.saturating_duration_since(*at).as_secs_f32();
                Some((&**big, (age / anim::COVER.as_secs_f32()).clamp(0.0, 1.0)))
            }
            _ => None,
        })
    }

    /// Нужна большая обложка `uri`: запускает загрузку, если её ещё нет.
    pub fn want_big(&mut self, uri: Option<&str>, http: Option<&reqwest::Client>) -> Task<Message> {
        let uri = uri.filter(|uri| cover_url(uri, BIG_SIZE).is_some());
        let (Some(uri), Some(http)) = (uri, http) else {
            return Task::none();
        };
        if self.big.iter().any(|(key, _)| key == uri) {
            return Task::none();
        }
        if self.big.len() >= BIG_KEEP {
            // Выкидываем самую старую. Если она ещё грузилась, её ответ просто
            // ляжет заново.
            self.big.remove(0);
        }
        self.big.push((uri.to_owned(), BigEntry::Loading));
        let http = http.clone();
        let cache_dir = self.cache_dir.clone();
        let uri = uri.to_owned();
        Task::perform(
            async move {
                let big = match load(&http, cache_dir.as_deref(), &uri, BIG_SIZE).await {
                    Some(bytes) => tokio::task::spawn_blocking(move || prepare_big(bytes))
                        .await
                        .ok()
                        .flatten()
                        .map(Arc::new),
                    None => None,
                };
                (uri, big)
            },
            |(uri, big)| Message::BigCoverLoaded(uri, big),
        )
    }

    /// Большая обложка загрузилась (или нет).
    pub fn big_finished(&mut self, uri: String, big: Option<Arc<Big>>) {
        let now = Instant::now();
        let entry = match big {
            Some(big) => {
                self.last_ready = Some(now);
                BigEntry::Ready(big, now)
            }
            None => BigEntry::Failed,
        };
        match self.big.iter_mut().find(|(key, _)| *key == uri) {
            Some((_, slot)) => *slot = entry,
            None => {
                if self.big.len() >= BIG_KEEP {
                    self.big.remove(0);
                }
                self.big.push((uri, entry));
            }
        }
    }

    /// Обложка по шаблону `uri`. Непрозрачность свежей обложки считается на
    /// момент построения вида.
    pub fn art(&self, uri: Option<&str>) -> Art<'_> {
        let now = Instant::now();
        let Some(uri) = uri.filter(|uri| cover_url(uri, SIZE).is_some()) else {
            return Art::Missing;
        };
        match self.entries.get(uri) {
            Some(Entry::Ready(handle, at)) => {
                let age = now.saturating_duration_since(*at).as_secs_f32();
                Art::Ready(handle, (age / anim::COVER.as_secs_f32()).clamp(0.0, 1.0))
            }
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
            .filter(|uri| cover_url(uri, SIZE).is_some())
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
            Some(bytes) => {
                let now = Instant::now();
                self.last_ready = Some(now);
                Entry::Ready(image::Handle::from_bytes(bytes), now)
            }
            None => Entry::Failed,
        };
        self.entries.insert(uri, entry);
        self.pump(http)
    }

    /// Проявляется ли сейчас какая-нибудь обложка.
    pub fn animating(&self, now: Instant) -> bool {
        self.last_ready
            .is_some_and(|at| now.saturating_duration_since(at) < anim::COVER)
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
                        let bytes = load(&http, cache_dir.as_deref(), &uri, SIZE).await;
                        (uri, bytes)
                    },
                    |(uri, bytes)| Message::CoverLoaded(uri, bytes),
                )
            })
            .collect();
        Task::batch(tasks)
    }
}

/// Картинка для полноэкранного плеера: сама обложка и маленькая размытая
/// затемнённая копия для фона. `None` — не картинка или слишком большая.
/// Распаковываем здесь, с пределами, и отдаём окну готовые пиксели: так
/// iced не распаковывает файл второй раз и без пределов.
fn prepare_big(bytes: Vec<u8>) -> Option<Big> {
    let mut limits = ::image::Limits::default();
    limits.max_image_width = Some(BIG_MAX_SIDE);
    limits.max_image_height = Some(BIG_MAX_SIDE);
    limits.max_alloc = Some(BIG_MAX_ALLOC);
    let mut reader = ::image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    reader.limits(limits);
    let picture = reader.decode().ok()?;
    let cover = if picture.width() > BIG_SIDE || picture.height() > BIG_SIDE {
        picture.resize(BIG_SIDE, BIG_SIDE, ::image::imageops::FilterType::Triangle)
    } else {
        picture.clone()
    }
    .to_rgba8();
    let small = picture
        .resize_exact(
            BACKDROP_SIDE,
            BACKDROP_SIDE,
            ::image::imageops::FilterType::Triangle,
        )
        .to_rgba8();
    let mut pixels = ::image::imageops::blur(&small, BACKDROP_SIGMA).into_raw();
    for pixel in pixels.as_chunks_mut::<4>().0 {
        for channel in pixel.iter_mut().take(3) {
            let dimmed = u16::from(*channel) * BACKDROP_BRIGHTNESS / 256;
            *channel = u8::try_from(dimmed).unwrap_or(u8::MAX);
        }
    }
    Some(Big {
        cover: image::Handle::from_rgba(cover.width(), cover.height(), cover.into_raw()),
        backdrop: image::Handle::from_rgba(BACKDROP_SIDE, BACKDROP_SIDE, pixels),
    })
}

/// Адрес картинки размера `size` из шаблона, только на разрешённых хостах.
fn cover_url(template: &str, size: &str) -> Option<Url> {
    if !template.contains("%%") {
        return None;
    }
    let url = Url::parse(&format!("https://{}", template.replace("%%", size))).ok()?;
    let trusted = url
        .host_str()
        .is_some_and(|host| ALLOWED_HOSTS.contains(&host));
    (trusted && url.username().is_empty() && url.password().is_none()).then_some(url)
}

/// Имя файла в кэше: шаблон, где всё кроме букв, цифр и дефиса заменено на `_`.
fn cache_name(template: &str, size: &str) -> String {
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
    format!("{safe}-{:016x}-{size}.img", fnv1a(template))
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
async fn load(
    http: &reqwest::Client,
    cache_dir: Option<&Path>,
    template: &str,
    size: &str,
) -> Option<Vec<u8>> {
    let path = cache_dir.map(|dir| dir.join(cache_name(template, size)));
    if let Some(path) = path.clone() {
        let cached = tokio::task::spawn_blocking(move || std::fs::read(path))
            .await
            .ok()
            .and_then(Result::ok);
        if let Some(bytes) = cached.filter(|bytes| looks_like_image(bytes)) {
            return Some(bytes);
        }
    }
    let bytes = download(http, template, size).await.ok()?;
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

async fn download(http: &reqwest::Client, template: &str, size: &str) -> Result<Vec<u8>, NetError> {
    let url =
        cover_url(template, size).ok_or(NetError::Other("неверный адрес обложки".to_owned()))?;
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
        let url = cover_url("avatars.yandex.net/get-music-content/38044/abc/%%", SIZE).unwrap();
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
            assert!(cover_url(bad, SIZE).is_none(), "{bad}");
        }
    }

    #[test]
    fn cache_name_is_safe() {
        let name = cache_name("avatars.yandex.net/get-music-content/38044/a..b/%%", SIZE);
        assert!(name.starts_with("avatars_yandex_net_get-music-content_38044_a__b___-"));
        assert!(name.ends_with("-100x100.img"));
        let evil = cache_name("../../evil", SIZE);
        assert!(evil.starts_with("______evil-"));
        assert!(!evil.contains('/') && !evil.contains(".."));
        // Адреса, которые замена символов склеивает, дают разные имена.
        assert_ne!(cache_name("a/b", SIZE), cache_name("a.b", SIZE));
        assert_eq!(cache_name("a/b", SIZE), cache_name("a/b", SIZE));
        assert_ne!(cache_name("a/b", SIZE), cache_name("a/b", BIG_SIZE));
    }

    #[test]
    fn images_are_recognized() {
        assert!(looks_like_image(&[0xFF, 0xD8, 0xFF, 0xE0]));
        assert!(looks_like_image(b"\x89PNG\r\n"));
        assert!(looks_like_image(b"RIFF\0\0\0\0WEBPVP8 "));
        assert!(!looks_like_image(b"<html>"));
        assert!(!looks_like_image(b""));
    }

    #[test]
    fn big_cover_gets_a_dim_backdrop() {
        let mut png = Vec::new();
        let picture = ::image::RgbaImage::from_pixel(64, 64, ::image::Rgba([250, 40, 70, 255]));
        picture
            .write_to(
                &mut std::io::Cursor::new(&mut png),
                ::image::ImageFormat::Png,
            )
            .unwrap();
        let big = prepare_big(png).unwrap();
        let image::Handle::Rgba { width, pixels, .. } = &big.backdrop else {
            panic!("фон не RGBA");
        };
        assert_eq!(*width, BACKDROP_SIDE);
        // Красный канал потемнел примерно вдвое, альфа не тронута.
        assert!((110..=125).contains(&pixels[0]), "{}", pixels[0]);
        assert_eq!(pixels[3], 255);
        assert!(prepare_big(b"<html>".to_vec()).is_none());
    }

    #[test]
    fn huge_big_cover_is_refused_and_large_one_is_shrunk() {
        let png = |side: u32| {
            let mut bytes = Vec::new();
            ::image::GrayImage::new(side, side)
                .write_to(
                    &mut std::io::Cursor::new(&mut bytes),
                    ::image::ImageFormat::Png,
                )
                .unwrap();
            bytes
        };
        // Сжимается в пару килобайт, а развернулась бы в 16 Мпикс.
        assert!(prepare_big(png(4000)).is_none());
        let big = prepare_big(png(1200)).unwrap();
        let image::Handle::Rgba { width, height, .. } = &big.cover else {
            panic!("обложка не RGBA");
        };
        assert_eq!((*width, *height), (BIG_SIDE, BIG_SIDE));
    }
}
