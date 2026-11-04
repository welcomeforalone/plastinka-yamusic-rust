//! Поток кода устройства: запрос кода и опрос сервера до подтверждения.

use std::fmt;
use std::time::Duration;

use reqwest::{StatusCode, Url};
use serde::Deserialize;
use tokio::time::{Instant, sleep};

use super::{AuthError, OAUTH_BASE, OAuthApp, OAuthErrorBody, TokenResponse, error_from_response};
use crate::net::NetError;

/// Имя устройства на странице «Доступы к данным» в Яндекс ID.
const DEVICE_NAME: &str = "Plastinka";
/// Куда можно отправлять пользователя вводить код.
const ALLOWED_HOSTS: [&str; 5] = [
    "oauth.yandex.ru",
    "oauth.yandex.com",
    "ya.ru",
    "yandex.ru",
    "yandex.com",
];

const DEFAULT_INTERVAL: Duration = Duration::from_secs(5);
const MIN_INTERVAL: Duration = Duration::from_secs(1);
const MAX_INTERVAL: Duration = Duration::from_secs(30);
/// Насколько увеличивать интервал по ответу `slow_down` (RFC 8628).
const SLOW_DOWN_STEP: Duration = Duration::from_secs(5);

const DEFAULT_LIFETIME: Duration = Duration::from_secs(300);
const MIN_LIFETIME: Duration = Duration::from_secs(60);
const MAX_LIFETIME: Duration = Duration::from_secs(1800);

/// Сколько сетевых сбоев подряд терпеть во время опроса.
const MAX_TRANSIENT_FAILURES: u32 = 3;

/// Выданный код. `device_code` остаётся внутри: с ним можно получить токен.
pub struct DeviceCode {
    device_code: String,
    /// Код, который пользователь вводит на странице Яндекса.
    pub user_code: String,
    /// Страница ввода кода.
    pub verification_url: Url,
    /// Как часто опрашивать сервер.
    pub interval: Duration,
    /// Сколько действует код.
    pub expires_in: Duration,
}

impl fmt::Debug for DeviceCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceCode")
            .field("user_code", &self.user_code)
            .field("verification_url", &self.verification_url.as_str())
            .field("interval", &self.interval)
            .field("expires_in", &self.expires_in)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct DeviceCodeBody {
    device_code: String,
    user_code: String,
    verification_url: String,
    #[serde(default)]
    interval: Option<u64>,
    #[serde(default)]
    expires_in: Option<u64>,
}

/// Запрашивает код для входа. `device_id` один и тот же между запусками.
pub async fn request_device_code(
    http: &reqwest::Client,
    app: &OAuthApp,
    device_id: &str,
) -> Result<DeviceCode, AuthError> {
    let response = http
        .post(format!("{OAUTH_BASE}/device/code"))
        .form(&[
            ("client_id", app.client_id.as_str()),
            ("device_id", device_id),
            ("device_name", DEVICE_NAME),
        ])
        .send()
        .await
        .map_err(NetError::from)?;
    let status = response.status();
    let body = response.text().await.map_err(NetError::from)?;
    parse_device_code(status, &body)
}

/// Опрашивает сервер, пока пользователь не подтвердит вход или код не истечёт.
/// Отмену (Ctrl+C) обрабатывает вызывающий код.
pub async fn wait_for_token(
    http: &reqwest::Client,
    app: &OAuthApp,
    code: &DeviceCode,
) -> Result<TokenResponse, AuthError> {
    let deadline = Instant::now() + code.expires_in;
    let mut interval = code.interval;
    let mut failures = 0;
    loop {
        sleep(interval).await;
        if Instant::now() >= deadline {
            return Err(AuthError::CodeExpired);
        }
        let poll = poll_once(http, app, code)
            .await
            .unwrap_or_else(Poll::Transient);
        match poll {
            Poll::Ready(token) => return Ok(token),
            Poll::Pending => failures = 0,
            Poll::SlowDown => {
                failures = 0;
                interval = interval.saturating_add(SLOW_DOWN_STEP).min(MAX_INTERVAL);
            }
            Poll::Transient(err) => {
                failures += 1;
                if failures >= MAX_TRANSIENT_FAILURES {
                    return Err(err);
                }
            }
            Poll::Failed(err) => return Err(err),
        }
    }
}

/// Результат одного опроса.
#[derive(Debug)]
enum Poll {
    Ready(TokenResponse),
    Pending,
    SlowDown,
    /// Сбой сети или сервера: можно опросить ещё раз.
    Transient(AuthError),
    Failed(AuthError),
}

async fn poll_once(
    http: &reqwest::Client,
    app: &OAuthApp,
    code: &DeviceCode,
) -> Result<Poll, AuthError> {
    let response = http
        .post(format!("{OAUTH_BASE}/token"))
        .form(&[
            ("grant_type", "device_code"),
            ("code", code.device_code.as_str()),
            ("client_id", app.client_id.as_str()),
            ("client_secret", app.client_secret.as_str()),
        ])
        .send()
        .await
        .map_err(NetError::from)?;
    let status = response.status();
    let body = response.text().await.map_err(NetError::from)?;
    Ok(classify_poll(status, &body))
}

