//! Главное окно: боковое меню, «Мне нравится», настройки и панель плеера.
//!
//! Звука на этапе 5 нет: кнопки плеера меняют только то, что нарисовано.
//! Список длинный (до тысяч строк), поэтому строятся только видимые строки.

use std::sync::Arc;
use std::time::Duration;

use iced::widget::{
    Space, button, center, column, container, hover, mouse_area, row, scrollable, slider, tooltip,
};
use iced::{Alignment, Background, Border, Color, Element, Length, Padding, Radians, Task};

use super::Message as App;
use super::Message::Library as Lib;
use super::covers::{Art, Covers};
use super::icons::{Icon, icon};
use super::marquee::marquee;
use super::theme::{self, Style};
use super::widgets::{self, CoverArt, Kind, Size};
use super::{Dialog, Failure, FailureKind};
use crate::api::{self, AccountStatus, PlaybackPermit, Subscription, Track};
use crate::storage::token::StoredToken;
use crate::texts::{self, sentence};

/// Высота строки трека.
const ROW: f32 = 56.0;
/// Сколько строк строить сверх видимых, сверху и снизу.
const OVERSCAN: usize = 8;
/// Отступ над первой строкой.
const LIST_TOP: f32 = 8.0;
/// Громкость, которую возвращает кнопка звука, если громкость была нулевой.
const UNMUTE_VOLUME: f32 = 0.5;
/// Через сколько показывать скелетон: быстрые ответы не мигают.
const SKELETON_DELAY: Duration = Duration::from_millis(300);

/// Разделы бокового меню. Пока работает только «Мне нравится» и настройки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Wave,
    Search,
    Liked,
    Playlists,
    Albums,
    Artists,
    Settings,
}

