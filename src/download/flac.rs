//! FLAC из mp4. Яндекс отдаёт без потерь `flac-mp4`: кадры FLAC лежат в
//! контейнере mp4, по одному кадру в отсчёте. Для файла на диске кадры
//! вынимаются и собираются в обычный `.flac`: `fLaC`, блок STREAMINFO и кадры
//! подряд. Звук не перекодируется, кадры остаются теми же байтами.

use std::io::Cursor;

use symphonia::core::codecs::CODEC_TYPE_FLAC;
use symphonia::core::errors::Error;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Размер блока STREAMINFO.
const STREAMINFO_LEN: usize = 34;
/// Метка файла FLAC.
pub const MAGIC: &[u8; 4] = b"fLaC";
/// Заголовок блока метаданных: «последний блок», тип 0 (STREAMINFO), длина 34.
const STREAMINFO_HEADER: [u8; 4] = [0x80, 0, 0, STREAMINFO_LEN as u8];
/// Наибольшее число отсчётов: поле «всего отсчётов» занимает 36 бит.
const MAX_TOTAL: u64 = (1 << 36) - 1;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FlacError {
    #[error("не удалось прочитать контейнер FLAC: {0}")]
    Container(String),
    #[error("в файле нет дорожки FLAC")]
    NoTrack,
    #[error("в файле нет кадров FLAC")]
    NoFrames,
    #[error("кадр FLAC повреждён: так сохранять нельзя")]
    BadFrame,
    #[error("в параметрах FLAC не хватает данных")]
    NoParams,
}

/// Поля STREAMINFO. Нули в размерах кадров и MD5 значат «неизвестно».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamInfo {
    pub min_block: u16,
    pub max_block: u16,
    /// 24 бита.
    pub min_frame: u32,
    pub max_frame: u32,
    /// 20 бит.
    pub sample_rate: u32,
    /// От 1 до 8.
    pub channels: u8,
    /// От 4 до 32.
    pub bits: u8,
    /// 36 бит; 0 — неизвестно.
    pub total: u64,
    pub md5: [u8; 16],
}

impl StreamInfo {
    /// 34 байта блока STREAMINFO. `None` — поля не помещаются в свои биты.
    pub fn to_bytes(self) -> Option<[u8; STREAMINFO_LEN]> {
        let fits = self.sample_rate < (1 << 20)
            && (1..=8).contains(&self.channels)
            && (4..=32).contains(&self.bits)
            && self.total <= MAX_TOTAL
            && self.min_frame < (1 << 24)
            && self.max_frame < (1 << 24);
        if !fits {
            return None;
        }
        let mut out = Vec::with_capacity(STREAMINFO_LEN);
        out.extend_from_slice(&self.min_block.to_be_bytes());
        out.extend_from_slice(&self.max_block.to_be_bytes());
        out.extend_from_slice(self.min_frame.to_be_bytes().get(1..)?);
        out.extend_from_slice(self.max_frame.to_be_bytes().get(1..)?);
        // 20 бит частоты, 3 бита «каналов − 1», 5 бит «бит − 1», 36 бит отсчётов.
        let packed = (u64::from(self.sample_rate) << 44)
            | (u64::from(self.channels - 1) << 41)
            | (u64::from(self.bits - 1) << 36)
            | self.total;
        out.extend_from_slice(&packed.to_be_bytes());
        out.extend_from_slice(&self.md5);
        out.try_into().ok()
    }

    /// Разбор блока: обратное к [`StreamInfo::to_bytes`].
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let bytes: &[u8; STREAMINFO_LEN] = bytes.try_into().ok()?;
        let be16 = |at: usize| Some(u16::from_be_bytes(bytes.get(at..at + 2)?.try_into().ok()?));
        let be24 = |at: usize| {
            let part = bytes.get(at..at + 3)?;
            Some(u32::from_be_bytes([
                0,
                *part.first()?,
                *part.get(1)?,
                *part.get(2)?,
            ]))
        };
        let packed = u64::from_be_bytes(bytes.get(10..18)?.try_into().ok()?);
        Some(Self {
            min_block: be16(0)?,
            max_block: be16(2)?,
            min_frame: be24(4)?,
            max_frame: be24(7)?,
            sample_rate: u32::try_from(packed >> 44).ok()?,
            channels: u8::try_from(((packed >> 41) & 0x7) + 1).ok()?,
            bits: u8::try_from(((packed >> 36) & 0x1F) + 1).ok()?,
            total: packed & MAX_TOTAL,
            md5: bytes.get(18..34)?.try_into().ok()?,
        })
    }

    /// Блок по параметрам кодека, когда готового нет. Размеры блоков и кадров
    /// неизвестны: берутся допустимые границы, а не выдуманные значения.
    pub fn from_params(sample_rate: u32, channels: u8, bits: u8, total: u64) -> Self {
        Self {
            min_block: 16,
            max_block: u16::MAX,
            min_frame: 0,
            max_frame: 0,
            sample_rate,
            channels,
            bits,
            total,
            md5: [0; 16],
        }
    }
}

