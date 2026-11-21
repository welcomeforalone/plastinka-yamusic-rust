//! «Мне нравится» и данные треков.
//!
//! Сначала `GET /users/{uid}/likes/tracks` даёт идентификаторы, потом
//! `POST /tracks` пачками — сами треки. Разбор мягкий: каждый трек
//! разбирается отдельно, непонятный становится заглушкой «недоступен».
//! Подробности в `docs/api/stage-4.md`.

use std::collections::HashMap;
use std::time::Duration;

use reqwest::StatusCode;
use reqwest::header::AUTHORIZATION;
use serde::Deserialize;
use serde_json::Value;

use super::{API_BASE, ApiError, MUSIC_CLIENT, auth_header, check_status};
use crate::net::NetError;

/// Сколько треков запрашивать за раз. Предела у сервиса не нашли, 250 —
/// около 600 КБ ответа.
pub const TRACKS_BATCH: usize = 250;

/// Сколько ответов 400 до первого успешного считаем признаком сломанного
/// запроса, а не плохого идентификатора.
const MAX_REJECTED_BEFORE_SUCCESS: usize = 32;

/// Сколько лишних запросов можно потратить на деление пачек с плохими
/// идентификаторами. Дальше отвергнутые пачки сразу становятся заглушками.
const SPLIT_BUDGET: usize = 128;

/// Идентификаторы треков из «Мне нравится».
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LikedIds {
    pub revision: Option<u64>,
    /// В порядке, который прислал сервис.
    pub ids: Vec<String>,
    /// Сколько элементов списка не удалось разобрать.
    pub unreadable: usize,
}

/// Исполнитель трека или альбома: имя и, если есть, идентификатор для
/// перехода на его страницу.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistRef {
    /// Только число: идентификатор попадает в адрес запроса.
    pub id: Option<String>,
    pub name: String,
}

/// Трек для списка.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    pub id: String,
    pub title: Option<String>,
    pub version: Option<String>,
    pub artists: Vec<ArtistRef>,
    pub album_id: Option<String>,
    pub album_title: Option<String>,
    pub duration: Option<Duration>,
    /// Только явное `available: true` без поля `error`.
    pub available: bool,
    /// Шаблон адреса обложки с `%%` вместо размера.
    pub cover_uri: Option<String>,
    /// Пометка о содержании (`contentWarning`).
    pub warning: Warning,
    /// Громкость трека (`r128`), если сервис её сообщил: для нормализации.
    pub loudness: Option<Loudness>,
    /// Есть ли текст песни у Яндекса (`lyricsInfo`). `None` — сервис не
    /// сообщил: текст надо пробовать запросить.
    pub lyrics: Option<LyricsInfo>,
}

/// Какие тексты песни есть у Яндекса (`lyricsInfo`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LyricsInfo {
    /// Есть текст с метками времени (LRC).
    pub synced: bool,
    /// Есть обычный текст.
    pub text: bool,
}

impl LyricsInfo {
    /// Из объекта трека. Нет `lyricsInfo` — берём `lyricsAvailable: true`
    /// (текст есть, вид неизвестен); иначе сервис ничего не сказал.
    fn parse(track: &Value) -> Option<Self> {
        if let Some(info) = track.get("lyricsInfo").filter(|info| info.is_object()) {
            let flag = |key: &str| info.get(key).and_then(Value::as_bool);
            let (synced, text) = (
                flag("hasAvailableSyncLyrics"),
                flag("hasAvailableTextLyrics"),
            );
            if synced.is_some() || text.is_some() {
                return Some(Self {
                    synced: synced.unwrap_or(false),
                    text: text.unwrap_or(false),
                });
            }
        }
        (track.get("lyricsAvailable").and_then(Value::as_bool) == Some(true)).then_some(Self {
            synced: true,
            text: true,
        })
    }

    /// Текста точно нет: запрашивать у Яндекса незачем.
    pub fn is_empty(self) -> bool {
        !self.synced && !self.text
    }
}

/// Громкость трека по EBU R128 (поле `r128` ответа).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Loudness {
    /// Интегральная громкость, LUFS.
    pub integrated: f64,
    /// Истинный пик, дБFS (dBTP).
    pub true_peak: f64,
}

// Числа конечны (см. `parse`), так что равенство рефлексивно.
impl Eq for Loudness {}

impl Loudness {
    /// Целевая громкость нормализации, LUFS. Допущение: так принято у
    /// потоковых сервисов, у Яндекса значение не документировано.
    const TARGET_LUFS: f64 = -14.0;
    /// Пик после усиления не выше этого, дБFS.
    const PEAK_CEILING: f64 = -1.0;
    const MIN_GAIN_DB: f64 = -15.0;
    const MAX_GAIN_DB: f64 = 6.0;

    /// Из объекта `{"i": …, "tp": …}`. Без обоих чисел — `None`.
    fn parse(value: Option<&Value>) -> Option<Self> {
        let value = value?;
        let number = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_f64)
                .filter(|number| number.is_finite())
        };
        Some(Self {
            integrated: number("i")?,
            true_peak: number("tp")?,
        })
    }

    /// Поправка громкости, дБ: к цели в -14 LUFS, но так, чтобы пик не
    /// вылез выше -1 дБFS; в пределах от -15 до +6 дБ.
    pub fn gain_db(self) -> f64 {
        let wanted = Self::TARGET_LUFS - self.integrated;
        let allowed = Self::PEAK_CEILING - self.true_peak;
        wanted
            .min(allowed)
            .clamp(Self::MIN_GAIN_DB, Self::MAX_GAIN_DB)
    }
}