impl Section {
    fn title(self) -> &'static str {
        match self {
            Self::Wave => "Моя волна",
            Self::Search => "Поиск",
            Self::Liked => "Мне нравится",
            Self::Playlists => "Плейлисты",
            Self::Albums => "Альбомы",
            Self::Artists => "Исполнители",
            Self::Settings => "Настройки",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Self::Wave => Icon::Wave,
            Self::Search => Icon::Search,
            Self::Liked => Icon::Heart,
            Self::Playlists => Icon::Playlist,
            Self::Albums => Icon::Album,
            Self::Artists => Icon::Artist,
            Self::Settings => Icon::Settings,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Repeat {
    Off,
    All,
    One,
}

#[derive(Clone)]
pub enum Message {
    Nav(Section),
    LikesLoaded(Result<Arc<Vec<Track>>, Failure>),
    ShowSkeleton,
    RetryLikes,
    Scrolled(scrollable::Viewport),
    Select(usize),
    Play(usize),
    PlayAll,
    TogglePlay,
    Previous,
    Next,
    ToggleShuffle,
    CycleRepeat,
    Seek(f32),
    Volume(f32),
    ToggleMute,
    Noop,
}

/// Что главное окно просит сделать приложение.
pub enum Effect {
    /// Токен больше не действует.
    Relogin,
}

enum Likes {
    Loading { skeleton: bool },
    Ready(Arc<Vec<Track>>),
    Failed(Failure),
}

/// Что нарисовано в плеере. Звука нет до этапа 6.
struct Player {
    current: Option<usize>,
    playing: bool,
    position: f32,
    volume: f32,
    muted: bool,
    shuffle: bool,
    repeat: Repeat,
}

pub struct Library {
    token: Arc<StoredToken>,
    status: AccountStatus,
    /// Пропуск к плееру: главное окно без него не создаётся. Плеер этапа 6
    /// получит его отсюда.
    _permit: PlaybackPermit,
    section: Section,
    likes: Likes,
    selected: Option<usize>,
    scroll_y: f32,
    /// Высота окна: список заведомо не выше, поэтому строк построится с
    /// запасом. Высоту самого списка `scrollable` сообщает только при
    /// прокрутке, а после увеличения окна она бы устарела.
    viewport: f32,
    player: Player,
}

impl Library {
    pub fn new(
        token: Arc<StoredToken>,
        status: AccountStatus,
        permit: PlaybackPermit,
        window_height: f32,
        http: Option<&reqwest::Client>,
    ) -> (Self, Task<Message>) {
        let mut library = Self {
            token,
            status,
            _permit: permit,
            section: Section::Liked,
            likes: Likes::Loading { skeleton: false },
            selected: None,
            scroll_y: 0.0,
            viewport: window_height,
            player: Player {
                current: None,
                playing: false,
                position: 0.0,
                volume: 0.7,
                muted: false,
                shuffle: false,
                repeat: Repeat::Off,
            },
        };
        let task = library.load_likes(http);
        (library, task)
    }

    pub fn token(&self) -> Arc<StoredToken> {
        self.token.clone()
    }

    pub fn resized(&mut self, window_height: f32) {
        self.viewport = window_height;
    }

    fn load_likes(&mut self, http: Option<&reqwest::Client>) -> Task<Message> {
        self.likes = Likes::Loading { skeleton: false };
        let Some(http) = http.cloned() else {
            return Task::done(Message::LikesLoaded(Err(Failure {
                kind: FailureKind::Other,
                text: "не удалось подготовить сетевое соединение".to_owned(),
            })));
        };
        let token = self.token.clone();
        let uid = self.status.account().uid;
        let load = Task::perform(
            async move {
                let liked = api::fetch_liked_ids(&http, &token.access_token, uid).await?;
                let tracks =
                    api::fetch_tracks(&http, &token.access_token, &liked.ids, |_| {}).await?;
                Ok(Arc::new(tracks))
            },
            |result: Result<_, api::ApiError>| Message::LikesLoaded(result.map_err(Failure::from)),
        );
        let skeleton = Task::perform(tokio::time::sleep(SKELETON_DELAY), |()| {
            Message::ShowSkeleton
        });
        Task::batch([load, skeleton])
    }

    fn tracks(&self) -> &[Track] {
        match &self.likes {
            Likes::Ready(tracks) => tracks,
            _ => &[],
        }
    }

    fn current(&self) -> Option<&Track> {
        self.player
            .current
            .and_then(|index| self.tracks().get(index))
    }

    pub fn animating(&self) -> bool {
        matches!(self.likes, Likes::Loading { skeleton: true })
    }

    pub fn update(
        &mut self,
        message: Message,
        http: Option<&reqwest::Client>,
    ) -> (Task<Message>, Option<Effect>) {
        match message {
            Message::Nav(section) => {
                // Список при возврате в раздел создаётся заново, с начала.
                if section != self.section {
                    self.scroll_y = 0.0;
                }
                self.section = section;
            }
            Message::LikesLoaded(result) => match result {
                Ok(tracks) => {
                    self.likes = Likes::Ready(tracks);
                    self.selected = None;
                    self.player.current = None;
                }
                Err(failure) if failure.kind == FailureKind::Unauthorized => {
                    return (Task::none(), Some(Effect::Relogin));
                }
                Err(failure) => self.likes = Likes::Failed(failure),
            },
            Message::ShowSkeleton => {
                if let Likes::Loading { skeleton } = &mut self.likes {
                    *skeleton = true;
                }
            }
            Message::RetryLikes => return (self.load_likes(http), None),
            Message::Scrolled(viewport) => {
                self.scroll_y = viewport.absolute_offset().y;
            }
            Message::Select(index) => self.selected = Some(index),
            Message::Play(index) => self.play(index),
            Message::PlayAll => {
                let first = self.tracks().iter().position(|track| track.available);
                if let Some(index) = first {
                    self.play(index);
                }
            }
            Message::TogglePlay => {
                if self.player.current.is_some() {
                    self.player.playing = !self.player.playing;
                }
            }
            Message::Previous => {
                if self.player.position > 3.0 {
                    self.player.position = 0.0;
                } else if let Some(index) = self.player.current {
                    self.step(index, false);
                }
            }
            Message::Next => {
                if let Some(index) = self.player.current {
                    self.step(index, true);
                }
            }
            Message::ToggleShuffle => self.player.shuffle = !self.player.shuffle,
            Message::CycleRepeat => {
                self.player.repeat = match self.player.repeat {
                    Repeat::Off => Repeat::All,
                    Repeat::All => Repeat::One,
                    Repeat::One => Repeat::Off,
                };
            }
            Message::Seek(position) => self.player.position = position,
            Message::Volume(volume) => {
                self.player.volume = volume;
                self.player.muted = volume <= 0.0;
            }
            Message::ToggleMute => {
                if self.player.volume <= 0.0 {
                    // Включить звук при нулевой громкости — вернуть громкость.
                    self.player.volume = UNMUTE_VOLUME;
                    self.player.muted = false;
                } else {
                    self.player.muted = !self.player.muted;
                }
            }
            Message::Noop => {}
        }
        (Task::none(), None)
    }

    fn play(&mut self, index: usize) {
        if self
            .tracks()
            .get(index)
            .is_some_and(|track| track.available)
        {
            self.player.current = Some(index);
            self.player.playing = true;
            self.player.position = 0.0;
            self.selected = Some(index);
        }
    }

    /// Соседний доступный трек. С повтором списка — по кругу.
    fn step(&mut self, from: usize, forward: bool) {
        let count = self.tracks().len();
        let wrap = self.player.repeat == Repeat::All;
        let mut index = from;
        for _ in 0..count {
            index = match (forward, index) {
                (true, i) if i + 1 < count => i + 1,
                (true, _) if wrap => 0,
                (false, 0) if wrap => count.saturating_sub(1),
                (false, i) if i > 0 => i - 1,
                _ => return,
            };
            if self
                .tracks()
                .get(index)
                .is_some_and(|track| track.available)
            {
                self.play(index);
                return;
            }
        }
    }

    /// Диапазон строк, которые надо построить.
    fn visible_range(&self) -> (usize, usize) {
        let count = self.tracks().len();
        let first = rows_in(self.scroll_y - LIST_TOP).saturating_sub(OVERSCAN);
        let last = (rows_in(self.scroll_y - LIST_TOP + self.viewport) + 1 + OVERSCAN).min(count);
        (first.min(last), last)
    }

    /// Обложки, которые сейчас на экране: видимые строки и трек в плеере.
    pub fn visible_covers(&self) -> Vec<&str> {
        let (first, last) = self.visible_range();
        let mut uris: Vec<&str> = self
            .current()
            .and_then(|track| track.cover_uri.as_deref())
            .into_iter()
            .collect();
        if self.section == Section::Liked {
            uris.extend(
                self.tracks()
                    .get(first..last)
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|track| track.cover_uri.as_deref()),
            );
        }
        uris
    }

    // ------------------------------------------------------------ вид

    /// `collapsed` — меню свёрнуто до иконок, `compact` — узкое окно: в плеере
    /// меньше кнопок.
    pub fn view<'a>(
        &'a self,
        covers: &'a Covers,
        collapsed: bool,
        compact: bool,
        turn: f32,
    ) -> Element<'a, App> {
        let content: Element<'a, App> = match self.section {
            Section::Liked => self.liked(covers, turn),
            Section::Settings => self.settings(),
            section => placeholder(section),
        };
        let panel = container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .clip(true)
            .style(|_| container::Style {
                background: Some(theme::SURFACE_1.into()),
                border: Border::default().rounded(16),
                ..container::Style::default()
            });
        // Как в официальном клиенте: меню во всю высоту слева, справа
        // карточка раздела и под ней карточка плеера.
        let right = column![panel, self.player_bar(covers, compact)]
            .spacing(8)
            .width(Length::Fill)
            .padding(Padding {
                top: 0.0,
                right: 8.0,
                bottom: 8.0,
                left: 0.0,
            });
        row![self.sidebar(collapsed), right]
            .height(Length::Fill)
            .into()
    }

    fn sidebar(&self, compact: bool) -> Element<'_, App> {
        let item = |section: Section| nav_item(section, self.section == section, compact);
        // Кнопка сворачивания появляется, когда мышь над шапкой меню.
        let brand: Element<'_, App> = if compact {
            hover(
                container(widgets::logo(28.0))
                    .center_x(Length::Fill)
                    .height(48)
                    .center_y(48),
                container(sidebar_toggle(true))
                    .center_x(Length::Fill)
                    .height(48)
                    .center_y(48),
            )
        } else {
            hover(
                row![
                    widgets::logo(28.0),
                    theme::line(super::APP_NAME, Style::Headline, theme::TEXT_PRIMARY),
                ]
                .spacing(10)
                .padding([0, 8])
                // Во всю ширину: верхний слой `hover` не шире нижнего.
                .width(Length::Fill)
                .height(48)
                .align_y(Alignment::Center),
                row![Space::new().width(Length::Fill), sidebar_toggle(false)]
                    .height(48)
                    .align_y(Alignment::Center),
            )
        };
        let collection: Element<'_, App> = if compact {
            container(widgets::divider()).padding([10, 8]).into()
        } else {
            container(theme::line(
                "Коллекция",
                Style::Caption,
                theme::TEXT_TERTIARY,
            ))
            .padding(Padding {
                top: 16.0,
                right: 12.0,
                bottom: 6.0,
                left: 12.0,
            })
            .into()
        };
        column![
            brand,
            Space::new().height(12),
            item(Section::Wave),
            item(Section::Search),
            collection,
            item(Section::Liked),
            item(Section::Playlists),
            item(Section::Albums),
            item(Section::Artists),
            Space::new().height(Length::Fill),
            item(Section::Settings),
            Space::new().height(8),
            self.account_card(compact),
        ]
        .spacing(2)
        .width(if compact { 72 } else { 232 })
        .padding(Padding {
            top: 4.0,
            right: 12.0,
            bottom: 12.0,
            left: 12.0,
        })
        .into()
    }

    fn account_card(&self, compact: bool) -> Element<'_, App> {
        let account = self.status.account();
        let name = account
            .display_name
            .clone()
            .or_else(|| account.login.clone())
            .unwrap_or_else(|| account.name());
        let initial = name
            .chars()
            .next()
            .map(|ch| ch.to_uppercase().to_string())
            .unwrap_or_default();
        let avatar = avatar(initial, 36.0);
        let open = Lib(Message::Nav(Section::Settings));
        if compact {
            return widgets::with_tooltip(
                button(container(avatar).center_x(Length::Fill))
                    .padding([4, 0])
                    .style(|_, _| button::Style::default())
                    .on_press(open),
                "Аккаунт и настройки",
                tooltip::Position::Right,
            );
        }
        let card = row![
            avatar,
            column![
                theme::line(name, Style::BodyStrong, theme::TEXT_PRIMARY),
                theme::line("Плюс активен", Style::Caption, theme::ACCENT),
            ]
            .width(Length::Fill),
        ]
        .spacing(10)
        .align_y(Alignment::Center);
        button(card)
            .padding(8)
            .width(Length::Fill)
            .style(|_, status| button::Style {
                background: Some(
                    if status == button::Status::Hovered {
                        theme::SURFACE_3
                    } else {
                        theme::SURFACE_2
                    }
                    .into(),
                ),
                border: Border::default().rounded(12),
                ..button::Style::default()
            })
            .on_press(open)
            .into()
    }

    fn liked<'a>(&'a self, covers: &'a Covers, turn: f32) -> Element<'a, App> {
        match &self.likes {
            Likes::Loading { skeleton: false } => Space::new().into(),
            Likes::Loading { skeleton: true } => skeleton(turn),
            Likes::Failed(failure) => {
                let (badge, text) = if failure.kind == FailureKind::Offline {
                    (
                        Icon::Offline,
                        "Нет соединения с сервером. Проверьте интернет или прокси и попробуйте ещё раз."
                            .to_owned(),
                    )
                } else {
                    (Icon::Error, sentence(&failure.text))
                };
                state_block(
                    widgets::badge(badge, theme::ERROR, theme::ERROR_SOFT),
                    "Не удалось загрузить «Мне нравится»",
                    text,
                    widgets::action(
                        Kind::Primary,
                        Some(Icon::Retry),
                        "Повторить",
                        Some(Lib(Message::RetryLikes)),
                    ),
                )
            }
            Likes::Ready(tracks) if tracks.is_empty() => column![
                container(liked_header(tracks, false)).padding(Padding {
                    top: 28.0,
                    right: 32.0,
                    bottom: 0.0,
                    left: 32.0,
                }),
                state_block(
                    widgets::badge(Icon::Heart, theme::TEXT_SECONDARY, theme::SURFACE_2),
                    "Здесь пока пусто",
                    "Отмечайте треки сердцем — они появятся в «Мне нравится».".to_owned(),
                    widgets::action(
                        Kind::Secondary,
                        Some(Icon::Search),
                        "Найти музыку",
                        Some(Lib(Message::Nav(Section::Search))),
                    ),
                ),
            ]
            .into(),
            Likes::Ready(tracks) => column![
                container(liked_header(tracks, true)).padding(Padding {
                    top: 28.0,
                    right: 32.0,
                    bottom: 0.0,
                    left: 32.0,
                }),
                container(list_header()).padding(Padding {
                    top: 20.0,
                    right: 32.0,
                    bottom: 0.0,
                    left: 32.0,
                }),
                self.rows(tracks, covers),
            ]
            .into(),
        }
    }

    /// Видимые строки списка, выше и ниже — пустое место нужной высоты.
    fn rows<'a>(&'a self, tracks: &'a [Track], covers: &'a Covers) -> Element<'a, App> {
        let (first, last) = self.visible_range();
        let mut list = column![Space::new().height(LIST_TOP + rows_height(first))];
        for (index, track) in tracks
            .get(first..last)
            .unwrap_or_default()
            .iter()
            .enumerate()
            .map(|(offset, track)| (first + offset, track))
        {
            list = list.push(self.track_row(index, track, covers));
        }
        list = list.push(Space::new().height(rows_height(tracks.len().saturating_sub(last)) + 8.0));
        scrollable(container(list).padding(Padding {
            top: 0.0,
            right: 32.0,
            bottom: 0.0,
            left: 32.0,
        }))
        .direction(scrollable::Direction::Vertical(
            scrollable::Scrollbar::new()
                .width(4)
                .scroller_width(4)
                .margin(5),
        ))
        .on_scroll(|viewport| Lib(Message::Scrolled(viewport)))
        .height(Length::Fill)
        .style(|_, status| {
            let active = !matches!(status, scrollable::Status::Active { .. });
            let rail = scrollable::Rail {
                background: None,
                border: Border::default(),
                scroller: scrollable::Scroller {
                    background: (if active {
                        theme::TEXT_DISABLED
                    } else {
                        theme::SURFACE_4
                    })
                    .into(),
                    border: Border::default().rounded(2),
                },
            };
            scrollable::Style {
                container: container::Style::default(),
                vertical_rail: rail,
                horizontal_rail: rail,
                gap: None,
                auto_scroll: scrollable::AutoScroll {
                    background: theme::SURFACE_3.into(),
                    border: Border::default().rounded(widgets::ROUND),
                    shadow: widgets::raised_shadow(),
                    icon: theme::TEXT_PRIMARY,
                },
            }
        })
        .into()
    }

    fn track_row<'a>(
        &'a self,
        index: usize,
        track: &'a Track,
        covers: &'a Covers,
    ) -> Element<'a, App> {
        let current = self.player.current == Some(index);
        let selected = self.selected == Some(index);
        let make = |hovered: bool| -> Element<'a, App> {
            let background = match (selected, hovered) {
                (true, _) => Some(theme::SURFACE_3),
                (false, true) => Some(theme::SURFACE_2),
                (false, false) => None,
            };
            container(track_cells(
                index,
                track,
                covers,
                current,
                self.player.playing,
                hovered,
            ))
            .height(ROW)
            .padding(Padding {
                top: 0.0,
                right: 16.0,
                bottom: 0.0,
                left: 8.0,
            })
            .center_y(ROW)
            .style(move |_| container::Style {
                background: background.map(Background::Color),
                border: Border::default().rounded(widgets::RADIUS_M),
                ..container::Style::default()
            })
            .into()
        };
        mouse_area(hover(make(false), make(true)))
            .on_press(Lib(Message::Select(index)))
            .on_double_click(Lib(Message::Play(index)))
            .into()
    }

    fn settings(&self) -> Element<'_, App> {
        let account = self.status.account();
        let name = account.name();
        let initial = name
            .chars()
            .next()
            .map(|ch| ch.to_uppercase().to_string())
            .unwrap_or_default();
        let plus = match self.status.subscription() {
            Subscription::Active(details) => {
                let mut text = "Подписка Плюс активна".to_owned();
                if details.family {
                    text.push_str(", семейная");
                }
                if let Some(until) = details.until {
                    text.push_str(&format!(", до {until}"));
                }
                text
            }
            _ => "Подписка Плюс не подтверждена".to_owned(),
        };
        let account_card = container(
            row![
                avatar(initial, 44.0),
                column![
                    theme::line(name, Style::BodyStrong, theme::TEXT_PRIMARY),
                    theme::line(plus, Style::Secondary, theme::ACCENT),
                ]
                .spacing(2)
                .width(Length::Fill),
                widgets::action(
                    Kind::Danger,
                    Some(Icon::Logout),
                    "Выйти",
                    Some(App::OpenDialog(Dialog::ConfirmLogout)),
                ),
            ]
            .spacing(14)
            .align_y(Alignment::Center),
        )
        .padding(16)
        .width(Length::Fill)
        .style(|_| surface_2(14.0));
        let about = button(
            row![
                icon(Icon::About, 20.0, theme::TEXT_SECONDARY),
                theme::line("О программе", Style::Body, theme::TEXT_PRIMARY).width(Length::Fill),
                theme::line(
                    concat!("Версия ", env!("CARGO_PKG_VERSION")),
                    Style::Secondary,
                    theme::TEXT_TERTIARY
                ),
                icon(Icon::ChevronDown, 18.0, theme::TEXT_TERTIARY)
                    .rotation(Radians(-std::f32::consts::FRAC_PI_2)),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
        )
        .padding([14, 16])
        .width(Length::Fill)
        .style(|_, status| button::Style {
            background: Some(
                if status == button::Status::Hovered {
                    theme::SURFACE_3
                } else {
                    theme::SURFACE_2
                }
                .into(),
            ),
            border: Border::default().rounded(12),
            ..button::Style::default()
        })
        .on_press(App::OpenDialog(Dialog::About));
        scrollable(
            column![
                theme::line("Настройки", Style::Display, theme::TEXT_PRIMARY),
                column![
                    theme::line("Аккаунт", Style::Title, theme::TEXT_PRIMARY),
                    account_card,
                ]
                .spacing(16),
                about,
            ]
            .spacing(28)
            .width(Length::Fill)
            .padding(Padding {
                top: 28.0,
                right: 32.0,
                bottom: 28.0,
                left: 32.0,
            }),
        )
        .into()
    }

    fn player_bar<'a>(&'a self, covers: &'a Covers, compact: bool) -> Element<'a, App> {
        let player = &self.player;
        let track = self.current();
        let has_track = track.is_some();
        let on = |message: Message| has_track.then_some(Lib(message));

        let (cover, title, artist): (Element<'a, App>, String, String) = match track {
            Some(track) => (
                widgets::cover(cover_art(covers, track), 56.0, 8.0),
                track.display_title(),
                track.artists_text(),
            ),
            None => (
                container(center(icon(Icon::Empty, 24.0, theme::TEXT_SECONDARY)))
                    .width(56)
                    .height(56)
                    .style(|_| surface_2(8.0))
                    .into(),
                "Ничего не играет".to_owned(),
                "Выберите трек или включите «Мою волну»".to_owned(),
            ),
        };
        // Длинное название не обрезается, а прокручивается: его видно целиком.
        let now_playing = row![
            cover,
            column![
                marquee(title, Style::BodyStrong, theme::TEXT_PRIMARY),
                marquee(artist, Style::Secondary, theme::TEXT_SECONDARY),
            ]
            .spacing(2)
            .width(Length::Fill),
        ]
        .spacing(12)
        .align_y(Alignment::Center)
        .width(Length::FillPortion(if compact { 3 } else { 1 }));

        let play_icon = if player.playing {
            Icon::PauseFilled
        } else {
            Icon::PlayFilled
        };
        let play = button(center(icon(
            play_icon,
            22.0,
            if has_track {
                theme::ON_ACCENT
            } else {
                theme::TEXT_DISABLED
            },
        )))
        .width(48)
        .height(48)
        .padding(0)
        .style(|_, status| button::Style {
            background: Some(
                match status {
                    button::Status::Active => theme::ACCENT,
                    button::Status::Hovered => theme::ACCENT_HOVER,
                    button::Status::Pressed => theme::ACCENT_PRESSED,
                    button::Status::Disabled => theme::SURFACE_3,
                }
                .into(),
            ),
            border: Border::default().rounded(widgets::ROUND),
            ..button::Style::default()
        })
        .on_press_maybe(on(Message::TogglePlay));
        let repeat_icon = if player.repeat == Repeat::One {
            Icon::RepeatOne
        } else {
            Icon::Repeat
        };
        // «Не нравится» заработает с «Моей волной» (этап 8). В узком окне его
        // нет: место нужнее названию трека.
        let mut buttons = row![].spacing(8).align_y(Alignment::Center);
        if !compact {
            buttons = buttons.push(widgets::icon_button(Icon::Dislike, Size::S, false, None));
        }
        let buttons = buttons.extend([
            widgets::icon_button(
                Icon::Shuffle,
                Size::S,
                player.shuffle,
                on(Message::ToggleShuffle),
            ),
            widgets::icon_button(Icon::PreviousFilled, Size::M, false, on(Message::Previous)),
            play.into(),
            widgets::icon_button(Icon::NextFilled, Size::M, false, on(Message::Next)),
            widgets::icon_button(
                repeat_icon,
                Size::S,
                player.repeat != Repeat::Off,
                on(Message::CycleRepeat),
            ),
            widgets::icon_button(Icon::HeartFilled, Size::S, has_track, on(Message::Noop)),
        ]);

        let total = track
            .and_then(|track| track.duration)
            .map_or(0.0, |duration| duration.as_secs_f32());
        let position = player.position.min(total);
        let seek = row![
            theme::line(clock(position), Style::Caption, theme::TEXT_TERTIARY)
                .width(36)
                .align_x(Alignment::End),
            bar(0.0..=total.max(1.0), position, has_track, |value| {
                Lib(Message::Seek(value))
            }),
            theme::line(clock(total), Style::Caption, theme::TEXT_TERTIARY).width(36),
        ]
        .spacing(10)
        .align_y(Alignment::Center);
        // Края плеера одинаковой ширины, поэтому кнопки по центру. В узком
        // окне левому краю с названием отдаём больше.
        let controls = column![buttons, seek]
            .spacing(4)
            .align_x(Alignment::Center)
            .width(if compact { 300 } else { 440 });

        let volume = if player.muted { 0.0 } else { player.volume };
        let volume_icon = match volume {
            v if v <= 0.0 => Icon::VolumeMute,
            v if v < 0.5 => Icon::VolumeLow,
            _ => Icon::Volume,
        };
        let mut extras = row![widgets::icon_button(Icon::Lyrics, Size::S, false, None)]
            .spacing(4)
            .align_y(Alignment::Center);
        if !compact {
            extras = extras
                .push(widgets::icon_button(Icon::Queue, Size::S, false, None))
                .push(widgets::icon_button(Icon::MiniPlayer, Size::S, false, None));
        }
        let extras = container(
            extras
                .push(widgets::icon_button(
                    volume_icon,
                    Size::S,
                    false,
                    Some(Lib(Message::ToggleMute)),
                ))
                .push(
                    container(bar(0.0..=1.0, volume, true, |value| {
                        Lib(Message::Volume(value))
                    }))
                    .width(96),
                ),
        )
        .align_x(Alignment::End)
        .width(Length::FillPortion(if compact { 2 } else { 1 }));

        container(
            row![now_playing, controls, extras]
                .spacing(16)
                .padding([0, 16])
                .align_y(Alignment::Center),
        )
        .height(88)
        .center_y(88)
        .style(|_| container::Style {
            background: Some(theme::SURFACE_1.into()),
            border: Border::default().rounded(16),
            ..container::Style::default()
        })
        .into()
    }
}

