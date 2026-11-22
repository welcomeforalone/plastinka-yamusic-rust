//! YandexMusicTime: время прослушивания (день, неделя, месяц, год, всего,
//! прошлая сессия) и серия дней подряд, когда пользователь открывал
//! приложение. Только логика: сеть не нужна, аудио не трогаем.
//!
//! Данные — секунды по дням и счётчик серии — лежат в `listen_time.json`
//! рядом с `config.toml`. Модуль не знает, где взять «сегодня» (UTC или
//! местное время), день всегда приходит параметром.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use serde_json::{Map, Value};

use crate::api::{Ymd, civil_from_days, days_from_civil, parse_date, today_utc};
use crate::storage::{AppDirs, StorageError};

const VERSION: u64 = 1;
const FILE_NAME: &str = "listen_time.json";
const BROKEN_NAME: &str = "listen_time.broken.json";
/// Файл больше — не наш.
const MAX_FILE: u64 = 4 * 1024 * 1024;

/// Одна запись за раз: общий временный файл у параллельных сохранений.
static WRITE: Mutex<()> = Mutex::new(());

// ------------------------------------------------------------------ даты

/// День существует в календаре (туда и обратно через номер дня).
fn valid_day(day: Ymd) -> bool {
    civil_from_days(days_from_civil(day)) == day
}

/// Номер дня недели: понедельник — 0, воскресенье — 6. 1970-01-01 — четверг.
fn weekday(day_number: i64) -> i64 {
    (day_number + 3).rem_euclid(7)
}

/// Сегодня по местным часам (граница дня — местная полночь). Не удалось
/// получить дату — UTC.
pub fn today_local() -> Ymd {
    use chrono::Datelike;
    let date = chrono::Local::now().date_naive();
    let day = (date.year(), date.month(), date.day());
    if valid_day(day) { day } else { today_utc() }
}

/// За какой период показывать время в заголовке окна.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Period {
    #[default]
    Total,
    Year,
    Month,
    Week,
    Day,
    /// Текущий запуск приложения.
    Session,
}

impl Period {
    pub const ALL: [Self; 6] = [
        Self::Total,
        Self::Year,
        Self::Month,
        Self::Week,
        Self::Day,
        Self::Session,
    ];

    /// Значение в `config.toml`.
    pub fn key(self) -> &'static str {
        match self {
            Self::Total => "total",
            Self::Year => "year",
            Self::Month => "month",
            Self::Week => "week",
            Self::Day => "day",
            Self::Session => "session",
        }
    }

    /// Из `config.toml`; неизвестная строка — `None`.
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|period| period.key() == text)
    }

    /// Подпись перед временем в заголовке.
    pub fn label(self) -> &'static str {
        match self {
            Self::Total => "Время",
            Self::Year => "За год",
            Self::Month => "За месяц",
            Self::Week => "За неделю",
            Self::Day => "За день",
            Self::Session => "Сессия",
        }
    }

    /// Короткое название для переключателя в настройках.
    pub fn short(self) -> &'static str {
        match self {
            Self::Total => "Всего",
            Self::Year => "Год",
            Self::Month => "Месяц",
            Self::Week => "Неделя",
            Self::Day => "День",
            Self::Session => "Сессия",
        }
    }
}

// ------------------------------------------------------------- статистика

/// Время по периодам, секунды.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Totals {
    pub day: u64,
    pub week: u64,
    pub month: u64,
    pub year: u64,
    pub total: u64,
}

/// Неделя с понедельника по воскресенье.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Week {
    pub monday: Ymd,
    /// Секунды по дням Пн…Вс; дни после «сегодня» — 0.
    pub days: [u64; 7],
    pub total: u64,
}

/// Серия дней подряд.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Streak {
    pub count: u64,
    pub best: u64,
    pub last: Option<Ymd>,
}

