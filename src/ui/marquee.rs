//! Бегущая строка: текст, который не помещается по ширине, плавно уезжает
//! влево до конца и возвращается обратно, с паузами по краям. Короткий текст
//! стоит на месте. Так название трека в плеере всегда можно прочитать
//! целиком: многоточия в iced нет, а обрезанный текст теряет конец.

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::{self, Renderer as _};
use iced::advanced::text::{self, Renderer as _, paragraph::Plain};
use iced::advanced::widget::{Operation, Tree, Widget, tree};
use iced::advanced::{Clipboard, Shell};
use iced::alignment::Vertical;
use iced::time::Instant;
use iced::widget::text::{LineHeight, Shaping, Wrapping};
use iced::{
    Color, Element, Event, Length, Pixels, Point, Rectangle, Size, Theme, Vector, mouse, window,
};

use super::theme::Style;

/// Пауза у каждого края, секунды.
const PAUSE: f32 = 2.0;
/// Скорость прокрутки, пикселей в секунду.
const SPEED: f32 = 40.0;
/// Кадров в секунду во время прокрутки. Бегущей строке хватает 30: если
/// просить перерисовку на каждом кадре, всё окно рисуется с частотой монитора.
const SCROLL_FPS: f32 = 30.0;

type Paragraph = <iced::Renderer as text::Renderer>::Paragraph;

pub struct Marquee {
    content: String,
    style: Style,
    color: Color,
    /// Короткий текст стоит по центру, а не слева.
    centered: bool,
}

/// Однострочный текст в стиле `style`, длинный — бегущей строкой.
pub fn marquee(content: impl Into<String>, style: Style, color: Color) -> Marquee {
    Marquee {
        content: content.into(),
        style,
        color,
        centered: false,
    }
}

#[derive(Default)]
struct State {
    paragraph: Plain<Paragraph>,
    /// На сколько пикселей текст шире места. Ноль — помещается.
    overflow: f32,
    /// Когда началась прокрутка. Сбрасывается при смене текста или ширины.
    started: Option<Instant>,
    offset: f32,
}

impl Marquee {
    /// Если текст помещается, он стоит по центру.
    pub fn centered(mut self) -> Self {
        self.centered = true;
        self
    }

    fn text(&self, line: f32) -> text::Text<&str, iced::Font> {
        let (size, line_height, font) = self.style.metrics();
        text::Text {
            content: &self.content,
            bounds: Size::new(f32::INFINITY, line),
            size: Pixels(size),
            line_height: LineHeight::Absolute(Pixels(line_height)),
            font,
            align_x: text::Alignment::Left,
            align_y: Vertical::Top,
            shaping: Shaping::Advanced,
            wrapping: Wrapping::None,
        }
    }
}

impl<Message> Widget<Message, Theme, iced::Renderer> for Marquee {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Shrink)
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        _renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let state = tree.state.downcast_mut::<State>();
        let line = self.style.metrics().1;
        let size = limits.resolve(Length::Fill, Length::Fixed(line), Size::new(0.0, line));
        let changed = state.paragraph.update(self.text(line));
        let overflow = (state.paragraph.min_width() - size.width).max(0.0);
        if changed || (overflow - state.overflow).abs() > 0.5 {
            state.overflow = overflow;
            state.started = None;
            state.offset = 0.0;
        }
        layout::Node::new(size)
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
        if state.overflow <= 0.0 {
            state.offset = 0.0;
            return;
        }
        let started = *state.started.get_or_insert(*now);
        let elapsed = now.saturating_duration_since(started).as_secs_f32();
        let (offset, wait) = offset_at(elapsed, state.overflow);
        state.offset = offset;
        request_next(shell, *now, wait);
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
        let shift = if self.centered && state.overflow <= 0.0 {
            ((bounds.width - state.paragraph.min_width()) / 2.0).max(0.0)
        } else {
            0.0
        };
        renderer.with_layer(clip, |renderer| {
            renderer.fill_paragraph(
                state.paragraph.raw(),
                Point::new(bounds.x - state.offset + shift, bounds.y),
                self.color,
                clip,
            );
        });
    }
}

impl<'a, Message: 'a> From<Marquee> for Element<'a, Message> {
    fn from(marquee: Marquee) -> Self {
        Element::new(marquee)
    }
}

/// Просит следующую перерисовку. На паузе кадры не нужны: перерисовка
/// просится к концу паузы (`wait`), во время прокрутки — к следующему кадру.
fn request_next<Message>(shell: &mut Shell<'_, Message>, now: Instant, wait: Option<f32>) {
    let delay = match wait {
        Some(seconds) => seconds,
        None => 1.0 / SCROLL_FPS,
    };
    match std::time::Duration::try_from_secs_f32(delay)
        .ok()
        .and_then(|delay| now.checked_add(delay))
    {
        Some(at) => shell.request_redraw_at(window::RedrawRequest::At(at)),
        None => shell.request_redraw(),
    }
}

