//! Окно приложения на iced: вход, «Нужна подписка», главное окно с боковым
//! меню, списком «Мне нравится» и панелью плеера, окно «О программе».
//!
//! Главное окно и плеер в нём создаются только с пропуском
//! [`PlaybackPermit`], то есть после явного подтверждения Плюса.

mod anim;
mod corners;
mod covers;
mod dark;
mod drag_pulse;
mod filter;
#[cfg(debug_assertions)]
mod fps;
mod gate;
mod heartbeat;
mod history;
mod history_store;
mod icons;
mod library;
mod lyrics_view;
mod marquee;
mod mini;
mod palette;
mod pause;
mod pointer;
mod queue;
mod render;
mod scroll;
mod settings;
mod theme;
mod updates;
mod visualizer;
mod widgets;

use std::io::Write as _;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use iced::animation::Animation;
use iced::widget::{column, container, row, stack};
use iced::window;
use iced::{Element, Length, Size, Subscription, Task, keyboard};

use crate::APP_NAME;
use crate::api::{self, AccountStatus, ApiError, AudioQuality, PlaybackPermit};
use crate::auth::{self, AuthError, DeviceCode, OAuthApp};
use crate::autostart;
use crate::discord;
use crate::instance;
use crate::media_controls;
use crate::net::{self, NetError};
use crate::player::eq::EqState;
use crate::plugins::PluginSettings;
use crate::storage::config::{self, Config, SyncConfig};
use crate::storage::token::{StoredToken, TokenStore};
use crate::storage::{self, AppDirs, StorageError};
use crate::sync::service::{Service, ServiceConfig};
use crate::sync::store::{SecretStore, default_device_name};
use crate::texts::sentence;
use crate::tray;
use crate::update;

/// Как запущена программа (флаги командной строки).
#[derive(Debug, Clone, Copy, Default)]
pub struct Launch {
    /// `--tray`: окно не открывается, пока его не попросят (значок в трее).
    pub tray: bool,
}

/// Параметры запуска: `App::boot` — обычная функция без аргументов.
static LAUNCH: OnceLock<Launch> = OnceLock::new();

fn launch() -> Launch {
    LAUNCH.get().copied().unwrap_or_default()
}

/// Запускает окно. Блокирует поток до закрытия окна.
pub fn run(launch: Launch) -> iced::Result {
    let _ = LAUNCH.set(launch);
    // Рендер выбирается до запуска iced: см. `render`. Не вышло прочитать
    // настройку — берём значение по умолчанию (аппаратное ускорение).
    let gpu = AppDirs::discover()
        .ok()
        .and_then(|dirs| Config::load(&dirs.config_file()).ok())
        .and_then(|config| config.hardware_acceleration)
        .unwrap_or(true);
    render::choose(gpu);
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
    let settings = window::Settings {
        size: Size::new(1280.0, 800.0),
        min_size: Some(Size::new(960.0, 600.0)),
        // Свой заголовок окна: системная рамка не нужна. В macOS наоборот:
        // системная рамка нужна ради кнопок «светофора» (см. `main_platform_specific`).
        decorations: cfg!(target_os = "macos"),
        // Углы в Linux скругляем сами: за ними должно быть видно рабочий стол.
        transparent: corners::ENABLED && !render::software(),
        // Закрытие обрабатываем сами: несохранённый токен надо отозвать.
        exit_on_close_request: false,
        icon: app_icon(),
        platform_specific: main_platform_specific(),
        ..window::Settings::default()
    };
    #[cfg(debug_assertions)]
    let settings = test_window(settings);
    settings
}

/// Отладочный хук `PLASTINKA_TEST_WINDOW=x,y,ширина,высота` (логические
/// единицы, только с тестовыми хуками): главное окно сразу открывается в этом
/// месте и этого размера. Для замеров `bench/tools`: окно за краем экрана не
/// мелькает и не мешает пользователю.
#[cfg(debug_assertions)]
fn test_window(mut settings: window::Settings) -> window::Settings {
    if !storage::test_hooks_active() {
        return settings;
    }
    let Ok(value) = std::env::var("PLASTINKA_TEST_WINDOW") else {
        return settings;
    };
    let numbers: Vec<f32> = value
        .split(',')
        .filter_map(|part| part.trim().parse().ok())
        .collect();
    if let [x, y, width, height] = numbers.as_slice() {
        settings.position = window::Position::Specific(iced::Point::new(*x, *y));
        settings.size = Size::new(*width, *height);
    }
    settings
}

