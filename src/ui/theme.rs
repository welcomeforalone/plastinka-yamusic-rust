//! Цвета, шрифты и стили текста. Облик с этапа 6.5 — чёрный фон, нейтральные
//! серые, акцент на выбор пользователя (`docs/ui/stage-6.5.md`).

use std::borrow::Cow;
use std::sync::atomic::{AtomicU32, Ordering};

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
/// Рост в топе месяца.
pub const GROWTH: Color = rgb(0x4CD07D);
pub const ERROR_SOFT: Color = rgba(0xFF6B6B, 0.14);
pub const ERROR_HOVER: Color = rgba(0xFF6B6B, 0.24);
pub const ERROR_PRESSED: Color = rgba(0xFF6B6B, 0.34);
pub const SCRIM: Color = rgba(0x000000, 0.72);
/// Кнопка «закрыть» в заголовке окна при наведении, как у системных окон.
pub const CLOSE_HOVER: Color = rgb(0xC42B1C);

// ---------------------------------------------------------------- акцент

/// Акцентный цвет. Выбирается в «Настройки → Внешний вид», хранится в
/// `config.toml` ключом [`Accent::key`]: готовый — именем (`red`), свой —
/// записью `#RRGGBB`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Accent {
    #[default]
    Red,
    Teal,
    Orange,
    Violet,
    Blue,
    /// Свой цвет, `0xRRGGBB`. Строить через [`Accent::custom`]: оно обрезает
    /// старшие биты, чтобы равные цвета были равны и по `==`.
    Custom(u32),
}

/// Маска цвета `0xRRGGBB`.
const RGB_MASK: u32 = 0x00FF_FFFF;
/// Признак своего цвета в числе, которое лежит в [`ACCENT`].
const CUSTOM_FLAG: u32 = 1 << 24;

impl Accent {
    /// Готовые акценты, по порядку в настройках.
    pub const PRESETS: [Self; 5] = [
        Self::Red,
        Self::Teal,
        Self::Orange,
        Self::Violet,
        Self::Blue,
    ];

    pub fn custom(rgb: u32) -> Self {
        Self::Custom(rgb & RGB_MASK)
    }

    /// Ключ для `config.toml`: имя готового акцента или `#RRGGBB`.
    pub fn key(self) -> Cow<'static, str> {
        match self {
            Self::Red => Cow::Borrowed("red"),
            Self::Teal => Cow::Borrowed("teal"),
            Self::Orange => Cow::Borrowed("orange"),
            Self::Violet => Cow::Borrowed("violet"),
            Self::Blue => Cow::Borrowed("blue"),
            Self::Custom(rgb) => Cow::Owned(format_hex(rgb)),
        }
    }

    /// Акцент по ключу из настроек. Непонятный ключ — `None`.
    pub fn from_key(key: &str) -> Option<Self> {
        let key = key.trim();
        Self::PRESETS
            .into_iter()
            .find(|accent| accent.key().eq_ignore_ascii_case(key))
            .or_else(|| parse_hex(key).map(Self::custom))
    }

    pub fn title(self) -> Cow<'static, str> {
        match self {
            Self::Red => Cow::Borrowed("Красный"),
            Self::Teal => Cow::Borrowed("Бирюзовый"),
            Self::Orange => Cow::Borrowed("Оранжевый"),
            Self::Violet => Cow::Borrowed("Фиолетовый"),
            Self::Blue => Cow::Borrowed("Синий"),
            Self::Custom(rgb) => Cow::Owned(format!("Свой цвет {}", format_hex(rgb))),
        }
    }

    /// Цвет как число `0xRRGGBB`.
    pub fn rgb(self) -> u32 {
        match self {
            Self::Red => 0xFA2D48,
            Self::Teal => 0x3DD6C3,
            Self::Orange => 0xFF8A3D,
            Self::Violet => 0x9D7BFF,
            Self::Blue => 0x3D8BFF,
            Self::Custom(rgb) => rgb & RGB_MASK,
        }
    }

    pub fn color(self) -> Color {
        rgb(self.rgb())
    }

    /// Текст и иконки на акценте: белые на тёмных акцентах, почти чёрные на
    /// светлых.
    fn on(self) -> Color {
        contrast_on(self.color())
    }

    /// Число для [`ACCENT`]: номер готового акцента или цвет с признаком.
    fn encode(self) -> u32 {
        match self {
            Self::Red => 0,
            Self::Teal => 1,
            Self::Orange => 2,
            Self::Violet => 3,
            Self::Blue => 4,
            Self::Custom(rgb) => CUSTOM_FLAG | (rgb & RGB_MASK),
        }
    }

    fn decode(value: u32) -> Self {
        if value & CUSTOM_FLAG != 0 {
            return Self::custom(value);
        }
        Self::PRESETS
            .into_iter()
            .find(|accent| accent.encode() == value)
            .unwrap_or_default()
    }
}

