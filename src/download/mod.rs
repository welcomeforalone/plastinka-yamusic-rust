//! Скачивание треков в файл. Только по явному действию пользователя
//! («Скачать» в меню трека или плеера, «Скачать всё» на странице альбома и
//! плейлиста) и только в папку, которую он выбрал сам. Плеер по-прежнему
//! держит поток в памяти: общего кэша треков и скрытых копий нет.
//!
//! Порядок одного трека ([`run`]): проверка «уже скачан» → прямая ссылка тем
//! же путём, что у плеера → загрузка в память с докачкой при обрыве (до трёх
//! повторов) → при необходимости FLAC вынимается из mp4 → теги → запись во
//! временный `*.part` рядом → переименование. При любой ошибке `.part`
//! удаляется. Токен уходит только в запросы к API.

pub mod flac;
pub mod name;
pub mod queue;
pub mod tags;

use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::StatusCode;
use reqwest::header::{CONTENT_RANGE, CONTENT_TYPE, RANGE};

use crate::api::{self, AudioQuality, Codec, StreamError, StreamSource, Track};
use crate::net::{self, NetError};
use crate::plugins::fckcensor;

/// Сколько раз повторять загрузку после обрыва.
const RETRIES: usize = 3;
/// Паузы перед повторами.
const BACKOFF: [Duration; RETRIES] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
];
/// Больше этого не качаем: так сбойное хранилище не займёт всю память.
const MAX_BYTES: usize = 1 << 30;
/// Размер обложки в теге.
const COVER_SIZE: &str = "1000x1000";
/// Обложка больше этого в тег не идёт.
const MAX_COVER: usize = 12 * 1024 * 1024;
/// Расширения, по которым узнаём уже скачанный трек.
const EXTENSIONS: [&str; 4] = ["mp3", "m4a", "flac", "aac"];

/// Вид сохранённого файла.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Mp3,
    /// AAC в mp4, как пришёл.
    M4a,
    /// Настоящий FLAC.
    Flac,
    /// AAC без контейнера (ADTS).
    Aac,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::M4a => "m4a",
            Self::Flac => "flac",
            Self::Aac => "aac",
        }
    }
}

/// Что лежит в скачанных байтах (по началу файла).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Container {
    Mp3,
    Adts,
    Mp4,
    Flac,
}

fn sniff(data: &[u8]) -> Option<Container> {
    if data.starts_with(flac::MAGIC) {
        return Some(Container::Flac);
    }
    if data.get(4..8) == Some(b"ftyp") {
        return Some(Container::Mp4);
    }
    if data.starts_with(b"ID3") {
        return Some(Container::Mp3);
    }
    match data {
        // Синхрослово MPEG: 11 единиц. Слой 00 — это ADTS (AAC), иначе MP3.
        [0xFF, second, ..] if second & 0xE0 == 0xE0 => {
            if second & 0x06 == 0 {
                Some(Container::Adts)
            } else {
                Some(Container::Mp3)
            }
        }
        _ => None,
    }
}

/// Ошибки скачивания одного трека. Текст показывается пользователю.
#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("сервис музыки не принял токен")]
    Unauthorized,
    #[error("трек недоступен для скачивания")]
    Unavailable,
    #[error("{0}")]
    Net(String),
    #[error("хранилище прислало не звук или неизвестный формат")]
    Format,
    #[error("{0}")]
    Flac(#[from] flac::FlacError),
    #[error("не удалось записать файл: {0}")]
    Disk(String),
    #[error("{0}")]
    Other(String),
}

impl From<StreamError> for DownloadError {
    fn from(err: StreamError) -> Self {
        match err {
            StreamError::Unavailable => Self::Unavailable,
            StreamError::Api(api::ApiError::Unauthorized) => Self::Unauthorized,
            StreamError::Api(api::ApiError::Net(err)) => Self::Net(err.to_string()),
            StreamError::Api(err) => Self::Other(err.to_string()),
        }
    }
}

