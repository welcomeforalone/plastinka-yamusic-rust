//! Прямая ссылка на звук трека: `download-info`, XML с адресом в хранилище и
//! подпись MD5. Звук по ссылке скачивает плеер, в память. Подробности в
//! `docs/api/stage-6.md`.

use md5::{Digest, Md5};
use reqwest::StatusCode;
use reqwest::header::AUTHORIZATION;
use serde::Deserialize;
use serde_json::Value;

use super::tracks::plausible_id;
use super::{API_BASE, ApiError, MUSIC_CLIENT, auth_header};
use crate::net::NetError;

/// Соль подписи прямой ссылки.
const SIGN_SALT: &str = "XGRlBW9FXlekgbPrRHuSiA";

/// Откуда брать звук трека.
#[derive(Clone, PartialEq, Eq)]
pub enum StreamSource {
    /// Прямая ссылка в хранилище и битрейт, кбит/с. Ссылка живёт около
    /// минуты: для повторных запросов её получают заново.
    Url { url: String, kbps: u32 },
    /// Поддельный поток из отладочного хука `PLASTINKA_TEST_STREAM`.
    #[cfg(debug_assertions)]
    Fake(String),
}

impl StreamSource {
    /// Битрейт потока, кбит/с.
    pub fn kbps(&self) -> u32 {
        match self {
            Self::Url { kbps, .. } => *kbps,
            // Поддельная тишина — 128 кбит/с.
            #[cfg(debug_assertions)]
            Self::Fake(_) => 128,
        }
    }
}

/// Ссылка в `Debug` не попадает: в ней подпись.
impl std::fmt::Debug for StreamSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Url { kbps, .. } => write!(f, "Url(…, {kbps} кбит/с)"),
            #[cfg(debug_assertions)]
            Self::Fake(name) => write!(f, "Fake({name})"),
        }
    }
}

/// Ошибки получения ссылки.
#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    #[error(transparent)]
    Api(#[from] ApiError),
    /// Нет полной версии в MP3: трек убран, недоступен в регионе или сервис
    /// даёт только 30-секундный фрагмент.
    #[error("трек недоступен для прослушивания")]
    Unavailable,
}

impl From<NetError> for StreamError {
    fn from(err: NetError) -> Self {
        Self::Api(ApiError::Net(err))
    }
}

/// Прямая ссылка на MP3 лучшего качества для трека `track_id`.
pub async fn fetch_stream_source(
    http: &reqwest::Client,
    token: &str,
    track_id: &str,
) -> Result<StreamSource, StreamError> {
    #[cfg(debug_assertions)]
    if let Some(name) = test_hook::fake(token)? {
        return Ok(StreamSource::Fake(name));
    }
    if !plausible_id(track_id) {
        return Err(StreamError::Unavailable);
    }
    let response = http
        .get(format!("{API_BASE}/tracks/{track_id}/download-info"))
        .header(AUTHORIZATION, auth_header(token)?)
        .header("X-Yandex-Music-Client", MUSIC_CLIENT)
        .send()
        .await
        .map_err(NetError::from)?;
    let status = response.status();
    let body = response.text().await.map_err(NetError::from)?;
    let (info_url, kbps) = pick_variant(status, &body)?;

    // Токен сюда не передаём: адрес подписан сам.
    let response = http.get(&info_url).send().await.map_err(NetError::from)?;
    let status = response.status();
    if !status.is_success() {
        return Err(info_status(status).into());
    }
    let xml = response.text().await.map_err(NetError::from)?;
    let info = parse_info(&xml).ok_or(ApiError::InvalidResponse)?;
    Ok(StreamSource::Url {
        url: info.direct_link(),
        kbps,
    })
}

/// Коды `download-info`. 403 здесь — нет прав на трек, а не «токен не
/// действует»: выходить из аккаунта из-за одного трека незачем.
fn download_info_status(status: StatusCode) -> Result<(), StreamError> {
    match status {
        StatusCode::UNAUTHORIZED => Err(ApiError::Unauthorized.into()),
        StatusCode::BAD_REQUEST | StatusCode::FORBIDDEN | StatusCode::NOT_FOUND => {
            Err(StreamError::Unavailable)
        }
        status if status.is_success() => Ok(()),
        status => Err(info_status(status).into()),
    }
}

