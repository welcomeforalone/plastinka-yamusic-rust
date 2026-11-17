//! Текст песни: колонка строк, которую виджет рисует сам. Активная строка
//! считается по часам, а не по сообщениям окна: позиция плеера приходит
//! раз в 200 мс, поэтому виджет сам ведёт её от последней известной (с
//! небольшим опережением, чтобы строка загоралась вместе с голосом) и просит
//! перерисовку ровно к началу следующей строки. Прокрутка к активной строке —
//! по времени (ease-out за 400 мс, при смене цели подхватывается без рывка),
//! цвет строк меняется за 200 мс. Кадры (~60 в секунду) просятся только пока
//! идёт доводка; в покое окно не перерисовывается вовсе.

use std::time::Duration;

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::{self, Renderer as _};
use iced::advanced::text::{self, Renderer as _, paragraph::Plain};
use iced::advanced::widget::{Tree, Widget, tree};
use iced::advanced::{Clipboard, Shell};
use iced::alignment::Vertical;
use iced::time::Instant;
use iced::widget::text::{LineHeight, Shaping, Wrapping};
use iced::{Color, Element, Event, Length, Pixels, Point, Rectangle, Size, Theme, mouse, window};

use super::theme::{self, Style};

/// Где стоит активная строка: доля высоты сверху.
const FOCUS: f32 = 0.3;
/// Свободное место под последней строкой, долей высоты: чтобы и последняя
/// строка могла встать на место активной.
const TAIL: f32 = 0.6;
/// Промежуток между строками.
const GAP: f32 = 14.0;
/// Сколько длится доводка прокрутки.
const SCROLL_TIME: Duration = Duration::from_millis(400);
/// Сколько длится смена цвета строки.
const COLOR_TIME: Duration = Duration::from_millis(200);
/// Кадр во время доводки (~60 в секунду).
const FRAME: Duration = Duration::from_millis(16);
/// Опережение: строка загорается чуть раньше метки, потому что звук идёт
/// с задержкой вывода.
pub const LEAD_MS: u64 = 180;
/// Пикселей на «строку» колеса мыши.
const WHEEL_STEP: f32 = 60.0;
/// Пауза в тексте (пустая строка) выше этой доли высоты строки.
const PAUSE_HEIGHT: f32 = 0.6;

/// Последняя известная позиция плеера и когда она пришла.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Anchor {
    pub position_ms: u64,
    pub at: Instant,
    /// Идёт ли звук: на паузе позиция стоит.
    pub playing: bool,
}

/// Где трек «сейчас» в момент `now`: последняя позиция плюс прошедшее время
/// и опережение (пока играет).
pub fn position_at(anchor: &Anchor, now: Instant) -> u64 {
    if !anchor.playing {
        return anchor.position_ms;
    }
    let elapsed = now.saturating_duration_since(anchor.at);
    let elapsed = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
    anchor
        .position_ms
        .saturating_add(elapsed)
        .saturating_add(LEAD_MS)
}

/// Номер строки, которая звучит в `position_ms`; до первой — `None`.
pub fn active_in(times: &[u64], position_ms: u64) -> Option<usize> {
    times
        .partition_point(|time| *time <= position_ms)
        .checked_sub(1)
}

/// Через сколько начнётся следующая строка. `None` — дальше строк нет.
pub fn until_next(times: &[u64], position_ms: u64) -> Option<Duration> {
    let next = times.iter().find(|time| **time > position_ms)?;
    Some(Duration::from_millis(next - position_ms))
}

/// Через сколько просить следующую перерисовку: к кадру, пока что-то
/// движется, иначе к началу следующей строки (если звук идёт); иначе никогда.
pub fn next_wake(animating: bool, playing: bool, until_next: Option<Duration>) -> Option<Duration> {
    let line = until_next.filter(|_| playing);
    match (animating, line) {
        (true, Some(line)) => Some(FRAME.min(line)),
        (true, None) => Some(FRAME),
        (false, line) => line,
    }
}

