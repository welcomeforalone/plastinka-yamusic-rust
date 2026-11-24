//! Состояние обновления для «Настройки → О программе»: чистая логика без окна.

use std::path::PathBuf;

use crate::update::{Available, Check, Installed, Package};

/// Что сейчас с обновлением. В `config.toml` не пишется.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum State {
    #[default]
    Idle,
    Checking,
    /// Новее ничего нет; внутри — версия этой сборки.
    UpToDate(String),
    Available(Available),
    Downloading(Available),
    /// Файл заменён; внутри — куда записана новая версия.
    Installed(PathBuf),
    /// Скачано и проверено, замена не выполнялась (отладочный хук).
    Verified,
    Failed(String),
}

impl State {
    /// Идёт проверка или установка: новые не начинаем.
    pub fn busy(&self) -> bool {
        matches!(
            self,
            Self::Checking | Self::Downloading(_) | Self::Installed(_)
        )
    }

    /// Кнопка «Проверить обновления». `false` — уже что-то идёт.
    pub fn start_check(&mut self) -> bool {
        if self.busy() {
            return false;
        }
        *self = Self::Checking;
        true
    }

    /// Итог проверки. `manual` — по кнопке: результат и ошибка показываются
    /// всегда. Автоматическая проверка молча ждёт своей очереди, если уже
    /// что-то идёт, и ошибок не показывает. Возвращает версию, о которой стоит
    /// сообщить всплывающим сообщением (только для автоматической проверки).
    pub fn finish_check(&mut self, result: Result<Check, String>, manual: bool) -> Option<String> {
        if manual {
            if *self != Self::Checking {
                return None;
            }
        } else if self.busy() {
            return None;
        }
        match result {
            Ok(Check::UpToDate(version)) => {
                *self = Self::UpToDate(version);
                None
            }
            Ok(Check::Available(found)) => {
                let version = found.version.clone();
                *self = Self::Available(found);
                (!manual).then_some(version)
            }
            Err(text) => {
                if manual {
                    *self = Self::Failed(text);
                }
                None
            }
        }
    }

    /// Кнопка «Обновить»: что скачивать. `None` — скачивать нечего.
    pub fn start_install(&mut self) -> Option<Package> {
        let Self::Available(found) = self else {
            return None;
        };
        let package = found.package.clone()?;
        *self = Self::Downloading(found.clone());
        Some(package)
    }

    /// Итог скачивания и замены.
    pub fn installed(&mut self, result: Result<Installed, String>) {
        if !matches!(self, Self::Downloading(_)) {
            return;
        }
        *self = match result {
            Ok(Installed::Replaced { exe }) => Self::Installed(exe),
            Ok(Installed::Verified) => Self::Verified,
            Err(text) => Self::Failed(text),
        };
    }

    /// Куда перезапускаться.
    pub fn restart_target(&self) -> Option<PathBuf> {
        match self {
            Self::Installed(exe) => Some(exe.clone()),
            _ => None,
        }
    }

    /// Строка состояния.
    pub fn status(&self) -> String {
        match self {
            Self::Idle => "Проверьте, нет ли новой версии".to_owned(),
            Self::Checking => "Проверяем…".to_owned(),
            Self::UpToDate(version) => format!("У вас последняя версия {version}"),
            Self::Available(found) => format!("Доступна версия {}", found.version),
            Self::Downloading(_) => "Скачиваем…".to_owned(),
            Self::Installed(_) => "Обновление установлено. Перезапустите Plastinka".to_owned(),
            Self::Verified => "Файл скачан и проверен, замена не выполнялась (отладка)".to_owned(),
            Self::Failed(text) => text.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(with_package: bool) -> Available {
        Available {
            version: "0.0.6".to_owned(),
            page_url: "https://github.com/o/r/releases/tag/v0.0.6".to_owned(),
            package: with_package.then(|| Package {
                name: "Plastinka.exe".to_owned(),
                url: "https://github.com/o/r/Plastinka.exe".to_owned(),
                sha_url: "https://github.com/o/r/Plastinka.exe.sha256".to_owned(),
            }),
        }
    }

    #[test]
    fn manual_check_shows_result_and_errors() {
        let mut state = State::default();
        assert!(state.start_check());
        assert!(!state.start_check());
        assert_eq!(state.status(), "Проверяем…");
        assert_eq!(state.finish_check(Err("нет сети".to_owned()), true), None);
        assert_eq!(state, State::Failed("нет сети".to_owned()));
        assert!(state.start_check());
        let toast = state.finish_check(Ok(Check::Available(found(true))), true);
        assert_eq!(toast, None);
        assert_eq!(state.status(), "Доступна версия 0.0.6");
    }

    #[test]
    fn auto_check_is_quiet_and_toasts_once_per_find() {
        let mut state = State::default();
        // Ошибка автопроверки ничего не показывает.
        assert_eq!(state.finish_check(Err("нет сети".to_owned()), false), None);
        assert_eq!(state, State::Idle);
        let toast = state.finish_check(Ok(Check::Available(found(false))), false);
        assert_eq!(toast.as_deref(), Some("0.0.6"));
        assert!(matches!(state, State::Available(_)));
        // Во время ручной проверки результат автоматической не вмешивается.
        let mut busy = State::Checking;
        assert_eq!(
            busy.finish_check(Ok(Check::UpToDate("0.0.5".to_owned())), false),
            None
        );
        assert_eq!(busy, State::Checking);
    }

    #[test]
    fn install_flow_and_failure() {
        let mut state = State::Available(found(true));
        let package = state.start_install().expect("файл есть");
        assert_eq!(package.name, "Plastinka.exe");
        assert!(matches!(state, State::Downloading(_)));
        assert!(state.busy());
        state.installed(Ok(Installed::Replaced {
            exe: PathBuf::from("Plastinka.exe"),
        }));
        assert_eq!(state.restart_target(), Some(PathBuf::from("Plastinka.exe")));

        let mut failed = State::Available(found(true));
        assert!(failed.start_install().is_some());
        failed.installed(Err("Обновление не установлено".to_owned()));
        assert_eq!(failed.status(), "Обновление не установлено");
        assert_eq!(failed.restart_target(), None);
    }

    #[test]
    fn install_needs_a_package() {
        let mut state = State::Available(found(false));
        assert!(state.start_install().is_none());
        assert!(matches!(state, State::Available(_)));
        assert!(State::Idle.clone().start_install().is_none());
    }
}
