//! FckCensor: названия, обложки и исполнители без цензуры из списка
//! `Hazzz895/FckCensorData` (`list_v2.json`) и звук без цензуры — из
//! хранилищ списка, с Яндекса или из файла, который выбрал сам пользователь.
//!
//! Откуда берётся звук трека, по порядку ([`Censor::sound`]):
//!
//! 1. ручная замена пользователя — трек Яндекса или свой файл;
//! 2. `__fckCensor.audioSourceId` из списка — трек Яндекса;
//! 3. прямая ссылка из хранилищ списка (`tracksStorages`): файлы без цензуры
//!    лежат в том же репозитории, что и список, `urlTemplate` с `%%` —
//!    подстановка номера файла, как в оригинальном аддоне;
//! 4. версия того же трека с `contentWarning: explicit`, найденная поиском;
//! 5. сам трек.
//!
//! Ссылки хранилищ — только https. Формат и источники — `docs/api/stage-7.5.md`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{may_write, read_limited, stale, unix_now};
use crate::api::{self, AlbumCard, ApiError, ArtistRef, Found, SearchKind, Track, Warning};
use crate::net::NetError;
use crate::player::Sound;
use crate::storage::{AppDirs, StorageError};

/// Список без цензуры.
pub const LIST_URL: &str =
    "https://raw.githubusercontent.com/Hazzz895/FckCensorData/main/list_v2.json";
/// Страница проекта списка: ссылка в настройках.
pub const PROJECT_URL: &str = "https://github.com/Hazzz895/FckCensorData";
/// Больше списка не читаем: сейчас он около 110 КБ.
const LIST_LIMIT: usize = 4 * 1024 * 1024;
/// Кэш списка старше — обновляем в фоне.
pub const LIST_FRESH: Duration = Duration::from_secs(6 * 3600);
/// Сколько помнить результат автопоиска.
const EXPLICIT_FRESH: Duration = Duration::from_secs(30 * 24 * 3600);
/// Сколько ждать автопоиска, прежде чем играть сам трек.
pub const SEARCH_TIMEOUT: Duration = Duration::from_secs(10);
/// Разница длительностей, при которой трек ещё считается тем же.
const DURATION_SLACK: Duration = Duration::from_secs(10);
/// Файлы в кэше плагинов.
const LIST_FILE: &str = "fckcensor-list.json";
const EXPLICIT_FILE: &str = "fckcensor-explicit.json";
/// Файл ручных замен в каталоге настроек.
const REPLACEMENTS_FILE: &str = "replacements.toml";
/// Расширения своих файлов, которые умеет плеер.
pub const AUDIO_EXTENSIONS: [&str; 7] = ["mp3", "flac", "ogg", "oga", "wav", "m4a", "aac"];

/// Ошибки списка. Тексты — для окна.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ListError {
    #[error("{0}")]
    Net(String),
    #[error("GitHub ответил кодом {0}")]
    Status(u16),
    #[error("список слишком большой")]
    TooLarge,
    #[error("список повреждён или в непонятном формате")]
    Broken,
}

impl From<NetError> for ListError {
    fn from(err: NetError) -> Self {
        Self::Net(err.to_string())
    }
}

/// Что список меняет у трека.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrackPatch {
    pub title: Option<String>,
    pub artists: Option<Vec<ArtistRef>>,
    pub cover_uri: Option<String>,
    /// Трек Яндекса, у которого брать звук (`audioSourceId`).
    pub sound: Option<String>,
}

/// Что список меняет у альбома.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AlbumPatch {
    pub title: Option<String>,
    pub artists: Option<Vec<ArtistRef>>,
    pub cover_uri: Option<String>,
    /// Состав альбома: номера треков Яндекса по порядку (диски подряд).
    pub volumes: Vec<String>,
    /// Исполнители треков альбома = исполнители альбома.
    pub replace_artists: bool,
}

/// Разобранный список.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CensorList {
    pub tracks: HashMap<String, TrackPatch>,
    pub albums: HashMap<String, AlbumPatch>,
    /// Треки, у которых на Яндексе цензурная версия.
    pub censored: HashSet<String>,
    /// Прямые ссылки на звук без цензуры из хранилищ списка: номер трека →
    /// https-адрес файла (репозиторий списка).
    pub urls: HashMap<String, String>,
}

impl CensorList {
    /// Трек после подмены. `None` — подменять нечего.
    pub fn patch(&self, track: &Track) -> Option<Track> {
        let patch = self.tracks.get(&track.id)?;
        if patch.title.is_none() && patch.artists.is_none() && patch.cover_uri.is_none() {
            return None;
        }
        let mut patched = track.clone();
        if let Some(title) = &patch.title {
            patched.title = Some(title.clone());
            // Версия («Clean», «Radio edit») относится к цензурному названию.
            patched.version = None;
        }
        if let Some(artists) = &patch.artists {
            patched.artists = artists.clone();
        }
        if let Some(cover) = &patch.cover_uri {
            patched.cover_uri = Some(cover.clone());
        }
        Some(patched)
    }

