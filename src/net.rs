//! Общий HTTP-клиент и сетевые ошибки для входа и API музыки.

use std::error::Error as _;
use std::time::Duration;

/// Сетевые ошибки. Адреса запросов в текст не попадают.
#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("нет соединения с сервером. Проверьте интернет и повторите")]
    Offline,
    #[error(
        "не удалось установить защищённое соединение ({0}). Причиной может быть \
         антивирус или прокси, которые проверяют HTTPS"
    )]
    Tls(String),
    #[error("сервер не ответил вовремя. Повторите попытку")]
    Timeout,
    #[error("ответ сервера оборвался. Повторите попытку")]
    Interrupted,
    #[error("сетевой запрос не удался: {0}")]
    Other(String),
}

impl From<reqwest::Error> for NetError {
    fn from(err: reqwest::Error) -> Self {
        if err.is_timeout() {
            Self::Timeout
        } else if looks_like_tls(&err) {
            // Ошибки TLS тоже считаются ошибками подключения, поэтому проверяем их раньше.
            Self::Tls(root_cause(&err))
        } else if err.is_connect() {
            Self::Offline
        } else if err.is_body() || err.is_decode() {
            Self::Interrupted
        } else {
            Self::Other(root_cause(&err))
        }
    }
}

/// Самая глубокая причина ошибки: обычно это сообщение ОС на языке системы.
/// Верхний уровень не берём: в нём адрес запроса.
fn root_cause(err: &reqwest::Error) -> String {
    let Some(mut cause) = err.source() else {
        return "без подробностей".to_owned();
    };
    while let Some(source) = cause.source() {
        cause = source;
    }
    cause.to_string()
}

fn looks_like_tls(err: &reqwest::Error) -> bool {
    let mut source = err.source();
    while let Some(cause) = source {
        let text = cause.to_string().to_lowercase();
        if ["certificate", "tls", "handshake"]
            .iter()
            .any(|word| text.contains(word))
        {
            return true;
        }
        source = cause.source();
    }
    false
}

/// HTTP-клиент для коротких запросов: вход и метаданные. Для аудио нужен
/// отдельный клиент без общего таймаута.
///
/// Редиректы выключены: при 307/308 тело POST с секретами (код устройства,
/// client_secret, токен) ушло бы на новый адрес.
pub fn http_client() -> Result<reqwest::Client, NetError> {
    reqwest::Client::builder()
        .user_agent(concat!("Plastinka/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .https_only(true)
        .build()
        .map_err(NetError::from)
}

/// HTTP-клиент для звука. Общего таймаута нет: трек качается долго. Вместо
/// него таймаут на тишину между кусками ответа. Редиректы разрешены: запросы
/// без тела и без токена.
pub fn audio_client() -> Result<reqwest::Client, NetError> {
    reqwest::Client::builder()
        .user_agent(concat!("Plastinka/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::limited(5))
        .https_only(true)
        .build()
        .map_err(NetError::from)
}
