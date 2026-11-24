//! Главное окно: боковое меню, разделы, страницы альбома, плейлиста и
//! исполнителя, настройки и панель плеера.
//!
//! Плеер ([`audio::Player`]) создаётся вместе с главным окном по пропуску
//! подписки. Окно шлёт ему команды и получает позицию и состояние через
//! подписку, само ничего не ждёт. Играет очередь ([`Queue`]) — список, из
//! которого запущен трек. Списки длинные (до тысяч строк), поэтому строятся
//! только видимые строки.

mod browse;
mod copies;
mod history_screen;
mod listen;
mod lyrics;
mod mini;
mod pages;
mod plugins;
mod queue_panel;
mod together;
mod wave;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use iced::widget::{
    Space, button, center, column, container, hover, mouse_area, row, scrollable, slider,
    text_input, tooltip, vertical_slider,
};
use iced::{Alignment, Border, Color, Element, Length, Padding, Point, Subscription, Task, mouse};

use super::Message as App;
use super::Message::Library as Lib;
use super::anim;
use super::covers::{Art, Avatar, Covers, Grade};
use super::filter;
use super::gate;
use super::history;
use super::history_store;
use super::icons::{Icon, icon, spinner};
use super::marquee::{marquee, marquee_element};
use super::palette;
use super::queue::{self, ListKey, Queue, Rng};
use super::scroll;
use super::settings;
use super::theme::{self, Style};
use super::visualizer::{self, Analyzer};
use super::widgets::{self, CoverArt, Kind, Size};
use super::{Failure, FailureKind, Moment, Prefs};
use crate::api::{
    self, AccountStatus, AudioQuality, Codec, PlaybackPermit, SearchKind, StreamInfo, Track,
};
use crate::media_controls;
use crate::player::eq::{EqSettings, EqState};
use crate::player::{self as audio, Command, PlayError, Sound};
use crate::plugins::PluginSettings;
use crate::plugins::fckcensor::{Censor, Decision};
use crate::plugins::slopless::Slop;
use crate::relink::Relinks;
use crate::storage::token::StoredToken;
use crate::sync::service::Repeat as TogetherRepeat;
use crate::texts::{self, sentence};
use iced::animation::Animation;
use iced::time::Instant;
pub use listen::Msg as ListenMsg;
pub use pages::Target;
use pages::{Collected, Collections, Entry, Loaded, PlaylistTab, SearchState, ShelfId};
pub use plugins::Msg as PluginMsg;
use plugins::{PluginIo, Replace, TrackMenu};
pub use together::Msg as TogetherMsg;
pub use wave::Msg as WaveMsg;
use wave::{Leave, Wave};

/// Высота строки трека.
const ROW: f32 = 56.0;
/// Сколько строк строить сверх видимых, сверху и снизу.
const OVERSCAN: usize = 8;
/// Отступ над первой строкой и под последней.
const LIST_TOP: f32 = 8.0;
const LIST_BOTTOM: f32 = 8.0;
/// Список для прокрутки по команде и поле поиска для Ctrl + F.
const LIST_ID: &str = "liked-list";
const SEARCH_ID: &str = "liked-search";
/// Высота поля поиска.
const SEARCH_HEIGHT: f32 = 44.0;
/// Отступ полосы прокрутки от правого края.
const RAIL_RIGHT: f32 = 6.0;
/// Громкость, которую возвращает кнопка звука, если громкость была нулевой.
const UNMUTE_VOLUME: f32 = 0.5;
/// Через сколько показывать скелетон: быстрые ответы не мигают.
const SKELETON_DELAY: Duration = Duration::from_millis(300);
/// Ширина меню: развёрнутого и свёрнутого.
pub const SIDEBAR: f32 = 232.0;
pub const SIDEBAR_COLLAPSED: f32 = 72.0;
/// Боковой отступ содержимого меню и сторона кнопки сворачивания.
const SIDEBAR_PAD: f32 = 12.0;
const TOGGLE: f32 = 32.0;
/// Уже этой ширины меню показывает только иконки (пока сворачивается).
const SIDEBAR_ICONS_BELOW: f32 = 150.0;
/// Кнопки и перемотка в середине плеера.
const CONTROLS: f32 = 440.0;
/// Поля плеера, промежутки между его частями и разделитель у меню.
/// Непрозрачность столбиков спектра под плеером.
const SPECTRUM_ALPHA: f32 = 0.22;
/// Полноэкранный плеер: заголовок окна, строка с крестиком, плеер и
/// подписи под обложкой по высоте; поля по бокам; пределы обложки.
const FULLSCREEN_CHROME: f32 = gate::TITLE_BAR_HEIGHT + 40.0 + 89.0 + 110.0;
const FULLSCREEN_SIDE: f32 = 64.0;
const FULLSCREEN_COVER_MIN: f32 = 160.0;
const FULLSCREEN_COVER_MAX: f32 = 560.0;
/// Вуаль поверх размытого фона.
const FULLSCREEN_SCRIM: f32 = 0.25;
/// Затемнение заглушки на фоне: как у размытой обложки (120 из 256).
const PLACEHOLDER_DIM: f32 = 0.53;
const PLAYER_CHROME: f32 = 20.0 * 2.0 + 16.0 * 2.0 + 1.0;
/// Поля раздела: сверху и по бокам.
pub(super) const PAGE_TOP: f32 = 32.0;
pub(super) const PAGE_SIDE: f32 = 40.0;
/// Правый край плеера: промежуток между его элементами, ширина кнопки и
/// отступ после значка качества (`Space` в `player_bar`).
const EXTRAS_GAP: f32 = 4.0;
const EXTRAS_BUTTON: f32 = 32.0;
const EXTRAS_BADGE_SPACE: f32 = 8.0;
/// Оценка ширины значка качества: CaptionStrong 12 px полужирным, берём с
/// запасом на символ; поля кнопки 8 по бокам и рамка 1 с каждой стороны.
const BADGE_CHAR: f32 = 7.5;
const BADGE_FRAME: f32 = 8.0 * 2.0 + 2.0;
/// Ширина середины плеера в узком окне.
const CONTROLS_COMPACT: f32 = 300.0;
/// Панель «Настройки звука»: ширина карточки и отступ от краёв окна.
const SOUND_PANEL_WIDTH: f32 = 520.0;
const SOUND_PANEL_MARGIN: f32 = 16.0;
/// Высота плеера и его правый отступ.
const PLAYER_HEIGHT: f32 = 88.0;
const PLAYER_SIDE: f32 = 20.0;
/// Значок громкости: сторона кнопки; он последний справа в плеере.
const VOLUME_ICON_SIDE: f32 = 32.0;
/// Всплывающая панель громкости: размеры «таблетки», высота ползунка и
/// зазор до плеера (через него курсор переходит со значка на панель).
const VOLUME_PANEL_WIDTH: f32 = 44.0;
const VOLUME_PANEL_HEIGHT: f32 = 212.0;
const VOLUME_SLIDER_HEIGHT: f32 = 150.0;
const VOLUME_PANEL_GAP: f32 = 32.0;
/// Сколько панель громкости остаётся после ухода курсора.
const VOLUME_CLOSE_DELAY: Duration = Duration::from_millis(250);
/// Появление и исчезновение панели громкости.
const VOLUME_PANEL_FADE: Duration = Duration::from_millis(160);

/// Какой значок качества помещается в плеер.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Quality {
    #[default]
    Hidden,
    Short,
    Full,
}

/// Что из правого края плеера (кроме значка громкости, он есть всегда)
/// помещается по ширине.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ExtrasFit {
    more: bool,
    lyrics: bool,
    queue: bool,
    devices: bool,
    equalizer: bool,
    mini: bool,
    quality: Quality,
}

/// Ширина правого края плеера шириной `bar_width`: то, что остаётся после
/// полей и середины, делится между названием трека и краем в долях
/// `FillPortion` (1:1, а в узком окне 3:2).
fn extras_room(bar_width: f32, compact: bool) -> f32 {
    let controls = if compact { CONTROLS_COMPACT } else { CONTROLS };
    let free = (bar_width - PLAYER_CHROME - controls).max(0.0);
    if compact {
        free * 2.0 / 5.0
    } else {
        free / 2.0
    }
}

/// Ширина значка качества по тексту (оценка, не измерение).
fn badge_width(info: StreamInfo, full: bool) -> f32 {
    quality_text(info, full).chars().count() as f32 * BADGE_CHAR + BADGE_FRAME
}

/// Что помещается в правый край шириной `room`. Громкость есть всегда; к ней
/// по приоритету добавляются меню трека, текст песни, очередь, «Устройства»
/// («Слушать вместе»: ниже громкости, выше эквалайзера), эквалайзер (не
/// в полноэкранном плеере), мини-плеер, короткий значок качества и полный.
/// Приоритет строгий: если очередной элемент не влез, следующие не берутся.
fn extras_fit(room: f32, fullscreen: bool, badge: Option<StreamInfo>) -> ExtrasFit {
    let mut left = room - EXTRAS_BUTTON;
    let mut open = true;
    let mut take = |width: f32| {
        let fits = open && left >= width;
        if fits {
            left -= width;
        }
        open = fits;
        fits
    };
    let step = EXTRAS_BUTTON + EXTRAS_GAP;
    let more = take(step);
    let lyrics = take(step);
    let queue = take(step);
    let devices = take(step);
    let equalizer = !fullscreen && take(step);
    let mini = take(step);
    let mut fit = ExtrasFit {
        more,
        lyrics,
        queue,
        devices,
        equalizer,
        mini,
        quality: Quality::Hidden,
    };
    // Значок, пустое место после него и два промежутка.
    let tail = EXTRAS_BADGE_SPACE + 2.0 * EXTRAS_GAP;
    if let Some(info) = badge.filter(|_| mini) {
        let short = badge_width(info, false) + tail;
        let full = badge_width(info, true) + tail;
        if left >= full {
            fit.quality = Quality::Full;
        } else if left >= short {
            fit.quality = Quality::Short;
        }
    }
    fit
}

/// Где на экране плеер: отступы слева и справа при ширине меню `sidebar`.
/// Сообщения выводятся по его центру.
pub fn player_insets(sidebar: f32) -> (f32, f32) {
    (sidebar + 1.0, 0.0)
}
/// Громкость при запуске.
const START_VOLUME: f32 = 0.7;
/// Раздел, с которого открывается окно после входа: «Моя волна», как в
/// официальном клиенте. Волна сама не запускается — только её экран.
const START_SECTION: Section = Section::Wave;
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
    History,
    /// YandexMusicTime: время прослушивания и серия.
    Stats,
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
            Self::History => "История",
            Self::Stats => "Статистика",
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
            Self::History => Icon::History,
            Self::Stats => Icon::Stats,
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
    /// Перейти на страницу альбома, исполнителя или плейлиста.
    Open(Target),
    /// Открыть и сразу включить (кнопка Play на карточке).
    PlayCard(Target),
    Back,
    /// Пришли данные страницы: её номер, номер страницы списка, данные.
    PageLoaded(u64, u32, Loaded),
    RetryPage,
    CollectionLoaded(Collected),
    RetryCollection,
    /// Вкладка «Вы собрали» или «Вам понравились» в разделе «Плейлисты».
    PlaylistTab(PlaylistTab),
    /// «Показать все» или «Свернуть» у полки коллекции.
    ToggleShelf(ShelfId),
    /// Поиск по каталогу: текст в поле, ввод замер, Enter, вкладка, ответ.
    SearchInput(String),
    SearchSettled(u64),
    SearchSubmit,
    SearchTab(SearchKind),
    SearchLoaded(u64, SearchKind, u32, Result<api::SearchResult, Failure>),
    ClearCatalogSearch,
    RetrySearch,
    Scrolled(scrollable::Viewport),
    /// Щелчки колеса над списком (вниз — больше нуля): плавная прокрутка.
    Wheel(f32),
    /// Ручку полосы прокрутки тянут: список сразу на этот сдвиг.
    ScrollTo(f32),
    /// Поиск трека в списке: новый текст запроса.
    Search(String),
    ClearSearch,
    /// Строка списка выбрана.
    Select(ListKey, usize),
    /// Включить трек списка.
    PlayFrom(ListKey, usize),
    /// «Слушать»: весь список с начала.
    PlayList(ListKey),
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
    /// Громкость из системного плеера (MPRIS): панель громкости не трогаем.
    VolumeSet(f32),
    /// Колесо мыши над плеером: вверх — громче.
    VolumeScroll(f32),
    ToggleMute,
    /// «Перемешать»: весь список вперемешку.
    ShuffleList(ListKey),
    /// Мышь над ползунком перемотки или ушла с него.
    SeekHover(bool),
    /// Открыть панель «Настройки звука» (или закрыть, если открыта).
    ToggleSoundPanel,
    CloseSoundPanel,
    /// Мышь над значком громкости или ушла с него.
    VolumeHover(bool),
    /// Мышь над панелью громкости или ушла с неё.
    VolumePanelHover(bool),
    /// Ползунок громкости отпустили.
    VolumeRelease,
    /// Открыть или закрыть полноэкранный плеер.
    Fullscreen(bool),
    Audio(audio::Event),
    /// Плагины: загрузка данных, меню трека, замены.
    Plugin(plugins::Msg),
    /// «Моя волна».
    Wave(wave::Msg),
    /// «Слушать вместе»: события службы и кнопки.
    Together(together::Msg),
    /// Текст песни: панель, загрузка.
    Lyrics(lyrics::Msg),
    /// Сердце: поставить или снять «Нравится» треку (настоящему треку из
    /// списка, не его подмене FckCensor).
    ToggleLike(Track),
    /// Серверная история: ответ (номер запроса), «Повторить», играть трек
    /// (день, группа, номер в группе).
    HistoryLoaded(u64, Result<Arc<Vec<api::HistoryDay>>, Failure>),
    HistoryReload,
    PlayHistoryItem(usize, usize, usize),
    /// Прослушивания прошлых запусков прочитаны с диска.
    LocalPlaysLoaded(Vec<history::LocalPlay>),
    /// YandexMusicTime: файл прочитан, тик, запись закончилась.
    Listen(listen::Msg),
    /// Аватар аккаунта из Яндекс ID (`None` — нет или не загрузился).
    AvatarLoaded(Option<Arc<Avatar>>),
    /// Играть с трека очереди (панель «Очередь»).
    PlayQueueAt(usize),
    /// Играть трек из «Недавно играли» (номер в истории).
    PlayHistory(usize),
    /// Открыть мини-плеер (окно открывает приложение: см. `Effect::OpenMini`).
    MiniRequest,
    /// Команда системного плеера: медиаклавиши, оверлей, MPRIS.
    Media(media_controls::Command),
    /// Перемотка на позицию (секунды), без ползунка.
    SeekTo(f32),
    /// Ответ сервера на сердце: трек, какое состояние просили, результат.
    LikeDone(Track, bool, Result<(), Failure>),
    Noop,
}

/// Что главное окно просит сделать приложение.
pub enum Effect {
    /// Открыть окно мини-плеера.
    OpenMini,
    /// Выбор в меню настроек волны изменился: запомнить в `config.toml`.
    WaveSettings(api::WaveSettings),
    /// Токен больше не действует.
    Relogin,
    /// Всплывающее сообщение.
    Toast(String),
    /// Новое сопряжение: запомнить в `config.toml`.
    TogetherPaired(crate::storage::config::PairedDevice),
    /// Открыть диалог (подтверждение восстановления статистики).
    OpenDialog(super::Dialog),
    /// Закрыть диалог и показать сообщение, если оно есть.
    DialogDone(Option<String>),
}

enum Likes {
    Loading { skeleton: bool },
    Ready(Arc<Vec<Track>>),
    Failed(Failure),
}

/// Состояние плеера, как его видит окно. Что играет — в очереди.
struct Player {
    /// Должен звучать (не пауза).
    playing: bool,
    /// Трек открыт в плеере: паузу и перемотку можно слать ему. После конца
    /// или ошибки трек надо запускать заново.
    loaded: bool,
    /// Звук ждёт сеть.
    buffering: bool,
    position: f32,
    /// Когда пришла последняя позиция: от неё текст песни ведёт время сам.
    position_at: Instant,
    /// Куда отпустят ползунок перемотки, пока его тянут.
    seeking: Option<f32>,
    volume: f32,
    muted: bool,
    shuffle: bool,
    repeat: Repeat,
    /// Кодек и битрейт текущего потока для значка качества.
    stream: Option<StreamInfo>,
    /// Номер запуска трека: события прежних запусков не учитываем.
    session: u64,
    /// Сколько перемоток отправлено в этом запуске. Позиция из события, в
    /// котором перемоток меньше, устарела.
    seeks: u64,
    /// Сколько неиграющих треков пропущено подряд.
    skipped: usize,
    /// В этом запуске звук уже шёл: ошибка — обрыв, а не «не включился».
    sounded: bool,
}