    /// Карточка альбома после подмены.
    pub fn patch_album(&self, card: &AlbumCard) -> Option<AlbumCard> {
        let patch = self.albums.get(&card.id)?;
        let mut patched = card.clone();
        if let Some(title) = &patch.title {
            patched.title = title.clone();
            patched.version = None;
        }
        if let Some(artists) = &patch.artists {
            patched.artists = artists.clone();
        }
        if let Some(cover) = &patch.cover_uri {
            patched.cover_uri = Some(cover.clone());
        }
        Some(patched)
    }

    /// Трек Яндекса, у которого брать звук, по списку.
    pub fn sound_of(&self, id: &str) -> Option<&str> {
        self.tracks.get(id)?.sound.as_deref()
    }

    /// Прямая ссылка на звук без цензуры из хранилищ списка.
    pub fn replacement_url(&self, id: &str) -> Option<&str> {
        self.urls.get(id).map(String::as_str)
    }

    /// Нужна ли треку версия без цензуры.
    pub fn censored(&self, track: &Track) -> bool {
        track.warning != Warning::Explicit
            && (track.warning == Warning::Clean
                || self.censored.contains(&track.id)
                || self.tracks.contains_key(&track.id))
    }
}

/// Разбирает `list_v2.json`. Непонятные записи пропускаются.
pub fn parse_list(body: &[u8]) -> Result<CensorList, ListError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| ListError::Broken)?;
    let sources = value
        .get("sources")
        .and_then(Value::as_array)
        .ok_or(ListError::Broken)?;
    let mut list = CensorList::default();
    for source in sources {
        if let Some(tracks) = source.get("tracks").and_then(Value::as_object) {
            for (id, track) in tracks {
                let Some(id) = list_id(id) else { continue };
                if !track.is_object() {
                    continue;
                }
                list.tracks.insert(id, track_patch(track));
            }
        }
        if let Some(albums) = source.get("albums").and_then(Value::as_object) {
            for (id, album) in albums {
                let Some(id) = list_id(id) else { continue };
                if !album.is_object() {
                    continue;
                }
                list.albums.insert(id, album_patch(album));
            }
        }
        // Хранилища звука: прямые ссылки на файлы без цензуры (как в
        // оригинальном аддоне) и номера треков как признак цензуры.
        for storage in source
            .get("tracksStorages")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let ids: Vec<String> = storage
                .get("trackIds")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|item| api::numeric_id(item.get("id").unwrap_or(item)))
                .collect();
            let keys: Vec<String> = storage
                .get("tracks")
                .and_then(Value::as_object)
                .into_iter()
                .flat_map(|tracks| tracks.keys())
                .filter_map(|key| list_id(key))
                .collect();
            list.censored.extend(ids.iter().cloned().chain(keys.iter().cloned()));

            let template = storage
                .get("urlTemplate")
                .and_then(Value::as_str)
                .and_then(storage_template);
            let Some(template) = template else {
                continue;
            };
            // `trackIds`: файл звука — шаблон с номером самого трека.
            for id in ids {
                list.urls.insert(id.clone(), template.replace("%%", &id));
            }
            // `tracks`: номер трека → номер файла, готовая ссылка или объект
            // с полем `url`.
            if let Some(files) = storage.get("tracks").and_then(Value::as_object) {
                for (key, value) in files {
                    let Some(id) = list_id(key) else {
                        continue;
                    };
                    if let Some(url) = file_url(value, &template) {
                        list.urls.insert(id, url);
                    }
                }
            }
        }
    }
    Ok(list)
}

fn list_id(key: &str) -> Option<String> {
    api::numeric_id(&Value::String(key.to_owned()))
}

/// Шаблон ссылки хранилища: обязателен `%%`, без схемы подразумевается
/// https (хранилище списка живёт на GitHub).
fn storage_template(raw: &str) -> Option<String> {
    if raw.len() > 1800
        || !raw.contains("%%")
        || raw.chars().any(|ch| ch.is_whitespace() || ch.is_control())
    {
        return None;
    }
    let url = if raw.contains("://") {
        raw.to_owned()
    } else {
        format!("https://{raw}")
    };
    plausible_url(&url).then_some(url)
}

/// Адрес файла звука из значения хранилища: номер или строка без схемы —
/// подстановка в шаблон, готовая ссылка — как есть.
fn file_url(value: &Value, template: &str) -> Option<String> {
    let raw: String = match value {
        Value::Number(number) => number.to_string(),
        Value::String(text) => text.clone(),
        Value::Object(fields) => match fields.get("url")? {
            Value::Number(number) => number.to_string(),
            Value::String(text) => text.clone(),
            _ => return None,
        },
        _ => return None,
    };
    if raw.chars().any(|ch| ch.is_whitespace() || ch.is_control()) || raw.len() > 1800 {
        return None;
    }
    let url = if raw.contains("://") {
        raw
    } else {
        template.replace("%%", &raw)
    };
    plausible_url(&url).then_some(url)
}

/// Ссылка хранилища: только https, разумной длины, без мусора.
fn plausible_url(url: &str) -> bool {
    url.starts_with("https://")
        && url.len() <= 2000
        && !url.chars().any(|ch| ch.is_whitespace() || ch.is_control())
}

/// Адрес обложки из списка: шаблон без схемы, с `%%`.
fn cover(value: Option<&Value>) -> Option<String> {
    let uri = value.and_then(Value::as_str).and_then(api::clean)?;
    (!uri.contains("://") && uri.contains("%%")).then_some(uri)
}

