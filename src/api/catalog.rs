//! Каталог и коллекция: поиск, альбомы, исполнители, плейлисты, а также
//! понравившиеся альбомы, исполнители и плейлисты пользователя.
//!
//! Разбор мягкий, как у «Мне нравится»: каждый трек, альбом, исполнитель и
//! плейлист разбирается отдельно, непонятный пропускается. Каталог отвечает
//! и без токена, поэтому его 403 — «нет доступа», а не «войдите заново».
//! Подробности в `docs/api/stage-7.md`.

use std::collections::HashMap;
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
/// Сколько просить в блоках «Встречали в Моей волне», «Новые релизы» и
/// «Собираем для вас». Сервис может прислать меньше.
const MET_ARTISTS: u32 = 16;
const NEW_RELEASES: u32 = 30;
const PERSONAL_PLAYLISTS: u32 = 10;
/// Сколько треков истории просить с полными данными (исполнители,
/// длительность). У остальных только идентификаторы, в топ они не идут.
const HISTORY_MODELS: u32 = 5000;

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
    /// Короткое описание для карточки: есть у плейлистов «Собираем для
    /// вас» («Звучит по-вашему каждый день»).
    pub note: Option<String>,
}

/// Как исполнитель сдвинулся в топе месяца.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trend {
    Up,
    Down,
    Same,
    New,
}

impl Trend {
    fn from_value(value: Option<&Value>) -> Option<Self> {
        match value.and_then(Value::as_str)? {
            "up" => Some(Self::Up),
            "down" => Some(Self::Down),
            "same" => Some(Self::Same),
            "new" => Some(Self::New),
            _ => None,
        }
    }
}

/// Исполнитель из топа: место, сдвиг и сколько его слушали.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopArtist {
    pub card: ArtistCard,
    pub position: Option<u32>,
    pub trend: Option<Trend>,
    pub listened: Duration,
    /// Сколько раз слушали: есть только в топе по истории.
    pub plays: Option<u32>,
}

/// Полный топ исполнителей по истории прослушивания. Топ месяца у сервиса —
/// не больше пяти, история — все прослушанные за последние дни.
#[derive(Debug, Clone, Default)]
pub struct ListeningTop {
    /// За сколько дней история.
    pub days: usize,
    pub artists: Vec<TopArtist>,
}

/// Раздел «Исполнители»: топ месяца, встреченные в «Моей волне» и
/// понравившиеся (недавние первыми). Топ и «Моя волна» — дополнения: если
/// их не прислали, раздел всё равно показываем.
#[derive(Debug, Clone, Default)]
pub struct MyArtists {
    pub top: Vec<TopArtist>,
    pub met: Vec<ArtistCard>,
    pub liked: Vec<ArtistCard>,
}

/// Раздел «Альбомы»: новые релизы для пользователя и понравившиеся.
#[derive(Debug, Clone, Default)]
pub struct MyAlbums {
    pub releases: Vec<AlbumCard>,
    pub liked: Vec<AlbumCard>,
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

/// Раздел «Плейлисты»: собранные сервисом для пользователя («Плейлист
/// дня», «Дежавю», «Премьера»), свои и понравившиеся. `partial` — свои или
/// понравившиеся получить не удалось.
#[derive(Debug, Clone, Default)]
pub struct MyPlaylists {
    pub personal: Vec<PlaylistCard>,
    pub own: Vec<PlaylistCard>,
    pub liked: Vec<PlaylistCard>,
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
    /// Топ исполнителей месяца.
    TopArtists,
    /// «Встречали в Моей волне».
    MetArtists,
    /// Новые релизы для пользователя.
    NewReleases,
    /// «Собираем для вас»: плейлист дня, «Дежавю», «Премьера».
    PersonalPlaylists,
    /// История прослушивания по дням.
    History,
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
            | Self::LikedArtists(_)
            | Self::History => Scope::Collection,
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
                vec![("with-timestamps", "true".to_owned())],
            ),
            Self::TopArtists => ("/personal/top/artists/month".to_owned(), Vec::new()),
            Self::MetArtists => (
                "/landing-blocks/artists".to_owned(),
                vec![("count", MET_ARTISTS.to_string())],
            ),
            Self::NewReleases => (
                "/landing-blocks/new-releases".to_owned(),
                vec![("count", NEW_RELEASES.to_string())],
            ),
            Self::PersonalPlaylists => (
                "/landing-blocks/personal-playlists".to_owned(),
                vec![("count", PERSONAL_PLAYLISTS.to_string())],
            ),
            Self::History => (
                "/music-history".to_owned(),
                vec![("fullModelsCount", HISTORY_MODELS.to_string())],
            ),
        }
    }

    /// Новые адреса (`personal`, `landing-blocks`) отвечают объектом без
    /// обёртки `{"result":…}`.
    fn bare(&self) -> bool {
        matches!(
            self,
            Self::TopArtists | Self::MetArtists | Self::NewReleases | Self::PersonalPlaylists
        )
    }
}

