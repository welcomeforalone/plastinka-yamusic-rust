//! Главное окно: боковое меню, «Мне нравится», настройки и панель плеера.
//!
//! Плеер ([`audio::Player`]) создаётся вместе с главным окном по пропуску
//! подписки. Окно шлёт ему команды и получает позицию и состояние через
//! подписку, само ничего не ждёт. Список длинный (до тысяч строк), поэтому
//! строятся только видимые строки.

use std::sync::Arc;
use std::time::Duration;

use iced::widget::{
    Space, button, center, column, container, hover, mouse_area, row, scrollable, slider, tooltip,
};
use iced::{
    Alignment, Background, Border, Color, Element, Length, Padding, Radians, Subscription, Task,
    mouse,
};

use super::Message as App;
use super::Message::Library as Lib;
use super::covers::{Art, Covers};
use super::icons::{Icon, icon, spinner};
use super::marquee::marquee;
use super::theme::{self, Style};
use super::widgets::{self, CoverArt, Kind, Size};
use super::{Dialog, Failure, FailureKind};
use crate::api::{self, AccountStatus, PlaybackPermit, Track};
use crate::player::{self as audio, Command, PlayError};
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
/// Ширина меню: развёрнутого и свёрнутого.
const SIDEBAR: f32 = 232.0;
const SIDEBAR_COLLAPSED: f32 = 72.0;
/// Кнопки и перемотка в середине плеера.
const CONTROLS: f32 = 440.0;
/// Поля плеера, промежутки между его частями и отступ справа от него.
const PLAYER_CHROME: f32 = 16.0 * 2.0 + 16.0 * 2.0 + 8.0;
/// Сколько нужно правому краю плеера с полным значком качества
/// («MP3 · 320») и с коротким («320») при узкой громкости.
const EXTRAS_WITH_QUALITY: f32 = 330.0;
const EXTRAS_WITH_SHORT_QUALITY: f32 = 260.0;
/// Ширина ползунка громкости: обычная и рядом с коротким значком.
const VOLUME_WIDTH: f32 = 96.0;
const VOLUME_WIDTH_NARROW: f32 = 72.0;

/// Какой значок качества помещается в плеер.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Quality {
    Hidden,
    Short,
    Full,
}

/// Где на экране плеер: отступы слева и справа. Сообщения выводятся по его
/// центру.
pub fn player_insets(collapsed: bool) -> (f32, f32) {
    (
        if collapsed {
            SIDEBAR_COLLAPSED
        } else {
            SIDEBAR
        },
        8.0,
    )
}
/// Громкость при запуске.
const START_VOLUME: f32 = 0.7;
/// Шаг громкости колесом мыши над плеером.
const VOLUME_STEP: f32 = 0.05;
/// «Назад» дальше этой секунды трека возвращает к его началу.
const RESTART_AFTER: f32 = 3.0;
/// Сколько неиграющих треков подряд пропускать самим.
const MAX_SKIPS: usize = 5;

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
    /// Ползунок перемотки тянут: показываем время, плеер не трогаем.
    SeekDrag(f32),
    /// Ползунок отпустили.
    SeekCommit,
    Volume(f32),
    /// Колесо мыши над плеером: вверх — громче.
    VolumeScroll(f32),
    ToggleMute,
    /// Перемешать и слушать.
    ShuffleAll,
    Audio(audio::Event),
    Noop,
}

/// Что главное окно просит сделать приложение.
pub enum Effect {
    /// Токен больше не действует.
    Relogin,
    /// Всплывающее сообщение.
    Toast(String),
}

enum Likes {
    Loading { skeleton: bool },
    Ready(Arc<Vec<Track>>),
    Failed(Failure),
}