/// Множитель громкости для трека: 1, если нормализация выключена или у
/// трека нет данных о громкости.
pub fn normalization_gain(loudness: Option<Loudness>, enabled: bool) -> f32 {
    match loudness.filter(|_| enabled) {
        // Множитель в пределах 0,18…2: в `f32` без потерь.
        Some(loudness) => 10_f64.powf(loudness.gain_db() / 20.0) as f32,
        None => 1.0,
    }
}

/// Пометка о содержании трека: `explicit` — есть ненормативная лексика,
/// `clean` — цензурная версия.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Warning {
    #[default]
    None,
    Explicit,
    Clean,
}

impl Warning {
    fn parse(value: Option<&Value>) -> Self {
        match value.and_then(Value::as_str) {
            Some("explicit") => Self::Explicit,
            Some("clean") => Self::Clean,
            _ => Self::None,
        }
    }
}

impl Track {
    /// Заглушка для трека, данные которого получить не удалось.
    pub(super) fn missing(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            title: None,
            version: None,
            artists: Vec::new(),
            album_id: None,
            album_title: None,
            duration: None,
            available: false,
            cover_uri: None,
            warning: Warning::None,
            loudness: None,
            lyrics: None,
        }
    }

    /// Название с версией: «Название (Live)».
    pub fn display_title(&self) -> String {
        let title = self.title.as_deref().unwrap_or("Без названия");
        match &self.version {
            Some(version) => format!("{title} ({version})"),
            None => title.to_owned(),
        }
    }

    /// Исполнители через запятую.
    pub fn artists_text(&self) -> String {
        artists_text(&self.artists)
    }

    /// Длительность `м:сс` или `ч:мм:сс`, без неё — `-:--`.
    pub fn duration_text(&self) -> String {
        let Some(duration) = self.duration else {
            return "-:--".to_owned();
        };
        let total = duration.as_secs();
        let (hours, minutes, seconds) = (total / 3600, total / 60 % 60, total % 60);
        if hours > 0 {
            format!("{hours}:{minutes:02}:{seconds:02}")
        } else {
            format!("{minutes}:{seconds:02}")
        }
    }
}

/// Идентификаторы треков из «Мне нравится» пользователя `uid`.
pub async fn fetch_liked_ids(
    http: &reqwest::Client,
    token: &str,
    uid: u64,
) -> Result<LikedIds, ApiError> {
    #[cfg(debug_assertions)]
    if let Some(fixture) = test_hook::fixture(token)? {
        return parse_liked_ids(fixture.likes.0, &fixture.likes.1);
    }
    let response = http
        .get(format!("{API_BASE}/users/{uid}/likes/tracks"))
        .header(AUTHORIZATION, auth_header(token)?)
        .header("X-Yandex-Music-Client", MUSIC_CLIENT)
        .send()
        .await
        .map_err(NetError::from)?;
    let status = response.status();
    let body = response.text().await.map_err(NetError::from)?;
    parse_liked_ids(status, &body)
}

/// «Не нравится» для треков `ids` пользователя `uid`: запись в аккаунт.
/// Окно зовёт её только после отдельного согласия пользователя (Slopless).
pub async fn add_disliked_tracks(
    http: &reqwest::Client,
    token: &str,
    uid: u64,
    ids: &[String],
) -> Result<(), ApiError> {
    if ids.is_empty() || !ids.iter().all(|id| plausible_id(id)) {
        return Ok(());
    }
    #[cfg(debug_assertions)]
    if crate::storage::test_hooks_active() {
        // С фальшивым токеном в аккаунт ничего не пишем.
        return Ok(());
    }
    let response = http
        .post(format!(
            "{API_BASE}/users/{uid}/dislikes/tracks/add-multiple"
        ))
        .header(AUTHORIZATION, auth_header(token)?)
        .header("X-Yandex-Music-Client", MUSIC_CLIENT)
        .form(&[("track-ids", ids.join(","))])
        .send()
        .await
        .map_err(NetError::from)?;
    check_status(response.status())
}

/// Поставить «Нравится» трекам `ids` (`id` или `id:albumId`) пользователя
/// `uid`: запись в аккаунт по явному нажатию сердца.
pub async fn add_liked_tracks(
    http: &reqwest::Client,
    token: &str,
    uid: u64,
    ids: &[String],
) -> Result<(), ApiError> {
    write_likes(http, token, uid, ids, "add-multiple").await
}

/// Снять «Нравится» с треков `ids` (`id` или `id:albumId`).
pub async fn remove_liked_tracks(
    http: &reqwest::Client,
    token: &str,
    uid: u64,
    ids: &[String],
) -> Result<(), ApiError> {
    write_likes(http, token, uid, ids, "remove").await
}

/// `POST /users/{uid}/likes/tracks/{action}`, форма `track-ids`. Ответ
/// `{"result":{"revision":N}}` нам не нужен: список обновляет окно.
async fn write_likes(
    http: &reqwest::Client,
    token: &str,
    uid: u64,
    ids: &[String],
    action: &str,
) -> Result<(), ApiError> {
    let Some(form) = like_ids(ids) else {
        return Ok(());
    };
    #[cfg(debug_assertions)]
    if crate::storage::test_hooks_active() {
        // С фальшивым токеном в аккаунт ничего не пишем.
        return Ok(());
    }
    let response = http
        .post(format!("{API_BASE}/users/{uid}/likes/tracks/{action}"))
        .header(AUTHORIZATION, auth_header(token)?)
        .header("X-Yandex-Music-Client", MUSIC_CLIENT)
        .form(&[("track-ids", form)])
        .send()
        .await
        .map_err(NetError::from)?;
    check_status(response.status())
}