fn parse_device_code(status: StatusCode, body: &str) -> Result<DeviceCode, AuthError> {
    if !status.is_success() {
        return Err(error_from_response(status, body));
    }
    let raw: DeviceCodeBody = serde_json::from_str(body).map_err(|_| AuthError::InvalidResponse)?;
    Ok(DeviceCode {
        verification_url: validate_verification_url(&raw.verification_url)?,
        interval: clamp_secs(raw.interval, DEFAULT_INTERVAL, MIN_INTERVAL, MAX_INTERVAL),
        expires_in: clamp_secs(raw.expires_in, DEFAULT_LIFETIME, MIN_LIFETIME, MAX_LIFETIME),
        device_code: raw.device_code,
        user_code: raw.user_code,
    })
}

/// Ошибки при опросе приходят как HTTP 400 с JSON-телом, поэтому статус
/// смотрим вместе с телом.
fn classify_poll(status: StatusCode, body: &str) -> Poll {
    if status.is_success() {
        return match serde_json::from_str::<TokenResponse>(body) {
            Ok(token) => Poll::Ready(token),
            Err(_) => Poll::Failed(AuthError::InvalidResponse),
        };
    }
    match serde_json::from_str::<OAuthErrorBody>(body) {
        Ok(err) if err.error == "authorization_pending" => return Poll::Pending,
        Ok(err) if err.error == "slow_down" => return Poll::SlowDown,
        _ => {}
    }
    // Сбой сервера и 429 — временные: опрашиваем дальше, пока не кончатся попытки.
    match error_from_response(status, body) {
        err @ AuthError::Server(_) => Poll::Transient(err),
        err => Poll::Failed(err),
    }
}

/// Ссылку для ввода кода показываем, только если она ведёт на Яндекс по https.
fn validate_verification_url(raw: &str) -> Result<Url, AuthError> {
    let url = Url::parse(raw).map_err(|_| AuthError::SuspiciousUrl)?;
    let trusted_host = url
        .host_str()
        .is_some_and(|host| ALLOWED_HOSTS.contains(&host));
    if url.scheme() == "https"
        && trusted_host
        && url.username().is_empty()
        && url.password().is_none()
    {
        Ok(url)
    } else {
        Err(AuthError::SuspiciousUrl)
    }
}

fn clamp_secs(value: Option<u64>, default: Duration, min: Duration, max: Duration) -> Duration {
    value.map_or(default, Duration::from_secs).clamp(min, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_code_is_parsed_and_clamped() {
        let body = r#"{"device_code":"dc","user_code":"h5nbcr6c",
            "verification_url":"https://oauth.yandex.ru/device","interval":0,"expires_in":99999}"#;
        let code = parse_device_code(StatusCode::OK, body).unwrap();
        assert_eq!(code.user_code, "h5nbcr6c");
        assert_eq!(code.interval, MIN_INTERVAL);
        assert_eq!(code.expires_in, MAX_LIFETIME);
        assert!(!format!("{code:?}").contains("\"dc\""));
    }

    #[test]
    fn device_code_defaults_when_timing_missing() {
        let body =
            r#"{"device_code":"dc","user_code":"u","verification_url":"https://ya.ru/device"}"#;
        let code = parse_device_code(StatusCode::OK, body).unwrap();
        assert_eq!(code.interval, DEFAULT_INTERVAL);
        assert_eq!(code.expires_in, DEFAULT_LIFETIME);
    }

    #[test]
    fn device_code_errors_are_mapped() {
        let body = r#"{"error":"invalid_client","error_description":"..."}"#;
        assert!(matches!(
            parse_device_code(StatusCode::BAD_REQUEST, body),
            Err(AuthError::ClientRejected(_))
        ));
        assert!(matches!(
            parse_device_code(StatusCode::OK, "not json"),
            Err(AuthError::InvalidResponse)
        ));
    }

    #[test]
    fn only_yandex_https_urls_are_accepted() {
        assert!(validate_verification_url("https://oauth.yandex.ru/device").is_ok());
        assert!(validate_verification_url("https://ya.ru/device").is_ok());
        for bad in [
            "http://oauth.yandex.ru/device",
            "https://oauth.yandex.ru.evil.example/device",
            "https://evil.example/oauth.yandex.ru",
            "https://user@oauth.yandex.ru/device",
            "javascript:alert(1)",
            "",
        ] {
            assert!(
                matches!(
                    validate_verification_url(bad),
                    Err(AuthError::SuspiciousUrl)
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn poll_responses_are_classified() {
        let bad = StatusCode::BAD_REQUEST;
        let poll = |code: &str| classify_poll(bad, &format!(r#"{{"error":"{code}"}}"#));
        assert!(matches!(poll("authorization_pending"), Poll::Pending));
        assert!(matches!(poll("slow_down"), Poll::SlowDown));
        assert!(matches!(
            poll("invalid_grant"),
            Poll::Failed(AuthError::CodeExpired)
        ));
        assert!(matches!(
            poll("access_denied"),
            Poll::Failed(AuthError::AccessDenied)
        ));
        assert!(matches!(
            classify_poll(StatusCode::SERVICE_UNAVAILABLE, ""),
            Poll::Transient(AuthError::Server(503))
        ));
        assert!(matches!(
            classify_poll(StatusCode::TOO_MANY_REQUESTS, "Too Many Requests"),
            Poll::Transient(AuthError::Server(429))
        ));
        assert!(matches!(
            classify_poll(
                StatusCode::OK,
                r#"{"access_token":"t","refresh_token":"r"}"#
            ),
            Poll::Ready(_)
        ));
        assert!(matches!(
            classify_poll(StatusCode::OK, "{}"),
            Poll::Failed(AuthError::InvalidResponse)
        ));
    }
}
