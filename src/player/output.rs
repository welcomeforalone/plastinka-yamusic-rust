//! Общее состояние сеанса трека и источник звука для микшера rodio.
//!
//! Декодер кладёт отсчёты в очередь, источник забирает их в звуковом потоке.
//! Источник никогда не ждёт: нет данных — отдаёт тишину. Пауза, громкость и
//! остановка меняются плавно, без щелчков. После тишины звук продолжается,
//! только когда в очереди есть запас: иначе он рвался бы на кусочки.

use std::collections::VecDeque;
use std::num::NonZero;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use rodio::Source;

use super::buffer::StreamBuffer;
use super::eq::{EqControl, Equalizer};
use crate::api::StreamInfo;

/// Сколько отсчётов источник забирает из очереди за раз.
const TAKE: usize = 2048;
/// Тишина, пока нет данных или пауза: столько кадров за раз.
const SILENCE_FRAMES: usize = 256;
/// Время плавного изменения громкости.
const RAMP: f32 = 0.01;
/// Запас звука, с которым источник начинает играть или продолжает после
/// тишины, мс.
const PREBUFFER_MS: u32 = 400;

/// Громкость всех сеансов: биты `f32`, уже с учётом «без звука».
#[derive(Default)]
pub struct Volume(AtomicU32);

impl Volume {
    pub fn set(&self, gain: f32) {
        let gain = if gain.is_finite() {
            gain.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.0.store(gain.to_bits(), Ordering::Relaxed);
    }

    fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
}

/// Сколько последних отсчётов (моно) помнит отвод для визуализатора.
pub const TAP_LEN: usize = 2048;

/// Отвод звука для визуализатора: последние [`TAP_LEN`] отсчётов в памяти,
/// сведённые в моно. На диск ничего не пишется (правило 1). Источник кладёт
/// сюда каждую порцию, которую отдаёт микшеру, окно читает копию.
pub struct Tap {
    ring: Mutex<TapRing>,
}

struct TapRing {
    samples: Vec<f32>,
    /// Куда писать следующий отсчёт.
    next: usize,
    rate: u32,
    /// Когда пришла последняя порция звука (не тишины).
    fed_at: Option<std::time::Instant>,
}

impl Default for Tap {
    fn default() -> Self {
        Self {
            ring: Mutex::new(TapRing {
                samples: vec![0.0; TAP_LEN],
                next: 0,
                rate: 0,
                fed_at: None,
            }),
        }
    }
}

impl Tap {
    /// Порция отсчётов с `channels` каналами (кадры подряд).
    fn feed(&self, samples: &[f32], channels: usize, rate: u32) {
        let channels = channels.max(1);
        let mut ring = lock(&self.ring);
        ring.rate = rate;
        ring.fed_at = Some(std::time::Instant::now());
        let scale = 1.0 / channels as f32;
        for frame in samples.chunks_exact(channels) {
            let mono = frame.iter().sum::<f32>() * scale;
            let next = ring.next;
            if let Some(slot) = ring.samples.get_mut(next) {
                *slot = mono;
            }
            ring.next = (next + 1) % TAP_LEN;
        }
    }

    /// Копирует последние отсчёты в `out` по порядку (старые первыми) и
    /// возвращает частоту. `None` — звука не было последние `fresh`: пауза,
    /// конец трека или ожидание сети.
    pub fn snapshot(&self, out: &mut [f32; TAP_LEN], fresh: Duration) -> Option<u32> {
        let ring = lock(&self.ring);
        let recent = ring
            .fed_at
            .is_some_and(|at| at.elapsed() <= fresh && ring.rate > 0);
        if !recent {
            return None;
        }
        let (newer, older) = ring.samples.split_at(ring.next.min(TAP_LEN));
        for (slot, sample) in out.iter_mut().zip(older.iter().chain(newer)) {
            *slot = *sample;
        }
        Some(ring.rate)
    }

