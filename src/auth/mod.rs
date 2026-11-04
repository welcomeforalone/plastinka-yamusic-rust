//! Вход по коду устройства (OAuth Яндекс ID) и отзыв токена при выходе.
//! Токен передаётся в `storage` и нигде не выводится.

mod device;

pub use device::{DeviceCode, request_device_code, wait_for_token};

use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::StatusCode;
use serde::Deserialize;

use crate::net::NetError;
use crate::storage::config::Config;
use crate::storage::token::StoredToken;

const OAUTH_BASE: &str = "https://oauth.yandex.ru";

/// OAuth client_id приложения Яндекс Музыки для Android. Публичное значение:
/// его используют все неофициальные клиенты, потому что API музыки принимает
/// только токены этого приложения. Не путать с `api::MUSIC_CLIENT`.
pub const DEFAULT_CLIENT_ID: &str = "23cabbbdc6cd418abb4b39c32c41195d";
/// OAuth client_secret того же приложения. Тоже публичный, но в логи его не
/// выводим.
pub const DEFAULT_CLIENT_SECRET: &str = "53bc75238f0c4d08a118e51fe9203300";

/// OAuth-приложение, от имени которого выполняется вход.
#[derive(Clone)]
pub struct OAuthApp {
    client_id: String,
    client_secret: String,
}

impl OAuthApp {
    /// Значения из настроек, а если их нет — значения по умолчанию.
    pub fn from_config(config: &Config) -> Self {
        Self {
            client_id: config
                .oauth_client_id
                .clone()
                .unwrap_or_else(|| DEFAULT_CLIENT_ID.to_owned()),
            client_secret: config
                .oauth_client_secret
                .clone()
                .unwrap_or_else(|| DEFAULT_CLIENT_SECRET.to_owned()),
        }
    }
}

impl fmt::Debug for OAuthApp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuthApp")
            .field("client_id", &self.client_id)
            .finish_non_exhaustive()
    }
}

/// Ошибки входа.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error(transparent)]
    Net(#[from] NetError),
    #[error("сервер Яндекса временно недоступен (код {0}). Повторите позже")]
    Server(u16),
    #[error("код подтверждения истёк. Запустите вход заново")]
    CodeExpired,
    #[error("вход отклонён на странице Яндекса")]
    AccessDenied,
    #[error("Яндекс отклонил приложение ({0}). Вход по коду устройства сейчас недоступен")]
    ClientRejected(String),
    #[error("Яндекс вернул ошибку входа «{0}». Запустите вход заново")]
    Rejected(String),
    #[error("неожиданный ответ сервера Яндекса (код {0})")]
    UnexpectedStatus(u16),
    #[error("сервер Яндекса прислал непонятный ответ")]
    InvalidResponse,
    #[error("сервер прислал подозрительную ссылку для входа, вход прерван")]
    SuspiciousUrl,
}

/// Ошибки отзыва токена. Тексты свои: советы из [`AuthError`] про повторный
/// вход здесь не подходят.
#[derive(Debug, thiserror::Error)]
pub enum RevokeError {
    #[error(transparent)]
    Net(#[from] NetError),
    #[error("сервер Яндекса временно недоступен (код {0})")]
    Server(u16),
    #[error("Яндекс не даёт отозвать этот токен с устройства")]
    Unsupported,
    #[error("Яндекс не узнал приложение, которому выдан токен ({0})")]
    ClientRejected(String),
    #[error("Яндекс отказал в отзыве: «{0}»")]
    Rejected(String),
    #[error("неожиданный ответ сервера Яндекса (код {0})")]
    UnexpectedStatus(u16),
}

/// Ответ OAuth-сервера с токеном.
#[derive(Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_in: Option<u64>,
}

impl fmt::Debug for TokenResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenResponse")
            .field("access_token", &"<скрыт>")
            .field("has_refresh_token", &self.refresh_token.is_some())
            .field("expires_in", &self.expires_in)
            .finish()
    }
}

impl TokenResponse {
    /// Запись для хранилища: срок жизни переводится в момент истечения.
    pub fn into_stored(self) -> StoredToken {
        let expires_at = self
            .expires_in
            .and_then(|secs| SystemTime::now().checked_add(Duration::from_secs(secs)))
            .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
            .map(|since_epoch| since_epoch.as_secs());
        StoredToken {
            access_token: self.access_token,
            refresh_token: self.refresh_token,
            expires_at,
        }
    }
}

/// Тело ошибки OAuth-сервера: `{"error": "...", "error_description": "..."}`.
#[derive(Deserialize)]
struct OAuthErrorBody {
    error: String,
}

