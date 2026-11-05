//! Потоковое воспроизведение: загрузка трека в память, декодирование
//! (symphonia) и вывод звука (rodio). Аудио никогда не пишется на диск.
//! Плеер создаётся только с пропуском [`PlaybackPermit`], то есть после
//! успешной проверки подписки.
//!
//! Окно шлёт [`Command`] и получает [`Event`]. Внутри поток управления
//! (звуковое устройство и сеанс трека), задача загрузки на своём рантайме
//! tokio и поток декодера на каждый трек. Окно ничего не ждёт.

mod buffer;
mod decode;
mod download;
#[cfg(debug_assertions)]
mod fake;
mod output;

use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use rodio::{CpalError, CpalErrorKind, DeviceSinkBuilder, MixerDeviceSink};
use tokio::sync::mpsc as async_mpsc;

use crate::api::{self, PlaybackPermit};
use crate::net;
use crate::storage::token::StoredToken;
use buffer::{Failure, LinkFailure, StreamBuffer};
use output::{Shared, Volume};
pub use output::{TAP_LEN, Tap};

/// Как часто поток управления сообщает позицию.
const TICK: Duration = Duration::from_millis(200);

/// Команды окна.
#[derive(Debug, Clone)]
pub enum Command {
    /// Играть трек с позиции `start`. `id` окно выбирает само и узнаёт по
    /// нему свои события.
    Play {
        id: u64,
        track_id: String,
        start: Duration,
    },
    Pause,
    Resume,
    Seek(Duration),
    /// Громкость 0…1, уже с учётом «без звука».
    Volume(f32),
    Stop,
}

/// События для окна.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Позиция трека. `buffering` — звук ждёт сеть. `kbps` — битрейт MP3,
    /// когда ссылка уже получена. `seeks` — сколько перемоток этого трека
    /// плеер уже получил: по нему окно отбрасывает позицию до перемотки.
    Progress {
        id: u64,
        position: Duration,
        buffering: bool,
        kbps: Option<u32>,
        seeks: u64,
    },
    /// Трек доигран до конца.
    Ended {
        id: u64,
    },
    Failed {
        id: u64,
        error: PlayError,
    },
}

/// Почему трек не играет. Тексты — для окна.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlayError {
    #[error(
        "не удалось открыть звуковое устройство. Проверьте, что наушники или колонки подключены"
    )]
    DeviceOpen,
    #[error("звуковое устройство отключилось")]
    DeviceLost,
    #[error("трек недоступен для прослушивания")]
    Unavailable,
    #[error("сервис музыки не принял токен")]
    Unauthorized,
    #[error("{0}")]
    Offline(String),
    #[error("загрузка прервалась: {0}")]
    Network(String),
    #[error("не удалось получить ссылку на трек: {0}")]
    Link(String),
    #[error("{0}")]
    Storage(String),
    #[error("{0}")]
    Format(String),
    #[error("трек слишком большой для воспроизведения")]
    TooLarge,
    #[error("плеер не запустился: {0}")]
    Internal(String),
}

impl PlayError {
    /// Беда с самим треком: следующий, скорее всего, сыграет.
    pub fn track_specific(&self) -> bool {
        matches!(
            self,
            Self::Unavailable | Self::Storage(_) | Self::Format(_) | Self::TooLarge
        )
    }
}

impl From<Failure> for PlayError {
    fn from(failure: Failure) -> Self {
        match failure {
            Failure::Network(reason) => Self::Network(reason),
            Failure::Link(reason) => Self::Link(reason),
            Failure::Storage(reason) => Self::Storage(reason),
            Failure::TooLarge => Self::TooLarge,
            Failure::NoLink(failure) => failure.into(),
        }
    }
}

impl From<LinkFailure> for PlayError {
    fn from(failure: LinkFailure) -> Self {
        match failure {
            LinkFailure::Unavailable => Self::Unavailable,
            LinkFailure::Unauthorized => Self::Unauthorized,
            LinkFailure::Offline(reason) => Self::Offline(reason),
            LinkFailure::Other(reason) => Self::Link(reason),
        }
    }
}

/// Плеер окна. При удалении останавливает звук.
pub struct Player {
    commands: mpsc::Sender<Message>,
    events: Events,
    tap: Arc<Tap>,
}

enum Message {
    Command(Command),
    Shutdown,
}

