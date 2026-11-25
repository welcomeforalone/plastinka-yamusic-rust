// В релизе под Windows окно открывается без консоли. Команды из терминала
// (`Plastinka login` и т. п.) подключаются к консоли родителя, см. `attach_console`.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::fmt::Display;
use std::io::{self, Write};
use std::process::ExitCode;
use std::time::Duration;

use clap::error::{ContextKind, ErrorKind};
use clap::{ArgAction, CommandFactory, Parser, Subcommand};
use plastinka::api::{self, ApiError, Subscription};
use plastinka::auth::{self, AuthError, OAuthApp};
use plastinka::instance;
use plastinka::net::{self, NetError};
use plastinka::storage::config::Config;
use plastinka::storage::token::TokenStore;
use plastinka::storage::{self, AppDirs, StorageError};
use plastinka::texts::tracks_word;
use plastinka::{APP_NAME, DISCLAIMER};

/// Подключает вывод к консоли терминала, из которого запущена команда.
///
/// Релиз под Windows собран как оконное приложение: своей консоли у него нет,
/// и без этого `--help`, `login` и другие команды ничего бы не вывели. Если
/// родитель без консоли (запуск из проводника) или вывод уже перенаправлен в
/// файл, вызов ничего не меняет.
#[cfg(all(windows, not(debug_assertions)))]
#[allow(unsafe_code)]
fn attach_console() {
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    // SAFETY: функция без указателей; при ошибке просто возвращает 0, и
    // приложение работает дальше без консоли.
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

/// В отладке и вне Windows своя консоль уже есть.
#[cfg(not(all(windows, not(debug_assertions))))]
fn attach_console() {}

/// Подсказка про другой бэкенд wgpu: выбор есть только в Linux (Vulkan и
/// OpenGL); в Windows собран один DX12, в macOS — Metal.
#[cfg(all(unix, not(target_os = "macos")))]
const BACKEND_HINT: &str =
    "Можно также попробовать другой бэкенд: переменная окружения WGPU_BACKEND=gl (или vulkan). ";
#[cfg(not(all(unix, not(target_os = "macos"))))]
const BACKEND_HINT: &str = "";

/// Текст ошибки запуска окна. Рисует только видеокарта (wgpu), запасного
/// программного рендера нет, поэтому сбой отрисовки объясняем отдельно.
fn startup_error_text(err: &iced::Error) -> String {
    match err {
        iced::Error::GraphicsCreationFailed(reason) => format!(
            "{}{}Подробности: {reason}",
            "Ошибка: не удалось запустить отрисовку на видеокарте. Обновите драйвер видеокарты. ",
            BACKEND_HINT
        ),
        other => format!("Ошибка: не удалось открыть окно: {other}"),
    }
}

/// Показывает ошибку запуска окна: в релизе под Windows консоли нет, и текст
/// в stderr пользователь не увидит.
#[cfg(all(windows, not(debug_assertions)))]
fn show_startup_error(text: &str) {
    let _ = rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title(APP_NAME)
        .set_description(text)
        .show();
}

#[cfg(not(all(windows, not(debug_assertions))))]
fn show_startup_error(_text: &str) {}

/// Шаблон справки: clap сам пишет только «Usage», поэтому подписи задаём явно.
/// Шаблон не наследуется, его нужно указывать у каждой команды.
const HELP_TEMPLATE: &str = "{about}\n\nИспользование: {usage}\n\n{all-args}";

/// Где пользователь может отозвать доступ вручную.
const DATA_ACCESS_URL: &str = "https://id.yandex.ru/personal/data-access";

/// Где оформить подписку.
const PLUS_URL: &str = "https://plus.yandex.ru";

/// Неофициальный клиент Яндекс Музыки: только потоковое воспроизведение.
#[derive(Debug, Parser)]
#[command(
    name = APP_NAME,
    bin_name = "Plastinka",
    version,
    disable_help_flag = true,
    disable_version_flag = true,
    disable_help_subcommand = true,
    subcommand_help_heading = "Команды",
    subcommand_value_name = "КОМАНДА",
    next_help_heading = "Параметры",
    help_template = HELP_TEMPLATE
)]
struct Cli {
    /// Показать справку
    #[arg(short = 'h', long = "help", action = ArgAction::Help, global = true)]
    _help: Option<bool>,