/// Часы прокрутки с паузой: пока курсор над строкой, время не идёт, а после
/// ухода продолжается с того же значения.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
struct Clock {
    /// Секунды прокрутки, накопленные до последней остановки.
    before: f32,
    /// С какого момента часы идут. `None` — стоят (или ещё не запускались).
    since: Option<Instant>,
}

impl Clock {
    /// Сколько секунд прокрутки прошло к моменту `now`.
    fn elapsed(&self, now: Instant) -> f32 {
        let running = self.since.map_or(0.0, |since| {
            now.saturating_duration_since(since).as_secs_f32()
        });
        self.before + running
    }

    /// Запускает часы, если они стоят.
    fn resume(&mut self, now: Instant) {
        self.since.get_or_insert(now);
    }

    /// Останавливает часы, запомнив пройденное время.
    fn pause(&mut self, now: Instant) {
        self.before = self.elapsed(now);
        self.since = None;
    }
}

/// Бегущая строка для любого элемента (например, ряда ссылок). Элемент
/// раскладывается без ограничения по ширине; если он шире места, строка
/// ездит туда и обратно. Пока курсор над строкой, она стоит, чтобы ссылку
/// можно было нажать.
pub fn marquee_element<'a, Message: 'a>(
    content: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    Element::new(MarqueeElement {
        content: content.into(),
    })
}

struct MarqueeElement<'a, Message> {
    content: Element<'a, Message>,
}

#[derive(Default)]
struct ElementState {
    /// На сколько пикселей элемент шире места. Ноль — помещается.
    overflow: f32,
    clock: Clock,
    offset: f32,
    hovered: bool,
}

/// Курсор в координатах содержимого: сдвинут на `offset`, а вне границ строки
/// «парит» и не попадает ни в одну ссылку (в том числе в обрезанную часть).
fn content_cursor(cursor: mouse::Cursor, bounds: Rectangle, offset: f32) -> mouse::Cursor {
    let shift = Vector::new(offset, 0.0);
    match cursor.position_over(bounds) {
        Some(position) => mouse::Cursor::Available(position + shift),
        None => cursor.levitate() + shift,
    }
}

impl<Message> Widget<Message, Theme, iced::Renderer> for MarqueeElement<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<ElementState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(ElementState::default())
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&[self.content.as_widget()]);
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Shrink)
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let Tree {
            state, children, ..
        } = tree;
        let state = state.downcast_mut::<ElementState>();
        let child = match children.first_mut() {
            Some(child_tree) => {
                let max = limits.max();
                let child_limits =
                    layout::Limits::new(Size::ZERO, Size::new(f32::INFINITY, max.height));
                self.content
                    .as_widget_mut()
                    .layout(child_tree, renderer, &child_limits)
            }
            None => layout::Node::new(Size::ZERO),
        };
        let child_size = child.size();
        let size = limits.resolve(Length::Fill, Length::Shrink, child_size);
        let overflow = (child_size.width - size.width).max(0.0);
        if (overflow - state.overflow).abs() > 0.5 {
            state.overflow = overflow;
            state.clock = Clock::default();
            state.offset = 0.0;
        }
        layout::Node::with_children(size, vec![child])
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
        let Tree {
            state, children, ..
        } = tree;
        let state = state.downcast_mut::<ElementState>();
        let bounds = layout.bounds();

        // Ссылки получают события с курсором, пересчитанным под сдвиг.
        if let (Some(child_tree), Some(child_layout)) =
            (children.first_mut(), layout.children().next())
        {
            self.content.as_widget_mut().update(
                child_tree,
                event,
                child_layout,
                content_cursor(cursor, bounds, state.offset),
                renderer,
                clipboard,
                shell,
                &Rectangle {
                    x: viewport.x + state.offset,
                    ..*viewport
                },
            );
        }

        if state.overflow <= 0.0 {
            state.offset = 0.0;
            return;
        }

        // Пауза при наведении: время прокрутки не идёт, кадры не нужны.
        let hovered = cursor.is_over(bounds);
        if hovered != state.hovered {
            state.hovered = hovered;
            let now = Instant::now();
            if hovered {
                state.clock.pause(now);
            } else {
                state.clock.resume(now);
                // Кадры на время наведения не просились: запускаем снова.
                shell.request_redraw();
            }
        }

        let Event::Window(window::Event::RedrawRequested(now)) = event else {
            return;
        };
        if state.hovered {
            return;
        }
        state.clock.resume(*now);
        let (offset, wait) = offset_at(state.clock.elapsed(*now), state.overflow);
        state.offset = offset;
        request_next(shell, *now, wait);
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
        let state = tree.state.downcast_ref::<ElementState>();
        let bounds = layout.bounds();
        let Some(clip) = bounds.intersection(viewport) else {
            return;
        };
        let (Some(child_tree), Some(child_layout)) =
            (tree.children.first(), layout.children().next())
        else {
            return;
        };
        let cursor = content_cursor(cursor, bounds, state.offset);
        renderer.with_layer(clip, |renderer| {
            renderer.with_translation(Vector::new(-state.offset, 0.0), |renderer| {
                self.content.as_widget().draw(
                    child_tree,
                    renderer,
                    theme,
                    style,
                    child_layout,
                    cursor,
                    &Rectangle {
                        x: clip.x + state.offset,
                        ..clip
                    },
                );
            });
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
        let state = tree.state.downcast_ref::<ElementState>();
        let (Some(child_tree), Some(child_layout)) =
            (tree.children.first(), layout.children().next())
        else {
            return mouse::Interaction::None;
        };
        self.content.as_widget().mouse_interaction(
            child_tree,
            child_layout,
            content_cursor(cursor, layout.bounds(), state.offset),
            &Rectangle {
                x: viewport.x + state.offset,
                ..*viewport
            },
            renderer,
        )
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        if let (Some(child_tree), Some(child_layout)) =
            (tree.children.first_mut(), layout.children().next())
        {
            operation.traverse(&mut |operation| {
                self.content
                    .as_widget_mut()
                    .operate(child_tree, child_layout, renderer, operation);
            });
        }
    }
}

