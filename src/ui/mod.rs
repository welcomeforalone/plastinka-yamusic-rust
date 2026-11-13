//! Окно приложения на iced: вход, «Нужна подписка», главное окно с боковым
//! меню, списком «Мне нравится» и панелью плеера, окно «О программе».
//!
//! Главное окно и плеер в нём создаются только с пропуском
//! [`PlaybackPermit`], то есть после явного подтверждения Плюса.

mod anim;
mod covers;
mod drag_pulse;
mod filter;
mod gate;
mod heartbeat;
mod history;
mod icons;
mod library;
mod lyrics_view;
mod marquee;
mod mini;
mod pause;
mod pointer;
mod queue;
mod scroll;
mod theme;
mod visualizer;
mod widgets;

use std::io::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use iced::animation::Animation;
use iced::widget::{column, container, mouse_area, row, stack};
use iced::window::{self, Direction};
use iced::{Element, Length, Size, Subscription, Task, keyboard, mouse};

use crate::APP_NAME;
use crate::api::{self, AccountStatus, ApiError, AudioQuality, PlaybackPermit};
use crate::auth::{self, AuthError, DeviceCode, OAuthApp};
use crate::discord;
use crate::media_controls;
use crate::net::{self, NetError};
use crate::plugins::PluginSettings;
use crate::storage::config::{self, Config};
use crate::storage::token::{StoredToken, TokenStore};
use crate::storage::{self, AppDirs, StorageError};
use crate::texts::sentence;

/// Запускает окно. Блокирует поток до закрытия окна.
pub fn run() -> iced::Result {
    // Окон два (главное и мини-плеер), поэтому `daemon`: он не закрывается
    // сам, выход — явный (`iced::exit`) при закрытии главного окна.
    let mut app = iced::daemon(App::boot, App::update, App::view)
        .title(App::title)
        .theme(App::theme)
        .scale_factor(App::scale_factor)
        .subscription(App::subscription)
        .default_font(theme::REGULAR)
        .antialiasing(true);
    for font in theme::FONTS {
        app = app.font(font);
    }
    app.run()
}

fn window_settings() -> window::Settings {
    window::Settings {
        size: Size::new(1280.0, 800.0),
        min_size: Some(Size::new(960.0, 600.0)),
        // Свой заголовок окна: системная рамка не нужна.
        decorations: false,
        // Закрытие обрабатываем сами: несохранённый токен надо отозвать.
        exit_on_close_request: false,
        icon: app_icon(),
        platform_specific: platform_specific(),
        ..window::Settings::default()
    }
}

/// Иконка окна (заголовок, панель задач, Alt+Tab). Без неё — системная.
pub(crate) fn app_icon() -> Option<window::Icon> {
    const PNG: &[u8] = include_bytes!("../../assets/app/plastinka.png");
    let image = image::load_from_memory_with_format(PNG, image::ImageFormat::Png)
        .ok()?
        .into_rgba8();
    let (width, height) = image.dimensions();
    window::icon::from_rgba(image.into_raw(), width, height).ok()
}

/// Платформенные параметры окна, общие для главного окна и мини-плеера.
pub(crate) fn platform_specific() -> window::settings::PlatformSpecific {
    window::settings::PlatformSpecific {
        // Тень у окна без системной рамки.
        #[cfg(windows)]
        undecorated_shadow: true,
        // Совпадает с именем plastinka.desktop: по нему Wayland и X11 берут
        // иконку и группируют окна на панели.
        #[cfg(target_os = "linux")]
        application_id: APP_ID.to_owned(),
        ..window::settings::PlatformSpecific::default()
    }
}

/// Идентификатор приложения в Linux — имя .desktop-файла без расширения.
#[cfg(target_os = "linux")]
const APP_ID: &str = "plastinka";

/// Через сколько повторять запрос, если нет сети.
const OFFLINE_RETRY: Duration = Duration::from_secs(10);
/// Сколько показывать «Скопировано» и всплывающее сообщение.
const COPIED_FOR: Duration = Duration::from_secs(2);
const TOAST_FOR: Duration = Duration::from_secs(6);
/// Сколько ждать отзыва несохранённого токена при закрытии окна.
const REVOKE_ON_CLOSE: Duration = Duration::from_secs(3);
/// Ширина окна, ниже которой в плеере остаются только главные кнопки.
pub const COMPACT_WIDTH: f32 = 1100.0;

/// Ошибка для показа в окне. Тексты — из ошибок модулей, на русском.
#[derive(Debug, Clone)]
pub struct Failure {
    pub kind: FailureKind,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// Нет сети, сервер не ответил, соединение оборвалось.
    Offline,
    /// Хранилище паролей недоступно или заблокировано.
    Keyring,
    /// Токен не действует.
    Unauthorized,
    /// Код устройства истёк.
    CodeExpired,
    Other,
}

impl Failure {
    fn new(kind: FailureKind, text: impl ToString) -> Self {
        Self {
            kind,
            text: text.to_string(),
        }
    }
}

impl From<NetError> for Failure {
    fn from(err: NetError) -> Self {
        let kind = match err {
            NetError::Offline | NetError::Timeout | NetError::Interrupted => FailureKind::Offline,
            NetError::Tls(_) | NetError::Other(_) => FailureKind::Other,
        };
        Self::new(kind, err)
    }
}

impl From<StorageError> for Failure {
    fn from(err: StorageError) -> Self {
        let kind = match err {
            StorageError::SecretStoreUnavailable(_)
            | StorageError::SecretStoreLocked
            | StorageError::SecretStore(_)
            | StorageError::SecretStoreDuplicates => FailureKind::Keyring,
            _ => FailureKind::Other,
        };
        Self::new(kind, err)
    }
}

impl From<AuthError> for Failure {
    fn from(err: AuthError) -> Self {
        match err {
            AuthError::Net(err) => err.into(),
            AuthError::CodeExpired => Self::new(FailureKind::CodeExpired, err),
            err => Self::new(FailureKind::Other, err),
        }
    }
}

impl From<ApiError> for Failure {
    fn from(err: ApiError) -> Self {
        match err {
            ApiError::Net(err) => err.into(),
            ApiError::Unauthorized => Self::new(FailureKind::Unauthorized, err),
            err => Self::new(FailureKind::Other, err),
        }
    }
}

/// Всё, что нужно для входа: приложение OAuth, `device_id` и открытое
/// хранилище паролей, проверенное пробным чтением.
pub struct LoginSetup {
    app: OAuthApp,
    store: TokenStore,
}

/// Что повторить по кнопке «Повторить».
#[derive(Clone)]
pub enum Retry {
    Start,
    Login,
    Save(Arc<StoredToken>, Option<AccountStatus>),
    Check(Arc<StoredToken>),
}

/// Карточка ошибки на весь экран.
pub struct Problem {
    pub kind: FailureKind,
    pub title: String,
    pub text: String,
    pub retry: Retry,
    /// Когда повторить самому (только без сети).
    pub retry_at: Option<Instant>,
    /// Показаны ли подсказки «Как включить».
    pub show_help: bool,
}

enum Screen {
    Starting,
    Problem(Problem),
    Login(gate::Login),
    Checking,
    Paywall(gate::Paywall),
    Main(Box<library::Library>),
}

/// То, что окно хранит в `config.toml`: вид меню и внешний вид.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prefs {
    pub sidebar_collapsed: bool,
    pub accent: theme::Accent,
    /// Масштаб окна, проценты.
    pub scale: u32,
    pub visualizer: bool,
    /// Качество звука и нормализация громкости (раздел «Звук»).
    pub quality: AudioQuality,
    pub normalize: bool,
    /// Искать текст песни в LRCLib, если у Яндекса его нет.
    pub lyrics_lrclib: bool,
    /// Discord Rich Presence (блок «Discord»).
    pub discord: discord::Settings,
    /// Application ID Discord задан: без него блок «Discord» неактивен.
    /// В `config.toml` не пишется.
    pub discord_ready: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            sidebar_collapsed: false,
            accent: theme::Accent::default(),
            scale: config::SCALE_DEFAULT,
            visualizer: true,
            quality: AudioQuality::default(),
            normalize: false,
            lyrics_lrclib: true,
            discord: discord::Settings::default(),
            discord_ready: false,
        }
    }
}

