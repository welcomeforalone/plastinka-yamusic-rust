//! Связь с системой через `souvlaki`: создание, приём событий, применение
//! действий. Логики решений здесь нет (она в `super`), только вызовы.
//!
//! Платформы: Windows — SMTC, нужен HWND окна; macOS — Now Playing. Linux
//! сюда не попадает: там свой MPRIS (`super::mpris`). Громкости в системном
//! плеере обеих платформ нет.

use std::time::Instant;

use souvlaki::{MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig};

use super::{Action, Command, Events, Playback, State, Status, Syncer};
use crate::api::PlaybackPermit;

/// Имя в системном плеере.
const DISPLAY_NAME: &str = "Plastinka";
/// Имя на шине D-Bus: souvlaki требует его и вне Linux, но не использует.
const DBUS_NAME: &str = "plastinka";

/// Системный плеер. При удалении отключается от системы (`Drop` souvlaki).
pub struct Controls {
    inner: MediaControls,
    syncer: Syncer,
    events: Events,
}

impl Controls {
    /// Создаёт системный плеер. Нужен пропуск подписки (правило 2). `hwnd` —
    /// окно в Windows (без него SMTC не работает); в macOS не используется.
    /// Любая ошибка возвращается текстом: приложение работает и без него.
    pub fn start(_permit: &PlaybackPermit, hwnd: Option<isize>) -> Result<Self, String> {
        let hwnd = platform_hwnd(hwnd)?;
        let mut inner = MediaControls::new(PlatformConfig {
            display_name: DISPLAY_NAME,
            dbus_name: DBUS_NAME,
            hwnd,
        })
        .map_err(|err| err.to_string())?;
        let (sender, events) = Events::channel();
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
            events,
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
                // В SMTC и Now Playing громкости нет.
                Action::Volume(_) => Ok(()),
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
