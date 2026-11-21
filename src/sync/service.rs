//! Служба «Слушать вместе»: приём и установка соединений, сопряжение,
//! готовая связь, восстановление после обрыва. Работает на своём рантайме
//! tokio (один рабочий поток), как `discord::Rpc` и плеер: окно отправляет
//! [`Command`] и забирает [`Events`] через `Subscription::run_with`.
//!
//! Окно живёт в своих часах (`std::time::Instant`) и никогда не видит
//! смещение часов собеседника: служба сама переводит моменты `at` туда и
//! обратно (у неё своя «эпоха» часов, от которой считаются миллисекунды
//! протокола).

mod link;

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::pin::pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use iced::futures::Stream;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::{Notify, oneshot};
use tokio::time::{sleep, timeout};

use super::SyncError;
use super::clock::{BURST_INTERVAL, LIVENESS_TIMEOUT, PING_INTERVAL};
use super::discovery::{Announce, Discovery, Visible};
use super::handshake::{
    Claim, ClaimOutcome, Ephemeral, Handshake, Host, Local, Mode, PairSecret, PeerInfo, Role,
    Timeouts, accept_with, initiate_with,
};
pub use super::proto::{Action, Repeat};
use super::proto::{ByeReason, State, Tick, WireTrack};
use super::store::{SecretStore, account_tag};
use crate::api::{PlaybackPermit, Track};
use crate::storage::config::PairedDevice;

/// Порт приёма по умолчанию (§2).
pub const DEFAULT_PORT: u16 = 47800;
/// Сколько ждём `bye shutdown` при закрытии службы.
const SHUTDOWN_WAIT: Duration = Duration::from_millis(400);

/// Настройки службы.
#[derive(Debug, Clone)]
pub struct ServiceConfig {
    pub id: String,
    pub name: String,
    /// Сопряжённые устройства из конфига (для загрузки их секретов).
    pub paired: Vec<PairedDevice>,
    /// Адрес приёма. В тестах — `127.0.0.1`.
    pub bind: IpAddr,
    /// Предпочитаемый порт; занят — любой свободный.
    pub port: u16,
    /// Объявлять себя и искать других по mDNS. В тестах выключено.
    pub mdns: bool,
    pub timeouts: Timeouts,
    pub connect_timeout: Duration,
    pub burst_interval: Duration,
    pub ping_interval: Duration,
    pub liveness: Duration,
    pub restore_interval: Duration,
    pub restore_window: Duration,
}

impl ServiceConfig {
    pub fn new(id: String, name: String, paired: Vec<PairedDevice>) -> Self {
        Self {
            id,
            name,
            paired,
            bind: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            port: DEFAULT_PORT,
            mdns: true,
            timeouts: Timeouts::default(),
            connect_timeout: Duration::from_secs(3),
            burst_interval: BURST_INTERVAL,
            ping_interval: PING_INTERVAL,
            liveness: LIVENESS_TIMEOUT,
            restore_interval: Duration::from_secs(5),
            restore_window: Duration::from_secs(600),
        }
    }
}

/// Состояние воспроизведения для окна (§4.3), в часах окна.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayState {
    pub rev: u64,
    pub by: String,
    pub owner: String,
    pub track: Option<Track>,
    pub playing: bool,
    /// Позиция в момент `at`.
    pub position: Duration,
    pub at: Instant,
    pub next: Option<Track>,
    pub shuffle: bool,
    pub repeat: Repeat,
    pub wave: bool,
    pub initial: bool,
    pub join: bool,
}

/// Позиция ведущего (`tick`), в часах окна.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayTick {
    pub rev: u64,
    pub position: Duration,
    pub at: Instant,
    pub buffering: bool,
}

/// То, что окно отправляет собеседнику по готовой связи.
#[derive(Debug, Clone, PartialEq)]
pub enum Outgoing {
    State(Box<PlayState>),
    Tick(PlayTick),
    Command(Action),
    Liked { track: String, liked: bool },
}

/// Команды окна службе.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Подключиться к устройству из списка видимых («Подключить»).
    Connect { id: String },
    /// Подключиться по адресу `ip:port`.
    ConnectAddr(SocketAddr),
    /// Решение пользователя о коде сопряжения: «Коды совпадают» — `true`.
    Decide(bool),
    /// «Отключить».
    Disconnect,
    /// «Забыть» устройство: удалить его секрет. Из конфига убирает окно.
    Forget { id: String },
    /// Сообщение по готовой связи.
    Send(Outgoing),
}