impl Prefs {
    fn from_config(config: &Config) -> Self {
        Self {
            sidebar_collapsed: config.sidebar_collapsed,
            accent: config
                .accent
                .as_deref()
                .and_then(theme::Accent::from_key)
                .unwrap_or_default(),
            scale: config.scale_percent(),
            visualizer: config.visualizer.unwrap_or(true),
            quality: config
                .audio_quality
                .as_deref()
                .and_then(AudioQuality::from_key)
                .unwrap_or_default(),
            normalize: config.normalize.unwrap_or(false),
            lyrics_lrclib: config.lyrics_lrclib.unwrap_or(true),
            discord: discord::Settings {
                enabled: config.discord_enabled.unwrap_or(true),
                status: config
                    .discord_status
                    .as_deref()
                    .and_then(discord::StatusKind::from_key)
                    .unwrap_or_default(),
                on_pause: config.discord_on_pause.unwrap_or(false),
                button: config.discord_button.unwrap_or(true),
                lang: config
                    .discord_lang
                    .as_deref()
                    .and_then(discord::Lang::from_key)
                    .unwrap_or_default(),
                app: config
                    .discord_app_name
                    .as_deref()
                    .and_then(discord::AppName::from_key)
                    .unwrap_or_default(),
            },
            discord_ready: !discord::app_id(
                config.discord_app_id.as_deref(),
                discord::AppName::default(),
            )
            .is_empty(),
        }
    }

    fn write(self, config: &mut Config) {
        config.sidebar_collapsed = self.sidebar_collapsed;
        config.accent = Some(self.accent.key().to_owned());
        config.scale = Some(i64::from(self.scale));
        config.visualizer = Some(self.visualizer);
        config.audio_quality = Some(self.quality.key().to_owned());
        config.normalize = Some(self.normalize);
        config.lyrics_lrclib = Some(self.lyrics_lrclib);
        config.discord_enabled = Some(self.discord.enabled);
        config.discord_status = Some(self.discord.status.key().to_owned());
        config.discord_on_pause = Some(self.discord.on_pause);
        config.discord_button = Some(self.discord.button);
        config.discord_lang = Some(self.discord.lang.key().to_owned());
        config.discord_app_name = Some(self.discord.app.key().to_owned());
    }
}

/// Ctrl + = / Ctrl + − / Ctrl + 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zoom {
    In,
    Out,
    Reset,
}

/// Диалог поверх окна.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialog {
    Licenses,
    ConfirmLogout,
    /// Согласие: Slopless ставит «Не нравится» ИИ-трекам (запись в аккаунт).
    SloplessConsent,
}

#[derive(Clone)]
pub enum Message {
    WindowOpened(Option<window::Id>),
    /// Размер любого окна (главного или мини-плеера).
    AnyResized(window::Id, Size),
    /// Просьба закрыть любое окно (крестик, Alt + F4).
    CloseRequested(window::Id),
    /// Мини-плеер: открыть из нижнего плеера; закрепить поверх окон;
    /// вернуть главное окно; закрыть; тянуть за фон; окно сдвинули.
    MiniOpen,
    MiniPin,
    MiniExpand,
    MiniClose,
    MiniDrag,
    MiniMoved(window::Id, iced::Point),
    /// Системный дескриптор окна (только Windows) для таймера перетаскивания.
    NativeHandle(Option<isize>),
    /// Команда системного плеера (медиаклавиши, оверлей, MPRIS).
    Media(media_controls::Command),
    WindowResized(Size),
    /// Свёрнуто ли окно (`None` — платформа не ответила, считаем «нет»).
    Minimized(Option<bool>),
    DragWindow,
    ToggleMaximize,
    Minimize,
    CloseWindow,
    ResizeWindow(Direction),
    Tick(Instant),
    Frame(Instant),
    Key(keyboard::Event),
    /// Esc, в том числе когда клавишу уже забрало поле ввода.
    Escape,
    Noop,
    OpenUrl(String),

    TokenLoaded(Result<Option<Arc<StoredToken>>, Failure>),
    Retry,
    ToggleHelp,

    CodeReceived(Result<(Arc<LoginSetup>, Arc<DeviceCode>), Failure>),
    NewCode,
    CopyCode,
    TokenReceived(Result<Arc<StoredToken>, Failure>),
    LoginChecked(Arc<StoredToken>, Result<AccountStatus, Failure>),
    TokenSaved(Arc<StoredToken>, Option<AccountStatus>, Result<(), Failure>),

    StatusChecked(Arc<StoredToken>, Result<AccountStatus, Failure>),
    Recheck,
    Logout,
    LoggedOut(Arc<StoredToken>, Result<(), Failure>),

    ConfigLoaded(
        Prefs,
        PluginSettings,
        Option<String>,
        api::WaveSettings,
        mini::MiniPrefs,
    ),
    ToggleSidebar,
    SetAccent(theme::Accent),
    Zoom(Zoom),
    ToggleVisualizer,
    /// Качество звука и нормализация громкости (панель «Настройки звука»).
    SetAudioQuality(AudioQuality),
    SetNormalize(bool),
    SetLyricsLrclib(bool),
    /// Настройки Discord Rich Presence.
    SetDiscord(discord::Settings),
    /// Новые настройки плагинов.
    SetPlugins(PluginSettings),
    /// Пользователь разрешил Slopless ставить «Не нравится».
    AllowDislike,
    PrefsSaved(Result<(), Failure>),

    OpenDialog(Dialog),
    CloseDialog,
    CoverLoaded(covers::Key, Option<Vec<u8>>),
    BigCoverLoaded(String, Option<Arc<covers::Big>>),
    Library(library::Message),
}

pub struct App {
    window: Option<window::Id>,
    size: Size,
    /// Окно свёрнуто: перерисовки стоят (см. `pause`).
    minimized: bool,
    http: Result<reqwest::Client, String>,
    covers: covers::Covers,
    screen: Screen,
    /// Хранилище и приложение для входа: нужны, пока идёт вход.
    login_setup: Option<Arc<LoginSetup>>,
    /// Токен получен, но ещё не сохранён. При закрытии окна его отзываем,
    /// иначе он останется действующим, а пользователь его нигде не увидит.
    unsaved: Option<Arc<StoredToken>>,
    dialog: Option<Dialog>,
    /// Диалог на экране: после закрытия он ещё тает.
    dialog_shown: Option<Dialog>,
    dialog_anim: Animation<bool>,
    toast_anim: Animation<bool>,
    /// Появление текущего экрана или раздела.
    page: Animation<bool>,
    /// Меню свёрнуто (плавно).
    sidebar: Animation<bool>,
    /// Вид меню и внешний вид. Запоминаются в `config.toml`.
    prefs: Prefs,
    /// Discord Rich Presence: фоновая задача, пока открыто главное окно.
    discord: Option<discord::Rpc>,
    /// Application ID, с которым запущена задача Discord.
    discord_app_id: String,
    /// `discord_app_id` из `config.toml` (своё приложение вместо встроенных).
    discord_override: Option<String>,
    /// Системный плеер: медиаклавиши и оверлей; только в главном окне.
    media: Option<media_controls::Controls>,
    /// Системный плеер не удалось создать или он отказал: больше не пробуем.
    media_failed: bool,
    /// Системный дескриптор окна (Windows); без него SMTC не создать.
    hwnd: Option<isize>,
    /// Окно мини-плеера, пока оно открыто (только из главного экрана).
    mini: Option<window::Id>,
    /// Закреплено поверх других окон (по умолчанию да).
    mini_pinned: bool,
    /// Положение и раскладка мини-плеера (`[mini_player]` в `config.toml`).
    mini_prefs: mini::MiniPrefs,
    /// Окно закрывается: статус Discord уже убран.
    closing: bool,
    /// Плагины: что включено. Тоже в `config.toml`, таблица `[plugins]`.
    plugins: PluginSettings,
    /// Выбор в меню «Моей волны» (`[wave_settings]`, в аккаунт не пишется).
    wave_settings: api::WaveSettings,
    /// Тема iced с текущим акцентом.
    theme: iced::Theme,
    /// Настройки сейчас пишутся; `save_again` — за это время они изменились.
    saving: bool,
    save_again: bool,
    started: Instant,
    now: Instant,
    toast: Option<(String, Instant)>,
}