/// Сколько прокрутки до цели, чтобы строка с верхом `line_top` встала на
/// место активной. Прокрутка зажата в пределах содержимого.
pub fn scroll_target(line_top: f32, viewport: f32, content: f32) -> f32 {
    let max = (content - viewport).max(0.0);
    (line_top - viewport * FOCUS).clamp(0.0, max)
}

/// Номер строки под точкой `y` (в координатах содержимого). Промежутки между
/// строками делятся пополам.
pub fn hit(lines: &[(f32, f32)], y: f32) -> Option<usize> {
    lines
        .iter()
        .position(|(top, height)| y >= top - GAP / 2.0 && y < top + height + GAP / 2.0)
}

/// Прокрутка по времени: кубическая кривая Эрмита от текущего положения со
/// своей скоростью к цели с нулевой скоростью. Из покоя это ease-out
/// (`1 − (1 − s)³`), а смена цели посреди движения продолжает его с прежними
/// положением и скоростью: рывка нет.
#[derive(Debug, Clone, Copy)]
pub struct Scroll {
    from: f32,
    /// Скорость в начале, пикселей в секунду.
    speed: f32,
    target: f32,
    start: Instant,
}

impl Scroll {
    /// Стоит на месте.
    pub fn rest(at: f32, now: Instant) -> Self {
        Self {
            from: at,
            speed: 0.0,
            target: at,
            start: now,
        }
    }

    pub fn target(&self) -> f32 {
        self.target
    }

    /// Доля пройденного времени, 0…1.
    fn share(&self, now: Instant) -> f32 {
        let elapsed = now.saturating_duration_since(self.start).as_secs_f32();
        (elapsed / SCROLL_TIME.as_secs_f32()).clamp(0.0, 1.0)
    }

    pub fn value(&self, now: Instant) -> f32 {
        let s = self.share(now);
        if s >= 1.0 {
            return self.target;
        }
        let d = SCROLL_TIME.as_secs_f32();
        let h00 = 2.0 * s * s * s - 3.0 * s * s + 1.0;
        let h10 = s * s * s - 2.0 * s * s + s;
        let h01 = -2.0 * s * s * s + 3.0 * s * s;
        h00 * self.from + h10 * d * self.speed + h01 * self.target
    }

    /// Скорость в момент `now`, пикселей в секунду.
    pub fn velocity(&self, now: Instant) -> f32 {
        let s = self.share(now);
        if s >= 1.0 {
            return 0.0;
        }
        let d = SCROLL_TIME.as_secs_f32();
        let h00 = 6.0 * s * s - 6.0 * s;
        let h10 = 3.0 * s * s - 4.0 * s + 1.0;
        let h01 = -6.0 * s * s + 6.0 * s;
        (h00 * self.from + h10 * d * self.speed + h01 * self.target) / d
    }

    /// Доехала ли до цели.
    pub fn done(&self, now: Instant) -> bool {
        // Стоящая на месте прокрутка не «едет», сколько бы времени ни прошло.
        let at_rest = self.from == self.target && self.speed == 0.0;
        at_rest || self.share(now) >= 1.0
    }

    /// Новая цель. Движущаяся прокрутка продолжается с текущей скоростью;
    /// из покоя разгоняется как ease-out.
    pub fn retarget(&mut self, now: Instant, target: f32) {
        let here = self.value(now);
        let speed = if self.done(now) {
            3.0 * (target - here) / SCROLL_TIME.as_secs_f32()
        } else {
            self.velocity(now)
        };
        *self = Self {
            from: here,
            speed,
            target,
            start: now,
        };
    }
}

/// Цвета строк: прошедшие, звучащая, будущие.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Colors {
    pub past: Color,
    pub active: Color,
    pub future: Color,
}

impl Colors {
    /// Цвет строки `index` при активной `active`.
    fn of(&self, index: usize, active: Option<usize>) -> Color {
        match active {
            Some(active) if index == active => self.active,
            Some(active) if index < active => self.past,
            _ => self.future,
        }
    }
}

