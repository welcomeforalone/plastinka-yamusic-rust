//! Настройки приложения в `config.toml`. Секретов пользователя здесь нет:
//! токен хранится только в системном хранилище паролей (см. `token`).

use std::fs;
use std::io;
use std::path::Path;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};

use super::StorageError;
use crate::player::eq::{BANDS, CustomPreset, EqState, GAIN_LIMIT, PresetId, preset_by_key};

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
    /// Эквалайзер (таблица `[equalizer]`). Читать и писать через
    /// [`Config::equalizer`] и [`Config::set_equalizer`].
    #[serde(
        skip_serializing_if = "EqConfig::is_empty",
        deserialize_with = "lenient_table"
    )]
    pub equalizer: EqConfig,
}

/// Таблица `[equalizer]`: усиления в десятых долях дБ, целые. Значение
/// неверного типа — как будто его нет, массив не из 10 чисел отбрасывается.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EqConfig {
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    auto_preamp: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    preamp: Option<i64>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient_gains"
    )]
    gains: Option<Vec<i64>>,
    /// Ключ встроенного пресета или имя своего.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    preset: Option<String>,
    /// Свои пресеты (`[[equalizer.presets]]`).
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "lenient_presets"
    )]
    presets: Vec<EqPresetConfig>,
}

