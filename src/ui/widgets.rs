//! Общие элементы окна: кнопки, карточки, логотип, обложки, плашки.

use iced::widget::{
    Space, button, center, column, container, hover, image, row, stack, text, tooltip,
};
use iced::{Alignment, Background, Border, Color, Element, Length, Padding, Shadow, Vector};

use super::Message;
use super::anim;
use super::icons::{Icon, icon, spinner};
use super::theme::{self, Style};

/// Скругления из `tokens.md`.
pub const RADIUS_S: f32 = 6.0;
pub const RADIUS_M: f32 = 10.0;
pub const RADIUS_XL: f32 = 20.0;
pub const ROUND: f32 = 999.0;

/// Вид кнопки с подписью.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Primary,
    Secondary,
    Danger,
}

impl Kind {
    /// Цвет подписи и иконки.
    fn content(self, enabled: bool) -> Color {
        if !enabled {
            return theme::text_disabled();
        }
        match self {
            Self::Primary => theme::on_accent(),
            Self::Secondary => theme::text_primary(),
            Self::Danger => theme::error(),
        }
    }

    fn background(self, status: button::Status) -> Option<Color> {
        use button::Status::{Active, Disabled, Hovered, Pressed};
        match (self, status) {
            (Self::Primary, Active) => Some(theme::accent()),
            (Self::Primary, Hovered) => Some(theme::accent_hover()),
            (Self::Primary, Pressed) => Some(theme::accent_pressed()),
            // «Таблетка» с рамкой: без заливки, при наведении — лёгкий фон.
            (Self::Secondary, Active) => None,
            (Self::Secondary, Hovered) => Some(theme::surface_3()),
            (Self::Secondary, Pressed) => Some(theme::surface_4()),
            (Self::Danger, Active) => Some(theme::error_soft()),
            (Self::Danger, Hovered) => Some(theme::error_hover()),
            (Self::Danger, Pressed) => Some(theme::error_pressed()),
            (Self::Secondary, Disabled) => None,
            (_, Disabled) => Some(theme::surface_2()),
        }
    }

    fn border(self, status: button::Status) -> Color {
        match (self, status) {
            (Self::Secondary, button::Status::Disabled) => theme::surface_3(),
            (Self::Secondary, _) => theme::outline(),
            _ => Color::TRANSPARENT,
        }
    }
}

/// Стиль кнопки. Фон в покое и при наведении рисует [`anim::hover_fill`]
/// (плавно), здесь — только нажатие и недоступность.
fn button_style(kind: Kind, status: button::Status) -> button::Style {
    let background = match status {
        button::Status::Pressed | button::Status::Disabled => kind.background(status),
        button::Status::Active | button::Status::Hovered => None,
    };
    button::Style {
        background: background.map(Background::Color),
        text_color: kind.content(status != button::Status::Disabled),
        border: Border {
            color: kind.border(status),
            width: if kind == Kind::Secondary { 1.0 } else { 0.0 },
            radius: ROUND.into(),
        },
        shadow: Shadow::default(),
        snap: true,
    }
}

/// Кнопка-«таблетка» с подписью и необязательной иконкой, высота 40.
/// `on_press: None` — кнопка недоступна.
pub fn action<'a>(
    kind: Kind,
    leading: Option<Icon>,
    caption: &'a str,
    on_press: Option<Message>,
) -> Element<'a, Message> {
    let color = kind.content(on_press.is_some());
    action_with(
        kind,
        leading.map(|name| icon(name, 18.0, color).into()),
        caption,
        on_press,
    )
}

/// Кнопка с подписью на всю доступную ширину, содержимое по центру.
pub fn wide_action<'a>(
    kind: Kind,
    leading: Option<Icon>,
    caption: &'a str,
    on_press: Option<Message>,
) -> Element<'a, Message> {
    let color = kind.content(on_press.is_some());
    action_button(
        kind,
        leading.map(|name| icon(name, 18.0, color).into()),
        caption,
        on_press,
        true,
    )
}