/// Сколько целых строк помещается в `px`. Отрицательное и не число — ноль.
fn rows_in(px: f32) -> usize {
    if px.is_finite() && px > 0.0 {
        // Значение неотрицательное и не больше высоты списка: потерь нет.
        (px / ROW) as usize
    } else {
        0
    }
}

/// Высота `count` строк. Строк меньше 2^24, поэтому точность не теряется.
fn rows_height(count: usize) -> f32 {
    count as f32 * ROW
}

fn clock(seconds: f32) -> String {
    // Отрицательное и не число превращаются в ноль.
    texts::clock(seconds.max(0.0) as u64)
}

fn surface_2(radius: f32) -> container::Style {
    container::Style {
        background: Some(theme::SURFACE_2.into()),
        border: Border::default().rounded(radius),
        ..container::Style::default()
    }
}

fn avatar<'a>(initial: String, size: f32) -> Element<'a, App> {
    container(theme::line(initial, Style::BodyStrong, theme::TEXT_PRIMARY))
        .center(size)
        .style(|_| container::Style {
            background: Some(theme::avatar()),
            border: Border::default().rounded(widgets::ROUND),
            ..container::Style::default()
        })
        .into()
}

fn cover_art<'a>(covers: &'a Covers, track: &Track) -> CoverArt<'a> {
    match covers.art(track.cover_uri.as_deref()) {
        Art::Ready(handle) => CoverArt::Image(handle),
        Art::Loading => CoverArt::Loading,
        Art::Missing => CoverArt::Placeholder(theme::cover_placeholder(&track.id)),
    }
}

