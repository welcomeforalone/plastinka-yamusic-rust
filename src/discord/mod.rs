//! Discord Rich Presence: статус «Слушает» с названием трека, исполнителем,
//! обложкой и таймером. Свой клиент локального IPC Discord (именованный канал
//! или сокет), без сторонних крейтов. Окно только кладёт состояние в канал
//! (`Rpc::set`), всё остальное делает фоновая задача: подключение, повторы,
//! обновление статуса, очистка. События RPC окну не нужны, перерисовок они
//! не вызывают. Ни токена Яндекса, ни ссылок на звук здесь нет.
//!
//! Протокол и поля — в `docs/api/sound-and-rpc.md`.

mod ipc;
mod task;

use serde_json::{Value, json};

pub use task::Rpc;

/// Application ID приложения «Plastinka» в Discord Developer Portal. Имя и
/// иконку статуса («Слушает …») Discord берёт из приложения, а не из статуса,
/// поэтому для каждого имени своё приложение. Подменить можно
/// `discord_app_id` в `config.toml`.
pub const DEFAULT_DISCORD_APP_ID: &str = "1568580284411281423";
/// Приложение пользователя с именем «Яндекс Музыка».
const APP_ID_YANDEX_RU: &str = "1568582823898783775";
/// Приложение пользователя с именем «Yandex Music».
const APP_ID_YANDEX_EN: &str = "1568582948469342320";

/// Предел длины строки статуса у Discord, символов.
const TEXT_LIMIT: usize = 128;
/// Пределы кнопки: подпись и адрес.
const LABEL_LIMIT: usize = 32;
const URL_LIMIT: usize = 512;
/// На сколько должно «съехать» начало трека, чтобы статус пересчитали
/// (перемотка). Обычный ход времени статус не меняет.
const START_SHIFT_MS: i64 = 2000;

/// Что показывать в строке статуса («Слушает …») — `status_display_type`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StatusKind {
    /// Название приложения.
    App,
    /// Исполнитель (по умолчанию).
    #[default]
    Artist,
    /// Название трека.
    Track,
}

impl StatusKind {
    pub const ALL: [Self; 3] = [Self::App, Self::Artist, Self::Track];

    pub fn key(self) -> &'static str {
        match self {
            Self::App => "app",
            Self::Artist => "artist",
            Self::Track => "track",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.key() == key)
    }

    /// Подпись выбора в настройках.
    pub fn title(self) -> &'static str {
        match self {
            Self::App => "Название приложения",
            Self::Artist => "Исполнителя",
            Self::Track => "Название трека",
        }
    }

    fn display_type(self) -> u8 {
        match self {
            Self::App => 0,
            Self::Artist => 1,
            Self::Track => 2,
        }
    }
}

/// Имя в статусе «Слушает …»: какое приложение Discord подключать. Исключение
/// из правила 6 по решению пользователя (2026-11-07): имя видно только в
/// Discord, программа по-прежнему называется Plastinka.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AppName {
    #[default]
    Plastinka,
    YandexRu,
    YandexEn,
}

impl AppName {
    pub const ALL: [Self; 3] = [Self::Plastinka, Self::YandexRu, Self::YandexEn];

    pub fn key(self) -> &'static str {
        match self {
            Self::Plastinka => "plastinka",
            Self::YandexRu => "yandex-ru",
            Self::YandexEn => "yandex-en",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|name| name.key() == key)
    }

    /// Подпись выбора в настройках — так же, как в Discord.
    pub fn title(self) -> &'static str {
        match self {
            Self::Plastinka => "Plastinka",
            Self::YandexRu => "Яндекс Музыка",
            Self::YandexEn => "Yandex Music",
        }
    }

    /// Встроенный Application ID приложения с этим именем.
    fn builtin_id(self) -> &'static str {
        match self {
            Self::Plastinka => DEFAULT_DISCORD_APP_ID,
            Self::YandexRu => APP_ID_YANDEX_RU,
            Self::YandexEn => APP_ID_YANDEX_EN,
        }
    }

    /// Есть ли для имени приложение (можно выбрать в настройках).
    pub fn available(self) -> bool {
        !self.builtin_id().is_empty()
    }
}

/// Язык подписей статуса.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Lang {
    #[default]
    Ru,
    En,
}

