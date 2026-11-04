use std::io::{self, Write};
use std::process::ExitCode;

use clap::error::{ContextKind, ErrorKind};
use clap::{ArgAction, CommandFactory, Parser, Subcommand};
use plastinka::storage::{AppDirs, StorageError};
use plastinka::{APP_NAME, DISCLAIMER};

/// Шаблон справки: clap сам пишет только «Usage», поэтому подписи задаём явно.
/// Шаблон не наследуется, его нужно указывать у каждой команды.
const HELP_TEMPLATE: &str = "{about}\n\nИспользование: {usage}\n\n{all-args}";

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
    /// О программе и где лежат её данные
    #[command(help_template = HELP_TEMPLATE)]
    About,
}

#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("не удалось вывести текст: {0}")]
    Output(#[from] io::Error),
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => return report_parse_error(&err),
    };
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        // Читатель вывода закрылся раньше времени (например, `| head`): это не ошибка.
        Err(CliError::Output(err)) if err.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
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

    let _ = writeln!(io::stderr(), "Ошибка: {message}\nСправка: plastinka --help");
    ExitCode::from(2)
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

fn run(cli: Cli) -> Result<(), CliError> {
    match cli.command {
        Some(Command::About) => about(),
        // Окна пока нет, поэтому без команды показываем справку.
        None => Ok(Cli::command().print_help()?),
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