/// Откуда брать звук.
#[derive(Debug, Clone)]
pub enum Source {
    /// Трек Яндекса (сам, копия или замена FckCensor).
    Track(String),
    /// Прямая ссылка из хранилищ списка FckCensor.
    Url(String),
    /// Сначала искать версию без цензуры (FckCensor), иначе сам трек.
    Search(Box<Track>),
}

/// Задание на один трек. Собирается окном: что показано и что играть.
#[derive(Debug, Clone)]
pub struct Job {
    /// Номер трека, который выбрал пользователь.
    pub id: String,
    pub source: Source,
    /// «Исполнитель — Название» для сообщений.
    pub label: String,
    /// Имя файла без расширения.
    pub stem: String,
    pub tags: tags::Tags,
    /// Шаблон адреса обложки (`%%` вместо размера).
    pub cover: Option<String>,
}

/// Всё общее для заданий. Токена в `Debug` нет.
pub struct Context {
    pub http: reqwest::Client,
    pub audio_http: reqwest::Client,
    pub token: String,
    pub quality: AudioQuality,
    pub dir: PathBuf,
}

impl std::fmt::Debug for Context {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Context")
            .field("quality", &self.quality)
            .field("dir", &self.dir)
            .finish_non_exhaustive()
    }
}

/// Итог одного задания.
#[derive(Debug)]
pub enum Outcome {
    /// Файл сохранён; `tagged` — теги записаны.
    Saved {
        tagged: bool,
    },
    /// Такой файл уже есть.
    Exists,
    Failed(DownloadError),
}

/// Выполняет задание.
pub async fn run(ctx: &Context, job: &Job) -> Outcome {
    match run_inner(ctx, job).await {
        Ok(outcome) => outcome,
        Err(err) => Outcome::Failed(err),
    }
}

async fn run_inner(ctx: &Context, job: &Job) -> Result<Outcome, DownloadError> {
    if exists_already(&ctx.dir, &job.stem).await {
        return Ok(Outcome::Exists);
    }
    let resolved = resolve(ctx, &job.source).await;
    let (data, codec) = fetch_audio(ctx, &resolved).await?;
    let (data, format) = blocking(move || prepare(codec, data)).await??;
    // Год и номер, обложка: не вышло — файл просто без них.
    let mut tags = job.tags.clone();
    match api::fetch_track_meta(&ctx.http, &ctx.token, &job.id).await {
        Ok(meta) => {
            tags.year = meta.year;
            tags.number = meta.number;
        }
        Err(api::ApiError::Unauthorized) => return Err(DownloadError::Unauthorized),
        Err(_) => {}
    }
    if let Some(template) = &job.cover {
        tags.cover = fetch_cover(&ctx.http, template).await;
    }
    let (dir, stem) = (ctx.dir.clone(), job.stem.clone());
    let written = blocking(move || write_file(&dir, &stem, format, data, &tags)).await??;
    Ok(match written {
        Written::Saved { tagged } => Outcome::Saved { tagged },
        Written::Exists => Outcome::Exists,
    })
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, DownloadError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| DownloadError::Other("фоновая операция прервалась".to_owned()))
}

/// Есть ли в папке файл с этим именем (с любым из расширений).
async fn exists_already(dir: &Path, stem: &str) -> bool {
    let (dir, stem) = (dir.to_path_buf(), stem.to_owned());
    blocking(move || {
        EXTENSIONS
            .iter()
            .any(|extension| dir.join(name::file_name(&stem, extension)).exists())
    })
    .await
    .unwrap_or(false)
}

/// Что именно качать.
enum Resolved {
    Track(String),
    Url(String),
}

async fn resolve(ctx: &Context, source: &Source) -> Resolved {
    match source {
        Source::Track(id) => Resolved::Track(id.clone()),
        Source::Url(url) => Resolved::Url(url.clone()),
        Source::Search(track) => {
            let search = fckcensor::find_explicit(&ctx.http, &ctx.token, track);
            // Ошибка или долго — берём сам трек, как плеер.
            match tokio::time::timeout(fckcensor::SEARCH_TIMEOUT, search).await {
                Ok(Ok(Some(id))) => Resolved::Track(id),
                _ => Resolved::Track(track.id.clone()),
            }
        }
    }
}

