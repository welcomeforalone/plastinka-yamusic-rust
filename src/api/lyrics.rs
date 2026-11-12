//! Тексты песен Яндекса: `GET /tracks/{id}/lyrics` отдаёт подписанную ссылку
//! на файл с текстом, сам файл качается отдельным запросом. Только
//! метаданные: звук здесь не участвует. Спека — `docs/api/stage-9.md`.

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use hmac::{Hmac, KeyInit, Mac};
use reqwest::StatusCode;
use reqwest::header::AUTHORIZATION;
use serde_json::Value;
use sha2::Sha256;

use super::stream::allowed_url;
use super::tracks::plausible_id;
use super::{API_BASE, ApiError, MUSIC_CLIENT, SIGN_KEY, auth_header, check_status};
use crate::net::{self, NetError};

/// Предел размера ответа и файла с текстом.
const MAX_BYTES: usize = 256 * 1024;
/// Таймаут каждого из двух запросов.
const TIMEOUT: Duration = Duration::from_secs(15);

/// Вид текста.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LyricsFormat {
    /// С метками времени (`format=LRC`).
    Lrc,
    /// Обычный (`format=TEXT`).
    Text,
}

impl LyricsFormat {
    fn query(self) -> &'static str {
        match self {
            Self::Lrc => "LRC",
            Self::Text => "TEXT",
        }
    }
}