/// Цвет строки в момент смены активной: плавный переход от прежнего класса к
/// новому. `progress` — 0…1.
pub fn blend(
    colors: &Colors,
    index: usize,
    from: Option<usize>,
    to: Option<usize>,
    progress: f32,
) -> Color {
    let before = colors.of(index, from);
    let after = colors.of(index, to);
    if before == after || progress >= 1.0 {
        after
    } else if progress <= 0.0 {
        before
    } else {
        theme::mix(before, after, progress)
    }
}

type Paragraph = <iced::Renderer as text::Renderer>::Paragraph;

pub struct LyricsView<'a, Message> {
    lines: Vec<&'a str>,
    /// Метки времени строк, мс; пусто — обычный текст без подсветки.
    times: Vec<u64>,
    anchor: Option<Anchor>,
    style: Style,
    colors: Colors,
    /// Что показано: при смене текста прокрутка начинается заново.
    key: u64,
    on_press: Option<Box<dyn Fn(usize) -> Message + 'a>>,
}

/// Прокручиваемый текст. `times` пустой — обычный текст.
pub fn lyrics_view<'a, Message>(
    lines: Vec<&'a str>,
    times: Vec<u64>,
    anchor: Option<Anchor>,
    style: Style,
    colors: Colors,
    key: u64,
) -> LyricsView<'a, Message> {
    LyricsView {
        lines,
        times,
        anchor,
        style,
        colors,
        key,
        on_press: None,
    }
}

impl<'a, Message> LyricsView<'a, Message> {
    /// Щелчок по строке: сообщение по её номеру (только у синхронного текста).
    pub fn on_press(mut self, f: impl Fn(usize) -> Message + 'a) -> Self {
        self.on_press = Some(Box::new(f));
        self
    }

    fn text<'t>(&self, content: &'t str, width: f32) -> text::Text<&'t str, iced::Font> {
        let (size, line_height, font) = self.style.metrics();
        text::Text {
            content,
            bounds: Size::new(width, f32::INFINITY),
            size: Pixels(size),
            line_height: LineHeight::Absolute(Pixels(line_height)),
            font,
            align_x: text::Alignment::Left,
            align_y: Vertical::Top,
            shaping: Shaping::Advanced,
            wrapping: Wrapping::Word,
        }
    }
}

struct State {
    key: Option<u64>,
    paragraphs: Vec<Plain<Paragraph>>,
    /// Верх и высота каждой строки в координатах содержимого.
    tops: Vec<(f32, f32)>,
    content: f32,
    viewport: f32,
    scroll: Scroll,
    /// Пользователь листал колесом: цель не пересчитывается до смены строки.
    manual: bool,
    active: Option<usize>,
    /// Какая строка была активной до последней смены и когда сменилась.
    from_active: Option<usize>,
    changed: Option<Instant>,
    /// Момент последнего кадра: по нему рисуем.
    now: Instant,
    /// Первый показ после смены текста: встаём на место без проезда.
    fresh: bool,
}

impl Default for State {
    fn default() -> Self {
        let now = Instant::now();
        Self {
            key: None,
            paragraphs: Vec::new(),
            tops: Vec::new(),
            content: 0.0,
            viewport: 0.0,
            scroll: Scroll::rest(0.0, now),
            manual: false,
            active: None,
            from_active: None,
            changed: None,
            now,
            fresh: true,
        }
    }
}

impl State {
    /// Идёт ли смена цвета.
    fn color_running(&self, now: Instant) -> bool {
        self.changed
            .is_some_and(|at| now.saturating_duration_since(at) < COLOR_TIME)
    }

    fn color_progress(&self, now: Instant) -> f32 {
        self.changed.map_or(1.0, |at| {
            now.saturating_duration_since(at).as_secs_f32() / COLOR_TIME.as_secs_f32()
        })
    }
}

