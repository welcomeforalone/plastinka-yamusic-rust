//! YandexMusicTime в окне: учёт секунд, пока звук реально идёт, серия дней,
//! запись `listen_time.json`, таймер в заголовке окна и экран «Статистика»
//! (сброс, экспорт и восстановление из файла).
//!
//! Время берётся по монотонным часам (`Instant`): целые секунды переносятся
//! в статистику на каждом тике и при остановке звука, остаток меньше секунды
//! не теряется. Файл пишется в фоне раз в [`SAVE_EVERY`], при выключении
//! плагина и при выходе (синхронно: файл маленький). Пока файл не прочитан,
//! секунды копятся в памяти и потом прибавляются к прочитанному, а на диск
//! до чтения ничего не уходит, чтобы не затереть прошлые запуски.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use iced::time::Instant;
use iced::widget::{Space, column, container, row, scrollable};
use iced::{Alignment, Border, Color, Element, Length, Padding, Task};

use super::{Effect, Library, Message, PAGE_SIDE, PAGE_TOP};
use crate::api::{Ymd, days_from_civil};
use crate::plugins::PluginSettings;
use crate::plugins::listen_time::{
    self, ListenStats, Period, Totals, Week, days_text, flame, format_long, format_short,
    share_text, today_local,
};
use crate::storage::{self, StorageError};
use crate::ui::Dialog;
use crate::ui::Message as App;
use crate::ui::gate;
use crate::ui::icons::{Icon, icon, spinner};
use crate::ui::scroll;
use crate::ui::theme::{self, Style};
use crate::ui::widgets::{self, Kind};

/// Как часто пишем файл, если есть изменения.
const SAVE_EVERY: Duration = Duration::from_secs(30);
/// Тик, пока плагин включён, а звука нет: смена дня для серии.
const IDLE_TICK: Duration = Duration::from_secs(60);
/// Разрыв между двумя отметками времени больше этого — сон или гибернация
/// компьютера, зависшее окно: такой кусок не засчитывается. Короткие
/// остановки цикла событий (перетаскивание окна, до двух минут) считаются.
const MAX_GAP: Duration = Duration::from_secs(120);
/// Размер файла копии, который готовы читать.
const MAX_BACKUP: u64 = 4 * 1024 * 1024;
/// Минимальная ширина карточки на экране статистики.
const CARD_MIN: f32 = 200.0;
const CARD_GAP: f32 = 12.0;
/// Размер огонька в заголовке.
const FLAME_SIZE: f32 = 14.0;
/// Отступ таймера от краёв окна: не заезжает на название и кнопки окна, а
/// центр остаётся центром окна.
const SIDE_GAP: f32 = 170.0;

#[derive(Clone)]
pub enum Msg {
    /// Файл прочитан.
    Loaded(Box<ListenStats>),
    /// Секундный (или минутный) тик.
    Tick,
    /// Фоновая запись закончилась: удалась ли.
    Saved(bool),
    /// Кнопки экрана «Статистика».
    Copy,
    Export,
    /// Копия сохранена: `Ok(None)` — окно выбора закрыли.
    Exported(Result<Option<String>, String>),
    Restore,
    /// Файл для восстановления разобран (или текст ошибки).
    RestorePicked(Result<Option<Box<ListenStats>>, String>),
    RestoreConfirmed,
    ResetConfirmed,
}

/// Номер последнего снимка и номер записанного: фоновая запись может прийти
/// не по порядку, старый снимок новый не затирает (как у истории).
static NEXT: AtomicU64 = AtomicU64::new(0);
static WRITTEN: Mutex<u64> = Mutex::new(0);

fn next_serial() -> u64 {
    NEXT.fetch_add(1, Ordering::Relaxed).wrapping_add(1)
}

/// Записывает снимок, если его никто не обогнал.
fn write_snapshot(stats: &ListenStats, serial: u64) -> Result<(), StorageError> {
    let mut written = WRITTEN.lock().unwrap_or_else(PoisonError::into_inner);
    if serial <= *written {
        return Ok(());
    }
    listen_time::save(stats)?;
    *written = serial;
    Ok(())
}

/// Звук реально идёт: не пауза, не буферизация, трек открыт в плеере.
pub(super) fn sounding(playing: bool, loaded: bool, buffering: bool) -> bool {
    playing && loaded && !buffering
}

/// Что показать в заголовке.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Title {
    pub time: Option<String>,
    /// Дней подряд.
    pub streak: Option<u64>,
}

/// «Дней подряд: 5» или «5».
fn streak_text(count: u64, label: bool) -> String {
    if label {
        format!("Дней подряд: {count}")
    } else {
        count.to_string()
    }
}

/// Отладочный хук `PLASTINKA_TEST_STREAK=<число>`: серия и рекорд на экране
/// и в заголовке равны этому числу. Только в отладочной сборке и с тестовыми
/// хуками; неверное число не действует.
fn streak_hook() -> Option<u64> {
    #[cfg(debug_assertions)]
    {
        if !storage::test_hooks_active() {
            return None;
        }
        std::env::var("PLASTINKA_TEST_STREAK")
            .ok()
            .and_then(|value| parse_streak(&value))
    }
    #[cfg(not(debug_assertions))]
    {
        None
    }
}

/// Число дней из хука; не число — `None`.
#[cfg(any(debug_assertions, test))]
fn parse_streak(text: &str) -> Option<u64> {
    text.trim().parse().ok()
}

/// Числа экрана «Статистика».
#[derive(Debug, Clone, PartialEq, Eq)]
struct StatsData {
    totals: Totals,
    week: Week,
    /// Номер сегодняшнего дня в неделе (Пн — 0).
    today_index: usize,
    session: u64,
    previous: u64,
    streak: u64,
    best: u64,
}

/// Строка недели.
#[derive(Debug, Clone, PartialEq)]
struct WeekRow {
    name: &'static str,
    /// «дд.мм».
    date: String,
    secs: u64,
    /// Доля самого длинного дня недели в промилле (0…1000).
    permille: u16,
    today: bool,
    future: bool,
}