/// Цвет текста и значков на фоне `color`: белый на тёмном, почти чёрный на
/// светлом.
pub fn contrast_on(color: Color) -> Color {
    let luma = 0.299 * color.r + 0.587 * color.g + 0.114 * color.b;
    if luma > 0.6 {
        rgb(0x0A0A0A)
    } else {
        rgb(0xFFFFFF)
    }
}

/// Разбор `#RRGGBB`: пробелы по краям и регистр не важны, `#` обязателен.
pub fn parse_hex(text: &str) -> Option<u32> {
    let digits = text.trim().strip_prefix('#')?;
    if digits.len() != 6 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(digits, 16).ok()
}

/// Запись цвета `#RRGGBB` прописными.
pub fn format_hex(rgb: u32) -> String {
    format!("#{:06X}", rgb & RGB_MASK)
}

/// Текущий акцент. Стили iced рисуются из многих мест (замыкания стилей,
/// цвета иконок), поэтому значение общее для всего окна, а не передаётся
/// каждому виду. Меняет его только `App` (настройки и их загрузка). Одно
/// число без блокировок: номер готового акцента или свой цвет с признаком.
static ACCENT: AtomicU32 = AtomicU32::new(0);

pub fn set_accent(accent: Accent) {
    ACCENT.store(accent.encode(), Ordering::Relaxed);
}

pub fn current_accent() -> Accent {
    Accent::decode(ACCENT.load(Ordering::Relaxed))
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
        for accent in Accent::PRESETS {
            assert_eq!(Accent::from_key(&accent.key()), Some(accent));
        }
        let custom = Accent::custom(0x12AB9F);
        assert_eq!(Accent::from_key(&custom.key()), Some(custom));
        assert_eq!(custom.key(), "#12AB9F");
        assert_eq!(Accent::from_key(" RED "), Some(Accent::Red));
        assert_eq!(Accent::from_key("фиолетовый"), None);
        assert_eq!(Accent::from_key(""), None);
        assert_eq!(
            Accent::from_key("  #fa2d48 "),
            Some(Accent::Custom(0xFA2D48))
        );
        // Без `#`, не та длина и не шестнадцатеричные цифры — не цвет.
        assert_eq!(Accent::from_key("FA2D48"), None);
        assert_eq!(Accent::from_key("#FA2D4"), None);
        assert_eq!(Accent::from_key("#FA2D488"), None);
        assert_eq!(Accent::from_key("#GG2D48"), None);
        assert_eq!(Accent::from_key("#+A2D48"), None);
    }

    #[test]
    fn hex_parse_and_format() {
        assert_eq!(parse_hex("#000000"), Some(0));
        assert_eq!(parse_hex("#ffffff"), Some(0xFFFFFF));
        assert_eq!(parse_hex("#"), None);
        assert_eq!(parse_hex("#ЯЯЯ"), None);
        assert_eq!(format_hex(0), "#000000");
        assert_eq!(format_hex(0xABCDEF), "#ABCDEF");
        // Лишние старшие биты отбрасываются.
        assert_eq!(format_hex(0xFF12_3456), "#123456");
    }

    #[test]
    fn accent_encoding_round_trips() {
        let all = Accent::PRESETS.into_iter().chain([
            Accent::custom(0),
            Accent::custom(0xFFFFFF),
            Accent::custom(7),
        ]);
        for accent in all {
            assert_eq!(Accent::decode(accent.encode()), accent);
        }
        assert_eq!(Accent::custom(0x1FF0000), Accent::Custom(0xFF0000));
        // Неизвестный номер — акцент по умолчанию.
        assert_eq!(Accent::decode(99), Accent::default());
    }

    #[test]
    fn contrast_follows_luma() {
        assert_eq!(Accent::Red.on(), rgb(0xFFFFFF));
        assert_eq!(Accent::Teal.on(), rgb(0x0A0A0A));
        assert_eq!(Accent::custom(0x000000).on(), rgb(0xFFFFFF));
        assert_eq!(Accent::custom(0xFFFFFF).on(), rgb(0x0A0A0A));
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