impl Player {
    /// Новая позиция из события плеера, перемотки или начала трека.
    fn set_position(&mut self, seconds: f32) {
        self.position = seconds;
        self.position_at = Instant::now();
    }

    fn gain(&self) -> f32 {
        if self.muted { 0.0 } else { self.volume }
    }
}

pub struct Library {
    token: Arc<StoredToken>,
    status: AccountStatus,
    /// Плеер. Не запустился — текст ошибки для сообщения при попытке играть.
    engine: Result<audio::Player, String>,
    /// Генератор для перемешивания.
    rng: Rng,
    section: Section,
    /// Переезд подсветки меню: из какого раздела и когда начался.
    nav_glide: Option<(Section, Instant)>,
    /// Открытые страницы поверх раздела, последняя — на экране.
    stack: Vec<Entry>,
    /// Последний выданный номер страницы.
    serial: u64,
    /// Прокрутка раздела, пока поверх него открыта страница.
    root_scroll: f32,
    likes: Likes,
    /// Идентификаторы треков из «Мне нравится»: сердце в строках.
    liked: HashSet<String>,
    /// Треки, чей запрос сердца ещё идёт: повторные нажатия по ним
    /// игнорируются, пока не придёт ответ.
    like_busy: HashSet<String>,
    collections: Collections,
    search: SearchState,
    /// Что играет и что дальше.
    queue: Option<Queue>,
    selected: Option<(ListKey, usize)>,
    scroll_y: f32,
    /// Высота окна: список заведомо не выше, поэтому строк построится с
    /// запасом. Высоту самого списка `scrollable` сообщает только при
    /// прокрутке, а после увеличения окна она бы устарела.
    viewport: f32,
    /// Ширина раздела справа от меню: по ней строится сетка карточек.
    panel_width: f32,
    player: Player,
    motion: PlayerMotion,
    /// Отвод звука плеера и спектр для визуализатора.
    tap: Option<Arc<audio::Tap>>,
    spectrum: Analyzer,
    /// Полноэкранный плеер открыт (выезжает снизу).
    fullscreen: Animation<bool>,
    /// Плавная прокрутка списка колесом, пока она идёт.
    glide: Option<scroll::Glide>,
    /// Высота видимой части списка, по последнему сообщению `scrollable`.
    list_height: Option<f32>,
    /// Поиск трека: запрос, строки треков для поиска и номера найденных
    /// (`None` — запрос пустой, видно всё).
    query: String,
    haystacks: Vec<String>,
    shown: Option<Vec<usize>>,
    /// Соединение для запросов, которые окно запускает само (плагины).
    http: Option<reqwest::Client>,
    /// Аватар аккаунта из Яндекс ID; только в памяти. Библиотека живёт до
    /// выхода из аккаунта, так что при выходе он исчезает вместе с ней.
    avatar: Option<Arc<Avatar>>,
    /// Масштаб экрана ОС (DPI окна); с масштабом приложения даёт размер
    /// кружков в физических пикселях.
    dpi: f32,
    plugins: PluginSettings,
    /// Настройки звука: качество (уходит плееру) и нормализация громкости.
    quality: AudioQuality,
    normalize: bool,
    /// Эквалайзер включён (подсветка кнопки в плеере).
    eq_enabled: bool,
    censor: Censor,
    /// Копии недоступных треков из других релизов.
    relinks: Relinks,
    slop: Slop,
    io: PluginIo,
    /// Задачи, которые набрались за одно сообщение (поиск версии без
    /// цензуры, «Не нравится»): уходят вместе с ответом.
    tasks: Vec<Task<Message>>,
    /// Где последний раз нажали мышь: там открывается меню трека.
    pointer: Point,
    menu: Option<TrackMenu>,
    replace: Option<Replace>,
    /// Трек, который включили вручную: Slopless его не пропускает.
    chosen: Option<String>,
    /// «Моя волна»: сессия, данные очереди, события обратной связи.
    wave: Wave,
    /// Текст песни: панель справа и загрузка.
    lyrics: lyrics::State,
    /// Последние прослушанные треки за сеанс (панель «Очередь»).
    history: history::History,
    /// Настоящий трек, который сейчас запущен: когда он сменится другим, он
    /// уходит в историю, если успел зазвучать.
    now_playing: Option<Track>,
    /// Серверная история (вкладка «История») и номер её последнего запроса.
    server_history: history_screen::HistoryLoad,
    history_serial: u64,
    /// Что зазвучало за сеанс (вкладка «История»: сервер показывает не сразу).
    local_plays: history::LocalPlays,
    /// Файл прослушиваний (`None` — тестовые хуки или нет каталога кэша) и
    /// прочитан ли он: до этого не пишем, иначе затрём прошлые запуски.
    plays_file: Option<std::path::PathBuf>,
    plays_restored: bool,
    /// Вкладка «История» на экране: сервер плюс сеанс, без дублей.
    history_shown: Arc<Vec<api::HistoryDay>>,
    /// Откуда построена очередь (контекст для вкладки «История»).
    play_context: api::HistoryContext,
    /// Сообщения Яндексу о прослушивании (`play-audio`).
    plays: api::PlayTracker,
    /// «Слушать вместе»: служба, связь, ведущий и ведомый.
    together: together::Together,
    /// YandexMusicTime: время прослушивания и серия дней.
    listen: listen::Listen,
}

/// Анимации плеера.
struct PlayerMotion {
    /// Нажатие Play: кнопка коротко сжимается.
    pulse: Animation<bool>,
    /// Прежний трек, чья обложка тает, и ход затухания.
    cover_from: Option<Track>,
    /// Трек, обложка которого сейчас в плеере.
    cover_now: Option<Track>,
    cover_fade: Animation<bool>,
    /// Ручка и заливка ползунков проявляются при наведении.
    seek_hover: Animation<bool>,
    /// Панель «Настройки звука»: открыта или закрыта.
    sound_panel: Animation<bool>,
    /// Всплывающая панель громкости: показана или спрятана.
    volume_panel: Animation<bool>,
    /// Курсор над значком громкости и над панелью; ползунок тянут.
    volume_over_icon: bool,
    volume_over_panel: bool,
    volume_dragging: bool,
    /// Когда панель закрыть, если курсор так и не вернулся.
    volume_close_at: Option<Instant>,
    /// Громкость на ползунке: после щелчка по иконке и колеса — плавно.
    volume: Animation<f32>,
}

impl PlayerMotion {
    fn new(volume: f32) -> Self {
        Self {
            pulse: anim::settled(true, anim::PANEL),
            cover_from: None,
            cover_now: None,
            cover_fade: anim::settled(true, anim::COVER),
            seek_hover: anim::settled(false, anim::HOVER),
            sound_panel: anim::popover(false),
            volume_panel: anim::settled(false, VOLUME_PANEL_FADE),
            volume_over_icon: false,
            volume_over_panel: false,
            volume_dragging: false,
            volume_close_at: None,
            volume: still_volume(volume),
        }
    }

    fn running(&self, now: Instant) -> bool {
        self.pulse.is_animating(now)
            || self.cover_fade.is_animating(now)
            || self.seek_hover.is_animating(now)
            || self.sound_panel.is_animating(now)
            || self.volume_panel.is_animating(now)
            || self.volume_close_at.is_some()
            || self.volume.is_animating(now)
    }

    /// Панель громкости открыта, пока курсор над значком или панелью либо
    /// ползунок тянут. Когда держать нечем, закрытие ставится через
    /// [`VOLUME_CLOSE_DELAY`]: курсору хватает времени перейти со значка на
    /// панель.
    fn volume_sync(&mut self, now: Instant) {
        let held = self.volume_over_icon || self.volume_over_panel || self.volume_dragging;
        if held {
            self.volume_close_at = None;
            if !self.volume_panel.value() {
                self.volume_panel.go_mut(true, now);
            }
        } else if self.volume_panel.value() && self.volume_close_at.is_none() {
            self.volume_close_at = Some(now + VOLUME_CLOSE_DELAY);
        }
    }

    /// Кадр: пора ли закрывать панель громкости.
    fn volume_tick(&mut self, now: Instant) {
        if let Some(at) = self.volume_close_at
            && now >= at
        {
            self.volume_close_at = None;
            self.volume_panel.go_mut(false, now);
        }
    }
}

/// Громкость в целых процентах для подписи на панели: без звука — 0.
fn volume_percent(volume: f32, muted: bool) -> u32 {
    if muted || !volume.is_finite() {
        return 0;
    }
    // Значение уже зажато в 0…100, потери при приведении нет.
    (volume.clamp(0.0, 1.0) * 100.0).round() as u32
}

/// Громкость без анимации: ползунок тянут, он идёт за мышью.
fn still_volume(volume: f32) -> Animation<f32> {
    Animation::new(volume)
        .duration(anim::PANEL)
        .easing(iced::animation::Easing::EaseOutCubic)
}

/// Отладочный хук `PLASTINKA_TEST_START` (только с тестовыми хуками, один
/// раз за запуск): сообщения, которые окно получило бы от пользователя.
/// `play` — играть «Мне нравится» и открыть этот список (замеры
/// `bench/tools`); `liked-wave` — играть «Мне нравится» и открыть «Мою волну»; `wave` —
/// запустить «Мою волну»; `lyrics` — играть «Мне нравится» и открыть панель
/// текста; `queue` — то же с панелью очереди; `fullscreen` — то же с полноэкранным плеером; `wave-menu` —
/// запустить «Мою волну» и открыть меню её настроек. Ничего не сохраняет.
#[cfg(debug_assertions)]
fn test_start() -> Vec<Task<Message>> {
    use std::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if !crate::storage::test_hooks_active() || DONE.swap(true, Ordering::Relaxed) {
        return Vec::new();
    }
    match std::env::var("PLASTINKA_TEST_START").as_deref() {
        Ok("play") => vec![
            Task::done(Message::PlayList(ListKey::Liked)),
            Task::done(Message::Nav(Section::Liked)),
        ],
        Ok("liked-wave") => vec![
            Task::done(Message::PlayList(ListKey::Liked)),
            Task::done(Message::Nav(Section::Wave)),
        ],
        Ok("lyrics") => vec![
            Task::done(Message::PlayList(ListKey::Liked)),
            Task::done(Message::Lyrics(lyrics::Msg::Toggle)),
        ],
        Ok("history") => vec![Task::done(Message::Nav(Section::History))],
        Ok("stats") => vec![Task::done(Message::Nav(Section::Stats))],
        Ok("queue") => vec![
            Task::done(Message::PlayList(ListKey::Liked)),
            Task::done(Message::Lyrics(lyrics::Msg::ToggleQueue)),
        ],
        Ok("fullscreen") => vec![
            Task::done(Message::PlayList(ListKey::Liked)),
            Task::done(Message::Fullscreen(true)),
        ],
        Ok("mini") => vec![
            Task::done(Message::PlayList(ListKey::Liked)),
            Task::done(Message::MiniRequest),
        ],
        Ok("wave-menu") => vec![
            Task::done(Message::Nav(Section::Wave)),
            Task::done(Message::Wave(WaveMsg::Start)),
            Task::done(Message::Wave(WaveMsg::Menu)),
        ],
        Ok("wave") => vec![
            Task::done(Message::Nav(Section::Wave)),
            Task::done(Message::Wave(WaveMsg::Start)),
        ],
        _ => Vec::new(),
    }
}

impl Library {
    pub fn new(
        token: Arc<StoredToken>,
        status: AccountStatus,
        permit: PlaybackPermit,
        window_height: f32,
        http: Option<&reqwest::Client>,
        plugins: &PluginSettings,
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
        let tap = engine.as_ref().ok().map(audio::Player::tap);
        let mut library = Self {
            token,
            status,
            engine,
            rng: Rng::new(),
            section: START_SECTION,
            nav_glide: None,
            stack: Vec::new(),
            serial: 0,
            root_scroll: 0.0,
            likes: Likes::Loading { skeleton: false },
            liked: HashSet::new(),
            like_busy: HashSet::new(),
            collections: Collections::default(),
            search: SearchState::new(),
            queue: None,
            selected: None,
            scroll_y: 0.0,
            viewport: window_height,
            panel_width: 1280.0 - SIDEBAR,
            player: Player {
                playing: false,
                loaded: false,
                buffering: false,
                position: 0.0,
                position_at: Instant::now(),
                seeking: None,
                volume: START_VOLUME,
                muted: false,
                shuffle: false,
                repeat: Repeat::Off,
                stream: None,
                session: 0,
                seeks: 0,
                skipped: 0,
                sounded: false,
            },
            motion: PlayerMotion::new(START_VOLUME),
            tap,
            spectrum: Analyzer::default(),
            fullscreen: anim::settled(false, anim::FULLSCREEN),
            glide: None,
            list_height: None,
            query: String::new(),
            haystacks: Vec::new(),
            shown: None,
            http: http.cloned(),
            avatar: None,
            dpi: 1.0,
            plugins: PluginSettings::default(),
            quality: AudioQuality::default(),
            normalize: false,
            eq_enabled: false,
            censor: Censor::default(),
            relinks: Relinks::default(),
            slop: Slop::default(),
            io: PluginIo::default(),
            tasks: Vec::new(),
            pointer: Point::ORIGIN,
            menu: None,
            replace: None,
            chosen: None,
            wave: Wave::new(),
            lyrics: lyrics::State::new(),
            history: history::History::default(),
            now_playing: None,
            server_history: history_screen::HistoryLoad::Idle,
            history_serial: 0,
            local_plays: history::LocalPlays::default(),
            plays_file: history_store::system_path(),
            plays_restored: false,
            history_shown: Arc::new(Vec::new()),
            play_context: history_screen::liked_context(),
            plays: api::PlayTracker::default(),
            together: together::Together::new(),
            listen: listen::Listen::new(),
        };
        let task = library.load_likes(http);
        let plugins = library.plugins_changed(plugins);
        // Серверная история нужна панели «Очередь» («Недавно играли»): один
        // раз в фоне, ошибка тихая.
        let history = library.history_load(http);
        // Прослушивания прошлых запусков — с диска, в фоне.
        let restored = Task::perform(
            history_store::load_in_background(library.plays_file.clone()),
            Message::LocalPlaysLoaded,
        );
        // Занятия для меню «Моей волны» — заранее: иначе первое открытие
        // меню показывает его без раздела «Занятие», и оно вырастает.
        let stations = library.wave_load_stations();
        // Аватар из Яндекс ID грузится отдельно и экран не задерживает.
        let avatar = library.avatar_load(http);
        // Статистика прослушиваний — с диска, в фоне.
        let listen = Library::listen_load();
        // Кэш копий недоступных треков — с диска, в фоне.
        let relinks = Task::perform(crate::relink::load(), |cache| {
            Message::Plugin(plugins::Msg::RelinkDisk(Box::new(cache)))
        });
        (
            library,
            Task::batch([
                task, plugins, history, restored, stations, avatar, relinks, listen,
            ]),
        )
    }

    /// Аватар аккаунта: адрес из Паспорта, затем картинка без токена. Любая
    /// ошибка даёт `None`, остаётся буква.
    fn avatar_load(&self, http: Option<&reqwest::Client>) -> Task<Message> {
        let Some(http) = http.cloned() else {
            return Task::none();
        };
        let token = self.token.access_token.clone();
        Task::perform(
            async move {
                let url = api::fetch_avatar_url(&http, &token).await?;
                super::covers::load_avatar(&http, &url).await
            },
            Message::AvatarLoaded,
        )
    }

    pub fn avatar(&self) -> Option<&Avatar> {
        self.avatar.as_deref()
    }

    pub fn set_dpi(&mut self, dpi: f32) {
        if dpi.is_finite() && dpi > 0.0 {
            self.dpi = dpi;
        }
    }

    /// Физических пикселей в логическом: масштаб приложения × DPI окна.
    pub fn pixel_scale(&self, prefs: &Prefs) -> f32 {
        prefs.scale as f32 / 100.0 * self.dpi
    }

    /// Статус аккаунта и подписки (для «Настройки → Аккаунт»).
    pub(super) fn status(&self) -> &AccountStatus {
        &self.status
    }

    /// Открыта ли страница настроек (не страница альбома и т. п.).
    pub fn settings_open(&self) -> bool {
        self.section == Section::Settings && self.stack.is_empty()
    }

    pub fn token(&self) -> Arc<StoredToken> {
        self.token.clone()
    }

    pub fn resized(&mut self, window_height: f32) {
        self.viewport = window_height;
    }