/// Пункт меню. Свёрнутое меню — только иконка с подсказкой.
/// Кнопка «свернуть / развернуть меню». Фон непрозрачный: в свёрнутом меню
/// кнопка ложится поверх логотипа.
fn sidebar_toggle<'a>(collapsed: bool) -> Element<'a, App> {
    let (name, tip) = if collapsed {
        (Icon::Forward, "Развернуть меню")
    } else {
        (Icon::Back, "Свернуть меню")
    };
    let toggle = button(center(icon(name, 18.0, theme::TEXT_PRIMARY)))
        .width(32)
        .height(32)
        .padding(0)
        .style(|_, status| button::Style {
            background: Some(
                match status {
                    button::Status::Hovered | button::Status::Pressed => theme::SURFACE_3,
                    _ => theme::SURFACE_2,
                }
                .into(),
            ),
            border: Border::default().rounded(widgets::ROUND),
            ..button::Style::default()
        })
        .on_press(App::ToggleSidebar);
    widgets::with_tooltip(toggle, tip, tooltip::Position::Right)
}

fn nav_item<'a>(section: Section, selected: bool, compact: bool) -> Element<'a, App> {
    let make = move |hovered: bool| -> Element<'a, App> {
        let (icon_color, text_color, font) = match (selected, hovered) {
            (true, _) => (theme::ACCENT, theme::TEXT_PRIMARY, Style::BodyStrong),
            (false, true) => (theme::TEXT_PRIMARY, theme::TEXT_PRIMARY, Style::Body),
            (false, false) => (theme::TEXT_SECONDARY, theme::TEXT_SECONDARY, Style::Body),
        };
        let glyph = icon(section.icon(), 20.0, icon_color);
        let content: Element<'a, App> = if compact {
            center(glyph).into()
        } else {
            row![glyph, theme::line(section.title(), font, text_color)]
                .spacing(12)
                .align_y(Alignment::Center)
                .into()
        };
        button(container(content).center_y(Length::Fill))
            .width(Length::Fill)
            .height(if compact { 44 } else { 40 })
            .padding([0, 12])
            .style(move |_, status| button::Style {
                background: match (selected, status) {
                    (true, _) => Some(theme::ACCENT_SOFT.into()),
                    (false, button::Status::Hovered) => Some(theme::SURFACE_2.into()),
                    (false, button::Status::Pressed) => Some(theme::SURFACE_3.into()),
                    _ => None,
                },
                border: Border::default().rounded(widgets::RADIUS_M),
                ..button::Style::default()
            })
            .on_press(Lib(Message::Nav(section)))
            .into()
    };
    let item = if selected {
        make(false)
    } else {
        hover(make(false), make(true))
    };
    if compact {
        widgets::with_tooltip(item, section.title(), tooltip::Position::Right)
    } else {
        item
    }
}

