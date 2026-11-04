use std::fmt::Display;
use std::io::{self, Write};
use std::process::ExitCode;
use std::time::Duration;

use clap::error::{ContextKind, ErrorKind};
use clap::{ArgAction, CommandFactory, Parser, Subcommand};
use plastinka::api::{self, ApiError};
use plastinka::auth::{self, AuthError, OAuthApp};
use plastinka::net::{self, NetError};
use plastinka::storage::config::Config;
use plastinka::storage::token::TokenStore;
use plastinka::storage::{self, AppDirs, StorageError};
use plastinka::{APP_NAME, DISCLAIMER};

/// Шаблон справки: clap сам пишет только «Usage», поэтому подписи задаём явно.
/// Шаблон не наследуется, его нужно указывать у каждой команды.
const HELP_TEMPLATE: &str = "{about}\n\nИспользование: {usage}\n\n{all-args}";

/// Где пользователь может отозвать доступ вручную.
const DATA_ACCESS_URL: &str = "https://id.yandex.ru/personal/data-access";

/// Неофициальный клиент Яндекс Музыки: только потоковое воспроизведение.
#[derive(Debug, Parser)]
#[command(
    name = APP_NAME,
    bin_name = "plastinka",
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
    /// Показать, под каким аккаунтом выполнен вход
    #[command(help_template = HELP_TEMPLATE)]
    Account,
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
    #[error("вход не выполнен. Войдите командой: plastinka login")]
    NotLoggedIn,
    #[error("токен больше не действует (отозван или истёк). Войдите заново: plastinka login")]
    TokenExpired,
    #[error("Яндекс выдал токен, но сервис музыки его не принимает. Токен не сохранён")]
    TokenRejected,
    #[error("вход отменён")]
    Cancelled,
    #[error("не удалось запустить асинхронную среду: {0}")]
    Runtime(io::Error),
    #[error("не удалось вывести текст: {0}")]
    Output(#[from] io::Error),
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => return report_parse_error(&err),
    };
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
            || "plastinka --help".to_owned(),
            |command| format!("plastinka {command} --help"),
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
        Some(Command::About) => about(),
        // Окна пока нет, поэтому без команды показываем справку.
        None => Ok(Cli::command().print_help()?),
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
        result = api::fetch_account(&http, &token.access_token) => result,
        () = ctrl_c() => {
            discard_token(&http, &app, &token.access_token).await;
            return Err(CliError::Cancelled);
        }
    };
    let account = match checked {
        Ok(account) => Some(account),
        Err(ApiError::Unauthorized) => {
            discard_token(&http, &app, &token.access_token).await;
            return Err(CliError::TokenRejected);
        }
        Err(err) => {
            warn(format_args!(
                "не удалось проверить токен: {err}. Сохраняю его, проверить можно командой plastinka account"
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
    match account {
        Some(account) => say(format_args!(
            "Вход выполнен: {}. Токен сохранён в системном хранилище паролей.",
            account.name()
        )),
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
    match api::fetch_account(&http, &token.access_token).await {
        Ok(account) => say(format_args!("Вход выполнен: {}", account.name())),
        Err(ApiError::Unauthorized) => Err(CliError::TokenExpired),
        Err(err) => Err(err.into()),
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
