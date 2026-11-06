//! Визуализатор спектра под плеером.
//!
//! Источник звука кладёт последние отсчёты в отвод ([`Tap`], только
//! память). На каждом кадре окно берёт копию, считает спектр (БПФ на 2048
//! отсчётов с окном Ханна), раскладывает его по [`BARS`] полосам в
//! логарифмической шкале и сглаживает: столбики быстро растут и медленно
//! опускаются. Нет звука (пауза, конец трека) — столбики плавно опускаются, и
//! кадры больше не нужны.

use std::time::Duration;

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::{self, Quad, Renderer as _};
use iced::advanced::widget::{Tree, Widget};
use iced::time::Instant;
use iced::{Border, Color, Element, Length, Rectangle, Size, Theme, mouse};

use crate::player::{TAP_LEN, Tap};

/// Сколько столбиков.
pub const BARS: usize = 64;
/// Нижняя и верхняя частоты шкалы, Гц.
const LOW_HZ: f32 = 35.0;
const HIGH_HZ: f32 = 16_000.0;
/// Громкость полосы, которая даёт пустой и полный столбик, дБ.
const FLOOR_DB: f32 = -62.0;
const CEIL_DB: f32 = -12.0;
/// Наклон: высокие частоты в музыке тише, поднимаем их на столько дБ на
/// октаву выше 1 кГц.
const TILT_DB_PER_OCTAVE: f32 = 3.0;
/// Скорость падения столбика: доля высоты в секунду.
const FALL_PER_SECOND: f32 = 1.4;
/// Как быстро столбик догоняет рост (чем больше, тем резче).
const RISE_RATE: f32 = 28.0;
/// Отсчёты старше этого — уже не звук, а пауза или конец.
const FRESH: Duration = Duration::from_millis(150);
/// Ниже этой высоты столбик считается опустившимся.
const REST: f32 = 0.002;

/// Спектр и сглаженные столбики. Вся работа — в потоке окна, на кадре.
pub struct Analyzer {
    samples: Box<[f32; TAP_LEN]>,
    re: Vec<f32>,
    im: Vec<f32>,
    window: Vec<f32>,
    window_sum: f32,
    twiddles: Vec<(f32, f32)>,
    levels: [f32; BARS],
    last: Option<Instant>,
    /// Отладочный хук `PLASTINKA_TEST_SPECTRUM`: вместо тишины поддельного
    /// потока — смесь синусов, чтобы столбики было видно.
    #[cfg(debug_assertions)]
    demo: Option<u32>,
}

impl Default for Analyzer {
    fn default() -> Self {
        let n = TAP_LEN;
        let window: Vec<f32> = (0..n)
            .map(|i| {
                let phase = std::f32::consts::TAU * i as f32 / (n - 1) as f32;
                0.5 - 0.5 * phase.cos()
            })
            .collect();
        let window_sum = window.iter().sum();
        let twiddles = (0..n / 2)
            .map(|k| {
                let angle = -std::f32::consts::TAU * k as f32 / n as f32;
                (angle.cos(), angle.sin())
            })
            .collect();
        Self {
            samples: Box::new([0.0; TAP_LEN]),
            re: vec![0.0; n],
            im: vec![0.0; n],
            window,
            window_sum,
            twiddles,
            levels: [0.0; BARS],
            last: None,
            #[cfg(debug_assertions)]
            demo: demo_hook().then_some(0),
        }
    }
}

impl Analyzer {
    /// Новый кадр. `tap` — отвод звука, если плеер есть и визуализатор
    /// включён. Без него столбики опускаются.
    pub fn frame(&mut self, tap: Option<&Tap>, now: Instant) {
        let dt = self
            .last
            .map_or(0.0, |last| {
                now.saturating_duration_since(last).as_secs_f32()
            })
            .min(0.1);
        self.last = Some(now);
        let rate = tap.and_then(|tap| tap.snapshot(&mut self.samples, FRESH));
        let mut targets = [0.0; BARS];
        if let Some(rate) = rate {
            #[cfg(debug_assertions)]
            self.fill_demo(rate);
            self.spectrum(rate, &mut targets);
        }
        for (level, target) in self.levels.iter_mut().zip(targets) {
            *level = if target > *level {
                *level + (target - *level) * (dt * RISE_RATE).min(1.0)
            } else {
                (*level - dt * FALL_PER_SECOND).max(target)
            };
        }
    }