impl Lang {
    pub const ALL: [Self; 2] = [Self::Ru, Self::En];

    pub fn key(self) -> &'static str {
        match self {
            Self::Ru => "ru",
            Self::En => "en",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|lang| lang.key() == key)
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Ru => "Русский",
            Self::En => "English",
        }
    }

    fn listen(self) -> &'static str {
        match self {
            Self::Ru => "Слушать",
            Self::En => "Listen",
        }
    }

    fn paused(self) -> &'static str {
        match self {
            Self::Ru => "(пауза)",
            Self::En => "(paused)",
        }
    }
}

/// Настройки статуса (блок «Discord» в «Настройках»).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    /// Показывать статус в Discord.
    pub enabled: bool,
    pub status: StatusKind,
    /// Оставлять статус на паузе (без таймера, с пометкой).
    pub on_pause: bool,
    /// Кнопка «Слушать» со ссылкой на трек.
    pub button: bool,
    pub lang: Lang,
    /// Имя в статусе «Слушает …».
    pub app: AppName,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: true,
            status: StatusKind::default(),
            on_pause: false,
            button: true,
            lang: Lang::default(),
            app: AppName::default(),
        }
    }
}

/// Трек в статусе. Свой локальный файл — только названия (`local`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    pub id: String,
    pub title: String,
    pub artists: String,
    pub album_id: Option<String>,
    pub album: Option<String>,
    /// Шаблон адреса обложки Яндекса с `%%` вместо размера.
    pub cover: Option<String>,
    pub duration_ms: Option<u64>,
    /// Играет свой файл пользователя: без обложки и ссылок.
    pub local: bool,
}

/// Состояние для задачи: что играет и как показывать.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct State {
    pub settings: Settings,
    pub track: Option<Track>,
    pub playing: bool,
    /// Когда трек «начался» по часам (unix, мс): сейчас минус позиция.
    pub start_ms: i64,
}

/// Готовый статус: тело без времени, чтобы сравнивать содержимое отдельно от
/// таймера.
#[derive(Debug, Clone, PartialEq)]
pub struct Activity {
    body: Value,
    start: Option<i64>,
    end: Option<i64>,
}

impl Activity {
    /// Тело для `SET_ACTIVITY` (с таймером, если он есть).
    pub fn to_json(&self) -> Value {
        let mut body = self.body.clone();
        if let (Some(start), Some(map)) = (self.start, body.as_object_mut()) {
            let mut stamps = serde_json::Map::new();
            stamps.insert("start".to_owned(), json!(start));
            if let Some(end) = self.end {
                stamps.insert("end".to_owned(), json!(end));
            }
            map.insert("timestamps".to_owned(), Value::Object(stamps));
        }
        body
    }
}

/// Нужно ли слать статус `new`, если последним ушёл `last`: он изменился по
/// содержанию или таймер съехал больше чем на [`START_SHIFT_MS`]
/// (перемотка). Ход времени статус не меняет.
pub fn should_send(last: &Option<Activity>, new: &Option<Activity>) -> bool {
    match (last, new) {
        (None, None) => false,
        (Some(last), Some(new)) => {
            last.body != new.body
                || last.end.zip(last.start).map(|(e, s)| e - s)
                    != new.end.zip(new.start).map(|(e, s)| e - s)
                || match (last.start, new.start) {
                    (Some(a), Some(b)) => (a - b).abs() > START_SHIFT_MS,
                    (None, None) => false,
                    _ => true,
                }
        }
        _ => true,
    }
}