/// Устройство в списке видимых.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub os: String,
    pub paired: bool,
}

/// Готовая связь.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkInfo {
    pub peer: PeerInfo,
    pub role: Role,
    /// Пользователь сам нажал «Подключить» (в начальном `state` ставится
    /// `join: true`).
    pub join: bool,
    /// Это восстановление оборвавшейся связи.
    pub restored: bool,
}

/// События службы окну.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Видимые устройства (список целиком).
    Devices(Vec<DeviceInfo>),
    /// Свои адреса `ip:port` для подключения вручную.
    Addresses(Vec<String>),
    /// Нужно сравнить код и ответить [`Command::Decide`].
    Pairing {
        peer: PeerInfo,
        /// «123 456».
        code: String,
        /// Мы подключились сами.
        initiator: bool,
    },
    /// Новое сопряжение: окно сохраняет устройство в конфиг. `saved` —
    /// секрет лёг в хранилище паролей; иначе он живёт только до закрытия.
    Paired {
        peer: PeerInfo,
        saved: bool,
    },
    /// Связь готова (после первого `pong`).
    Linked(LinkInfo),
    State(Box<PlayState>),
    Tick(PlayTick),
    /// Ведомый просит ведущего выполнить действие.
    Command(Action),
    Liked {
        track: String,
        liked: bool,
    },
    /// Связь закончилась. `lost` — оборвалась сама (будет восстановление).
    Disconnected {
        peer: PeerInfo,
        text: String,
        lost: bool,
    },
    /// Подключение или сопряжение не удалось.
    Failed {
        text: String,
    },
    /// Проблема службы (порт, mDNS, хранилище паролей).
    Problem(String),
}

/// Поток событий для `iced::Subscription::run_with`. Приёмник забирается
/// один раз.
#[derive(Clone)]
pub struct Events {
    id: u64,
    receiver: Arc<Mutex<Option<UnboundedReceiver<Event>>>>,
}

impl std::hash::Hash for Events {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl Events {
    /// Поток событий. Второй вызов даёт пустой поток.
    pub fn stream(&self) -> impl Stream<Item = Event> + use<> {
        let receiver = lock(&self.receiver).take();
        iced::futures::stream::unfold(receiver, |receiver| async move {
            let mut receiver = receiver?;
            let event = receiver.recv().await?;
            Some((event, Some(receiver)))
        })
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

// ------------------------------------------------------------ отмена

/// Отмена с ожиданием: флаг и уведомление.
#[derive(Default)]
struct Cancel {
    flag: AtomicBool,
    notify: Notify,
}

impl Cancel {
    fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.notify.notify_one();
    }

    async fn wait(&self) {
        loop {
            if self.flag.load(Ordering::SeqCst) {
                return;
            }
            self.notify.notified().await;
        }
    }
}

// ------------------------------------------------------------ занятость

enum SessionCmd {
    Decide(bool),
    Send(Outgoing),
    Disconnect,
    Shutdown,
}

/// Всё, что нужно сессии от слота: приёмник команд окна и отмена.
struct SessionParts {
    token: u64,
    cmds: UnboundedReceiver<SessionCmd>,
    cancel: Arc<Cancel>,
}

struct Holder {
    token: u64,
    peer: Option<String>,
    outbound: bool,
    linked: bool,
    cancel: Arc<Cancel>,
    cmds: UnboundedSender<SessionCmd>,
}

#[derive(Default)]
struct Slot {
    token: u64,
    holder: Option<Holder>,
}

/// §6: при встречном подключении остаётся соединение, инициатор которого
/// с меньшим `id` (сравнение строк). Входящее инициировал `peer_id`, своё
/// исходящее — `local_id`.
pub fn incoming_wins_crossing(local_id: &str, peer_id: &str) -> bool {
    peer_id < local_id
}

// ------------------------------------------------------------ общее

/// Почему слот не отдан.
#[derive(Debug)]
enum ClaimDenied {
    /// Занят другой связью.
    Busy,
    /// Встречное подключение уступает нашему исходящему (§6).
    Crossing,
}

struct Shared {
    me: Local,
    cfg: ServiceConfig,
    epoch: Instant,
    store: SecretStore,
    secrets: Mutex<HashMap<String, PairSecret>>,
    events: UnboundedSender<Event>,
    slot: Mutex<Slot>,
    devices: Mutex<Vec<Visible>>,
    restore: Mutex<Option<Arc<Cancel>>>,
    discovery: Mutex<Option<Discovery>>,
    stopping: AtomicBool,
}

impl Shared {
    fn emit(&self, event: Event) {
        let _ = self.events.send(event);
    }

