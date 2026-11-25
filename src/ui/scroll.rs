//! Плавная прокрутка длинного списка и своя полоса прокрутки.
//!
//! У `scrollable` из iced колесо мыши двигает список рывком на 60 px за
//! щелчок, а ручка полосы при длинном списке сжимается до точки в 2 px.
//! Поэтому:
//! - [`wheel`] перехватывает щелчки колеса (не прокрутку тачпада, она и так
//!   плавная) и отдаёт их приложению, а то плавно доводит список до цели
//!   (`Glide`);
//! - [`smooth`] делает то же самое для любого `scrollable`, но сам, без
//!   сообщений приложению: колесо копится в остатке пути, а `scrollable`
//!   получает его кусками на каждом кадре;
//! - [`rail`] рисует свою полосу с ручкой не короче [`THUMB_MIN`]: её можно
//!   тянуть мышью, щелчок мимо ручки переносит её под курсор.

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::{self, Quad, Renderer as _};
use iced::advanced::widget::{self, Tree, Widget, tree};
use iced::advanced::{Clipboard, Shell, overlay};
use iced::time::Instant;
use iced::{
    Border, Color, Element, Event, Length, Point, Rectangle, Size, Theme, Vector, mouse, window,
};

use super::theme;

/// Сколько пикселей прокручивает один щелчок колеса.
pub const WHEEL_STEP: f32 = 100.0;
/// Постоянная времени доводки: за неё остаётся ~37 % пути, за три — 5 %.
const GLIDE_TAU: f32 = 0.075;
/// Ближе этого список считается доехавшим.
const GLIDE_DONE: f32 = 0.5;
/// Ширина полосы (зона, за которую можно ухватиться) и ручки.
pub const RAIL_WIDTH: f32 = 14.0;
const THUMB_WIDTH: f32 = 6.0;
const THUMB_WIDTH_ACTIVE: f32 = 8.0;
/// Самая короткая ручка.
const THUMB_MIN: f32 = 40.0;
/// Отступ ручки от краёв полосы по вертикали.
const RAIL_PAD: f32 = 4.0;

// ---------------------------------------------------------------- доводка

/// Плавная прокрутка к цели. Каждый кадр список проходит долю оставшегося
/// пути: быстро в начале и мягко в конце. Новый щелчок колеса двигает цель,
/// а не начинает заново, поэтому частые щелчки сливаются в одно движение.
#[derive(Debug, Clone, Copy)]
pub struct Glide {
    pos: f32,
    target: f32,
    last: Option<Instant>,
}

impl Glide {
    pub fn new(from: f32, target: f32) -> Self {
        Self {
            pos: from,
            target,
            last: None,
        }
    }

    pub fn target(&self) -> f32 {
        self.target
    }

    pub fn retarget(&mut self, target: f32) {
        self.target = target;
    }

    /// Шаг на кадр `now`: новое положение и доехал ли список.
    pub fn step(&mut self, now: Instant) -> (f32, bool) {
        let dt = self
            .last
            .map_or(1.0 / 60.0, |last| {
                now.saturating_duration_since(last).as_secs_f32()
            })
            .min(0.05);
        self.last = Some(now);
        let share = 1.0 - (-dt / GLIDE_TAU).exp();
        self.pos += (self.target - self.pos) * share;
        let done = (self.target - self.pos).abs() < GLIDE_DONE || !self.pos.is_finite();
        if done {
            self.pos = self.target;
        }
        (self.pos, done)
    }
}

// ---------------------------------------------------------------- колесо

/// Щелчки колеса над `content` уходят в `on_lines` (вниз — положительные),
/// остальные события — как обычно.
pub fn wheel<'a, Message: 'a>(
    content: impl Into<Element<'a, Message>>,
    on_lines: impl Fn(f32) -> Message + 'a,
) -> Element<'a, Message> {
    Element::new(Wheel {
        content: content.into(),
        on_lines: Box::new(on_lines),
    })
}

