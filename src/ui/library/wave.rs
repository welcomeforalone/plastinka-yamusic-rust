//! «Моя волна»: состояние, очередь, обратная связь и экран.
//!
//! Волна — обычная очередь плеера (`ListKey::Wave`), которая растёт по мере
//! догрузки. Звук идёт тем же путём, что у остальных списков (`start`,
//! `launch`, `decide`), поэтому FckCensor и Slopless работают и здесь. Окно
//! шлёт события обратной связи по одному, в порядке возникновения: следующее
//! уходит только после ответа на предыдущее. Подробности запросов и источники
//! — `docs/api/stage-8.md`.

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use iced::gradient::Linear;
use iced::widget::{Space, button, center, column, container, row, stack};
use iced::{Alignment, Background, Border, Color, Degrees, Element, Length, Task, mouse};

use super::super::anim;
use super::super::covers::Covers;
use super::super::icons::{Icon, icon, spinner};
use super::super::marquee::marquee;
use super::super::queue::{ListKey, Queue};
use super::super::theme::{self, Style};
use super::super::visualizer;
use super::super::widgets::{self, Kind, Size};
use super::{
    App, Effect, Failure, FailureKind, Lib, Library, Message, Section, browse, crossfade_cover,
    fill, plugins, quality_badge, sentence, state_block,
};
use crate::api::{
    self, Activity, Diversity, Language, Mood, QUEUE_LEN, Track, WaveBatch, WaveColors, WaveEvent,
    WaveSettings, WaveTracks, fetch_activities, fetch_wave_tracks, send_wave_feedback, start_wave,
};
use crate::plugins::vibe;
use iced::animation::Animation;
use iced::time::Instant;

/// Сколько последних идентификаторов сессии помним для отсева повторов.
const SEEN_LIMIT: usize = 500;
/// Догружаем, когда впереди текущего трека осталось не больше стольких.
const TOP_UP_AHEAD: usize = 2;
/// Сколько пустых (после отсева повторов) порций подряд ещё пробуем.
const MAX_EMPTY: u8 = 2;
/// Сколько неотправленных событий храним (при долгом обрыве связи).
const FEEDBACK_LIMIT: usize = 100;
/// Обложка и квадрат вокруг неё, где рисуется круговой спектр: лучи
/// начинаются чуть дальше угла скруглённого квадрата и растут наружу.
const COVER: f32 = 220.0;
const ART: f32 = 380.0;
const RAYS_INNER: f32 = 152.0;
const RAYS_LENGTH: f32 = 36.0;
/// Ширина названия, исполнителя и подписи «Далее».
const TEXT_WIDTH: f32 = 560.0;
/// Период пульсации колец и размах масштаба.
const PULSE_SECONDS: f32 = 2.4;
const PULSE_SCALE: f32 = 0.06;
/// Смена цвета волны (трек, включение плагина).
const TINT_FADE: Duration = Duration::from_millis(300);

/// Сообщения волны.
#[derive(Clone)]
pub enum Msg {
    /// Запустить новую сессию (кнопка Play, «Повторить» после ошибки старта).
    Start,
    Started(u64, Result<WaveBatch, Failure>),
    Fetched(u64, Result<WaveTracks, Failure>),
    /// Сессия протухла, пришла новая.
    Renewed(u64, Result<WaveBatch, Failure>),
    /// Волна встала (очередь кончилась, догрузка не удалась): ещё раз.
    Retry,
    /// «Не нравится»: пропуск, запись в аккаунт и следующий трек.
    Dislike,
    DislikeDone(Result<(), Failure>),
    FeedbackDone,
    /// Таблетка источника: открыть или закрыть меню настроек волны.
    Menu,
    CloseMenu,
    /// Ответ со списком занятий.
    Stations(Result<Vec<Activity>, Failure>),
    /// Выбор в меню.
    Pick(Pick),
}

/// Выбор в меню настроек волны; `None` — «по умолчанию».
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick {
    Activity(Option<Activity>),
    Diversity(Option<Diversity>),
    Mood(Option<Mood>),
    Language(Option<Language>),
    /// Кнопка «Сбросить».
    Reset,
}

/// Занятия для меню: грузятся при первом открытии меню. Не загрузились —
/// раздел скрыт, при следующем открытии пробуем ещё раз.
#[derive(Clone)]
enum Stations {
    Idle,
    Loading,
    Ready(Vec<Activity>),
    Failed,
}

/// Настройки после выбора: прежние, если выбор ничего не меняет.
fn picked(current: &WaveSettings, pick: Pick) -> WaveSettings {
    let mut next = current.clone();
    match pick {
        Pick::Activity(activity) => next.activity = activity,
        Pick::Diversity(diversity) => next.diversity = diversity,
        Pick::Mood(mood) => next.mood = mood,
        Pick::Language(language) => next.language = language,
        Pick::Reset => next = WaveSettings::default(),
    }
    next
}

/// Что видно, пока волна не в очереди.
#[derive(Clone)]
enum Phase {
    Idle,
    Starting,
    Failed(Failure),
    Empty,
}

/// Данные трека волны, лежат рядом с очередью под тем же номером.
#[derive(Clone)]
struct Meta {
    session: String,
    batch_id: String,
    from: Option<String>,
    liked: bool,
    /// Для VibeColorizer (цвет под обложку): оттенок и цвета из ответа.
    hue: Option<f64>,
    colors: Option<WaveColors>,
}

/// Идентификаторы, которые уже были в сессии (последние [`SEEN_LIMIT`]).
#[derive(Default)]
struct Seen {
    order: VecDeque<String>,
    set: HashSet<String>,
}

impl Seen {
    /// `true` — идентификатор новый.
    fn insert(&mut self, id: &str) -> bool {
        if !self.set.insert(id.to_owned()) {
            return false;
        }
        self.order.push_back(id.to_owned());
        while self.order.len() > SEEN_LIMIT {
            if let Some(old) = self.order.pop_front() {
                self.set.remove(&old);
            }
        }
        true
    }
}

/// Нужна ли догрузка: впереди трека `current` в очереди из `len` треков
/// осталось не больше [`TOP_UP_AHEAD`].
fn needs_more(current: usize, len: usize) -> bool {
    len.saturating_sub(current.saturating_add(1)) <= TOP_UP_AHEAD
}

/// Только новые треки порции, вместе с данными для очереди.
fn take_fresh(seen: &mut Seen, batch: &WaveBatch) -> Vec<(Track, Meta)> {
    batch
        .tracks
        .iter()
        .filter(|wave| seen.insert(&wave.track.id))
        .map(|wave| {
            let meta = Meta {
                session: batch.session.clone(),
                batch_id: wave.batch_id.clone(),
                from: batch.from.clone(),
                liked: wave.liked,
                hue: wave.hue,
                colors: wave.colors.clone(),
            };
            (wave.track.clone(), meta)
        })
        .collect()
}

/// Повтор в волне — только «выкл ↔ трек».
fn toggled_repeat(one: bool) -> bool {
    !one
}

/// Событие, которое ждёт отправки.
#[derive(Debug, Clone, PartialEq)]
struct Pending {
    session: String,
    batch_id: String,
    from: Option<String>,
    event: WaveEvent,
}

/// Как трек покинули.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Leave {
    /// Доигран до конца.
    Finish,
    /// Пропущен: «Следующий», «Не нравится», другой трек или другой список.
    Skip,
}

#[derive(Clone)]
struct Announced {
    session: String,
    batch_id: String,
    from: Option<String>,
    track_id: String,
}

/// Какой трек волны уже объявлен («trackStarted» ушёл). Закрывающее событие
/// (`skip` или `trackFinished`) возможно только для объявленного трека и
/// только один раз.
#[derive(Default)]
struct Tracker {
    announced: Option<Announced>,
}

impl Tracker {
    fn started(&mut self, meta: &Meta, track_id: String) -> Option<Pending> {
        if self.announced.is_some() {
            return None;
        }
        self.announced = Some(Announced {
            session: meta.session.clone(),
            batch_id: meta.batch_id.clone(),
            from: meta.from.clone(),
            track_id: track_id.clone(),
        });
        Some(Pending {
            session: meta.session.clone(),
            batch_id: meta.batch_id.clone(),
            from: meta.from.clone(),
            event: WaveEvent::TrackStarted { track_id },
        })
    }

    fn leave(&mut self, how: Leave, played: Duration) -> Option<Pending> {
        let announced = self.announced.take()?;
        let track_id = announced.track_id;
        let event = match how {
            Leave::Finish => WaveEvent::TrackFinished { track_id, played },
            Leave::Skip => WaveEvent::Skipped { track_id, played },
        };
        Some(Pending {
            session: announced.session,
            batch_id: announced.batch_id,
            from: announced.from,
            event,
        })
    }
}

/// Очередь событий: один запрос в полёте, порядок сохраняется.
#[derive(Default)]
struct Feedback {
    queue: VecDeque<Pending>,
    in_flight: bool,
}

impl Feedback {
    fn push(&mut self, pending: Pending) {
        self.queue.push_back(pending);
        while self.queue.len() > FEEDBACK_LIMIT {
            self.queue.pop_front();
        }
    }