fn artists(value: Option<&Value>) -> Option<Vec<ArtistRef>> {
    let artists = api::parse_artist_refs(value);
    (!artists.is_empty()).then_some(artists)
}

fn text(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).and_then(api::clean)
}

fn track_patch(value: &Value) -> TrackPatch {
    let spoof = value.get("__fckCensor");
    TrackPatch {
        title: text(value.get("title")),
        artists: artists(value.get("artists")),
        cover_uri: cover(value.get("coverUri")),
        sound: spoof
            .and_then(|spoof| spoof.get("audioSourceId"))
            .and_then(api::numeric_id),
    }
}

fn album_patch(value: &Value) -> AlbumPatch {
    let volumes = value
        .get("volumes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_array)
        .flatten()
        .filter_map(|track| api::numeric_id(track.get("id").unwrap_or(track)))
        .collect();
    AlbumPatch {
        title: text(value.get("title")),
        artists: artists(value.get("artists")),
        cover_uri: cover(value.get("coverUri")),
        volumes,
        replace_artists: value
            .get("__fckCensor")
            .and_then(|spoof| spoof.get("replaceArtistsInAlbumVolumes"))
            .and_then(Value::as_bool)
            == Some(true),
    }
}

// ------------------------------------------------------------ загрузка списка

/// Список в кэше: тело ответа, `ETag`, когда получен.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedList {
    pub body: String,
    #[serde(default)]
    pub etag: Option<String>,
    pub fetched: u64,
}

/// Готовый список и когда он получен.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub list: Arc<CensorList>,
    pub fetched: u64,
    /// Обновить не удалось, работает кэш: текст ошибки.
    pub stale: Option<String>,
}

fn cache_path(name: &str) -> Option<PathBuf> {
    super::cache_dir().map(|dir| dir.join(name))
}

/// Список из кэша, если он есть и читается.
pub async fn cached() -> Option<(Loaded, CachedList)> {
    #[cfg(debug_assertions)]
    if super::test_hook().is_some() {
        return None;
    }
    let path = cache_path(LIST_FILE)?;
    let cached: CachedList = crate::storage::blocking(move || {
        let text = std::fs::read_to_string(&path).map_err(|_| StorageError::Interrupted)?;
        serde_json::from_str(&text).map_err(|_| StorageError::Interrupted)
    })
    .await
    .ok()?;
    let list = parse_list(cached.body.as_bytes()).ok()?;
    Some((
        Loaded {
            list: Arc::new(list),
            fetched: cached.fetched,
            stale: None,
        },
        cached,
    ))
}

/// Свежий список с GitHub. `cache` — что уже есть: с его `ETag` ответ 304
/// означает «не изменился». Новый список кладётся в кэш.
pub async fn refresh(
    http: reqwest::Client,
    cache: Option<CachedList>,
) -> Result<Loaded, ListError> {
    #[cfg(debug_assertions)]
    if let Some(name) = super::test_hook() {
        return test_list(&name);
    }
    let mut request = http.get(LIST_URL);
    if let Some(etag) = cache.as_ref().and_then(|cache| cache.etag.as_deref()) {
        request = request.header(reqwest::header::IF_NONE_MATCH, etag);
    }
    let response = request.send().await.map_err(NetError::from)?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_MODIFIED
        && let Some(mut cache) = cache
    {
        let list = parse_list(cache.body.as_bytes())?;
        cache.fetched = unix_now();
        let fetched = cache.fetched;
        store_list(cache).await;
        return Ok(Loaded {
            list: Arc::new(list),
            fetched,
            stale: None,
        });
    }
    if !status.is_success() {
        return Err(ListError::Status(status.as_u16()));
    }
    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .filter(|etag| etag.len() <= 200 && etag.is_ascii())
        .map(str::to_owned);
    let body = read_limited(response, LIST_LIMIT)
        .await?
        .ok_or(ListError::TooLarge)?;
    let list = parse_list(&body)?;
    let fetched = unix_now();
    if let Ok(body) = String::from_utf8(body) {
        store_list(CachedList {
            body,
            etag,
            fetched,
        })
        .await;
    }
    Ok(Loaded {
        list: Arc::new(list),
        fetched,
        stale: None,
    })
}

async fn store_list(cache: CachedList) {
    if !may_write() {
        return;
    }
    let Some(path) = cache_path(LIST_FILE) else {
        return;
    };
    // Кэш не записался — не беда: в следующий раз скачаем заново.
    let _ = crate::storage::blocking(move || {
        let text = serde_json::to_string(&cache).map_err(|_| StorageError::Interrupted)?;
        write_atomic(&path, &text)
    })
    .await;
}

/// Запись через временный файл: при сбое не остаётся обрезанный файл.
fn write_atomic(path: &Path, text: &str) -> Result<(), StorageError> {
    let io_error = |source| StorageError::ConfigIo {
        path: path.to_path_buf(),
        source,
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io_error)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text).map_err(io_error)?;
    std::fs::rename(&tmp, path).map_err(io_error)
}

// ------------------------------------------------------------ автопоиск

/// Название для сравнения: нижний регистр, «ё» как «е», только буквы,
/// цифры и звёздочки цензуры («с*ка») через пробел.
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = true;
    for ch in text.chars().flat_map(char::to_lowercase) {
        let ch = if ch == 'ё' { 'е' } else { ch };
        if ch.is_alphanumeric() || ch == '*' {
            out.push(ch);
            space = false;
        } else if !space && (ch.is_whitespace() || ch == '-' || ch == '—') {
            out.push(' ');
            space = true;
        }
    }
    out.trim_end().to_owned()
}

