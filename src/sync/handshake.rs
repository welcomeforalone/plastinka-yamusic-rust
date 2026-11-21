//! Рукопожатие (§3 спеки): три открытых кадра, ключи, затем узнавание по
//! секрету или сопряжение по коду.
//!
//! Порядок работы окна:
//! 1. [`initiate`] (мы подключились) или [`accept`] (к нам подключились)
//!    доходят до конца открытых кадров и возвращают [`Handshake`].
//! 2. По [`Handshake::mode`] окно узнаёт, что делать: при сопряжении
//!    показать код собеседника и имя, при узнавании ничего не показывать.
//! 3. [`Handshake::finish`] завершает проверку. В режиме сопряжения она ждёт
//!    решение пользователя из `oneshot` одновременно с `confirm`/`bye`
//!    собеседника; итог — [`Established`] с шифрованной связью и, при
//!    сопряжении, новым секретом.

use std::fmt;
use std::future::Future;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite, ReadHalf, WriteHalf, split};
use tokio::sync::oneshot;
use tokio::time::{Instant, sleep_until, timeout};

use super::crypto::{
    EphemeralKey, OpenKey, PUBLIC_KEY_LEN, SealKey, ct_eq, hkdf_expand, hkdf_extract, random_bytes,
    sha256,
};
use super::proto::{self, ByeReason, Message, is_device_id};
use super::wire::{FrameReader, MAX_FRAME, encode_frame, write_frame};
use super::{PROTOCOL_VERSION, SyncError};

/// Сколько ждём каждый кадр рукопожатия.
pub const FRAME_TIMEOUT: Duration = Duration::from_secs(10);
/// Сколько ждём решение о сопряжении.
pub const PAIRING_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_NAME_CHARS: usize = 64;

/// Сторона соединения.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Подключилась сама (`i`).
    Initiator,
    /// Приняла соединение (`r`).
    Responder,
}

impl Role {
    fn byte(self) -> u8 {
        match self {
            Self::Initiator => b'i',
            Self::Responder => b'r',
        }
    }

    fn other(self) -> Self {
        match self {
            Self::Initiator => Self::Responder,
            Self::Responder => Self::Initiator,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Initiator => "i",
            Self::Responder => "r",
        }
    }
}

/// Секрет сопряжения (32 байта). В `Debug` не показывается.
#[derive(Clone, PartialEq, Eq)]
pub struct PairSecret([u8; 32]);

impl PairSecret {
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for PairSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PairSecret(..)")
    }
}

/// Код сопряжения из шести цифр.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SasCode(u32);

impl SasCode {
    pub fn value(self) -> u32 {
        self.0
    }

    /// «123456».
    pub fn digits(self) -> String {
        format!("{:06}", self.0)
    }
}

impl fmt::Display for SasCode {
    /// «123 456».
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:03} {:03}", self.0 / 1000, self.0 % 1000)
    }
}

/// Что делает проверка после открытых кадров.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Оба знают секрет: проверка идёт сама.
    Recognize,
    /// Нужно сравнить код и подтвердить его.
    Pair { code: SasCode },
}

/// Данные этого устройства для `hello`.
#[derive(Debug, Clone)]
pub struct Local {
    /// 32 hex, нижний регистр.
    pub id: String,
    pub name: String,
    /// `windows`, `linux`, `macos` или `android`.
    pub os: String,
    /// Метка аккаунта (12 hex).
    pub acc: String,
}

/// Собеседник. Строки уже очищены и обрезаны.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerInfo {
    pub id: String,
    pub name: String,
    pub os: String,
}

/// То, что рукопожатию нужно от хранилища и состояния программы.
pub trait Host: Send + Sync {
    /// Сохранённый секрет сопряжения с устройством `peer_id`.
    fn pair_secret(&self, peer_id: &str) -> Option<PairSecret>;

    /// Ответчик спрашивает, можно ли принять связь с `peer_id`. `None` —
    /// занят (уже есть связь или идёт сопряжение): собеседник получит
    /// `bye busy`. `Granted` — отметка «занят»: она снимается, когда [`Claim`]
    /// выпадет из рук. Он живёт в [`Handshake`], затем в [`Established`], а
    /// при любой ошибке `accept` или `finish` уничтожается вместе с ними.
    /// Проверка и отметка должны быть одной атомарной операцией.
    fn try_claim(&self, peer_id: &str) -> ClaimOutcome;
}

/// Ответ хоста на вопрос «можно ли принять связь» (§6).
pub enum ClaimOutcome {
    /// Можно: отметка «занят».
    Granted(Claim),
    /// Занят: собеседник получит открытый `bye busy`.
    Busy,
    /// Встречное подключение, которое уступает своему исходящему: закрыть
    /// без `bye`, чтобы собеседник не увидел ошибку «занят».
    DropSilently,
}

/// Отметка «занят» с гарантированным снятием: при `Drop` выполняется
/// замыкание, которое её снимает.
pub struct Claim {
    release: Option<Box<dyn FnOnce() + Send>>,
}

impl Claim {
    pub fn new(release: impl FnOnce() + Send + 'static) -> Self {
        Self {
            release: Some(Box::new(release)),
        }
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            release();
        }
    }
}

impl fmt::Debug for Claim {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Claim")
    }
}

/// Таймауты рукопожатия.
#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    pub frame: Duration,
    pub pairing: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            frame: FRAME_TIMEOUT,
            pairing: PAIRING_TIMEOUT,
        }
    }
}

/// Эфемерный ключ и nonce стороны.
pub(crate) struct Ephemeral {
    pub key: EphemeralKey,
    pub nonce: [u8; 16],
}

impl Ephemeral {
    pub(crate) fn generate() -> Result<Self, SyncError> {
        Ok(Self {
            key: EphemeralKey::generate()?,
            nonce: random_bytes()?,
        })
    }
}

// ---------------------------------------------------------------- кадры

