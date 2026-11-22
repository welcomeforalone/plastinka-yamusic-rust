//! Область с тёмными цветами при любой теме окна.
//!
//! Размытая затемнённая обложка (полноэкранный плеер) и фон мини-плеера
//! всегда тёмные, поэтому тексты и кнопки на них берут тёмный тон текущего
//! семейства, даже если окно светлое. Цвета ролей (`theme::text_primary()` и
//! другие) читаются и при сборке вида, и позже, в замыканиях стилей, которые
//! вызываются при отрисовке. Поэтому [`scope`] включает тёмный тон на всё
//! время работы поддерева: при его раскладке, событиях и рисовании.

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::widget::{self, Tree, Widget, tree};
use iced::advanced::{Clipboard, Shell, overlay};
use iced::{Element, Event, Length, Rectangle, Size, Theme, Vector, mouse};

use super::theme;

/// `content` в тёмном тоне. Сам `content` тоже нужно собирать внутри
/// [`theme::dark_scope`]: цвета в нём читаются при сборке.
pub fn scope<'a, Message>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message>
where
    Message: 'a,
{
    // Тема iced для встроенных стилей виджетов (поля ввода, подсказки).
    let theme = theme::dark_scope(theme::theme);
    Element::new(Dark {
        content: content.into(),
        theme,
    })
}

struct Dark<'a, Message> {
    content: Element<'a, Message>,
    theme: Theme,
}

impl<Message> Widget<Message, Theme, iced::Renderer> for Dark<'_, Message> {
    fn tag(&self) -> tree::Tag {
        self.content.as_widget().tag()
    }

    fn state(&self) -> tree::State {
        self.content.as_widget().state()
    }

    fn children(&self) -> Vec<Tree> {
        self.content.as_widget().children()
    }

    fn diff(&self, tree: &mut Tree) {
        self.content.as_widget().diff(tree);
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
        theme::dark_scope(|| self.content.as_widget_mut().layout(tree, renderer, limits))
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
        theme::dark_scope(|| {
            self.content.as_widget_mut().update(
                tree, event, layout, cursor, renderer, clipboard, shell, viewport,
            );
        });
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        _theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        theme::dark_scope(|| {
            self.content.as_widget().draw(
                tree,
                renderer,
                &self.theme,
                style,
                layout,
                cursor,
                viewport,
            );
        });
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        theme::dark_scope(|| {
            self.content
                .as_widget()
                .mouse_interaction(tree, layout, cursor, viewport, renderer)
        })
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        theme::dark_scope(|| {
            self.content
                .as_widget_mut()
                .operate(tree, layout, renderer, operation);
        });
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, iced::Renderer>> {
        self.content
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation)
    }
}