    /// Моно-отсчёты в отвод — для тестов визуализатора.
    #[cfg(test)]
    pub fn feed_for_tests(&self, samples: &[f32], rate: u32) {
        self.feed(samples, 1, rate);
    }
}

#[derive(Default)]
struct Queue {
    samples: VecDeque<f32>,
    /// Позиция первого отсчёта после последней перемотки.
    base: Duration,
    /// Сколько кадров источник забрал после `base`.
    frames: u64,
    /// Декодер дошёл до конца (или до ошибки): новых отсчётов не будет.
    finished: bool,
}

/// Состояние сеанса: общее для плеера, загрузчика, декодера и источника.
pub struct Shared {
    pub buffer: Arc<StreamBuffer>,
    volume: Arc<Volume>,
    tap: Arc<Tap>,
    /// Параметры эквалайзера: общие для всех сеансов плеера.
    eq: Arc<EqControl>,
    queue: Mutex<Queue>,
    /// Частота дискретизации, когда декодер её узнал.
    rate: AtomicU32,
    /// Кодек и битрейт потока, когда ссылка уже получена.
    stream: Mutex<Option<StreamInfo>>,
    /// Множитель нормализации громкости: биты `f32`.
    norm: AtomicU32,
    /// Перемотка, которую декодер ещё не выполнил.
    seek: Mutex<Option<Duration>>,
    /// Ошибка декодера (формат не читается).
    decode_error: Mutex<Option<String>>,
    paused: AtomicBool,
    stopped: AtomicBool,
    /// Источник отдал последний отсчёт и закончился.
    drained: AtomicBool,
    /// Декодер завершился, а источника так и не появилось.
    decoder_done: AtomicBool,
    /// Источник ждёт данных.
    starved: AtomicBool,
    /// Звук уже начался (хотя бы один настоящий отсчёт).
    started: AtomicBool,
    /// Декодер разобрал заголовок. До этого чтение не прерываем: перемотку
    /// он выполнит сам, когда начнёт декодировать.
    decoding: AtomicBool,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Shared {
    pub fn new(
        buffer: Arc<StreamBuffer>,
        volume: Arc<Volume>,
        tap: Arc<Tap>,
        eq: Arc<EqControl>,
        start: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            buffer,
            volume,
            tap,
            eq,
            queue: Mutex::new(Queue {
                base: start,
                ..Queue::default()
            }),
            rate: AtomicU32::new(0),
            stream: Mutex::new(None),
            norm: AtomicU32::new(1.0_f32.to_bits()),
            seek: Mutex::new((start > Duration::ZERO).then_some(start)),
            decode_error: Mutex::new(None),
            paused: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            drained: AtomicBool::new(false),
            decoder_done: AtomicBool::new(false),
            starved: AtomicBool::new(true),
            started: AtomicBool::new(false),
            decoding: AtomicBool::new(false),
        })
    }

