//! Графический эквалайзер: 10 полос, пиковые фильтры второго порядка
//! (формулы из общеизвестного «Audio EQ Cookbook», добротность в одну
//! октаву). Всё считается в памяти на лету, на диск пишутся только
//! параметры в `config.toml` (правило 1).
//!
//! Окно меняет [`EqControl`] (атомики, без блокировок), звуковой поток
//! читает его в [`Equalizer::process`] и плавно идёт к новым значениям.

use std::f64::consts::PI;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};

/// Число полос.
pub const BANDS: usize = 10;
/// Центральные частоты полос, Гц.
pub const FREQS: [u32; BANDS] = [32, 64, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];
/// Пределы усиления полосы и предусилителя, десятые доли дБ.
pub const GAIN_LIMIT: i16 = 120;
/// Не больше стольких своих пресетов.
pub const MAX_CUSTOM: usize = 32;
/// Длина имени своего пресета, символов.
pub const NAME_MAX: usize = 32;

/// Добротность пикового фильтра (октава).
const Q: f64 = 1.41;
/// Полоса не применяется, если её частота не ниже этой доли частоты
/// дискретизации: у верхней границы фильтр искажается.
const MAX_RATIO: f64 = 0.45;
/// Быстрее этого текущее усиление к цели не идёт, дБ за вызов `process`
/// (порция до 2048 отсчётов, около 23 мс).
const STEP_DB: f32 = 1.5;

fn clamp_gain(value: i16) -> i16 {
    value.clamp(-GAIN_LIMIT, GAIN_LIMIT)
}

/// Параметры эквалайзера. Усиления — в десятых долях дБ (целые: настройки
/// сравниваются по `Eq` и пишутся в TOML без дробей).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EqSettings {
    pub enabled: bool,
    pub auto_preamp: bool,
    pub preamp: i16,
    pub gains: [i16; BANDS],
}

impl EqSettings {
    /// Все значения в пределах ±[`GAIN_LIMIT`].
    pub fn sanitized(mut self) -> Self {
        self.preamp = clamp_gain(self.preamp);
        for gain in &mut self.gains {
            *gain = clamp_gain(*gain);
        }
        self
    }

    /// Предусилитель, который реально применяется: при `auto_preamp` ещё
    /// минус наибольшее положительное усиление полосы, чтобы пики не
    /// упирались в 0 dBFS.
    pub fn effective_preamp(&self) -> i16 {
        if self.auto_preamp {
            let peak = self.gains.iter().copied().max().unwrap_or(0).max(0);
            self.preamp.saturating_sub(peak)
        } else {
            self.preamp
        }
    }
}

/// Общее состояние эквалайзера для звукового потока: без блокировок.
#[derive(Default)]
pub struct EqControl {
    enabled: AtomicBool,
    auto_preamp: AtomicBool,
    preamp: AtomicI32,
    gains: [AtomicI32; BANDS],
    version: AtomicU64,
}

impl EqControl {
    /// Записывает параметры (значения вне пределов обрезаются) и увеличивает
    /// версию: звуковой поток заметит её на следующей порции.
    pub fn set(&self, settings: EqSettings) {
        let settings = settings.sanitized();
        self.enabled.store(settings.enabled, Ordering::Relaxed);
        self.auto_preamp
            .store(settings.auto_preamp, Ordering::Relaxed);
        self.preamp
            .store(i32::from(settings.preamp), Ordering::Relaxed);
        for (slot, gain) in self.gains.iter().zip(settings.gains) {
            slot.store(i32::from(gain), Ordering::Relaxed);
        }
        self.version.fetch_add(1, Ordering::Release);
    }

    /// Номер версии: растёт с каждым `set`.
    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    /// Версия и параметры. Версия читается первой: если запись шла
    /// одновременно, следующая порция заберёт её целиком.
    pub fn load(&self) -> (u64, EqSettings) {
        let version = self.version();
        let to_gain = |value: i32| clamp_gain(i16::try_from(value).unwrap_or(0));
        let mut settings = EqSettings {
            enabled: self.enabled.load(Ordering::Relaxed),
            auto_preamp: self.auto_preamp.load(Ordering::Relaxed),
            preamp: to_gain(self.preamp.load(Ordering::Relaxed)),
            gains: [0; BANDS],
        };
        for (gain, slot) in settings.gains.iter_mut().zip(&self.gains) {
            *gain = to_gain(slot.load(Ordering::Relaxed));
        }
        (version, settings)
    }
}