/// Отзывает токен на сервере. Уже недействительный токен сервер тоже
/// «отзывает» с ответом 200. Токен без `device_id` отозвать нельзя.
pub async fn revoke(
    http: &reqwest::Client,
    app: &OAuthApp,
    access_token: &str,
) -> Result<(), RevokeError> {
    let response = http
        .post(format!("{OAUTH_BASE}/revoke_token"))
        .form(&[
            ("access_token", access_token),
            ("client_id", app.client_id.as_str()),
            ("client_secret", app.client_secret.as_str()),
        ])
        .send()
        .await
        .map_err(NetError::from)?;
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let body = response.text().await.map_err(NetError::from)?;
    Err(revoke_error_from_response(status, &body))
}

/// Сервер перегружен или ограничивает частоту запросов: стоит повторить позже.
fn is_temporary(status: StatusCode) -> bool {
    status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS
}

/// Ошибка входа из неуспешного ответа OAuth-сервера.
fn error_from_response(status: StatusCode, body: &str) -> AuthError {
    if is_temporary(status) {
        return AuthError::Server(status.as_u16());
    }
    match serde_json::from_str::<OAuthErrorBody>(body) {
        Ok(body) => match body.error.as_str() {
            "invalid_grant" | "expired_token" => AuthError::CodeExpired,
            "access_denied" => AuthError::AccessDenied,
            "invalid_client" | "unauthorized_client" => AuthError::ClientRejected(body.error),
            _ => AuthError::Rejected(body.error),
        },
        Err(_) => AuthError::UnexpectedStatus(status.as_u16()),
    }
}

fn revoke_error_from_response(status: StatusCode, body: &str) -> RevokeError {
    if is_temporary(status) {
        return RevokeError::Server(status.as_u16());
    }
    match serde_json::from_str::<OAuthErrorBody>(body) {
        Ok(body) => match body.error.as_str() {
            "unsupported_token_type" => RevokeError::Unsupported,
            "invalid_client" | "unauthorized_client" => RevokeError::ClientRejected(body.error),
            _ => RevokeError::Rejected(body.error),
        },
        Err(_) => RevokeError::UnexpectedStatus(status.as_u16()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oauth_errors_are_mapped() {
        let bad = StatusCode::BAD_REQUEST;
        let error = |code: &str| error_from_response(bad, &format!(r#"{{"error":"{code}"}}"#));
        assert!(matches!(error("invalid_grant"), AuthError::CodeExpired));
        assert!(matches!(error("expired_token"), AuthError::CodeExpired));
        assert!(matches!(error("access_denied"), AuthError::AccessDenied));
        assert!(matches!(
            error("invalid_client"),
            AuthError::ClientRejected(_)
        ));
        assert!(matches!(
            error("Basic auth required"),
            AuthError::Rejected(_)
        ));
        assert!(matches!(
            error_from_response(StatusCode::BAD_GATEWAY, "<html>"),
            AuthError::Server(502)
        ));
        assert!(matches!(
            error_from_response(StatusCode::TOO_MANY_REQUESTS, ""),
            AuthError::Server(429)
        ));
        assert!(matches!(
            error_from_response(bad, "<html>"),
            AuthError::UnexpectedStatus(400)
        ));
    }

    #[test]
    fn revoke_errors_are_mapped() {
        let bad = StatusCode::BAD_REQUEST;
        let error =
            |code: &str| revoke_error_from_response(bad, &format!(r#"{{"error":"{code}"}}"#));
        assert!(matches!(
            error("unsupported_token_type"),
            RevokeError::Unsupported
        ));
        assert!(matches!(
            error("invalid_client"),
            RevokeError::ClientRejected(_)
        ));
        assert!(matches!(error("invalid_grant"), RevokeError::Rejected(_)));
        assert!(matches!(
            revoke_error_from_response(StatusCode::SERVICE_UNAVAILABLE, ""),
            RevokeError::Server(503)
        ));
        assert!(matches!(
            revoke_error_from_response(bad, "<html>"),
            RevokeError::UnexpectedStatus(400)
        ));
    }

    #[test]
    fn config_overrides_default_app() {
        let config = Config {
            oauth_client_id: Some("own-id".to_owned()),
            ..Config::default()
        };
        let app = OAuthApp::from_config(&config);
        assert_eq!(app.client_id, "own-id");
        assert_eq!(app.client_secret, DEFAULT_CLIENT_SECRET);
        assert!(!format!("{app:?}").contains(DEFAULT_CLIENT_SECRET));
    }

    #[test]
    fn token_response_hides_token_and_converts() {
        let response: TokenResponse =
            serde_json::from_str(r#"{"access_token":"y0_secret","expires_in":60}"#).unwrap();
        assert!(!format!("{response:?}").contains("secret"));
        let stored = response.into_stored();
        assert_eq!(stored.access_token, "y0_secret");
        assert!(stored.expires_at.is_some());
    }
}
