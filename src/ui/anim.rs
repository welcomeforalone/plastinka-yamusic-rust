//! Анимации окна на `iced::Animation`.
//!
//! - [`hover_fill`] — фон, который плавно проявляется при наведении. Состояние
//!   анимации живёт в дереве виджетов, поэтому у каждой строки и кнопки своё.
//!   Кадры виджет просит сам и только пока идёт переход: в простое окно не
//!   перерисовывается.
//! - [`motion`] — сдвиг, масштаб и «вуаль» (полупрозрачный слой поверх) для
//!   появления экранов, диалогов, сообщений и полноэкранного плеера.
//!   Прозрачности целого поддерева в iced нет, поэтому появление из чёрного —
//!   это вуаль цвета фона, которая тает.
//!
//! Длительности — в `docs/ui/stage-6.5.md`.

use std::time::Duration;

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::{self, Quad, Renderer as _};
use iced::advanced::widget::{self, Tree, Widget, tree};
use iced::advanced::{Clipboard, Shell, overlay};
use iced::animation::{Animation, Easing};
use iced::time::Instant;
use iced::{
    Border, Color, Element, Event, Length, Rectangle, Size, Theme, Transformation, Vector, mouse,
    window,
};

use super::theme;

/// Смена цвета при наведении.
pub const HOVER: Duration = Duration::from_millis(120);
/// Появление раздела или экрана.
pub const PAGE: Duration = Duration::from_millis(200);
/// Меню, диалоги, сообщения.
pub const PANEL: Duration = Duration::from_millis(220);
/// Вход и выход полноэкранного плеера.
pub const FULLSCREEN: Duration = Duration::from_millis(320);
/// Перекрёстное затухание обложек.
pub const COVER: Duration = Duration::from_millis(260);

/// Переход «выкл → вкл», который уже начался в `now`.
pub fn entering(duration: Duration, now: Instant) -> Animation<bool> {
    Animation::new(false)
        .duration(duration)
        .easing(Easing::EaseOutCubic)
        .go(true, now)
}

/// Переход в покое: включён или выключен без анимации.
pub fn settled(value: bool, duration: Duration) -> Animation<bool> {
    Animation::new(value)
        .duration(duration)
        .easing(Easing::EaseOutCubic)
}

/// Доля перехода: 0 — выключено, 1 — включено.
pub fn progress(animation: &Animation<bool>, now: Instant) -> f32 {
    animation.interpolate(0.0, 1.0, now)
}

// ---------------------------------------------------------------- наведение

/// Фон `idle`, при наведении плавно переходит в `hovered`.
pub fn hover_fill<'a, Message>(
    content: impl Into<Element<'a, Message>>,
    idle: Color,
    hovered: Color,
    radius: f32,
) -> HoverFill<'a, Message> {
    HoverFill {
        content: content.into(),
        idle,
        hovered,
        radius,
    }
}

pub struct HoverFill<'a, Message> {
    content: Element<'a, Message>,
    idle: Color,
    hovered: Color,
    radius: f32,
}

struct HoverState {
    hovered: bool,
    animation: Animation<bool>,
}

impl<Message> Widget<Message, Theme, iced::Renderer> for HoverFill<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<HoverState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(HoverState {
            hovered: false,
            animation: Animation::new(false)
                .duration(HOVER)
                .easing(Easing::EaseOut),
        })
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
        if let Some(child) = tree.children.first_mut() {
            self.content.as_widget_mut().update(
                child, event, layout, cursor, renderer, clipboard, shell, viewport,
            );
        }
        let state = tree.state.downcast_mut::<HoverState>();
        match event {
            Event::Window(window::Event::RedrawRequested(now)) => {
                if state.animation.is_animating(*now) {
                    shell.request_redraw();
                }
            }
            Event::Mouse(_) | Event::Window(_) | Event::Touch(_) => {
                let over = cursor.is_over(layout.bounds());
                if over != state.hovered {
                    state.hovered = over;
                    state.animation.go_mut(over, Instant::now());
                    shell.request_redraw();
                }
            }
            _ => {}
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
        let state = tree.state.downcast_ref::<HoverState>();
        let t = progress(&state.animation, Instant::now());
        let color = theme::mix(self.idle, self.hovered, t);
        if color.a > 0.0 {
            renderer.fill_quad(
                Quad {
                    bounds: layout.bounds(),
                    border: Border::default().rounded(self.radius),
                    ..Quad::default()
                },
                color,
            );
        }
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

impl<'a, Message: 'a> From<HoverFill<'a, Message>> for Element<'a, Message> {
    fn from(fill: HoverFill<'a, Message>) -> Self {
        Element::new(fill)
    }
}

// ---------------------------------------------------------------- движение

/// Содержимое со сдвигом `offset` и масштабом `scale` относительно центра.
/// События идут как обычно: переходы короткие, и в покое сдвига нет.
pub fn motion<'a, Message>(content: impl Into<Element<'a, Message>>) -> Motion<'a, Message> {
    Motion {
        content: content.into(),
        offset: Vector::ZERO,
        scale: 1.0,
        veil: None,
    }
}