/// Отправляет запрос и возвращает `result` ответа.
async fn fetch(http: &reqwest::Client, token: &str, request: &Request) -> Result<Value, ApiError> {
    let (status, body) = send(http, token, request).await?;
    if request.bare() {
        bare_envelope(status, &body)
    } else {
        envelope(request.scope(), status, &body)
    }
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

/// Ответ без обёртки: сам объект. Обёртку `result`, если она всё же есть,
/// снимаем. Коды — как у каталога.
fn bare_envelope(status: StatusCode, body: &str) -> Result<Value, ApiError> {
    if status == StatusCode::NOT_FOUND || status == StatusCode::GONE {
        return Err(ApiError::NotFound);
    }
    check_status(status)?;
    let mut value: Value = serde_json::from_str(body).map_err(|_| ApiError::InvalidResponse)?;
    if !value.is_object() {
        return Err(ApiError::InvalidResponse);
    }
    Ok(
        match value.get_mut("result").filter(|result| result.is_object()) {
            Some(result) => result.take(),
            None => value,
        },
    )
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

/// Раздел «Плейлисты». Свои и понравившиеся — основное: не пришли оба —
/// ошибка, не пришёл один — второй всё равно показываем. «Собираем для
/// вас» — дополнение, его ошибка раздел не портит.
pub async fn fetch_my_playlists(
    http: &reqwest::Client,
    token: &str,
    uid: u64,
) -> Result<MyPlaylists, ApiError> {
    let (own_request, liked_request) = (Request::MyPlaylists(uid), Request::LikedPlaylists(uid));
    let (own, liked, personal) = tokio::join!(
        fetch(http, token, &own_request),
        fetch(http, token, &liked_request),
        fetch(http, token, &Request::PersonalPlaylists),
    );
    let own = own.and_then(|value| list_of(&value, "playlist", playlist_card));
    let liked = liked.and_then(|value| recent_first(&value, "playlist", playlist_card));
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
            let own: Vec<PlaylistCard> = own
                .unwrap_or_default()
                .into_iter()
                .filter(|card| card.kind != LIKED_PLAYLIST_KIND)
                .collect();
            let liked = liked
                .unwrap_or_default()
                .into_iter()
                .filter(|card| {
                    !own.iter()
                        .any(|mine| mine.owner == card.owner && mine.kind == card.kind)
                })
                .collect();
            Ok(MyPlaylists {
                personal: personal
                    .map(|value| personal_playlists(&value))
                    .unwrap_or_default(),
                own,
                liked,
                partial,
            })
        }
    }
}

/// Раздел «Альбомы»: понравившиеся (недавние первыми) и новые релизы.
/// Релизы — дополнение: их ошибка раздел не портит.
pub async fn fetch_my_albums(
    http: &reqwest::Client,
    token: &str,
    uid: u64,
) -> Result<MyAlbums, ApiError> {
    let liked_request = Request::LikedAlbums(uid);
    let (liked, releases) = tokio::join!(
        fetch(http, token, &liked_request),
        fetch(http, token, &Request::NewReleases),
    );
    Ok(MyAlbums {
        liked: recent_first(&liked?, "album", album_card)?,
        releases: releases
            .map(|value| new_releases(&value))
            .unwrap_or_default(),
    })
}