    /// Следующее событие к отправке, если предыдущее уже закончено.
    fn next(&mut self) -> Option<Pending> {
        if self.in_flight {
            return None;
        }
        let pending = self.queue.pop_front()?;
        self.in_flight = true;
        Some(pending)
    }

    fn done(&mut self) {
        self.in_flight = false;
    }
}

/// Цвет волны на экране: плавно переходит к новому за [`TINT_FADE`].
struct Tint {
    from: Color,
    to: Color,
    since: Instant,
}

impl Tint {
    fn new(color: Color) -> Self {
        Self {
            from: color,
            to: color,
            since: Instant::now(),
        }
    }

    /// Доля перехода от 0 до 1 (с замедлением к концу).
    fn progress(&self, now: Instant) -> f32 {
        let elapsed = now.saturating_duration_since(self.since).as_secs_f32();
        let t = (elapsed / TINT_FADE.as_secs_f32()).clamp(0.0, 1.0);
        1.0 - (1.0 - t).powi(3)
    }

    fn at(&self, now: Instant) -> Color {
        theme::mix(self.from, self.to, self.progress(now))
    }

    fn animating(&self, now: Instant) -> bool {
        self.from != self.to && now.saturating_duration_since(self.since) < TINT_FADE
    }

    /// Новая цель: переход идёт от цвета, который виден сейчас.
    fn retarget(&mut self, target: Color, now: Instant) {
        if target == self.to {
            return;
        }
        self.from = self.at(now);
        self.to = target;
        self.since = now;
    }
}

/// Состояние волны в окне.
pub(super) struct Wave {
    phase: Phase,
    /// Номер запуска: ответы прежних запусков отбрасываются.
    serial: u64,
    session: Option<String>,
    /// Данные треков очереди под теми же номерами.
    items: Vec<Meta>,
    seen: Seen,
    fetching: bool,
    /// Сессию уже пересоздавали из-за `unknownSession`.
    renewed: bool,
    empty_streak: u8,
    /// Догрузка не удалась: сама не повторяется, пока пользователь не
    /// попросит (следующий трек или «Повторить»).
    blocked: bool,
    /// Пользователь ждёт следующий трек, которого пока нет.
    waiting: bool,
    /// Номер в очереди, с которого продолжить, когда придёт порция.
    resume_at: usize,
    repeat_one: bool,
    tracker: Tracker,
    feedback: Feedback,
    /// От этого момента считается пульсация колец.
    started: Instant,
    /// Цвет экрана волны.
    tint: Tint,
    /// Откуда построена очередь, если играет не волна: подпись в таблетке.
    source: String,
    /// Выбор в меню настроек (в `config.toml`, не в аккаунте).
    settings: WaveSettings,
    /// Меню настроек: открыто или закрыто (плавно).
    menu: Animation<bool>,
    stations: Stations,
}

impl Wave {
    pub(super) fn new() -> Self {
        Self {
            phase: Phase::Idle,
            serial: 0,
            session: None,
            items: Vec::new(),
            seen: Seen::default(),
            fetching: false,
            renewed: false,
            empty_streak: 0,
            blocked: false,
            waiting: false,
            resume_at: 0,
            repeat_one: false,
            tracker: Tracker::default(),
            feedback: Feedback::default(),
            started: Instant::now(),
            tint: Tint::new(theme::accent()),
            source: String::new(),
            settings: WaveSettings::default(),
            menu: anim::popover(false),
            stations: Stations::Idle,
        }
    }
}

/// Что показывает экран «Моя волна».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    /// Играет волна.
    Wave,
    /// Играет (или на паузе) трек не из волны: тот же экран с таблеткой
    /// источника и крестиком.
    Foreign,
    /// Ничего не играет или волна запускается/не запустилась: стартовый вид
    /// по фазе.
    Start,
}

/// Выбор вида экрана. `idle` — волна не запускается и не в ошибке (фаза
/// `Idle`): пока она грузится, остаётся «Загружаем волну…», а не чужой трек.
fn screen_for(in_wave: bool, idle: bool, has_track: bool) -> Screen {
    if in_wave {
        Screen::Wave
    } else if idle && has_track {
        Screen::Foreign
    } else {
        Screen::Start
    }
}

/// Есть ли у трека волны свой цвет из ответа (цвета или оттенок): тогда
/// большая обложка для VibeColorizer не нужна. Не волна — своего цвета нет.
fn has_own_color(meta: Option<&Meta>) -> bool {
    meta.is_some_and(|meta| {
        meta.hue.is_some()
            || meta
                .colors
                .as_ref()
                .is_some_and(|colors| colors.accent.is_some() || colors.average.is_some())
    })
}

/// Источники цвета для VibeColorizer: данные волны (если играет она) и
/// средний цвет обложки (для любого трека).
fn vibe_sources(meta: Option<&Meta>, cover: Option<[u8; 3]>) -> vibe::Sources<'_> {
    let colors = meta.and_then(|meta| meta.colors.as_ref());
    vibe::Sources {
        accent: colors.and_then(|c| c.accent.as_deref()),
        average: colors.and_then(|c| c.average.as_deref()),
        hue: meta.and_then(|meta| meta.hue),
        cover,
    }
}

/// Подпись источника очереди для таблетки. `title` — название страницы
/// (альбома, плейлиста, исполнителя), если оно известно.
fn source_text(key: &ListKey, title: Option<&str>) -> String {
    let with = |kind: &str| match title {
        Some(title) => format!("{kind} «{title}»"),
        None => kind.to_owned(),
    };
    match key {
        // Отвязанная очередь осталась от «Мне нравится».
        ListKey::Liked | ListKey::LikedFound(_) | ListKey::Detached => "Мне нравится".to_owned(),
        ListKey::Album(_) => with("Альбом"),
        ListKey::Playlist(_) => with("Плейлист"),
        ListKey::ArtistPopular(_) | ListKey::ArtistTracks(_) => match title {
            Some(name) => format!("Исполнитель {name}"),
            None => "Исполнитель".to_owned(),
        },
        ListKey::TopTracks(_) => "Топ месяца".to_owned(),
        ListKey::SearchTracks(_) => "Поиск".to_owned(),
        ListKey::Wave => "Моя волна".to_owned(),
    }
}

impl Library {
    /// Играющая очередь — «Моя волна».
    pub(super) fn in_wave(&self) -> bool {
        self.queue
            .as_ref()
            .is_some_and(|queue| queue.key == ListKey::Wave)
    }

    /// Что сейчас показывает экран волны.
    fn wave_screen(&self) -> Screen {
        let has_track = self.player.loaded && self.current_shown().is_some();
        screen_for(
            self.in_wave(),
            matches!(self.wave.phase, Phase::Idle),
            has_track,
        )
    }

    /// Запоминает, откуда построена очередь `key` (для таблетки на экране
    /// волны): название берём у страницы в стопке, пока она есть.
    pub(super) fn remember_source(&mut self, key: &ListKey) {
        let title = self.stack.iter().find_map(|entry| {
            let serial = match key {
                ListKey::Album(serial)
                | ListKey::Playlist(serial)
                | ListKey::ArtistPopular(serial)
                | ListKey::ArtistTracks(serial) => *serial,
                _ => return None,
            };
            if entry.serial != serial {
                return None;
            }
            match &entry.page {
                super::pages::Page::Album { load, .. } => {
                    load.ready().map(|view| view.card.title.clone())
                }
                super::pages::Page::Playlist { load, .. } => {
                    load.ready().map(|view| view.card.title.clone())
                }
                super::pages::Page::Artist { load, .. } => {
                    load.ready().map(|view| view.card.name.clone())
                }
                super::pages::Page::ArtistTracks { name, .. } => Some(name.clone()),
                super::pages::Page::TopArtists { .. } => None,
            }
        });
        self.wave.source = source_text(key, title.as_deref());
    }

    /// Нужны ли кадры: кольца пульсируют, только пока экран волны виден.
    pub(super) fn wave_animating(&self) -> bool {
        if self.section != Section::Wave || !self.stack.is_empty() {
            return false;
        }
        match self.wave_screen() {
            Screen::Wave => self.wave.waiting,
            // Спектр под чужим треком идёт кадрами визуализатора.
            Screen::Foreign => false,
            Screen::Start => matches!(self.wave.phase, Phase::Idle | Phase::Starting),
        }
    }

    /// Цвет волны на экране в момент `now` (с плавным переходом).
    pub(super) fn wave_color(&self, now: Instant) -> Color {
        self.wave.tint.at(now)
    }

    /// Идёт переход цвета: нужны кадры.
    pub(super) fn wave_tint_running(&self, now: Instant) -> bool {
        self.wave.tint.animating(now)
    }

    /// Данные волны для играющего трека: есть, только когда играет волна.
    fn current_wave_meta(&self) -> Option<&Meta> {
        if !self.in_wave() {
            return None;
        }
        self.wave.items.get(self.queue.as_ref()?.current)
    }

