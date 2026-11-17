//! Локальные данные: каталоги настроек и кэша, токен в системном
//! хранилище секретов. В кэш попадают только метаданные и обложки.

pub mod config;
pub mod token;

use std::io;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;

use crate::APP_NAME;

/// Ошибки работы с локальными данными.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("не удалось определить домашний каталог пользователя")]
    NoHomeDir,
    #[error("не удалось прочитать или записать настройки {}: {source}", .path.display())]
    ConfigIo {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    // Текст строки с ошибкой не выводим: там может быть свой client_secret.
    #[error("файл настроек {} повреждён{}: {message}", .path.display(), at_line(.line))]
    ConfigParse {
        path: PathBuf,
        line: Option<usize>,
        message: String,
    },
    #[error("не удалось подготовить настройки к сохранению: {0}")]
    ConfigSerialize(#[from] toml::ser::Error),
    #[error("системное хранилище паролей недоступно ({0}). {hint}", hint = token::UNAVAILABLE_HINT)]
    SecretStoreUnavailable(String),
    #[error("нет доступа к системному хранилищу паролей. Разблокируйте его и повторите")]
    SecretStoreLocked,
    #[error("сохранённый токен повреждён. Войдите заново: Plastinka login")]
    BadStoredToken,
    #[error(
        "в хранилище паролей несколько записей Plastinka. Удалите их командой Plastinka logout"
    )]
    SecretStoreDuplicates,
    #[error("ошибка системного хранилища паролей: {0}")]
    SecretStore(String),
    #[error("фоновая операция с локальными данными прервалась")]
    Interrupted,
}

/// Отладочный запуск с фальшивым токеном (`PLASTINKA_TEST_TOKEN`). Окно тогда
/// не пишет в `config.toml`, и проверки не меняют настройки пользователя.
pub fn test_hooks_active() -> bool {
    cfg!(debug_assertions)
        && std::env::var("PLASTINKA_TEST_TOKEN").is_ok_and(|value| !value.trim().is_empty())
}

fn at_line(line: &Option<usize>) -> String {
    line.map(|line| format!(" в строке {line}"))
        .unwrap_or_default()
}

/// Каталоги приложения в стандартных для ОС местах.
#[derive(Debug, Clone)]
pub struct AppDirs {
    config: PathBuf,
    cache: PathBuf,
}

impl AppDirs {
    /// Находит каталоги, но не создаёт их.
    pub fn discover() -> Result<Self, StorageError> {
        let dirs = ProjectDirs::from("", "", APP_NAME).ok_or(StorageError::NoHomeDir)?;
        Ok(Self {
            config: dirs.config_dir().to_path_buf(),
            cache: dirs.cache_dir().to_path_buf(),
        })
    }

    /// Каталог настроек.
    pub fn config_dir(&self) -> &Path {
        &self.config
    }

    /// Файл настроек `config.toml`.
    pub fn config_file(&self) -> PathBuf {
        self.config.join("config.toml")
    }

    /// Каталог кэша метаданных и обложек.
    pub fn cache_dir(&self) -> &Path {
        &self.cache
    }
}

/// Выполняет блокирующую работу с диском или хранилищем паролей вне рабочих
/// потоков tokio.
pub async fn blocking<T, F>(work: F) -> Result<T, StorageError>
where
    F: FnOnce() -> Result<T, StorageError> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| StorageError::Interrupted)?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirs_are_named_after_app() {
        let dirs = AppDirs::discover().expect("каталоги должны находиться");
        let name = APP_NAME.to_lowercase();
        for path in [dirs.config_dir(), dirs.cache_dir()] {
            let path = path.to_string_lossy().to_lowercase();
            assert!(path.contains(&name), "в пути нет имени приложения: {path}");
        }
        assert!(dirs.config_file().starts_with(dirs.config_dir()));
    }
}