/// Раздел «Исполнители»: понравившиеся (недавние первыми), топ месяца и
/// «Встречали в Моей волне». Топ и волна — дополнения.
pub async fn fetch_my_artists(
    http: &reqwest::Client,
    token: &str,
    uid: u64,
) -> Result<MyArtists, ApiError> {
    let liked_request = Request::LikedArtists(uid);
    let (liked, top, met) = tokio::join!(
        fetch(http, token, &liked_request),
        fetch(http, token, &Request::TopArtists),
        fetch(http, token, &Request::MetArtists),
    );
    Ok(MyArtists {
        liked: recent_first(&liked?, "artist", artist_card)?,
        top: top.map(|value| top_artists(&value)).unwrap_or_default(),
        met: met.map(|value| met_artists(&value)).unwrap_or_default(),
    })
}

/// Полный топ исполнителей по истории прослушивания.
pub async fn fetch_listening_top(
    http: &reqwest::Client,
    token: &str,
) -> Result<ListeningTop, ApiError> {
    let value = fetch(http, token, &Request::History).await?;
    if !value.is_object() {
        return Err(ApiError::InvalidResponse);
    }
    Ok(listening_top(&value))
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

/// Список коллекции с отметкой `timestamp` (когда отметили): недавние
/// первыми. Без отметки — в конце, в порядке сервиса. Элемент — сам объект
/// или обёртка, как у [`list_of`].
fn recent_first<T>(
    value: &Value,
    wrapper: &str,
    parse: impl Fn(&Value) -> Option<T>,
) -> Result<Vec<T>, ApiError> {
    let items = value.as_array().ok_or(ApiError::InvalidResponse)?;
    let mut dated: Vec<(Option<&str>, T)> = items
        .iter()
        .filter_map(|item| {
            let inner = item.get(wrapper).filter(|inner| inner.is_object());
            let stamp = item.get("timestamp").and_then(Value::as_str);
            parse(inner.unwrap_or(item)).map(|parsed| (stamp, parsed))
        })
        .collect();
    // Отметки в одном формате ISO 8601 с одним поясом, поэтому сравниваются
    // как строки. Сортировка устойчивая: равные остаются в порядке сервиса.
    dated.sort_by(|(a, _), (b, _)| b.cmp(a));
    Ok(dated.into_iter().map(|(_, item)| item).collect())
}

/// Массив `key` из объекта ответа; не массив — пусто.
fn items_of<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

/// Топ месяца: `{"artists":[{"artist":…,"listenTimeSeconds":…,"top":…}]}`.
fn top_artists(value: &Value) -> Vec<TopArtist> {
    items_of(value, "artists")
        .iter()
        .filter_map(|item| {
            let top = item.get("top");
            Some(TopArtist {
                card: artist_card(item.get("artist")?)?,
                position: count_of(top.and_then(|top| top.get("position"))),
                trend: Trend::from_value(top.and_then(|top| top.get("progress"))),
                listened: Duration::from_secs(
                    item.get("listenTimeSeconds")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                ),
                plays: None,
            })
        })
        .collect()
}

/// Полный топ по истории: `{"historyTabs":[{"date","items":[{"context",
/// "tracks":[{"data":{"fullModel":трек}}]}]}]}`. Время — сумма длительностей
/// прослушанных треков: история не говорит, дослушан ли трек. Места — по
/// времени, при равенстве — по числу прослушиваний, дальше — кого история
/// показала раньше. «Сборники» (`various`) в топ не идут.
fn listening_top(value: &Value) -> ListeningTop {
    struct Tally {
        card: ArtistCard,
        plays: u32,
        millis: u64,
        order: usize,
    }
    let tabs = items_of(value, "historyTabs");
    let mut tally: HashMap<String, Tally> = HashMap::new();
    let models = tabs
        .iter()
        .flat_map(|tab| items_of(tab, "items"))
        .flat_map(|item| items_of(item, "tracks"))
        .filter_map(|track| track.get("data")?.get("fullModel"));
    for model in models {
        let millis = model.get("durationMs").and_then(Value::as_u64).unwrap_or(0);
        let mut counted: Vec<String> = Vec::new();
        let artists = items_of(model, "artists")
            .iter()
            .filter(|artist| artist.get("various").and_then(Value::as_bool) != Some(true))
            .filter_map(artist_card);
        for card in artists {
            // Один исполнитель дважды в треке — одно прослушивание.
            if counted.contains(&card.id) {
                continue;
            }
            counted.push(card.id.clone());
            let order = tally.len();
            let entry = tally.entry(card.id.clone()).or_insert(Tally {
                card,
                plays: 0,
                millis: 0,
                order,
            });
            entry.plays = entry.plays.saturating_add(1);
            entry.millis = entry.millis.saturating_add(millis);
        }
    }
    let mut ranked: Vec<Tally> = tally.into_values().collect();
    ranked.sort_by(|a, b| {
        b.millis
            .cmp(&a.millis)
            .then(b.plays.cmp(&a.plays))
            .then(a.order.cmp(&b.order))
    });
    ListeningTop {
        days: tabs.len(),
        artists: ranked
            .into_iter()
            .zip(1u32..)
            .map(|(tally, place)| TopArtist {
                card: tally.card,
                position: Some(place),
                trend: None,
                listened: Duration::from_millis(tally.millis),
                plays: Some(tally.plays),
            })
            .collect(),
    }
}

/// «Встречали в Моей волне»: `{"items":[{"data":{"artist":…}}]}`.
fn met_artists(value: &Value) -> Vec<ArtistCard> {
    items_of(value, "items")
        .iter()
        .filter_map(|item| artist_card(item.get("data")?.get("artist")?))
        .collect()
}

/// Новые релизы: `{"newReleases":[{"album":…,"artists":[…],"releaseDate":…}]}`.
/// У альбома здесь `albumType` и `cover.uri`, исполнители — рядом с ним.
fn new_releases(value: &Value) -> Vec<AlbumCard> {
    items_of(value, "newReleases")
        .iter()
        .filter_map(|item| {
            let album = item.get("album")?;
            let year = item
                .get("releaseDate")
                .and_then(Value::as_str)
                .and_then(|date| date.get(..4))
                .and_then(|year| year.parse::<u32>().ok());
            Some(AlbumCard {
                id: album.get("id").and_then(numeric_id)?,
                title: text_of(album, "title")?,
                version: text_of(album, "version"),
                year,
                kind: AlbumKind::from_value(album.get("albumType").or_else(|| album.get("type"))),
                artists: parse_artist_refs(item.get("artists").or_else(|| album.get("artists"))),
                cover_uri: cover_of(album.get("cover").and_then(|cover| cover.get("uri")))
                    .or_else(|| cover_of(album.get("coverUri"))),
                track_count: None,
                available: true,
            })
        })
        .collect()
}

/// «Собираем для вас»: `{"items":[{"data":{"playlist":…,"description":…}}]}`.
fn personal_playlists(value: &Value) -> Vec<PlaylistCard> {
    items_of(value, "items")
        .iter()
        .filter_map(|item| {
            let data = item.get("data")?;
            let mut card = playlist_card(data.get("playlist")?)?;
            card.note = text_of(data, "description");
            Some(card)
        })
        .collect()
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
        note: None,
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
            // Новые адреса отвечают без обёртки `result`.
            Request::TopArtists => return Some(bare(top_artists(empty))),
            Request::MetArtists => return Some(bare(met_artists(empty))),
            Request::NewReleases => return Some(bare(new_releases(empty))),
            Request::PersonalPlaylists => return Some(bare(personal_playlists(empty))),
            Request::History => history(empty, many),
        }))
    }

    fn bare(value: Value) -> (StatusCode, String) {
        (StatusCode::OK, value.to_string())
    }

    fn top_artists(empty: bool) -> Value {
        let trends = ["same", "up", "down", "new"];
        let artists: Vec<Value> = if empty {
            Vec::new()
        } else {
            ARTISTS
                .iter()
                .zip(1u64..)
                .zip(trends)
                .map(|(((id, name), place), trend)| {
                    json!({
                        "artist": artist_value(id, name),
                        "listenTimeSeconds": 13_110 / place,
                        "top": { "position": place, "progress": trend },
                    })
                })
                .collect()
        };
        json!({ "artists": artists })
    }

    /// История за 10 дней: исполнители из `ARTISTS` (чем выше в списке, тем
    /// чаще) и «из волны»; плюс трек без данных, «сборник» и исполнитель,
    /// записанный в треке дважды. `many` — ещё 300 исполнителей.
    fn history(empty: bool, many: bool) -> Value {
        if empty {
            return json!({ "historyTabs": [] });
        }
        let model = |id: u64, artists: Vec<Value>| {
            json!({ "type": "track", "data": {
                "itemId": { "trackId": id.to_string() },
                "fullModel": {
                    "id": id.to_string(),
                    "title": format!("Трек {id}"),
                    "durationMs": 200_000,
                    "artists": artists,
                },
            }})
        };
        let mut tabs = Vec::new();
        for day in 0..10u64 {
            let mut tracks = Vec::new();
            for (index, (id, name)) in (0u64..).zip(ARTISTS) {
                for play in 0..(4 - index) {
                    tracks.push(model(
                        day * 100 + index * 10 + play,
                        vec![artist_value(id, name)],
                    ));
                }
            }
            let wave = (2000 + day % 6).to_string();
            tracks.push(model(
                day * 100 + 90,
                vec![artist_value(&wave, &format!("Из волны {wave}"))],
            ));
            tracks.push(json!({ "type": "track", "data": { "itemId": { "trackId": "1" } } }));
            tracks.push(model(
                day * 100 + 91,
                vec![json!({ "id": 171, "name": "Various artists", "various": true })],
            ));
            if let Some((id, name)) = ARTISTS.get(1) {
                tracks.push(model(
                    day * 100 + 92,
                    vec![artist_value(id, name), artist_value(id, name)],
                ));
            }
            tabs.push(json!({
                "date": format!("2026-11-{:02}", 10 - day),
                "items": [{ "context": { "type": "wave" }, "tracks": tracks }],
            }));
        }
        if many {
            let tracks: Vec<Value> = (0..300u64)
                .map(|n| {
                    let id = (3000 + n).to_string();
                    model(9000 + n, vec![artist_value(&id, &format!("Ещё {n}"))])
                })
                .collect();
            tabs.push(json!({ "date": "2026-10-31", "items": [{ "tracks": tracks }] }));
        }
        json!({ "historyTabs": tabs })
    }

    fn met_artists(empty: bool) -> Value {
        let items: Vec<Value> = if empty {
            Vec::new()
        } else {
            (0..8u64)
                .map(|n| {
                    let id = (2000 + n).to_string();
                    json!({
                        "type": "artist_item",
                        "data": { "artist": artist_value(&id, &format!("Из волны {n}")) },
                    })
                })
                .chain([json!({ "type": "artist_item", "data": {} })])
                .collect()
        };
        json!({ "items": items })
    }

    fn new_releases(empty: bool) -> Value {
        let items: Vec<Value> = if empty {
            Vec::new()
        } else {
            (0..12u64)
                .map(|n| {
                    let id = 40_000 + n;
                    json!({
                        "album": {
                            "id": id,
                            "title": format!("Новинка {n}"),
                            "albumType": if n % 2 == 0 { "single" } else { "album" },
                            "cover": { "uri": format!("avatars.example/r{id}/%%") },
                        },
                        "artists": [artist_ref(n)],
                        "releaseDate": "2026-10-29T21:00:00+00:00",
                    })
                })
                .collect()
        };
        json!({ "newReleases": items })
    }

    fn personal_playlists(empty: bool) -> Value {
        let items: Vec<Value> = if empty {
            Vec::new()
        } else {
            [
                ("Плейлист дня", "Звучит по-вашему каждый день"),
                ("Дежавю", "Знакомит с тем, что вы ещё не слушали"),
                ("Премьера", "Открывает вам главные новинки"),
            ]
            .iter()
            .zip(0u64..)
            .map(|((title, note), n)| {
                json!({
                    "type": "personal_playlist_item",
                    "data": {
                        "playlist": {
                            "uid": 503_646_255,
                            "kind": 152_216_765 + n,
                            "title": title,
                            "cover": { "uri": format!("avatars.example/pp{n}/%%") },
                        },
                        "description": note,
                    },
                })
            })
            .collect()
        };
        json!({ "items": items })
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
        // Недавно отмеченный не первый в ответе: окно ставит его первым.
        let stamps = [
            "2024-02-19T08:28:09+00:00",
            "2026-11-05T14:58:52+00:00",
            "2025-06-01T10:00:00+00:00",
            "2024-04-13T11:46:38+00:00",
        ];
        Value::Array(
            ARTISTS
                .iter()
                .zip(stamps)
                .map(|((id, name), stamp)| {
                    json!({ "timestamp": stamp, "artist": artist_value(id, name) })
                })
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

    fn bare_sample(request: &Request) -> Value {
        let (status, body) = fixtures::reply("sample", request).unwrap();
        bare_envelope(status, &body).unwrap()
    }

    #[test]
    fn top_artists_carry_place_trend_and_time() {
        let top = top_artists(&bare_sample(&Request::TopArtists));
        assert_eq!(top.len(), 4);
        assert_eq!(top[0].card.name, "Король и Шут");
        assert_eq!(top[0].position, Some(1));
        assert_eq!(top[0].trend, Some(Trend::Same));
        assert_eq!(top[1].trend, Some(Trend::Up));
        assert_eq!(top[3].trend, Some(Trend::New));
        assert_eq!(top[0].listened, Duration::from_secs(13_110));
        // Ответ, как у сервиса: числовые id, без обёртки `result`.
        let live = serde_json::json!({"artists":[
            {"artist":{"id":36825,"name":"Radiohead","cover":{"uri":"avatars.example/x/%%"}},
             "listenTimeSeconds":13110,"top":{"position":1,"progress":"same"}},
            {"artist":{"name":"без id"},"listenTimeSeconds":5},
            {"artist":{"id":9,"name":"Без места"},"top":{"progress":"???"}}]});
        let top = top_artists(&live);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].card.id, "36825");
        assert_eq!(top[1].position, None);
        assert_eq!(top[1].trend, None);
        assert_eq!(top[1].listened, Duration::ZERO);
        assert!(top_artists(&serde_json::json!({"artists":"нет"})).is_empty());
    }

    #[test]
    fn listening_top_counts_history() {
        let (status, body) = fixtures::reply("sample", &Request::History).unwrap();
        let top = listening_top(&envelope(Scope::Collection, status, &body).unwrap());
        assert_eq!(top.days, 10);
        // Четыре исполнителя из `ARTISTS` и шесть «из волны»; «сборник» и
        // трек без данных не считаются.
        assert_eq!(top.artists.len(), 10);
        let first = &top.artists[0];
        assert_eq!(first.card.name, "Король и Шут");
        assert_eq!(first.position, Some(1));
        assert_eq!(first.plays, Some(40));
        assert_eq!(first.listened, Duration::from_secs(40 * 200));
        // Tame Impala: 3 трека в день и трек, где он записан дважды, — 40,
        // как у первого; при равенстве выше тот, кто в истории раньше.
        assert_eq!(top.artists[1].card.name, "Tame Impala");
        assert_eq!(top.artists[1].plays, Some(40));
        assert_eq!(top.artists[2].plays, Some(20));
        assert!(top.artists.iter().all(|artist| artist.card.id != "171"));
        let places: Vec<_> = top.artists.iter().filter_map(|a| a.position).collect();
        assert_eq!(places, (1..=10).collect::<Vec<u32>>());

        let empty = fixtures::reply("empty", &Request::History).unwrap();
        let empty = listening_top(&envelope(Scope::Collection, empty.0, &empty.1).unwrap());
        assert_eq!((empty.days, empty.artists.len()), (0, 0));
        // Мусор вместо дней и треков — пустой топ, не паника.
        let junk = serde_json::json!({"historyTabs":[1,{"items":"x"},{"items":[{"tracks":[
            {"data":{"fullModel":{"durationMs":-5,"artists":[{"id":"x"},{"id":7,"name":"Ок"}]}}},
            {"data":{"fullModel":{"durationMs":18446744073709551615u64,"artists":[{"id":7,"name":"Ок"}]}}}
        ]}]}]});
        let junk = listening_top(&junk);
        assert_eq!(junk.days, 3);
        assert_eq!(junk.artists.len(), 1);
        assert_eq!(junk.artists[0].plays, Some(2));
    }

    #[test]
    fn landing_blocks_are_parsed() {
        let met = met_artists(&bare_sample(&Request::MetArtists));
        assert_eq!(met.len(), 8, "элемент без исполнителя пропущен");
        let releases = new_releases(&bare_sample(&Request::NewReleases));
        assert_eq!(releases.len(), 12);
        assert_eq!(releases[0].kind, AlbumKind::Single);
        assert_eq!(releases[1].kind, AlbumKind::Album);
        assert_eq!(releases[0].year, Some(2026));
        assert_eq!(
            releases[0].cover_uri.as_deref(),
            Some("avatars.example/r40000/%%")
        );
        assert_eq!(releases[0].artists[0].name, "Король и Шут");
        let personal = personal_playlists(&bare_sample(&Request::PersonalPlaylists));
        assert_eq!(personal.len(), 3);
        assert_eq!(personal[0].title, "Плейлист дня");
        assert_eq!(
            personal[0].note.as_deref(),
            Some("Звучит по-вашему каждый день")
        );
        assert_eq!(personal[0].owner, "503646255");
    }

    #[test]
    fn bare_answers_accept_both_shapes() {
        let plain = bare_envelope(StatusCode::OK, r#"{"items":[]}"#).unwrap();
        assert!(plain.get("items").is_some());
        let wrapped = bare_envelope(StatusCode::OK, r#"{"result":{"items":[1]}}"#).unwrap();
        assert_eq!(wrapped["items"][0], 1);
        for bad in ["[]", "<html>", "\"text\""] {
            assert!(matches!(
                bare_envelope(StatusCode::OK, bad),
                Err(ApiError::InvalidResponse)
            ));
        }
        assert!(matches!(
            bare_envelope(StatusCode::NOT_FOUND, ""),
            Err(ApiError::NotFound)
        ));
        assert!(bare_envelope(StatusCode::SERVICE_UNAVAILABLE, "").is_err());
    }

    #[test]
    fn liked_lists_put_recent_first() {
        let liked =
            recent_first(&sample(&Request::LikedArtists(42)), "artist", artist_card).unwrap();
        let names: Vec<&str> = liked.iter().map(|card| card.name.as_str()).collect();
        assert_eq!(names[0], "Tame Impala");
        assert_eq!(names[1], "Оркестр");
        assert_eq!(names[3], "Король и Шут");
        // Без отметки — в конце, равные — в порядке сервиса.
        let mixed = serde_json::json!([{"id":1,"title":"A"},
            {"timestamp":"2025-01-01T00:00:00+00:00","album":{"id":2,"title":"B"}},
            {"timestamp":"2025-01-01T00:00:00+00:00","album":{"id":3,"title":"C"}},
            {"timestamp":"2026-01-01T00:00:00+00:00","album":{"id":4,"title":"D"}}]);
        let ids: Vec<String> = recent_first(&mixed, "album", album_card)
            .unwrap()
            .into_iter()
            .map(|album| album.id)
            .collect();
        assert_eq!(ids, ["4", "2", "3", "1"]);
        assert!(matches!(
            recent_first(&serde_json::json!({}), "album", album_card),
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
