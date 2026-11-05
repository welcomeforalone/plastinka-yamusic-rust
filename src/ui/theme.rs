//! Цвета, шрифты и стили текста. Облик с этапа 6.5 — чёрный фон, нейтральные
//! серые, акцент на выбор пользователя (`docs/ui/stage-6.5.md`).

use std::sync::atomic::{AtomicU8, Ordering};

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

pub const BG: Color = rgb(0x000000);
pub const SURFACE_1: Color = rgb(0x0B0B0B);
pub const SURFACE_2: Color = rgb(0x141414);
pub const SURFACE_3: Color = rgb(0x1E1E1E);
pub const SURFACE_4: Color = rgb(0x2A2A2A);
/// Тонкие разделители: между меню и разделом, над плеером, под шапкой списка.
pub const BORDER: Color = rgb(0x1A1A1A);
/// Рамка кнопки-«таблетки» без заливки.
pub const OUTLINE: Color = rgb(0x333333);
pub const TEXT_PRIMARY: Color = rgb(0xF2F2F2);
pub const TEXT_SECONDARY: Color = rgb(0xA8A8A8);
pub const TEXT_TERTIARY: Color = rgb(0x8A8A8A);
pub const TEXT_DISABLED: Color = rgb(0x5A5A5A);
pub const ERROR: Color = rgb(0xFF6B6B);
pub const ERROR_SOFT: Color = rgba(0xFF6B6B, 0.14);
pub const ERROR_HOVER: Color = rgba(0xFF6B6B, 0.24);
pub const ERROR_PRESSED: Color = rgba(0xFF6B6B, 0.34);
pub const SCRIM: Color = rgba(0x000000, 0.72);
/// Кнопка «закрыть» в заголовке окна при наведении, как у системных окон.
pub const CLOSE_HOVER: Color = rgb(0xC42B1C);

// ---------------------------------------------------------------- акцент

/// Акцентный цвет. Выбирается в «Настройки → Внешний вид», хранится в
/// `config.toml` ключом [`Accent::key`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Accent {
    #[default]
    Red,
    Teal,
    Orange,
    Violet,
    Blue,
}

impl Accent {
    pub const ALL: [Self; 5] = [
        Self::Red,
        Self::Teal,
        Self::Orange,
        Self::Violet,
        Self::Blue,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Self::Red => "red",
            Self::Teal => "teal",
            Self::Orange => "orange",
            Self::Violet => "violet",
            Self::Blue => "blue",
        }
    }

    /// Акцент по ключу из настроек. Неизвестный ключ — `None`.
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|accent| accent.key().eq_ignore_ascii_case(key.trim()))
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Red => "Красный",
            Self::Teal => "Бирюзовый",
            Self::Orange => "Оранжевый",
            Self::Violet => "Фиолетовый",
            Self::Blue => "Синий",
        }
    }

    pub fn color(self) -> Color {
        rgb(match self {
            Self::Red => 0xFA2D48,
            Self::Teal => 0x3DD6C3,
            Self::Orange => 0xFF8A3D,
            Self::Violet => 0x9D7BFF,
            Self::Blue => 0x3D8BFF,
        })
    }

    /// Текст и иконки на акценте: белые на тёмных акцентах, почти чёрные на
    /// светлых.
    fn on(self) -> Color {
        match self {
            Self::Red | Self::Violet | Self::Blue => rgb(0xFFFFFF),
            Self::Teal | Self::Orange => rgb(0x0A0A0A),
        }
    }

    fn index(self) -> u8 {
        match self {
            Self::Red => 0,
            Self::Teal => 1,
            Self::Orange => 2,
            Self::Violet => 3,
            Self::Blue => 4,
        }
    }
}

/// Текущий акцент. Стили iced рисуются из многих мест (замыкания стилей,
/// цвета иконок), поэтому значение общее для всего окна, а не передаётся
/// каждому виду. Меняет его только `App` (настройки и их загрузка).
static ACCENT: AtomicU8 = AtomicU8::new(0);

pub fn set_accent(accent: Accent) {
    ACCENT.store(accent.index(), Ordering::Relaxed);
}

pub fn current_accent() -> Accent {
    let index = ACCENT.load(Ordering::Relaxed);
    Accent::ALL
        .into_iter()
        .find(|accent| accent.index() == index)
        .unwrap_or_default()
}

