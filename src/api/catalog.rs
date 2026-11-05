//! Каталог и коллекция: поиск, альбомы, исполнители, плейлисты, а также
//! понравившиеся альбомы, исполнители и плейлисты пользователя.
//!
//! Разбор мягкий, как у «Мне нравится»: каждый трек, альбом, исполнитель и
//! плейлист разбирается отдельно, непонятный пропускается. Каталог отвечает
//! и без токена, поэтому его 403 — «нет доступа», а не «войдите заново».
//! Подробности в `docs/api/stage-7.md`.

use std::time::Duration;

use reqwest::StatusCode;
use reqwest::header::AUTHORIZATION;
use serde_json::Value;

use super::tracks::{
    self, ArtistRef, Track, clean, id_text, load_tracks, numeric_id, parse_artist_refs,
    parse_track, plausible_id,
};
use super::{API_BASE, ApiError, MUSIC_CLIENT, auth_header, check_status};
use crate::net::NetError;

/// Самая дальняя страница поиска: на `page` от 100 сервис отвечает 400.
pub const SEARCH_LAST_PAGE: u32 = 99;
/// Сколько символов запроса отправлять.
const SEARCH_MAX_CHARS: usize = 256;
/// Треков исполнителя на страницу «Все треки».
pub const ARTIST_TRACKS_PAGE: u32 = 100;
/// Альбомов исполнителя за запрос и сколько таких запросов делать.
const ARTIST_ALBUMS_PAGE: u32 = 100;
const ARTIST_ALBUMS_PAGES: u32 = 10;
/// `kind` плейлиста «Мне нравится»: у него свой раздел.
const LIKED_PLAYLIST_KIND: &str = "3";

// ------------------------------------------------------------- модель

/// Тип релиза.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlbumKind {
    Album,
    Single,
    Compilation,
    Podcast,
    Audiobook,
}

impl AlbumKind {
    fn from_value(value: Option<&Value>) -> Self {
        match value.and_then(Value::as_str) {
            Some("single") => Self::Single,
            Some("compilation") => Self::Compilation,
            Some("podcast") => Self::Podcast,
            Some("audiobook") => Self::Audiobook,
            _ => Self::Album,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Album => "Альбом",
            Self::Single => "Сингл",
            Self::Compilation => "Сборник",
            Self::Podcast => "Подкаст",
            Self::Audiobook => "Аудиокнига",
        }
    }
}

/// Альбом в списке: карточка или шапка.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlbumCard {
    pub id: String,
    pub title: String,
    pub version: Option<String>,
    pub year: Option<u32>,
    pub kind: AlbumKind,
    pub artists: Vec<ArtistRef>,
    pub cover_uri: Option<String>,
    pub track_count: Option<u32>,
    pub available: bool,
}

impl AlbumCard {
    /// Название с версией: «Название (Deluxe)».
    pub fn display_title(&self) -> String {
        match &self.version {
            Some(version) => format!("{} ({version})", self.title),
            None => self.title.clone(),
        }
    }
}

/// Исполнитель в списке или в шапке.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistCard {
    pub id: String,
    pub name: String,
    pub cover_uri: Option<String>,
    pub tracks: Option<u32>,
    pub albums: Option<u32>,
}

/// Плейлист в списке или в шапке.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistCard {
    /// `uid` владельца.
    pub owner: String,
    pub kind: String,
    pub title: String,
    pub owner_name: Option<String>,
    pub track_count: Option<u32>,
    pub cover_uri: Option<String>,
}

/// Альбом с треками (диски склеены по порядку).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Album {
    pub card: AlbumCard,
    pub tracks: Vec<Track>,
}

/// Страница исполнителя.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artist {
    pub card: ArtistCard,
    /// Слушателей за последний месяц.
    pub listeners: Option<u64>,
    pub popular: Vec<Track>,
    /// Все альбомы, свежие первыми.
    pub albums: Vec<AlbumCard>,
}

/// Плейлист с треками.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Playlist {
    pub card: PlaylistCard,
    pub description: Option<String>,
    pub tracks: Vec<Track>,
}

/// Страница списка: что пришло и сколько всего.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: usize,
}

/// Вкладка поиска.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SearchKind {
    Tracks,
    Albums,
    Artists,
    Playlists,
}

impl SearchKind {
    pub const ALL: [Self; 4] = [Self::Tracks, Self::Albums, Self::Artists, Self::Playlists];

    fn key(self) -> &'static str {
        match self {
            Self::Tracks => "track",
            Self::Albums => "album",
            Self::Artists => "artist",
            Self::Playlists => "playlist",
        }
    }

    /// Раздел в ответе сервиса.
    fn section(self) -> &'static str {
        match self {
            Self::Tracks => "tracks",
            Self::Albums => "albums",
            Self::Artists => "artists",
            Self::Playlists => "playlists",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Tracks => "Треки",
            Self::Albums => "Альбомы",
            Self::Artists => "Исполнители",
            Self::Playlists => "Плейлисты",
        }
    }
}

/// Найденное на одной странице одной вкладки.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    Tracks(Page<Track>),
    Albums(Page<AlbumCard>),
    Artists(Page<ArtistCard>),
    Playlists(Page<PlaylistCard>),
}

/// Результат поиска: найденное и исправленный запрос, если сервис исправил
/// опечатку.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    pub found: Found,
    pub corrected: Option<String>,
}

/// Плейлисты пользователя: свои и понравившиеся. `partial` — один из двух
/// списков получить не удалось.
#[derive(Debug, Clone)]
pub struct MyPlaylists {
    pub playlists: Vec<PlaylistCard>,
    pub partial: Option<String>,
}

// ------------------------------------------------------------- запросы

