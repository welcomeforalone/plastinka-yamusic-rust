//! Встроенные плагины: свои модули на Rust по мотивам аддонов PulseSync.
//! Код аддонов не переносим, только поведение. FckCensor включён по умолчанию,
//! остальные выключены и включаются в «Настройки → Плагины».
//!
//! - [`fckcensor`] — названия, обложки и исполнители без цензуры из списка
//!   `Hazzz895/FckCensorData`, звук — только с Яндекса или из своего файла.
//! - [`slopless`] — метки ИИ-исполнителей и треков по API slopless.art,
//!   автопропуск.
//! - [`copy_name`] — «Исполнитель — Название» в буфер обмена.
//! - [`vibe`] — цвет «Моей волны» под обложку текущего трека.
//! - [`listen_time`] — время прослушивания по дням и серия дней подряд.
//!
//! На диск плагины кэшируют только метаданные (правило 1).

pub mod copy_name;
pub mod fckcensor;
pub mod listen_time;
pub mod slopless;
pub mod vibe;

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::storage::AppDirs;
use crate::storage::config::PluginsConfig;

/// Настройки плагинов, как их видит окно. В `config.toml` — таблица
/// `[plugins]` ([`PluginsConfig`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginSettings {
    pub fckcensor: bool,
    /// Искать на Яндексе версию трека без цензуры.
    pub explicit_search: bool,
    pub slopless: bool,
    /// Пропускать ИИ-треки в очереди.
    pub autoskip: bool,
    /// Ставить «Не нравится» пропущенным ИИ-трекам. Только после согласия.
    pub dislike: bool,
    pub copy_name: bool,
    /// Формат копирования, см. [`copy_name::format`].
    pub copy_format: String,
    /// VibeColorizer: цвет «Моей волны» под обложку.
    pub vibe: bool,
    /// YandexMusicTime: время прослушивания и серия дней.
    pub listen_time: bool,
    /// Время (и серия) в заголовке окна.
    pub listen_timer: bool,
    pub listen_period: listen_time::Period,
    pub listen_seconds: bool,
    /// Подписи «За день:» и «Дней подряд:».
    pub listen_label: bool,
    /// Серия дней подряд рядом со временем.
    pub listen_streak: bool,
    /// Серия огоньком вместо текста.
    pub listen_flame: bool,
    /// Пункт «Статистика» в меню.
    pub listen_menu: bool,
}

impl Default for PluginSettings {
    fn default() -> Self {
        Self {
            // Автозамена зацензуренных треков работает сразу, без настроек.
            fckcensor: true,
            explicit_search: true,
            slopless: false,
            autoskip: true,
            dislike: false,
            copy_name: false,
            copy_format: copy_name::DEFAULT_FORMAT.to_owned(),
            vibe: false,
            listen_time: false,
            listen_timer: true,
            listen_period: listen_time::Period::Total,
            listen_seconds: true,
            listen_label: true,
            listen_streak: true,
            listen_flame: false,
            listen_menu: true,
        }
    }
}

impl PluginSettings {
    pub fn from_config(config: &PluginsConfig) -> Self {
        let defaults = Self::default();
        Self {
            fckcensor: config.fckcensor.unwrap_or(defaults.fckcensor),
            explicit_search: config
                .fckcensor_explicit
                .unwrap_or(defaults.explicit_search),
            slopless: config.slopless.unwrap_or(defaults.slopless),
            autoskip: config.slopless_autoskip.unwrap_or(defaults.autoskip),
            dislike: config.slopless_dislike.unwrap_or(defaults.dislike),
            copy_name: config.copy_track_name.unwrap_or(defaults.copy_name),
            copy_format: config
                .copy_track_name_format
                .clone()
                .filter(|format| !format.trim().is_empty())
                .map(|format| copy_name::limit(&format))
                .unwrap_or(defaults.copy_format),
            vibe: config.vibe_colorizer.unwrap_or(defaults.vibe),
            listen_time: config.listen_time.unwrap_or(defaults.listen_time),
            listen_timer: config.listen_timer.unwrap_or(defaults.listen_timer),
            listen_period: config
                .listen_period
                .as_deref()
                .and_then(listen_time::Period::parse)
                .unwrap_or(defaults.listen_period),
            listen_seconds: config.listen_seconds.unwrap_or(defaults.listen_seconds),
            listen_label: config.listen_label.unwrap_or(defaults.listen_label),
            listen_streak: config.listen_streak.unwrap_or(defaults.listen_streak),
            listen_flame: config.listen_flame.unwrap_or(defaults.listen_flame),
            listen_menu: config.listen_menu.unwrap_or(defaults.listen_menu),
        }
    }