    /// Миллисекунды часов службы (от эпохи); можно быть и отрицательными.
    fn ms(&self, at: Instant) -> i64 {
        match at.checked_duration_since(self.epoch) {
            Some(after) => i64::try_from(after.as_millis()).unwrap_or(i64::MAX),
            None => -i64::try_from(self.epoch.duration_since(at).as_millis()).unwrap_or(i64::MAX),
        }
    }

    fn now_ms(&self) -> i64 {
        self.ms(Instant::now())
    }

    fn instant(&self, ms: i64) -> Instant {
        let shift = Duration::from_millis(ms.unsigned_abs());
        let moved = if ms >= 0 {
            self.epoch.checked_add(shift)
        } else {
            self.epoch.checked_sub(shift)
        };
        moved.unwrap_or(self.epoch)
    }

    /// Занять слот под сессию. `peer` — кто на том конце, если известно.
    /// Занято — `None`, кроме встречного подключения (§6): тогда по
    /// [`incoming_wins_crossing`] побеждает входящее (старая попытка
    /// отменяется, её отметка станет холостой) или остаётся своя.
    fn claim(
        self: &Arc<Self>,
        peer: Option<&str>,
        outbound: bool,
    ) -> Result<(Claim, SessionParts), ClaimDenied> {
        let mut slot = lock(&self.slot);
        if let Some(holder) = &slot.holder {
            let crossing = !outbound
                && holder.outbound
                && !holder.linked
                && peer.is_some()
                && holder.peer.as_deref() == peer;
            let incoming_wins = peer.is_some_and(|peer| incoming_wins_crossing(&self.me.id, peer));
            if !crossing {
                return Err(ClaimDenied::Busy);
            }
            if !incoming_wins {
                return Err(ClaimDenied::Crossing);
            }
            holder.cancel.cancel();
        }
        slot.token += 1;
        let token = slot.token;
        let cancel = Arc::new(Cancel::default());
        let (cmds_tx, cmds_rx) = mpsc::unbounded_channel();
        slot.holder = Some(Holder {
            token,
            peer: peer.map(str::to_owned),
            outbound,
            linked: false,
            cancel: cancel.clone(),
            cmds: cmds_tx,
        });
        let shared = self.clone();
        let claim = Claim::new(move || {
            let mut slot = lock(&shared.slot);
            if slot
                .holder
                .as_ref()
                .is_some_and(|holder| holder.token == token)
            {
                slot.holder = None;
            }
        });
        Ok((
            claim,
            SessionParts {
                token,
                cmds: cmds_rx,
                cancel,
            },
        ))
    }

    fn set_linked(&self, token: u64) {
        if let Some(holder) = lock(&self.slot)
            .holder
            .as_mut()
            .filter(|h| h.token == token)
        {
            holder.linked = true;
        }
    }

    fn is_busy(&self) -> bool {
        lock(&self.slot).holder.is_some()
    }

    fn to_session(&self, command: SessionCmd) {
        if let Some(holder) = &lock(&self.slot).holder {
            let _ = holder.cmds.send(command);
        }
    }

    fn cancel_restore(&self) {
        if let Some(cancel) = lock(&self.restore).take() {
            cancel.cancel();
        }
    }

    fn secret_of(&self, id: &str) -> Option<PairSecret> {
        lock(&self.secrets).get(id).cloned()
    }

    fn emit_devices(&self) {
        let paired = lock(&self.secrets);
        let list = lock(&self.devices)
            .iter()
            .map(|device| DeviceInfo {
                id: device.id.clone(),
                name: device.name.clone(),
                os: device.os.clone(),
                paired: paired.contains_key(&device.id),
            })
            .collect();
        drop(paired);
        self.emit(Event::Devices(list));
    }