fn liked_header<'a>(tracks: &[Track], with_actions: bool) -> Element<'a, App> {
    // `sum` паникует при переполнении, а длительность приходит от сервера.
    let total = tracks
        .iter()
        .filter_map(|track| track.duration)
        .fold(Duration::ZERO, Duration::saturating_add);
    let count = tracks.len();
    let mut subtitle = format!("{count} {}", texts::tracks_word(count));
    if count > 0 {
        subtitle.push_str(&format!(" · {}", texts::total_duration(total)));
    }
    let tile = container(center(icon(Icon::HeartFilled, 40.0, theme::ON_ACCENT)))
        .width(88)
        .height(88)
        .style(|_| container::Style {
            background: Some(theme::liked_tile()),
            border: Border::default().rounded(14),
            ..container::Style::default()
        });
    let mut header = row![
        tile,
        column![
            theme::line("Коллекция", Style::Caption, theme::TEXT_TERTIARY),
            theme::line("Мне нравится", Style::Display, theme::TEXT_PRIMARY),
            theme::line(subtitle, Style::Secondary, theme::TEXT_SECONDARY),
        ]
        .spacing(2)
        .width(Length::Fill),
    ]
    .spacing(20)
    .align_y(Alignment::Center);
    if with_actions {
        header = header
            .push(widgets::action(
                Kind::Primary,
                Some(Icon::PlayFilled),
                "Слушать",
                Some(Lib(Message::PlayAll)),
            ))
            .push(widgets::icon_button(
                Icon::Shuffle,
                Size::M,
                false,
                Some(Lib(Message::Noop)),
            ))
            .push(widgets::icon_button(
                Icon::More,
                Size::M,
                false,
                Some(Lib(Message::Noop)),
            ));
    }
    header.into()
}

