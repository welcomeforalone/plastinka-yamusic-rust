//! Запросы к API музыки: пока только статус аккаунта и подписки, дальше
//! треки, плейлисты, поиск, «Моя волна». Запросы свои, на reqwest; крейт
//! `yandex-music` пока не подключён. Сетевые ошибки возвращаются как
//! значения и не роняют приложение.

mod account;

use reqwest::header::HeaderValue;

use crate::net::NetError;

pub use account::{
    Account, AccountStatus, Date, PlaybackPermit, PlusDetails, Subscription, fetch_status,
};

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
    /// Неверно задан отладочный хук `PLASTINKA_TEST_STATUS`.
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
