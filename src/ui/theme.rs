//! Цвета, шрифты и стили текста. Облик с этапа 6.5 — чёрный фон, нейтральные
//! серые, акцент на выбор пользователя (`docs/ui/stage-6.5.md`).

use std::borrow::Cow;
use std::cell::Cell;
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

// ---------------------------------------------------------------- схема

/// Тон схемы: тёмный или светлый.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tone {
    #[default]
    Dark,
    Light,
}

/// Режим тона в настройках: свой выбор или как в системе.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
    System,
}

impl ThemeMode {
    pub const ALL: [Self; 3] = [Self::Dark, Self::Light, Self::System];

    /// Ключ для `config.toml`.
    pub fn key(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
            Self::System => "system",
        }
    }

    /// Режим по ключу из настроек: пробелы и регистр не важны.
    pub fn from_key(key: &str) -> Option<Self> {
        let key = key.trim();
        Self::ALL
            .into_iter()
            .find(|mode| mode.key().eq_ignore_ascii_case(key))
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Dark => "Тёмная",
            Self::Light => "Светлая",
            Self::System => "Как в системе",
        }
    }

    /// Итоговый тон; `system` — тон, который сообщила система.
    pub fn tone(self, system: Tone) -> Tone {
        match self {
            Self::Dark => Tone::Dark,
            Self::Light => Tone::Light,
            Self::System => system,
        }
    }
}

impl Tone {
    /// Тон по режиму системы; если система его не сообщила — тёмный.
    pub fn from_system(mode: iced::theme::Mode) -> Self {
        match mode {
            iced::theme::Mode::Light => Self::Light,
            iced::theme::Mode::Dark | iced::theme::Mode::None => Self::Dark,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Dark => "тёмная",
            Self::Light => "светлая",
        }
    }
}

/// Цвета окна одной палитры: фон, поверхности, рамки и текст.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Colors {
    pub bg: Color,
    pub surface_1: Color,
    pub surface_2: Color,
    pub surface_3: Color,
    pub surface_4: Color,
    /// Тонкие разделители: между меню и разделом, над плеером, под шапкой списка.
    pub border: Color,
    /// Рамка кнопки-«таблетки» без заливки.
    pub outline: Color,
    pub text_primary: Color,
    pub text_secondary: Color,
    pub text_tertiary: Color,
    pub text_disabled: Color,
}

impl Colors {
    /// Порядок: фон, четыре поверхности, рамка, контур, четыре цвета текста.
    const fn new(hex: [u32; 11]) -> Self {
        let [bg, s1, s2, s3, s4, border, outline, tp, ts, tt, td] = hex;
        Self {
            bg: rgb(bg),
            surface_1: rgb(s1),
            surface_2: rgb(s2),
            surface_3: rgb(s3),
            surface_4: rgb(s4),
            border: rgb(border),
            outline: rgb(outline),
            text_primary: rgb(tp),
            text_secondary: rgb(ts),
            text_tertiary: rgb(tt),
            text_disabled: rgb(td),
        }
    }
}

/// Цветовое семейство: у каждого есть тёмный и светлый тон.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Family {
    #[default]
    Classic,
    Graphite,
    Coffee,
    Slate,
    Midnight,
    Ocean,
    Forest,
    Matcha,
    Amber,
    Dusk,
    Sakura,
    Velvet,
}

impl Family {
    /// Семейства по порядку в настройках.
    pub const ALL: [Self; 12] = [
        Self::Classic,
        Self::Graphite,
        Self::Coffee,
        Self::Slate,
        Self::Midnight,
        Self::Ocean,
        Self::Forest,
        Self::Matcha,
        Self::Amber,
        Self::Dusk,
        Self::Sakura,
        Self::Velvet,
    ];

