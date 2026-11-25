//! Автозапуск вместе с системой. Состояние берём у самой системы и в
//! `config.toml` не дублируем: Windows — значение в `HKCU\...\Run`, Linux —
//! файл `~/.config/autostart/Plastinka.desktop`, macOS — LaunchAgent в
//! `~/Library/LaunchAgents`. Запускается программа с флагом `--tray`
//! (окно прячется в трей).
//!
//! Вызовы блокирующие (реестр, диск): окно зовёт их через `storage::blocking`.

use auto_launch::{AutoLaunch, AutoLaunchBuilder, MacOSLaunchMode, WindowsEnableMode};

use crate::APP_NAME;

/// Аргументы запуска из автозапуска.
const ARGS: [&str; 1] = ["--tray"];

/// Путь программы в том виде, как его нужно записать в запись автозапуска.
/// В Windows и Linux команда — одна строка, поэтому путь с пробелами берём в
/// кавычки; в macOS путь — отдельный элемент списка, там кавычек быть не
/// должно.
fn command_path(path: &str) -> String {
    if cfg!(target_os = "macos") || path.starts_with('"') {
        path.to_owned()
    } else {
        format!("\"{path}\"")
    }
}

fn launcher() -> Result<AutoLaunch, String> {
    let exe = std::env::current_exe()
        .map_err(|err| format!("не удалось определить путь программы: {err}"))?;
    let path = exe
        .to_str()
        .ok_or_else(|| "путь к программе содержит недопустимые символы".to_owned())?;
    AutoLaunchBuilder::new()
        .set_app_name(APP_NAME)
        .set_app_path(&command_path(path))
        .set_args(&ARGS)
        // Только для текущего пользователя: права администратора не нужны.
        .set_windows_enable_mode(WindowsEnableMode::CurrentUser)
        .set_macos_launch_mode(MacOSLaunchMode::LaunchAgent)
        .build()
        .map_err(|err| err.to_string())
}

/// Включён ли автозапуск в системе.
pub fn is_enabled() -> Result<bool, String> {
    launcher()?
        .is_enabled()
        .map_err(|err| format!("не удалось проверить автозапуск: {err}"))
}

/// Включает или выключает автозапуск.
pub fn set_enabled(enabled: bool) -> Result<(), String> {
    let launcher = launcher()?;
    if enabled {
        launcher
            .enable()
            .map_err(|err| format!("не удалось включить автозапуск: {err}"))
    } else {
        launcher
            .disable()
            .map_err(|err| format!("не удалось выключить автозапуск: {err}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_path_is_quoted_outside_macos() {
        if cfg!(target_os = "macos") {
            assert_eq!(command_path("/a b/Plastinka"), "/a b/Plastinka");
        } else {
            assert_eq!(
                command_path(r"C:\Users\Anna Ivanova\Plastinka.exe"),
                r#""C:\Users\Anna Ivanova\Plastinka.exe""#
            );
            // Уже в кавычках — не удваиваем.
            assert_eq!(command_path(r#""C:\a\P.exe""#), r#""C:\a\P.exe""#);
        }
    }

    #[test]
    fn launcher_runs_hidden_in_tray() {
        let launcher = launcher().expect("сборка без обращения к системе");
        assert_eq!(launcher.get_app_name(), APP_NAME);
        assert_eq!(launcher.get_args(), ["--tray"]);
    }
}