    /// Какого цвета должна быть волна: акцент, а с VibeColorizer — цвет
    /// играющего трека из любого источника. У трека волны сначала цвета и
    /// оттенок из ответа, у остальных — только среднее обложки.
    fn wave_target(&self, covers: &Covers) -> Color {
        let accent = theme::accent();
        if !self.plugins.vibe {
            return accent;
        }
        let Some(track) = self.current_shown() else {
            return accent;
        };
        let cover = covers.average(track.cover_uri.as_deref());
        vibe::pick(vibe_sources(self.current_wave_meta(), cover)).unwrap_or(accent)
    }

    /// Обновляет цель перехода цвета. Зовётся после каждого сообщения окна
    /// и когда загрузилась обложка.
    pub fn sync_wave_tint(&mut self, covers: &Covers, now: Instant) {
        let target = self.wave_target(covers);
        self.wave.tint.retarget(target, now);
    }

    /// Большая обложка играющего трека: для экрана волны (она на виду) и
    /// для VibeColorizer, если своего цвета у трека нет (не волна или в
    /// ответе волны нет ни цвета, ни оттенка).
    pub fn wave_cover(&self) -> Option<&str> {
        // На стартовом экране волны обложки не видно.
        let visible = self.section == Section::Wave
            && self.stack.is_empty()
            && self.wave_screen() != Screen::Start;
        let needs_color = self.plugins.vibe && !has_own_color(self.current_wave_meta());
        if !(visible || needs_color) {
            return None;
        }
        self.current_shown()?.cover_uri.as_deref()
    }

    /// Повтор текущего трека: в волне свой, в списках — настройка плеера.
    pub(super) fn repeat_one(&self) -> bool {
        if self.in_wave() {
            self.wave.repeat_one
        } else {
            self.player.repeat == super::Repeat::One
        }
    }

    /// «Повтор» в плеере: в волне переключает «выкл ↔ трек».
    pub(super) fn cycle_wave_repeat(&mut self) {
        self.wave.repeat_one = toggled_repeat(self.wave.repeat_one);
    }

    // ---------------------------------------------------------- события

    /// Закрывает объявленный трек волны: `skip` или `trackFinished`. Если
    /// звук трека так и не пошёл, события нет.
    pub(super) fn wave_leave(&mut self, how: Leave) {
        let position = Duration::try_from_secs_f32(self.player.position).unwrap_or_default();
        let played = match how {
            Leave::Finish => self
                .current_duration()
                .and_then(|seconds| Duration::try_from_secs_f32(seconds).ok())
                .unwrap_or(position),
            Leave::Skip => position,
        };
        if let Some(pending) = self.wave.tracker.leave(how, played) {
            self.wave.feedback.push(pending);
        }
    }

    /// Звук трека пошёл: объявляем его (`trackStarted`).
    pub(super) fn wave_sounded(&mut self) {
        if !self.in_wave() {
            return;
        }
        let Some(queue) = &self.queue else {
            return;
        };
        let (Some(track), Some(meta)) = (queue.track(), self.wave.items.get(queue.current)) else {
            return;
        };
        if let Some(pending) = self.wave.tracker.started(meta, api::queue_id(track)) {
            self.wave.feedback.push(pending);
        }
    }

    /// Следующее событие уходит, когда закончился предыдущий запрос.
    fn feedback_pump(&mut self) {
        let Some(http) = self.http.clone() else {
            return;
        };
        let Some(pending) = self.wave.feedback.next() else {
            return;
        };
        let token = self.token.clone();
        self.tasks.push(Task::perform(
            async move {
                // Ошибки обратной связи не показываем: звуку они не мешают.
                let _ = send_wave_feedback(
                    &http,
                    &token.access_token,
                    &pending.session,
                    &pending.batch_id,
                    pending.from.as_deref(),
                    &pending.event,
                )
                .await;
            },
            |()| Message::Wave(Msg::FeedbackDone),
        ));
    }

    /// После каждого сообщения: догрузка и отправка событий.
    pub(super) fn wave_pump(&mut self) {
        self.wave_top_up();
        self.feedback_pump();
    }

    // ---------------------------------------------------------- очередь

    fn wave_top_up(&mut self) {
        if !self.in_wave() || self.wave.fetching || self.wave.blocked {
            return;
        }
        let Some(queue) = &self.queue else {
            return;
        };
        if !needs_more(queue.current, queue.tracks.len()) {
            return;
        }
        let (Some(session), Some(http)) = (self.wave.session.clone(), self.http.clone()) else {
            return;
        };
        let skip = queue.tracks.len().saturating_sub(QUEUE_LEN);
        let ids: Vec<String> = queue.tracks.iter().skip(skip).map(api::queue_id).collect();
        let token = self.token.clone();
        let serial = self.wave.serial;
        self.wave.fetching = true;
        self.tasks.push(Task::perform(
            async move { fetch_wave_tracks(&http, &token.access_token, &session, &ids).await },
            move |result| Message::Wave(Msg::Fetched(serial, result.map_err(Failure::from))),
        ));
    }

    /// Новые треки в конец очереди.
    fn wave_append(&mut self, fresh: Vec<(Track, Meta)>) {
        let Some(queue) = &mut self.queue else {
            return;
        };
        if queue.key != ListKey::Wave {
            return;
        }
        let tracks = Arc::make_mut(&mut queue.tracks);
        for (track, meta) in fresh {
            if meta.liked {
                self.liked.insert(track.id.clone());
            }
            tracks.push(track);
            self.wave.items.push(meta);
        }
    }

    /// Порция пришла (из догрузки или из новой сессии).
    fn wave_take(&mut self, batch: &WaveBatch) -> Option<Effect> {
        let fresh = take_fresh(&mut self.wave.seen, batch);
        self.wave.session = Some(batch.session.clone());
        if fresh.is_empty() {
            self.wave.empty_streak = self.wave.empty_streak.saturating_add(1);
            if self.wave.empty_streak > MAX_EMPTY {
                self.wave.blocked = true;
                self.wave.waiting = false;
                return Some(Effect::Toast(
                    "Волна пока не нашла новых треков. Попробуйте позже.".to_owned(),
                ));
            }
            // Пустая порция: `wave_pump` попросит следующую.
            return None;
        }
        self.wave.empty_streak = 0;
        self.wave_append(fresh);
        if self.wave.waiting {
            self.wave.waiting = false;
            return self.start_toward(self.wave.resume_at, 0.0, true);
        }
        None
    }

    /// Очередь кончилась, а пользователю нужен следующий трек: ждём порцию.
    pub(super) fn wave_exhausted(&mut self) -> Option<Effect> {
        if !self.in_wave() {
            return None;
        }
        self.wave_leave(Leave::Skip);
        self.send(crate::player::Command::Stop);
        self.halt();
        self.player.set_position(0.0);
        self.wave.resume_at = self.queue.as_ref().map_or(0, |queue| queue.tracks.len());
        self.wave.waiting = true;
        self.wave.blocked = false;
        self.wave.empty_streak = 0;
        None
    }

    fn wave_fetch_failed(&mut self, failure: &Failure) -> Option<Effect> {
        self.wave.fetching = false;
        self.wave.blocked = true;
        self.wave.waiting = false;
        if failure.kind == FailureKind::Unauthorized {
            return Some(Effect::Relogin);
        }
        Some(Effect::Toast(sentence(&format!(
            "Не удалось загрузить продолжение волны: {}",
            failure.text
        ))))
    }

    fn wave_event_radio_started(&mut self, batch: &WaveBatch) {
        self.wave.feedback.push(Pending {
            session: batch.session.clone(),
            batch_id: batch.batch_id.clone(),
            from: batch.from.clone(),
            event: WaveEvent::RadioStarted,
        });
    }

    fn wave_new_session(&mut self) -> Option<Task<Message>> {
        let http = self.http.clone()?;
        let token = self.token.clone();
        let serial = self.wave.serial;
        let settings = self.wave.settings.clone();
        Some(Task::perform(
            async move { start_wave(&http, &token.access_token, &settings).await },
            move |result| Message::Wave(Msg::Renewed(serial, result.map_err(Failure::from))),
        ))
    }

    // ---------------------------------------------------------- сообщения

