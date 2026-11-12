//! Тексты песен: Яндекс (`api::fetch_lyrics_text`), запасной LRCLib
//! (`lrclib`), разбор LRC (`lrc`) и кэш на диске (`cache`). Только
//! метаданные: звук здесь не участвует. Интерфейс получает [`Outcome`] или
//! понятную ошибку на русском; ошибки сети приложение не роняют.
//!
//! Порядок: Яндекс; если текста нет или он не получен и включён LRCLib —
//! LRCLib. Результат каждого источника кэшируется отдельно (`cache`), ошибки
//! не кэшируются. Спека — `docs/api/stage-9.md`.

pub mod cache;
pub mod lrc;
pub mod lrclib;

use std::time::Duration;

use crate::api::{self, ApiError, LyricsFormat, LyricsInfo};
use crate::net::NetError;
use cache::Cache;

/// Откуда текст.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Yandex,
    LrcLib,
}

impl Source {
    /// Ключ в имени файла кэша.
    pub fn key(self) -> &'static str {
        match self {
            Self::Yandex => "yandex",
            Self::LrcLib => "lrclib",
        }
    }

    /// Подпись источника в интерфейсе.
    pub fn title(self) -> &'static str {
        match self {
            Self::Yandex => "Яндекс",
            Self::LrcLib => "LRCLib",
        }
    }
}

/// Строка текста с меткой времени.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub time_ms: u64,
    /// Пустой текст — пауза.
    pub text: String,
}

/// Найденный текст. Хотя бы одно из полей заполнено.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lyrics {
    pub source: Source,
    /// Строки с метками времени, по возрастанию времени.
    pub synced: Option<Vec<Line>>,
    /// Обычный текст.
    pub plain: Option<String>,
}

/// Чем кончился поиск текста.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Found(Lyrics),
    /// Трек без слов (LRCLib).
    Instrumental,
    /// Текста нет ни в одном из источников.
    Missing,
}

/// Ошибка получения текста: текст для показа на русском.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LyricsError {
    /// Сервис музыки не принял токен: нужен повторный вход.
    #[error("вход больше не действует")]
    Unauthorized,
    #[error("{0}")]
    Unavailable(String),
}

impl LyricsError {
    fn bad_answer() -> Self {
        Self::Unavailable("Сервис текстов прислал непонятный ответ".to_owned())
    }
}

impl From<NetError> for LyricsError {
    fn from(err: NetError) -> Self {
        Self::Unavailable(err.to_string())
    }
}

impl From<ApiError> for LyricsError {
    fn from(err: ApiError) -> Self {
        match err {
            ApiError::Unauthorized => Self::Unauthorized,
            ApiError::Net(err) => err.into(),
            ApiError::Server(_) | ApiError::UnexpectedStatus(_) => {
                Self::Unavailable("Сервис текстов временно недоступен. Повторите позже".to_owned())
            }
            _ => Self::bad_answer(),
        }
    }
}

/// Что известно о треке для поиска текста.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// Идентификатор трека на Яндексе (только цифры).
    pub track_id: String,
    pub title: String,
    /// Исполнители через запятую.
    pub artist: String,
    pub album: Option<String>,
    pub duration: Option<Duration>,
    /// Что Яндекс сообщил о тексте в данных трека.
    pub hint: Option<LyricsInfo>,
}

/// Ищет текст песни. `lrclib` — разрешён ли запасной источник.
pub async fn load(
    http: &reqwest::Client,
    token: &str,
    request: &Request,
    lrclib: bool,
) -> Result<Outcome, LyricsError> {
    #[cfg(debug_assertions)]
    if let Some(result) = hook::reply(token).await {
        return result;
    }
    let cache = Cache::system();
    resolve(
        &cache,
        request,
        lrclib,
        unix_now(),
        || yandex(http, token, request),
        || lrclib::fetch(http, request),
    )
    .await
}