/// Коэффициенты одного фильтра (a0 уже поделён).
#[derive(Debug, Clone, Copy, Default)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    /// Нулевое усиление или частота слишком высокая — фильтр пропускается.
    active: bool,
}

impl Biquad {
    fn peaking(freq: u32, rate: u32, gain_db: f32) -> Self {
        if gain_db == 0.0 || f64::from(freq) >= MAX_RATIO * f64::from(rate) {
            return Self::default();
        }
        let a = 10_f64.powf(f64::from(gain_db) / 40.0);
        let w0 = 2.0 * PI * f64::from(freq) / f64::from(rate);
        let alpha = w0.sin() / (2.0 * Q);
        let cos = w0.cos();
        let a0 = 1.0 + alpha / a;
        Self {
            b0: (1.0 + alpha * a) / a0,
            b1: -2.0 * cos / a0,
            b2: (1.0 - alpha * a) / a0,
            a1: -2.0 * cos / a0,
            a2: (1.0 - alpha / a) / a0,
            active: true,
        }
    }
}

/// Память фильтров одного канала (транспонированная прямая форма II).
type Memory = [[f64; 2]; BANDS];

fn db_to_linear(db: f32) -> f64 {
    10_f64.powf(f64::from(db) / 20.0)
}

/// Шаг текущего значения к цели, не быстрее [`STEP_DB`].
fn slew(current: f32, target: f32) -> f32 {
    let delta = target - current;
    if delta.abs() <= STEP_DB {
        target
    } else {
        current + STEP_DB.copysign(delta)
    }
}

/// Цели в дБ по настройкам: выключен — все нули.
fn targets(settings: &EqSettings) -> ([f32; BANDS], f32) {
    if !settings.enabled {
        return ([0.0; BANDS], 0.0);
    }
    let mut gains = [0.0; BANDS];
    for (slot, gain) in gains.iter_mut().zip(settings.gains) {
        *slot = f32::from(gain) / 10.0;
    }
    (gains, f32::from(settings.effective_preamp()) / 10.0)
}

/// Один отсчёт одного канала через все фильтры и предусилитель. Нечисло на
/// входе или в памяти: память обнуляется, отсчёт — ноль, звук не «залипает».
fn run_channel(biquads: &[Biquad; BANDS], memory: &mut Memory, input: f32, preamp: f64) -> f32 {
    let mut value = f64::from(input);
    if value.is_finite() {
        for (biquad, cells) in biquads.iter().zip(memory.iter_mut()) {
            if !biquad.active {
                continue;
            }
            let [z1, z2] = cells;
            let out = biquad.b0 * value + *z1;
            *z1 = biquad.b1 * value - biquad.a1 * out + *z2;
            *z2 = biquad.b2 * value - biquad.a2 * out;
            value = out;
        }
        let result = (value * preamp) as f32;
        if result.is_finite() {
            return result;
        }
    }
    *memory = [[0.0; 2]; BANDS];
    0.0
}

/// Эквалайзер одного источника: свои коэффициенты и память фильтров.
pub struct Equalizer {
    rate: u32,
    /// Версия `EqControl`, которую уже учли.
    seen: u64,
    target: [f32; BANDS],
    target_preamp: f32,
    /// Текущие (сглаженные) усиления полос и предусилитель, дБ.
    gains: [f32; BANDS],
    preamp: f32,
    /// Множитель предусилителя в конце прошлой порции.
    preamp_linear: f64,
    biquads: [Biquad; BANDS],
    /// Память фильтров по каналам.
    memory: Vec<Memory>,
}