fn info_status(status: StatusCode) -> ApiError {
    if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
        ApiError::Server(status.as_u16())
    } else {
        ApiError::UnexpectedStatus(status.as_u16())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Variant {
    codec: String,
    #[serde(default)]
    preview: bool,
    download_info_url: String,
    #[serde(default)]
    bitrate_in_kbps: u32,
}

/// Адрес XML и битрейт варианта MP3 без обрезки с наибольшим битрейтом.
fn pick_variant(status: StatusCode, body: &str) -> Result<(String, u32), StreamError> {
    download_info_status(status)?;
    let envelope: Value = serde_json::from_str(body).map_err(|_| ApiError::InvalidResponse)?;
    // Ошибка приходит объектом в `result`: `{"name":"…","message":"not-found"}`.
    let Some(items) = envelope.get("result").and_then(Value::as_array) else {
        return if envelope.get("result").is_some_and(Value::is_object) {
            Err(StreamError::Unavailable)
        } else {
            Err(ApiError::InvalidResponse.into())
        };
    };
    let best = items
        .iter()
        // Непонятный вариант пропускаем: остальные могут подойти.
        .filter_map(|item| Variant::deserialize(item).ok())
        .filter(|variant| variant.codec == "mp3" && !variant.preview)
        .filter(|variant| allowed_url(&variant.download_info_url))
        .max_by_key(|variant| variant.bitrate_in_kbps);
    best.map(|variant| (variant.download_info_url, variant.bitrate_in_kbps))
        .ok_or(StreamError::Unavailable)
}

/// Адрес на хосте Яндекса по https. Ссылки на чужие хосты не открываем.
fn allowed_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    allowed_host(host)
}

fn allowed_host(host: &str) -> bool {
    !host.is_empty()
        && host
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b".-".contains(&byte))
        && (host.ends_with(".yandex.net") || host.ends_with(".yandex.ru"))
}

/// Поля XML, из которых строится ссылка.
#[derive(Debug, PartialEq, Eq)]
struct DownloadInfo {
    host: String,
    path: String,
    ts: String,
    s: String,
}

impl DownloadInfo {
    fn direct_link(&self) -> String {
        // `path` начинается с «/» (проверено при разборе), в подпись — без него.
        let path = self.path.strip_prefix('/').unwrap_or(&self.path);
        let mut hasher = Md5::new();
        hasher.update(SIGN_SALT.as_bytes());
        hasher.update(path.as_bytes());
        hasher.update(self.s.as_bytes());
        let sign: String = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        format!(
            "https://{}/get-mp3/{sign}/{}{}",
            self.host, self.ts, self.path
        )
    }
}

/// Разбирает `<download-info>`: пять плоских тегов без вложенности и атрибутов.
fn parse_info(xml: &str) -> Option<DownloadInfo> {
    let info = DownloadInfo {
        host: tag(xml, "host")?,
        path: tag(xml, "path")?,
        ts: tag(xml, "ts")?,
        s: tag(xml, "s")?,
    };
    // `s` вживую — base64 для адресов (43 знака, бывают «-» и «_»), `ts` —
    // шестнадцатеричное число.
    let token = |text: &str| {
        !text.is_empty()
            && text.len() <= 256
            && text
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
    };
    let path_ok = info.path.starts_with('/')
        && info.path.len() <= 4096
        && info
            .path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_/.=~%".contains(&b));
    (allowed_host(&info.host) && path_ok && token(&info.ts) && token(&info.s)).then_some(info)
}

/// Текст первого тега `<name>…</name>`.
fn tag(xml: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let (_, rest) = xml.split_once(&open)?;
    let (text, _) = rest.split_once(&close)?;
    Some(text.trim().to_owned())
}

#[cfg(debug_assertions)]
mod test_hook {
    use crate::api::ApiError;

    const VAR: &str = "PLASTINKA_TEST_STREAM";

    /// Имена поддельных потоков. Что каждый делает, описано в плеере.
    pub const NAMES: &[&str] = &[
        "silence",
        "slow",
        "flaky",
        "drop",
        "expired",
        "garbage",
        "offline",
        "unavailable",
    ];