    /// Секрет в память и, по возможности, в хранилище паролей. `true` —
    /// сохранился в хранилище.
    async fn remember_secret(self: &Arc<Self>, id: &str, secret: PairSecret) -> bool {
        lock(&self.secrets).insert(id.to_owned(), secret.clone());
        let (store, id_owned) = (self.store.clone(), id.to_owned());
        let result = tokio::task::spawn_blocking(move || store.save(&id_owned, &secret)).await;
        let saved = match result {
            Ok(Ok(())) => true,
            Ok(Err(err)) => {
                self.emit(Event::Problem(format!(
                    "Не удалось сохранить сопряжение: {err}. Оно действует до закрытия \
                     программы, потом код придётся подтвердить снова"
                )));
                false
            }
            Err(_) => false,
        };
        self.emit_devices();
        saved
    }

    async fn forget_secret(self: &Arc<Self>, id: &str) {
        lock(&self.secrets).remove(id);
        let (store, id_owned) = (self.store.clone(), id.to_owned());
        if let Ok(Err(err)) = tokio::task::spawn_blocking(move || store.delete(&id_owned)).await {
            self.emit(Event::Problem(format!(
                "Не удалось удалить секрет из хранилища паролей: {err}"
            )));
        }
        self.emit_devices();
    }
}

/// Секреты для рукопожатия.
struct SecretsHost {
    shared: Arc<Shared>,
}

impl Host for SecretsHost {
    fn pair_secret(&self, peer_id: &str) -> Option<PairSecret> {
        self.shared.secret_of(peer_id)
    }

    /// Исходящее рукопожатие слот занимает само, до подключения.
    fn try_claim(&self, _peer_id: &str) -> ClaimOutcome {
        ClaimOutcome::Busy
    }
}

/// Хост входящего соединения: занимает слот и откладывает его части.
struct InboundHost {
    shared: Arc<Shared>,
    parts: Mutex<Option<SessionParts>>,
}

impl Host for InboundHost {
    fn pair_secret(&self, peer_id: &str) -> Option<PairSecret> {
        self.shared.secret_of(peer_id)
    }