impl Equalizer {
    /// Источник с `channels` каналами и частотой `rate`. Текущие значения
    /// сразу берутся из `control`: новый трек не «въезжает» в эквалайзер.
    pub fn new(channels: usize, rate: u32, control: &EqControl) -> Self {
        let (seen, settings) = control.load();
        let (target, target_preamp) = targets(&settings);
        let mut biquads = [Biquad::default(); BANDS];
        for ((biquad, freq), gain) in biquads.iter_mut().zip(FREQS).zip(target) {
            *biquad = Biquad::peaking(freq, rate, gain);
        }
        Self {
            rate,
            seen,
            target,
            target_preamp,
            gains: target,
            preamp: target_preamp,
            preamp_linear: db_to_linear(target_preamp),
            biquads,
            memory: vec![[[0.0; 2]; BANDS]; channels.max(1)],
        }
    }

    /// Обрабатывает порцию на месте: отсчёты кадрами (interleaved). Без
    /// выделений памяти, блокировок и системных вызовов.
    pub fn process(&mut self, control: &EqControl, samples: &mut [f32]) {
        let version = control.version();
        if version != self.seen {
            let (seen, settings) = control.load();
            self.seen = seen;
            (self.target, self.target_preamp) = targets(&settings);
        }
        // Шаг к цели. Коэффициенты пересчитываются только у сдвинувшихся полос.
        for (index, (((gain, target), biquad), freq)) in self
            .gains
            .iter_mut()
            .zip(&self.target)
            .zip(self.biquads.iter_mut())
            .zip(FREQS)
            .enumerate()
        {
            let next = slew(*gain, *target);
            if next != *gain {
                *gain = next;
                *biquad = Biquad::peaking(freq, self.rate, next);
                // Полоса выключилась: её память не должна дождаться
                // следующего включения и щёлкнуть старым значением.
                if !biquad.active {
                    for memory in &mut self.memory {
                        if let Some(cells) = memory.get_mut(index) {
                            *cells = [0.0; 2];
                        }
                    }
                }
            }
        }
        self.preamp = slew(self.preamp, self.target_preamp);
        // Всё дошло до нуля, и прошлая порция уже закончилась на единичном
        // множителе (иначе на стыке была бы ступенька): точный обход, память
        // забываем.
        if self.preamp == 0.0
            && self.preamp_linear == 1.0
            && self.gains.iter().all(|gain| *gain == 0.0)
        {
            self.memory.fill([[0.0; 2]; BANDS]);
            self.preamp_linear = 1.0;
            return;
        }
        let channels = self.memory.len();
        let from = self.preamp_linear;
        let to = db_to_linear(self.preamp);
        let frames = samples.len() / channels;
        // Предусилитель меняется по кадрам плавно, без ступеньки на стыке порций.
        let step = if frames > 0 {
            (to - from) / frames as f64
        } else {
            0.0
        };
        let mut preamp = from;
        for frame in samples.chunks_exact_mut(channels) {
            preamp += step;
            for (sample, memory) in frame.iter_mut().zip(self.memory.iter_mut()) {
                *sample = run_channel(&self.biquads, memory, *sample, preamp);
            }
        }
        self.preamp_linear = to;
    }
}

// ------------------------------------------------------------------ пресеты

/// Встроенный пресет: значения полос в десятых долях дБ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preset {
    pub key: &'static str,
    pub title: &'static str,
    pub gains: [i16; BANDS],
}

const fn preset(key: &'static str, title: &'static str, gains: [i16; BANDS]) -> Preset {
    Preset { key, title, gains }
}