fn same_artist(a: &[ArtistRef], b: &[ArtistRef]) -> bool {
    a.iter().any(|left| {
        b.iter().any(|right| match (&left.id, &right.id) {
            (Some(x), Some(y)) => x == y,
            _ => normalize(&left.name) == normalize(&right.name),
        })
    })
}

/// Названия совпадают или одно начинается с другого. Звёздочка цензуры
/// совпадает с любой одной буквой: «м*лния» и «молния» — одно слово.
fn same_title(a: &str, b: &str) -> bool {
    let (a, b) = (normalize(a), normalize(b));
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let same = |x: char, y: char| x == y || (x == '*' && y != ' ') || (y == '*' && x != ' ');
    // Сравниваем по общей длине: так проверяется и «начинается с».
    a.chars().zip(b.chars()).all(|(x, y)| same(x, y))
}

/// Версия трека `original` без цензуры среди `candidates`: explicit, тот же
/// исполнитель, похожее название, длительность в пределах 10 с. Из
/// подходящих — с ближайшей длительностью.
pub fn pick_explicit<'a>(original: &Track, candidates: &'a [Track]) -> Option<&'a Track> {
    let title = original.title.as_deref()?;
    candidates
        .iter()
        .filter(|candidate| {
            candidate.warning == Warning::Explicit
                && candidate.available
                && candidate.id != original.id
                && same_artist(&original.artists, &candidate.artists)
                && candidate
                    .title
                    .as_deref()
                    .is_some_and(|other| same_title(title, other))
        })
        .filter_map(|candidate| {
            let gap = match (original.duration, candidate.duration) {
                (Some(a), Some(b)) => a.abs_diff(b),
                _ => Duration::ZERO,
            };
            (gap <= DURATION_SLACK).then_some((gap, candidate))
        })
        .min_by_key(|(gap, _)| *gap)
        .map(|(_, candidate)| candidate)
}

/// Ищет на Яндексе версию трека без цензуры. `Ok(None)` — такой нет.
pub async fn find_explicit(
    http: &reqwest::Client,
    token: &str,
    original: &Track,
) -> Result<Option<String>, ApiError> {
    let artist = original
        .artists
        .first()
        .map(|artist| artist.name.as_str())
        .unwrap_or_default();
    let Some(title) = original.title.as_deref() else {
        return Ok(None);
    };
    let query = format!("{artist} {title}");
    let found = api::search(http, token, &query, SearchKind::Tracks, 0).await?;
    let Found::Tracks(page) = found.found else {
        return Ok(None);
    };
    Ok(pick_explicit(original, &page.items).map(|track| track.id.clone()))
}

/// Найденные версии без цензуры: номер трека → номер explicit-трека или
/// «нет такой», и когда искали.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExplicitCache {
    #[serde(default)]
    pub found: HashMap<String, (Option<String>, u64)>,
}

impl ExplicitCache {
    /// Результат поиска, если он ещё свежий.
    pub fn get(&self, id: &str) -> Option<Option<&str>> {
        let (found, at) = self.found.get(id)?;
        (!stale(*at, EXPLICIT_FRESH)).then_some(found.as_deref())
    }

    pub fn insert(&mut self, id: String, found: Option<String>) {
        self.found.insert(id, (found, unix_now()));
    }
}

pub async fn load_explicit() -> ExplicitCache {
    #[cfg(debug_assertions)]
    if super::test_hook().is_some() {
        return ExplicitCache::default();
    }
    let Some(path) = cache_path(EXPLICIT_FILE) else {
        return ExplicitCache::default();
    };
    crate::storage::blocking(move || {
        let text = std::fs::read_to_string(&path).map_err(|_| StorageError::Interrupted)?;
        let mut cache: ExplicitCache =
            serde_json::from_str(&text).map_err(|_| StorageError::Interrupted)?;
        cache.found.retain(|id, (found, at)| {
            api::plausible_id(id)
                && found.as_deref().is_none_or(api::plausible_id)
                && !stale(*at, EXPLICIT_FRESH)
        });
        Ok(cache)
    })
    .await
    .unwrap_or_default()
}

pub async fn save_explicit(cache: ExplicitCache) {
    if !may_write() {
        return;
    }
    let Some(path) = cache_path(EXPLICIT_FILE) else {
        return;
    };
    let _ = crate::storage::blocking(move || {
        let text = serde_json::to_string(&cache).map_err(|_| StorageError::Interrupted)?;
        write_atomic(&path, &text)
    })
    .await;
}

// ------------------------------------------------------------ ручные замены

/// Замена звука трека, которую выбрал пользователь.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Replacement {
    /// Трек Яндекса.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<String>,
    /// Подпись трека Яндекса: «Исполнитель — Название».
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Свой файл на компьютере.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
}

impl Replacement {
    pub fn track(track: &Track) -> Self {
        Self {
            track: Some(track.id.clone()),
            title: Some(format!(
                "{} — {}",
                track.artists_text(),
                track.display_title()
            )),
            file: None,
        }
    }

