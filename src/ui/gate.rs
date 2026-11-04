//! Экраны до главного окна: вход по коду, карточки ошибок, «Нужна подписка».
//! Здесь же заголовок окна, диалоги и всплывающее сообщение.

use std::sync::Arc;
use std::time::Instant;

use iced::task;
use iced::widget::{
    Space, button, center, column, container, hover, mouse_area, opaque, right, row, scrollable,
    stack,
};
use iced::{Alignment, Border, Element, Length, Padding};

use super::icons::{Icon, icon, spinner};
use super::theme::{self, Style};
use super::widgets::{self, Kind, Size};
use super::{Dialog, FailureKind, Message, Problem};
use crate::api::{AccountStatus, Subscription};
use crate::auth::DeviceCode;
use crate::storage::token::{StoredToken, UNAVAILABLE_HINT};
use crate::{APP_NAME, DISCLAIMER};

/// Где оформить подписку.
const PLUS_URL: &str = "https://plus.yandex.ru";

// ---------------------------------------------------------------- состояние

/// Экран входа.
pub struct Login {
    /// Пояснение сверху, например «вход больше не действует».
    notice: Option<String>,
    phase: Phase,
    pub copied_until: Option<Instant>,
}

enum Phase {
    /// Запрашиваем код.
    Requesting,
    /// Код показан, ждём подтверждения. Опрос отменяется, когда фаза меняется.
    Waiting {
        code: Arc<DeviceCode>,
        expires_at: Instant,
        _poll: task::Handle,
    },
    /// Токен получен: проверяем и сохраняем.
    Finishing {
        code: Arc<DeviceCode>,
    },
    Expired {
        code: Arc<DeviceCode>,
    },
    Failed {
        code: Option<Arc<DeviceCode>>,
        message: String,
    },
}

impl Login {
    pub fn new(notice: Option<String>) -> Self {
        Self {
            notice,
            phase: Phase::Requesting,
            copied_until: None,
        }
    }

    /// Код, который ещё можно ввести.
    pub fn code(&self) -> Option<&DeviceCode> {
        match &self.phase {
            Phase::Waiting { code, .. } | Phase::Finishing { code } => Some(code),
            _ => None,
        }
    }

    fn shown_code(&self) -> Option<&DeviceCode> {
        match &self.phase {
            Phase::Requesting => None,
            Phase::Waiting { code, .. } | Phase::Finishing { code } | Phase::Expired { code } => {
                Some(code)
            }
            Phase::Failed { code, .. } => code.as_deref(),
        }
    }

    pub fn waiting(&mut self, code: Arc<DeviceCode>, poll: task::Handle) {
        let expires_at = Instant::now() + code.expires_in;
        self.phase = Phase::Waiting {
            code,
            expires_at,
            _poll: poll,
        };
    }

    pub fn finishing(&mut self) {
        let phase = std::mem::replace(&mut self.phase, Phase::Requesting);
        self.phase = match phase {
            Phase::Waiting { code, .. } | Phase::Finishing { code } => Phase::Finishing { code },
            other => other,
        };
    }

    pub fn expire(&mut self) {
        let phase = std::mem::replace(&mut self.phase, Phase::Requesting);
        self.phase = match phase {
            Phase::Waiting { code, .. } | Phase::Finishing { code } | Phase::Expired { code } => {
                Phase::Expired { code }
            }
            other => other,
        };
    }

    pub fn expire_if_due(&mut self, now: Instant) {
        if matches!(self.phase, Phase::Waiting { expires_at, .. } if now >= expires_at) {
            self.expire();
        }
    }

    pub fn fail(&mut self, message: String) {
        let phase = std::mem::replace(&mut self.phase, Phase::Requesting);
        let code = match phase {
            Phase::Waiting { code, .. } | Phase::Finishing { code } | Phase::Expired { code } => {
                Some(code)
            }
            Phase::Failed { code, .. } => code,
            Phase::Requesting => None,
        };
        self.phase = Phase::Failed { code, message };
    }