/// Кнопка, у которой вместо иконки свой элемент (например, спиннер).
pub fn action_with<'a>(
    kind: Kind,
    leading: Option<Element<'a, Message>>,
    caption: &'a str,
    on_press: Option<Message>,
) -> Element<'a, Message> {
    action_button(kind, leading, caption, on_press, false)
}

/// Широкая кнопка со своим элементом вместо иконки.
pub fn wide_action_with<'a>(
    kind: Kind,
    leading: Option<Element<'a, Message>>,
    caption: &'a str,
    on_press: Option<Message>,
) -> Element<'a, Message> {
    action_button(kind, leading, caption, on_press, true)
}

fn action_button<'a>(
    kind: Kind,
    leading: Option<Element<'a, Message>>,
    caption: &'a str,
    on_press: Option<Message>,
    wide: bool,
) -> Element<'a, Message> {
    let color = kind.content(on_press.is_some());
    let mut content = row![].spacing(8).align_y(Alignment::Center);
    let left = if leading.is_some() { 16.0 } else { 18.0 };
    if let Some(leading) = leading {
        content = content.push(leading);
    }
    content = content.push(theme::line(caption, Style::BodyStrong, color));
    let content = container(content).center_y(Length::Fill);
    let content = if wide {
        content.center_x(Length::Fill)
    } else {
        content
    };
    let enabled = on_press.is_some();
    let button = button(content)
        .width(if wide { Length::Fill } else { Length::Shrink })
        .height(40)
        .padding(Padding {
            top: 0.0,
            right: 18.0,
            bottom: 0.0,
            left,
        })
        .style(move |_, status| button_style(kind, status))
        .on_press_maybe(on_press);
    if !enabled {
        return button.into();
    }
    let idle = kind.background(button::Status::Active);
    let hovered = kind.background(button::Status::Hovered);
    let (idle, hovered) = match (idle, hovered) {
        (Some(idle), Some(hovered)) => (idle, hovered),
        (None, Some(hovered)) => (theme::clear(hovered), hovered),
        (Some(idle), None) => (idle, theme::clear(idle)),
        (None, None) => (Color::TRANSPARENT, Color::TRANSPARENT),
    };
    anim::hover_fill(button, idle, hovered, ROUND).into()
}

/// Размер круглой иконочной кнопки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    /// 40 px, иконка 20.
    M,
    /// 32 px, иконка 18.
    S,
}

/// Круглая кнопка с иконкой. `active` — включённая функция (акцент).
/// Без `on_press` кнопка недоступна.
pub fn icon_button<'a>(
    name: Icon,
    size: Size,
    active: bool,
    on_press: Option<Message>,
) -> Element<'a, Message> {
    let (side, glyph) = match size {
        Size::M => (40.0, 20.0),
        Size::S => (32.0, 18.0),
    };
    let enabled = on_press.is_some();
    let make = move |hovered: bool, on_press: Option<Message>| -> Element<'a, Message> {
        let color = match (enabled, active, hovered) {
            (false, _, _) => theme::text_disabled(),
            (true, true, false) => theme::accent(),
            (true, true, true) => theme::accent_hover(),
            (true, false, false) => theme::text_secondary(),
            (true, false, true) => theme::text_primary(),
        };
        button(center(icon(name, glyph, color)))
            .width(side)
            .height(side)
            .padding(0)
            .style(move |_, status| button::Style {
                background: match status {
                    button::Status::Pressed => Some(theme::surface_4().into()),
                    _ => None,
                },
                border: Border::default().rounded(ROUND),
                ..button::Style::default()
            })
            .on_press_maybe(on_press)
            .into()
    };
    if enabled {
        anim::hover_fill(
            hover(make(false, None), make(true, on_press)),
            theme::clear(theme::surface_3()),
            theme::surface_3(),
            ROUND,
        )
        .into()
    } else {
        make(false, None)
    }
}

/// Подсказка при наведении.
pub fn with_tooltip<'a>(
    content: impl Into<Element<'a, Message>>,
    tip: impl iced::widget::text::IntoFragment<'a>,
    position: tooltip::Position,
) -> Element<'a, Message> {
    tooltip(
        content,
        container(theme::line(tip, Style::Caption, theme::text_primary()))
            .padding([6, 10])
            .style(|_| container::Style {
                background: Some(theme::surface_3().into()),
                border: Border::default().rounded(RADIUS_S),
                shadow: raised_shadow(),
                ..container::Style::default()
            }),
        position,
    )
    .gap(6)
    .into()
}