    /// Столбики ещё не опустились: нужны кадры.
    pub fn moving(&self) -> bool {
        self.levels.iter().any(|level| *level > REST)
    }

    /// Визуализатор выключили: столбики пропадают сразу.
    pub fn clear(&mut self) {
        self.levels = [0.0; BARS];
        self.last = None;
    }

    /// Смесь синусов с меняющейся громкостью вместо настоящих отсчётов.
    #[cfg(debug_assertions)]
    fn fill_demo(&mut self, rate: u32) {
        let Some(frame) = self.demo.as_mut() else {
            return;
        };
        *frame = frame.wrapping_add(1);
        let t = *frame as f32 / 30.0;
        let tones = [55.0, 110.0, 220.0, 440.0, 880.0, 1760.0, 3520.0, 7040.0];
        let rate = rate.max(1) as f32;
        for (i, sample) in self.samples.iter_mut().enumerate() {
            let x = i as f32 / rate;
            *sample = tones
                .iter()
                .enumerate()
                .map(|(k, hz)| {
                    let beat = 0.5 + 0.5 * (t * (0.7 + 0.37 * k as f32)).sin();
                    0.12 * beat * (std::f32::consts::TAU * hz * x).sin()
                })
                .sum();
        }
    }

    pub fn levels(&self) -> [f32; BARS] {
        self.levels
    }

    /// Высоты полос 0..1 для отсчётов в `self.samples`.
    fn spectrum(&mut self, rate: u32, out: &mut [f32; BARS]) {
        for ((re, im), (sample, weight)) in self
            .re
            .iter_mut()
            .zip(self.im.iter_mut())
            .zip(self.samples.iter().zip(&self.window))
        {
            *re = sample * weight;
            *im = 0.0;
        }
        fft(&mut self.re, &mut self.im, &self.twiddles);
        let n = self.re.len();
        let bin_hz = rate as f32 / n as f32;
        // Амплитуда синуса полной громкости даёт 1.
        let norm = 2.0 / self.window_sum.max(1.0);
        let nyquist = (n / 2).saturating_sub(1);
        let ratio = HIGH_HZ / LOW_HZ;
        for (band, slot) in out.iter_mut().enumerate() {
            let low = LOW_HZ * ratio.powf(band as f32 / BARS as f32);
            let high = LOW_HZ * ratio.powf((band + 1) as f32 / BARS as f32);
            // Номера полос малы и неотрицательны: потерь при переводе нет.
            let first = ((low / bin_hz) as usize).clamp(1, nyquist);
            let last = ((high / bin_hz) as usize).clamp(first, nyquist);
            let peak = (first..=last)
                .filter_map(|bin| Some((*self.re.get(bin)?, *self.im.get(bin)?)))
                .map(|(re, im)| (re * re + im * im).sqrt() * norm)
                .fold(0.0_f32, f32::max);
            let center = (low * high).sqrt();
            let db = 20.0 * (peak + 1e-9).log10() + TILT_DB_PER_OCTAVE * (center / 1000.0).log2();
            let level = (db - FLOOR_DB) / (CEIL_DB - FLOOR_DB);
            // Битые отсчёты (бесконечность, NaN) не должны ломать столбик.
            *slot = if level.is_nan() {
                0.0
            } else {
                level.clamp(0.0, 1.0)
            };
        }
    }
}

