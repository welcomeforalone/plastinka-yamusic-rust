//! Статус аккаунта (`GET /account/status`): кто вошёл и есть ли Плюс.
//!
//! Разбор свой: модель крейта `yandex-music` падает на неполном ответе и не
//! знает блока `subscription`. Проверка закрыта по умолчанию: пропуск к
//! плееру даёт только явное `plus.hasPlus == true`, всё непонятное — это
//! «подписка не подтверждена». Подробности в `docs/api/stage-3.md`.

use std::fmt;

use reqwest::StatusCode;
use reqwest::header::AUTHORIZATION;
use serde::Deserialize;
use serde_json::Value;

use super::{API_BASE, ApiError, MUSIC_CLIENT, auth_header, check_status};
use crate::net::NetError;

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

/// Что известно о подписке.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subscription {
    /// Плюс подтверждён.
    Active(PlusDetails),
    /// Плюса нет.
    Missing { can_start_trial: bool },
    /// Сервис недоступен в регионе аккаунта. Пропуска нет даже с Плюсом.
    RegionUnavailable { region_code: Option<String> },
    /// Сервис не прислал понятных сведений о Плюсе.
    Unconfirmed,
}

/// Сведения об активной подписке, только для показа пользователю.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlusDetails {
    /// Самая поздняя известная дата окончания.
    pub until: Option<Date>,
    /// `None` — сервис не сообщил.
    pub auto_renewal: Option<bool>,
    pub family: bool,
}

/// Календарная дата из ответа сервиса, без часового пояса.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Date {
    pub year: u16,
    pub month: u8,
    pub day: u8,
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}.{:02}.{:04}", self.day, self.month, self.year)
    }
}

/// Результат успешного запроса статуса. Поля закрыты: статус с
/// подтверждённым Плюсом нельзя собрать вручную, только получить от сервиса.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountStatus {
    account: Account,
    subscription: Subscription,
}

impl AccountStatus {
    pub fn account(&self) -> &Account {
        &self.account
    }

    pub fn subscription(&self) -> &Subscription {
        &self.subscription
    }

    /// Пропуск к плееру, если Плюс подтверждён.
    pub fn playback_permit(&self) -> Option<PlaybackPermit> {
        match self.subscription {
            Subscription::Active(_) => Some(PlaybackPermit {
                uid: self.account.uid,
            }),
            _ => None,
        }
    }
}

/// Пропуск к плееру. Создаётся только в этом модуле и только из статуса с
/// подтверждённым Плюсом. Плеер принимает его в конструкторе, поэтому без
/// проверки подписки плеер не создать.
#[derive(Debug)]
pub struct PlaybackPermit {
    uid: u64,
}

impl PlaybackPermit {
    /// Аккаунт, для которого проверена подписка.
    pub fn uid(&self) -> u64 {
        self.uid
    }
}

/// Проверяет токен и подписку (`GET /account/status`).
pub async fn fetch_status(http: &reqwest::Client, token: &str) -> Result<AccountStatus, ApiError> {
    #[cfg(debug_assertions)]
    if let Some((status, body)) = test_hook::fixture(token)? {
        return parse_status(status, body);
    }
    let response = http
        .get(format!("{API_BASE}/account/status"))
        .header(AUTHORIZATION, auth_header(token)?)
        .header("X-Yandex-Music-Client", MUSIC_CLIENT)
        .send()
        .await
        .map_err(NetError::from)?;
    let status = response.status();
    let body = response.text().await.map_err(NetError::from)?;
    parse_status(status, &body)
}

#[derive(Deserialize)]
struct StatusEnvelope {
    result: StatusResult,
}

/// `plus` и `subscription` берём как сырой JSON: неожиданный тип в них не
/// должен ломать разбор аккаунта.
#[derive(Deserialize)]
struct StatusResult {
    account: RawAccount,
    #[serde(default)]
    plus: Option<Value>,
    #[serde(default)]
    subscription: Option<Value>,
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
    /// `None` — поля нет, `Some(Value::Null)` — пришёл `null`.
    #[serde(default, deserialize_with = "present")]
    service_available: Option<Value>,
    #[serde(default)]
    region_code: Option<Value>,
    #[serde(default)]
    non_owner_family_member: Option<Value>,
}

/// Отличает `null` от отсутствующего поля: serde по умолчанию сводит оба к `None`.
fn present<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

