//! Связь с системой через `souvlaki`: создание, приём событий, применение
//! действий. Логики решений здесь нет (она в `super`), только вызовы.
//!
//! Платформы: Windows — SMTC, нужен HWND окна; Linux — MPRIS по D-Bus
//! (бэкенд `use_zbus`, свой поток службы внутри souvlaki). `#[cfg]` разводит
//! ветки: HWND нужен только Windows, громкость есть только в MPRIS.

use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use souvlaki::{MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig};
use tokio::sync::mpsc;

use super::{Action, Command, Playback, State, Status, Syncer};
use crate::api::PlaybackPermit;

/// Имя в системном плеере (Linux: `identity`).
const DISPLAY_NAME: &str = "Plastinka";
/// Имя на шине D-Bus: `org.mpris.MediaPlayer2.plastinka` (Linux).
const DBUS_NAME: &str = "plastinka";

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

/// Системный плеер. При удалении отключается от системы (`Drop` souvlaki).
pub struct Controls {
    inner: MediaControls,
    syncer: Syncer,
    events: Events,
}

impl Controls {
    /// Создаёт системный плеер. Нужен пропуск подписки (правило 2). `hwnd` —
    /// окно в Windows (без него SMTC не работает); в Linux не используется.
    /// Любая ошибка возвращается текстом: приложение работает и без него.
    pub fn start(_permit: &PlaybackPermit, hwnd: Option<isize>) -> Result<Self, String> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let hwnd = platform_hwnd(hwnd)?;
        let mut inner = MediaControls::new(PlatformConfig {
            display_name: DISPLAY_NAME,
            dbus_name: DBUS_NAME,
            hwnd,
        })
        .map_err(|err| err.to_string())?;
        let (sender, receiver) = mpsc::unbounded_channel();
        // Обработчик зовут из чужого потока: только кладём команду в канал.
        inner
            .attach(move |event| {
                if let Some(command) = Command::from_event(&event) {
                    let _ = sender.send(command);
                }
            })
            .map_err(|err| err.to_string())?;
        Ok(Self {
            inner,
            syncer: Syncer::default(),
            events: Events {
                id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
                receiver: Arc::new(Mutex::new(Some(receiver))),
            },
        })
    }

    pub fn events(&self) -> Events {
        self.events.clone()
    }

    /// Сообщает системе, что изменилось. Ошибка — системный плеер дальше
    /// не нужен (окно его убирает и продолжает работать).
    pub fn sync(&mut self, state: &State) -> Result<(), String> {
        for action in self.syncer.plan(state, Instant::now()) {
            let result = match action {
                Action::Metadata(meta) => self.inner.set_metadata(MediaMetadata {
                    title: Some(&meta.title),
                    album: Some(&meta.album),
                    artist: Some(&meta.artist),
                    cover_url: meta.cover_url.as_deref(),
                    duration: meta.duration,
                }),
                Action::Playback(playback) => self.inner.set_playback(to_souvlaki(playback)),
                Action::Volume(percent) => set_volume(&mut self.inner, f64::from(percent) / 100.0),
            };
            result.map_err(|err| err.to_string())?;
        }
        Ok(())
    }
}

fn to_souvlaki(playback: Playback) -> MediaPlayback {
    let progress = Some(MediaPosition(playback.position));
    match playback.status {
        Status::Stopped => MediaPlayback::Stopped,
        Status::Paused => MediaPlayback::Paused { progress },
        Status::Playing => MediaPlayback::Playing { progress },
    }
}

/// Громкость в MPRIS: апплеты и `playerctl volume` видят текущее значение.
#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
fn set_volume(inner: &mut MediaControls, volume: f64) -> Result<(), souvlaki::Error> {
    inner.set_volume(volume)
}

/// В SMTC (Windows) и в macOS громкости в системном плеере нет.
#[cfg(not(all(unix, not(any(target_os = "macos", target_os = "ios")))))]
fn set_volume(_inner: &mut MediaControls, _volume: f64) -> Result<(), souvlaki::Error> {
    Ok(())
}

/// HWND для souvlaki. Windows: обязателен. Остальные: не нужен.
#[cfg(windows)]
fn platform_hwnd(hwnd: Option<isize>) -> Result<Option<*mut std::ffi::c_void>, String> {
    let hwnd = hwnd
        .filter(|hwnd| *hwnd != 0)
        .ok_or_else(|| "нет дескриптора окна".to_owned())?;
    // Число получено из `raw_window_handle`; обратно в указатель без `as`.
    Ok(Some(std::ptr::with_exposed_provenance_mut::<
        std::ffi::c_void,
    >(hwnd.cast_unsigned())))
}

#[cfg(not(windows))]
fn platform_hwnd(_hwnd: Option<isize>) -> Result<Option<*mut std::ffi::c_void>, String> {
    Ok(None)
}
