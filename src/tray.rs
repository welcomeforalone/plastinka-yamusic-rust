//! Значок в области уведомлений (трей) и его меню: «Показать окно» /
//! «Скрыть окно», «Играть» / «Пауза», «Следующий», «Предыдущий», «Выход».
//!
//! Windows и macOS — через `tray-icon` (меню `muda`); Linux — тот же
//! `tray-icon` с ksni: StatusNotifierItem по D-Bus, без GTK. Значок должен
//! создаваться в главном потоке после запуска цикла событий окна, поэтому
//! его заводит окно на первом сообщении, а не `main`.
//!
//! События значка и меню приходят из системных обработчиков; здесь они
//! переправляются в канал [`Action`], который окно читает подпиской
//! ([`actions`]). Сам значок только рисуется: что делать с командами —
//! решает окно.

use std::sync::{Mutex, OnceLock};

use iced::futures::Stream;
use tokio::sync::mpsc;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::APP_NAME;

/// Что просит пользователь через значок.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Щелчок по значку: показать окно (или поднять, если оно уже на виду).
    Show,
    /// Пункт «Показать окно» / «Скрыть окно».
    Toggle,
    /// Пункт «Играть» / «Пауза».
    PlayPause,
    Next,
    Previous,
    /// Полный выход, как при закрытии окна без трея.
    Quit,
}

/// Идентификаторы пунктов меню.
const ID_TOGGLE: &str = "toggle";
const ID_PLAY: &str = "play";
const ID_NEXT: &str = "next";
const ID_PREVIOUS: &str = "previous";
const ID_QUIT: &str = "quit";

/// Linux: служба значков может подняться позже программы (запуск из
/// автозапуска при входе), поэтому создание значка повторяется: раз в
/// [`RETRY_EVERY`], всего [`ATTEMPTS`] попыток. В Windows и macOS попытка одна.
pub const RETRY_EVERY: std::time::Duration = std::time::Duration::from_secs(1);
#[cfg(all(unix, not(target_os = "macos")))]
pub const ATTEMPTS: u8 = 15;
#[cfg(not(all(unix, not(target_os = "macos"))))]
pub const ATTEMPTS: u8 = 1;

/// Почему значок не создался.
#[derive(Debug, Clone)]
pub struct CreateError {
    pub text: String,
    /// Имеет смысл повторить (служба значков может появиться позже).
    pub retry: bool,
}

impl CreateError {
    fn fatal(text: impl ToString) -> Self {
        Self {
            text: text.to_string(),
            retry: false,
        }
    }
}

/// Подсказка Windows обрезается на 127 символах: оставляем запас.
const TOOLTIP_MAX: usize = 120;

/// Что значок должен показывать: меняется вместе с окном и плеером.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct View {
    /// Главное окно спрятано в трей.
    pub hidden: bool,
    /// Есть главный экран с плеером: без него пункты плеера неактивны
    /// (правило 2: без проверки Плюса плеера нет).
    pub player: bool,
    pub playing: bool,
    /// «Исполнитель — Название» играющего трека; пусто, если ничего нет.
    pub track: String,
}

/// Тексты и доступность пунктов для [`View`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct Labels {
    toggle: &'static str,
    play: &'static str,
    player_enabled: bool,
    tooltip: String,
}

impl View {
    fn labels(&self) -> Labels {
        Labels {
            toggle: if self.hidden {
                "Показать окно"
            } else {
                "Скрыть окно"
            },
            play: if self.playing {
                "Пауза"
            } else {
                "Играть"
            },
            player_enabled: self.player,
            tooltip: tooltip(self.playing, &self.track),
        }
    }
}

/// Подсказка значка: имя приложения, а при игре — трек, обрезанный по длине.
fn tooltip(playing: bool, track: &str) -> String {
    if !playing || track.trim().is_empty() {
        return APP_NAME.to_owned();
    }
    let mut chars = track.trim().chars();
    let head: String = chars.by_ref().take(TOOLTIP_MAX).collect();
    if chars.next().is_some() {
        let mut head: String = head.chars().take(TOOLTIP_MAX - 1).collect();
        head.push('…');
        head
    } else {
        head
    }
}

/// Действие по идентификатору пункта меню.
fn action_for_item(id: &str) -> Option<Action> {
    match id {
        ID_TOGGLE => Some(Action::Toggle),
        ID_PLAY => Some(Action::PlayPause),
        ID_NEXT => Some(Action::Next),
        ID_PREVIOUS => Some(Action::Previous),
        ID_QUIT => Some(Action::Quit),
        _ => None,
    }
}

/// Действие по событию значка: только отпускание левой кнопки.
fn action_for_click(event: &TrayIconEvent) -> Option<Action> {
    match event {
        TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } => Some(Action::Show),
        _ => None,
    }
}

