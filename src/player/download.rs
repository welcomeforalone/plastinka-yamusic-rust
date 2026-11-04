//! Загрузка трека в [`StreamBuffer`]: HTTP-запросы с `Range`, повторы при
//! обрыве сети, новая ссылка, если старая протухла. Только в память.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use reqwest::StatusCode;
use reqwest::header::{CONTENT_RANGE, CONTENT_TYPE, RANGE};

use super::buffer::{Failure, LinkFailure, StreamBuffer};
use crate::api::{ApiError, StreamError, StreamSource};
use crate::net::NetError;

/// Паузы между повторами подряд без прогресса. Кончились — ошибка сети.
const BACKOFF: [Duration; 5] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
    Duration::from_secs(8),
];
/// Сколько раз за трек можно получить ссылку заново.
const MAX_RELINKS: u32 = 3;

/// Получает ссылку на трек (заново, когда старая протухла).
pub type Relink = Box<
    dyn FnMut() -> Pin<Box<dyn Future<Output = Result<StreamSource, StreamError>> + Send>> + Send,
>;

pub(super) enum FetchError {
    /// Ссылка больше не действует: получить новую.
    Expired,
    /// Временный сбой: повторить. `progress` — успели ли что-то скачать.
    Retry {
        progress: bool,
        reason: String,
    },
    Fatal(Failure),
}

/// Качает трек целиком в `buffer`, пока его не отменят. Ошибки остаются в
/// буфере. `on_link` узнаёт
/// каждую новую ссылку (для значка качества).
pub async fn run(
    buffer: Arc<StreamBuffer>,
    http: reqwest::Client,
    mut relink: Relink,
    mut on_link: impl FnMut(&StreamSource) + Send,
) {
    let mut source = match relink().await {
        Ok(source) => {
            on_link(&source);
            source
        }
        Err(err) => {
            // Читатель ждёт размер файла: ошибка его разбудит.
            buffer.fail(Failure::NoLink(link_failure(err)));
            return;
        }
    };
    #[cfg(debug_assertions)]
    let mut fake = super::fake::Server::for_source(&source);
    let (mut relinks, mut misses) = (0, 0);
    loop {
        if buffer.is_cancelled() {
            return;
        }
        let Some(start) = buffer.next_start() else {
            return;
        };
        #[cfg(debug_assertions)]
        let result = match &mut fake {
            Some(server) => server.fetch(&buffer, start).await,
            None => fetch(&buffer, &http, &source, start).await,
        };
        #[cfg(not(debug_assertions))]
        let result = fetch(&buffer, &http, &source, start).await;
        // Хранилище приняло ссылку — счётчик новых ссылок с нуля: ограничение
        // только на протухания подряд.
        let retry_reason = match result {
            Ok(()) => {
                (relinks, misses) = (0, 0);
                continue;
            }
            Err(FetchError::Retry { progress, reason }) => {
                if progress {
                    (relinks, misses) = (0, 0);
                }
                reason
            }
            Err(FetchError::Expired) if relinks < MAX_RELINKS => {
                relinks += 1;
                match relink().await {
                    Ok(fresh) => {
                        #[cfg(debug_assertions)]
                        if let Some(server) = &mut fake {
                            server.relinked();
                        }
                        on_link(&fresh);
                        source = fresh;
                        continue;
                    }
                    Err(StreamError::Api(ApiError::Net(err))) => err.to_string(),
                    // Как в начале трека: токен не принят — вход заново,
                    // трек стал недоступен — следующий.
                    Err(err) => {
                        buffer.fail(Failure::NoLink(link_failure(err)));
                        return;
                    }
                }
            }
            Err(FetchError::Expired) => {
                buffer.fail(Failure::Link(
                    "ссылка на трек больше не действует".to_owned(),
                ));
                return;
            }
            Err(FetchError::Fatal(failure)) => {
                buffer.fail(failure);
                return;
            }
        };
        let Some(pause) = BACKOFF.get(misses) else {
            buffer.fail(Failure::Network(retry_reason));
            return;
        };
        misses += 1;
        tokio::time::sleep(*pause).await;
    }
}

fn link_failure(err: StreamError) -> LinkFailure {
    match err {
        StreamError::Unavailable => LinkFailure::Unavailable,
        StreamError::Api(ApiError::Unauthorized) => LinkFailure::Unauthorized,
        StreamError::Api(ApiError::Net(err)) => LinkFailure::Offline(err.to_string()),
        err => LinkFailure::Other(err.to_string()),
    }
}

/// Один запрос с `Range: bytes=start-`. Возвращается, когда ответ кончился
/// или загрузку пора начать с другого места.
async fn fetch(
    buffer: &StreamBuffer,
    http: &reqwest::Client,
    source: &StreamSource,
    start: u64,
) -> Result<(), FetchError> {
    let url = match source {
        StreamSource::Url { url, .. } => url,
        #[cfg(debug_assertions)]
        StreamSource::Fake(_) => {
            return Err(FetchError::Fatal(Failure::Storage(
                "поддельный поток без сервера".to_owned(),
            )));
        }
    };
    let mut response = http
        .get(url)
        .header(RANGE, format!("bytes={start}-"))
        .send()
        .await
        .map_err(|err| FetchError::Retry {
            progress: false,
            reason: NetError::from(err).to_string(),
        })?;
    let (offset, total) = check_response(&response)?;
    buffer.set_total(total).map_err(FetchError::Fatal)?;
    let mut pos = offset;
    // Сервер не понял Range и отдаёт файл с начала: байты до `start` уже
    // есть, их пропускаем. Иначе запрос бросался бы сразу и повторялся без
    // конца.
    let mut skip = start.saturating_sub(offset);
    let mut progress = false;
    loop {
        let restart = if skip > 0 {
            buffer.is_cancelled()
        } else {
            buffer.should_restart(pos)
        };
        if restart {
            return Ok(());
        }
        match response.chunk().await {
            Ok(Some(bytes)) => {
                let skipped = usize::try_from(skip).unwrap_or(usize::MAX).min(bytes.len());
                skip = skip.saturating_sub(skipped as u64);
                pos = pos.saturating_add(skipped as u64);
                let Some(bytes) = bytes.get(skipped..).filter(|rest| !rest.is_empty()) else {
                    continue;
                };
                let written = buffer.write(pos, bytes);
                pos = pos.saturating_add(written as u64);
                progress |= written > 0;
                if written < bytes.len() {
                    return Ok(());
                }
            }
            Ok(None) if progress || pos >= total => return Ok(()),
            Ok(None) => {
                return Err(FetchError::Retry {
                    progress,
                    reason: NetError::Interrupted.to_string(),
                });
            }
            Err(err) => {
                return Err(FetchError::Retry {
                    progress,
                    reason: NetError::from(err).to_string(),
                });
            }
        }
    }
}