/// Строки Пн…Вс: у пустой недели долей нет.
fn week_rows(week: &Week, today_index: usize) -> Vec<WeekRow> {
    const NAMES: [&str; 7] = [
        "Понедельник",
        "Вторник",
        "Среда",
        "Четверг",
        "Пятница",
        "Суббота",
        "Воскресенье",
    ];
    let longest = week.days.iter().copied().max().unwrap_or(0);
    let monday = days_from_civil(week.monday);
    NAMES
        .iter()
        .zip(week.days.iter())
        .zip(0_i64..)
        .enumerate()
        .map(|(index, ((&name, &secs), offset))| {
            let (_, month, day) = crate::api::civil_from_days(monday + offset);
            let permille = if longest == 0 {
                0
            } else {
                let value = u128::from(secs) * 1000 / u128::from(longest);
                u16::try_from(value).unwrap_or(1000)
            };
            WeekRow {
                name,
                date: format!("{day:02}.{month:02}"),
                secs,
                permille,
                today: index == today_index,
                future: index > today_index,
            }
        })
        .collect()
}

/// Состояние учёта.
pub(super) struct Listen {
    enabled: bool,
    stats: ListenStats,
    /// Файл прочитан: до этого не пишем и серию не отмечаем.
    loaded: bool,
    /// Секунды этого запуска.
    session: u64,
    /// С какого момента идут ещё не перенесённые секунды.
    since: Option<Instant>,
    /// Есть изменения, которых нет на диске.
    dirty: bool,
    /// Окно свёрнуто: серию не отмечаем.
    minimized: bool,
    saved_at: Instant,
    /// Последняя запись не удалась (тост показан один раз).
    failed: bool,
    /// Копия из файла ждёт подтверждения в диалоге и её описание для него.
    pending_restore: Option<Box<ListenStats>>,
    restore_text: String,
}

impl Listen {
    pub(super) fn new() -> Self {
        Self {
            enabled: false,
            stats: ListenStats::default(),
            loaded: false,
            session: 0,
            since: None,
            dirty: false,
            minimized: false,
            saved_at: Instant::now(),
            failed: false,
            pending_restore: None,
            restore_text: String::new(),
        }
    }

    /// Переносит целые секунды в статистику за `today`, остаток оставляет.
    fn fold(&mut self, now: Instant, today: Ymd) {
        let Some(since) = self.since else {
            return;
        };
        let gap = now.saturating_duration_since(since);
        if gap > MAX_GAP {
            // Сон компьютера или зависшее окно: это не время прослушивания.
            self.since = Some(now);
            return;
        }
        let whole = gap.as_secs();
        if whole == 0 {
            return;
        }
        self.stats.add(today, whole);
        self.session = self.session.saturating_add(whole);
        self.dirty = true;
        self.since = since.checked_add(Duration::from_secs(whole)).or(Some(now));
    }

    /// Звук пошёл или остановился (и тик): переносит секунды, начинает или
    /// заканчивает отсчёт.
    fn sync(&mut self, sounding: bool, now: Instant, today: Ymd) {
        if !self.enabled {
            self.since = None;
            return;
        }
        self.fold(now, today);
        match (sounding, self.since) {
            (true, None) => self.since = Some(now),
            (false, Some(_)) => self.since = None,
            _ => {}
        }
    }

    /// Плагин включили или выключили. Выключение переносит накопленное.
    fn set_enabled(&mut self, enabled: bool, now: Instant, today: Ymd) {
        if self.enabled && !enabled {
            self.fold(now, today);
            self.since = None;
        }
        self.enabled = enabled;
    }

    /// Отметка дня для серии.
    fn visit(&mut self, today: Ymd) {
        if self.enabled && self.loaded && !self.minimized && self.stats.streak.visit(today) {
            self.dirty = true;
        }
    }

    /// Файл прочитан: копившееся в памяти прибавляется к нему.
    fn loaded(&mut self, mut disk: ListenStats) {
        for (&day, &secs) in &self.stats.days {
            disk.add(day, secs);
        }
        if !self.stats.days.is_empty() {
            self.dirty = true;
        }
        self.stats = disk;
        self.loaded = true;
    }

    /// Что писать в файл: в `prev` — текущая сессия, если она была, иначе
    /// прежняя. Так «прошлая сессия» переживает и падение приложения.
    fn snapshot(&self) -> ListenStats {
        let mut stats = self.stats.clone();
        if self.session > 0 {
            stats.previous_session = self.session;
        }
        stats
    }

    /// Пора писать: есть что и прошло достаточно времени. Отмечает запись.
    fn take_due(&mut self, now: Instant) -> bool {
        let due =
            self.loaded && self.dirty && now.saturating_duration_since(self.saved_at) >= SAVE_EVERY;
        if due {
            self.dirty = false;
            self.saved_at = now;
        }
        due
    }

    /// Запись прямо сейчас (выключение плагина), если есть что писать.
    fn take_now(&mut self, now: Instant) -> bool {
        let due = self.loaded && self.dirty;
        if due {
            self.dirty = false;
            self.saved_at = now;
        }
        due
    }

    /// Выход: переносит секунды и пишет синхронно. Ошибку не показываем:
    /// окно уже закрывается.
    pub(super) fn shutdown(&mut self) {
        let (now, today) = (Instant::now(), today_local());
        self.fold(now, today);
        self.since = None;
        if self.take_now(now) {
            let _ = write_snapshot(&self.snapshot(), next_serial());
        }
    }

    /// Сброс времени: серия и рекорд остаются, отсчёт идёт с этого момента.
    fn reset(&mut self, now: Instant, today: Ymd) {
        self.fold(now, today);
        self.stats.reset_time();
        self.session = 0;
        if self.since.is_some() {
            self.since = Some(now);
        }
        self.dirty = true;
    }

    /// Подтверждённое восстановление: данные заменяются данными копии (не
    /// складываются), серия отмечается по обычным правилам. `false` — копии
    /// нет.
    fn restore(&mut self, now: Instant, today: Ymd) -> bool {
        let Some(copy) = self.pending_restore.take() else {
            return false;
        };
        self.stats = *copy;
        self.session = 0;
        if self.since.is_some() {
            self.since = Some(now);
        }
        self.dirty = true;
        self.visit(today);
        true
    }

