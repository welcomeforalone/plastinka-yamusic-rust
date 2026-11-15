//! Элементы экрана настроек: шапка страницы, строка настройки, переключатель,
//! сегменты, ползунок со значением и поле ввода.

use iced::widget::{Space, button, column, container, row, slider, stack, text_input, toggler};
use iced::{Alignment, Border, Color, Element, Length, Padding};

use crate::ui::Message as App;
use crate::ui::icons::{Icon, icon};
use crate::ui::theme::{self, Style};
use crate::ui::widgets::{self, Size};

/// Радиус полей и сегментов.
const RADIUS: f32 = 12.0;
/// Высота поля ввода.
const FIELD_HEIGHT: f32 = 44.0;
/// Подпись со значением слева от ползунка (вмещает «−12,0 дБ»).
const VALUE_WIDTH: f32 = 72.0;

/// Шапка страницы: название прописными, пояснение и разделитель.
pub fn header<'a>(title: &str, summary: &'a str) -> Element<'a, App> {
    column![
        theme::line(
            title.to_uppercase(),
            Style::CaptionStrong,
            theme::TEXT_PRIMARY
        ),
        theme::line(summary, Style::Secondary, theme::TEXT_TERTIARY),
        Space::new().height(12),
        widgets::divider(),
    ]
    .spacing(4)
    .into()
}

/// Строка настройки: заголовок с пояснением слева, управление справа.
pub fn setting_row<'a>(
    title: &'a str,
    hint: String,
    control: Element<'a, App>,
) -> Element<'a, App> {
    padded_row(title, hint, control, Padding::from([16, 0]))
}

/// Та же строка с другими полями (во всплывающих панелях).
pub fn padded_row<'a>(
    title: &'a str,
    hint: String,
    control: Element<'a, App>,
    padding: Padding,
) -> Element<'a, App> {
    row![
        column![
            theme::line(title, Style::BodyStrong, theme::TEXT_PRIMARY),
            theme::label(hint, Style::Secondary, theme::TEXT_TERTIARY),
        ]
        .spacing(2)
        .width(Length::Fill),
        control,
    ]
    .spacing(16)
    .align_y(Alignment::Center)
    .padding(padding)
    .into()
}

/// Переключатель настройки в цветах акцента.
pub fn switch<'a>(on: bool, on_toggle: impl Fn(bool) -> App + 'a) -> Element<'a, App> {
    toggler(on)
        .on_toggle(on_toggle)
        .size(22)
        .style(|_, status| {
            let (on, hovered) = match status {
                toggler::Status::Active { is_toggled } => (is_toggled, false),
                toggler::Status::Hovered { is_toggled } => (is_toggled, true),
                toggler::Status::Disabled { is_toggled } => (is_toggled, false),
            };
            toggler::Style {
                background: match (on, hovered) {
                    (true, false) => theme::accent(),
                    (true, true) => theme::accent_hover(),
                    (false, false) => theme::SURFACE_4,
                    (false, true) => theme::OUTLINE,
                }
                .into(),
                background_border_width: 0.0,
                background_border_color: Color::TRANSPARENT,
                foreground: (if on {
                    theme::on_accent()
                } else {
                    theme::TEXT_SECONDARY
                })
                .into(),
                foreground_border_width: 0.0,
                foreground_border_color: Color::TRANSPARENT,
                text_color: None,
                border_radius: None,
                padding_ratio: 0.12,
            }
        })
        .into()
}

/// Сегменты: выбор одного из нескольких значений в общей плашке.
// Пока не используются: понадобятся разделам этапа 11.
#[allow(dead_code)]
pub fn segments<'a, T>(
    items: impl IntoIterator<Item = (T, &'a str)>,
    selected: T,
    on_select: impl Fn(T) -> App,
) -> Element<'a, App>
where
    T: Copy + PartialEq + 'a,
{
    let buttons = items.into_iter().map(|(value, title)| {
        let chosen = value == selected;
        let (font, color) = if chosen {
            (Style::BodyStrong, theme::TEXT_PRIMARY)
        } else {
            (Style::Body, theme::TEXT_SECONDARY)
        };
        button(container(theme::line(title, font, color)).center_y(Length::Fill))
            .height(36)
            .padding([0, 16])
            .style(move |_, status| button::Style {
                background: match (chosen, status) {
                    (true, _) => Some(theme::SURFACE_3.into()),
                    (false, button::Status::Hovered | button::Status::Pressed) => {
                        Some(theme::SURFACE_2.into())
                    }
                    _ => None,
                },
                border: Border::default().rounded(RADIUS - 2.0),
                ..button::Style::default()
            })
            .on_press(on_select(value))
            .into()
    });
    container(row(buttons).spacing(2))
        .padding(2)
        .style(|_| container::Style {
            background: Some(theme::SURFACE_1.into()),
            border: Border::default().rounded(RADIUS),
            ..container::Style::default()
        })
        .into()
}

