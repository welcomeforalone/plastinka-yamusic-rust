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

/// Размер, который подставляется в шаблон адреса вместо `%%`: для строк
/// списка и для карточек и шапок.
const SIZE: &str = "100x100";
const MEDIUM_SIZE: &str = "300x300";
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
/// Больше картинка 300×300 не бывает; всё крупнее — не обложка.
const MAX_BYTES: usize = 2 * 1024 * 1024;
/// Откуда можно брать обложки.
/// `t2.genius.com` — обложки из списка FckCensor (только картинки, те же
/// ограничения размера).
const ALLOWED_HOSTS: [&str; 3] = [
    "avatars.yandex.net",
    "avatars.mds.yandex.net",
    "t2.genius.com",
];

/// Сколько сжатых байт обложек держать в памяти. Около 25 МБ — это больше
/// тысячи обложек: за день прослушивания в предел не упираемся, но память не
/// растёт бесконечно. Вытесненная обложка снова берётся из дискового кэша.
const MAX_CACHED_BYTES: usize = 24 * 1024 * 1024;
/// Вес неудавшейся обложки в том же счёте байт: записи копеечные, но и их
/// накопится не бесконечно много.
const FAILED_WEIGHT: usize = 512;

enum Entry {
    Loading,
    /// Картинка, когда она пришла (свежая обложка проявляется плавно), размер
    /// сжатых байт и номер последнего показа (для вытеснения).
    Ready {
        handle: image::Handle,
        at: Instant,
        bytes: usize,
        used: u64,
    },
    /// Не вышло загрузить; `used` — номер последнего показа.
    Failed {
        used: u64,
    },
}

impl Entry {
    /// Вес записи в общем счёте байт (`Loading` не весит и не вытесняется).
    fn weight(&self) -> usize {
        match self {
            Self::Loading => 0,
            Self::Ready { bytes, .. } => *bytes,
            Self::Failed { .. } => FAILED_WEIGHT,
        }
    }

    /// Номер последнего показа; у `Loading` его нет: его не вытесняют.
    fn used(&self) -> Option<u64> {
        match self {
            Self::Loading => None,
            Self::Ready { used, .. } | Self::Failed { used } => Some(*used),
        }
    }

    fn touch(&mut self, tick: u64) {
        if let Self::Ready { used, .. } | Self::Failed { used } = self {
            *used = tick;
        }
    }
}

/// Размер обложки: маленькая — в строках списка и в плеере, средняя — в
/// карточках и шапках альбома, плейлиста, исполнителя.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Grade {
    Small,
    Medium,
}

impl Grade {
    fn size(self) -> &'static str {
        match self {
            Self::Small => SIZE,
            Self::Medium => MEDIUM_SIZE,
        }
    }
}

/// Ключ обложки: размер и шаблон `cover_uri`.
pub type Key = (Grade, String);