    pub fn animating(&self) -> bool {
        matches!(
            self.phase,
            Phase::Requesting | Phase::Waiting { .. } | Phase::Finishing { .. }
        )
    }
}

/// Экран «Нужна подписка».
pub struct Paywall {
    pub token: Arc<StoredToken>,
    status: AccountStatus,
    pub checking: bool,
    pub error: Option<String>,
}

impl Paywall {
    pub fn new(token: Arc<StoredToken>, status: AccountStatus) -> Self {
        Self {
            token,
            status,
            checking: false,
            error: None,
        }
    }
}

// ---------------------------------------------------------------- каркас

/// Полоса заголовка 36 px: тянет окно, двойной клик разворачивает.
pub fn title_bar<'a>(with_brand: bool) -> Element<'a, Message> {
    let brand: Element<'a, Message> = if with_brand {
        row![
            widgets::logo(18.0),
            theme::line(APP_NAME, Style::CaptionStrong, theme::TEXT_SECONDARY),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into()
    } else {
        Space::new().into()
    };
    let drag = mouse_area(
        container(brand)
            .padding(Padding::default().left(16))
            .width(Length::Fill)
            .height(Length::Fill)
            .center_y(Length::Fill),
    )
    .on_press(Message::DragWindow)
    .on_double_click(Message::ToggleMaximize);
    row![
        drag,
        window_button(Icon::WindowMinimize, Message::Minimize, false),
        window_button(Icon::WindowMaximize, Message::ToggleMaximize, false),
        window_button(Icon::WindowClose, Message::CloseWindow, true),
    ]
    .height(36)
    .into()
}

fn window_button<'a>(name: Icon, on_press: Message, close: bool) -> Element<'a, Message> {
    let make = move |hovered: bool| -> Element<'a, Message> {
        let color = if hovered {
            theme::TEXT_PRIMARY
        } else {
            theme::TEXT_SECONDARY
        };
        button(center(icon(name, 16.0, color)))
            .width(46)
            .height(36)
            .padding(0)
            .style(move |_, status| button::Style {
                background: match status {
                    button::Status::Hovered | button::Status::Pressed if close => {
                        Some(theme::CLOSE_HOVER.into())
                    }
                    button::Status::Hovered => Some(theme::SURFACE_3.into()),
                    button::Status::Pressed => Some(theme::SURFACE_4.into()),
                    _ => None,
                },
                ..button::Style::default()
            })
            .on_press(on_press.clone())
            .into()
    };
    hover(make(false), make(true))
}

/// Экран до главного окна: содержимое по центру, внизу обязательная строка.
pub fn frame<'a>(body: Element<'a, Message>) -> Element<'a, Message> {
    column![
        center(scrollable(center(body).padding(24)).height(Length::Shrink)).height(Length::Fill),
        container(theme::line(
            DISCLAIMER,
            Style::Caption,
            theme::TEXT_TERTIARY
        ))
        .center_x(Length::Fill)
        .padding(Padding::default().bottom(14)),
    ]
    .into()
}

pub fn loading<'a>(caption: &'a str, turn: f32) -> Element<'a, Message> {
    column![
        spinner(32.0, theme::ACCENT, turn),
        theme::line(caption, Style::Body, theme::TEXT_SECONDARY),
    ]
    .spacing(16)
    .align_x(Alignment::Center)
    .into()
}

// ---------------------------------------------------------------- ошибки