    pub fn file(path: PathBuf) -> Self {
        Self {
            track: None,
            title: None,
            file: Some(path),
        }
    }

    /// Звук замены. Своё приоритетнее: файл, потом трек.
    pub fn sound(&self) -> Option<Sound> {
        if let Some(file) = &self.file {
            return Some(Sound::File(file.clone()));
        }
        self.track
            .as_ref()
            .filter(|id| api::plausible_id(id))
            .map(|id| Sound::Track(id.clone()))
    }

    /// Подпись для меню.
    pub fn label(&self) -> String {
        match (&self.file, &self.title) {
            (Some(file), _) => file.file_name().map_or_else(
                || file.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            ),
            (None, Some(title)) => title.clone(),
            (None, None) => self.track.clone().unwrap_or_default(),
        }
    }
}

/// Ручные замены: номер трека → замена. Файл `replacements.toml` в
/// каталоге настроек.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Replacements {
    #[serde(default)]
    pub tracks: HashMap<String, Replacement>,
}

fn replacements_path() -> Result<PathBuf, StorageError> {
    Ok(AppDirs::discover()?.config_dir().join(REPLACEMENTS_FILE))
}

impl Replacements {
    /// Читает замены. Файла нет — замен нет. Битые записи пропускаются.
    pub async fn load() -> Result<Self, StorageError> {
        #[cfg(debug_assertions)]
        if crate::storage::test_hooks_active() {
            return Ok(Self::default());
        }
        let path = replacements_path()?;
        crate::storage::blocking(move || {
            let text = match std::fs::read_to_string(&path) {
                Ok(text) => text,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(Self::default());
                }
                Err(source) => return Err(StorageError::ConfigIo { path, source }),
            };
            let value: toml::Value =
                toml::from_str(&text).map_err(|err| StorageError::ConfigParse {
                    path: path.clone(),
                    line: None,
                    message: err.message().to_owned(),
                })?;
            let mut replacements = Self::default();
            if let Some(tracks) = value.get("tracks").and_then(toml::Value::as_table) {
                for (id, item) in tracks {
                    let Ok(replacement) = item.clone().try_into::<Replacement>() else {
                        continue;
                    };
                    if api::plausible_id(id) && replacement.sound().is_some() {
                        replacements.tracks.insert(id.clone(), replacement);
                    }
                }
            }
            Ok(replacements)
        })
        .await
    }

    /// Пишет замены. С тестовыми хуками не пишет.
    pub async fn save(self) -> Result<(), StorageError> {
        if !may_write() {
            return Ok(());
        }
        let path = replacements_path()?;
        crate::storage::blocking(move || {
            let text = toml::to_string(&self)?;
            write_atomic(&path, &text)
        })
        .await
    }
}

/// Номер трека из ссылки на Яндекс Музыку: `…/album/1/track/2`,
/// `…/track/2`, с доменами `music.yandex.ru|com|by|kz|uz`. Не ссылка —
/// `None`.
pub fn track_from_link(text: &str) -> Option<String> {
    let text = text.trim();
    let rest = text
        .strip_prefix("https://")
        .or_else(|| text.strip_prefix("http://"))
        .unwrap_or(text);
    let (host, path) = rest.split_once('/')?;
    let host = host.strip_prefix("www.").unwrap_or(host);
    let known = ["ru", "com", "by", "kz", "uz"]
        .iter()
        .any(|zone| host == format!("music.yandex.{zone}"));
    if !known {
        return None;
    }
    let path = path.split(['?', '#']).next().unwrap_or_default();
    let mut parts = path.split('/').filter(|part| !part.is_empty());
    while let Some(part) = parts.next() {
        if part == "track" {
            let id = parts.next()?;
            return api::plausible_id(id).then(|| id.to_owned());
        }
    }
    None
}

// ------------------------------------------------------------ состояние

/// Состояние списка в окне.
#[derive(Debug, Clone, Default)]
pub enum ListState {
    #[default]
    Off,
    Loading,
    Ready(Loaded),
    Failed(String),
}

/// Что играть вместо трека.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Play(Sound),
    /// Сначала поискать версию без цензуры.
    Search,
    /// Звука нет: трек недоступен и замены нет.
    Silent,
}

/// FckCensor в окне: список, ручные замены, найденные версии.
#[derive(Debug, Default)]
pub struct Censor {
    pub state: ListState,
    /// Список, пока обновляется (кэш).
    cache: Option<CachedList>,
    pub replacements: Replacements,
    pub explicit: ExplicitCache,
    /// Подменённые треки по номеру: виды берут их вместо исходных.
    pub patched: HashMap<String, Track>,
}

impl Censor {
    pub fn list(&self) -> Option<&Arc<CensorList>> {
        match &self.state {
            ListState::Ready(loaded) => Some(&loaded.list),
            _ => None,
        }
    }

    /// Список пришёл: из кэша или с GitHub.
    pub fn loaded(&mut self, loaded: Loaded, cache: Option<CachedList>) {
        if cache.is_some() {
            self.cache = cache;
        }
        self.patched.clear();
        self.state = ListState::Ready(loaded);
    }

    /// Обновить не удалось. Есть список — работаем по нему.
    pub fn failed(&mut self, error: String) {
        match &mut self.state {
            ListState::Ready(loaded) => loaded.stale = Some(error),
            state => *state = ListState::Failed(error),
        }
    }