    /// Ширина окна и меню (уже после сворачивания): по ним строится сетка.
    pub fn set_layout(&mut self, window_width: f32, sidebar: f32) {
        self.panel_width = (window_width - sidebar - 1.0).max(0.0);
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

    /// «Мне нравится».
    fn tracks(&self) -> &[Track] {
        match &self.likes {
            Likes::Ready(tracks) => tracks,
            _ => &[],
        }
    }

    fn liked_tracks(&self) -> Option<Arc<Vec<Track>>> {
        match &self.likes {
            Likes::Ready(tracks) => Some(tracks.clone()),
            _ => None,
        }
    }

    /// Трек в плеере.
    fn current(&self) -> Option<&Track> {
        self.queue.as_ref().and_then(Queue::track)
    }

    /// Строка `index` списка `key` с треком `track` — играющий трек. В том
    /// же списке сравниваем место, в другом — сам трек.
    fn is_current(&self, key: &ListKey, index: usize, track: &Track) -> bool {
        let Some(queue) = &self.queue else {
            return false;
        };
        if queue.key == *key {
            queue.current == index
        } else {
            queue.track().is_some_and(|playing| playing.id == track.id)
        }
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
            || self.loading_on_screen()
            || self.lyrics_loading()
            || self.history_loading()
            || self.listen_loading()
            || self.wave_animating()
    }

    /// Насколько выехал полноэкранный плеер: 0 — его нет, 1 — во всё окно.
    pub fn fullscreen_shown(&self, now: Instant) -> f32 {
        anim::progress(&self.fullscreen, now)
    }

    /// Esc: закрыть полноэкранный плеер. `false` — он и не был открыт.
    pub fn close_fullscreen(&mut self) -> bool {
        let open = self.fullscreen.value();
        if open {
            self.fullscreen.go_mut(false, Instant::now());
        }
        open
    }

    /// Большая обложка, которая нужна полноэкранному плееру сейчас.
    pub fn fullscreen_cover(&self) -> Option<&str> {
        if !self.fullscreen.value() {
            return None;
        }
        self.current_shown()
            .and_then(|track| track.cover_uri.as_deref())
    }

    /// Нужны ли кадры визуализатору: играет звук или столбики опускаются.
    pub fn visualizing(&self, enabled: bool) -> bool {
        enabled && (self.player.playing || self.spectrum.moving())
    }

    /// Кадр визуализатора. `enabled` — он включён в настройках.
    /// Кадр: спектр (`enabled` — визуализатор включён) и плавная прокрутка.
    pub fn frame(&mut self, now: Instant, enabled: bool) -> Task<Message> {
        self.motion.volume_tick(now);
        if enabled {
            let tap = self.tap.as_deref().filter(|_| self.player.playing);
            self.spectrum.frame(tap, now);
        } else {
            self.spectrum.clear();
        }
        let Some(glide) = &mut self.glide else {
            return Task::none();
        };
        let (offset, done) = glide.step(now);
        if done {
            self.glide = None;
        }
        scroll_list(offset)
    }

    /// Высота прокручиваемой части экрана со всеми строками.
    fn list_content(&self) -> f32 {
        self.body().content()
    }

    /// Сколько строк в списке с учётом поиска.
    fn shown_count(&self) -> usize {
        self.shown.as_ref().map_or(self.tracks().len(), Vec::len)
    }

    /// Номер трека в строке `row` списка с учётом поиска.
    fn shown_at(&self, row: usize) -> Option<usize> {
        match &self.shown {
            Some(shown) => shown.get(row).copied(),
            None => (row < self.tracks().len()).then_some(row),
        }
    }

    /// Новый запрос: список фильтруется и встаёт в начало.
    fn search(&mut self, query: String) -> Task<Message> {
        self.query = query;
        self.shown = filter::matching(&self.haystacks, &self.query);
        self.selected = None;
        self.jump(0.0)
    }

    /// Esc: сбросить поиск — в «Мне нравится» или по каталогу. `None` —
    /// сбрасывать нечего.
    pub fn clear_search(&mut self) -> Option<Task<Message>> {
        if !self.stack.is_empty() {
            return None;
        }
        match self.section {
            Section::Liked if !self.query.is_empty() => Some(self.search(String::new())),
            Section::Search if !self.search.input.is_empty() => Some(self.clear_catalog_search()),
            _ => None,
        }
    }

    /// Ctrl + F: в «Мне нравится» — поиск трека в списке, в других
    /// разделах — поиск по каталогу.
    pub fn focus_search(&mut self, http: Option<&reqwest::Client>) -> Task<Message> {
        if self.section == Section::Liked && self.stack.is_empty() {
            return if self.tracks().is_empty() {
                Task::none()
            } else {
                iced::widget::operation::focus(SEARCH_ID)
            };
        }
        if self.section == Section::Search && self.stack.is_empty() {
            return iced::widget::operation::focus(pages::CATALOG_SEARCH_ID);
        }
        self.nav(Section::Search, http)
    }

    /// Поле поиска по каталогу очищено.
    fn clear_catalog_search(&mut self) -> Task<Message> {
        let input = self.search_input(String::new());
        Task::batch([
            input,
            iced::widget::operation::focus(pages::CATALOG_SEARCH_ID),
        ])
    }

    /// События плеера: позиция, конец трека, ошибки.
    pub fn subscription(&self) -> Subscription<App> {
        let audio = match &self.engine {
            Ok(engine) => {
                Subscription::run_with(engine.events(), |events: &audio::Events| events.stream())
                    .map(|event| Lib(Message::Audio(event)))
            }
            Err(_) => Subscription::none(),
        };
        // Ведущий раз в секунду сообщает позицию ведомому.
        let tick = if self.together_ticking() {
            iced::time::every(Duration::from_secs(1))
                .map(|_| Lib(Message::Together(together::Msg::Tick)))
        } else {
            Subscription::none()
        };
        Subscription::batch([audio, tick, self.listen_subscription()])
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
        // Пришли новые треки — FckCensor запоминает их подмены.
        let lists = matches!(
            message,
            Message::LikesLoaded(_)
                | Message::PageLoaded(..)
                | Message::SearchLoaded(..)
                | Message::Plugin(
                    PluginMsg::CensorDisk(_)
                        | PluginMsg::CensorRefreshed(_)
                        | PluginMsg::AlbumVolumes(..)
                )
        );
        let (task, effect) = self.handle(message, http);
        if lists {
            self.note_all();
        }
        let pump = self.pump();
        self.wave_pump();
        // Что у нас изменилось, уходит собеседнику.
        self.together_sync();
        // Время прослушивания: старт и остановка звука.
        self.listen_sync();
        let queued = Task::batch(std::mem::take(&mut self.tasks));
        // Текст песни: открыли панель или сменился трек — запрашиваем.
        let lyrics = self.lyrics_sync();
        (Task::batch([task, pump, queued, lyrics]), effect)
    }

    fn handle(
        &mut self,
        message: Message,
        http: Option<&reqwest::Client>,
    ) -> (Task<Message>, Option<Effect>) {
        // Кнопки ведомого, которые решает ведущий, уходят ему командой.
        if self.together_intercept(&message) {
            return (Task::none(), None);
        }
        let effect = match message {
            Message::Together(msg) => return self.together_msg(msg, http),
            Message::Listen(msg) => return self.listen_msg(msg),
            Message::Nav(section) => {
                self.close_wave_menu();
                return (self.nav(section, http), None);
            }
            Message::Open(target) => {
                // Из полноэкранного плеера (щелчок по исполнителю) — к
                // странице: плеер сворачивается. Из меню трека — меню
                // закрывается.
                self.menu = None;
                self.close_fullscreen();
                return (self.open(target, false, http), None);
            }
            Message::PlayCard(target) => return (self.open(target, true, http), None),
            Message::Back => return (self.back().unwrap_or_else(Task::none), None),
            Message::PageLoaded(serial, page, loaded) => {
                return self.page_loaded(serial, page, loaded, http);
            }
            Message::RetryPage => return (self.retry_page(http), None),
            Message::CollectionLoaded(collected) => self.collection_loaded(collected),
            Message::RetryCollection => {
                return (self.load_collection(self.section, true, http), None);
            }
            Message::SearchInput(text) => return (self.search_input(text), None),
            Message::SearchSettled(edit) => return (self.search_settled(Some(edit), http), None),
            Message::SearchSubmit => return (self.search_settled(None, http), None),
            Message::SearchTab(tab) => return (self.search_tab(tab, http), None),
            Message::PlaylistTab(tab) => {
                self.collections.playlist_tab = tab;
                None
            }
            Message::ToggleShelf(shelf) => {
                self.collections.toggle(shelf);
                None
            }
            Message::SearchLoaded(generation, kind, page, result) => {
                return self.search_loaded(generation, kind, page, result, http);
            }
            Message::ClearCatalogSearch => return (self.clear_catalog_search(), None),
            Message::RetrySearch => return (self.retry_search(http), None),
            Message::LikesLoaded(result) => match result {
                Ok(tracks) => {
                    self.haystacks = tracks.iter().map(filter::haystack).collect();
                    self.liked = tracks.iter().map(|track| track.id.clone()).collect();
                    self.likes = Likes::Ready(tracks);
                    self.shown = filter::matching(&self.haystacks, &self.query);
                    #[cfg(debug_assertions)]
                    self.tasks.extend(test_start());
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
                self.list_height = Some(viewport.bounds().height);
                // Цель за концом списка (высота была неизвестна) — правим.
                let max = self.max_scroll();
                if let Some(glide) = &mut self.glide
                    && glide.target() > max
                {
                    glide.retarget(max);
                }
                // Список прокрутили к концу — догружаем следующую страницу.
                return (self.more(http), None);
            }
            Message::Wheel(lines) => {
                let max = self.max_scroll();
                let base = self.glide.map_or(self.scroll_y, |glide| glide.target());
                let target = (base + lines * scroll::WHEEL_STEP).clamp(0.0, max);
                match &mut self.glide {
                    Some(glide) => glide.retarget(target),
                    None if (target - self.scroll_y).abs() >= 0.5 => {
                        self.glide = Some(scroll::Glide::new(self.scroll_y, target));
                    }
                    None => {}
                }
                None
            }
            Message::ScrollTo(offset) => {
                self.glide = None;
                return (scroll_list(offset.clamp(0.0, self.max_scroll())), None);
            }
            Message::Search(query) => return (self.search(query), None),
            Message::ClearSearch => return (self.search(String::new()), None),
            Message::Select(key, index) => {
                self.selected = Some((key, index));
                None
            }
            Message::PlayFrom(key, index) => self.play_from(key, index),
            Message::PlayList(key) => self.play_list(key, false),
            Message::ShuffleList(key) => self.play_list(key, true),
            Message::TogglePlay => {
                self.motion.pulse = anim::entering(anim::PANEL, Instant::now());
                self.toggle_play()
            }
            Message::SeekHover(over) => {
                self.motion.seek_hover.go_mut(over, Instant::now());
                None
            }
            Message::ToggleSoundPanel => {
                let open = !self.motion.sound_panel.value();
                self.motion.sound_panel.go_mut(open, Instant::now());
                None
            }
            Message::CloseSoundPanel => {
                self.close_sound_panel();
                None
            }
            Message::VolumeHover(over) => {
                self.motion.volume_over_icon = over;
                self.motion.volume_sync(Instant::now());
                None
            }
            Message::VolumePanelHover(over) => {
                self.motion.volume_over_panel = over;
                self.motion.volume_sync(Instant::now());
                None
            }
            Message::VolumeRelease => {
                self.motion.volume_dragging = false;
                self.motion.volume_sync(Instant::now());
                None
            }
            Message::Fullscreen(open) => {
                // Без трека смотреть не на что.
                if !open || self.current().is_some() {
                    self.fullscreen.go_mut(open, Instant::now());
                }
                None
            }
            Message::Lyrics(msg) => return self.lyrics_msg(msg),
            Message::PlayQueueAt(index) => self.play_queue_at(index),
            Message::HistoryLoaded(serial, result) => self.history_loaded(serial, result),
            Message::HistoryReload => return (self.history_load(http), None),
            Message::LocalPlaysLoaded(plays) => {
                self.local_plays_loaded(plays);
                None
            }
            Message::AvatarLoaded(handle) => {
                self.avatar = handle;
                None
            }
            Message::PlayHistoryItem(day, item, index) => self.play_history_item(day, item, index),
            Message::PlayHistory(index) => self.play_history(index),
            Message::MiniRequest => return (Task::none(), Some(Effect::OpenMini)),
            Message::Media(command) => {
                return match self.media_message(command) {
                    Some(message) => self.update(message, http),
                    None => (Task::none(), None),
                };
            }
            Message::SeekTo(position) => {
                let end = self
                    .current()
                    .and_then(|track| track.duration)
                    .map(|duration| duration.as_secs_f32());
                let position = position.max(0.0);
                self.seek_to(end.map_or(position, |end| position.min(end)));
                None
            }
            Message::Previous => match self.queue.as_ref().map(|queue| queue.current) {
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
                self.queue
                    .as_ref()
                    .map(|queue| queue.current)
                    .and_then(|index| self.step(index, true))
            }
            // В «Моей волне» перемешивания нет.
            Message::ToggleShuffle if self.in_wave() => None,
            Message::ToggleShuffle => {
                self.player.shuffle = !self.player.shuffle;
                if self.player.shuffle
                    && let Some(queue) = &mut self.queue
                {
                    let current = queue.current;
                    let censor = self.plugins.fckcensor.then_some(&self.censor);
                    let relinks = &self.relinks;
                    queue.shuffle(Some(current), &mut self.rng, &|track| {
                        plugins::playable(censor, relinks, track)
                    });
                }
                None
            }
            // В волне повтор переключается «выкл ↔ трек», а повтор списков
            // пользователя остаётся как был.
            Message::CycleRepeat if self.in_wave() => {
                self.cycle_wave_repeat();
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
                // Пока ползунок тянут, панель не закрывается.
                self.motion.volume_dragging = true;
                self.motion.volume_sync(Instant::now());
                self.player.volume = volume;
                self.player.muted = volume <= 0.0;
                self.motion.volume = still_volume(volume);
                self.send(Command::Volume(self.player.gain()));
                None
            }
            Message::VolumeSet(volume) => {
                self.player.volume = volume.clamp(0.0, 1.0);
                self.player.muted = self.player.volume <= 0.0;
                self.motion
                    .volume
                    .go_mut(self.player.gain(), Instant::now());
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
                self.motion
                    .volume
                    .go_mut(self.player.gain(), Instant::now());
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
                self.motion
                    .volume
                    .go_mut(self.player.gain(), Instant::now());
                self.send(Command::Volume(self.player.gain()));
                None
            }
            Message::Audio(event) => self.audio_event(event),
            Message::Plugin(msg) => return self.plugin_update(msg),
            Message::Wave(msg) => return self.wave_update(msg),
            Message::ToggleLike(track) => return self.toggle_like(track),
            Message::LikeDone(track, target, result) => {
                self.like_busy.remove(&track.id);
                match result {
                    Ok(()) => {
                        self.together_notify_liked(&track.id, target);
                        None
                    }
                    Err(failure) => {
                        // Сервер не принял: возвращаем сердце как было.
                        self.set_liked(&track, !target);
                        if failure.kind == FailureKind::Unauthorized {
                            Some(Effect::Relogin)
                        } else {
                            let action = if target {
                                "поставить «Нравится»"
                            } else {
                                "снять «Нравится»"
                            };
                            Some(Effect::Toast(sentence(&format!(
                                "Не удалось {action}: {}",
                                failure.text
                            ))))
                        }
                    }
                }
            }
            Message::Noop => None,
        };
        (Task::none(), effect)
    }

    /// Нажали сердце: сразу меняем сердце и список, потом шлём запрос.
    /// Пока запрос по треку идёт, повторное нажатие игнорируется: так
    /// состояние на экране не расходится с сервером.
    fn toggle_like(&mut self, track: Track) -> (Task<Message>, Option<Effect>) {
        if self.like_busy.contains(&track.id) {
            return (Task::none(), None);
        }
        let Some(http) = self.http.clone() else {
            let toast = Effect::Toast(sentence("не удалось подготовить сетевое соединение"));
            return (Task::none(), Some(toast));
        };
        let target = !self.liked.contains(&track.id);
        self.set_liked(&track, target);
        self.like_busy.insert(track.id.clone());
        let token = self.token.clone();
        let uid = self.status.account().uid;
        let ids = vec![api::queue_id(&track)];
        let task = Task::perform(
            async move {
                if target {
                    api::add_liked_tracks(&http, &token.access_token, uid, &ids).await
                } else {
                    api::remove_liked_tracks(&http, &token.access_token, uid, &ids).await
                }
                .map_err(Failure::from)
            },
            move |result| Message::LikeDone(track, target, result),
        );
        (task, None)
    }

    /// Показывает трек лайкнутым или нет: сердца и список «Мне нравится».
    /// Играющая очередь «Мне нравится» держит свой список, но номера в нём
    /// перестают совпадать с экраном, поэтому её отвязываем.
    fn set_liked(&mut self, track: &Track, liked: bool) {
        let list = match &mut self.likes {
            Likes::Ready(tracks) => Some(Arc::make_mut(tracks)),
            _ => None,
        };
        let changed = edit_likes(&mut self.liked, list, track, liked);
        if !changed {
            return;
        }
        self.haystacks = self.tracks().iter().map(filter::haystack).collect();
        self.shown = filter::matching(&self.haystacks, &self.query);
        if let Some(queue) = &mut self.queue
            && matches!(queue.key, ListKey::Liked | ListKey::LikedFound(_))
        {
            queue.key = ListKey::Detached;
        }
        if self
            .selected
            .as_ref()
            .is_some_and(|(key, _)| matches!(key, ListKey::Liked | ListKey::LikedFound(_)))
        {
            self.selected = None;
        }
    }

    /// Запускает трек `index` очереди с секунды `from`.
    fn start(&mut self, index: usize, from: f32) -> Option<Effect> {
        self.start_toward(index, from, true)
    }

    /// Запускает трек `index`. ИИ-треки (Slopless с автопропуском)
    /// пропускаются в сторону `forward`.
    fn start_toward(&mut self, index: usize, from: f32, forward: bool) -> Option<Effect> {
        let mut index = index;
        for _ in 0..plugins::MAX_AI_SKIPS {
            let track = self.queue.as_ref()?.get(index)?.clone();
            if !self.skip_as_ai(&track) {
                return self.launch(index, track, from);
            }
            self.dislike_ai(&track);
            let Some(next) = self.neighbour(index, forward).filter(|next| *next != index) else {
                // В волне за концом очереди будет догрузка.
                return if forward { self.wave_exhausted() } else { None };
            };
            index = next;
        }
        Some(Effect::Toast(
            "Дальше в очереди только ИИ-треки: Slopless их пропускает".to_owned(),
        ))
    }

    /// Новый трек запущен: считается играющим для сообщений Яндексу. Шлём
    /// только про поток Яндекса: свой файл и поиск версии без цензуры нет.
    fn report_begin(&mut self, track: &Track, decision: &Decision) {
        let (id, reportable) = match decision {
            Decision::Play(Sound::Track(id)) => (id.clone(), true),
            _ => (track.id.clone(), false),
        };
        let shown = self.shown(track);
        let length = shown.duration.or(track.duration);
        // У замены FckCensor другой трек: альбом оригинала к нему не относится.
        // У копии недоступного трека — свой альбом.
        let album = if id == track.id {
            track.album_id.clone()
        } else {
            self.relinks
                .copy_of(&track.id)
                .filter(|copy| copy.id == id)
                .and_then(|copy| copy.album_id.clone())
        };
        self.plays.begin(
            &id,
            album.as_deref(),
            length,
            reportable,
            api::new_play_id(),
        );
    }

    /// Звук пошёл: сообщение о начале трека.
    fn report_start(&mut self) {
        let uid = self.status.account().uid;
        if let Some(report) = self.plays.sounded(uid) {
            self.send_play_report(report);
        }
    }

    /// Трек сменился, кончился или встал: сообщение о сыгранном. `ended` —
    /// доигран до конца (сыграно всё), иначе по позиции.
    fn report_finish(&mut self, ended: bool) {
        let played = if ended {
            self.current_duration().unwrap_or(self.player.position)
        } else {
            self.player.position
        };
        let uid = self.status.account().uid;
        if let Some(report) = self.plays.finish(uid, played) {
            self.send_play_report(report);
        }
    }

    /// Отправка в фоне; ошибки молча (строка в stderr), игре не мешают.
    fn send_play_report(&mut self, report: api::PlayReport) {
        // Ответ Яндексу о прослушивании шлёт только ведущий.
        if self.together_follower() {
            return;
        }
        let Some(http) = self.http.clone() else {
            return;
        };
        let token = self.token.access_token.clone();
        self.tasks.push(Task::perform(
            api::send_quietly(http, token, report),
            |()| Message::Noop,
        ));
    }

    /// Запускает трек `track` (место `index` в очереди) с секунды `from`.
    fn launch(&mut self, index: usize, track: Track, from: f32) -> Option<Effect> {
        let decision = self.decide(&track);
        if decision == Decision::Silent {
            return None;
        }
        // Прежний трек закончил играть: сообщаем Яндексу, сколько сыграно
        // (до `report_begin`: тот заменяет играющий трек).
        self.report_finish(false);
        self.report_begin(&track, &decision);
        // Прежний трек волны уходит: событие `skip`, если он успел зазвучать.
        self.wave_leave(Leave::Skip);
        // Сменившийся трек — в историю (если он зазвучал и это не он же).
        if let Some(previous) = self.now_playing.take()
            && previous.id != track.id
        {
            self.history.record(previous, self.player.sounded);
        }
        self.now_playing = Some(track.clone());
        if let Some(queue) = &mut self.queue {
            queue.current = index;
        }
        let now = Instant::now();
        let shown = self.shown(&track).clone();
        let changed = self
            .motion
            .cover_now
            .as_ref()
            .is_none_or(|old| old.id != shown.id || old.cover_uri != shown.cover_uri);
        if changed {
            // Обложка прежнего трека тает под новой.
            self.motion.cover_from = self.motion.cover_now.replace(shown);
            self.motion.cover_fade = anim::entering(anim::COVER, now);
        }
        self.motion.pulse = anim::entering(anim::PANEL, now);
        let player = &mut self.player;
        player.playing = true;
        player.loaded = true;
        player.buffering = true;
        player.set_position(from);
        player.seeking = None;
        player.stream = None;
        player.sounded = false;
        player.session = player.session.wrapping_add(1);
        player.seeks = 0;
        if let Err(text) = &self.engine {
            let text = sentence(&format!("Не удалось включить звук: {text}"));
            self.halt();
            return Some(Effect::Toast(text));
        }
        match decision {
            Decision::Play(sound) => {
                let norm = self.sound_norm(&sound, &track);
                self.send(Command::Play {
                    id: self.player.session,
                    sound,
                    start: seconds(from),
                    norm,
                });
            }
            Decision::Search => {
                // Прежний трек замолкает, пока ищем версию без цензуры.
                self.send(Command::Stop);
                self.search_explicit(&track);
            }
            Decision::Silent => {}
        }
        None
    }

    /// Параметры эквалайзера: запоминаются и уходят плееру (при создании
    /// окна и при каждом изменении).
    pub fn set_equalizer(&mut self, settings: EqSettings) {
        self.eq_enabled = settings.enabled;
        self.send(Command::Equalizer(settings));
    }

    /// Настройки звука из `config.toml`: качество действует со следующего
    /// запроса потока, нормализация — со следующего трека.
    pub fn set_sound(&mut self, quality: AudioQuality, normalize: bool) {
        if self.quality != quality {
            self.quality = quality;
            self.send(Command::Quality(quality));
        }
        if self.normalize != normalize {
            self.normalize = normalize;
            // Нормализация действует на играющий трек сразу.
            let norm = self
                .current()
                .and_then(|track| match self.decide(track) {
                    Decision::Play(sound) => Some(self.sound_norm(&sound, track)),
                    _ => None,
                })
                .unwrap_or(1.0);
            self.send(Command::Norm(norm));
        }
    }

    /// Что слушаем сейчас — для статуса Discord. Начало трека считается по
    /// часам: сейчас минус позиция, поэтому на ходу времени оно не меняется.
    pub fn discord_state(&self, settings: crate::discord::Settings) -> crate::discord::State {
        let idle = crate::discord::State {
            settings,
            ..crate::discord::State::default()
        };
        // Трек остановлен (конец, ошибка) — статуса нет; пауза — другое дело.
        let (Some(shown), Some(current)) = (self.current_shown(), self.current()) else {
            return idle;
        };
        if !self.player.loaded {
            return idle;
        }
        let local = matches!(self.decide(current), Decision::Play(Sound::File(_)));
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
            .unwrap_or(0);
        let position_ms = (self.player.position.max(0.0) * 1000.0).round() as i64;
        crate::discord::State {
            settings,
            track: Some(crate::discord::Track {
                id: shown.id.clone(),
                title: shown.display_title(),
                artists: shown.artists_text(),
                album_id: shown.album_id.clone(),
                album: shown.album_title.clone(),
                cover: shown.cover_uri.clone(),
                duration_ms: shown
                    .duration
                    .and_then(|duration| u64::try_from(duration.as_millis()).ok()),
                local,
            }),
            playing: self.player.playing,
            start_ms: now_ms.saturating_sub(position_ms),
        }
    }

    /// Пропуск для системного плеера: есть, только пока Плюс подтверждён.
    pub fn media_permit(&self) -> Option<PlaybackPermit> {
        self.status.playback_permit()
    }

    /// Что делать окну по команде системного плеера. `None` — ничего (уже
    /// играет, уже на паузе).
    fn media_message(&self, command: media_controls::Command) -> Option<Message> {
        use media_controls::Command as C;
        let seek = |forward: bool, by: f32| {
            let by = if forward { by } else { -by };
            Message::SeekTo(self.player.position + by)
        };
        match command {
            C::Play => (!self.player.playing).then_some(Message::TogglePlay),
            C::Pause | C::Stop => self.player.playing.then_some(Message::TogglePlay),
            C::Toggle => Some(Message::TogglePlay),
            C::Next => Some(Message::Next),
            C::Previous => Some(Message::Previous),
            C::SeekTo(position) => Some(Message::SeekTo(position)),
            C::SeekBy(forward, by) => Some(seek(forward, by)),
            C::SetVolume(volume) => Some(Message::VolumeSet(volume)),
        }
    }

    /// Что сейчас играет — для системного плеера (медиаклавиши, оверлей).
    pub fn media_state(&self) -> media_controls::State {
        let volume_percent = volume_percent(self.player.volume, self.player.muted);
        // Трек остановлен (конец, ошибка) — как «ничего не играет».
        let idle = media_controls::State {
            volume_percent,
            ..media_controls::State::default()
        };
        let (Some(shown), Some(current)) = (self.current_shown(), self.current()) else {
            return idle;
        };
        if !self.player.loaded {
            return idle;
        }
        let local = matches!(self.decide(current), Decision::Play(Sound::File(_)));
        media_controls::State {
            track: Some(media_controls::Track {
                id: shown.id.clone(),
                title: shown.display_title(),
                artists: shown.artists_text(),
                album: shown.album_title.clone().unwrap_or_default(),
                // Для своего файла чужая обложка не нужна.
                cover: if local { None } else { shown.cover_uri.clone() },
                duration_ms: shown
                    .duration
                    .and_then(|duration| u64::try_from(duration.as_millis()).ok()),
            }),
            playing: self.player.playing,
            position_ms: (self.player.position.max(0.0) * 1000.0).round() as u64,
            volume_percent,
        }
    }

    /// Закрывает панель «Настройки звука». `false` — она и не была открыта.
    pub fn close_sound_panel(&mut self) -> bool {
        let open = self.motion.sound_panel.value();
        if open {
            self.motion.sound_panel.go_mut(false, Instant::now());
        }
        open
    }

    /// Панель «Настройки звука» поверх окна: карточка справа над плеером.
    /// Щелчок вне карточки закрывает её. `None` — панель закрыта.
    pub fn sound_panel(&self, prefs: &Prefs, now: Instant) -> Option<Element<'_, App>> {
        let frame = anim::Popover::at(&self.motion.sound_panel, now);
        if !frame.visible() {
            return None;
        }
        let header = row![
            container(theme::line(
                "Настройки звука",
                Style::Title,
                theme::text_primary()
            ))
            .width(Length::Fill),
            widgets::icon_button(
                Icon::Close,
                Size::S,
                false,
                Some(Lib(Message::CloseSoundPanel))
            ),
        ]
        .align_y(Alignment::Center);
        // Появление: карточка проступает из прозрачности, поднимаясь на
        // несколько пикселей и чуть вырастая, за ней — содержимое.
        let content = anim::motion(column![header, settings::sound_options(prefs)].spacing(12))
            .reveal(theme::surface_2(), frame.content);
        let card = container(content)
            .width(Length::Fill)
            .max_width(SOUND_PANEL_WIDTH)
            .padding(20)
            .style(move |_| widgets::popover_card(frame.shell, 16.0));
        let hidden = 1.0 - frame.motion;
        let card = anim::motion(iced::widget::opaque(card))
            .offset(0.0, 12.0 * hidden)
            .scale(0.97 + 0.03 * frame.motion);
        let placed = container(card)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::End)
            .align_y(Alignment::End)
            .padding(Padding {
                right: SOUND_PANEL_MARGIN,
                bottom: PLAYER_HEIGHT + 1.0 + SOUND_PANEL_MARGIN,
                left: SOUND_PANEL_MARGIN,
                top: SOUND_PANEL_MARGIN,
            });
        Some(iced::widget::opaque(
            mouse_area(placed).on_press(Lib(Message::CloseSoundPanel)),
        ))
    }