    // ---------------------------------------------------------- плеер

    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.buffer.cancel();
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::SeqCst);
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    /// Прерывание ставится под замком перемотки: иначе декодер мог бы взять
    /// перемотку раньше и получить прерывание уже на новом месте.
    pub fn request_seek(&self, to: Duration) {
        let mut seek = lock(&self.seek);
        *seek = Some(to);
        if self.decoding.load(Ordering::SeqCst) {
            self.buffer.interrupt();
        }
    }

    /// Позиция: куда перематываем, если перемотка ещё идёт.
    pub fn position(&self) -> Duration {
        if let Some(target) = *lock(&self.seek) {
            return target;
        }
        let queue = lock(&self.queue);
        let rate = self.rate.load(Ordering::Relaxed);
        if rate == 0 {
            return queue.base;
        }
        let played = Duration::try_from_secs_f64(queue.frames as f64 / f64::from(rate));
        queue.base.saturating_add(played.unwrap_or_default())
    }

    /// Звука нет, хотя он должен идти: ждём сеть или перемотку.
    pub fn buffering(&self) -> bool {
        !self.paused.load(Ordering::SeqCst)
            && (lock(&self.seek).is_some()
                || !self.started.load(Ordering::SeqCst)
                || self.starved.load(Ordering::SeqCst))
    }

    /// Всё отыграно (или играть нечего).
    pub fn finished(&self) -> bool {
        self.drained.load(Ordering::SeqCst) || self.decoder_done.load(Ordering::SeqCst)
    }

    pub fn stream(&self) -> Option<StreamInfo> {
        *lock(&self.stream)
    }

    pub fn decode_error(&self) -> Option<String> {
        lock(&self.decode_error).clone()
    }

    pub fn set_stream(&self, info: StreamInfo) {
        *lock(&self.stream) = Some(info);
    }

    /// Множитель нормализации громкости трека (1 — без поправки).
    pub fn set_norm(&self, norm: f32) {
        let norm = if norm.is_finite() && norm > 0.0 {
            norm
        } else {
            1.0
        };
        self.norm.store(norm.to_bits(), Ordering::Relaxed);
    }

    fn norm(&self) -> f32 {
        f32::from_bits(self.norm.load(Ordering::Relaxed))
    }

    // ---------------------------------------------------------- декодер

    pub fn set_rate(&self, rate: u32) {
        self.rate.store(rate, Ordering::Relaxed);
    }

    /// Заголовок разобран: с этого момента перемотка прерывает ожидание
    /// данных.
    pub fn start_decoding(&self) {
        let _seek = lock(&self.seek);
        self.decoding.store(true, Ordering::SeqCst);
    }

    pub fn take_seek(&self) -> Option<Duration> {
        let mut seek = lock(&self.seek);
        let target = seek.take();
        if target.is_some() {
            self.buffer.clear_interrupt();
        }
        target
    }

    pub fn seek_pending(&self) -> bool {
        lock(&self.seek).is_some()
    }

    /// Очередь после перемотки: старые отсчёты выбрасываем.
    pub fn reset(&self, base: Duration) {
        let mut queue = lock(&self.queue);
        queue.samples.clear();
        queue.base = base;
        queue.frames = 0;
        queue.finished = false;
    }

    pub fn push(&self, samples: &[f32]) {
        lock(&self.queue).samples.extend(samples.iter().copied());
    }

    pub fn queued(&self) -> usize {
        lock(&self.queue).samples.len()
    }

    pub fn finish(&self) {
        lock(&self.queue).finished = true;
    }

    pub fn fail_decode(&self, text: String) {
        lock(&self.decode_error).get_or_insert(text);
        self.finish();
    }

    /// Декодер завершился. Если источника нет, сеанс закончен.
    pub fn decoder_exited(&self, with_output: bool) {
        self.finish();
        if !with_output {
            self.decoder_done.store(true, Ordering::SeqCst);
        }
    }
}

/// Источник для микшера: один на трек, с постоянными каналами и частотой.
pub struct Output {
    shared: Arc<Shared>,
    channels: NonZero<u16>,
    rate: NonZero<u32>,
    local: Vec<f32>,
    equalizer: Equalizer,
    pos: usize,
    /// Номер канала следующего отсчёта: громкость меняется между кадрами.
    channel: u16,
    gain: f32,
    step: f32,
    /// В `local` тишина, а не отсчёты трека.
    silent: bool,
    /// Запас набран: играем всё, что есть в очереди.
    ready: bool,
    /// Сколько отсчётов нужно в очереди, чтобы начать.
    prebuffer: usize,
}

impl Output {
    pub fn new(shared: Arc<Shared>, channels: NonZero<u16>, rate: NonZero<u32>) -> Self {
        // Отсчёты кадрами, без потерь: частота меньше 2^24.
        let step = 1.0 / (rate.get() as f32 * RAMP);
        let prebuffer = usize::try_from(rate.get() / 1000 * PREBUFFER_MS)
            .unwrap_or(usize::MAX)
            .saturating_mul(usize::from(channels.get()));
        let equalizer = Equalizer::new(usize::from(channels.get()), rate.get(), &shared.eq);
        Self {
            shared,
            channels,
            rate,
            local: Vec::with_capacity(TAKE),
            equalizer,
            pos: 0,
            channel: 0,
            gain: 0.0,
            step,
            silent: true,
            ready: false,
            prebuffer,
        }
    }

    fn target(&self) -> f32 {
        if self.shared.paused.load(Ordering::Relaxed) || self.shared.stopped.load(Ordering::Relaxed)
        {
            0.0
        } else {
            self.shared.volume.get()
        }
    }

    fn silence(&mut self) {
        self.local.clear();
        self.local
            .resize(SILENCE_FRAMES * usize::from(self.channels.get()), 0.0);
        self.silent = true;
    }

