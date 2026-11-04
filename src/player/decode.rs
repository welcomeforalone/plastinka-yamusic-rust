//! Поток декодера: symphonia читает MP3 из буфера в памяти и кладёт отсчёты
//! в очередь сеанса. Ждать сеть здесь можно: звуковой поток от этого не
//! зависит.

use std::num::NonZero;
use std::sync::Arc;
use std::time::Duration;

use rodio::mixer::Mixer;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_NULL, Decoder, DecoderOptions};
use symphonia::core::errors::Error;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;

use super::buffer::Reader;
use super::output::{Output, Shared};

/// Сколько секунд звука держать в очереди впереди.
const AHEAD: u32 = 2;
/// Пауза, когда очередь полна или трек кончился.
const IDLE: Duration = Duration::from_millis(20);
const UNREADABLE: &str = "не удалось прочитать звук: формат не поддерживается или файл повреждён";

/// Декодирует сеанс до его остановки. Источник звука добавляет в `mixer`
/// сам, когда узнаёт каналы и частоту.
pub fn run(shared: Arc<Shared>, mixer: Mixer) {
    let with_output = decode(&shared, &mixer);
    shared.decoder_exited(with_output);
}

/// Возвращает, был ли создан источник звука.
fn decode(shared: &Arc<Shared>, mixer: &Mixer) -> bool {
    let Some(reader) = Reader::open(shared.buffer.clone()) else {
        return false;
    };
    let stream = MediaSourceStream::new(Box::new(reader), MediaSourceStreamOptions::default());
    let mut hint = Hint::new();
    hint.with_extension("mp3").mime_type("audio/mpeg");
    let options = FormatOptions {
        enable_gapless: false,
        ..FormatOptions::default()
    };
    let probed = match symphonia::default::get_probe().format(
        &hint,
        stream,
        &options,
        &MetadataOptions::default(),
    ) {
        Ok(probed) => probed,
        Err(_) => {
            fail_unless_stream(shared);
            return false;
        }
    };
    let mut format = probed.format;
    let Some(track) = format
        .tracks()
        .iter()
        .find(|track| track.codec_params.codec != CODEC_TYPE_NULL)
    else {
        shared.fail_decode(UNREADABLE.to_owned());
        return false;
    };
    let track_id = track.id;
    let params = track.codec_params.clone();
    let (Some(rate), Some(channels)) = (
        params.sample_rate.and_then(NonZero::new),
        params
            .channels
            .and_then(|channels| u16::try_from(channels.count()).ok())
            .and_then(NonZero::new),
    ) else {
        shared.fail_decode(UNREADABLE.to_owned());
        return false;
    };
    let mut decoder =
        match symphonia::default::get_codecs().make(&params, &DecoderOptions::default()) {
            Ok(decoder) => decoder,
            Err(_) => {
                shared.fail_decode(UNREADABLE.to_owned());
                return false;
            }
        };
    shared.set_rate(rate.get());
    mixer.add(Output::new(shared.clone(), channels, rate));
    shared.start_decoding();

    let limit = usize::try_from(rate.get().saturating_mul(AHEAD))
        .unwrap_or(usize::MAX)
        .saturating_mul(usize::from(channels.get()));
    // Буфер преобразования и сколько кадров в него помещается.
    let mut samples: Option<(usize, SampleBuffer<f32>)> = None;
    let mut ended = false;
    while !shared.is_stopped() {
        if let Some(target) = shared.take_seek() {
            seek(
                shared,
                format.as_mut(),
                decoder.as_mut(),
                track_id,
                rate,
                target,
            );
            ended = false;
            continue;
        }
        if ended || shared.queued() > limit {
            std::thread::sleep(IDLE);
            continue;
        }
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(err) => {
                // Чтение прервали ради перемотки: она на следующем круге.
                if shared.seek_pending() {
                    continue;
                }
                end_of_stream(shared, &err);
                ended = true;
                continue;
            }
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = *decoded.spec();
                if spec.rate != rate.get() || spec.channels.count() != usize::from(channels.get()) {
                    shared.fail_decode(UNREADABLE.to_owned());
                    ended = true;
                    continue;
                }
                let frames = decoded.capacity();
                let buffer = match &mut samples {
                    Some((capacity, buffer)) if *capacity >= frames => buffer,
                    slot => {
                        &mut slot
                            .insert((frames, SampleBuffer::new(frames as u64, spec)))
                            .1
                    }
                };
                buffer.copy_interleaved_ref(decoded);
                shared.push(buffer.samples());
            }
            // Битый кадр пропускаем: следующий может быть целым.
            Err(Error::DecodeError(_)) => {}
            Err(err) => {
                if shared.seek_pending() {
                    continue;
                }
                end_of_stream(shared, &err);
                ended = true;
            }
        }
    }
    true
}

