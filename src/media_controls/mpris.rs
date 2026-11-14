//! Системный плеер Linux: своя служба MPRIS на `zbus` (апплеты KDE и GNOME,
//! `playerctl`, медиаклавиши через них).
//!
//! Почему не souvlaki: его MPRIS отдаёт в `Position` ту позицию, что была
//! сообщена последней, и не двигает её, пока трек играет; сигнала `Seeked`
//! нет совсем, а `mpris:trackid` у всех треков один (`/`). Клиенты видели
//! застывшую позицию, не замечали перемотку и путали треки. Здесь позиция
//! считается в момент чтения ([`Timeline`]), после перемотки, паузы,
//! продолжения и смены трека уходит `Seeked`, у каждого трека свой путь.
//!
//! Чтения свойств и вызовы методов обслуживает поток `zbus`; сигналы
//! отправляет свой поток, чтобы окно не ждало шину. Наружу уходят только
//! метаданные: звук не передаётся и на диск ничего не пишется.

use std::collections::HashMap;
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use zbus::blocking::Connection;
use zbus::blocking::connection::Builder;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, Value};

use super::timeline::{Timeline, track_path};
use super::{Action, Command, Events, Meta, State, Status, Syncer};
use crate::api::PlaybackPermit;

/// Имя в апплетах (`Identity`).
const DISPLAY_NAME: &str = "Plastinka";
/// Ярлык `plastinka.desktop` (`packaging/linux`): по нему апплет берёт значок.
const DESKTOP_ENTRY: &str = "plastinka";
const BUS_NAME: &str = "org.mpris.MediaPlayer2.plastinka";
const OBJECT_PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER_INTERFACE: &str = "org.mpris.MediaPlayer2.Player";
/// `mpris:trackid`, когда трека нет (так велит спецификация).
const NO_TRACK: &str = "/org/mpris/MediaPlayer2/TrackList/NoTrack";

/// Что видят клиенты. Общее для окна и потока `zbus`.
struct Now {
    meta: Option<Meta>,
    /// `mpris:trackid` текущего трека.
    track: OwnedObjectPath,
    timeline: Timeline,
    /// Громкость от 0 до 1.
    volume: f64,
}

impl Now {
    fn metadata(&self) -> HashMap<&'static str, Value<'static>> {
        let mut dict = HashMap::new();
        dict.insert(
            "mpris:trackid",
            Value::from(self.track.clone().into_inner()),
        );
        let Some(meta) = &self.meta else {
            return dict;
        };
        if let Some(length) = meta.duration {
            dict.insert("mpris:length", Value::from(micros(length)));
        }
        if let Some(url) = &meta.cover_url {
            dict.insert("mpris:artUrl", Value::from(url.clone()));
        }
        dict.insert("xesam:title", Value::from(meta.title.clone()));
        if !meta.artist.is_empty() {
            dict.insert("xesam:artist", Value::from(vec![meta.artist.clone()]));
        }
        if !meta.album.is_empty() {
            dict.insert("xesam:album", Value::from(meta.album.clone()));
        }
        dict
    }

    fn status(&self) -> &'static str {
        match self.timeline.status() {
            Status::Stopped => "Stopped",
            Status::Paused => "Paused",
            Status::Playing => "Playing",
        }
    }
}

fn lock(now: &Mutex<Now>) -> MutexGuard<'_, Now> {
    now.lock().unwrap_or_else(PoisonError::into_inner)
}

fn no_track() -> OwnedObjectPath {
    ObjectPath::from_static_str_unchecked(NO_TRACK).into()
}

/// Микросекунды MPRIS. Больше `i64` трек длиться не может, но без `as`.
fn micros(duration: Duration) -> i64 {
    i64::try_from(duration.as_micros()).unwrap_or(i64::MAX)
}

/// `org.mpris.MediaPlayer2`: окно не поднимается и не закрывается по
/// просьбе системы, поэтому `CanRaise` и `CanQuit` — нет.
struct Root;

#[zbus::interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    fn raise(&self) {}

    fn quit(&self) {}

    #[zbus(property)]
    fn can_quit(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn can_raise(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn identity(&self) -> &str {
        DISPLAY_NAME
    }

    #[zbus(property)]
    fn desktop_entry(&self) -> &str {
        DESKTOP_ENTRY
    }

    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        Vec::new()
    }

    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        Vec::new()
    }
}