pub struct Motion<'a, Message> {
    content: Element<'a, Message>,
    offset: Vector,
    scale: f32,
    veil: Option<(Color, f32)>,
}

impl<Message> Motion<'_, Message> {
    pub fn offset(mut self, x: f32, y: f32) -> Self {
        self.offset = Vector::new(x, y);
        self
    }

    pub fn scale(mut self, scale: f32) -> Self {
        self.scale = scale;
        self
    }

    /// Слой цвета `color` поверх содержимого (со скруглением `radius`).
    /// Прозрачный — не рисуется.
    pub fn veil(mut self, color: Color, radius: f32) -> Self {
        self.veil = (color.a > 0.0).then_some((color, radius));
        self
    }

    fn transformation(&self, bounds: Rectangle) -> Option<Transformation> {
        let still = self.offset == Vector::ZERO && (self.scale - 1.0).abs() < f32::EPSILON;
        if still || !self.scale.is_finite() || self.scale <= 0.0 {
            return None;
        }
        let center = bounds.center();
        Some(
            Transformation::translate(center.x + self.offset.x, center.y + self.offset.y)
                * Transformation::scale(self.scale)
                * Transformation::translate(-center.x, -center.y),
        )
    }
}

impl<Message> Widget<Message, Theme, iced::Renderer> for Motion<'_, Message> {
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
        self.content.as_widget_mut().layout(tree, renderer, limits)
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
        self.content.as_widget_mut().update(
            tree, event, layout, cursor, renderer, clipboard, shell, viewport,
        );
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
        let bounds = layout.bounds();
        let draw = |renderer: &mut iced::Renderer, viewport: &Rectangle| {
            self.content
                .as_widget()
                .draw(tree, renderer, theme, style, layout, cursor, viewport);
            if let Some((color, radius)) = self.veil {
                renderer.fill_quad(
                    Quad {
                        bounds,
                        border: Border::default().rounded(radius),
                        ..Quad::default()
                    },
                    color,
                );
            }
        };
        match self.transformation(bounds) {
            Some(transformation) => {
                // Сдвинутое содержимое не должно отсекаться по старому месту.
                let wide = viewport.expand(bounds.width.max(bounds.height));
                renderer.with_transformation(transformation, |renderer| draw(renderer, &wide));
            }
            None => draw(renderer, viewport),
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
        self.content
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(tree, layout, renderer, operation);
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

impl<'a, Message: 'a> From<Motion<'a, Message>> for Element<'a, Message> {
    fn from(motion: Motion<'a, Message>) -> Self {
        Element::new(motion)
    }
}

// ---------------------------------------------------------------- подмена

/// Под курсором — `hovered`, иначе `idle`. В отличие от `hover` из iced,
/// рисуется только один вариант: `hover` кладёт верхний слой поверх
/// нижнего, и номер трека просвечивал из-под кнопки Play.
pub fn swap<'a, Message: 'a>(
    idle: impl Into<Element<'a, Message>>,
    hovered: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    Element::new(Swap {
        idle: idle.into(),
        hovered: hovered.into(),
    })
}

struct Swap<'a, Message> {
    idle: Element<'a, Message>,
    hovered: Element<'a, Message>,
}

#[derive(Default)]
struct SwapState {
    hovered: bool,
}

impl<'a, Message> Swap<'a, Message> {
    /// Вариант для состояния `hovered` и его номер среди детей.
    fn pick(&self, hovered: bool) -> (&Element<'a, Message>, usize) {
        if hovered {
            (&self.hovered, 1)
        } else {
            (&self.idle, 0)
        }
    }
}