#[derive(Serialize, Deserialize)]
struct Hello {
    v: u32,
    role: String,
    id: String,
    name: String,
    os: String,
    acc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    nonce: Option<String>,
    known: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
enum Open {
    Hello(Hello),
    Reveal { key: String, nonce: String },
    Bye { reason: ByeReason },
}

fn to_json(open: &Open) -> Result<Vec<u8>, SyncError> {
    serde_json::to_vec(open).map_err(|_| SyncError::Protocol("сбой кодирования"))
}

fn hello_frame(
    local: &Local,
    role: Role,
    known: bool,
    commit: Option<&[u8; 32]>,
    key: Option<(&[u8; PUBLIC_KEY_LEN], &[u8; 16])>,
) -> Result<Vec<u8>, SyncError> {
    to_json(&Open::Hello(Hello {
        v: PROTOCOL_VERSION,
        role: role.name().to_owned(),
        id: local.id.clone(),
        name: local.name.clone(),
        os: local.os.clone(),
        acc: local.acc.clone(),
        commit: commit.map(|c| STANDARD.encode(c)),
        key: key.map(|(k, _)| STANDARD.encode(k)),
        nonce: key.map(|(_, n)| STANDARD.encode(n)),
        known,
    }))
}

/// Кадр 1: `hello` инициатора.
pub(crate) fn hello_initiator_frame(
    local: &Local,
    known: bool,
    commit: &[u8; 32],
) -> Result<Vec<u8>, SyncError> {
    hello_frame(local, Role::Initiator, known, Some(commit), None)
}

/// Кадр 2: `hello` ответчика.
pub(crate) fn hello_responder_frame(
    local: &Local,
    known: bool,
    key: &[u8; PUBLIC_KEY_LEN],
    nonce: &[u8; 16],
) -> Result<Vec<u8>, SyncError> {
    hello_frame(local, Role::Responder, known, None, Some((key, nonce)))
}

/// Кадр 3: `reveal`.
pub(crate) fn reveal_frame(
    key: &[u8; PUBLIC_KEY_LEN],
    nonce: &[u8; 16],
) -> Result<Vec<u8>, SyncError> {
    to_json(&Open::Reveal {
        key: STANDARD.encode(key),
        nonce: STANDARD.encode(nonce),
    })
}

pub(crate) fn commit_of(key: &[u8], nonce: &[u8]) -> [u8; 32] {
    sha256(&[key, nonce])
}

/// `TH`: SHA-256 трёх кадров вместе с префиксами длины.
pub(crate) fn transcript_hash(f1: &[u8], f2: &[u8], f3: &[u8]) -> Result<[u8; 32], SyncError> {
    let (a, b, c) = (encode_frame(f1)?, encode_frame(f2)?, encode_frame(f3)?);
    Ok(sha256(&[&a, &b, &c]))
}

/// Ключи, код и новый секрет (§3.2).
pub(crate) struct Keys {
    pub i2r: [u8; 32],
    pub r2i: [u8; 32],
    pub code: u32,
    pub pair: [u8; 32],
}

pub(crate) fn derive_keys(th: &[u8; 32], z: &[u8; 32]) -> Result<Keys, SyncError> {
    let prk = hkdf_extract(th, z)?;
    let sas: [u8; 4] = hkdf_expand(&prk, b"plastinka/1 sas")?;
    Ok(Keys {
        i2r: hkdf_expand(&prk, b"plastinka/1 i2r")?,
        r2i: hkdf_expand(&prk, b"plastinka/1 r2i")?,
        code: u32::from_be_bytes(sas) % 1_000_000,
        pair: hkdf_expand(&prk, b"plastinka/1 pair")?,
    })
}

/// `HMAC-SHA256(secret, "plastinka/1 auth " || role || TH)`, роль — отправителя.
pub(crate) fn auth_mac(
    secret: &[u8; 32],
    sender: Role,
    th: &[u8; 32],
) -> Result<[u8; 32], SyncError> {
    super::crypto::hmac_sha256(secret, &[b"plastinka/1 auth ", &[sender.byte()], th])
}

// ---------------------------------------------------- ввод-вывод с таймаутом

async fn timed<T>(
    limit: Duration,
    work: impl Future<Output = Result<T, SyncError>>,
) -> Result<T, SyncError> {
    timeout(limit, work).await.map_err(|_| SyncError::Timeout)?
}

/// Открытый `bye`: отправка по возможности, ошибки не важны.
async fn plain_bye<W: AsyncWrite + Unpin>(writer: &mut W, reason: ByeReason, limit: Duration) {
    if let Ok(bytes) = to_json(&Open::Bye { reason }) {
        let _ = timed(limit, write_frame(writer, &bytes)).await;
    }
}

fn decode_exact<const N: usize>(text: &str) -> Option<[u8; N]> {
    STANDARD.decode(text).ok()?.try_into().ok()
}

fn peer_info(hello: &Hello) -> PeerInfo {
    let name: String = crate::api::clean(&hello.name)
        .map(|name| name.chars().take(MAX_NAME_CHARS).collect())
        .unwrap_or_else(|| "Устройство".to_owned());
    let os_ok = (1..=16).contains(&hello.os.len())
        && hello
            .os
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    PeerInfo {
        id: hello.id.clone(),
        name,
        os: if os_ok {
            hello.os.clone()
        } else {
            "unknown".to_owned()
        },
    }
}

enum Check {
    Fine,
    Refuse(ByeReason),
    Drop(&'static str),
}

/// Проверка `hello` собеседника (§3.1).
fn check_hello(hello: &Hello, expected_role: Role, local: &Local) -> Check {
    if hello.v != PROTOCOL_VERSION || hello.role != expected_role.name() || !is_device_id(&hello.id)
    {
        return Check::Refuse(ByeReason::Version);
    }
    if hello.id == local.id {
        return Check::Drop("соединение с самим собой");
    }
    if hello.acc != local.acc {
        return Check::Refuse(ByeReason::Account);
    }
    Check::Fine
}

fn parse_open(frame: &[u8]) -> Result<Open, SyncError> {
    serde_json::from_slice(frame).map_err(|_| SyncError::Protocol("непонятное сообщение"))
}

// ----------------------------------------------------------- рукопожатие

/// Рукопожатие дошло до проверки: ключи есть, режим известен.
pub struct Handshake<S> {
    peer: PeerInfo,
    role: Role,
    mode: Mode,
    th: [u8; 32],
    keys: Keys,
    secret: Option<PairSecret>,
    reader: FrameReader<ReadHalf<S>>,
    writer: WriteHalf<S>,
    timeouts: Timeouts,
    claim: Option<Claim>,
}

struct Opened<S> {
    reader: FrameReader<ReadHalf<S>>,
    writer: WriteHalf<S>,
    role: Role,
    peer: PeerInfo,
    own_known: bool,
    peer_known: bool,
    secret: Option<PairSecret>,
    th: [u8; 32],
    z: [u8; 32],
    timeouts: Timeouts,
    claim: Option<Claim>,
}

impl<S: AsyncRead + AsyncWrite + Unpin> Handshake<S> {
    fn new(opened: Opened<S>) -> Result<Self, SyncError> {
        let keys = derive_keys(&opened.th, &opened.z)?;
        let recognize = opened.own_known && opened.peer_known && opened.secret.is_some();
        let mode = if recognize {
            Mode::Recognize
        } else {
            Mode::Pair {
                code: SasCode(keys.code),
            }
        };
        Ok(Self {
            peer: opened.peer,
            role: opened.role,
            mode,
            th: opened.th,
            keys,
            secret: opened.secret,
            reader: opened.reader,
            writer: opened.writer,
            timeouts: opened.timeouts,
            claim: opened.claim,
        })
    }