impl<Message> Widget<Message, Theme, iced::Renderer> for LyricsView<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        _renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let size = limits.resolve(Length::Fill, Length::Fill, Size::ZERO);
        let state = tree.state.downcast_mut::<State>();
        if state.key != Some(self.key) {
            *state = State {
                key: Some(self.key),
                ..State::default()
            };
        }
        state
            .paragraphs
            .resize_with(self.lines.len(), Plain::default);
        let (_, line_height, _) = self.style.metrics();
        let mut y = size.height * FOCUS;
        state.tops.clear();
        for (line, paragraph) in self.lines.iter().zip(state.paragraphs.iter_mut()) {
            let height = if line.is_empty() {
                line_height * PAUSE_HEIGHT
            } else {
                paragraph.update(self.text(line, size.width));
                paragraph.min_height()
            };
            state.tops.push((y, height));
            y += height + GAP;
        }
        state.content = y + size.height * TAIL;
        state.viewport = size.height;
        layout::Node::new(size)
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<State>();
        let bounds = layout.bounds();
        match event {
            Event::Mouse(mouse::Event::WheelScrolled { delta }) if cursor.is_over(bounds) => {
                let max = (state.content - state.viewport).max(0.0);
                match delta {
                    // Колесо: плавно к цели, щелчки складываются.
                    mouse::ScrollDelta::Lines { y, .. } => {
                        let base = if state.scroll.done(state.now) {
                            state.scroll.value(state.now)
                        } else {
                            state.scroll.target()
                        };
                        let target = (base - y * WHEEL_STEP).clamp(0.0, max);
                        state.scroll.retarget(state.now, target);
                    }
                    // Тачпад и так плавный: двигаем сразу.
                    mouse::ScrollDelta::Pixels { y, .. } => {
                        let at = (state.scroll.value(state.now) - y).clamp(0.0, max);
                        state.scroll = Scroll::rest(at, state.now);
                    }
                }
                state.manual = true;
                shell.capture_event();
                shell.request_redraw();
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let (Some(on_press), Some(position)) =
                    (&self.on_press, cursor.position_over(bounds))
                    && !self.times.is_empty()
                {
                    let y = position.y - bounds.y + state.scroll.value(state.now);
                    if let Some(index) = hit(&state.tops, y) {
                        shell.publish(on_press(index));
                        shell.capture_event();
                    }
                }
            }
            Event::Window(window::Event::RedrawRequested(now)) => {
                let now = *now;
                state.now = now;
                let position = self.anchor.map(|anchor| position_at(&anchor, now));
                let active = position.and_then(|position| active_in(&self.times, position));
                if active != state.active {
                    state.from_active = state.active;
                    state.active = active;
                    state.changed = (!state.fresh).then_some(now);
                    state.manual = false;
                }
                if !state.manual {
                    let target = state
                        .active
                        .and_then(|index| state.tops.get(index))
                        .map_or(0.0, |(top, _)| {
                            scroll_target(*top, state.viewport, state.content)
                        });
                    if state.fresh {
                        // Первый показ: сразу на месте, без проезда с начала.
                        state.scroll = Scroll::rest(target, now);
                    } else if (state.scroll.target() - target).abs() > 0.5 {
                        state.scroll.retarget(now, target);
                    }
                }
                state.fresh = false;
                let animating = !state.scroll.done(now) || state.color_running(now);
                let playing = self.anchor.is_some_and(|anchor| anchor.playing);
                let next = position.and_then(|position| until_next(&self.times, position));
                if let Some(wait) = next_wake(animating, playing, next)
                    && let Some(at) = now.checked_add(wait)
                {
                    shell.request_redraw_at(window::RedrawRequest::At(at));
                }
            }
            _ => {}
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<State>();
        let bounds = layout.bounds();
        let Some(clip) = bounds.intersection(viewport) else {
            return;
        };
        let offset = state.scroll.value(state.now);
        let progress = state.color_progress(state.now);
        renderer.with_layer(clip, |renderer| {
            for (index, (paragraph, (top, height))) in
                state.paragraphs.iter().zip(&state.tops).enumerate()
            {
                let y = bounds.y + top - offset;
                // Строки вне области не рисуем.
                if y + height < clip.y
                    || y > clip.y + clip.height
                    || self.lines.get(index).is_none_or(|line| line.is_empty())
                {
                    continue;
                }
                let color = if self.times.is_empty() {
                    self.colors.future
                } else {
                    blend(
                        &self.colors,
                        index,
                        state.from_active,
                        state.active,
                        progress,
                    )
                };
                renderer.fill_paragraph(paragraph.raw(), Point::new(bounds.x, y), color, clip);
            }
        });
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let state = tree.state.downcast_ref::<State>();
        let bounds = layout.bounds();
        match (&self.on_press, cursor.position_over(bounds)) {
            (Some(_), Some(position))
                if !self.times.is_empty()
                    && hit(
                        &state.tops,
                        position.y - bounds.y + state.scroll.value(state.now),
                    )
                    .is_some() =>
            {
                mouse::Interaction::Pointer
            }
            _ => mouse::Interaction::None,
        }
    }
}

