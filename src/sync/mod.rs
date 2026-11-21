//! «Слушать вместе»: ядро протокола синхронного воспроизведения между
//! устройствами по локальной сети. Описание побайтно — `docs/api/sync.md`.
//!
//! Здесь нет ни mDNS, ни окна: кадры, рукопожатие с шифрованием, сообщения,
//! оценка часов и чистая логика выбора состояния и подстройки позиции.
//! Звук по сети не ходит (правило 1), токен Яндекса тоже.

pub mod clock;
pub mod crypto;
pub mod discovery;
pub mod handshake;
pub mod proto;
pub mod service;
pub mod state;
pub mod store;
pub mod wire;

use std::io;

pub use handshake::{
    Claim, ClaimOutcome, Established, Handshake, Host, Local, Mode, PairSecret, PeerInfo, Role,
    SasCode, accept, initiate,
};
pub use proto::{ByeReason, Message};

/// Версия протокола.
pub const PROTOCOL_VERSION: u32 = 1;

/// Ошибки связи. Тексты показываются пользователю.
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("ошибка связи: {0}")]
    Io(#[from] io::Error),
    #[error("собеседник слишком долго не отвечает")]
    Timeout,
    #[error("неверная длина кадра: {0} байт")]
    FrameLength(usize),
    #[error("собеседник нарушил протокол: {0}")]
    Protocol(&'static str),
    #[error("не удалось выполнить шифрование")]
    Crypto,
    #[error("не удалось расшифровать сообщение: связь повреждена")]
    Decrypt,
    #[error("счётчик сообщений исчерпан")]
    CounterExhausted,
    #[error("ключ собеседника не совпал с обязательством: связь небезопасна")]
    CommitMismatch,
    #[error("устройство не узнано или секрет не подошёл. Подтвердите код заново")]
    AuthFailed,
    #[error("собеседник закрыл связь: {0}")]
    PeerBye(ByeReason),
    #[error("не удалось запустить обнаружение устройств: {0}")]
    Discovery(String),
    #[error("встречное соединение уступило другому")]
    Superseded,
    #[error("мы закрыли связь: {0}")]
    Declined(ByeReason),
}