    /// Отметка «занят», взятая ответчиком в `try_claim`. Её надо держать,
    /// пока связь жива (или передать дальше).
    pub fn take_claim(&mut self) -> Option<Claim> {
        self.claim.take()
    }

    pub fn peer(&self) -> &PeerInfo {
        &self.peer
    }

    pub fn role(&self) -> Role {
        self.role
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Завершает проверку. При узнавании `decision` не нужен (его можно
    /// сразу отбросить). При сопряжении ждём из него решение пользователя
    /// («Коды совпадают» — `true`, отмена — `false`); отброшенный отправитель
    /// считается отменой. Через `pairing` секунд без итога — `bye rejected`.
    pub async fn finish(
        self,
        decision: oneshot::Receiver<bool>,
    ) -> Result<Established<S>, SyncError> {
        let (seal_key, open_key) = match self.role {
            Role::Initiator => (&self.keys.i2r, &self.keys.r2i),
            Role::Responder => (&self.keys.r2i, &self.keys.i2r),
        };
        let mut reader = SecureReader {
            frames: self.reader,
            open: OpenKey::new(open_key)?,
        };
        let mut writer = SecureWriter {
            writer: self.writer,
            seal: SealKey::new(seal_key)?,
        };
        let limit = self.timeouts.frame;
        let new_secret = match self.mode {
            Mode::Recognize => {
                let secret = self
                    .secret
                    .ok_or(SyncError::Protocol("нет секрета для узнавания"))?;
                recognize(
                    &mut reader,
                    &mut writer,
                    &secret,
                    self.role,
                    &self.th,
                    limit,
                )
                .await?;
                None
            }
            Mode::Pair { .. } => {
                confirm_pair(&mut reader, &mut writer, decision, self.timeouts).await?;
                Some(PairSecret(self.keys.pair))
            }
        };
        Ok(Established {
            peer: self.peer,
            role: self.role,
            new_secret,
            reader,
            writer,
            claim: self.claim,
        })
    }
}

/// Сколько дочитываем входящее после своего `bye`.
const LINGER: Duration = Duration::from_millis(300);

/// Шифрованный `bye`, затем «мягкое закрытие»: непрочитанное от собеседника
/// дочитываем до его закрытия. Иначе закрытый сокет с непрочитанными данными
/// шлёт сброс (RST), и собеседник теряет наш `bye` вместо понятной причины.
async fn encrypted_bye<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    writer: &mut SecureWriter<W>,
    reader: &mut SecureReader<R>,
    reason: ByeReason,
    limit: Duration,
) {
    let _ = timed(limit, writer.send(&Message::Bye { reason })).await;
    let _ = timeout(LINGER, async { while reader.recv().await.is_ok() {} }).await;
}

/// Узнавание: обмен `auth`.
async fn recognize<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    reader: &mut SecureReader<R>,
    writer: &mut SecureWriter<W>,
    secret: &PairSecret,
    role: Role,
    th: &[u8; 32],
    limit: Duration,
) -> Result<(), SyncError> {
    let mine = auth_mac(secret.as_bytes(), role, th)?;
    timed(
        limit,
        writer.send(&Message::Auth {
            mac: STANDARD.encode(mine),
        }),
    )
    .await?;
    match timed(limit, reader.recv()).await? {
        Some(Message::Auth { mac }) => {
            let expected = auth_mac(secret.as_bytes(), role.other(), th)?;
            let good = STANDARD.decode(mac).is_ok_and(|got| ct_eq(&got, &expected));
            if good {
                Ok(())
            } else {
                encrypted_bye(writer, reader, ByeReason::Auth, limit).await;
                Err(SyncError::AuthFailed)
            }
        }
        Some(Message::Bye {
            reason: ByeReason::Auth,
        }) => Err(SyncError::AuthFailed),
        Some(Message::Bye { reason }) => Err(SyncError::PeerBye(reason)),
        _ => Err(SyncError::Protocol("ожидалось подтверждение личности")),
    }
}

/// Сопряжение: ждём решение пользователя и `confirm` собеседника.
async fn confirm_pair<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    reader: &mut SecureReader<R>,
    writer: &mut SecureWriter<W>,
    mut decision: oneshot::Receiver<bool>,
    timeouts: Timeouts,
) -> Result<(), SyncError> {
    let limit = timeouts.frame;
    let deadline = sleep_until(Instant::now() + timeouts.pairing);
    tokio::pin!(deadline);
    let (mut decided, mut sent, mut got) = (false, false, false);
    while !(sent && got) {
        tokio::select! {
            answer = &mut decision, if !decided => {
                decided = true;
                if matches!(answer, Ok(true)) {
                    if let Err(error) = timed(limit, writer.send(&Message::Confirm)).await {
                        // Собеседник мог отказаться и закрыть связь: тогда его
                        // `bye` ещё лежит в буфере и объясняет обрыв лучше.
                        if let Ok(Some(Message::Bye { reason })) =
                            timed(limit, reader.recv()).await
                        {
                            return Err(SyncError::PeerBye(reason));
                        }
                        return Err(error);
                    }
                    sent = true;
                } else {
                    encrypted_bye(writer, reader, ByeReason::Rejected, limit).await;
                    return Err(SyncError::Declined(ByeReason::Rejected));
                }
            }
            frame = reader.recv() => match frame? {
                Some(Message::Confirm) => got = true,
                Some(Message::Bye { reason }) => return Err(SyncError::PeerBye(reason)),
                _ => return Err(SyncError::Protocol("ожидалось подтверждение кода")),
            },
            () = &mut deadline => {
                encrypted_bye(writer, reader, ByeReason::Rejected, limit).await;
                return Err(SyncError::Declined(ByeReason::Rejected));
            }
        }
    }
    Ok(())
}

/// Мы подключились: открытые кадры со стороны инициатора. `expected_peer` —
/// `id` из mDNS (по нему ищется секрет); при подключении по адресу `None`.
pub async fn initiate<S, H>(
    stream: S,
    local: &Local,
    expected_peer: Option<&str>,
    host: &H,
) -> Result<Handshake<S>, SyncError>
where
    S: AsyncRead + AsyncWrite + Unpin,
    H: Host + ?Sized,
{
    let eph = Ephemeral::generate()?;
    initiate_with(stream, local, expected_peer, host, eph, Timeouts::default()).await
}

pub(crate) async fn initiate_with<S, H>(
    stream: S,
    local: &Local,
    expected_peer: Option<&str>,
    host: &H,
    eph: Ephemeral,
    timeouts: Timeouts,
) -> Result<Handshake<S>, SyncError>
where
    S: AsyncRead + AsyncWrite + Unpin,
    H: Host + ?Sized,
{
    let limit = timeouts.frame;
    let (read_half, mut writer) = split(stream);
    let mut reader = FrameReader::new(read_half);
    let secret = expected_peer.and_then(|id| host.pair_secret(id));
    let own_known = secret.is_some();

    let commit = commit_of(eph.key.public(), &eph.nonce);
    let f1 = hello_initiator_frame(local, own_known, &commit)?;
    timed(limit, write_frame(&mut writer, &f1)).await?;

    let f2 = timed(limit, reader.read()).await?;
    let hello = match parse_open(&f2)? {
        Open::Hello(hello) => hello,
        Open::Bye { reason } => return Err(SyncError::PeerBye(reason)),
        Open::Reveal { .. } => return Err(SyncError::Protocol("ожидался hello")),
    };
    match check_hello(&hello, Role::Responder, local) {
        Check::Fine => {}
        Check::Refuse(reason) => {
            plain_bye(&mut writer, reason, limit).await;
            return Err(SyncError::Declined(reason));
        }
        Check::Drop(why) => return Err(SyncError::Protocol(why)),
    }
    if expected_peer.is_some_and(|id| id != hello.id) {
        return Err(SyncError::Protocol("по адресу ответило другое устройство"));
    }
    let key = hello
        .key
        .as_deref()
        .and_then(decode_exact::<PUBLIC_KEY_LEN>);
    let nonce = hello.nonce.as_deref().and_then(decode_exact::<16>);
    let (Some(peer_key), Some(_)) = (key, nonce) else {
        return Err(SyncError::Protocol("в hello нет ключа или nonce"));
    };

    let f3 = reveal_frame(eph.key.public(), &eph.nonce)?;
    timed(limit, write_frame(&mut writer, &f3)).await?;

    let z = eph.key.agree(&peer_key)?;
    let th = transcript_hash(&f1, &f2, &f3)?;
    Handshake::new(Opened {
        reader,
        writer,
        role: Role::Initiator,
        peer: peer_info(&hello),
        own_known,
        peer_known: hello.known,
        secret,
        th,
        z,
        timeouts,
        claim: None,
    })
}

/// К нам подключились: открытые кадры со стороны ответчика.
pub async fn accept<S, H>(stream: S, local: &Local, host: &H) -> Result<Handshake<S>, SyncError>
where
    S: AsyncRead + AsyncWrite + Unpin,
    H: Host + ?Sized,
{
    let eph = Ephemeral::generate()?;
    accept_with(stream, local, host, eph, Timeouts::default()).await
}

pub(crate) async fn accept_with<S, H>(
    stream: S,
    local: &Local,
    host: &H,
    eph: Ephemeral,
    timeouts: Timeouts,
) -> Result<Handshake<S>, SyncError>
where
    S: AsyncRead + AsyncWrite + Unpin,
    H: Host + ?Sized,
{
    let limit = timeouts.frame;
    let (read_half, mut writer) = split(stream);
    let mut reader = FrameReader::new(read_half);

    let f1 = timed(limit, reader.read()).await?;
    let hello = match parse_open(&f1)? {
        Open::Hello(hello) => hello,
        _ => return Err(SyncError::Protocol("ожидался hello")),
    };
    match check_hello(&hello, Role::Initiator, local) {
        Check::Fine => {}
        Check::Refuse(reason) => {
            plain_bye(&mut writer, reason, limit).await;
            return Err(SyncError::Declined(reason));
        }
        Check::Drop(why) => return Err(SyncError::Protocol(why)),
    }
    let Some(commit) = hello.commit.as_deref().and_then(decode_exact::<32>) else {
        return Err(SyncError::Protocol("в hello нет commit"));
    };
    let claim = match host.try_claim(&hello.id) {
        ClaimOutcome::Granted(claim) => claim,
        ClaimOutcome::Busy => {
            plain_bye(&mut writer, ByeReason::Busy, limit).await;
            return Err(SyncError::Declined(ByeReason::Busy));
        }
        ClaimOutcome::DropSilently => return Err(SyncError::Superseded),
    };
    let secret = host.pair_secret(&hello.id);
    let own_known = secret.is_some();

    let f2 = hello_responder_frame(local, own_known, eph.key.public(), &eph.nonce)?;
    timed(limit, write_frame(&mut writer, &f2)).await?;

    let f3 = timed(limit, reader.read()).await?;
    let (key, nonce) = match parse_open(&f3)? {
        Open::Reveal { key, nonce } => (
            decode_exact::<PUBLIC_KEY_LEN>(&key),
            decode_exact::<16>(&nonce),
        ),
        _ => return Err(SyncError::Protocol("ожидался reveal")),
    };
    let (Some(peer_key), Some(peer_nonce)) = (key, nonce) else {
        return Err(SyncError::Protocol("в reveal нет ключа или nonce"));
    };
    if !ct_eq(&commit_of(&peer_key, &peer_nonce), &commit) {
        return Err(SyncError::CommitMismatch);
    }

    let z = eph.key.agree(&peer_key)?;
    let th = transcript_hash(&f1, &f2, &f3)?;
    Handshake::new(Opened {
        reader,
        writer,
        role: Role::Responder,
        peer: peer_info(&hello),
        own_known,
        peer_known: hello.known,
        secret,
        th,
        z,
        timeouts,
        claim: Some(claim),
    })
}

// ------------------------------------------------------- готовая связь

/// Читатель шифрованных сообщений.
pub struct SecureReader<R> {
    frames: FrameReader<R>,
    open: OpenKey,
}

impl<R: AsyncRead + Unpin> SecureReader<R> {
    /// Следующий кадр. `None` — кадр пришёл, но сообщение пропущено
    /// (неизвестное или с неверными полями): связь жива. Отмена безопасна.
    pub async fn recv(&mut self) -> Result<Option<Message>, SyncError> {
        let frame = self.frames.read().await?;
        let plain = self.open.open(&frame)?;
        Ok(proto::parse(&plain))
    }
}

/// Писатель шифрованных сообщений.
pub struct SecureWriter<W> {
    writer: W,
    seal: SealKey,
}

impl<W: AsyncWrite + Unpin> SecureWriter<W> {
    /// Шифрует и отправляет сообщение. Не отменять посреди отправки: кадр
    /// уйдёт не целиком и связь придётся закрыть.
    pub async fn send(&mut self, message: &Message) -> Result<(), SyncError> {
        let json = proto::encode(message).ok_or(SyncError::Protocol("сбой кодирования"))?;
        if json.len().saturating_add(16) > MAX_FRAME {
            return Err(SyncError::FrameLength(json.len().saturating_add(16)));
        }
        let frame = self.seal.seal(&json)?;
        write_frame(&mut self.writer, &frame).await
    }
}

/// Готовая связь после проверки.
pub struct Established<S> {
    pub peer: PeerInfo,
    pub role: Role,
    /// Новый секрет при состоявшемся сопряжении: сохранить под `peer.id`.
    pub new_secret: Option<PairSecret>,
    pub reader: SecureReader<ReadHalf<S>>,
    pub writer: SecureWriter<WriteHalf<S>>,
    /// Отметка «занят» ответчика (если не забрана через
    /// [`Handshake::take_claim`] раньше): живёт, пока живёт связь.
    pub claim: Option<Claim>,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use tokio::io::{DuplexStream, duplex};