    /// Серия и рекорд для показа. Отладочный хук `PLASTINKA_TEST_STREAK`
    /// подменяет оба числа; в файл он ничего не пишет.
    fn shown_streak(&self) -> (u64, u64) {
        match streak_hook() {
            Some(days) => (days, days),
            None => (self.stats.streak.count, self.stats.streak.best),
        }
    }

    /// Данные экрана «Статистика» с живыми секундами.
    fn stats_data(&self, now: Instant, today: Ymd) -> StatsData {
        let live = self.live(now);
        let mut totals = self.stats.totals(today);
        let mut week = self.stats.week(today);
        for value in [
            &mut totals.day,
            &mut totals.week,
            &mut totals.month,
            &mut totals.year,
            &mut totals.total,
            &mut week.total,
        ] {
            *value = value.saturating_add(live);
        }
        let today_index = usize::try_from(days_from_civil(today) - days_from_civil(week.monday))
            .unwrap_or_default();
        if let Some(slot) = week.days.get_mut(today_index) {
            *slot = slot.saturating_add(live);
        }
        let (streak, best) = self.shown_streak();
        StatsData {
            totals,
            week,
            today_index,
            session: self.session.saturating_add(live),
            previous: self.stats.previous_session,
            streak,
            best,
        }
    }

    /// Секунды, которые ещё не перенесены в статистику.
    fn live(&self, now: Instant) -> u64 {
        self.since
            .map_or(0, |since| now.saturating_duration_since(since).as_secs())
    }

    /// Время за период вместе с живыми секундами.
    fn period_secs(&self, period: Period, now: Instant, today: Ymd) -> u64 {
        let totals = self.stats.totals(today);
        let base = match period {
            Period::Total => totals.total,
            Period::Year => totals.year,
            Period::Month => totals.month,
            Period::Week => totals.week,
            Period::Day => totals.day,
            Period::Session => self.session,
        };
        base.saturating_add(self.live(now))
    }

    /// Что показать в заголовке по настройкам. `None` — ничего.
    fn title(&self, settings: &PluginSettings, now: Instant, today: Ymd) -> Option<Title> {
        if !settings.listen_time || !self.loaded {
            return None;
        }
        let time = settings.listen_timer.then(|| {
            let text = format_short(
                self.period_secs(settings.listen_period, now, today),
                settings.listen_seconds,
            );
            if settings.listen_label {
                format!("{}: {text}", settings.listen_period.label())
            } else {
                text
            }
        });
        // Нулевая серия (окно открыто свёрнутым): показывать нечего.
        let shown = self.shown_streak().0;
        let streak = (settings.listen_streak && shown > 0).then_some(shown);
        (time.is_some() || streak.is_some()).then_some(Title { time, streak })
    }
}

impl Drop for Listen {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Library {
    /// Чтение файла статистики в фоне (при создании окна).
    pub(super) fn listen_load() -> Task<Message> {
        Task::perform(
            async {
                storage::blocking(|| Ok(listen_time::load()))
                    .await
                    .unwrap_or_default()
            },
            |stats| Message::Listen(Msg::Loaded(Box::new(stats))),
        )
    }

    /// Фоновая запись снимка.
    fn listen_save_task(&self) -> Task<Message> {
        let (snapshot, serial) = (self.listen.snapshot(), next_serial());
        Task::perform(
            async move {
                storage::blocking(move || write_snapshot(&snapshot, serial))
                    .await
                    .is_ok()
            },
            |ok| Message::Listen(Msg::Saved(ok)),
        )
    }

    fn listen_sounding(&self) -> bool {
        sounding(
            self.player.playing,
            self.player.loaded,
            self.player.buffering,
        )
    }

    /// Конец `Library::update`: старт и остановка звука ловятся сразу, а не
    /// на следующем тике.
    pub(super) fn listen_sync(&mut self) {
        if !self.listen.enabled {
            return;
        }
        let now = Instant::now();
        let sounding = self.listen_sounding();
        self.listen.sync(sounding, now, today_local());
        if self.listen.take_due(now) {
            let save = self.listen_save_task();
            self.tasks.push(save);
        }
    }

    /// Настройка плагина изменилась. Выключение сразу сохраняет накопленное.
    pub(super) fn listen_settings(&mut self, enabled: bool) -> Task<Message> {
        let now = Instant::now();
        self.listen.set_enabled(enabled, now, today_local());
        if enabled {
            let sounding = self.listen_sounding();
            self.listen.sync(sounding, now, today_local());
            self.listen.visit(today_local());
            return Task::none();
        }
        if self.listen.take_now(now) {
            self.listen_save_task()
        } else {
            Task::none()
        }
    }

    /// Окно свёрнуто или развёрнуто.
    pub fn listen_minimized(&mut self, minimized: bool) {
        self.listen.minimized = minimized;
    }

    /// Выход из приложения: переносит секунды и пишет файл синхронно.
    pub fn listen_shutdown(&mut self) {
        self.listen.shutdown();
    }