    /// Показать версию
    #[arg(short = 'V', long = "version", action = ArgAction::Version)]
    _version: Option<bool>,

    /// Служебный флаг: программа перезапущена после обновления
    #[arg(long = "after-update", hide = true)]
    after_update: bool,

    /// Запустить свёрнутым в трей
    #[arg(long = "tray")]
    tray: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Войти по коду устройства
    #[command(help_template = HELP_TEMPLATE)]
    Login,
    /// Выйти: удалить токен с этого компьютера и отозвать его
    #[command(help_template = HELP_TEMPLATE)]
    Logout,
    /// Показать, под каким аккаунтом выполнен вход и есть ли подписка Плюс
    #[command(help_template = HELP_TEMPLATE)]
    Account,
    /// Показать треки из «Мне нравится»
    #[command(help_template = HELP_TEMPLATE)]
    Likes,
    /// О программе и где лежат её данные
    #[command(help_template = HELP_TEMPLATE)]
    About,
}

#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error(transparent)]
    Api(#[from] ApiError),
    #[error(transparent)]
    Net(#[from] NetError),
    #[error("вход не выполнен. Войдите командой: Plastinka login")]
    NotLoggedIn,
    #[error("токен больше не действует (отозван или истёк). Войдите заново: Plastinka login")]
    TokenExpired,
    #[error("Яндекс выдал токен, но сервис музыки его не принимает. Токен не сохранён")]
    TokenRejected,
    #[error("{0}. Подписка не проверена")]
    StatusUnknown(ApiError),
    #[error("вход отменён")]
    Cancelled,
    #[error("загрузка отменена")]
    LoadCancelled,
    #[error("не удалось запустить асинхронную среду: {0}")]
    Runtime(io::Error),
    #[error("не удалось вывести текст: {0}")]
    Output(#[from] io::Error),
}

fn main() -> ExitCode {
    // Есть аргументы — это команда (или `--help`): вывод нужен в терминале.
    if std::env::args_os().len() > 1 {
        attach_console();
    }
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => return report_parse_error(&err),
    };
    // Без команды открывается окно. У iced свой рантайм tokio.
    if cli.command.is_none() {
        // Один экземпляр: второй запуск просит первый показать окно и выходит.
        // Запуск из автозапуска (`--tray`) окно не показывает.
        // После обновления старый процесс ещё выходит: ждём его блокировку.
        if instance::claim(cli.after_update) == instance::Claim::Running {
            if !cli.tray {
                instance::ask_to_show();
            }
            return ExitCode::SUCCESS;
        }
        return match plastinka::ui::run(plastinka::ui::Launch { tray: cli.tray }) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                let text = startup_error_text(&err);
                let _ = writeln!(io::stderr(), "{text}");
                show_startup_error(&text);
                ExitCode::FAILURE
            }
        };
    }
    // `#[tokio::main]` паникует, если среда не запустилась, поэтому собираем её сами.
    let result = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => {
            let result = runtime.block_on(run(cli));
            // Не ждём бесконечно фоновые операции, брошенные после отмены.
            runtime.shutdown_timeout(Duration::from_secs(2));
            result
        }
        Err(err) => Err(CliError::Runtime(err)),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        // Читатель вывода закрылся раньше времени (например, `| head`): это не ошибка.
        Err(CliError::Output(err)) if err.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(CliError::Cancelled) => {
            let _ = writeln!(io::stderr(), "Вход отменён.");
            ExitCode::from(130)
        }
        Err(CliError::LoadCancelled) => {
            let _ = writeln!(io::stderr(), "Загрузка отменена.");
            ExitCode::from(130)
        }
        Err(err) => {
            // Если не удалось написать даже в stderr, сообщить об ошибке уже некуда.
            let _ = writeln!(io::stderr(), "Ошибка: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Справку и версию печатает clap, ошибки разбора аргументов — по-русски.
fn report_parse_error(err: &clap::Error) -> ExitCode {
    if matches!(
        err.kind(),
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
    ) {
        // Если stdout уже закрыт, печатать справку некуда, но это не ошибка.
        let _ = err.print();
        return ExitCode::SUCCESS;
    }

    let context = |kind| err.get(kind).map(ToString::to_string).unwrap_or_default();
    let mut message = match err.kind() {
        ErrorKind::InvalidSubcommand => format!(
            "неизвестная команда {}",
            shown(&context(ContextKind::InvalidSubcommand))
        ),
        ErrorKind::UnknownArgument => {
            let arg = context(ContextKind::InvalidArg);
            if arg.starts_with('-') {
                format!("неизвестный параметр {}", shown(&arg))
            } else {
                format!("лишний аргумент {}", shown(&arg))
            }
        }
        ErrorKind::InvalidValue | ErrorKind::ValueValidation => format!(
            "неверное значение {} у параметра {}",
            shown(&context(ContextKind::InvalidValue)),
            context(ContextKind::InvalidArg)
        ),
        ErrorKind::MissingRequiredArgument => format!(
            "не хватает обязательных параметров: {}",
            context(ContextKind::InvalidArg)
        ),
        _ => "неверные аргументы командной строки".to_owned(),
    };
    let suggestion = err
        .get(ContextKind::SuggestedSubcommand)
        .or_else(|| err.get(ContextKind::SuggestedArg));
    if let Some(suggestion) = suggestion {
        message.push_str(&format!(
            ". Возможно, имелось в виду {}",
            shown(&suggestion.to_string())
        ));
    }

    let _ = writeln!(io::stderr(), "Ошибка: {message}\nСправка: {}", help_hint());
    ExitCode::from(2)
}

/// Как открыть справку: для ошибки в команде — справку этой команды.
fn help_hint() -> String {
    std::env::args_os()
        .nth(1)
        .and_then(|arg| arg.into_string().ok())
        .filter(|arg| Cli::command().find_subcommand(arg).is_some())
        .map_or_else(
            || "Plastinka --help".to_owned(),
            |command| format!("Plastinka {command} --help"),
        )
}

/// Значение из командной строки для сообщения: в кавычках, пустое подписано
/// словами, слишком длинное обрезано.
fn shown(value: &str) -> String {
    const MAX_CHARS: usize = 80;
    if value.is_empty() {
        return "(пустая строка)".to_owned();
    }
    let mut chars = value.chars();
    let head: String = chars.by_ref().take(MAX_CHARS).collect();
    if chars.next().is_some() {
        format!("«{head}…»")
    } else {
        format!("«{head}»")
    }
}

async fn run(cli: Cli) -> Result<(), CliError> {
    match cli.command {
        Some(Command::Login) => login().await,
        Some(Command::Logout) => logout().await,
        Some(Command::Account) => account().await,
        Some(Command::Likes) => likes().await,
        Some(Command::About) => about(),
        // Без команды открывается окно, сюда не доходим.
        None => Ok(()),
    }
}

/// Строка в stdout. Блокировку не держим между вызовами.
fn say(text: impl Display) -> Result<(), CliError> {
    writeln!(io::stdout(), "{text}")?;
    Ok(())
}

/// Предупреждение в stderr. Если вывести его не удалось, работу не прерываем.
fn warn(text: impl Display) {
    let _ = writeln!(io::stderr(), "Предупреждение: {text}");
}

async fn login() -> Result<(), CliError> {
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
    // Хранилище проверяем до входа: без него незачем просить пользователя
    // вводить код. Пробное чтение заранее вызывает запрос на разблокировку.
    let store = storage::blocking(|| {
        let store = TokenStore::open()?;
        match store.load() {
            // Повреждённую запись вход перезапишет.
            Ok(_) | Err(StorageError::BadStoredToken) => Ok(store),
            Err(err) => Err(err),
        }
    })
    .await?;
    let app = OAuthApp::from_config(&config);
    let http = net::http_client()?;

    // Обработчик Ctrl+C, раз поставленный, перехватывает сигнал до конца
    // работы, поэтому отмена нужна на каждом долгом шаге.
    let token = tokio::select! {
        result = obtain_token(&http, &app, &device_id) => result?,
        () = ctrl_c() => return Err(CliError::Cancelled),
    };

    let checked = tokio::select! {
        result = api::fetch_status(&http, &token.access_token) => result,
        () = ctrl_c() => {
            discard_token(&http, &app, &token.access_token).await;
            return Err(CliError::Cancelled);
        }
    };
    let status = match checked {
        Ok(status) => Some(status),
        Err(ApiError::Unauthorized) => {
            discard_token(&http, &app, &token.access_token).await;
            return Err(CliError::TokenRejected);
        }
        Err(err) => {
            warn(format_args!(
                "не удалось проверить токен и подписку: {err}. Сохраняю токен, проверить можно командой Plastinka account"
            ));
            None
        }
    };

    // Сохранение по Ctrl+C не прерываем: запись в хранилище идёт в отдельном
    // потоке и всё равно завершится.
    let stored = token.into_stored();
    let access_token = stored.access_token.clone();
    if let Err(err) = storage::blocking(move || store.save(&stored)).await {
        discard_token(&http, &app, &access_token).await;
        return Err(err.into());
    }
    match status {
        Some(status) => {
            say(format_args!(
                "Вход выполнен: {}. Токен сохранён в системном хранилище паролей.",
                status.account().name()
            ))?;
            say(subscription_text(status.subscription()))
        }
        None => say("Токен сохранён в системном хранилище паролей."),
    }
}

/// Запрашивает код, показывает его и ждёт, пока пользователь подтвердит вход.
async fn obtain_token(
    http: &reqwest::Client,
    app: &OAuthApp,
    device_id: &str,
) -> Result<auth::TokenResponse, CliError> {
    let code = auth::request_device_code(http, app, device_id).await?;
    say(format_args!(
        "Откройте страницу {} и введите код:\n\n    {}\n\n\
         Код действует {} мин. Жду подтверждения (Ctrl+C — отмена)…",
        code.verification_url,
        code.user_code,
        code.expires_in.as_secs().div_ceil(60)
    ))?;
    Ok(auth::wait_for_token(http, app, &code).await?)
}

/// Отзывает полученный, но не сохранённый токен, чтобы он не остался
/// действующим. Результат не важен: токен нигде не записан. Повторный
/// Ctrl+C прерывает и отзыв.
async fn discard_token(http: &reqwest::Client, app: &OAuthApp, access_token: &str) {
    tokio::select! {
        _ = auth::revoke(http, app, access_token) => {}
        () = ctrl_c() => {}
    }
}

async fn logout() -> Result<(), CliError> {
    let (loaded, deleted) = storage::blocking(|| {
        let store = TokenStore::open()?;
        // Удаляем и то, что прочитать не удалось: повреждённую запись, дубли.
        let loaded = store.load();
        Ok((loaded, store.delete()?))
    })
    .await?;
    if deleted {
        // Сбой вывода не должен помешать отзыву.
        let _ = say("Токен удалён с этого компьютера.");
    }
    let token = match loaded {
        Ok(Some(token)) => token,
        Ok(None) if deleted => return Ok(()),
        Ok(None) => return say("Вход не выполнен, выходить не из чего."),
        Err(err) if !deleted => return Err(err.into()),
        Err(_) => {
            warn(format_args!(
                "сохранённый токен не удалось прочитать, поэтому на сервере он не отозван. \
                 Отозвать доступ можно вручную: {DATA_ACCESS_URL}"
            ));
            return Ok(());
        }
    };

    // Битые настройки не мешают выходу: тогда отзываем от имени приложения
    // по умолчанию.
    let config = match load_config().await {
        Ok(config) => config,
        Err(err) => {
            warn(format_args!(
                "{err}. Отзываю токен от имени приложения по умолчанию"
            ));
            Config::default()
        }
    };
    let app = OAuthApp::from_config(&config);
    let revoked = match net::http_client() {
        Ok(http) => auth::revoke(&http, &app, &token.access_token).await,
        Err(err) => Err(err.into()),
    };
    match revoked {
        Ok(()) => say("Токен отозван на сервере Яндекса."),
        // Локальный выход уже выполнен, поэтому это предупреждение, а не ошибка.
        Err(err) => {
            warn(format_args!(
                "отозвать токен на сервере не удалось: {err}. \
                 Отозвать доступ можно вручную: {DATA_ACCESS_URL}"
            ));
            Ok(())
        }
    }
}

async fn load_config() -> Result<Config, StorageError> {
    let path = AppDirs::discover()?.config_file();
    storage::blocking(move || Config::load(&path)).await
}

async fn account() -> Result<(), CliError> {
    let token = storage::blocking(|| TokenStore::open()?.load())
        .await?
        .ok_or(CliError::NotLoggedIn)?;
    let http = net::http_client()?;
    // Ошибка запроса — это «проверить не удалось», а не «подписки нет».
    let status = match api::fetch_status(&http, &token.access_token).await {
        Ok(status) => status,
        Err(ApiError::Unauthorized) => return Err(CliError::TokenExpired),
        Err(err) => return Err(CliError::StatusUnknown(err)),
    };
    say(format_args!("Вход выполнен: {}", status.account().name()))?;
    say(subscription_text(status.subscription()))
}

async fn likes() -> Result<(), CliError> {
    let token = storage::blocking(|| TokenStore::open()?.load())
        .await?
        .ok_or(CliError::NotLoggedIn)?;
    let http = net::http_client()?;
    tokio::select! {
        result = show_likes(&http, &token.access_token) => result,
        () = ctrl_c() => Err(CliError::LoadCancelled),
    }
}

async fn show_likes(http: &reqwest::Client, token: &str) -> Result<(), CliError> {
    let status = api::fetch_status(http, token).await.map_err(token_error)?;
    let liked = api::fetch_liked_ids(http, token, status.account().uid)
        .await
        .map_err(token_error)?;
    if liked.unreadable > 0 {
        warn(format_args!(
            "часть списка сервис прислал в непонятном виде, пропущено: {}",
            liked.unreadable
        ));
    }
    let total = liked.ids.len();
    if total == 0 {
        return say("В «Мне нравится» пока пусто.");
    }

    // Прогресс — в stderr, чтобы не мешать выводу списка в файл. Только для
    // больших списков и не чаще раза на пачку.
    let mut shown = 0;
    let tracks = api::fetch_tracks(http, token, &liked.ids, |done| {
        if total > api::TRACKS_BATCH && done < total && done >= shown + api::TRACKS_BATCH {
            shown = done;
            let _ = writeln!(io::stderr(), "Загружено треков: {done} из {total}…");
        }
    })
    .await
    .map_err(token_error)?;

    let unavailable = tracks.iter().filter(|track| !track.available).count();
    let mut out = io::stdout().lock();
    writeln!(out, "Мне нравится: {total} {}", tracks_word(total))?;
    if status.playback_permit().is_none() {
        writeln!(out, "{}", subscription_text(status.subscription()))?;
    }
    let width = total.to_string().len();
    for (number, track) in (1..).zip(&tracks) {
        // Заглушка: сервис ничего не рассказал о треке.
        if track.title.is_none() && track.artists.is_empty() {
            writeln!(
                out,
                "{number:>width$}. нет данных о треке {}  [недоступен]",
                track.id
            )?;
            continue;
        }
        let mark = if track.available {
            ""
        } else {
            "  [недоступен]"
        };
        writeln!(
            out,
            "{number:>width$}. {} — {}  {}{mark}",
            track.artists_text(),
            track.display_title(),
            track.duration_text()
        )?;
    }
    if unavailable > 0 {
        writeln!(
            out,
            "Недоступно: {unavailable} {}. Такие треки нельзя послушать: их убрали из каталога или они недоступны в вашем регионе.",
            tracks_word(unavailable)
        )?;
    }
    Ok(())
}

/// 401 и 403 значат, что токен больше не действует.
fn token_error(err: ApiError) -> CliError {
    match err {
        ApiError::Unauthorized => CliError::TokenExpired,
        err => err.into(),
    }
}

/// Что сказать пользователю о подписке.
fn subscription_text(subscription: &Subscription) -> String {
    match subscription {
        Subscription::Active(details) => {
            let mut text = "Подписка Плюс активна".to_owned();
            if details.family {
                text.push_str(" (семейная)");
            }
            if let Some(until) = details.until {
                text.push_str(&format!(" до {until}"));
            }
            match details.auto_renewal {
                Some(true) => text.push_str(", продлевается автоматически"),
                Some(false) => text.push_str(", без автопродления"),
                None => {}
            }
            text.push('.');
            text
        }
        Subscription::Missing { can_start_trial } => {
            let trial = if *can_start_trial {
                " Для аккаунта доступен пробный период."
            } else {
                ""
            };
            format!(
                "Нужна подписка Плюс: без неё {APP_NAME} не воспроизводит музыку.{trial}\n\
                 Оформите подписку на {PLUS_URL} и проверьте снова: Plastinka account"
            )
        }
        Subscription::RegionUnavailable { region_code } => {
            let region = region_code
                .as_deref()
                .map(|code| format!(" (код региона {code})"))
                .unwrap_or_default();
            format!(
                "Сервис музыки недоступен в регионе аккаунта{region}. \
                 Воспроизведение невозможно, даже если подписка Плюс есть."
            )
        }
        Subscription::Unconfirmed => "Подписку Плюс подтвердить не удалось: сервис не прислал \
                                      сведений о ней. Пока подписка не подтверждена, \
                                      воспроизведение недоступно. Повторите позже: \
                                      Plastinka account"
            .to_owned(),
    }
}

/// Ждёт Ctrl+C. Если обработчик поставить не удалось, просто никогда не
/// завершается, чтобы не отменить вход по ошибке.
async fn ctrl_c() {
    if tokio::signal::ctrl_c().await.is_err() {
        std::future::pending::<()>().await;
    }
}

fn about() -> Result<(), CliError> {
    let dirs = AppDirs::discover()?;
    let mut out = io::stdout().lock();
    writeln!(out, "{APP_NAME} {}", env!("CARGO_PKG_VERSION"))?;
    writeln!(out, "{DISCLAIMER}")?;
    writeln!(
        out,
        "Только потоковое воспроизведение, нужна активная подписка Плюс."
    )?;
    writeln!(out)?;
    writeln!(out, "Настройки: {}", dirs.config_dir().display())?;
    writeln!(
        out,
        "Кэш метаданных и обложек: {}",
        dirs.cache_dir().display()
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn shown_marks_empty_and_truncates_long_values() {
        assert_eq!(shown(""), "(пустая строка)");
        assert_eq!(shown("about"), "«about»");
        let long = "я".repeat(200);
        assert_eq!(shown(&long), format!("«{}…»", "я".repeat(80)));
    }
}
