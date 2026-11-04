//! Локальные данные: каталоги настроек и кэша, токен в системном
//! хранилище секретов. В кэш попадают только метаданные и обложки.

use std::path::{Path, PathBuf};

use directories::ProjectDirs;

use crate::APP_NAME;

/// Ошибки работы с локальными данными.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("не удалось определить домашний каталог пользователя")]
    NoHomeDir,
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

    /// Каталог кэша метаданных и обложек.
    pub fn cache_dir(&self) -> &Path {
        &self.cache
    }
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
    }
}