pub fn problem<'a>(problem: &'a Problem, now: Instant) -> Element<'a, Message> {
    let (badge_icon, body) = match problem.kind {
        FailureKind::Offline => {
            let mut body =
                "Не получилось связаться с сервером Яндекса. Проверьте сеть или прокси.".to_owned();
            if let Some(at) = problem.retry_at {
                let left = at.saturating_duration_since(now).as_secs_f32().ceil();
                body.push_str(&format!(" Новая попытка через {left:.0} с."));
            }
            (Icon::Offline, body)
        }
        FailureKind::Keyring => (
            Icon::Lock,
            format!(
                "Системное хранилище паролей недоступно. Без него {APP_NAME} не запомнит \
                 вход. Включите Диспетчер учётных данных (Windows) или Secret Service \
                 (Linux) и повторите."
            ),
        ),
        _ => (Icon::Error, problem.text.clone()),
    };
    let mut buttons = row![widgets::action(
        Kind::Primary,
        Some(Icon::Retry),
        "Повторить",
        Some(Message::Retry)
    )]
    .spacing(12);
    if problem.kind == FailureKind::Keyring {
        buttons = buttons.push(widgets::action(
            Kind::Secondary,
            Some(Icon::About),
            if problem.show_help {
                "Скрыть подробности"
            } else {
                "Как включить"
            },
            Some(Message::ToggleHelp),
        ));
    }
    let mut content = column![
        widgets::badge(badge_icon, theme::ERROR, theme::ERROR_SOFT),
        widgets::heading(problem.title.clone(), body),
    ]
    .spacing(20)
    .align_x(Alignment::Center);
    if problem.kind == FailureKind::Keyring && problem.show_help {
        content = content.push(widgets::notice(
            Icon::About,
            format!("{UNAVAILABLE_HINT}. Подробности: {}", problem.text),
            false,
        ));
    }
    widgets::card(content.push(buttons))
}

// ---------------------------------------------------------------- вход

pub fn login<'a>(login: &'a Login, now: Instant, turn: f32) -> Element<'a, Message> {
    let shown = login.shown_code();
    let url = shown.map(|code| {
        let url = &code.verification_url;
        let host = url.host_str().unwrap_or_default();
        (url.as_str().to_owned(), format!("{host}{}", url.path()))
    });

    let mut content = column![
        widgets::logo(56.0),
        widgets::heading(
            format!("Вход в {APP_NAME}"),
            "Подтвердите вход на странице Яндекса. Пароль вводится только там — приложение его не видит."
                .to_owned(),
        ),
    ]
    .spacing(20)
    .align_x(Alignment::Center);
    if let Some(notice) = &login.notice {
        content = content.push(widgets::notice(Icon::About, notice.clone(), false));
    }

    let open_step: Element<'a, Message> = match &url {
        Some((full, short)) => widgets::link(
            short.clone(),
            Style::BodyStrong,
            Message::OpenUrl(full.clone()),
        ),
        None => theme::line("страницу входа", Style::Body, theme::TEXT_SECONDARY).into(),
    };
    let steps = column![
        step(
            "1",
            row![
                theme::line("Откройте", Style::Body, theme::TEXT_SECONDARY),
                open_step
            ]
            .spacing(12)
            .align_y(Alignment::Center)
            .into()
        ),
        step(
            "2",
            theme::line("Введите код:", Style::Body, theme::TEXT_SECONDARY).into()
        ),
    ]
    .spacing(12)
    .width(Length::Fill);

    let copyable = login.code().is_some();
    content = content.push(column![steps, code_box(shown, copyable, turn)].spacing(16));

    let copied = login.copied_until.is_some_and(|until| now < until);
    let bottom: Element<'a, Message> = match &login.phase {
        Phase::Requesting => widgets::waiting("Получаем код…".to_owned(), turn),
        Phase::Waiting { .. } | Phase::Finishing { .. } => {
            let open = url.as_ref().map(|(full, _)| Message::OpenUrl(full.clone()));
            let buttons = row![
                widgets::wide_action(
                    Kind::Primary,
                    Some(Icon::ExternalLink),
                    "Открыть страницу",
                    open
                ),
                widgets::wide_action(
                    Kind::Secondary,
                    Some(if copied { Icon::Check } else { Icon::Copy }),
                    if copied {
                        "Скопировано"
                    } else {
                        "Скопировать код"
                    },
                    Some(Message::CopyCode)
                ),
            ]
            .spacing(12);
            let status: Element<'a, Message> = match &login.phase {
                Phase::Waiting { expires_at, .. } => {
                    let left = expires_at.saturating_duration_since(now).as_secs();
                    row![
                        widgets::waiting("Ждём подтверждения…".to_owned(), turn),
                        Space::new().width(Length::Fill),
                        theme::line(
                            format!("Код действует {}", crate::texts::clock(left)),
                            Style::Secondary,
                            theme::TEXT_TERTIARY
                        ),
                    ]
                    .align_y(Alignment::Center)
                    .into()
                }
                _ => widgets::waiting("Проверяем вход…".to_owned(), turn),
            };
            column![buttons, status].spacing(20).into()
        }
        Phase::Expired { .. } => column![
            widgets::notice(
                Icon::Expired,
                "Код истёк. Получите новый, чтобы продолжить.".to_owned(),
                true
            ),
            wide_new_code(),
        ]
        .spacing(20)
        .into(),
        Phase::Failed { message, .. } => column![
            widgets::notice(Icon::Error, message.clone(), true),
            wide_new_code()
        ]
        .spacing(20)
        .into(),
    };
    widgets::card(content.push(bottom))
}