/// Что запрашиваем. По нему строится адрес, по нему же отладочный хук
/// подбирает готовый ответ.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Request {
    Search {
        text: String,
        kind: SearchKind,
        page: u32,
    },
    Album(String),
    ArtistBrief(String),
    ArtistAlbums {
        id: String,
        page: u32,
    },
    ArtistTracks {
        id: String,
        page: u32,
    },
    Playlist {
        owner: String,
        kind: String,
    },
    MyPlaylists(u64),
    LikedPlaylists(u64),
    LikedAlbums(u64),
    LikedArtists(u64),
}

/// Кто отвечает: каталог (работает и без токена) или коллекция
/// пользователя. От этого зависит смысл 403.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    Catalog,
    Collection,
}

impl Request {
    fn scope(&self) -> Scope {
        match self {
            Self::MyPlaylists(_)
            | Self::LikedPlaylists(_)
            | Self::LikedAlbums(_)
            | Self::LikedArtists(_) => Scope::Collection,
            _ => Scope::Catalog,
        }
    }

    /// Путь и параметры запроса. Идентификаторы уже проверены: только цифры.
    fn target(&self) -> (String, Vec<(&'static str, String)>) {
        match self {
            Self::Search { text, kind, page } => (
                "/search".to_owned(),
                vec![
                    ("text", text.clone()),
                    ("type", kind.key().to_owned()),
                    ("page", page.to_string()),
                    ("nocorrect", "false".to_owned()),
                ],
            ),
            Self::Album(id) => (format!("/albums/{id}/with-tracks"), Vec::new()),
            Self::ArtistBrief(id) => (format!("/artists/{id}/brief-info"), Vec::new()),
            Self::ArtistAlbums { id, page } => (
                format!("/artists/{id}/direct-albums"),
                vec![
                    ("page", page.to_string()),
                    ("page-size", ARTIST_ALBUMS_PAGE.to_string()),
                    ("sort-by", "year".to_owned()),
                ],
            ),
            Self::ArtistTracks { id, page } => (
                format!("/artists/{id}/tracks"),
                vec![
                    ("page", page.to_string()),
                    ("page-size", ARTIST_TRACKS_PAGE.to_string()),
                ],
            ),
            Self::Playlist { owner, kind } => {
                (format!("/users/{owner}/playlists/{kind}"), Vec::new())
            }
            Self::MyPlaylists(uid) => (format!("/users/{uid}/playlists/list"), Vec::new()),
            Self::LikedPlaylists(uid) => (format!("/users/{uid}/likes/playlists"), Vec::new()),
            Self::LikedAlbums(uid) => (
                format!("/users/{uid}/likes/albums"),
                vec![("rich", "true".to_owned())],
            ),
            Self::LikedArtists(uid) => (
                format!("/users/{uid}/likes/artists"),
                vec![("with-timestamps", "false".to_owned())],
            ),
        }
    }
}

/// Отправляет запрос и возвращает `result` ответа.
async fn fetch(http: &reqwest::Client, token: &str, request: &Request) -> Result<Value, ApiError> {
    let (status, body) = send(http, token, request).await?;
    envelope(request.scope(), status, &body)
}

async fn send(
    http: &reqwest::Client,
    token: &str,
    request: &Request,
) -> Result<(StatusCode, String), ApiError> {
    #[cfg(debug_assertions)]
    if let Some(reply) = test_hook::reply(token, request).await? {
        return Ok(reply);
    }
    let (path, query) = request.target();
    // Параметры кодируются: «&», «#» и «?» в тексте поиска их не ломают.
    let url = reqwest::Url::parse_with_params(&format!("{API_BASE}{path}"), &query)
        .map_err(|_| ApiError::InvalidResponse)?;
    let response = http
        .get(url)
        .header(AUTHORIZATION, auth_header(token)?)
        .header("X-Yandex-Music-Client", MUSIC_CLIENT)
        .send()
        .await
        .map_err(NetError::from)?;
    let status = response.status();
    let body = response.text().await.map_err(NetError::from)?;
    Ok((status, body))
}

/// Код ответа и обёртка `{"result":…}`.
fn envelope(scope: Scope, status: StatusCode, body: &str) -> Result<Value, ApiError> {
    match (scope, status) {
        (Scope::Catalog, StatusCode::FORBIDDEN) => return Err(ApiError::Forbidden),
        (Scope::Catalog, StatusCode::NOT_FOUND | StatusCode::GONE) => {
            return Err(ApiError::NotFound);
        }
        _ => check_status(status)?,
    }
    let mut value: Value = serde_json::from_str(body).map_err(|_| ApiError::InvalidResponse)?;
    value
        .get_mut("result")
        .map(Value::take)
        .ok_or(ApiError::InvalidResponse)
}

// ------------------------------------------------------------- методы

/// Поиск по вкладке `kind`, страница `page` с нуля. Пустой запрос и страницы
/// дальше [`SEARCH_LAST_PAGE`] ничего не запрашивают.
pub async fn search(
    http: &reqwest::Client,
    token: &str,
    text: &str,
    kind: SearchKind,
    page: u32,
) -> Result<SearchResult, ApiError> {
    let text: String = text.trim().chars().take(SEARCH_MAX_CHARS).collect();
    if text.is_empty() || page > SEARCH_LAST_PAGE {
        return Ok(parse_search(kind, &Value::Null));
    }
    let request = Request::Search { text, kind, page };
    let result = fetch(http, token, &request).await?;
    Ok(parse_search(kind, &result))
}

/// Альбом с треками. `id` — число.
pub async fn fetch_album(http: &reqwest::Client, token: &str, id: &str) -> Result<Album, ApiError> {
    let id = checked_id(id)?;
    let result = fetch(http, token, &Request::Album(id)).await?;
    parse_album(&result)
}

/// Исполнитель: шапка, популярные треки и все альбомы. Не пришли альбомы
/// отдельным запросом — берём первые из `brief-info`.
pub async fn fetch_artist(
    http: &reqwest::Client,
    token: &str,
    id: &str,
) -> Result<Artist, ApiError> {
    let id = checked_id(id)?;
    let brief_request = Request::ArtistBrief(id.clone());
    let (brief, albums) = tokio::join!(
        fetch(http, token, &brief_request),
        fetch_artist_albums(http, token, &id),
    );
    let mut artist = parse_artist(&brief?)?;
    if let Ok(albums) = albums
        && !albums.is_empty()
    {
        artist.albums = albums;
    }
    Ok(artist)
}

async fn fetch_artist_albums(
    http: &reqwest::Client,
    token: &str,
    id: &str,
) -> Result<Vec<AlbumCard>, ApiError> {
    let mut albums = Vec::new();
    for page in 0..ARTIST_ALBUMS_PAGES {
        let request = Request::ArtistAlbums {
            id: id.to_owned(),
            page,
        };
        let result = fetch(http, token, &request).await?;
        let (items, total) = parse_list(&result, "albums", album_card);
        let got = items.len();
        albums.extend(items);
        if got == 0 || albums.len() >= total {
            break;
        }
    }
    Ok(albums)
}

/// Страница «Все треки» исполнителя, `page` с нуля.
pub async fn fetch_artist_tracks(
    http: &reqwest::Client,
    token: &str,
    id: &str,
    page: u32,
) -> Result<Page<Track>, ApiError> {
    let id = checked_id(id)?;
    let result = fetch(http, token, &Request::ArtistTracks { id, page }).await?;
    let (items, total) = parse_list(&result, "tracks", parse_track);
    Ok(Page { items, total })
}

/// Плейлист `kind` пользователя `owner` с треками. Треки без данных
/// догружаются через `POST /tracks`.
pub async fn fetch_playlist(
    http: &reqwest::Client,
    token: &str,
    owner: &str,
    kind: &str,
) -> Result<Playlist, ApiError> {
    let request = Request::Playlist {
        owner: checked_id(owner)?,
        kind: checked_id(kind)?,
    };
    let result = fetch(http, token, &request).await?;
    let (mut playlist, pending) = parse_playlist(&result)?;
    if pending.is_empty() {
        return Ok(playlist);
    }
    let ids: Vec<String> = pending.iter().map(|(_, id)| id.clone()).collect();
    let loaded = load_tracks(
        &ids,
        |batch| {
            let (http, token) = (http.clone(), token.to_owned());
            async move { post_batch(&http, &token, &batch).await }
        },
        |_| {},
    )
    .await?;
    for ((index, _), track) in pending.into_iter().zip(loaded) {
        if let Some(slot) = playlist.tracks.get_mut(index) {
            *slot = track;
        }
    }
    Ok(playlist)
}

/// Пачка треков для плейлиста: в отладочной сборке с хуком — готовый ответ.
async fn post_batch(
    http: &reqwest::Client,
    token: &str,
    ids: &[String],
) -> Result<(StatusCode, String), ApiError> {
    #[cfg(debug_assertions)]
    if let Some(reply) = test_hook::tracks(token, ids)? {
        return Ok(reply);
    }
    tracks::post_tracks(http, token, ids).await
}

/// Свои плейлисты пользователя и понравившиеся чужие. Не пришёл один из
/// списков — второй всё равно показываем.
pub async fn fetch_my_playlists(
    http: &reqwest::Client,
    token: &str,
    uid: u64,
) -> Result<MyPlaylists, ApiError> {
    let (own_request, liked_request) = (Request::MyPlaylists(uid), Request::LikedPlaylists(uid));
    let (own, liked) = tokio::join!(
        fetch(http, token, &own_request),
        fetch(http, token, &liked_request),
    );
    let own = own.and_then(|value| list_of(&value, "playlist", playlist_card));
    let liked = liked.and_then(|value| list_of(&value, "playlist", playlist_card));
    match (own, liked) {
        (Err(err), Err(_)) => Err(err),
        (Err(ApiError::Unauthorized), _) | (_, Err(ApiError::Unauthorized)) => {
            Err(ApiError::Unauthorized)
        }
        (own, liked) => {
            let partial = match (&own, &liked) {
                (Err(err), _) => Some(format!("Не удалось загрузить ваши плейлисты: {err}")),
                (_, Err(err)) => Some(format!(
                    "Не удалось загрузить понравившиеся плейлисты: {err}"
                )),
                _ => None,
            };
            let mut playlists: Vec<PlaylistCard> = own
                .unwrap_or_default()
                .into_iter()
                .filter(|card| card.kind != LIKED_PLAYLIST_KIND)
                .collect();
            for card in liked.unwrap_or_default() {
                let duplicate = playlists
                    .iter()
                    .any(|own| own.owner == card.owner && own.kind == card.kind);
                if !duplicate {
                    playlists.push(card);
                }
            }
            Ok(MyPlaylists { playlists, partial })
        }
    }
}

/// Понравившиеся альбомы.
pub async fn fetch_liked_albums(
    http: &reqwest::Client,
    token: &str,
    uid: u64,
) -> Result<Vec<AlbumCard>, ApiError> {
    let result = fetch(http, token, &Request::LikedAlbums(uid)).await?;
    list_of(&result, "album", album_card)
}

/// Понравившиеся исполнители.
pub async fn fetch_liked_artists(
    http: &reqwest::Client,
    token: &str,
    uid: u64,
) -> Result<Vec<ArtistCard>, ApiError> {
    let result = fetch(http, token, &Request::LikedArtists(uid)).await?;
    list_of(&result, "artist", artist_card)
}

/// Идентификатор для адреса: только число. Иначе — «не найдено», запроса
/// нет.
fn checked_id(id: &str) -> Result<String, ApiError> {
    numeric_id(&Value::String(id.to_owned())).ok_or(ApiError::NotFound)
}

// ------------------------------------------------------------- разбор

fn parse_search(kind: SearchKind, result: &Value) -> SearchResult {
    // На пустой запрос `result` — строка "text-is-empty": это «ничего».
    let section = result.get(kind.section()).unwrap_or(&Value::Null);
    let found = match kind {
        SearchKind::Tracks => Found::Tracks(search_page(section, parse_track)),
        SearchKind::Albums => Found::Albums(search_page(section, album_card)),
        SearchKind::Artists => Found::Artists(search_page(section, artist_card)),
        SearchKind::Playlists => Found::Playlists(search_page(section, playlist_card)),
    };
    let corrected = (result.get("misspellCorrected").and_then(Value::as_bool) == Some(true))
        .then(|| result.get("misspellResult").and_then(Value::as_str))
        .flatten()
        .and_then(clean);
    SearchResult { found, corrected }
}

fn search_page<T>(section: &Value, parse: impl Fn(&Value) -> Option<T>) -> Page<T> {
    let (items, total) = parse_list(section, "results", parse);
    Page { items, total }
}

/// Массив `key` из объекта и общее число (`total` рядом или в `pager`).
/// Непонятные элементы пропускаются. Число не меньше пришедшего.
fn parse_list<T>(value: &Value, key: &str, parse: impl Fn(&Value) -> Option<T>) -> (Vec<T>, usize) {
    let items: Vec<T> = value
        .get(key)
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(&parse).collect())
        .unwrap_or_default();
    let total = value
        .get("total")
        .or_else(|| value.get("pager").and_then(|pager| pager.get("total")))
        .and_then(Value::as_u64)
        .and_then(|total| usize::try_from(total).ok())
        .unwrap_or(0)
        .max(items.len());
    (items, total)
}

