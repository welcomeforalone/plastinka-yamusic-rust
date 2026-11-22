//! Палитра своего акцента в «Настройки → Внешний вид»: квадрат
//! «насыщенность и яркость», полоса тона, поле `#RRGGBB` и превью.
//!
//! Цвет хранится как HSV, а не RGB: на сером и чёрном RGB не помнит тон, и
//! при перетаскивании маркер прыгал бы. Пока идёт перетаскивание, акцент
//! меняется во всём окне, но в `config.toml` ничего не пишется: запись — один
//! раз, когда кнопку мыши отпустили (или когда в поле введён верный цвет).

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::{self, Quad, Renderer as _};
use iced::advanced::widget::{Tree, tree};
use iced::advanced::{Clipboard, Shell, Widget};
use iced::gradient::Linear;
use iced::widget::{Space, column, container, row, text_input};
use iced::{
    Alignment, Background, Border, Color, Degrees, Element, Event, Length, Point, Rectangle,
    Shadow, Size, Theme, Vector, mouse,
};

use super::Message as App;
use super::theme::{self, Accent, Style};
use super::widgets;

/// Высота квадрата «насыщенность и яркость» и полосы тона.
const FIELD_HEIGHT: f32 = 168.0;
const HUE_HEIGHT: f32 = 18.0;
/// Диаметр маркера на квадрате и на полосе тона.
const FIELD_MARKER: f32 = 18.0;
const HUE_MARKER: f32 = 24.0;

// ------------------------------------------------------------------ цвет

/// Цвет в HSV: тон 0–360°, насыщенность и яркость 0–1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hsv {
    pub h: f32,
    pub s: f32,
    pub v: f32,
}

/// Число в границах; NaN и бесконечность дают нижнюю границу.
fn bounded(value: f32, min: f32, max: f32) -> f32 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        min
    }
}

/// Канал 0–1 в байт с округлением и ограничением.
fn to_byte(channel: f32) -> u8 {
    (bounded(channel, 0.0, 1.0) * 255.0).round() as u8
}

impl Hsv {
    /// Приводит значения к допустимым: тон 0–360, остальное 0–1, NaN — 0.
    pub fn sanitized(self) -> Self {
        Self {
            h: bounded(self.h, 0.0, 360.0),
            s: bounded(self.s, 0.0, 1.0),
            v: bounded(self.v, 0.0, 1.0),
        }
    }

    /// Цвет как `0xRRGGBB`. Тон 360° — тот же красный, что и 0°.
    pub fn to_rgb(self) -> u32 {
        let Self { h, s, v } = self.sanitized();
        let h = if h >= 360.0 { 0.0 } else { h };
        let chroma = v * s;
        let sector = h / 60.0;
        let x = chroma * (1.0 - (sector % 2.0 - 1.0).abs());
        let (r, g, b) = if sector < 1.0 {
            (chroma, x, 0.0)
        } else if sector < 2.0 {
            (x, chroma, 0.0)
        } else if sector < 3.0 {
            (0.0, chroma, x)
        } else if sector < 4.0 {
            (0.0, x, chroma)
        } else if sector < 5.0 {
            (x, 0.0, chroma)
        } else {
            (chroma, 0.0, x)
        };
        let m = v - chroma;
        let r = u32::from(to_byte(r + m));
        let g = u32::from(to_byte(g + m));
        let b = u32::from(to_byte(b + m));
        (r << 16) | (g << 8) | b
    }

    /// HSV из `0xRRGGBB`. У серого и чёрного тона нет: берётся `hue`, чтобы
    /// он не терялся.
    pub fn from_rgb(rgb: u32, hue: f32) -> Self {
        let [_, r, g, b] = rgb.to_be_bytes();
        let r = f32::from(r) / 255.0;
        let g = f32::from(g) / 255.0;
        let b = f32::from(b) / 255.0;
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let delta = max - min;
        let h = if delta <= 0.0 {
            bounded(hue, 0.0, 360.0)
        } else if max == r {
            60.0 * ((g - b) / delta).rem_euclid(6.0)
        } else if max == g {
            60.0 * ((b - r) / delta + 2.0)
        } else {
            60.0 * ((r - g) / delta + 4.0)
        };
        let s = if max <= 0.0 { 0.0 } else { delta / max };
        Self { h, s, v: max }.sanitized()
    }

