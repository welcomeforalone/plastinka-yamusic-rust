//! Запросы к API музыки: пока только статус аккаунта, дальше треки,
//! плейлисты, поиск, «Моя волна». Запросы свои, на reqwest; крейт
//! `yandex-music` пока не подключён. Сетевые ошибки возвращаются как
//! значения и не роняют приложение.

use reqwest::StatusCode;
use reqwest::header::{AUTHORIZATION, HeaderValue};
use serde::Deserialize;

use crate::net::NetError;

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
}

/// Аккаунт, под которым выполнен вход.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub uid: u64,
    pub login: Option<String>,
    pub display_name: Option<String>,
}

impl Account {
    /// Как назвать аккаунт пользователю.
    pub fn name(&self) -> String {
        match (&self.display_name, &self.login) {
            (Some(name), Some(login)) if name != login => format!("{name} ({login})"),
            (_, Some(login)) => login.clone(),
            (Some(name), None) => name.clone(),
            (None, None) => format!("uid {}", self.uid),
        }
    }
}

/// Проверяет токен и возвращает аккаунт (`GET /account/status`).
pub async fn fetch_account(http: &reqwest::Client, token: &str) -> Result<Account, ApiError> {
    let response = http
        .get(format!("{API_BASE}/account/status"))
        .header(AUTHORIZATION, auth_header(token)?)
        .header("X-Yandex-Music-Client", MUSIC_CLIENT)
        .send()
        .await
        .map_err(NetError::from)?;
    let status = response.status();
    let body = response.text().await.map_err(NetError::from)?;
    parse_account(status, &body)
}

/// Заголовок `Authorization: OAuth <token>`, помеченный как секретный.
fn auth_header(token: &str) -> Result<HeaderValue, ApiError> {
    let mut value =
        HeaderValue::from_str(&format!("OAuth {token}")).map_err(|_| ApiError::Unauthorized)?;
    value.set_sensitive(true);
    Ok(value)
}

#[derive(Deserialize)]
struct StatusEnvelope {
    result: StatusResult,
}

#[derive(Deserialize)]
struct StatusResult {
    account: RawAccount,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawAccount {
    #[serde(default)]
    uid: Option<u64>,
    #[serde(default)]
    login: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
}

fn parse_account(status: StatusCode, body: &str) -> Result<Account, ApiError> {
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return Err(ApiError::Unauthorized);
    }
    if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
        return Err(ApiError::Server(status.as_u16()));
    }
    if !status.is_success() {
        return Err(ApiError::UnexpectedStatus(status.as_u16()));
    }
    let envelope: StatusEnvelope =
        serde_json::from_str(body).map_err(|_| ApiError::InvalidResponse)?;
    let raw = envelope.result.account;
    // Без токена сервер тоже отвечает 200, только без uid: такой токен не рабочий.
    let uid = raw.uid.ok_or(ApiError::Unauthorized)?;
    Ok(Account {
        uid,
        login: raw.login,
        display_name: raw.display_name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_with_uid_is_accepted() {
        let body = r#"{"result":{"account":{"uid":42,"login":"user","displayName":"Имя"}}}"#;
        let account = parse_account(StatusCode::OK, body).unwrap();
        assert_eq!(account.uid, 42);
        assert_eq!(account.name(), "Имя (user)");
    }

    #[test]
    fn anonymous_response_means_bad_token() {
        let body = r#"{"result":{"account":{"region":168,"serviceAvailable":true}}}"#;
        assert!(matches!(
            parse_account(StatusCode::OK, body),
            Err(ApiError::Unauthorized)
        ));
    }

    #[test]
    fn statuses_are_mapped() {
        assert!(matches!(
            parse_account(StatusCode::UNAUTHORIZED, r#"{"status":401}"#),
            Err(ApiError::Unauthorized)
        ));
        assert!(matches!(
            parse_account(StatusCode::BAD_GATEWAY, ""),
            Err(ApiError::Server(502))
        ));
        assert!(matches!(
            parse_account(StatusCode::NOT_FOUND, ""),
            Err(ApiError::UnexpectedStatus(404))
        ));
        assert!(matches!(
            parse_account(StatusCode::OK, "<html>"),
            Err(ApiError::InvalidResponse)
        ));
    }

    #[test]
    fn token_with_bad_characters_is_rejected() {
        assert!(matches!(
            auth_header("bad\ntoken"),
            Err(ApiError::Unauthorized)
        ));
        assert!(auth_header("y0_good").unwrap().is_sensitive());
    }
}
