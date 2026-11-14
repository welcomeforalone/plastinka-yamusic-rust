//! Системный плеер: медиаклавиши и оверлей. Windows (SMTC) и macOS — через
//! `souvlaki` (`system`), Linux — свой MPRIS на `zbus` (`mpris`): у MPRIS из
//! souvlaki позиция стоит на месте и нет сигнала `Seeked`, поэтому таймлайн в
//! апплетах KDE/GNOME и `playerctl` врал. Окно кладёт сюда снимок «что играет» ([`State`]), а
//! чистая логика ([`Syncer`]) решает, что сообщить системе: метаданные только
//! при смене трека, состояние — при смене или перемотке. События системы
//! ([`Command`]) приходят обычными сообщениями окна через [`Events`], без
//! своих перерисовок. В системный плеер уходят только метаданные (название,
//! исполнители, альбом, https-обложка Яндекса, длительность): звук не
//! передаётся и на диск ничего не пишется.
//!
//! Создать [`Controls`] можно только с [`PlaybackPermit`] (правило 2: без
//! подтверждённого Плюса системного плеера нет).

#[cfg(all(unix, not(target_os = "macos")))]
mod mpris;
#[cfg(not(all(unix, not(target_os = "macos"))))]
mod system;
#[cfg(any(test, all(unix, not(target_os = "macos"))))]
mod timeline;

use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

#[cfg(all(unix, not(target_os = "macos")))]
pub use mpris::Controls;
#[cfg(not(all(unix, not(target_os = "macos"))))]
pub use system::Controls;

/// Что просит система: клавиши, оверлей, MPRIS-клиенты.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    Play,
    Pause,
    Toggle,
    Next,
    Previous,
    Stop,
    /// Перемотка на позицию (секунды).
    SeekTo(f32),
    /// Перемотка на сдвиг: вперёд (`true`) или назад (секунды).
    SeekBy(bool, f32),
    /// Громкость от 0 до 1 (MPRIS `Volume`: ползунок апплета, колесо мыши
    /// над виджетом плеера, `playerctl volume`).
    SetVolume(f32),
}

/// Сдвиг «перемотать» без указания размера (кнопки FastForward/Rewind).
#[cfg(not(all(unix, not(target_os = "macos"))))]
const SEEK_STEP: Duration = Duration::from_secs(10);

#[cfg(not(all(unix, not(target_os = "macos"))))]
impl Command {
    /// Событие souvlaki → команда окна. Незнакомые и ненужные события
    /// (`Raise`, `Quit`, `OpenUri`) пропускаются: окно не
    /// закрывается и не открывает ссылки по просьбе системы.
    pub fn from_event(event: &souvlaki::MediaControlEvent) -> Option<Self> {
        use souvlaki::{MediaControlEvent as E, SeekDirection};
        let forward = |direction: &SeekDirection| matches!(direction, SeekDirection::Forward);
        Some(match event {
            E::Play => Self::Play,
            E::Pause => Self::Pause,
            E::Toggle => Self::Toggle,
            E::Next => Self::Next,
            E::Previous => Self::Previous,
            E::Stop => Self::Stop,
            E::SetPosition(position) => Self::SeekTo(position.0.as_secs_f32()),
            E::Seek(direction) => Self::SeekBy(forward(direction), SEEK_STEP.as_secs_f32()),
            E::SeekBy(direction, by) => Self::SeekBy(forward(direction), by.as_secs_f32()),
            // Не число — пропускаем; вне 0..1 (MPRIS разрешает больше 1) —
            // обрезаем до допустимого.
            E::SetVolume(volume) if volume.is_finite() => {
                Self::SetVolume(volume.clamp(0.0, 1.0) as f32)
            }
            _ => return None,
        })
    }
}

/// Поток команд для `iced::Subscription::run_with`. Сравнивается по номеру:
/// приёмник забирается один раз (как у `player::Events`).
#[derive(Clone)]
pub struct Events {
    id: u64,
    receiver: Arc<Mutex<Option<mpsc::UnboundedReceiver<Command>>>>,
}