    pub(super) fn wave_update(&mut self, msg: Msg) -> (Task<Message>, Option<Effect>) {
        match msg {
            Msg::Start => self.wave_start(),
            Msg::Menu => {
                let open = !self.wave.menu.value();
                self.wave.menu.go_mut(open, Instant::now());
                (
                    if open {
                        self.wave_load_stations()
                    } else {
                        Task::none()
                    },
                    None,
                )
            }
            Msg::CloseMenu => {
                self.close_wave_menu();
                (Task::none(), None)
            }
            Msg::Stations(result) => {
                match result {
                    Ok(list) => self.wave.stations = Stations::Ready(list),
                    Err(failure) if failure.kind == FailureKind::Unauthorized => {
                        self.wave.stations = Stations::Idle;
                        return (Task::none(), Some(Effect::Relogin));
                    }
                    // Занятий нет: раздел скрыт, остальное меню работает.
                    Err(_) => self.wave.stations = Stations::Failed,
                }
                (Task::none(), None)
            }
            Msg::Pick(pick) => self.wave_pick(pick),
            Msg::Started(serial, result) => {
                if serial != self.wave.serial {
                    return (Task::none(), None);
                }
                (Task::none(), self.wave_started(result))
            }
            Msg::Fetched(serial, result) => {
                if serial != self.wave.serial {
                    return (Task::none(), None);
                }
                self.wave.fetching = false;
                if !self.in_wave() {
                    return (Task::none(), None);
                }
                match result {
                    Ok(WaveTracks::Batch(batch)) => {
                        self.wave.renewed = false;
                        (Task::none(), self.wave_take(&batch))
                    }
                    Ok(WaveTracks::Expired) if self.wave.renewed => {
                        self.wave.blocked = true;
                        self.wave.waiting = false;
                        let toast = Effect::Toast(
                            "Сеанс волны истёк. Запустите «Мою волну» заново.".to_owned(),
                        );
                        (Task::none(), Some(toast))
                    }
                    Ok(WaveTracks::Expired) => match self.wave_new_session() {
                        Some(task) => {
                            self.wave.renewed = true;
                            self.wave.fetching = true;
                            (task, None)
                        }
                        None => (Task::none(), None),
                    },
                    Err(failure) => (Task::none(), self.wave_fetch_failed(&failure)),
                }
            }
            Msg::Renewed(serial, result) => {
                if serial != self.wave.serial {
                    return (Task::none(), None);
                }
                self.wave.fetching = false;
                if !self.in_wave() {
                    return (Task::none(), None);
                }
                match result {
                    Ok(batch) => {
                        self.wave.renewed = false;
                        self.wave_event_radio_started(&batch);
                        (Task::none(), self.wave_take(&batch))
                    }
                    Err(failure) => (Task::none(), self.wave_fetch_failed(&failure)),
                }
            }
            Msg::Retry => {
                if self.in_wave() {
                    (Task::none(), self.wave_exhausted())
                } else {
                    self.wave_start()
                }
            }
            Msg::Dislike => self.wave_dislike(),
            Msg::DislikeDone(result) => {
                let effect = match result {
                    Ok(()) => None,
                    Err(failure) if failure.kind == FailureKind::Unauthorized => {
                        Some(Effect::Relogin)
                    }
                    Err(failure) => Some(Effect::Toast(sentence(&format!(
                        "Не удалось поставить «Не нравится»: {}",
                        failure.text
                    )))),
                };
                (Task::none(), effect)
            }
            Msg::FeedbackDone => {
                self.wave.feedback.done();
                (Task::none(), None)
            }
        }
    }

    /// Новая сессия «Моей волны».
    fn wave_start(&mut self) -> (Task<Message>, Option<Effect>) {
        if matches!(self.wave.phase, Phase::Starting) {
            return (Task::none(), None);
        }
        let Some(http) = self.http.clone() else {
            let toast = Effect::Toast(sentence("не удалось подготовить сетевое соединение"));
            return (Task::none(), Some(toast));
        };
        self.wave.phase = Phase::Starting;
        self.wave.serial = self.wave.serial.wrapping_add(1);
        self.wave.fetching = false;
        let token = self.token.clone();
        let serial = self.wave.serial;
        let settings = self.wave.settings.clone();
        let task = Task::perform(
            async move { start_wave(&http, &token.access_token, &settings).await },
            move |result| Message::Wave(Msg::Started(serial, result.map_err(Failure::from))),
        );
        (task, None)
    }

    /// Занятия для меню: один запрос на открытие, пока список не получен.
    fn wave_load_stations(&mut self) -> Task<Message> {
        if !matches!(self.wave.stations, Stations::Idle | Stations::Failed) {
            return Task::none();
        }
        let Some(http) = self.http.clone() else {
            self.wave.stations = Stations::Failed;
            return Task::none();
        };
        self.wave.stations = Stations::Loading;
        let token = self.token.clone();
        Task::perform(
            async move { fetch_activities(&http, &token.access_token).await },
            |result| Message::Wave(Msg::Stations(result.map_err(Failure::from))),
        )
    }

    /// Выбор в меню: смена любой настройки — новая сессия волны, она сразу
    /// запускается. Выбор запоминается в `config.toml` (эффект), в аккаунт не
    /// пишется.
    fn wave_pick(&mut self, pick: Pick) -> (Task<Message>, Option<Effect>) {
        let next = picked(&self.wave.settings, pick);
        if next == self.wave.settings {
            return (Task::none(), None);
        }
        self.wave.settings = next.clone();
        // Прежний запуск (если ещё идёт) отменяется номером сессии.
        if matches!(self.wave.phase, Phase::Starting) {
            self.wave.phase = Phase::Idle;
        }
        let (task, _) = self.wave_start();
        (task, Some(Effect::WaveSettings(next)))
    }

    /// Настройки волны из `config.toml`.
    pub fn set_wave_settings(&mut self, settings: WaveSettings) {
        self.wave.settings = settings;
    }

    /// Esc или щелчок вне меню. `false` — оно и не было открыто.
    pub fn close_wave_menu(&mut self) -> bool {
        let open = self.wave.menu.value();
        if open {
            self.wave.menu.go_mut(false, Instant::now());
        }
        open
    }

    /// Меню движется: ему нужны кадры.
    pub fn wave_menu_running(&self, now: Instant) -> bool {
        self.wave.menu.is_animating(now)
    }

    fn wave_started(&mut self, result: Result<WaveBatch, Failure>) -> Option<Effect> {
        let batch = match result {
            Ok(batch) => batch,
            Err(failure) if failure.kind == FailureKind::Unauthorized => {
                self.wave.phase = Phase::Idle;
                return Some(Effect::Relogin);
            }
            Err(failure) => {
                self.wave.phase = Phase::Failed(failure);
                return None;
            }
        };
        // Данные прежней сессии не нужны; события, что ещё не ушли, остаются.
        self.wave.seen = Seen::default();
        self.wave.items.clear();
        let fresh = take_fresh(&mut self.wave.seen, &batch);
        if fresh.is_empty() {
            self.wave.phase = Phase::Empty;
            return None;
        }
        self.wave.phase = Phase::Idle;
        self.wave.session = Some(batch.session.clone());
        self.wave.renewed = false;
        self.wave.empty_streak = 0;
        self.wave.blocked = false;
        self.wave.waiting = false;
        self.wave.repeat_one = false;
        self.wave.resume_at = 0;
        self.wave_event_radio_started(&batch);
        self.queue = Some(Queue::new(ListKey::Wave, Arc::new(Vec::new()), 0));
        self.wave_append(fresh);
        self.chosen = None;
        self.player.skipped = 0;
        self.start(0, 0.0)
    }

    /// «Не нравится»: `skip`, запись в аккаунт (настоящему треку, не
    /// подмене) и следующий трек. Запись в аккаунт — решение пользователя:
    /// он сам нажал кнопку.
    fn wave_dislike(&mut self) -> (Task<Message>, Option<Effect>) {
        if !self.in_wave() {
            return (Task::none(), None);
        }
        let Some(track) = self.current().cloned() else {
            return (Task::none(), None);
        };
        self.wave_leave(Leave::Skip);
        // Сервер сам снимает «Нравится» при «Не нравится».
        if self.liked.contains(&track.id) {
            self.set_liked(&track, false);
        }
        let task = match self.http.clone() {
            Some(http) => {
                let token = self.token.clone();
                let uid = self.status.account().uid;
                let ids = vec![track.id.clone()];
                Task::perform(
                    async move {
                        api::add_disliked_tracks(&http, &token.access_token, uid, &ids)
                            .await
                            .map_err(Failure::from)
                    },
                    |result| Message::Wave(Msg::DislikeDone(result)),
                )
            }
            None => Task::none(),
        };
        self.player.skipped = 0;
        let index = self.queue.as_ref().map_or(0, |queue| queue.current);
        let effect = self.step(index, true);
        (task, effect)
    }

    // ------------------------------------------------------------ вид