    /// Чистый тон: насыщенность и яркость полные.
    fn pure(self) -> Color {
        color_of(
            Self {
                s: 1.0,
                v: 1.0,
                ..self
            }
            .to_rgb(),
        )
    }
}

fn color_of(rgb: u32) -> Color {
    let [_, r, g, b] = rgb.to_be_bytes();
    Color::from_rgb8(r, g, b)
}

/// Радужная заливка: образец «Свой цвет» в ряду акцентов.
pub fn rainbow() -> Background {
    Background::Gradient(hue_gradient().into())
}

fn hue_gradient() -> Linear {
    let mut gradient = Linear::new(Degrees(90.0));
    for step in 0..=6u8 {
        let hue = f32::from(step) * 60.0;
        let color = Hsv {
            h: hue,
            s: 1.0,
            v: 1.0,
        }
        .pure();
        gradient = gradient.add_stop(f32::from(step) / 6.0, color);
    }
    gradient
}

// ------------------------------------------------------------- состояние

/// Состояние палитры. Живёт в `App`, пока открыты настройки.
#[derive(Debug, Clone)]
pub struct State {
    /// Блок палитры раскрыт.
    pub open: bool,
    hsv: Hsv,
    /// Текст в поле `#RRGGBB`: как ввёл пользователь.
    text: String,
    /// Цвет менялся перетаскиванием, но ещё не записан в `config.toml`.
    dirty: bool,
}

impl Default for State {
    fn default() -> Self {
        Self::new(Accent::default())
    }
}

impl State {
    pub fn new(accent: Accent) -> Self {
        Self {
            open: false,
            hsv: Hsv::from_rgb(accent.rgb(), 0.0),
            text: theme::format_hex(accent.rgb()),
            dirty: false,
        }
    }

    /// Палитра показывает `accent`: тон серого остаётся прежним.
    pub fn sync(&mut self, accent: Accent) {
        self.hsv = Hsv::from_rgb(accent.rgb(), self.hsv.h);
        self.text = theme::format_hex(accent.rgb());
        self.dirty = false;
    }

    /// Выбранный цвет как акцент.
    pub fn accent(&self) -> Accent {
        Accent::custom(self.hsv.to_rgb())
    }

    /// Точка на квадрате, `x` и `y` от 0 до 1 (`y` — сверху вниз).
    pub fn pick(&mut self, x: f32, y: f32) {
        self.hsv.s = bounded(x, 0.0, 1.0);
        self.hsv.v = 1.0 - bounded(y, 0.0, 1.0);
        self.changed();
    }

    /// Точка на полосе тона, `x` от 0 до 1.
    pub fn pick_hue(&mut self, x: f32) {
        self.hsv.h = bounded(x, 0.0, 1.0) * 360.0;
        self.changed();
    }

    fn changed(&mut self) {
        self.text = theme::format_hex(self.hsv.to_rgb());
        self.dirty = true;
    }

    /// Отпустили кнопку мыши: `true`, если цвет надо записать.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// Ввод в поле. Верный цвет — акцент, который надо применить и записать.
    pub fn input(&mut self, text: String) -> Option<Accent> {
        let rgb = theme::parse_hex(&text);
        self.text = text;
        let rgb = rgb?;
        self.hsv = Hsv::from_rgb(rgb, self.hsv.h);
        self.dirty = false;
        Some(Accent::custom(rgb))
    }

    /// В поле неверный цвет.
    fn invalid(&self) -> bool {
        theme::parse_hex(&self.text).is_none()
    }
}

/// Сообщения палитры.
#[derive(Debug, Clone)]
pub enum Msg {
    /// Показать или спрятать блок.
    Toggle,
    /// Точка на квадрате: насыщенность (слева направо) и «высота» сверху вниз.
    Field(f32, f32),
    /// Точка на полосе тона.
    Hue(f32),
    /// Кнопку мыши отпустили.
    Released,
    /// Текст в поле `#RRGGBB`.
    Hex(String),
}

// ------------------------------------------------------------------- вид