impl Streak {
    /// Чинит битое состояние: недопустимая дата — серия заново, нулевая
    /// серия при известном дне — единица, рекорд не меньше серии.
    fn normalize(&mut self) {
        match self.last {
            Some(last) if !valid_day(last) => {
                self.last = None;
                self.count = 0;
            }
            Some(_) if self.count == 0 => self.count = 1,
            _ => {}
        }
        self.best = self.best.max(self.count);
    }

    /// Отметка «сегодня приложение открыто». `true` — состояние изменилось.
    pub fn visit(&mut self, day: Ymd) -> bool {
        let before = *self;
        self.normalize();
        if valid_day(day) {
            match self.last {
                None => {
                    self.count = 1;
                    self.last = Some(day);
                }
                Some(last) => {
                    let (last_n, day_n) = (days_from_civil(last), days_from_civil(day));
                    if day_n == last_n + 1 {
                        self.count = self.count.saturating_add(1);
                        self.last = Some(day);
                    } else if day_n > last_n {
                        self.count = 1;
                        self.last = Some(day);
                    }
                    // Тот же день или раньше: ничего не меняем.
                }
            }
            self.best = self.best.max(self.count);
        }
        *self != before
    }
}

/// Данные плагина.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ListenStats {
    /// Секунды прослушивания за каждый день.
    pub days: BTreeMap<Ymd, u64>,
    /// Длина прошлой сессии, секунды.
    pub previous_session: u64,
    pub streak: Streak,
}

impl ListenStats {
    /// Прибавляет секунды к дню. Ноль — ничего не делает.
    pub fn add(&mut self, day: Ymd, secs: u64) {
        if secs == 0 {
            return;
        }
        let entry = self.days.entry(day).or_insert(0);
        *entry = entry.saturating_add(secs);
    }

    /// Суммы по периодам. Дни после `today` входят только в `total`.
    pub fn totals(&self, today: Ymd) -> Totals {
        let today_n = days_from_civil(today);
        let monday_n = today_n - weekday(today_n);
        let mut totals = Totals::default();
        for (&day, &secs) in &self.days {
            totals.total = totals.total.saturating_add(secs);
            let n = days_from_civil(day);
            if n > today_n {
                continue;
            }
            if day == today {
                totals.day = totals.day.saturating_add(secs);
            }
            if n >= monday_n {
                totals.week = totals.week.saturating_add(secs);
            }
            if day.0 == today.0 {
                totals.year = totals.year.saturating_add(secs);
                if day.1 == today.1 {
                    totals.month = totals.month.saturating_add(secs);
                }
            }
        }
        totals
    }

    /// Неделя, в которую входит `today`.
    pub fn week(&self, today: Ymd) -> Week {
        let today_n = days_from_civil(today);
        let monday_n = today_n - weekday(today_n);
        let mut days = [0_u64; 7];
        let mut total = 0_u64;
        for (offset, slot) in (0_i64..).zip(days.iter_mut()) {
            let n = monday_n + offset;
            if n > today_n {
                break;
            }
            *slot = self
                .days
                .get(&civil_from_days(n))
                .copied()
                .unwrap_or_default();
            total = total.saturating_add(*slot);
        }
        Week {
            monday: civil_from_days(monday_n),
            days,
            total,
        }
    }

    /// Сбрасывает время, но не серию и не рекорд.
    pub fn reset_time(&mut self) {
        self.days.clear();
        self.previous_session = 0;
    }
}

// ----------------------------------------------------------------- огонёк

/// Вид огонька серии.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flame {
    pub level: u32,
    /// Основной цвет.
    pub color: (u8, u8, u8),
    /// Светлая сердцевина того же оттенка.
    pub core: (u8, u8, u8),
}