    use super::super::crypto::test_util::{hex, unhex};
    use super::super::proto::Tick;
    use super::*;

    const ID_I: &str = "11111111111111111111111111111111";
    const ID_R: &str = "22222222222222222222222222222222";
    const ACC: &str = "0123456789ab";
    const SHORT: Timeouts = Timeouts {
        frame: Duration::from_millis(300),
        pairing: Duration::from_millis(300),
    };

    fn local(id: &str, name: &str, os: &str) -> Local {
        Local {
            id: id.into(),
            name: name.into(),
            os: os.into(),
            acc: ACC.into(),
        }
    }

    struct TestHost {
        secret: Option<[u8; 32]>,
        busy: Arc<AtomicBool>,
        silent: AtomicBool,
    }

    impl TestHost {
        fn new(secret: Option<[u8; 32]>) -> Self {
            Self {
                secret,
                busy: Arc::new(AtomicBool::new(false)),
                silent: AtomicBool::new(false),
            }
        }
    }

    impl Host for TestHost {
        fn pair_secret(&self, _peer_id: &str) -> Option<PairSecret> {
            self.secret.map(PairSecret::new)
        }

        fn try_claim(&self, _peer_id: &str) -> ClaimOutcome {
            if self.silent.load(Ordering::SeqCst) {
                return ClaimOutcome::DropSilently;
            }
            if self.busy.swap(true, Ordering::SeqCst) {
                return ClaimOutcome::Busy;
            }
            let busy = self.busy.clone();
            ClaimOutcome::Granted(Claim::new(move || busy.store(false, Ordering::SeqCst)))
        }
    }