    pub fn fake(token: &str) -> Result<Option<String>, super::StreamError> {
        let Some(name) = crate::api::test_hook(VAR, token)? else {
            return Ok(None);
        };
        match name.as_str() {
            "offline" => Err(ApiError::Net(crate::net::NetError::Offline).into()),
            "unavailable" => Err(super::StreamError::Unavailable),
            name if NAMES.contains(&name) => Ok(Some(name.to_owned())),
            _ => Err(crate::api::unknown_fixture(VAR, NAMES).into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?><download-info>\
        <host>s1.storage.yandex.net</host><path>/rmusic/U2Fs_dGVk-X1=</path>\
        <ts>00065f7992e23d4d</ts><region>-1</region><s>2ca0f09f</s></download-info>";

    #[test]
    fn parses_info_and_signs_link() {
        let info = parse_info(XML).unwrap();
        assert_eq!(info.host, "s1.storage.yandex.net");
        assert_eq!(info.path, "/rmusic/U2Fs_dGVk-X1=");
        // MD5 считали отдельно: соль + "rmusic/U2Fs_dGVk-X1=" + "2ca0f09f".
        let link = info.direct_link();
        let expected_sign = {
            let mut hasher = Md5::new();
            hasher.update(b"XGRlBW9FXlekgbPrRHuSiArmusic/U2Fs_dGVk-X1=2ca0f09f");
            hasher
                .finalize()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        };
        assert_eq!(
            link,
            format!(
                "https://s1.storage.yandex.net/get-mp3/{expected_sign}/00065f7992e23d4d/rmusic/U2Fs_dGVk-X1="
            )
        );
    }

    #[test]
    fn md5_matches_known_value() {
        let mut hasher = Md5::new();
        hasher.update(b"abc");
        let hex: String = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(hex, "900150983cd24fb0d6963f7d28e17f72");
    }

    /// Подпись `s` вживую — base64 для адресов: с «-» и «_».
    #[test]
    fn accepts_url_safe_sign() {
        let xml = XML.replace("2ca0f09f", "SVvHXQ_aa-Wbd0");
        assert_eq!(parse_info(&xml).unwrap().s, "SVvHXQ_aa-Wbd0");
    }

    #[test]
    fn rejects_strange_info() {
        for bad in [
            XML.replace("s1.storage.yandex.net", "evil.example.com"),
            XML.replace("s1.storage.yandex.net", "x.yandex.net/@evil"),
            XML.replace("/rmusic/", "rmusic/"),
            XML.replace("/rmusic/", "/r music/"),
            XML.replace("<s>2ca0f09f</s>", "<s></s>"),
            XML.replace("2ca0f09f", "2ca0/f09f"),
            XML.replace("2ca0f09f", "2ca0+f09f"),
            XML.replace("<ts>00065f7992e23d4d</ts>", ""),
            "не xml".to_owned(),
        ] {
            assert!(parse_info(&bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn picks_full_mp3_with_best_bitrate() {
        let body = r#"{"result":[
            {"codec":"mp3","preview":true,"downloadInfoUrl":"https://api.music.yandex.net/p","bitrateInKbps":320},
            {"codec":"aac","preview":false,"downloadInfoUrl":"https://api.music.yandex.net/aac","bitrateInKbps":256},
            {"codec":"mp3","preview":false,"downloadInfoUrl":"https://api.music.yandex.net/128","bitrateInKbps":128},
            {"codec":"mp3","preview":false,"downloadInfoUrl":"https://api.music.yandex.net/320","bitrateInKbps":320},
            {"codec":"mp3","preview":false,"downloadInfoUrl":"https://evil.example.com/999","bitrateInKbps":999},
            {"codec":"mp3"}
        ]}"#;
        assert_eq!(
            pick_variant(StatusCode::OK, body).unwrap(),
            ("https://api.music.yandex.net/320".to_owned(), 320)
        );
    }

    #[test]
    fn preview_only_is_unavailable() {
        let body = r#"{"result":[{"codec":"mp3","gain":false,"preview":true,
            "downloadInfoUrl":"https://api.music.yandex.net/get-mp3/x","direct":false,"bitrateInKbps":128}]}"#;
        assert!(matches!(
            pick_variant(StatusCode::OK, body),
            Err(StreamError::Unavailable)
        ));
    }

    #[test]
    fn error_codes() {
        let not_found = r#"{"result":{"name":"track-download-info-error","message":"not-found"}}"#;
        assert!(matches!(
            pick_variant(StatusCode::NOT_FOUND, not_found),
            Err(StreamError::Unavailable)
        ));
        assert!(matches!(
            pick_variant(StatusCode::OK, not_found),
            Err(StreamError::Unavailable)
        ));
        assert!(matches!(
            pick_variant(StatusCode::FORBIDDEN, ""),
            Err(StreamError::Unavailable)
        ));
        assert!(matches!(
            pick_variant(StatusCode::UNAUTHORIZED, ""),
            Err(StreamError::Api(ApiError::Unauthorized))
        ));
        assert!(matches!(
            pick_variant(StatusCode::SERVICE_UNAVAILABLE, ""),
            Err(StreamError::Api(ApiError::Server(503)))
        ));
        assert!(matches!(
            pick_variant(StatusCode::OK, "<html>"),
            Err(StreamError::Api(ApiError::InvalidResponse))
        ));
    }

    #[test]
    fn debug_hides_link() {
        let source = StreamSource::Url {
            url: "https://s1.storage.yandex.net/get-mp3/secret".to_owned(),
            kbps: 320,
        };
        assert!(!format!("{source:?}").contains("secret"));
    }
}