/// Быстрое преобразование Фурье по основанию 2 на месте. Длина — степень
/// двойки, `twiddles` — `e^(-2πik/n)` для `k < n/2`.
fn fft(re: &mut [f32], im: &mut [f32], twiddles: &[(f32, f32)]) {
    let n = re.len().min(im.len());
    if n < 2 || !n.is_power_of_two() {
        return;
    }
    let bits = n.trailing_zeros();
    for i in 0..n {
        let j = i.reverse_bits() >> (usize::BITS - bits);
        if j > i {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let half = len / 2;
        let step = n / len;
        for (re_block, im_block) in re.chunks_exact_mut(len).zip(im.chunks_exact_mut(len)) {
            let (re_low, re_high) = re_block.split_at_mut(half);
            let (im_low, im_high) = im_block.split_at_mut(half);
            for (k, ((ar, ai), (br, bi))) in re_low
                .iter_mut()
                .zip(im_low.iter_mut())
                .zip(re_high.iter_mut().zip(im_high.iter_mut()))
                .enumerate()
            {
                let (wr, wi) = twiddles.get(k * step).copied().unwrap_or((1.0, 0.0));
                let tr = *br * wr - *bi * wi;
                let ti = *br * wi + *bi * wr;
                *br = *ar - tr;
                *bi = *ai - ti;
                *ar += tr;
                *ai += ti;
            }
        }
        len *= 2;
    }
}

/// Включён ли отладочный хук: только вместе с фальшивым токеном.
#[cfg(debug_assertions)]
fn demo_hook() -> bool {
    let set = |var: &str| std::env::var_os(var).is_some_and(|value| !value.is_empty());
    set("PLASTINKA_TEST_SPECTRUM") && set("PLASTINKA_TEST_TOKEN")
}

// ---------------------------------------------------------------- вид

/// Столбики `levels` цвета `color` во всю ширину и высоту, снизу вверх.
pub fn bars<'a, Message: 'a>(levels: [f32; BARS], color: Color) -> Element<'a, Message> {
    Element::new(Bars { levels, color })
}

struct Bars {
    levels: [f32; BARS],
    color: Color,
}

/// Промежуток между столбиками.
const GAP: f32 = 2.0;

impl<Message> Widget<Message, Theme, iced::Renderer> for Bars {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn layout(
        &mut self,
        _tree: &mut Tree,
        _renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(limits.max())
    }

    fn draw(
        &self,
        _tree: &Tree,
        renderer: &mut iced::Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let count = BARS as f32;
        let width = ((bounds.width - GAP * (count - 1.0)) / count).max(1.0);
        for (index, level) in self.levels.iter().enumerate() {
            let height = (level * bounds.height).min(bounds.height);
            if height < 0.5 {
                continue;
            }
            let x = bounds.x + index as f32 * (width + GAP);
            renderer.fill_quad(
                Quad {
                    bounds: Rectangle {
                        x,
                        y: bounds.y + bounds.height - height,
                        width,
                        height,
                    },
                    border: Border::default().rounded(iced::border::Radius {
                        top_left: 2.0,
                        top_right: 2.0,
                        bottom_right: 0.0,
                        bottom_left: 0.0,
                    }),
                    ..Quad::default()
                },
                self.color,
            );
        }
    }
}

// ------------------------------------------------------- круговой спектр

/// Сколько лучей по кругу.
pub const RAYS: usize = 96;
/// Диаметр точки луча и шаг между точками, пиксели.
const DOT: f32 = 4.0;
const DOT_STEP: f32 = 2.5;
/// Самый короткий луч (на тишине): одна точка.
const MIN_LENGTH: f32 = DOT;

/// Номер полосы спектра для луча `ray`. Симметрия по вертикали: низкие
/// частоты внизу круга, высокие вверху, обе половины зеркальны.
pub fn ray_band(ray: usize) -> usize {
    let ray = ray % RAYS;
    let from_bottom = ray.min(RAYS - ray);
    let half = RAYS / 2;
    // Номер от 0 до half включительно растягиваем на все полосы.
    (from_bottom * (BARS - 1) + half / 2) / half
}

/// Угол луча в радианах: 0 — вниз, дальше по кругу.
pub fn ray_angle(ray: usize) -> f32 {
    std::f32::consts::TAU * (ray % RAYS) as f32 / RAYS as f32
}

/// Длина луча по уровню полосы: от одной точки до `max`.
pub fn ray_length(level: f32, max: f32) -> f32 {
    let level = if level.is_finite() {
        level.clamp(0.0, 1.0)
    } else {
        0.0
    };
    MIN_LENGTH + (max - MIN_LENGTH).max(0.0) * level
}