/// Значение поля `track-ids`: идентификаторы через запятую. Пусто или есть
/// непохожий на настоящий (`id` или `id:albumId`) — `None`: такой запрос не
/// отправляем.
fn like_ids(ids: &[String]) -> Option<String> {
    let valid = !ids.is_empty()
        && ids.iter().all(|id| {
            let (track, album) = match id.split_once(':') {
                Some((track, album)) => (track, Some(album)),
                None => (id.as_str(), None),
            };
            plausible_id(track) && album.is_none_or(numeric_id_text)
        });
    valid.then(|| ids.join(","))
}

fn numeric_id_text(text: &str) -> bool {
    (1..=20).contains(&text.len()) && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// Исполнители через запятую, без них — «Неизвестный исполнитель».
pub fn artists_text(artists: &[ArtistRef]) -> String {
    if artists.is_empty() {
        return "Неизвестный исполнитель".to_owned();
    }
    let names: Vec<&str> = artists.iter().map(|artist| artist.name.as_str()).collect();
    names.join(", ")
}

/// Данные треков в порядке `ids`. Для каждого идентификатора — трек или
/// заглушка «недоступен». Запросы идут пачками по [`TRACKS_BATCH`];
/// `progress` получает, сколько идентификаторов уже обработано.
pub async fn fetch_tracks(
    http: &reqwest::Client,
    token: &str,
    ids: &[String],
    progress: impl FnMut(usize),
) -> Result<Vec<Track>, ApiError> {
    #[cfg(debug_assertions)]
    if let Some(fixture) = test_hook::fixture(token)? {
        let (status, body) = fixture.tracks;
        return load_tracks(
            ids,
            |_| std::future::ready(Ok((status, body.clone()))),
            progress,
        )
        .await;
    }
    // Пачка и клиент передаются во владение: так future запроса можно
    // отправить в другой поток (окно запускает загрузку в фоне).
    load_tracks(
        ids,
        |batch| {
            let (http, token) = (http.clone(), token.to_owned());
            async move { post_tracks(&http, &token, &batch).await }
        },
        progress,
    )
    .await
}

pub(super) async fn post_tracks(
    http: &reqwest::Client,
    token: &str,
    ids: &[String],
) -> Result<(StatusCode, String), ApiError> {
    let response = http
        .post(format!("{API_BASE}/tracks"))
        .header(AUTHORIZATION, auth_header(token)?)
        .header("X-Yandex-Music-Client", MUSIC_CLIENT)
        .form(&[
            ("track-ids", ids.join(",")),
            ("with-positions", "false".to_owned()),
        ])
        .send()
        .await
        .map_err(NetError::from)?;
    let status = response.status();
    let body = response.text().await.map_err(NetError::from)?;
    Ok((status, body))
}

/// Запрашивает треки пачками. Сервис отвечает 400 на всю пачку, если в ней
/// есть плохой идентификатор, поэтому такую пачку делим пополам.
///
/// Сломанный запрос отличаем от плохих идентификаторов по всему списку, а не
/// по одной пачке: ни одного успешного ответа на несколько треков (или
/// слишком много отказов до первого успеха) — ошибка.
pub(super) async fn load_tracks<F>(
    ids: &[String],
    mut send: impl FnMut(Vec<String>) -> F,
    mut progress: impl FnMut(usize),
) -> Result<Vec<Track>, ApiError>
where
    F: Future<Output = Result<(StatusCode, String), ApiError>>,
{
    let broken = || ApiError::UnexpectedStatus(StatusCode::BAD_REQUEST.as_u16());
    let valid: Vec<String> = ids.iter().filter(|id| plausible_id(id)).cloned().collect();
    let mut queue: Vec<&[String]> = valid.chunks(TRACKS_BATCH).rev().collect();
    let mut found: HashMap<String, Track> = HashMap::new();
    let (mut rejected, mut splits, mut done) = (0, 0, ids.len().saturating_sub(valid.len()));
    let mut accepted = false;
    while let Some(batch) = queue.pop() {
        let (status, body) = send(batch.to_vec()).await?;
        if status == StatusCode::BAD_REQUEST {
            rejected += 1;
            if !accepted && rejected > MAX_REJECTED_BEFORE_SUCCESS {
                return Err(broken());
            }
            // Один трек делить некуда, а бюджет на деление кончился: такие
            // треки станут заглушками.
            let halves = batch
                .split_at_checked(batch.len() / 2)
                .filter(|(left, _)| !left.is_empty() && splits < SPLIT_BUDGET);
            if let Some((left, right)) = halves {
                splits += 1;
                queue.push(right);
                queue.push(left);
                continue;
            }
        } else {
            accepted = true;
            for track in parse_tracks(status, &body)? {
                found.entry(track.id.clone()).or_insert(track);
            }
        }
        done += batch.len();
        progress(done);
    }
    if !accepted && valid.len() > 1 {
        return Err(broken());
    }
    Ok(ids
        .iter()
        .map(|id| found.get(id).cloned().unwrap_or_else(|| Track::missing(id)))
        .collect())
}

/// Сервис принимает число от 1 до `i32::MAX` или UUID (так выглядят
/// загруженные пользователем треки). Остальное ломает всю пачку.
pub(crate) fn plausible_id(id: &str) -> bool {
    if id.bytes().all(|byte| byte.is_ascii_digit()) {
        return id.parse::<i32>().is_ok_and(|number| number > 0);
    }
    id.len() == 36
        && id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
}

#[derive(Deserialize)]
struct LikesEnvelope {
    result: LikesResult,
}

#[derive(Deserialize)]
struct LikesResult {
    library: Library,
}

#[derive(Deserialize)]
struct Library {
    #[serde(default)]
    revision: Option<Value>,
    tracks: Vec<Value>,
}

fn parse_liked_ids(status: StatusCode, body: &str) -> Result<LikedIds, ApiError> {
    check_status(status)?;
    let envelope: LikesEnvelope =
        serde_json::from_str(body).map_err(|_| ApiError::InvalidResponse)?;
    let library = envelope.result.library;
    let mut ids = Vec::with_capacity(library.tracks.len());
    let mut unreadable = 0;
    for item in &library.tracks {
        match item.get("id").and_then(id_text) {
            Some(id) => ids.push(id),
            None => unreadable += 1,
        }
    }
    // Список не пустой, но ни один элемент не разобран: это не «пусто».
    if ids.is_empty() && unreadable > 0 {
        return Err(ApiError::InvalidResponse);
    }
    Ok(LikedIds {
        revision: library.revision.as_ref().and_then(Value::as_u64),
        ids,
        unreadable,
    })
}

#[derive(Deserialize)]
struct TracksEnvelope {
    result: Vec<Value>,
}

pub(super) fn parse_tracks(status: StatusCode, body: &str) -> Result<Vec<Track>, ApiError> {
    check_status(status)?;
    let envelope: TracksEnvelope =
        serde_json::from_str(body).map_err(|_| ApiError::InvalidResponse)?;
    Ok(envelope.result.iter().filter_map(parse_track).collect())
}

/// Трек из ответа. Нет понятного `id` — `None`.
pub(super) fn parse_track(value: &Value) -> Option<Track> {
    let id = value.get("id").and_then(id_text)?;
    let text = |key: &str| value.get(key).and_then(Value::as_str).and_then(clean);
    let album = value
        .get("albums")
        .and_then(Value::as_array)
        .and_then(|albums| albums.first());
    let artists = parse_artist_refs(value.get("artists"));
    let error = value.get("error").is_some_and(|error| !error.is_null());
    Some(Track {
        title: text("title"),
        version: text("version"),
        artists,
        album_id: album.and_then(|album| album.get("id")).and_then(numeric_id),
        album_title: album
            .and_then(|album| album.get("title"))
            .and_then(Value::as_str)
            .and_then(clean),
        duration: value
            .get("durationMs")
            .and_then(Value::as_u64)
            .map(Duration::from_millis),
        // Отладочный хук `PLASTINKA_TEST_UNAVAILABLE` объявляет трек недоступным.
        available: value.get("available").and_then(Value::as_bool) == Some(true)
            && !error
            && !crate::relink::forced_unavailable(&id),
        cover_uri: text("coverUri"),
        warning: Warning::parse(value.get("contentWarning")),
        loudness: Loudness::parse(value.get("r128")),
        lyrics: LyricsInfo::parse(value),
        id,
    })
}

/// Исполнители из массива `artists`: у каждого имя, идентификатор — если
/// это число. Без имени исполнитель пропускается.
pub(crate) fn parse_artist_refs(value: Option<&Value>) -> Vec<ArtistRef> {
    value
        .and_then(Value::as_array)
        .map(|artists| {
            artists
                .iter()
                .filter_map(|artist| {
                    let name = artist.get("name").and_then(Value::as_str).and_then(clean)?;
                    Some(ArtistRef {
                        id: artist.get("id").and_then(numeric_id),
                        name,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Числовой идентификатор (альбома, исполнителя, пользователя, плейлиста)
/// строкой или числом: от 1 до 20 цифр. Он попадает в адрес следующего
/// запроса, поэтому другие символы не принимаем.
pub(crate) fn numeric_id(value: &Value) -> Option<String> {
    let text = match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.as_u64()?.to_string(),
        _ => return None,
    };
    let digits = (1..=20).contains(&text.len()) && text.bytes().all(|byte| byte.is_ascii_digit());
    (digits && text.bytes().any(|byte| byte != b'0')).then_some(text)
}

/// Идентификатор строкой или неотрицательным целым числом. Идентификатор
/// попадает в вывод, поэтому берём только короткие строки из видимых
/// символов ASCII: настоящие идентификаторы — числа и UUID.
pub(super) fn id_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text)
            if (1..=64).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_graphic()) =>
        {
            Some(text.clone())
        }
        Value::Number(number) => number.as_u64().map(|number| number.to_string()),
        _ => None,
    }
}

/// Текст из ответа для показа: без управляющих символов и символов смены
/// направления письма (они могли бы испортить вывод в терминал) и лишних
/// пробелов. Пустой — `None`.
pub(crate) fn clean(text: &str) -> Option<String> {
    let cleaned: String = text
        .chars()
        .map(|ch| if unsafe_for_output(ch) { ' ' } else { ch })
        .collect();
    let trimmed = cleaned.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn unsafe_for_output(ch: char) -> bool {
    ch.is_control()
        || matches!(
            ch,
            '\u{2028}' | '\u{2029}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
        )
}

/// Готовые ответы сервиса: для юнит-тестов и отладочного хука.
#[cfg(any(test, debug_assertions))]
mod fixtures {
    use reqwest::StatusCode;

    pub struct Fixture {
        pub likes: (StatusCode, String),
        pub tracks: (StatusCode, String),
    }

    pub const NAMES: &[&str] = &[
        "empty",
        "sample",
        "many",
        "401",
        "503",
        "broken",
        "tracks-400",
        "tracks-503",
        "tracks-broken",
    ];

    /// Сколько треков в фикстуре `many`.
    pub const MANY: u32 = 1200;

    pub const SAMPLE_LIKES: &str = r#"{"result":{"library":{"uid":42,"revision":7,"tracks":[
        {"id":"29238706","albumId":"10101","timestamp":"2026-10-01T12:00:00+00:00"},
        {"id":7019818,"albumId":"777"},
        {"id":"1"},
        {"id":"2"},
        {"id":"0f6c8a5e-1b2c-4d3e-9f00-123456789abc"},
        {"id":"4294967296"},
        {"id":"3"},
        {"albumId":"5"},
        {"id":"999"}
    ]}}}"#;

    pub const SAMPLE_TRACKS: &str = r#"{"result":[
        {"id":"29238706","realId":"29238706","title":"Дурак и молния","available":true,
         "durationMs":114330,"coverUri":"avatars.example/abc/%%",
         "artists":[{"id":41052,"name":"Король и Шут"}],
         "albums":[{"id":10101,"title":"Камнем по голове"}]},
        {"id":"7019818","title":"Sempre libera","version":"Live","available":true,
         "durationMs":3725000,
         "artists":[{"name":"Солистка"},{"name":"Оркестр"},{"id":5}],
         "albums":[]},
        {"id":"1","title":"Без исполнителя\u001b[31m","available":true},
        {"id":"2","title":"Снят с каталога","available":false,"durationMs":60000,
         "artists":[{"name":"Кто-то"}]},
        {"id":"0f6c8a5e-1b2c-4d3e-9f00-123456789abc","error":"not-found"},
        {"id":"3","title":17,"artists":"не список","durationMs":-5,"available":"да"},
        {"title":"без id"}
    ]}"#;

    pub fn fixture(name: &str) -> Option<Fixture> {
        let sample_likes = (StatusCode::OK, SAMPLE_LIKES.to_owned());
        let ok_tracks = (StatusCode::OK, SAMPLE_TRACKS.to_owned());
        let fixture = match name {
            "empty" => Fixture {
                likes: (
                    StatusCode::OK,
                    r#"{"result":{"library":{"uid":42,"revision":1,"tracks":[]}}}"#.to_owned(),
                ),
                tracks: ok_tracks,
            },
            "sample" => Fixture {
                likes: sample_likes,
                tracks: ok_tracks,
            },
            "many" => many(),
            "401" => Fixture {
                likes: (StatusCode::UNAUTHORIZED, r#"{"status":401}"#.to_owned()),
                tracks: ok_tracks,
            },
            "503" => Fixture {
                likes: (StatusCode::SERVICE_UNAVAILABLE, String::new()),
                tracks: ok_tracks,
            },
            "broken" => Fixture {
                likes: (StatusCode::OK, r#"{"result":{"library":{}}}"#.to_owned()),
                tracks: ok_tracks,
            },
            "tracks-400" => Fixture {
                likes: sample_likes,
                tracks: (
                    StatusCode::BAD_REQUEST,
                    r#"{"error":{"name":"validate"}}"#.to_owned(),
                ),
            },
            "tracks-503" => Fixture {
                likes: sample_likes,
                tracks: (StatusCode::SERVICE_UNAVAILABLE, String::new()),
            },
            "tracks-broken" => Fixture {
                likes: sample_likes,
                tracks: (StatusCode::OK, "<html>".to_owned()),
            },
            _ => return None,
        };
        Some(fixture)
    }

    /// Большой список: проверка пачек и прогресса.
    fn many() -> Fixture {
        let ids = 100_001..=100_000 + MANY;
        let likes: Vec<String> = ids
            .clone()
            .map(|id| format!(r#"{{"id":"{id}"}}"#))
            .collect();
        let tracks: Vec<String> = ids
            .map(|id| {
                format!(
                    r#"{{"id":"{id}","title":"Трек {id}","available":true,"durationMs":{},
                    "artists":[{{"name":"Исполнитель {}"}}]}}"#,
                    (id % 400 + 60) * 1000,
                    id % 50
                )
            })
            .collect();
        Fixture {
            likes: (
                StatusCode::OK,
                format!(
                    r#"{{"result":{{"library":{{"uid":42,"revision":3,"tracks":[{}]}}}}}}"#,
                    likes.join(",")
                ),
            ),
            tracks: (
                StatusCode::OK,
                format!(r#"{{"result":[{}]}}"#, tracks.join(",")),
            ),
        }
    }
}

/// Отладочный хук `PLASTINKA_TEST_LIKES`: вместо запросов к сервису разбираем
/// готовые ответы для списка и для треков. Условия — у `api::test_hook`.
#[cfg(debug_assertions)]
mod test_hook {
    use super::fixtures::{self, Fixture};
    use crate::api::ApiError;

    const VAR: &str = "PLASTINKA_TEST_LIKES";

    pub fn fixture(token: &str) -> Result<Option<Fixture>, ApiError> {
        let Some(name) = crate::api::test_hook(VAR, token)? else {
            return Ok(None);
        };
        fixtures::fixture(&name)
            .map(Some)
            .ok_or_else(|| crate::api::unknown_fixture(VAR, fixtures::NAMES))
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    #[test]
    fn lyrics_info_is_parsed_softly() {
        let info = |json: &str| LyricsInfo::parse(&serde_json::from_str(json).unwrap());
        assert_eq!(
            info(
                r#"{"lyricsInfo":{"hasAvailableSyncLyrics":true,"hasAvailableTextLyrics":false}}"#
            ),
            Some(LyricsInfo {
                synced: true,
                text: false
            })
        );
        let none = info(
            r#"{"lyricsInfo":{"hasAvailableSyncLyrics":false,"hasAvailableTextLyrics":false}}"#,
        );
        assert!(none.is_some_and(LyricsInfo::is_empty));
        // Нет `lyricsInfo`: `lyricsAvailable: true` — есть, вид неизвестен.
        assert_eq!(
            info(r#"{"lyricsAvailable":true}"#),
            Some(LyricsInfo {
                synced: true,
                text: true
            })
        );
        // Ничего не сказано или поля неверного типа — неизвестно.
        assert_eq!(info(r#"{"lyricsAvailable":false}"#), None);
        assert_eq!(info(r#"{}"#), None);
        assert_eq!(info(r#"{"lyricsInfo":"да"}"#), None);
        assert_eq!(info(r#"{"lyricsInfo":{}}"#), None);
    }

    fn sample_ids() -> LikedIds {
        parse_liked_ids(StatusCode::OK, fixtures::SAMPLE_LIKES).unwrap()
    }

    fn no_progress(_: usize) {}

    async fn load_sample(ids: &[String]) -> Vec<Track> {
        load_tracks(
            ids,
            |_| std::future::ready(Ok((StatusCode::OK, fixtures::SAMPLE_TRACKS.to_owned()))),
            no_progress,
        )
        .await
        .unwrap()
    }

    /// Ответ сервиса, который отвергает пачки с идентификаторами из `bad`.
    fn reply(batch: &[String], bad: &[&str]) -> (StatusCode, String) {
        if batch.iter().any(|id| bad.contains(&id.as_str())) {
            return (StatusCode::BAD_REQUEST, String::new());
        }
        let items: Vec<String> = batch
            .iter()
            .map(|id| format!(r#"{{"id":"{id}","title":"т{id}","available":true}}"#))
            .collect();
        (
            StatusCode::OK,
            format!(r#"{{"result":[{}]}}"#, items.join(",")),
        )
    }

    fn numbered(count: usize) -> Vec<String> {
        (1..=count).map(|id| id.to_string()).collect()
    }

    #[test]
    fn liked_ids_keep_order_and_count_unreadable() {
        let liked = sample_ids();
        assert_eq!(liked.revision, Some(7));
        assert_eq!(liked.unreadable, 1);
        assert_eq!(
            liked.ids,
            [
                "29238706",
                "7019818",
                "1",
                "2",
                "0f6c8a5e-1b2c-4d3e-9f00-123456789abc",
                "4294967296",
                "3",
                "999"
            ]
        );
    }

    #[test]
    fn empty_library_is_fine() {
        let liked = parse_liked_ids(
            StatusCode::OK,
            r#"{"result":{"library":{"uid":1,"tracks":[]}}}"#,
        )
        .unwrap();
        assert!(liked.ids.is_empty());
        assert_eq!(liked.revision, None);
    }

    fn loudness(integrated: f64, true_peak: f64) -> Loudness {
        Loudness {
            integrated,
            true_peak,
        }
    }

    #[test]
    fn r128_is_parsed_when_complete() {
        let track = parse_track(&serde_json::json!({
            "id": "1", "r128": {"i": -9.5, "tp": 0.4}
        }))
        .unwrap();
        assert_eq!(track.loudness, Some(loudness(-9.5, 0.4)));
        let half = parse_track(&serde_json::json!({"id": "1", "r128": {"i": -9.5}})).unwrap();
        assert_eq!(half.loudness, None);
        let none = parse_track(&serde_json::json!({"id": "1"})).unwrap();
        assert_eq!(none.loudness, None);
    }

    #[test]
    fn normalization_gain_is_bounded() {
        // Нет данных или выключено — без поправки.
        assert!((normalization_gain(None, true) - 1.0).abs() < 1e-6);
        assert!((normalization_gain(Some(loudness(-9.0, -3.0)), false) - 1.0).abs() < 1e-6);
        // Громкий трек (-9 LUFS) тише на 5 дБ.
        let loud = loudness(-9.0, -0.5).gain_db();
        assert!((loud + 5.0).abs() < 1e-9, "{loud}");
        // Тихий трек (-20 LUFS) хочет +6, но пик -2 даёт запас только +1.
        let quiet = loudness(-20.0, -2.0).gain_db();
        assert!((quiet - 1.0).abs() < 1e-9, "{quiet}");
        // Тихий с низким пиком: потолок усиления +6.
        assert!((loudness(-30.0, -20.0).gain_db() - 6.0).abs() < 1e-9);
        // Очень громкий: не тише -15.
        assert!((loudness(5.0, 3.0).gain_db() + 15.0).abs() < 1e-9);
        let factor = normalization_gain(Some(loudness(-8.0, 0.0)), true);
        assert!((factor - 10_f32.powf(-6.0 / 20.0)).abs() < 1e-4);
    }

    #[test]
    fn likes_errors_are_mapped() {
        assert!(matches!(
            parse_liked_ids(StatusCode::FORBIDDEN, ""),
            Err(ApiError::Unauthorized)
        ));
        assert!(matches!(
            parse_liked_ids(StatusCode::BAD_GATEWAY, ""),
            Err(ApiError::Server(502))
        ));
        for broken in [
            "",
            "<html>",
            r#"{"result":{}}"#,
            r#"{"result":{"library":{}}}"#,
            r#"{"result":{"library":{"tracks":{}}}}"#,
        ] {
            assert!(
                matches!(
                    parse_liked_ids(StatusCode::OK, broken),
                    Err(ApiError::InvalidResponse)
                ),
                "{broken}"
            );
        }
    }

    #[tokio::test]
    async fn tracks_are_parsed_leniently_in_request_order() {
        let tracks = load_sample(&sample_ids().ids).await;
        assert_eq!(tracks.len(), 8);
        let [normal, live, no_artist, gone, uuid, bad_id, odd, absent] = tracks.as_slice() else {
            panic!("ожидалось 8 треков");
        };

        assert_eq!(normal.artists_text(), "Король и Шут");
        assert_eq!(normal.display_title(), "Дурак и молния");
        assert_eq!(normal.album_title.as_deref(), Some("Камнем по голове"));
        assert_eq!(normal.album_id.as_deref(), Some("10101"));
        assert_eq!(normal.duration_text(), "1:54");
        assert!(normal.available);

        assert_eq!(live.display_title(), "Sempre libera (Live)");
        assert_eq!(live.artists_text(), "Солистка, Оркестр");
        assert_eq!(live.duration_text(), "1:02:05");
        assert_eq!(live.album_title, None);

        assert_eq!(no_artist.artists_text(), "Неизвестный исполнитель");
        assert_eq!(no_artist.display_title(), "Без исполнителя [31m");
        assert_eq!(no_artist.duration_text(), "-:--");

        assert!(!gone.available);
        assert!(!uuid.available);
        assert_eq!(uuid.display_title(), "Без названия");

        assert_eq!(bad_id, &Track::missing("4294967296"));
        assert!(!odd.available);
        assert_eq!(odd.title, None);
        assert_eq!(odd.duration, None);
        assert_eq!(absent, &Track::missing("999"));
    }

    #[tokio::test]
    async fn implausible_ids_are_never_sent() {
        let sent = RefCell::new(Vec::new());
        let ids: Vec<String> = ["5", "0", "-1", "2147483648", "abc", "", "12"]
            .map(str::to_owned)
            .into();
        let tracks = load_tracks(
            &ids,
            |batch: Vec<String>| {
                sent.borrow_mut().extend(batch);
                std::future::ready(Ok((StatusCode::OK, r#"{"result":[]}"#.to_owned())))
            },
            no_progress,
        )
        .await
        .unwrap();
        assert_eq!(sent.into_inner(), ["5", "12"]);
        assert_eq!(tracks.len(), ids.len());
        assert!(tracks.iter().all(|track| !track.available));
    }

    /// Загружает `ids`, отвергая пачки с `bad`. Возвращает недоступные
    /// треки (или ошибку), число запросов и последний прогресс.
    async fn load_with_bad(
        ids: &[String],
        bad: &[&str],
    ) -> (Result<Vec<String>, ApiError>, usize, usize) {
        let requests = RefCell::new(0);
        let last_progress = RefCell::new(0);
        let result = load_tracks(
            ids,
            |batch: Vec<String>| {
                *requests.borrow_mut() += 1;
                std::future::ready(Ok(reply(&batch, bad)))
            },
            |done| *last_progress.borrow_mut() = done,
        )
        .await
        .map(|tracks| {
            tracks
                .into_iter()
                .filter(|track| !track.available)
                .map(|track| track.id)
                .collect()
        });
        (result, requests.into_inner(), last_progress.into_inner())
    }

    #[tokio::test]
    async fn rejected_batch_is_split_until_bad_id_is_isolated() {
        let (result, requests, progress) = load_with_bad(&numbered(600), &["300"]).await;
        assert_eq!(result.unwrap(), ["300"]);
        assert!(requests < 40, "{requests}");
        assert_eq!(progress, 600);
    }

    #[tokio::test]
    async fn bad_ids_at_batch_start_do_not_abort() {
        // Семь плохих подряд в начале пачки: до первого успеха много отказов.
        let bad = ["1", "2", "3", "4", "5", "6", "7"];
        let (result, _, _) = load_with_bad(&numbered(600), &bad).await;
        assert_eq!(result.unwrap(), bad);
    }

    #[tokio::test]
    async fn rejected_tail_batch_does_not_break_list() {
        // 252 трека: вторая пачка из двух, и сервис отвергает оба.
        let (result, _, _) = load_with_bad(&numbered(252), &["251", "252"]).await;
        assert_eq!(result.unwrap(), ["251", "252"]);
    }

    #[tokio::test]
    async fn split_budget_limits_requests() {
        // Каждый десятый трек плохой: делить до конца слишком дорого.
        let ids = numbered(1000);
        let bad: Vec<String> = ids.iter().step_by(10).cloned().collect();
        let bad: Vec<&str> = bad.iter().map(String::as_str).collect();
        let (result, requests, progress) = load_with_bad(&ids, &bad).await;
        let unavailable = result.unwrap();
        assert!(bad.iter().all(|id| unavailable.contains(&id.to_string())));
        assert!(requests <= 4 + 2 * SPLIT_BUDGET + 1, "{requests}");
        assert_eq!(progress, 1000);
    }

    #[tokio::test]
    async fn constant_rejection_stops_with_error() {
        let ids = numbered(600);
        let all: Vec<&str> = ids.iter().map(String::as_str).collect();
        let (result, requests, _) = load_with_bad(&ids, &all).await;
        assert!(matches!(result, Err(ApiError::UnexpectedStatus(400))));
        assert_eq!(requests, MAX_REJECTED_BEFORE_SUCCESS + 1);

        // Небольшой список до предела не доходит, но и заглушками не
        // становится.
        let few = numbered(5);
        let all: Vec<&str> = few.iter().map(String::as_str).collect();
        let (result, _, _) = load_with_bad(&few, &all).await;
        assert!(matches!(result, Err(ApiError::UnexpectedStatus(400))));

        // Один плохой трек во всём списке — просто заглушка.
        let (result, _, _) = load_with_bad(&numbered(1), &["1"]).await;
        assert_eq!(result.unwrap(), ["1"]);
    }

    #[tokio::test]
    async fn tracks_errors_are_mapped() {
        let ids = vec!["5".to_owned()];
        for (status, body, expected) in [
            (StatusCode::UNAUTHORIZED, "", "Unauthorized"),
            (StatusCode::SERVICE_UNAVAILABLE, "", "Server(503)"),
            (StatusCode::OK, "<html>", "InvalidResponse"),
            (StatusCode::OK, r#"{"result":{}}"#, "InvalidResponse"),
        ] {
            let result = load_tracks(
                &ids,
                |_| std::future::ready(Ok((status, body.to_owned()))),
                no_progress,
            )
            .await;
            assert_eq!(format!("{:?}", result.unwrap_err()), expected);
        }
    }

    #[tokio::test]
    async fn empty_list_sends_nothing() {
        let tracks = load_tracks(
            &[],
            |_| -> std::future::Ready<Result<(StatusCode, String), ApiError>> {
                panic!("запроса быть не должно")
            },
            no_progress,
        )
        .await
        .unwrap();
        assert!(tracks.is_empty());
    }

    #[test]
    fn unreadable_only_list_is_not_empty() {
        assert!(matches!(
            parse_liked_ids(
                StatusCode::OK,
                r#"{"result":{"library":{"tracks":[{"albumId":"5"},{"id":null}]}}}"#
            ),
            Err(ApiError::InvalidResponse)
        ));
    }

    #[test]
    fn output_is_cleaned() {
        assert_eq!(
            clean("a\u{202E}b\u{2028}c\u{0007}d\u{85}e").as_deref(),
            Some("a b c d e")
        );
        assert_eq!(clean(" \u{2066} "), None);
        let long = "1".repeat(65);
        for id in [long.as_str(), "1 2", "трек", "a\u{202E}"] {
            assert_eq!(id_text(&Value::String(id.to_owned())), None, "{id}");
        }
    }

    #[test]
    fn ids_with_control_characters_are_unreadable() {
        let liked = parse_liked_ids(
            StatusCode::OK,
            r#"{"result":{"library":{"tracks":[{"id":"1\u001b[2J"},{"id":-3},{"id":2.5},{"id":"4"}]}}}"#,
        )
        .unwrap();
        assert_eq!(liked.ids, ["4"]);
        assert_eq!(liked.unreadable, 3);
    }

    #[test]
    fn like_ids_accept_only_real_ids() {
        let ids = |list: &[&str]| list.iter().map(|id| (*id).to_owned()).collect::<Vec<_>>();
        assert_eq!(like_ids(&ids(&["1", "2:3"])).as_deref(), Some("1,2:3"));
        assert_eq!(like_ids(&[]), None);
        for bad in ["", "0", "abc", "1:", "1:x", "1:2:3", "1,2", ":5", "1 :2"] {
            assert_eq!(like_ids(&ids(&["5", bad])), None, "{bad}");
        }
    }

    #[test]
    fn like_status_is_mapped() {
        assert!(check_status(StatusCode::OK).is_ok());
        assert!(matches!(
            check_status(StatusCode::UNAUTHORIZED),
            Err(ApiError::Unauthorized)
        ));
        assert!(matches!(
            check_status(StatusCode::BAD_GATEWAY),
            Err(ApiError::Server(502))
        ));
    }

    #[test]
    fn plausible_ids() {
        for good in [
            "1",
            "2147483647",
            "007019818",
            "0f6c8a5e-1b2c-4d3e-9f00-123456789abc",
        ] {
            assert!(plausible_id(good), "{good}");
        }
        for bad in [
            "",
            "0",
            "2147483648",
            "-5",
            "abc",
            "12 ",
            "0f6c8a5e-1b2c-4d3e-9f00-123456789abz",
        ] {
            assert!(!plausible_id(bad), "{bad}");
        }
    }

    #[test]
    fn every_fixture_exists() {
        for name in fixtures::NAMES {
            assert!(fixtures::fixture(name).is_some(), "{name}");
        }
        let many = fixtures::fixture("many").unwrap();
        let liked = parse_liked_ids(many.likes.0, &many.likes.1).unwrap();
        assert_eq!(liked.ids.len(), fixtures::MANY as usize);
        assert_eq!(
            parse_tracks(many.tracks.0, &many.tracks.1).unwrap().len(),
            fixtures::MANY as usize
        );
    }
}