    pub(super) fn listen_msg(&mut self, msg: Msg) -> (Task<Message>, Option<Effect>) {
        let (now, today) = (Instant::now(), today_local());
        match msg {
            Msg::Loaded(stats) => {
                self.listen.loaded(*stats);
                self.listen.visit(today);
                (Task::none(), None)
            }
            Msg::Tick => {
                self.listen.visit(today);
                (Task::none(), None)
            }
            Msg::Saved(true) => {
                self.listen.failed = false;
                (Task::none(), None)
            }
            Msg::Saved(false) => {
                // Тост один раз: запись повторяется каждые полминуты.
                let first = !self.listen.failed;
                self.listen.failed = true;
                self.listen.dirty = true;
                let effect = first.then(|| {
                    Effect::Toast("Не удалось сохранить статистику прослушиваний".to_owned())
                });
                (Task::none(), effect)
            }
            Msg::Copy => {
                let data = self.listen.stats_data(now, today);
                let streak = listen_time::Streak {
                    count: data.streak,
                    best: data.best,
                    last: None,
                };
                let text = share_text(&data.totals, &data.week, &streak);
                (
                    iced::clipboard::write(text),
                    Some(Effect::Toast("Статистика скопирована".to_owned())),
                )
            }
            Msg::Export => {
                if !self.listen.loaded {
                    return (Task::none(), None);
                }
                self.listen.fold(now, today);
                let bytes = listen_time::to_json(&self.listen.snapshot());
                let (year, month, day) = today;
                let name = format!("plastinka-listen-time-{year:04}-{month:02}-{day:02}.json");
                (
                    Task::perform(export_file(bytes, name), |result| {
                        Message::Listen(Msg::Exported(result))
                    }),
                    None,
                )
            }
            Msg::Exported(result) => {
                let toast = match result {
                    Ok(Some(name)) => Some(format!("Копия сохранена: {name}")),
                    Ok(None) => None,
                    Err(error) => Some(format!("Не удалось сохранить копию: {error}")),
                };
                (Task::none(), toast.map(Effect::Toast))
            }
            Msg::Restore => {
                if !self.listen.loaded {
                    return (Task::none(), None);
                }
                (
                    Task::perform(pick_backup(), |result| {
                        Message::Listen(Msg::RestorePicked(result))
                    }),
                    None,
                )
            }
            Msg::RestorePicked(Ok(Some(copy))) => {
                let totals = copy.totals(today);
                self.listen.restore_text = format!(
                    "Текущие данные заменятся данными из файла: всего {}, серия {}, рекорд {}. \
                     Отменить нельзя.",
                    format_long(totals.total),
                    days_text(copy.streak.count),
                    days_text(copy.streak.best),
                );
                self.listen.pending_restore = Some(copy);
                (
                    Task::none(),
                    Some(Effect::OpenDialog(Dialog::ConfirmListenRestore)),
                )
            }
            Msg::RestorePicked(Ok(None)) => (Task::none(), None),
            Msg::RestorePicked(Err(error)) => (Task::none(), Some(Effect::Toast(error))),
            Msg::RestoreConfirmed => {
                let done = self.listen.restore(now, today);
                let save = self.listen_save_now(now);
                let toast = done.then(|| "Статистика восстановлена".to_owned());
                (save, Some(Effect::DialogDone(toast)))
            }
            Msg::ResetConfirmed => {
                self.listen.reset(now, today);
                let save = self.listen_save_now(now);
                (
                    save,
                    Some(Effect::DialogDone(Some(
                        "Время прослушивания сброшено".to_owned(),
                    ))),
                )
            }
        }
    }

    /// Сразу записать файл, если есть что.
    fn listen_save_now(&mut self, now: Instant) -> Task<Message> {
        if self.listen.take_now(now) {
            self.listen_save_task()
        } else {
            Task::none()
        }
    }

    /// Экран «Статистика» открыт, а файл ещё читается: крутится спиннер.
    pub fn listen_loading(&self) -> bool {
        self.section == super::Section::Stats && self.stack.is_empty() && !self.listen.loaded
    }

    /// Описание копии для диалога восстановления.
    pub fn listen_restore_text(&self) -> &str {
        &self.listen.restore_text
    }

    /// Диалог восстановления закрыт без подтверждения: копию выбрасываем.
    pub fn listen_discard_restore(&mut self) {
        self.listen.pending_restore = None;
    }

    /// Тик подписки: раз в секунду, пока звук идёт, иначе раз в минуту.
    pub(super) fn listen_subscription(&self) -> iced::Subscription<crate::ui::Message> {
        if !self.listen.enabled {
            return iced::Subscription::none();
        }
        let every = if self.listen_sounding() {
            Duration::from_secs(1)
        } else {
            IDLE_TICK
        };
        iced::time::every(every).map(|_| App::Library(Message::Listen(Msg::Tick)))
    }

    /// Таймер для центра заголовка окна. `None` — шапка как раньше.
    pub(in crate::ui) fn listen_title<'a>(&self) -> Option<Element<'a, App>> {
        let title = self
            .listen
            .title(&self.plugins, Instant::now(), today_local())?;
        Some(title_view(&title, &self.plugins))
    }
}

/// Цвет огонька серии.
fn flame_color(days: u64) -> Color {
    let (r, g, b) = flame(days).color;
    Color::from_rgb8(r, g, b)
}

fn title_view<'a>(title: &Title, settings: &PluginSettings) -> Element<'a, App> {
    let caption = |text: String| theme::line(text, Style::Caption, theme::text_secondary());
    let mut items = row![].spacing(6).align_y(Alignment::Center);
    if let Some(time) = &title.time {
        items = items.push(caption(time.clone()));
    }
    if title.time.is_some() && title.streak.is_some() {
        items = items.push(theme::line("|", Style::Caption, theme::text_tertiary()));
    }
    if let Some(count) = title.streak {
        if settings.listen_flame {
            items = items.push(
                row![
                    icon(Icon::Flame, FLAME_SIZE, flame_color(count)),
                    caption(count.to_string()),
                ]
                .spacing(4)
                .align_y(Alignment::Center),
            );
        } else {
            items = items.push(caption(streak_text(count, settings.listen_label)));
        }
    }
    // Поля по бокам одинаковые: таймер по центру окна, а не по центру
    // свободного места. Текст не ловит мышь, перетаскивание под ним работает.
    container(items)
        .padding(Padding::default().left(SIDE_GAP).right(SIDE_GAP))
        .center_x(Length::Fill)
        .center_y(gate::TITLE_BAR_HEIGHT)
        .into()
}

/// Сколько карточек влезает в ширину `inner` (не меньше одной).
fn card_columns(inner: f32) -> usize {
    let mut columns = 1;
    for n in 2_u8..=8 {
        if f32::from(n) * CARD_MIN + f32::from(n - 1) * CARD_GAP <= inner {
            columns = usize::from(n);
        }
    }
    columns
}

/// Пункт «Статистика» в меню виден.
pub(super) fn stats_in_menu(settings: &PluginSettings) -> bool {
    settings.listen_time && settings.listen_menu
}

/// Текст ошибки записи и чтения по-русски.
fn io_text(error: &std::io::Error) -> &'static str {
    use std::io::ErrorKind;
    match error.kind() {
        ErrorKind::NotFound => "папка или файл не найдены",
        ErrorKind::PermissionDenied => "нет доступа",
        ErrorKind::StorageFull => "нет места на диске",
        _ => "ошибка записи",
    }
}