/// Порядок источников и кэш. Сами запросы приходят замыканиями.
async fn resolve<Y, L>(
    cache: &Cache,
    request: &Request,
    lrclib: bool,
    now: u64,
    from_yandex: impl Fn() -> Y,
    from_lrclib: impl Fn() -> L,
) -> Result<Outcome, LyricsError>
where
    Y: Future<Output = Result<Outcome, LyricsError>>,
    L: Future<Output = Result<Outcome, LyricsError>>,
{
    let id = request.track_id.as_str();
    // Ошибка Яндекса: если LRCLib тоже ничего не найдёт, «текста нет» было
    // бы неправдой.
    let mut yandex_error = None;
    // Яндекс сам сказал, что текста нет: не спрашиваем.
    if !request.hint.is_some_and(LyricsInfo::is_empty) {
        match cache.get(Source::Yandex, id, now) {
            Some(Outcome::Found(lyrics)) => return Ok(Outcome::Found(lyrics)),
            Some(_) => {}
            None => match from_yandex().await {
                Ok(outcome @ Outcome::Found(_)) => {
                    cache.put(Source::Yandex, id, &outcome, now);
                    return Ok(outcome);
                }
                // У Яндекса нет «инструментала»: считаем отсутствием текста.
                Ok(_) => cache.put(Source::Yandex, id, &Outcome::Missing, now),
                Err(err) => yandex_error = Some(err),
            },
        }
    }
    if !lrclib {
        return yandex_error.map_or(Ok(Outcome::Missing), Err);
    }
    let outcome = match cache.get(Source::LrcLib, id, now) {
        Some(outcome) => outcome,
        None => match from_lrclib().await {
            Ok(outcome) => {
                cache.put(Source::LrcLib, id, &outcome, now);
                outcome
            }
            Err(err) => return Err(yandex_error.unwrap_or(err)),
        },
    };
    match (outcome, yandex_error) {
        (Outcome::Missing, Some(err)) => Err(err),
        (outcome, _) => Ok(outcome),
    }
}

/// Яндекс: сначала с метками времени, если их нет — обычный текст. Признаки
/// трека подсказывают, что искать.
async fn yandex(
    http: &reqwest::Client,
    token: &str,
    request: &Request,
) -> Result<Outcome, LyricsError> {
    let id = request.track_id.as_str();
    let (try_synced, try_plain) = match request.hint {
        Some(info) => (info.synced, info.text),
        None => (true, true),
    };
    if try_synced
        && let Some(text) = api::fetch_lyrics_text(http, token, id, LyricsFormat::Lrc).await?
    {
        let lines = lrc::parse(&text);
        if !lines.is_empty() {
            return Ok(Outcome::Found(Lyrics {
                source: Source::Yandex,
                synced: Some(lines),
                plain: None,
            }));
        }
        // Пришёл текст без меток: показываем как обычный.
        return Ok(plain_found(text));
    }
    if try_plain
        && let Some(text) = api::fetch_lyrics_text(http, token, id, LyricsFormat::Text).await?
    {
        return Ok(plain_found(text));
    }
    Ok(Outcome::Missing)
}

