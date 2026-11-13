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
    /// Качество звука: `lossless`, `normal` или `low`. Неизвестное значение
    /// окно заменяет на `normal`.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    pub audio_quality: Option<String>,
    /// Нормализация громкости треков (по умолчанию выключена).
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    pub normalize: Option<bool>,
    /// Discord Rich Presence: показывать статус (по умолчанию да).
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    pub discord_enabled: Option<bool>,
    /// Что в строке статуса: `app`, `artist` или `track`.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    pub discord_status: Option<String>,
    /// Показывать статус на паузе (по умолчанию нет).
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    pub discord_on_pause: Option<bool>,
    /// Кнопка «Слушать» со ссылкой на трек (по умолчанию да).
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    pub discord_button: Option<bool>,
    /// Язык подписей статуса: `ru` или `en`.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    pub discord_lang: Option<String>,
    /// Имя в статусе Discord: `plastinka`, `yandex-ru`, `yandex-en`.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    pub discord_app_name: Option<String>,
    /// Application ID Discord вместо встроенного.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    pub discord_app_id: Option<String>,
    /// Запасной источник текстов песен — LRCLib, если у Яндекса текста нет
    /// (по умолчанию да).
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    pub lyrics_lrclib: Option<bool>,
    /// Выбор в меню «Моей волны» (таблица `[wave_settings]`). Только у нас, в
    /// аккаунт не пишется.
    #[serde(
        skip_serializing_if = "WaveConfig::is_empty",
        deserialize_with = "lenient_table"
    )]
    pub wave_settings: WaveConfig,
    /// Мини-плеер (таблица `[mini_player]`): где стоит окно и какая раскладка.
    #[serde(
        skip_serializing_if = "MiniConfig::is_empty",
        deserialize_with = "lenient_table"
    )]
    pub mini_player: MiniConfig,
    /// Плагины (таблица `[plugins]`).
    #[serde(
        skip_serializing_if = "PluginsConfig::is_empty",
        deserialize_with = "lenient_table"
    )]
    pub plugins: PluginsConfig,
}

/// Таблица `[mini_player]`: положение окна мини-плеера (целые логические
/// координаты) и раскладка. Значение неверного типа — как будто его нет.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MiniConfig {
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    pub x: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    pub y: Option<i64>,
    /// Низкая горизонтальная раскладка (420 × 132) вместо высокой (320 × 440).
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    pub compact: Option<bool>,
}

impl MiniConfig {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Таблица `[wave_settings]`: что выбрано в меню «Моей волны». Значение
/// неверного типа — как будто его нет.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WaveConfig {
    /// Тег занятия (`activity:{tag}`) и его подпись, чтобы показать её без сети.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    pub activity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    pub activity_name: Option<String>,
    /// `favorite`, `discover`, `popular`.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    pub diversity: Option<String>,
    /// `active`, `fun`, `calm`, `sad`.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    pub mood: Option<String>,
    /// `russian`, `not-russian`, `without-words`.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    pub language: Option<String>,
}

impl WaveConfig {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Таблица `[plugins]`: какие плагины включены и их настройки. Значение
/// неверного типа — как будто его нет.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginsConfig {
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    pub fckcensor: Option<bool>,
    /// Искать версию трека без цензуры на Яндексе.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    pub fckcensor_explicit: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    pub slopless: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    pub slopless_autoskip: Option<bool>,
    /// Согласие ставить «Не нравится» ИИ-трекам (запись в аккаунт).
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    pub slopless_dislike: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    pub copy_track_name: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    pub copy_track_name_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    pub vibe_colorizer: Option<bool>,
}

impl PluginsConfig {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }
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

