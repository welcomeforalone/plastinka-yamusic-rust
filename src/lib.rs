//! Plastinka — неофициальный десктопный клиент Яндекс Музыки.
//!
//! Только потоковое воспроизведение и только при активной подписке Плюс.

pub mod api;
pub mod auth;
pub mod discord;
pub mod net;
pub mod player;
pub mod plugins;
pub mod storage;
pub mod texts;
pub mod ui;

/// Имя приложения: заголовок окна, каталоги настроек и кэша.
pub const APP_NAME: &str = "Plastinka";

/// Обязательная строка для окна «О программе».
pub const DISCLAIMER: &str = "Неофициальный клиент, не связан с ООО «Яндекс»";