/// Поток событий для `iced::Subscription::run_with`. Сравнивается по номеру
/// плеера: приёмник забирается один раз.
#[derive(Clone)]
pub struct Events {
    id: u64,
    receiver: Arc<Mutex<Option<async_mpsc::UnboundedReceiver<Event>>>>,
}

impl Hash for Events {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl Events {
    /// Поток событий. Второй вызов даёт пустой поток.
    pub fn stream(&self) -> impl iced::futures::Stream<Item = Event> + use<> {
        let receiver = self
            .receiver
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        iced::futures::stream::unfold(receiver, |receiver| async move {
            let mut receiver = receiver?;
            let event = receiver.recv().await?;
            Some((event, Some(receiver)))
        })
    }
}

impl Player {
    /// Запускает плеер. Звуковое устройство открывается при первом треке.
    /// `http` — клиент для запросов к API музыки (ссылки на треки).
    pub fn new(
        _permit: &PlaybackPermit,
        http: reqwest::Client,
        token: Arc<StoredToken>,
    ) -> Result<Self, PlayError> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let audio_http = net::audio_client().map_err(|err| PlayError::Internal(err.to_string()))?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("plastinka-net")
            .enable_all()
            .build()
            .map_err(|err| PlayError::Internal(err.to_string()))?;
        let (commands, inbox) = mpsc::channel();
        let (events_tx, events_rx) = async_mpsc::unbounded_channel();
        let tap = Arc::new(Tap::default());
        let control = Control {
            runtime,
            api_http: http,
            audio_http,
            token,
            events: events_tx,
            device: None,
            device_failed: Arc::new(AtomicBool::new(false)),
            volume: Arc::new(Volume::default()),
            tap: tap.clone(),
            session: None,
            last_progress: None,
        };
        std::thread::Builder::new()
            .name("plastinka-player".to_owned())
            .spawn(move || control.run(&inbox))
            .map_err(|err| PlayError::Internal(err.to_string()))?;
        Ok(Self {
            commands,
            events: Events {
                id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
                receiver: Arc::new(Mutex::new(Some(events_rx))),
            },
            tap,
        })
    }

    /// Отправляет команду. Поток плеера завершился — команда пропадает,
    /// окно узнает об этом по отсутствию событий.
    pub fn send(&self, command: Command) {
        let _ = self.commands.send(Message::Command(command));
    }

    pub fn events(&self) -> Events {
        self.events.clone()
    }

    /// Отвод звука для визуализатора (только память).
    pub fn tap(&self) -> Arc<Tap> {
        self.tap.clone()
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.commands.send(Message::Shutdown);
    }
}

/// Ошибка вывода, после которой поток звука не восстановится сам. Смену
/// устройства по умолчанию cpal переживает сам (`DeviceChanged`).
fn device_lost(err: &CpalError) -> bool {
    matches!(
        err.kind(),
        CpalErrorKind::DeviceNotAvailable
            | CpalErrorKind::HostUnavailable
            | CpalErrorKind::StreamInvalidated
    )
}

/// Трек, который играет сейчас.
struct Session {
    id: u64,
    seeks: u64,
    shared: Arc<Shared>,
    download: tokio::task::JoinHandle<()>,
}

impl Session {
    fn stop(self) {
        self.shared.stop();
        self.download.abort();
        // Поток декодера сам заметит остановку и выйдет.
    }
}

struct Control {
    runtime: tokio::runtime::Runtime,
    api_http: reqwest::Client,
    audio_http: reqwest::Client,
    token: Arc<StoredToken>,
    events: async_mpsc::UnboundedSender<Event>,
    device: Option<MixerDeviceSink>,
    device_failed: Arc<AtomicBool>,
    volume: Arc<Volume>,
    tap: Arc<Tap>,
    session: Option<Session>,
    last_progress: Option<Event>,
}