/// Окно «Сохранить как» и запись копии. `Ok(None)` — окно закрыли.
async fn export_file(bytes: Vec<u8>, name: String) -> Result<Option<String>, String> {
    let Some(file) = rfd::AsyncFileDialog::new()
        .set_title("Сохранить копию статистики")
        .add_filter("JSON", &["json"])
        .set_file_name(name)
        .save_file()
        .await
    else {
        return Ok(None);
    };
    let path = file.path().to_path_buf();
    let shown = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let target = path.clone();
    let result = storage::blocking(move || {
        std::fs::write(&target, bytes).map_err(|source| StorageError::ConfigIo {
            path: target.clone(),
            source,
        })
    })
    .await;
    match result {
        Ok(()) => Ok(Some(shown)),
        Err(StorageError::ConfigIo { source, .. }) => Err(io_text(&source).to_owned()),
        Err(_) => Err("запись прервалась".to_owned()),
    }
}

/// Читает копию, не больше [`MAX_BACKUP`]. Ошибка — готовый текст тоста.
fn read_backup(path: &std::path::Path) -> Result<Box<ListenStats>, String> {
    let meta = std::fs::metadata(path)
        .map_err(|error| format!("Не удалось прочитать файл: {}", io_text(&error)))?;
    if meta.len() > MAX_BACKUP {
        return Err("Файл больше 4 МБ: это не копия статистики".to_owned());
    }
    let bytes = std::fs::read(path)
        .map_err(|error| format!("Не удалось прочитать файл: {}", io_text(&error)))?;
    listen_time::from_json(&bytes).map(Box::new).ok_or_else(|| {
        "Файл не похож на копию статистики Plastinka. Текущие данные не изменены".to_owned()
    })
}

/// Окно выбора файла и разбор копии. `Ok(None)` — окно закрыли.
async fn pick_backup() -> Result<Option<Box<ListenStats>>, String> {
    let Some(file) = rfd::AsyncFileDialog::new()
        .set_title("Восстановить статистику из копии")
        .add_filter("JSON", &["json"])
        .pick_file()
        .await
    else {
        return Ok(None);
    };
    let path = file.path().to_path_buf();
    match storage::blocking(move || Ok(read_backup(&path))).await {
        Ok(result) => result.map(Some),
        Err(_) => Err("Не удалось прочитать файл".to_owned()),
    }
}

fn card_style(_: &iced::Theme) -> container::Style {
    container::Style {
        background: Some(theme::surface_1().into()),
        border: Border {
            color: theme::border(),
            width: 1.0,
            radius: widgets::RADIUS_M.into(),
        },
        ..container::Style::default()
    }
}

/// Карточка с подписью и значением; `flame` — цвет огонька слева от значения.
fn stat_card<'a>(label: &'a str, value: String, flame: Option<Color>) -> Element<'a, App> {
    let mut line = row![].spacing(8).align_y(Alignment::Center);
    if let Some(color) = flame {
        line = line.push(icon(Icon::Flame, 22.0, color));
    }
    line = line.push(theme::line(value, Style::Title, theme::text_primary()));
    container(
        column![
            theme::line(label, Style::Secondary, theme::text_tertiary()),
            line
        ]
        .spacing(6),
    )
    .padding(16)
    .width(Length::Fill)
    .style(card_style)
    .into()
}

/// Сетка карточек по `columns` в ряд; неполный ряд добивается пустым местом.
fn card_grid<'a>(cards: Vec<Element<'a, App>>, columns: usize) -> Element<'a, App> {
    let mut grid = column![].spacing(CARD_GAP);
    let mut cards = cards.into_iter();
    loop {
        let mut line = row![].spacing(CARD_GAP);
        let mut filled = 0;
        for card in cards.by_ref().take(columns) {
            line = line.push(card);
            filled += 1;
        }
        if filled == 0 {
            break;
        }
        for _ in filled..columns {
            line = line.push(Space::new().width(Length::Fill));
        }
        grid = grid.push(line);
    }
    grid.into()
}

/// Строка недели: день, дата, полоса и время.
fn week_row<'a>(row_data: &WeekRow) -> Element<'a, App> {
    let (name_color, time_color) = if row_data.future {
        (theme::text_disabled(), theme::text_disabled())
    } else if row_data.today {
        (theme::text_primary(), theme::text_primary())
    } else {
        (theme::text_secondary(), theme::text_secondary())
    };
    let date = if row_data.today {
        format!("{} · сегодня", row_data.date)
    } else {
        row_data.date.clone()
    };
    let name_style = if row_data.today {
        Style::BodyStrong
    } else {
        Style::Body
    };
    let bar: Element<'a, App> = if row_data.permille == 0 {
        Space::new().width(Length::Fill).into()
    } else {
        let fill = container(Space::new())
            .width(Length::FillPortion(row_data.permille))
            .height(8)
            .style(|_| container::Style {
                background: Some(theme::accent().into()),
                border: Border::default().rounded(4),
                ..container::Style::default()
            });
        let rest = Space::new().width(Length::FillPortion(
            1000_u16.saturating_sub(row_data.permille),
        ));
        container(row![fill, rest])
            .width(Length::Fill)
            .height(8)
            .into()
    };
    let line = row![
        container(theme::line(row_data.name, name_style, name_color)).width(120),
        container(theme::line(date, Style::Secondary, theme::text_tertiary())).width(130),
        bar,
        container(theme::line(
            format_short(row_data.secs, false),
            Style::Body,
            time_color
        ))
        .width(90)
        .align_x(Alignment::End),
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    let today = row_data.today;
    container(line)
        .padding([8, 12])
        .width(Length::Fill)
        .style(move |_| container::Style {
            background: today.then(|| theme::surface_3().into()),
            border: Border::default().rounded(widgets::RADIUS_M),
            ..container::Style::default()
        })
        .into()
}