    /// Экран «Моя волна».
    pub(super) fn wave_view<'a>(
        &'a self,
        covers: &'a Covers,
        turn: f32,
        now: Instant,
        visualizer: bool,
    ) -> Element<'a, App> {
        let screen = self.wave_screen();
        let playing = screen != Screen::Start;
        let body = if playing {
            self.wave_playing(covers, turn, now, visualizer, screen == Screen::Foreign)
        } else {
            match &self.wave.phase {
                Phase::Idle => self.wave_idle(now),
                Phase::Starting => center(
                    column![
                        spinner(40.0, self.wave_color(now), turn),
                        theme::line("Загружаем волну…", Style::Body, theme::TEXT_SECONDARY),
                    ]
                    .spacing(14)
                    .align_x(Alignment::Center),
                )
                .into(),
                Phase::Failed(failure) => wave_problem(
                    "Не удалось запустить волну",
                    failure.text.clone(),
                    Lib(Message::Wave(Msg::Retry)),
                ),
                Phase::Empty => wave_problem(
                    "Волна пока не нашла треков",
                    "Попробуйте позже.".to_owned(),
                    Lib(Message::Wave(Msg::Retry)),
                ),
            }
        };
        let tint = self.wave_color(now);
        let mut layers = stack![
            fill(move || container::Style {
                background: Some(Background::Gradient(
                    Linear::new(Degrees(180.0))
                        .add_stop(0.0, theme::alpha(tint, 0.35))
                        .add_stop(1.0, theme::clear(tint))
                        .into(),
                )),
                ..container::Style::default()
            }),
            body
        ];
        // Полноэкранный плеер: значок в правом верхнем углу.
        if playing {
            layers = layers.push(
                container(widgets::with_tooltip(
                    widgets::icon_button(
                        Icon::Expand,
                        Size::M,
                        false,
                        Some(Lib(Message::Fullscreen(true))),
                    ),
                    "Полноэкранный плеер",
                    iced::widget::tooltip::Position::Left,
                ))
                .width(Length::Fill)
                .align_x(Alignment::End)
                .padding(16),
            );
        }
        layers.into()
    }

    /// Волна не запущена: большая кнопка и кольца, что медленно пульсируют.
    fn wave_idle<'a>(&'a self, now: Instant) -> Element<'a, App> {
        let seconds = now
            .saturating_duration_since(self.wave.started)
            .as_secs_f32();
        let accent = self.wave_color(now);
        let ring = move |size: f32, shift: f32, alpha: f32| -> Element<'a, App> {
            let wave =
                0.5 - 0.5 * (std::f32::consts::TAU * (seconds / PULSE_SECONDS + shift)).cos();
            let side = size * (1.0 + PULSE_SCALE * wave);
            center(
                container(Space::new())
                    .width(side)
                    .height(side)
                    .style(move |_| container::Style {
                        background: Some(theme::alpha(accent, alpha).into()),
                        border: Border::default().rounded(side / 2.0),
                        ..container::Style::default()
                    }),
            )
            .into()
        };
        let play = button(center(icon(Icon::PlayFilled, 40.0, theme::on_accent())))
            .width(96)
            .height(96)
            .padding(0)
            .style(move |_, status| button::Style {
                background: Some(
                    match status {
                        button::Status::Hovered => theme::mix(accent, Color::WHITE, 0.16),
                        button::Status::Pressed => theme::mix(accent, Color::BLACK, 0.16),
                        _ => accent,
                    }
                    .into(),
                ),
                border: Border::default().rounded(48.0),
                ..button::Style::default()
            })
            .on_press(Lib(Message::Wave(Msg::Start)));
        let rings = container(stack![
            ring(280.0, 0.5, 0.05),
            ring(220.0, 0.25, 0.08),
            ring(160.0, 0.0, 0.14),
            center(mouse_pointer(play)),
        ])
        .width(300)
        .height(300);
        center(
            column![
                rings,
                theme::line("Моя волна", Style::Display, theme::TEXT_PRIMARY),
                theme::label(
                    "Бесконечный поток музыки под ваш вкус. «Нравится» и «Не нравится» настраивают его на ходу.",
                    Style::Body,
                    theme::TEXT_SECONDARY,
                )
                .center()
                .width(420),
            ]
            .spacing(12)
            .align_x(Alignment::Center),
        )
        .into()
    }

    /// Волна играет: обложка с круговым спектром, название, кнопки, «Далее».
    /// `foreign` — играет не волна: сверху таблетка источника и крестик,
    /// кнопки как в плеере снизу для обычной очереди.
    fn wave_playing<'a>(
        &'a self,
        covers: &'a Covers,
        turn: f32,
        now: Instant,
        visualizer: bool,
        foreign: bool,
    ) -> Element<'a, App> {
        let Some(queue) = &self.queue else {
            return Space::new().into();
        };
        let (Some(original), Some(track)) = (self.current(), self.current_shown()) else {
            return Space::new().into();
        };
        let exhausted = queue.current + 1 >= queue.tracks.len();
        // Очередь кончилась и догрузка не удалась: волна встала.
        if !foreign && self.wave.blocked && exhausted && !self.player.loaded && !self.wave.waiting {
            return wave_problem(
                "Волна прервалась",
                "Не удалось получить продолжение. Проверьте соединение.".to_owned(),
                Lib(Message::Wave(Msg::Retry)),
            );
        }
        let accent = self.wave_color(now);
        let halo = move |side: f32, alpha: f32| -> Element<'a, App> {
            center(
                container(Space::new())
                    .width(side)
                    .height(side)
                    .style(move |_| container::Style {
                        background: Some(theme::alpha(accent, alpha).into()),
                        border: Border::default().rounded(side / 2.0),
                        ..container::Style::default()
                    }),
            )
            .into()
        };
        // Обложка: сначала маленькая, большая проявляется поверх неё.
        let base = crossfade_cover(
            covers,
            track,
            self.motion.cover_from.as_ref(),
            super::anim::progress(&self.motion.cover_fade, now),
            COVER,
            16.0,
        );
        let cover: Element<'a, App> = match covers.big(track.cover_uri.as_deref(), now) {
            Some((big, opacity)) if opacity >= 1.0 => {
                widgets::picture(&big.cover, COVER, 16.0, 1.0)
            }
            Some((big, opacity)) => {
                stack![base, widgets::picture(&big.cover, COVER, 16.0, opacity)].into()
            }
            None => base,
        };
        // Щелчок по обложке открывает полноэкранный плеер; при наведении
        // проступает значок «развернуть», как в плеере снизу.
        let glyph: Element<'a, App> = icon(Icon::Expand, 24.0, theme::TEXT_PRIMARY).into();
        let badge = container(center(glyph))
            .width(COVER)
            .height(COVER)
            .style(|_| container::Style {
                background: Some(theme::alpha(theme::BG, 0.5).into()),
                border: Border::default().rounded(16.0),
                ..container::Style::default()
            });
        let cover: Element<'a, App> = iced::widget::mouse_area(widgets::with_tooltip(
            iced::widget::hover(cover, badge),
            "Полноэкранный плеер",
            iced::widget::tooltip::Position::Top,
        ))
        .interaction(mouse::Interaction::Pointer)
        .on_press(Lib(Message::Fullscreen(true)))
        .into();
        // Круговой спектр за обложкой; выключен в настройках — мягкий ореол.
        let backdrop: Element<'a, App> = if visualizer {
            center(visualizer::rays(
                self.spectrum.levels(),
                accent,
                RAYS_INNER,
                RAYS_LENGTH,
            ))
            .into()
        } else {
            stack![halo(300.0, 0.06), halo(256.0, 0.09)].into()
        };
        let art = container(stack![backdrop, center(cover)])
            .width(ART)
            .height(ART);

        let mut tags = row![].spacing(6).align_y(Alignment::Center);
        if self.ai_track(original) {
            tags = tags.push(plugins::tag("ИИ", theme::ERROR));
        }
        if self.replaced(original) {
            tags = tags.push(plugins::tag("Замена", theme::accent()));
        }
        let artists: Element<'a, App> = if track.artists.iter().any(|artist| artist.id.is_some()) {
            container(browse::artist_links(
                &track.artists,
                Style::Headline,
                theme::TEXT_SECONDARY,
            ))
            .width(TEXT_WIDTH)
            .align_x(Alignment::Center)
            .clip(true)
            .into()
        } else {
            container(
                marquee(track.artists_text(), Style::Headline, theme::TEXT_SECONDARY).centered(),
            )
            .width(TEXT_WIDTH)
            .into()
        };
        let liked = self.liked.contains(&original.id);
        let playing = self.player.playing;
        // Порядок, как в плеере снизу: «Не нравится», назад, пауза, вперёд,
        // повтор, «Нравится»; вне волны ещё и перемешивание.
        let dislike: Element<'a, App> = if foreign {
            // Как в плеере снизу вне волны: «Не нравится» не работает.
            widgets::icon_button(Icon::Dislike, Size::M, false, None)
        } else {
            widgets::with_tooltip(
                widgets::icon_button(
                    Icon::Dislike,
                    Size::M,
                    false,
                    Some(Lib(Message::Wave(Msg::Dislike))),
                ),
                "Не нравится: пропустить трек",
                iced::widget::tooltip::Position::Top,
            )
        };
        let shuffle: Element<'a, App> = if foreign {
            widgets::icon_button(
                Icon::Shuffle,
                Size::M,
                self.player.shuffle,
                Some(Lib(Message::ToggleShuffle)),
            )
        } else {
            Space::new().width(0).into()
        };
        let (repeat_icon, repeat_on) = if foreign {
            (
                if self.repeat_one() {
                    Icon::RepeatOne
                } else {
                    Icon::Repeat
                },
                self.player.repeat != super::Repeat::Off,
            )
        } else {
            (
                if self.wave.repeat_one {
                    Icon::RepeatOne
                } else {
                    Icon::Repeat
                },
                self.wave.repeat_one,
            )
        };
        let buttons = row![
            dislike,
            shuffle,
            widgets::icon_button(
                Icon::PreviousFilled,
                Size::M,
                false,
                // В волне назад можно, только если в ней уже что-то играло.
                (foreign || queue.current > 0).then_some(Lib(Message::Previous)),
            ),
            widgets::icon_button(
                if playing {
                    Icon::PauseFilled
                } else {
                    Icon::PlayFilled
                },
                Size::M,
                true,
                Some(Lib(Message::TogglePlay)),
            ),
            widgets::icon_button(Icon::NextFilled, Size::M, false, Some(Lib(Message::Next)),),
            widgets::with_tooltip(
                widgets::icon_button(
                    repeat_icon,
                    Size::M,
                    repeat_on,
                    Some(Lib(Message::CycleRepeat)),
                ),
                if foreign {
                    "Повтор"
                } else {
                    "Повторять трек"
                },
                iced::widget::tooltip::Position::Top,
            ),
            widgets::icon_button(
                if liked {
                    Icon::HeartFilled
                } else {
                    Icon::Heart
                },
                Size::M,
                liked,
                Some(Lib(Message::ToggleLike(original.clone()))),
            ),
        ]
        .spacing(14)
        .align_y(Alignment::Center);

        let next: Element<'a, App> = if foreign {
            Space::new().height(18).into()
        } else if self.wave.waiting {
            widgets::waiting("Загружаем волну…".to_owned(), turn)
        } else {
            let text = match queue.tracks.get(queue.current + 1) {
                Some(next) => {
                    let next = self.shown(next);
                    format!("Далее: {} — {}", next.display_title(), next.artists_text())
                }
                None if self.wave.blocked => "Далее: пока неизвестно".to_owned(),
                None => "Далее: загружаем…".to_owned(),
            };
            container(theme::line(text, Style::Secondary, theme::TEXT_TERTIARY))
                .width(TEXT_WIDTH)
                .align_x(Alignment::Center)
                .clip(true)
                .into()
        };
        let quality: Element<'a, App> = match self.player.stream {
            Some(info) => quality_badge(info, true),
            None => Space::new().height(24).into(),
        };
        // Длинное название прокручивается, как в плеере снизу.
        let title = container(
            marquee(track.display_title(), Style::Display, theme::TEXT_PRIMARY).centered(),
        )
        .width(TEXT_WIDTH);
        // Над названием: «Моя волна» или (чужая очередь) таблетка источника
        // и крестик, что запускает волну.
        let head: Element<'a, App> = if foreign {
            let label = if self.wave.source.is_empty() {
                "Очередь"
            } else {
                self.wave.source.as_str()
            };
            row![
                // Таблетка открывает то же меню, что и у волны: выбор
                // запускает волну с этими настройками.
                source_pill(label.to_owned(), Some(Lib(Message::Wave(Msg::Menu)))),
                widgets::with_tooltip(
                    widgets::icon_button(
                        Icon::Close,
                        Size::S,
                        false,
                        Some(Lib(Message::Wave(Msg::Start))),
                    ),
                    "Запустить «Мою волну»",
                    iced::widget::tooltip::Position::Right,
                ),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .into()
        } else {
            source_pill(
                self.wave.settings.label(),
                Some(Lib(Message::Wave(Msg::Menu))),
            )
        };
        center(
            column![
                head,
                art,
                column![title, artists, tags]
                    .spacing(6)
                    .align_x(Alignment::Center),
                buttons,
                quality,
                next,
            ]
            .spacing(10)
            .align_x(Alignment::Center),
        )
        .into()
    }
}