fn parse_status(status: StatusCode, body: &str) -> Result<AccountStatus, ApiError> {
    check_status(status)?;
    let envelope: StatusEnvelope =
        serde_json::from_str(body).map_err(|_| ApiError::InvalidResponse)?;
    let result = envelope.result;
    let raw = &result.account;
    // Без токена сервер тоже отвечает 200, только без uid: такой токен не рабочий.
    let uid = raw.uid.ok_or(ApiError::Unauthorized)?;
    let subscription = classify(raw, result.plus.as_ref(), result.subscription.as_ref());
    Ok(AccountStatus {
        account: Account {
            uid,
            login: raw.login.clone(),
            display_name: raw.display_name.clone(),
        },
        subscription,
    })
}

fn classify(
    account: &RawAccount,
    plus: Option<&Value>,
    subscription: Option<&Value>,
) -> Subscription {
    // Нет поля — сервис доступен (спека, п. 3); непонятное значение — не подтверждено.
    match account.service_available {
        None | Some(Value::Bool(true)) => {}
        Some(Value::Bool(false)) => {
            return Subscription::RegionUnavailable {
                region_code: account
                    .region_code
                    .as_ref()
                    .and_then(Value::as_str)
                    .filter(|code| {
                        (1..=3).contains(&code.len())
                            && code.bytes().all(|b| b.is_ascii_alphabetic())
                    })
                    .map(str::to_ascii_uppercase),
            };
        }
        Some(_) => return Subscription::Unconfirmed,
    }
    match plus
        .and_then(|plus| plus.get("hasPlus"))
        .and_then(Value::as_bool)
    {
        Some(true) => {
            let family_member = account
                .non_owner_family_member
                .as_ref()
                .and_then(Value::as_bool)
                == Some(true);
            Subscription::Active(plus_details(subscription, family_member))
        }
        Some(false) => Subscription::Missing {
            can_start_trial: subscription
                .and_then(|sub| sub.get("canStartTrial"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
        },
        None => Subscription::Unconfirmed,
    }
}

/// Сроки и вид подписки. Разбор мягкий: непонятное просто пропускается.
fn plus_details(subscription: Option<&Value>, family_member: bool) -> PlusDetails {
    let Some(sub) = subscription else {
        return PlusDetails {
            family: family_member,
            ..PlusDetails::default()
        };
    };
    let own = array(sub, "autoRenewable");
    let family = array(sub, "familyAutoRenewable");
    let non_auto = sub
        .get("nonAutoRenewable")
        .filter(|value| value.is_object());

    let until = own
        .iter()
        .chain(family)
        .filter_map(|entry| entry.get("expires"))
        .chain(non_auto.and_then(|value| value.get("end")))
        .chain(sub.get("end"))
        .filter_map(Value::as_str)
        .filter_map(parse_date)
        .max();

    let finished: Vec<Option<bool>> = own
        .iter()
        .chain(family)
        .map(|entry| entry.get("finished").and_then(Value::as_bool))
        .collect();
    let auto_renewal = if finished.contains(&Some(false)) {
        Some(true)
    } else if finished.iter().all(|value| *value == Some(true))
        && (!finished.is_empty() || non_auto.is_some())
    {
        Some(false)
    } else {
        None
    };

    PlusDetails {
        until,
        auto_renewal,
        family: !family.is_empty() || family_member,
    }
}

fn array<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

/// День из даты ISO 8601 (`2027-05-12T00:00:00+03:00` или `2027-05-12`).
fn parse_date(text: &str) -> Option<Date> {
    let day_part = text.split('T').next()?;
    let mut parts = day_part.split('-');
    let year: u16 = parts.next()?.parse().ok()?;
    let month: u8 = parts.next()?.parse().ok()?;
    let day: u8 = parts.next()?.parse().ok()?;
    if parts.next().is_some()
        || !(1000..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
    {
        return None;
    }
    Some(Date { year, month, day })
}

/// Статус с подтверждённым Плюсом для тестов других модулей.
#[cfg(test)]
pub fn sample_plus_status() -> AccountStatus {
    parse_status(StatusCode::OK, fixtures::PLUS).unwrap()
}

/// Готовые ответы сервиса: для юнит-тестов и отладочного хука.
#[cfg(any(test, debug_assertions))]
mod fixtures {
    use reqwest::StatusCode;

    pub const ALL: &[(&str, StatusCode, &str)] = &[
        ("plus", StatusCode::OK, PLUS),
        ("family", StatusCode::OK, FAMILY),
        ("no-plus", StatusCode::OK, NO_PLUS),
        ("no-plus-field", StatusCode::OK, NO_PLUS_FIELD),
        ("region", StatusCode::OK, REGION),
        ("anonymous", StatusCode::OK, ANONYMOUS),
        ("401", StatusCode::UNAUTHORIZED, UNAUTHORIZED),
        ("503", StatusCode::SERVICE_UNAVAILABLE, ""),
        ("broken", StatusCode::OK, "<html>"),
    ];

    pub const PLUS: &str = r#"{"result":{
        "account":{"uid":42,"login":"user","displayName":"Имя","serviceAvailable":true,"child":false},
        "plus":{"hasPlus":true,"isTutorialCompleted":true},
        "subscription":{"autoRenewable":[{"expires":"2027-05-12T00:00:00+03:00","vendor":"Yandex","finished":false}],
            "familyAutoRenewable":[],"hadAnySubscription":true,"canStartTrial":false}}}"#;

    pub const FAMILY: &str = r#"{"result":{
        "account":{"uid":43,"login":"member","serviceAvailable":true,"nonOwnerFamilyMember":true},
        "plus":{"hasPlus":true},
        "subscription":{"autoRenewable":[],
            "familyAutoRenewable":[{"expires":"2027-01-31T00:00:00+03:00","finished":true}]}}}"#;

    pub const NO_PLUS: &str = r#"{"result":{
        "account":{"uid":44,"login":"free","serviceAvailable":true},
        "plus":{"hasPlus":false,"isTutorialCompleted":false},
        "subscription":{"autoRenewable":[],"familyAutoRenewable":[],"canStartTrial":true}}}"#;

    pub const NO_PLUS_FIELD: &str =
        r#"{"result":{"account":{"uid":45,"login":"odd","serviceAvailable":true}}}"#;

    pub const REGION: &str = r#"{"result":{
        "account":{"uid":46,"login":"away","serviceAvailable":false,"regionCode":"aq"},
        "plus":{"hasPlus":true}}}"#;

    pub const ANONYMOUS: &str = r#"{"result":{"account":{"now":"2026-11-04T16:33:44+03:00",
        "region":168,"regionCode":"am","serviceAvailable":true}}}"#;

    pub const UNAUTHORIZED: &str = r#"{"status":401,"error":"Unauthorized"}"#;
}