impl Hash for Events {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl Events {
    /// Канал команд: отправитель — системе, `Events` — окну.
    fn channel() -> (mpsc::UnboundedSender<Command>, Self) {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let (sender, receiver) = mpsc::unbounded_channel();
        let events = Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            receiver: Arc::new(Mutex::new(Some(receiver))),
        };
        (sender, events)
    }

    /// Поток команд. Второй вызов даёт пустой поток.
    pub fn stream(&self) -> impl iced::futures::Stream<Item = Command> + use<> {
        let receiver = self
            .receiver
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        iced::futures::stream::unfold(receiver, |receiver| async move {
            let mut receiver = receiver?;
            let command = receiver.recv().await?;
            Some((command, Some(receiver)))
        })
    }
}

/// Трек для системного плеера.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Track {
    pub id: String,
    pub title: String,
    pub artists: String,
    pub album: String,
    /// Шаблон обложки Яндекса (`avatars.yandex.net/.../%%`).
    pub cover: Option<String>,
    pub duration_ms: Option<u64>,
}

/// Снимок плеера окна.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct State {
    /// `None` — ничего не открыто или трек остановлен.
    pub track: Option<Track>,
    pub playing: bool,
    pub position_ms: u64,
    /// Слышимая громкость в процентах (0, если звук выключен).
    pub volume_percent: u32,
}

/// Метаданные для системы. Поля всегда заполнены: на Windows `None` не
/// очищает прежнее значение.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meta {
    /// Номер трека: из него MPRIS строит `mpris:trackid`.
    pub track_id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// Готовый https-адрес обложки; `None` — обложки нет.
    pub cover_url: Option<String>,
    pub duration: Option<Duration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    Stopped,
    Paused,
    Playing,
}

/// Состояние воспроизведения для системы.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Playback {
    pub status: Status,
    pub position: Duration,
}

/// Что сообщить системе.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Metadata(Meta),
    Playback(Playback),
    /// Громкость в процентах. Её показывает только MPRIS (Linux): в SMTC
    /// громкости нет.
    Volume(u32),
}

/// На сколько позиция должна разойтись с ожидаемой, чтобы считать это
/// перемоткой и сообщить системе заново.
const JUMP: Duration = Duration::from_millis(1500);

/// Что уже сообщено системе.
struct Sent {
    track: Option<Track>,
    status: Status,
    position: Duration,
    at: Instant,
}

/// Решает, что сообщать системе. Чистая логика: время приходит параметром.
#[derive(Default)]
pub struct Syncer {
    sent: Option<Sent>,
    /// Сообщённая громкость, проценты.
    sent_volume: Option<u32>,
}

impl Syncer {
    /// Забыть всё сообщённое (системный плеер создан заново).
    pub fn reset(&mut self) {
        self.sent = None;
        self.sent_volume = None;
    }

    /// Что сообщить по новому снимку. Пустой список — ничего не изменилось.
    pub fn plan(&mut self, state: &State, now: Instant) -> Vec<Action> {
        let mut actions = Vec::new();
        let status = match (&state.track, state.playing) {
            (None, _) => Status::Stopped,
            (Some(_), true) => Status::Playing,
            (Some(_), false) => Status::Paused,
        };
        let position = Duration::from_millis(state.position_ms);
        let track_changed = self
            .sent
            .as_ref()
            .is_none_or(|sent| sent.track != state.track);
        if track_changed && let Some(track) = &state.track {
            actions.push(Action::Metadata(meta(track)));
        }
        let playback_changed = self.sent.as_ref().is_none_or(|sent| {
            if sent.status != status || track_changed {
                return true;
            }
            // Ожидаемая позиция: на ходу время течёт само.
            let elapsed = if status == Status::Playing {
                now.saturating_duration_since(sent.at)
            } else {
                Duration::ZERO
            };
            let expected = sent.position + elapsed;
            position.abs_diff(expected) > JUMP
        });
        if playback_changed {
            actions.push(Action::Playback(Playback { status, position }));
            self.sent = Some(Sent {
                track: state.track.clone(),
                status,
                position,
                at: now,
            });
        }
        if self.sent_volume != Some(state.volume_percent) {
            self.sent_volume = Some(state.volume_percent);
            actions.push(Action::Volume(state.volume_percent));
        }
        actions
    }
}