struct Wheel<'a, Message> {
    content: Element<'a, Message>,
    on_lines: Box<dyn Fn(f32) -> Message + 'a>,
}

impl<Message> Widget<Message, Theme, iced::Renderer> for Wheel<'_, Message> {
    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        match tree.children.first_mut() {
            Some(child) => self.content.as_widget_mut().layout(child, renderer, limits),
            None => layout::Node::new(Size::ZERO),
        }
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        if let Event::Mouse(mouse::Event::WheelScrolled {
            delta: mouse::ScrollDelta::Lines { y, .. },
        }) = event
            && *y != 0.0
            && cursor.is_over(layout.bounds())
        {
            shell.publish((self.on_lines)(-*y));
            shell.capture_event();
            return;
        }
        if let Some(child) = tree.children.first_mut() {
            self.content.as_widget_mut().update(
                child, event, layout, cursor, renderer, clipboard, shell, viewport,
            );
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        if let Some(child) = tree.children.first() {
            self.content
                .as_widget()
                .draw(child, renderer, theme, style, layout, cursor, viewport);
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        tree.children
            .first()
            .map_or(mouse::Interaction::None, |child| {
                self.content
                    .as_widget()
                    .mouse_interaction(child, layout, cursor, viewport, renderer)
            })
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        if let Some(child) = tree.children.first_mut() {
            self.content
                .as_widget_mut()
                .operate(child, layout, renderer, operation);
        }
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, iced::Renderer>> {
        let child = tree.children.first_mut()?;
        self.content
            .as_widget_mut()
            .overlay(child, layout, renderer, viewport, translation)
    }
}

// ---------------------------------------------------------------- плавное колесо

/// Остаток пути колеса, который ещё предстоит отдать вложенному виджету.
/// Тот же `Glide`, что у списка раздела: цель растёт с каждым щелчком, а
/// отданное уже запоминается, чтобы выдавать только приращения.
#[derive(Debug, Default)]
struct Remainder {
    glide: Option<Glide>,
    /// Сколько пути уже отдано.
    sent: f32,
}

impl Remainder {
    /// Щелчок колеса: добавить `distance` пикселей к цели (вниз — плюс).
    fn push(&mut self, distance: f32) {
        let glide = self.glide.get_or_insert_with(|| {
            self.sent = 0.0;
            Glide::new(0.0, 0.0)
        });
        glide.retarget(glide.target() + distance);
    }

    /// Шаг на кадр `now`: сколько пикселей отдать и остался ли путь.
    fn advance(&mut self, now: Instant) -> Option<(f32, bool)> {
        let glide = self.glide.as_mut()?;
        let (pos, done) = glide.step(now);
        let step = pos - self.sent;
        self.sent = pos;
        if done {
            self.glide = None;
        }
        Some((step, !done))
    }

    fn clear(&mut self) {
        self.glide = None;
    }
}

/// Колесо мыши над `content` (обычно `scrollable`) крутит плавно: щелчки
/// копятся, и содержимое доезжает до цели за несколько кадров. Тачпад и
/// остальные события уходят содержимому как есть. Сообщений приложению нет.
pub fn smooth<'a, Message: 'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    Element::new(Smooth {
        content: content.into(),
    })
}

struct Smooth<'a, Message> {
    content: Element<'a, Message>,
}

#[derive(Default)]
struct SmoothState {
    remainder: Remainder,
    /// Где был курсор на последнем щелчке: туда уходят кадры доводки.
    cursor: Point,
}