/// Блок палитры под строкой «Акцент».
pub fn view(state: &State) -> Element<'_, App> {
    let hsv = state.hsv;
    let chosen = color_of(hsv.to_rgb());

    let field = surface(
        Kind::Field(hsv.pure()),
        (hsv.s, 1.0 - hsv.v),
        chosen,
        Box::new(|x, y| App::Palette(Msg::Field(x, y))),
    );
    let strip = surface(
        Kind::Hue,
        (hsv.h / 360.0, 0.5),
        hsv.pure(),
        Box::new(|x, _| App::Palette(Msg::Hue(x))),
    );

    let invalid = state.invalid();
    let preview = container(Space::new())
        .width(36)
        .height(36)
        .style(move |_| container::Style {
            background: Some(chosen.into()),
            border: Border {
                color: theme::outline(),
                width: 1.0,
                radius: widgets::ROUND.into(),
            },
            ..container::Style::default()
        });
    let input = text_input("#RRGGBB", &state.text)
        .on_input(|text| App::Palette(Msg::Hex(text)))
        .size(14)
        .width(120)
        .padding([9, 16])
        .style(move |_, status| {
            let border = if invalid {
                theme::error()
            } else if matches!(status, text_input::Status::Focused { .. }) {
                theme::text_secondary()
            } else {
                theme::outline()
            };
            text_input::Style {
                background: Color::TRANSPARENT.into(),
                border: Border {
                    color: border,
                    width: 1.0,
                    radius: widgets::ROUND.into(),
                },
                icon: theme::text_tertiary(),
                placeholder: theme::text_tertiary(),
                value: theme::text_primary(),
                selection: theme::alpha(theme::accent(), 0.35),
            }
        });
    let mut bottom = row![preview, input].spacing(12).align_y(Alignment::Center);
    if invalid {
        bottom = bottom.push(theme::line(
            "Нужен цвет вида #RRGGBB",
            Style::Caption,
            theme::error(),
        ));
    }

    column![field, strip, bottom].spacing(16).into()
}

fn surface<'a>(
    kind: Kind,
    marker: (f32, f32),
    marker_fill: Color,
    on_change: Box<dyn Fn(f32, f32) -> App + 'a>,
) -> Element<'a, App> {
    Element::new(Surface {
        kind,
        marker,
        marker_fill,
        on_change,
    })
}

#[derive(Clone, Copy)]
enum Kind {
    /// Квадрат: слева белый, справа чистый тон (его цвет), снизу чёрный.
    Field(Color),
    /// Полоса тона.
    Hue,
}

/// Поверхность выбора цвета: рисует градиенты и маркер сама и следит за
/// мышью. Перетаскивание не прерывается, когда курсор вышел за границы:
/// позиция зажимается в них.
struct Surface<'a> {
    kind: Kind,
    /// Маркер: доли ширины и высоты.
    marker: (f32, f32),
    marker_fill: Color,
    on_change: Box<dyn Fn(f32, f32) -> App + 'a>,
}

/// Идёт ли перетаскивание.
#[derive(Default)]
struct Drag(bool);

impl Surface<'_> {
    fn height(&self) -> f32 {
        match self.kind {
            Kind::Field(_) => FIELD_HEIGHT,
            Kind::Hue => HUE_HEIGHT,
        }
    }

    /// Точка в долях размера, зажатая в границы.
    fn normalize(point: Point, bounds: Rectangle) -> (f32, f32) {
        let part = |offset: f32, size: f32| {
            if size > 0.0 {
                bounded(offset / size, 0.0, 1.0)
            } else {
                0.0
            }
        };
        (
            part(point.x - bounds.x, bounds.width),
            part(point.y - bounds.y, bounds.height),
        )
    }
}