    type Outcome = Result<Established<DuplexStream>, SyncError>;

    /// Полное рукопожатие двух сторон. `decisions` — ответы пользователей.
    async fn run(
        host_i: &TestHost,
        host_r: &TestHost,
        decisions: (bool, bool),
    ) -> (Outcome, Outcome) {
        run_with(host_i, host_r, decisions, local(ID_R, "Телефон", "android")).await
    }

    async fn run_with(
        host_i: &TestHost,
        host_r: &TestHost,
        decisions: (bool, bool),
        local_r: Local,
    ) -> (Outcome, Outcome) {
        let (a, b) = duplex(1 << 20);
        let local_i = local(ID_I, "Компьютер", "windows");
        let side_i = async {
            let hs = initiate(a, &local_i, Some(ID_R), host_i).await?;
            let (tx, rx) = oneshot::channel();
            let _ = tx.send(decisions.0);
            hs.finish(rx).await
        };
        let side_r = async {
            let hs = accept(b, &local_r, host_r).await?;
            let (tx, rx) = oneshot::channel();
            let _ = tx.send(decisions.1);
            hs.finish(rx).await
        };
        tokio::join!(side_i, side_r)
    }

    #[test]
    fn futures_are_send() {
        fn assert_send<T: Send>(_: &T) {}
        let host = TestHost::new(None);
        let (a, _b) = duplex(64);
        let me = local(ID_I, "x", "linux");
        assert_send(&initiate(a, &me, None, &host));
    }

    #[tokio::test]
    async fn pairing_both_confirm() {
        let (host_i, host_r) = (TestHost::new(None), TestHost::new(None));
        let (i, r) = run(&host_i, &host_r, (true, true)).await;
        let (mut i, mut r) = (i.unwrap(), r.unwrap());
        let secret = i.new_secret.clone().unwrap();
        assert_eq!(Some(&secret), r.new_secret.as_ref());
        assert_eq!(i.peer.id, ID_R);
        assert_eq!(i.peer.name, "Телефон");
        assert_eq!(r.peer.os, "windows");

        // Обмен сообщениями в обе стороны.
        let tick = Message::Tick(Tick {
            rev: 1,
            position_ms: 2,
            at: 3,
            buffering: false,
        });
        i.writer.send(&tick).await.unwrap();
        r.writer.send(&Message::Ping { n: 1, t0: 7 }).await.unwrap();
        assert_eq!(r.reader.recv().await.unwrap(), Some(tick));
        assert_eq!(
            i.reader.recv().await.unwrap(),
            Some(Message::Ping { n: 1, t0: 7 })
        );
    }

    #[tokio::test]
    async fn codes_are_equal_and_pairing_asks_the_user() {
        let (a, b) = duplex(1 << 20);
        let (host_i, host_r) = (TestHost::new(None), TestHost::new(None));
        let (li, lr) = (local(ID_I, "A", "linux"), local(ID_R, "B", "macos"));
        let (hi, hr) = tokio::join!(initiate(a, &li, None, &host_i), accept(b, &lr, &host_r));
        let (hi, hr) = (hi.unwrap(), hr.unwrap());
        let (Mode::Pair { code: ci }, Mode::Pair { code: cr }) = (hi.mode(), hr.mode()) else {
            panic!("ожидалось сопряжение");
        };
        assert_eq!(ci, cr);
        assert_eq!(ci.digits().len(), 6);
        assert_eq!(ci.to_string().len(), 7);
    }

    #[tokio::test]
    async fn pairing_rejected_by_one_side() {
        let (host_i, host_r) = (TestHost::new(None), TestHost::new(None));
        let (i, r) = run(&host_i, &host_r, (false, true)).await;
        assert!(matches!(i, Err(SyncError::Declined(ByeReason::Rejected))));
        assert!(matches!(r, Err(SyncError::PeerBye(ByeReason::Rejected))));
    }