/// Встроенные пресеты по порядку показа.
pub const PRESETS: [Preset; 13] = [
    preset("flat", "Плоский", [0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
    preset("bass", "Бас", [60, 50, 40, 20, 0, 0, 0, 0, 0, 0]),
    preset("treble", "Высокие", [0, 0, 0, 0, 0, 0, 20, 40, 50, 60]),
    preset(
        "v-shape",
        "Бас и верх",
        [50, 40, 20, 0, -20, -20, 0, 20, 40, 50],
    ),
    preset(
        "vocal",
        "Вокал",
        [-20, -20, -10, 10, 30, 40, 40, 20, 0, -10],
    ),
    preset("rock", "Рок", [40, 30, 20, 0, -10, -10, 10, 30, 40, 40]),
    preset("pop", "Поп", [-10, 0, 20, 30, 40, 30, 10, 0, -10, -10]),
    preset(
        "electronic",
        "Электроника",
        [50, 40, 10, 0, -20, 10, 0, 20, 40, 50],
    ),
    preset(
        "hip-hop",
        "Хип-хоп",
        [50, 50, 30, 10, -10, -10, 10, 0, 20, 30],
    ),
    preset(
        "classical",
        "Классика",
        [30, 20, 10, 0, 0, 0, -10, -10, 20, 30],
    ),
    preset("jazz", "Джаз", [30, 20, 10, 20, -10, -10, 0, 10, 20, 30]),
    preset(
        "acoustic",
        "Акустика",
        [30, 30, 20, 10, 20, 10, 20, 30, 20, 10],
    ),
    preset("warm", "Тёплый", [30, 30, 20, 10, 0, 0, -10, -20, -30, -40]),
];

/// Встроенный пресет по ключу.
pub fn preset_by_key(key: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|preset| preset.key == key)
}

/// Встроенный пресет с такими полосами (предусилитель не сравнивается).
pub fn preset_for_gains(gains: &[i16; BANDS]) -> Option<&'static Preset> {
    PRESETS.iter().find(|preset| preset.gains == *gains)
}

/// Свой пресет пользователя.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomPreset {
    pub name: String,
    pub preamp: i16,
    pub gains: [i16; BANDS],
}

/// Выбранный пресет: ключ встроенного или имя своего.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresetId {
    Builtin(&'static str),
    Custom(String),
}

/// Почему имя или пресет не приняты.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PresetError {
    #[error("введите название пресета")]
    Empty,
    #[error("название длиннее {NAME_MAX} символов")]
    TooLong,
    #[error("такое название уже есть")]
    Duplicate,
    #[error("можно сохранить не больше {MAX_CUSTOM} пресетов")]
    Limit,
}

/// Состояние эквалайзера в окне: параметры, выбранный пресет и свои
/// пресеты. Не знает про TOML — его читает и пишет `storage::config`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EqState {
    pub settings: EqSettings,
    /// `None` — «Свои значения».
    pub preset: Option<PresetId>,
    pub custom: Vec<CustomPreset>,
}

impl Default for EqState {
    fn default() -> Self {
        Self {
            settings: EqSettings {
                auto_preamp: true,
                ..EqSettings::default()
            },
            preset: None,
            custom: Vec::new(),
        }
    }
}

impl EqState {
    /// Имя после `trim`, если оно годится для нового пресета.
    pub fn check_name(&self, name: &str) -> Result<String, PresetError> {
        let name = name.trim();
        let length = name.chars().count();
        if length == 0 {
            return Err(PresetError::Empty);
        }
        if length > NAME_MAX {
            return Err(PresetError::TooLong);
        }
        let lower = name.to_lowercase();
        let taken = PRESETS
            .iter()
            .any(|preset| preset.title.to_lowercase() == lower || preset.key == lower)
            || self
                .custom
                .iter()
                .any(|custom| custom.name.to_lowercase() == lower);
        if taken {
            return Err(PresetError::Duplicate);
        }
        Ok(name.to_owned())
    }

    /// Добавляет свой пресет (значения обрезаются до пределов).
    pub fn add_custom(
        &mut self,
        name: &str,
        preamp: i16,
        gains: [i16; BANDS],
    ) -> Result<(), PresetError> {
        let name = self.check_name(name)?;
        if self.custom.len() >= MAX_CUSTOM {
            return Err(PresetError::Limit);
        }
        let EqSettings { preamp, gains, .. } = EqSettings {
            preamp,
            gains,
            ..EqSettings::default()
        }
        .sanitized();
        self.custom.push(CustomPreset {
            name,
            preamp,
            gains,
        });
        Ok(())
    }