/// Смесь цветов: `t` = 0 — `from`, 1 — `to`. Альфа смешивается тоже.
pub fn mix(from: Color, to: Color, t: f32) -> Color {
    let t = if t.is_finite() {
        t.clamp(0.0, 1.0)
    } else {
        0.0
    };
    Color {
        r: from.r + (to.r - from.r) * t,
        g: from.g + (to.g - from.g) * t,
        b: from.b + (to.b - from.b) * t,
        a: from.a + (to.a - from.a) * t,
    }
}

/// Цвет с другой прозрачностью.
pub fn alpha(color: Color, a: f32) -> Color {
    Color { a, ..color }
}

/// Тот же цвет, полностью прозрачный: переход из него меняет только
/// прозрачность.
pub fn clear(color: Color) -> Color {
    alpha(color, 0.0)
}

pub fn accent() -> Color {
    current_accent().color()
}

pub fn accent_hover() -> Color {
    mix(accent(), Color::WHITE, 0.16)
}

pub fn accent_pressed() -> Color {
    mix(accent(), Color::BLACK, 0.16)
}

pub fn on_accent() -> Color {
    current_accent().on()
}

/// Фон выбранного пункта меню, плашки «Нужна подписка».
pub fn accent_soft() -> Color {
    alpha(accent(), 0.14)
}

// ---------------------------------------------------------------- шрифты

/// Inter Regular, Semi Bold, Bold и Extra Bold: кладутся в бинарник.
pub const FONTS: [&[u8]; 4] = [
    include_bytes!("../../assets/fonts/Inter-Regular.ttf"),
    include_bytes!("../../assets/fonts/Inter-SemiBold.ttf"),
    include_bytes!("../../assets/fonts/Inter-Bold.ttf"),
    include_bytes!("../../assets/fonts/Inter-ExtraBold.ttf"),
];

pub const REGULAR: Font = Font::with_name("Inter");
pub const SEMIBOLD: Font = Font {
    weight: font::Weight::Semibold,
    ..REGULAR
};
pub const BOLD: Font = Font {
    weight: font::Weight::Bold,
    ..REGULAR
};
pub const EXTRABOLD: Font = Font {
    weight: font::Weight::ExtraBold,
    ..REGULAR
};

/// Тема iced: нужна для стилей по умолчанию (поля ввода, подсказки).
pub fn theme() -> Theme {
    Theme::custom(
        "Plastinka",
        Palette {
            background: BG,
            text: TEXT_PRIMARY,
            primary: accent(),
            success: accent(),
            warning: accent_hover(),
            danger: ERROR,
        },
    )
}

/// Стили текста по ролям.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// Название коллекции в шапке: очень крупно и жирно.
    Hero,
    /// Заголовок экрана («Настройки»).
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
            Self::Hero => (56.0, 64.0, EXTRABOLD),
            Self::Display => (40.0, 48.0, EXTRABOLD),
            Self::Title => (22.0, 28.0, BOLD),
            Self::Headline => (16.0, 22.0, BOLD),
            Self::Body => (14.0, 20.0, REGULAR),
            Self::BodyStrong => (14.0, 20.0, SEMIBOLD),
            Self::Secondary => (13.0, 18.0, REGULAR),
            Self::Caption => (12.0, 16.0, REGULAR),
            Self::CaptionStrong => (12.0, 16.0, SEMIBOLD),
            Self::Code => (36.0, 44.0, BOLD),
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

/// Плитка «Мне нравится» в шапке: акцент, уходящий в тёмный.
pub fn liked_tile() -> Background {
    diagonal(accent(), mix(accent(), rgb(0x1A1033), 0.7))
}

/// Аватар аккаунта.
pub fn avatar() -> Background {
    diagonal(rgb(0x3A3A3A), rgb(0x6A6A6A))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accent_keys_round_trip() {
        for accent in Accent::ALL {
            assert_eq!(Accent::from_key(accent.key()), Some(accent));
        }
        assert_eq!(Accent::from_key(" RED "), Some(Accent::Red));
        assert_eq!(Accent::from_key("фиолетовый"), None);
        assert_eq!(Accent::from_key(""), None);
    }

    #[test]
    fn mix_clamps_and_survives_nan() {
        let black = Color::BLACK;
        let white = Color::WHITE;
        assert_eq!(mix(black, white, 2.0), white);
        assert_eq!(mix(black, white, -1.0), black);
        assert_eq!(mix(black, white, f32::NAN), black);
    }
}