/// Статус по состоянию. `None` — статус нужно очистить (выключен, ничего не
/// играет, пауза без «Показывать на паузе»).
pub fn activity(state: &State) -> Option<Activity> {
    let settings = state.settings;
    let track = state.track.as_ref().filter(|_| settings.enabled)?;
    if !state.playing && !settings.on_pause {
        return None;
    }
    let paused = !state.playing;
    let mut body = serde_json::Map::new();
    body.insert("type".to_owned(), json!(2));
    body.insert(
        "status_display_type".to_owned(),
        json!(settings.status.display_type()),
    );
    if let Some(details) = fit(&track.title) {
        body.insert("details".to_owned(), json!(details));
    }
    let mut artists = track.artists.clone();
    if paused {
        // Пометка не должна пропасть при обрезке.
        let mark = settings.lang.paused();
        let room = TEXT_LIMIT.saturating_sub(mark.chars().count() + 1);
        artists = format!("{} {mark}", clip(artists.trim(), room));
    }
    if let Some(state_text) = fit(&artists) {
        body.insert("state".to_owned(), json!(state_text));
    }
    let link = (!track.local).then(|| track_url(track));
    if let Some(link) = link.as_deref().filter(|link| link.len() <= URL_LIMIT) {
        body.insert("details_url".to_owned(), json!(link));
    }
    let mut assets = serde_json::Map::new();
    if let Some(cover) = track
        .cover
        .as_deref()
        .filter(|_| !track.local)
        .and_then(cover_url)
    {
        assets.insert("large_image".to_owned(), json!(cover));
    }
    if let Some(album) = track.album.as_deref().filter(|_| !track.local)
        && let Some(text) = fit(album)
    {
        assets.insert("large_text".to_owned(), json!(text));
    }
    if !assets.is_empty() {
        body.insert("assets".to_owned(), Value::Object(assets));
    }
    if settings.button
        && let Some(link) = link.filter(|link| link.len() <= URL_LIMIT)
    {
        body.insert(
            "buttons".to_owned(),
            json!([{
                "label": clip(settings.lang.listen(), LABEL_LIMIT),
                "url": link,
            }]),
        );
    }
    // Таймер — только пока играет: на паузе он бежал бы дальше.
    let (start, end) = if paused {
        (None, None)
    } else {
        let end = track
            .duration_ms
            .filter(|duration| *duration > 0)
            .and_then(|duration| i64::try_from(duration).ok())
            .map(|duration| state.start_ms.saturating_add(duration));
        (Some(state.start_ms), end)
    };
    Some(Activity {
        body: Value::Object(body),
        start,
        end,
    })
}