impl Control {
    fn run(mut self, inbox: &mpsc::Receiver<Message>) {
        loop {
            match inbox.recv_timeout(TICK) {
                Ok(Message::Command(command)) => self.handle(command),
                Ok(Message::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            self.poll();
        }
        if let Some(session) = self.session.take() {
            session.stop();
        }
        // Рантайм не ждёт задачи загрузки: они уже отменены.
        self.runtime.shutdown_background();
    }

    fn emit(&self, event: Event) {
        let _ = self.events.send(event);
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::Play {
                id,
                track_id,
                start,
            } => self.play(id, track_id, start),
            Command::Pause => self.with_session(|shared| shared.set_paused(true)),
            Command::Resume => self.with_session(|shared| shared.set_paused(false)),
            Command::Seek(to) => {
                if let Some(session) = &mut self.session {
                    session.seeks = session.seeks.wrapping_add(1);
                    session.shared.request_seek(to);
                }
            }
            Command::Volume(volume) => self.volume.set(volume),
            Command::Stop => {
                if let Some(session) = self.session.take() {
                    session.stop();
                }
            }
        }
    }

    fn with_session(&self, action: impl FnOnce(&Shared)) {
        if let Some(session) = &self.session {
            action(&session.shared);
        }
    }

    /// Устройство вывода: открывается при первом треке и заново, если
    /// прежнее отвалилось (например, отключили наушники).
    fn mixer(&mut self) -> Result<rodio::mixer::Mixer, PlayError> {
        if self.device_failed.swap(false, Ordering::SeqCst) {
            self.device = None;
        }
        if let Some(device) = &self.device {
            return Ok(device.mixer().clone());
        }
        let failed = self.device_failed.clone();
        let mut device = DeviceSinkBuilder::from_default_device()
            .and_then(|builder| {
                builder
                    .with_error_callback(move |err: CpalError| {
                        if device_lost(&err) {
                            failed.store(true, Ordering::SeqCst);
                        }
                    })
                    .open_sink_or_fallback()
            })
            .map_err(|_| PlayError::DeviceOpen)?;
        device.log_on_drop(false);
        let mixer = device.mixer().clone();
        self.device = Some(device);
        Ok(mixer)
    }

    fn play(&mut self, id: u64, track_id: String, start: Duration) {
        if let Some(session) = self.session.take() {
            session.stop();
        }
        self.last_progress = None;
        let mixer = match self.mixer() {
            Ok(mixer) => mixer,
            Err(error) => return self.emit(Event::Failed { id, error }),
        };
        let shared = Shared::new(
            StreamBuffer::new(),
            self.volume.clone(),
            self.tap.clone(),
            start,
        );

        let (http, token) = (self.api_http.clone(), self.token.clone());
        let relink: download::Relink = Box::new(move || {
            let (http, token, track_id) = (http.clone(), token.clone(), track_id.clone());
            Box::pin(async move {
                api::fetch_stream_source(&http, &token.access_token, &track_id).await
            })
        });
        let (buffer, audio_http, quality) = (
            shared.buffer.clone(),
            self.audio_http.clone(),
            shared.clone(),
        );
        let on_link = move |source: &api::StreamSource| quality.set_kbps(source.kbps());
        let download = self
            .runtime
            .spawn(download::run(buffer, audio_http, relink, on_link));

        let decoder_shared = shared.clone();
        let spawned = std::thread::Builder::new()
            .name("plastinka-decoder".to_owned())
            .spawn(move || decode::run(decoder_shared, mixer));
        let session = Session {
            id,
            seeks: 0,
            shared,
            download,
        };
        if let Err(err) = spawned {
            session.stop();
            return self.emit(Event::Failed {
                id,
                error: PlayError::Internal(err.to_string()),
            });
        }
        self.session = Some(session);
    }

    /// Позиция, конец трека, ошибки.
    fn poll(&mut self) {
        let Some(session) = &self.session else {
            return;
        };
        // Устройство пропало: звук не выводится, позиция встала. Трек
        // останавливаем, следующий откроет устройство заново.
        if self.device_failed.load(Ordering::SeqCst) {
            let id = session.id;
            if let Some(session) = self.session.take() {
                session.stop();
            }
            return self.emit(Event::Failed {
                id,
                error: PlayError::DeviceLost,
            });
        }
        let (id, seeks, shared) = (session.id, session.seeks, session.shared.clone());
        let result = if shared.finished() {
            Some(match (shared.buffer.failure(), shared.decode_error()) {
                (Some(failure), _) => Err(failure.into()),
                (None, Some(text)) => Err(PlayError::Format(text)),
                (None, None) => Ok(()),
            })
        } else {
            None
        };
        if let Some(result) = result {
            if let Some(session) = self.session.take() {
                session.stop();
            }
            return self.emit(match result {
                Ok(()) => Event::Ended { id },
                Err(error) => Event::Failed { id, error },
            });
        }
        let progress = Event::Progress {
            id,
            // Окну хватает десятых долей секунды.
            position: Duration::from_millis(
                u64::try_from(shared.position().as_millis()).unwrap_or(u64::MAX) / 100 * 100,
            ),
            buffering: shared.buffering(),
            kbps: shared.kbps(),
            seeks,
        };
        if self.last_progress.as_ref() != Some(&progress) {
            self.last_progress = Some(progress.clone());
            self.emit(progress);
        }
    }
}

// Поддельный поток есть только в отладочной сборке.
#[cfg(test)]
#[cfg(debug_assertions)]
mod tests {
    use std::num::NonZero;
    use std::time::Instant;