fn meta(track: &Track) -> Meta {
    Meta {
        track_id: track.id.clone(),
        title: track.title.clone(),
        artist: track.artists.clone(),
        album: track.album.clone(),
        cover_url: track.cover.as_deref().and_then(crate::discord::cover_url),
        // Нулевая длительность — «неизвестно»: с ней MPRIS отверг бы любую
        // перемотку (позиция больше длины трека).
        duration: track
            .duration_ms
            .filter(|ms| *ms > 0)
            .map(Duration::from_millis),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    use souvlaki::{MediaControlEvent as E, MediaPosition, SeekDirection};

    fn track(id: &str) -> Track {
        Track {
            id: id.to_owned(),
            title: format!("Название {id}"),
            artists: "Исполнитель".to_owned(),
            album: "Альбом".to_owned(),
            cover: Some("avatars.yandex.net/get-music-content/1/abc/%%".to_owned()),
            duration_ms: Some(200_000),
        }
    }

    fn state(id: &str, playing: bool, position_ms: u64) -> State {
        State {
            track: Some(track(id)),
            playing,
            position_ms,
            volume_percent: 70,
        }
    }

    #[test]
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    fn events_become_commands() {
        assert_eq!(Command::from_event(&E::Play), Some(Command::Play));
        assert_eq!(Command::from_event(&E::Pause), Some(Command::Pause));
        assert_eq!(Command::from_event(&E::Toggle), Some(Command::Toggle));
        assert_eq!(Command::from_event(&E::Next), Some(Command::Next));
        assert_eq!(Command::from_event(&E::Previous), Some(Command::Previous));
        assert_eq!(Command::from_event(&E::Stop), Some(Command::Stop));
        assert_eq!(
            Command::from_event(&E::SetPosition(MediaPosition(Duration::from_millis(1500)))),
            Some(Command::SeekTo(1.5))
        );
        assert_eq!(
            Command::from_event(&E::Seek(SeekDirection::Backward)),
            Some(Command::SeekBy(false, 10.0))
        );
        assert_eq!(
            Command::from_event(&E::SeekBy(SeekDirection::Forward, Duration::from_secs(5))),
            Some(Command::SeekBy(true, 5.0))
        );
        assert_eq!(
            Command::from_event(&E::SetVolume(0.5)),
            Some(Command::SetVolume(0.5))
        );
    }

    #[test]
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    fn volume_is_clamped_and_nan_dropped() {
        assert_eq!(
            Command::from_event(&E::SetVolume(1.7)),
            Some(Command::SetVolume(1.0))
        );
        assert_eq!(
            Command::from_event(&E::SetVolume(-0.2)),
            Some(Command::SetVolume(0.0))
        );
        assert_eq!(Command::from_event(&E::SetVolume(f64::NAN)), None);
    }

    #[test]
    fn volume_is_reported_on_change_only() {
        let mut syncer = Syncer::default();
        let start = Instant::now();
        syncer.plan(&state("1", true, 0), start);
        let mut louder = state("1", true, 0);
        louder.volume_percent = 85;
        assert_eq!(syncer.plan(&louder, start), vec![Action::Volume(85)]);
        assert!(syncer.plan(&louder, start).is_empty());
    }

    #[test]
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    fn unwanted_events_are_ignored() {
        assert_eq!(Command::from_event(&E::Quit), None);
        assert_eq!(Command::from_event(&E::Raise), None);
        assert_eq!(Command::from_event(&E::OpenUri("x".to_owned())), None);
    }

    #[test]
    fn new_track_sends_full_metadata_and_playback() {
        let mut syncer = Syncer::default();
        let now = Instant::now();
        let actions = syncer.plan(&state("1", true, 0), now);
        assert_eq!(actions.len(), 3);
        assert_eq!(actions.get(2), Some(&Action::Volume(70)));
        let Some(Action::Metadata(meta)) = actions.first() else {
            return;
        };
        assert_eq!(meta.title, "Название 1");
        assert_eq!(meta.artist, "Исполнитель");
        assert_eq!(meta.album, "Альбом");
        assert_eq!(
            meta.cover_url.as_deref(),
            Some("https://avatars.yandex.net/get-music-content/1/abc/400x400")
        );
        assert_eq!(meta.duration, Some(Duration::from_secs(200)));
        assert_eq!(
            actions.get(1),
            Some(&Action::Playback(Playback {
                status: Status::Playing,
                position: Duration::ZERO
            }))
        );
    }

    #[test]
    fn zero_duration_is_unknown() {
        let mut silent = track("1");
        silent.duration_ms = Some(0);
        assert_eq!(meta(&silent).duration, None);
        assert_eq!(meta(&track("7")).track_id, "7");
    }

    #[test]
    fn foreign_cover_host_is_dropped() {
        let mut other = track("1");
        other.cover = Some("evil.example/%%".to_owned());
        assert_eq!(meta(&other).cover_url, None);
    }

    #[test]
    fn steady_playback_sends_nothing() {
        let mut syncer = Syncer::default();
        let start = Instant::now();
        syncer.plan(&state("1", true, 0), start);
        // Прошла секунда, позиция выросла на секунду: всё идёт как шло.
        let later = start + Duration::from_secs(1);
        assert!(syncer.plan(&state("1", true, 1000), later).is_empty());
    }

    #[test]
    fn pause_and_resume_are_reported() {
        let mut syncer = Syncer::default();
        let start = Instant::now();
        syncer.plan(&state("1", true, 0), start);
        let paused = syncer.plan(&state("1", false, 3000), start + Duration::from_secs(3));
        assert_eq!(
            paused,
            vec![Action::Playback(Playback {
                status: Status::Paused,
                position: Duration::from_secs(3)
            })]
        );
        // На паузе позиция стоит: ничего нового.
        assert!(
            syncer
                .plan(&state("1", false, 3000), start + Duration::from_secs(9))
                .is_empty()
        );
        let resumed = syncer.plan(&state("1", true, 3000), start + Duration::from_secs(10));
        assert_eq!(resumed.len(), 1);
    }

    #[test]
    fn seek_is_reported_but_small_drift_is_not() {
        let mut syncer = Syncer::default();
        let start = Instant::now();
        syncer.plan(&state("1", true, 0), start);
        let drift = start + Duration::from_secs(10);
        assert!(syncer.plan(&state("1", true, 10_800), drift).is_empty());
        let seek = syncer.plan(&state("1", true, 120_000), drift);
        assert_eq!(
            seek,
            vec![Action::Playback(Playback {
                status: Status::Playing,
                position: Duration::from_secs(120)
            })]
        );
    }

    #[test]
    fn track_change_resends_both() {
        let mut syncer = Syncer::default();
        let start = Instant::now();
        syncer.plan(&state("1", true, 0), start);
        let actions = syncer.plan(&state("2", true, 0), start + Duration::from_secs(1));
        assert!(matches!(actions.first(), Some(Action::Metadata(_))));
        assert!(matches!(actions.get(1), Some(Action::Playback(_))));
    }

    #[test]
    fn stop_reports_stopped_once() {
        let mut syncer = Syncer::default();
        let start = Instant::now();
        syncer.plan(&state("1", true, 0), start);
        let idle = State {
            volume_percent: 70,
            ..State::default()
        };
        let stopped = syncer.plan(&idle, start + Duration::from_secs(1));
        assert_eq!(
            stopped,
            vec![Action::Playback(Playback {
                status: Status::Stopped,
                position: Duration::ZERO
            })]
        );
        assert!(
            syncer
                .plan(&idle, start + Duration::from_secs(2))
                .is_empty()
        );
    }

    #[test]
    fn reset_resends_everything() {
        let mut syncer = Syncer::default();
        let start = Instant::now();
        syncer.plan(&state("1", true, 0), start);
        syncer.reset();
        assert_eq!(syncer.plan(&state("1", true, 0), start).len(), 3);
    }
}
