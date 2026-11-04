//! Бегущая строка: текст, который не помещается по ширине, плавно уезжает
//! влево до конца и возвращается обратно, с паузами по краям. Короткий текст
//! стоит на месте. Так название трека в плеере всегда можно прочитать
//! целиком: многоточия в iced нет, а обрезанный текст теряет конец.

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::{self, Renderer as _};
use iced::advanced::text::{self, Renderer as _, paragraph::Plain};
use iced::advanced::widget::{Tree, Widget, tree};
use iced::advanced::{Clipboard, Shell};
use iced::alignment::Vertical;
use iced::time::Instant;
use iced::widget::text::{LineHeight, Shaping, Wrapping};
use iced::{Color, Element, Event, Length, Pixels, Point, Rectangle, Size, Theme, mouse, window};

use super::theme::Style;

/// Пауза у каждого края, секунды.
const PAUSE: f32 = 2.0;
/// Скорость прокрутки, пикселей в секунду.
const SPEED: f32 = 40.0;

type Paragraph = <iced::Renderer as text::Renderer>::Paragraph;

pub struct Marquee {
    content: String,
    style: Style,
    color: Color,
}

/// Однострочный текст в стиле `style`, длинный — бегущей строкой.
pub fn marquee(content: impl Into<String>, style: Style, color: Color) -> Marquee {
    Marquee {
        content: content.into(),
        style,
        color,
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
        // На паузе кадры не нужны: просим перерисовку к концу паузы.
        match wait
            .and_then(|seconds| std::time::Duration::try_from_secs_f32(seconds).ok())
            .and_then(|pause| now.checked_add(pause))
        {
            Some(at) => shell.request_redraw_at(window::RedrawRequest::At(at)),
            None => shell.request_redraw(),
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
        renderer.with_layer(clip, |renderer| {
            renderer.fill_paragraph(
                state.paragraph.raw(),
                Point::new(bounds.x - state.offset, bounds.y),
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
    fn odd_time_does_not_break() {
        assert_eq!(offset_at(-5.0, 80.0), (0.0, Some(2.0)));
        let (offset, _) = offset_at(f32::MAX, 80.0);
        assert!((0.0..=80.0).contains(&offset));
    }
}
