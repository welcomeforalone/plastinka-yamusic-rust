//! Пульс кадров: невидимый виджет, который гонит анимации из самой
//! перерисовки.
//!
//! Зачем. Пока пользователь тянет окно за заголовок или за край, Windows
//! крутит собственный (модальный) цикл, и главный цикл winit стоит: таймеры и
//! подписки ничего не обновляют, спектр под плеером замирает. Единственное,
//! что в этом цикле продолжает приходить, — перерисовка окна по `WM_PAINT`
//! (`RedrawRequested`). Сообщение, которое виджет публикует прямо во время
//! перерисовки, iced обрабатывает тут же. Поэтому пока нужны кадры, виджет на
//! каждой перерисовке просит следующую и (не чаще [`PERIOD`], если
//! `smooth` выключен) публикует сообщение кадра. Когда кадры не нужны, он
//! ничего не просит, и окно в простое не перерисовывается.

use std::time::Duration;

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::widget::{Tree, Widget, tree};
use iced::advanced::{Clipboard, Shell};
use iced::time::Instant;
use iced::{Element, Event, Length, Rectangle, Size, Theme, mouse, window};

/// Как часто публиковать кадр, если не нужна плавность по частоте экрана:
/// спиннерам и спектру хватает 30 в секунду.
const PERIOD: Duration = Duration::from_millis(30);

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

/// Пора ли публиковать кадр.
fn due(last: Option<Instant>, now: Instant, smooth: bool) -> bool {
    smooth || last.is_none_or(|last| now.saturating_duration_since(last) >= PERIOD)
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
        let Event::Window(window::Event::RedrawRequested(now)) = event else {
            return;
        };
        let state = tree.state.downcast_mut::<State>();
        if !self.active {
            state.last = None;
            return;
        }
        if due(state.last, *now, self.smooth) {
            state.last = Some(*now);
            shell.publish((self.on_frame)(*now));
        }
        shell.request_redraw();
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
}