/// Канал действий: отправитель — обработчикам системы, приёмник — подписке.
struct Channel {
    sender: mpsc::UnboundedSender<Action>,
    receiver: Mutex<Option<mpsc::UnboundedReceiver<Action>>>,
}

fn channel() -> &'static Channel {
    static CHANNEL: OnceLock<Channel> = OnceLock::new();
    CHANNEL.get_or_init(|| {
        let (sender, receiver) = mpsc::unbounded_channel();
        Channel {
            sender,
            receiver: Mutex::new(Some(receiver)),
        }
    })
}

/// Поток действий для подписки окна. Приёмник забирается один раз.
pub fn actions() -> impl Stream<Item = Action> {
    let receiver = channel()
        .receiver
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    iced::futures::stream::unfold(receiver, |receiver| async move {
        let mut receiver = receiver?;
        let action = receiver.recv().await?;
        Some((action, Some(receiver)))
    })
}

/// Спрятанное окно закрывается совсем (а не скрывается), если система не
/// умеет скрывать окно: в Wayland winit этого не может.
pub fn hide_by_closing() -> bool {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let set = |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty());
        let forced_x11 = std::env::var("WINIT_UNIX_BACKEND").is_ok_and(|backend| backend == "x11");
        set("WAYLAND_DISPLAY") && !forced_x11
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    {
        false
    }
}

/// Есть ли в сессии служба, которая рисует значки (StatusNotifierWatcher).
#[cfg(all(unix, not(target_os = "macos")))]
fn watcher_present() -> Result<(), CreateError> {
    use zbus::blocking::Connection;
    use zbus::blocking::fdo::DBusProxy;
    use zbus::names::BusName;
    const NAMES: [&str; 2] = [
        "org.kde.StatusNotifierWatcher",
        "org.freedesktop.StatusNotifierWatcher",
    ];
    let connection = Connection::session()
        .map_err(|err| CreateError::fatal(format!("нет сессионной шины D-Bus: {err}")))?;
    let proxy = DBusProxy::new(&connection).map_err(CreateError::fatal)?;
    for name in NAMES {
        let name = BusName::try_from(name).map_err(CreateError::fatal)?;
        if proxy.name_has_owner(name).map_err(CreateError::fatal)? {
            return Ok(());
        }
    }
    Err(CreateError {
        retry: true,
        text: "нет службы значков StatusNotifierWatcher (в GNOME нужно расширение AppIndicator)"
            .to_owned(),
    })
}

/// В Windows и macOS значки рисует сама система.
#[cfg(not(all(unix, not(target_os = "macos"))))]
fn watcher_present() -> Result<(), CreateError> {
    Ok(())
}

/// Значок в трее. Живёт, пока жив; при удалении значок пропадает.
pub struct Tray {
    // Меню держим живым: на Linux оно читается из снимка.
    _menu: Menu,
    toggle: MenuItem,
    play: MenuItem,
    next: MenuItem,
    previous: MenuItem,
    icon: TrayIcon,
    shown: Option<Labels>,
}

impl Tray {
    /// Создаёт значок. Вызывать в главном потоке после запуска цикла окна.
    pub fn create() -> Result<Self, CreateError> {
        // Без службы значков (например, GNOME без расширения AppIndicator)
        // значок никто не нарисует, а окно, спрятанное «в трей», пропало бы.
        watcher_present()?;
        let sender = channel().sender.clone();
        let for_icon = sender.clone();
        // Обработчики ставятся один раз на процесс; ошибка отправки значит,
        // что окно уже закрылось.
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            if let Some(action) = action_for_click(&event) {
                let _ = for_icon.send(action);
            }
        }));
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if let Some(action) = action_for_item(&event.id.0) {
                let _ = sender.send(action);
            }
        }));

        let view = View::default();
        let labels = view.labels();
        let toggle = MenuItem::with_id(ID_TOGGLE, labels.toggle, true, None);
        let play = MenuItem::with_id(ID_PLAY, labels.play, labels.player_enabled, None);
        let next = MenuItem::with_id(ID_NEXT, "Следующий", labels.player_enabled, None);
        let previous = MenuItem::with_id(ID_PREVIOUS, "Предыдущий", labels.player_enabled, None);
        let quit = MenuItem::with_id(ID_QUIT, "Выход", true, None);
        let menu = Menu::new();
        menu.append_items(&[
            &toggle,
            &PredefinedMenuItem::separator(),
            &play,
            &next,
            &previous,
            &PredefinedMenuItem::separator(),
            &quit,
        ])
        .map_err(CreateError::fatal)?;

        let icon = TrayIconBuilder::new()
            .with_icon(tray_image().map_err(CreateError::fatal)?)
            .with_tooltip(&labels.tooltip)
            .with_menu(Box::new(menu.clone()))
            // Левая кнопка показывает окно, меню — по правой.
            .with_menu_on_left_click(false)
            .build()
            .map_err(CreateError::fatal)?;
        Ok(Self {
            _menu: menu,
            toggle,
            play,
            next,
            previous,
            icon,
            shown: None,
        })
    }

    /// Приводит значок к виду `view`. Меняет только то, что изменилось.
    pub fn apply(&mut self, view: &View) {
        let labels = view.labels();
        if self.shown.as_ref() == Some(&labels) {
            return;
        }
        self.toggle.set_text(labels.toggle);
        self.play.set_text(labels.play);
        for item in [&self.play, &self.next, &self.previous] {
            item.set_enabled(labels.player_enabled);
        }
        // Подсказка не критична: ошибку системы не показываем.
        let _ = self.icon.set_tooltip(Some(&labels.tooltip));
        self.shown = Some(labels);
    }
}