fn wide_new_code<'a>() -> Element<'a, Message> {
    widgets::wide_action(
        Kind::Primary,
        Some(Icon::Retry),
        "Получить новый код",
        Some(Message::NewCode),
    )
}

fn step<'a>(number: &'a str, content: Element<'a, Message>) -> Element<'a, Message> {
    row![
        container(theme::line(number, Style::Caption, theme::TEXT_SECONDARY))
            .center(24)
            .style(|_| container::Style {
                background: Some(theme::SURFACE_3.into()),
                border: Border::default().rounded(widgets::ROUND),
                ..container::Style::default()
            }),
        content,
    ]
    .spacing(12)
    .align_y(Alignment::Center)
    .into()
}

/// Код устройства крупно. Трекинга в iced нет: буквы ставим по одной.
fn code_box<'a>(code: Option<&DeviceCode>, copyable: bool, turn: f32) -> Element<'a, Message> {
    let inner: Element<'a, Message> = match code {
        None => center(spinner(28.0, theme::ACCENT, turn)).into(),
        Some(code) => {
            let color = if copyable {
                theme::TEXT_PRIMARY
            } else {
                theme::TEXT_DISABLED
            };
            let letters = code
                .user_code
                .chars()
                .map(|ch| theme::line(ch.to_string(), Style::Code, color).into());
            stack![
                center(iced::widget::Row::with_children(letters).spacing(3)),
                right(widgets::icon_button(
                    Icon::Copy,
                    Size::S,
                    false,
                    copyable.then_some(Message::CopyCode)
                ))
                .center_y(Length::Fill)
                .padding(Padding::default().right(20)),
            ]
            .into()
        }
    };
    container(inner)
        .width(Length::Fill)
        .height(76)
        .style(|_| container::Style {
            background: Some(theme::SURFACE_2.into()),
            border: Border::default().rounded(14),
            ..container::Style::default()
        })
        .into()
}

// ---------------------------------------------------------------- подписка