    fn try_claim(&self, peer_id: &str) -> ClaimOutcome {
        match self.shared.claim(Some(peer_id), false) {
            Ok((claim, parts)) => {
                *lock(&self.parts) = Some(parts);
                ClaimOutcome::Granted(claim)
            }
            Err(ClaimDenied::Busy) => ClaimOutcome::Busy,
            Err(ClaimDenied::Crossing) => ClaimOutcome::DropSilently,
        }
    }
}

// ------------------------------------------------------------ тексты

/// Текст отказа для пользователя (§6).
fn failure_text(err: &SyncError, name: &str) -> String {
    match err {
        SyncError::PeerBye(ByeReason::Busy) => format!("{name} уже связано с другим устройством"),
        SyncError::PeerBye(ByeReason::Version) => format!("Обновите Plastinka на {name}"),
        SyncError::PeerBye(ByeReason::Account) => format!("На {name} другой аккаунт Яндекса"),
        SyncError::PeerBye(ByeReason::Rejected) | SyncError::Declined(ByeReason::Rejected) => {
            "Сопряжение отменено".to_owned()
        }
        SyncError::PeerBye(ByeReason::Auth) | SyncError::AuthFailed => {
            format!("{name} не узнало это устройство. Подтвердите код заново")
        }
        SyncError::Timeout => format!("{name} не отвечает"),
        other => format!("Не удалось связаться с {name}: {other}"),
    }
}

/// Такой отказ не лечится повтором: восстановление связи прекращается.
fn is_fatal(err: &SyncError) -> bool {
    matches!(
        err,
        SyncError::AuthFailed
            | SyncError::CommitMismatch
            | SyncError::Declined(_)
            | SyncError::PeerBye(
                ByeReason::Auth | ByeReason::Rejected | ByeReason::Account | ByeReason::Version
            )
    )
}

// ------------------------------------------------------------ сессии

/// Откуда сессия и зачем.
#[derive(Clone, Copy)]
struct Origin {
    join: bool,
    restored: bool,
}

/// Итог сессии для вызывающего.
enum SessionEnd {
    /// Тихо: отменено, прервано выходом или отказом пользователя.
    Silent,
    /// Не дошло до связи.
    Failed { fatal: bool },
    /// Связь была и закончилась. `lost` — оборвалась сама.
    Link { lost: bool, peer: PeerInfo },
}

enum Verified {
    Done(Box<super::handshake::Established<TcpStream>>),
    Cancelled,
    Failed(SyncError),
}

/// Проверка после открытых кадров: узнавание или сопряжение.
async fn verify(
    shared: &Arc<Shared>,
    hs: Handshake<TcpStream>,
    parts: &mut SessionParts,
) -> Verified {
    let peer = hs.peer().clone();
    let (tx, rx) = oneshot::channel();
    let mut tx = Some(tx);
    if let Mode::Pair { code } = hs.mode() {
        shared.emit(Event::Pairing {
            peer: peer.clone(),
            code: code.to_string(),
            initiator: hs.role() == Role::Initiator,
        });
    }
    let mut finish = pin!(hs.finish(rx));
    loop {
        tokio::select! {
            result = &mut finish => {
                return match result {
                    Ok(est) => Verified::Done(Box::new(est)),
                    Err(err) => Verified::Failed(err),
                };
            }
            command = parts.cmds.recv() => match command {
                Some(SessionCmd::Decide(accept)) => {
                    if let Some(tx) = tx.take() {
                        let _ = tx.send(accept);
                    }
                }
                Some(SessionCmd::Send(_)) => {}
                Some(SessionCmd::Disconnect | SessionCmd::Shutdown) | None => {
                    return Verified::Cancelled;
                }
            },
            () = parts.cancel.wait() => return Verified::Cancelled,
        }
    }
}

/// Рукопожатие пройдено: проверка, сохранение секрета, связь.
async fn session(
    shared: &Arc<Shared>,
    hs: Handshake<TcpStream>,
    _claim: Claim,
    mut parts: SessionParts,
    origin: Origin,
) -> SessionEnd {
    let peer = hs.peer().clone();
    let role = hs.role();
    let est = match verify(shared, hs, &mut parts).await {
        Verified::Done(est) => *est,
        Verified::Cancelled => return SessionEnd::Silent,
        Verified::Failed(err) => {
            if !shared.stopping.load(Ordering::SeqCst) {
                shared.emit(Event::Failed {
                    text: failure_text(&err, &peer.name),
                });
            }
            return SessionEnd::Failed {
                fatal: is_fatal(&err),
            };
        }
    };
    if let Some(secret) = est.new_secret.clone() {
        let saved = shared.remember_secret(&peer.id, secret).await;
        shared.emit(Event::Paired {
            peer: peer.clone(),
            saved,
        });
    }
    shared.set_linked(parts.token);
    let info = LinkInfo {
        peer: peer.clone(),
        role,
        join: origin.join,
        restored: origin.restored,
    };
    let outcome = link::run(shared, est, &mut parts, info).await;
    let lost = matches!(outcome, link::LinkEnd::Lost);
    SessionEnd::Link { lost, peer }
}

/// Куда подключаемся.
#[derive(Clone)]
struct Target {
    peer_id: Option<String>,
    name: String,
    addrs: Vec<SocketAddr>,
}

/// Одна исходящая попытка: занять слот, подключиться, рукопожатие, сессия.
/// `report` — сообщать пользователю об отказах (восстановление молчит).
async fn connect_once(
    shared: &Arc<Shared>,
    target: &Target,
    join: bool,
    restored: bool,
    report: bool,
) -> SessionEnd {
    let fail = |text: String| {
        if report {
            shared.emit(Event::Failed { text });
        }
        SessionEnd::Failed { fatal: false }
    };
    let Ok((claim, parts)) = shared.claim(target.peer_id.as_deref(), true) else {
        return fail("Уже есть связь с другим устройством".to_owned());
    };
    let attempt = async {
        let mut last_error = None;
        for addr in &target.addrs {
            match timeout(shared.cfg.connect_timeout, TcpStream::connect(addr)).await {
                Ok(Ok(stream)) => {
                    let _ = stream.set_nodelay(true);
                    let host = SecretsHost {
                        shared: shared.clone(),
                    };
                    let result = match Ephemeral::generate() {
                        Ok(eph) => {
                            initiate_with(
                                stream,
                                &shared.me,
                                target.peer_id.as_deref(),
                                &host,
                                eph,
                                shared.cfg.timeouts,
                            )
                            .await
                        }
                        Err(err) => Err(err),
                    };
                    return result.map(|hs| (hs, *addr));
                }
                Ok(Err(err)) => last_error = Some(SyncError::Io(err)),
                Err(_) => last_error = Some(SyncError::Timeout),
            }
        }
        Err(last_error.unwrap_or(SyncError::Timeout))
    };
    let connected = tokio::select! {
        result = attempt => result,
        () = parts.cancel.wait() => return SessionEnd::Silent,
    };
    match connected {
        Ok((hs, _addr)) => {
            let origin = Origin { join, restored };
            session(shared, hs, claim, parts, origin).await
        }
        Err(err) => {
            let text = failure_text(&err, &target.name);
            let end = fail(text);
            if is_fatal(&err) {
                return SessionEnd::Failed { fatal: true };
            }
            end
        }
    }
}

/// Восстановление оборвавшейся связи (§5.3): 10 минут, раз в 5 с, без `join`,
/// пока собеседник виден в mDNS (или по последнему адресу).
async fn restore(shared: Arc<Shared>, peer: PeerInfo, last_addr: Option<SocketAddr>) {
    let cancel = Arc::new(Cancel::default());
    *lock(&shared.restore) = Some(cancel.clone());
    let mut deadline = Instant::now() + shared.cfg.restore_window;
    loop {
        tokio::select! {
            () = sleep(shared.cfg.restore_interval) => {}
            () = cancel.wait() => return,
        }
        if shared.stopping.load(Ordering::SeqCst) {
            return;
        }
        if Instant::now() >= deadline {
            shared.emit(Event::Failed {
                text: format!("Не удалось восстановить связь с {}", peer.name),
            });
            return;
        }
        if shared.is_busy() {
            continue;
        }
        let seen = lock(&shared.devices)
            .iter()
            .find(|device| device.id == peer.id)
            .map(|device| device.addrs.clone());
        let addrs = seen.unwrap_or_else(|| last_addr.into_iter().collect());
        if addrs.is_empty() {
            continue;
        }
        let target = Target {
            peer_id: Some(peer.id.clone()),
            name: peer.name.clone(),
            addrs,
        };
        match connect_once(&shared, &target, false, true, false).await {
            SessionEnd::Link { lost: true, .. } => {
                deadline = Instant::now() + shared.cfg.restore_window;
            }
            SessionEnd::Link { lost: false, .. } | SessionEnd::Failed { fatal: true } => return,
            SessionEnd::Silent | SessionEnd::Failed { fatal: false } => {}
        }
    }
}

async fn run_outbound(shared: Arc<Shared>, target: Target) {
    let end = connect_once(&shared, &target, true, false, true).await;
    if let SessionEnd::Link { lost: true, peer } = end {
        let last = target.addrs.first().copied();
        restore(shared, peer, last).await;
    }
}

async fn run_inbound(shared: Arc<Shared>, stream: TcpStream) {
    let _ = stream.set_nodelay(true);
    let host = InboundHost {
        shared: shared.clone(),
        parts: Mutex::new(None),
    };
    let Ok(eph) = Ephemeral::generate() else {
        return;
    };
    // Ошибки открытых кадров (сканеры портов, чужие программы) молчим.
    let Ok(mut hs) = accept_with(stream, &shared.me, &host, eph, shared.cfg.timeouts).await else {
        return;
    };
    let (Some(claim), Some(parts)) = (hs.take_claim(), lock(&host.parts).take()) else {
        return;
    };
    let origin = Origin {
        join: false,
        restored: false,
    };
    if let SessionEnd::Link { lost: true, peer } = session(&shared, hs, claim, parts, origin).await
    {
        restore(shared, peer, None).await;
    }
}

async fn listen(shared: Arc<Shared>, listener: TcpListener) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                tokio::spawn(run_inbound(shared.clone(), stream));
            }
            Err(_) => sleep(Duration::from_millis(200)).await,
        }
    }
}