/// Список коллекции: массив, где элемент — сам объект или обёртка
/// `{"<wrapper>": объект, "timestamp": …}`. Не массив — непонятный ответ.
fn list_of<T>(
    value: &Value,
    wrapper: &str,
    parse: impl Fn(&Value) -> Option<T>,
) -> Result<Vec<T>, ApiError> {
    let items = value.as_array().ok_or(ApiError::InvalidResponse)?;
    Ok(items
        .iter()
        .filter_map(|item| {
            let inner = item.get(wrapper).filter(|inner| inner.is_object());
            parse(inner.unwrap_or(item))
        })
        .collect())
}

fn text_of(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).and_then(clean)
}

fn count_of(value: Option<&Value>) -> Option<u32> {
    value
        .and_then(Value::as_u64)
        .and_then(|count| u32::try_from(count).ok())
}

/// Шаблон обложки. Пустой или без `%%` окну не нужен.
fn cover_of(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .and_then(clean)
        .filter(|uri| uri.contains("%%"))
}

fn has_error(value: &Value) -> bool {
    value.get("error").is_some_and(|error| !error.is_null())
}

fn album_card(value: &Value) -> Option<AlbumCard> {
    if has_error(value) {
        return None;
    }
    Some(AlbumCard {
        id: value.get("id").and_then(numeric_id)?,
        title: text_of(value, "title")?,
        version: text_of(value, "version"),
        year: count_of(value.get("year")),
        kind: AlbumKind::from_value(value.get("type")),
        artists: parse_artist_refs(value.get("artists")),
        cover_uri: cover_of(value.get("coverUri")).or_else(|| cover_of(value.get("ogImage"))),
        track_count: count_of(value.get("trackCount")),
        available: value.get("available").and_then(Value::as_bool) != Some(false),
    })
}

