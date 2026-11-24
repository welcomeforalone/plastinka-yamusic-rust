//! Пульс кадров: невидимый виджет, который гонит анимации из самой
//! перерисовки.
//!
//! На программном рендере виджет работает только пока окно тянут (см. выбор в
//! `App::subscription`): в остальное время кадры гонит подписка, иначе
//! каждый кадр давал бы две перерисовки (одну от самого виджета, вторую от
//! сообщения кадра).
//!
//! Зачем. Пока пользователь тянет окно за заголовок или за край, Windows
//! крутит собственный (модальный) цикл, и главный цикл winit стоит: таймеры и
//! подписки ничего не обновляют, спектр под плеером замирает. Единственное,
//! что в этом цикле продолжает приходить, — перерисовка окна по `WM_PAINT`
//! (`RedrawRequested`). Сообщение, которое виджет публикует прямо во время
//! перерисовки, iced обрабатывает тут же. Поэтому пока нужны кадры, виджет
//! просит следующую перерисовку и (не чаще [`PERIOD`], если `smooth`
//! выключен) публикует сообщение кадра. Когда кадры не нужны, он ничего не
//! просит, и окно в простое не перерисовывается.
//!
//! Цена кадра — перерисовка всего окна, поэтому следующая перерисовка просится
//! к сроку следующего кадра (`last + PERIOD`), а не на каждый vsync: иначе окно
//! рисуется с частотой монитора. Во время перетаскивания окна это важно
//! вдвойне: непрерывная просьба «перерисуй сейчас» превращается в цепочку
//! `WM_PAINT` на частоте монитора, и GPU грузится заметно сильнее. Поэтому
//! кадры во время перетаскивания ведут сами события `Moved`/`Resized` (iced
//! доставляет их и в модальном цикле Windows): на событие виджет просит одну
//! перерисовку, если пора публиковать кадр. Так перерисовок не больше
//! 30 в секунду, а спектр не замирает, пока окно движется. Если мышь с
//! зажатым окном стоит, событий нет: тогда перерисовку ставит в очередь
//! таймер потока из `drag_pulse` (в модальном цикле он срабатывает), а кадр
//! публикует этот виджет по своему сроку `PERIOD`.

use std::time::Duration;

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::widget::{Tree, Widget, tree};
use iced::advanced::{Clipboard, Shell};
use iced::time::Instant;
use iced::{Element, Event, Length, Rectangle, Size, Theme, mouse, window};

/// Как часто публиковать кадр, если не нужна плавность по частоте экрана:
/// спиннерам и спектру хватает 30 в секунду.
pub const PERIOD: Duration = Duration::from_millis(30);

/// Просит следующий кадр анимации. На видеокарте — на ближайшей перерисовке,
/// на программном рендере (tiny-skia) — не раньше чем через [`PERIOD`]: там
/// каждый кадр стоит процессора, и больше 30 в секунду не рисуем. Для
/// одиночных просьб на смену состояния (курсор вошёл или вышел) не нужна:
/// там хватает `shell.request_redraw()`.
pub fn next_frame<Message>(shell: &mut Shell<'_, Message>, now: Instant) {
    match frame_redraw(super::render::software(), now) {
        Redraw::Now => shell.request_redraw(),
        Redraw::At(at) => shell.request_redraw_at(window::RedrawRequest::At(at)),
    }
}

/// Через сколько ждать следующего кадра, если виджет хотел `wait`: на
/// программном рендере не меньше [`PERIOD`].
pub fn frame_wait(wait: Duration) -> Duration {
    throttled_wait(super::render::software(), wait)
}

/// Выбор срока кадра: видеокарта — сразу, программный рендер — через период.
fn frame_redraw(software: bool, now: Instant) -> Redraw {
    if software {
        Redraw::At(now + PERIOD)
    } else {
        Redraw::Now
    }
}

/// Ожидание до кадра с учётом рендера (см. [`frame_wait`]).
fn throttled_wait(software: bool, wait: Duration) -> Duration {
    if software { wait.max(PERIOD) } else { wait }
}

/// Невидимый виджет. `active` — кадры нужны; `smooth` — на каждой
/// перерисовке (переходы), а не раз в [`PERIOD`].
pub fn heartbeat<'a, Message: 'a>(
    active: bool,
    smooth: bool,
    on_frame: impl Fn(Instant) -> Message + 'a,
) -> Element<'a, Message> {
    Element::new(Heartbeat {
        active,
        smooth,
        on_frame: Box::new(on_frame),
    })
}