impl App {
    fn boot() -> (Self, Task<Message>) {
        let cache_dir = AppDirs::discover()
            .ok()
            .map(|dirs| dirs.cache_dir().to_path_buf());
        let now = Instant::now();
        let (main_window, open_main) = window::open(window_settings());
        let app = Self {
            window: Some(main_window),
            size: window_settings().size,
            minimized: false,
            http: net::http_client().map_err(|err| err.to_string()),
            covers: covers::Covers::new(cache_dir),
            screen: Screen::Starting,
            login_setup: None,
            unsaved: None,
            dialog: None,
            dialog_shown: None,
            dialog_anim: anim::settled(false, anim::PANEL),
            toast_anim: anim::settled(false, anim::PANEL),
            page: anim::entering(anim::PAGE, now),
            sidebar: anim::settled(false, anim::PANEL),
            prefs: Prefs::default(),
            discord: None,
            discord_app_id: String::new(),
            discord_override: None,
            media: None,
            media_failed: false,
            hwnd: None,
            mini: None,
            mini_pinned: true,
            mini_prefs: mini::MiniPrefs::default(),
            closing: false,
            plugins: PluginSettings::default(),
            wave_settings: api::WaveSettings::default(),
            theme: theme::theme(),
            saving: false,
            save_again: false,
            started: now,
            now,
            toast: None,
        };
        let task = Task::batch([
            open_main.map(|id| Message::WindowOpened(Some(id))),
            Task::perform(load_token(), Message::TokenLoaded),
            // Не прочитались настройки — всё по умолчанию.
            Task::perform(load_config(), |config| {
                let config = config.unwrap_or_default();
                Message::ConfigLoaded(
                    Prefs::from_config(&config),
                    plugin_settings(&config),
                    config.discord_app_id.clone(),
                    api::WaveSettings::from_config(&config.wave_settings),
                    mini::MiniPrefs::from_config(&config.mini_player),
                )
            }),
        ]);
        (app, task)
    }

    fn http(&self) -> Result<reqwest::Client, Failure> {
        self.http
            .clone()
            .map_err(|text| Failure::new(FailureKind::Other, text))
    }

    /// Доля оборота спиннера: один оборот в секунду.
    fn turn(&self) -> f32 {
        let elapsed = self.now.saturating_duration_since(self.started);
        (elapsed.as_millis() % 1000) as f32 / 1000.0
    }

    fn title(&self, window: window::Id) -> String {
        if self.mini == Some(window) {
            format!("{APP_NAME} — мини-плеер")
        } else {
            APP_NAME.to_owned()
        }
    }

    fn theme(&self, _window: window::Id) -> iced::Theme {
        self.theme.clone()
    }

    fn scale_factor(&self, window: window::Id) -> f32 {
        // Мини-плеер нарисован под свой размер: масштаб главного окна к нему
        // не применяется.
        if self.mini == Some(window) {
            return 1.0;
        }
        // Проценты — от 75 до 200: потерь при переводе нет.
        self.prefs.scale as f32 / 100.0
    }

    fn compact(&self) -> bool {
        self.size.width < COMPACT_WIDTH
    }

    /// Обработка сообщения. Сменился экран или раздел — он появляется плавно.
    fn update(&mut self, message: Message) -> Task<Message> {
        self.now = Instant::now();
        let before = self.page_key();
        // Кадры и секундные тики статус не меняют.
        let discord_relevant = !matches!(
            message,
            Message::Frame(_) | Message::Tick(_) | Message::MiniMoved(..)
        );
        let task = self.handle(message);
        let task = Task::batch([task, self.sync_mini()]);
        if discord_relevant {
            self.sync_discord();
            self.sync_media();
        }
        drag_pulse::set_wanted(self.wants_frames());
        // Сетка карточек строится по ширине раздела справа от меню.
        let sidebar = if self.prefs.sidebar_collapsed {
            library::SIDEBAR_COLLAPSED
        } else {
            library::SIDEBAR
        };
        if let Screen::Main(library) = &mut self.screen {
            library.set_layout(self.size.width, sidebar);
        }
        if self.page_key() != before {
            self.page = anim::entering(anim::PAGE, self.now);
        }
        // Ушедшие диалог и сообщение убираем, когда они растаяли.
        if self.dialog.is_none() && !self.dialog_anim.is_animating(self.now) {
            self.dialog_shown = None;
        }
        if self.toast.is_some()
            && !self.toast_anim.value()
            && !self.toast_anim.is_animating(self.now)
        {
            self.toast = None;
        }
        task
    }

    /// Отдаёт задаче Discord RPC, что сейчас играет. Задача сама решает, что
    /// и когда отправлять; окну ответы не приходят.
    fn sync_discord(&mut self) {
        if self.closing {
            return;
        }
        let Screen::Main(library) = &self.screen else {
            // Вышли из аккаунта или окно не главное: статус очищается.
            self.discord = None;
            return;
        };
        // Сменили имя в статусе — подключаемся к другому приложению; старое
        // при этом убирает свой статус.
        let app_id = discord::app_id(self.discord_override.as_deref(), self.prefs.discord.app);
        if app_id != self.discord_app_id {
            self.discord = None;
        }
        if self.discord.is_none() {
            self.discord = discord::Rpc::start(&app_id);
            self.discord_app_id = app_id;
        }
        if let Some(rpc) = &self.discord {
            rpc.set(library.discord_state(self.prefs.discord));
        }
    }

    /// Системный плеер: создаётся в главном окне после подтверждения Плюса
    /// (пропуск берётся у библиотеки), убирается при выходе из аккаунта.
    /// Любая ошибка — окно работает без него, причина в stderr (без токена).
    fn sync_media(&mut self) {
        if self.closing {
            return;
        }
        let Screen::Main(library) = &self.screen else {
            self.media = None;
            self.media_failed = false;
            return;
        };
        if self.media.is_none() && !self.media_failed {
            // На Windows ждём окно: без HWND SMTC не создать.
            if self.window.is_none() || (cfg!(windows) && self.hwnd.is_none()) {
                return;
            }
            let Some(permit) = library.media_permit() else {
                return;
            };
            match media_controls::Controls::start(&permit, self.hwnd) {
                Ok(controls) => self.media = Some(controls),
                Err(err) => {
                    self.media_failed = true;
                    let _ = writeln!(
                        std::io::stderr(),
                        "Предупреждение: системный плеер недоступен: {err}"
                    );
                }
            }
        }
        if let Some(controls) = &mut self.media
            && let Err(err) = controls.sync(&library.media_state())
        {
            self.media = None;
            self.media_failed = true;
            let _ = writeln!(
                std::io::stderr(),
                "Предупреждение: системный плеер отключён: {err}"
            );
        }
    }

    /// Какой экран на виду: для перехода между экранами.
    fn page_key(&self) -> (u8, Option<(library::Section, u64)>) {
        match &self.screen {
            Screen::Starting => (0, None),
            Screen::Problem(_) => (1, None),
            Screen::Login(_) => (2, None),
            Screen::Checking => (3, None),
            Screen::Paywall(_) => (4, None),
            Screen::Main(library) => (5, Some(library.view_key())),
        }
    }

    /// Идёт ли сейчас переход, которому нужны кадры.
    fn transitioning(&self) -> bool {
        let now = self.now;
        self.page.is_animating(now)
            || self.sidebar.is_animating(now)
            || self.toast_anim.is_animating(now)
            || self.dialog_anim.is_animating(now)
            || self.covers.animating(now)
            || match &self.screen {
                Screen::Main(library) => library.transitioning(now),
                _ => false,
            }
    }

    /// Доли переходов для видов в этом кадре.
    fn moment(&self) -> Moment {
        let collapsed = anim::progress(&self.sidebar, self.now);
        Moment {
            now: self.now,
            page: anim::progress(&self.page, self.now),
            sidebar: library::SIDEBAR + (library::SIDEBAR_COLLAPSED - library::SIDEBAR) * collapsed,
        }
    }

    fn open_dialog(&mut self, dialog: Dialog) {
        self.dialog = Some(dialog);
        self.dialog_shown = Some(dialog);
        self.dialog_anim.go_mut(true, Instant::now());
    }

    fn close_dialog(&mut self) {
        if self.dialog.take().is_some() {
            self.dialog_anim.go_mut(false, Instant::now());
        }
    }

