//! Виджет десяти полос эквалайзера: сетка +12 / 0 / −12 дБ, столбцы с
//! заполнением от нуля, ручки. Перетаскивание по вертикали меняет полосу с
//! шагом 0,5 дБ и прилипанием к нулю, двойной щелчок — 0 дБ. Колесо мыши
//! полосы не трогает: страница под курсором должна прокручиваться.
//! Рисуется квадами (как `palette.rs`), подписи — обычные текстовые виджеты
//! рядом: столбцы и подписи делят ширину поровну.

use iced::advanced::layout::{self, Layout};
use iced::advanced::mouse::{Click, click};
use iced::advanced::renderer::{self, Quad, Renderer as _};
use iced::advanced::widget::{Tree, tree};
use iced::advanced::{Clipboard, Shell, Widget};
use iced::widget::{Space, column, container, row};
use iced::{Border, Color, Element, Event, Length, Rectangle, Size, Theme, mouse};

use crate::player::eq::{BANDS, FREQS, GAIN_LIMIT};
use crate::ui::Message as App;
use crate::ui::theme::{self, Style};

/// Высота области полос (без подписей под ней).
const PLOT_HEIGHT: f32 = 200.0;
/// Отступ дорожки сверху и снизу: большая ручка не выходит за виджет.
const PLOT_PAD: f32 = 12.0;
/// Ширина дорожки, диаметр ручки обычный и под курсором.
const TRACK_WIDTH: f32 = 4.0;
const HANDLE: f32 = 16.0;
const HANDLE_HOT: f32 = 20.0;
/// Колонка подписей шкалы слева.
const AXIS_WIDTH: f32 = 40.0;
/// Шаг значения: 0,5 дБ в десятых долях.
const STEP: i16 = 5;

// ------------------------------------------------------- чистая геометрия

/// Вертикальный отрезок дорожки: `top` — +12 дБ, `bottom` — −12 дБ.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Span {
    top: f32,
    bottom: f32,
}

impl Span {
    fn new(bounds: Rectangle) -> Self {
        Self {
            top: bounds.y + PLOT_PAD,
            bottom: bounds.y + bounds.height - PLOT_PAD,
        }
    }

    fn middle(self) -> f32 {
        (self.top + self.bottom) / 2.0
    }

    /// Координата нулевой линии — середина; значение вне пределов обрезается.
    fn y_of(self, tenths: i16) -> f32 {
        let limit = f32::from(GAIN_LIMIT);
        let value = f32::from(tenths.clamp(-GAIN_LIMIT, GAIN_LIMIT));
        self.middle() - value / limit * (self.bottom - self.top) / 2.0
    }

    /// Значение по координате: шаг 0,5 дБ, в пределах ±0,5 дБ от нуля — 0.
    fn tenths_at(self, y: f32) -> i16 {
        let half = (self.bottom - self.top) / 2.0;
        if !y.is_finite() || half <= 0.0 {
            return 0;
        }
        let limit = f32::from(GAIN_LIMIT);
        let step = f32::from(STEP);
        let raw = ((self.middle() - y) / half * limit).clamp(-limit, limit);
        if raw.abs() < step {
            return 0;
        }
        ((raw / step).round() * step) as i16
    }
}

/// Номер столбца под `x`; `left` и `width` — область полос.
fn column_at(x: f32, left: f32, width: f32) -> Option<usize> {
    if !x.is_finite() || width <= 0.0 {
        return None;
    }
    let offset = x - left;
    if offset < 0.0 || offset >= width {
        return None;
    }
    let index = (offset / (width / BANDS as f32)).floor() as usize;
    Some(index.min(BANDS - 1))
}

/// Середина столбца по горизонтали.
fn column_center(index: usize, left: f32, width: f32) -> f32 {
    left + (index as f32 + 0.5) * width / BANDS as f32
}

// ----------------------------------------------------------------- подписи

/// Подпись полосы: `+4,5`, `0`, `−2` (настоящий минус).
pub fn format_value(tenths: i16) -> String {
    if tenths == 0 {
        return "0".to_owned();
    }
    let sign = if tenths > 0 { '+' } else { '−' };
    let abs = tenths.unsigned_abs();
    let (whole, frac) = (abs / 10, abs % 10);
    if frac == 0 {
        format!("{sign}{whole}")
    } else {
        format!("{sign}{whole},{frac}")
    }
}

