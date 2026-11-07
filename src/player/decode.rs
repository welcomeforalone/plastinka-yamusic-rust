//! Поток декодера: symphonia читает MP3 из буфера в памяти и кладёт отсчёты
//! в очередь сеанса. Ждать сеть здесь можно: звуковой поток от этого не
//! зависит.

use std::num::NonZero;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
use crate::api::Codec;

/// Сколько секунд звука держать в очереди впереди.
const AHEAD: u32 = 2;
/// Пауза, когда очередь полна или трек кончился.
const IDLE: Duration = Duration::from_millis(20);
/// Сколько пакетов пробуем декодировать, чтобы узнать каналы.
const FIRST_PACKETS: usize = 16;
const UNREADABLE: &str = "не удалось прочитать звук: формат не поддерживается или файл повреждён";

/// Декодирует сеанс до его остановки. Источник звука добавляет в `mixer`
/// сам, когда узнаёт каналы и частоту. `extension` — расширение своего
/// файла; без него формат берётся из кодека потока Яндекса (по умолчанию MP3).
pub fn run(shared: Arc<Shared>, mixer: Mixer, extension: Option<String>) {
    let with_output = decode(&shared, &mixer, extension.as_deref());
    shared.decoder_exited(with_output);
}

/// Возвращает, был ли создан источник звука.
fn decode(shared: &Arc<Shared>, mixer: &Mixer, extension: Option<&str>) -> bool {
    let Some(mut reader) = Reader::open(shared.buffer.clone()) else {
        return false;
    };
    // mp4 открываем как «не перематывается»: читатель не обходит весь файл
    // (у фрагментированного это сотни запросов), а читает подряд.
    let streaming = reader.sniff_mp4();
    let mut switch = streaming.then(|| reader.seekable_switch());
    if let Some(switch) = &switch {
        switch.store(false, Ordering::SeqCst);
    }
    let mut hint = Hint::new();
    match extension {
        Some(extension) => hint.with_extension(extension),
        // Размер файла уже известен, а кодек — раньше: ссылку получают до
        // загрузки. Подсказка лишь ускоряет выбор: символы формата
        // symphonia проверяет сама.
        None => match shared.stream().map(|stream| stream.codec) {
            Some(Codec::Flac) => hint.with_extension("flac").mime_type("audio/flac"),
            Some(Codec::Aac) => hint.with_extension("aac").mime_type("audio/aac"),
            _ => hint.with_extension("mp3").mime_type("audio/mpeg"),
        },
    };
    let options = FormatOptions {
        enable_gapless: false,
        ..FormatOptions::default()
    };
    let probe = |reader: Reader| {
        let stream = MediaSourceStream::new(Box::new(reader), MediaSourceStreamOptions::default());
        symphonia::default::get_probe().format(&hint, stream, &options, &MetadataOptions::default())
    };
    trace!(
        shared.buffer,
        "декодер: открываем, mp4 без перемотки = {streaming}"
    );
    let mut probed = probe(reader);
    trace!(
        shared.buffer,
        "формат открыт: {}; {}",
        probed.is_ok(),
        shared.buffer.trace_state()
    );
    // Дальше источник перематывается: назад читатель прыгает сам. Вперёд, за
    // уже разобранные фрагменты, он идёт подряд (см. `seek`).
    if let Some(switch) = &switch {
        switch.store(true, Ordering::SeqCst);
    }
    if probed.is_err() && streaming && !shared.is_stopped() {
        // Не открылся без перемотки (например, moov в конце файла): ещё раз
        // с перемоткой, файл уже в памяти.
        switch = None;
        trace!(shared.buffer, "открываем заново с перемоткой");
        let Some(reader) = Reader::open(shared.buffer.clone()) else {
            return false;
        };
        probed = probe(reader);
    }
    let probed = match probed {
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
    trace!(
        shared.buffer,
        "дорожка: кодек {:?}, {:?} Гц, {:?} бит",
        params.codec,
        params.sample_rate,
        params.bits_per_sample
    );
    let mut decoder =
        match symphonia::default::get_codecs().make(&params, &DecoderOptions::default()) {
            Ok(decoder) => decoder,
            Err(_) => {
                shared.fail_decode(UNREADABLE.to_owned());
                return false;
            }
        };
    // У AAC в mp4 каналов в заголовке нет, а декодер узнаёт их только по
    // первому пакету: тогда декодируем его заранее, отсчёты кладём в очередь
    // после создания источника.
    let from_decoder = decoder.codec_params();
    let known = (
        params
            .sample_rate
            .or(from_decoder.sample_rate)
            .and_then(NonZero::new),
        params
            .channels
            .or(from_decoder.channels)
            .and_then(|channels| u16::try_from(channels.count()).ok())
            .and_then(NonZero::new),
    );
    let mut first: Vec<f32> = Vec::new();
    let spec = match known {
        (Some(rate), Some(channels)) => Some((rate, channels)),
        _ => first_spec(
            shared,
            format.as_mut(),
            decoder.as_mut(),
            track_id,
            &mut first,
        ),
    };
    let Some((rate, channels)) = spec else {
        shared.fail_decode(UNREADABLE.to_owned());
        return false;
    };
    shared.set_rate(rate.get());
    mixer.add(Output::new(shared.clone(), channels, rate));
    shared.start_decoding();
    trace!(
        shared.buffer,
        "декодер пошёл; {}",
        shared.buffer.trace_state()
    );
    if !first.is_empty() {
        shared.push(&first);
    }

    let limit = usize::try_from(rate.get().saturating_mul(AHEAD))
        .unwrap_or(usize::MAX)
        .saturating_mul(usize::from(channels.get()));
    // Буфер преобразования и сколько кадров в него помещается.
    let mut samples: Option<(usize, SampleBuffer<f32>)> = None;
    let mut ended = false;
    // Был ли хоть один декодированный пакет: поток, который кончился, ничего не
    // дав, — это не пустой трек, а нечитаемый формат (запасной путь в плеере).
    let mut produced = !first.is_empty();
    while !shared.is_stopped() {
        if let Some(target) = shared.take_seek() {
            seek(
                shared,
                format.as_mut(),
                decoder.as_mut(),
                track_id,
                rate,
                target,
                switch.as_deref(),
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
                if !produced
                    && matches!(&err, Error::IoError(io) if io.kind() == std::io::ErrorKind::UnexpectedEof)
                {
                    shared.fail_decode(UNREADABLE.to_owned());
                } else {
                    end_of_stream(shared, &err);
                }
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
                produced = true;
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

/// Частота и каналы по первому декодированному пакету; его отсчёты — в
/// `out`. Пробуем несколько пакетов: битый или чужой можно пропустить.
fn first_spec(
    shared: &Shared,
    format: &mut dyn FormatReader,
    decoder: &mut dyn Decoder,
    track_id: u32,
    out: &mut Vec<f32>,
) -> Option<(NonZero<u32>, NonZero<u16>)> {
    for _ in 0..FIRST_PACKETS {
        if shared.is_stopped() {
            return None;
        }
        let packet = format.next_packet().ok()?;
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = *decoded.spec();
                let mut buffer = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
                buffer.copy_interleaved_ref(decoded);
                out.extend_from_slice(buffer.samples());
                return Some((
                    NonZero::new(spec.rate)?,
                    NonZero::new(u16::try_from(spec.channels.count()).ok()?)?,
                ));
            }
            Err(Error::DecodeError(_)) => {}
            Err(_) => return None,
        }
    }
    None
}

fn seek(
    shared: &Shared,
    format: &mut dyn FormatReader,
    decoder: &mut dyn Decoder,
    track_id: u32,
    rate: NonZero<u32>,
    target: Duration,
    switch: Option<&AtomicBool>,
) {
    let time = Time::new(target.as_secs(), f64::from(target.subsec_nanos()) / 1e9);
    let to = SeekTo::Time {
        time,
        track_id: Some(track_id),
    };
    // Фрагментированный mp4: за разобранные фрагменты читатель идёт по
    // атомам. Без перемотки он читает подряд, и загрузка не прыгает по
    // файлу запросами Range (по запросу на фрагмент).
    if let Some(switch) = switch {
        switch.store(false, Ordering::SeqCst);
    }
    trace!(
        shared.buffer,
        "перемотка на {target:?}; {}",
        shared.buffer.trace_state()
    );
    let result = format.seek(SeekMode::Coarse, to);
    trace!(
        shared.buffer,
        "перемотка готова: {}; {}",
        result.is_ok(),
        shared.buffer.trace_state()
    );
    if let Some(switch) = switch {
        switch.store(true, Ordering::SeqCst);
    }
    match result {
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
