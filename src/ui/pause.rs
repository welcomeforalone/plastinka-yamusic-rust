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

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::widget::{self, Tree, Widget};
use iced::advanced::{Clipboard, Shell, overlay};
use iced::{Element, Event, Length, Rectangle, Size, Theme, Vector, mouse, window};

/// Обёртка: пока `paused`, дети не получают `RedrawRequested`.
pub fn pause<'a, Message: 'a>(
    content: impl Into<Element<'a, Message>>,
    paused: bool,
) -> Element<'a, Message> {
    Element::new(Pause {
        content: content.into(),
        paused,
    })
}

struct Pause<'a, Message> {
    content: Element<'a, Message>,
    paused: bool,
}

/// Нужно ли скрыть событие от детей.
fn swallows(paused: bool, event: &Event) -> bool {
    paused && matches!(event, Event::Window(window::Event::RedrawRequested(_)))
}

impl<Message> Widget<Message, Theme, iced::Renderer> for Pause<'_, Message> {
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
        if swallows(self.paused, event) {
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
        assert!(swallows(true, &redraw));
        assert!(!swallows(false, &redraw));
        assert!(!swallows(true, &resized));
        assert!(!swallows(true, &Event::Mouse(mouse::Event::CursorLeft)));
    }
}