/// `Shadow/Raised`.
pub fn raised_shadow() -> Shadow {
    Shadow {
        color: Color::from_rgba(0.0, 0.0, 0.0, theme::shadow_alpha(0.35)),
        offset: Vector::new(0.0, 4.0),
        blur_radius: 12.0,
    }
}

/// `Shadow/Popover`.
pub fn popover_shadow() -> Shadow {
    Shadow {
        color: Color::from_rgba(0.0, 0.0, 0.0, theme::shadow_alpha(0.5)),
        offset: Vector::new(0.0, 12.0),
        blur_radius: 32.0,
    }
}

/// Карточка всплывающей панели со скруглением `radius`: фон, рамка и тень с
/// общей непрозрачностью `alpha` (для появления и ухода панели).
pub fn popover_card(alpha: f32, radius: f32) -> container::Style {
    let alpha = alpha.clamp(0.0, 1.0);
    let mut shadow = popover_shadow();
    shadow.color.a *= alpha;
    container::Style {
        background: Some(theme::alpha(theme::surface_2(), alpha).into()),
        border: Border::default()
            .rounded(radius)
            .width(1)
            .color(theme::alpha(theme::border(), alpha)),
        shadow,
        ..container::Style::default()
    }
}

/// Сплошной прямоугольник: фон, разделитель, полоса скелетона.
pub fn block<'a>(
    width: impl Into<Length>,
    height: impl Into<Length>,
    color: Color,
    radius: f32,
) -> Element<'a, Message> {
    container(Space::new())
        .width(width)
        .height(height)
        .style(move |_| container::Style {
            background: Some(color.into()),
            border: Border::default().rounded(radius),
            ..container::Style::default()
        })
        .into()
}

/// Горизонтальный разделитель 1 px.
pub fn divider<'a>() -> Element<'a, Message> {
    block(Length::Fill, 1, theme::border(), 0.0)
}

/// Вертикальный разделитель 1 px.
pub fn vertical_divider<'a>() -> Element<'a, Message> {
    block(1, Length::Fill, theme::border(), 0.0)
}

/// Тень большой обложки в шапке.
pub fn cover_shadow() -> Shadow {
    Shadow {
        color: Color::from_rgba(0.0, 0.0, 0.0, theme::shadow_alpha(0.5)),
        offset: Vector::new(0.0, 8.0),
        blur_radius: 24.0,
    }
}

/// Карточка входа и подписки: 480 px, `surface-1`, рамка, радиус 20.
pub fn card<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(content)
        .width(480)
        .padding(40)
        .style(|_| container::Style {
            background: Some(theme::surface_1().into()),
            border: Border {
                color: theme::border(),
                width: 1.0,
                radius: RADIUS_XL.into(),
            },
            ..container::Style::default()
        })
        .into()
}

/// Круг 64 px с иконкой 28 px: заголовок карточки.
pub fn badge<'a>(name: Icon, color: Color, background: Color) -> Element<'a, Message> {
    container(center(icon(name, 28.0, color)))
        .width(64)
        .height(64)
        .style(move |_| container::Style {
            background: Some(background.into()),
            border: Border::default().rounded(ROUND),
            ..container::Style::default()
        })
        .into()
}