/// Откуда начинается тело ответа и какой длины файл.
fn check_response(response: &reqwest::Response) -> Result<(u64, u64), FetchError> {
    let status = response.status();
    match status {
        StatusCode::UNAUTHORIZED
        | StatusCode::FORBIDDEN
        | StatusCode::NOT_FOUND
        | StatusCode::GONE => return Err(FetchError::Expired),
        status if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS => {
            return Err(FetchError::Retry {
                progress: false,
                reason: format!("хранилище временно недоступно (код {})", status.as_u16()),
            });
        }
        StatusCode::OK | StatusCode::PARTIAL_CONTENT => {}
        status => {
            return Err(FetchError::Fatal(Failure::Storage(format!(
                "неожиданный ответ хранилища (код {})",
                status.as_u16()
            ))));
        }
    }
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if !audio_type(content_type) {
        return Err(FetchError::Fatal(Failure::Storage(
            "хранилище прислало не звук".to_owned(),
        )));
    }
    let bad = || {
        FetchError::Fatal(Failure::Storage(
            "хранилище не сообщило размер файла".to_owned(),
        ))
    };
    if status == StatusCode::PARTIAL_CONTENT {
        let range = response
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(bad)?;
        parse_content_range(range).ok_or_else(bad)
    } else {
        // Сервер не понял Range и отдаёт файл с начала.
        response
            .content_length()
            .map(|total| (0, total))
            .ok_or_else(bad)
    }
}

/// Текст, JSON и XML вместо звука — страница ошибки или портал провайдера.
fn audio_type(content_type: &str) -> bool {
    let kind = content_type.to_ascii_lowercase();
    !(kind.starts_with("text/") || kind.contains("json") || kind.contains("xml"))
}

/// `bytes 100-199/1000` → `(100, 1000)`.
fn parse_content_range(value: &str) -> Option<(u64, u64)> {
    let rest = value.trim().strip_prefix("bytes ")?;
    let (range, total) = rest.split_once('/')?;
    let (first, last) = range.split_once('-')?;
    let (first, last, total) = (
        first.trim().parse::<u64>().ok()?,
        last.trim().parse::<u64>().ok()?,
        total.trim().parse::<u64>().ok()?,
    );
    (first <= last && last < total).then_some((first, total))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_range() {
        assert_eq!(parse_content_range("bytes 0-15/481115"), Some((0, 481_115)));
        assert_eq!(parse_content_range("bytes 100-199/200"), Some((100, 200)));
        assert_eq!(parse_content_range("bytes */481115"), None);
        assert_eq!(parse_content_range("bytes 5-4/10"), None);
        assert_eq!(parse_content_range("bytes 0-10/10"), None);
        assert_eq!(parse_content_range("items 0-1/2"), None);
    }

    /// Сервер, который не понимает Range: всегда `200` и файл с начала.
    /// Загрузка с середины не бросает такой ответ сразу, а пропускает
    /// скачанное начало.
    #[test]
    fn server_ignoring_range_fills_the_rest() {
        use std::io::{Read as _, Write as _};

        const TOTAL: usize = 200_000;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut request = [0; 2048];
            let _ = conn.read(&mut request).unwrap();
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: audio/mpeg\r\n\
                 Content-Length: {TOTAL}\r\nConnection: close\r\n\r\n"
            );
            conn.write_all(head.as_bytes()).unwrap();
            let body: Vec<u8> = (0..TOTAL).map(|i| (i % 251) as u8).collect();
            conn.write_all(&body).unwrap();
        });
        let buffer = StreamBuffer::new();
        buffer.set_total(TOTAL as u64).unwrap();
        buffer.write(0, &vec![0; 100_000]);
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let source = StreamSource::Url {
            url: format!("http://127.0.0.1:{port}/track"),
            kbps: 128,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = runtime.block_on(fetch(&buffer, &http, &source, 100_000));
        server.join().unwrap();
        assert!(result.is_ok());
        assert_eq!(buffer.next_start(), None);
        let mut reader = super::super::buffer::Reader::open(buffer).unwrap();
        std::io::Seek::seek(&mut reader, std::io::SeekFrom::Start(150_000)).unwrap();
        let mut byte = [0; 1];
        reader.read_exact(&mut byte).unwrap();
        assert_eq!(byte[0], (150_000 % 251) as u8);
    }

    #[test]
    fn audio_types() {
        assert!(audio_type("audio/mpeg"));
        assert!(audio_type(""));
        assert!(audio_type("application/octet-stream"));
        assert!(!audio_type("text/html; charset=utf-8"));
        assert!(!audio_type("application/json"));
    }
}