pub fn paywall<'a>(paywall: &'a Paywall, turn: f32) -> Element<'a, Message> {
    let name = paywall.status.account().name();
    let (title, body, hint) = match paywall.status.subscription() {
        Subscription::Missing { can_start_trial } => {
            let mut body = format!(
                "{APP_NAME} воспроизводит музыку только при активной подписке Яндекс Плюс. \
                 В аккаунте {name} подписки не нашлось."
            );
            if *can_start_trial {
                body.push_str(" Для аккаунта доступен пробный период.");
            }
            (
                "Нужна подписка Плюс",
                body,
                "Только что оформили подписку? Подождите минуту и проверьте снова.",
            )
        }
        Subscription::RegionUnavailable { region_code } => {
            let region = region_code
                .as_deref()
                .map(|code| format!(" (код региона {code})"))
                .unwrap_or_default();
            (
                "Сервис недоступен в регионе",
                format!(
                    "Сервис музыки недоступен в регионе аккаунта {name}{region}. \
                     Воспроизведение невозможно, даже если подписка Плюс есть."
                ),
                "Если вы сменили регион аккаунта, проверьте снова.",
            )
        }
        Subscription::Unconfirmed | Subscription::Active(_) => (
            "Подписку не удалось подтвердить",
            format!(
                "Сервис не прислал понятных сведений о подписке Плюс для аккаунта {name}. \
                 Пока подписка не подтверждена, воспроизведение недоступно."
            ),
            "Попробуйте проверить снова через минуту.",
        ),
    };
    let mut content = column![
        widgets::badge(Icon::Lock, theme::ACCENT, theme::ACCENT_SOFT),
        widgets::heading(title.to_owned(), body),
        widgets::notice(Icon::About, hint.to_owned(), false),
    ]
    .spacing(20)
    .align_x(Alignment::Center);
    if let Some(error) = &paywall.error {
        content = content.push(widgets::notice(Icon::Error, error.clone(), true));
    }
    let recheck: Element<'a, Message> = if paywall.checking {
        widgets::wide_action_with(
            Kind::Primary,
            Some(spinner(18.0, theme::ON_ACCENT, turn).into()),
            "Проверяем…",
            Some(Message::Noop),
        )
    } else {
        widgets::wide_action(
            Kind::Primary,
            Some(Icon::Retry),
            "Проверить снова",
            Some(Message::Recheck),
        )
    };
    let buttons = row![
        recheck,
        widgets::wide_action(
            Kind::Secondary,
            Some(Icon::Logout),
            "Выйти",
            (!paywall.checking).then_some(Message::Logout)
        ),
    ]
    .spacing(12);
    let plus = row![
        theme::line("Оформить подписку:", Style::Secondary, theme::TEXT_TERTIARY),
        widgets::link(
            "plus.yandex.ru".to_owned(),
            Style::Secondary,
            Message::OpenUrl(PLUS_URL.to_owned())
        ),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    widgets::card(content.push(buttons).push(plus))
}

// ---------------------------------------------------------------- поверх окна

/// Всплывающее сообщение над плеером.
/// Сообщение внизу, по центру между отступами `insets` (слева, справа).
pub fn toast<'a>(text: &'a str, insets: (f32, f32)) -> Element<'a, Message> {
    container(
        container(theme::label(text, Style::Body, theme::TEXT_PRIMARY))
            .max_width(560)
            .padding([12, 16])
            .style(|_| container::Style {
                background: Some(theme::SURFACE_3.into()),
                border: Border {
                    color: theme::BORDER,
                    width: 1.0,
                    radius: widgets::RADIUS_M.into(),
                },
                shadow: widgets::popover_shadow(),
                ..container::Style::default()
            }),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .align_x(Alignment::Center)
    .align_y(Alignment::End)
    .padding(
        Padding::default()
            .bottom(104)
            .left(insets.0)
            .right(insets.1),
    )
    .into()
}

/// Диалог поверх затемнения. Клик мимо карточки закрывает его.
pub fn dialog<'a>(dialog: Dialog) -> Element<'a, Message> {
    let body: Element<'a, Message> = match dialog {
        Dialog::About => about(),
        Dialog::Licenses => licenses(),
        Dialog::ConfirmLogout => confirm_logout(),
    };
    let close = right(widgets::icon_button(
        Icon::Close,
        Size::S,
        false,
        Some(Message::CloseDialog),
    ));
    let card = container(stack![body, close])
        .width(460)
        .padding(24)
        .style(|_| container::Style {
            background: Some(theme::SURFACE_2.into()),
            border: Border {
                color: theme::BORDER,
                width: 1.0,
                radius: widgets::RADIUS_XL.into(),
            },
            shadow: widgets::popover_shadow(),
            ..container::Style::default()
        });
    opaque(
        mouse_area(
            container(opaque(card))
                .center(Length::Fill)
                .style(|_| container::Style {
                    background: Some(theme::SCRIM.into()),
                    ..container::Style::default()
                }),
        )
        .on_press(Message::CloseDialog),
    )
}