/// Таблетка источника над названием: подпись и ⌄. `on_press` — меню
/// настроек волны; без него таблетка не нажимается, без наведения и
/// курсора-руки.
fn source_pill<'a>(label: String, on_press: Option<App>) -> Element<'a, App> {
    // Одно целое: подпись и шеврон в одном ряду, равные отступы по краям,
    // у шеврона ни подложки, ни своей кнопки.
    let content = row![
        container(theme::line(label, Style::BodyStrong, theme::TEXT_PRIMARY))
            .max_width(360)
            .clip(true),
        icon(Icon::ChevronDown, 16.0, theme::TEXT_SECONDARY),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    let style = |fill: f32| button::Style {
        background: Some(theme::alpha(theme::TEXT_PRIMARY, fill).into()),
        border: Border::default().rounded(widgets::ROUND),
        ..button::Style::default()
    };
    match on_press {
        // Наведение подсвечивает всю таблетку.
        Some(message) => mouse_pointer(
            button(content)
                .padding([6, 14])
                .style(move |_, status| match status {
                    button::Status::Hovered | button::Status::Pressed => style(0.2),
                    _ => style(0.1),
                })
                .on_press(message),
        ),
        None => container(content)
            .padding([6, 14])
            .style(move |_| container::Style {
                background: Some(theme::alpha(theme::TEXT_PRIMARY, 0.1).into()),
                border: Border::default().rounded(widgets::ROUND),
                ..container::Style::default()
            })
            .into(),
    }
}

impl Library {
    /// Меню настроек волны поверх окна: таблетки-переключатели по группам.
    /// `None` — закрыто.
    ///
    /// Стоит по центру области контента: правее бокового меню (`sidebar`),
    /// между заголовком окна и нижним плеером. Низкое окно — меню ниже не
    /// становится, содержимое прокручивается внутри него (`window` — размер
    /// окна в единицах раскладки).
    pub fn wave_menu(
        &self,
        now: Instant,
        window: iced::Size,
        sidebar: f32,
    ) -> Option<Element<'_, App>> {
        let frame = anim::Popover::at(&self.wave.menu, now);
        if !frame.visible() || self.section != Section::Wave || !self.stack.is_empty() {
            return None;
        }
        let settings = &self.wave.settings;
        let pick = |pick: Pick| Lib(Message::Wave(Msg::Pick(pick)));
        let header = row![
            container(theme::line(
                "Настройки волны",
                Style::Title,
                theme::TEXT_PRIMARY
            ))
            .width(Length::Fill),
            widgets::icon_button(
                Icon::Close,
                Size::S,
                false,
                Some(Lib(Message::Wave(Msg::CloseMenu))),
            ),
        ]
        .align_y(Alignment::Center);
        let mut groups = column![].spacing(18);
        // Занятия — только если список получен.
        if let Stations::Ready(activities) = &self.wave.stations
            && !activities.is_empty()
        {
            let mut pills = vec![choice_pill(
                "Любое".to_owned(),
                settings.activity.is_none(),
                pick(Pick::Activity(None)),
            )];
            for activity in activities {
                let selected = settings
                    .activity
                    .as_ref()
                    .is_some_and(|chosen| chosen.tag == activity.tag);
                pills.push(choice_pill(
                    activity.name.clone(),
                    selected,
                    pick(Pick::Activity(Some(activity.clone()))),
                ));
            }
            groups = groups.push(group("Занятие", pills));
        }
        groups = groups
            .push(group(
                "Характер",
                choices(
                    "По умолчанию",
                    settings.diversity,
                    Diversity::ALL,
                    Diversity::title,
                    |value| pick(Pick::Diversity(value)),
                ),
            ))
            .push(group(
                "Настроение",
                choices(
                    "Любое",
                    settings.mood,
                    Mood::ALL,
                    Mood::title,
                    |value| pick(Pick::Mood(value)),
                ),
            ))
            .push(group(
                "Язык",
                choices(
                    "Любой",
                    settings.language,
                    Language::ALL,
                    Language::title,
                    |value| pick(Pick::Language(value)),
                ),
            ));
        let reset = widgets::action(
            Kind::Secondary,
            Some(Icon::Retry),
            "Сбросить",
            (!settings.is_default()).then(|| pick(Pick::Reset)),
        );
        let footer = row![
            container(theme::label(
                "Смена настроек запускает новую волну",
                Style::Caption,
                theme::TEXT_TERTIARY,
            ))
            .width(Length::Fill),
            reset,
        ]
        .spacing(12)
        .align_y(Alignment::Center);
        let (top, bottom) = (TITLE_BAR + WAVE_MENU_MARGIN, PLAYER_BAR + WAVE_MENU_MARGIN);
        let available = menu_height(window.height, top, bottom);
        let groups = iced::widget::scrollable(groups).width(Length::Fill);
        // Появление: карточка проступает из прозрачности, опускаясь на
        // несколько пикселей и чуть вырастая, за ней — содержимое.
        let content = anim::motion(column![header, groups, footer].spacing(18))
            .reveal(theme::SURFACE_2, frame.content);
        let card = container(content)
            .width(Length::Fill)
            .max_width(WAVE_MENU_WIDTH)
            .max_height(available)
            .padding(20)
            .style(move |_| widgets::popover_card(frame.shell, 16.0));
        let hidden = 1.0 - frame.motion;
        let card = anim::motion(iced::widget::opaque(card))
            .offset(0.0, -12.0 * hidden)
            .scale(0.97 + 0.03 * frame.motion);
        let placed = container(card)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .padding(iced::Padding {
                top,
                right: WAVE_MENU_MARGIN,
                bottom,
                left: sidebar + 1.0 + WAVE_MENU_MARGIN,
            });
        Some(iced::widget::opaque(
            iced::widget::mouse_area(placed).on_press(Lib(Message::Wave(Msg::CloseMenu))),
        ))
    }
}