fn artist_card(value: &Value) -> Option<ArtistCard> {
    if has_error(value) {
        return None;
    }
    let counts = value.get("counts");
    Some(ArtistCard {
        id: value.get("id").and_then(numeric_id)?,
        name: text_of(value, "name")?,
        cover_uri: cover_of(value.get("cover").and_then(|cover| cover.get("uri")))
            .or_else(|| cover_of(value.get("ogImage"))),
        tracks: count_of(counts.and_then(|counts| counts.get("tracks"))),
        albums: count_of(counts.and_then(|counts| counts.get("directAlbums"))),
    })
}

fn playlist_card(value: &Value) -> Option<PlaylistCard> {
    let owner = value.get("owner");
    let cover = value.get("cover");
    let cover_uri = cover_of(cover.and_then(|cover| cover.get("uri")))
        .or_else(|| {
            cover
                .and_then(|cover| cover.get("itemsUri"))
                .and_then(Value::as_array)
                .and_then(|items| items.iter().find_map(|item| cover_of(Some(item))))
        })
        .or_else(|| cover_of(value.get("ogImage")));
    Some(PlaylistCard {
        owner: value
            .get("uid")
            .or_else(|| owner.and_then(|owner| owner.get("uid")))
            .and_then(numeric_id)?,
        kind: value.get("kind").and_then(numeric_id)?,
        title: text_of(value, "title")?,
        owner_name: owner
            .and_then(|owner| text_of(owner, "name").or_else(|| text_of(owner, "login"))),
        track_count: count_of(value.get("trackCount")),
        cover_uri,
    })
}

fn parse_album(result: &Value) -> Result<Album, ApiError> {
    if has_error(result) {
        return Err(ApiError::NotFound);
    }
    let card = album_card(result).ok_or(ApiError::InvalidResponse)?;
    let tracks = result
        .get("volumes")
        .and_then(Value::as_array)
        .map(|volumes| {
            volumes
                .iter()
                .filter_map(Value::as_array)
                .flatten()
                .filter_map(parse_track)
                .collect()
        })
        .unwrap_or_default();
    Ok(Album { card, tracks })
}

fn parse_artist(result: &Value) -> Result<Artist, ApiError> {
    let artist = result.get("artist").ok_or(ApiError::InvalidResponse)?;
    if has_error(artist) {
        return Err(ApiError::NotFound);
    }
    let card = artist_card(artist).ok_or(ApiError::InvalidResponse)?;
    let tracks = |key: &str| -> Vec<Track> {
        result
            .get(key)
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(parse_track).collect())
            .unwrap_or_default()
    };
    Ok(Artist {
        card,
        listeners: result
            .get("stats")
            .and_then(|stats| stats.get("lastMonthListeners"))
            .and_then(Value::as_u64),
        popular: tracks("popularTracks"),
        albums: result
            .get("albums")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(album_card).collect())
            .unwrap_or_default(),
    })
}