/// Значение с единицей: `−3,0 дБ`, `0,0 дБ`, `+12,0 дБ`.
pub fn format_db(tenths: i16) -> String {
    let sign = match tenths {
        0 => "",
        t if t > 0 => "+",
        _ => "−",
    };
    let abs = tenths.unsigned_abs();
    format!("{sign}{},{} дБ", abs / 10, abs % 10)
}

/// Частота полосы: `32`, `1k`, `16k`.
pub fn format_freq(hz: u32) -> String {
    if hz < 1000 {
        return hz.to_string();
    }
    let (kilo, rest) = (hz / 1000, hz % 1000 / 100);
    if rest == 0 {
        format!("{kilo}k")
    } else {
        format!("{kilo},{rest}k")
    }
}

// ------------------------------------------------------------------ виджет

/// Собранный блок: шкала слева, полосы, под ними значения и частоты.
pub fn view<'a>(
    gains: [i16; BANDS],
    enabled: bool,
    on_band: impl Fn(usize, i16) -> App + 'a,
    on_commit: App,
) -> Element<'a, App> {
    let plot = Element::new(Bands {
        gains,
        enabled,
        on_band: Box::new(on_band),
        on_commit,
    });
    let cell = |text: String, color: Color| -> Element<'a, App> {
        container(theme::line(text, Style::Caption, color))
            .width(Length::Fill)
            .center_x(Length::Fill)
            .into()
    };
    let values = row(gains
        .iter()
        .map(|gain| cell(format_value(*gain), theme::TEXT_SECONDARY)));
    let freqs = row(FREQS
        .iter()
        .map(|hz| cell(format_freq(*hz), theme::TEXT_TERTIARY)));
    // Подписи шкалы по центру линий: высота строки 16, линия на отступе 12.
    let axis = column![
        theme::line("+12", Style::Caption, theme::TEXT_TERTIARY),
        Space::new().height(Length::Fill),
        theme::line("0", Style::Caption, theme::TEXT_TERTIARY),
        Space::new().height(Length::Fill),
        theme::line("−12", Style::Caption, theme::TEXT_TERTIARY),
    ]
    .width(AXIS_WIDTH)
    .height(PLOT_HEIGHT)
    .padding([PLOT_PAD - 8.0, 0.0]);
    row![
        axis,
        column![plot, values, freqs].spacing(2).width(Length::Fill)
    ]
    .into()
}

struct Bands<'a> {
    gains: [i16; BANDS],
    enabled: bool,
    on_band: Box<dyn Fn(usize, i16) -> App + 'a>,
    on_commit: App,
}

/// Состояние взаимодействия: перетаскиваемый и подсвеченный столбцы.
#[derive(Default)]
struct Interaction {
    drag: Option<usize>,
    hover: Option<usize>,
    /// За это нажатие значение менялось: при отпускании пишем в конфиг.
    changed: bool,
    last_click: Option<Click>,
}

impl Bands<'_> {
    fn gain(&self, index: usize) -> i16 {
        self.gains.get(index).copied().unwrap_or(0)
    }

    /// Публикует новое значение, если оно отличается от показанного.
    fn set(&self, state: &mut Interaction, shell: &mut Shell<'_, App>, index: usize, value: i16) {
        if value != self.gain(index) {
            state.changed = true;
            shell.publish((self.on_band)(index, value));
        }
    }
}