    /// Ключ для `config.toml`.
    pub fn key(self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::Graphite => "graphite",
            Self::Coffee => "coffee",
            Self::Slate => "slate",
            Self::Midnight => "midnight",
            Self::Ocean => "ocean",
            Self::Forest => "forest",
            Self::Matcha => "matcha",
            Self::Amber => "amber",
            Self::Dusk => "dusk",
            Self::Sakura => "sakura",
            Self::Velvet => "velvet",
        }
    }

    /// Семейство по ключу из настроек: пробелы и регистр не важны.
    pub fn from_key(key: &str) -> Option<Self> {
        let key = key.trim();
        Self::ALL
            .into_iter()
            .find(|family| family.key().eq_ignore_ascii_case(key))
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Classic => "Классика",
            Self::Graphite => "Графит",
            Self::Coffee => "Кофе",
            Self::Slate => "Сланец",
            Self::Midnight => "Полночь",
            Self::Ocean => "Океан",
            Self::Forest => "Лес",
            Self::Matcha => "Матча",
            Self::Amber => "Янтарь",
            Self::Dusk => "Закат",
            Self::Sakura => "Сакура",
            Self::Velvet => "Бархат",
        }
    }

    /// Цвета семейства в заданном тоне.
    pub fn colors(self, tone: Tone) -> &'static Colors {
        let index = self.index() * 2 + usize::from(tone == Tone::Light);
        PALETTES.get(index).unwrap_or(&FALLBACK)
    }

    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|family| *family == self)
            .unwrap_or(0)
    }
}

/// Запасная палитра «Классика» (тёмная) на случай ошибки в номере.
const FALLBACK: Colors = Colors::new([
    0x000000, 0x0B0B0B, 0x141414, 0x1E1E1E, 0x2A2A2A, 0x1A1A1A, 0x333333, 0xF2F2F2, 0xA8A8A8,
    0x8A8A8A, 0x5A5A5A,
]);