/// Ширина карточки меню волны.
const WAVE_MENU_WIDTH: f32 = 460.0;
/// Отступ карточки от краёв области контента.
const WAVE_MENU_MARGIN: f32 = 16.0;
/// Высота заголовка окна и нижнего плеера (с линией): их меню не закрывает.
const TITLE_BAR: f32 = 36.0;
const PLAYER_BAR: f32 = super::PLAYER_HEIGHT + 1.0;

/// Наибольшая высота меню в окне высотой `window` при отступах `top` и
/// `bottom`: не меньше разумного минимума, чтобы в очень низком окне меню не
/// схлопнулось в ноль.
fn menu_height(window: f32, top: f32, bottom: f32) -> f32 {
    (window - top - bottom).max(160.0)
}

/// Группа таблеток с подписью.
fn group<'a>(title: &'a str, pills: Vec<Element<'a, App>>) -> Element<'a, App> {
    column![
        theme::line(title, Style::Caption, theme::TEXT_TERTIARY),
        row(pills).spacing(8).wrap().vertical_spacing(8),
    ]
    .spacing(8)
    .into()
}

/// Таблетки выбора из `all` и «по умолчанию» первой.
fn choices<'a, T: Copy + PartialEq>(
    none: &'a str,
    current: Option<T>,
    all: &'static [T],
    title: fn(T) -> &'static str,
    message: impl Fn(Option<T>) -> App,
) -> Vec<Element<'a, App>> {
    let mut pills = vec![choice_pill(
        none.to_owned(),
        current.is_none(),
        message(None),
    )];
    for value in all {
        pills.push(choice_pill(
            title(*value).to_owned(),
            current == Some(*value),
            message(Some(*value)),
        ));
    }
    pills
}

/// Таблетка-переключатель: выбранная — в цвете акцента.
fn choice_pill<'a>(title: String, selected: bool, message: App) -> Element<'a, App> {
    let pill = button(theme::line(
        title,
        Style::CaptionStrong,
        if selected {
            theme::on_accent()
        } else {
            theme::TEXT_SECONDARY
        },
    ))
    .padding([6, 14])
    .style(move |_, status| button::Style {
        background: Some(
            match (selected, status) {
                (true, _) => theme::accent(),
                (false, button::Status::Hovered | button::Status::Pressed) => theme::SURFACE_4,
                (false, _) => theme::SURFACE_3,
            }
            .into(),
        ),
        border: Border::default().rounded(widgets::ROUND),
        ..button::Style::default()
    })
    .on_press(message);
    mouse_pointer(pill)
}

/// Курсор-«рука» над кнопкой.
fn mouse_pointer<'a>(content: impl Into<Element<'a, App>>) -> Element<'a, App> {
    iced::widget::mouse_area(content)
        .interaction(mouse::Interaction::Pointer)
        .into()
}