impl Library {
    /// Экран «Статистика».
    pub(super) fn listen_view(&self, turn: f32) -> Element<'_, App> {
        if !self.listen.loaded {
            return container(iced::widget::center(
                column![
                    spinner(40.0, theme::accent(), turn),
                    theme::line(
                        "Загружаем статистику…",
                        Style::Body,
                        theme::text_secondary()
                    ),
                ]
                .spacing(14)
                .align_x(Alignment::Center),
            ))
            .into();
        }
        let data = self.listen.stats_data(Instant::now(), today_local());
        let inner = (self.panel_width - 2.0 * PAGE_SIDE).max(CARD_MIN);
        let columns = card_columns(inner);
        let periods = vec![
            stat_card("Сегодня", format_long(data.totals.day), None),
            stat_card("За неделю", format_long(data.totals.week), None),
            stat_card("За месяц", format_long(data.totals.month), None),
            stat_card("За год", format_long(data.totals.year), None),
            stat_card("Всего", format_long(data.totals.total), None),
            stat_card("Текущая сессия", format_long(data.session), None),
            stat_card("Прошлая сессия", format_long(data.previous), None),
        ];
        let flames = self.plugins.listen_flame;
        let streaks = vec![
            stat_card(
                "Серия дней подряд",
                days_text(data.streak),
                flames.then(|| flame_color(data.streak)),
            ),
            stat_card(
                "Рекорд",
                days_text(data.best),
                flames.then(|| flame_color(data.best)),
            ),
        ];
        let rows = week_rows(&data.week, data.today_index);
        let week = container(
            column![
                column(rows.iter().map(week_row)).spacing(2),
                widgets::divider(),
                theme::line(
                    format!("Итого за неделю: {}", format_short(data.week.total, false)),
                    Style::BodyStrong,
                    theme::text_primary()
                ),
            ]
            .spacing(12),
        )
        .padding(16)
        .width(Length::Fill)
        .style(card_style);
        let ready = self.listen.loaded;
        let press = |msg: Msg| ready.then(|| App::Library(Message::Listen(msg)));
        let buttons = row![
            widgets::action(
                Kind::Secondary,
                Some(Icon::Copy),
                "Скопировать",
                press(Msg::Copy)
            ),
            widgets::action(Kind::Secondary, None, "Экспортировать", press(Msg::Export)),
            widgets::action(
                Kind::Secondary,
                None,
                "Восстановить из файла",
                press(Msg::Restore)
            ),
            widgets::action(
                Kind::Danger,
                None,
                "Сбросить",
                Some(App::OpenDialog(Dialog::ConfirmListenReset))
            ),
        ]
        .spacing(12);
        let page = column![
            column![
                theme::line("Статистика", Style::Display, theme::text_primary()),
                theme::line(
                    "Время прослушивания в Plastinka",
                    Style::Body,
                    theme::text_secondary()
                ),
            ]
            .spacing(4),
            card_grid(periods, columns),
            card_grid(streaks, columns),
            theme::line("Эта неделя", Style::Title, theme::text_primary()),
            week,
            buttons.wrap(),
        ]
        .spacing(20);
        scroll::smooth(
            scrollable(container(page).width(Length::Fill).padding(Padding {
                top: PAGE_TOP,
                right: PAGE_SIDE,
                bottom: PAGE_TOP,
                left: PAGE_SIDE,
            }))
            .width(Length::Fill)
            .height(Length::Fill),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: Ymd = (2026, 11, 22);

    fn at(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    fn on() -> Listen {
        let mut listen = Listen::new();
        listen.enabled = true;
        listen.loaded = true;
        listen
    }

    #[test]
    fn sound_rule() {
        assert!(sounding(true, true, false));
        assert!(!sounding(false, true, false), "пауза");
        assert!(!sounding(true, true, true), "буферизация");
        assert!(!sounding(true, false, false), "трек не открыт");
        assert!(!sounding(false, false, false));
    }

    #[test]
    fn whole_seconds_move_and_remainder_stays() {
        let base = Instant::now();
        let mut listen = on();
        listen.sync(true, base, DAY);
        listen.sync(true, at(base, 2_700), DAY);
        assert_eq!(listen.stats.days.get(&DAY), Some(&2));
        assert_eq!(listen.session, 2);
        assert_eq!(listen.live(at(base, 2_700)), 0);
        // Остаток 0,7 с не потерян: ещё 0,4 с дают третью секунду.
        listen.sync(true, at(base, 3_100), DAY);
        assert_eq!(listen.stats.days.get(&DAY), Some(&3));
        // Остановка переносит целые секунды и сбрасывает отсчёт.
        listen.sync(false, at(base, 4_200), DAY);
        assert_eq!(listen.stats.days.get(&DAY), Some(&4));
        assert!(listen.since.is_none());
        // Пока звука нет, ничего не копится.
        listen.sync(false, at(base, 60_000), DAY);
        assert_eq!(listen.session, 4);
        assert!(listen.dirty);
    }

    #[test]
    fn nothing_counts_when_disabled_or_sub_second() {
        let base = Instant::now();
        let mut off = Listen::new();
        off.sync(true, base, DAY);
        off.sync(true, at(base, 5_000), DAY);
        assert!(off.stats.days.is_empty() && off.since.is_none());

        let mut listen = on();
        listen.sync(true, base, DAY);
        listen.sync(false, at(base, 900), DAY);
        assert!(listen.stats.days.is_empty(), "меньше секунды не пишем");
        assert!(!listen.dirty);
    }

    #[test]
    fn disabling_moves_pending_seconds() {
        let base = Instant::now();
        let mut listen = on();
        listen.sync(true, base, DAY);
        listen.set_enabled(false, at(base, 3_500), DAY);
        assert_eq!(listen.stats.days.get(&DAY), Some(&3));
        assert!(listen.since.is_none() && listen.dirty);
        assert!(listen.take_now(at(base, 3_500)));
        assert!(!listen.take_now(at(base, 3_500)));
    }

    #[test]
    fn seconds_before_load_are_added_to_the_file() {
        let base = Instant::now();
        let mut listen = Listen::new();
        listen.enabled = true;
        listen.sync(true, base, DAY);
        listen.sync(true, at(base, 5_000), DAY);
        assert!(!listen.take_now(at(base, 5_000)), "до чтения не пишем");
        // Серию до чтения не отмечаем.
        listen.visit(DAY);
        assert_eq!(listen.stats.streak.count, 0);

        let mut disk = ListenStats::default();
        disk.add(DAY, 100);
        disk.previous_session = 77;
        disk.streak.visit((2026, 11, 21));
        listen.loaded(disk);
        assert_eq!(listen.stats.days.get(&DAY), Some(&105));
        assert_eq!(listen.stats.previous_session, 77);
        listen.visit(DAY);
        assert_eq!(listen.stats.streak.count, 2);
        assert!(listen.dirty);
        // Идущее время продолжает копиться после чтения.
        listen.sync(true, at(base, 6_000), DAY);
        assert_eq!(listen.stats.days.get(&DAY), Some(&106));
    }

    #[test]
    fn snapshot_writes_session_as_previous() {
        let mut listen = on();
        listen.stats.previous_session = 50;
        assert_eq!(listen.snapshot().previous_session, 50);
        listen.session = 9;
        let snapshot = listen.snapshot();
        assert_eq!(snapshot.previous_session, 9);
        // Показываемое значение остаётся прочитанным из файла.
        assert_eq!(listen.stats.previous_session, 50);
    }

    #[test]
    fn saving_is_throttled() {
        let base = Instant::now();
        let mut listen = on();
        listen.saved_at = base;
        listen.dirty = true;
        assert!(!listen.take_due(at(base, 29_000)));
        assert!(listen.take_due(at(base, 30_000)));
        assert!(!listen.take_due(at(base, 61_000)), "изменений нет");
        listen.dirty = true;
        assert!(!listen.take_due(at(base, 40_000)));
        assert!(listen.take_due(at(base, 61_000)));
    }

    #[test]
    fn minimized_window_does_not_mark_the_streak() {
        let mut listen = on();
        listen.minimized = true;
        listen.visit(DAY);
        assert_eq!(listen.stats.streak.count, 0);
        listen.minimized = false;
        listen.visit(DAY);
        assert_eq!(listen.stats.streak.count, 1);
    }

    fn settings() -> PluginSettings {
        PluginSettings {
            listen_time: true,
            ..PluginSettings::default()
        }
    }

    #[test]
    fn title_follows_settings() {
        let base = Instant::now();
        let mut listen = on();
        listen.stats.add(DAY, 3903);
        listen.stats.add((2026, 11, 1), 60);
        listen.session = 125;
        listen.stats.streak.visit(DAY);
        let title = |s: &PluginSettings| listen.title(s, base, DAY);

        let all = settings();
        assert_eq!(
            title(&all),
            Some(Title {
                time: Some("Время: 1ч 6м 03с".to_owned()),
                streak: Some(1)
            })
        );
        let mut s = settings();
        s.listen_period = Period::Day;
        s.listen_seconds = false;
        assert_eq!(
            title(&s).and_then(|t| t.time).as_deref(),
            Some("За день: 1ч 5м")
        );
        s.listen_period = Period::Month;
        assert_eq!(
            title(&s).and_then(|t| t.time).as_deref(),
            Some("За месяц: 1ч 6м")
        );
        s.listen_period = Period::Session;
        s.listen_seconds = true;
        s.listen_label = false;
        assert_eq!(title(&s).and_then(|t| t.time).as_deref(), Some("2м 05с"));
        // Только серия, только время, ничего.
        s.listen_timer = false;
        assert_eq!(
            title(&s),
            Some(Title {
                time: None,
                streak: Some(1)
            })
        );
        s.listen_timer = true;
        s.listen_streak = false;
        assert_eq!(title(&s).map(|t| t.streak), Some(None));
        s.listen_timer = false;
        assert_eq!(title(&s), None);
        // Плагин выключен.
        let mut off = settings();
        off.listen_time = false;
        assert_eq!(title(&off), None);
        // Живые секунды идут сразу.
        listen.since = Some(base);
        assert_eq!(
            listen
                .title(&all, at(base, 1_500), DAY)
                .and_then(|t| t.time)
                .as_deref(),
            Some("Время: 1ч 6м 04с")
        );
        // Файл не прочитан — таймер скрыт.
        listen.loaded = false;
        assert_eq!(listen.title(&all, base, DAY), None);
    }

    #[test]
    fn long_gap_is_not_counted() {
        let base = Instant::now();
        let mut listen = on();
        listen.sync(true, base, DAY);
        // Сон компьютера: два часа между отметками не засчитываются.
        listen.sync(true, at(base, 7_200_000), DAY);
        assert!(listen.stats.days.is_empty() && listen.session == 0);
        assert_eq!(listen.since, Some(at(base, 7_200_000)));
        // Отсчёт идёт дальше с нового момента.
        listen.sync(true, at(base, 7_203_000), DAY);
        assert_eq!(listen.stats.days.get(&DAY), Some(&3));
        // Остановка цикла событий на две минуты (перетаскивание) считается.
        listen.sync(true, at(base, 7_203_000 + 120_000), DAY);
        assert_eq!(listen.stats.days.get(&DAY), Some(&123));
        // Чуть больше двух минут — уже нет.
        listen.sync(true, at(base, 7_323_000 + 120_001), DAY);
        assert_eq!(listen.stats.days.get(&DAY), Some(&123));
    }

    #[test]
    fn reset_keeps_streak_and_restarts_counting() {
        let base = Instant::now();
        let mut listen = on();
        listen.stats.streak.visit(DAY);
        listen.stats.previous_session = 40;
        listen.sync(true, base, DAY);
        listen.sync(true, at(base, 5_500), DAY);
        listen.reset(at(base, 5_900), DAY);
        assert!(listen.stats.days.is_empty());
        assert_eq!((listen.session, listen.stats.previous_session), (0, 0));
        assert_eq!(
            (listen.stats.streak.count, listen.stats.streak.best),
            (1, 1)
        );
        assert!(listen.dirty);
        // Звук продолжает идти: счёт с момента сброса.
        listen.sync(true, at(base, 7_000), DAY);
        assert_eq!(listen.stats.days.get(&DAY), Some(&1));
        assert_eq!(listen.session, 1);
    }

    #[test]
    fn restore_replaces_instead_of_adding() {
        let base = Instant::now();
        let mut copy = ListenStats::default();
        copy.add((2026, 11, 20), 1000);
        copy.previous_session = 7;
        copy.streak.visit((2026, 11, 19));
        copy.streak.visit((2026, 11, 20));
        copy.streak.visit((2026, 11, 22));
        copy.streak.best = 9;

        let mut listen = on();
        listen.stats.add(DAY, 50);
        listen.session = 12;
        listen.sync(true, base, DAY);
        for _ in 0..2 {
            listen.pending_restore = Some(Box::new(copy.clone()));
            assert!(listen.restore(at(base, 2_000), DAY));
            // Серия: копия закончилась в этот же день, рекорд из копии цел.
            assert_eq!(listen.stats.days.get(&DAY), None);
            assert_eq!(listen.stats.days.get(&(2026, 11, 20)), Some(&1000));
            assert_eq!(listen.stats.previous_session, 7);
            assert_eq!(listen.session, 0);
            assert_eq!(listen.stats.streak.best, 9);
        }
        // Второй раз ничего не удвоилось.
        assert_eq!(listen.stats.totals(DAY).total, 1000);
        // Без кандидата восстановить нечего.
        assert!(!listen.restore(at(base, 3_000), DAY));
    }

    #[test]
    fn restore_marks_streak_by_normal_rules() {
        let mut copy = ListenStats::default();
        copy.streak.visit((2026, 11, 10));
        copy.streak.visit((2026, 11, 11));
        copy.streak.best = 30;
        let mut listen = on();
        listen.pending_restore = Some(Box::new(copy));
        assert!(listen.restore(Instant::now(), DAY));
        // Пропущенные дни не прибавляются: серия началась заново, рекорд есть.
        assert_eq!(
            (listen.stats.streak.count, listen.stats.streak.best),
            (1, 30)
        );
        assert_eq!(listen.stats.streak.last, Some(DAY));
    }

    #[test]
    fn stats_data_includes_live_seconds() {
        let base = Instant::now();
        let mut listen = on();
        // 2026-11-22 — воскресенье.
        listen.stats.add((2026, 11, 16), 100);
        listen.stats.add(DAY, 60);
        listen.session = 60;
        listen.since = Some(base);
        let data = listen.stats_data(at(base, 2_500), DAY);
        assert_eq!(data.totals.day, 62);
        assert_eq!(data.totals.week, 162);
        assert_eq!(data.totals.total, 162);
        assert_eq!(data.week.total, 162);
        assert_eq!(data.today_index, 6);
        assert_eq!(data.week.days, [100, 0, 0, 0, 0, 0, 62]);
        assert_eq!(data.session, 62);
        assert_eq!(listen.stats_data(base, DAY).session, 60);
    }

    #[test]
    fn week_rows_shares_and_markers() {
        let mut stats = ListenStats::default();
        stats.add((2026, 11, 16), 100);
        stats.add((2026, 11, 18), 50);
        stats.add(DAY, 25);
        // Среда, 18 ноября.
        let week = stats.week((2026, 11, 18));
        let rows = week_rows(&week, 2);
        assert_eq!(rows.len(), 7);
        let first = rows.first();
        assert_eq!(
            first.map(|r| (r.name, r.date.as_str())),
            Some(("Понедельник", "16.11"))
        );
        assert_eq!(first.map(|r| r.permille), Some(1000));
        assert_eq!(
            rows.get(2).map(|r| (r.permille, r.today, r.future)),
            Some((500, true, false))
        );
        assert_eq!(rows.get(3).map(|r| (r.permille, r.future)), Some((0, true)));
        assert_eq!(rows.get(6).map(|r| r.date.as_str()), Some("22.11"));
        // Пустая неделя: полос нет.
        let empty = ListenStats::default().week(DAY);
        assert!(week_rows(&empty, 6).iter().all(|r| r.permille == 0));
    }

    #[test]
    fn card_columns_fit_the_width() {
        assert_eq!(card_columns(100.0), 1);
        assert_eq!(card_columns(CARD_MIN), 1);
        assert_eq!(card_columns(2.0 * CARD_MIN + CARD_GAP - 1.0), 1);
        assert_eq!(card_columns(2.0 * CARD_MIN + CARD_GAP), 2);
        assert_eq!(card_columns(3.0 * CARD_MIN + 2.0 * CARD_GAP), 3);
        assert!(card_columns(100_000.0) >= 3);
    }

    #[test]
    fn menu_item_visibility() {
        let mut s = PluginSettings::default();
        assert!(!stats_in_menu(&s), "плагин выключен");
        s.listen_time = true;
        assert!(stats_in_menu(&s));
        s.listen_menu = false;
        assert!(!stats_in_menu(&s));
        s.listen_menu = true;
        s.listen_time = false;
        assert!(!stats_in_menu(&s));
    }

    #[test]
    fn streak_hook_parses_only_numbers() {
        assert_eq!(parse_streak("10"), Some(10));
        assert_eq!(parse_streak(" 1000 "), Some(1000));
        assert_eq!(parse_streak("-1"), None);
        assert_eq!(parse_streak("много"), None);
        assert_eq!(parse_streak(""), None);
    }

    #[test]
    fn streak_text_with_and_without_label() {
        assert_eq!(streak_text(5, true), "Дней подряд: 5");
        assert_eq!(streak_text(5, false), "5");
    }
}