fn seek(
    shared: &Shared,
    format: &mut dyn FormatReader,
    decoder: &mut dyn Decoder,
    track_id: u32,
    rate: NonZero<u32>,
    target: Duration,
) {
    let time = Time::new(target.as_secs(), f64::from(target.subsec_nanos()) / 1e9);
    let to = SeekTo::Time {
        time,
        track_id: Some(track_id),
    };
    match format.seek(SeekMode::Coarse, to) {
        Ok(seeked) => {
            decoder.reset();
            let base = Duration::try_from_secs_f64(seeked.actual_ts as f64 / f64::from(rate.get()))
                .unwrap_or(target);
            shared.reset(base);
        }
        // Новую перемотку попросили, пока шла эта.
        Err(_) if shared.seek_pending() => {}
        // За концом трека — трек кончился.
        Err(Error::SeekError(_)) => {
            shared.reset(target);
            shared.finish();
        }
        Err(err) => {
            shared.reset(target);
            end_of_stream(shared, &err);
        }
    }
}

/// Конец данных. Конец файла — обычное окончание; сбой потока описан в
/// буфере; остальное — ошибка формата.
fn end_of_stream(shared: &Shared, err: &Error) {
    match err {
        Error::IoError(io) if io.kind() == std::io::ErrorKind::UnexpectedEof => shared.finish(),
        _ => fail_unless_stream(shared),
    }
}

/// Ошибка чтения из-за обрыва сети или отмены уже записана в буфере,
/// формату её не приписываем.
fn fail_unless_stream(shared: &Shared) {
    if shared.buffer.failure().is_some() || shared.is_stopped() {
        shared.finish();
    } else {
        shared.fail_decode(UNREADABLE.to_owned());
    }
}

// Поддельная тишина есть только в отладочной сборке.
#[cfg(test)]
#[cfg(debug_assertions)]
mod tests {
    use symphonia::core::io::MediaSource;

    use super::super::buffer::StreamBuffer;
    use super::super::fake::silent_mp3;
    use super::*;

    /// Поддельная тишина и правда читается symphonia: на ней проверяет
    /// тестировщик.
    #[test]
    fn silent_mp3_decodes() {
        let data = silent_mp3(3);
        let buffer = StreamBuffer::new();
        buffer.set_total(data.len() as u64).unwrap();
        buffer.write(0, &data);
        let reader = Reader::open(buffer).unwrap();
        assert_eq!(reader.byte_len(), Some(data.len() as u64));
        let stream = MediaSourceStream::new(Box::new(reader), MediaSourceStreamOptions::default());
        let mut probed = symphonia::default::get_probe()
            .format(
                &Hint::new(),
                stream,
                &FormatOptions::default(),
                &MetadataOptions::default(),
            )
            .unwrap();
        let track = probed.format.default_track().unwrap().clone();
        assert_eq!(track.codec_params.sample_rate, Some(44_100));
        let mut decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .unwrap();
        let mut frames = 0;
        while let Ok(packet) = probed.format.next_packet() {
            let decoded = decoder.decode(&packet).unwrap();
            frames += decoded.frames();
        }
        assert!(frames > 44_100 * 2, "{frames}");
        let seeked = probed
            .format
            .seek(
                SeekMode::Coarse,
                SeekTo::Time {
                    time: Time::new(2, 0.0),
                    track_id: None,
                },
            )
            .unwrap();
        assert!(seeked.actual_ts > 44_100, "{}", seeked.actual_ts);
    }
}