    fn handle(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::WindowOpened(id) => {
                self.window = id;
                return id.map_or_else(Task::none, |id| {
                    Task::batch([
                        window::size(id).map(Message::WindowResized),
                        window::run(id, native_handle).map(Message::NativeHandle),
                    ])
                });
            }
            Message::AnyResized(id, size) => {
                // Размер мини-плеера главному окну не нужен.
                if self.window == Some(id) {
                    return self.handle(Message::WindowResized(size));
                }
            }
            Message::CloseRequested(id) => {
                return if self.mini == Some(id) {
                    self.close_mini(false)
                } else {
                    self.close()
                };
            }
            Message::MiniOpen => return self.open_mini(),
            Message::MiniPin => {
                self.mini_pinned = !self.mini_pinned;
                if let Some(id) = self.mini {
                    return window::set_level(id, mini::level(self.mini_pinned));
                }
            }
            Message::MiniExpand => return self.close_mini(true),
            Message::MiniClose => return self.close_mini(false),
            Message::MiniDrag => {
                if let Some(id) = self.mini {
                    return window::drag(id);
                }
            }
            Message::MiniMoved(id, point) => {
                if self.mini == Some(id) {
                    self.mini_prefs.moved(point);
                }
            }
            Message::NativeHandle(hwnd) => {
                if let Some(hwnd) = hwnd {
                    drag_pulse::set_window(hwnd);
                }
                self.hwnd = hwnd;
            }
            Message::Media(command) => {
                let http = self.http.as_ref().ok();
                if let Screen::Main(library) = &mut self.screen {
                    let (task, _) = library.update(library::Message::Media(command), http);
                    return task.map(Message::Library);
                }
            }
            Message::WindowResized(size) => {
                self.size = size;
                if let Screen::Main(library) = &mut self.screen {
                    library.resized(size.height);
                }
                // Сворачивание и разворачивание всегда приходят как Resized:
                // узнаём, свёрнуто ли окно (у окна без рамки размер свёрнутого
                // не нулевой, iced сам рисовать не перестанет).
                return self.with_window(|id| window::is_minimized(id).map(Message::Minimized));
            }
            Message::Minimized(state) => self.minimized = state.unwrap_or(false),
            Message::DragWindow => {
                drag_pulse::start();
                return self.with_window(window::drag);
            }
            Message::ToggleMaximize => return self.with_window(window::toggle_maximize),
            Message::Minimize => return self.with_window(|id| window::minimize(id, true)),
            Message::CloseWindow => return self.close(),
            Message::ResizeWindow(direction) => {
                drag_pulse::start();
                return self.with_window(|id| window::drag_resize(id, direction));
            }
            Message::Tick(now) => {
                self.now = now;
                return self.tick(now);
            }
            Message::Frame(now) => {
                self.now = now;
                if let Screen::Main(library) = &mut self.screen {
                    return library
                        .frame(now, self.prefs.visualizer)
                        .map(Message::Library);
                }
            }
            Message::Key(event) => return self.key(event),
            Message::Escape => return self.escape(),
            Message::Noop => {}
            Message::OpenUrl(url) => {
                if let Err(err) = open::that_detached(&url) {
                    self.show_toast(format!("Не удалось открыть браузер: {err}. Адрес: {url}"));
                }
            }

            Message::TokenLoaded(result) => return self.token_loaded(result),
            Message::Retry => return self.retry(),
            Message::ToggleHelp => {
                if let Screen::Problem(problem) = &mut self.screen {
                    problem.show_help = !problem.show_help;
                }
            }

            Message::CodeReceived(result) => return self.code_received(result),
            Message::NewCode => return self.start_login(None),
            Message::CopyCode => {
                if let Screen::Login(login) = &mut self.screen
                    && let Some(code) = login.code()
                {
                    let code = code.user_code.clone();
                    login.copied_until = Some(Instant::now() + COPIED_FOR);
                    return iced::clipboard::write(code);
                }
            }
            Message::TokenReceived(result) => return self.token_received(result),
            Message::LoginChecked(token, result) => return self.login_checked(token, result),
            Message::TokenSaved(token, status, result) => {
                return self.token_saved(token, status, result);
            }

            Message::StatusChecked(token, result) => return self.status_checked(token, result),
            Message::Recheck => {
                if let Screen::Paywall(paywall) = &mut self.screen
                    && !paywall.checking
                {
                    paywall.checking = true;
                    paywall.error = None;
                    let token = paywall.token.clone();
                    return self.check_status(token);
                }
            }
            Message::Logout => return self.logout(),
            Message::LoggedOut(token, result) => match result {
                // Отзываем только после удаления с компьютера, как в CLI.
                Ok(()) => {
                    return Task::batch([self.start_login(None), self.revoke_in_background(token)]);
                }
                Err(failure) => self.show_toast(format!("Выйти не удалось: {}", failure.text)),
            },

            Message::ConfigLoaded(prefs, plugins, discord_override, wave_settings, mini_prefs) => {
                self.discord_override = discord_override;
                // Открытый мини-плеер уже стоит где стоит: берём только раскладку
                // и положение на следующее открытие.
                if self.mini.is_none() {
                    self.mini_prefs = mini_prefs;
                }
                if let Screen::Main(library) = &mut self.screen {
                    library.set_wave_settings(wave_settings.clone());
                }
                self.wave_settings = wave_settings;
                let rescaled = prefs.scale != self.prefs.scale;
                // Вид меню при запуске — без анимации.
                self.sidebar = anim::settled(prefs.sidebar_collapsed, anim::PANEL);
                self.apply_prefs(prefs);
                let plugins = self.apply_plugins(plugins);
                if rescaled {
                    return Task::batch([plugins, self.refresh_size()]);
                }
                return plugins;
            }
            Message::SetPlugins(plugins) => {
                // «Не нравится» — запись в аккаунт: сначала согласие.
                if plugins.dislike && !self.plugins.dislike {
                    self.open_dialog(Dialog::SloplessConsent);
                    return Task::none();
                }
                let task = self.apply_plugins(plugins);
                return Task::batch([task, self.save_prefs()]);
            }
            Message::AllowDislike => {
                self.close_dialog();
                let plugins = PluginSettings {
                    dislike: true,
                    ..self.plugins.clone()
                };
                let task = self.apply_plugins(plugins);
                return Task::batch([task, self.save_prefs()]);
            }
            Message::ToggleSidebar => {
                let prefs = Prefs {
                    sidebar_collapsed: !self.prefs.sidebar_collapsed,
                    ..self.prefs
                };
                return self.change_prefs(prefs);
            }
            Message::SetAccent(accent) => {
                let prefs = Prefs {
                    accent,
                    ..self.prefs
                };
                return self.change_prefs(prefs);
            }
            Message::Zoom(zoom) => {
                let scale = match zoom {
                    Zoom::In => self.prefs.scale.saturating_add(config::SCALE_STEP),
                    Zoom::Out => self.prefs.scale.saturating_sub(config::SCALE_STEP),
                    Zoom::Reset => config::SCALE_DEFAULT,
                }
                .clamp(config::SCALE_MIN, config::SCALE_MAX);
                if scale == self.prefs.scale {
                    return Task::none();
                }
                let prefs = Prefs {
                    scale,
                    ..self.prefs
                };
                return Task::batch([self.change_prefs(prefs), self.refresh_size()]);
            }
            Message::SetAudioQuality(quality) => {
                let prefs = Prefs {
                    quality,
                    ..self.prefs
                };
                return self.change_prefs(prefs);
            }
            Message::SetDiscord(settings) => {
                let prefs = Prefs {
                    discord: settings,
                    ..self.prefs
                };
                return self.change_prefs(prefs);
            }
            Message::SetLyricsLrclib(lyrics_lrclib) => {
                let prefs = Prefs {
                    lyrics_lrclib,
                    ..self.prefs
                };
                let save = self.change_prefs(prefs);
                // Открытая панель текста перезапрашивает его сразу.
                let sync = match &mut self.screen {
                    Screen::Main(library) => library.lyrics_sync().map(Message::Library),
                    _ => Task::none(),
                };
                return Task::batch([save, sync]);
            }
            Message::SetNormalize(normalize) => {
                let prefs = Prefs {
                    normalize,
                    ..self.prefs
                };
                return self.change_prefs(prefs);
            }
            Message::ToggleVisualizer => {
                let prefs = Prefs {
                    visualizer: !self.prefs.visualizer,
                    ..self.prefs
                };
                return self.change_prefs(prefs);
            }
            Message::PrefsSaved(result) => {
                self.saving = false;
                if let Err(failure) = result {
                    self.save_again = false;
                    self.show_toast(format!("Не удалось сохранить настройки: {}", failure.text));
                } else if std::mem::take(&mut self.save_again) {
                    return self.save_prefs();
                }
            }

            Message::OpenDialog(dialog) => self.open_dialog(dialog),
            Message::CloseDialog => self.close_dialog(),
            Message::CoverLoaded(key, bytes) => {
                let http = self.http.as_ref().ok();
                return self.covers.finished(key, bytes, http);
            }
            Message::BigCoverLoaded(uri, big) => {
                self.covers.big_finished(uri, big);
                // Среднее по обложке может поменять цвет волны.
                if let Screen::Main(library) = &mut self.screen {
                    library.sync_wave_tint(&self.covers, Instant::now());
                }
            }
            Message::Library(message) => {
                if let Screen::Main(library) = &mut self.screen {
                    let http = self.http.as_ref().ok();
                    let (task, effect) = library.update(message, http);
                    let task = task.map(Message::Library);
                    library.sync_wave_tint(&self.covers, self.now);
                    let covers = Task::batch([
                        self.covers.show(library.visible_covers(), http),
                        self.covers.want_big(library.fullscreen_cover(), http),
                        self.covers.want_big(library.wave_cover(), http),
                        self.covers
                            .want_big(self.mini.and(library.mini_cover()), http),
                    ]);
                    let effect = match effect {
                        Some(library::Effect::Relogin) => self.start_login(Some(
                            "Вход больше не действует. Войдите заново.".to_owned(),
                        )),
                        Some(library::Effect::Toast(text)) => {
                            self.show_toast(text);
                            Task::none()
                        }
                        Some(library::Effect::OpenMini) => self.open_mini(),
                        Some(library::Effect::WaveSettings(settings)) => {
                            self.wave_settings = settings;
                            self.save_prefs()
                        }
                        None => Task::none(),
                    };
                    return Task::batch([task, covers, effect]);
                }
            }
        }
        Task::none()
    }

    /// Новые настройки плагинов: окну — тоже.
    fn apply_plugins(&mut self, plugins: PluginSettings) -> Task<Message> {
        self.plugins = plugins;
        match &mut self.screen {
            Screen::Main(library) => {
                let task = library.plugins_changed(&self.plugins).map(Message::Library);
                library.sync_wave_tint(&self.covers, Instant::now());
                let cover = self
                    .covers
                    .want_big(library.wave_cover(), self.http.as_ref().ok());
                Task::batch([task, cover])
            }
            _ => Task::none(),
        }
    }

    fn apply_prefs(&mut self, prefs: Prefs) {
        self.sidebar.go_mut(prefs.sidebar_collapsed, Instant::now());
        if prefs.accent != self.prefs.accent {
            theme::set_accent(prefs.accent);
            self.theme = theme::theme();
        }
        if let Screen::Main(library) = &mut self.screen {
            library.set_sound(prefs.quality, prefs.normalize);
            library.set_lyrics_lrclib(prefs.lyrics_lrclib);
        }
        self.prefs = prefs;
    }

    /// Новые настройки: применить и записать в `config.toml`.
    fn change_prefs(&mut self, prefs: Prefs) -> Task<Message> {
        self.apply_prefs(prefs);
        self.save_prefs()
    }

    /// Пишет настройки. Пока идёт запись, новая не начинается: иначе старая
    /// могла бы закончиться позже и затереть новые значения.
    fn save_prefs(&mut self) -> Task<Message> {
        // С фальшивым токеном окно в `config.toml` не пишет.
        if storage::test_hooks_active() {
            return Task::none();
        }
        if self.saving {
            self.save_again = true;
            return Task::none();
        }
        self.saving = true;
        Task::perform(
            save_prefs(
                self.prefs,
                self.plugins.clone(),
                self.wave_settings.clone(),
                self.mini_prefs,
            ),
            Message::PrefsSaved,
        )
    }

    /// Размер окна в единицах раскладки меняется вместе с масштабом.
    fn refresh_size(&self) -> Task<Message> {
        self.with_window(|id| window::size(id).map(Message::WindowResized))
    }

    fn with_window(&self, task: impl FnOnce(window::Id) -> Task<Message>) -> Task<Message> {
        self.window.map_or_else(Task::none, task)
    }

    /// Открывает мини-плеер из нижнего плеера. Главное окно сворачивается
    /// (мини-плеер занимает его место), его возвращает «развернуть» или
    /// крестик мини-плеера.
    fn open_mini(&mut self) -> Task<Message> {
        let on_main = matches!(self.screen, Screen::Main(_));
        if !mini::should_open(on_main, self.mini.is_some()) {
            return Task::none();
        }
        let (id, open) = window::open(mini::settings(&self.mini_prefs, self.mini_pinned));
        self.mini = Some(id);
        let http = self.http.as_ref().ok();
        let cover = match &self.screen {
            Screen::Main(library) => self.covers.want_big(library.mini_cover(), http),
            _ => Task::none(),
        };
        Task::batch([
            open.map(|_| Message::Noop),
            self.with_window(|main| window::minimize(main, true)),
            cover,
        ])
    }

    /// Закрывает мини-плеер и возвращает главное окно (`focus` — на передний
    /// план). Положение окна запоминается.
    fn close_mini(&mut self, focus: bool) -> Task<Message> {
        let Some(id) = self.mini.take() else {
            return Task::none();
        };
        let restore = self.with_window(|main| {
            if focus {
                Task::batch([window::minimize(main, false), window::gain_focus(main)])
            } else {
                window::minimize(main, false)
            }
        });
        Task::batch([window::close(id), restore, self.save_prefs()])
    }

    /// Мини-плеер сам закрывается, когда главное окно ушло с главного экрана
    /// (выход из аккаунта).
    fn sync_mini(&mut self) -> Task<Message> {
        let on_main = matches!(self.screen, Screen::Main(_));
        if self.closing || !mini::should_close(on_main, self.mini.is_some()) {
            return Task::none();
        }
        self.close_mini(true)
    }

    /// Закрывает окно. Несохранённый токен сначала отзывается, но не дольше
    /// [`REVOKE_ON_CLOSE`]: без сети окно всё равно закроется.
    fn close(&mut self) -> Task<Message> {
        // Статус в Discord очищается до закрытия окна и больше не появляется.
        self.discord = None;
        // Мини-плеер закрывается вместе с главным окном: положение
        // запоминается до выхода.
        if self.mini.take().is_some() {
            // Выходим только после записи: иначе выход оборвёт её.
            return self.save_prefs().chain(Task::done(Message::CloseWindow));
        }
        self.closing = true;
        let (Some(token), Ok(http)) = (self.unsaved.take(), self.http()) else {
            return iced::exit();
        };
        Task::perform(
            async move {
                let _ = tokio::time::timeout(REVOKE_ON_CLOSE, revoke(http, token)).await;
            },
            |()| Message::CloseWindow,
        )
    }

    /// Esc: закрыть диалог, иначе меню трека, иначе полноэкранный плеер,
    /// иначе сбросить поиск.
    fn escape(&mut self) -> Task<Message> {
        if self.dialog.is_some() {
            self.close_dialog();
        } else if let Screen::Main(library) = &mut self.screen
            && !library.close_overlay()
            && !library.close_fullscreen()
            && !library.close_lyrics()
            && let Some(task) = library.clear_search()
        {
            return task.map(Message::Library);
        }
        Task::none()
    }

    fn show_toast(&mut self, text: String) {
        self.toast = Some((text, Instant::now() + TOAST_FOR));
        self.toast_anim.go_mut(true, Instant::now());
    }

    fn key(&mut self, event: keyboard::Event) -> Task<Message> {
        let keyboard::Event::KeyPressed {
            key,
            physical_key,
            modifiers,
            ..
        } = event
        else {
            return Task::none();
        };
        // Ctrl + F — по физической клавише, чтобы работало в любой
        // раскладке (в русской это «а»).
        if modifiers.command()
            && physical_key == keyboard::key::Physical::Code(keyboard::key::Code::KeyF)
        {
            let http = self.http.as_ref().ok();
            return match &mut self.screen {
                Screen::Main(library) => library.focus_search(http).map(Message::Library),
                _ => Task::none(),
            };
        }
        match key.as_ref() {
            keyboard::Key::Character(ch) if modifiers.command() => match ch {
                "=" | "+" => self.update(Message::Zoom(Zoom::In)),
                "-" | "_" => self.update(Message::Zoom(Zoom::Out)),
                "0" => self.update(Message::Zoom(Zoom::Reset)),
                _ => Task::none(),
            },
            _ => Task::none(),
        }
    }

    /// Раз в секунду: отсчёты, повтор без сети, скрытие подсказок.
    fn tick(&mut self, now: Instant) -> Task<Message> {
        if self.toast.as_ref().is_some_and(|(_, until)| now >= *until) {
            // Сообщение тает, текст убирается после.
            self.toast_anim.go_mut(false, now);
        }
        match &mut self.screen {
            Screen::Login(login) => {
                if login.copied_until.is_some_and(|until| now >= until) {
                    login.copied_until = None;
                }
                login.expire_if_due(now);
            }
            Screen::Problem(problem) if problem.retry_at.is_some_and(|at| now >= at) => {
                return self.retry();
            }
            _ => {}
        }
        Task::none()
    }

    fn problem(&mut self, failure: &Failure, title: &str, retry: Retry) {
        let retry_at =
            (failure.kind == FailureKind::Offline).then(|| Instant::now() + OFFLINE_RETRY);
        self.screen = Screen::Problem(Problem {
            kind: failure.kind,
            title: title.to_owned(),
            text: sentence(&failure.text),
            retry,
            retry_at,
            show_help: false,
        });
    }

    fn retry(&mut self) -> Task<Message> {
        let Screen::Problem(problem) = &self.screen else {
            return Task::none();
        };
        match problem.retry.clone() {
            Retry::Start => {
                self.screen = Screen::Starting;
                Task::perform(load_token(), Message::TokenLoaded)
            }
            Retry::Login => self.start_login(None),
            Retry::Save(token, status) => self.save_token(token, status),
            Retry::Check(token) => {
                self.screen = Screen::Checking;
                self.check_status(token)
            }
        }
    }

    fn token_loaded(&mut self, result: Result<Option<Arc<StoredToken>>, Failure>) -> Task<Message> {
        match result {
            Ok(Some(token)) => {
                self.screen = Screen::Checking;
                self.check_status(token)
            }
            Ok(None) => self.start_login(None),
            Err(failure) => {
                self.problem(&failure, "Не удалось прочитать вход", Retry::Start);
                Task::none()
            }
        }
    }

    fn start_login(&mut self, notice: Option<String>) -> Task<Message> {
        self.close_dialog();
        self.screen = Screen::Login(gate::Login::new(notice));
        match self.http() {
            Ok(http) => Task::perform(prepare_login(http), Message::CodeReceived),
            Err(failure) => Task::done(Message::CodeReceived(Err(failure))),
        }
    }

    fn code_received(
        &mut self,
        result: Result<(Arc<LoginSetup>, Arc<DeviceCode>), Failure>,
    ) -> Task<Message> {
        let http = self.http();
        let Screen::Login(login) = &mut self.screen else {
            return Task::none();
        };
        let (setup, code) = match result {
            Ok(received) => received,
            Err(failure) => {
                let title = match failure.kind {
                    FailureKind::Keyring => "Не удалось сохранить вход",
                    FailureKind::Offline => "Нет подключения к интернету",
                    _ => "Не удалось начать вход",
                };
                self.problem(&failure, title, Retry::Login);
                return Task::none();
            }
        };
        let Ok(http) = http else {
            return Task::none();
        };
        let app = setup.app.clone();
        let wait_code = code.clone();
        let (task, handle) = Task::perform(
            async move {
                auth::wait_for_token(&http, &app, &wait_code)
                    .await
                    .map(|token| Arc::new(token.into_stored()))
                    .map_err(Failure::from)
            },
            Message::TokenReceived,
        )
        .abortable();
        login.waiting(code, handle.abort_on_drop());
        self.login_setup = Some(setup);
        task
    }

    fn token_received(&mut self, result: Result<Arc<StoredToken>, Failure>) -> Task<Message> {
        let Screen::Login(login) = &mut self.screen else {
            return Task::none();
        };
        match result {
            Ok(token) => {
                login.finishing();
                self.unsaved = Some(token.clone());
                match self.http() {
                    Ok(http) => {
                        let check = token.clone();
                        Task::perform(
                            async move {
                                api::fetch_status(&http, &check.access_token)
                                    .await
                                    .map_err(Failure::from)
                            },
                            move |result| Message::LoginChecked(token.clone(), result),
                        )
                    }
                    Err(failure) => Task::done(Message::LoginChecked(token, Err(failure))),
                }
            }
            Err(failure) if failure.kind == FailureKind::CodeExpired => {
                login.expire();
                Task::none()
            }
            Err(failure) if failure.kind == FailureKind::Offline => {
                self.problem(&failure, "Нет подключения к интернету", Retry::Login);
                Task::none()
            }
            Err(failure) => {
                login.fail(sentence(&failure.text));
                Task::none()
            }
        }
    }

    fn login_checked(
        &mut self,
        token: Arc<StoredToken>,
        result: Result<AccountStatus, Failure>,
    ) -> Task<Message> {
        let Screen::Login(login) = &mut self.screen else {
            return Task::none();
        };
        match result {
            Err(failure) if failure.kind == FailureKind::Unauthorized => {
                login.fail(
                    "Яндекс выдал токен, но сервис музыки его не принимает. Токен не сохранён."
                        .to_owned(),
                );
                self.unsaved = None;
                self.revoke_in_background(token)
            }
            // Подписку проверим ещё раз после сохранения.
            result => self.save_token(token, result.ok()),
        }
    }

    fn save_token(
        &mut self,
        token: Arc<StoredToken>,
        status: Option<AccountStatus>,
    ) -> Task<Message> {
        let Some(setup) = self.login_setup.clone() else {
            return self.start_login(None);
        };
        if let Screen::Login(login) = &mut self.screen {
            login.finishing();
        } else {
            self.screen = Screen::Checking;
        }
        let stored = token.clone();
        Task::perform(
            in_storage(move || setup.store.save(&stored)),
            move |result| Message::TokenSaved(token.clone(), status.clone(), result),
        )
    }

    fn token_saved(
        &mut self,
        token: Arc<StoredToken>,
        status: Option<AccountStatus>,
        result: Result<(), Failure>,
    ) -> Task<Message> {
        if let Err(failure) = result {
            let title = if failure.kind == FailureKind::Keyring {
                "Не удалось сохранить вход"
            } else {
                "Не удалось сохранить токен"
            };
            self.problem(&failure, title, Retry::Save(token, status));
            return Task::none();
        }
        self.login_setup = None;
        self.unsaved = None;
        match status {
            Some(status) => self.route(token, status),
            None => {
                self.screen = Screen::Checking;
                self.check_status(token)
            }
        }
    }

    fn check_status(&self, token: Arc<StoredToken>) -> Task<Message> {
        match self.http() {
            Ok(http) => {
                let check = token.clone();
                Task::perform(
                    async move {
                        api::fetch_status(&http, &check.access_token)
                            .await
                            .map_err(Failure::from)
                    },
                    move |result| Message::StatusChecked(token.clone(), result),
                )
            }
            Err(failure) => Task::done(Message::StatusChecked(token, Err(failure))),
        }
    }

    fn status_checked(
        &mut self,
        token: Arc<StoredToken>,
        result: Result<AccountStatus, Failure>,
    ) -> Task<Message> {
        let on_paywall = match &self.screen {
            Screen::Checking => false,
            Screen::Paywall(paywall) if paywall.checking => true,
            _ => return Task::none(),
        };
        match result {
            Ok(status) => self.route(token, status),
            Err(failure) if failure.kind == FailureKind::Unauthorized => {
                self.start_login(Some("Вход больше не действует. Войдите заново.".to_owned()))
            }
            Err(failure) if on_paywall => {
                if let Screen::Paywall(paywall) = &mut self.screen {
                    paywall.checking = false;
                    paywall.error =
                        Some(format!("Проверить не удалось. {}", sentence(&failure.text)));
                }
                Task::none()
            }
            Err(failure) => {
                // Ошибка запроса — «проверить не удалось», а не «подписка есть».
                let title = if failure.kind == FailureKind::Offline {
                    "Нет подключения к интернету"
                } else {
                    "Не удалось проверить подписку"
                };
                self.problem(&failure, title, Retry::Check(token));
                Task::none()
            }
        }
    }

    /// Куда идти после проверки подписки. Главное окно — только с пропуском.
    fn route(&mut self, token: Arc<StoredToken>, status: AccountStatus) -> Task<Message> {
        match status.playback_permit() {
            Some(permit) => self.open_main(token, status, permit),
            None => {
                self.screen = Screen::Paywall(gate::Paywall::new(token, status));
                Task::none()
            }
        }
    }

    fn open_main(
        &mut self,
        token: Arc<StoredToken>,
        status: AccountStatus,
        permit: PlaybackPermit,
    ) -> Task<Message> {
        let (library, task) = library::Library::new(
            token,
            status,
            permit,
            self.size.height,
            self.http.as_ref().ok(),
            &self.plugins,
        );
        let mut library = library;
        library.set_sound(self.prefs.quality, self.prefs.normalize);
        library.set_lyrics_lrclib(self.prefs.lyrics_lrclib);
        library.set_wave_settings(self.wave_settings.clone());
        self.screen = Screen::Main(Box::new(library));
        task.map(Message::Library)
    }

    fn logout(&mut self) -> Task<Message> {
        self.close_dialog();
        let token = match &self.screen {
            Screen::Paywall(paywall) => paywall.token.clone(),
            Screen::Main(library) => library.token(),
            _ => return Task::none(),
        };
        // Сначала удаляем токен с компьютера, отзываем после удаления.
        Task::perform(
            in_storage(|| TokenStore::open()?.delete().map(|_| ())),
            move |result| Message::LoggedOut(token.clone(), result),
        )
    }

    /// Отзывает токен на сервере. Результат не важен: локально токена нет.
    fn revoke_in_background(&self, token: Arc<StoredToken>) -> Task<Message> {
        let Ok(http) = self.http() else {
            return Task::none();
        };
        Task::perform(revoke(http, token), |()| Message::Noop)
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subscriptions = vec![
            window::resize_events().map(|(id, size)| Message::AnyResized(id, size)),
            keyboard::listen().map(Message::Key),
            // Поле поиска забирает Esc себе и только снимает фокус, а
            // `keyboard::listen` видит лишь незабранные клавиши.
            iced::event::listen_with(|event, _, _| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::Escape),
                _ => None,
            }),
            // Кнопка мыши «Назад» — на предыдущую страницу.
            iced::event::listen_with(|event, _, _| match event {
                iced::Event::Mouse(iced::mouse::Event::ButtonPressed(
                    iced::mouse::Button::Back,
                )) => Some(Message::Library(library::Message::Back)),
                _ => None,
            }),
            window::close_requests().map(Message::CloseRequested),
        ];
        // Положение мини-плеера запоминаем, только пока оно открыто.
        if self.mini.is_some() {
            subscriptions.push(iced::event::listen_with(|event, _, id| match event {
                iced::Event::Window(window::Event::Moved(point)) => {
                    Some(Message::MiniMoved(id, point))
                }
                _ => None,
            }));
        }
        // Пока открыт мини-плеер, секундный тик обновляет его прогресс.
        let ticking = self.mini.is_some()
            || self.toast_anim.value()
            || match &self.screen {
                Screen::Login(_) => true,
                Screen::Problem(problem) => problem.retry_at.is_some(),
                _ => false,
            };
        if ticking {
            subscriptions.push(iced::time::every(Duration::from_secs(1)).map(Message::Tick));
        }
        // Кадры гонит невидимый виджет `heartbeat` из самой перерисовки (в
        // `decorate`): так анимации и спектр идут и пока окно тянут за
        // заголовок или край, когда таймеры и подписки стоят.
        if let Screen::Main(library) = &self.screen {
            subscriptions.push(library.subscription());
        }
        if let Some(controls) = &self.media {
            subscriptions.push(
                Subscription::run_with(controls.events(), |events: &media_controls::Events| {
                    events.stream()
                })
                .map(Message::Media),
            );
        }
        Subscription::batch(subscriptions)
    }

    /// Нужны ли кадры: окно не свёрнуто, и что-то движется.
    fn wants_frames(&self) -> bool {
        !self.minimized && (self.transitioning() || self.animating())
    }

    /// Идёт ли то, чему нужны кадры раз в 30 мс: спиннеры, скелетон, спектр.
    fn animating(&self) -> bool {
        match &self.screen {
            Screen::Starting | Screen::Checking => true,
            Screen::Login(login) => login.animating(),
            Screen::Paywall(paywall) => paywall.checking,
            Screen::Main(library) => {
                library.animating() || library.visualizing(self.prefs.visualizer)
            }
            Screen::Problem(_) => false,
        }
    }

    fn view(&self, window: window::Id) -> Element<'_, Message> {
        if self.mini == Some(window) {
            return self.mini_view();
        }
        let turn = self.turn();
        let moment = self.moment();
        let body: Element<'_, Message> = match &self.screen {
            Screen::Starting => gate::loading("Открываем…", turn),
            Screen::Checking => gate::loading("Проверяем подписку…", turn),
            Screen::Problem(problem) => gate::problem(problem, self.now),
            Screen::Login(login) => gate::login(login, self.now, turn),
            Screen::Paywall(paywall) => gate::paywall(paywall, turn),
            // Полноэкранный плеер закрыл окно целиком: под ним строить нечего.
            Screen::Main(library) if library.fullscreen_shown(self.now) >= 1.0 => {
                return self.decorate(iced::widget::Space::new().into());
            }
            Screen::Main(library) => {
                return self.decorate(library.view(
                    &self.covers,
                    &self.prefs,
                    moment,
                    self.compact(),
                    self.size.width,
                    turn,
                ));
            }
        };
        self.decorate(appear(gate::frame(body), moment.page))
    }

    /// Окно мини-плеера. Вне главного экрана его нет (правило 2), поэтому
    /// пустое: окно тут же закрывается.
    fn mini_view(&self) -> Element<'_, Message> {
        let Screen::Main(library) = &self.screen else {
            return iced::widget::Space::new().into();
        };
        let average = library
            .mini_cover()
            .and_then(|uri| self.covers.big(Some(uri), self.now))
            .and_then(|(big, _)| big.average);
        library.mini_view(
            &self.covers,
            self.mini_prefs.compact,
            self.mini_pinned,
            average,
            self.now,
        )
    }

    /// Заголовок окна, края для изменения размера, диалог и сообщение.
    fn decorate<'a>(&'a self, content: Element<'a, Message>) -> Element<'a, Message> {
        let with_brand = !matches!(self.screen, Screen::Main(_));
        let window = container(column![gate::title_bar(with_brand), content])
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(theme::BG.into()),
                ..container::Style::default()
            });
        let mut layers = stack![window].width(Length::Fill).height(Length::Fill);
        if let Screen::Main(library) = &self.screen {
            let sheet = library.fullscreen(
                &self.covers,
                &self.prefs,
                self.now,
                self.compact(),
                self.size,
                self.turn(),
                gate::title_bar(false),
            );
            if let Some(sheet) = sheet {
                layers = layers.push(sheet);
            }
            if let Some(panel) = library.volume_panel(self.now) {
                layers = layers.push(panel);
            }
            if let Some(panel) = library.sound_panel(&self.prefs, self.now) {
                layers = layers.push(panel);
            }
            let scale = (self.prefs.scale as f32 / 100.0).max(0.1);
            let window_size = Size::new(self.size.width / scale, self.size.height / scale);
            if let Some(menu) = library.wave_menu(self.now, window_size, self.moment().sidebar) {
                layers = layers.push(menu);
            }
            if let Some(overlay) = library.overlay(self.size) {
                layers = layers.push(overlay);
            }
        }
        if let Some((text, _)) = &self.toast {
            // В главном окне — по центру плеера, а не всего окна.
            // В полноэкранном плеере плеер во всю ширину окна.
            let insets = match &self.screen {
                Screen::Main(library) if library.fullscreen_shown(self.now) < 0.5 => {
                    library::player_insets(self.moment().sidebar)
                }
                _ => (0.0, 0.0),
            };
            let shown = anim::progress(&self.toast_anim, self.now);
            layers = layers.push(gate::toast(text, insets, shown));
        }
        if let Some(dialog) = self.dialog_shown {
            let shown = anim::progress(&self.dialog_anim, self.now);
            layers = layers.push(gate::dialog(dialog, shown, self.dialog.is_some()));
        }
        // Переходам — кадры с частотой экрана, спиннерам и спектру хватает 30
        // в секунду. Когда ничего не движется, кадров нет вовсе.
        let transitioning = self.transitioning();
        // Свёрнутому окну кадры не нужны (и пауза всё равно их не пропустит).
        let layers = layers.push(heartbeat::heartbeat(
            self.wants_frames(),
            transitioning,
            Message::Frame,
        ));
        let layers = layers.push(resize_edges());
        // Где нажали мышь — для меню трека у курсора.
        let root: Element<'a, Message> = if matches!(self.screen, Screen::Main(_)) {
            pointer::pointer(layers, |at| {
                Message::Library(library::Message::Plugin(library::PluginMsg::Pointer(at)))
            })
        } else {
            layers.into()
        };
        // В свёрнутом окне (клиентская область не 0×0) перерисовку не просим.
        pause::pause(root, self.minimized)
    }
}