    pub fn cache(&self) -> Option<CachedList> {
        self.cache.clone()
    }

    /// Запомнить подмены для новых треков.
    pub fn note<'a>(&mut self, tracks: impl IntoIterator<Item = &'a Track>) {
        let Some(list) = self.list().cloned() else {
            return;
        };
        for track in tracks {
            if !self.patched.contains_key(&track.id)
                && let Some(patched) = list.patch(track)
            {
                self.patched.insert(track.id.clone(), patched);
            }
        }
    }

    /// Можно ли включить трек: доступен сам или есть замена.
    pub fn playable(&self, track: &Track) -> bool {
        track.available
            || self.replacements.tracks.contains_key(&track.id)
            || self
                .list()
                .is_some_and(|list| list.sound_of(&track.id).is_some())
            || self
                .list()
                .is_some_and(|list| list.replacement_url(&track.id).is_some())
    }

    /// Откуда брать звук трека (см. начало модуля).
    pub fn sound(&self, track: &Track, explicit_search: bool) -> Decision {
        if let Some(sound) = self
            .replacements
            .tracks
            .get(&track.id)
            .and_then(Replacement::sound)
        {
            return Decision::Play(sound);
        }
        let list = self.list();
        if let Some(id) = list.and_then(|list| list.sound_of(&track.id)) {
            return Decision::Play(Sound::Track(id.to_owned()));
        }
        // Звук без цензуры из хранилищ списка: прямая ссылка на файл.
        if let Some(url) = list.and_then(|list| list.replacement_url(&track.id)) {
            return Decision::Play(Sound::Url(url.to_owned()));
        }
        if !track.available {
            return Decision::Silent;
        }
        let wanted = explicit_search
            && (track.warning == Warning::Clean || list.is_some_and(|list| list.censored(track)));
        if wanted {
            return match self.explicit.get(&track.id) {
                Some(Some(id)) => Decision::Play(Sound::Track(id.to_owned())),
                Some(None) => Decision::Play(Sound::Track(track.id.clone())),
                None => Decision::Search,
            };
        }
        Decision::Play(Sound::Track(track.id.clone()))
    }
}

/// Готовые списки для хука `PLASTINKA_TEST_PLUGINS`.
#[cfg(debug_assertions)]
fn test_list(name: &str) -> Result<Loaded, ListError> {
    match name {
        "offline" => Err(ListError::Net(NetError::Offline.to_string())),
        "broken" => Err(ListError::Broken),
        _ => Ok(Loaded {
            list: Arc::new(parse_list(SAMPLE_LIST.as_bytes())?),
            fetched: unix_now(),
            stale: None,
        }),
    }
}