/// Начало файла FLAC: метка, заголовок блока и STREAMINFO.
pub fn header(info: &[u8; STREAMINFO_LEN]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + STREAMINFO_LEN);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&STREAMINFO_HEADER);
    out.extend_from_slice(info);
    out
}

/// Кадр FLAC начинается с 14-битного кода синхронизации 0b11111111111110.
fn is_frame(data: &[u8]) -> bool {
    matches!(data, [0xFF, second, _, ..] if second & 0xFE == 0xF8)
}

/// Вынимает кадры FLAC из mp4 `data` и собирает файл `.flac`.
pub fn from_mp4(data: Vec<u8>) -> Result<Vec<u8>, FlacError> {
    let stream = MediaSourceStream::new(
        Box::new(Cursor::new(data)),
        MediaSourceStreamOptions::default(),
    );
    let mut hint = Hint::new();
    hint.with_extension("mp4");
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            stream,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|err| FlacError::Container(err.to_string()))?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|track| track.codec_params.codec == CODEC_TYPE_FLAC)
        .ok_or(FlacError::NoTrack)?;
    let (track_id, params) = (track.id, track.codec_params.clone());
    // Отсчёт времени равен частоте дискретизации: тогда длительности кадров
    // в тиках — это числа отсчётов.
    let ticks_are_samples = match (params.time_base, params.sample_rate) {
        (Some(base), Some(rate)) => base.numer == 1 && base.denom == rate,
        _ => false,
    };
    let mut frames = Vec::new();
    let (mut count, mut samples) = (0_u64, 0_u64);
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            // Конец файла symphonia сообщает ошибкой ввода-вывода.
            Err(Error::IoError(err)) if err.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(err) => return Err(FlacError::Container(err.to_string())),
        };
        if packet.track_id() != track_id {
            continue;
        }
        if !is_frame(packet.buf()) {
            return Err(FlacError::BadFrame);
        }
        frames.extend_from_slice(packet.buf());
        count += 1;
        samples = samples.saturating_add(packet.dur());
    }
    if count == 0 {
        return Err(FlacError::NoFrames);
    }
    let known_total = (ticks_are_samples && samples > 0 && samples <= MAX_TOTAL).then_some(samples);
    let info = match params
        .extra_data
        .as_deref()
        .and_then(StreamInfo::from_bytes)
    {
        // Блок из mp4 — как его записал кодировщик. Не знает отсчётов — дописываем.
        Some(mut info) => {
            if info.total == 0
                && let Some(total) = known_total
            {
                info.total = total;
            }
            info
        }
        None => {
            let channels = params
                .channels
                .and_then(|channels| u8::try_from(channels.count()).ok())
                .ok_or(FlacError::NoParams)?;
            let bits = params
                .bits_per_sample
                .and_then(|bits| u8::try_from(bits).ok())
                .ok_or(FlacError::NoParams)?;
            let rate = params.sample_rate.ok_or(FlacError::NoParams)?;
            StreamInfo::from_params(rate, channels, bits, known_total.unwrap_or(0))
        }
    };
    let info = info.to_bytes().ok_or(FlacError::NoParams)?;
    let mut out = header(&info);
    out.extend_from_slice(&frames);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_info() -> StreamInfo {
        StreamInfo {
            min_block: 4096,
            max_block: 4096,
            min_frame: 1000,
            max_frame: 12_345,
            sample_rate: 44_100,
            channels: 2,
            bits: 16,
            total: 441_000,
            md5: [7; 16],
        }
    }

    #[test]
    fn streaminfo_matches_reference_layout() {
        let bytes = sample_info().to_bytes().unwrap();
        assert_eq!(bytes.len(), 34);
        // Блоки по 4096.
        assert_eq!(&bytes[..4], &[0x10, 0x00, 0x10, 0x00]);
        // Размеры кадров: 1000 = 0x0003E8, 12345 = 0x003039.
        assert_eq!(&bytes[4..10], &[0x00, 0x03, 0xE8, 0x00, 0x30, 0x39]);
        // 44100 Гц = 0x0AC44 (20 бит), каналов 2 -> 001, бит 16 -> 01111,
        // отсчётов 441000 = 0x6BAA8 (36 бит).
        assert_eq!(
            &bytes[10..18],
            &[0x0A, 0xC4, 0x42, 0xF0, 0x00, 0x06, 0xBA, 0xA8]
        );
        assert_eq!(&bytes[18..], &[7; 16]);
    }

    #[test]
    fn streaminfo_roundtrips() {
        let info = sample_info();
        assert_eq!(
            StreamInfo::from_bytes(&info.to_bytes().unwrap()),
            Some(info)
        );
        assert_eq!(StreamInfo::from_bytes(&[0; 10]), None);
    }

    #[test]
    fn streaminfo_rejects_values_that_do_not_fit() {
        let too_many = StreamInfo {
            channels: 9,
            ..sample_info()
        };
        assert!(too_many.to_bytes().is_none());
        let too_fast = StreamInfo {
            sample_rate: 1 << 20,
            ..sample_info()
        };
        assert!(too_fast.to_bytes().is_none());
        let too_long = StreamInfo {
            total: 1 << 36,
            ..sample_info()
        };
        assert!(too_long.to_bytes().is_none());
    }

    #[test]
    fn params_fallback_uses_legal_block_range() {
        let info = StreamInfo::from_params(48_000, 2, 24, 0);
        let bytes = info.to_bytes().unwrap();
        assert_eq!(StreamInfo::from_bytes(&bytes), Some(info));
        assert_eq!((info.min_block, info.max_block), (16, 65_535));
    }

    #[test]
    fn header_is_magic_block_header_and_info() {
        let info = sample_info().to_bytes().unwrap();
        let head = header(&info);
        assert_eq!(&head[..8], b"fLaC\x80\x00\x00\x22");
        assert_eq!(&head[8..], &info);
        assert_eq!(head.len(), 42);
    }

    #[test]
    fn frame_sync_is_checked() {
        assert!(is_frame(&[0xFF, 0xF8, 0xC9, 0x18]));
        assert!(is_frame(&[0xFF, 0xF9, 0xC9, 0x18]));
        assert!(!is_frame(&[0xFF, 0xFB, 0x90, 0x00]));
        assert!(!is_frame(&[0x00, 0xF8, 0xC9]));
        assert!(!is_frame(&[0xFF, 0xF8]));
    }

    // ----- mp4, собранный вручную

    fn atom(name: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = u32::try_from(payload.len() + 8)
            .unwrap()
            .to_be_bytes()
            .to_vec();
        out.extend_from_slice(name);
        out.extend_from_slice(payload);
        out
    }

    /// Полный бокс: версия, флаги и содержимое.
    fn full(name: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut body = vec![0, 0, 0, 0];
        body.extend_from_slice(payload);
        atom(name, &body)
    }

    fn u32s(values: &[u32]) -> Vec<u8> {
        values
            .iter()
            .flat_map(|value| value.to_be_bytes())
            .collect()
    }

    fn fake_frame(number: u8) -> Vec<u8> {
        let mut frame = vec![0xFF, 0xF8, 0xC9, 0x18, number];
        frame.extend(std::iter::repeat_n(number, 20 + usize::from(number)));
        frame
    }

    /// mp4 с одной дорожкой FLAC и заданными кадрами. `info` — STREAMINFO
    /// для `dfLa`; `first_byte` портит первый байт данных.
    fn mp4_with(frames: &[Vec<u8>], info: &[u8; 34], first_byte: Option<u8>) -> Vec<u8> {
        let ftyp = atom(b"ftyp", b"isom\0\0\x02\0isomiso2mp41");
        let mdat_payload: Vec<u8> = frames.iter().flatten().copied().collect();
        let mdat = atom(b"mdat", &mdat_payload);
        let sizes: Vec<u32> = frames
            .iter()
            .map(|frame| u32::try_from(frame.len()).unwrap())
            .collect();
        let count = u32::try_from(frames.len()).unwrap();

        let mut dfla_body = vec![0, 0, 0, 0];
        dfla_body.extend_from_slice(&[0x80, 0, 0, 34]);
        dfla_body.extend_from_slice(info);
        let dfla = atom(b"dfLa", &dfla_body);
        // Звуковой образец: 6 байт резерва, индекс данных, 8 байт резерва,
        // каналы, бит, 4 байта, частота 16.16.
        let mut entry = vec![0; 6];
        entry.extend_from_slice(&1_u16.to_be_bytes());
        entry.extend_from_slice(&[0; 8]);
        entry.extend_from_slice(&2_u16.to_be_bytes());
        entry.extend_from_slice(&16_u16.to_be_bytes());
        entry.extend_from_slice(&[0; 4]);
        entry.extend_from_slice(&(44_100_u32 << 16).to_be_bytes());
        entry.extend_from_slice(&dfla);
        let stsd = full(b"stsd", &[u32s(&[1]), atom(b"fLaC", &entry)].concat());
        let stts = full(b"stts", &u32s(&[1, count, 4096]));
        let stsc = full(b"stsc", &u32s(&[1, 1, count, 1]));
        let mut stsz_body = u32s(&[0, count]);
        stsz_body.extend_from_slice(&u32s(&sizes));
        let stsz = full(b"stsz", &stsz_body);
        // Смещение данных узнаём после сборки moov: пока ноль.
        let stco = full(b"stco", &u32s(&[1, 0]));
        let stbl = atom(b"stbl", &[stsd, stts, stsc, stsz, stco].concat());
        let minf = atom(
            b"minf",
            &[full(b"smhd", &[0; 4]), atom(b"dinf", &[]), stbl].concat(),
        );
        let mut hdlr = u32s(&[0]);
        hdlr.extend_from_slice(b"soun");
        hdlr.extend_from_slice(&[0; 12]);
        hdlr.push(0);
        let hdlr = full(b"hdlr", &hdlr);
        let mdhd = full(b"mdhd", &u32s(&[0, 0, 44_100, count * 4096, 0x55C4_0000]));
        let mdia = atom(b"mdia", &[mdhd, hdlr, minf].concat());
        let mut tkhd = u32s(&[0, 0, 1, 0, count * 4096]);
        tkhd.extend_from_slice(&[0; 8 + 2 + 2 + 2 + 2]);
        tkhd.extend_from_slice(&[0; 36]);
        tkhd.extend_from_slice(&[0; 8]);
        let tkhd = full(b"tkhd", &tkhd);
        let trak = atom(b"trak", &[tkhd, mdia].concat());
        let mut mvhd = u32s(&[0, 0, 1000, 1000]);
        mvhd.extend_from_slice(&0x0001_0000_u32.to_be_bytes());
        mvhd.extend_from_slice(&[1, 0]);
        mvhd.extend_from_slice(&[0; 10]);
        mvhd.extend_from_slice(&[0; 36]);
        mvhd.extend_from_slice(&[0; 24]);
        mvhd.extend_from_slice(&u32s(&[2]));
        let mvhd = full(b"mvhd", &mvhd);
        let moov = atom(b"moov", &[mvhd, trak].concat());
        let mut out = [ftyp, moov, mdat].concat();
        let real_start = out.len() - mdat_payload.len();
        let at = out.windows(4).position(|window| window == b"stco").unwrap() + 4 + 4 + 4;
        out[at..at + 4].copy_from_slice(&u32::try_from(real_start).unwrap().to_be_bytes());
        if let Some(byte) = first_byte {
            out[real_start] = byte;
        }
        out
    }

    #[test]
    fn extracts_frames_from_mp4() {
        let frames: Vec<Vec<u8>> = (0..5).map(fake_frame).collect();
        let info = StreamInfo {
            total: 0,
            ..sample_info()
        }
        .to_bytes()
        .unwrap();
        let flac = from_mp4(mp4_with(&frames, &info, None)).unwrap();
        assert_eq!(&flac[..8], b"fLaC\x80\x00\x00\x22");
        let parsed = StreamInfo::from_bytes(&flac[8..42]).unwrap();
        // Блок взят из mp4, нулевое число отсчётов заменено подсчитанным.
        assert_eq!(parsed.sample_rate, 44_100);
        assert_eq!(parsed.max_frame, 12_345);
        assert_eq!(parsed.total, 5 * 4096);
        let expected: Vec<u8> = frames.iter().flatten().copied().collect();
        assert_eq!(&flac[42..], expected.as_slice());
    }

    #[test]
    fn keeps_known_total_samples() {
        let frames: Vec<Vec<u8>> = (0..3).map(fake_frame).collect();
        let info = sample_info().to_bytes().unwrap();
        let flac = from_mp4(mp4_with(&frames, &info, None)).unwrap();
        assert_eq!(StreamInfo::from_bytes(&flac[8..42]).unwrap().total, 441_000);
    }

    #[test]
    fn rejects_frame_without_sync() {
        let frames: Vec<Vec<u8>> = (0..3).map(fake_frame).collect();
        let info = sample_info().to_bytes().unwrap();
        // Портим первый байт первого кадра.
        let broken = mp4_with(&frames, &info, Some(0x12));
        assert_eq!(from_mp4(broken), Err(FlacError::BadFrame));
    }

    #[test]
    fn rejects_non_mp4() {
        assert!(matches!(
            from_mp4("это вовсе не mp4, а просто текст".repeat(10).into_bytes()),
            Err(FlacError::Container(_))
        ));
    }
}