    /// Сохраняет текущие предусилитель и полосы под именем и выбирает его.
    pub fn save_current_as(&mut self, name: &str) -> Result<(), PresetError> {
        let name = self.check_name(name)?;
        self.add_custom(&name, self.settings.preamp, self.settings.gains)?;
        self.preset = Some(PresetId::Custom(name));
        Ok(())
    }

    /// Удаляет свой пресет. Если он был выбран — «Свои значения».
    pub fn remove_custom(&mut self, name: &str) {
        self.custom.retain(|custom| custom.name != name);
        if self.preset == Some(PresetId::Custom(name.to_owned())) {
            self.preset = None;
        }
    }

    /// Применяет пресет: полосы (у своего ещё и предусилитель) и выбор.
    /// `false` — такого пресета нет, ничего не изменилось.
    pub fn apply_preset(&mut self, id: &PresetId) -> bool {
        match id {
            PresetId::Builtin(key) => {
                let Some(preset) = preset_by_key(key) else {
                    return false;
                };
                self.settings.gains = preset.gains;
            }
            PresetId::Custom(name) => {
                let Some(custom) = self.custom.iter().find(|custom| custom.name == *name) else {
                    return false;
                };
                self.settings.gains = custom.gains;
                self.settings.preamp = custom.preamp;
            }
        }
        self.preset = Some(id.clone());
        true
    }

    /// Какому пресету соответствуют текущие значения: сначала свои
    /// (полосы и предусилитель), потом встроенные (только полосы).
    pub fn matching_preset(&self) -> Option<PresetId> {
        let settings = &self.settings;
        self.custom
            .iter()
            .find(|custom| custom.gains == settings.gains && custom.preamp == settings.preamp)
            .map(|custom| PresetId::Custom(custom.name.clone()))
            .or_else(|| preset_for_gains(&settings.gains).map(|p| PresetId::Builtin(p.key)))
    }