impl EqConfig {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Свой пресет в `config.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct EqPresetConfig {
    name: String,
    #[serde(default, deserialize_with = "lenient")]
    preamp: Option<i64>,
    gains: Vec<i64>,
}

/// Десять усилений, обрезанных до пределов. Другая длина — `None`.
fn gains_array(values: &[i64]) -> Option<[i16; BANDS]> {
    if values.len() != BANDS {
        return None;
    }
    let mut gains = [0; BANDS];
    for (slot, value) in gains.iter_mut().zip(values) {
        *slot = clamp_gain(*value);
    }
    Some(gains)
}

fn clamp_gain(value: i64) -> i16 {
    let limit = i64::from(GAIN_LIMIT);
    i16::try_from(value.clamp(-limit, limit)).unwrap_or(0)
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

/// Усиления: не массив из 10 целых — как будто их нет.
fn lenient_gains<'de, D>(deserializer: D) -> Result<Option<Vec<i64>>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = toml::Value::deserialize(deserializer)?;
    let gains: Option<Vec<i64>> = value.try_into().ok();
    Ok(gains.filter(|gains| gains.len() == BANDS))
}

/// Свои пресеты: не массив — пустой список, битый пресет отбрасывается, а
/// соседние остаются.
fn lenient_presets<'de, D>(deserializer: D) -> Result<Vec<EqPresetConfig>, D::Error>
where
    D: Deserializer<'de>,
{
    let toml::Value::Array(items) = toml::Value::deserialize(deserializer)? else {
        return Ok(Vec::new());
    };
    Ok(items
        .into_iter()
        .filter_map(|item| item.try_into::<EqPresetConfig>().ok())
        .filter(|preset| preset.gains.len() == BANDS)
        .collect())
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

    /// Состояние эквалайзера. Всё проверено: числа в пределах, свои пресеты
    /// с допустимыми именами без повторов, выбранный пресет существует и
    /// совпадает с полосами (иначе «Свои значения»).
    pub fn equalizer(&self) -> EqState {
        let raw = &self.equalizer;
        let mut state = EqState::default();
        let settings = &mut state.settings;
        settings.enabled = raw.enabled.unwrap_or(settings.enabled);
        settings.auto_preamp = raw.auto_preamp.unwrap_or(settings.auto_preamp);
        settings.preamp = raw.preamp.map_or(settings.preamp, clamp_gain);
        if let Some(gains) = raw.gains.as_deref().and_then(gains_array) {
            settings.gains = gains;
        }
        for preset in &raw.presets {
            if let Some(gains) = gains_array(&preset.gains) {
                let preamp = preset.preamp.map_or(0, clamp_gain);
                // Недопустимое имя или повтор — пресет отбрасывается.
                let _ = state.add_custom(&preset.name, preamp, gains);
            }
        }
        let preset = raw.preset.as_deref().and_then(|name| {
            preset_by_key(name)
                .map(|preset| PresetId::Builtin(preset.key))
                .or_else(|| {
                    state
                        .custom
                        .iter()
                        .find(|custom| custom.name == name)
                        .map(|custom| PresetId::Custom(custom.name.clone()))
                })
        });
        state.preset = preset.filter(|id| state.preset_matches(id));
        state
    }

    /// Записывает состояние эквалайзера. Состояние по умолчанию таблицу не
    /// создаёт.
    pub fn set_equalizer(&mut self, state: &EqState) {
        if *state == EqState::default() {
            self.equalizer = EqConfig::default();
            return;
        }
        let settings = state.settings.sanitized();
        let to_i64 = |gains: &[i16; BANDS]| gains.iter().map(|g| i64::from(*g)).collect();
        self.equalizer = EqConfig {
            enabled: Some(settings.enabled),
            auto_preamp: Some(settings.auto_preamp),
            preamp: Some(i64::from(settings.preamp)),
            gains: Some(to_i64(&settings.gains)),
            preset: state.preset.as_ref().map(|id| match id {
                PresetId::Builtin(key) => (*key).to_owned(),
                PresetId::Custom(name) => name.clone(),
            }),
            presets: state
                .custom
                .iter()
                .map(|custom: &CustomPreset| EqPresetConfig {
                    name: custom.name.clone(),
                    preamp: Some(i64::from(custom.preamp.clamp(-GAIN_LIMIT, GAIN_LIMIT))),
                    gains: to_i64(&custom.gains),
                })
                .collect(),
        };
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

    #[test]
    fn equalizer_round_trip() {
        let mut state = EqState::default();
        state.settings.enabled = true;
        state.settings.preamp = -30;
        state.settings.gains = [50, 40, 20, 0, -20, -20, 0, 20, 40, 50];
        state.preset = Some(PresetId::Builtin("v-shape"));
        state
            .add_custom("Наушники", 5, [30, 20, 0, 0, 0, 0, 0, 10, 20, 30])
            .unwrap();
        let mut config = Config::default();
        config.set_equalizer(&state);
        let text = toml::to_string(&config).unwrap();
        assert!(text.contains("[equalizer]"), "{text}");
        assert!(text.contains("[[equalizer.presets]]"), "{text}");
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.equalizer(), state);
        // Значения по умолчанию таблицу не создают.
        let mut config = Config::default();
        config.set_equalizer(&EqState::default());
        assert!(!toml::to_string(&config).unwrap().contains("equalizer"));
        assert!(
            !toml::to_string(&Config::default())
                .unwrap()
                .contains("equalizer")
        );
        assert_eq!(Config::default().equalizer(), EqState::default());
    }

    #[test]
    fn equalizer_survives_bad_values() {
        // Три числа, строка внутри, не массив: усиления отброшены, остальное живо.
        for gains in ["[1, 2, 3]", "[1, 2, 3, 4, 5, 6, 7, 8, 9, \"x\"]", "7"] {
            let config: Config = toml::from_str(&format!(
                "accent = \"teal\"\n[equalizer]\nenabled = true\npreamp = -20\ngains = {gains}\n"
            ))
            .unwrap();
            let state = config.equalizer();
            assert!(state.settings.enabled);
            assert_eq!(state.settings.preamp, -20);
            assert_eq!(state.settings.gains, [0; BANDS]);
            assert_eq!(config.accent.as_deref(), Some("teal"));
        }
        // Таблица неверного типа.
        let config: Config = toml::from_str("equalizer = 5\naccent = \"teal\"\n").unwrap();
        assert_eq!(config.equalizer, EqConfig::default());
        assert_eq!(config.accent.as_deref(), Some("teal"));
        // Поле неверного типа.
        let config: Config =
            toml::from_str("[equalizer]\nenabled = \"да\"\npreamp = 10\n").unwrap();
        assert!(!config.equalizer().settings.enabled);
        assert_eq!(config.equalizer().settings.preamp, 10);
        // Вне пределов — обрезаются.
        let config: Config = toml::from_str(
            "[equalizer]\npreamp = 9999\ngains = [500, -500, 0, 0, 0, 0, 0, 0, 0, 120]\n",
        )
        .unwrap();
        let settings = config.equalizer().settings;
        assert_eq!(settings.preamp, GAIN_LIMIT);
        assert_eq!(settings.gains[0], GAIN_LIMIT);
        assert_eq!(settings.gains[1], -GAIN_LIMIT);
        assert_eq!(settings.gains[9], GAIN_LIMIT);
    }

    #[test]
    fn equalizer_presets_are_validated() {
        let zeros = "gains = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0]";
        let config: Config = toml::from_str(&format!(
            "[equalizer]
preset = \"Один\"
gains = [10, 0, 0, 0, 0, 0, 0, 0, 0, 0]
[[equalizer.presets]]
name = \"Один\"
preamp = 0
gains = [10, 0, 0, 0, 0, 0, 0, 0, 0, 0]
[[equalizer.presets]]
name = \"Битый\"
gains = [1, 2, 3]
[[equalizer.presets]]
name = \"Строка\"
gains = [1, 2, 3, 4, 5, 6, 7, 8, 9, \"x\"]
[[equalizer.presets]]
{zeros}
[[equalizer.presets]]
name = \"  ОДИН \"
{zeros}
[[equalizer.presets]]
name = \"Рок\"
{zeros}
[[equalizer.presets]]
name = \"Два\"
preamp = 500
gains = [0, 0, 0, 0, 0, 0, 0, 0, 0, 300]
"
        ))
        .unwrap();
        let state = config.equalizer();
        let names: Vec<&str> = state.custom.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Один", "Два"]);
        let two = state.custom.get(1).unwrap();
        assert_eq!(two.preamp, GAIN_LIMIT);
        assert_eq!(two.gains[9], GAIN_LIMIT);
        assert_eq!(state.preset, Some(PresetId::Custom("Один".to_owned())));
        // presets неверного типа — пустой список; пресет, не совпадающий с
        // полосами, — «Свои значения».
        let config: Config =
            toml::from_str("[equalizer]\npresets = 5\npreset = \"rock\"\n").unwrap();
        let state = config.equalizer();
        assert!(state.custom.is_empty());
        assert_eq!(state.preset, None);
        // Больше 32 пресетов не читаем.
        let mut text = String::from("[equalizer]\n");
        for i in 0..40 {
            text.push_str(&format!(
                "[[equalizer.presets]]\nname = \"п{i}\"\n{zeros}\n"
            ));
        }
        let config: Config = toml::from_str(&text).unwrap();
        assert_eq!(
            config.equalizer().custom.len(),
            crate::player::eq::MAX_CUSTOM
        );
    }
}