/// Текст песни `track_id` в виде `format`. `Ok(None)` — текста нет (трек без
/// текста, нет прав, пустой файл). Токен уходит только в запрос ссылки, но
/// не в запрос самого файла.
pub async fn fetch_lyrics_text(
    http: &reqwest::Client,
    token: &str,
    track_id: &str,
    format: LyricsFormat,
) -> Result<Option<String>, ApiError> {
    // Идентификатор идёт в адрес: только цифры.
    if !plausible_id(track_id) || !track_id.bytes().all(|byte| byte.is_ascii_digit()) {
        return Ok(None);
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let url = lyrics_url(track_id, format, ts);
    let response = http
        .get(url)
        .header(AUTHORIZATION, auth_header(token)?)
        .header("X-Yandex-Music-Client", MUSIC_CLIENT)
        .timeout(TIMEOUT)
        .send()
        .await
        .map_err(NetError::from)?;
    let status = response.status();
    let body = net::read_limited(response, MAX_BYTES).await?;
    let body = String::from_utf8_lossy(&body);
    let Some(link) = parse_link(status, &body)? else {
        return Ok(None);
    };

    // Токен сюда не передаём: ссылка подписана сама, хост чужой для токена.
    let response = http
        .get(link)
        .timeout(TIMEOUT)
        .send()
        .await
        .map_err(NetError::from)?;
    let status = response.status();
    if matches!(
        status,
        StatusCode::NOT_FOUND | StatusCode::FORBIDDEN | StatusCode::GONE
    ) {
        return Ok(None);
    }
    check_status(status)?;
    let bytes = net::read_limited(response, MAX_BYTES).await?;
    Ok(clean_text(&String::from_utf8_lossy(&bytes)))
}

/// Подпись текстов: HMAC-SHA256 от `trackId + timestamp` (в таком порядке,
/// без разделителей), base64 целиком, вместе с «=». Порядок обратный
/// `get-file-info`.
fn lyrics_sign(track_id: &str, ts: u64) -> String {
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(SIGN_KEY.as_bytes()) else {
        return String::new();
    };
    mac.update(format!("{track_id}{ts}").as_bytes());
    STANDARD.encode(mac.finalize().into_bytes())
}

/// Адрес запроса. Подпись кодируется сразу и вклеивается в строку: в ней
/// бывают `+`, `/` и `=`, а при передаче через параметры reqwest кодирование
/// прошло бы дважды.
fn lyrics_url(track_id: &str, format: LyricsFormat, ts: u64) -> String {
    let sign = percent_encode(&lyrics_sign(track_id, ts));
    format!(
        "{API_BASE}/tracks/{track_id}/lyrics?format={}&timeStamp={ts}&sign={sign}",
        format.query()
    )
}

/// Кодирование по байтам: без изменений только `A–Z a–z 0–9 - _ . ~`.
fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Ответ на запрос ссылки: ссылка на файл, `None` — текста нет.
fn parse_link(status: StatusCode, body: &str) -> Result<Option<String>, ApiError> {
    match status {
        StatusCode::UNAUTHORIZED => return Err(ApiError::Unauthorized),
        // Нет текста или нет прав на него: это не «токен не действует».
        StatusCode::NOT_FOUND | StatusCode::FORBIDDEN => return Ok(None),
        status if !status.is_success() => return check_status(status).map(|()| None),
        _ => {}
    }
    let envelope: Value = serde_json::from_str(body).map_err(|_| ApiError::InvalidResponse)?;
    let Some(result) = envelope.get("result").filter(|result| !result.is_null()) else {
        // Ошибка приходит объектом `error`: текста нет.
        return if envelope.get("error").is_some() {
            Ok(None)
        } else {
            Err(ApiError::InvalidResponse)
        };
    };
    match result.get("downloadUrl").and_then(Value::as_str) {
        Some(link) if allowed_url(link) => Ok(Some(link.to_owned())),
        // Чужой хост или не https: такую ссылку не открываем.
        Some(_) => Err(ApiError::InvalidResponse),
        None => Ok(None),
    }
}

/// Текст файла: без BOM и `\r`, пустой — `None`.
fn clean_text(text: &str) -> Option<String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let text = text.replace('\r', "");
    (!text.trim().is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Эталон посчитан отдельно в Python: `hmac.new(key, b"123451700000000",
    /// sha256)`, base64 стандартный, «=» оставлен.
    const REFERENCE: &str = "Ucx7ny+5EZuyvj1G6+agFdud6MV8nQXkvdrNs95cyEM=";

    #[test]
    fn sign_is_hmac_of_id_then_timestamp_with_padding() {
        assert_eq!(lyrics_sign("12345", 1_700_000_000), REFERENCE);
    }

    #[test]
    fn sign_is_encoded_once() {
        let url = lyrics_url("12345", LyricsFormat::Lrc, 1_700_000_000);
        assert_eq!(
            url,
            "https://api.music.yandex.net/tracks/12345/lyrics?format=LRC&timeStamp=1700000000\
             &sign=Ucx7ny%2B5EZuyvj1G6%2BagFdud6MV8nQXkvdrNs95cyEM%3D"
        );
        assert!(lyrics_url("1", LyricsFormat::Text, 1).contains("format=TEXT"));
    }

    #[test]
    fn percent_encode_keeps_only_unreserved() {
        assert_eq!(
            percent_encode("a+b/c=d-e_f.g~h 1"),
            "a%2Bb%2Fc%3Dd-e_f.g~h%201"
        );
    }

    #[test]
    fn link_is_taken_from_download_url() {
        let body = r#"{"result":{"downloadUrl":"https://storage.mds.yandex.net/lyrics/1",
            "lyricId":1,"externalLyricId":"x","writers":[],"major":{"id":1,"name":"A"}}}"#;
        assert_eq!(
            parse_link(StatusCode::OK, body).unwrap().as_deref(),
            Some("https://storage.mds.yandex.net/lyrics/1")
        );
    }

    #[test]
    fn no_lyrics_is_not_an_error() {
        assert_eq!(parse_link(StatusCode::NOT_FOUND, "").unwrap(), None);
        assert_eq!(parse_link(StatusCode::FORBIDDEN, "").unwrap(), None);
        assert_eq!(
            parse_link(StatusCode::OK, r#"{"result":{"writers":[]}}"#).unwrap(),
            None
        );
        assert_eq!(
            parse_link(StatusCode::OK, r#"{"error":{"name":"not-found"}}"#).unwrap(),
            None
        );
    }

    #[test]
    fn bad_answers_are_errors() {
        assert!(matches!(
            parse_link(StatusCode::UNAUTHORIZED, ""),
            Err(ApiError::Unauthorized)
        ));
        assert!(matches!(
            parse_link(StatusCode::SERVICE_UNAVAILABLE, ""),
            Err(ApiError::Server(503))
        ));
        assert!(matches!(
            parse_link(StatusCode::BAD_REQUEST, ""),
            Err(ApiError::UnexpectedStatus(400))
        ));
        assert!(matches!(
            parse_link(StatusCode::OK, "не json"),
            Err(ApiError::InvalidResponse)
        ));
        assert!(matches!(
            parse_link(StatusCode::OK, "{}"),
            Err(ApiError::InvalidResponse)
        ));
    }

    #[test]
    fn foreign_hosts_and_http_are_rejected() {
        for link in [
            "http://storage.mds.yandex.net/x",
            "https://evil.example/x",
            "https://yandex.net.evil.example/x",
        ] {
            let body = format!(r#"{{"result":{{"downloadUrl":"{link}"}}}}"#);
            assert!(matches!(
                parse_link(StatusCode::OK, &body),
                Err(ApiError::InvalidResponse)
            ));
        }
    }

    #[test]
    fn text_is_cleaned() {
        assert_eq!(
            clean_text("\u{feff}[00:01.00] a\r\n[00:02.00] b\r\n").as_deref(),
            Some("[00:01.00] a\n[00:02.00] b\n")
        );
        assert_eq!(clean_text(" \n\r\n"), None);
    }
}