    /// Новая порция. `false` — источник закончился.
    fn refill(&mut self) -> bool {
        self.pos = 0;
        // Трек остановили: доигрываем остаток очереди, пока звук затихает.
        let stopped = self.shared.stopped.load(Ordering::Relaxed);
        if stopped && (self.gain <= 0.0 || self.silent) {
            return false;
        }
        // На паузе, когда звук уже затих, очередь не трогаем.
        if self.shared.paused.load(Ordering::Relaxed) && self.gain <= 0.0 {
            self.silence();
            return true;
        }
        let channels = usize::from(self.channels.get());
        let mut queue = lock(&self.shared.queue);
        let enough =
            self.ready || stopped || queue.finished || queue.samples.len() >= self.prebuffer;
        let take = if enough {
            queue.samples.len().min(TAKE) / channels * channels
        } else {
            0
        };
        if take == 0 {
            let finished = queue.finished && queue.samples.is_empty();
            drop(queue);
            if stopped {
                return false;
            }
            if finished {
                self.shared.drained.store(true, Ordering::SeqCst);
                return false;
            }
            // Данные кончились: ждём запас, потом звук плавно нарастает.
            self.ready = false;
            self.gain = 0.0;
            self.shared.starved.store(true, Ordering::Relaxed);
            self.silence();
            return true;
        }
        if !self.ready {
            trace!(self.shared.buffer, "звук пошёл");
        }
        self.ready = true;
        self.local.clear();
        self.local.extend(queue.samples.drain(..take));
        queue.frames = queue.frames.saturating_add((take / channels) as u64);
        drop(queue);
        // Эквалайзер — до отвода: спектр показывает то, что слышно.
        self.equalizer.process(&self.shared.eq, &mut self.local);
        // Визуализатору — то же, что уходит в микшер, без громкости.
        self.shared.tap.feed(&self.local, channels, self.rate.get());
        self.silent = false;
        self.shared.starved.store(false, Ordering::Relaxed);
        self.shared.started.store(true, Ordering::Relaxed);
        true
    }
}

impl Iterator for Output {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        // Остановленный трек затих: заканчиваем на границе кадра.
        if self.channel == 0 && self.gain <= 0.0 && self.shared.stopped.load(Ordering::Relaxed) {
            return None;
        }
        if self.pos >= self.local.len() && !self.refill() {
            return None;
        }
        let sample = self.local.get(self.pos).copied().unwrap_or(0.0);
        self.pos += 1;
        if self.channel == 0 {
            let target = self.target();
            self.gain = if self.gain < target {
                (self.gain + self.step).min(target)
            } else {
                (self.gain - self.step).max(target)
            };
        }
        self.channel = (self.channel + 1) % self.channels.get();
        if self.silent {
            Some(0.0)
        } else {
            // Поправка нормализации не выводит отсчёт за пределы шкалы.
            Some((sample * self.gain * self.gain * self.shared.norm()).clamp(-1.0, 1.0))
        }
    }
}

impl Source for Output {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> NonZero<u16> {
        self.channels
    }

