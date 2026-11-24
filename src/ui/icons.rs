//! Иконки Lucide из `design/icons` (ISC), встроены в бинарник.

use iced::widget::svg::{self, Svg};
use iced::{Color, Radians};

/// Иконки, которые использует окно.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    About,
    Album,
    Artist,
    Back,
    Chat,
    Check,
    ChevronDown,
    Clock,
    Close,
    Copy,
    Devices,
    Dislike,
    Download,
    Empty,
    Equalizer,
    Error,
    Expand,
    Expired,
    ExternalLink,
    Flame,
    Forward,
    Heart,
    HeartFilled,
    History,
    Lock,
    Logout,
    Lyrics,
    MiniPlayer,
    Minus,
    More,
    NextFilled,
    Offline,
    Palette,
    PauseFilled,
    Pin,
    PlayFilled,
    Playlist,
    Plug,
    Plus,
    PreviousFilled,
    Queue,
    Repeat,
    RepeatOne,
    Retry,
    Search,
    Settings,
    Shuffle,
    Spinner,
    Stats,
    /// Сдвиг в топе: вверх, вниз, новый.
    TrendDown,
    TrendNew,
    TrendSame,
    TrendUp,
    Volume,
    VolumeLow,
    VolumeMute,
    Wave,
    #[cfg(not(target_os = "macos"))]
    WindowClose,
    WindowMaximize,
    #[cfg(not(target_os = "macos"))]
    WindowMinimize,
}

impl Icon {
    fn bytes(self) -> &'static [u8] {
        macro_rules! file {
            ($name:literal) => {
                include_bytes!(concat!("../../design/icons/", $name, ".svg"))
            };
        }
        match self {
            Self::About => file!("about"),
            Self::Album => file!("album"),
            Self::Artist => file!("artist"),
            Self::Back => file!("back"),
            Self::Chat => include_bytes!("../../assets/icons/chat.svg"),
            Self::Check => file!("check"),
            Self::ChevronDown => include_bytes!("../../assets/icons/chevron-down.svg"),
            Self::Clock => file!("clock"),
            Self::Close => file!("close"),
            Self::Copy => file!("copy"),
            Self::Devices => include_bytes!("../../assets/icons/devices.svg"),
            Self::Dislike => file!("dislike"),
            Self::Download => include_bytes!("../../assets/icons/download.svg"),
            Self::Empty => file!("empty"),
            Self::Equalizer => include_bytes!("../../assets/icons/equalizer.svg"),
            Self::Error => file!("error"),
            Self::Expand => file!("expand"),
            Self::Expired => file!("expired"),
            Self::ExternalLink => file!("external-link"),
            Self::Flame => include_bytes!("../../assets/icons/flame.svg"),
            Self::Forward => file!("forward"),
            Self::Heart => file!("heart"),
            Self::HeartFilled => file!("heart-filled"),
            Self::History => include_bytes!("../../assets/icons/history.svg"),
            Self::Lock => file!("lock"),
            Self::Logout => file!("logout"),
            Self::Lyrics => include_bytes!("../../assets/icons/lyrics-text.svg"),
            Self::MiniPlayer => include_bytes!("../../assets/icons/square-arrow-out-down-left.svg"),
            // Иконки, которых нет в макете, лежат в `assets/icons` (тоже Lucide).
            Self::Minus => include_bytes!("../../assets/icons/minus.svg"),
            Self::More => file!("more"),
            Self::NextFilled => file!("next-filled"),
            Self::Palette => include_bytes!("../../assets/icons/palette.svg"),
            Self::Pin => file!("pin"),
            Self::Offline => file!("offline"),
            Self::PauseFilled => file!("pause-filled"),
            Self::PlayFilled => file!("play-filled"),
            Self::Playlist => file!("playlist"),
            Self::Plug => include_bytes!("../../assets/icons/plug.svg"),
            Self::Plus => file!("plus"),
            Self::PreviousFilled => file!("previous-filled"),
            Self::Queue => include_bytes!("../../assets/icons/list-music.svg"),
            Self::Repeat => file!("repeat"),
            Self::RepeatOne => file!("repeat-one"),
            Self::Retry => file!("retry"),
            Self::Search => file!("search"),
            Self::Settings => file!("settings"),
            Self::Shuffle => file!("shuffle"),
            Self::Spinner => file!("spinner"),
            Self::Stats => include_bytes!("../../assets/icons/chart-column.svg"),
            // Свои простые фигуры: треугольники, черта и точка.
            Self::TrendDown => include_bytes!("../../assets/icons/trend-down.svg"),
            Self::TrendNew => include_bytes!("../../assets/icons/trend-new.svg"),
            Self::TrendSame => include_bytes!("../../assets/icons/trend-same.svg"),
            Self::TrendUp => include_bytes!("../../assets/icons/trend-up.svg"),
            Self::Volume => file!("volume"),
            Self::VolumeLow => file!("volume-low"),
            Self::VolumeMute => file!("volume-mute"),
            Self::Wave => file!("wave"),
            #[cfg(not(target_os = "macos"))]
            Self::WindowClose => file!("window-close"),
            Self::WindowMaximize => file!("window-maximize"),
            #[cfg(not(target_os = "macos"))]
            Self::WindowMinimize => file!("window-minimize"),
        }
    }
}

/// Иконка размера `size` в цвете `color`. iced перекрашивает всю картинку,
/// поэтому `currentColor` в файлах не мешает.
pub fn icon(icon: Icon, size: f32, color: Color) -> Svg<'static> {
    iced::widget::svg(svg::Handle::from_memory(icon.bytes()))
        .width(size)
        .height(size)
        .style(move |_, _| svg::Style { color: Some(color) })
}

/// Спиннер: иконка, повёрнутая на угол `turn` (доля оборота, 0..1).
pub fn spinner(size: f32, color: Color, turn: f32) -> Svg<'static> {
    icon(Icon::Spinner, size, color).rotation(Radians(turn * std::f32::consts::TAU))
}