impl<Message> Widget<Message, Theme, iced::Renderer> for Smooth<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<SmoothState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(SmoothState::default())
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        match tree.children.first_mut() {
            Some(child) => self.content.as_widget_mut().layout(child, renderer, limits),
            None => layout::Node::new(Size::ZERO),
        }
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<SmoothState>();
        match event {
            Event::Mouse(mouse::Event::WheelScrolled {
                delta: mouse::ScrollDelta::Lines { x, y },
            }) if *x == 0.0 && *y != 0.0 => {
                if let Some(position) = cursor.position_over(layout.bounds()) {
                    state.cursor = position;
                    state.remainder.push(-*y * WHEEL_STEP);
                    shell.capture_event();
                    shell.request_redraw();
                    return;
                }
            }
            // Тачпад: пользователь взял прокрутку в свои руки.
            Event::Mouse(mouse::Event::WheelScrolled {
                delta: mouse::ScrollDelta::Pixels { .. },
            }) if cursor.is_over(layout.bounds()) => state.remainder.clear(),
            _ => {}
        }
        let Some(child) = tree.children.first_mut() else {
            return;
        };
        self.content.as_widget_mut().update(
            child, event, layout, cursor, renderer, clipboard, shell, viewport,
        );
        if let Event::Window(window::Event::RedrawRequested(now)) = event {
            let state = tree.state.downcast_mut::<SmoothState>();
            let Some((step, more)) = state.remainder.advance(*now) else {
                return;
            };
            if step != 0.0 {
                // iced применяет пиксельную прокрутку как есть: сдвиг = −y.
                let wheel = Event::Mouse(mouse::Event::WheelScrolled {
                    delta: mouse::ScrollDelta::Pixels { x: 0.0, y: -step },
                });
                let at = mouse::Cursor::Available(state.cursor);
                if let Some(child) = tree.children.first_mut() {
                    self.content.as_widget_mut().update(
                        child, &wheel, layout, at, renderer, clipboard, shell, viewport,
                    );
                }
            }
            if more {
                shell.request_redraw();
            }
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        if let Some(child) = tree.children.first() {
            self.content
                .as_widget()
                .draw(child, renderer, theme, style, layout, cursor, viewport);
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        tree.children
            .first()
            .map_or(mouse::Interaction::None, |child| {
                self.content
                    .as_widget()
                    .mouse_interaction(child, layout, cursor, viewport, renderer)
            })
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        if let Some(child) = tree.children.first_mut() {
            self.content
                .as_widget_mut()
                .operate(child, layout, renderer, operation);
        }
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, iced::Renderer>> {
        let child = tree.children.get_mut(0)?;
        self.content
            .as_widget_mut()
            .overlay(child, layout, renderer, viewport, translation)
    }
}

// ---------------------------------------------------------------- полоса

/// Полоса прокрутки во всю высоту для списка высотой `content`, сдвинутого
/// на `offset`. Видимая часть — высота самой полосы. Тянут ручку — в
/// `on_drag` приходит новый сдвиг.
pub fn rail<'a, Message: 'a>(
    offset: f32,
    content: f32,
    on_drag: impl Fn(f32) -> Message + 'a,
) -> Element<'a, Message> {
    Element::new(Rail {
        offset,
        content,
        on_drag: Box::new(on_drag),
    })
}

struct Rail<'a, Message> {
    offset: f32,
    content: f32,
    on_drag: Box<dyn Fn(f32) -> Message + 'a>,
}

#[derive(Default)]
struct RailState {
    /// Ручку держат: где в ней схватили, от её верха.
    grab: Option<f32>,
    hovered: bool,
}

/// Ручка на полосе: верх и длина. `None` — список весь виден.
fn thumb(track: f32, viewport: f32, content: f32, offset: f32) -> Option<(f32, f32)> {
    if content <= viewport + 1.0 || track <= 0.0 {
        return None;
    }
    let length = (track * viewport / content).clamp(THUMB_MIN.min(track), track);
    let room = track - length;
    let max_offset = content - viewport;
    let top = room * (offset / max_offset).clamp(0.0, 1.0);
    Some((top, length))
}

/// Сдвиг списка, при котором верх ручки стоит на `top`.
fn offset_for(top: f32, track: f32, viewport: f32, content: f32) -> f32 {
    let Some((_, length)) = thumb(track, viewport, content, 0.0) else {
        return 0.0;
    };
    let room = track - length;
    if room <= 0.0 {
        return 0.0;
    }
    (top / room).clamp(0.0, 1.0) * (content - viewport)
}