/// Огонёк по числу дней: 0–9 — уровень 1, 10–49 — 2, 50–99 — 3, 100–199 — 4,
/// 200–299 — 5 и дальше +1 за каждую сотню. Ноль дней — тот же уровень 1,
/// но серый.
pub fn flame(days: u64) -> Flame {
    let level = match days {
        0..=9 => 1,
        10..=49 => 2,
        50..=99 => 3,
        100..=199 => 4,
        _ => u32::try_from(days / 100)
            .unwrap_or(u32::MAX)
            .saturating_add(3),
    };
    if days == 0 {
        return Flame {
            level,
            color: (128, 128, 128),
            core: (200, 200, 200),
        };
    }
    // (оттенок в градусах, насыщенность, яркость).
    let (hue, sat, light) = match level {
        1 => (28.0, 0.95, 0.52),
        2 => (10.0, 0.90, 0.50),
        3 => (275.0, 0.70, 0.58),
        4 => (175.0, 0.75, 0.45),
        _ => {
            // Золотой угол от синего: соседние уровни заметно различаются.
            let step = f64::from(level.saturating_sub(5));
            ((220.0 + step * 137.5).rem_euclid(360.0), 0.80, 0.55)
        }
    };
    Flame {
        level,
        color: hsl_to_rgb(hue, sat, light),
        core: hsl_to_rgb(hue, sat * 0.8, (light + 0.28).min(0.9)),
    }
}