/// Картинка значка из иконки приложения.
fn tray_image() -> Result<Icon, String> {
    const PNG: &[u8] = include_bytes!("../assets/app/plastinka.png");
    // Windows рисует значок 16–32 пикселя, остальным нужен запас.
    let size: u32 = if cfg!(windows) { 32 } else { 64 };
    let image = image::load_from_memory_with_format(PNG, image::ImageFormat::Png)
        .map_err(|err| err.to_string())?;
    let image = image::imageops::resize(
        &image.into_rgba8(),
        size,
        size,
        image::imageops::FilterType::Lanczos3,
    );
    let (width, height) = image.dimensions();
    Icon::from_rgba(image.into_raw(), width, height).map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(hidden: bool, player: bool, playing: bool, track: &str) -> View {
        View {
            hidden,
            player,
            playing,
            track: track.to_owned(),
        }
    }

    #[test]
    fn toggle_text_follows_window_state() {
        assert_eq!(view(false, false, false, "").labels().toggle, "Скрыть окно");
        assert_eq!(
            view(true, false, false, "").labels().toggle,
            "Показать окно"
        );
    }

    #[test]
    fn play_text_and_availability_follow_player() {
        let idle = view(false, false, false, "").labels();
        assert_eq!(idle.play, "Играть");
        assert!(!idle.player_enabled, "без плеера пункты неактивны");
        let playing = view(false, true, true, "А — Б").labels();
        assert_eq!(playing.play, "Пауза");
        assert!(playing.player_enabled);
    }

    #[test]
    fn tooltip_shows_track_only_while_playing() {
        assert_eq!(tooltip(false, "Исполнитель — Название"), "Plastinka");
        assert_eq!(tooltip(true, ""), "Plastinka");
        assert_eq!(tooltip(true, "  "), "Plastinka");
        assert_eq!(
            tooltip(true, "Исполнитель — Название"),
            "Исполнитель — Название"
        );
    }

    #[test]
    fn long_tooltip_is_cut_with_ellipsis() {
        let long = "я".repeat(300);
        let text = tooltip(true, &long);
        assert_eq!(text.chars().count(), TOOLTIP_MAX);
        assert!(text.ends_with('…'));
        // Ровно в предел — без обрезки.
        let exact = "я".repeat(TOOLTIP_MAX);
        assert_eq!(tooltip(true, &exact), exact);
    }

    #[test]
    fn menu_items_map_to_actions() {
        assert_eq!(action_for_item("toggle"), Some(Action::Toggle));
        assert_eq!(action_for_item("play"), Some(Action::PlayPause));
        assert_eq!(action_for_item("next"), Some(Action::Next));
        assert_eq!(action_for_item("previous"), Some(Action::Previous));
        assert_eq!(action_for_item("quit"), Some(Action::Quit));
        assert_eq!(action_for_item("other"), None);
    }

    #[test]
    fn only_left_button_release_shows_window() {
        use tray_icon::dpi::PhysicalPosition;
        let click = |button, button_state| TrayIconEvent::Click {
            id: tray_icon::TrayIconId::new("t"),
            position: PhysicalPosition::new(0.0, 0.0),
            rect: tray_icon::Rect::default(),
            button,
            button_state,
        };
        assert_eq!(
            action_for_click(&click(MouseButton::Left, MouseButtonState::Up)),
            Some(Action::Show)
        );
        assert_eq!(
            action_for_click(&click(MouseButton::Left, MouseButtonState::Down)),
            None
        );
        assert_eq!(
            action_for_click(&click(MouseButton::Right, MouseButtonState::Up)),
            None
        );
    }

    #[test]
    fn tray_image_decodes() {
        assert!(tray_image().is_ok());
    }
}