/// Колонки строки: номер 32, обложка 40, название, альбом 240, «нравится» 32,
/// длительность 44.
fn columns<'a>(
    index: Element<'a, App>,
    cover: Element<'a, App>,
    title: Element<'a, App>,
    album: Element<'a, App>,
    like: Element<'a, App>,
    duration: Element<'a, App>,
) -> Element<'a, App> {
    row![
        container(index).center(32),
        container(cover).width(40),
        container(title).width(Length::Fill).clip(true),
        container(album).width(240).clip(true),
        container(like).center(32),
        container(duration).width(44).align_x(Alignment::End),
    ]
    .spacing(16)
    .align_y(Alignment::Center)
    .into()
}

fn list_header<'a>() -> Element<'a, App> {
    let caption = |text: &'a str| theme::line(text, Style::Caption, theme::TEXT_TERTIARY);
    column![
        container(columns(
            caption("#").into(),
            Space::new().into(),
            caption("Название").into(),
            caption("Альбом").into(),
            Space::new().into(),
            icon(Icon::Clock, 16.0, theme::TEXT_TERTIARY).into(),
        ))
        .height(36)
        .center_y(36)
        .padding(Padding {
            top: 0.0,
            right: 16.0,
            bottom: 0.0,
            left: 8.0,
        }),
        widgets::divider(),
    ]
    .into()
}