    #[tokio::test]
    async fn pairing_dropped_decision_is_rejection() {
        let (a, b) = duplex(1 << 20);
        let (host_i, host_r) = (TestHost::new(None), TestHost::new(None));
        let (li, lr) = (local(ID_I, "A", "linux"), local(ID_R, "B", "macos"));
        let (hi, hr) = tokio::join!(initiate(a, &li, None, &host_i), accept(b, &lr, &host_r));
        let (hi, hr) = (hi.unwrap(), hr.unwrap());
        let (_keep, rx_i) = oneshot::channel::<bool>();
        let (tx_r, rx_r) = oneshot::channel();
        drop(_keep);
        let _ = tx_r.send(true);
        let (i, r) = tokio::join!(hi.finish(rx_i), hr.finish(rx_r));
        assert!(matches!(i, Err(SyncError::Declined(ByeReason::Rejected))));
        assert!(matches!(r, Err(SyncError::PeerBye(ByeReason::Rejected))));
    }

    #[tokio::test]
    async fn pairing_times_out_without_decision() {
        let (a, b) = duplex(1 << 20);
        let (host_i, host_r) = (TestHost::new(None), TestHost::new(None));
        let (li, lr) = (local(ID_I, "A", "linux"), local(ID_R, "B", "macos"));
        let (hi, hr) = tokio::join!(
            initiate_with(a, &li, None, &host_i, Ephemeral::generate().unwrap(), SHORT),
            accept_with(b, &lr, &host_r, Ephemeral::generate().unwrap(), SHORT)
        );
        let (hi, hr) = (hi.unwrap(), hr.unwrap());
        // Отправители живы, но молчат.
        let (_tx_i, rx_i) = oneshot::channel::<bool>();
        let (_tx_r, rx_r) = oneshot::channel::<bool>();
        let (i, r) = tokio::join!(hi.finish(rx_i), hr.finish(rx_r));
        // Кто первым дождался срока, тот отказал сам, второй получил `bye`.
        for outcome in [i, r] {
            assert!(matches!(
                outcome,
                Err(SyncError::Declined(ByeReason::Rejected))
                    | Err(SyncError::PeerBye(ByeReason::Rejected))
            ));
        }
    }

    #[tokio::test]
    async fn recognition_with_same_secret() {
        let secret = Some([5u8; 32]);
        let (host_i, host_r) = (TestHost::new(secret), TestHost::new(secret));
        let (i, r) = run(&host_i, &host_r, (false, false)).await;
        let (i, r) = (i.unwrap(), r.unwrap());
        // Решения не понадобились: пользователь ничего не нажимал.
        assert!(i.new_secret.is_none() && r.new_secret.is_none());
    }

    #[tokio::test]
    async fn recognition_with_wrong_secret() {
        let host_i = TestHost::new(Some([5u8; 32]));
        let host_r = TestHost::new(Some([6u8; 32]));
        let (i, r) = run(&host_i, &host_r, (true, true)).await;
        assert!(matches!(i, Err(SyncError::AuthFailed)));
        assert!(matches!(r, Err(SyncError::AuthFailed)));
    }

    #[tokio::test]
    async fn one_side_unknown_means_pairing() {
        let host_i = TestHost::new(Some([5u8; 32]));
        let host_r = TestHost::new(None);
        let (i, r) = run(&host_i, &host_r, (true, true)).await;
        let (i, r) = (i.unwrap(), r.unwrap());
        // Секрет заменяется новым.
        assert!(i.new_secret.is_some());
        assert_eq!(i.new_secret, r.new_secret);
    }

    /// Скрипт вместо инициатора: сам шлёт сырые кадры.
    async fn scripted_initiator(
        hello: Vec<u8>,
        reveal_key: Option<EphemeralKey>,
        local_r: &Local,
        host_r: &TestHost,
    ) -> (Option<Vec<u8>>, Result<Handshake<DuplexStream>, SyncError>) {
        let (a, b) = duplex(1 << 20);
        let (ra, mut wa) = split(a);
        let mut reader = FrameReader::new(ra);
        let script = async {
            write_frame(&mut wa, &hello).await.unwrap();
            let reply = timeout(Duration::from_millis(500), reader.read()).await;
            if let (Some(key), Ok(Ok(_))) = (reveal_key, &reply) {
                let reveal = reveal_frame(key.public(), &[9; 16]).unwrap();
                write_frame(&mut wa, &reveal).await.unwrap();
            }
            reply.ok().and_then(Result::ok)
        };
        let (reply, result) = tokio::join!(
            script,
            accept_with(b, local_r, host_r, Ephemeral::generate().unwrap(), SHORT)
        );
        (reply, result)
    }

    fn raw_hello(v: u32, acc: &str, commit: &[u8; 32]) -> Vec<u8> {
        let mut me = local(ID_I, "Скрипт", "linux");
        me.acc = acc.into();
        let mut value: serde_json::Value =
            serde_json::from_slice(&hello_initiator_frame(&me, false, commit).unwrap()).unwrap();
        value["v"] = v.into();
        serde_json::to_vec(&value).unwrap()
    }