    /// Множитель нормализации для звука `sound` трека `track`. Свой файл —
    /// без поправки; замена из FckCensor берёт громкость трека замены, если
    /// она известна.
    fn sound_norm(&self, sound: &Sound, track: &Track) -> f32 {
        let Sound::Track(id) = sound else {
            return 1.0;
        };
        let shown = self.shown(track);
        // Копия недоступного трека: громкость берём у неё, если она в памяти.
        let loudness = [Some(shown), Some(track), self.relinks.track(id)]
            .into_iter()
            .flatten()
            .find(|candidate| candidate.id == *id)
            .and_then(|candidate| candidate.loudness);
        api::normalization_gain(loudness, self.normalize)
    }

    /// Трек остановлен: после конца или ошибки его надо запускать заново.
    fn halt(&mut self) {
        // Трек остановился (конец, ошибка, смена): сообщаем о сыгранном.
        self.report_finish(false);
        self.player.playing = false;
        self.player.loaded = false;
        self.player.buffering = false;
    }

    fn toggle_play(&mut self) -> Option<Effect> {
        let index = self.queue.as_ref()?.current;
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
        self.player.set_position(position);
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
                stream,
                seeks,
            } if id == session && seeks >= self.player.seeks => {
                self.player.set_position(position.as_secs_f32());
                self.player.buffering = buffering;
                self.player.stream = stream;
                if !buffering {
                    self.together_progress();
                    self.player.skipped = 0;
                    if !self.player.sounded {
                        self.wave_sounded();
                        self.report_start();
                        self.history_note_play();
                    }
                    self.player.sounded = true;
                }
                None
            }
            audio::Event::Ended { id } if id == session && self.together_follower() => {
                // Ведомый: трек кончился — ждём состояние ведущего.
                self.halt();
                self.together_settled();
                None
            }
            audio::Event::Ended { id } if id == session => {
                // Доигран целиком: сыграно — вся длительность.
                self.report_finish(true);
                self.wave_leave(Leave::Finish);
                self.halt();
                self.finished()
            }
            audio::Event::Failed { id, error } if id == session => {
                self.wave_leave(Leave::Skip);
                self.failed(&error)
            }
            _ => None,
        }
    }

    /// Трек доигран: дальше по повтору и очереди.
    fn finished(&mut self) -> Option<Effect> {
        let index = self.queue.as_ref()?.current;
        if self.repeat_one() {
            return self.start(index, 0.0);
        }
        match self.neighbour(index, true) {
            Some(next) => self.start(next, 0.0),
            None if self.in_wave() => self.wave_exhausted(),
            None => {
                // Очередь кончилась: трек остаётся выбранным, с начала.
                self.player.set_position(0.0);
                None
            }
        }
    }

    fn failed(&mut self, error: &PlayError) -> Option<Effect> {
        self.halt();
        // Трек не играет — и качество его потока уже ни о чём не говорит.
        self.player.stream = None;
        if *error == PlayError::Unauthorized {
            return Some(Effect::Relogin);
        }
        let index = self.queue.as_ref()?.current;
        let title = self
            .current_shown()
            .map(Track::display_title)
            .unwrap_or_default();
        // Ведомому дальше пропускать нечего: очередь у ведущего.
        if self.together_follower() {
            return self.together_failed(&title, &error.to_string());
        }
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
        let Some(next) = self.neighbour(from, forward) else {
            // В волне за концом очереди будет догрузка.
            return if forward { self.wave_exhausted() } else { None };
        };
        self.start_toward(next, 0.0, forward)
    }

    /// Соседний доступный трек очереди с учётом перемешивания и повтора.
    fn neighbour(&mut self, from: usize, forward: bool) -> Option<usize> {
        // В волне нет ни перемешивания, ни повтора списка.
        let (shuffle, wrap) = if self.in_wave() {
            (false, false)
        } else {
            (self.player.shuffle, self.player.repeat == Repeat::All)
        };
        let censor = self.plugins.fckcensor.then_some(&self.censor);
        let relinks = &self.relinks;
        self.queue
            .as_mut()?
            .neighbour(from, forward, shuffle, wrap, &mut self.rng, &|track| {
                plugins::playable(censor, relinks, track)
            })
    }

    // ------------------------------------------------------------ вид

    /// `compact` — узкое окно: в плеере меньше кнопок. `width` — ширина окна.
    #[allow(clippy::too_many_arguments)]
    pub fn view<'a>(
        &'a self,
        covers: &'a Covers,
        prefs: &Prefs,
        palette: &'a palette::State,
        settings_state: &'a settings::State,
        eq: &'a EqState,
        moment: Moment,
        compact: bool,
        width: f32,
        turn: f32,
    ) -> Element<'a, App> {
        let collapsed = prefs.sidebar_collapsed;
        let content: Element<'a, App> = match (self.stack.last(), self.section) {
            (Some(entry), _) => self.page_view(entry, covers, turn, moment.now),
            (None, Section::Liked) => self.liked(covers, turn),
            (None, Section::Wave) => self.wave_view(covers, turn, moment.now, prefs.visualizer),
            (None, Section::Settings) => settings::view(
                settings::Ctx {
                    library: self,
                    prefs: *prefs,
                    palette,
                    eq,
                    eq_name: &settings_state.eq_name,
                    width,
                    pixel_scale: self.pixel_scale(prefs),
                    update: &settings_state.update,
                    tray: settings_state.tray,
                    autostart: settings_state.autostart,
                },
                settings_state,
                // Размер окна уже в единицах раскладки (см. `App::refresh_size`).
                width,
            ),
            (None, Section::History) => self.history_view(covers, turn),
            (None, Section::Stats) => self.listen_view(turn),
            (None, Section::Search) => self.search_view(covers, turn, moment.now),
            (None, Section::Playlists | Section::Albums | Section::Artists) => {
                self.collection_view(covers, turn, moment.now)
            }
        };
        let panel = container(super::appear(content, moment.page))
            .width(Length::Fill)
            .height(Length::Fill)
            .clip(true);
        // Меню во всю высоту слева, справа раздел и под ним плеер. Фон везде
        // чёрный, части разделены тонкими линиями.
        let sidebar = if collapsed {
            SIDEBAR_COLLAPSED
        } else {
            SIDEBAR
        };
        let page: Element<'a, App> = match self.lyrics_panel(covers, moment.now, turn) {
            Some(lyrics) => row![panel, lyrics].height(Length::Fill).into(),
            None => panel.into(),
        };
        let right = column![
            page,
            widgets::divider(),
            self.player_bar(
                covers,
                compact,
                width - sidebar,
                turn,
                moment.now,
                prefs.visualizer,
                false
            )
        ]
        .width(Length::Fill);
        row![
            self.sidebar(moment.sidebar, self.pixel_scale(prefs), moment.now),
            widgets::vertical_divider(),
            right
        ]
        .height(Length::Fill)
        .into()
    }

    /// Полноэкранный плеер поверх окна: размытая обложка на фоне, крупная
    /// обложка слева, место под текст песни справа, плеер внизу. Выезжает
    /// снизу. `title_bar` — заголовок окна: за него окно по-прежнему можно
    /// таскать. `None` — плеер закрыт.
    #[allow(clippy::too_many_arguments)]
    pub fn fullscreen<'a>(
        &'a self,
        covers: &'a Covers,
        prefs: &Prefs,
        now: Instant,
        compact: bool,
        window: iced::Size,
        turn: f32,
        title_bar: Element<'a, App>,
    ) -> Option<Element<'a, App>> {
        let shown = anim::progress(&self.fullscreen, now);
        if shown <= 0.0 {
            return None;
        }
        let track = self.current_shown();
        let big = track.and_then(|track| covers.big(track.cover_uri.as_deref(), now));
        let small = track.map(|track| player_art(covers, track));

        // Фон: размытая обложка. Пока её нет — чёрный, у трека без обложки —
        // цвета заглушки под вуалью.
        let backdrop: Element<'a, App> = match (big, &small) {
            (Some((big, opacity)), _) => iced::widget::image(big.backdrop.clone())
                .width(Length::Fill)
                .height(Length::Fill)
                .content_fit(iced::ContentFit::Cover)
                .opacity(opacity)
                .into(),
            (None, Some(CoverArt::Placeholder(background))) => {
                let background = *background;
                // Размытая обложка уже затемнена при подготовке, заглушку
                // затемняем здесь так же.
                iced::widget::stack![
                    fill(move || container::Style {
                        background: Some(background),
                        ..container::Style::default()
                    }),
                    fill(|| container::Style {
                        background: Some(theme::alpha(theme::bg(), PLACEHOLDER_DIM).into()),
                        ..container::Style::default()
                    }),
                ]
                .into()
            }
            _ => Space::new().into(),
        };

        let side = ((window.height - FULLSCREEN_CHROME) * 0.9)
            .min((window.width - 3.0 * FULLSCREEN_SIDE) / 2.0)
            .clamp(FULLSCREEN_COVER_MIN, FULLSCREEN_COVER_MAX);
        let radius = 16.0;
        let base = widgets::cover(small.unwrap_or(CoverArt::Loading), side, radius);
        let cover: Element<'a, App> = match big {
            Some((big, opacity)) if opacity >= 1.0 => {
                widgets::picture(&big.cover, side, radius, 1.0)
            }
            Some((big, opacity)) => {
                iced::widget::stack![base, widgets::picture(&big.cover, side, radius, opacity)]
                    .into()
            }
            None => base,
        };
        let cover = container(cover).style(move |_| container::Style {
            background: Some(theme::surface_2().into()),
            border: Border::default().rounded(radius),
            shadow: widgets::cover_shadow(),
            ..container::Style::default()
        });
        let title = track.map_or_else(|| "Ничего не играет".to_owned(), Track::display_title);
        // Исполнители — ссылки, как в официальном клиенте: щелчок открывает
        // страницу исполнителя и сворачивает плеер. Длинный список — в две
        // строки, дальше обрезается.
        let artist: Element<'a, App> = match track {
            Some(track) if track.artists.iter().any(|artist| artist.id.is_some()) => {
                let (_, line, _) = Style::Headline.metrics();
                container(browse::artist_lines(
                    &track.artists,
                    Style::Headline,
                    theme::text_secondary(),
                ))
                .width(side)
                .max_height(line * 2.0)
                .clip(true)
                .into()
            }
            Some(track) => marquee(
                track.artists_text(),
                Style::Headline,
                theme::text_secondary(),
            )
            .into(),
            None => Space::new().into(),
        };
        let about = column![
            cover,
            column![
                marquee(title, Style::Display, theme::text_primary()),
                artist
            ]
            .spacing(6)
            .width(side),
        ]
        .spacing(28)
        .width(side);
        let lyrics = self.lyrics_fullscreen(turn);
        let body = row![about, lyrics]
            .spacing(FULLSCREEN_SIDE)
            .padding([0.0, FULLSCREEN_SIDE])
            .align_y(Alignment::Center)
            .height(Length::Fill);
        let close = container(widgets::with_tooltip(
            widgets::icon_button(
                Icon::Close,
                Size::M,
                false,
                Some(Lib(Message::Fullscreen(false))),
            ),
            "Свернуть · Esc",
            tooltip::Position::Left,
        ))
        // Справа: там же, где кнопка, которая открывает плеер.
        .align_right(Length::Fill)
        .padding([0.0, 24.0]);
        let player = self.player_bar(
            covers,
            compact,
            window.width,
            turn,
            now,
            prefs.visualizer,
            true,
        );
        let layers = iced::widget::stack![
            fill(|| container::Style {
                background: Some(theme::bg().into()),
                ..container::Style::default()
            }),
            backdrop,
            // Вуаль: на светлой обложке текст и кнопки должны читаться.
            fill(|| container::Style {
                background: Some(theme::alpha(theme::bg(), FULLSCREEN_SCRIM).into()),
                ..container::Style::default()
            }),
            column![title_bar, close, body, player],
        ]
        .width(Length::Fill)
        .height(Length::Fill);
        let sheet =
            anim::motion(iced::widget::opaque(layers)).offset(0.0, window.height * (1.0 - shown));
        Some(sheet.into())
    }

    /// Меню шириной `width`: узкое — только иконки.
    fn sidebar(&self, width: f32, pixel_scale: f32, now: Instant) -> Element<'_, App> {
        let compact = width < SIDEBAR_ICONS_BELOW;
        // Подсветка переезжает пружиной; цвета пунктов меняются вместе с ней.
        let glide = self.nav_progress(now);
        let item = |section: Section| {
            let active = if section == self.section {
                glide.clamp(0.0, 1.0)
            } else if self.nav_glide.is_some_and(|(from, _)| from == section) {
                1.0 - glide.clamp(0.0, 1.0)
            } else {
                0.0
            };
            nav_item(section, self.section == section, active, compact)
        };
        // Кнопка сворачивания появляется, когда мышь над шапкой меню. Её место
        // зависит от ширины плавно, а не от смены вида: в раскрытом меню она у
        // правого края с тем же отступом, что у пунктов (12), в свёрнутом —
        // по центру узкой колонки; пока меню едет, не прыгает.
        let share = ((width - SIDEBAR_COLLAPSED) / (SIDEBAR - SIDEBAR_COLLAPSED)).clamp(0.0, 1.0);
        let toggle_left = (width - 2.0 * SIDEBAR_PAD - TOGGLE - 8.0 * (1.0 - share)).max(0.0);
        let toggle = container(sidebar_toggle(compact))
            .padding(Padding::default().left(toggle_left))
            .width(Length::Fill)
            .height(48)
            .center_y(48);
        let brand: Element<'_, App> = if compact {
            hover(
                container(widgets::logo(28.0))
                    .center_x(Length::Fill)
                    .height(48)
                    .center_y(48),
                toggle,
            )
        } else {
            hover(
                row![
                    widgets::logo(28.0),
                    theme::line(super::APP_NAME, Style::Headline, theme::text_primary()),
                ]
                .spacing(10)
                .padding([0, 8])
                // Во всю ширину: верхний слой `hover` не шире нижнего.
                .width(Length::Fill)
                .height(48)
                .align_y(Alignment::Center),
                toggle,
            )
        };
        let collection: Element<'_, App> = if compact {
            container(widgets::divider()).padding([10, 8]).into()
        } else {
            container(theme::line(
                "Коллекция",
                Style::Caption,
                theme::text_tertiary(),
            ))
            .padding(Padding {
                top: 16.0,
                right: 12.0,
                bottom: 6.0,
                left: 12.0,
            })
            .into()
        };
        let mut menu = column![
            brand,
            Space::new().height(12),
            item(Section::Wave),
            item(Section::Search),
            collection,
            item(Section::Liked),
            item(Section::Playlists),
            item(Section::Albums),
            item(Section::Artists),
            item(Section::History),
        ];
        // «Статистика» (YandexMusicTime) видна, только если плагин и пункт
        // включены; без неё меню на один пункт короче.
        let stats_visible = listen::stats_in_menu(&self.plugins);
        if stats_visible {
            menu = menu.push(item(Section::Stats));
        }
        let menu = menu
            .push(Space::new().height(Length::Fill))
            .push(item(Section::Settings))
            .push(Space::new().height(8))
            .push(self.account_card(compact, pixel_scale))
            .spacing(2)
            .width(width)
            .padding(Padding {
                top: 4.0,
                right: SIDEBAR_PAD,
                bottom: 12.0,
                left: SIDEBAR_PAD,
            });
        let menu = anim::nav_highlight(
            menu,
            menu_index(self.section, stats_visible),
            self.nav_glide.map(|(_, started)| started),
            glide,
            theme::surface_3(),
            widgets::RADIUS_M,
        );
        container(menu).width(width).clip(true).into()
    }

    /// Доля пути подсветки меню (с перелётом за 1). Переезда нет — 1.
    fn nav_progress(&self, now: Instant) -> f32 {
        self.nav_glide.map_or(1.0, |(_, started)| {
            anim::nav_spring(now.saturating_duration_since(started))
        })
    }

    /// Подсветка меню ещё едет.
    fn nav_gliding(&self, now: Instant) -> bool {
        self.nav_glide
            .is_some_and(|(_, started)| now.saturating_duration_since(started) < anim::NAV_GLIDE)
    }

    /// Идёт ли переход внутри главного окна.
    pub fn transitioning(&self, now: Instant) -> bool {
        self.motion.running(now)
            || self.together_animating(now)
            || self.wave_menu_running(now)
            || self.lyrics_transitioning(now)
            || self.fullscreen.is_animating(now)
            || self.glide.is_some()
            || self.wave_tint_running(now)
            || self.nav_gliding(now)
    }

    fn account_card(&self, compact: bool, pixel_scale: f32) -> Element<'_, App> {
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
        let avatar = avatar(initial, 36.0, self.avatar(), pixel_scale);
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
                theme::line(name, Style::BodyStrong, theme::text_primary()),
                theme::line("Плюс активен", Style::Caption, theme::accent()),
            ]
            .width(Length::Fill),
        ]
        .spacing(10)
        .align_y(Alignment::Center);
        let card = button(card)
            .padding(8)
            .width(Length::Fill)
            .style(|_, status| button::Style {
                background: match status {
                    button::Status::Pressed => Some(theme::surface_3().into()),
                    _ => None,
                },
                border: Border::default().rounded(12),
                ..button::Style::default()
            })
            .on_press(open);
        anim::hover_fill(
            card,
            theme::clear(theme::surface_2()),
            theme::surface_2(),
            12.0,
        )
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
                    widgets::badge(badge, theme::error(), theme::error_soft()),
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
                    top: PAGE_TOP,
                    right: PAGE_SIDE,
                    bottom: 0.0,
                    left: PAGE_SIDE,
                }),
                state_block(
                    widgets::badge(Icon::Heart, theme::text_secondary(), theme::surface_2()),
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
            Likes::Ready(tracks) => {
                let body: Element<'a, App> = if self.shown_count() == 0 {
                    state_block(
                        widgets::badge(Icon::Search, theme::text_secondary(), theme::surface_2()),
                        "Ничего не нашлось",
                        format!(
                            "В «Мне нравится» нет треков по запросу «{}».",
                            self.query.trim()
                        ),
                        widgets::action(
                            Kind::Secondary,
                            Some(Icon::Close),
                            "Сбросить поиск",
                            Some(Lib(Message::ClearSearch)),
                        ),
                    )
                } else {
                    column![
                        container(list_header()).padding(Padding {
                            top: 20.0,
                            right: PAGE_SIDE,
                            bottom: 0.0,
                            left: PAGE_SIDE,
                        }),
                        self.liked_rows(tracks, covers),
                    ]
                    .into()
                };
                column![
                    container(liked_header(tracks, true)).padding(Padding {
                        top: PAGE_TOP,
                        right: PAGE_SIDE,
                        bottom: 0.0,
                        left: PAGE_SIDE,
                    }),
                    container(search_box(&self.query)).padding(Padding {
                        top: 24.0,
                        right: PAGE_SIDE,
                        bottom: 0.0,
                        left: PAGE_SIDE,
                    }),
                    body,
                ]
                .into()
            }
        }
    }

    /// Строки «Мне нравится». Номер в строке — номер трека во всём списке,
    /// даже при поиске; очередь при поиске — найденные треки.
    fn liked_rows<'a>(&'a self, tracks: &'a [Track], covers: &'a Covers) -> Element<'a, App> {
        let key = if self.query.trim().is_empty() {
            ListKey::Liked
        } else {
            ListKey::LikedFound(self.query.clone())
        };
        let filtered = !matches!(key, ListKey::Liked);
        self.scroller(self.body(), None, move |row| {
            let index = self.shown_at(row)?;
            let track = tracks.get(index)?;
            let at = if filtered { row } else { index };
            Some(self.track_row(&key, at, index + 1, track, covers))
        })
    }

    /// `bar_width` — ширина плеера: от неё зависит, сколько кнопок и значок
    /// качества потока помещаются справа.
    #[allow(clippy::too_many_arguments)]
    fn player_bar<'a>(
        &'a self,
        covers: &'a Covers,
        compact: bool,
        bar_width: f32,
        turn: f32,
        now: Instant,
        visualizer: bool,
        fullscreen: bool,
    ) -> Element<'a, App> {
        let player = &self.player;
        let track = self.current_shown();
        let has_track = track.is_some();
        let liked = track.is_some_and(|track| self.liked.contains(&track.id));
        // Лайк ставится настоящему треку очереди, а не подмене FckCensor.
        let like_message = self
            .current()
            .map(|track| Lib(Message::ToggleLike(track.clone())));
        let on = |message: Message| has_track.then_some(Lib(message));
        let motion = &self.motion;

        let (cover, title, artist): (Element<'a, App>, String, String) = match track {
            Some(track) => (
                crossfade_cover(
                    covers,
                    track,
                    motion.cover_from.as_ref(),
                    anim::progress(&motion.cover_fade, now),
                    56.0,
                    8.0,
                ),
                track.display_title(),
                track.artists_text(),
            ),
            None => (
                container(center(icon(Icon::Empty, 24.0, theme::text_secondary())))
                    .width(56)
                    .height(56)
                    .style(|_| surface_2(8.0))
                    .into(),
                "Ничего не играет".to_owned(),
                "Выберите трек в списке".to_owned(),
            ),
        };
        // Щелчок по обложке открывает полноэкранный плеер; при наведении на
        // ней проступает значок «развернуть».
        let cover: Element<'a, App> = if has_track && !fullscreen {
            let glyph: Element<'a, App> = icon(Icon::Expand, 20.0, theme::text_primary()).into();
            let badge = container(center(glyph))
                .width(56)
                .height(56)
                .style(|_| container::Style {
                    background: Some(theme::alpha(theme::bg(), 0.5).into()),
                    border: Border::default().rounded(8.0),
                    ..container::Style::default()
                });
            let cover = widgets::with_tooltip(
                hover(cover, badge),
                "Полноэкранный плеер",
                tooltip::Position::Top,
            );
            mouse_area(cover)
                .interaction(mouse::Interaction::Pointer)
                .on_press(Lib(Message::Fullscreen(true)))
                .into()
        } else {
            cover
        };
        // Длинное название трека не обрезается, а прокручивается: его видно
        // целиком. Подсказка без трека стоит на месте и обрезается: бегущая
        // строка в простое заставляла бы окно перерисовываться всё время.
        let texts: Element<'a, App> = if has_track {
            // Исполнители — ссылки на их страницы, одной бегущей строкой (пока
            // курсор над ней, она стоит). Без идентификаторов ссылок нет —
            // обычная бегущая строка с текстом.
            let artist: Element<'a, App> = match track {
                Some(track) if track.artists.iter().any(|artist| artist.id.is_some()) => {
                    marquee_element(browse::artist_links(
                        &track.artists,
                        Style::Secondary,
                        theme::text_secondary(),
                    ))
                }
                _ => marquee(artist, Style::Secondary, theme::text_secondary()).into(),
            };
            let mut lines = column![
                marquee(title, Style::BodyStrong, theme::text_primary()),
                artist
            ]
            .spacing(2)
            .width(Length::Fill);
            // Ведомый: чей трек играет.
            if let Some(caption) = self.together_caption() {
                lines = lines.push(theme::line(caption, Style::Caption, theme::accent()));
            }
            lines.into()
        } else {
            container(
                column![
                    theme::line(title, Style::BodyStrong, theme::text_primary()),
                    theme::line(artist, Style::Secondary, theme::text_secondary()),
                ]
                .spacing(2),
            )
            .width(Length::Fill)
            .clip(true)
            .into()
        };
        // Правый щелчок по названию — меню трека.
        let now_playing = mouse_area(
            row![cover, texts]
                .spacing(12)
                .align_y(Alignment::Center)
                .width(Length::Fill),
        )
        .on_right_press(Lib(Message::Plugin(PluginMsg::PlayerMenu)));
        let now_playing =
            container(now_playing).width(Length::FillPortion(if compact { 3 } else { 1 }));

        let glyph_color = if has_track {
            theme::on_accent()
        } else {
            theme::text_disabled()
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
                background: match status {
                    button::Status::Pressed => Some(theme::accent_pressed().into()),
                    button::Status::Disabled => Some(theme::surface_3().into()),
                    _ => None,
                },
                border: Border::default().rounded(widgets::ROUND),
                ..button::Style::default()
            })
            .on_press_maybe(on(Message::TogglePlay));
        let play: Element<'a, App> = if has_track {
            anim::hover_fill(play, theme::accent(), theme::accent_hover(), widgets::ROUND).into()
        } else {
            play.into()
        };
        // Нажатие: кнопка коротко сжимается и возвращается.
        let squeeze = (std::f32::consts::PI * anim::progress(&motion.pulse, now)).sin();
        let play = anim::motion(play).scale(1.0 - 0.1 * squeeze);
        // Ведомый показывает режимы ведущего: решает он.
        let master = self.together_master();
        let in_wave = self.in_wave() || master.is_some_and(|master| master.wave);
        let shuffle_on = master.map_or(player.shuffle, |master| master.shuffle);
        let (repeat_on, repeat_one) = match master {
            Some(master) => (
                master.repeat != TogetherRepeat::Off,
                master.repeat == TogetherRepeat::One,
            ),
            None => (
                self.repeat_one() || (!in_wave && player.repeat != Repeat::Off),
                self.repeat_one(),
            ),
        };
        let repeat_icon = if repeat_one {
            Icon::RepeatOne
        } else {
            Icon::Repeat
        };
        // «Не нравится» работает в «Моей волне»: пропуск, отзыв и запись в
        // аккаунт. В узком окне его нет: место нужнее названию трека.
        let mut buttons = row![].spacing(8).align_y(Alignment::Center);
        if !compact {
            buttons = buttons.push(widgets::icon_button(
                Icon::Dislike,
                Size::S,
                false,
                in_wave.then_some(Lib(Message::Wave(WaveMsg::Dislike))),
            ));
        }
        let shuffle: Element<'a, App> = if in_wave {
            widgets::with_tooltip(
                widgets::icon_button(Icon::Shuffle, Size::S, false, None),
                "В «Моей волне» перемешивания нет",
                tooltip::Position::Top,
            )
        } else {
            widgets::icon_button(
                Icon::Shuffle,
                Size::S,
                shuffle_on,
                on(Message::ToggleShuffle),
            )
        };
        let buttons = buttons.extend([
            shuffle,
            widgets::icon_button(Icon::PreviousFilled, Size::M, false, on(Message::Previous)),
            play.into(),
            widgets::icon_button(Icon::NextFilled, Size::M, false, on(Message::Next)),
            widgets::icon_button(repeat_icon, Size::S, repeat_on, on(Message::CycleRepeat)),
            // Сердце закрашено, только если трек в «Мне нравится».
            widgets::icon_button(
                if liked {
                    Icon::HeartFilled
                } else {
                    Icon::Heart
                },
                Size::S,
                liked,
                like_message,
            ),
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
            theme::line(clock(position), Style::Caption, theme::text_tertiary())
                .width(36)
                .align_x(Alignment::End),
            mouse_area(bar(
                0.0..=range_end,
                position.min(range_end),
                seekable,
                anim::progress(&motion.seek_hover, now),
                |value| Lib(Message::SeekDrag(value)),
                Some(Lib(Message::SeekCommit)),
            ))
            .on_enter(Lib(Message::SeekHover(true)))
            .on_exit(Lib(Message::SeekHover(false))),
            theme::line(total_text, Style::Caption, theme::text_tertiary()).width(36),
        ]
        .spacing(10)
        .align_y(Alignment::Center);
        // Края плеера одинаковой ширины, поэтому кнопки по центру. В узком
        // окне левому краю с названием отдаём больше.
        let controls = column![buttons, seek]
            .spacing(4)
            .align_x(Alignment::Center)
            .width(if compact { CONTROLS_COMPACT } else { CONTROLS });

        let volume = if player.muted { 0.0 } else { player.volume };
        let volume_icon = match volume {
            v if v <= 0.0 => Icon::VolumeMute,
            v if v < 0.5 => Icon::VolumeLow,
            _ => Icon::Volume,
        };
        let shown = player.stream.filter(|_| has_track);
        let fit = extras_fit(extras_room(bar_width, compact), fullscreen, shown);
        let mut extras = row![].spacing(EXTRAS_GAP).align_y(Alignment::Center);
        if let Some(info) = shown.filter(|_| fit.quality != Quality::Hidden) {
            extras = extras
                .push(quality_badge(info, fit.quality == Quality::Full))
                .push(Space::new().width(EXTRAS_BADGE_SPACE));
        }
        if fit.more {
            extras = extras.push(widgets::with_tooltip(
                widgets::icon_button(
                    Icon::More,
                    Size::S,
                    false,
                    on(Message::Plugin(PluginMsg::PlayerMenu)),
                ),
                "Меню трека",
                tooltip::Position::Top,
            ));
        }
        if fit.lyrics {
            extras = extras.push(widgets::with_tooltip(
                widgets::icon_button(
                    Icon::Lyrics,
                    Size::S,
                    self.lyrics_open(),
                    // В полноэкранном плеере текст всегда на виду.
                    if fullscreen {
                        None
                    } else {
                        on(Message::Lyrics(lyrics::Msg::Toggle))
                    },
                ),
                "Текст песни",
                tooltip::Position::Top,
            ));
        }
        if fit.queue {
            extras = extras.push(widgets::with_tooltip(
                widgets::icon_button(
                    Icon::Queue,
                    Size::S,
                    self.queue_open(),
                    if fullscreen {
                        None
                    } else {
                        on(Message::Lyrics(lyrics::Msg::ToggleQueue))
                    },
                ),
                "Очередь",
                tooltip::Position::Top,
            ));
        }
        // «Слушать вместе»: карточка с устройствами; горит, пока связь есть.
        if fit.devices {
            extras = extras.push(widgets::with_tooltip(
                widgets::icon_button(
                    Icon::Devices,
                    Size::S,
                    self.together_is_linked() || self.together_open(),
                    Some(Lib(Message::Together(TogetherMsg::TogglePanel))),
                ),
                "Слушать вместе",
                tooltip::Position::Top,
            ));
        }
        // В полноэкранном плеере эквалайзера нет: он в настройках.
        if fit.equalizer && !fullscreen {
            extras = extras.push(widgets::with_tooltip(
                widgets::icon_button(
                    Icon::Equalizer,
                    Size::S,
                    self.eq_enabled,
                    Some(App::OpenEqualizer),
                ),
                "Эквалайзер",
                tooltip::Position::Top,
            ));
        }
        if fit.mini {
            extras = extras.push(widgets::with_tooltip(
                widgets::icon_button(Icon::MiniPlayer, Size::S, false, Some(App::MiniOpen)),
                "Мини-плеер",
                tooltip::Position::Top,
            ));
        }
        // Громкость — фиксированной ширины: iced раскладывает такие элементы
        // раньше `Fill`, так что ей место достаётся первой, а остальное
        // прижато вправо и обрезается, если оценка ширины ошиблась (масштаб).
        // Над значком всплывает панель громкости (`volume_panel`).
        let volume_button = container(
            mouse_area(widgets::icon_button(
                volume_icon,
                Size::S,
                false,
                Some(Lib(Message::ToggleMute)),
            ))
            .on_enter(Lib(Message::VolumeHover(true)))
            .on_exit(Lib(Message::VolumeHover(false))),
        )
        .width(VOLUME_ICON_SIDE);
        let extras = container(
            row![
                container(extras)
                    .width(Length::Fill)
                    .align_x(Alignment::End)
                    .clip(true),
                volume_button
            ]
            .spacing(EXTRAS_GAP)
            .align_y(Alignment::Center),
        )
        .width(Length::FillPortion(if compact { 2 } else { 1 }));

        let bar = container(
            row![now_playing, controls, extras]
                .spacing(16)
                .padding([0.0, PLAYER_SIDE])
                .align_y(Alignment::Center),
        )
        .height(PLAYER_HEIGHT)
        .center_y(PLAYER_HEIGHT);
        // Спектр — тихим слоем под кнопками.
        let bar: Element<'a, App> = if visualizer && self.spectrum.moving() {
            // Цвет волны: акцент, а с VibeColorizer — цвет трека.
            let color = theme::alpha(self.wave_color(now), SPECTRUM_ALPHA);
            iced::widget::stack![
                container(visualizer::bars(self.spectrum.levels(), color))
                    .padding(Padding {
                        top: 14.0,
                        ..Padding::ZERO
                    })
                    .height(88),
                bar
            ]
            .into()
        } else {
            bar.into()
        };
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

    /// Всплывающая панель громкости над значком: проценты и вертикальный
    /// ползунок. Слой на всё окно, прижатый к правому нижнему углу: значок
    /// последний справа в плеере, а плеер стоит у нижнего края окна, так что
    /// положение не зависит от ширины окна и масштаба (координаты логические).
    /// `None` — панель спрятана и ничего не перекрывает.
    pub fn volume_panel(&self, now: Instant) -> Option<Element<'_, App>> {
        let shown = anim::progress(&self.motion.volume_panel, now);
        if shown <= 0.0 {
            return None;
        }
        let player = &self.player;
        let percent = volume_percent(player.volume, player.muted);
        let shown_volume = self
            .motion
            .volume
            .interpolate_with(|value| value, now)
            .clamp(0.0, 1.0);
        let slider = vertical_slider(0.0..=1.0, shown_volume, |value| Lib(Message::Volume(value)))
            .on_release(Lib(Message::VolumeRelease))
            .step(0.01_f32)
            .width(16)
            .height(VOLUME_SLIDER_HEIGHT)
            .style(|_, _| slider::Style {
                rail: slider::Rail {
                    backgrounds: (theme::accent().into(), theme::surface_4().into()),
                    width: 4.0,
                    border: Border::default().rounded(2),
                },
                handle: slider::Handle {
                    // Ручка-пилюля: поперёк дорожки на всю ширину ползунка.
                    shape: slider::HandleShape::Rectangle {
                        width: 6,
                        border_radius: 3.0.into(),
                    },
                    background: theme::text_primary().into(),
                    border_width: 0.0,
                    border_color: Color::TRANSPARENT,
                },
            });
        let card = container(
            column![
                theme::line(
                    format!("{percent}%"),
                    Style::CaptionStrong,
                    theme::text_secondary()
                ),
                slider,
            ]
            .spacing(12)
            .align_x(Alignment::Center),
        )
        .width(VOLUME_PANEL_WIDTH)
        .height(VOLUME_PANEL_HEIGHT)
        .padding([16.0, 0.0])
        .center_x(VOLUME_PANEL_WIDTH)
        .style(|_| container::Style {
            background: Some(theme::surface_2().into()),
            border: Border::default()
                .rounded(VOLUME_PANEL_WIDTH / 2.0)
                .width(1)
                .color(theme::border()),
            shadow: widgets::popover_shadow(),
            ..container::Style::default()
        });
        // Появление: поднимается на несколько пикселей, чуть растёт и
        // проступает из фона.
        let hidden = 1.0 - shown;
        let card = anim::motion(card)
            .offset(0.0, 8.0 * hidden)
            .scale(0.94 + 0.06 * shown)
            .veil(theme::alpha(theme::bg(), hidden), VOLUME_PANEL_WIDTH / 2.0);
        // Прозрачная полоса под панелью закрывает зазор до значка: курсор
        // идёт со значка на панель, не покидая общую область.
        let region = column![card, Space::new().height(VOLUME_PANEL_GAP)];
        let region = mouse_area(region)
            .on_enter(Lib(Message::VolumePanelHover(true)))
            .on_exit(Lib(Message::VolumePanelHover(false)))
            .on_scroll(|delta| {
                let (mouse::ScrollDelta::Lines { y, .. } | mouse::ScrollDelta::Pixels { y, .. }) =
                    delta;
                if y == 0.0 {
                    Lib(Message::Noop)
                } else {
                    Lib(Message::VolumeScroll(y))
                }
            });
        // Центр панели над центром значка; низ полосы — на верхнем краю значка.
        let right = PLAYER_SIDE + (VOLUME_ICON_SIDE - VOLUME_PANEL_WIDTH) / 2.0;
        let bottom = (PLAYER_HEIGHT + VOLUME_ICON_SIDE) / 2.0;
        Some(
            container(iced::widget::opaque(region))
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(Alignment::End)
                .align_y(Alignment::End)
                .padding(Padding {
                    right,
                    bottom,
                    ..Padding::ZERO
                })
                .into(),
        )
    }
}