/// Палитры: для каждого семейства по порядку [`Family::ALL`] тёмная, затем светлая.
const PALETTES: [Colors; 24] = [
    // classic, dark
    Colors::new([
        0x000000, 0x0B0B0B, 0x141414, 0x1E1E1E, 0x2A2A2A, 0x1A1A1A, 0x333333, 0xF2F2F2, 0xA8A8A8,
        0x8A8A8A, 0x5A5A5A,
    ]),
    // classic, light
    Colors::new([
        0xFFFFFF, 0xF4F4F4, 0xEBEBEB, 0xE1E1E1, 0xD6D6D6, 0xE5E5E5, 0xCDCDCD, 0x111111, 0x545454,
        0x696969, 0xA5A5A5,
    ]),
    // graphite, dark
    Colors::new([
        0x131417, 0x1C1D20, 0x242528, 0x2C2D31, 0x36383B, 0x292A2D, 0x3D3F43, 0xEEF0F3, 0xABADB0,
        0x909194, 0x646669,
    ]),
    // graphite, light
    Colors::new([
        0xF2F3F5, 0xE9EAEC, 0xE1E3E5, 0xD9DBDE, 0xCFD1D4, 0xDDDEE1, 0xC8CACD, 0x16181C, 0x545559,
        0x67696C, 0x9EA0A3,
    ]),
    // coffee, dark
    Colors::new([
        0x13100E, 0x1C1816, 0x241F1C, 0x2C2723, 0x36302C, 0x292421, 0x3D3732, 0xF4EFEA, 0xAFABA7,
        0x938F8B, 0x676360,
    ]),
    // coffee, light
    Colors::new([
        0xF8F4EF, 0xF0ECE6, 0xEAE5DF, 0xE2DDD8, 0xDAD4CE, 0xE5E0DB, 0xD3CEC7, 0x1F1A15, 0x5C5752,
        0x6F6B66, 0xA6A19C,
    ]),
    // slate, dark
    Colors::new([
        0x0E1418, 0x151C20, 0x1B2227, 0x222A2F, 0x2A3238, 0x1F272C, 0x30393F, 0xEBF1F5, 0xA7ADB1,
        0x8C9296, 0x60666A,
    ]),
    // slate, light
    Colors::new([
        0xEDF2F6, 0xE4EAEF, 0xDDE4E9, 0xD6DDE3, 0xCCD5DB, 0xD9E0E5, 0xC5CED6, 0x141C22, 0x51585D,
        0x646B70, 0x9BA1A5,
    ]),
    // midnight, dark
    Colors::new([
        0x090E1A, 0x101624, 0x161C2C, 0x1C2335, 0x242C3F, 0x1A2031, 0x293247, 0xEAF0FF, 0xA5ABB9,
        0x898F9D, 0x5D626F,
    ]),
    // midnight, light
    Colors::new([
        0xEEF3FB, 0xE5EBF5, 0xDDE4EF, 0xD5DDEA, 0xCBD4E3, 0xD8E0EC, 0xC3CDDD, 0x0F1A2E, 0x4D5767,
        0x626A7A, 0x99A1AD,
    ]),
    // ocean, dark
    Colors::new([
        0x03181D, 0x092026, 0x0E272D, 0x142F35, 0x1B383F, 0x122C32, 0x203F46, 0xE4F4F7, 0x9FB1B4,
        0x839599, 0x576A6E,
    ]),
    // ocean, light
    Colors::new([
        0xE5F4F7, 0xDBEDF0, 0xD3E7EB, 0xCAE1E5, 0xC0D9DE, 0xCEE3E7, 0xB8D3D8, 0x082830, 0x466168,
        0x5A737A, 0x91A6AB,
    ]),
    // forest, dark
    Colors::new([
        0x07140D, 0x0D1D14, 0x12241A, 0x182C20, 0x1F3528, 0x16281E, 0x243C2E, 0xE6F3EA, 0xA2AFA6,
        0x86938B, 0x5A675F,
    ]),
    // forest, light
    Colors::new([
        0xE9F5EC, 0xE0EEE4, 0xD8E8DD, 0xD0E2D5, 0xC5DACC, 0xD3E4D8, 0xBED4C5, 0x0E2416, 0x4B5F52,
        0x5F7165, 0x96A69B,
    ]),
    // matcha, dark
    Colors::new([
        0x111407, 0x191D0E, 0x202413, 0x272C19, 0x303520, 0x242916, 0x373D25, 0xEEF2E2, 0xAAAE9F,
        0x8F9384, 0x636758,
    ]),
    // matcha, light
    Colors::new([
        0xF2F5E1, 0xEBEED8, 0xE5E8D0, 0xDEE2C8, 0xD6DABD, 0xE0E5CB, 0xD0D5B6, 0x1D220D, 0x595D48,
        0x6C705B, 0xA1A590,
    ]),
    // amber, dark
    Colors::new([
        0x161007, 0x1F180D, 0x271F12, 0x2F2617, 0x3A2F1E, 0x2C2315, 0x413623, 0xF6EFE3, 0xB2ABA0,
        0x968F84, 0x696359,
    ]),
    // amber, light
    Colors::new([
        0xFBF4E5, 0xF5ECDC, 0xEFE6D4, 0xEADFCB, 0xE3D7C1, 0xECE2CF, 0xDDD1B9, 0x2A1F0C, 0x655B49,
        0x776E5C, 0xACA393,
    ]),
    // dusk, dark
    Colors::new([
        0x190C07, 0x23130D, 0x2B1912, 0x332018, 0x3E281F, 0x301D16, 0x462E24, 0xFAEDE6, 0xB5A8A2,
        0x998C86, 0x6D605A,
    ]),
    // dusk, light
    Colors::new([
        0xFDEEE5, 0xF7E6DC, 0xF2DFD4, 0xEDD8CC, 0xE6CFC2, 0xEFDBCF, 0xE1C8BB, 0x2E1810, 0x68544C,
        0x7B675F, 0xAE9D94,
    ]),
    // sakura, dark
    Colors::new([
        0x190A11, 0x231119, 0x2B1720, 0x331D27, 0x3E2530, 0x301B24, 0x462B37, 0xFAECF1, 0xB5A7AD,
        0x998B91, 0x6D5E64,
    ]),
    // sakura, light
    Colors::new([
        0xFDEEF4, 0xF7E5ED, 0xF2DEE6, 0xEDD6E0, 0xE7CDD8, 0xEFD9E2, 0xE2C6D1, 0x2D1620, 0x67525B,
        0x7A666E, 0xAE9CA3,
    ]),
    // velvet, dark
    Colors::new([
        0x120C21, 0x1A132A, 0x201932, 0x261F3B, 0x2F2745, 0x241D37, 0x352C4D, 0xF0EDFB, 0xACA8B8,
        0x918C9D, 0x656072,
    ]),
    // velvet, light
    Colors::new([
        0xF2EEFD, 0xEAE6F8, 0xE4DFF3, 0xDDD8EF, 0xD5CFE9, 0xE0DBF1, 0xCFC8E5, 0x1F1A33, 0x5A556C,
        0x6D687E, 0xA29DB0,
    ]),
];