/// HSL в RGB: оттенок в градусах, остальное от 0 до 1.
fn hsl_to_rgb(hue: f64, sat: f64, light: f64) -> (u8, u8, u8) {
    let chroma = (1.0 - (2.0 * light - 1.0).abs()) * sat;
    let sector = hue.rem_euclid(360.0) / 60.0;
    let x = chroma * (1.0 - (sector.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match sector {
        s if s < 1.0 => (chroma, x, 0.0),
        s if s < 2.0 => (x, chroma, 0.0),
        s if s < 3.0 => (0.0, chroma, x),
        s if s < 4.0 => (0.0, x, chroma),
        s if s < 5.0 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let shift = light - chroma / 2.0;
    // Значение зажато в 0..=255, приведение без потерь.
    let byte = |value: f64| ((value + shift) * 255.0).round().clamp(0.0, 255.0) as u8;
    (byte(r), byte(g), byte(b))
}

// ----------------------------------------------------------- форматирование

fn hms(secs: u64) -> (u64, u64, u64) {
    (secs / 3600, secs % 3600 / 60, secs % 60)
}

/// «1ч 5м 03с» или «1ч 5м»; без часов — «5м 03с».
pub fn format_short(secs: u64, with_seconds: bool) -> String {
    let (h, m, s) = hms(secs);
    let mut parts = Vec::new();
    if h > 0 {
        parts.push(format!("{h}ч"));
    }
    if h > 0 || m > 0 || !with_seconds {
        parts.push(format!("{m}м"));
    }
    if with_seconds {
        parts.push(format!("{s:02}с"));
    }
    parts.join(" ")
}

/// «01 ч 05 м 03 с» для экрана статистики.
pub fn format_long(secs: u64) -> String {
    let (h, m, s) = hms(secs);
    format!("{h:02} ч {m:02} м {s:02} с")
}

/// «1 день», «3 дня», «11 дней».
pub fn days_text(n: u64) -> String {
    let word = crate::texts::plural(
        usize::try_from(n).unwrap_or(usize::MAX),
        ["день", "дня", "дней"],
    );
    format!("{n} {word}")
}

/// Текст для буфера обмена.
pub fn share_text(totals: &Totals, week: &Week, streak: &Streak) -> String {
    const NAMES: [&str; 7] = ["Пн", "Вт", "Ср", "Чт", "Пт", "Сб", "Вс"];
    let by_day = NAMES
        .iter()
        .zip(week.days.iter())
        .map(|(name, &secs)| format!("{name} {}", format_short(secs, false)))
        .collect::<Vec<_>>()
        .join(", ");
    [
        "Моя статистика прослушиваний в Plastinka".to_owned(),
        format!("Сегодня: {}", format_long(totals.day)),
        format!("Неделя: {} ({by_day})", format_long(week.total)),
        format!("Месяц: {}", format_long(totals.month)),
        format!("Год: {}", format_long(totals.year)),
        format!("Всего: {}", format_long(totals.total)),
        format!(
            "Серия: {}, рекорд: {}",
            days_text(streak.count),
            days_text(streak.best)
        ),
    ]
    .join("\n")
}

// ------------------------------------------------------------------- файл

/// Файл в каталоге настроек. В тестах, с тестовыми хуками и без каталога —
/// `None`.
fn system_path() -> Option<PathBuf> {
    if cfg!(test) || !super::may_write() {
        return None;
    }
    AppDirs::discover()
        .ok()
        .map(|dirs| dirs.config_dir().join(FILE_NAME))
}

/// Статистика с диска. Блокирующая: вызывать через `storage::blocking`.
pub fn load() -> ListenStats {
    system_path().map_or_else(ListenStats::default, |path| load_from(&path))
}

/// Записывает статистику. Блокирующая.
pub fn save(stats: &ListenStats) -> Result<(), StorageError> {
    match system_path() {
        Some(path) => save_to(&path, stats),
        None => Ok(()),
    }
}

fn date_text((year, month, day): Ymd) -> String {
    format!("{year:04}-{month:02}-{day:02}")
}

/// Строго `YYYY-MM-DD` и существующий день.
fn parse_day(text: &str) -> Option<Ymd> {
    if text.len() != 10 {
        return None;
    }
    parse_date(text).filter(|&day| valid_day(day))
}

fn encode(stats: &ListenStats) -> Value {
    let days: Map<String, Value> = stats
        .days
        .iter()
        // Год вне 0000–9999 обратно не прочитать.
        .filter(|((year, _, _), _)| (0..=9999).contains(year))
        .map(|(&day, &secs)| (date_text(day), Value::from(secs)))
        .collect();
    let mut streak = Map::new();
    streak.insert("count".into(), Value::from(stats.streak.count));
    streak.insert("best".into(), Value::from(stats.streak.best));
    if let Some(last) = stats.streak.last {
        streak.insert("last".into(), Value::from(date_text(last)));
    }
    let mut root = Map::new();
    root.insert("v".into(), Value::from(VERSION));
    root.insert("days".into(), Value::Object(days));
    root.insert("prev".into(), Value::from(stats.previous_session));
    root.insert("streak".into(), Value::Object(streak));
    Value::Object(root)
}

/// Разбор файла. `None` — файл не наш или поля неверные.
fn decode(value: &Value) -> Option<ListenStats> {
    if value.get("v")?.as_u64()? != VERSION {
        return None;
    }
    let mut stats = ListenStats::default();
    if let Some(days) = value.get("days") {
        for (key, secs) in days.as_object()? {
            // Битая запись дня пропускается.
            if let (Some(day), Some(secs)) = (parse_day(key), secs.as_u64()) {
                stats.add(day, secs);
            }
        }
    }
    if let Some(prev) = value.get("prev") {
        stats.previous_session = prev.as_u64()?;
    }
    if let Some(streak) = value.get("streak") {
        let streak = streak.as_object()?;
        stats.streak.count = streak.get("count").map_or(Some(0), Value::as_u64)?;
        stats.streak.best = streak.get("best").map_or(Some(0), Value::as_u64)?;
        stats.streak.last = match streak.get("last") {
            None | Some(Value::Null) => None,
            Some(last) => {
                let last = last.as_str()?;
                if parse_day(last).is_none() {
                    // Неверная дата: серия заново, рекорд остаётся.
                    stats.streak.count = 0;
                }
                parse_day(last)
            }
        };
        stats.streak.normalize();
    }
    Some(stats)
}

/// Статистика в формате `listen_time.json`, с отступами: для экспорта.
pub fn to_json(stats: &ListenStats) -> Vec<u8> {
    serde_json::to_vec_pretty(&encode(stats)).unwrap_or_default()
}

/// Разбор копии статистики. Файл не трогает: он может быть чужим.
/// `None` — не наш формат.
pub fn from_json(bytes: &[u8]) -> Option<ListenStats> {
    let value = serde_json::from_slice::<Value>(bytes).ok()?;
    decode(&value)
}

/// Не наш файл уходит в `listen_time.broken.json`, а не затирается.
fn set_aside(path: &Path) {
    let _ = std::fs::rename(path, path.with_file_name(BROKEN_NAME));
}

fn load_from(path: &Path) -> ListenStats {
    let Ok(meta) = std::fs::metadata(path) else {
        return ListenStats::default();
    };
    if meta.len() > MAX_FILE {
        set_aside(path);
        return ListenStats::default();
    }
    // Не прочиталось (занят, нет прав): файл не трогаем.
    let Ok(bytes) = std::fs::read(path) else {
        return ListenStats::default();
    };
    from_json(&bytes).unwrap_or_else(|| {
        set_aside(path);
        ListenStats::default()
    })
}

fn save_to(path: &Path, stats: &ListenStats) -> Result<(), StorageError> {
    let io_error = |source| StorageError::ConfigIo {
        path: path.to_path_buf(),
        source,
    };
    let text = serde_json::to_vec(&encode(stats)).map_err(|_| StorageError::Interrupted)?;
    let _guard = WRITE.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io_error)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(io_error)?;
    std::fs::rename(&tmp, path).map_err(io_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("plastinka-listen-{name}-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn file(&self) -> PathBuf {
            self.0.join(FILE_NAME)
        }

        fn broken(&self) -> PathBuf {
            self.0.join(BROKEN_NAME)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn totals_and_week_boundaries() {
        // 2026-11-22 — воскресенье, неделя 16–22 ноября.
        let today = (2026, 11, 22);
        let mut stats = ListenStats::default();
        stats.add((2026, 11, 15), 1); // воскресенье прошлой недели
        stats.add((2026, 11, 16), 10); // понедельник
        stats.add((2026, 11, 22), 100);
        stats.add((2026, 11, 1), 1000);
        stats.add((2026, 10, 31), 10_000);
        stats.add((2025, 12, 31), 100_000);
        stats.add((2026, 11, 25), 1_000_000); // из будущего
        let totals = stats.totals(today);
        assert_eq!(totals.day, 100);
        assert_eq!(totals.week, 110);
        assert_eq!(totals.month, 1111);
        assert_eq!(totals.year, 11_111);
        assert_eq!(totals.total, 1_111_111);
        let week = stats.week(today);
        assert_eq!(week.monday, (2026, 11, 16));
        assert_eq!(week.days, [10, 0, 0, 0, 0, 0, 100]);
        assert_eq!(week.total, 110);

        // Понедельник: в неделе только он сам.
        let monday = stats.totals((2026, 11, 16));
        assert_eq!(monday.week, 10);
        assert_eq!(monday.day, 10);
        assert_eq!(stats.week((2026, 11, 16)).days, [10, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn week_crosses_month_year_and_leap_day() {
        let mut stats = ListenStats::default();
        // 2025-12-29 — понедельник, 2026-01-02 — пятница.
        stats.add((2025, 12, 29), 1);
        stats.add((2025, 12, 31), 2);
        stats.add((2026, 1, 2), 4);
        let week = stats.week((2026, 1, 2));
        assert_eq!(week.monday, (2025, 12, 29));
        assert_eq!(week.days, [1, 0, 2, 0, 4, 0, 0]);
        let totals = stats.totals((2026, 1, 2));
        assert_eq!((totals.week, totals.month, totals.year), (7, 4, 4));

        // Високосный 2024: 29 февраля — четверг, неделя 26 февраля — 3 марта.
        let mut leap = ListenStats::default();
        leap.add((2024, 2, 29), 5);
        leap.add((2024, 3, 1), 7);
        let week = leap.week((2024, 3, 1));
        assert_eq!(week.monday, (2024, 2, 26));
        assert_eq!(week.days, [0, 0, 0, 5, 7, 0, 0]);
        // 2023-02-28 — вторник, а 2023-03-01 — среда: 29 февраля нет.
        assert_eq!(leap.week((2023, 3, 1)).monday, (2023, 2, 27));
    }

    #[test]
    fn add_ignores_zero_and_saturates() {
        let mut stats = ListenStats::default();
        stats.add((2026, 1, 1), 0);
        assert!(stats.days.is_empty());
        stats.add((2026, 1, 1), u64::MAX);
        stats.add((2026, 1, 1), 5);
        assert_eq!(stats.days.get(&(2026, 1, 1)), Some(&u64::MAX));
        assert_eq!(stats.totals((2026, 1, 1)).total, u64::MAX);
    }

    #[test]
    fn reset_keeps_streak() {
        let mut stats = ListenStats::default();
        stats.add((2026, 1, 1), 50);
        stats.previous_session = 9;
        stats.streak.visit((2026, 1, 1));
        stats.reset_time();
        assert!(stats.days.is_empty());
        assert_eq!(stats.previous_session, 0);
        assert_eq!(stats.streak.count, 1);
        assert_eq!(stats.streak.best, 1);
        assert_eq!(stats.streak.last, Some((2026, 1, 1)));
    }

    #[test]
    fn streak_transitions() {
        let mut streak = Streak::default();
        assert!(streak.visit((2026, 2, 27)));
        assert_eq!((streak.count, streak.best), (1, 1));
        // Тот же день и раньше — без изменений.
        assert!(!streak.visit((2026, 2, 27)));
        assert!(!streak.visit((2026, 2, 1)));
        assert_eq!(streak.last, Some((2026, 2, 27)));
        // Следующий день, через конец месяца.
        assert!(streak.visit((2026, 2, 28)));
        assert!(streak.visit((2026, 3, 1)));
        assert_eq!((streak.count, streak.best), (3, 3));
        // Пропуск дня: серия заново, рекорд остаётся.
        assert!(streak.visit((2026, 3, 3)));
        assert_eq!((streak.count, streak.best), (1, 3));
        // Високосный день.
        let mut leap = Streak::default();
        leap.visit((2024, 2, 28));
        leap.visit((2024, 2, 29));
        leap.visit((2024, 3, 1));
        assert_eq!(leap.count, 3);
        // Конец года.
        leap.visit((2024, 3, 3));
        let mut year = Streak::default();
        year.visit((2025, 12, 31));
        year.visit((2026, 1, 1));
        assert_eq!(year.count, 2);
    }

    #[test]
    fn broken_streak_is_normalized() {
        let mut streak = Streak {
            count: 0,
            best: 0,
            last: Some((2026, 5, 1)),
        };
        assert!(streak.visit((2026, 5, 2)));
        assert_eq!((streak.count, streak.best), (2, 2));

        let mut low_best = Streak {
            count: 7,
            best: 3,
            last: Some((2026, 5, 1)),
        };
        assert!(low_best.visit((2026, 5, 1)));
        assert_eq!(low_best.best, 7);

        let mut bad_date = Streak {
            count: 4,
            best: 4,
            last: Some((2026, 2, 30)),
        };
        bad_date.visit((2026, 3, 1));
        assert_eq!((bad_date.count, bad_date.best), (1, 4));
        assert_eq!(bad_date.last, Some((2026, 3, 1)));

        // Недопустимый «сегодня» состояние не портит.
        let mut good = Streak {
            count: 2,
            best: 2,
            last: Some((2026, 5, 1)),
        };
        assert!(!good.visit((2026, 13, 40)));
        assert_eq!(good.count, 2);
    }

    #[test]
    fn flame_levels() {
        let level = |days| flame(days).level;
        assert_eq!(level(0), 1);
        assert_eq!(level(9), 1);
        assert_eq!(level(10), 2);
        assert_eq!(level(49), 2);
        assert_eq!(level(50), 3);
        assert_eq!(level(99), 3);
        assert_eq!(level(100), 4);
        assert_eq!(level(199), 4);
        assert_eq!(level(200), 5);
        assert_eq!(level(299), 5);
        assert_eq!(level(300), 6);
        assert_eq!(level(1000), 13);
        assert_eq!(level(u64::MAX), u32::MAX);
        // Серый без серии; цвета уровней различаются.
        assert_eq!(flame(0).color, (128, 128, 128));
        let colors: Vec<_> = [1, 10, 50, 100, 200, 300, 400]
            .into_iter()
            .map(|days| flame(days).color)
            .collect();
        for (i, a) in colors.iter().enumerate() {
            for b in colors.iter().skip(i + 1) {
                assert_ne!(a, b);
            }
        }
        // Сердцевина светлее основного цвета на всех уровнях.
        for days in [1, 10, 50, 100, 200, 300, 400] {
            let f = flame(days);
            let sum = |c: (u8, u8, u8)| u32::from(c.0) + u32::from(c.1) + u32::from(c.2);
            assert!(sum(f.core) > sum(f.color), "{days}");
        }
        // Огромное число не паникует.
        let _ = flame(u64::MAX);
    }

    #[test]
    fn hsl_primary_colors() {
        assert_eq!(hsl_to_rgb(0.0, 1.0, 0.5), (255, 0, 0));
        assert_eq!(hsl_to_rgb(120.0, 1.0, 0.5), (0, 255, 0));
        assert_eq!(hsl_to_rgb(240.0, 1.0, 0.5), (0, 0, 255));
        assert_eq!(hsl_to_rgb(0.0, 0.0, 1.0), (255, 255, 255));
    }

    #[test]
    fn formatting() {
        assert_eq!(format_short(3903, true), "1ч 5м 03с");
        assert_eq!(format_short(3903, false), "1ч 5м");
        assert_eq!(format_short(303, true), "5м 03с");
        assert_eq!(format_short(3, true), "03с");
        assert_eq!(format_short(0, false), "0м");
        assert_eq!(format_short(3600, true), "1ч 0м 00с");
        assert_eq!(format_long(3903), "01 ч 05 м 03 с");
        assert_eq!(format_long(0), "00 ч 00 м 00 с");
        assert_eq!(format_long(360_000), "100 ч 00 м 00 с");
        assert_eq!(days_text(1), "1 день");
        assert_eq!(days_text(3), "3 дня");
        assert_eq!(days_text(11), "11 дней");
        assert_eq!(days_text(21), "21 день");
        assert_eq!(days_text(0), "0 дней");
    }

    #[test]
    fn share_text_has_periods_and_streak() {
        let mut stats = ListenStats::default();
        stats.add((2026, 11, 22), 3903);
        stats.streak.visit((2026, 11, 22));
        let today = (2026, 11, 22);
        let text = share_text(&stats.totals(today), &stats.week(today), &stats.streak);
        assert!(text.starts_with("Моя статистика прослушиваний в Plastinka"));
        assert!(text.contains("Сегодня: 01 ч 05 м 03 с"));
        assert!(text.contains("Вс 1ч 5м"));
        assert!(text.contains("Серия: 1 день, рекорд: 1 день"));
        assert!(!text.contains("Яндекс"));
    }

    #[test]
    fn stats_survive_save_and_load() {
        let dir = TempDir::new("roundtrip");
        let mut stats = ListenStats::default();
        stats.add((2026, 11, 22), 3600);
        stats.add((2024, 2, 29), 7);
        stats.previous_session = 123;
        stats.streak = Streak {
            count: 5,
            best: 10,
            last: Some((2026, 11, 22)),
        };
        save_to(&dir.file(), &stats).unwrap();
        assert_eq!(load_from(&dir.file()), stats);
        assert!(!dir.file().with_extension("json.tmp").exists());
        let text = std::fs::read_to_string(dir.file()).unwrap();
        assert!(text.contains(r#""2026-11-22":3600"#), "{text}");
        assert!(text.contains(r#""last":"2026-11-22""#), "{text}");
        // Повторная запись заменяет файл.
        stats.reset_time();
        save_to(&dir.file(), &stats).unwrap();
        assert_eq!(load_from(&dir.file()), stats);
        assert!(!dir.broken().exists());
    }

    #[test]
    fn json_export_round_trips_without_touching_files() {
        let mut stats = ListenStats::default();
        stats.add((2026, 11, 22), 3600);
        stats.previous_session = 5;
        stats.streak.visit((2026, 11, 22));
        assert_eq!(from_json(&to_json(&stats)), Some(stats));
        assert_eq!(from_json(b"not json"), None);
        assert_eq!(from_json(br#"{"v":9}"#), None);
    }

    #[test]
    fn missing_file_is_empty_and_nothing_is_created() {
        let dir = TempDir::new("missing");
        assert_eq!(load_from(&dir.file()), ListenStats::default());
        assert!(!dir.broken().exists());
    }

    #[test]
    fn broken_file_is_set_aside() {
        for body in [
            "не json",
            "",
            "{}",
            r#"{"v":2,"days":{}}"#,
            r#"{"v":1,"days":5}"#,
            r#"{"v":1,"prev":"x"}"#,
            r#"{"v":1,"streak":{"count":"a"}}"#,
            "[1,2,3]",
        ] {
            let dir = TempDir::new("broken");
            std::fs::write(dir.file(), body).unwrap();
            assert_eq!(load_from(&dir.file()), ListenStats::default(), "{body}");
            assert!(!dir.file().exists(), "{body}");
            assert_eq!(std::fs::read_to_string(dir.broken()).unwrap(), body);
        }
        // Новый битый файл заменяет прошлый «broken».
        let dir = TempDir::new("twice");
        std::fs::write(dir.file(), "первый").unwrap();
        load_from(&dir.file());
        std::fs::write(dir.file(), "второй").unwrap();
        load_from(&dir.file());
        assert_eq!(std::fs::read_to_string(dir.broken()).unwrap(), "второй");
        // Слишком большой файл — тоже не наш.
        let dir = TempDir::new("huge");
        std::fs::write(
            dir.file(),
            vec![b' '; usize::try_from(MAX_FILE).unwrap() + 1],
        )
        .unwrap();
        assert_eq!(load_from(&dir.file()), ListenStats::default());
        assert!(dir.broken().exists());
    }

    #[test]
    fn bad_day_entries_are_skipped() {
        let dir = TempDir::new("baddays");
        let body = r#"{"v":1,"days":{
            "2026-11-22":60,"2026-13-01":5,"2026-02-30":6,"мусор":7,
            "2026-11-23":"x","2026-11-24":-1,"2026-11-25xx":9,"2026-11-26":2
        },"prev":4,"streak":{"count":3,"best":2,"last":"2026-11-22"}}"#;
        std::fs::write(dir.file(), body).unwrap();
        let stats = load_from(&dir.file());
        let days: Vec<_> = stats.days.iter().map(|(&d, &s)| (d, s)).collect();
        assert_eq!(days, [((2026, 11, 22), 60), ((2026, 11, 26), 2)]);
        assert_eq!(stats.previous_session, 4);
        // Рекорд подтянут до серии.
        assert_eq!((stats.streak.count, stats.streak.best), (3, 3));
        assert!(!dir.broken().exists());

        // Неверная дата серии: серия заново, рекорд остаётся.
        let body = r#"{"v":1,"streak":{"count":3,"best":8,"last":"2026-02-30"}}"#;
        std::fs::write(dir.file(), body).unwrap();
        let stats = load_from(&dir.file());
        assert_eq!(
            stats.streak,
            Streak {
                count: 0,
                best: 8,
                last: None
            }
        );
    }
}