fn about<'a>() -> Element<'a, Message> {
    column![
        widgets::logo(64.0),
        column![
            theme::line(APP_NAME, Style::Title, theme::TEXT_PRIMARY),
            theme::line(
                concat!("Версия ", env!("CARGO_PKG_VERSION")),
                Style::Secondary,
                theme::TEXT_TERTIARY
            ),
        ]
        .spacing(2)
        .align_x(Alignment::Center),
        widgets::divider(),
        theme::label(DISCLAIMER, Style::BodyStrong, theme::TEXT_PRIMARY).center(),
        theme::label(
            "Музыка воспроизводится потоком через API Яндекс Музыки по вашей подписке. \
             Аудио не сохраняется на диск.",
            Style::Secondary,
            theme::TEXT_SECONDARY
        )
        .center(),
        widgets::action(
            Kind::Secondary,
            None,
            "Сторонние лицензии",
            Some(Message::OpenDialog(Dialog::Licenses))
        ),
    ]
    .spacing(16)
    .padding([12, 16])
    .align_x(Alignment::Center)
    .into()
}

/// Главные сторонние компоненты. Полный список зависимостей с лицензиями
/// кладём в релиз отдельным файлом.
const LICENSES: &[(&str, &str)] = &[
    ("iced — интерфейс", "MIT"),
    ("Inter — шрифт", "SIL Open Font License 1.1"),
    ("Lucide — иконки", "ISC"),
    ("tokio, reqwest, serde — сеть и данные", "MIT"),
    ("keyring-core — хранилище паролей", "MIT или Apache-2.0"),
    (
        "rustls, aws-lc-rs — защищённое соединение",
        "Apache-2.0, ISC, MIT",
    ),
    ("image — обложки", "MIT или Apache-2.0"),
    // MPL-2.0: код symphonia не меняем, исходники — github.com/pdeljanov/Symphonia.
    ("symphonia — декодер MP3", "MPL-2.0"),
    ("rodio, cpal — вывод звука", "MIT или Apache-2.0"),
    ("md-5 — подпись ссылок", "MIT или Apache-2.0"),
];

fn licenses<'a>() -> Element<'a, Message> {
    let rows = LICENSES.iter().map(|(name, license)| {
        row![
            theme::label(*name, Style::Body, theme::TEXT_PRIMARY).width(Length::Fill),
            theme::label(*license, Style::Secondary, theme::TEXT_SECONDARY),
        ]
        .spacing(12)
        .into()
    });
    column![
        theme::line("Сторонние лицензии", Style::Title, theme::TEXT_PRIMARY),
        theme::label(
            "Полный список зависимостей с текстами лицензий прилагается к релизу.",
            Style::Secondary,
            theme::TEXT_SECONDARY
        ),
        iced::widget::Column::with_children(rows).spacing(10),
        widgets::action(
            Kind::Ghost,
            None,
            "Назад",
            Some(Message::OpenDialog(Dialog::About))
        ),
    ]
    .spacing(16)
    .padding(Padding::default().top(4))
    .into()
}

fn confirm_logout<'a>() -> Element<'a, Message> {
    column![
        theme::line("Выйти из аккаунта?", Style::Title, theme::TEXT_PRIMARY),
        theme::label(
            "Токен удалится с этого компьютера и будет отозван на сервере Яндекса. \
             Чтобы снова слушать музыку, нужно будет войти заново.",
            Style::Body,
            theme::TEXT_SECONDARY
        ),
        row![
            widgets::action(
                Kind::Danger,
                Some(Icon::Logout),
                "Выйти",
                Some(Message::Logout)
            ),
            widgets::action(Kind::Secondary, None, "Отмена", Some(Message::CloseDialog)),
        ]
        .spacing(12),
    ]
    .spacing(16)
    .padding(Padding::default().top(4))
    .into()
}