/// Текущая схема: номер семейства и тон одним числом (как [`ACCENT`]).
static SCHEME: AtomicU32 = AtomicU32::new(0);

fn encode_scheme(family: Family, tone: Tone) -> u32 {
    let index = Family::ALL
        .iter()
        .position(|item| *item == family)
        .unwrap_or(0);
    // Номер семейства всегда меньше 12, потеря данных невозможна.
    (u32::try_from(index).unwrap_or(0) << 1) | u32::from(tone == Tone::Light)
}

fn decode_scheme(value: u32) -> (Family, Tone) {
    let tone = if value & 1 == 1 {
        Tone::Light
    } else {
        Tone::Dark
    };
    let family = usize::try_from(value >> 1)
        .ok()
        .and_then(|index| Family::ALL.get(index).copied())
        .unwrap_or_default();
    (family, tone)
}

pub fn set_scheme(family: Family, tone: Tone) {
    SCHEME.store(encode_scheme(family, tone), Ordering::Relaxed);
}

pub fn current_family() -> Family {
    decode_scheme(SCHEME.load(Ordering::Relaxed)).0
}

thread_local! {
    /// Сколько областей с принудительно тёмным тоном включено сейчас
    /// (см. [`dark_scope`]). Своё значение у каждого потока: вид окна
    /// собирается и рисуется в одном потоке, а параллельные тесты и фоновые
    /// задачи не видят чужих областей.
    static DARK_DEPTH: Cell<u32> = const { Cell::new(0) };
}

fn dark_depth() -> u32 {
    DARK_DEPTH.with(Cell::get)
}

/// Выключает принудительный тёмный тон, когда выходит из области видимости.
struct DarkGuard;

impl DarkGuard {
    fn enter() -> Self {
        DARK_DEPTH.with(|depth| depth.set(depth.get().saturating_add(1)));
        Self
    }
}

impl Drop for DarkGuard {
    fn drop(&mut self) {
        DARK_DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

/// Выполняет `build` с тёмным тоном вместо тона окна: цвета ролей
/// (`text_primary()`, `bg()` и другие) внутри берутся из тёмной палитры
/// текущего семейства.
pub fn dark_scope<R>(build: impl FnOnce() -> R) -> R {
    let _guard = DarkGuard::enter();
    build()
}

/// Тон, в котором сейчас рисуется окно: тон темы или тёмный внутри
/// [`dark_scope`].
pub fn current_tone() -> Tone {
    if dark_depth() > 0 {
        return Tone::Dark;
    }
    decode_scheme(SCHEME.load(Ordering::Relaxed)).1
}

/// Цвета текущей схемы.
pub fn colors() -> &'static Colors {
    current_family().colors(current_tone())
}

pub fn bg() -> Color {
    colors().bg
}

pub fn surface_1() -> Color {
    colors().surface_1
}

pub fn surface_2() -> Color {
    colors().surface_2
}

pub fn surface_3() -> Color {
    colors().surface_3
}

pub fn surface_4() -> Color {
    colors().surface_4
}

pub fn border() -> Color {
    colors().border
}

pub fn outline() -> Color {
    colors().outline
}

pub fn text_primary() -> Color {
    colors().text_primary
}

pub fn text_secondary() -> Color {
    colors().text_secondary
}

pub fn text_tertiary() -> Color {
    colors().text_tertiary
}

pub fn text_disabled() -> Color {
    colors().text_disabled
}

pub fn error() -> Color {
    match current_tone() {
        Tone::Dark => rgb(0xFF6B6B),
        Tone::Light => rgb(0xD23F3F),
    }
}

pub fn error_soft() -> Color {
    alpha(error(), 0.14)
}

pub fn error_hover() -> Color {
    alpha(error(), 0.24)
}

pub fn error_pressed() -> Color {
    alpha(error(), 0.34)
}

/// Рост в топе месяца.
pub fn growth() -> Color {
    match current_tone() {
        Tone::Dark => rgb(0x4CD07D),
        Tone::Light => rgb(0x1C9A50),
    }
}

pub fn scrim() -> Color {
    match current_tone() {
        Tone::Dark => rgba(0x000000, 0.72),
        Tone::Light => rgba(0x0F0F14, 0.40),
    }
}

/// Кнопка «закрыть» в заголовке окна при наведении, как у системных окон.
#[cfg(not(target_os = "macos"))]
pub const CLOSE_HOVER: Color = rgb(0xC42B1C);

// ---------------------------------------------------------------- акцент

/// Акцентный цвет. Выбирается в «Настройки → Внешний вид», хранится в
/// `config.toml` ключом [`Accent::key`]: готовый — именем (`red`), свой —
/// записью `#RRGGBB`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Accent {
    #[default]
    Red,
    Orange,
    Yellow,
    Green,
    Teal,
    Blue,
    Violet,
    Pink,
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
    pub const PRESETS: [Self; 8] = [
        Self::Red,
        Self::Orange,
        Self::Yellow,
        Self::Green,
        Self::Teal,
        Self::Blue,
        Self::Violet,
        Self::Pink,
    ];