/// Прокручиваемый список раздела высотой `total`: полоса прокрутки своя (у
/// iced ручка длинного списка — точка), колесо прокручивает плавно. Тачпад и
/// клавиши — как обычно у `scrollable`.
fn scrolling<'a>(library: &Library, list: Element<'a, App>, total: f32) -> Element<'a, App> {
    let list = scrollable(container(list).padding(Padding {
        top: 0.0,
        right: PAGE_SIDE,
        bottom: 0.0,
        left: PAGE_SIDE,
    }))
    .id(LIST_ID)
    .direction(scrollable::Direction::Vertical(
        scrollable::Scrollbar::hidden(),
    ))
    .on_scroll(|viewport| Lib(Message::Scrolled(viewport)))
    .height(Length::Fill)
    .style(|_, _| {
        let rail = scrollable::Rail {
            background: None,
            border: Border::default(),
            scroller: scrollable::Scroller {
                background: Color::TRANSPARENT.into(),
                border: Border::default(),
            },
        };
        scrollable::Style {
            container: container::Style::default(),
            vertical_rail: rail,
            horizontal_rail: rail,
            gap: None,
            auto_scroll: scrollable::AutoScroll {
                background: theme::surface_3().into(),
                border: Border::default().rounded(widgets::ROUND),
                shadow: widgets::raised_shadow(),
                icon: theme::text_primary(),
            },
        }
    });
    let rail = row![
        Space::new().width(Length::Fill),
        container(scroll::rail(library.scroll_y, total, |offset| {
            Lib(Message::ScrollTo(offset))
        }))
        .padding(Padding {
            right: RAIL_RIGHT,
            ..Padding::ZERO
        }),
    ];
    iced::widget::stack![
        scroll::wheel(list, |lines| Lib(Message::Wheel(lines))),
        rail
    ]
    .height(Length::Fill)
    .into()
}

