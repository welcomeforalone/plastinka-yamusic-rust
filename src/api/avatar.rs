//! Аватар аккаунта из Яндекс ID.
//!
//! В `account/status` поля аватара нет. Его отдаёт Паспорт: `GET
//! login.yandex.ru/info?format=json` с тем же токеном в заголовке. В ответе
//! `default_avatar_id` и `is_avatar_empty`; картинка лежит на
//! `avatars.yandex.net/get-yapic/<id>/islands-200` (размеры у yapic именованные).
//! Аватар необязателен: любая ошибка даёт `None` и не мешает входу и проверке
//! подписки. Токен уходит только в заголовок запроса.

use reqwest::header::AUTHORIZATION;
use serde::Deserialize;

use super::auth_header;
use crate::net::read_limited;

const INFO_URL: &str = "https://login.yandex.ru/info?format=json";
/// Ответ Паспорта — крошечный JSON; больше — это не он.
const MAX_INFO_BYTES: usize = 64 * 1024;
/// Самый длинный разумный идентификатор аватара.
const MAX_ID_LEN: usize = 128;

/// Адрес аватара аккаунта или `None`, если его нет или запрос не удался.
pub async fn fetch_avatar_url(http: &reqwest::Client, token: &str) -> Option<String> {
    // Фальшивый токен тестового хука в сеть не ходит.
    #[cfg(debug_assertions)]
    if std::env::var("PLASTINKA_TEST_TOKEN")
        .ok()
        .is_some_and(|test| !test.is_empty() && test == token)
    {
        return None;
    }
    let response = http
        .get(INFO_URL)
        .header(AUTHORIZATION, auth_header(token).ok()?)
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let body = read_limited(response, MAX_INFO_BYTES).await.ok()?;
    parse_avatar_url(&String::from_utf8_lossy(&body))
}

#[derive(Deserialize)]
struct Info {
    #[serde(default)]
    default_avatar_id: Option<String>,
    #[serde(default)]
    is_avatar_empty: Option<bool>,
}

/// Адрес аватара из ответа Паспорта. Идентификатор проверяется до подстановки
/// в адрес: только латиница, цифры, `/`, `_` и `-`, без `..`.
fn parse_avatar_url(body: &str) -> Option<String> {
    let info: Info = serde_json::from_str(body).ok()?;
    if info.is_avatar_empty == Some(true) {
        return None;
    }
    let id = info.default_avatar_id?;
    let valid = !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && !id.contains("..")
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '_' | '-'));
    valid.then(|| format!("https://avatars.yandex.net/get-yapic/{id}/islands-200"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usual_answer_gives_url() {
        let body = r#"{"login":"u","default_avatar_id":"12345/abc-DEF_1","is_avatar_empty":false}"#;
        assert_eq!(
            parse_avatar_url(body).as_deref(),
            Some("https://avatars.yandex.net/get-yapic/12345/abc-DEF_1/islands-200")
        );
    }

    #[test]
    fn empty_avatar_gives_none() {
        let body = r#"{"default_avatar_id":"12345/abc","is_avatar_empty":true}"#;
        assert_eq!(parse_avatar_url(body), None);
    }

    #[test]
    fn missing_or_empty_id_gives_none() {
        assert_eq!(parse_avatar_url(r#"{"login":"u"}"#), None);
        assert_eq!(parse_avatar_url(r#"{"default_avatar_id":""}"#), None);
        assert_eq!(parse_avatar_url(r#"{"default_avatar_id":null}"#), None);
    }

    #[test]
    fn missing_empty_flag_is_fine() {
        assert!(parse_avatar_url(r#"{"default_avatar_id":"1/2"}"#).is_some());
    }

    #[test]
    fn junk_id_gives_none() {
        for id in [
            "../etc", "a/../b", "a b", "a?x=1", "a#b", "a\\b", "evil.com", "a%2e",
        ] {
            let body = format!(r#"{{"default_avatar_id":"{}"}}"#, id.replace('\\', "\\\\"));
            assert_eq!(parse_avatar_url(&body), None, "{id}");
        }
        let long = "a".repeat(MAX_ID_LEN + 1);
        let body = format!(r#"{{"default_avatar_id":"{long}"}}"#);
        assert_eq!(parse_avatar_url(&body), None);
    }

    #[test]
    fn broken_json_gives_none() {
        assert_eq!(parse_avatar_url("не json"), None);
        assert_eq!(parse_avatar_url(""), None);
        assert_eq!(parse_avatar_url(r#"{"default_avatar_id":5}"#), None);
    }
}