fn track_cells<'a>(
    index: usize,
    track: &'a Track,
    covers: &'a Covers,
    current: bool,
    playing: bool,
    hovered: bool,
) -> Element<'a, App> {
    let available = track.available;
    let (title_color, other_color) = match (available, current) {
        (false, _) => (theme::TEXT_DISABLED, theme::TEXT_DISABLED),
        (true, true) => (theme::ACCENT, theme::TEXT_SECONDARY),
        (true, false) => (theme::TEXT_PRIMARY, theme::TEXT_SECONDARY),
    };
    let number = index.saturating_add(1).to_string();
    // Играющий: столбики, при наведении — пауза; на паузе — play акцентом.
    let index_cell: Element<'a, App> = match (available, current, playing, hovered) {
        (true, true, true, true) => {
            play_cell(Icon::PauseFilled, theme::TEXT_PRIMARY, Message::TogglePlay)
        }
        (true, true, true, false) => equalizer(),
        (true, true, false, _) => play_cell(Icon::PlayFilled, theme::ACCENT, Message::TogglePlay),
        (true, false, _, true) => {
            play_cell(Icon::PlayFilled, theme::TEXT_PRIMARY, Message::Play(index))
        }
        _ => theme::line(number, Style::Secondary, theme::TEXT_TERTIARY).into(),
    };
    let cover = if available {
        widgets::cover(cover_art(covers, track), 40.0, widgets::RADIUS_S)
    } else {
        widgets::block(40, 40, theme::SURFACE_3, widgets::RADIUS_S)
    };
    let title = column![
        theme::line(track.display_title(), Style::Body, title_color),
        theme::line(track.artists_text(), Style::Secondary, other_color),
    ]
    .spacing(2);
    let album = theme::line(
        track.album_title.clone().unwrap_or_default(),
        Style::Secondary,
        other_color,
    );
    let like = icon(
        Icon::HeartFilled,
        18.0,
        if available {
            theme::ACCENT
        } else {
            theme::TEXT_DISABLED
        },
    );
    let duration = theme::line(
        track.duration_text(),
        Style::Secondary,
        theme::TEXT_TERTIARY,
    );
    let cells = columns(
        index_cell,
        cover,
        title.into(),
        album.into(),
        like.into(),
        duration.into(),
    );
    if available {
        cells
    } else {
        widgets::with_tooltip(
            cells,
            "Трек недоступен: его убрали из каталога или он недоступен в вашем регионе",
            tooltip::Position::FollowCursor,
        )
    }
}