impl Widget<App, Theme, iced::Renderer> for Bands<'_> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<Interaction>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(Interaction::default())
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fixed(PLOT_HEIGHT))
    }

    fn layout(
        &mut self,
        _tree: &mut Tree,
        _renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::atomic(limits, Length::Fill, Length::Fixed(PLOT_HEIGHT))
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
        let state = tree.state.downcast_mut::<Interaction>();
        let bounds = layout.bounds();
        let span = Span::new(bounds);
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let Some(point) = cursor.position_over(bounds) else {
                    return;
                };
                let Some(index) = column_at(point.x, bounds.x, bounds.width) else {
                    return;
                };
                let click = Click::new(point, mouse::Button::Left, state.last_click);
                state.last_click = Some(click);
                if click.kind() == click::Kind::Single {
                    state.drag = Some(index);
                    state.changed = false;
                    self.set(state, shell, index, span.tenths_at(point.y));
                } else {
                    // Двойной щелчок: в ноль и сразу в конфиг.
                    state.drag = None;
                    state.changed = false;
                    shell.publish((self.on_band)(index, 0));
                    shell.publish(self.on_commit.clone());
                }
                shell.capture_event();
                shell.request_redraw();
            }
            // Позиция берётся из курсора, а не из события: внутри прокрутки
            // курсор уже пересчитан, а событие нет.
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if let Some(index) = state.drag {
                    if let Some(point) = cursor.position() {
                        self.set(state, shell, index, span.tenths_at(point.y));
                    }
                    shell.capture_event();
                }
                let hover = cursor
                    .position_over(bounds)
                    .and_then(|point| column_at(point.x, bounds.x, bounds.width));
                if hover != state.hover {
                    state.hover = hover;
                    shell.request_redraw();
                }
            }
            Event::Mouse(mouse::Event::CursorLeft) if state.hover.is_some() => {
                state.hover = None;
                shell.request_redraw();
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
                if state.drag.is_some() =>
            {
                state.drag = None;
                if std::mem::take(&mut state.changed) {
                    shell.publish(self.on_commit.clone());
                }
                shell.capture_event();
                shell.request_redraw();
            }
            _ => {}
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let span = Span::new(bounds);
        let state = tree.state.downcast_ref::<Interaction>();
        let hot = state.drag.or_else(|| {
            cursor
                .position_over(bounds)
                .and_then(|point| column_at(point.x, bounds.x, bounds.width))
        });
        let mut quad = |x: f32, y: f32, width: f32, height: f32, radius: f32, color: Color| {
            renderer.fill_quad(
                Quad {
                    bounds: Rectangle {
                        x,
                        y,
                        width,
                        height,
                    },
                    border: Border::default().rounded(radius),
                    ..Quad::default()
                },
                color,
            );
        };

        // Сетка: +12, 0 и −12 дБ; нулевая линия ярче.
        for (y, color) in [
            (span.top, theme::BORDER),
            (span.bottom, theme::BORDER),
            (span.middle(), theme::OUTLINE),
        ] {
            quad(bounds.x, y - 0.5, bounds.width, 1.0, 0.0, color);
        }

        let (fill, handle) = if self.enabled {
            (theme::accent(), theme::TEXT_PRIMARY)
        } else {
            (theme::TEXT_DISABLED, theme::TEXT_DISABLED)
        };
        for (index, gain) in self.gains.iter().enumerate() {
            let x = column_center(index, bounds.x, bounds.width);
            quad(
                x - TRACK_WIDTH / 2.0,
                span.top,
                TRACK_WIDTH,
                span.bottom - span.top,
                TRACK_WIDTH / 2.0,
                theme::SURFACE_3,
            );
            let y = span.y_of(*gain);
            let (from, to) = if y < span.middle() {
                (y, span.middle())
            } else {
                (span.middle(), y)
            };
            quad(
                x - TRACK_WIDTH / 2.0,
                from,
                TRACK_WIDTH,
                to - from,
                0.0,
                fill,
            );
            let size = if hot == Some(index) {
                HANDLE_HOT
            } else {
                HANDLE
            };
            quad(
                x - size / 2.0,
                y - size / 2.0,
                size,
                size,
                size / 2.0,
                handle,
            );
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let state = tree.state.downcast_ref::<Interaction>();
        if state.drag.is_some() {
            mouse::Interaction::Grabbing
        } else if cursor.is_over(layout.bounds()) {
            mouse::Interaction::Grab
        } else {
            mouse::Interaction::None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span() -> Span {
        Span {
            top: 10.0,
            bottom: 190.0,
        }
    }

    #[test]
    fn value_to_y_maps_limits_and_zero() {
        let span = span();
        assert_eq!(span.y_of(GAIN_LIMIT), 10.0);
        assert_eq!(span.y_of(-GAIN_LIMIT), 190.0);
        assert_eq!(span.y_of(0), 100.0);
        assert_eq!(span.y_of(60), 55.0);
        // Вне пределов — обрезается.
        assert_eq!(span.y_of(500), 10.0);
        assert_eq!(span.y_of(-500), 190.0);
    }

    #[test]
    fn y_to_value_round_trips_on_steps() {
        let span = span();
        for tenths in (-GAIN_LIMIT..=GAIN_LIMIT).step_by(usize::from(STEP.unsigned_abs())) {
            if tenths.abs() < STEP {
                continue;
            }
            assert_eq!(span.tenths_at(span.y_of(tenths)), tenths, "{tenths}");
        }
    }

    #[test]
    fn y_to_value_clamps_outside() {
        let span = span();
        assert_eq!(span.tenths_at(-1000.0), GAIN_LIMIT);
        assert_eq!(span.tenths_at(1000.0), -GAIN_LIMIT);
        assert_eq!(span.tenths_at(f32::NAN), 0);
        assert_eq!(span.tenths_at(f32::INFINITY), 0);
        let empty = Span {
            top: 5.0,
            bottom: 5.0,
        };
        assert_eq!(empty.tenths_at(0.0), 0);
    }

    #[test]
    fn value_snaps_to_half_decibel_and_sticks_to_zero() {
        let span = span();
        // 1 дБ = 180 / 24 = 7,5 пикселя.
        let y = |db: f32| span.middle() - db * 7.5;
        assert_eq!(span.tenths_at(y(1.1)), 10);
        assert_eq!(span.tenths_at(y(1.3)), 15);
        assert_eq!(span.tenths_at(y(-2.6)), -25);
        // Липнет к нулю: меньше 0,5 дБ в обе стороны.
        assert_eq!(span.tenths_at(y(0.0)), 0);
        assert_eq!(span.tenths_at(y(0.4)), 0);
        assert_eq!(span.tenths_at(y(-0.4)), 0);
        // От 0,5 дБ — уже значение.
        assert_eq!(span.tenths_at(y(0.6)), 5);
        assert_eq!(span.tenths_at(y(-0.6)), -5);
    }

    #[test]
    fn columns_share_width_equally() {
        let (left, width) = (100.0, 500.0);
        assert_eq!(column_at(99.0, left, width), None);
        assert_eq!(column_at(100.0, left, width), Some(0));
        assert_eq!(column_at(149.9, left, width), Some(0));
        assert_eq!(column_at(150.0, left, width), Some(1));
        assert_eq!(column_at(599.9, left, width), Some(BANDS - 1));
        assert_eq!(column_at(600.0, left, width), None);
        assert_eq!(column_at(f32::NAN, left, width), None);
        assert_eq!(column_at(120.0, left, 0.0), None);
        assert_eq!(column_center(0, left, width), 125.0);
        assert_eq!(column_center(9, left, width), 575.0);
    }

    #[test]
    fn narrow_widget_keeps_columns_apart() {
        // Узкая страница: столбцы не накладываются, центры идут по порядку.
        let width = 300.0;
        let centers: Vec<f32> = (0..BANDS).map(|i| column_center(i, 0.0, width)).collect();
        for pair in centers.windows(2) {
            assert!(pair[1] - pair[0] >= 29.0);
        }
        assert_eq!(column_at(centers[3], 0.0, width), Some(3));
    }

    #[test]
    fn value_labels() {
        assert_eq!(format_value(45), "+4,5");
        assert_eq!(format_value(0), "0");
        assert_eq!(format_value(-20), "−2");
        assert_eq!(format_value(-5), "−0,5");
        assert_eq!(format_value(120), "+12");
        assert_eq!(format_value(-120), "−12");
    }

    #[test]
    fn db_labels() {
        assert_eq!(format_db(-30), "−3,0 дБ");
        assert_eq!(format_db(0), "0,0 дБ");
        assert_eq!(format_db(125 - 5), "+12,0 дБ");
        assert_eq!(format_db(5), "+0,5 дБ");
        assert_eq!(format_db(-95), "−9,5 дБ");
    }

    #[test]
    fn frequency_labels() {
        let labels: Vec<String> = FREQS.iter().map(|hz| format_freq(*hz)).collect();
        assert_eq!(
            labels,
            [
                "32", "64", "125", "250", "500", "1k", "2k", "4k", "8k", "16k"
            ]
        );
        assert_eq!(format_freq(1500), "1,5k");
    }
}