/// Сдвиг текста через `elapsed` секунд после начала и, если сейчас пауза,
/// сколько секунд она ещё продлится. Цикл: пауза, путь влево, пауза, путь
/// обратно.
fn offset_at(elapsed: f32, overflow: f32) -> (f32, Option<f32>) {
    let travel = overflow / SPEED;
    let period = 2.0 * (PAUSE + travel);
    let t = elapsed.max(0.0) % period;
    if t < PAUSE {
        (0.0, Some(PAUSE - t))
    } else if t < PAUSE + travel {
        ((t - PAUSE) * SPEED, None)
    } else if t < 2.0 * PAUSE + travel {
        (overflow, Some(2.0 * PAUSE + travel - t))
    } else {
        (
            (overflow - (t - 2.0 * PAUSE - travel) * SPEED).max(0.0),
            None,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrolls_there_and_back_with_pauses() {
        // 80 пикселей при 40 пикс/с — две секунды пути, цикл 8 секунд.
        assert_eq!(offset_at(0.0, 80.0), (0.0, Some(2.0)));
        assert_eq!(offset_at(3.0, 80.0), (40.0, None));
        assert_eq!(offset_at(4.5, 80.0), (80.0, Some(1.5)));
        assert_eq!(offset_at(7.0, 80.0), (40.0, None));
        assert_eq!(offset_at(8.0, 80.0), (0.0, Some(2.0)));
    }

    #[test]
    fn clock_stands_while_hovered() {
        let start = Instant::now();
        let at = |seconds: f32| start + std::time::Duration::from_secs_f32(seconds);
        let mut clock = Clock::default();
        assert_eq!(clock.elapsed(start), 0.0);
        clock.resume(start);
        assert!((clock.elapsed(at(3.0)) - 3.0).abs() < 1e-3);
        // Наведение на 10 секунд: время стоит.
        clock.pause(at(3.0));
        assert!((clock.elapsed(at(13.0)) - 3.0).abs() < 1e-3);
        // Повторная остановка ничего не меняет.
        clock.pause(at(14.0));
        assert!((clock.elapsed(at(20.0)) - 3.0).abs() < 1e-3);
        // Ушли — продолжает с того же места.
        clock.resume(at(20.0));
        assert!((clock.elapsed(at(22.0)) - 5.0).abs() < 1e-3);
        // Повторный запуск не сдвигает отсчёт.
        clock.resume(at(21.0));
        assert!((clock.elapsed(at(22.0)) - 5.0).abs() < 1e-3);
    }

    #[test]
    fn odd_time_does_not_break() {
        assert_eq!(offset_at(-5.0, 80.0), (0.0, Some(2.0)));
        let (offset, _) = offset_at(f32::MAX, 80.0);
        assert!((0.0..=80.0).contains(&offset));
    }
}