/// Секунды ползунка в `Duration`. Отрицательное и не число — ноль.
fn seconds(value: f32) -> Duration {
    Duration::try_from_secs_f32(value.max(0.0)).unwrap_or_default()
}

fn clock(seconds: f32) -> String {
    // Отрицательное и не число превращаются в ноль.
    texts::clock(seconds.max(0.0) as u64)
}

fn surface_2(radius: f32) -> container::Style {
    container::Style {
        background: Some(theme::surface_2().into()),
        border: Border::default().rounded(radius),
        ..container::Style::default()
    }
}

/// Аватар: картинка из Яндекс ID, если загружена, иначе буква на градиенте.
pub(super) fn avatar<'a>(
    initial: String,
    size: f32,
    picture: Option<&Avatar>,
    pixel_scale: f32,
) -> Element<'a, App> {
    // Уровень не мельче кружка в физических пикселях: так iced почти не
    // уменьшает картинку и «лесенок» нет.
    if let Some(handle) = picture.and_then(|avatar| avatar.pick(size * pixel_scale)) {
        return iced::widget::image(handle.clone())
            .width(size)
            .height(size)
            .filter_method(iced::widget::image::FilterMethod::Linear)
            .content_fit(iced::ContentFit::Cover)
            .border_radius(size / 2.0)
            .into();
    }
    container(theme::line(
        initial,
        Style::BodyStrong,
        theme::text_primary(),
    ))
    .center(size)
    .style(|_| container::Style {
        background: Some(theme::avatar()),
        border: Border::default().rounded(widgets::ROUND),
        ..container::Style::default()
    })
    .into()
}

