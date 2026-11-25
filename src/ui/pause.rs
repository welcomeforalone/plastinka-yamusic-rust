//! Пауза перерисовки для свёрнутого окна.
//!
//! Зачем. У окна без системной рамки (`decorations: false`) в свёрнутом
//! виде клиентская область не 0×0, а, например, 159×27. iced пропускает
//! отрисовку только при нулевом размере, поэтому свёрнутое окно продолжало
//! бы перерисовываться: спектр, спиннеры, бегущая строка и анимации сами
//! просят следующую перерисовку на каждом `RedrawRequested`. Обёртка пока
//! `paused` не передаёт детям это событие, и никто не просит следующий кадр.
//! Всё остальное (остальные события, раскладка, состояние, оверлеи) идёт
//! содержимому как обычно, поэтому прокрутка и фокус не теряются.
//!
//! Та же обёртка работает и для окна без фокуса ([`idle`]): декоративные
//! виджеты (столбики «играет», бегущая строка, курсор поля ввода) сами просят
//! кадр на каждой перерисовке и без неё бы рисовали окно бесконечно. Но
//! пока пользователь водит мышью по окну без фокуса, отклик на наведение
//! нужен, поэтому после последнего ввода перерисовки идут ещё [`GRACE`].

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::widget::tree;
use iced::advanced::widget::{self, Tree, Widget};
use iced::advanced::{Clipboard, Shell, overlay};
use iced::time::Instant;
use iced::{Element, Event, Length, Rectangle, Size, Theme, Vector, mouse, window};

/// Сколько после последнего ввода (мышь, касание, клавиатура) окно без фокуса
/// ещё перерисовывается как обычно: хватает на плавное наведение.
const GRACE: std::time::Duration = std::time::Duration::from_millis(400);

/// Обёртка: пока `paused`, дети не получают `RedrawRequested`.
pub fn pause<'a, Message: 'a>(
    content: impl Into<Element<'a, Message>>,
    paused: bool,
) -> Element<'a, Message> {
    Element::new(Pause {
        content: content.into(),
        paused,
        grace: false,
    })
}

/// Обёртка для окна без фокуса: пока `idle`, дети не получают
/// `RedrawRequested`, кроме [`GRACE`] после ввода. В счётчик кадров не входит.
pub fn idle<'a, Message: 'a>(
    content: impl Into<Element<'a, Message>>,
    idle: bool,
) -> Element<'a, Message> {
    Element::new(Pause {
        content: content.into(),
        paused: idle,
        grace: true,
    })
}

struct Pause<'a, Message> {
    content: Element<'a, Message>,
    paused: bool,
    /// Окно без фокуса: после недавнего ввода перерисовки не прячем.
    grace: bool,
}

/// Состояние обёртки: когда в последний раз был ввод.
#[derive(Default)]
struct State {
    last_input: Option<Instant>,
}

/// Нужно ли скрыть событие от детей. `recent` — ввод был не раньше [`GRACE`]
/// назад (учитывается, только если `grace`).
fn swallows(paused: bool, grace: bool, recent: bool, event: &Event) -> bool {
    paused
        && !(grace && recent)
        && matches!(event, Event::Window(window::Event::RedrawRequested(_)))
}

/// Ввод, после которого окно без фокуса перерисовывается как обычно.
fn is_input(event: &Event) -> bool {
    matches!(
        event,
        Event::Mouse(_) | Event::Touch(_) | Event::Keyboard(_)
    )
}

impl<Message> Widget<Message, Theme, iced::Renderer> for Pause<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
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
        #[cfg(debug_assertions)]
        if !self.grace && matches!(event, Event::Window(window::Event::RedrawRequested(_))) {
            super::fps::frame();
        }
        let state = tree.state.downcast_mut::<State>();
        if is_input(event) {
            state.last_input = Some(Instant::now());
        }
        let recent = state
            .last_input
            .is_some_and(|at| Instant::now().saturating_duration_since(at) < GRACE);
        if swallows(self.paused, self.grace, recent, event) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use iced::time::Instant;

    #[test]
    fn only_redraw_is_swallowed_while_paused() {
        let redraw = Event::Window(window::Event::RedrawRequested(Instant::now()));
        let resized = Event::Window(window::Event::Resized(Size::new(10.0, 10.0)));
        assert!(swallows(true, false, false, &redraw));
        assert!(!swallows(false, false, false, &redraw));
        assert!(!swallows(true, false, false, &resized));
        assert!(!swallows(
            true,
            false,
            false,
            &Event::Mouse(mouse::Event::CursorLeft)
        ));
    }

    #[test]
    fn idle_window_redraws_shortly_after_input() {
        let redraw = Event::Window(window::Event::RedrawRequested(Instant::now()));
        assert!(swallows(true, true, false, &redraw));
        assert!(!swallows(true, true, true, &redraw));
        // Свёрнутому окну ввод ничего не возвращает.
        assert!(swallows(true, false, true, &redraw));
        assert!(is_input(&Event::Mouse(mouse::Event::CursorLeft)));
        assert!(!is_input(&redraw));
    }
}
