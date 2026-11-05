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
    Check,
    Clock,
    Close,
    Copy,
    Dislike,
    Empty,
    Error,
    Expand,
    Expired,
    ExternalLink,
    Forward,
    Heart,
    HeartFilled,
    Lock,
    Logout,
    Lyrics,
    MiniPlayer,
    Minus,
    More,
    NextFilled,
    Offline,
    PauseFilled,
    PlayFilled,
    Playlist,
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
    Volume,
    VolumeLow,
    VolumeMute,
    Wave,
    WindowClose,
    WindowMaximize,
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
            Self::Check => file!("check"),
            Self::Clock => file!("clock"),
            Self::Close => file!("close"),
            Self::Copy => file!("copy"),
            Self::Dislike => file!("dislike"),
            Self::Empty => file!("empty"),
            Self::Error => file!("error"),
            Self::Expand => file!("expand"),
            Self::Expired => file!("expired"),
            Self::ExternalLink => file!("external-link"),
            Self::Forward => file!("forward"),
            Self::Heart => file!("heart"),
            Self::HeartFilled => file!("heart-filled"),
            Self::Lock => file!("lock"),
            Self::Logout => file!("logout"),
            Self::Lyrics => file!("lyrics"),
            Self::MiniPlayer => file!("mini-player"),
            // Иконки, которых нет в макете, лежат в `assets/icons` (тоже Lucide).
            Self::Minus => include_bytes!("../../assets/icons/minus.svg"),
            Self::More => file!("more"),
            Self::NextFilled => file!("next-filled"),
            Self::Offline => file!("offline"),
            Self::PauseFilled => file!("pause-filled"),
            Self::PlayFilled => file!("play-filled"),
            Self::Playlist => file!("playlist"),
            Self::Plus => file!("plus"),
            Self::PreviousFilled => file!("previous-filled"),
            Self::Queue => file!("queue"),
            Self::Repeat => file!("repeat"),
            Self::RepeatOne => file!("repeat-one"),
            Self::Retry => file!("retry"),
            Self::Search => file!("search"),
            Self::Settings => file!("settings"),
            Self::Shuffle => file!("shuffle"),
            Self::Spinner => file!("spinner"),
            Self::Volume => file!("volume"),
            Self::VolumeLow => file!("volume-low"),
            Self::VolumeMute => file!("volume-mute"),
            Self::Wave => file!("wave"),
            Self::WindowClose => file!("window-close"),
            Self::WindowMaximize => file!("window-maximize"),
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
