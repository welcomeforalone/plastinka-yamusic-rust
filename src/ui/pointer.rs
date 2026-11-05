//! Где нажали мышь: меню трека открывается у курсора.
//!
//! Внутри `scrollable` координаты курсора считаются от начала списка, а
//! меню рисуется поверх всего окна. Поэтому место нажатия берёт обёртка
//! вокруг всего окна и сообщает его раньше, чем нажатие дойдёт до строки:
//! сообщение о месте приходит первым, следом — «открыть меню».

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::widget::{self, Tree, Widget};
use iced::advanced::{Clipboard, Shell, overlay};
use iced::{Element, Event, Length, Point, Rectangle, Size, Theme, Vector, mouse};

/// Обёртка: при нажатии левой или правой кнопки сообщает `on_press(место)`.
pub fn pointer<'a, Message: 'a>(
    content: impl Into<Element<'a, Message>>,
    on_press: impl Fn(Point) -> Message + 'a,
) -> Element<'a, Message> {
    Element::new(Pointer {
        content: content.into(),
        on_press: Box::new(on_press),
    })
}

struct Pointer<'a, Message> {
    content: Element<'a, Message>,
    on_press: Box<dyn Fn(Point) -> Message + 'a>,
}

impl<Message> Widget<Message, Theme, iced::Renderer> for Pointer<'_, Message> {
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
        // Сначала место, потом само нажатие: так окно знает, где открыть меню.
        if let Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left | mouse::Button::Right)) =
            event
            && let Some(position) = cursor.position()
        {
            shell.publish((self.on_press)(position));
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