/// Свои адреса `ip:port` для ручного подключения.
fn own_addresses(bind: IpAddr, port: u16) -> Vec<String> {
    if !bind.is_unspecified() {
        return vec![SocketAddr::new(bind, port).to_string()];
    }
    let mut addrs: Vec<Ipv4Addr> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|iface| !iface.is_loopback())
        .filter_map(|iface| match iface.ip() {
            IpAddr::V4(ip) if !ip.is_link_local() => Some(ip),
            _ => None,
        })
        .collect();
    addrs = super::discovery::sort_addrs(addrs);
    addrs
        .into_iter()
        .map(|ip| SocketAddr::from((ip, port)).to_string())
        .collect()
}

/// Начальная работа в рантайме: секреты, порт, mDNS, приём.
async fn init(shared: Arc<Shared>) {
    // Секреты ранее сопряжённых устройств.
    let ids: Vec<String> = shared
        .cfg
        .paired
        .iter()
        .map(|device| device.id.clone())
        .collect();
    let store = shared.store.clone();
    let loaded = tokio::task::spawn_blocking(move || {
        let mut found = Vec::new();
        let mut failure = None;
        for id in ids {
            match store.load(&id) {
                Ok(Some(secret)) => found.push((id, secret)),
                Ok(None) => {}
                Err(err) => failure = Some(err.to_string()),
            }
        }
        (found, failure)
    })
    .await;
    if let Ok((found, failure)) = loaded {
        lock(&shared.secrets).extend(found);
        if let Some(text) = failure {
            shared.emit(Event::Problem(format!(
                "Хранилище паролей недоступно: {text}. Сопряжённые устройства придётся \
                 подтвердить заново"
            )));
        }
    }

    // Порт: предпочитаемый, затем любой свободный.
    let bind = shared.cfg.bind;
    let preferred = shared.cfg.port;
    let listener = match TcpListener::bind((bind, preferred)).await {
        Ok(listener) => Some(listener),
        Err(_) => match TcpListener::bind((bind, 0)).await {
            Ok(listener) => {
                shared.emit(Event::Problem(format!(
                    "Порт {preferred} занят, подключения принимаются на другом порту"
                )));
                Some(listener)
            }
            Err(err) => {
                shared.emit(Event::Problem(format!(
                    "Не удалось открыть порт для подключений ({err}). Другие устройства не \
                     смогут подключиться к этому, но оно может подключаться само. Проверьте \
                     брандмауэр"
                )));
                None
            }
        },
    };
    let port = listener
        .as_ref()
        .and_then(|listener| listener.local_addr().ok())
        .map_or(0, |addr| addr.port());
    if let Some(listener) = listener {
        shared.emit(Event::Addresses(own_addresses(bind, port)));
        tokio::spawn(listen(shared.clone(), listener));
    }

    if shared.cfg.mdns && port != 0 {
        let announce = Announce {
            id: shared.me.id.clone(),
            name: shared.me.name.clone(),
            os: shared.me.os.clone(),
            acc: shared.me.acc.clone(),
            port,
        };
        let (tx, mut rx) = mpsc::unbounded_channel();
        let started = tokio::task::spawn_blocking(move || Discovery::start(&announce, tx)).await;
        match started {
            Ok(Ok(discovery)) => {
                *lock(&shared.discovery) = Some(discovery);
                let table = shared.clone();
                tokio::spawn(async move {
                    while let Some(list) = rx.recv().await {
                        *lock(&table.devices) = list;
                        table.emit_devices();
                    }
                });
            }
            Ok(Err(err)) => shared.emit(Event::Problem(format!("{err}. Подключайтесь по адресу"))),
            Err(_) => {}
        }
    }
}