    fn sample_rate(&self) -> NonZero<u32> {
        self.rate
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared() -> Arc<Shared> {
        let volume = Arc::new(Volume::default());
        volume.set(1.0);
        Shared::new(
            StreamBuffer::new(),
            volume,
            Arc::new(Tap::default()),
            Arc::new(EqControl::default()),
            Duration::ZERO,
        )
    }

    #[test]
    fn tap_keeps_latest_mono_samples_in_order() {
        let tap = Tap::default();
        let mut out = [9.0; TAP_LEN];
        assert_eq!(tap.snapshot(&mut out, Duration::from_secs(1)), None);
        // Стерео: левый 1, правый 3 → моно 2; больше длины кольца.
        let frames = TAP_LEN + 10;
        let samples: Vec<f32> = (0..frames)
            .flat_map(|i| [i as f32, i as f32 + 2.0])
            .collect();
        tap.feed(&samples, 2, 44_100);
        assert_eq!(tap.snapshot(&mut out, Duration::from_secs(1)), Some(44_100));
        assert_eq!(out[0], 10.0 + 1.0);
        assert_eq!(out[TAP_LEN - 1], (frames - 1) as f32 + 1.0);
        assert!(out.windows(2).all(|pair| pair[1] > pair[0]));
        // Звук давно не приходил — снимка нет.
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(tap.snapshot(&mut out, Duration::from_millis(1)), None);
    }

    fn output(shared: &Arc<Shared>) -> Output {
        Output::new(
            shared.clone(),
            NonZero::new(2).unwrap(),
            NonZero::new(1000).unwrap(),
        )
    }

    #[test]
    fn silence_while_starved_then_samples_then_end() {
        let shared = shared();
        shared.set_rate(1000);
        let mut out = output(&shared);
        assert_eq!(out.next(), Some(0.0));
        assert!(shared.buffering());
        shared.push(&vec![1.0; 2000]);
        // Остаток тишины, потом нарастающие отсчёты.
        let samples: Vec<f32> = out.by_ref().take(4000).collect();
        assert!(samples.iter().any(|s| *s > 0.99));
        shared.finish();
        assert_eq!(out.by_ref().filter(|s| *s > 0.0).count(), 0);
        assert!(shared.finished());
        assert_eq!(shared.position(), Duration::from_secs(1));
    }

    #[test]
    fn pause_stops_consuming_after_fade() {
        let shared = shared();
        shared.set_rate(1000);
        shared.push(&vec![1.0; 20_000]);
        let mut out = output(&shared);
        out.by_ref().take(2000).count();
        shared.set_paused(true);
        out.by_ref().take(6000).count();
        let before = shared.queued();
        out.by_ref().take(6000).count();
        assert_eq!(shared.queued(), before);
        assert!(!shared.buffering());
    }

    #[test]
    fn stop_ends_source_and_seek_reports_target() {
        let shared = shared();
        shared.request_seek(Duration::from_secs(30));
        assert_eq!(shared.position(), Duration::from_secs(30));
        assert_eq!(shared.take_seek(), Some(Duration::from_secs(30)));
        shared.reset(Duration::from_secs(29));
        assert_eq!(shared.position(), Duration::from_secs(29));
        let mut out = output(&shared);
        shared.stop();
        assert_eq!(out.next(), None);
    }

    /// Остановка не обрывает звук: он затихает за [`RAMP`] и только потом
    /// источник кончается.
    #[test]
    fn stop_fades_out() {
        let shared = shared();
        shared.set_rate(1000);
        shared.push(&vec![1.0; 4000]);
        let mut out = output(&shared);
        out.by_ref().take(100).count();
        shared.stop();
        let tail: Vec<f32> = out.by_ref().take(1000).collect();
        assert!(tail.len() > 2 && tail.len() < 100, "{}", tail.len());
        assert!(tail.first().unwrap() > tail.last().unwrap());
        assert!(tail.windows(2).all(|pair| pair[0] >= pair[1]));
    }

    /// После тишины звук не продолжается кусочками: источник ждёт запас.
    #[test]
    fn waits_for_prebuffer_after_starving() {
        let shared = shared();
        shared.set_rate(1000);
        let mut out = output(&shared);
        // 0,1 с при запасе 0,4 с: пока тишина.
        shared.push(&vec![1.0; 200]);
        assert!(out.by_ref().take(2000).all(|s| s == 0.0));
        assert_eq!(shared.queued(), 200);
        shared.push(&vec![1.0; 800]);
        assert!(out.by_ref().take(2000).any(|s| s > 0.0));
        // Конец трека играет и без запаса.
        shared.push(&vec![1.0; 100]);
        shared.finish();
        out.by_ref().count();
        assert!(shared.finished());
    }

    /// Порция проходит через эквалайзер: предусилитель −6 дБ уменьшает звук.
    #[test]
    fn samples_go_through_equalizer() {
        let shared = shared();
        shared.set_rate(1000);
        shared.eq.set(crate::player::eq::EqSettings {
            enabled: true,
            preamp: -60,
            ..Default::default()
        });
        shared.push(&vec![1.0; 6000]);
        let mut out = output(&shared);
        let samples: Vec<f32> = out.by_ref().take(5000).collect();
        let last = samples.last().copied().unwrap_or(0.0);
        assert!((last - 0.501).abs() < 0.01, "{last}");
    }

    #[test]
    fn volume_is_clamped() {
        let volume = Volume::default();
        volume.set(f32::NAN);
        assert_eq!(volume.get(), 0.0);
        volume.set(7.0);
        assert_eq!(volume.get(), 1.0);
    }
}