/// Обложки, которые уже есть или грузятся.
pub struct Covers {
    cache_dir: Option<PathBuf>,
    entries: HashMap<Key, Entry>,
    /// Сумма весов `entries` (см. [`Entry::weight`]) и её предел.
    cached_bytes: usize,
    max_bytes: usize,
    /// Номер последнего вызова `show`: свежесть записей для вытеснения.
    tick: u64,
    /// Ключи, видимые при последнем `show`: их не вытесняют.
    visible: HashSet<Key>,
    /// Что видно сейчас и ещё не загружено, по порядку.
    wanted: Vec<Key>,
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
    /// Средний цвет обложки (считается один раз при подготовке): для
    /// плагина VibeColorizer.
    pub average: Option<[u8; 3]>,
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
        Self::with_limit(cache_dir, MAX_CACHED_BYTES)
    }

    fn with_limit(cache_dir: Option<PathBuf>, max_bytes: usize) -> Self {
        Self {
            cache_dir: cache_dir.map(|dir| dir.join("covers")),
            entries: HashMap::new(),
            cached_bytes: 0,
            max_bytes,
            tick: 0,
            visible: HashSet::new(),
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

    /// Средний цвет большой обложки `uri`, если она уже загружена.
    pub fn average(&self, uri: Option<&str>) -> Option<[u8; 3]> {
        let uri = uri?;
        self.big.iter().find_map(|(key, entry)| match entry {
            BigEntry::Ready(big, _) if key == uri => big.average,
            _ => None,
        })
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
        self.art_of(Grade::Small, uri)
    }

    /// Обложка размера `grade`.
    pub fn art_of(&self, grade: Grade, uri: Option<&str>) -> Art<'_> {
        let now = Instant::now();
        let Some(uri) = uri.filter(|uri| cover_url(uri, grade.size()).is_some()) else {
            return Art::Missing;
        };
        match self.entries.get(&(grade, uri.to_owned())) {
            Some(Entry::Ready { handle, at, .. }) => {
                let age = now.saturating_duration_since(*at).as_secs_f32();
                Art::Ready(handle, (age / anim::COVER.as_secs_f32()).clamp(0.0, 1.0))
            }
            Some(Entry::Failed { .. }) => Art::Missing,
            Some(Entry::Loading) | None => Art::Loading,
        }
    }

    /// Задаёт, что видно сейчас, и запускает загрузку того, чего ещё нет.
    /// Невидимое из очереди выкидывается.
    pub fn show<'a>(
        &mut self,
        visible: impl IntoIterator<Item = (Grade, &'a str)>,
        http: Option<&reqwest::Client>,
    ) -> Task<Message> {
        self.tick = self.tick.wrapping_add(1);
        let mut seen = HashSet::new();
        let mut wanted = Vec::new();
        self.visible.clear();
        for (grade, uri) in visible {
            if cover_url(uri, grade.size()).is_none() {
                continue;
            }
            let key = (grade, uri.to_owned());
            if !seen.insert(key.clone()) {
                continue;
            }
            // Видимое отмечаем свежим; чего нет (в том числе вытесненное) — просим.
            match self.entries.get_mut(&key) {
                Some(entry) => entry.touch(self.tick),
                None => wanted.push(key.clone()),
            }
            self.visible.insert(key);
        }
        self.wanted = wanted;
        self.pump(http)
    }

    /// Загрузка закончилась.
    pub fn finished(
        &mut self,
        key: Key,
        bytes: Option<Vec<u8>>,
        http: Option<&reqwest::Client>,
    ) -> Task<Message> {
        let entry = match bytes {
            Some(bytes) => {
                let now = Instant::now();
                self.last_ready = Some(now);
                Entry::Ready {
                    bytes: bytes.len(),
                    handle: image::Handle::from_bytes(bytes),
                    at: now,
                    used: self.tick,
                }
            }
            None => Entry::Failed { used: self.tick },
        };
        self.put(key, entry);
        self.evict();
        self.pump(http)
    }

    /// Кладёт запись, поддерживая сумму весов (старая запись того же ключа
    /// вычитается).
    fn put(&mut self, key: Key, entry: Entry) {
        self.cached_bytes = self.cached_bytes.saturating_add(entry.weight());
        if let Some(old) = self.entries.insert(key, entry) {
            self.cached_bytes = self.cached_bytes.saturating_sub(old.weight());
        }
    }

    /// Пока сумма больше предела, удаляет самые давние готовые записи. Не
    /// трогает `Loading` и то, что видно сейчас.
    fn evict(&mut self) {
        if self.cached_bytes <= self.max_bytes {
            return;
        }
        let mut old: Vec<(u64, Key)> = self
            .entries
            .iter()
            .filter(|(key, _)| !self.visible.contains(*key))
            .filter_map(|(key, entry)| Some((entry.used()?, key.clone())))
            .collect();
        old.sort_by_key(|(used, _)| *used);
        for (_, key) in old {
            if self.cached_bytes <= self.max_bytes {
                break;
            }
            if let Some(entry) = self.entries.remove(&key) {
                self.cached_bytes = self.cached_bytes.saturating_sub(entry.weight());
            }
        }
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
        let started: Vec<Key> = self.wanted.drain(..free).collect();
        let tasks: Vec<_> = started
            .into_iter()
            .map(|key| {
                self.put(key.clone(), Entry::Loading);
                let http = http.clone();
                let cache_dir = self.cache_dir.clone();
                Task::perform(
                    async move {
                        let (grade, uri) = &key;
                        let bytes = load(&http, cache_dir.as_deref(), uri, grade.size()).await;
                        (key, bytes)
                    },
                    |(key, bytes)| Message::CoverLoaded(key, bytes),
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
    let average = crate::plugins::vibe::average_rgb(small.as_raw());
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
        average,
    })
}

/// Адрес картинки размера `size` из шаблона, только на разрешённых хостах.
fn cover_url(template: &str, size: &str) -> Option<Url> {
    if !template.contains("%%") {
        return None;
    }
    let url = Url::parse(&format!("https://{}", template.replace("%%", size))).ok()?;
    is_trusted(&url).then_some(url)
}

/// Разрешённый хост, https, без логина и пароля в адресе.
fn is_trusted(url: &Url) -> bool {
    url.scheme() == "https"
        && url
            .host_str()
            .is_some_and(|host| ALLOWED_HOSTS.contains(&host))
        && url.username().is_empty()
        && url.password().is_none()
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
    download_url(http, url).await
}

/// Аватар аккаунта по готовому адресу (без токена), только в памяти: на диск
/// не пишется. Хост проверяется так же, как у обложек.
pub async fn load_avatar(http: &reqwest::Client, url: &str) -> Option<Arc<Avatar>> {
    let url = Url::parse(url).ok().filter(is_trusted)?;
    let bytes = download_url(http, url).await.ok()?;
    if !looks_like_image(&bytes) {
        return None;
    }
    // Уровни считает процессор: не в async-потоке.
    tokio::task::spawn_blocking(move || Avatar::build(bytes))
        .await
        .ok()
        .flatten()
        .map(Arc::new)
}

/// Самый крупный уровень аватара и самый мелкий, который ещё делаем.
const AVATAR_MAX: u32 = 256;
const AVATAR_MIN: f32 = 24.0;
/// Шаг между уровнями: корень из двух.
const AVATAR_STEP: f32 = std::f32::consts::SQRT_2;

/// Аватар в нескольких размерах. iced уменьшает картинку билинейно и без
/// mip-уровней: при уменьшении больше чем вдвое появляются «лесенки». Поэтому
/// заранее готовим уровни с шагом √2 и берём ближайший не мельче нужного.
#[derive(Debug)]
pub struct Avatar {
    /// По возрастанию стороны; квадраты.
    levels: Vec<(u32, image::Handle)>,
}

impl Avatar {
    /// Из байтов картинки: обрезка по центру до квадрата, затем уровни
    /// `Lanczos3` из исходника (а не друг из друга). `None` — не картинка или
    /// слишком большая.
    fn build(bytes: Vec<u8>) -> Option<Self> {
        let mut limits = ::image::Limits::default();
        limits.max_image_width = Some(BIG_MAX_SIDE);
        limits.max_image_height = Some(BIG_MAX_SIDE);
        limits.max_alloc = Some(BIG_MAX_ALLOC);
        let mut reader = ::image::ImageReader::new(std::io::Cursor::new(bytes))
            .with_guessed_format()
            .ok()?;
        reader.limits(limits);
        let picture = reader.decode().ok()?;
        Self::from_picture(&picture)
    }

    fn from_picture(picture: &::image::DynamicImage) -> Option<Self> {
        let side = picture.width().min(picture.height());
        if side == 0 {
            return None;
        }
        let square = picture.crop_imm(
            (picture.width() - side) / 2,
            (picture.height() - side) / 2,
            side,
            side,
        );
        let mut levels: Vec<(u32, image::Handle)> = Vec::new();
        for size in Self::sizes(side) {
            let level = if size == side {
                square.to_rgba8()
            } else {
                square
                    .resize_exact(size, size, ::image::imageops::FilterType::Lanczos3)
                    .to_rgba8()
            };
            levels.push((size, image::Handle::from_rgba(size, size, level.into_raw())));
        }
        levels.reverse();
        Some(Self { levels })
    }

    /// Стороны уровней для исходника со стороной `side`, от крупного.
    fn sizes(side: u32) -> Vec<u32> {
        let top = side.min(AVATAR_MAX);
        let mut sizes = vec![top];
        let mut exact = top as f32;
        loop {
            exact /= AVATAR_STEP;
            if exact < AVATAR_MIN {
                break;
            }
            let size = exact.round() as u32;
            if sizes.last() != Some(&size) {
                sizes.push(size);
            }
        }
        sizes
    }

    /// Самый мелкий уровень, не меньше `physical_px`; если таких нет (или
    /// размер не число) — самый крупный.
    pub fn pick(&self, physical_px: f32) -> Option<&image::Handle> {
        if !physical_px.is_finite() {
            return self.levels.last().map(|(_, handle)| handle);
        }
        self.levels
            .iter()
            .find(|(side, _)| *side as f32 >= physical_px)
            .or_else(|| self.levels.last())
            .map(|(_, handle)| handle)
    }

    #[cfg(test)]
    fn sides(&self) -> Vec<u32> {
        self.levels.iter().map(|(side, _)| *side).collect()
    }
}

async fn download_url(http: &reqwest::Client, url: Url) -> Result<Vec<u8>, NetError> {
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

    fn key(n: u32) -> Key {
        (Grade::Small, format!("avatars.yandex.net/get-music/{n}/%%"))
    }

    fn uri(n: u32) -> String {
        key(n).1
    }

    /// Показывает обложки `ns` и сразу «загружает» недостающие по 100 байт.
    fn show_and_load(covers: &mut Covers, ns: &[u32]) {
        let uris: Vec<String> = ns.iter().map(|n| uri(*n)).collect();
        let _ = covers.show(uris.iter().map(|uri| (Grade::Small, uri.as_str())), None);
        for key in std::mem::take(&mut covers.wanted) {
            let _ = covers.finished(key, Some(vec![0; 100]), None);
        }
    }

    fn has(covers: &Covers, n: u32) -> bool {
        covers.entries.contains_key(&key(n))
    }

    #[test]
    fn oldest_cover_is_evicted_first() {
        let mut covers = Covers::with_limit(None, 250);
        show_and_load(&mut covers, &[1]);
        show_and_load(&mut covers, &[2]);
        show_and_load(&mut covers, &[3]);
        // 300 байт при пределе 250: ушла самая давняя.
        assert!(!has(&covers, 1));
        assert!(has(&covers, 2) && has(&covers, 3));
        assert_eq!(covers.cached_bytes, 200);
    }

    #[test]
    fn visible_and_loading_are_never_evicted() {
        let mut covers = Covers::with_limit(None, 150);
        show_and_load(&mut covers, &[1, 2, 3]);
        // Все три видны: предел превышен, но вытеснять нечего.
        assert!(has(&covers, 1) && has(&covers, 2) && has(&covers, 3));
        assert_eq!(covers.cached_bytes, 300);
        covers.put(key(9), Entry::Loading);
        show_and_load(&mut covers, &[3]);
        let _ = covers.finished(key(4), Some(vec![0; 100]), None);
        assert!(has(&covers, 9));
        assert!(has(&covers, 3));
        assert!(!has(&covers, 1) && !has(&covers, 2));
    }

    #[test]
    fn reloading_same_key_keeps_sum_correct() {
        let mut covers = Covers::with_limit(None, 10_000);
        let _ = covers.finished(key(1), Some(vec![0; 100]), None);
        let _ = covers.finished(key(1), Some(vec![0; 40]), None);
        assert_eq!(covers.cached_bytes, 40);
        let _ = covers.finished(key(1), None, None);
        assert_eq!(covers.cached_bytes, FAILED_WEIGHT);
        covers.put(key(1), Entry::Loading);
        assert_eq!(covers.cached_bytes, 0);
    }

    #[test]
    fn evicted_cover_is_wanted_again() {
        let mut covers = Covers::with_limit(None, 150);
        show_and_load(&mut covers, &[1]);
        show_and_load(&mut covers, &[2]);
        assert!(!has(&covers, 1));
        let uris = [uri(1)];
        let _ = covers.show(uris.iter().map(|uri| (Grade::Small, uri.as_str())), None);
        assert_eq!(covers.wanted, vec![key(1)]);
    }

    fn side_of(handle: Option<&image::Handle>) -> Option<u32> {
        match handle {
            Some(image::Handle::Rgba { width, .. }) => Some(*width),
            _ => None,
        }
    }

    fn solid(width: u32, height: u32) -> ::image::DynamicImage {
        ::image::DynamicImage::ImageRgba8(::image::RgbaImage::from_pixel(
            width,
            height,
            ::image::Rgba([10, 20, 30, 255]),
        ))
    }

    #[test]
    fn avatar_levels_step_by_sqrt_two() {
        let avatar = Avatar::from_picture(&solid(200, 200)).unwrap();
        assert_eq!(avatar.sides(), vec![25, 35, 50, 71, 100, 141, 200]);
        // Все уровни квадратные, и пиксели не пустые.
        for (side, handle) in &avatar.levels {
            let image::Handle::Rgba {
                width,
                height,
                pixels,
                ..
            } = handle
            else {
                panic!("уровень не RGBA");
            };
            assert_eq!((*width, *height), (*side, *side));
            assert_eq!(pixels.len(), (side * side * 4) as usize);
        }
    }

    #[test]
    fn avatar_is_cropped_to_a_square_and_capped() {
        // Широкая картинка: обрезка по центру до меньшей стороны.
        let avatar = Avatar::from_picture(&solid(300, 120)).unwrap();
        assert_eq!(avatar.sides().last(), Some(&120));
        assert!(avatar.levels.iter().all(|(side, _)| *side <= 120));
        // Высокая тоже.
        let avatar = Avatar::from_picture(&solid(90, 400)).unwrap();
        assert_eq!(avatar.sides().last(), Some(&90));
        // Огромная ужимается до предела.
        let avatar = Avatar::from_picture(&solid(600, 600)).unwrap();
        assert_eq!(avatar.sides().last(), Some(&AVATAR_MAX));
        // Крошечная: один уровень.
        let avatar = Avatar::from_picture(&solid(10, 10)).unwrap();
        assert_eq!(avatar.sides(), vec![10]);
    }

    #[test]
    fn avatar_pick_takes_smallest_level_that_is_not_smaller() {
        let avatar = Avatar::from_picture(&solid(200, 200)).unwrap();
        // Точное попадание.
        assert_eq!(side_of(avatar.pick(100.0)), Some(100));
        // Между уровнями: ближайший сверху.
        assert_eq!(side_of(avatar.pick(72.0)), Some(100));
        assert_eq!(side_of(avatar.pick(70.5)), Some(71));
        // Больше самого крупного: самый крупный.
        assert_eq!(side_of(avatar.pick(500.0)), Some(200));
        // Ноль и мелочь: самый мелкий; не число: самый крупный.
        assert_eq!(side_of(avatar.pick(0.0)), Some(25));
        assert_eq!(side_of(avatar.pick(-3.0)), Some(25));
        assert_eq!(side_of(avatar.pick(f32::NAN)), Some(200));
        assert_eq!(side_of(avatar.pick(f32::INFINITY)), Some(200));
    }

    #[test]
    fn avatar_from_bytes_refuses_garbage() {
        assert!(Avatar::build(b"not an image".to_vec()).is_none());
        let mut png = Vec::new();
        solid(50, 50)
            .write_to(
                &mut std::io::Cursor::new(&mut png),
                ::image::ImageFormat::Png,
            )
            .unwrap();
        let avatar = Avatar::build(png).unwrap();
        assert_eq!(avatar.sides(), vec![25, 35, 50]);
    }

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