fn plain_found(text: String) -> Outcome {
    Outcome::Found(Lyrics {
        source: Source::Yandex,
        synced: None,
        plain: Some(text),
    })
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// Отладочный хук `PLASTINKA_TEST_LYRICS`: готовые ответы вместо сети и
/// кэша. Работает только с фальшивым токеном и ничего не пишет на диск.
#[cfg(debug_assertions)]
mod hook {
    use super::{Line, Lyrics, LyricsError, Outcome, Source};

    const SAMPLE: &[(u64, &str)] = &[
        (0, ""),
        (4_000, "Первая строка тестового текста"),
        (8_000, "Вторая строка, чуть длиннее первой"),
        (12_000, ""),
        (14_000, "Третья строка после паузы"),
        (18_000, "Четвёртая строка"),
        (22_000, "Пятая строка"),
        (26_000, "Шестая строка"),
        (30_000, "Седьмая строка"),
        (34_000, "Восьмая строка"),
        (38_000, "Девятая строка"),
        (42_000, "Последняя строка"),
    ];

    /// Имена: `synced`, `plain`, `none`, `instrumental`, `error`, `slow`
    /// (через 1,5 с, потом как `synced`), `unauthorized`.
    pub async fn reply(token: &str) -> Option<Result<Outcome, LyricsError>> {
        let name = std::env::var("PLASTINKA_TEST_LYRICS")
            .ok()
            .filter(|value| !value.is_empty())?;
        let test_token = std::env::var("PLASTINKA_TEST_TOKEN").ok();
        if test_token
            .as_deref()
            .is_none_or(|test| test.is_empty() || test != token)
        {
            return Some(Err(LyricsError::Unavailable(
                "PLASTINKA_TEST_LYRICS работает только с фальшивым токеном из PLASTINKA_TEST_TOKEN"
                    .to_owned(),
            )));
        }
        let lines = || {
            SAMPLE
                .iter()
                .map(|(time_ms, text)| Line {
                    time_ms: *time_ms,
                    text: (*text).to_owned(),
                })
                .collect::<Vec<_>>()
        };
        let plain = || {
            SAMPLE
                .iter()
                .map(|(_, text)| *text)
                .collect::<Vec<_>>()
                .join("\n")
        };
        let found = |synced: bool| {
            Ok(Outcome::Found(Lyrics {
                source: Source::Yandex,
                synced: synced.then(lines),
                plain: Some(plain()),
            }))
        };
        Some(match name.as_str() {
            "synced" => found(true),
            "plain" => found(false),
            "none" => Ok(Outcome::Missing),
            "instrumental" => Ok(Outcome::Instrumental),
            "error" => Err(LyricsError::Unavailable(
                "Не удалось получить текст (проверочная ошибка)".to_owned(),
            )),
            "unauthorized" => Err(LyricsError::Unauthorized),
            "slow" => {
                tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                found(true)
            }
            _ => Err(LyricsError::Unavailable(format!(
                "неизвестное значение PLASTINKA_TEST_LYRICS. Возможные: synced, plain, \
                 none, instrumental, error, unauthorized, slow (получено: {name})"
            ))),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    fn request(hint: Option<LyricsInfo>) -> Request {
        Request {
            track_id: "42".to_owned(),
            title: "Название".to_owned(),
            artist: "Исполнитель".to_owned(),
            album: None,
            duration: Some(Duration::from_secs(200)),
            hint,
        }
    }

    fn found(source: Source) -> Outcome {
        Outcome::Found(Lyrics {
            source,
            synced: None,
            plain: Some("текст".to_owned()),
        })
    }

    fn cache(name: &str) -> Cache {
        let dir =
            std::env::temp_dir().join(format!("plastinka-resolve-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Cache::in_dir(dir)
    }

    fn err(text: &str) -> LyricsError {
        LyricsError::Unavailable(text.to_owned())
    }

    /// Прогон с подсчётом обращений к источникам.
    async fn run(
        cache: &Cache,
        request: &Request,
        lrclib: bool,
        yandex: Result<Outcome, LyricsError>,
        lrc: Result<Outcome, LyricsError>,
    ) -> (Result<Outcome, LyricsError>, u32, u32) {
        let (y_calls, l_calls) = (Cell::new(0), Cell::new(0));
        let result = resolve(
            cache,
            request,
            lrclib,
            1000,
            || {
                y_calls.set(y_calls.get() + 1);
                std::future::ready(yandex.clone())
            },
            || {
                l_calls.set(l_calls.get() + 1);
                std::future::ready(lrc.clone())
            },
        )
        .await;
        (result, y_calls.get(), l_calls.get())
    }

    #[tokio::test]
    async fn yandex_found_skips_lrclib() {
        let cache = cache("y-found");
        let (result, y, l) = run(
            &cache,
            &request(None),
            true,
            Ok(found(Source::Yandex)),
            Ok(found(Source::LrcLib)),
        )
        .await;
        assert_eq!(result, Ok(found(Source::Yandex)));
        assert_eq!((y, l), (1, 0));
        // Второй раз — из кэша без запросов.
        let (result, y, l) = run(&cache, &request(None), true, Err(err("x")), Err(err("x"))).await;
        assert_eq!(result, Ok(found(Source::Yandex)));
        assert_eq!((y, l), (0, 0));
    }

    #[tokio::test]
    async fn missing_at_yandex_goes_to_lrclib() {
        let cache = cache("fallback");
        let (result, y, l) = run(
            &cache,
            &request(None),
            true,
            Ok(Outcome::Missing),
            Ok(found(Source::LrcLib)),
        )
        .await;
        assert_eq!(result, Ok(found(Source::LrcLib)));
        assert_eq!((y, l), (1, 1));
        // Отрицательный ответ Яндекса тоже в кэше: повторно его не зовут.
        let (_, y, l) = run(&cache, &request(None), true, Err(err("x")), Err(err("x"))).await;
        assert_eq!((y, l), (0, 0));
    }

    #[tokio::test]
    async fn lrclib_off_means_no_fallback() {
        let cache = cache("off");
        let (result, y, l) = run(
            &cache,
            &request(None),
            false,
            Ok(Outcome::Missing),
            Ok(found(Source::LrcLib)),
        )
        .await;
        assert_eq!(result, Ok(Outcome::Missing));
        assert_eq!((y, l), (1, 0));
    }

    #[tokio::test]
    async fn hint_without_lyrics_skips_yandex() {
        let cache = cache("hint");
        let none = Some(LyricsInfo {
            synced: false,
            text: false,
        });
        let (result, y, l) = run(
            &cache,
            &request(none),
            true,
            Err(err("x")),
            Ok(Outcome::Instrumental),
        )
        .await;
        assert_eq!(result, Ok(Outcome::Instrumental));
        assert_eq!((y, l), (0, 1));
    }

    #[tokio::test]
    async fn errors_are_not_cached_and_not_hidden() {
        let cache = cache("errors");
        // Яндекс упал, LRCLib не нашёл: это ошибка, не «текста нет».
        let (result, _, _) = run(
            &cache,
            &request(None),
            true,
            Err(err("яндекс")),
            Ok(Outcome::Missing),
        )
        .await;
        assert_eq!(result, Err(err("яндекс")));
        // Яндекс упал, LRCLib нашёл: показываем найденное (другой кэш: в
        // первом LRCLib уже ответил «нет»).
        let other = self::cache("errors-2");
        let (result, _, _) = run(
            &other,
            &request(None),
            true,
            Err(err("яндекс")),
            Ok(found(Source::LrcLib)),
        )
        .await;
        assert_eq!(result, Ok(found(Source::LrcLib)));
        // Ошибка не закэширована: Яндекс спрашивают снова.
        let (_, y, _) = run(
            &cache,
            &request(None),
            false,
            Ok(Outcome::Missing),
            Ok(Outcome::Missing),
        )
        .await;
        assert_eq!(y, 1);
    }

    #[tokio::test]
    async fn both_failed_reports_first_error() {
        let cache = cache("both");
        let (result, _, _) = run(
            &cache,
            &request(None),
            true,
            Err(LyricsError::Unauthorized),
            Err(err("lrclib")),
        )
        .await;
        assert_eq!(result, Err(LyricsError::Unauthorized));
        let (result, _, _) = run(
            &cache,
            &request(None),
            false,
            Err(err("яндекс")),
            Ok(Outcome::Missing),
        )
        .await;
        assert_eq!(result, Err(err("яндекс")));
    }

    #[test]
    fn api_errors_have_russian_text() {
        assert_eq!(
            LyricsError::from(ApiError::Unauthorized),
            LyricsError::Unauthorized
        );
        let LyricsError::Unavailable(text) = LyricsError::from(ApiError::Server(503)) else {
            return;
        };
        assert!(text.contains("недоступен"));
        let LyricsError::Unavailable(text) = LyricsError::from(ApiError::Net(NetError::Offline))
        else {
            return;
        };
        assert!(text.contains("соединения"));
    }
}