/// Таблица неверного типа (`plugins = 5`) — как будто её нет.
fn lenient_table<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned + Default,
{
    let value = toml::Value::deserialize(deserializer)?;
    Ok(value.try_into().unwrap_or_default())
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
    fn custom_accent_is_kept_as_text() {
        let config: Config = toml::from_str(
            "accent = \"#12ab9f\"
",
        )
        .unwrap();
        assert_eq!(config.accent.as_deref(), Some("#12ab9f"));
    }

    #[test]
    fn sound_settings_survive_bad_values() {
        let config: Config = toml::from_str(
            "accent = \"teal\"
audio_quality = \"lossless\"
normalize = true
",
        )
        .unwrap();
        assert_eq!(config.audio_quality.as_deref(), Some("lossless"));
        assert_eq!(config.normalize, Some(true));
        let broken: Config = toml::from_str(
            "audio_quality = 7
normalize = \"да\"
",
        )
        .unwrap();
        assert_eq!(broken.audio_quality, None);
        assert_eq!(broken.normalize, None);
        // Неизвестное значение — «Оптимальное».
        let unknown: Config = toml::from_str("audio_quality = \"ultra\"").unwrap();
        let quality = unknown
            .audio_quality
            .as_deref()
            .and_then(crate::api::AudioQuality::from_key)
            .unwrap_or_default();
        assert_eq!(quality, crate::api::AudioQuality::Normal);
    }

    #[test]
    fn discord_settings_survive_bad_values() {
        let config: Config = toml::from_str(
            "discord_enabled = false
discord_status = \"track\"
discord_app_id = \"123\"
discord_button = \"да\"
",
        )
        .unwrap();
        assert_eq!(config.discord_enabled, Some(false));
        assert_eq!(config.discord_status.as_deref(), Some("track"));
        assert_eq!(config.discord_app_id.as_deref(), Some("123"));
        assert_eq!(config.discord_button, None);
        // Ничего не задано — в файл ничего не пишется.
        let text = toml::to_string(&Config::default()).unwrap();
        assert!(!text.contains("discord"));
    }

    #[test]
    fn lyrics_lrclib_setting() {
        let config: Config = toml::from_str(
            "lyrics_lrclib = false
",
        )
        .unwrap();
        assert_eq!(config.lyrics_lrclib, Some(false));
        let config: Config = toml::from_str(
            "lyrics_lrclib = \"нет\"
",
        )
        .unwrap();
        assert_eq!(config.lyrics_lrclib, None);
        assert!(
            !toml::to_string(&Config::default())
                .unwrap()
                .contains("lyrics")
        );
    }

    #[test]
    fn wave_settings_survive_bad_values() {
        let config: Config = toml::from_str(
            "[wave_settings]
mood = \"calm\"
language = 5
activity = \"work-background\"
",
        )
        .unwrap();
        assert_eq!(config.wave_settings.mood.as_deref(), Some("calm"));
        assert_eq!(config.wave_settings.language, None);
        // Вся таблица неверного типа — как будто её нет.
        let config: Config = toml::from_str(
            "wave_settings = 5
",
        )
        .unwrap();
        assert_eq!(config.wave_settings, WaveConfig::default());
        // Пусто — в файл ничего не пишется.
        assert!(
            !toml::to_string(&Config::default())
                .unwrap()
                .contains("wave_settings")
        );
        // Запись и чтение обратно.
        let mut config = Config::default();
        config.wave_settings.diversity = Some("popular".to_owned());
        let text = toml::to_string(&config).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.wave_settings.diversity.as_deref(), Some("popular"));
    }

    #[test]
    fn plugins_survive_bad_values() {
        let config: Config = toml::from_str(
            "accent = \"teal\"
[plugins]
fckcensor = true
slopless = \"да\"
copy_track_name_format = \"{title}\"
",
        )
        .unwrap();
        assert_eq!(config.plugins.fckcensor, Some(true));
        assert_eq!(config.plugins.slopless, None);
        assert_eq!(
            config.plugins.copy_track_name_format.as_deref(),
            Some("{title}")
        );
        let broken: Config = toml::from_str(
            "plugins = 5
accent = \"teal\"
",
        )
        .unwrap();
        assert_eq!(broken.plugins, PluginsConfig::default());
        assert_eq!(broken.accent.as_deref(), Some("teal"));
        // Пустая таблица не пишется.
        assert!(
            !toml::to_string(&Config::default())
                .unwrap()
                .contains("plugins")
        );
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