/// `org.mpris.MediaPlayer2.Player`. Команды уходят окну через канал.
struct Player {
    now: Arc<Mutex<Now>>,
    commands: mpsc::UnboundedSender<Command>,
}

impl Player {
    fn send(&self, command: Command) {
        // Окно закрыто — команду некому выполнять.
        let _ = self.commands.send(command);
    }
}

#[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    fn next(&self) {
        self.send(Command::Next);
    }

    fn previous(&self) {
        self.send(Command::Previous);
    }

    fn pause(&self) {
        self.send(Command::Pause);
    }

    fn play_pause(&self) {
        self.send(Command::Toggle);
    }

    fn stop(&self) {
        self.send(Command::Stop);
    }

    fn play(&self) {
        self.send(Command::Play);
    }

    /// Сдвиг в микросекундах: больше нуля — вперёд.
    fn seek(&self, offset: i64) {
        if offset == 0 {
            return;
        }
        let by = Duration::from_micros(offset.unsigned_abs());
        self.send(Command::SeekBy(offset > 0, by.as_secs_f32()));
    }

    /// Перемотка на позицию (микросекунды). По спецификации запрос к
    /// другому треку (устаревший `trackid`) или за пределы трека
    /// пропускается.
    fn set_position(&self, track_id: ObjectPath<'_>, position: i64) {
        let Ok(position) = u64::try_from(position).map(Duration::from_micros) else {
            return;
        };
        {
            let now = lock(&self.now);
            if now.meta.is_none() || now.track.as_str() != track_id.as_str() {
                return;
            }
            if now
                .timeline
                .length()
                .is_some_and(|length| position > length)
            {
                return;
            }
        }
        self.send(Command::SeekTo(position.as_secs_f32()));
    }

    /// Ссылки по просьбе системы не открываем.
    fn open_uri(&self, _uri: String) {}

    #[zbus(property)]
    fn playback_status(&self) -> &'static str {
        lock(&self.now).status()
    }

    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn metadata(&self) -> HashMap<&'static str, Value<'static>> {
        lock(&self.now).metadata()
    }

    #[zbus(property)]
    fn volume(&self) -> f64 {
        lock(&self.now).volume
    }

    /// Ползунок апплета, колесо над виджетом, `playerctl volume`. Не число
    /// пропускаем, вне 0..1 (MPRIS разрешает больше 1) обрезаем.
    #[zbus(property)]
    fn set_volume(&mut self, volume: f64) {
        if !volume.is_finite() {
            return;
        }
        let volume = volume.clamp(0.0, 1.0);
        lock(&self.now).volume = volume;
        // Ровно 0..1: точности `f32` хватает с запасом.
        self.send(Command::SetVolume(volume as f32));
    }

    /// Считается в момент чтения. Сигнала об изменении у `Position` нет по
    /// спецификации: о скачках сообщает `Seeked`.
    #[zbus(property(emits_changed_signal = "false"))]
    fn position(&self) -> i64 {
        micros(lock(&self.now).timeline.position(Instant::now()))
    }

    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_play(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_pause(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_seek(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_control(&self) -> bool {
        true
    }
}

/// Сигнал для потока отправки.
enum Signal {
    /// `PropertiesChanged` интерфейса плеера: имя свойства → новое значение.
    Changed(HashMap<&'static str, Value<'static>>),
    /// `Seeked`: позиция в микросекундах.
    Seeked(i64),
}

fn emit(connection: &Connection, signal: Signal) -> zbus::Result<()> {
    match signal {
        Signal::Changed(changed) => connection.emit_signal(
            None::<()>,
            OBJECT_PATH,
            "org.freedesktop.DBus.Properties",
            "PropertiesChanged",
            &(PLAYER_INTERFACE, changed, Vec::<&str>::new()),
        ),
        Signal::Seeked(position) => connection.emit_signal(
            None::<()>,
            OBJECT_PATH,
            PLAYER_INTERFACE,
            "Seeked",
            &position,
        ),
    }
}

/// Системный плеер. При удалении имя на шине освобождается и апплеты его
/// убирают.
pub struct Controls {
    now: Arc<Mutex<Now>>,
    syncer: Syncer,
    events: Events,
    /// Закрытие канала останавливает поток отправки, а с ним уходит
    /// последнее соединение с шиной.
    signals: std_mpsc::Sender<Vec<Signal>>,
    /// Ошибка потока отправки: шина отвалилась.
    failure: Arc<Mutex<Option<String>>>,
    /// Держит имя на шине и объекты.
    _connection: Connection,
}

impl Controls {
    /// Создаёт службу MPRIS. Нужен пропуск подписки (правило 2). Окно (HWND)
    /// в Linux не нужно. Любая ошибка возвращается текстом: приложение
    /// работает и без системного плеера.
    pub fn start(_permit: &PlaybackPermit, _hwnd: Option<isize>) -> Result<Self, String> {
        let (commands, events) = Events::channel();
        let now = Arc::new(Mutex::new(Now {
            meta: None,
            track: no_track(),
            timeline: Timeline::new(Instant::now()),
            volume: 1.0,
        }));
        let player = Player {
            now: Arc::clone(&now),
            commands,
        };
        let connection = Builder::session()
            .and_then(|builder| builder.name(BUS_NAME))
            .and_then(|builder| builder.serve_at(OBJECT_PATH, Root))
            .and_then(|builder| builder.serve_at(OBJECT_PATH, player))
            .and_then(Builder::build)
            .map_err(|err| format!("D-Bus: {err}"))?;
        let (signals, inbox) = std_mpsc::channel::<Vec<Signal>>();
        let failure = Arc::new(Mutex::new(None));
        let sender = connection.clone();
        let failed = Arc::clone(&failure);
        std::thread::Builder::new()
            .name("plastinka-mpris".to_owned())
            .spawn(move || {
                // Канал закрыт — `Controls` удалён, поток выходит.
                for batch in inbox {
                    for signal in batch {
                        if let Err(err) = emit(&sender, signal) {
                            *failed.lock().unwrap_or_else(PoisonError::into_inner) =
                                Some(format!("D-Bus: {err}"));
                            return;
                        }
                    }
                }
            })
            .map_err(|err| err.to_string())?;
        Ok(Self {
            now,
            syncer: Syncer::default(),
            events,
            signals,
            failure,
            _connection: connection,
        })
    }

    pub fn events(&self) -> Events {
        self.events.clone()
    }

    /// Сообщает системе, что изменилось. Ошибка — системный плеер дальше
    /// не нужен (окно его убирает и продолжает работать).
    pub fn sync(&mut self, state: &State) -> Result<(), String> {
        if let Some(err) = self
            .failure
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            return Err(err);
        }
        let at = Instant::now();
        let actions = self.syncer.plan(state, at);
        if actions.is_empty() {
            return Ok(());
        }
        let mut changed = HashMap::new();
        let mut metadata_changed = false;
        let mut seeked = None;
        {
            // Всё меняется под одной блокировкой: клиент, который читает
            // свойства между сигналами, не увидит позицию прежнего трека с
            // метаданными нового.
            let mut now = lock(&self.now);
            for action in actions {
                match action {
                    Action::Metadata(meta) => {
                        now.track = OwnedObjectPath::try_from(track_path(&meta.track_id))
                            .unwrap_or_else(|_| no_track());
                        now.timeline.set_length(meta.duration);
                        now.meta = Some(meta);
                        metadata_changed = true;
                    }
                    Action::Playback(playback) => {
                        now.timeline.set(playback, at);
                        if playback.status == Status::Stopped {
                            now.meta = None;
                            now.track = no_track();
                            now.timeline.set_length(None);
                            metadata_changed = true;
                        } else {
                            // Старт, пауза, продолжение, перемотка, новый
                            // трек: позиция не та, что клиент насчитал сам.
                            seeked = Some(micros(playback.position));
                        }
                        changed.insert("PlaybackStatus", Value::from(now.status()));
                    }
                    Action::Volume(percent) => {
                        now.volume = f64::from(percent) / 100.0;
                        changed.insert("Volume", Value::from(now.volume));
                    }
                }
            }
            if metadata_changed {
                changed.insert("Metadata", Value::from(now.metadata()));
            }
        }
        let mut batch = Vec::new();
        if !changed.is_empty() {
            batch.push(Signal::Changed(changed));
        }
        if let Some(position) = seeked {
            batch.push(Signal::Seeked(position));
        }
        self.signals
            .send(batch)
            .map_err(|_| "поток системного плеера остановлен".to_owned())
    }
}