impl Widget<App, Theme, iced::Renderer> for Surface<'_> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<Drag>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(Drag::default())
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fixed(self.height()))
    }

    fn layout(
        &mut self,
        _tree: &mut Tree,
        _renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::atomic(limits, Length::Fill, Length::Fixed(self.height()))
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, App>,
        _viewport: &Rectangle,
    ) {
        let drag = tree.state.downcast_mut::<Drag>();
        let bounds = layout.bounds();
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let Some(point) = cursor.position_over(bounds) {
                    drag.0 = true;
                    let (x, y) = Self::normalize(point, bounds);
                    shell.publish((self.on_change)(x, y));
                    shell.capture_event();
                }
            }
            // Позиция берётся из курсора, а не из события: внутри прокрутки
            // курсор уже пересчитан, а событие нет.
            Event::Mouse(mouse::Event::CursorMoved { .. }) if drag.0 => {
                if let Some(point) = cursor.position() {
                    let (x, y) = Self::normalize(point, bounds);
                    shell.publish((self.on_change)(x, y));
                }
                shell.capture_event();
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) if drag.0 => {
                drag.0 = false;
                shell.publish(App::Palette(Msg::Released));
                shell.capture_event();
            }
            _ => {}
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
        let border = Border::default().rounded(widgets::RADIUS_S);
        match self.kind {
            Kind::Field(pure) => {
                let across = Linear::new(Degrees(90.0))
                    .add_stop(0.0, Color::WHITE)
                    .add_stop(1.0, pure);
                let down = Linear::new(Degrees(180.0))
                    .add_stop(0.0, theme::clear(Color::BLACK))
                    .add_stop(1.0, Color::BLACK);
                for gradient in [across, down] {
                    renderer.fill_quad(
                        Quad {
                            bounds,
                            border,
                            ..Quad::default()
                        },
                        Background::Gradient(gradient.into()),
                    );
                }
            }
            Kind::Hue => {
                renderer.fill_quad(
                    Quad {
                        bounds,
                        border: Border::default().rounded(bounds.height / 2.0),
                        ..Quad::default()
                    },
                    Background::Gradient(hue_gradient().into()),
                );
            }
        }

        let size = match self.kind {
            Kind::Field(_) => FIELD_MARKER,
            Kind::Hue => HUE_MARKER,
        };
        renderer.fill_quad(
            Quad {
                bounds: Rectangle {
                    x: bounds.x + bounds.width * self.marker.0 - size / 2.0,
                    y: bounds.y + bounds.height * self.marker.1 - size / 2.0,
                    width: size,
                    height: size,
                },
                border: Border {
                    color: Color::WHITE,
                    width: 3.0,
                    radius: (size / 2.0).into(),
                },
                shadow: Shadow {
                    color: theme::alpha(Color::BLACK, 0.5),
                    offset: Vector::new(0.0, 1.0),
                    blur_radius: 4.0,
                },
                ..Quad::default()
            },
            self.marker_fill,
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
        if tree.state.downcast_ref::<Drag>().0 || cursor.is_over(layout.bounds()) {
            mouse::Interaction::Crosshair
        } else {
            mouse::Interaction::None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hsv(h: f32, s: f32, v: f32) -> Hsv {
        Hsv { h, s, v }
    }

    #[test]
    fn hsv_to_rgb_corners() {
        assert_eq!(hsv(0.0, 1.0, 1.0).to_rgb(), 0xFF0000);
        assert_eq!(hsv(60.0, 1.0, 1.0).to_rgb(), 0xFFFF00);
        assert_eq!(hsv(120.0, 1.0, 1.0).to_rgb(), 0x00FF00);
        assert_eq!(hsv(180.0, 1.0, 1.0).to_rgb(), 0x00FFFF);
        assert_eq!(hsv(240.0, 1.0, 1.0).to_rgb(), 0x0000FF);
        assert_eq!(hsv(300.0, 1.0, 1.0).to_rgb(), 0xFF00FF);
        // 360° — снова красный.
        assert_eq!(hsv(360.0, 1.0, 1.0).to_rgb(), 0xFF0000);
        // Без насыщенности — серый, тон не важен.
        assert_eq!(hsv(200.0, 0.0, 1.0).to_rgb(), 0xFFFFFF);
        assert_eq!(hsv(200.0, 0.0, 0.5).to_rgb(), 0x808080);
        // Без яркости — чёрный.
        assert_eq!(hsv(200.0, 1.0, 0.0).to_rgb(), 0x000000);
    }

    #[test]
    fn hsv_survives_garbage() {
        assert_eq!(hsv(f32::NAN, f32::NAN, f32::NAN).to_rgb(), 0);
        assert_eq!(hsv(f32::INFINITY, 1.0, 1.0).to_rgb(), 0xFF0000);
        assert_eq!(hsv(-50.0, 2.0, 7.0).to_rgb(), 0xFF0000);
        assert_eq!(hsv(720.0, 1.0, 1.0).to_rgb(), 0xFF0000);
        let fixed = hsv(f32::NAN, -1.0, 9.0).sanitized();
        assert_eq!(fixed, hsv(0.0, 0.0, 1.0));
    }

    #[test]
    fn rgb_to_hsv_keeps_hue_on_grey() {
        let grey = Hsv::from_rgb(0x808080, 200.0);
        assert_eq!(grey.h, 200.0);
        assert_eq!(grey.s, 0.0);
        let black = Hsv::from_rgb(0x000000, 123.0);
        assert_eq!((black.h, black.s, black.v), (123.0, 0.0, 0.0));
        let white = Hsv::from_rgb(0xFFFFFF, f32::NAN);
        assert_eq!((white.h, white.s, white.v), (0.0, 0.0, 1.0));
        let red = Hsv::from_rgb(0xFF0000, 77.0);
        assert_eq!((red.h, red.s, red.v), (0.0, 1.0, 1.0));
    }

    #[test]
    fn rgb_round_trips_through_hsv() {
        let steps = (0..=255u32).step_by(15);
        for r in steps.clone() {
            for g in steps.clone() {
                for b in steps.clone() {
                    let rgb = (r << 16) | (g << 8) | b;
                    assert_eq!(Hsv::from_rgb(rgb, 0.0).to_rgb(), rgb, "{rgb:06X}");
                }
            }
        }
        // Готовые акценты тоже.
        for accent in Accent::PRESETS {
            assert_eq!(Hsv::from_rgb(accent.rgb(), 0.0).to_rgb(), accent.rgb());
        }
    }

    #[test]
    fn dragging_marks_dirty_until_release() {
        let mut state = State::new(Accent::Red);
        assert!(!state.take_dirty());
        state.pick(0.5, 0.25);
        assert!(state.take_dirty());
        assert!(!state.take_dirty());
        state.pick_hue(2.0);
        assert_eq!(state.hsv.h, 360.0);
        assert!(state.take_dirty());
        assert_eq!(state.text, theme::format_hex(state.hsv.to_rgb()));
    }

    #[test]
    fn pick_clamps_and_keeps_hue_at_black() {
        let mut state = State::new(Accent::Blue);
        state.pick_hue(0.5);
        state.pick(f32::NAN, 5.0);
        assert_eq!(state.accent(), Accent::custom(0));
        assert_eq!(state.hsv.h, 180.0);
        // Поймали серый и вернулись: тон не потерян.
        state.pick(1.0, 0.0);
        assert_eq!(state.accent(), Accent::custom(0x00FFFF));
    }

    #[test]
    fn hex_input_applies_only_valid_colors() {
        let mut state = State::new(Accent::Red);
        assert_eq!(state.input("#12".to_owned()), None);
        assert!(state.invalid());
        assert_eq!(
            state.input(" #00ff00 ".to_owned()),
            Some(Accent::Custom(0x00FF00))
        );
        assert!(!state.invalid());
        assert_eq!(state.hsv.h, 120.0);
        assert!(!state.take_dirty());
        assert_eq!(state.input("".to_owned()), None);
        assert!(state.invalid());
    }

    #[test]
    fn sync_follows_accent_and_keeps_grey_hue() {
        let mut state = State::new(Accent::Red);
        state.pick_hue(0.5);
        state.sync(Accent::custom(0x808080));
        assert_eq!(state.hsv.h, 180.0);
        assert_eq!(state.text, "#808080");
        assert!(!state.take_dirty());
    }

    #[test]
    fn normalize_clamps_to_bounds() {
        let bounds = Rectangle {
            x: 10.0,
            y: 20.0,
            width: 100.0,
            height: 50.0,
        };
        assert_eq!(
            Surface::normalize(Point::new(60.0, 45.0), bounds),
            (0.5, 0.5)
        );
        assert_eq!(
            Surface::normalize(Point::new(-500.0, 900.0), bounds),
            (0.0, 1.0)
        );
        let empty = Rectangle {
            width: 0.0,
            height: 0.0,
            ..bounds
        };
        assert_eq!(Surface::normalize(Point::new(5.0, 5.0), empty), (0.0, 0.0));
    }
}
