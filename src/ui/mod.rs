//! Окно приложения на iced: вход, «Нужна подписка», главное окно с боковым
//! меню, списком «Мне нравится» и панелью плеера, окно «О программе».
//!
//! Главное окно и плеер в нём создаются только с пропуском
//! [`PlaybackPermit`], то есть после явного подтверждения Плюса.

mod covers;
mod gate;
mod icons;
mod library;
mod marquee;
mod theme;
mod widgets;

use std::sync::Arc;
use std::time::{Duration, Instant};

use iced::widget::{column, container, mouse_area, row, stack};
use iced::window::{self, Direction};
use iced::{Element, Length, Size, Subscription, Task, keyboard, mouse};

use crate::APP_NAME;
use crate::api::{self, AccountStatus, ApiError, PlaybackPermit};
use crate::auth::{self, AuthError, DeviceCode, OAuthApp};
use crate::net::{self, NetError};
use crate::storage::config::Config;
use crate::storage::token::{StoredToken, TokenStore};
use crate::storage::{self, AppDirs, StorageError};
use crate::texts::sentence;

/// Запускает окно. Блокирует поток до закрытия окна.
pub fn run() -> iced::Result {
    let mut app = iced::application(App::boot, App::update, App::view)
        .title(APP_NAME)
        .theme(theme::theme())
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

/// Диалог поверх окна.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialog {
    About,
    Licenses,
    ConfirmLogout,
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

    ConfigLoaded(bool),
    ToggleSidebar,
    SidebarSaved(Result<(), Failure>),

    OpenDialog(Dialog),
    CloseDialog,
    CoverLoaded(String, Option<Vec<u8>>),
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
    /// Боковое меню свёрнуто. Запоминается в `config.toml`.
    sidebar_collapsed: bool,
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
            sidebar_collapsed: false,
            started: now,
            now,
            toast: None,
        };
        let task = Task::batch([
            window::latest().map(Message::WindowOpened),
            Task::perform(load_token(), Message::TokenLoaded),
            // Не прочитались настройки — меню просто развёрнуто.
            Task::perform(load_config(), |config| {
                Message::ConfigLoaded(config.is_ok_and(|config| config.sidebar_collapsed))
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

    fn compact(&self) -> bool {
        self.size.width < COMPACT_WIDTH
    }

    fn update(&mut self, message: Message) -> Task<Message> {
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
            Message::Frame(now) => self.now = now,
            Message::Key(event) => return self.key(event),
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

            Message::ConfigLoaded(collapsed) => self.sidebar_collapsed = collapsed,
            Message::ToggleSidebar => {
                self.sidebar_collapsed = !self.sidebar_collapsed;
                if storage::test_hooks_active() {
                    return Task::none();
                }
                return Task::perform(save_sidebar(self.sidebar_collapsed), Message::SidebarSaved);
            }
            Message::SidebarSaved(result) => {
                if let Err(failure) = result {
                    self.show_toast(format!("Не удалось запомнить вид меню: {}", failure.text));
                }
            }

            Message::OpenDialog(dialog) => self.dialog = Some(dialog),
            Message::CloseDialog => self.dialog = None,
            Message::CoverLoaded(uri, bytes) => {
                let http = self.http.as_ref().ok();
                return self.covers.finished(uri, bytes, http);
            }
            Message::Library(message) => {
                if let Screen::Main(library) = &mut self.screen {
                    let http = self.http.as_ref().ok();
                    let (task, effect) = library.update(message, http);
                    let task = task.map(Message::Library);
                    let covers = self.covers.show(library.visible_covers(), http);
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

    fn show_toast(&mut self, text: String) {
        self.toast = Some((text, Instant::now() + TOAST_FOR));
    }

    fn key(&mut self, event: keyboard::Event) -> Task<Message> {
        if let keyboard::Event::KeyPressed {
            key: keyboard::Key::Named(keyboard::key::Named::Escape),
            ..
        } = event
        {
            self.dialog = None;
        }
        Task::none()
    }

    /// Раз в секунду: отсчёты, повтор без сети, скрытие подсказок.
    fn tick(&mut self, now: Instant) -> Task<Message> {
        if self.toast.as_ref().is_some_and(|(_, until)| now >= *until) {
            self.toast = None;
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
        self.dialog = None;
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
        );
        self.screen = Screen::Main(Box::new(library));
        task.map(Message::Library)
    }

    fn logout(&mut self) -> Task<Message> {
        self.dialog = None;
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
            window::close_requests().map(|_| Message::CloseWindow),
        ];
        let ticking = self.toast.is_some()
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
            Screen::Main(library) => library.animating(),
            Screen::Problem(_) => false,
        };
        if animating {
            subscriptions.push(iced::time::every(Duration::from_millis(33)).map(Message::Frame));
        }
        if let Screen::Main(library) = &self.screen {
            subscriptions.push(library.subscription());
        }
        Subscription::batch(subscriptions)
    }

    fn view(&self) -> Element<'_, Message> {
        let turn = self.turn();
        let body: Element<'_, Message> = match &self.screen {
            Screen::Starting => gate::loading("Открываем…", turn),
            Screen::Checking => gate::loading("Проверяем подписку…", turn),
            Screen::Problem(problem) => gate::problem(problem, self.now),
            Screen::Login(login) => gate::login(login, self.now, turn),
            Screen::Paywall(paywall) => gate::paywall(paywall, turn),
            Screen::Main(library) => {
                return self.decorate(library.view(
                    &self.covers,
                    self.sidebar_collapsed,
                    self.compact(),
                    self.size.width,
                    turn,
                ));
            }
        };
        self.decorate(gate::frame(body))
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
        if let Some((text, _)) = &self.toast {
            // В главном окне — по центру плеера, а не всего окна.
            let insets = match self.screen {
                Screen::Main(_) => library::player_insets(self.sidebar_collapsed),
                _ => (0.0, 0.0),
            };
            layers = layers.push(gate::toast(text, insets));
        }
        if let Some(dialog) = self.dialog {
            layers = layers.push(gate::dialog(dialog));
        }
        layers.push(resize_edges()).into()
    }
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

/// Запоминает, свёрнуто ли меню. Остальные настройки не трогает.
async fn save_sidebar(collapsed: bool) -> Result<(), Failure> {
    let path = AppDirs::discover()?.config_file();
    storage::blocking(move || {
        let mut config = Config::load(&path)?;
        config.sidebar_collapsed = collapsed;
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