/// Пример списка: подмены для треков из `PLASTINKA_TEST_LIKES=sample`.
#[cfg(any(test, debug_assertions))]
pub const SAMPLE_LIST: &str = r#"{
  "sources": [
    {
      "supportedVersions": ">=2.0.0",
      "tracks": {
        "29238706": { "title": "Дурак и молния (без цензуры)", "durationMs": 1 },
        "7019818": {
          "artists": [{ "id": 5, "name": "Солистка без цензуры" }],
          "__fckCensor": { "audioSourceId": "1" }
        },
        "2": { "title": "Снят с каталога", "__fckCensor": { "audioSourceId": "29238706" } },
        "bad-id": { "title": "пропустить" },
        "3": "не объект"
      },
      "albums": {
        "10101": {
          "title": "Камнем по голове (без цензуры)",
          "volumes": [[{ "id": 29238706 }, { "id": "7019818" }, { "id": "x" }]],
          "__fckCensor": { "replaceArtistsInAlbumVolumes": true },
          "artists": [{ "id": 41052, "name": "Король и Шут" }]
        }
      },
      "tracksStorages": [
        { "urlTemplate": "example.invalid/%%", "trackIds": [500000, { "id": 500001, "durationMs": 1 }] }
      ]
    },
    { "tracksStorages": [{ "urlTemplate": "example.invalid/%%", "tracks": { "500002": 1 } }] }
  ]
}"#;
#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: &str, title: &str, artist: (&str, &str), seconds: u64, warning: Warning) -> Track {
        Track {
            id: id.to_owned(),
            title: Some(title.to_owned()),
            version: None,
            artists: vec![ArtistRef {
                id: (!artist.0.is_empty()).then(|| artist.0.to_owned()),
                name: artist.1.to_owned(),
            }],
            album_id: None,
            album_title: None,
            duration: Some(Duration::from_secs(seconds)),
            available: true,
            cover_uri: None,
            warning,
            loudness: None,
            lyrics: None,
        }
    }

    #[test]
    fn sample_list_is_parsed_leniently() {
        let list = parse_list(SAMPLE_LIST.as_bytes()).unwrap();
        assert_eq!(list.tracks.len(), 3, "запись с плохим номером пропущена");
        let first = list.tracks.get("29238706").unwrap();
        assert_eq!(first.title.as_deref(), Some("Дурак и молния (без цензуры)"));
        assert_eq!(list.sound_of("7019818"), Some("1"));
        let album = list.albums.get("10101").unwrap();
        assert_eq!(album.volumes, ["29238706", "7019818"]);
        assert!(album.replace_artists);
        let mut censored: Vec<_> = list.censored.iter().cloned().collect();
        censored.sort();
        assert_eq!(censored, ["500000", "500001", "500002"]);
        // Ссылки хранилищ: шаблон с подстановкой номера, без схемы — https.
        assert_eq!(
            list.replacement_url("500000"),
            Some("https://example.invalid/500000")
        );
        assert_eq!(
            list.replacement_url("500001"),
            Some("https://example.invalid/500001")
        );
        assert_eq!(
            list.replacement_url("500002"),
            Some("https://example.invalid/1"),
            "значение-номер — это номер файла, не трека"
        );
        assert!(matches!(parse_list(b"{}"), Err(ListError::Broken)));
        assert!(matches!(parse_list(b"<html>"), Err(ListError::Broken)));
    }

    #[test]
    fn storage_urls_resolve_like_the_addon() {
        let list = parse_list(
            br#"{"sources":[{"tracks":{"7019818":{"__fckCensor":{"audioSourceId":"9"}}},
                "tracksStorages":[{"urlTemplate":"cdn.example.org/t/%%",
                "trackIds":[7019818,5],
                "tracks":{"6":"https://a.example.org/x.mp3","7":{"url":8,"durationMs":1},
                          "8":"http://plain.example.org/y.mp3"}}]}]}"#,
        )
        .unwrap();
        assert_eq!(list.replacement_url("5"), Some("https://cdn.example.org/t/5"));
        assert_eq!(list.replacement_url("6"), Some("https://a.example.org/x.mp3"));
        assert_eq!(
            list.replacement_url("7"),
            Some("https://cdn.example.org/t/8"),
            "объект: url-номер подставляется в шаблон"
        );
        assert_eq!(list.replacement_url("8"), None, "http отклоняется");
        assert_eq!(list.replacement_url("7019818"), Some("https://cdn.example.org/t/7019818"));

        // Шаблон без `%%` и пустой — хранилище без ссылок.
        let broken = parse_list(
            br#"{"sources":[{"tracksStorages":[{"urlTemplate":"no-placeholder.example.org","trackIds":[1]}]}]}"#,
        )
        .unwrap();
        assert_eq!(broken.replacement_url("1"), None);
        assert!(broken.censored.contains("1"), "признак цензуры остаётся");

        let mut censor = Censor::default();
        censor.loaded(
            Loaded {
                list: Arc::new(list),
                fetched: 0,
                stale: None,
            },
            None,
        );

        // Ручная замена важнее прямой ссылки.
        censor.replacements.tracks.insert(
            "5".to_owned(),
            Replacement::file(PathBuf::from("a.flac")),
        );
        let manual = track("5", "x", ("", "y"), 1, Warning::None);
        assert_eq!(
            censor.sound(&manual, true),
            Decision::Play(Sound::File(PathBuf::from("a.flac")))
        );

        // audioSourceId из списка важнее ссылки хранилища.
        let sourced = track("7019818", "x", ("", "y"), 1, Warning::Clean);
        assert_eq!(
            censor.sound(&sourced, true),
            Decision::Play(Sound::Track("9".to_owned()))
        );

        // Ссылка хранилища важнее автопоиска: играют файл, а не поиск.
        let linked = track("6", "x", ("", "y"), 1, Warning::Clean);
        assert_eq!(
            censor.sound(&linked, true),
            Decision::Play(Sound::Url("https://a.example.org/x.mp3".to_owned()))
        );

        // Недоступный трек со ссылкой в хранилище играет.
        let mut gone = track("7", "x", ("", "y"), 1, Warning::None);
        gone.available = false;
        assert!(censor.playable(&gone));
        assert_eq!(
            censor.sound(&gone, true),
            Decision::Play(Sound::Url("https://cdn.example.org/t/8".to_owned()))
        );
    }

    #[test]
    fn covers_with_scheme_are_ignored() {
        let list = parse_list(
            br#"{"sources":[{"tracks":{"1":{"coverUri":"https://evil/%%"},"2":{"coverUri":"t2.genius.com/unsafe/%%/x"}}}]}"#,
        )
        .unwrap();
        assert_eq!(list.tracks.get("1").unwrap().cover_uri, None);
        assert_eq!(
            list.tracks.get("2").unwrap().cover_uri.as_deref(),
            Some("t2.genius.com/unsafe/%%/x")
        );
    }

    #[test]
    fn patch_changes_only_listed_fields() {
        let list = parse_list(SAMPLE_LIST.as_bytes()).unwrap();
        let mut original = track(
            "29238706",
            "Дурак и м*лния",
            ("41052", "Король и Шут"),
            114,
            Warning::Clean,
        );
        original.version = Some("Clean".to_owned());
        let patched = list.patch(&original).unwrap();
        assert_eq!(patched.display_title(), "Дурак и молния (без цензуры)");
        assert_eq!(patched.artists, original.artists);
        // Только звук — показывать нечего.
        let sound_only = track("1", "x", ("", "y"), 1, Warning::None);
        let list =
            parse_list(br#"{"sources":[{"tracks":{"1":{"__fckCensor":{"audioSourceId":"2"}}}}]}"#)
                .unwrap();
        assert_eq!(list.patch(&sound_only), None);
    }

    #[test]
    fn explicit_version_is_picked_carefully() {
        let original = track(
            "1",
            "Дурак и м*лния",
            ("41052", "Король и Шут"),
            114,
            Warning::Clean,
        );
        let candidates = vec![
            // Не explicit.
            track(
                "2",
                "Дурак и молния",
                ("41052", "Король и Шут"),
                114,
                Warning::None,
            ),
            // Другой исполнитель.
            track(
                "3",
                "Дурак и молния",
                ("9", "Кавер-группа"),
                114,
                Warning::Explicit,
            ),
            // Слишком другая длительность.
            track(
                "4",
                "Дурак и молния",
                ("41052", "Король и Шут"),
                200,
                Warning::Explicit,
            ),
            track(
                "5",
                "Дурак и молния",
                ("41052", "Король и Шут"),
                120,
                Warning::Explicit,
            ),
            track(
                "6",
                "Дурак и молния (Explicit)",
                ("41052", "Король и Шут"),
                116,
                Warning::Explicit,
            ),
        ];
        assert_eq!(
            pick_explicit(&original, &candidates).map(|t| t.id.as_str()),
            Some("6")
        );
        // По имени без идентификатора, без учёта регистра и «ё».
        let by_name = track(
            "7",
            "ДУРАК И МОЛНИЯ",
            ("", "король и шут"),
            114,
            Warning::Explicit,
        );
        assert_eq!(
            pick_explicit(&original, std::slice::from_ref(&by_name)).map(|t| t.id.as_str()),
            Some("7")
        );
        assert_eq!(normalize("  Ёлка — с*ка!! "), "елка с*ка");
        assert!(same_title("Ёлка с*ка", "елка сука (Explicit)"));
        assert!(!same_title("м*лния", "малина"));
    }

    #[test]
    fn sound_follows_priority() {
        let list = parse_list(SAMPLE_LIST.as_bytes()).unwrap();
        let mut censor = Censor::default();
        censor.loaded(
            Loaded {
                list: Arc::new(list),
                fetched: 0,
                stale: None,
            },
            None,
        );
        let plain = track("42", "Обычный", ("", "Кто-то"), 100, Warning::None);
        assert_eq!(
            censor.sound(&plain, true),
            Decision::Play(Sound::Track("42".to_owned()))
        );
        // В списке — сначала поиск, потом найденное.
        let censored = track(
            "29238706",
            "Дурак",
            ("41052", "Король и Шут"),
            114,
            Warning::None,
        );
        assert_eq!(censor.sound(&censored, true), Decision::Search);
        assert_eq!(
            censor.sound(&censored, false),
            Decision::Play(Sound::Track("29238706".to_owned()))
        );
        censor
            .explicit
            .insert("29238706".to_owned(), Some("777".to_owned()));
        assert_eq!(
            censor.sound(&censored, true),
            Decision::Play(Sound::Track("777".to_owned()))
        );
        // Замена из списка важнее поиска, ручная — важнее всего.
        let sourced = track("7019818", "x", ("", "y"), 1, Warning::Clean);
        assert_eq!(
            censor.sound(&sourced, true),
            Decision::Play(Sound::Track("1".to_owned()))
        );
        censor.replacements.tracks.insert(
            "7019818".to_owned(),
            Replacement::file(PathBuf::from("a.flac")),
        );
        assert_eq!(
            censor.sound(&sourced, true),
            Decision::Play(Sound::File(PathBuf::from("a.flac")))
        );
        // Недоступный трек играет только с заменой.
        let mut gone = track("99", "Нет", ("", "y"), 1, Warning::None);
        gone.available = false;
        assert!(!censor.playable(&gone));
        assert_eq!(censor.sound(&gone, true), Decision::Silent);
        let mut listed_gone = track("2", "Снят", ("", "y"), 1, Warning::None);
        listed_gone.available = false;
        assert!(censor.playable(&listed_gone));
    }

    #[test]
    fn links_are_parsed() {
        assert_eq!(
            track_from_link("https://music.yandex.ru/album/123/track/456?utm=1").as_deref(),
            Some("456")
        );
        assert_eq!(
            track_from_link("music.yandex.com/track/789").as_deref(),
            Some("789")
        );
        assert_eq!(
            track_from_link("https://www.music.yandex.kz/album/1/track/2#x").as_deref(),
            Some("2")
        );
        assert_eq!(
            track_from_link("https://music.yandex.ru.evil.example/track/1"),
            None
        );
        assert_eq!(track_from_link("https://music.yandex.ru/album/123"), None);
        assert_eq!(
            track_from_link("https://music.yandex.ru/track/../../x"),
            None
        );
        assert_eq!(track_from_link("Король и Шут"), None);
    }

    #[test]
    fn replacements_round_trip() {
        let mut replacements = Replacements::default();
        replacements.tracks.insert(
            "1".to_owned(),
            Replacement::track(&track("2", "Песня", ("", "Группа"), 1, Warning::Explicit)),
        );
        replacements.tracks.insert(
            "3".to_owned(),
            Replacement::file(PathBuf::from("Музыка").join("трек.flac")),
        );
        let text = toml::to_string(&replacements).unwrap();
        let back: Replacements = toml::from_str(&text).unwrap();
        assert_eq!(back, replacements);
        assert_eq!(back.tracks.get("1").unwrap().label(), "Группа — Песня");
        assert_eq!(back.tracks.get("3").unwrap().label(), "трек.flac");
    }
}