/// Платформенные параметры главного окна. В macOS заголовок системный, но
/// прозрачный и без текста, а содержимое лежит под ним: окно выглядит
/// «без рамки», а слева сверху остаются настоящие кнопки macOS (закрыть,
/// свернуть, полноэкранный режим). Сдвинуть их winit не умеет.
fn main_platform_specific() -> window::settings::PlatformSpecific {
    window::settings::PlatformSpecific {
        #[cfg(target_os = "macos")]
        title_hidden: true,
        #[cfg(target_os = "macos")]
        titlebar_transparent: true,
        #[cfg(target_os = "macos")]
        fullsize_content_view: true,
        ..platform_specific()
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

/// Период кадров подписки на программном рендере: 30 кадров в секунду.
const SOFTWARE_FRAME: Duration = Duration::from_millis(33);
/// Как часто проверять обновления, пока включена автопроверка.
const UPDATE_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
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
    /// Цветовое семейство и режим тона (тёмная, светлая, как в системе).
    pub theme: theme::Family,
    pub theme_mode: theme::ThemeMode,
    /// Масштаб окна, проценты.
    pub scale: u32,
    pub visualizer: bool,
    /// Аппаратное ускорение (рендер на видеокарте); действует после перезапуска (см. `render`).
    pub hardware_acceleration: bool,
    /// Проверять обновления автоматически (по умолчанию нет).
    pub auto_update: bool,
    /// Крестик прячет окно в трей (если трей есть).
    pub close_to_tray: bool,
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
            theme: theme::Family::default(),
            theme_mode: theme::ThemeMode::default(),
            scale: config::SCALE_DEFAULT,
            visualizer: true,
            hardware_acceleration: true,
            auto_update: false,
            close_to_tray: true,
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
            theme: config
                .theme
                .as_deref()
                .and_then(theme::Family::from_key)
                .unwrap_or_default(),
            theme_mode: config
                .theme_mode
                .as_deref()
                .and_then(theme::ThemeMode::from_key)
                .unwrap_or_default(),
            scale: config.scale_percent(),
            visualizer: config.visualizer.unwrap_or(true),
            hardware_acceleration: config.hardware_acceleration.unwrap_or(true),
            auto_update: config.auto_update.unwrap_or(false),
            close_to_tray: config.close_to_tray.unwrap_or(true),
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
        config.accent = Some(self.accent.key().into_owned());
        config.theme = Some(self.theme.key().to_owned());
        config.theme_mode = Some(self.theme_mode.key().to_owned());
        config.scale = Some(i64::from(self.scale));
        config.visualizer = Some(self.visualizer);
        config.hardware_acceleration = Some(self.hardware_acceleration);
        config.auto_update = Some(self.auto_update);
        config.close_to_tray = Some(self.close_to_tray);
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
    /// Подтверждение «Сбросить всё» в настройках.
    ConfirmResetSettings,
    /// Согласие: Slopless ставит «Не нравится» ИИ-трекам (запись в аккаунт).
    SloplessConsent,
    /// Сбросить время прослушивания (серия остаётся).
    ConfirmListenReset,
    /// Заменить статистику данными из выбранной копии.
    ConfirmListenRestore,
}

#[derive(Clone)]
pub enum Message {
    WindowOpened(Option<window::Id>),
    /// Размер любого окна (главного или мини-плеера).
    AnyResized(window::Id, Size),
    /// Масштаб экрана ОС (DPI) главного окна: узнали или изменился.
    Dpi(f32),
    DpiChanged(window::Id, f32),
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
    /// Значок в трее заводится на первом сообщении, уже в цикле окна.
    /// Номер попытки создать значок (с нуля).
    TrayInit(u8),
    /// Щелчок по значку или пункт его меню.
    Tray(tray::Action),
    /// Второй запуск программы просит показать окно.
    ShowRequested,
    /// Полный выход (в отличие от `CloseWindow`, которое может спрятать окно).
    Exit,
    /// Автозапуск: что сказала система, переключатель, итог записи.
    AutostartLoaded(Result<bool, String>),
    ToggleAutostart,
    AutostartSet(bool, Result<(), String>),
    /// Переключатель «Сворачивать в трей при закрытии».
    ToggleCloseToTray,
    WindowResized(Size),
    /// Свёрнуто ли окно (`None` — платформа не ответила, считаем «нет»).
    Minimized(Option<bool>),
    /// Развёрнуто ли окно и режим окна — для скругления углов в Linux.
    Maximized(bool),
    WindowMode(window::Mode),
    DragWindow,
    #[cfg(not(target_os = "macos"))]
    ToggleMaximize,
    #[cfg(not(target_os = "macos"))]
    Minimize,
    /// Кнопка закрытия в заголовке: прячет окно в трей или закрывает программу.
    CloseWindow,
    #[cfg(not(target_os = "macos"))]
    ResizeWindow(window::Direction),
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
        EqState,
        SyncConfig,
    ),
    ToggleSidebar,
    SetAccent(theme::Accent),
    /// Цветовое семейство и режим тона из «Настройки → Оформление».
    SetTheme(theme::Family),
    SetThemeMode(theme::ThemeMode),
    /// Тон системной темы: при запуске и когда пользователь его меняет.
    SystemTone(iced::theme::Mode),
    /// Палитра своего акцента.
    Palette(palette::Msg),
    /// Экран настроек: раздел, поиск, сброс.
    Settings(settings::Msg),
    /// Кнопка эквалайзера в плеере: «Настройки → Эквалайзер».
    OpenEqualizer,
    Zoom(Zoom),
    ToggleVisualizer,
    ToggleHardwareAcceleration,
    /// Автопроверка обновлений: переключатель, таймер и запуск.
    ToggleAutoUpdate,
    UpdateAuto,
    /// Кнопка «Проверить обновления».
    UpdateCheck,
    /// Итог проверки; `true` — по кнопке.
    UpdateChecked(bool, Result<update::Check, String>),
    /// Кнопка «Обновить»: скачать, сверить и заменить файл.
    UpdateInstall,
    UpdateInstalled(Result<update::Installed, String>),
    /// Кнопка «Перезапустить» после обновления.
    UpdateRestart,
    /// Качество звука и нормализация громкости (панель «Настройки звука»).
    SetAudioQuality(AudioQuality),
    SetNormalize(bool),
    SetLyricsLrclib(bool),
    /// Настройки Discord Rich Presence.
    SetDiscord(discord::Settings),
    /// «Слушать вместе»: включить или выключить, имя устройства, задержка.
    SetSyncEnabled(bool),
    SetSyncName(String),
    SetSyncDelay(i64),
    /// Открыть раздел настроек «Слушать вместе».
    OpenSyncSettings,
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
    /// Главное окно спрятано в трей (его нет на экране). Для перерисовок это
    /// то же, что свёрнутое окно.
    hidden: bool,
    /// Значок в трее; `None` — трея нет или он не создался.
    tray: Option<tray::Tray>,
    /// Масштаб экрана ОС (DPI) главного окна.
    dpi: f32,
    /// Окно развёрнуто или на весь экран: углы прямые (см. `corners`).
    maximized: bool,
    fullscreen: bool,
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
    /// Палитра своего акцента в настройках.
    palette: palette::State,
    /// Раздел и запрос на экране настроек (в `config.toml` не пишется).
    settings: settings::State,
    /// Discord Rich Presence: фоновая задача, пока открыто главное окно.
    discord: Option<discord::Rpc>,
    /// Application ID, с которым запущена задача Discord.
    discord_app_id: String,
    /// `discord_app_id` из `config.toml` (своё приложение вместо встроенных).
    discord_override: Option<String>,
    /// «Слушать вместе»: служба связи, пока включена и открыто главное окно.
    sync: Option<Service>,
    /// Настройки `[sync]`: хранит окно, пишет в `config.toml`.
    sync_cfg: SyncConfig,
    /// Имя компьютера для имени устройства по умолчанию.
    sync_default_name: String,
    /// Служба не запустилась (хранилище паролей и т. п.): повторно не пробуем,
    /// пока настройку не переключат.
    sync_failed: bool,
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
    /// Эквалайзер: параметры, выбранный пресет, свои пресеты (`[equalizer]`).
    eq: EqState,
    /// Выбор в меню «Моей волны» (`[wave_settings]`, в аккаунт не пишется).
    wave_settings: api::WaveSettings,
    /// Тема iced с текущими цветами и акцентом.
    theme: iced::Theme,
    /// Тон системной темы (в `config.toml` не пишется).
    system_tone: theme::Tone,
    /// Настройки сейчас пишутся; `save_again` — за это время они изменились.
    saving: bool,
    save_again: bool,
    started: Instant,
    now: Instant,
    toast: Option<(String, Instant)>,
    /// Версия, о которой уже сообщили всплывающим сообщением в этом запуске.
    update_toasted: Option<String>,
}