/// Отладочный хук `PLASTINKA_TEST_STATUS`: вместо запроса к сервису разбираем
/// готовый ответ. Условия — у `api::test_hook`.
#[cfg(debug_assertions)]
mod test_hook {
    use reqwest::StatusCode;

    use super::fixtures;
    use crate::api::ApiError;

    const VAR: &str = "PLASTINKA_TEST_STATUS";

    pub fn fixture(token: &str) -> Result<Option<(StatusCode, &'static str)>, ApiError> {
        let Some(name) = crate::api::test_hook(VAR, token)? else {
            return Ok(None);
        };
        fixtures::ALL
            .iter()
            .find(|(known, _, _)| *known == name)
            .map(|&(_, status, body)| Some((status, body)))
            .ok_or_else(|| {
                let known: Vec<&str> = fixtures::ALL.iter().map(|(known, _, _)| *known).collect();
                crate::api::unknown_fixture(VAR, &known)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(body: &str) -> AccountStatus {
        parse_status(StatusCode::OK, body).unwrap()
    }

    #[test]
    fn plus_gives_permit_and_details() {
        let status = ok(fixtures::PLUS);
        assert_eq!(status.account().name(), "Имя (user)");
        assert_eq!(
            status.subscription(),
            &Subscription::Active(PlusDetails {
                until: Some(Date {
                    year: 2027,
                    month: 5,
                    day: 12
                }),
                auto_renewal: Some(true),
                family: false,
            })
        );
        assert_eq!(status.playback_permit().unwrap().uid(), 42);
    }

    #[test]
    fn family_member_is_marked() {
        let status = ok(fixtures::FAMILY);
        let Subscription::Active(details) = status.subscription() else {
            panic!("ожидался Плюс: {status:?}");
        };
        assert!(details.family);
        assert_eq!(details.auto_renewal, Some(false));
        assert_eq!(details.until.unwrap().to_string(), "31.01.2027");
        assert!(status.playback_permit().is_some());
    }

    #[test]
    fn no_plus_gives_no_permit() {
        let status = ok(fixtures::NO_PLUS);
        assert_eq!(
            status.subscription(),
            &Subscription::Missing {
                can_start_trial: true
            }
        );
        assert!(status.playback_permit().is_none());
    }

    #[test]
    fn missing_or_odd_plus_is_unconfirmed() {
        assert_eq!(
            ok(fixtures::NO_PLUS_FIELD).subscription(),
            &Subscription::Unconfirmed
        );
        for plus in [
            r#"null"#,
            r#"{}"#,
            r#"{"hasPlus":null}"#,
            r#"{"hasPlus":"true"}"#,
            r#"{"hasPlus":1}"#,
            r#"true"#,
        ] {
            let body = format!(r#"{{"result":{{"account":{{"uid":1}},"plus":{plus}}}}}"#);
            let status = ok(&body);
            assert_eq!(status.subscription(), &Subscription::Unconfirmed, "{plus}");
            assert!(status.playback_permit().is_none(), "{plus}");
        }
    }

    #[test]
    fn region_unavailable_blocks_even_with_plus() {
        let status = ok(fixtures::REGION);
        assert_eq!(
            status.subscription(),
            &Subscription::RegionUnavailable {
                region_code: Some("AQ".to_owned())
            }
        );
        assert!(status.playback_permit().is_none());

        for odd in [r#""no""#, "null", "0", "{}"] {
            let body = format!(
                r#"{{"result":{{"account":{{"uid":1,"serviceAvailable":{odd}}},"plus":{{"hasPlus":true}}}}}}"#
            );
            let status = ok(&body);
            assert_eq!(status.subscription(), &Subscription::Unconfirmed, "{odd}");
            assert!(status.playback_permit().is_none(), "{odd}");
        }
        // Поля нет совсем — сервис считается доступным.
        let absent = r#"{"result":{"account":{"uid":1},"plus":{"hasPlus":true}}}"#;
        assert!(ok(absent).playback_permit().is_some());

        let junk_code =
            r#"{"result":{"account":{"uid":1,"serviceAvailable":false,"regionCode":"<script>"}}}"#;
        assert_eq!(
            ok(junk_code).subscription(),
            &Subscription::RegionUnavailable { region_code: None }
        );
    }

    #[test]
    fn anonymous_response_means_bad_token() {
        assert!(matches!(
            parse_status(StatusCode::OK, fixtures::ANONYMOUS),
            Err(ApiError::Unauthorized)
        ));
    }

    #[test]
    fn statuses_are_mapped() {
        assert!(matches!(
            parse_status(StatusCode::UNAUTHORIZED, fixtures::UNAUTHORIZED),
            Err(ApiError::Unauthorized)
        ));
        assert!(matches!(
            parse_status(StatusCode::FORBIDDEN, ""),
            Err(ApiError::Unauthorized)
        ));
        assert!(matches!(
            parse_status(StatusCode::SERVICE_UNAVAILABLE, ""),
            Err(ApiError::Server(503))
        ));
        assert!(matches!(
            parse_status(StatusCode::TOO_MANY_REQUESTS, ""),
            Err(ApiError::Server(429))
        ));
        assert!(matches!(
            parse_status(StatusCode::NOT_FOUND, ""),
            Err(ApiError::UnexpectedStatus(404))
        ));
        for broken in [
            "<html>",
            "",
            "{}",
            r#"{"result":{}}"#,
            r#"{"result":{"account":{"uid":"x"}}}"#,
        ] {
            assert!(
                matches!(
                    parse_status(StatusCode::OK, broken),
                    Err(ApiError::InvalidResponse)
                ),
                "{broken}"
            );
        }
    }

    #[test]
    fn odd_subscription_block_does_not_break_plus() {
        for sub in [
            r#"null"#,
            r#"[]"#,
            r#"{"autoRenewable":"x","end":5}"#,
            r#"{"autoRenewable":[{"expires":"скоро","finished":"no"}]}"#,
        ] {
            let body = format!(
                r#"{{"result":{{"account":{{"uid":1}},"plus":{{"hasPlus":true}},"subscription":{sub}}}}}"#
            );
            assert_eq!(
                ok(&body).subscription(),
                &Subscription::Active(PlusDetails::default()),
                "{sub}"
            );
        }
    }

    #[test]
    fn latest_date_wins() {
        let body = r#"{"result":{"account":{"uid":1},"plus":{"hasPlus":true},
            "subscription":{"autoRenewable":[{"expires":"2027-01-01T00:00:00+03:00","finished":true}],
            "nonAutoRenewable":{"start":"2026-01-01","end":"2027-03-05T10:00:00Z"}}}}"#;
        let Subscription::Active(details) = ok(body).subscription().clone() else {
            panic!("ожидался Плюс");
        };
        assert_eq!(details.until.unwrap().to_string(), "05.03.2027");
        assert_eq!(details.auto_renewal, Some(false));
    }

    #[test]
    fn dates_are_validated() {
        assert_eq!(
            parse_date("2027-05-12"),
            Some(Date {
                year: 2027,
                month: 5,
                day: 12
            })
        );
        for bad in [
            "",
            "2027",
            "2027-13-01",
            "2027-01-32",
            "2027-01-01-01",
            "27-01-01",
            "дата",
        ] {
            assert_eq!(parse_date(bad), None, "{bad}");
        }
    }

    #[test]
    fn every_fixture_parses_without_panic() {
        for (_, status, body) in fixtures::ALL {
            let _ = parse_status(*status, body);
        }
    }
}