/// Обложка, под которой тает обложка прежнего трека `from`. `shown` — доля
/// перехода.
/// Прокрутить список «Мне нравится» на `offset` от начала.
fn scroll_list(offset: f32) -> Task<Message> {
    iced::widget::operation::scroll_to(
        LIST_ID,
        scrollable::AbsoluteOffset {
            x: None,
            y: Some(offset),
        },
    )
}

/// Слой во всю ширину и высоту со стилем `style`.
fn fill<'a>(style: impl Fn() -> container::Style + 'a) -> Element<'a, App> {
    container(Space::new())
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| style())
        .into()
}

/// Обложка трека в плеере: 300×300, с запасом под масштаб экрана (56
/// логических пикселей при 200 % — 112 физических, а в маленькой копии их 100).
fn player_art<'a>(covers: &'a Covers, track: &Track) -> CoverArt<'a> {
    match covers.art_of(Grade::Medium, track.cover_uri.as_deref()) {
        Art::Ready(handle, opacity) => CoverArt::Image(handle, opacity),
        Art::Loading => CoverArt::Loading,
        Art::Missing => CoverArt::Placeholder(theme::cover_placeholder(&track.id)),
    }
}

fn crossfade_cover<'a>(
    covers: &'a Covers,
    track: &Track,
    from: Option<&Track>,
    shown: f32,
    size: f32,
    radius: f32,
) -> Element<'a, App> {
    let current = player_art(covers, track);
    let previous = from.map(|from| player_art(covers, from));
    match (current, previous) {
        (CoverArt::Image(handle, opacity), Some(CoverArt::Image(old, _))) if shown < 1.0 => {
            iced::widget::stack![
                widgets::picture(old, size, radius, 1.0),
                widgets::picture(handle, size, radius, opacity * shown),
            ]
            .into()
        }
        (current, _) => widgets::cover(current, size, radius),
    }
}

fn cover_art<'a>(covers: &'a Covers, track: &Track) -> CoverArt<'a> {
    match covers.art(track.cover_uri.as_deref()) {
        Art::Ready(handle, opacity) => CoverArt::Image(handle, opacity),
        Art::Loading => CoverArt::Loading,
        Art::Missing => CoverArt::Placeholder(theme::cover_placeholder(&track.id)),
    }
}

/// Пункт меню. Свёрнутое меню — только иконка с подсказкой.
/// Кнопка «свернуть / развернуть меню». Фон непрозрачный: в свёрнутом меню
/// кнопка ложится поверх логотипа.
fn sidebar_toggle<'a>(collapsed: bool) -> Element<'a, App> {
    // Шеврон Lucide 16 px строго по центру круга.
    let (name, tip) = if collapsed {
        (Icon::Forward, "Развернуть меню")
    } else {
        (Icon::Back, "Свернуть меню")
    };
    let toggle = button(center(icon(name, 16.0, theme::text_primary())))
        .width(TOGGLE)
        .height(TOGGLE)
        .padding(0)
        .style(|_, status| button::Style {
            background: Some(
                match status {
                    button::Status::Hovered | button::Status::Pressed => theme::surface_3(),
                    _ => theme::surface_2(),
                }
                .into(),
            ),
            border: Border::default().rounded(widgets::ROUND),
            ..button::Style::default()
        })
        .on_press(App::ToggleSidebar);
    widgets::with_tooltip(toggle, tip, tooltip::Position::Right)
}

/// Номер пункта `section` в колонке меню ([`Library::sidebar`]): под ним
/// рисуется подсветка. Порядок тот же, что у детей колонки.
fn menu_index(section: Section, stats_visible: bool) -> usize {
    // 0 — шапка, 1 — отступ, 4 — «Коллекция», потом (если виден пункт
    // «Статистика» — 10) распорка, настройки, отступ, карточка аккаунта.
    // Скрытый пункт не сдвигает остальные; раздел, которого нет в меню,
    // получает номер несуществующего ребёнка, подсветки у него нет.
    match section {
        Section::Wave => 2,
        Section::Search => 3,
        Section::Liked => 5,
        Section::Playlists => 6,
        Section::Albums => 7,
        Section::Artists => 8,
        Section::History => 9,
        Section::Stats if stats_visible => 10,
        Section::Stats => usize::MAX,
        Section::Settings => 11 + usize::from(stats_visible),
    }
}

/// Пункт меню. Фона у выбранного нет: его рисует общая подсветка
/// ([`anim::nav_highlight`]). `active` — насколько пункт выбран (0…1): цвета
/// значка и подписи меняются вместе с переездом подсветки.
fn nav_item<'a>(section: Section, selected: bool, active: f32, compact: bool) -> Element<'a, App> {
    let make = move |hovered: bool| -> Element<'a, App> {
        let (icon_color, text_color, font) = if hovered && !selected {
            (
                theme::text_primary(),
                theme::text_primary(),
                Style::BodyStrong,
            )
        } else {
            (
                theme::mix(theme::text_secondary(), theme::accent(), active),
                theme::mix(theme::text_secondary(), theme::text_primary(), active),
                Style::BodyStrong,
            )
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
                    (false, button::Status::Pressed) => Some(theme::surface_3().into()),
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
        anim::hover_fill(
            hover(make(false), make(true)),
            theme::clear(theme::surface_2()),
            theme::surface_2(),
            widgets::RADIUS_M,
        )
        .into()
    };
    if compact {
        widgets::with_tooltip(item, section.title(), tooltip::Position::Right)
    } else {
        item
    }
}

/// Ставит или снимает «Нравится» в множестве идентификаторов и в списке
/// «Мне нравится» (если он загружен): новый трек — в начало, снятый
/// убирается. `true` — что-то изменилось.
fn edit_likes(
    ids: &mut HashSet<String>,
    list: Option<&mut Vec<Track>>,
    track: &Track,
    liked: bool,
) -> bool {
    let mut changed = if liked {
        ids.insert(track.id.clone())
    } else {
        ids.remove(&track.id)
    };
    if let Some(list) = list {
        let at = list.iter().position(|item| item.id == track.id);
        match (liked, at) {
            (true, None) => {
                list.insert(0, track.clone());
                changed = true;
            }
            (false, Some(at)) => {
                list.remove(at);
                changed = true;
            }
            _ => {}
        }
    }
    changed
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
    let tile = container(center(icon(Icon::HeartFilled, 64.0, theme::on_accent())))
        .width(HEADER_COVER)
        .height(HEADER_COVER)
        .style(|_| container::Style {
            background: Some(theme::liked_tile()),
            border: Border::default().rounded(12),
            shadow: widgets::cover_shadow(),
            ..container::Style::default()
        });
    let mut info = column![
        theme::line("Коллекция", Style::Body, theme::text_secondary()),
        theme::line("Мне нравится", Style::Hero, theme::text_primary()),
        theme::line(subtitle, Style::Body, theme::text_secondary()),
    ]
    .spacing(4)
    .width(Length::Fill);
    if with_actions {
        info = info.push(Space::new().height(14)).push(
            row![
                widgets::action(
                    Kind::Primary,
                    Some(Icon::PlayFilled),
                    "Слушать",
                    Some(Lib(Message::PlayList(ListKey::Liked))),
                ),
                widgets::action(
                    Kind::Secondary,
                    Some(Icon::Shuffle),
                    "Перемешать",
                    Some(Lib(Message::ShuffleList(ListKey::Liked))),
                ),
                widgets::icon_button(Icon::More, Size::M, false, Some(Lib(Message::Noop))),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        );
    }
    row![tile, info]
        .spacing(28)
        .align_y(Alignment::Center)
        .into()
}

/// Обложка или плитка в шапке коллекции.
const HEADER_COVER: f32 = 176.0;

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

/// Поле поиска: где оно, подсказка и высота.
struct SearchField {
    id: &'static str,
    placeholder: &'static str,
    height: f32,
}

/// Поле «Поиск трека» в «Мне нравится».
fn search_box<'a>(query: &str) -> Element<'a, App> {
    search_field(
        SearchField {
            id: SEARCH_ID,
            placeholder: "Поиск трека",
            height: SEARCH_HEIGHT,
        },
        query,
        |value| Lib(Message::Search(value)),
        None,
        Lib(Message::ClearSearch),
    )
}

/// Поле поиска — «таблетка» с лупой, крестик сбрасывает запрос.
fn search_field<'a>(
    field: SearchField,
    query: &str,
    on_input: impl Fn(String) -> App + 'a,
    on_submit: Option<App>,
    on_clear: App,
) -> Element<'a, App> {
    let mut input = text_input(field.placeholder, query)
        .id(field.id)
        .on_input(on_input)
        .size(14)
        .padding(0)
        .style(|_, _| text_input::Style {
            background: Color::TRANSPARENT.into(),
            border: Border::default(),
            icon: theme::text_tertiary(),
            placeholder: theme::text_tertiary(),
            value: theme::text_primary(),
            selection: theme::alpha(theme::accent(), 0.35),
        });
    if let Some(message) = on_submit {
        input = input.on_submit(message);
    }
    let glyph: Element<'a, App> = icon(Icon::Search, 18.0, theme::text_tertiary()).into();
    let mut row = row![glyph, input].spacing(12).align_y(Alignment::Center);
    let right = if query.is_empty() {
        16.0
    } else {
        row = row.push(widgets::icon_button(
            Icon::Close,
            Size::S,
            false,
            Some(on_clear),
        ));
        6.0
    };
    container(row)
        .width(Length::Fill)
        .height(field.height)
        .center_y(field.height)
        .padding(Padding {
            top: 0.0,
            right,
            bottom: 0.0,
            left: 16.0,
        })
        .style(|_| container::Style {
            border: Border {
                color: theme::outline(),
                width: 1.0,
                radius: widgets::ROUND.into(),
            },
            ..container::Style::default()
        })
        .into()
}