impl App {
    fn boot() -> (Self, Task<Message>) {
        let cache_dir = AppDirs::discover()
            .ok()
            .map(|dirs| dirs.cache_dir().to_path_buf());
        let now = Instant::now();
        // С `--tray` окно не открывается совсем: его покажет значок в трее
        // (или сразу, если трей не создался, см. `Message::TrayInit`).
        let start_hidden = launch().tray;
        let (main_window, open_main) = if start_hidden {
            (None, Task::none())
        } else {
            let (id, open) = window::open(window_settings());
            (Some(id), open.map(|id| Message::WindowOpened(Some(id))))
        };
        let app = Self {
            window: main_window,
            size: window_settings().size,
            minimized: false,
            hidden: start_hidden,
            tray: None,
            dpi: 1.0,
            maximized: false,
            fullscreen: false,
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
            sync: None,
            sync_cfg: SyncConfig::default(),
            sync_default_name: default_device_name(),
            sync_failed: false,
            media: None,
            media_failed: false,
            hwnd: None,
            mini: None,
            mini_pinned: true,
            mini_prefs: mini::MiniPrefs::default(),
            closing: false,
            plugins: PluginSettings::default(),
            eq: EqState::default(),
            wave_settings: api::WaveSettings::default(),
            theme: theme::theme(),
            system_tone: theme::Tone::default(),
            palette: palette::State::default(),
            settings: settings::State::default(),
            saving: false,
            save_again: false,
            started: now,
            now,
            toast: None,
            update_toasted: None,
        };
        let task = Task::batch([
            // Остаток прошлого обновления (`Plastinka.exe.old`) убираем тихо.
            Task::perform(
                storage::blocking(|| {
                    update::cleanup();
                    Ok(())
                }),
                |_| Message::Noop,
            ),
            open_main,
            Task::done(Message::TrayInit(0)),
            Task::perform(load_autostart(), Message::AutostartLoaded),
            Task::perform(load_token(), Message::TokenLoaded),
            iced::system::theme().map(Message::SystemTone),
            // Не прочитались настройки — всё по умолчанию.
            Task::perform(load_config(), |config| {
                let config = config.unwrap_or_default();
                Message::ConfigLoaded(
                    Prefs::from_config(&config),
                    plugin_settings(&config),
                    config.discord_app_id.clone(),
                    api::WaveSettings::from_config(&config.wave_settings),
                    mini::MiniPrefs::from_config(&config.mini_player),
                    config.equalizer(),
                    config.sync.clone(),
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

    fn set_dpi(&mut self, dpi: f32) -> Task<Message> {
        if dpi.is_finite() && dpi > 0.0 {
            self.dpi = dpi;
            if let Screen::Main(library) = &mut self.screen {
                library.set_dpi(dpi);
            }
        }
        Task::none()
    }

    fn compact(&self) -> bool {
        self.size.width < COMPACT_WIDTH
    }

    /// Обработка сообщения. Сменился экран или раздел — он появляется плавно.
    fn update(&mut self, message: Message) -> Task<Message> {
        self.now = Instant::now();
        #[cfg(debug_assertions)]
        fps::message();
        let before = self.page_key();
        // Кадры и секундные тики статус не меняют.
        let discord_relevant = !matches!(
            message,
            Message::Frame(_) | Message::Tick(_) | Message::MiniMoved(..)
        );
        let task = self.handle(message);
        let task = Task::batch([task, self.sync_mini()]);
        let task = if discord_relevant {
            self.sync_discord();
            self.sync_media();
            self.sync_tray();
            Task::batch([task, self.sync_together()])
        } else {
            task
        };
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

    /// «Слушать вместе»: служба создаётся в главном окне после подтверждения
    /// Плюса (пропуск берётся у библиотеки), если функция включена, и
    /// убирается при выключении, выходе из аккаунта и закрытии окна (при
    /// удалении она прощается с собеседником). Здесь же команды библиотеки
    /// уходят службе.
    fn sync_together(&mut self) -> Task<Message> {
        if self.closing {
            self.sync = None;
            return Task::none();
        }
        let Screen::Main(library) = &mut self.screen else {
            self.sync = None;
            return Task::none();
        };
        library.together_config(&self.sync_cfg, &self.sync_default_name);
        if !self.sync_cfg.enabled() {
            self.sync_failed = false;
            if self.sync.take().is_some() {
                library.together_started(None);
            }
            return Task::none();
        }
        let mut created_id = false;
        let mut problem = None;
        if self.sync.is_none()
            && !self.sync_failed
            && let Some(permit) = library.media_permit()
        {
            let (id, created) = self.sync_cfg.ensure_device_id();
            let name = self
                .sync_cfg
                .device_name
                .clone()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| self.sync_default_name.clone());
            #[cfg_attr(not(debug_assertions), allow(unused_mut))]
            let mut config = ServiceConfig::new(id, name, self.sync_cfg.paired.clone());
            // Тестовые хуки: только 127.0.0.1 и без mDNS, чтобы не вызывать окно
            // брандмауэра.
            #[cfg(debug_assertions)]
            if storage::test_hooks_active() {
                config.bind = std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);
                config.mdns = false;
            }
            let started = SecretStore::open()
                .map_err(|err| err.to_string())
                .and_then(|store| {
                    Service::new(&permit, config, store).map_err(|err| err.to_string())
                });
            match started {
                Ok(service) => {
                    library.together_started(Some(service.device_id().to_owned()));
                    self.sync = Some(service);
                    created_id = created;
                }
                Err(text) => {
                    self.sync_failed = true;
                    problem = Some(format!("Слушать вместе не запустилось: {text}"));
                }
            }
        }
        if let Some(service) = &self.sync {
            for command in library.together_take() {
                service.send(command);
            }
        }
        if let Some(text) = problem {
            self.show_toast(text);
        }
        if created_id {
            self.save_prefs()
        } else {
            Task::none()
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
        // Копия статистики, которую не подтвердили, выбрасывается.
        if self.dialog == Some(Dialog::ConfirmListenRestore)
            && let Screen::Main(library) = &mut self.screen
        {
            library.listen_discard_restore();
        }
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
                        window::scale_factor(id).map(Message::Dpi),
                    ])
                });
            }
            Message::Dpi(dpi) => return self.set_dpi(dpi),
            Message::DpiChanged(id, dpi) => {
                if self.window == Some(id) {
                    return self.set_dpi(dpi);
                }
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
                    self.request_close()
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
                let minimized =
                    self.with_window(|id| window::is_minimized(id).map(Message::Minimized));
                // Развёрнутость нужна только для углов: где их скругляет
                // система, лишних запросов не делаем.
                if !corners::ENABLED {
                    return minimized;
                }
                return Task::batch([
                    minimized,
                    self.with_window(|id| window::is_maximized(id).map(Message::Maximized)),
                    self.with_window(|id| window::mode(id).map(Message::WindowMode)),
                ]);
            }
            Message::Minimized(state) => {
                self.minimized = state.unwrap_or(false);
                self.apply_paused();
            }
            Message::Maximized(maximized) => self.maximized = maximized,
            Message::WindowMode(mode) => self.fullscreen = mode == window::Mode::Fullscreen,
            Message::DragWindow => {
                drag_pulse::start();
                return self.with_window(window::drag);
            }
            #[cfg(not(target_os = "macos"))]
            Message::ToggleMaximize => return self.with_window(window::toggle_maximize),
            #[cfg(not(target_os = "macos"))]
            Message::Minimize => return self.with_window(|id| window::minimize(id, true)),
            Message::CloseWindow => return self.request_close(),
            Message::Exit => return self.close(),
            Message::TrayInit(attempt) => match tray::Tray::create() {
                Ok(tray) => {
                    self.tray = Some(tray);
                    self.settings.tray = true;
                }
                // Служба значков может подняться позже: пока окно спрятано,
                // оно остаётся спрятанным до последней неудачной попытки.
                Err(err) if err.retry && attempt + 1 < tray::ATTEMPTS => {
                    return Task::perform(tokio::time::sleep(tray::RETRY_EVERY), move |()| {
                        Message::TrayInit(attempt + 1)
                    });
                }
                Err(err) => {
                    // Без трея работаем как раньше: крестик закрывает программу.
                    let _ = writeln!(
                        std::io::stderr(),
                        "Предупреждение: значок в трее недоступен: {}",
                        err.text
                    );
                    if self.hidden {
                        return self.show_window();
                    }
                }
            },
            Message::Tray(action) => return self.tray_action(action),
            Message::ShowRequested => return self.show_window(),
            Message::AutostartLoaded(result) => {
                self.settings.autostart = result.unwrap_or(false);
            }
            Message::ToggleAutostart => {
                let enable = !self.settings.autostart;
                // С тестовыми хуками в систему ничего не пишем.
                if storage::test_hooks_active() {
                    self.settings.autostart = enable;
                } else {
                    return Task::perform(set_autostart(enable), move |result| {
                        Message::AutostartSet(enable, result)
                    });
                }
            }
            Message::AutostartSet(enable, result) => match result {
                Ok(()) => self.settings.autostart = enable,
                Err(text) => self.show_toast(text),
            },
            Message::ToggleCloseToTray => {
                let prefs = Prefs {
                    close_to_tray: !self.prefs.close_to_tray,
                    ..self.prefs
                };
                return self.change_prefs(prefs);
            }
            #[cfg(not(target_os = "macos"))]
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

            Message::ConfigLoaded(
                prefs,
                plugins,
                discord_override,
                wave_settings,
                mini_prefs,
                eq,
                sync,
            ) => {
                self.discord_override = discord_override;
                self.sync_cfg = sync;
                self.apply_eq(eq);
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
                let check = self.auto_update_check();
                if rescaled {
                    return Task::batch([plugins, check, self.refresh_size()]);
                }
                return Task::batch([plugins, check]);
            }
            Message::SetSyncEnabled(on) => {
                self.sync_cfg.enabled = Some(on);
                self.sync_failed = false;
                return self.save_prefs();
            }
            Message::SetSyncName(name) => {
                self.sync_cfg.device_name = (!name.trim().is_empty()).then_some(name);
                return self.save_prefs();
            }
            Message::SetSyncDelay(ms) => {
                self.sync_cfg.delay_ms =
                    Some(ms.clamp(-config::SYNC_DELAY_LIMIT, config::SYNC_DELAY_LIMIT));
                return self.save_prefs();
            }
            Message::OpenSyncSettings => {
                if let Screen::Main(library) = &mut self.screen {
                    library.close_together_panel();
                }
                let open = self
                    .settings
                    .update(settings::Msg::Open(settings::Category::Together));
                let nav = Task::done(Message::Library(library::Message::Nav(
                    library::Section::Settings,
                )));
                return nav.chain(open);
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
                self.palette.sync(accent);
                return self.change_prefs(prefs);
            }
            Message::SetTheme(family) => {
                let prefs = Prefs {
                    theme: family,
                    ..self.prefs
                };
                return self.change_prefs(prefs);
            }
            Message::SetThemeMode(mode) => {
                let prefs = Prefs {
                    theme_mode: mode,
                    ..self.prefs
                };
                return self.change_prefs(prefs);
            }
            Message::SystemTone(mode) => {
                self.system_tone = theme::Tone::from_system(mode);
                self.sync_scheme();
            }
            Message::Palette(message) => return self.palette_message(message),
            Message::Settings(message) => return self.settings_message(message),
            Message::OpenEqualizer => {
                // Сначала раздел окна, потом страница и прокрутка в начало.
                let open = self
                    .settings
                    .update(settings::Msg::Open(settings::Category::Equalizer));
                let nav = Task::done(Message::Library(library::Message::Nav(
                    library::Section::Settings,
                )));
                return nav.chain(open);
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
            Message::ToggleAutoUpdate => {
                let prefs = Prefs {
                    auto_update: !self.prefs.auto_update,
                    ..self.prefs
                };
                let enabled = prefs.auto_update;
                let save = self.change_prefs(prefs);
                // Включили — проверяем сразу, а не через 6 часов.
                return if enabled {
                    Task::batch([save, self.auto_update_check()])
                } else {
                    save
                };
            }
            Message::UpdateAuto => return self.auto_update_check(),
            Message::UpdateCheck => {
                if self.settings.update.start_check() {
                    return Task::perform(check_update(), |result| {
                        Message::UpdateChecked(true, result)
                    });
                }
            }
            Message::UpdateChecked(manual, result) => {
                if let Some(version) = self.settings.update.finish_check(result, manual)
                    && self.update_toasted.as_deref() != Some(version.as_str())
                {
                    self.show_toast(format!(
                        "Доступна Plastinka {version} — Настройки → О программе"
                    ));
                    self.update_toasted = Some(version);
                }
            }
            Message::UpdateInstall => {
                if let Some(package) = self.settings.update.start_install() {
                    return Task::perform(install_update(package), Message::UpdateInstalled);
                }
            }
            Message::UpdateInstalled(result) => self.settings.update.installed(result),
            Message::UpdateRestart => {
                if let Some(exe) = self.settings.update.restart_target() {
                    match update::restart(&exe) {
                        // Обычный выход окна: все сохранения успевают пройти.
                        Ok(()) => return self.close(),
                        Err(err) => {
                            self.show_toast(format!("Не удалось перезапустить: {err}"));
                        }
                    }
                }
            }
            Message::ToggleHardwareAcceleration => {
                let prefs = Prefs {
                    hardware_acceleration: !self.prefs.hardware_acceleration,
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
                // «Забыть» устройство: из конфига убирает окно, секрет — служба.
                let forget = if let library::Message::Together(library::TogetherMsg::Forget(id)) =
                    &message
                {
                    self.sync_cfg.forget(id);
                    self.save_prefs()
                } else {
                    Task::none()
                };
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
                        Some(library::Effect::OpenDialog(dialog)) => {
                            self.open_dialog(dialog);
                            Task::none()
                        }
                        Some(library::Effect::DialogDone(toast)) => {
                            self.close_dialog();
                            if let Some(text) = toast {
                                self.show_toast(text);
                            }
                            Task::none()
                        }
                        Some(library::Effect::OpenMini) => self.open_mini(),
                        Some(library::Effect::WaveSettings(settings)) => {
                            self.wave_settings = settings;
                            self.save_prefs()
                        }
                        Some(library::Effect::TogetherPaired(device)) => {
                            self.sync_cfg.remember(device);
                            self.save_prefs()
                        }
                        None => Task::none(),
                    };
                    return Task::batch([task, covers, effect, forget]);
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
        theme::set_accent(prefs.accent);
        if let Screen::Main(library) = &mut self.screen {
            library.set_sound(prefs.quality, prefs.normalize);
            library.set_lyrics_lrclib(prefs.lyrics_lrclib);
        }
        self.prefs = prefs;
        self.sync_scheme();
    }

    /// Итоговый тон: выбранный в настройках или тон системы.
    fn tone(&self) -> theme::Tone {
        self.prefs.theme_mode.tone(self.system_tone)
    }

    /// Применяет семейство и тон: общая схема цветов и тема iced.
    fn sync_scheme(&mut self) {
        theme::set_scheme(self.prefs.theme, self.tone());
        self.theme = theme::theme();
    }

    /// Экран настроек. «Сбросить всё» возвращает значения по умолчанию и
    /// применяет их так же, как обычное изменение каждой настройки.
    fn settings_message(&mut self, message: settings::Msg) -> Task<Message> {
        match message {
            settings::Msg::ResetAll => {}
            settings::Msg::Eq(message) => return self.eq_message(message),
            message => return self.settings.update(message),
        }
        self.close_dialog();
        let prefs = settings::reset_prefs(&self.prefs, self.discord_override.as_deref());
        let rescaled = prefs.scale != self.prefs.scale;
        self.palette.sync(prefs.accent);
        let plugins = self.apply_plugins(PluginSettings::default());
        self.apply_eq(settings::reset_eq(&self.eq));
        let save = self.change_prefs(prefs);
        // Открытая панель текста перезапрашивает его (LRCLib мог включиться):
        // после `change_prefs`, когда новое значение уже у главного экрана.
        let lyrics = match &mut self.screen {
            Screen::Main(library) => library.lyrics_sync().map(Message::Library),
            _ => Task::none(),
        };
        let size = if rescaled {
            self.refresh_size()
        } else {
            Task::none()
        };
        Task::batch([plugins, lyrics, save, size])
    }

    /// Страница эквалайзера. Параметры сразу уходят плееру, а в
    /// `config.toml` пишутся только по отпусканию и дискретным действиям.
    fn eq_message(&mut self, message: settings::EqMsg) -> Task<Message> {
        let opens_name = matches!(message, settings::EqMsg::NameOpen);
        let effect = settings::update_eq(&mut self.eq, &mut self.settings.eq_name, message);
        if effect.apply
            && let Screen::Main(library) = &mut self.screen
        {
            library.set_equalizer(self.eq.settings);
        }
        let focus = if opens_name && self.settings.eq_name.open {
            settings::State::focus_eq_name()
        } else {
            Task::none()
        };
        let save = if effect.save {
            self.save_prefs()
        } else {
            Task::none()
        };
        Task::batch([focus, save])
    }

    /// Новые параметры эквалайзера: окну и плееру.
    fn apply_eq(&mut self, eq: EqState) {
        self.eq = eq;
        if let Screen::Main(library) = &mut self.screen {
            library.set_equalizer(self.eq.settings);
        }
    }

    /// Палитра своего акцента. Перетаскивание только показывает цвет
    /// (во всём окне), запись в `config.toml` — когда кнопку отпустили.
    fn palette_message(&mut self, message: palette::Msg) -> Task<Message> {
        use palette::Msg;
        match message {
            Msg::Toggle => {
                self.palette.open = !self.palette.open;
                if self.palette.open {
                    self.palette.sync(self.prefs.accent);
                }
                Task::none()
            }
            Msg::Field(x, y) => {
                self.palette.pick(x, y);
                self.preview_accent();
                Task::none()
            }
            Msg::Hue(x) => {
                self.palette.pick_hue(x);
                self.preview_accent();
                Task::none()
            }
            Msg::Released => {
                if self.palette.take_dirty() {
                    self.save_prefs()
                } else {
                    Task::none()
                }
            }
            Msg::Hex(text) => match self.palette.input(text) {
                Some(accent) if accent != self.prefs.accent => {
                    let prefs = Prefs {
                        accent,
                        ..self.prefs
                    };
                    self.change_prefs(prefs)
                }
                _ => Task::none(),
            },
        }
    }

    /// Цвет из палитры сразу становится акцентом окна, но не пишется на диск.
    fn preview_accent(&mut self) {
        let accent = self.palette.accent();
        if accent != self.prefs.accent {
            self.prefs.accent = accent;
            theme::set_accent(accent);
            self.theme = theme::theme();
        }
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
                self.eq.clone(),
                self.wave_settings.clone(),
                self.mini_prefs,
                self.sync_cfg.clone(),
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
        // Значок убираем до выхода: иначе Windows оставит его до наведения мыши.
        self.tray = None;
        // Статус в Discord очищается до закрытия окна и больше не появляется.
        self.discord = None;
        // Время прослушивания: файл небольшой, пишем до выхода синхронно.
        if let Screen::Main(library) = &mut self.screen {
            library.listen_shutdown();
        }
        // Собеседник узнаёт о выходе (`bye shutdown`), служба останавливается.
        self.sync = None;
        // Мини-плеер закрывается вместе с главным окном: положение
        // запоминается до выхода.
        if self.mini.take().is_some() {
            // Выходим только после записи: иначе выход оборвёт её.
            return self.save_prefs().chain(Task::done(Message::Exit));
        }
        self.closing = true;
        let (Some(token), Ok(http)) = (self.unsaved.take(), self.http()) else {
            return iced::exit();
        };
        Task::perform(
            async move {
                let _ = tokio::time::timeout(REVOKE_ON_CLOSE, revoke(http, token)).await;
            },
            |()| Message::Exit,
        )
    }

    /// Крестик главного окна: с треем и включённой настройкой окно прячется
    /// (музыка играет), иначе программа закрывается.
    fn request_close(&mut self) -> Task<Message> {
        if self.tray.is_some() && self.prefs.close_to_tray && !self.closing {
            return self.hide_window();
        }
        self.close()
    }

    /// Прячет главное окно в трей. Где окно скрыть нельзя (Wayland), оно
    /// закрывается совсем и открывается заново при показе.
    fn hide_window(&mut self) -> Task<Message> {
        if self.hidden {
            return Task::none();
        }
        self.hidden = true;
        self.apply_paused();
        let Some(id) = self.window else {
            return Task::none();
        };
        if tray::hide_by_closing() {
            self.window = None;
            window::close(id)
        } else {
            window::set_mode(id, window::Mode::Hidden)
        }
    }

    /// Показывает главное окно: из трея, из свёрнутого или просто поднимает
    /// его, если оно на виду.
    fn show_window(&mut self) -> Task<Message> {
        let was_hidden = std::mem::replace(&mut self.hidden, false);
        self.apply_paused();
        let Some(id) = self.window else {
            // Последний размер: `self.size` в единицах раскладки, окну нужны
            // логические единицы, то есть с учётом масштаба программы.
            let mut settings = window_settings();
            let scale = self.prefs.scale as f32 / 100.0;
            let size = Size::new(self.size.width * scale, self.size.height * scale);
            if size.width.is_finite() && size.height.is_finite() && size.width > 0.0 {
                settings.size = size;
            }
            let (id, open) = window::open(settings);
            self.window = Some(id);
            return open.map(|id| Message::WindowOpened(Some(id)));
        };
        let mut tasks = Vec::new();
        if was_hidden {
            tasks.push(window::set_mode(id, window::Mode::Windowed));
        }
        tasks.push(window::minimize(id, false));
        tasks.push(window::gain_focus(id));
        Task::batch(tasks)
    }

    fn tray_action(&mut self, action: tray::Action) -> Task<Message> {
        use media_controls::Command;
        match action {
            tray::Action::Show => self.show_window(),
            tray::Action::Toggle if self.hidden => self.show_window(),
            tray::Action::Toggle => self.hide_window(),
            tray::Action::PlayPause => self.handle(Message::Media(Command::Toggle)),
            tray::Action::Next => self.handle(Message::Media(Command::Next)),
            tray::Action::Previous => self.handle(Message::Media(Command::Previous)),
            tray::Action::Quit => self.close(),
        }
    }

    /// Окно не на виду (свёрнуто или в трее): кадры и серия дней стоят.
    fn paused(&self) -> bool {
        self.minimized || self.hidden
    }

    fn apply_paused(&mut self) {
        let paused = self.paused();
        #[cfg(debug_assertions)]
        fps::set_minimized(paused);
        // Окно не на виду серию дней не отмечает.
        if let Screen::Main(library) = &mut self.screen {
            library.listen_minimized(paused);
        }
    }

    /// Приводит значок в трее к состоянию окна и плеера.
    fn sync_tray(&mut self) {
        let Some(tray) = &mut self.tray else {
            return;
        };
        let mut view = tray::View {
            hidden: self.hidden,
            ..tray::View::default()
        };
        if let Screen::Main(library) = &self.screen {
            view.player = true;
            let state = library.media_state();
            view.playing = state.playing;
            if let Some(track) = state.track {
                view.track = format!("{} — {}", track.artists, track.title);
            }
        }
        tray.apply(&view);
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
        {
            // В настройках Esc сначала очищает поиск настроек.
            if library.settings_open() && self.settings.clear_query() {
                return Task::none();
            }
            if let Some(task) = library.clear_search() {
                return task.map(Message::Library);
            }
        }
        Task::none()
    }

    /// Автоматическая проверка обновлений: только если она включена и ничего
    /// другого не идёт. Состояние не трогает, пока не пришёл ответ.
    fn auto_update_check(&self) -> Task<Message> {
        if !self.prefs.auto_update || self.settings.update.busy() {
            return Task::none();
        }
        Task::perform(check_update(), |result| {
            Message::UpdateChecked(false, result)
        })
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
        library.set_dpi(self.dpi);
        library.set_sound(self.prefs.quality, self.prefs.normalize);
        library.set_lyrics_lrclib(self.prefs.lyrics_lrclib);
        // Параметры эквалайзера уходят плееру вместе с громкостью и качеством.
        library.set_equalizer(self.eq.settings);
        library.set_wave_settings(self.wave_settings.clone());
        // Окно могло остаться в трее: серия дней его не засчитывает.
        library.listen_minimized(self.paused());
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
            iced::system::theme_changes().map(Message::SystemTone),
            iced::event::listen_with(|event, _, id| match event {
                iced::Event::Window(window::Event::Rescaled(dpi)) => {
                    Some(Message::DpiChanged(id, dpi))
                }
                _ => None,
            }),
        ];
        // Автопроверка обновлений раз в 6 часов (первая — при запуске).
        if self.prefs.auto_update {
            subscriptions.push(iced::time::every(UPDATE_EVERY).map(|_| Message::UpdateAuto));
        }
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
        // На программном рендере кадры гонит подписка: одно сообщение — одна
        // перерисовка. Пока окно тянут (модальный цикл Windows, подписки
        // стоят), кадры ведёт `heartbeat`, и вместе они не работают.
        // Период 33 мс, а не `heartbeat::PERIOD`: таймер iced срабатывает
        // чуть раньше срока, и с 30 мс кадров выходило около 38 в секунду.
        if render::software() && !drag_pulse::active() && self.wants_frames() {
            subscriptions.push(iced::time::every(SOFTWARE_FRAME).map(Message::Frame));
        }
        // Кадры гонит невидимый виджет `heartbeat` из самой перерисовки (в
        // `decorate`): так анимации и спектр идут и пока окно тянут за
        // заголовок или край, когда таймеры и подписки стоят.
        if let Screen::Main(library) = &self.screen {
            subscriptions.push(library.subscription());
        }
        if let Some(service) = &self.sync {
            subscriptions.push(
                Subscription::run_with(
                    service.events(),
                    |events: &crate::sync::service::Events| events.stream(),
                )
                .map(|event| {
                    Message::Library(library::Message::Together(library::TogetherMsg::Event(
                        Box::new(event),
                    )))
                }),
            );
        }
        // Второй запуск просит показать окно (только пока мы первый экземпляр).
        if instance::serving() {
            subscriptions
                .push(Subscription::run(instance::requests).map(|()| Message::ShowRequested));
        }
        if self.tray.is_some() {
            subscriptions.push(Subscription::run(tray::actions).map(Message::Tray));
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
        !self.paused() && (self.transitioning() || self.animating())
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
                    &self.palette,
                    &self.settings,
                    &self.eq,
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
        // Фон мини-плеера всегда тёмный: тёмный тон при любой теме окна.
        let view = theme::dark_scope(|| {
            dark::scope(library.mini_view(
                &self.covers,
                self.mini_prefs.compact,
                self.mini_pinned,
                average,
                self.now,
            ))
        });
        // Мини-плеер не разворачивается, масштаба у него нет.
        let radius = corners::radius(corners::ENABLED && !render::software(), false, false, 1.0);
        match corners::eraser(radius) {
            Some(eraser) => stack![view, eraser]
                .width(Length::Fill)
                .height(Length::Fill)
                .into(),
            None => view,
        }
    }

    /// Заголовок окна, края для изменения размера, диалог и сообщение.
    fn decorate<'a>(&'a self, content: Element<'a, Message>) -> Element<'a, Message> {
        let with_brand = !matches!(self.screen, Screen::Main(_));
        // Таймер YandexMusicTime — по центру заголовка главного окна.
        let timer = match &self.screen {
            Screen::Main(library) => library.listen_title(),
            _ => None,
        };
        let window = container(column![gate::title_bar(with_brand, timer), content])
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(theme::bg().into()),
                ..container::Style::default()
            });
        let mut layers = stack![window].width(Length::Fill).height(Length::Fill);
        if let Screen::Main(library) = &self.screen {
            // Фон полноэкранного плеера — затемнённая обложка: тёмный тон при
            // любой теме окна.
            let sheet = theme::dark_scope(|| {
                library
                    .fullscreen(
                        &self.covers,
                        &self.prefs,
                        self.now,
                        self.compact(),
                        self.size,
                        self.turn(),
                        gate::title_bar(false, library.listen_title()),
                    )
                    .map(dark::scope)
            });
            if let Some(sheet) = sheet {
                layers = layers.push(sheet);
            }
            if let Some(panel) = library.volume_panel(self.now) {
                layers = layers.push(panel);
            }
            if let Some(panel) = library.sound_panel(&self.prefs, self.now) {
                layers = layers.push(panel);
            }
            if let Some(panel) = library.together_panel(self.now) {
                layers = layers.push(panel);
            }
            if let Some(dialog) = library.together_pairing_dialog() {
                layers = layers.push(dialog);
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
            let detail = match &self.screen {
                Screen::Main(library) => library.listen_restore_text(),
                _ => "",
            };
            layers = layers.push(gate::dialog(dialog, shown, self.dialog.is_some(), detail));
        }
        // Переходам — кадры с частотой экрана, спиннерам и спектру хватает 30
        // в секунду. Когда ничего не движется, кадров нет вовсе.
        let transitioning = self.transitioning();
        // Свёрнутому окну кадры не нужны (и пауза всё равно их не пропустит).
        let layers = layers.push(heartbeat::heartbeat(
            self.wants_frames() && (!render::software() || drag_pulse::active()),
            transitioning,
            Message::Frame,
        ));
        // В macOS рамка системная и размер меняет сама система: своих полос нет.
        #[cfg(not(target_os = "macos"))]
        let layers = layers.push(resize_edges());
        // Скругление углов — поверх всего, мышь не ловит.
        let scale = self.prefs.scale as f32 / 100.0;
        let radius = corners::radius(
            corners::ENABLED && !render::software(),
            self.maximized,
            self.fullscreen,
            scale,
        );
        let layers = match corners::eraser(radius) {
            Some(eraser) => layers.push(eraser),
            None => layers,
        };
        // Где нажали мышь — для меню трека у курсора.
        let root: Element<'a, Message> = if matches!(self.screen, Screen::Main(_)) {
            pointer::pointer(layers, |at| {
                Message::Library(library::Message::Plugin(library::PluginMsg::Pointer(at)))
            })
        } else {
            layers.into()
        };
        // В свёрнутом окне (клиентская область не 0×0) перерисовку не просим.
        pause::pause(root, self.paused())
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
        .veil(theme::alpha(theme::bg(), hidden), 0.0)
        .into()
}

/// Невидимые полосы по краям окна: окно без рамки само размер не меняет.
#[cfg(not(target_os = "macos"))]
fn resize_edges<'a>() -> Element<'a, Message> {
    const EDGE: f32 = 5.0;
    let edge = |width: Length, height: Length, direction: window::Direction, cursor| {
        iced::widget::mouse_area(
            container(iced::widget::Space::new())
                .width(width)
                .height(height),
        )
        .interaction(cursor)
        .on_press(Message::ResizeWindow(direction))
    };
    use iced::mouse::Interaction::{
        ResizingDiagonallyDown as Down, ResizingDiagonallyUp as Up,
        ResizingHorizontally as Horizontal, ResizingVertically as Vertical,
    };
    let fixed = Length::Fixed(EDGE);
    let top = row![
        edge(fixed, fixed, window::Direction::NorthWest, Down),
        edge(Length::Fill, fixed, window::Direction::North, Vertical),
        edge(fixed, fixed, window::Direction::NorthEast, Up),
    ];
    let middle = row![
        edge(fixed, Length::Fill, window::Direction::West, Horizontal),
        iced::widget::Space::new()
            .width(Length::Fill)
            .height(Length::Fill),
        edge(fixed, Length::Fill, window::Direction::East, Horizontal),
    ]
    .height(Length::Fill);
    let bottom = row![
        edge(fixed, fixed, window::Direction::SouthWest, Up),
        edge(Length::Fill, fixed, window::Direction::South, Vertical),
        edge(fixed, fixed, window::Direction::SouthEast, Down),
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

/// Включён ли автозапуск в системе. С тестовыми хуками реестр не трогаем.
async fn load_autostart() -> Result<bool, String> {
    if storage::test_hooks_active() {
        return Ok(false);
    }
    tokio::task::spawn_blocking(autostart::is_enabled)
        .await
        .unwrap_or_else(|_| Err("фоновая операция прервалась".to_owned()))
}

/// Включает или выключает автозапуск в системе (реестр, файл на диске).
async fn set_autostart(enable: bool) -> Result<(), String> {
    tokio::task::spawn_blocking(move || autostart::set_enabled(enable))
        .await
        .unwrap_or_else(|_| Err("фоновая операция прервалась".to_owned()))
}

/// Отзывает токен на сервере. Ошибка не важна: токена на компьютере нет.
/// Проверка обновлений на GitHub; ошибка — текст для пользователя.
async fn check_update() -> Result<update::Check, String> {
    update::check().await.map_err(|err| err.to_string())
}

/// Скачивание, сверка SHA-256 и замена файла программы.
async fn install_update(package: update::Package) -> Result<update::Installed, String> {
    update::install(package)
        .await
        .map_err(|err| err.to_string())
}

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

/// Запоминает вид меню, внешний вид, плагины и эквалайзер. Остальные настройки не трогает.
async fn save_prefs(
    prefs: Prefs,
    plugins: PluginSettings,
    eq: EqState,
    wave_settings: api::WaveSettings,
    mini_prefs: mini::MiniPrefs,
    sync: SyncConfig,
) -> Result<(), Failure> {
    let path = AppDirs::discover()?.config_file();
    storage::blocking(move || {
        let mut config = Config::load(&path)?;
        prefs.write(&mut config);
        plugins.write(&mut config.plugins);
        config.set_equalizer(&eq);
        wave_settings.write(&mut config.wave_settings);
        mini_prefs.write(&mut config.mini_player);
        config.sync = sync;
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
