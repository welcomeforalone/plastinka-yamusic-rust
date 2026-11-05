//! Настройки приложения в `config.toml`. Секретов пользователя здесь нет:
//! токен хранится только в системном хранилище паролей (см. `token`).

use std::fs;
use std::io;
use std::path::Path;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};

use super::StorageError;

/// Настройки приложения.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Постоянный идентификатор устройства для входа по коду. Не секрет: с ним
    /// повторный вход заменяет старый токен, а не копит новые.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    /// Своё OAuth-приложение вместо приложения по умолчанию (см. `auth`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oauth_client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oauth_client_secret: Option<String>,
    /// Боковое меню свёрнуто до иконок.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub sidebar_collapsed: bool,
    /// Акцентный цвет окна: `red`, `teal`, `orange`, `violet`, `blue`.
    /// Неизвестное значение окно заменяет красным.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    pub accent: Option<String>,
    /// Масштаб окна в процентах, см. [`Config::scale_percent`].
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    pub scale: Option<i64>,
    /// Визуализатор спектра под плеером.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    pub visualizer: Option<bool>,
}

/// Масштаб окна: от 75 до 200 % шагом 25.
pub const SCALE_MIN: u32 = 75;
pub const SCALE_MAX: u32 = 200;
pub const SCALE_STEP: u32 = 25;
pub const SCALE_DEFAULT: u32 = 100;

/// Значение неверного типа (`scale = "big"`) — как будто его нет: из-за
/// одной опечатки во внешнем виде не должны пропасть остальные настройки.
fn lenient<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let value = toml::Value::deserialize(deserializer)?;
    Ok(value.try_into().ok())
}

/// Масштаб в допустимых пределах, к ближайшему шагу.
pub fn normalize_scale(percent: i64) -> u32 {
    let min = i64::from(SCALE_MIN);
    let max = i64::from(SCALE_MAX);
    let step = i64::from(SCALE_STEP);
    let clamped = percent.clamp(min, max);
    let snapped = min + (clamped - min + step / 2) / step * step;
    u32::try_from(snapped.min(max)).unwrap_or(SCALE_DEFAULT)
}

impl Config {
    /// Читает настройки. Если файла ещё нет, возвращает настройки по умолчанию.
    pub fn load(path: &Path) -> Result<Self, StorageError> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => {
                return Err(StorageError::ConfigIo {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        toml::from_str(&text).map_err(|err| StorageError::ConfigParse {
            path: path.to_path_buf(),
            line: err.span().map(|span| line_number(&text, span.start)),
            message: err.message().to_owned(),
        })
    }

    /// Сохраняет настройки через временный файл, чтобы при сбое не остался
    /// обрезанный `config.toml`.
    pub fn save(&self, path: &Path) -> Result<(), StorageError> {
        let text = toml::to_string(self)?;
        let io_error = |source| StorageError::ConfigIo {
            path: path.to_path_buf(),
            source,
        };
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(io_error)?;
        }
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, text).map_err(io_error)?;
        fs::rename(&tmp, path).map_err(io_error)
    }

    /// Масштаб окна в процентах: вне пределов — к ближайшей границе, между
    /// шагами — к ближайшему шагу. Нет значения — 100 %.
    pub fn scale_percent(&self) -> u32 {
        self.scale.map_or(SCALE_DEFAULT, normalize_scale)
    }

    /// Возвращает идентификатор устройства и создаёт его, если его нет или он
    /// испорчен. Второе значение — `true`, если настройки нужно сохранить.
    pub fn ensure_device_id(&mut self) -> (String, bool) {
        match &self.device_id {
            Some(id) if is_valid_device_id(id) => (id.clone(), false),
            _ => {
                let id = uuid::Uuid::new_v4().to_string();
                self.device_id = Some(id.clone());
                (id, true)
            }
        }
    }
}

/// Номер строки (с единицы) для смещения в байтах.
fn line_number(text: &str, offset: usize) -> usize {
    text.bytes().take(offset).filter(|&b| b == b'\n').count() + 1
}

/// Яндекс принимает от 6 до 50 печатных ASCII-символов.
fn is_valid_device_id(id: &str) -> bool {
    (6..=50).contains(&id.len()) && id.bytes().all(|b| (32..=126).contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_config_path() -> std::path::PathBuf {
        std::env::temp_dir()
            .join(format!("plastinka-test-{}", uuid::Uuid::new_v4()))
            .join("config.toml")
    }

    #[test]
    fn missing_file_gives_defaults() {
        let config = Config::load(&temp_config_path()).unwrap();
        assert_eq!(config, Config::default());
    }

    #[test]
    fn save_and_load_round_trip() {
        let path = temp_config_path();
        let mut config = Config::default();
        let (id, created) = config.ensure_device_id();
        assert!(created);
        config.save(&path).unwrap();

        let mut loaded = Config::load(&path).unwrap();
        assert_eq!(loaded, config);
        assert_eq!(loaded.ensure_device_id(), (id, false));
        if let Some(dir) = path.parent() {
            let _ = fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn broken_device_id_is_replaced() {
        let mut config = Config {
            device_id: Some("abc".to_owned()),
            ..Config::default()
        };
        let (id, created) = config.ensure_device_id();
        assert!(created);
        assert!(is_valid_device_id(&id));
    }

    #[test]
    fn appearance_survives_bad_values() {
        let config: Config = toml::from_str(
            "device_id = \"abcdef\"
accent = 5
scale = \"big\"
visualizer = \"yes\"
",
        )
        .unwrap();
        assert_eq!(config.device_id.as_deref(), Some("abcdef"));
        assert_eq!(config.accent, None);
        assert_eq!(config.scale, None);
        assert_eq!(config.visualizer, None);
        assert_eq!(config.scale_percent(), 100);
    }

    #[test]
    fn scale_is_clamped_and_snapped() {
        assert_eq!(normalize_scale(10), 75);
        assert_eq!(normalize_scale(-500), 75);
        assert_eq!(normalize_scale(i64::MAX), 200);
        assert_eq!(normalize_scale(110), 100);
        assert_eq!(normalize_scale(113), 125);
        assert_eq!(normalize_scale(150), 150);
        assert_eq!(normalize_scale(199), 200);
    }

    #[test]
    fn broken_file_is_reported() {
        let path = temp_config_path();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).unwrap();
        }
        fs::write(
            &path,
            "device_id = \"abcdef\"\noauth_client_secret = [секрет",
        )
        .unwrap();
        let err = Config::load(&path).unwrap_err();
        assert!(
            matches!(err, StorageError::ConfigParse { line: Some(2), .. }),
            "{err:?}"
        );
        assert!(!err.to_string().contains("секрет"), "{err}");
        if let Some(dir) = path.parent() {
            let _ = fs::remove_dir_all(dir);
        }
    }
}