/// Что нужно видам для анимаций в этом кадре.
#[derive(Debug, Clone, Copy)]
pub struct Moment {
    pub now: Instant,
    /// Появление раздела: 0 — только что сменился, 1 — на месте.
    pub page: f32,
    /// Ширина меню сейчас (меню сворачивается плавно).
    pub sidebar: f32,
}

/// Появление экрана: снизу на несколько пикселей и из черноты.
pub fn appear<'a>(content: Element<'a, Message>, shown: f32) -> Element<'a, Message> {
    if shown >= 1.0 {
        return content;
    }
    let hidden = 1.0 - shown;
    anim::motion(content)
        .offset(0.0, 12.0 * hidden)
        .veil(theme::alpha(theme::BG, hidden), 0.0)
        .into()
}

/// Невидимые полосы по краям окна: окно без рамки само размер не меняет.
fn resize_edges<'a>() -> Element<'a, Message> {
    const EDGE: f32 = 5.0;
    let edge = |width: Length, height: Length, direction: Direction, cursor| {
        mouse_area(
            container(iced::widget::Space::new())
                .width(width)
                .height(height),
        )
        .interaction(cursor)
        .on_press(Message::ResizeWindow(direction))
    };
    use mouse::Interaction::{
        ResizingDiagonallyDown as Down, ResizingDiagonallyUp as Up,
        ResizingHorizontally as Horizontal, ResizingVertically as Vertical,
    };
    let fixed = Length::Fixed(EDGE);
    let top = row![
        edge(fixed, fixed, Direction::NorthWest, Down),
        edge(Length::Fill, fixed, Direction::North, Vertical),
        edge(fixed, fixed, Direction::NorthEast, Up),
    ];
    let middle = row![
        edge(fixed, Length::Fill, Direction::West, Horizontal),
        iced::widget::Space::new()
            .width(Length::Fill)
            .height(Length::Fill),
        edge(fixed, Length::Fill, Direction::East, Horizontal),
    ]
    .height(Length::Fill);
    let bottom = row![
        edge(fixed, fixed, Direction::SouthWest, Up),
        edge(Length::Fill, fixed, Direction::South, Vertical),
        edge(fixed, fixed, Direction::SouthEast, Down),
    ];
    column![top, middle, bottom].into()
}

