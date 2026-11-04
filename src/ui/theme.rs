//! Цвета, шрифты и стили текста из `design/tokens.md`.

use iced::font::{self, Font};
use iced::gradient::Linear;
use iced::theme::Palette;
use iced::widget::text::{self, LineHeight, Text, Wrapping};
use iced::{Background, Color, Degrees, Pixels, Theme};

const fn rgb(hex: u32) -> Color {
    let [_, r, g, b] = hex.to_be_bytes();
    Color::from_rgb8(r, g, b)
}

const fn rgba(hex: u32, alpha: f32) -> Color {
    let [_, r, g, b] = hex.to_be_bytes();
    Color::from_rgba8(r, g, b, alpha)
}

pub const BG: Color = rgb(0x0D0E12);
pub const SURFACE_1: Color = rgb(0x15171C);
pub const SURFACE_2: Color = rgb(0x1C1F26);
pub const SURFACE_3: Color = rgb(0x252932);
pub const SURFACE_4: Color = rgb(0x2F343E);
pub const BORDER: Color = rgb(0x2A2E37);
pub const TEXT_PRIMARY: Color = rgb(0xF1F2F4);
pub const TEXT_SECONDARY: Color = rgb(0xA4A9B4);
pub const TEXT_TERTIARY: Color = rgb(0x8A909C);
pub const TEXT_DISABLED: Color = rgb(0x5B606B);
pub const ACCENT: Color = rgb(0x3DD6C3);
pub const ACCENT_HOVER: Color = rgb(0x5FE0D0);
pub const ACCENT_PRESSED: Color = rgb(0x2DB8A7);
pub const ON_ACCENT: Color = rgb(0x04201C);
pub const ACCENT_SOFT: Color = rgba(0x3DD6C3, 0.14);
pub const ERROR: Color = rgb(0xFF6B6B);
pub const ERROR_SOFT: Color = rgba(0xFF6B6B, 0.14);
pub const ERROR_HOVER: Color = rgba(0xFF6B6B, 0.24);
pub const ERROR_PRESSED: Color = rgba(0xFF6B6B, 0.34);
pub const SCRIM: Color = rgba(0x000000, 0.64);
/// Кнопка «закрыть» в заголовке окна при наведении, как у системных окон.
pub const CLOSE_HOVER: Color = rgb(0xC42B1C);

/// Inter Regular и Semi Bold: кладутся в бинарник.
pub const FONTS: [&[u8]; 2] = [
    include_bytes!("../../assets/fonts/Inter-Regular.ttf"),
    include_bytes!("../../assets/fonts/Inter-SemiBold.ttf"),
];

pub const REGULAR: Font = Font::with_name("Inter");
pub const SEMIBOLD: Font = Font {
    weight: font::Weight::Semibold,
    ..REGULAR
};

/// Тема iced: нужна для стилей по умолчанию (поля ввода, подсказки).
pub fn theme() -> Theme {
    Theme::custom(
        "Plastinka",
        Palette {
            background: BG,
            text: TEXT_PRIMARY,
            primary: ACCENT,
            success: ACCENT,
            warning: ACCENT_HOVER,
            danger: ERROR,
        },
    )
}

/// Стили текста по ролям из `tokens.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Display,
    Title,
    Headline,
    Body,
    BodyStrong,
    Secondary,
    Caption,
    CaptionStrong,
    Code,
}

impl Style {
    pub(crate) fn metrics(self) -> (f32, f32, Font) {
        match self {
            Self::Display => (32.0, 40.0, SEMIBOLD),
            Self::Title => (22.0, 28.0, SEMIBOLD),
            Self::Headline => (16.0, 22.0, SEMIBOLD),
            Self::Body => (14.0, 20.0, REGULAR),
            Self::BodyStrong => (14.0, 20.0, SEMIBOLD),
            Self::Secondary => (13.0, 18.0, REGULAR),
            Self::Caption => (12.0, 16.0, REGULAR),
            Self::CaptionStrong => (12.0, 16.0, SEMIBOLD),
            Self::Code => (36.0, 44.0, SEMIBOLD),
        }
    }
}

/// Текст в стиле `style` и цвете `color`.
pub fn label<'a>(content: impl text::IntoFragment<'a>, style: Style, color: Color) -> Text<'a> {
    let (size, line, font) = style.metrics();
    iced::widget::text(content)
        .size(size)
        .line_height(LineHeight::Absolute(Pixels(line)))
        .font(font)
        .color(color)
}

/// Однострочный текст: длинный обрезается по ширине (многоточия в iced нет).
pub fn line<'a>(content: impl text::IntoFragment<'a>, style: Style, color: Color) -> Text<'a> {
    label(content, style, color).wrapping(Wrapping::None)
}

/// Заглушки обложек: градиент из левого верхнего угла в правый нижний.
const COVER_GRADIENTS: [(u32, u32); 12] = [
    (0x5B4BDB, 0xC15BD8),
    (0x1F8A70, 0x9AD94B),
    (0xE2574C, 0xF2A541),
    (0x2A3B8F, 0x4FB3E8),
    (0x8E2D5A, 0xF06C9B),
    (0x3C3C46, 0x8C8C99),
    (0x0E7C86, 0x57E0C6),
    (0xB5651D, 0xF4D35E),
    (0x6A1B9A, 0xF48FB1),
    (0x263238, 0x4DB6AC),
    (0xC62828, 0x6A1B9A),
    (0x1565C0, 0x00ACC1),
];

/// Диагональный градиент между двумя цветами.
pub fn diagonal(from: Color, to: Color) -> Background {
    // В iced угол 0° — снизу вверх, 135° — из левого верхнего угла в правый нижний.
    Background::Gradient(
        Linear::new(Degrees(135.0))
            .add_stop(0.0, from)
            .add_stop(1.0, to)
            .into(),
    )
}

/// Заглушка обложки, одна и та же для одного `seed`.
pub fn cover_placeholder(seed: &str) -> Background {
    // Сумма байтов: устойчиво между запусками и версиями Rust.
    let sum = seed.bytes().fold(0usize, |acc, byte| {
        acc.wrapping_mul(31).wrapping_add(usize::from(byte))
    });
    let (from, to) = COVER_GRADIENTS
        .get(sum % COVER_GRADIENTS.len())
        .copied()
        .unwrap_or((0x3C3C46, 0x8C8C99));
    diagonal(rgb(from), rgb(to))
}

/// Плитка «Мне нравится» в шапке.
pub fn liked_tile() -> Background {
    diagonal(ACCENT, rgb(0x1F5E8B))
}

/// Аватар аккаунта.
pub fn avatar() -> Background {
    diagonal(rgb(0x2A3B8F), rgb(0x4FB3E8))
}