/// Ошибка или пустая волна: заголовок, пояснение, «Повторить».
fn wave_problem<'a>(title: &'a str, text: String, retry: App) -> Element<'a, App> {
    state_block(
        widgets::badge(Icon::Wave, theme::TEXT_SECONDARY, theme::SURFACE_2),
        title,
        text,
        widgets::action(Kind::Primary, Some(Icon::Retry), "Повторить", Some(retry)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::WaveTrack;

    fn track(id: &str) -> Track {
        Track {
            id: id.to_owned(),
            title: Some(format!("Трек {id}")),
            version: None,
            artists: Vec::new(),
            album_id: Some("7".to_owned()),
            album_title: None,
            duration: None,
            available: true,
            cover_uri: None,
            warning: api::Warning::None,
            loudness: None,
            lyrics: None,
        }
    }

    fn colored_meta(hue: Option<f64>, accent: Option<&str>) -> Meta {
        Meta {
            session: "s".to_owned(),
            batch_id: "b".to_owned(),
            from: None,
            liked: false,
            hue,
            colors: accent.map(|accent| WaveColors {
                accent: Some(accent.to_owned()),
                average: None,
            }),
        }
    }

    #[test]
    fn own_color_only_for_wave_tracks_with_data() {
        // Не волна: своего цвета нет, нужна обложка.
        assert!(!has_own_color(None));
        assert!(!has_own_color(Some(&colored_meta(None, None))));
        assert!(has_own_color(Some(&colored_meta(Some(10.0), None))));
        assert!(has_own_color(Some(&colored_meta(None, Some("#FF0000")))));
        // Не волна: цвет только из обложки.
        let sources = vibe_sources(None, Some([0, 0, 255]));
        assert_eq!(sources.accent, None);
        assert_eq!(sources.hue, None);
        assert_eq!(sources.cover, Some([0, 0, 255]));
        // Волна: цвет из ответа идёт первым.
        let wave = colored_meta(Some(120.0), Some("#FF0000"));
        let sources = vibe_sources(Some(&wave), Some([0, 0, 255]));
        assert_eq!(sources.accent, Some("#FF0000"));
        assert_eq!(sources.hue, Some(120.0));
    }

    /// Играет «Мне нравится», не волна: с VibeColorizer цвет берётся из
    /// обложки, без плагина — акцент.
    #[test]
    fn vibe_colors_any_playing_source() {
        use super::super::super::covers::Big;
        use iced::widget::image;

        let uri = "avatars.yandex.net/get-music-content/1/cover/%%";
        let mut song = track("1");
        song.cover_uri = Some(uri.to_owned());
        let mut library = super::super::tests::test_library();
        library.queue = Some(Queue::new(ListKey::Liked, Arc::new(vec![song]), 0));
        assert!(!library.in_wave());
        let mut covers = Covers::new(None);
        let pixel = || image::Handle::from_rgba(1, 1, vec![0, 0, 255, 255]);
        covers.big_finished(
            uri.to_owned(),
            Some(Arc::new(Big {
                cover: pixel(),
                backdrop: pixel(),
                average: Some([0, 0, 255]),
            })),
        );

        // Без плагина обложка не нужна, цвет — акцент.
        library.plugins.vibe = false;
        assert_eq!(library.wave_cover(), None);
        assert_eq!(library.wave_target(&covers), theme::accent());

        library.plugins.vibe = true;
        assert_eq!(library.wave_cover(), Some(uri));
        let color = library.wave_target(&covers);
        assert_ne!(color, theme::accent());
        assert!(color.b > color.r && color.b > color.g, "{color:?}");
        let now = Instant::now();
        library.sync_wave_tint(&covers, now);
        assert_eq!(library.wave_color(now + TINT_FADE), color);
    }

    #[test]
    fn wave_track_changes_land_in_history() {
        let mut library = super::super::tests::test_library();
        let _ = library.wave_started(Ok(batch(&["1", "2", "3", "4"])));
        assert!(library.in_wave());
        // Первый трек зазвучал; «Следующий» — в историю уходит первый.
        library.player.sounded = true;
        let _ = library.update(Message::Next, None);
        let ids = |library: &Library| -> Vec<String> {
            library
                .history
                .tracks()
                .map(|track| track.id.clone())
                .collect()
        };
        assert_eq!(ids(&library), ["1"]);
        // Автопереход волны по концу трека.
        library.player.sounded = true;
        let session = library.player.session;
        let _ = library.audio_event(crate::player::Event::Ended { id: session });
        assert_eq!(ids(&library), ["1", "2"]);
        // Трек, что не успел зазвучать, пропускают: в историю он не идёт.
        let _ = library.update(Message::Next, None);
        assert_eq!(ids(&library), ["1", "2"]);
        // Треки волны перед текущим тоже попадают в «Недавно играли».
        let recent: Vec<String> = library
            .recent_tracks()
            .into_iter()
            .map(|track| track.id.clone())
            .collect();
        // Порядок источников: сначала сеанс (ближе к текущему), затем очередь.
        assert_eq!(recent, ["3", "1", "2"]);
    }

    #[test]
    fn menu_height_follows_window_with_a_floor() {
        assert_eq!(menu_height(800.0, 52.0, 105.0), 643.0);
        // Низкое окно: меню не меньше минимума, остальное — прокрутка внутри.
        assert_eq!(menu_height(200.0, 52.0, 105.0), 160.0);
    }

    #[test]
    fn pick_changes_one_setting_and_reset_clears() {
        let base = WaveSettings::default();
        let calm = picked(&base, Pick::Mood(Some(Mood::Calm)));
        assert_eq!(calm.mood, Some(Mood::Calm));
        let both = picked(&calm, Pick::Language(Some(Language::Russian)));
        assert_eq!(both.mood, Some(Mood::Calm));
        assert_eq!(both.language, Some(Language::Russian));
        // Тот же выбор ничего не меняет (волна не перезапускается).
        assert_eq!(picked(&both, Pick::Mood(Some(Mood::Calm))), both);
        // «Любое» снимает только свою настройку.
        assert_eq!(
            picked(&both, Pick::Mood(None)).language,
            Some(Language::Russian)
        );
        assert_eq!(picked(&both, Pick::Mood(None)).mood, None);
        assert!(picked(&both, Pick::Reset).is_default());
        let activity = Activity {
            tag: "workout".to_owned(),
            name: "Тренировка".to_owned(),
        };
        let with = picked(&both, Pick::Activity(Some(activity.clone())));
        assert_eq!(with.activity, Some(activity));
    }

    fn batch(ids: &[&str]) -> WaveBatch {
        WaveBatch {
            session: "s1".to_owned(),
            batch_id: "b1".to_owned(),
            from: Some("from-x".to_owned()),
            tracks: ids
                .iter()
                .map(|id| WaveTrack {
                    track: track(id),
                    liked: false,
                    hue: None,
                    colors: None,
                    batch_id: "b1".to_owned(),
                })
                .collect(),
        }
    }

    fn meta() -> Meta {
        Meta {
            session: "s1".to_owned(),
            batch_id: "b1".to_owned(),
            from: None,
            liked: false,
            hue: None,
            colors: None,
        }
    }

    #[test]
    fn screen_follows_what_is_playing() {
        // Ничего не играет — стартовый вид.
        assert_eq!(screen_for(false, true, false), Screen::Start);
        // Волна — её экран, как бы ни было с остальным.
        assert_eq!(screen_for(true, true, true), Screen::Wave);
        assert_eq!(screen_for(true, false, true), Screen::Wave);
        // Чужая очередь — тот же экран с таблеткой.
        assert_eq!(screen_for(false, true, true), Screen::Foreign);
        // Волна запускается или не вышла: «Загружаем…» и ошибка, а не трек.
        assert_eq!(screen_for(false, false, true), Screen::Start);
    }

    #[test]
    fn source_label_names_the_list() {
        assert_eq!(source_text(&ListKey::Liked, None), "Мне нравится");
        assert_eq!(
            source_text(&ListKey::LikedFound("a".to_owned()), None),
            "Мне нравится"
        );
        assert_eq!(source_text(&ListKey::Detached, None), "Мне нравится");
        assert_eq!(
            source_text(&ListKey::Album(1), Some("Камнем по голове")),
            "Альбом «Камнем по голове»"
        );
        assert_eq!(source_text(&ListKey::Album(1), None), "Альбом");
        assert_eq!(
            source_text(&ListKey::Playlist(2), Some("Утро")),
            "Плейлист «Утро»"
        );
        assert_eq!(
            source_text(&ListKey::ArtistPopular(3), Some("Король и Шут")),
            "Исполнитель Король и Шут"
        );
        assert_eq!(source_text(&ListKey::ArtistTracks(3), None), "Исполнитель");
        assert_eq!(source_text(&ListKey::SearchTracks(4), None), "Поиск");
    }

    #[test]
    fn duplicates_are_dropped_within_the_session() {
        let mut seen = Seen::default();
        let first = take_fresh(&mut seen, &batch(&["1", "2", "2", "3"]));
        let ids: Vec<&str> = first.iter().map(|(track, _)| track.id.as_str()).collect();
        assert_eq!(ids, ["1", "2", "3"]);
        let second = take_fresh(&mut seen, &batch(&["3", "4", "1"]));
        let ids: Vec<&str> = second.iter().map(|(track, _)| track.id.as_str()).collect();
        assert_eq!(ids, ["4"]);
        assert!(take_fresh(&mut seen, &batch(&["1", "4"])).is_empty());
    }

    #[test]
    fn seen_remembers_only_the_last_ids() {
        let mut seen = Seen::default();
        for id in 0..SEEN_LIMIT + 10 {
            assert!(seen.insert(&id.to_string()));
        }
        assert_eq!(seen.set.len(), SEEN_LIMIT);
        // Самые старые забыты и снова считаются новыми, свежие — нет.
        assert!(seen.insert("0"));
        assert!(!seen.insert(&(SEEN_LIMIT + 9).to_string()));
    }

    #[test]
    fn top_up_starts_when_two_or_fewer_tracks_are_ahead() {
        // Очередь из 10 треков: впереди 9 - current.
        assert!(!needs_more(0, 10));
        assert!(!needs_more(6, 10));
        assert!(needs_more(7, 10));
        assert!(needs_more(9, 10));
        assert!(needs_more(0, 0));
        assert!(needs_more(0, 3));
        assert!(!needs_more(0, 4));
    }

    #[test]
    fn skip_needs_trackstarted_and_comes_once() {
        let mut tracker = Tracker::default();
        let played = Duration::from_secs(12);
        // Звук не пошёл (автопропуск ИИ): события нет.
        assert_eq!(tracker.leave(Leave::Skip, played), None);
        let started = tracker.started(&meta(), "1:7".to_owned()).unwrap();
        assert_eq!(
            started.event,
            WaveEvent::TrackStarted {
                track_id: "1:7".to_owned()
            }
        );
        let skip = tracker.leave(Leave::Skip, played).unwrap();
        assert_eq!(
            skip.event,
            WaveEvent::Skipped {
                track_id: "1:7".to_owned(),
                played
            }
        );
        assert_eq!(skip.batch_id, "b1");
        // Второе закрытие того же трека не уходит.
        assert_eq!(tracker.leave(Leave::Finish, played), None);
    }

    #[test]
    fn repeat_of_a_track_is_a_new_started_finished_pair() {
        let mut tracker = Tracker::default();
        let mut events = Vec::new();
        for _ in 0..2 {
            events.extend(tracker.started(&meta(), "1:7".to_owned()));
            events.extend(tracker.leave(Leave::Finish, Duration::from_secs(100)));
        }
        let kinds: Vec<&str> = events
            .iter()
            .map(|pending| match pending.event {
                WaveEvent::TrackStarted { .. } => "started",
                WaveEvent::TrackFinished { .. } => "finished",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, ["started", "finished", "started", "finished"]);
    }

    #[test]
    fn started_is_not_announced_twice() {
        let mut tracker = Tracker::default();
        assert!(tracker.started(&meta(), "1".to_owned()).is_some());
        assert!(tracker.started(&meta(), "1".to_owned()).is_none());
    }

    #[test]
    fn feedback_goes_one_at_a_time_in_order() {
        let mut feedback = Feedback::default();
        let event = |n: u32| Pending {
            session: "s".to_owned(),
            batch_id: "b".to_owned(),
            from: None,
            event: WaveEvent::TrackStarted {
                track_id: n.to_string(),
            },
        };
        for n in 0..3 {
            feedback.push(event(n));
        }
        assert_eq!(feedback.next(), Some(event(0)));
        // Пока ответа нет, следующее не уходит.
        assert_eq!(feedback.next(), None);
        feedback.done();
        assert_eq!(feedback.next(), Some(event(1)));
        feedback.done();
        assert_eq!(feedback.next(), Some(event(2)));
        feedback.done();
        assert_eq!(feedback.next(), None);
    }

    #[test]
    fn feedback_queue_is_bounded() {
        let mut feedback = Feedback::default();
        for n in 0..FEEDBACK_LIMIT + 5 {
            feedback.push(Pending {
                session: "s".to_owned(),
                batch_id: n.to_string(),
                from: None,
                event: WaveEvent::RadioStarted,
            });
        }
        assert_eq!(feedback.queue.len(), FEEDBACK_LIMIT);
        assert_eq!(feedback.next().map(|p| p.batch_id), Some("5".to_owned()));
    }

    #[test]
    fn tint_fades_in_300_ms_from_the_visible_color() {
        let (red, blue) = (
            Color::from_rgb(1.0, 0.0, 0.0),
            Color::from_rgb(0.0, 0.0, 1.0),
        );
        let start = Instant::now();
        let mut tint = Tint {
            from: red,
            to: red,
            since: start,
        };
        assert!(!tint.animating(start));
        tint.retarget(blue, start);
        assert!(tint.animating(start));
        assert_eq!(tint.at(start), red);
        let half = start + Duration::from_millis(150);
        let middle = tint.at(half);
        assert!(middle.r > 0.0 && middle.r < 1.0 && middle.b > 0.0 && middle.b < 1.0);
        let end = start + TINT_FADE;
        assert_eq!(tint.at(end), blue);
        assert!(!tint.animating(end));
        // Новая цель посреди перехода стартует от цвета, который виден.
        tint.retarget(red, half);
        assert_eq!(tint.at(half), middle);
        // Та же цель ничего не перезапускает.
        let since = tint.since;
        tint.retarget(red, end);
        assert_eq!(tint.since, since);
    }

    #[test]
    fn wave_repeat_is_only_off_or_one() {
        let mut state = false;
        let mut seen = Vec::new();
        for _ in 0..4 {
            state = toggled_repeat(state);
            seen.push(state);
        }
        assert_eq!(seen, [true, false, true, false]);
    }

    #[test]
    fn meta_follows_the_batch_of_each_track() {
        let mut seen = Seen::default();
        let mut wave = batch(&["1", "2"]);
        if let Some(second) = wave.tracks.get_mut(1) {
            second.batch_id = "b2".to_owned();
            second.liked = true;
        }
        let fresh = take_fresh(&mut seen, &wave);
        let batches: Vec<&str> = fresh
            .iter()
            .map(|(_, meta)| meta.batch_id.as_str())
            .collect();
        assert_eq!(batches, ["b1", "b2"]);
        assert!(fresh.get(1).is_some_and(|(_, meta)| meta.liked));
        assert!(fresh.iter().all(|(_, meta)| meta.session == "s1"));
    }
}