/// Плейлист и треки без данных: их место в списке и идентификатор.
fn parse_playlist(result: &Value) -> Result<(Playlist, Vec<(usize, String)>), ApiError> {
    if has_error(result) {
        return Err(ApiError::NotFound);
    }
    let card = playlist_card(result).ok_or(ApiError::InvalidResponse)?;
    let items = result
        .get("tracks")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut tracks = Vec::with_capacity(items.len());
    let mut pending = Vec::new();
    for item in items {
        let full = item.get("track").filter(|track| track.is_object());
        if let Some(track) = full.and_then(parse_track) {
            tracks.push(track);
        } else if let Some(id) = item.get("id").and_then(id_text) {
            // Данных нет — заглушка, пока не догрузим. Неправильный
            // идентификатор запрашивать незачем: так и останется заглушкой.
            if plausible_id(&id) {
                pending.push((tracks.len(), id.clone()));
            }
            tracks.push(Track::missing(&id));
        }
    }
    let playlist = Playlist {
        card,
        description: text_of(result, "description"),
        tracks,
    };
    Ok((playlist, pending))
}

/// Короткое число для подписи: «980», «12 тыс.», «2,1 млн».
pub fn compact_number(value: u64) -> String {
    // Делим на 100 и 100 000: одна цифра после запятой.
    match value {
        0..=9_999 => value.to_string(),
        10_000..=999_999 => format!("{} тыс.", value / 1000),
        _ => {
            let tenths = value / 100_000;
            let (whole, fraction) = (tenths / 10, tenths % 10);
            if fraction == 0 || whole >= 100 {
                format!("{whole} млн")
            } else {
                format!("{whole},{fraction} млн")
            }
        }
    }
}

/// Сумма длительностей: `sum` паникует при переполнении, а длительности
/// приходят от сервера.
pub fn total_duration(tracks: &[Track]) -> Duration {
    tracks
        .iter()
        .filter_map(|track| track.duration)
        .fold(Duration::ZERO, Duration::saturating_add)
}

// ------------------------------------------------------------- фикстуры

/// Готовые ответы каталога: для юнит-тестов и отладочного хука
/// `PLASTINKA_TEST_CATALOG`. Альбом, исполнитель и плейлист собираются по
/// любому идентификатору, поэтому в окне работают все переходы.
#[cfg(any(test, debug_assertions))]
mod fixtures {
    use reqwest::StatusCode;
    use serde_json::{Value, json};

    use super::{Request, SearchKind};

    pub const NAMES: &[&str] = &[
        "sample", "empty", "many", "slow", "401", "403", "404", "503", "broken",
    ];

    /// Сколько треков в большом плейлисте и альбомов в большой коллекции.
    pub const MANY_TRACKS: u64 = 5000;
    pub const MANY_ALBUMS: u64 = 400;
    /// Сколько всего треков находит большой поиск.
    pub const MANY_FOUND: u64 = 1990;

    fn ok(result: Value) -> (StatusCode, String) {
        (StatusCode::OK, json!({ "result": result }).to_string())
    }