    /// Пресет с этим идентификатором существует и совпадает с текущими
    /// полосами (у своего ещё и с предусилителем).
    pub fn preset_matches(&self, id: &PresetId) -> bool {
        match id {
            PresetId::Builtin(key) => {
                preset_by_key(key).is_some_and(|preset| preset.gains == self.settings.gains)
            }
            PresetId::Custom(name) => self.custom.iter().any(|custom| {
                custom.name == *name
                    && custom.gains == self.settings.gains
                    && custom.preamp == self.settings.preamp
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(band: usize, gain: i16) -> EqSettings {
        let mut settings = EqSettings {
            enabled: true,
            ..EqSettings::default()
        };
        settings.gains[band] = gain;
        settings
    }

    /// Синус частоты `freq` через эквалайзер с заданным усилением полосы.
    /// Возвращает отношение амплитуды на выходе к входу.
    fn gain_at(rate: u32, band: usize, gain: i16, freq: f64) -> f64 {
        let control = EqControl::default();
        let mut eq = Equalizer::new(1, rate, &control);
        control.set(settings(band, gain));
        let mut index = 0_usize;
        let mut chunk = |eq: &mut Equalizer| {
            let mut data: Vec<f32> = (0..2048)
                .map(|i| {
                    let t = (index + i) as f64 / f64::from(rate);
                    (2.0 * PI * freq * t).sin() as f32
                })
                .collect();
            index += 2048;
            eq.process(&control, &mut data);
            data
        };
        for _ in 0..20 {
            chunk(&mut eq);
        }
        // Амплитуда по среднеквадратичному: пик на частоте у границы Найквиста
        // зависит от фазы выборки.
        let (mut sum, mut count) = (0.0_f64, 0_u32);
        for _ in 0..8 {
            for sample in chunk(&mut eq) {
                sum += f64::from(sample).powi(2);
                count += 1;
            }
        }
        (sum / f64::from(count)).sqrt() * std::f64::consts::SQRT_2
    }

    #[test]
    fn boost_and_cut_at_band_center() {
        for rate in [44_100, 48_000] {
            for band in [1, 5, 8] {
                let freq = f64::from(FREQS[band]);
                let up = gain_at(rate, band, 60, freq);
                assert!((up - 1.995).abs() < 0.1, "{rate} {band}: {up}");
                let down = gain_at(rate, band, -60, freq);
                assert!((down - 0.501).abs() < 0.025, "{rate} {band}: {down}");
            }
        }
    }

    #[test]
    fn far_from_band_is_untouched() {
        let value = gain_at(44_100, 1, 120, 4000.0);
        assert!((value - 1.0).abs() < 0.03, "{value}");
    }

    #[test]
    fn zero_gains_are_bit_exact_bypass() {
        let control = EqControl::default();
        let mut eq = Equalizer::new(2, 44_100, &control);
        control.set(EqSettings {
            enabled: true,
            ..EqSettings::default()
        });
        let input: Vec<f32> = (0..2048)
            .map(|i| ((i * 37) % 200) as f32 / 100.0 - 1.0)
            .collect();
        let mut data = input.clone();
        eq.process(&control, &mut data);
        assert_eq!(data, input);
    }

    #[test]
    fn disabled_after_fade_is_bit_exact() {
        let control = EqControl::default();
        let mut eq = Equalizer::new(2, 44_100, &control);
        control.set(settings(3, 90));
        let input: Vec<f32> = (0..2048).map(|i| ((i * 13) % 100) as f32 / 100.0).collect();
        for _ in 0..10 {
            let mut data = input.clone();
            eq.process(&control, &mut data);
        }
        assert_ne!(eq.gains[3], 0.0);
        control.set(EqSettings::default());
        for _ in 0..10 {
            let mut data = input.clone();
            eq.process(&control, &mut data);
        }
        let mut data = input.clone();
        eq.process(&control, &mut data);
        assert_eq!(data, input);
    }

    #[test]
    fn preamp_scales_signal() {
        let control = EqControl::default();
        let mut eq = Equalizer::new(1, 44_100, &control);
        control.set(EqSettings {
            enabled: true,
            preamp: -60,
            ..EqSettings::default()
        });
        let mut last = Vec::new();
        for _ in 0..10 {
            last = vec![1.0; 2048];
            eq.process(&control, &mut last);
        }
        assert!(last.iter().all(|s| (s - 0.501).abs() < 0.01), "{last:?}");
    }

    #[test]
    fn effective_preamp_follows_auto() {
        let mut s = EqSettings {
            enabled: true,
            auto_preamp: true,
            preamp: -30,
            gains: [50, 40, 20, 0, -20, -20, 0, 20, 40, 50],
        };
        assert_eq!(s.effective_preamp(), -80);
        s.auto_preamp = false;
        assert_eq!(s.effective_preamp(), -30);
        // Только срезы: нечего компенсировать.
        s.auto_preamp = true;
        s.gains = [-20; BANDS];
        assert_eq!(s.effective_preamp(), -30);
    }

    #[test]
    fn target_is_approached_gradually() {
        let control = EqControl::default();
        let mut eq = Equalizer::new(1, 44_100, &control);
        control.set(settings(5, 120));
        let mut data = vec![0.1; 2048];
        eq.process(&control, &mut data);
        assert_eq!(eq.gains[5], STEP_DB);
        for _ in 0..20 {
            eq.process(&control, &mut data);
        }
        assert_eq!(eq.gains[5], 12.0);
    }

    #[test]
    fn channels_are_independent() {
        let control = EqControl::default();
        let mut eq = Equalizer::new(2, 44_100, &control);
        control.set(settings(5, 120));
        for _ in 0..20 {
            let mut data: Vec<f32> = (0..2048)
                .map(|i| {
                    if i % 2 == 0 {
                        (i as f32 * 0.14).sin()
                    } else {
                        0.0
                    }
                })
                .collect();
            eq.process(&control, &mut data);
            assert!(data.iter().skip(1).step_by(2).all(|s| *s == 0.0));
            assert!(data.iter().step_by(2).any(|s| *s != 0.0));
        }
    }

    #[test]
    fn nan_does_not_stick() {
        let control = EqControl::default();
        let mut eq = Equalizer::new(2, 44_100, &control);
        control.set(settings(5, 60));
        let mut data = vec![0.3; 2048];
        eq.process(&control, &mut data);
        let mut bad = vec![0.3; 2048];
        bad[100] = f32::NAN;
        bad[200] = f32::INFINITY;
        eq.process(&control, &mut bad);
        assert!(bad.iter().all(|s| s.is_finite()));
        let mut next = vec![0.3; 2048];
        eq.process(&control, &mut next);
        assert!(next.iter().all(|s| s.is_finite()));
        assert!(next.iter().any(|s| *s != 0.0));
    }

    #[test]
    fn bands_near_nyquist_are_skipped() {
        let control = EqControl::default();
        let mut eq = Equalizer::new(1, 22_050, &control);
        let mut s = settings(9, 120);
        s.gains[8] = 60;
        control.set(s);
        let mut data: Vec<f32> = (0..2048).map(|i| (i as f32 * 0.5).sin()).collect();
        for _ in 0..20 {
            eq.process(&control, &mut data);
        }
        assert!(!eq.biquads[9].active);
        assert!(eq.biquads[8].active);
        assert!(data.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn control_set_load_and_limits() {
        let control = EqControl::default();
        assert_eq!(control.version(), 0);
        let mut s = settings(2, 500);
        s.preamp = -500;
        s.gains[4] = -300;
        control.set(s);
        let (version, loaded) = control.load();
        assert_eq!(version, 1);
        assert_eq!(loaded.gains[2], GAIN_LIMIT);
        assert_eq!(loaded.gains[4], -GAIN_LIMIT);
        assert_eq!(loaded.preamp, -GAIN_LIMIT);
        assert!(loaded.enabled);
        control.set(loaded);
        assert_eq!(control.load(), (2, loaded));
    }

    #[test]
    fn presets_are_valid() {
        assert_eq!(PRESETS.len(), 13);
        assert!(preset_by_key("rock").is_some());
        assert!(preset_by_key("nope").is_none());
        assert_eq!(preset_for_gains(&[0; BANDS]).map(|p| p.key), Some("flat"));
        for preset in PRESETS {
            assert!(preset.gains.iter().all(|g| g.abs() <= GAIN_LIMIT));
        }
    }

    #[test]
    fn custom_names_are_checked() {
        let mut state = EqState::default();
        assert_eq!(state.check_name("   "), Err(PresetError::Empty));
        assert_eq!(state.check_name(&"я".repeat(33)), Err(PresetError::TooLong));
        assert_eq!(state.check_name("  РОК "), Err(PresetError::Duplicate));
        assert_eq!(state.check_name("v-shape"), Err(PresetError::Duplicate));
        assert!(state.add_custom(" Наушники ", 0, [10; BANDS]).is_ok());
        assert_eq!(
            state.custom.first().map(|c| c.name.as_str()),
            Some("Наушники")
        );
        assert_eq!(
            state.add_custom("наушники", 0, [0; BANDS]),
            Err(PresetError::Duplicate)
        );
        for i in 0..MAX_CUSTOM {
            let _ = state.add_custom(&format!("п{i}"), 0, [0; BANDS]);
        }
        assert_eq!(state.custom.len(), MAX_CUSTOM);
        assert_eq!(
            state.add_custom("лишний", 0, [0; BANDS]),
            Err(PresetError::Limit)
        );
    }

    #[test]
    fn apply_and_match_presets() {
        let mut state = EqState::default();
        assert!(state.apply_preset(&PresetId::Builtin("bass")));
        assert_eq!(state.matching_preset(), Some(PresetId::Builtin("bass")));
        assert!(!state.apply_preset(&PresetId::Builtin("nope")));
        state.settings.preamp = -20;
        assert!(state.save_current_as("Мой").is_ok());
        assert_eq!(state.preset, Some(PresetId::Custom("Мой".to_owned())));
        assert_eq!(
            state.matching_preset(),
            Some(PresetId::Custom("Мой".to_owned()))
        );
        state.settings.gains[0] = 0;
        assert_eq!(state.matching_preset(), None);
        state.remove_custom("Мой");
        assert_eq!(state.preset, None);
        assert!(state.custom.is_empty());
    }
}
