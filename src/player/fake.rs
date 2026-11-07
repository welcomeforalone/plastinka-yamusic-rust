//! Поддельный поток для отладочного хука `PLASTINKA_TEST_STREAM`: тишина в
//! MP3, собранная в памяти, и сервер с заданным поведением. Только в
//! отладочной сборке.
//!
//! | Имя | Что происходит |
//! |---|---|
//! | `silence` | 60 секунд тишины, отдаются сразу |
//! | `silence-flac` | 60 секунд тишины в настоящем FLAC; значок качества показывает FLAC |
//! | `broken-aac` | значится AAC, но вместо звука мусор: плеер молча повторяет трек MP3 (тишина) |
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

/// Отсчётов в кадре FLAC (код размера блока 0b1100) и частота 44,1 кГц.
const FLAC_BLOCK: usize = 4096;
const FLAC_RATE: u32 = 44_100;

fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0_u8;
    for byte in data {
        crc ^= byte;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ 0x07
            } else {
                crc << 1
            };
        }
    }
    crc
}

fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0_u16;
    for byte in data {
        crc ^= u16::from(*byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x8005
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// Нативный FLAC из `seconds` секунд тишины: стерео, 16 бит, 44,1 кГц,
/// кадры из постоянных подкадров. Нужен для проверки декодера без сети.
pub fn silent_flac(seconds: usize) -> Vec<u8> {
    let frames = seconds * FLAC_RATE as usize / FLAC_BLOCK;
    let total = (frames * FLAC_BLOCK) as u64;
    let mut data = b"fLaC".to_vec();
    // STREAMINFO — последний блок метаданных, 34 байта.
    data.extend_from_slice(&[0x80, 0, 0, 34]);
    data.extend_from_slice(&(FLAC_BLOCK as u16).to_be_bytes());
    data.extend_from_slice(&(FLAC_BLOCK as u16).to_be_bytes());
    data.extend_from_slice(&[0; 6]);
    // 20 бит частоты, 3 бита «каналов − 1», 5 бит «бит − 1», 36 бит отсчётов.
    let packed = (u64::from(FLAC_RATE) << 44) | (1 << 41) | (15 << 36) | total;
    data.extend_from_slice(&packed.to_be_bytes());
    data.extend_from_slice(&[0; 16]);
    for number in 0..frames {
        let mut frame = vec![0xFF, 0xF8, 0xC9, 0x18];
        // Номер кадра: UTF-8-кодирование (до 0x7FF кадров хватает на минуту).
        if number < 0x80 {
            frame.push(number as u8);
        } else {
            frame.push(0xC0 | (number >> 6) as u8);
            frame.push(0x80 | (number & 0x3F) as u8);
        }
        frame.push(crc8(&frame));
        // Два постоянных подкадра с нулевым значением.
        frame.extend_from_slice(&[0; 6]);
        let crc = crc16(&frame);
        frame.extend_from_slice(&crc.to_be_bytes());
        data.extend_from_slice(&frame);
    }
    data
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
            "garbage" | "broken-aac" => Kind::Garbage,
            _ => Kind::Silence,
        };
        let data = if name == "silence-flac" {
            silent_flac(SECONDS)
        } else if kind == Kind::Garbage {
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
