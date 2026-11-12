//! Настройки «Моей волны»: занятие, характер, настроение, язык. Это не запись
//! в аккаунт: настройки лежат в `config.toml`, а на сервер уходят seeds
//! нового сеанса (`POST /rotor/session/new`). Старые `settings2`/`settings3`
//! Яндекс отключил. Спека — раздел 4 `docs/api/stage-9.md`.

use reqwest::StatusCode;
use serde_json::Value;

use super::tracks::clean;
use super::{ApiError, check_status};
use crate::storage::config::WaveConfig;

/// «Моя волна».
const WAVE_SEED: &str = "user:onyourwave";
/// Сколько занятий берём из списка станций.
const MAX_ACTIVITIES: usize = 60;

/// Занятие: станция `activity:{tag}`. Подпись приходит от сервиса.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activity {
    pub tag: String,
    pub name: String,
}

macro_rules! choice {
    ($(#[$meta:meta])* $name:ident { $($variant:ident => ($key:literal, $title:literal)),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            /// Значение в seed и в `config.toml`.
            pub fn key(self) -> &'static str {
                match self {
                    $(Self::$variant => $key),+
                }
            }

            pub fn from_key(key: &str) -> Option<Self> {
                Self::ALL.iter().copied().find(|choice| choice.key() == key)
            }

            /// Подпись в меню.
            pub fn title(self) -> &'static str {
                match self {
                    $(Self::$variant => $title),+
                }
            }
        }
    };
}

choice!(
    /// «Характер» музыки.
    Diversity {
        Favorite => ("favorite", "Любимое"),
        Discover => ("discover", "Незнакомое"),
        Popular => ("popular", "Популярное"),
    }
);

choice!(
    /// «Настроение».
    Mood {
        Active => ("active", "Бодрое"),
        Fun => ("fun", "Весёлое"),
        Calm => ("calm", "Спокойное"),
        Sad => ("sad", "Грустное"),
    }
);

choice!(
    /// «Язык» треков.
    Language {
        Russian => ("russian", "Русский"),
        NotRussian => ("not-russian", "Иностранный"),
        WithoutWords => ("without-words", "Без слов"),
    }
);

/// Выбор в меню волны. Пусто — как «по умолчанию»: seed не передаётся.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WaveSettings {
    pub activity: Option<Activity>,
    pub diversity: Option<Diversity>,
    pub mood: Option<Mood>,
    pub language: Option<Language>,
}

impl WaveSettings {
    /// Ничего не выбрано: обычная «Моя волна».
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Seeds для `POST /rotor/session/new`. Без занятия — `user:onyourwave`;
    /// с занятием — `activity:{tag}` вместо него (спека молчит, можно ли
    /// вместе; **проверить вживую**). Настройки добавляются в обоих случаях.
    pub fn seeds(&self) -> Vec<String> {
        let mut seeds = vec![match &self.activity {
            Some(activity) => format!("activity:{}", activity.tag),
            None => WAVE_SEED.to_owned(),
        }];
        if let Some(diversity) = self.diversity {
            seeds.push(format!("settingDiversity:{}", diversity.key()));
        }
        if let Some(mood) = self.mood {
            seeds.push(format!("settingMoodEnergy:{}", mood.key()));
        }
        if let Some(language) = self.language {
            seeds.push(format!("settingLanguage:{}", language.key()));
        }
        seeds
    }

    /// Подпись таблетки: «Моя волна» или выбранное кратко, например
    /// «Бодрое · Русский».
    pub fn label(&self) -> String {
        let parts: Vec<&str> = [
            self.activity
                .as_ref()
                .map(|activity| activity.name.as_str()),
            self.mood.map(Mood::title),
            self.diversity.map(Diversity::title),
            self.language.map(Language::title),
        ]
        .into_iter()
        .flatten()
        .collect();
        if parts.is_empty() {
            "Моя волна".to_owned()
        } else {
            parts.join(" · ")
        }
    }