// ------------------------------------------------------------ Service

/// Метка эпохи часов службы и счётчик номеров потоков событий.
static NEXT_EVENTS_ID: AtomicU64 = AtomicU64::new(1);

/// Служба «Слушать вместе». Создаётся только с пропуском [`PlaybackPermit`]
/// (правило 2): без проверки подписки её не запустить.
pub struct Service {
    shared: Arc<Shared>,
    events: Events,
    runtime: Option<tokio::runtime::Runtime>,
}

impl Service {
    /// Запускает службу. Отладочные хуки (только в отладочной сборке):
    /// `PLASTINKA_TEST_SYNC_ID` — идентификатор устройства,
    /// `PLASTINKA_TEST_SYNC_PORT` — порт приёма.
    pub fn new(
        permit: &PlaybackPermit,
        #[cfg_attr(not(debug_assertions), allow(unused_mut))] mut config: ServiceConfig,
        store: SecretStore,
    ) -> Result<Self, SyncError> {
        #[cfg(debug_assertions)]
        {
            if let Some(id) = crate::storage::token::env_hook("PLASTINKA_TEST_SYNC_ID")
                .filter(|id| super::proto::is_device_id(id))
            {
                config.id = id;
            }
            if let Some(port) = crate::storage::token::env_hook("PLASTINKA_TEST_SYNC_PORT")
                .and_then(|port| port.trim().parse::<u16>().ok())
            {
                config.port = port;
            }
        }
        Self::start(permit.uid(), config, store)
    }