    pub fn write(&self, config: &mut PluginsConfig) {
        config.fckcensor = Some(self.fckcensor);
        config.fckcensor_explicit = Some(self.explicit_search);
        config.slopless = Some(self.slopless);
        config.slopless_autoskip = Some(self.autoskip);
        config.slopless_dislike = Some(self.dislike);
        config.copy_track_name = Some(self.copy_name);
        config.copy_track_name_format = Some(self.copy_format.clone());
        config.vibe_colorizer = Some(self.vibe);
        config.listen_time = Some(self.listen_time);
        config.listen_timer = Some(self.listen_timer);
        config.listen_period = Some(self.listen_period.key().to_owned());
        config.listen_seconds = Some(self.listen_seconds);
        config.listen_label = Some(self.listen_label);
        config.listen_streak = Some(self.listen_streak);
        config.listen_flame = Some(self.listen_flame);
        config.listen_menu = Some(self.listen_menu);
    }

    /// Все плагины включены: для отладочного хука `PLASTINKA_TEST_PLUGINS`.
    #[cfg(debug_assertions)]
    pub fn all_on() -> Self {
        Self {
            fckcensor: true,
            slopless: true,
            copy_name: true,
            vibe: true,
            listen_time: true,
            ..Self::default()
        }
    }
}

/// Каталог кэша плагинов (`<кэш>/plugins`). Только метаданные.
pub fn cache_dir() -> Option<PathBuf> {
    AppDirs::discover()
        .ok()
        .map(|dirs| dirs.cache_dir().join("plugins"))
}

/// Секунды от начала эпохи: время записей в кэше.
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// Запись `at` старше `max`.
pub fn stale(at: u64, max: Duration) -> bool {
    unix_now().saturating_sub(at) > max.as_secs()
}

/// Можно ли писать кэш плагинов: с тестовыми хуками ничего не пишем.
pub fn may_write() -> bool {
    !crate::storage::test_hooks_active()
}

/// Значение отладочного хука `PLASTINKA_TEST_PLUGINS`: готовые ответы вместо
/// GitHub и slopless.art. Работает только с фальшивым токеном.
#[cfg(debug_assertions)]
pub fn test_hook() -> Option<String> {
    if !crate::storage::test_hooks_active() {
        return None;
    }
    std::env::var("PLASTINKA_TEST_PLUGINS")
        .ok()
        .filter(|value| !value.trim().is_empty())
}

/// Читает тело ответа не больше `limit` байт. Больше — `None`.
pub async fn read_limited(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Option<Vec<u8>>, crate::net::NetError> {
    if response
        .content_length()
        .is_some_and(|length| length > u64::try_from(limit).unwrap_or(u64::MAX))
    {
        return Ok(None);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(crate::net::NetError::from)? {
        if body.len() + chunk.len() > limit {
            return Ok(None);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Some(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_defaults() {
        let defaults = PluginSettings::from_config(&PluginsConfig::default());
        assert_eq!(defaults, PluginSettings::default());
        assert!(defaults.fckcensor && !defaults.slopless && !defaults.copy_name && !defaults.vibe);
        assert!(!defaults.dislike, "запись в аккаунт — только по согласию");

        let mut changed = PluginSettings {
            slopless: true,
            dislike: true,
            vibe: true,
            copy_format: "{title} by {artist}".to_owned(),
            ..PluginSettings::default()
        };
        let mut config = PluginsConfig::default();
        changed.write(&mut config);
        assert_eq!(PluginSettings::from_config(&config), changed);

        // YandexMusicTime: по умолчанию выключен, остальное включено, кроме огонька.
        assert!(!defaults.listen_time && defaults.listen_timer && defaults.listen_menu);
        assert!(!defaults.listen_flame && defaults.listen_label && defaults.listen_streak);
        assert_eq!(defaults.listen_period, listen_time::Period::Total);
        changed = PluginSettings {
            listen_time: true,
            listen_period: listen_time::Period::Week,
            listen_seconds: false,
            listen_flame: true,
            ..changed
        };
        changed.write(&mut config);
        assert_eq!(PluginSettings::from_config(&config), changed);
        // Неизвестный период — по умолчанию, остальное сохраняется.
        config.listen_period = Some("century".to_owned());
        let parsed = PluginSettings::from_config(&config);
        assert_eq!(parsed.listen_period, listen_time::Period::Total);
        assert!(parsed.listen_flame);
        config.listen_period = Some("session".to_owned());
        assert_eq!(
            PluginSettings::from_config(&config).listen_period,
            listen_time::Period::Session
        );

        changed.copy_format = "   ".to_owned();
        changed.write(&mut config);
        assert_eq!(
            PluginSettings::from_config(&config).copy_format,
            copy_name::DEFAULT_FORMAT
        );
    }
}