/// Первые `limit` символов (по символам, не по байтам); обрезанное
/// заканчивается многоточием в пределах `limit`.
fn clip(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(limit.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Строка для Discord: обрезана до 128 символов, короче двух — дополнена
/// пробелом (иначе Discord отклоняет). Пустая — `None`.
fn fit(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let mut out = clip(text, TEXT_LIMIT);
    if out.chars().count() < 2 {
        out.push(' ');
    }
    Some(out)
}

/// Адрес обложки 400×400 из шаблона Яндекса (только его хосты).
pub fn cover_url(template: &str) -> Option<String> {
    if !template.contains("%%") {
        return None;
    }
    let url = format!("https://{}", template.replace("%%", "400x400"));
    let host = url
        .strip_prefix("https://")?
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    let trusted = matches!(
        host,
        "avatars.yandex.net" | "avatars.mds.yandex.net" | "music-avatars.music.yandex.net"
    );
    (trusted && url.len() <= URL_LIMIT).then_some(url)
}

/// Ссылка на трек на сайте Яндекс Музыки.
pub fn track_url(track: &Track) -> String {
    match &track.album_id {
        Some(album) => format!("https://music.yandex.ru/album/{album}/track/{}", track.id),
        None => format!("https://music.yandex.ru/track/{}", track.id),
    }
}

/// Application ID: из `config.toml`, иначе приложение выбранного имени
/// (`name`; имя без приложения — «Plastinka»), в отладке — из
/// `PLASTINKA_TEST_DISCORD_APP_ID`. Пусто — RPC отключён. Discord принимает
/// только числовые идентификаторы.
pub fn app_id(configured: Option<&str>, name: AppName) -> String {
    #[cfg(debug_assertions)]
    if let Ok(id) = std::env::var("PLASTINKA_TEST_DISCORD_APP_ID") {
        return id.trim().to_owned();
    }
    // Пустое или неверное значение из конфига — встроенный ID.
    let valid = |id: &&str| {
        !id.is_empty() && id.len() <= 32 && id.bytes().all(|byte| byte.is_ascii_digit())
    };
    let builtin = if name.available() {
        name.builtin_id()
    } else {
        DEFAULT_DISCORD_APP_ID
    };
    let id = configured
        .map(str::trim)
        .filter(valid)
        .unwrap_or(builtin)
        .to_owned();
    // Отладочный вывод кадров работает и без ID.
    #[cfg(debug_assertions)]
    if id.is_empty() && std::env::var("PLASTINKA_TEST_DISCORD").is_ok_and(|mode| mode == "log") {
        return "0".to_owned();
    }
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track() -> Track {
        Track {
            id: "42".to_owned(),
            title: "Дурак и молния".to_owned(),
            artists: "Король и Шут".to_owned(),
            album_id: Some("7".to_owned()),
            album: Some("Камнем по голове".to_owned()),
            cover: Some("avatars.yandex.net/get-music-content/1/abc/%%".to_owned()),
            duration_ms: Some(114_000),
            local: false,
        }
    }

    fn state(track: Track) -> State {
        State {
            track: Some(track),
            playing: true,
            start_ms: 1_000_000,
            ..State::default()
        }
    }

    fn json_of(state: &State) -> Value {
        activity(state)
            .map(|activity| activity.to_json())
            .unwrap_or(Value::Null)
    }

    #[test]
    fn playing_track_has_all_fields() {
        let value = json_of(&state(track()));
        assert_eq!(value["type"], 2);
        assert_eq!(value["status_display_type"], 1);
        assert_eq!(value["details"], "Дурак и молния");
        assert_eq!(value["state"], "Король и Шут");
        assert_eq!(
            value["details_url"],
            "https://music.yandex.ru/album/7/track/42"
        );
        assert_eq!(value["timestamps"]["start"], 1_000_000);
        assert_eq!(value["timestamps"]["end"], 1_114_000);
        assert_eq!(
            value["assets"]["large_image"],
            "https://avatars.yandex.net/get-music-content/1/abc/400x400"
        );
        assert_eq!(value["assets"]["large_text"], "Камнем по голове");
        assert_eq!(value["buttons"][0]["label"], "Слушать");
        assert_eq!(
            value["buttons"][0]["url"],
            "https://music.yandex.ru/album/7/track/42"
        );
    }

    #[test]
    fn text_is_cut_by_chars_and_short_text_is_padded() {
        let long = "Ж".repeat(300);
        let cut = fit(&long).unwrap();
        assert_eq!(cut.chars().count(), 128);
        assert!(cut.ends_with('…'));
        assert_eq!(fit("я").unwrap(), "я ");
        assert_eq!(fit("  ").as_deref(), None);
        assert_eq!(fit("ab").unwrap(), "ab");
        // Ровно 128 символов — без изменений.
        let exact = "ы".repeat(128);
        assert_eq!(fit(&exact).unwrap(), exact);
    }

    #[test]
    fn no_duration_means_start_only() {
        let mut t = track();
        t.duration_ms = None;
        let value = json_of(&state(t));
        assert_eq!(value["timestamps"]["start"], 1_000_000);
        assert!(value["timestamps"].get("end").is_none());
    }

    #[test]
    fn pause_clears_or_shows_without_timer() {
        let mut paused = state(track());
        paused.playing = false;
        // По умолчанию на паузе статус очищается.
        assert!(activity(&paused).is_none());
        paused.settings.on_pause = true;
        let value = json_of(&paused);
        assert!(value.get("timestamps").is_none());
        assert_eq!(value["state"], "Король и Шут (пауза)");
        paused.settings.lang = Lang::En;
        assert_eq!(json_of(&paused)["state"], "Король и Шут (paused)");
        // Пометка переживает обрезку длинных исполнителей.
        let mut long = track();
        long.artists = "Ф".repeat(300);
        paused.track = Some(long);
        let text = json_of(&paused)["state"].as_str().unwrap().to_owned();
        assert_eq!(text.chars().count(), 128);
        assert!(text.ends_with("(paused)"));
    }

    #[test]
    fn local_file_has_no_cover_or_links() {
        let mut t = track();
        t.local = true;
        let value = json_of(&state(t));
        assert_eq!(value["details"], "Дурак и молния");
        assert!(value.get("assets").is_none());
        assert!(value.get("buttons").is_none());
        assert!(value.get("details_url").is_none());
    }

    #[test]
    fn button_and_status_follow_settings() {
        let mut s = state(track());
        s.settings.button = false;
        s.settings.status = StatusKind::Track;
        s.settings.lang = Lang::En;
        let value = json_of(&s);
        assert!(value.get("buttons").is_none());
        assert_eq!(value["status_display_type"], 2);
        s.settings.button = true;
        assert_eq!(json_of(&s)["buttons"][0]["label"], "Listen");
        s.settings.status = StatusKind::App;
        assert_eq!(json_of(&s)["status_display_type"], 0);
    }

    #[test]
    fn disabled_or_empty_state_clears() {
        let mut s = state(track());
        s.settings.enabled = false;
        assert!(activity(&s).is_none());
        assert!(activity(&State::default()).is_none());
    }

    #[test]
    fn urls_are_built_for_yandex_only() {
        assert_eq!(
            cover_url("avatars.yandex.net/get-music-content/38044/abc/%%").as_deref(),
            Some("https://avatars.yandex.net/get-music-content/38044/abc/400x400")
        );
        assert_eq!(
            cover_url("music-avatars.music.yandex.net/get-music-content/38044/abc/%%").as_deref(),
            Some("https://music-avatars.music.yandex.net/get-music-content/38044/abc/400x400")
        );
        assert_eq!(cover_url("avatars.example/abc/%%"), None);
        assert_eq!(cover_url("avatars.yandex.net.evil.example/%%"), None);
        assert_eq!(cover_url("user@avatars.yandex.net/%%"), None);
        assert_eq!(cover_url("avatars.yandex.net/no-size"), None);
        let mut t = track();
        assert_eq!(track_url(&t), "https://music.yandex.ru/album/7/track/42");
        t.album_id = None;
        assert_eq!(track_url(&t), "https://music.yandex.ru/track/42");
    }

    #[test]
    fn resend_only_on_change_or_big_shift() {
        let base = activity(&state(track()));
        // Ничего не изменилось.
        assert!(!should_send(&base, &base.clone()));
        assert!(!should_send(&None, &None));
        // Мелкий дрейф таймера — не повод.
        let mut s = state(track());
        s.start_ms += 1500;
        assert!(!should_send(&base, &activity(&s)));
        // Перемотка больше 2 с — пересчёт.
        s.start_ms += 1000;
        assert!(should_send(&base, &activity(&s)));
        // Другой трек.
        let mut other = track();
        other.title = "Другой".to_owned();
        assert!(should_send(&base, &activity(&state(other))));
        // Пауза: очистка, потом снова игра.
        assert!(should_send(&base, &None));
        assert!(should_send(&None, &base));
        // Другая длительность при том же начале.
        let mut longer = track();
        longer.duration_ms = Some(200_000);
        assert!(should_send(&base, &activity(&state(longer))));
    }

    #[test]
    fn app_id_must_be_numeric() {
        // Без отладочной переменной: неверное значение из конфига — встроенный ID.
        if std::env::var("PLASTINKA_TEST_DISCORD_APP_ID").is_err() {
            let plastinka = AppName::Plastinka;
            assert_eq!(app_id(None, plastinka), DEFAULT_DISCORD_APP_ID);
            assert_eq!(app_id(Some("  1234567890  "), plastinka), "1234567890");
            assert_eq!(app_id(Some("abc"), plastinka), DEFAULT_DISCORD_APP_ID);
            assert_eq!(
                app_id(Some(&"1".repeat(40)), plastinka),
                DEFAULT_DISCORD_APP_ID
            );
            // Своё приложение из конфига важнее выбранного имени.
            assert_eq!(app_id(Some("42"), AppName::YandexRu), "42");
            // Имя без приложения — «Plastinka».
            for name in AppName::ALL {
                let expected = if name.available() {
                    name.builtin_id()
                } else {
                    DEFAULT_DISCORD_APP_ID
                };
                assert_eq!(app_id(None, name), expected);
            }
        }
    }

    #[test]
    fn keys_round_trip() {
        for kind in StatusKind::ALL {
            assert_eq!(StatusKind::from_key(kind.key()), Some(kind));
        }
        for name in AppName::ALL {
            assert_eq!(AppName::from_key(name.key()), Some(name));
        }
        assert!(AppName::Plastinka.available());
        for lang in Lang::ALL {
            assert_eq!(Lang::from_key(lang.key()), Some(lang));
        }
        assert_eq!(StatusKind::from_key("??"), None);
    }
}