    fn start(uid: u64, config: ServiceConfig, store: SecretStore) -> Result<Self, SyncError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("plastinka-sync")
            .enable_all()
            .build()?;
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let os = if cfg!(windows) {
            "windows"
        } else if cfg!(target_os = "macos") {
            "macos"
        } else {
            "linux"
        };
        let me = Local {
            id: config.id.clone(),
            name: config.name.clone(),
            os: os.to_owned(),
            acc: account_tag(uid),
        };
        let shared = Arc::new(Shared {
            me,
            cfg: config,
            epoch: Instant::now(),
            store,
            secrets: Mutex::new(HashMap::new()),
            events: events_tx,
            slot: Mutex::new(Slot::default()),
            devices: Mutex::new(Vec::new()),
            restore: Mutex::new(None),
            discovery: Mutex::new(None),
            stopping: AtomicBool::new(false),
        });
        runtime.spawn(init(shared.clone()));
        Ok(Self {
            shared,
            events: Events {
                id: NEXT_EVENTS_ID.fetch_add(1, Ordering::Relaxed),
                receiver: Arc::new(Mutex::new(Some(events_rx))),
            },
            runtime: Some(runtime),
        })
    }

    /// Идентификатор этого устройства (с учётом отладочного хука).
    pub fn device_id(&self) -> &str {
        &self.shared.me.id
    }

    /// Поток событий для подписки окна.
    pub fn events(&self) -> Events {
        self.events.clone()
    }

    /// Команда службе. Не блокирует.
    pub fn send(&self, command: Command) {
        let shared = &self.shared;
        match command {
            Command::Connect { id } => {
                shared.cancel_restore();
                let found = lock(&shared.devices)
                    .iter()
                    .find(|device| device.id == id)
                    .cloned();
                match found {
                    Some(device) => self.spawn(run_outbound(
                        shared.clone(),
                        Target {
                            peer_id: Some(device.id),
                            name: device.name,
                            addrs: device.addrs,
                        },
                    )),
                    None => shared.emit(Event::Failed {
                        text: "Устройство не найдено в сети. Подключитесь по адресу".to_owned(),
                    }),
                }
            }
            Command::ConnectAddr(addr) => {
                shared.cancel_restore();
                self.spawn(run_outbound(
                    shared.clone(),
                    Target {
                        peer_id: None,
                        name: addr.to_string(),
                        addrs: vec![addr],
                    },
                ));
            }
            Command::Decide(accept) => shared.to_session(SessionCmd::Decide(accept)),
            Command::Disconnect => {
                shared.cancel_restore();
                shared.to_session(SessionCmd::Disconnect);
            }
            Command::Forget { id } => {
                shared.cancel_restore();
                let shared = shared.clone();
                self.spawn(async move { shared.forget_secret(&id).await });
            }
            Command::Send(outgoing) => shared.to_session(SessionCmd::Send(outgoing)),
        }
    }

    fn spawn(&self, work: impl Future<Output = ()> + Send + 'static) {
        if let Some(runtime) = &self.runtime {
            runtime.spawn(work);
        }
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        let shared = &self.shared;
        shared.stopping.store(true, Ordering::SeqCst);
        shared.cancel_restore();
        shared.to_session(SessionCmd::Shutdown);
        // Ждём `bye shutdown`, но недолго: выход не должен зависать.
        let deadline = Instant::now() + SHUTDOWN_WAIT;
        while shared.is_busy() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(lock(&shared.discovery).take());
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

#[cfg(test)]
mod tests;