    use super::*;

    /// Когда перематывать в тесте.
    #[derive(Clone, Copy)]
    enum SeekAt {
        Never,
        /// Когда звук пошёл.
        Playing(Duration),
        /// Сразу после запуска декодера, пока он ждёт заголовок.
        Start(Duration),
    }

    /// Прогоняет поддельный поток через загрузку, декодер и микшер, забирая
    /// звук быстрее, чем он играет. Возвращает сеанс после конца.
    fn run_fake(name: &'static str, seek_at: SeekAt) -> Arc<Shared> {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let volume = Arc::new(Volume::default());
        volume.set(1.0);
        let shared = Shared::new(
            StreamBuffer::new(),
            volume,
            Arc::new(Tap::default()),
            Duration::ZERO,
        );
        let relink: download::Relink =
            Box::new(move || Box::pin(async move { Ok(api::StreamSource::Fake(name.to_owned())) }));
        let buffer = shared.buffer.clone();
        runtime.spawn(async move {
            download::run(buffer, reqwest::Client::new(), relink, |_| {}).await;
        });
        let (mixer, mut source) =
            rodio::mixer::mixer(NonZero::new(2).unwrap(), NonZero::new(44_100).unwrap());
        let decoder = shared.clone();
        std::thread::spawn(move || decode::run(decoder, mixer));
        if let SeekAt::Start(to) = seek_at {
            std::thread::sleep(Duration::from_millis(50));
            shared.request_seek(to);
        }
        let started = Instant::now();
        let mut seeked = false;
        while !shared.finished() {
            assert!(started.elapsed() < Duration::from_secs(60), "поток завис");
            for _ in 0..44_100 {
                let _ = source.next();
            }
            if let SeekAt::Playing(to) = seek_at
                && !seeked
                && shared.position() > Duration::ZERO
            {
                shared.request_seek(to);
                seeked = true;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        shared.stop();
        shared
    }

    #[test]
    fn silence_plays_to_the_end() {
        let shared = run_fake("silence", SeekAt::Never);
        assert_eq!(shared.buffer.failure(), None);
        assert_eq!(shared.decode_error(), None);
        assert!(
            shared.position() > Duration::from_secs(59),
            "{:?}",
            shared.position()
        );
    }

    #[test]
    fn seek_forward_skips_ahead() {
        let started = Instant::now();
        let shared = run_fake("silence", SeekAt::Playing(Duration::from_secs(50)));
        assert_eq!(shared.decode_error(), None);
        assert!(shared.position() > Duration::from_secs(59));
        // Отыграть 60 секунд в тесте заняло бы заметно дольше.
        assert!(started.elapsed() < Duration::from_secs(30));
    }

    /// На медленной сети весь файл качался бы 80 секунд. Перемотка на 50-ю
    /// секунду начинает новый запрос с её позиции.
    #[test]
    fn slow_seek_downloads_from_new_position() {
        let started = Instant::now();
        let shared = run_fake("slow", SeekAt::Playing(Duration::from_secs(50)));
        assert_eq!(shared.buffer.failure(), None);
        assert!(shared.position() > Duration::from_secs(59));
        assert!(
            started.elapsed() < Duration::from_secs(40),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn flaky_and_expired_recover() {
        for name in ["flaky", "expired"] {
            let shared = run_fake(name, SeekAt::Never);
            assert_eq!(shared.buffer.failure(), None, "{name}");
            assert!(shared.position() > Duration::from_secs(59), "{name}");
        }
    }

    /// Перемотка, пока декодер ещё разбирает заголовок, не ломает разбор:
    /// декодер выполнит её, когда начнёт.
    #[test]
    fn seek_before_header_is_applied_later() {
        let shared = run_fake("slow", SeekAt::Start(Duration::from_secs(55)));
        assert_eq!(shared.decode_error(), None);
        assert_eq!(shared.buffer.failure(), None);
        assert!(shared.position() > Duration::from_secs(59));
    }

    #[test]
    fn garbage_is_format_error() {
        let shared = run_fake("garbage", SeekAt::Never);
        assert!(shared.decode_error().is_some());
    }
}