    /// Из `config.toml`. Неизвестное или небезопасное значение — как будто
    /// его нет.
    pub fn from_config(config: &WaveConfig) -> Self {
        let activity = config
            .activity
            .as_deref()
            .filter(|tag| safe_tag(tag))
            .map(|tag| Activity {
                tag: tag.to_owned(),
                name: config
                    .activity_name
                    .as_deref()
                    .and_then(clean)
                    .unwrap_or_else(|| tag.to_owned()),
            });
        Self {
            activity,
            diversity: config.diversity.as_deref().and_then(Diversity::from_key),
            mood: config.mood.as_deref().and_then(Mood::from_key),
            language: config.language.as_deref().and_then(Language::from_key),
        }
    }

    /// В `config.toml`: пустой выбор стирает таблицу.
    pub fn write(&self, config: &mut WaveConfig) {
        *config = WaveConfig {
            activity: self.activity.as_ref().map(|activity| activity.tag.clone()),
            activity_name: self.activity.as_ref().map(|activity| activity.name.clone()),
            diversity: self.diversity.map(|value| value.key().to_owned()),
            mood: self.mood.map(|value| value.key().to_owned()),
            language: self.language.map(|value| value.key().to_owned()),
        };
    }
}

/// Тег станции идёт в тело запроса: буквы, цифры, `-`, `_`, `.`.
pub fn safe_tag(tag: &str) -> bool {
    (1..=64).contains(&tag.len())
        && tag
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// Занятия из ответа `GET /rotor/stations/list`: станции типа `activity`.
/// Непонятный элемент пропускается, повторы отбрасываются.
pub fn parse_activities(status: StatusCode, body: &str) -> Result<Vec<Activity>, ApiError> {
    check_status(status)?;
    let envelope: Value = serde_json::from_str(body).map_err(|_| ApiError::InvalidResponse)?;
    let items = envelope
        .get("result")
        .and_then(Value::as_array)
        .ok_or(ApiError::InvalidResponse)?;
    let mut activities: Vec<Activity> = Vec::new();
    for item in items {
        let Some(station) = item.get("station") else {
            continue;
        };
        let id = station.get("id");
        let kind = id.and_then(|id| id.get("type")).and_then(Value::as_str);
        let tag = id.and_then(|id| id.get("tag")).and_then(Value::as_str);
        let name = station.get("name").and_then(Value::as_str).and_then(clean);
        let (Some("activity"), Some(tag), Some(name)) = (kind, tag, name) else {
            continue;
        };
        if safe_tag(tag) && !activities.iter().any(|known| known.tag == tag) {
            activities.push(Activity {
                tag: tag.to_owned(),
                name,
            });
        }
        if activities.len() >= MAX_ACTIVITIES {
            break;
        }
    }
    Ok(activities)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn activity(tag: &str, name: &str) -> Activity {
        Activity {
            tag: tag.to_owned(),
            name: name.to_owned(),
        }
    }

    #[test]
    fn default_seed_is_plain_wave() {
        let settings = WaveSettings::default();
        assert!(settings.is_default());
        assert_eq!(settings.seeds(), vec!["user:onyourwave"]);
        assert_eq!(settings.label(), "Моя волна");
    }

    #[test]
    fn settings_become_seeds() {
        let settings = WaveSettings {
            activity: None,
            diversity: Some(Diversity::Discover),
            mood: Some(Mood::Calm),
            language: Some(Language::NotRussian),
        };
        assert_eq!(
            settings.seeds(),
            vec![
                "user:onyourwave",
                "settingDiversity:discover",
                "settingMoodEnergy:calm",
                "settingLanguage:not-russian",
            ]
        );
        let one = WaveSettings {
            language: Some(Language::WithoutWords),
            ..WaveSettings::default()
        };
        assert_eq!(
            one.seeds(),
            vec!["user:onyourwave", "settingLanguage:without-words"]
        );
    }

    #[test]
    fn activity_replaces_wave_seed() {
        let settings = WaveSettings {
            activity: Some(activity("work-background", "Работа")),
            mood: Some(Mood::Active),
            ..WaveSettings::default()
        };
        assert_eq!(
            settings.seeds(),
            vec!["activity:work-background", "settingMoodEnergy:active"]
        );
    }

    #[test]
    fn all_choices_roundtrip_and_have_titles() {
        for value in Diversity::ALL {
            assert_eq!(Diversity::from_key(value.key()), Some(*value));
            assert!(!value.title().is_empty());
        }
        for value in Mood::ALL {
            assert_eq!(Mood::from_key(value.key()), Some(*value));
        }
        for value in Language::ALL {
            assert_eq!(Language::from_key(value.key()), Some(*value));
        }
        assert_eq!(Mood::from_key("angry"), None);
        assert_eq!(
            Diversity::ALL.len() + Mood::ALL.len() + Language::ALL.len(),
            10
        );
    }

    #[test]
    fn label_is_short() {
        let settings = WaveSettings {
            activity: None,
            diversity: None,
            mood: Some(Mood::Active),
            language: Some(Language::Russian),
        };
        assert_eq!(settings.label(), "Бодрое · Русский");
        let with_activity = WaveSettings {
            activity: Some(activity("sleep", "Сон")),
            ..WaveSettings::default()
        };
        assert_eq!(with_activity.label(), "Сон");
    }

    #[test]
    fn config_roundtrip_and_bad_values() {
        let settings = WaveSettings {
            activity: Some(activity("workout", "Тренировка")),
            diversity: Some(Diversity::Popular),
            mood: Some(Mood::Fun),
            language: Some(Language::Russian),
        };
        let mut config = WaveConfig::default();
        settings.write(&mut config);
        assert_eq!(WaveSettings::from_config(&config), settings);
        // Пустой выбор стирает записи.
        WaveSettings::default().write(&mut config);
        assert_eq!(config, WaveConfig::default());
        // Битое — по умолчанию, остальное сохраняется.
        let broken = WaveConfig {
            activity: Some("плохой/тег".to_owned()),
            activity_name: Some("x".to_owned()),
            diversity: Some("что-то".to_owned()),
            mood: Some("calm".to_owned()),
            language: None,
        };
        let parsed = WaveSettings::from_config(&broken);
        assert_eq!(parsed.activity, None);
        assert_eq!(parsed.diversity, None);
        assert_eq!(parsed.mood, Some(Mood::Calm));
        // Нет названия занятия — подписью служит тег.
        let nameless = WaveConfig {
            activity: Some("sleep".to_owned()),
            ..WaveConfig::default()
        };
        assert_eq!(
            WaveSettings::from_config(&nameless).activity,
            Some(activity("sleep", "sleep"))
        );
    }

    #[test]
    fn activities_are_parsed_softly() {
        let body = r#"{"result":[
            {"station":{"id":{"type":"activity","tag":"work-background"},"name":"Работа"}},
            {"station":{"id":{"type":"mood","tag":"winter"},"name":"Зимнее"}},
            {"station":{"id":{"type":"activity","tag":"workout"},"name":"  Тренировка "}},
            {"station":{"id":{"type":"activity","tag":"workout"},"name":"Повтор"}},
            {"station":{"id":{"type":"activity","tag":"bad tag"},"name":"Плохой"}},
            {"station":{"id":{"type":"activity","tag":"noname"}}},
            {"station":{"id":{"type":"activity","tag":"sleep"},"name":"Сон"}},
            {"нет":"станции"},
            "мусор"
        ]}"#;
        let activities = parse_activities(StatusCode::OK, body).unwrap();
        assert_eq!(
            activities,
            vec![
                activity("work-background", "Работа"),
                activity("workout", "Тренировка"),
                activity("sleep", "Сон"),
            ]
        );
    }

    #[test]
    fn activity_list_errors() {
        assert!(matches!(
            parse_activities(StatusCode::SERVICE_UNAVAILABLE, ""),
            Err(ApiError::Server(503))
        ));
        assert!(matches!(
            parse_activities(StatusCode::UNAUTHORIZED, ""),
            Err(ApiError::Unauthorized)
        ));
        assert!(matches!(
            parse_activities(StatusCode::OK, "не json"),
            Err(ApiError::InvalidResponse)
        ));
        assert!(matches!(
            parse_activities(StatusCode::OK, r#"{"result":{}}"#),
            Err(ApiError::InvalidResponse)
        ));
        assert_eq!(
            parse_activities(StatusCode::OK, r#"{"result":[]}"#).unwrap(),
            vec![]
        );
    }
}
