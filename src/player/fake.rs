//! Поддельный поток для отладочного хука `PLASTINKA_TEST_STREAM`: тишина в
//! MP3, собранная в памяти, и сервер с заданным поведением. Только в
//! отладочной сборке.
//!
//! | Имя | Что происходит |
//! |---|---|
//! | `silence` | 60 секунд тишины, отдаются сразу |
//! | `slow` | то же, но медленнее, чем играет: видна загрузка |
//! | `flaky` | первый ответ обрывается на 40 %, повтор проходит |
//! | `drop` | каждый ответ обрывается на 40 %: после повторов — ошибка сети |
//! | `expired` | первый ответ обрывается на 30 %, дальше ссылка «протухла» до новой |
//! | `garbage` | вместо звука страница HTML: ошибка формата |

use std::sync::Arc;
use std::time::Duration;

use super::buffer::{Failure, StreamBuffer};
use super::download::FetchError;
use crate::api::StreamSource;
use crate::net::NetError;

/// Кадр MPEG-1 Layer III, 128 кбит/с, 44,1 кГц, стерео, без CRC. Боковая
/// информация из нулей — кадр декодируется в тишину.
const FRAME_HEADER: [u8; 4] = [0xFF, 0xFB, 0x90, 0x00];
/// 144 · 128000 / 44100, без байта выравнивания.
const FRAME_LEN: usize = 417;
/// 1152 отсчёта на кадр: 44100 / 1152 кадров в секунду.
const SECONDS: usize = 60;
const CHUNK: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Silence,
    Slow,
    Flaky,
    Drop,
    Expired,
    Garbage,
}

pub struct Server {
    kind: Kind,
    data: Arc<Vec<u8>>,
    opens: u32,
    relinked: bool,
}

/// MP3 из `seconds` секунд тишины.
pub fn silent_mp3(seconds: usize) -> Vec<u8> {
    let frames = seconds * 44_100 / 1152;
    let mut data = Vec::with_capacity(frames * FRAME_LEN);
    for _ in 0..frames {
        data.extend_from_slice(&FRAME_HEADER);
        data.resize(data.len() + FRAME_LEN - FRAME_HEADER.len(), 0);
    }
    data
}

impl Server {
    pub fn for_source(source: &StreamSource) -> Option<Self> {
        let StreamSource::Fake(name) = source else {
            return None;
        };
        let kind = match name.as_str() {
            "slow" => Kind::Slow,
            "flaky" => Kind::Flaky,
            "drop" => Kind::Drop,
            "expired" => Kind::Expired,
            "garbage" => Kind::Garbage,
            _ => Kind::Silence,
        };
        let data = if kind == Kind::Garbage {
            "<html><body>Доступ ограничен</body></html>\n"
                .repeat(4000)
                .into_bytes()
        } else {
            silent_mp3(SECONDS)
        };
        Some(Self {
            kind,
            data: Arc::new(data),
            opens: 0,
            relinked: false,
        })
    }

    pub fn relinked(&mut self) {
        self.relinked = true;
    }

    /// Как настоящий запрос с `Range: bytes=start-`.
    pub async fn fetch(&mut self, buffer: &StreamBuffer, start: u64) -> Result<(), FetchError> {
        self.opens += 1;
        let total = self.data.len() as u64;
        if start > 0 && self.kind == Kind::Expired && !self.relinked {
            return Err(FetchError::Expired);
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
        buffer.set_total(total).map_err(FetchError::Fatal)?;
        // Где ответ оборвётся.
        let cut = match self.kind {
            Kind::Flaky if self.opens == 1 => Some(total * 2 / 5),
            Kind::Drop => Some(total * 2 / 5),
            Kind::Expired if self.opens == 1 => Some(total * 3 / 10),
            _ => None,
        };
        let (pause, chunk) = if self.kind == Kind::Slow {
            // 12 КБ/с при 16 КБ/с звука.
            (Duration::from_millis(250), 3 * 1024)
        } else {
            (Duration::from_millis(5), CHUNK)
        };
        let mut pos = start;
        let mut progress = false;
        loop {
            if buffer.should_restart(pos) && pos != start {
                return Ok(());
            }
            if cut.is_some_and(|cut| pos >= cut) {
                return Err(FetchError::Retry {
                    progress,
                    reason: NetError::Interrupted.to_string(),
                });
            }
            let Ok(from) = usize::try_from(pos) else {
                return Err(FetchError::Fatal(Failure::TooLarge));
            };
            let mut until = from.saturating_add(chunk).min(self.data.len());
            if let Some(cut) = cut.and_then(|cut| usize::try_from(cut).ok()) {
                until = until.min(cut.max(from + 1));
            }
            let Some(bytes) = self.data.get(from..until).filter(|bytes| !bytes.is_empty()) else {
                return Ok(());
            };
            let written = buffer.write(pos, bytes);
            pos = pos.saturating_add(written as u64);
            progress |= written > 0;
            tokio::time::sleep(pause).await;
        }
    }
}