impl<Message> Widget<Message, Theme, iced::Renderer> for Swap<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<SwapState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(SwapState::default())
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.idle), Tree::new(&self.hovered)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&[&self.idle, &self.hovered]);
    }

    fn size(&self) -> Size<Length> {
        self.idle.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.idle.as_widget().size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let mut trees = tree.children.iter_mut();
        let (Some(idle_tree), Some(hovered_tree)) = (trees.next(), trees.next()) else {
            return layout::Node::new(Size::ZERO);
        };
        let idle = self
            .idle
            .as_widget_mut()
            .layout(idle_tree, renderer, limits);
        let hovered = self
            .hovered
            .as_widget_mut()
            .layout(hovered_tree, renderer, limits);
        layout::Node::with_children(idle.size(), vec![idle, hovered])
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
        let over = cursor.is_over(layout.bounds());
        let state = tree.state.downcast_mut::<SwapState>();
        if over != state.hovered {
            state.hovered = over;
            shell.request_redraw();
        }
        // События получает только видимый вариант.
        let index = usize::from(over);
        let content = if over {
            &mut self.hovered
        } else {
            &mut self.idle
        };
        if let (Some(child), Some(child_layout)) =
            (tree.children.get_mut(index), layout.children().nth(index))
        {
            content.as_widget_mut().update(
                child,
                event,
                child_layout,
                cursor,
                renderer,
                clipboard,
                shell,
                viewport,
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
        let (content, index) = self.pick(cursor.is_over(layout.bounds()));
        if let (Some(child), Some(child_layout)) =
            (tree.children.get(index), layout.children().nth(index))
        {
            content.as_widget().draw(
                child,
                renderer,
                theme,
                style,
                child_layout,
                cursor,
                viewport,
            );
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
        let (content, index) = self.pick(cursor.is_over(layout.bounds()));
        match (tree.children.get(index), layout.children().nth(index)) {
            (Some(child), Some(child_layout)) => content.as_widget().mouse_interaction(
                child,
                child_layout,
                cursor,
                viewport,
                renderer,
            ),
            _ => mouse::Interaction::None,
        }
    }
}

// ---------------------------------------------------------------- столбики

/// Сколько раз в секунду перерисовываются столбики играющего трека.
const EQUALIZER_FPS: f32 = 30.0;

/// Три прыгающих столбика у играющего трека. Пока виден, сам просит кадры
/// 30 раз в секунду; `moving` — `false` на паузе или пока звук ждёт сеть:
/// тогда столбики стоят.
pub fn equalizer<'a, Message: 'a>(color: Color, moving: bool) -> Element<'a, Message> {
    Element::new(Equalizer { color, moving })
}

struct Equalizer {
    color: Color,
    moving: bool,
}

/// Время, с которого считаются колебания: общее у всех столбиков, чтобы
/// после перестройки строки они не прыгали.
fn equalizer_clock(now: Instant) -> f32 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let start = *START.get_or_init(|| now);
    now.saturating_duration_since(start).as_secs_f32()
}

/// Высоты трёх столбиков (доли 0..1) в момент `t` секунд.
fn equalizer_heights(t: f32) -> [f32; 3] {
    // Разные частоты и сдвиги: столбики не ходят в ногу.
    // Модуль синуса: полный подъём и спад за π / speed секунд, 1–1,4 с.
    let bar = |speed: f32, phase: f32| 0.3 + 0.7 * (t * speed + phase).sin().abs();
    [bar(2.9, 0.0), bar(3.9, 1.1), bar(2.4, 2.3)]
}

impl<Message> Widget<Message, Theme, iced::Renderer> for Equalizer {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(13.0), Length::Fixed(16.0))
    }

    fn layout(
        &mut self,
        _tree: &mut Tree,
        _renderer: &iced::Renderer,
        _limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(Size::new(13.0, 16.0))
    }

    fn update(
        &mut self,
        _tree: &mut Tree,
        event: &Event,
        _layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        if self.moving
            && let Event::Window(window::Event::RedrawRequested(now)) = event
        {
            shell.request_redraw_at(*now + Duration::from_secs_f32(1.0 / EQUALIZER_FPS));
        }
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
        let heights = if self.moving {
            equalizer_heights(equalizer_clock(Instant::now()))
        } else {
            [0.6, 1.0, 0.45]
        };
        for (index, share) in heights.into_iter().enumerate() {
            let height = (bounds.height * share).max(2.0);
            renderer.fill_quad(
                Quad {
                    bounds: Rectangle {
                        x: bounds.x + index as f32 * 5.0,
                        y: bounds.y + bounds.height - height,
                        width: 3.0,
                        height,
                    },
                    border: Border::default().rounded(1.5),
                    ..Quad::default()
                },
                self.color,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entering_runs_from_zero_to_one() {
        let start = Instant::now();
        let animation = entering(PAGE, start);
        assert!(progress(&animation, start) < 0.01);
        assert!(animation.is_animating(start));
        let end = start + PAGE + Duration::from_millis(5);
        assert!((progress(&animation, end) - 1.0).abs() < 1e-6);
        assert!(!animation.is_animating(end));
    }

    #[test]
    fn settled_does_not_animate() {
        let now = Instant::now();
        let animation = settled(true, PANEL);
        assert!(!animation.is_animating(now));
        assert!((progress(&animation, now) - 1.0).abs() < 1e-6);
    }
}