/// Блокирующая работа с хранилищем в фоне, ошибка — для окна.
async fn in_storage<T, F>(work: F) -> Result<T, Failure>
where
    F: FnOnce() -> Result<T, StorageError> + Send + 'static,
    T: Send + 'static,
{
    storage::blocking(work).await.map_err(Failure::from)
}

/// Читает токен. Повреждённая запись — как будто входа не было: вход её
/// перезапишет.
async fn load_token() -> Result<Option<Arc<StoredToken>>, Failure> {
    storage::blocking(|| match TokenStore::open()?.load() {
        Ok(token) => Ok(token.map(Arc::new)),
        Err(StorageError::BadStoredToken) => Ok(None),
        Err(err) => Err(err),
    })
    .await
    .map_err(Failure::from)
}

/// Отзывает токен на сервере. Ошибка не важна: токена на компьютере нет.
async fn revoke(http: reqwest::Client, token: Arc<StoredToken>) {
    let config = load_config().await.unwrap_or_default();
    let app = OAuthApp::from_config(&config);
    let _ = auth::revoke(&http, &app, &token.access_token).await;
}

/// Настройки плагинов из `config.toml`. С отладочным хуком
/// `PLASTINKA_TEST_PLUGINS` включены все плагины (и ничего не пишется).
fn plugin_settings(config: &Config) -> PluginSettings {
    #[cfg(debug_assertions)]
    if crate::plugins::test_hook().is_some() {
        return PluginSettings::all_on();
    }
    PluginSettings::from_config(&config.plugins)
}