    #[tokio::test]
    async fn substituted_reveal_is_dropped() {
        let honest = EphemeralKey::from_scalar(&[0x11; 32]).unwrap();
        let other = EphemeralKey::from_scalar(&[0x33; 32]).unwrap();
        let commit = commit_of(honest.public(), &[9; 16]);
        let hello = raw_hello(1, ACC, &commit);
        let (host, lr) = (TestHost::new(None), local(ID_R, "B", "android"));
        // Раскрыт не тот ключ, который обещан.
        let (_, result) = scripted_initiator(hello.clone(), Some(other), &lr, &host).await;
        assert!(matches!(result, Err(SyncError::CommitMismatch)));
        // Тот же ключ и nonce — проходит.
        let host = TestHost::new(None);
        let (_, result) = scripted_initiator(hello, Some(honest), &lr, &host).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn tampered_responder_key_is_detected_by_keys() {
        // Подмена кадра 2: у сторон разный TH, а значит, разные ключи, и
        // шифрованный обмен не расшифруется.
        let (a, b) = duplex(1 << 20);
        let (host_i, host_r) = (TestHost::new(None), TestHost::new(None));
        let (li, lr) = (local(ID_I, "A", "linux"), local(ID_R, "B", "macos"));
        let (hi, hr) = tokio::join!(initiate(a, &li, None, &host_i), accept(b, &lr, &host_r));
        let (mut hi, hr) = (hi.unwrap(), hr.unwrap());
        hi.th[0] ^= 1;
        hi.keys = derive_keys(&hi.th, &[7; 32]).unwrap();
        let (tx_i, rx_i) = oneshot::channel();
        let (tx_r, rx_r) = oneshot::channel();
        let _ = (tx_i.send(true), tx_r.send(true));
        let (i, r) = tokio::join!(hi.finish(rx_i), hr.finish(rx_r));
        assert!(i.is_err() && r.is_err());
    }

    #[tokio::test]
    async fn foreign_account_gets_bye_account() {
        let host_i = TestHost::new(None);
        let host_r = TestHost::new(None);
        let mut lr = local(ID_R, "Телефон", "android");
        lr.acc = "ffffffffffff".into();
        let (i, r) = run_with(&host_i, &host_r, (true, true), lr).await;
        assert!(matches!(i, Err(SyncError::PeerBye(ByeReason::Account))));
        assert!(matches!(r, Err(SyncError::Declined(ByeReason::Account))));
    }

    #[tokio::test]
    async fn wrong_version_gets_bye_version() {
        let commit = [0u8; 32];
        let (host, lr) = (TestHost::new(None), local(ID_R, "B", "android"));
        let (reply, result) =
            scripted_initiator(raw_hello(2, ACC, &commit), None, &lr, &host).await;
        assert!(matches!(
            result,
            Err(SyncError::Declined(ByeReason::Version))
        ));
        let Some(Open::Bye { reason }) = reply.and_then(|f| parse_open(&f).ok()) else {
            panic!("ожидался открытый bye");
        };
        assert_eq!(reason, ByeReason::Version);
    }

    #[tokio::test]
    async fn crossing_loser_is_dropped_without_bye() {
        let host_i = TestHost::new(None);
        let host_r = TestHost::new(None);
        host_r.silent.store(true, Ordering::SeqCst);
        let (i, r) = run(&host_i, &host_r, (true, true)).await;
        // Инициатор видит обрыв, а не `bye busy`; ответчик ничего не шлёт.
        assert!(matches!(i, Err(SyncError::Io(_))), "{:?}", i.err());
        assert!(matches!(r, Err(SyncError::Superseded)));
        assert!(!host_r.busy.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn busy_responder_says_busy() {
        let host_i = TestHost::new(None);
        let host_r = TestHost::new(None);
        host_r.busy.store(true, Ordering::SeqCst);
        let (i, r) = run(&host_i, &host_r, (true, true)).await;
        assert!(matches!(i, Err(SyncError::PeerBye(ByeReason::Busy))));
        assert!(matches!(r, Err(SyncError::Declined(ByeReason::Busy))));
    }

    #[tokio::test]
    async fn claim_is_released_after_failures_and_with_the_link() {
        // Отказ после взятой отметки (подмена reveal) снимает её.
        let host = TestHost::new(None);
        let lr = local(ID_R, "B", "android");
        let honest = EphemeralKey::from_scalar(&[0x11; 32]).unwrap();
        let other = EphemeralKey::from_scalar(&[0x33; 32]).unwrap();
        let hello = raw_hello(1, ACC, &commit_of(honest.public(), &[9; 16]));
        let (_, result) = scripted_initiator(hello, Some(other), &lr, &host).await;
        assert!(matches!(result, Err(SyncError::CommitMismatch)));
        assert!(!host.busy.load(Ordering::SeqCst));

        // Отказ пользователя на этапе `finish` тоже снимает.
        let (host_i, host_r) = (TestHost::new(None), TestHost::new(None));
        let (i, r) = run(&host_i, &host_r, (false, true)).await;
        assert!(i.is_err() && r.is_err());
        assert!(!host_r.busy.load(Ordering::SeqCst));

        // Удачная связь держит отметку, пока жива.
        let (i, r) = run(&host_i, &host_r, (true, true)).await;
        let (i, r) = (i.unwrap(), r.unwrap());
        assert!(host_r.busy.load(Ordering::SeqCst));
        drop((i, r));
        assert!(!host_r.busy.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn silent_peer_times_out() {
        let (_a, b) = duplex(1024);
        let host = TestHost::new(None);
        let lr = local(ID_R, "B", "linux");
        let result = accept_with(b, &lr, &host, Ephemeral::generate().unwrap(), SHORT).await;
        assert!(matches!(result, Err(SyncError::Timeout)));
    }

    #[tokio::test]
    async fn connecting_to_self_is_refused() {
        let (a, b) = duplex(1 << 20);
        let host = TestHost::new(None);
        let me = local(ID_I, "Я", "linux");
        let (i, r) = tokio::join!(
            initiate_with(a, &me, None, &host, Ephemeral::generate().unwrap(), SHORT),
            accept_with(b, &me, &host, Ephemeral::generate().unwrap(), SHORT)
        );
        assert!(i.is_err() && r.is_err());
    }

    #[tokio::test]
    async fn tampered_encrypted_frame_breaks_link() {
        let (host_i, host_r) = (TestHost::new(None), TestHost::new(None));
        let (i, r) = run(&host_i, &host_r, (true, true)).await;
        let (mut i, mut r) = (i.unwrap(), r.unwrap());
        // Дважды один и тот же кадр ушёл бы с разным счётчиком; здесь портим
        // кадр по пути: пишем мимо шифра прямо в поток.
        write_frame(&mut i.writer.writer, &[0u8; 40]).await.unwrap();
        assert!(matches!(r.reader.recv().await, Err(SyncError::Decrypt)));
    }

    // ------------------------------------------------- тестовые векторы §8

    /// Все значения §8 спеки. Меняются только вместе со спекой.
    struct Vectors {
        f1: Vec<u8>,
        f2: Vec<u8>,
        f3: Vec<u8>,
        th: [u8; 32],
        z: [u8; 32],
        keys: Keys,
        mac_i: [u8; 32],
        mac_r: [u8; 32],
        enc_i2r: Vec<u8>,
        enc_r2i: Vec<u8>,
    }

    const PAIR_SECRET: [u8; 32] = [0x5a; 32];
    const JSON_I2R: &str = r#"{"t":"ping","n":1,"t0":1000}"#;
    const JSON_R2I: &str = r#"{"t":"pong","n":1,"t0":1000,"t1":2000}"#;

    fn vectors() -> Vectors {
        let (ki, kr) = (
            EphemeralKey::from_scalar(&[0x11; 32]).unwrap(),
            EphemeralKey::from_scalar(&[0x22; 32]).unwrap(),
        );
        let (ni, nr) = ([0xa1u8; 16], [0xb2u8; 16]);
        let li = local(ID_I, "Plastinka PC", "windows");
        let lr = local(ID_R, "Телефон", "android");
        let commit = commit_of(ki.public(), &ni);
        let f1 = hello_initiator_frame(&li, true, &commit).unwrap();
        let f2 = hello_responder_frame(&lr, true, kr.public(), &nr).unwrap();
        let f3 = reveal_frame(ki.public(), &ni).unwrap();
        let th = transcript_hash(&f1, &f2, &f3).unwrap();
        let z = ki.agree(kr.public()).unwrap();
        assert_eq!(z, kr.agree(ki.public()).unwrap());
        let keys = derive_keys(&th, &z).unwrap();
        let mac_i = auth_mac(&PAIR_SECRET, Role::Initiator, &th).unwrap();
        let mac_r = auth_mac(&PAIR_SECRET, Role::Responder, &th).unwrap();
        let enc_i2r = SealKey::new(&keys.i2r)
            .unwrap()
            .seal(JSON_I2R.as_bytes())
            .unwrap();
        let enc_r2i = SealKey::new(&keys.r2i)
            .unwrap()
            .seal(JSON_R2I.as_bytes())
            .unwrap();
        Vectors {
            f1,
            f2,
            f3,
            th,
            z,
            keys,
            mac_i,
            mac_r,
            enc_i2r,
            enc_r2i,
        }
    }

    fn text(bytes: &[u8]) -> String {
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[test]
    fn spec_vectors() {
        let v = vectors();
        assert_eq!(
            hex(EphemeralKey::from_scalar(&[0x11; 32]).unwrap().public()),
            "040217e617f0b6443928278f96999e69a23a4f2c152bdf6d6cdf66e5b80282d4ed194a7debcb97712d2dda3ca85aa8765a56f45fc758599652f2897c65306e5794"
        );
        assert_eq!(
            hex(EphemeralKey::from_scalar(&[0x22; 32]).unwrap().public()),
            "04d65a93977caa3d1b081852ff57a79e465f1660577304baead505dd3a48589cf350185e895372df6221ea3a137557e473fddb6755f05bd507c3c533fce9c91285"
        );
        assert_eq!(
            text(&v.f1),
            r#"{"t":"hello","v":1,"role":"i","id":"11111111111111111111111111111111","name":"Plastinka PC","os":"windows","acc":"0123456789ab","commit":"LaBgKnhsnKQOwMVrdnPYGyZBs+nloU8Mq2y+TZ6VUPc=","known":true}"#
        );
        assert_eq!(
            text(&v.f2),
            r#"{"t":"hello","v":1,"role":"r","id":"22222222222222222222222222222222","name":"Телефон","os":"android","acc":"0123456789ab","key":"BNZak5d8qj0bCBhS/1ennkZfFmBXcwS66tUF3TpIWJzzUBheiVNy32Ih6joTdVfkc/3bZ1XwW9UHw8Uz/OnJEoU=","nonce":"srKysrKysrKysrKysrKysg==","known":true}"#
        );
        assert_eq!(
            text(&v.f3),
            r#"{"t":"reveal","key":"BAIX5hfwtkQ5KCePlpmeaaI6TywVK99tbN9m5bgCgtTtGUp968uXcS0t2jyoWqh2Wlb0X8dYWZZS8ol8ZTBuV5Q=","nonce":"oaGhoaGhoaGhoaGhoaGhoQ=="}"#
        );
        let checks = [
            (
                &v.th[..],
                "b0975fc0f9e6817e94caf067f7845881c06c61c5dc1e03709860fc3c62c53495",
            ),
            (
                &v.z[..],
                "ccfc261f58193c98ca4ad4a53bbac6f0ee29bc4d48438090446908622ca79af6",
            ),
            (
                &v.keys.i2r[..],
                "92191036d962bbf4011354b33ff1a524d31f17c511f14c95dca01e9426094672",
            ),
            (
                &v.keys.r2i[..],
                "b78c07d294610f55e93f6cdc4c3d1629a606ed2febaffe37e8824bd45b2d1174",
            ),
            (
                &v.keys.pair[..],
                "c31c8ee574529acf74388b1e6a3aca29dd5fa7bb99eaf0469b8e919482c0206d",
            ),
            (
                &v.mac_i[..],
                "54b26c7acca254b46b2f0a811102b933404423a5a91786bb8f821b55cebdf6d1",
            ),
            (
                &v.mac_r[..],
                "0055176eb754a2ef5eaeb275586d918e9f88be4e4c5aab4631c046a331f859ca",
            ),
            (
                &v.enc_i2r[..],
                "c670538cef41bdbede912d8b317d5bd1b4798fc6807e6406b1c155f4c914844af3c83f74e083684e167ebb50",
            ),
            (
                &v.enc_r2i[..],
                "ba47d94ac26142a579f5e09a25da3387c53eef29edd72aba96e84976f681874d6a0c61fee24bb77b7ebbaafeb63abebe2120db9784d5",
            ),
        ];
        for (actual, expected) in checks {
            assert_eq!(hex(actual), expected);
        }
        assert_eq!(v.keys.code, 809_495);
        // Из шифрованного кадра получается исходный JSON.
        let mut open = OpenKey::new(&v.keys.i2r).unwrap();
        assert_eq!(text(&open.open(&v.enc_i2r).unwrap()), JSON_I2R);
        let mut open = OpenKey::new(&v.keys.r2i).unwrap();
        assert_eq!(text(&open.open(&v.enc_r2i).unwrap()), JSON_R2I);
        assert_eq!(unhex(&hex(&v.th)), v.th);
    }

    /// Настоящее рукопожатие с теми же ключами даёт те же TH и ключи.
    #[tokio::test]
    async fn real_handshake_matches_vectors() {
        let v = vectors();
        let eph_i = Ephemeral {
            key: EphemeralKey::from_scalar(&[0x11; 32]).unwrap(),
            nonce: [0xa1; 16],
        };
        let eph_r = Ephemeral {
            key: EphemeralKey::from_scalar(&[0x22; 32]).unwrap(),
            nonce: [0xb2; 16],
        };
        let (a, b) = duplex(1 << 20);
        let (host_i, host_r) = (
            TestHost::new(Some(PAIR_SECRET)),
            TestHost::new(Some(PAIR_SECRET)),
        );
        let li = local(ID_I, "Plastinka PC", "windows");
        let lr = local(ID_R, "Телефон", "android");
        let (hi, hr) = tokio::join!(
            initiate_with(a, &li, Some(ID_R), &host_i, eph_i, Timeouts::default()),
            accept_with(b, &lr, &host_r, eph_r, Timeouts::default())
        );
        let (hi, hr) = (hi.unwrap(), hr.unwrap());
        assert_eq!(hi.th, v.th);
        assert_eq!(hr.th, v.th);
        assert_eq!(hi.keys.pair, v.keys.pair);
        assert_eq!(hi.mode(), Mode::Recognize);
        let (_tx_i, rx_i) = oneshot::channel();
        let (_tx_r, rx_r) = oneshot::channel();
        let (i, r) = tokio::join!(hi.finish(rx_i), hr.finish(rx_r));
        assert!(i.is_ok() && r.is_ok());
    }
}