impl<Message> Rail<'_, Message> {
    /// Полоса без отступов: где по вертикали может ходить ручка.
    fn track(bounds: Rectangle) -> (f32, f32) {
        (
            bounds.y + RAIL_PAD,
            (bounds.height - 2.0 * RAIL_PAD).max(0.0),
        )
    }
}

impl<Message> Widget<Message, Theme, iced::Renderer> for Rail<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<RailState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(RailState::default())
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(RAIL_WIDTH), Length::Fill)
    }

    fn layout(
        &mut self,
        _tree: &mut Tree,
        _renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(limits.width(RAIL_WIDTH).max())
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
        let state = tree.state.downcast_mut::<RailState>();
        let bounds = layout.bounds();
        let (track_y, track) = Self::track(bounds);
        let viewport = bounds.height;
        let Some((top, length)) = thumb(track, viewport, self.content, self.offset) else {
            state.grab = None;
            return;
        };
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let Some(point) = cursor.position_over(bounds) else {
                    return;
                };
                let local = point.y - track_y;
                let grab = if (top..=top + length).contains(&local) {
                    local - top
                } else {
                    // Мимо ручки: ручка встаёт серединой под курсор.
                    let grab = length / 2.0;
                    let offset = offset_for(local - grab, track, viewport, self.content);
                    shell.publish((self.on_drag)(offset));
                    grab
                };
                state.grab = Some(grab);
                shell.capture_event();
                shell.request_redraw();
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                if let Some(grab) = state.grab {
                    let local = position.y - track_y - grab;
                    let offset = offset_for(local, track, viewport, self.content);
                    shell.publish((self.on_drag)(offset));
                    shell.capture_event();
                }
                let over = cursor.is_over(bounds);
                if over != state.hovered {
                    state.hovered = over;
                    shell.request_redraw();
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
                if state.grab.is_some() =>
            {
                state.grab = None;
                shell.request_redraw();
            }
            Event::Mouse(mouse::Event::CursorLeft) if state.hovered => {
                state.hovered = false;
                shell.request_redraw();
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
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<RailState>();
        let bounds = layout.bounds();
        let (track_y, track) = Self::track(bounds);
        let Some((top, length)) = thumb(track, bounds.height, self.content, self.offset) else {
            return;
        };
        let active = state.grab.is_some() || state.hovered;
        let width = if active {
            THUMB_WIDTH_ACTIVE
        } else {
            THUMB_WIDTH
        };
        let color: Color = match (state.grab.is_some(), state.hovered) {
            (true, _) => theme::text_secondary(),
            (false, true) => theme::text_tertiary(),
            (false, false) => theme::text_disabled(),
        };
        renderer.fill_quad(
            Quad {
                bounds: Rectangle {
                    x: bounds.x + (bounds.width - width) / 2.0,
                    y: track_y + top,
                    width,
                    height: length,
                },
                border: Border::default().rounded(width / 2.0),
                ..Quad::default()
            },
            color,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let state = tree.state.downcast_ref::<RailState>();
        let bounds = layout.bounds();
        let shown = thumb(
            Self::track(bounds).1,
            bounds.height,
            self.content,
            self.offset,
        )
        .is_some();
        if shown && (state.grab.is_some() || cursor.is_over(bounds)) {
            mouse::Interaction::Grab
        } else {
            mouse::Interaction::None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn glide_eases_to_the_target() {
        let start = Instant::now();
        let mut glide = Glide::new(0.0, 300.0);
        let (first, done) = glide.step(start);
        assert!(!done && first > 0.0 && first < 100.0, "{first}");
        let mut last = first;
        let mut frames = 0;
        loop {
            frames += 1;
            let (pos, done) = glide.step(start + Duration::from_millis(16 * frames));
            assert!(pos >= last - 1e-3, "назад: {pos} < {last}");
            last = pos;
            if done {
                break;
            }
            assert!(frames < 200, "не доехал");
        }
        assert_eq!(last, 300.0);
        // Примерно за полсекунды.
        assert!((15..45).contains(&frames), "{frames}");
        // Новая цель — продолжение, а не рывок.
        let mut glide = Glide::new(0.0, 100.0);
        let (pos, _) = glide.step(start);
        glide.retarget(200.0);
        let (next, _) = glide.step(start + Duration::from_millis(16));
        assert!(next > pos && next < 200.0);
    }

    #[test]
    fn remainder_delivers_the_whole_path_without_overshoot() {
        let start = Instant::now();
        let mut rest = Remainder::default();
        rest.push(WHEEL_STEP);
        let mut total = 0.0;
        let mut frames = 0;
        loop {
            let (step, more) = rest
                .advance(start + Duration::from_millis(16 * frames))
                .unwrap();
            assert!(step >= 0.0, "назад: {step}");
            total += step;
            assert!(total <= WHEEL_STEP + 1e-3, "проскочил: {total}");
            frames += 1;
            if !more {
                break;
            }
            assert!(frames < 200, "не доехал");
        }
        assert!((total - WHEEL_STEP).abs() < 1e-3, "{total}");
        // За полсекунды-секунду, но не мгновенно.
        assert!((15..60).contains(&frames), "{frames}");
        // Путь пройден: кадры ничего не отдают.
        assert!(rest.advance(start + Duration::from_secs(5)).is_none());
    }

    #[test]
    fn remainder_sums_clicks_and_clears() {
        let start = Instant::now();
        let mut rest = Remainder::default();
        rest.push(WHEEL_STEP);
        let (first, _) = rest.advance(start).unwrap();
        // Второй щелчок в ту же сторону и третий назад: цель — сумма.
        rest.push(WHEEL_STEP);
        rest.push(-WHEEL_STEP / 2.0);
        let mut total = first;
        for frame in 1..300 {
            let Some((step, more)) = rest.advance(start + Duration::from_millis(16 * frame)) else {
                break;
            };
            total += step;
            if !more {
                break;
            }
        }
        assert!((total - 1.5 * WHEEL_STEP).abs() < 1e-3, "{total}");
        // Тачпад сбрасывает остаток.
        rest.push(WHEEL_STEP);
        rest.clear();
        assert!(rest.advance(start).is_none());
        // Новый путь после сброса начинается с нуля.
        rest.push(-30.0);
        let (step, _) = rest.advance(start).unwrap();
        assert!(step < 0.0 && step > -30.0);
    }

    #[test]
    fn thumb_has_a_minimum_and_follows_offset() {
        // Длинный список: ручка не короче минимума.
        let (top, length) = thumb(600.0, 608.0, 54_000.0, 0.0).unwrap();
        assert_eq!((top, length), (0.0, THUMB_MIN));
        let (top, _) = thumb(600.0, 608.0, 54_000.0, 54_000.0 - 608.0).unwrap();
        assert_eq!(top, 600.0 - THUMB_MIN);
        // Сдвиг из положения ручки и обратно.
        let offset = offset_for(280.0, 600.0, 608.0, 54_000.0);
        let (top, _) = thumb(600.0, 608.0, 54_000.0, offset).unwrap();
        assert!((top - 280.0).abs() < 1e-2);
        assert_eq!(offset_for(-50.0, 600.0, 608.0, 54_000.0), 0.0);
        assert_eq!(offset_for(1e9, 600.0, 608.0, 54_000.0), 54_000.0 - 608.0);
        // Всё видно — ручки нет; странные числа не ломают.
        assert!(thumb(600.0, 608.0, 500.0, 0.0).is_none());
        assert!(thumb(0.0, 608.0, 54_000.0, 0.0).is_none());
        assert_eq!(offset_for(10.0, 600.0, 608.0, 500.0), 0.0);
    }
}