/// Состояние плеера, как его видит окно.
struct Player {
    current: Option<usize>,
    /// Должен звучать (не пауза).
    playing: bool,
    /// Трек открыт в плеере: паузу и перемотку можно слать ему. После конца
    /// или ошибки трек надо запускать заново.
    loaded: bool,
    /// Звук ждёт сеть.
    buffering: bool,
    position: f32,
    /// Куда отпустят ползунок перемотки, пока его тянут.
    seeking: Option<f32>,
    volume: f32,
    muted: bool,
    shuffle: bool,
    repeat: Repeat,
    /// Битрейт текущего потока для значка качества.
    kbps: Option<u32>,
    /// Номер запуска трека: события прежних запусков не учитываем.
    session: u64,
    /// Сколько перемоток отправлено в этом запуске. Позиция из события, в
    /// котором перемоток меньше, устарела.
    seeks: u64,
    /// Сколько неиграющих треков пропущено подряд.
    skipped: usize,
    /// В этом запуске звук уже шёл: ошибка — обрыв, а не «не включился».
    sounded: bool,
    /// Порядок треков при перемешивании.
    order: Vec<usize>,
}

impl Player {
    fn gain(&self) -> f32 {
        if self.muted { 0.0 } else { self.volume }
    }
}

pub struct Library {
    token: Arc<StoredToken>,
    status: AccountStatus,
    /// Плеер. Не запустился — текст ошибки для сообщения при попытке играть.
    engine: Result<audio::Player, String>,
    /// Состояние генератора для перемешивания.
    seed: u64,
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
        // Плеер создаётся только здесь и только с пропуском подписки.
        let engine = match http {
            Some(http) => audio::Player::new(&permit, http.clone(), token.clone())
                .map_err(|err| err.to_string()),
            None => Err("не удалось подготовить сетевое соединение".to_owned()),
        };
        if let Ok(engine) = &engine {
            engine.send(Command::Volume(START_VOLUME));
        }
        let mut library = Self {
            token,
            status,
            engine,
            seed: seed(),
            section: Section::Liked,
            likes: Likes::Loading { skeleton: false },
            selected: None,
            scroll_y: 0.0,
            viewport: window_height,
            player: Player {
                current: None,
                playing: false,
                loaded: false,
                buffering: false,
                position: 0.0,
                seeking: None,
                volume: START_VOLUME,
                muted: false,
                shuffle: false,
                repeat: Repeat::Off,
                kbps: None,
                session: 0,
                seeks: 0,
                skipped: 0,
                sounded: false,
                order: Vec::new(),
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

    /// Длительность текущего трека в секундах, если она известна.
    fn current_duration(&self) -> Option<f32> {
        self.current()
            .and_then(|track| track.duration)
            .map(|duration| duration.as_secs_f32())
            .filter(|seconds| *seconds > 0.0)
    }

    pub fn animating(&self) -> bool {
        matches!(self.likes, Likes::Loading { skeleton: true })
            || (self.player.playing && self.player.buffering)
    }

    /// События плеера: позиция, конец трека, ошибки.
    pub fn subscription(&self) -> Subscription<App> {
        match &self.engine {
            Ok(engine) => {
                Subscription::run_with(engine.events(), |events: &audio::Events| events.stream())
                    .map(|event| Lib(Message::Audio(event)))
            }
            Err(_) => Subscription::none(),
        }
    }

    fn send(&self, command: Command) {
        if let Ok(engine) = &self.engine {
            engine.send(command);
        }
    }

    pub fn update(
        &mut self,
        message: Message,
        http: Option<&reqwest::Client>,
    ) -> (Task<Message>, Option<Effect>) {
        let effect = match message {
            Message::Nav(section) => {
                // Список при возврате в раздел создаётся заново, с начала.
                if section != self.section {
                    self.scroll_y = 0.0;
                }
                self.section = section;
                None
            }
            Message::LikesLoaded(result) => match result {
                Ok(tracks) => {
                    self.likes = Likes::Ready(tracks);
                    self.selected = None;
                    self.stop();
                    None
                }
                Err(failure) if failure.kind == FailureKind::Unauthorized => Some(Effect::Relogin),
                Err(failure) => {
                    self.likes = Likes::Failed(failure);
                    None
                }
            },
            Message::ShowSkeleton => {
                if let Likes::Loading { skeleton } = &mut self.likes {
                    *skeleton = true;
                }
                None
            }
            Message::RetryLikes => return (self.load_likes(http), None),
            Message::Scrolled(viewport) => {
                self.scroll_y = viewport.absolute_offset().y;
                None
            }
            Message::Select(index) => {
                self.selected = Some(index);
                None
            }
            Message::Play(index) => {
                self.player.skipped = 0;
                self.start(index, 0.0)
            }
            Message::PlayAll => {
                self.player.skipped = 0;
                let first = if self.player.shuffle {
                    self.shuffle_order(None);
                    self.player.order.first().copied()
                } else {
                    self.tracks().iter().position(|track| track.available)
                };
                first.and_then(|index| self.start(index, 0.0))
            }
            Message::ShuffleAll => {
                self.player.shuffle = true;
                self.player.skipped = 0;
                self.shuffle_order(None);
                let first = self.player.order.first().copied();
                first.and_then(|index| self.start(index, 0.0))
            }
            Message::TogglePlay => self.toggle_play(),
            Message::Previous => match self.player.current {
                Some(_) if self.player.position > RESTART_AFTER => {
                    self.seek_to(0.0);
                    None
                }
                Some(index) => {
                    self.player.skipped = 0;
                    self.step(index, false)
                }
                None => None,
            },
            Message::Next => {
                self.player.skipped = 0;
                self.player.current.and_then(|index| self.step(index, true))
            }
            Message::ToggleShuffle => {
                self.player.shuffle = !self.player.shuffle;
                if self.player.shuffle {
                    self.shuffle_order(self.player.current);
                }
                None
            }
            Message::CycleRepeat => {
                self.player.repeat = match self.player.repeat {
                    Repeat::Off => Repeat::All,
                    Repeat::All => Repeat::One,
                    Repeat::One => Repeat::Off,
                };
                None
            }
            Message::SeekDrag(position) => {
                if self.current_duration().is_some() {
                    self.player.seeking = Some(position);
                }
                None
            }
            Message::SeekCommit => {
                if let Some(position) = self.player.seeking.take() {
                    self.seek_to(position);
                }
                None
            }
            Message::Volume(volume) => {
                self.player.volume = volume;
                self.player.muted = volume <= 0.0;
                self.send(Command::Volume(self.player.gain()));
                None
            }
            Message::VolumeScroll(direction) => {
                let step = if direction > 0.0 {
                    VOLUME_STEP
                } else {
                    -VOLUME_STEP
                };
                let base = if self.player.muted {
                    0.0
                } else {
                    self.player.volume
                };
                self.player.volume = (base + step).clamp(0.0, 1.0);
                self.player.muted = self.player.volume <= 0.0;
                self.send(Command::Volume(self.player.gain()));
                None
            }
            Message::ToggleMute => {
                if self.player.volume <= 0.0 {
                    // Включить звук при нулевой громкости — вернуть громкость.
                    self.player.volume = UNMUTE_VOLUME;
                    self.player.muted = false;
                } else {
                    self.player.muted = !self.player.muted;
                }
                self.send(Command::Volume(self.player.gain()));
                None
            }
            Message::Audio(event) => self.audio_event(event),
            Message::Noop => None,
        };
        (Task::none(), effect)
    }

    /// Запускает трек `index` с секунды `from`.
    fn start(&mut self, index: usize, from: f32) -> Option<Effect> {
        let track_id = self
            .tracks()
            .get(index)
            .filter(|track| track.available)?
            .id
            .clone();
        let player = &mut self.player;
        player.current = Some(index);
        player.playing = true;
        player.loaded = true;
        player.buffering = true;
        player.position = from;
        player.seeking = None;
        player.kbps = None;
        player.sounded = false;
        player.session = player.session.wrapping_add(1);
        player.seeks = 0;
        self.selected = Some(index);
        let command = Command::Play {
            id: self.player.session,
            track_id,
            start: seconds(from),
        };
        match &self.engine {
            Ok(engine) => {
                engine.send(command);
                None
            }
            Err(text) => {
                let text = sentence(&format!("Не удалось включить звук: {text}"));
                self.halt();
                Some(Effect::Toast(text))
            }
        }
    }

    /// Трек остановлен: после конца или ошибки его надо запускать заново.
    fn halt(&mut self) {
        self.player.playing = false;
        self.player.loaded = false;
        self.player.buffering = false;
    }

    /// Ничего не играет: новый список.
    fn stop(&mut self) {
        if self.player.loaded {
            self.send(Command::Stop);
        }
        self.halt();
        self.player.current = None;
        self.player.position = 0.0;
        self.player.kbps = None;
        self.player.order.clear();
    }

    fn toggle_play(&mut self) -> Option<Effect> {
        let index = self.player.current?;
        if self.player.playing {
            self.player.playing = false;
            self.send(Command::Pause);
            None
        } else if self.player.loaded {
            self.player.playing = true;
            self.send(Command::Resume);
            None
        } else {
            // После конца или ошибки — заново, с той же секунды.
            self.player.skipped = 0;
            self.start(index, self.player.position)
        }
    }

    fn seek_to(&mut self, position: f32) {
        self.player.position = position;
        if self.player.loaded {
            self.player.seeks = self.player.seeks.wrapping_add(1);
            self.player.buffering = self.player.playing;
            self.send(Command::Seek(seconds(position)));
        }
    }

    fn audio_event(&mut self, event: audio::Event) -> Option<Effect> {
        let session = self.player.session;
        match event {
            audio::Event::Progress {
                id,
                position,
                buffering,
                kbps,
                seeks,
            } if id == session && seeks >= self.player.seeks => {
                self.player.position = position.as_secs_f32();
                self.player.buffering = buffering;
                self.player.kbps = kbps;
                if !buffering {
                    self.player.skipped = 0;
                    self.player.sounded = true;
                }
                None
            }
            audio::Event::Ended { id } if id == session => {
                self.halt();
                self.finished()
            }
            audio::Event::Failed { id, error } if id == session => self.failed(&error),
            _ => None,
        }
    }

    /// Трек доигран: дальше по повтору и списку.
    fn finished(&mut self) -> Option<Effect> {
        let index = self.player.current?;
        if self.player.repeat == Repeat::One {
            return self.start(index, 0.0);
        }
        match self.neighbour(index, true) {
            Some(next) => self.start(next, 0.0),
            None => {
                // Список кончился: трек остаётся выбранным, с начала.
                self.player.position = 0.0;
                None
            }
        }
    }

    fn failed(&mut self, error: &PlayError) -> Option<Effect> {
        self.halt();
        // Трек не играет — и качество его потока уже ни о чём не говорит.
        self.player.kbps = None;
        if *error == PlayError::Unauthorized {
            return Some(Effect::Relogin);
        }
        let index = self.player.current?;
        let title = self
            .tracks()
            .get(index)
            .map(Track::display_title)
            .unwrap_or_default();
        // С самим треком беда — пробуем следующий, но не бесконечно.
        if error.track_specific() && self.player.skipped < MAX_SKIPS {
            let next = self.neighbour(index, true).filter(|next| *next != index);
            if let Some(next) = next {
                self.player.skipped += 1;
                let effect = self.start(next, 0.0);
                return effect.or_else(|| {
                    Some(Effect::Toast(sentence(&format!(
                        "«{title}» не играет: {error}. Включаем следующий трек"
                    ))))
                });
            }
        }
        let text = if self.player.sounded {
            format!("«{title}» прервался: {error}")
        } else {
            format!("Не удалось включить «{title}»: {error}")
        };
        Some(Effect::Toast(sentence(&text)))
    }

    /// Соседний трек и его запуск.
    fn step(&mut self, from: usize, forward: bool) -> Option<Effect> {
        let next = self.neighbour(from, forward)?;
        self.start(next, 0.0)
    }

    /// Соседний доступный трек: по порядку перемешивания или по списку. С
    /// повтором списка — по кругу.
    fn neighbour(&mut self, from: usize, forward: bool) -> Option<usize> {
        let wrap = self.player.repeat == Repeat::All;
        if self.player.shuffle {
            if !self.player.order.contains(&from) {
                self.shuffle_order(Some(from));
            }
            let order = &self.player.order;
            let at = order.iter().position(|index| *index == from)?;
            let count = order.len();
            let next = match (forward, at) {
                (true, at) if at + 1 < count => at + 1,
                (true, _) if wrap => 0,
                (false, 0) if wrap => count.checked_sub(1)?,
                (false, at) if at > 0 => at - 1,
                _ => return None,
            };
            return order.get(next).copied();
        }
        let count = self.tracks().len();
        let mut index = from;
        for _ in 0..count {
            index = match (forward, index) {
                (true, i) if i + 1 < count => i + 1,
                (true, _) if wrap => 0,
                (false, 0) if wrap => count.saturating_sub(1),
                (false, i) if i > 0 => i - 1,
                _ => return None,
            };
            if self
                .tracks()
                .get(index)
                .is_some_and(|track| track.available)
            {
                return Some(index);
            }
        }
        None
    }

    /// Новый порядок перемешивания доступных треков. `first` — в начало.
    fn shuffle_order(&mut self, first: Option<usize>) {
        let mut order: Vec<usize> = self
            .tracks()
            .iter()
            .enumerate()
            .filter(|(_, track)| track.available)
            .map(|(index, _)| index)
            .collect();
        // Тасование Фишера — Йетса.
        for i in (1..order.len()).rev() {
            let bound = u64::try_from(i).unwrap_or(u64::MAX).saturating_add(1);
            let j = usize::try_from(self.random() % bound).unwrap_or(0);
            order.swap(i, j);
        }
        if let Some(first) = first
            && let Some(at) = order.iter().position(|index| *index == first)
        {
            order.swap(0, at);
        }
        self.player.order = order;
    }

    /// Простой генератор xorshift: для перемешивания его хватает.
    fn random(&mut self) -> u64 {
        let mut x = self.seed;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.seed = x;
        x
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
    /// меньше кнопок. `width` — ширина окна.
    pub fn view<'a>(
        &'a self,
        covers: &'a Covers,
        collapsed: bool,
        compact: bool,
        width: f32,
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
        // Значок качества — полный, если правый край плеера вмещает его;
        // иначе короткий при узкой громкости. Плеер стоит справа от меню, а
        // не во всю ширину, как в макете.
        let sidebar = if collapsed {
            SIDEBAR_COLLAPSED
        } else {
            SIDEBAR
        };
        let edge = (width - sidebar - PLAYER_CHROME - CONTROLS) / 2.0;
        let quality = match edge {
            _ if compact => Quality::Hidden,
            edge if edge >= EXTRAS_WITH_QUALITY => Quality::Full,
            edge if edge >= EXTRAS_WITH_SHORT_QUALITY => Quality::Short,
            _ => Quality::Hidden,
        };
        let right = column![panel, self.player_bar(covers, compact, quality, turn)]
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
        .width(if compact { SIDEBAR_COLLAPSED } else { SIDEBAR })
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
            api::Subscription::Active(details) => {
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

    /// `quality` — показать значок качества потока.
    fn player_bar<'a>(
        &'a self,
        covers: &'a Covers,
        compact: bool,
        quality: Quality,
        turn: f32,
    ) -> Element<'a, App> {
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

        let glyph_color = if has_track {
            theme::ON_ACCENT
        } else {
            theme::TEXT_DISABLED
        };
        // Пока звук ждёт сеть — спиннер, как в макете.
        let glyph = match (player.playing, player.buffering) {
            (true, true) => spinner(22.0, glyph_color, turn),
            (true, false) => icon(Icon::PauseFilled, 22.0, glyph_color),
            (false, _) => icon(Icon::PlayFilled, 22.0, glyph_color),
        };
        let play = button(center(glyph))
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

        // Без длительности трек играет, но перематывать некуда.
        let total = self.current_duration();
        let seekable = has_track && total.is_some();
        // Пока тянут ползунок, слева время точки, куда отпустят.
        let shown = player.seeking.unwrap_or(player.position);
        let position = total.map_or(shown, |total| shown.min(total));
        let total_text = match (track, total) {
            (Some(_), None) => "-:--".to_owned(),
            (_, total) => clock(total.unwrap_or(0.0)),
        };
        let range_end = total.unwrap_or(0.0).max(1.0);
        let seek = row![
            theme::line(clock(position), Style::Caption, theme::TEXT_TERTIARY)
                .width(36)
                .align_x(Alignment::End),
            bar(
                0.0..=range_end,
                position.min(range_end),
                seekable,
                |value| Lib(Message::SeekDrag(value)),
                Some(Lib(Message::SeekCommit)),
            ),
            theme::line(total_text, Style::Caption, theme::TEXT_TERTIARY).width(36),
        ]
        .spacing(10)
        .align_y(Alignment::Center);
        // Края плеера одинаковой ширины, поэтому кнопки по центру. В узком
        // окне левому краю с названием отдаём больше.
        let controls = column![buttons, seek]
            .spacing(4)
            .align_x(Alignment::Center)
            .width(if compact { 300.0 } else { CONTROLS });

        let volume = if player.muted { 0.0 } else { player.volume };
        let volume_icon = match volume {
            v if v <= 0.0 => Icon::VolumeMute,
            v if v < 0.5 => Icon::VolumeLow,
            _ => Icon::Volume,
        };
        let mut extras = row![].spacing(4).align_y(Alignment::Center);
        let shown = player
            .kbps
            .filter(|_| has_track && quality != Quality::Hidden);
        if let Some(kbps) = shown {
            extras = extras
                .push(quality_badge(kbps, quality == Quality::Full))
                .push(Space::new().width(8));
        }
        let volume_width = if shown.is_some() && quality == Quality::Short {
            VOLUME_WIDTH_NARROW
        } else {
            VOLUME_WIDTH
        };
        extras = extras.push(widgets::icon_button(Icon::Lyrics, Size::S, false, None));
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
                    container(bar(
                        0.0..=1.0,
                        volume,
                        true,
                        |value| Lib(Message::Volume(value)),
                        None,
                    ))
                    .width(volume_width),
                ),
        )
        .align_x(Alignment::End)
        .width(Length::FillPortion(if compact { 2 } else { 1 }));

        let bar = container(
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
        });
        // Колесо мыши над плеером меняет громкость.
        mouse_area(bar)
            .on_scroll(|delta| {
                let (mouse::ScrollDelta::Lines { y, .. } | mouse::ScrollDelta::Pixels { y, .. }) =
                    delta;
                if y == 0.0 {
                    Lib(Message::Noop)
                } else {
                    Lib(Message::VolumeScroll(y))
                }
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

/// Секунды ползунка в `Duration`. Отрицательное и не число — ноль.
fn seconds(value: f32) -> Duration {
    Duration::try_from_secs_f32(value.max(0.0)).unwrap_or_default()
}

/// Затравка генератора перемешивания: время запуска, не ноль.
fn seed() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.subsec_nanos());
    u64::from(nanos) | 1 << 63
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
            .push(widgets::with_tooltip(
                widgets::icon_button(
                    Icon::Shuffle,
                    Size::M,
                    false,
                    Some(Lib(Message::ShuffleAll)),
                ),
                "Перемешать и слушать",
                tooltip::Position::Bottom,
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
/// `on_release` — когда ползунок отпустили.
fn bar<'a>(
    range: std::ops::RangeInclusive<f32>,
    value: f32,
    enabled: bool,
    on_change: impl Fn(f32) -> App + 'a,
    on_release: Option<App>,
) -> Element<'a, App> {
    let mut slider = slider(range, value, on_change);
    if let Some(message) = on_release {
        slider = slider.on_release(message);
    }
    slider
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

/// Значок качества потока: «MP3 · 320» или, когда мало места, «320».
fn quality_badge<'a>(kbps: u32, full: bool) -> Element<'a, App> {
    let text = if full {
        format!("MP3 · {kbps}")
    } else {
        kbps.to_string()
    };
    widgets::with_tooltip(
        container(theme::line(
            text,
            Style::CaptionStrong,
            theme::TEXT_SECONDARY,
        ))
        .padding([3, 8])
        .style(|_| container::Style {
            border: Border::default()
                .rounded(widgets::RADIUS_S)
                .width(1)
                .color(theme::BORDER),
            ..container::Style::default()
        }),
        if full {
            "Качество потока"
        } else {
            "Качество потока: MP3, кбит/с"
        },
        tooltip::Position::Top,
    )
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