struct Heartbeat<'a, Message> {
    active: bool,
    smooth: bool,
    on_frame: Box<dyn Fn(Instant) -> Message + 'a>,
}

#[derive(Default)]
struct State {
    last: Option<Instant>,
}

/// Когда просить следующую перерисовку.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Redraw {
    /// Сразу, на ближайшем кадре.
    Now,
    /// Не раньше указанного срока.
    At(Instant),
}

/// Какая перерисовка нужна после кадра. `last` — момент последнего
/// опубликованного кадра.
fn next_redraw(last: Instant, smooth: bool) -> Redraw {
    if smooth {
        Redraw::Now
    } else {
        Redraw::At(last + PERIOD)
    }
}

/// Пора ли публиковать кадр.
fn due(last: Option<Instant>, now: Instant, smooth: bool) -> bool {
    smooth || last.is_none_or(|last| now.saturating_duration_since(last) >= PERIOD)
}

/// Просить ли перерисовку на событие перемещения или изменения размера окна.
/// При `smooth` перерисовки и так идут на каждом кадре.
fn redraw_on_motion(last: Option<Instant>, now: Instant, active: bool, smooth: bool) -> bool {
    active && !smooth && due(last, now, false)
}

impl<Message> Widget<Message, Theme, iced::Renderer> for Heartbeat<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn size(&self) -> Size<Length> {
        // Не нулевой размер: пустые элементы `stack!` отбрасывает.
        Size::new(Length::Shrink, Length::Shrink)
    }

    fn layout(
        &mut self,
        _tree: &mut Tree,
        _renderer: &iced::Renderer,
        _limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(Size::ZERO)
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        _layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<State>();
        // На программном рендере переходы тоже идут не чаще 30 кадров в секунду.
        let smooth = self.smooth && !super::render::software();
        let now = match event {
            Event::Window(window::Event::Moved(_) | window::Event::Resized(_)) => {
                if redraw_on_motion(state.last, Instant::now(), self.active, smooth) {
                    shell.request_redraw();
                }
                return;
            }
            Event::Window(window::Event::RedrawRequested(now)) => now,
            _ => return,
        };
        if !self.active {
            state.last = None;
            return;
        }
        if due(state.last, *now, smooth) {
            state.last = Some(*now);
            shell.publish((self.on_frame)(*now));
        }
        let last = state.last.unwrap_or(*now);
        match next_redraw(last, smooth) {
            Redraw::Now => shell.request_redraw(),
            Redraw::At(at) => shell.request_redraw_at(window::RedrawRequest::At(at)),
        }
    }

    fn draw(
        &self,
        _tree: &Tree,
        _renderer: &mut iced::Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        _layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_throttled_unless_smooth() {
        let start = Instant::now();
        assert!(due(None, start, false));
        assert!(!due(Some(start), start + Duration::from_millis(10), false));
        assert!(due(Some(start), start + PERIOD, false));
        assert!(due(Some(start), start + Duration::from_millis(1), true));
    }

    #[test]
    fn redraw_follows_frame_deadline_unless_smooth() {
        let last = Instant::now();
        assert_eq!(next_redraw(last, false), Redraw::At(last + PERIOD));
        assert_eq!(next_redraw(last, true), Redraw::Now);
    }

    #[test]
    fn software_render_caps_frames_at_period() {
        let now = Instant::now();
        assert_eq!(frame_redraw(false, now), Redraw::Now);
        assert_eq!(frame_redraw(true, now), Redraw::At(now + PERIOD));
        let frame = Duration::from_millis(16);
        assert_eq!(throttled_wait(false, frame), frame);
        assert_eq!(throttled_wait(true, frame), PERIOD);
        let long = Duration::from_millis(500);
        assert_eq!(throttled_wait(true, long), long);
    }

    #[test]
    fn motion_requests_redraw_only_when_frame_is_due() {
        let start = Instant::now();
        let soon = start + Duration::from_millis(10);
        let later = start + PERIOD;
        assert!(!redraw_on_motion(Some(start), soon, true, false));
        assert!(redraw_on_motion(Some(start), later, true, false));
        assert!(redraw_on_motion(None, start, true, false));
        assert!(!redraw_on_motion(Some(start), later, true, true));
        assert!(!redraw_on_motion(Some(start), later, false, false));
    }
}