/// Плашка с иконкой и текстом: ошибка (`error-soft`) или пояснение (`surface-2`).
pub fn notice<'a>(name: Icon, message: String, error: bool) -> Element<'a, Message> {
    let (icon_color, text_color, background) = if error {
        (theme::error(), theme::text_primary(), theme::error_soft())
    } else {
        (
            theme::text_secondary(),
            theme::text_secondary(),
            theme::surface_2(),
        )
    };
    container(
        row![
            icon(name, 20.0, icon_color),
            theme::label(message, Style::Secondary, text_color).width(Length::Fill),
        ]
        .spacing(10)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding([12, 14])
    .style(move |_| container::Style {
        background: Some(background.into()),
        border: Border::default().rounded(RADIUS_M),
        ..container::Style::default()
    })
    .into()
}

/// Логотип: пластинка с бирюзовым центром. Диаметр `size`.
pub fn logo<'a>(size: f32) -> Element<'a, Message> {
    let disc =
        |diameter: f32, fill: Option<Color>, stroke: Option<Color>| -> Element<'a, Message> {
            center(
                container(Space::new())
                    .width(diameter)
                    .height(diameter)
                    .style(move |_| container::Style {
                        background: fill.map(Background::Color),
                        border: Border {
                            color: stroke.unwrap_or(Color::TRANSPARENT),
                            width: if stroke.is_some() { 1.0 } else { 0.0 },
                            radius: ROUND.into(),
                        },
                        ..container::Style::default()
                    }),
            )
            .into()
        };
    let scale = size / 28.0;
    container(stack![
        disc(
            size,
            Some(Color::from_rgb8(0x08, 0x09, 0x0B)),
            Some(theme::surface_4())
        ),
        disc(22.0 * scale, None, Some(theme::surface_3())),
        disc(17.0 * scale, None, Some(theme::surface_3())),
        disc(11.0 * scale, Some(theme::accent()), None),
        disc((3.0 * scale).max(2.0), Some(theme::bg()), None),
    ])
    .width(size)
    .height(size)
    .into()
}

/// Обложка: картинка, заглушка-градиент или `surface-3`, пока грузится.
pub enum CoverArt<'h> {
    /// Картинка и её непрозрачность.
    Image(&'h image::Handle, f32),
    Placeholder(Background),
    Loading,
}

/// Картинка обложки без подложки.
pub fn picture<'a>(
    handle: &image::Handle,
    size: f32,
    radius: f32,
    opacity: f32,
) -> Element<'a, Message> {
    image(handle.clone())
        .width(size)
        .height(size)
        .border_radius(radius)
        .opacity(opacity)
        .into()
}

pub fn cover<'a>(art: CoverArt<'_>, size: f32, radius: f32) -> Element<'a, Message> {
    match art {
        CoverArt::Image(handle, opacity) => {
            let picture = picture(handle, size, radius, opacity);
            if opacity >= 1.0 {
                picture
            } else {
                // Свежая обложка проявляется поверх заглушки загрузки.
                stack![block(size, size, theme::surface_3(), radius), picture].into()
            }
        }
        CoverArt::Placeholder(background) => container(Space::new())
            .width(size)
            .height(size)
            .style(move |_| container::Style {
                background: Some(background),
                border: Border::default().rounded(radius),
                ..container::Style::default()
            })
            .into(),
        CoverArt::Loading => block(size, size, theme::surface_3(), radius),
    }
}

/// Строка «спиннер + подпись».
pub fn waiting<'a>(caption: String, turn: f32) -> Element<'a, Message> {
    row![
        spinner(16.0, theme::accent(), turn),
        theme::line(caption, Style::Secondary, theme::text_secondary()),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

/// Текстовая ссылка акцентом.
pub fn link<'a>(caption: String, style: Style, on_press: Message) -> Element<'a, Message> {
    let font = match style {
        Style::BodyStrong | Style::CaptionStrong => theme::SEMIBOLD,
        _ => theme::REGULAR,
    };
    button(text(caption).font(font).size(match style {
        Style::Caption | Style::CaptionStrong => 12.0,
        Style::Secondary => 13.0,
        _ => 14.0,
    }))
    .padding(0)
    .style(|_, status| button::Style {
        background: None,
        text_color: if status == button::Status::Hovered {
            theme::accent_hover()
        } else {
            theme::accent()
        },
        ..button::Style::default()
    })
    .on_press(on_press)
    .into()
}

/// Заголовок и текст карточки по центру.
pub fn heading<'a>(title: String, body: String) -> Element<'a, Message> {
    column![
        theme::label(title, Style::Title, theme::text_primary()).center(),
        theme::label(body, Style::Body, theme::text_secondary())
            .center()
            .width(Length::Fill),
    ]
    .spacing(12)
    .align_x(Alignment::Center)
    .into()
}