/// Запоминает вид меню, внешний вид и плагины. Остальные настройки не трогает.
async fn save_prefs(
    prefs: Prefs,
    plugins: PluginSettings,
    wave_settings: api::WaveSettings,
    mini_prefs: mini::MiniPrefs,
) -> Result<(), Failure> {
    let path = AppDirs::discover()?.config_file();
    storage::blocking(move || {
        let mut config = Config::load(&path)?;
        prefs.write(&mut config);
        plugins.write(&mut config.plugins);
        wave_settings.write(&mut config.wave_settings);
        mini_prefs.write(&mut config.mini_player);
        config.save(&path)
    })
    .await
    .map_err(Failure::from)
}

async fn load_config() -> Result<Config, StorageError> {
    let path = AppDirs::discover()?.config_file();
    storage::blocking(move || Config::load(&path)).await
}

/// Готовит вход и запрашивает код устройства. Хранилище проверяется до
/// запроса кода: без него незачем просить пользователя вводить код.
async fn prepare_login(
    http: reqwest::Client,
) -> Result<(Arc<LoginSetup>, Arc<DeviceCode>), Failure> {
    let config_path = AppDirs::discover()?.config_file();
    let (config, device_id) = storage::blocking(move || {
        let mut config = Config::load(&config_path)?;
        let (device_id, created) = config.ensure_device_id();
        if created {
            config.save(&config_path)?;
        }
        Ok((config, device_id))
    })
    .await?;
    let store = storage::blocking(|| {
        let store = TokenStore::open()?;
        match store.load() {
            Ok(_) | Err(StorageError::BadStoredToken) => Ok(store),
            Err(err) => Err(err),
        }
    })
    .await?;
    let app = OAuthApp::from_config(&config);
    let code = auth::request_device_code(&http, &app, &device_id).await?;
    Ok((Arc::new(LoginSetup { app, store }), Arc::new(code)))
}