fn play_cell<'a>(name: Icon, color: Color, message: Message) -> Element<'a, App> {
    button(center(icon(name, 16.0, color)))
        .width(32)
        .height(32)
        .padding(0)
        .style(|_, _| button::Style::default())
        .on_press(Lib(message))
        .into()
}

/// Три столбика у играющего трека.
fn equalizer<'a>() -> Element<'a, App> {
    let bar = |height: f32| widgets::block(3, height, theme::ACCENT, 1.5);
    row![bar(10.0), bar(16.0), bar(7.0)]
        .spacing(2)
        .height(16)
        .align_y(Alignment::End)
        .into()
}

/// Ползунок перемотки или громкости: дорожка 4, ручка 12 при наведении.
fn bar<'a>(
    range: std::ops::RangeInclusive<f32>,
    value: f32,
    enabled: bool,
    on_change: impl Fn(f32) -> App + 'a,
) -> Element<'a, App> {
    slider(range, value, on_change)
        .step(0.01_f32)
        .height(16)
        .style(move |_, status| {
            let engaged = enabled && !matches!(status, slider::Status::Active);
            let filled = match (enabled, engaged) {
                (false, _) => theme::SURFACE_4,
                (true, true) => theme::ACCENT,
                (true, false) => theme::TEXT_SECONDARY,
            };
            slider::Style {
                rail: slider::Rail {
                    backgrounds: (filled.into(), theme::SURFACE_4.into()),
                    width: 4.0,
                    border: Border::default().rounded(2),
                },
                handle: slider::Handle {
                    shape: slider::HandleShape::Circle {
                        radius: if engaged { 6.0 } else { 0.0 },
                    },
                    background: theme::TEXT_PRIMARY.into(),
                    border_width: 0.0,
                    border_color: Color::TRANSPARENT,
                },
            }
        })
        .into()
}

/// Блок по центру панели: иконка, заголовок, текст, кнопка.
fn state_block<'a>(
    badge: Element<'a, App>,
    title: &'a str,
    text: String,
    action: Element<'a, App>,
) -> Element<'a, App> {
    center(
        column![
            badge,
            theme::line(title, Style::Title, theme::TEXT_PRIMARY),
            theme::label(text, Style::Body, theme::TEXT_SECONDARY)
                .center()
                .width(380),
            Space::new().height(4),
            action,
        ]
        .spacing(12)
        .align_x(Alignment::Center),
    )
    .into()
}

/// Раздел, которого ещё нет.
fn placeholder<'a>(section: Section) -> Element<'a, App> {
    let when = match section {
        Section::Wave => "«Моя волна» появится в одной из следующих версий.",
        _ => "Этот раздел появится в одной из следующих версий.",
    };
    state_block(
        widgets::badge(section.icon(), theme::TEXT_SECONDARY, theme::SURFACE_2),
        section.title(),
        when.to_owned(),
        widgets::action(
            Kind::Secondary,
            Some(Icon::Heart),
            "Открыть «Мне нравится»",
            Some(Lib(Message::Nav(Section::Liked))),
        ),
    )
}

/// Скелетон шапки и строк, мерцает (прозрачность 0.5 → 1 → 0.5).
fn skeleton<'a>(turn: f32) -> Element<'a, App> {
    let alpha = 0.75 + 0.25 * (turn * std::f32::consts::TAU).cos();
    let shade = Color {
        a: alpha,
        ..theme::SURFACE_2
    };
    let bar =
        move |width: f32, height: f32| widgets::block(width, height, shade, widgets::RADIUS_S);
    let header = row![
        widgets::block(88, 88, shade, 14.0),
        column![bar(80.0, 12.0), bar(280.0, 24.0), bar(160.0, 12.0)].spacing(10),
    ]
    .spacing(20)
    .align_y(Alignment::Center);
    let mut rows = column![].spacing(0);
    for row_index in 0..12 {
        let long = row_index % 3 != 1;
        rows = rows.push(
            container(columns(
                bar(16.0, 10.0),
                bar(40.0, 40.0),
                column![
                    bar(if long { 260.0 } else { 180.0 }, 12.0),
                    bar(if long { 140.0 } else { 110.0 }, 10.0)
                ]
                .spacing(8)
                .into(),
                bar(if long { 160.0 } else { 120.0 }, 10.0),
                Space::new().into(),
                bar(32.0, 10.0),
            ))
            .height(ROW)
            .center_y(ROW)
            .padding(Padding {
                top: 0.0,
                right: 16.0,
                bottom: 0.0,
                left: 8.0,
            }),
        );
    }
    column![
        header,
        Space::new().height(20),
        list_header(),
        Space::new().height(LIST_TOP),
        rows
    ]
    .spacing(0)
    .padding(Padding {
        top: 28.0,
        right: 32.0,
        bottom: 0.0,
        left: 32.0,
    })
    .into()
}
