//! Запросы к API музыки: статус аккаунта и подписки, «Мне нравится», прямая
//! ссылка на звук, дальше плейлисты, поиск, «Моя волна». Запросы свои, на reqwest; крейт
//! `yandex-music` не подключён (почему — `docs/api/stage-4.md`). Сетевые
//! ошибки возвращаются как значения и не роняют приложение.

mod account;
mod catalog;
mod history;
mod lyrics;
mod play_audio;
mod rotor;
mod stream;
mod tracks;
mod wave_settings;

use reqwest::StatusCode;
use reqwest::header::HeaderValue;

use crate::net::NetError;

#[cfg(test)]
pub(crate) use account::sample_plus_status;
pub use account::{
    Account, AccountStatus, Date, PlaybackPermit, PlusDetails, Subscription, fetch_status,
};
pub use catalog::{
    ARTIST_TRACKS_PAGE, Album, AlbumCard, AlbumKind, Artist, ArtistCard, Found, ListeningTop,
    MyAlbums, MyArtists, MyPlaylists, Page, Playlist, PlaylistCard, SEARCH_LAST_PAGE, SearchKind,
    SearchResult, TopArtist, Trend, compact_number, fetch_album, fetch_artist, fetch_artist_tracks,
    fetch_listening_top, fetch_my_albums, fetch_my_artists, fetch_my_playlists, fetch_playlist,
    search, total_duration,
};
pub use history::{
    HistoryContext, HistoryDay, HistoryItem, Ymd, day_label, fetch_history, today_utc,
};
pub use lyrics::{LyricsFormat, fetch_lyrics_text};
pub use play_audio::{PlayReport, Tracker as PlayTracker, new_play_id, send_quietly};
pub use rotor::{
    QUEUE_LEN, WaveBatch, WaveColors, WaveEvent, WaveTrack, WaveTracks, fetch_activities,
    fetch_wave_tracks, queue_id, send_wave_feedback, start_wave,
};
pub use stream::{
    AudioQuality, Codec, StreamError, StreamInfo, StreamSource, fetch_mp3_source,
    fetch_stream_source,
};
pub use tracks::{
    ArtistRef, LikedIds, Loudness, LyricsInfo, TRACKS_BATCH, Track, Warning, add_disliked_tracks,
    add_liked_tracks, artists_text, fetch_liked_ids, fetch_tracks, normalization_gain,
    remove_liked_tracks,
};
pub(crate) use tracks::{clean, numeric_id, parse_artist_refs, plausible_id};
pub use wave_settings::{Activity, Diversity, Language, Mood, WaveSettings};

/// Ключ подписи запросов `get-file-info` и текстов (общий для приложений
/// Яндекса, не секрет пользователя).
pub(crate) const SIGN_KEY: &str = "p93jhgh689SBReK6ghtw62";

const API_BASE: &str = "https://api.music.yandex.net";

/// Идентификатор клиента для API музыки (заголовок `X-Yandex-Music-Client`).
/// Это не OAuth client_id из `auth`.
pub const MUSIC_CLIENT: &str = "YandexMusicAndroid/24023621";

/// Ошибки API музыки.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error(transparent)]
    Net(#[from] NetError),
    #[error("сервис музыки не принял токен")]
    Unauthorized,
    #[error("сервис музыки временно недоступен (код {0}). Повторите позже")]
    Server(u16),
    #[error("неожиданный ответ сервиса музыки (код {0})")]
    UnexpectedStatus(u16),
    #[error("сервис музыки прислал непонятный ответ")]
    InvalidResponse,
    /// Альбом, исполнитель или плейлист удалён или его нет в каталоге.
    #[error("не найдено: его удалили или убрали из каталога")]
    NotFound,
    /// Каталог отказал в доступе (например, чужой закрытый плейлист). Это
    /// не «токен не действует»: каталог отвечает и без токена.
    #[error("нет доступа: владелец закрыл его или он недоступен в вашем регионе")]
    Forbidden,
    /// Неверно задан отладочный хук `PLASTINKA_TEST_*`.
    #[cfg(debug_assertions)]
    #[error("{0}")]
    TestHook(String),
}

/// Заголовок `Authorization: OAuth <token>`, помеченный как секретный.
fn auth_header(token: &str) -> Result<HeaderValue, ApiError> {
    let mut value =
        HeaderValue::from_str(&format!("OAuth {token}")).map_err(|_| ApiError::Unauthorized)?;
    value.set_sensitive(true);
    Ok(value)
}

/// Общая проверка кода ответа: 401 и 403 — токен не действует, 5xx и 429 —
/// временный сбой.
fn check_status(status: StatusCode) -> Result<(), ApiError> {
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return Err(ApiError::Unauthorized);
    }
    if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
        return Err(ApiError::Server(status.as_u16()));
    }
    if !status.is_success() {
        return Err(ApiError::UnexpectedStatus(status.as_u16()));
    }
    Ok(())
}

/// Значение отладочного хука, подменяющего ответ сервиса. Хук работает только
/// для токена из `PLASTINKA_TEST_TOKEN`, чтобы поддельный ответ нельзя было
/// получить для настоящего токена (в том числе только что выданного при
/// `login`). Пустое значение хук выключает.
#[cfg(debug_assertions)]
fn test_hook(var: &str, token: &str) -> Result<Option<String>, ApiError> {
    let Some(name) = std::env::var(var).ok().filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let test_token = std::env::var("PLASTINKA_TEST_TOKEN").ok();
    if test_token
        .as_deref()
        .is_none_or(|test| test.is_empty() || test != token)
    {
        return Err(ApiError::TestHook(format!(
            "{var} работает только с фальшивым токеном из PLASTINKA_TEST_TOKEN"
        )));
    }
    Ok(Some(name))
}

/// Ошибка хука с неизвестным именем фикстуры.
#[cfg(debug_assertions)]
fn unknown_fixture(var: &str, known: &[&str]) -> ApiError {
    ApiError::TestHook(format!(
        "неизвестное значение {var}. Возможные: {}",
        known.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_with_bad_characters_is_rejected() {
        assert!(matches!(
            auth_header("bad\ntoken"),
            Err(ApiError::Unauthorized)
        ));
        assert!(auth_header("y0_good").unwrap().is_sensitive());
    }
}
