//! Plastinka — неофициальный десктопный клиент Яндекс Музыки.
//!
//! Нужна активная подписка Плюс. Треки скачиваются только по явному действию
//! пользователя в выбранную им папку.

pub mod api;
pub mod auth;
pub mod autostart;
pub mod discord;
pub mod download;
pub mod instance;
pub mod lyrics;
pub mod media_controls;
pub mod net;
pub mod player;
pub mod plugins;
pub mod relink;
pub mod storage;
pub mod sync;
pub mod texts;
pub mod tray;
pub mod ui;
pub mod update;

/// Имя приложения: заголовок окна, каталоги настроек и кэша.
pub const APP_NAME: &str = "Plastinka";

/// Обязательная строка для окна «О программе».
pub const DISCLAIMER: &str = "Неофициальный клиент, не связан с ООО «Яндекс»";