    pub fn custom(rgb: u32) -> Self {
        Self::Custom(rgb & RGB_MASK)
    }

    /// Ключ для `config.toml`: имя готового акцента или `#RRGGBB`.
    pub fn key(self) -> Cow<'static, str> {
        match self {
            Self::Red => Cow::Borrowed("red"),
            Self::Orange => Cow::Borrowed("orange"),
            Self::Yellow => Cow::Borrowed("yellow"),
            Self::Green => Cow::Borrowed("green"),
            Self::Teal => Cow::Borrowed("teal"),
            Self::Blue => Cow::Borrowed("blue"),
            Self::Violet => Cow::Borrowed("violet"),
            Self::Pink => Cow::Borrowed("pink"),
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
            Self::Orange => Cow::Borrowed("Оранжевый"),
            Self::Yellow => Cow::Borrowed("Жёлтый"),
            Self::Green => Cow::Borrowed("Зелёный"),
            Self::Teal => Cow::Borrowed("Бирюзовый"),
            Self::Blue => Cow::Borrowed("Синий"),
            Self::Violet => Cow::Borrowed("Фиолетовый"),
            Self::Pink => Cow::Borrowed("Розовый"),
            Self::Custom(rgb) => Cow::Owned(format!("Свой цвет {}", format_hex(rgb))),
        }
    }

    /// Цвет как число `0xRRGGBB`.
    pub fn rgb(self) -> u32 {
        match self {
            Self::Red => 0xFA2D48,
            Self::Orange => 0xFF8A3D,
            Self::Yellow => 0xFFC53D,
            Self::Green => 0x3CCB7F,
            Self::Teal => 0x3DD6C3,
            Self::Blue => 0x3D8BFF,
            Self::Violet => 0x9D7BFF,
            Self::Pink => 0xFF4F9A,
            Self::Custom(rgb) => rgb & RGB_MASK,
        }
    }

    pub fn color(self) -> Color {
        rgb(self.rgb())
    }

    /// Число для [`ACCENT`]: номер готового акцента или цвет с признаком.
    fn encode(self) -> u32 {
        match self {
            Self::Red => 0,
            Self::Teal => 1,
            Self::Orange => 2,
            Self::Violet => 3,
            Self::Blue => 4,
            Self::Yellow => 5,
            Self::Green => 6,
            Self::Pink => 7,
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

/// Акцент окна. В светлом тоне слишком светлый цвет затемняется, чтобы
/// держать контраст 3:1 с фоном; образцы в настройках показывают исходный.
pub fn accent() -> Color {
    legible_on(current_accent().color(), bg(), current_tone())
}

/// Относительная яркость по WCAG.
fn luminance(color: Color) -> f32 {
    let lin = |c: f32| {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(color.r) + 0.7152 * lin(color.g) + 0.0722 * lin(color.b)
}

/// Контраст двух цветов по WCAG (от 1 до 21).
pub fn contrast(a: Color, b: Color) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Контраст акцента с фоном, ниже которого его затемняют в светлом тоне.
const ACCENT_MIN_CONTRAST: f32 = 3.0;

/// `color` на фоне `bg`: в светлом тоне подмешивает чёрный (не больше 50 %),
/// пока контраст не достигнет 3:1.
pub fn legible_on(color: Color, bg: Color, tone: Tone) -> Color {
    if tone == Tone::Dark {
        return color;
    }
    for step in 0..=10u8 {
        let candidate = mix(color, Color::BLACK, f32::from(step) * 0.05);
        if contrast(candidate, bg) >= ACCENT_MIN_CONTRAST {
            return candidate;
        }
    }
    mix(color, Color::BLACK, 0.5)
}

/// Альфа тени: в светлом тоне вдвое меньше.
pub fn shadow_alpha(dark: f32) -> f32 {
    match current_tone() {
        Tone::Dark => dark,
        Tone::Light => dark * 0.5,
    }
}

pub fn accent_hover() -> Color {
    mix(accent(), Color::WHITE, 0.16)
}

pub fn accent_pressed() -> Color {
    mix(accent(), Color::BLACK, 0.16)
}

pub fn on_accent() -> Color {
    contrast_on(accent())
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
            background: bg(),
            text: text_primary(),
            primary: accent(),
            success: accent(),
            warning: accent_hover(),
            danger: error(),
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
        assert_eq!(contrast_on(Accent::Red.color()), rgb(0xFFFFFF));
        assert_eq!(contrast_on(Accent::Teal.color()), rgb(0x0A0A0A));
        assert_eq!(contrast_on(Accent::custom(0x000000).color()), rgb(0xFFFFFF));
        assert_eq!(contrast_on(Accent::custom(0xFFFFFF).color()), rgb(0x0A0A0A));
    }

    #[test]
    fn mix_clamps_and_survives_nan() {
        let black = Color::BLACK;
        let white = Color::WHITE;
        assert_eq!(mix(black, white, 2.0), white);
        assert_eq!(mix(black, white, -1.0), black);
        assert_eq!(mix(black, white, f32::NAN), black);
    }

    #[test]
    fn classic_dark_matches_old_constants() {
        let c = Family::Classic.colors(Tone::Dark);
        assert_eq!(c.bg, rgb(0x000000));
        assert_eq!(c.surface_1, rgb(0x0B0B0B));
        assert_eq!(c.surface_2, rgb(0x141414));
        assert_eq!(c.surface_3, rgb(0x1E1E1E));
        assert_eq!(c.surface_4, rgb(0x2A2A2A));
        assert_eq!(c.border, rgb(0x1A1A1A));
        assert_eq!(c.outline, rgb(0x333333));
        assert_eq!(c.text_primary, rgb(0xF2F2F2));
        assert_eq!(c.text_secondary, rgb(0xA8A8A8));
        assert_eq!(c.text_tertiary, rgb(0x8A8A8A));
        assert_eq!(c.text_disabled, rgb(0x5A5A5A));
        assert_eq!(*c, FALLBACK);
        // Тон по умолчанию — тёмный: ошибки и рост как раньше.
        assert_eq!(error(), rgb(0xFF6B6B));
        assert_eq!(error_soft(), rgba(0xFF6B6B, 0.14));
        assert_eq!(error_hover(), rgba(0xFF6B6B, 0.24));
        assert_eq!(error_pressed(), rgba(0xFF6B6B, 0.34));
        assert_eq!(growth(), rgb(0x4CD07D));
        assert_eq!(scrim(), rgba(0x000000, 0.72));
    }

    #[test]
    fn family_keys_round_trip() {
        for family in Family::ALL {
            assert_eq!(Family::from_key(family.key()), Some(family));
            assert!(!family.title().is_empty());
        }
        assert_eq!(Family::from_key("  MIDNIGHT "), Some(Family::Midnight));
        assert_eq!(Family::from_key("полночь"), None);
        assert_eq!(Family::from_key(""), None);
        assert_eq!(Family::from_key("mid night"), None);
        assert_eq!(Family::default(), Family::Classic);
    }

    #[test]
    fn scheme_encoding_round_trips() {
        for family in Family::ALL {
            for tone in [Tone::Dark, Tone::Light] {
                assert_eq!(decode_scheme(encode_scheme(family, tone)), (family, tone));
            }
        }
        assert_eq!(decode_scheme(999), (Family::Classic, Tone::Light));
        assert_eq!(decode_scheme(998), (Family::Classic, Tone::Dark));
        assert_eq!(decode_scheme(0), (Family::Classic, Tone::Dark));
    }

    #[test]
    fn palettes_are_readable() {
        for family in Family::ALL {
            for tone in [Tone::Dark, Tone::Light] {
                let c = family.colors(tone);
                let name = format!("{}/{tone:?}", family.key());
                assert!(contrast(c.text_primary, c.bg) >= 7.0, "{name}: primary");
                assert!(
                    contrast(c.text_secondary, c.surface_2) >= 4.5,
                    "{name}: secondary"
                );
                assert!(contrast(c.text_tertiary, c.bg) >= 4.0, "{name}: tertiary");
                let (bg, text) = (luminance(c.bg), luminance(c.text_primary));
                match tone {
                    Tone::Light => assert!(bg > text, "{name}: светлый тон"),
                    Tone::Dark => assert!(bg < text, "{name}: тёмный тон"),
                }
            }
        }
    }

    #[test]
    fn theme_mode_keys_and_tone() {
        for mode in ThemeMode::ALL {
            assert_eq!(ThemeMode::from_key(mode.key()), Some(mode));
        }
        assert_eq!(ThemeMode::from_key(" System "), Some(ThemeMode::System));
        assert_eq!(ThemeMode::from_key("5"), None);
        assert_eq!(ThemeMode::default(), ThemeMode::Dark);
        assert_eq!(ThemeMode::Dark.tone(Tone::Light), Tone::Dark);
        assert_eq!(ThemeMode::Light.tone(Tone::Dark), Tone::Light);
        assert_eq!(ThemeMode::System.tone(Tone::Light), Tone::Light);
        assert_eq!(Tone::from_system(iced::theme::Mode::None), Tone::Dark);
        assert_eq!(Tone::from_system(iced::theme::Mode::Light), Tone::Light);
    }

    #[test]
    fn accents_are_legible_on_light_palettes() {
        for family in Family::ALL {
            let bg = family.colors(Tone::Light).bg;
            for accent in Accent::PRESETS {
                let shown = legible_on(accent.color(), bg, Tone::Light);
                assert!(
                    contrast(shown, bg) >= 3.0,
                    "{}/{}",
                    family.key(),
                    accent.key()
                );
            }
        }
        // В тёмном тоне цвет не меняется.
        let red = Accent::Red.color();
        assert_eq!(legible_on(red, rgb(0), Tone::Dark), red);
        // Подмешивается не больше половины чёрного: на фоне, с которым
        // контраста не добиться, остаётся серый 0.5.
        let white = Color::WHITE;
        let shown = legible_on(white, white, Tone::Light);
        assert!(shown.r >= 0.5 && contrast(shown, white) >= 3.0);
        let hopeless = rgb(0xBBBBBB);
        assert_eq!(
            legible_on(white, hopeless, Tone::Light),
            mix(white, Color::BLACK, 0.5)
        );
    }

    #[test]
    fn dark_scope_nests_and_releases() {
        assert_eq!(dark_depth(), 0);
        let inside = dark_scope(|| {
            dark_scope(|| assert_eq!(current_tone(), Tone::Dark));
            dark_depth()
        });
        assert_eq!(inside, 1);
        assert_eq!(dark_depth(), 0);
    }
}