impl<'a, Message: 'a> From<LyricsView<'a, Message>> for Element<'a, Message> {
    fn from(view: LyricsView<'a, Message>) -> Self {
        Element::new(view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_puts_line_at_a_third_and_clamps() {
        assert_eq!(scroll_target(1000.0, 600.0, 3000.0), 820.0);
        assert_eq!(scroll_target(180.0, 600.0, 3000.0), 0.0);
        assert_eq!(scroll_target(2900.0, 600.0, 3000.0), 2400.0);
        assert_eq!(scroll_target(500.0, 600.0, 400.0), 0.0);
    }

    #[test]
    fn hit_finds_line_with_gaps() {
        let lines = [(100.0, 30.0), (144.0, 60.0)];
        assert_eq!(hit(&lines, 99.0), Some(0));
        assert_eq!(hit(&lines, 136.9), Some(0));
        assert_eq!(hit(&lines, 137.0), Some(1));
        assert_eq!(hit(&lines, 10.0), None);
        assert_eq!(hit(&lines, 500.0), None);
        assert_eq!(hit(&[], 5.0), None);
    }

    fn anchor(playing: bool, at: Instant) -> Anchor {
        Anchor {
            position_ms: 10_000,
            at,
            playing,
        }
    }

    #[test]
    fn position_runs_forward_with_lead_while_playing() {
        let at = Instant::now();
        let playing = anchor(true, at);
        // Только что пришла: уже с опережением.
        assert_eq!(position_at(&playing, at), 10_000 + LEAD_MS);
        // Прошло 150 мс без новых событий.
        assert_eq!(
            position_at(&playing, at + Duration::from_millis(150)),
            10_000 + 150 + LEAD_MS
        );
        // Время не идёт назад.
        assert_eq!(
            position_at(&playing, at - Duration::from_millis(5)),
            10_000 + LEAD_MS
        );
    }

    #[test]
    fn paused_position_stands_still() {
        let at = Instant::now();
        let paused = anchor(false, at);
        assert_eq!(position_at(&paused, at + Duration::from_secs(5)), 10_000);
    }

    #[test]
    fn active_line_and_next_change() {
        let times = [1000, 5000, 5000, 9000];
        assert_eq!(active_in(&times, 0), None);
        assert_eq!(active_in(&times, 1000), Some(0));
        assert_eq!(active_in(&times, 5000), Some(2));
        assert_eq!(active_in(&times, 99_000), Some(3));
        assert_eq!(until_next(&times, 0), Some(Duration::from_millis(1000)));
        assert_eq!(until_next(&times, 1000), Some(Duration::from_millis(4000)));
        assert_eq!(until_next(&times, 9000), None);
        assert_eq!(until_next(&[], 5), None);
    }

    #[test]
    fn wake_is_at_next_line_when_idle_and_at_frame_when_moving() {
        let line = Some(Duration::from_millis(900));
        // Покой, играет: ровно к следующей строке.
        assert_eq!(
            next_wake(false, true, line),
            Some(Duration::from_millis(900))
        );
        // Покой, пауза или строк больше нет: перерисовок нет.
        assert_eq!(next_wake(false, false, line), None);
        assert_eq!(next_wake(false, true, None), None);
        // Движение: кадр, а если строка раньше кадра — к строке.
        assert_eq!(next_wake(true, true, line), Some(FRAME));
        assert_eq!(
            next_wake(true, true, Some(Duration::from_millis(5))),
            Some(Duration::from_millis(5))
        );
        assert_eq!(next_wake(true, false, line), Some(FRAME));
    }

    #[test]
    fn scroll_from_rest_is_ease_out_and_finishes() {
        let start = Instant::now();
        let mut scroll = Scroll::rest(0.0, start);
        scroll.retarget(start, 100.0);
        assert_eq!(scroll.value(start), 0.0);
        // Середина по времени: 1 − (1 − 0.5)³ = 0.875.
        let middle = scroll.value(start + SCROLL_TIME / 2);
        assert!((middle - 87.5).abs() < 0.01, "{middle}");
        assert_eq!(scroll.value(start + SCROLL_TIME), 100.0);
        assert!(!scroll.done(start + SCROLL_TIME / 2));
        assert!(scroll.done(start + SCROLL_TIME));
        // Монотонно, без отката.
        let mut last = 0.0;
        for step in 0..=40 {
            let value = scroll.value(start + SCROLL_TIME * step / 40);
            assert!(value >= last - 1e-3);
            last = value;
        }
    }

    #[test]
    fn retarget_in_motion_keeps_position_and_speed() {
        let start = Instant::now();
        let mut scroll = Scroll::rest(0.0, start);
        scroll.retarget(start, 100.0);
        let mid = start + Duration::from_millis(120);
        let (value, speed) = (scroll.value(mid), scroll.velocity(mid));
        assert!(speed > 0.0);
        // Цель сменилась на дальнюю: ни скачка положения, ни скачка скорости.
        scroll.retarget(mid, 300.0);
        assert!((scroll.value(mid) - value).abs() < 0.01);
        assert!((scroll.velocity(mid) - speed).abs() < 0.5);
        // Через миг положение рядом с прежним (плавно).
        let soon = mid + Duration::from_millis(16);
        assert!((scroll.value(soon) - value).abs() < speed * 0.016 * 1.5 + 0.5);
        assert_eq!(scroll.value(mid + SCROLL_TIME), 300.0);
        // Цель назад: тоже без скачка положения.
        let mut back = Scroll::rest(200.0, start);
        back.retarget(start, 260.0);
        let mid = start + Duration::from_millis(100);
        let before = back.value(mid);
        back.retarget(mid, 220.0);
        assert!((back.value(mid) - before).abs() < 0.01);
        assert_eq!(back.value(mid + SCROLL_TIME), 220.0);
    }

    #[test]
    fn colors_fade_between_classes() {
        let colors = Colors {
            past: Color::from_rgb(0.2, 0.2, 0.2),
            active: Color::WHITE,
            future: Color::from_rgb(0.6, 0.6, 0.6),
        };
        // Строка 2 стала активной (раньше была будущей).
        assert_eq!(blend(&colors, 2, Some(1), Some(2), 0.0), colors.future);
        assert_eq!(blend(&colors, 2, Some(1), Some(2), 1.0), colors.active);
        let half = blend(&colors, 2, Some(1), Some(2), 0.5);
        assert!(half.r > colors.future.r && half.r < colors.active.r);
        // Строка 1 из активной стала прошедшей.
        assert_eq!(blend(&colors, 1, Some(1), Some(2), 1.0), colors.past);
        // Класс не менялся: цвет прежний на любом шаге.
        assert_eq!(blend(&colors, 5, Some(1), Some(2), 0.3), colors.future);
        // Без активной строки все будущие.
        assert_eq!(blend(&colors, 0, None, None, 1.0), colors.future);
    }
}