fn list_header<'a>() -> Element<'a, App> {
    let caption = |text: &'a str| theme::line(text, Style::Caption, theme::text_tertiary());
    column![
        container(columns(
            caption("#").into(),
            Space::new().into(),
            caption("НАЗВАНИЕ").into(),
            caption("АЛЬБОМ").into(),
            Space::new().into(),
            icon(Icon::Clock, 16.0, theme::text_tertiary()).into(),
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

fn play_cell<'a>(name: Icon, color: Color, message: Message) -> Element<'a, App> {
    button(center(icon(name, 16.0, color)))
        .width(32)
        .height(32)
        .padding(0)
        .style(|_, _| button::Style::default())
        .on_press(Lib(message))
        .into()
}

/// Ползунок перемотки или громкости: дорожка 4, ручка 12 при наведении.
/// `hover` — доля наведения (ручка и акцентная заливка проявляются плавно),
/// `on_release` — когда ползунок отпустили.
fn bar<'a>(
    range: std::ops::RangeInclusive<f32>,
    value: f32,
    enabled: bool,
    hover: f32,
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
            let engaged = match (enabled, status) {
                (false, _) => 0.0,
                (true, slider::Status::Dragged) => 1.0,
                (true, _) => hover,
            };
            let filled = if enabled {
                theme::mix(theme::text_secondary(), theme::accent(), engaged)
            } else {
                theme::surface_4()
            };
            slider::Style {
                rail: slider::Rail {
                    backgrounds: (filled.into(), theme::surface_4().into()),
                    width: 4.0,
                    border: Border::default().rounded(2),
                },
                handle: slider::Handle {
                    shape: slider::HandleShape::Circle {
                        radius: 6.0 * engaged,
                    },
                    background: theme::text_primary().into(),
                    border_width: 0.0,
                    border_color: Color::TRANSPARENT,
                },
            }
        })
        .into()
}

/// Текст значка качества: «FLAC», «AAC · 256», «MP3 · 320»; когда мало
/// места — только битрейт («256»). У FLAC битрейта нет.
fn quality_text(info: StreamInfo, full: bool) -> String {
    let name = match info.codec {
        Codec::Flac => return "FLAC".to_owned(),
        Codec::Aac => "AAC",
        Codec::Mp3 => "MP3",
    };
    match (info.kbps, full) {
        (Some(kbps), true) => format!("{name} · {kbps}"),
        (Some(kbps), false) => kbps.to_string(),
        (None, _) => name.to_owned(),
    }
}

/// Значок качества потока.
fn quality_badge<'a>(info: StreamInfo, full: bool) -> Element<'a, App> {
    let text = quality_text(info, full);
    // Щелчок открывает панель «Настройки звука».
    let badge = button(theme::line(
        text,
        Style::CaptionStrong,
        theme::text_secondary(),
    ))
    .padding([3, 8])
    .style(|_, status| button::Style {
        background: match status {
            button::Status::Hovered | button::Status::Pressed => Some(theme::surface_3().into()),
            _ => None,
        },
        text_color: theme::text_secondary(),
        border: Border::default()
            .rounded(widgets::RADIUS_S)
            .width(1)
            .color(theme::border()),
        ..button::Style::default()
    })
    .on_press(Lib(Message::ToggleSoundPanel));
    widgets::with_tooltip(
        badge,
        match (info.codec, full) {
            (Codec::Flac, _) => "Качество потока: без потерь".to_owned(),
            (_, true) => "Качество потока".to_owned(),
            (Codec::Aac, false) => "Качество потока: AAC, кбит/с".to_owned(),
            (Codec::Mp3, false) => "Качество потока: MP3, кбит/с".to_owned(),
        },
        tooltip::Position::Top,
    )
}

/// Блок по центру панели: иконка, заголовок, текст, кнопка.
fn state_block<'a>(
    badge: Element<'a, App>,
    title: impl iced::widget::text::IntoFragment<'a>,
    text: String,
    action: Element<'a, App>,
) -> Element<'a, App> {
    center(
        column![
            badge,
            theme::line(title, Style::Title, theme::text_primary()),
            theme::label(text, Style::Body, theme::text_secondary())
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

/// Скелетон шапки и строк, мерцает (прозрачность 0.5 → 1 → 0.5).
fn skeleton<'a>(turn: f32) -> Element<'a, App> {
    let alpha = 0.75 + 0.25 * (turn * std::f32::consts::TAU).cos();
    let shade = Color {
        a: alpha,
        ..theme::surface_2()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_stats_item_does_not_shift_the_highlight() {
        // С пунктом «Статистика»: он 10-й, настройки на одну позицию ниже.
        assert_eq!(menu_index(Section::History, true), 9);
        assert_eq!(menu_index(Section::Stats, true), 10);
        assert_eq!(menu_index(Section::Settings, true), 12);
        // Без него: порядок прежний, у скрытого раздела подсветки нет.
        assert_eq!(menu_index(Section::History, false), 9);
        assert_eq!(menu_index(Section::Settings, false), 11);
        assert_eq!(menu_index(Section::Stats, false), usize::MAX);
        assert_eq!(
            menu_index(Section::Wave, false),
            menu_index(Section::Wave, true)
        );
    }

    /// Библиотека без сети и звука: плеер не создаётся, `launch` сразу
    /// «останавливается», но запись истории и сообщения о прослушивании идут
    /// так же, как в работе.
    pub(super) fn test_library() -> Library {
        let status = api::sample_plus_status();
        let permit = status.playback_permit().unwrap();
        let token = Arc::new(StoredToken {
            access_token: "test".to_owned(),
            refresh_token: None,
            expires_at: None,
        });
        Library::new(
            token,
            status,
            permit,
            800.0,
            None,
            &PluginSettings::default(),
        )
        .0
    }

    fn plain_track(id: &str) -> Track {
        Track {
            id: id.to_owned(),
            title: Some(format!("Трек {id}")),
            version: None,
            artists: Vec::new(),
            album_id: None,
            album_title: None,
            duration: None,
            available: true,
            cover_uri: None,
            warning: api::Warning::None,
            loudness: None,
            lyrics: None,
        }
    }

    fn history_ids(library: &Library) -> Vec<String> {
        library
            .history
            .tracks()
            .map(|track| track.id.clone())
            .collect()
    }

    #[test]
    fn window_opens_on_idle_wave_screen() {
        let library = test_library();
        assert_eq!(library.section, Section::Wave);
        assert!(library.stack.is_empty());
        // Волна не запускается сама, звука нет.
        assert!(!library.in_wave());
        assert!(library.queue.is_none());
        assert!(!library.player.playing);
        // «Мне нравится» всё равно грузится в фоне: от него зависят сердца.
        assert!(matches!(library.likes, Likes::Loading { .. }));
    }

    #[test]
    fn history_records_every_change_of_a_track_that_sounded() {
        let mut library = test_library();
        let list: Vec<Track> = ["1", "2", "3", "4", "5"]
            .iter()
            .map(|id| plain_track(id))
            .collect();
        library.queue = Some(Queue::new(ListKey::Liked, Arc::new(list.clone()), 0));
        library.launch(0, plain_track("1"), 0.0);
        // Трек не зазвучал: смена его в историю не кладёт.
        library.launch(1, plain_track("2"), 0.0);
        assert!(history_ids(&library).is_empty());
        // Выбор из списка.
        library.player.sounded = true;
        library.launch(2, plain_track("3"), 0.0);
        assert_eq!(history_ids(&library), ["2"]);
        // «Следующий».
        library.player.sounded = true;
        let _ = library.update(Message::Next, None);
        assert_eq!(history_ids(&library), ["2", "3"]);
        // «Предыдущий» (позиция в начале трека: переход, а не перемотка).
        library.player.sounded = true;
        let _ = library.update(Message::Previous, None);
        assert_eq!(history_ids(&library), ["2", "3", "4"]);
        // Автопереход по концу трека.
        library.player.sounded = true;
        let session = library.player.session;
        let _ = library.audio_event(audio::Event::Ended { id: session });
        assert_eq!(history_ids(&library), ["2", "3", "4", "3"]);
        // Тот же трек сам себя в историю не кладёт.
        library.player.sounded = true;
        let current = library.current().cloned().unwrap();
        let index = library.queue.as_ref().unwrap().current;
        library.launch(index, current, 0.0);
        assert_eq!(history_ids(&library), ["2", "3", "4", "3"]);
    }

    /// Треки вкладки «История» по группам: (подпись, [id]).
    fn shown_history(library: &Library) -> Vec<(String, Vec<String>)> {
        library
            .history_shown
            .iter()
            .flat_map(|day| &day.items)
            .map(|item| {
                let ids = item.tracks.iter().map(|track| track.id.clone()).collect();
                (item.context.label(), ids)
            })
            .collect()
    }

    #[test]
    fn sounded_track_lands_in_history_tab_at_once() {
        let mut library = test_library();
        // Сервер ответил: сегодня одна волна с треком «9».
        library.server_history = history_screen::HistoryLoad::Idle;
        library.history_serial = 1;
        let server = vec![api::HistoryDay {
            date: Some(api::today_utc()),
            items: vec![api::HistoryItem {
                context: api::HistoryContext::Wave,
                tracks: vec![plain_track("9")],
            }],
        }];
        let _ = library.history_loaded(1, Ok(Arc::new(server)));
        let list: Vec<Track> = ["1", "2"].iter().map(|id| plain_track(id)).collect();
        library.queue = Some(Queue::new(ListKey::Liked, Arc::new(list), 0));
        library.remember_context(&ListKey::Liked);
        library.launch(0, plain_track("1"), 0.0);
        // Пока трек не зазвучал, во вкладке его нет.
        assert_eq!(shown_history(&library).len(), 1);
        let sound = |library: &mut Library| {
            let id = library.player.session;
            let seeks = library.player.seeks;
            let _ = library.audio_event(audio::Event::Progress {
                id,
                position: Duration::from_secs(1),
                buffering: false,
                stream: None,
                seeks,
            });
        };
        sound(&mut library);
        library.launch(1, plain_track("2"), 0.0);
        sound(&mut library);
        // Свежие первыми, над серверной волной; повторный «зазвучал» не дублирует.
        sound(&mut library);
        let liked = "Мне нравится".to_owned();
        assert_eq!(
            shown_history(&library),
            [
                (liked, vec!["2".to_owned(), "1".to_owned()]),
                ("Моя волна".to_owned(), vec!["9".to_owned()]),
            ]
        );
        // Новый ответ сервера (уже с «1») слияние не ломает: без дублей.
        library.history_serial = 2;
        let server = vec![api::HistoryDay {
            date: Some(api::today_utc()),
            items: vec![api::HistoryItem {
                context: api::HistoryContext::Wave,
                tracks: vec![plain_track("1"), plain_track("9")],
            }],
        }];
        let _ = library.history_loaded(2, Ok(Arc::new(server)));
        let ids: Vec<String> = shown_history(&library)
            .into_iter()
            .flat_map(|(_, ids)| ids)
            .collect();
        assert_eq!(ids, ["2", "1", "9"]);
    }

    #[test]
    fn plays_from_disk_go_under_this_session() {
        let mut library = test_library();
        // В тестах файла нет: ни чтения, ни записи.
        assert!(library.plays_file.is_none());
        library.history_serial = 1;
        let _ = library.history_loaded(1, Ok(Arc::new(Vec::new())));
        library.queue = Some(Queue::new(
            ListKey::Liked,
            Arc::new(vec![plain_track("1")]),
            0,
        ));
        library.launch(0, plain_track("1"), 0.0);
        library.history_note_play();
        // С диска: вчерашний «5» и сегодняшний «1» (он уже есть в сеансе).
        let today = api::today_utc();
        let disk = |id: &str, date| history::LocalPlay {
            track: plain_track(id),
            context: api::HistoryContext::Wave,
            date,
            at: 0,
        };
        library.local_plays_loaded(vec![disk("5", (2000, 1, 1)), disk("1", today)]);
        // День, подпись группы и треки — одной строкой на группу.
        let rows: Vec<String> = library
            .history_shown
            .iter()
            .flat_map(|day| {
                day.items.iter().map(move |item| {
                    let ids: Vec<&str> = item.tracks.iter().map(|t| t.id.as_str()).collect();
                    format!("{:?} {} {}", day.date, item.context.label(), ids.join(","))
                })
            })
            .collect();
        assert_eq!(
            rows,
            [
                format!("{:?} Мне нравится 1", Some(today)),
                "Some((2000, 1, 1)) Моя волна 5".to_owned(),
            ]
        );
        // Повторное чтение ничего не меняет.
        library.local_plays_loaded(vec![disk("7", today)]);
        assert_eq!(library.local_plays.plays().count(), 2);
    }

    #[test]
    fn quality_text_names_codec() {
        let info = |codec, kbps| StreamInfo { codec, kbps };
        assert_eq!(quality_text(info(Codec::Flac, None), true), "FLAC");
        assert_eq!(quality_text(info(Codec::Flac, None), false), "FLAC");
        assert_eq!(quality_text(info(Codec::Aac, Some(256)), true), "AAC · 256");
        assert_eq!(quality_text(info(Codec::Aac, Some(256)), false), "256");
        assert_eq!(quality_text(info(Codec::Aac, None), true), "AAC");
        assert_eq!(quality_text(info(Codec::Mp3, Some(320)), true), "MP3 · 320");
        assert_eq!(quality_text(info(Codec::Mp3, Some(320)), false), "320");
    }

    fn mp3() -> Option<StreamInfo> {
        Some(StreamInfo {
            codec: Codec::Mp3,
            kbps: Some(320),
        })
    }

    /// Сколько элементов показано (значок качества: короткий — один, полный — два).
    fn fit_rank(fit: &ExtrasFit) -> usize {
        [
            fit.more,
            fit.lyrics,
            fit.queue,
            fit.devices,
            fit.equalizer,
            fit.mini,
        ]
        .into_iter()
        .filter(|shown| *shown)
        .count()
            + match fit.quality {
                Quality::Hidden => 0,
                Quality::Short => 1,
                Quality::Full => 2,
            }
    }

    #[test]
    fn extras_room_splits_free_space() {
        // Не компактное: свободное делится пополам.
        assert_eq!(
            extras_room(1100.0 - SIDEBAR, false),
            (1100.0 - SIDEBAR - PLAYER_CHROME - CONTROLS) / 2.0
        );
        // Компактное: середина уже, краю две пятых.
        assert_eq!(
            extras_room(728.0, true),
            (728.0 - PLAYER_CHROME - CONTROLS_COMPACT) * 0.4
        );
        // Тесное окно не даёт отрицательной ширины.
        assert_eq!(extras_room(100.0, false), 0.0);
    }

    #[test]
    fn extras_fit_keeps_priority_at_1100() {
        // Окно 1100 с меню: край ~177 px: четыре кнопки и громкость; «Устройства»
        // стоят выше эквалайзера, поэтому эквалайзер, мини-плеер и значок не влезают.
        let room = extras_room(1100.0 - SIDEBAR, false);
        let fit = extras_fit(room, false, mp3());
        assert!(fit.more && fit.lyrics && fit.queue && fit.devices);
        assert!(!fit.equalizer && !fit.mini);
        assert_eq!(fit.quality, Quality::Hidden);
    }

    #[test]
    fn extras_fit_shows_everything_when_wide() {
        let room = extras_room(1920.0 - SIDEBAR, false);
        let fit = extras_fit(room, false, mp3());
        assert!(fit.more && fit.lyrics && fit.queue && fit.devices && fit.equalizer && fit.mini);
        assert_eq!(fit.quality, Quality::Full);
        // Без потока значка нет.
        assert_eq!(extras_fit(room, false, None).quality, Quality::Hidden);
    }

    #[test]
    fn extras_fit_in_minimum_compact_window() {
        // 960 с меню 232: в узком окне кнопки тоже решает место.
        let room = extras_room(960.0 - SIDEBAR, true);
        let fit = extras_fit(room, false, mp3());
        assert!(fit.more && fit.lyrics);
        assert_eq!(fit.quality, Quality::Hidden);
        // Совсем нет места: остаётся одна громкость.
        assert_eq!(
            extras_fit(0.0, false, mp3()),
            ExtrasFit {
                more: false,
                lyrics: false,
                queue: false,
                devices: false,
                equalizer: false,
                mini: false,
                quality: Quality::Hidden,
            }
        );
    }

    #[test]
    fn extras_fit_fullscreen_has_no_equalizer() {
        let fit = extras_fit(1000.0, true, mp3());
        assert!(!fit.equalizer);
        assert!(fit.more && fit.lyrics && fit.queue && fit.devices && fit.mini);
        assert_eq!(fit.quality, Quality::Full);
        // Без эквалайзера мини-плеер появляется на кнопку раньше.
        let room = 5.0 * (EXTRAS_BUTTON + EXTRAS_GAP) + EXTRAS_BUTTON;
        assert!(extras_fit(room, true, None).mini);
        assert!(!extras_fit(room, false, None).mini);
    }

    #[test]
    fn extras_fit_is_monotonic() {
        let mut last = 0;
        for room in 0..600 {
            let rank = fit_rank(&extras_fit(room as f32, false, mp3()));
            assert!(rank >= last, "room {room}: {rank} < {last}");
            last = rank;
        }
        assert_eq!(last, 8);
    }

    #[test]
    fn volume_percent_rounds_and_mutes() {
        assert_eq!(volume_percent(0.204, false), 20);
        assert_eq!(volume_percent(0.205, false), 21);
        assert_eq!(volume_percent(1.0, false), 100);
        assert_eq!(volume_percent(0.0, false), 0);
        assert_eq!(volume_percent(0.7, true), 0);
        assert_eq!(volume_percent(1.7, false), 100);
        assert_eq!(volume_percent(f32::NAN, false), 0);
    }

    #[test]
    fn volume_panel_waits_before_closing() {
        let start = Instant::now();
        let mut motion = PlayerMotion::new(0.5);
        // Наведение открывает панель сразу.
        motion.volume_over_icon = true;
        motion.volume_sync(start);
        assert!(motion.volume_panel.value());
        assert!(motion.volume_close_at.is_none());
        // Курсор ушёл со значка: закрытие отложено.
        motion.volume_over_icon = false;
        motion.volume_sync(start);
        assert!(motion.volume_close_at.is_some());
        motion.volume_tick(start + VOLUME_CLOSE_DELAY / 2);
        assert!(motion.volume_panel.value());
        // Курсор перешёл на панель до срока: закрытие отменено.
        motion.volume_over_panel = true;
        motion.volume_sync(start + VOLUME_CLOSE_DELAY / 2);
        assert!(motion.volume_close_at.is_none());
        motion.volume_tick(start + VOLUME_CLOSE_DELAY * 2);
        assert!(motion.volume_panel.value());
        // Ушёл с панели при зажатом ползунке: панель держится.
        motion.volume_dragging = true;
        motion.volume_over_panel = false;
        motion.volume_sync(start + VOLUME_CLOSE_DELAY * 2);
        assert!(motion.volume_close_at.is_none());
        // Отпустили: закрытие через задержку.
        let released = start + VOLUME_CLOSE_DELAY * 3;
        motion.volume_dragging = false;
        motion.volume_sync(released);
        motion.volume_tick(released + VOLUME_CLOSE_DELAY / 2);
        assert!(motion.volume_panel.value());
        motion.volume_tick(released + VOLUME_CLOSE_DELAY);
        assert!(!motion.volume_panel.value());
        assert!(motion.volume_close_at.is_none());
    }

    fn track(id: &str) -> Track {
        Track {
            id: id.to_owned(),
            title: Some(format!("Трек {id}")),
            version: None,
            artists: Vec::new(),
            album_id: None,
            album_title: None,
            duration: None,
            available: true,
            cover_uri: None,
            warning: api::Warning::None,
            loudness: None,
            lyrics: None,
        }
    }

    fn ids(list: &[Track]) -> Vec<&str> {
        list.iter().map(|track| track.id.as_str()).collect()
    }

    #[test]
    fn like_goes_to_the_top_and_unlike_leaves_the_list() {
        let mut set: HashSet<String> = ["1", "2"].map(str::to_owned).into();
        let mut list = vec![track("1"), track("2")];
        assert!(edit_likes(&mut set, Some(&mut list), &track("3"), true));
        assert_eq!(ids(&list), ["3", "1", "2"]);
        assert!(set.contains("3"));
        assert!(edit_likes(&mut set, Some(&mut list), &track("1"), false));
        assert_eq!(ids(&list), ["3", "2"]);
        assert!(!set.contains("1"));
    }

    #[test]
    fn repeated_edits_change_nothing() {
        let mut set: HashSet<String> = ["1"].map(str::to_owned).into();
        let mut list = vec![track("1")];
        assert!(!edit_likes(&mut set, Some(&mut list), &track("1"), true));
        assert!(!edit_likes(&mut set, Some(&mut list), &track("9"), false));
        assert_eq!(ids(&list), ["1"]);
    }

    #[test]
    fn failed_like_is_rolled_back() {
        let mut set: HashSet<String> = ["1"].map(str::to_owned).into();
        let mut list = vec![track("1")];
        // Поставили лайк, сервер отказал: откат убирает трек и из списка.
        edit_likes(&mut set, Some(&mut list), &track("2"), true);
        edit_likes(&mut set, Some(&mut list), &track("2"), false);
        assert_eq!(ids(&list), ["1"]);
        assert_eq!(set.len(), 1);
        // Снятие откатывается возвратом трека (в начало списка).
        edit_likes(&mut set, Some(&mut list), &track("1"), false);
        edit_likes(&mut set, Some(&mut list), &track("1"), true);
        assert_eq!(ids(&list), ["1"]);
        assert!(set.contains("1"));
    }

    #[test]
    fn without_a_loaded_list_only_the_set_changes() {
        let mut set = HashSet::new();
        assert!(edit_likes(&mut set, None, &track("5"), true));
        assert!(set.contains("5"));
        assert!(edit_likes(&mut set, None, &track("5"), false));
        assert!(set.is_empty());
    }
}