/// Системный дескриптор окна для таймера перетаскивания (только Windows).
fn native_handle(window: &dyn window::Window) -> Option<isize> {
    use window::raw_window_handle::RawWindowHandle;
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_icon_decodes() {
        assert!(app_icon().is_some());
    }

    /// Выбор в панели звука доходит до `config.toml` и обратно; неизвестное
    /// значение даёт «Оптимальное».
    #[test]
    fn sound_choice_goes_to_config_and_back() {
        for quality in [
            AudioQuality::Lossless,
            AudioQuality::Normal,
            AudioQuality::Low,
        ] {
            // `discord_ready` не пишется в конфиг, а вычисляется по ID.
            let prefs = Prefs {
                quality,
                normalize: true,
                discord_ready: Prefs::from_config(&Config::default()).discord_ready,
                ..Prefs::default()
            };
            let mut config = Config::default();
            prefs.write(&mut config);
            assert_eq!(config.audio_quality.as_deref(), Some(quality.key()));
            assert_eq!(config.normalize, Some(true));
            assert_eq!(Prefs::from_config(&config), prefs);
        }
        let config = Config {
            audio_quality: Some("ultra".to_owned()),
            ..Config::default()
        };
        assert_eq!(Prefs::from_config(&config).quality, AudioQuality::Normal);
        assert!(!Prefs::from_config(&Config::default()).normalize);
    }
}