/// Ползунок со значением: слева подпись `value`, справа ползунок на всю
/// оставшуюся ширину (ширину строки задаёт вызывающий). `step` — шаг
/// значения, `on_release` — когда ползунок отпустили (запись в `config.toml`).
pub fn value_slider<'a>(
    value_text: String,
    range: std::ops::RangeInclusive<f32>,
    value: f32,
    step: f32,
    on_change: impl Fn(f32) -> App + 'a,
    on_release: Option<App>,
) -> Element<'a, App> {
    let mut bar = slider(range, value, on_change);
    if let Some(message) = on_release {
        bar = bar.on_release(message);
    }
    let bar = bar
        .step(step)
        .width(Length::Fill)
        .height(16)
        .style(|_, _| slider::Style {
            rail: slider::Rail {
                backgrounds: (theme::accent().into(), theme::SURFACE_3.into()),
                width: 4.0,
                border: Border::default().rounded(2),
            },
            handle: slider::Handle {
                shape: slider::HandleShape::Circle { radius: 8.0 },
                background: theme::TEXT_PRIMARY.into(),
                border_width: 0.0,
                border_color: Color::TRANSPARENT,
            },
        });
    row![
        container(theme::line(value_text, Style::Body, theme::TEXT_SECONDARY))
            .width(VALUE_WIDTH)
            .align_x(Alignment::End),
        bar,
    ]
    .spacing(12)
    .width(Length::Fill)
    .align_y(Alignment::Center)
    .into()
}

/// Что показывает поле ввода.
pub struct Field<'a> {
    pub id: Option<&'static str>,
    pub placeholder: &'a str,
    pub value: &'a str,
    /// Иконка слева от текста.
    pub icon: Option<Icon>,
    pub width: Length,
}

/// Поле ввода: высота 44, радиус 12, в фокусе рамка акцентом. `on_clear` —
/// крестик справа, пока в поле есть текст.
pub fn text_field<'a>(
    field: Field<'a>,
    on_input: impl Fn(String) -> App + 'a,
    on_clear: Option<App>,
) -> Element<'a, App> {
    let clearable = on_clear.is_some() && !field.value.is_empty();
    let left = if field.icon.is_some() { 44.0 } else { 16.0 };
    let right = if clearable { 44.0 } else { 16.0 };
    let mut input: text_input::TextInput<'a, App> = text_input(field.placeholder, field.value)
        .on_input(on_input)
        .size(14)
        .line_height(iced::widget::text::LineHeight::Absolute(iced::Pixels(20.0)))
        .padding(Padding {
            top: 12.0,
            right,
            bottom: 12.0,
            left,
        })
        .width(field.width)
        .style(|_, status| text_input::Style {
            background: theme::SURFACE_1.into(),
            border: Border {
                color: match status {
                    text_input::Status::Focused { .. } => theme::accent(),
                    _ => theme::OUTLINE,
                },
                width: 1.0,
                radius: RADIUS.into(),
            },
            icon: theme::TEXT_TERTIARY,
            placeholder: theme::TEXT_TERTIARY,
            value: theme::TEXT_PRIMARY,
            selection: theme::alpha(theme::accent(), 0.35),
        });
    if let Some(id) = field.id {
        input = input.id(id);
    }
    let mut layers = stack![input].width(field.width).height(FIELD_HEIGHT);
    if let Some(name) = field.icon {
        let glyph: Element<'a, App> = icon(name, 18.0, theme::TEXT_TERTIARY).into();
        layers = layers.push(
            container(glyph)
                .height(FIELD_HEIGHT)
                .center_y(FIELD_HEIGHT)
                .padding(Padding::default().left(14)),
        );
    }
    if let (true, Some(message)) = (clearable, on_clear) {
        layers = layers.push(
            container(widgets::icon_button(
                Icon::Close,
                Size::S,
                false,
                Some(message),
            ))
            .width(Length::Fill)
            .height(FIELD_HEIGHT)
            .center_y(FIELD_HEIGHT)
            .align_x(Alignment::End)
            .padding(Padding::default().right(6)),
        );
    }
    layers.into()
}