/// Ссылка на файл (или сами байты поддельного потока в отладке).
enum Link {
    Url(String, Option<Codec>),
    #[cfg(debug_assertions)]
    Fake(Vec<u8>, Codec),
}

async fn link(ctx: &Context, resolved: &Resolved) -> Result<Link, DownloadError> {
    match resolved {
        Resolved::Url(url) => Ok(Link::Url(url.clone(), None)),
        Resolved::Track(id) => {
            let source = api::fetch_stream_source(&ctx.http, &ctx.token, id, ctx.quality).await?;
            match source {
                StreamSource::Url { url, codec, .. } => Ok(Link::Url(url, Some(codec))),
                #[cfg(debug_assertions)]
                StreamSource::Fake(name) => Ok(fake_bytes(&name)),
            }
        }
    }
}

/// Поддельный поток отладочного хука `PLASTINKA_TEST_STREAM`: тишина.
#[cfg(debug_assertions)]
fn fake_bytes(name: &str) -> Link {
    use crate::player::fake_audio;
    match name {
        "silence-flac" => Link::Fake(fake_audio(true, 10), Codec::Flac),
        "garbage" | "broken-aac" => Link::Fake(b"<html>garbage</html>".repeat(50), Codec::Aac),
        _ => Link::Fake(fake_audio(false, 10), Codec::Mp3),
    }
}

/// Загрузка в память с докачкой. Каждый повтор получает новую ссылку: старая
/// живёт около минуты.
async fn fetch_audio(
    ctx: &Context,
    resolved: &Resolved,
) -> Result<(Vec<u8>, Option<Codec>), DownloadError> {
    let mut data: Vec<u8> = Vec::new();
    let mut last = String::new();
    for attempt in 0..=RETRIES {
        if let Some(pause) = attempt.checked_sub(1).and_then(|index| BACKOFF.get(index)) {
            tokio::time::sleep(*pause).await;
        }
        let (url, codec) = match link(ctx, resolved).await {
            Ok(Link::Url(url, codec)) => (url, codec),
            #[cfg(debug_assertions)]
            Ok(Link::Fake(bytes, codec)) => {
                tokio::time::sleep(Duration::from_millis(400)).await;
                return Ok((bytes, Some(codec)));
            }
            Err(DownloadError::Net(text)) => {
                last = text;
                continue;
            }
            Err(err) => return Err(err),
        };
        match transfer(&ctx.audio_http, &url, &mut data).await {
            Ok(()) => return Ok((data, codec)),
            Err(Step::Retry(text)) => last = text,
            Err(Step::Fatal(err)) => return Err(err),
        }
    }
    Err(DownloadError::Net(format!(
        "не удалось скачать после {RETRIES} повторов: {last}"
    )))
}

enum Step {
    /// Временный сбой: повторить, докачивая с места обрыва.
    Retry(String),
    Fatal(DownloadError),
}

/// Один запрос: докачивает `data` с её конца.
async fn transfer(http: &reqwest::Client, url: &str, data: &mut Vec<u8>) -> Result<(), Step> {
    let mut request = http.get(url);
    if !data.is_empty() {
        request = request.header(RANGE, format!("bytes={}-", data.len()));
    }
    let mut response = request
        .send()
        .await
        .map_err(|err| Step::Retry(NetError::from(err).to_string()))?;
    let status = response.status();
    match status {
        StatusCode::OK => data.clear(),
        StatusCode::PARTIAL_CONTENT => {
            let start = response
                .headers()
                .get(CONTENT_RANGE)
                .and_then(|value| value.to_str().ok())
                .and_then(range_start);
            if start != Some(data.len() as u64) {
                data.clear();
                return Err(Step::Retry("хранилище ответило не с того места".to_owned()));
            }
        }
        status
            if status.is_server_error()
                || matches!(
                    status,
                    StatusCode::TOO_MANY_REQUESTS
                        | StatusCode::FORBIDDEN
                        | StatusCode::NOT_FOUND
                        | StatusCode::GONE
                        | StatusCode::UNAUTHORIZED
                ) =>
        {
            return Err(Step::Retry(format!(
                "хранилище не отдало файл (код {})",
                status.as_u16()
            )));
        }
        status => {
            return Err(Step::Fatal(DownloadError::Net(format!(
                "неожиданный ответ хранилища (код {})",
                status.as_u16()
            ))));
        }
    }
    let kind = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if kind.starts_with("text/") || kind.contains("json") || kind.contains("xml") {
        return Err(Step::Fatal(DownloadError::Format));
    }
    let expected = response
        .content_length()
        .map(|length| (data.len() as u64).saturating_add(length));
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if data.len().saturating_add(chunk.len()) > MAX_BYTES {
                    return Err(Step::Fatal(DownloadError::Other(
                        "файл слишком большой".to_owned(),
                    )));
                }
                data.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(err) => return Err(Step::Retry(NetError::from(err).to_string())),
        }
    }
    match expected {
        Some(expected) if (data.len() as u64) < expected => {
            Err(Step::Retry(NetError::Interrupted.to_string()))
        }
        _ if data.is_empty() => Err(Step::Retry("хранилище прислало пустой файл".to_owned())),
        _ => Ok(()),
    }
}

