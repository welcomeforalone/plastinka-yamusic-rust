//! Окно приложения на iced: вход, «Нужна подписка», главное окно с боковым
//! меню, списком «Мне нравится» и панелью плеера, окно «О программе».
//!
//! Главное окно и плеер в нём создаются только с пропуском
//! [`PlaybackPermit`], то есть после явного подтверждения Плюса.

mod anim;
mod covers;
mod filter;
mod gate;
mod icons;
mod library;
mod marquee;
mod pointer;
mod queue;
mod scroll;
mod theme;
mod visualizer;
mod widgets;

use std::sync::Arc;
use std::time::{Duration, Instant};

use iced::animation::Animation;
use iced::widget::{column, container, mouse_area, row, stack};
use iced::window::{self, Direction};
use iced::{Element, Length, Size, Subscription, Task, keyboard, mouse};

use crate::APP_NAME;
use crate::api::{self, AccountStatus, ApiError, PlaybackPermit};
use crate::auth::{self, AuthError, DeviceCode, OAuthApp};
use crate::net::{self, NetError};
use crate::plugins::PluginSettings;
use crate::storage::config::{self, Config};
use crate::storage::token::{StoredToken, TokenStore};
use crate::storage::{self, AppDirs, StorageError};
use crate::texts::sentence;

/// Запускает окно. Блокирует поток до закрытия окна.
pub fn run() -> iced::Result {
    let mut app = iced::application(App::boot, App::update, App::view)
        .title(APP_NAME)
        .theme(App::theme)
        .scale_factor(App::scale_factor)
        .subscription(App::subscription)
        .default_font(theme::REGULAR)
        .window(window_settings())
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
        #[cfg(windows)]
        platform_specific: window::settings::PlatformSpecific {
            undecorated_shadow: true,
            ..window::settings::PlatformSpecific::default()
        },
        ..window::Settings::default()
    }
}

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
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            sidebar_collapsed: false,
            accent: theme::Accent::default(),
            scale: config::SCALE_DEFAULT,
            visualizer: true,
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
        }
    }

    fn write(self, config: &mut Config) {
        config.sidebar_collapsed = self.sidebar_collapsed;
        config.accent = Some(self.accent.key().to_owned());
        config.scale = Some(i64::from(self.scale));
        config.visualizer = Some(self.visualizer);
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
    WindowResized(Size),
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

    ConfigLoaded(Prefs, PluginSettings),
    ToggleSidebar,
    SetAccent(theme::Accent),
    Zoom(Zoom),
    ToggleVisualizer,
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
    /// Плагины: что включено. Тоже в `config.toml`, таблица `[plugins]`.
    plugins: PluginSettings,
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
        let app = Self {
            window: None,
            size: window_settings().size,
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
            plugins: PluginSettings::default(),
            theme: theme::theme(),
            saving: false,
            save_again: false,
            started: now,
            now,
            toast: None,
        };
        let task = Task::batch([
            window::latest().map(Message::WindowOpened),
            Task::perform(load_token(), Message::TokenLoaded),
            // Не прочитались настройки — всё по умолчанию.
            Task::perform(load_config(), |config| {
                let config = config.unwrap_or_default();
                Message::ConfigLoaded(Prefs::from_config(&config), plugin_settings(&config))
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

    fn theme(&self) -> iced::Theme {
        self.theme.clone()
    }

    fn scale_factor(&self) -> f32 {
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
        let task = self.handle(message);
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
                    window::size(id).map(Message::WindowResized)
                });
            }
            Message::WindowResized(size) => {
                self.size = size;
                if let Screen::Main(library) = &mut self.screen {
                    library.resized(size.height);
                }
            }
            Message::DragWindow => return self.with_window(window::drag),
            Message::ToggleMaximize => return self.with_window(window::toggle_maximize),
            Message::Minimize => return self.with_window(|id| window::minimize(id, true)),
            Message::CloseWindow => return self.close(),
            Message::ResizeWindow(direction) => {
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

            Message::ConfigLoaded(prefs, plugins) => {
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
            Message::BigCoverLoaded(uri, big) => self.covers.big_finished(uri, big),
            Message::Library(message) => {
                if let Screen::Main(library) = &mut self.screen {
                    let http = self.http.as_ref().ok();
                    let (task, effect) = library.update(message, http);
                    let task = task.map(Message::Library);
                    let covers = Task::batch([
                        self.covers.show(library.visible_covers(), http),
                        self.covers.want_big(library.fullscreen_cover(), http),
                    ]);
                    let effect = match effect {
                        Some(library::Effect::Relogin) => self.start_login(Some(
                            "Вход больше не действует. Войдите заново.".to_owned(),
                        )),
                        Some(library::Effect::Toast(text)) => {
                            self.show_toast(text);
                            Task::none()
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
            Screen::Main(library) => library.plugins_changed(&self.plugins).map(Message::Library),
            _ => Task::none(),
        }
    }

    fn apply_prefs(&mut self, prefs: Prefs) {
        self.sidebar.go_mut(prefs.sidebar_collapsed, Instant::now());
        if prefs.accent != self.prefs.accent {
            theme::set_accent(prefs.accent);
            self.theme = theme::theme();
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
            save_prefs(self.prefs, self.plugins.clone()),
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

    /// Закрывает окно. Несохранённый токен сначала отзывается, но не дольше
    /// [`REVOKE_ON_CLOSE`]: без сети окно всё равно закроется.
    fn close(&mut self) -> Task<Message> {
        let (Some(token), Ok(http)) = (self.unsaved.take(), self.http()) else {
            return self.with_window(window::close);
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
            window::resize_events().map(|(_, size)| Message::WindowResized(size)),
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
            window::close_requests().map(|_| Message::CloseWindow),
        ];
        let ticking = self.toast_anim.value()
            || match &self.screen {
                Screen::Login(_) => true,
                Screen::Problem(problem) => problem.retry_at.is_some(),
                _ => false,
            };
        if ticking {
            subscriptions.push(iced::time::every(Duration::from_secs(1)).map(Message::Tick));
        }
        let animating = match &self.screen {
            Screen::Starting | Screen::Checking => true,
            Screen::Login(login) => login.animating(),
            Screen::Paywall(paywall) => paywall.checking,
            Screen::Main(library) => {
                library.animating() || library.visualizing(self.prefs.visualizer)
            }
            Screen::Problem(_) => false,
        };
        // Переходам — кадры с частотой экрана, спиннерам и скелетону хватает
        // 30 в секунду. Когда ничего не движется, кадров нет вовсе.
        if self.transitioning() {
            subscriptions.push(window::frames().map(Message::Frame));
        } else if animating {
            subscriptions.push(iced::time::every(Duration::from_millis(33)).map(Message::Frame));
        }
        if let Screen::Main(library) = &self.screen {
            subscriptions.push(library.subscription());
        }
        Subscription::batch(subscriptions)
    }

    fn view(&self) -> Element<'_, Message> {
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
        let layers = layers.push(resize_edges());
        // Где нажали мышь — для меню трека у курсора.
        if matches!(self.screen, Screen::Main(_)) {
            pointer::pointer(layers, |at| {
                Message::Library(library::Message::Plugin(library::PluginMsg::Pointer(at)))
            })
        } else {
            layers.into()
        }
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
async fn save_prefs(prefs: Prefs, plugins: PluginSettings) -> Result<(), Failure> {
    let path = AppDirs::discover()?.config_file();
    storage::blocking(move || {
        let mut config = Config::load(&path)?;
        prefs.write(&mut config);
        plugins.write(&mut config.plugins);
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