/// Центры точек луча длиной `length`, начиная с радиуса `inner` от центра.
pub fn ray_dots(center: iced::Point, angle: f32, inner: f32, length: f32) -> Vec<iced::Point> {
    let (sin, cos) = angle.sin_cos();
    let count = ((length - DOT) / DOT_STEP).max(0.0) as usize + 1;
    (0..count)
        .map(|n| {
            let radius = inner + DOT / 2.0 + n as f32 * DOT_STEP;
            iced::Point::new(center.x + radius * sin, center.y + radius * cos)
        })
        .collect()
}

/// Круговой спектр: лучи из точек цвета `color` вокруг центра, начиная с
/// радиуса `inner`, длиной до `max`. Занимает квадрат со стороной
/// `2 * (inner + max)`.
pub fn rays<'a, Message: 'a>(
    levels: [f32; BARS],
    color: Color,
    inner: f32,
    max: f32,
) -> Element<'a, Message> {
    Element::new(Rays {
        levels,
        color,
        inner,
        max,
    })
}

struct Rays {
    levels: [f32; BARS],
    color: Color,
    inner: f32,
    max: f32,
}

impl Rays {
    fn side(&self) -> f32 {
        2.0 * (self.inner + self.max)
    }
}

impl<Message> Widget<Message, Theme, iced::Renderer> for Rays {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(self.side()), Length::Fixed(self.side()))
    }

    fn layout(
        &mut self,
        _tree: &mut Tree,
        _renderer: &iced::Renderer,
        _limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(Size::new(self.side(), self.side()))
    }

    fn draw(
        &self,
        _tree: &Tree,
        renderer: &mut iced::Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let center = bounds.center();
        for ray in 0..RAYS {
            let level = self.levels.get(ray_band(ray)).copied().unwrap_or(0.0);
            let length = ray_length(level, self.max);
            let dots = ray_dots(center, ray_angle(ray), self.inner, length);
            let count = dots.len().max(1) as f32;
            for (n, dot) in dots.into_iter().enumerate() {
                // К концу луча точки бледнеют.
                let fade = 1.0 - 0.6 * (n as f32 / count);
                renderer.fill_quad(
                    Quad {
                        bounds: Rectangle {
                            x: dot.x - DOT / 2.0,
                            y: dot.y - DOT / 2.0,
                            width: DOT,
                            height: DOT,
                        },
                        border: Border::default().rounded(DOT / 2.0),
                        ..Quad::default()
                    },
                    Color {
                        a: self.color.a * fade,
                        ..self.color
                    },
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rays_are_mirrored_with_bass_at_the_bottom() {
        assert_eq!(ray_band(0), 0);
        assert_eq!(ray_band(RAYS / 2), BARS - 1);
        for ray in 1..RAYS {
            assert_eq!(ray_band(ray), ray_band(RAYS - ray), "{ray}");
            assert!(ray_band(ray) < BARS);
        }
        // Вверх по кругу полоса не убывает.
        for ray in 1..=RAYS / 2 {
            assert!(ray_band(ray) >= ray_band(ray - 1));
        }
        assert_eq!(ray_angle(0), 0.0);
        assert!((ray_angle(RAYS / 2) - std::f32::consts::PI).abs() < 1e-5);
    }

    #[test]
    fn ray_length_follows_the_level_and_survives_odd_input() {
        assert_eq!(ray_length(0.0, 40.0), MIN_LENGTH);
        assert_eq!(ray_length(1.0, 40.0), 40.0);
        assert!(ray_length(0.5, 40.0) > MIN_LENGTH && ray_length(0.5, 40.0) < 40.0);
        assert_eq!(ray_length(f32::NAN, 40.0), MIN_LENGTH);
        assert_eq!(ray_length(7.0, 40.0), 40.0);
        assert_eq!(ray_length(1.0, 0.0), MIN_LENGTH);
    }

    #[test]
    fn dots_start_at_the_inner_radius_and_go_outward() {
        let center = iced::Point::new(100.0, 100.0);
        // Угол 0 — вниз.
        let dots = ray_dots(center, 0.0, 50.0, 20.0);
        let first = dots.first().unwrap();
        assert!((first.x - 100.0).abs() < 1e-3);
        assert!((first.y - (150.0 + DOT / 2.0)).abs() < 1e-3);
        assert!(dots.windows(2).all(|pair| pair[1].y > pair[0].y));
        // Одна точка на тишине, больше на длинном луче.
        assert_eq!(ray_dots(center, 1.0, 50.0, MIN_LENGTH).len(), 1);
        assert!(dots.len() > 1);
        // Все точки на лучах не ближе внутреннего радиуса.
        for ray in 0..RAYS {
            for dot in ray_dots(center, ray_angle(ray), 50.0, 30.0) {
                let distance = ((dot.x - 100.0).powi(2) + (dot.y - 100.0).powi(2)).sqrt();
                assert!(distance >= 50.0 + DOT / 2.0 - 1e-3, "{ray}: {distance}");
            }
        }
    }

    fn tone(hz: f32, rate: u32, amplitude: f32) -> Tap {
        let tap = Tap::default();
        let samples: Vec<f32> = (0..TAP_LEN)
            .map(|i| amplitude * (std::f32::consts::TAU * hz * i as f32 / rate as f32).sin())
            .collect();
        tap.feed_for_tests(&samples, rate);
        tap
    }

    #[test]
    fn fft_finds_a_sine() {
        let n = 64;
        let mut analyzer = Analyzer::default();
        let twiddles: Vec<(f32, f32)> = (0..n / 2)
            .map(|k| {
                let angle = -std::f32::consts::TAU * k as f32 / n as f32;
                (angle.cos(), angle.sin())
            })
            .collect();
        let mut re: Vec<f32> = (0..n)
            .map(|i| (std::f32::consts::TAU * 5.0 * i as f32 / n as f32).cos())
            .collect();
        let mut im = vec![0.0; n];
        fft(&mut re, &mut im, &twiddles);
        let magnitude = |k: usize| (re[k] * re[k] + im[k] * im[k]).sqrt();
        assert!((magnitude(5) - n as f32 / 2.0).abs() < 1e-3);
        assert!(magnitude(4) < 1e-3 && magnitude(6) < 1e-3);
        analyzer.clear();
        assert!(!analyzer.moving());
    }

    #[test]
    fn loud_tone_raises_its_band_and_silence_lets_bars_fall() {
        let mut analyzer = Analyzer::default();
        let tap = tone(1000.0, 44_100, 0.5);
        let start = Instant::now();
        for frame in 0..20 {
            analyzer.frame(Some(&tap), start + Duration::from_millis(16 * frame));
        }
        let levels = analyzer.levels();
        let (loudest, _) =
            levels.iter().enumerate().fold(
                (0, 0.0),
                |best, (i, l)| if *l > best.1 { (i, *l) } else { best },
            );
        let ratio = HIGH_HZ / LOW_HZ;
        let band_hz = LOW_HZ * ratio.powf((loudest as f32 + 0.5) / BARS as f32);
        assert!((700.0..1400.0).contains(&band_hz), "{band_hz}");
        assert!(levels[loudest] > 0.8);
        assert!(analyzer.moving());
        // Звука нет: за пару секунд всё опускается.
        let later = start + Duration::from_secs(1);
        for frame in 0..200 {
            analyzer.frame(None, later + Duration::from_millis(16 * frame));
        }
        assert!(!analyzer.moving());
    }

    #[test]
    fn odd_input_does_not_break() {
        let mut analyzer = Analyzer::default();
        let tap = tone(30_000.0, 8_000, f32::MAX);
        analyzer.frame(Some(&tap), Instant::now());
        assert!(analyzer.levels().iter().all(|l| (0.0..=1.0).contains(l)));
        let tap = Tap::default();
        tap.feed_for_tests(&[f32::NAN; TAP_LEN], 44_100);
        analyzer.frame(Some(&tap), Instant::now());
        assert!(analyzer.levels().iter().all(|l| (0.0..=1.0).contains(l)));
        let mut re = vec![1.0; 3];
        let mut im = vec![0.0; 3];
        fft(&mut re, &mut im, &[]);
        assert_eq!(re, vec![1.0; 3]);
    }
}