/// Начало диапазона из `bytes 100-199/1000`.
fn range_start(value: &str) -> Option<u64> {
    let rest = value.trim().strip_prefix("bytes ")?;
    let (range, _) = rest.split_once('/')?;
    let (first, _) = range.split_once('-')?;
    first.trim().parse().ok()
}

/// Обложка из шаблона Яндекса: только разрешённые хосты, только JPEG и PNG.
async fn fetch_cover(http: &reqwest::Client, template: &str) -> Option<tags::Cover> {
    let url = crate::ui::covers::cover_url(template, COVER_SIZE)?;
    let response = http.get(url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let bytes = net::read_limited(response, MAX_COVER).await.ok()?;
    tags::Cover::from_bytes(bytes)
}

/// Байты для записи и их вид. FLAC из mp4 пересобирается в обычный FLAC.
fn prepare(codec: Option<Codec>, data: Vec<u8>) -> Result<(Vec<u8>, Format), DownloadError> {
    match (sniff(&data), codec) {
        (Some(Container::Flac), _) => Ok((data, Format::Flac)),
        (Some(Container::Mp4), Some(Codec::Flac)) => Ok((flac::from_mp4(data)?, Format::Flac)),
        (Some(Container::Mp4), _) => Ok((data, Format::M4a)),
        (Some(Container::Mp3), _) => Ok((data, Format::Mp3)),
        (Some(Container::Adts), _) => Ok((data, Format::Aac)),
        (None, _) => Err(DownloadError::Format),
    }
}

enum Written {
    Saved { tagged: bool },
    Exists,
}

/// Теги, временный `.part`, переименование. Ошибка — `.part` удалён.
fn write_file(
    dir: &Path,
    stem: &str,
    format: Format,
    data: Vec<u8>,
    tags: &tags::Tags,
) -> Result<Written, DownloadError> {
    let file = name::file_name(stem, format.extension());
    let target = dir.join(&file);
    let part = dir.join(format!("{file}.part"));
    if target.exists() {
        return Ok(Written::Exists);
    }
    let (bytes, tagged) = tags::apply(format, data, tags);
    let result = std::fs::write(&part, &bytes).and_then(|()| {
        // `rename` в Unix затёр бы чужой файл, появившийся за это время.
        if target.exists() {
            return Err(std::io::ErrorKind::AlreadyExists.into());
        }
        std::fs::rename(&part, &target)
    });
    match result {
        Ok(()) => Ok(Written::Saved { tagged }),
        Err(err) => {
            let _ = std::fs::remove_file(&part);
            if err.kind() == std::io::ErrorKind::AlreadyExists {
                Ok(Written::Exists)
            } else {
                Err(DownloadError::Disk(err.to_string()))
            }
        }
    }
}

/// Папка пригодна: есть, это папка, в неё можно писать. Проверка записью
/// пустого временного файла, который тут же удаляется.
pub fn check_dir(dir: &Path) -> Result<(), String> {
    if !dir.is_dir() {
        return Err("папка не найдена".to_owned());
    }
    let probe = dir.join(format!(".plastinka-{}.part", std::process::id()));
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(err) => Err(format!("в папку нельзя записывать: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("plastinka-dl-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn mp3_bytes() -> Vec<u8> {
        let mut data = Vec::new();
        for _ in 0..20 {
            data.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x00]);
            data.resize(data.len() + 413, 0);
        }
        data
    }

    #[test]
    fn sniffs_containers() {
        assert_eq!(sniff(b"fLaC\0\0"), Some(Container::Flac));
        assert_eq!(sniff(b"\0\0\0\x18ftypM4A "), Some(Container::Mp4));
        assert_eq!(sniff(b"ID3\x04\0"), Some(Container::Mp3));
        assert_eq!(sniff(&[0xFF, 0xFB, 0x90, 0]), Some(Container::Mp3));
        assert_eq!(sniff(&[0xFF, 0xF1, 0x50, 0x80]), Some(Container::Adts));
        assert_eq!(sniff(b"<html>"), None);
        assert_eq!(sniff(b""), None);
    }

    #[test]
    fn extension_follows_content() {
        let (_, format) = prepare(Some(Codec::Mp3), mp3_bytes()).unwrap();
        assert_eq!(format.extension(), "mp3");
        let (_, format) = prepare(Some(Codec::Aac), b"\0\0\0\x18ftypM4A ".to_vec()).unwrap();
        assert_eq!(format.extension(), "m4a");
        let (_, format) = prepare(Some(Codec::Flac), b"fLaC....".to_vec()).unwrap();
        assert_eq!(format.extension(), "flac");
        assert!(prepare(Some(Codec::Mp3), b"<html>".to_vec()).is_err());
        // FLAC в mp4 без дорожки FLAC не сохраняем «как есть» под видом .flac.
        assert!(prepare(Some(Codec::Flac), b"\0\0\0\x18ftypM4A ".to_vec()).is_err());
    }

    #[test]
    fn range_start_is_parsed() {
        assert_eq!(range_start("bytes 100-199/1000"), Some(100));
        assert_eq!(range_start("bytes */1000"), None);
    }

    #[test]
    fn writes_file_via_part_and_skips_existing() {
        let dir = temp_dir("write");
        let tags = tags::Tags {
            title: "Название".to_owned(),
            artist: "Исполнитель".to_owned(),
            ..tags::Tags::default()
        };
        let stem = name::file_stem("Исполнитель", "Название");
        let written = write_file(&dir, &stem, Format::Mp3, mp3_bytes(), &tags).unwrap();
        assert!(matches!(written, Written::Saved { tagged: true }));
        let file = dir.join(name::file_name(&stem, "mp3"));
        assert!(file.is_file());
        // Второй раз — «уже скачан», файл не тронут.
        let before = std::fs::read(&file).unwrap();
        let again = write_file(&dir, &stem, Format::Mp3, mp3_bytes(), &tags).unwrap();
        assert!(matches!(again, Written::Exists));
        assert_eq!(std::fs::read(&file).unwrap(), before);
        // В папке один файл: `.part` нет.
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn failed_write_leaves_no_part() {
        let dir = temp_dir("fail");
        let missing = dir.join("нет такой");
        let result = write_file(
            &missing,
            "A — B",
            Format::Mp3,
            mp3_bytes(),
            &tags::Tags::default(),
        );
        assert!(matches!(result, Err(DownloadError::Disk(_))));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn existing_file_of_any_format_counts() {
        let dir = temp_dir("exists");
        std::fs::write(dir.join("A — B.m4a"), b"x").unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        assert!(runtime.block_on(exists_already(&dir, "A — B")));
        assert!(!runtime.block_on(exists_already(&dir, "A — C")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn check_dir_accepts_writable_and_rejects_missing() {
        let dir = temp_dir("check");
        assert!(check_dir(&dir).is_ok());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        assert!(check_dir(&dir.join("нет")).is_err());
        let file = dir.join("файл");
        std::fs::write(&file, b"x").unwrap();
        assert!(check_dir(&file).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