    pub fn reply(name: &str, request: &Request) -> Option<(StatusCode, String)> {
        let status = match name {
            "401" => StatusCode::UNAUTHORIZED,
            "403" => StatusCode::FORBIDDEN,
            "404" => StatusCode::NOT_FOUND,
            "503" => StatusCode::SERVICE_UNAVAILABLE,
            "broken" => return Some((StatusCode::OK, "<html>".to_owned())),
            "sample" | "slow" | "empty" | "many" => StatusCode::OK,
            _ => return None,
        };
        if status != StatusCode::OK {
            return Some((status, r#"{"error":{"name":"test"}}"#.to_owned()));
        }
        let empty = name == "empty";
        let many = name == "many";
        Some(ok(match request {
            Request::Search { text, kind, page } => search(text, *kind, *page, empty, many),
            Request::Album(id) => album(id, empty),
            Request::ArtistBrief(id) => artist(id, empty),
            Request::ArtistAlbums { id, page } => artist_albums(id, *page, empty),
            Request::ArtistTracks { id, page } => artist_tracks(id, *page, empty, many),
            Request::Playlist { owner, kind } => playlist(owner, kind, empty, many),
            Request::MyPlaylists(uid) => my_playlists(*uid, empty),
            Request::LikedPlaylists(_) => liked_playlists(empty),
            Request::LikedAlbums(_) => liked_albums(empty, many),
            Request::LikedArtists(_) => liked_artists(empty),
        }))
    }

    /// Ответ `POST /tracks` для треков без данных в плейлисте.
    #[cfg(debug_assertions)]
    pub fn tracks(ids: &[String]) -> (StatusCode, String) {
        let items: Vec<Value> = ids
            .iter()
            .filter_map(|id| id.parse::<u64>().ok())
            .map(|id| track(id, "Догруженный трек", 3))
            .collect();
        ok(Value::Array(items))
    }

    const ARTISTS: [(&str, &str); 4] = [
        ("41052", "Король и Шут"),
        ("853465", "Tame Impala"),
        ("1001", "Оркестр"),
        (
            "1002",
            "Очень длинное имя исполнителя, которое не помещается в карточку",
        ),
    ];

    fn artist_ref(id: u64) -> Value {
        let (artist_id, name) = ARTISTS
            .get(usize::try_from(id % 4).unwrap_or(0))
            .copied()
            .unwrap_or(("41052", "Король и Шут"));
        json!({ "id": artist_id.parse::<u64>().unwrap_or(1), "name": name })
    }

    /// Трек `id`: доступен, кроме каждого седьмого; у каждого пятого два
    /// исполнителя.
    fn track(id: u64, title: &str, album: u64) -> Value {
        let mut artists = vec![artist_ref(id)];
        if id.is_multiple_of(5) {
            artists.push(artist_ref(id + 1));
        }
        json!({
            "id": id.to_string(),
            "title": format!("{title} {id}"),
            "available": !id.is_multiple_of(7),
            "durationMs": (id % 300 + 60) * 1000,
            "coverUri": format!("avatars.example/{album}/%%"),
            "artists": artists,
            "albums": [{ "id": album, "title": format!("Альбом {album}") }],
        })
    }

    fn album_value(id: u64) -> Value {
        let kinds = [Value::Null, json!("single"), json!("compilation")];
        json!({
            "id": id,
            "title": if id % 9 == 4 {
                format!("Альбом с очень длинным названием, которое не влезает в карточку {id}")
            } else {
                format!("Альбом {id}")
            },
            "year": 1990 + id % 35,
            "type": kinds.get(usize::try_from(id % 3).unwrap_or(0)).cloned().unwrap_or(Value::Null),
            "trackCount": id % 20 + 1,
            "available": true,
            "coverUri": format!("avatars.example/a{id}/%%"),
            "artists": [artist_ref(id)],
        })
    }

    fn artist_value(id: &str, name: &str) -> Value {
        json!({
            "id": id,
            "name": name,
            "cover": { "uri": format!("avatars.example/p{id}/%%") },
            "counts": { "tracks": 219, "directAlbums": 16 },
        })
    }

    fn playlist_value(uid: u64, kind: u64, title: &str, count: u64) -> Value {
        json!({
            "uid": uid,
            "kind": kind,
            "title": title,
            "trackCount": count,
            "owner": { "uid": uid, "login": "listener", "name": "Слушатель" },
            "cover": { "type": "mosaic", "itemsUri": [format!("avatars.example/m{kind}/%%")] },
        })
    }

    fn numeric(id: &str) -> u64 {
        id.parse().unwrap_or(1)
    }

    fn search(text: &str, kind: SearchKind, page: u32, empty: bool, many: bool) -> Value {
        if empty {
            return json!({ "text": text, "misspellCorrected": false });
        }
        let base = u64::from(page) * 20;
        let (section, items, total) = match kind {
            SearchKind::Tracks => {
                let total = if many { MANY_FOUND } else { 7 };
                let items: Vec<Value> = (base..(base + 20).min(total))
                    .map(|n| track(500_000 + n, "Найденный трек", 10_101))
                    .chain((page == 0).then(|| json!({ "title": "без id" })))
                    .collect();
                ("tracks", items, total)
            }
            SearchKind::Albums => {
                let items = (0..10).map(|n| album_value(20_000 + n)).collect();
                ("albums", items, if many { 300 } else { 10 })
            }
            SearchKind::Artists => {
                let items = ARTISTS
                    .iter()
                    .map(|(id, name)| artist_value(id, name))
                    .chain([json!({ "name": "без id" })])
                    .collect();
                ("artists", items, 4)
            }
            SearchKind::Playlists => {
                let items = (0..6)
                    .map(|n| playlist_value(42, 1000 + n, &format!("Плейлист {n}"), n * 10))
                    .collect();
                ("playlists", items, 6)
            }
        };
        let mut result = json!({ "text": text, "misspellCorrected": false });
        if let Some(object) = result.as_object_mut() {
            if text.contains("ошибк") {
                object.insert("misspellCorrected".to_owned(), json!(true));
                object.insert("misspellResult".to_owned(), json!("исправленный запрос"));
            }
            object.insert(
                section.to_owned(),
                json!({ "total": total, "perPage": 20, "results": items }),
            );
        }
        result
    }

    fn album(id: &str, empty: bool) -> Value {
        let number = numeric(id);
        let mut value = album_value(number);
        let volumes: Vec<Vec<Value>> = if empty {
            Vec::new()
        } else {
            vec![
                (1..=8)
                    .map(|n| track(number * 100 + n, "Трек альбома", number))
                    .collect(),
                (9..=12)
                    .map(|n| track(number * 100 + n, "Второй диск", number))
                    .collect(),
            ]
        };
        if let Some(object) = value.as_object_mut() {
            object.insert("volumes".to_owned(), json!(volumes));
        }
        value
    }

    fn artist(id: &str, empty: bool) -> Value {
        let name = ARTISTS
            .iter()
            .find(|(known, _)| *known == id)
            .map_or("Исполнитель", |(_, name)| *name);
        let number = numeric(id);
        let popular: Vec<Value> = if empty {
            Vec::new()
        } else {
            (1..=10)
                .map(|n| track(number * 10 + n, "Популярный трек", 10_101))
                .collect()
        };
        let albums: Vec<Value> = if empty {
            Vec::new()
        } else {
            (0..9).map(|n| album_value(30_000 + n)).collect()
        };
        json!({
            "artist": artist_value(id, name),
            "popularTracks": popular,
            "albums": albums,
            "stats": { "lastMonthListeners": 2_100_000 },
        })
    }

    fn artist_albums(_id: &str, page: u32, empty: bool) -> Value {
        let total = if empty { 0 } else { 16 };
        let items: Vec<Value> = if page == 0 {
            (0..total).map(|n| album_value(30_000 + n)).collect()
        } else {
            Vec::new()
        };
        json!({ "pager": { "page": page, "perPage": 100, "total": total }, "albums": items })
    }

    fn artist_tracks(id: &str, page: u32, empty: bool, many: bool) -> Value {
        let total: u64 = if empty {
            0
        } else if many {
            1000
        } else {
            219
        };
        let base = u64::from(page) * 100;
        let number = numeric(id);
        let items: Vec<Value> = (base..(base + 100).min(total))
            .map(|n| track(number * 10_000 + n + 1, "Трек исполнителя", 10_101))
            .collect();
        json!({ "pager": { "page": page, "perPage": 100, "total": total }, "tracks": items })
    }

    fn playlist(owner: &str, kind: &str, empty: bool, many: bool) -> Value {
        let (uid, kind) = (numeric(owner), numeric(kind));
        let count = if empty {
            0
        } else if many {
            MANY_TRACKS
        } else {
            30
        };
        let mut value = playlist_value(uid, kind, &format!("Плейлист {kind}"), count);
        // В примере часть треков приходит без данных: их окно догружает.
        let items: Vec<Value> = (1..=count)
            .map(|n| {
                let id = kind * 100_000 + n;
                if !many && n % 10 == 0 {
                    json!({ "id": id, "timestamp": "2026-11-01T00:00:00+00:00" })
                } else {
                    json!({ "id": id, "track": track(id, "Трек плейлиста", 10_101) })
                }
            })
            .collect();
        if let Some(object) = value.as_object_mut() {
            object.insert("tracks".to_owned(), Value::Array(items));
            object.insert(
                "description".to_owned(),
                json!("Описание плейлиста для проверки"),
            );
        }
        value
    }

    fn my_playlists(uid: u64, empty: bool) -> Value {
        if empty {
            return json!([]);
        }
        json!([
            playlist_value(uid, 3, "Мне нравится", 9),
            playlist_value(uid, 1001, "Для пробежки", 30),
            playlist_value(uid, 1002, "Плейлист с очень длинным названием, которое не влезает", 12),
            { "title": "без uid и kind" },
            playlist_value(uid, 1003, "Пустой", 0),
        ])
    }

    fn liked_playlists(empty: bool) -> Value {
        if empty {
            return json!([]);
        }
        json!([
            { "timestamp": "2026-11-01T00:00:00+00:00", "playlist": playlist_value(414_787_002, 1076, "Чарт", 100) },
        ])
    }

    fn liked_albums(empty: bool, many: bool) -> Value {
        if empty {
            return json!([]);
        }
        let count = if many { MANY_ALBUMS } else { 14 };
        Value::Array(
            (0..count)
                .map(|n| json!({ "timestamp": "2026-11-01T00:00:00+00:00", "album": album_value(10_101 + n) }))
                .collect(),
        )
    }

    fn liked_artists(empty: bool) -> Value {
        if empty {
            return json!([]);
        }
        Value::Array(
            ARTISTS
                .iter()
                .map(|(id, name)| artist_value(id, name))
                .collect(),
        )
    }
}

/// Отладочный хук `PLASTINKA_TEST_CATALOG`: вместо запросов к сервису —
/// готовые ответы. Условия — у `api::test_hook`. `slow` отвечает через
/// 1,5 с: видно загрузку и можно уйти со страницы, пока она грузится.
#[cfg(debug_assertions)]
mod test_hook {
    use reqwest::StatusCode;

    use super::{Request, fixtures};
    use crate::api::ApiError;

    const VAR: &str = "PLASTINKA_TEST_CATALOG";

    fn name(token: &str) -> Result<Option<String>, ApiError> {
        crate::api::test_hook(VAR, token)
    }

    pub async fn reply(
        token: &str,
        request: &Request,
    ) -> Result<Option<(StatusCode, String)>, ApiError> {
        let Some(name) = name(token)? else {
            return Ok(None);
        };
        if name == "slow" {
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        }
        fixtures::reply(&name, request)
            .map(Some)
            .ok_or_else(|| crate::api::unknown_fixture(VAR, fixtures::NAMES))
    }

    pub fn tracks(token: &str, ids: &[String]) -> Result<Option<(StatusCode, String)>, ApiError> {
        Ok(name(token)?.map(|_| fixtures::tracks(ids)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(request: &Request) -> Value {
        let (status, body) = fixtures::reply("sample", request).unwrap();
        envelope(request.scope(), status, &body).unwrap()
    }

    #[test]
    fn search_is_parsed_leniently() {
        let request = Request::Search {
            text: "x".to_owned(),
            kind: SearchKind::Tracks,
            page: 0,
        };
        let result = parse_search(SearchKind::Tracks, &sample(&request));
        let Found::Tracks(page) = result.found else {
            panic!("ожидались треки");
        };
        // Элемент без id пропущен.
        assert_eq!(page.items.len(), 7);
        assert_eq!(page.total, 7);
        assert_eq!(result.corrected, None);
        assert_eq!(page.items[0].artists[0].id.as_deref(), Some("41052"));
    }

    #[test]
    fn empty_and_odd_search_results() {
        let nothing = parse_search(
            SearchKind::Albums,
            &serde_json::json!({"text":"zzz","misspellCorrected":false}),
        );
        assert_eq!(
            nothing.found,
            Found::Albums(Page {
                items: Vec::new(),
                total: 0
            })
        );
        // На пустой запрос сервис присылает строку вместо объекта.
        let text = parse_search(
            SearchKind::Artists,
            &Value::String("text-is-empty".to_owned()),
        );
        assert!(matches!(text.found, Found::Artists(page) if page.items.is_empty()));
        let corrected = parse_search(
            SearchKind::Tracks,
            &serde_json::json!({"misspellCorrected":true,"misspellResult":"король и шут",
                "tracks":{"total":"много","results":"не список"}}),
        );
        assert_eq!(corrected.corrected.as_deref(), Some("король и шут"));
        assert!(matches!(corrected.found, Found::Tracks(page) if page.total == 0));
    }

    #[test]
    fn artist_and_playlist_cards_skip_broken_items() {
        let request = Request::Search {
            text: "x".to_owned(),
            kind: SearchKind::Artists,
            page: 0,
        };
        let Found::Artists(page) = parse_search(SearchKind::Artists, &sample(&request)).found
        else {
            panic!("ожидались исполнители");
        };
        assert_eq!(page.items.len(), 4);
        let cards = list_of(
            &sample(&Request::MyPlaylists(42)),
            "playlist",
            playlist_card,
        )
        .unwrap();
        assert_eq!(cards.len(), 4, "элемент без uid и kind пропущен");
        assert_eq!(
            cards[1].cover_uri.as_deref(),
            Some("avatars.example/m1001/%%")
        );
        assert_eq!(cards[1].owner_name.as_deref(), Some("Слушатель"));
    }

    #[test]
    fn album_volumes_are_joined() {
        let album = parse_album(&sample(&Request::Album("10101".to_owned()))).unwrap();
        assert_eq!(album.card.id, "10101");
        assert_eq!(album.tracks.len(), 12);
        assert!(
            album.tracks[0]
                .title
                .as_deref()
                .unwrap()
                .starts_with("Трек альбома")
        );
        assert!(
            album.tracks[8]
                .title
                .as_deref()
                .unwrap()
                .starts_with("Второй диск")
        );
        // Неизвестный альбом приходит с кодом 200 и полем error.
        assert!(matches!(
            parse_album(&serde_json::json!({"id":999,"error":"not-found"})),
            Err(ApiError::NotFound)
        ));
        assert!(matches!(
            parse_album(&serde_json::json!({"id":1})),
            Err(ApiError::InvalidResponse)
        ));
    }

    #[test]
    fn artist_is_parsed() {
        let artist = parse_artist(&sample(&Request::ArtistBrief("41052".to_owned()))).unwrap();
        assert_eq!(artist.card.name, "Король и Шут");
        assert_eq!(artist.popular.len(), 10);
        assert_eq!(artist.albums.len(), 9);
        assert_eq!(artist.listeners, Some(2_100_000));
        assert!(matches!(
            parse_artist(&serde_json::json!({"artist":{"id":"9","error":"not-found"}})),
            Err(ApiError::NotFound)
        ));
    }

    #[test]
    fn playlist_tracks_without_data_are_pending() {
        let request = Request::Playlist {
            owner: "42".to_owned(),
            kind: "1001".to_owned(),
        };
        let (playlist, pending) = parse_playlist(&sample(&request)).unwrap();
        assert_eq!(playlist.tracks.len(), 30);
        assert_eq!(pending.len(), 3);
        let (index, id) = &pending[0];
        assert_eq!(*index, 9);
        assert!(!playlist.tracks[*index].available);
        assert_eq!(&playlist.tracks[*index].id, id);
    }

    #[test]
    fn statuses_depend_on_scope() {
        assert!(matches!(
            envelope(Scope::Catalog, StatusCode::FORBIDDEN, ""),
            Err(ApiError::Forbidden)
        ));
        assert!(matches!(
            envelope(Scope::Collection, StatusCode::FORBIDDEN, ""),
            Err(ApiError::Unauthorized)
        ));
        assert!(matches!(
            envelope(Scope::Catalog, StatusCode::UNAUTHORIZED, ""),
            Err(ApiError::Unauthorized)
        ));
        for gone in [StatusCode::NOT_FOUND, StatusCode::GONE] {
            assert!(matches!(
                envelope(Scope::Catalog, gone, ""),
                Err(ApiError::NotFound)
            ));
        }
        assert!(matches!(
            envelope(Scope::Collection, StatusCode::NOT_FOUND, ""),
            Err(ApiError::UnexpectedStatus(404))
        ));
        assert!(matches!(
            envelope(Scope::Catalog, StatusCode::OK, r#"{"error":{}}"#),
            Err(ApiError::InvalidResponse)
        ));
    }

    #[test]
    fn collection_accepts_both_item_shapes() {
        let wrapped = serde_json::json!([{"timestamp":"x","album":{"id":5,"title":"A"}},
            {"id":6,"title":"B"}, {"album":"не объект","id":7,"title":"C"}, 17]);
        let albums = list_of(&wrapped, "album", album_card).unwrap();
        let ids: Vec<&str> = albums.iter().map(|album| album.id.as_str()).collect();
        assert_eq!(ids, ["5", "6", "7"]);
        assert!(matches!(
            list_of(&serde_json::json!({}), "album", album_card),
            Err(ApiError::InvalidResponse)
        ));
    }

    #[test]
    fn ids_for_urls_are_numbers_only() {
        assert_eq!(checked_id("41052").unwrap(), "41052");
        for bad in [
            "",
            "0",
            "abc",
            "1/2",
            "../x",
            "1?a=b",
            "123456789012345678901",
        ] {
            assert!(matches!(checked_id(bad), Err(ApiError::NotFound)), "{bad}");
        }
    }

    #[test]
    fn search_query_is_encoded() {
        let request = Request::Search {
            text: "a&b#c?d".to_owned(),
            kind: SearchKind::Tracks,
            page: 0,
        };
        let (path, query) = request.target();
        let url = reqwest::Url::parse_with_params(&format!("{API_BASE}{path}"), &query).unwrap();
        assert!(url.as_str().contains("text=a%26b%23c%3Fd"), "{url}");
    }

    #[test]
    fn numbers_are_compact() {
        assert_eq!(compact_number(980), "980");
        assert_eq!(compact_number(12_345), "12 тыс.");
        assert_eq!(compact_number(2_100_000), "2,1 млн");
        assert_eq!(compact_number(6_682_987), "6,6 млн");
        assert_eq!(compact_number(150_000_000), "150 млн");
        assert!(!compact_number(u64::MAX).is_empty());
    }

    #[test]
    fn every_fixture_exists() {
        for name in fixtures::NAMES {
            assert!(fixtures::reply(name, &Request::Album("1".to_owned())).is_some());
        }
        let many = fixtures::reply(
            "many",
            &Request::Playlist {
                owner: "1".to_owned(),
                kind: "2".to_owned(),
            },
        )
        .unwrap();
        let value = envelope(Scope::Catalog, many.0, &many.1).unwrap();
        let (playlist, pending) = parse_playlist(&value).unwrap();
        assert_eq!(playlist.tracks.len(), fixtures::MANY_TRACKS as usize);
        assert!(pending.is_empty());
    }
}
