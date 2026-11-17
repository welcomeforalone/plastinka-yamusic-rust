//! Страницы настроек: каждая отдаёт список записей [`Entry`].

use std::borrow::Cow;

use iced::widget::{Space, button, center, column, container, row, toggler, tooltip};
use iced::{Alignment, Background, Border, Color, Element, Length, Padding};

use super::controls::{padded_row, setting_row, switch};
use super::{Category, Ctx, Entry};
use crate::api::{self, AudioQuality};
use crate::storage::config;
use crate::ui::Message as App;
use crate::ui::icons::{Icon, icon};
use crate::ui::library::avatar;
use crate::ui::theme::{self, Style};
use crate::ui::widgets::{self, Kind, Size};
use crate::ui::{Dialog, Prefs, Zoom, palette};

/// Записи страницы раздела.
pub fn entries<'a>(category: Category, ctx: &Ctx<'a>) -> Vec<Entry<'a>> {
    match category {
        Category::Appearance => appearance(ctx.prefs, ctx.palette),
        Category::Sound => sound(ctx.prefs),
        Category::Equalizer => super::eq_page::entries(ctx),
        Category::Lyrics => lyrics(ctx.prefs),
        Category::Plugins => ctx.library.plugin_entries(),
        Category::Discord => discord(ctx.prefs),
        Category::Account => account(ctx),
        Category::About => about(),
    }
}

/// Строка с переключателем или другим управлением справа: пояснение нужно и
/// самой строке, и поиску.
pub fn item<'a>(
    title: &'a str,
    hint: String,
    keywords: &'static str,
    control: Element<'a, App>,
) -> Entry<'a> {
    Entry::new(
        title,
        hint.clone(),
        keywords,
        setting_row(title, hint, control),
    )
}

// ------------------------------------------------------------ оформление

/// Образец акцента в ряду: кружок с галочкой, если он выбран.
fn swatch<'a>(
    fill: Background,
    mark: Element<'a, App>,
    selected: bool,
    tip: impl iced::widget::text::IntoFragment<'a>,
    on_press: App,
) -> Element<'a, App> {
    let swatch = button(mark)
        .width(30)
        .height(30)
        .padding(0)
        .style(move |_, status| button::Style {
            background: Some(fill),
            border: Border {
                color: match (selected, status) {
                    (true, _) => theme::TEXT_PRIMARY,
                    (false, button::Status::Hovered) => theme::TEXT_SECONDARY,
                    _ => Color::TRANSPARENT,
                },
                width: 2.0,
                radius: widgets::ROUND.into(),
            },
            ..button::Style::default()
        })
        .on_press(on_press);
    widgets::with_tooltip(swatch, tip, tooltip::Position::Top)
}

/// «Оформление»: акцент, масштаб, визуализатор.
fn appearance<'a>(prefs: Prefs, palette: &'a palette::State) -> Vec<Entry<'a>> {
    let current = prefs.accent;
    let mut swatches = row(theme::Accent::PRESETS.into_iter().map(|accent| {
        let selected = accent == current;
        let mark: Element<'a, App> = if selected {
            center(icon(Icon::Check, 16.0, theme::contrast_on(accent.color()))).into()
        } else {
            Space::new().into()
        };
        swatch(
            accent.color().into(),
            mark,
            selected,
            accent.title(),
            App::SetAccent(accent),
        )
    }))
    .spacing(10)
    .align_y(Alignment::Center);

    // Шестой образец: свой цвет. Радужный, пока выбран готовый акцент.
    let custom = matches!(current, theme::Accent::Custom(_));
    let (fill, mark): (Background, Element<'a, App>) = if custom {
        (
            current.color().into(),
            center(icon(Icon::Check, 16.0, theme::contrast_on(current.color()))).into(),
        )
    } else {
        (
            palette::rainbow(),
            center(icon(Icon::Plus, 16.0, Color::WHITE)).into(),
        )
    };
    let tip = if custom {
        current.title()
    } else {
        "Свой цвет".into()
    };
    swatches = swatches.push(swatch(
        fill,
        mark,
        custom || palette.open,
        tip,
        App::Palette(palette::Msg::Toggle),
    ));

    let hint = format!("{} — кнопки, выбранный трек, ползунки", current.title());
    let mut accent = column![setting_row("Акцент", hint.clone(), swatches.into())];
    if palette.open {
        accent = accent.push(container(palette::view(palette)).padding(Padding {
            top: 0.0,
            right: 0.0,
            bottom: 16.0,
            left: 0.0,
        }));
    }

    let scale = prefs.scale;
    let zoom = row![
        widgets::icon_button(
            Icon::Minus,
            Size::S,
            false,
            (scale > config::SCALE_MIN).then_some(App::Zoom(Zoom::Out)),
        ),
        container(theme::line(
            format!("{scale} %"),
            Style::BodyStrong,
            theme::TEXT_PRIMARY
        ))
        .center_x(56),
        widgets::icon_button(
            Icon::Plus,
            Size::S,
            false,
            (scale < config::SCALE_MAX).then_some(App::Zoom(Zoom::In)),
        ),
        widgets::action(
            Kind::Secondary,
            None,
            "Сбросить",
            (scale != config::SCALE_DEFAULT).then_some(App::Zoom(Zoom::Reset)),
        ),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    vec![
        Entry::new(
            "Акцент",
            hint,
            "цвет тема палитра красный бирюзовый оранжевый фиолетовый синий свой",
            accent.into(),
        ),
        item(
            "Масштаб",
            "Ctrl + = и Ctrl + − меняют масштаб, Ctrl + 0 — 100 %".to_owned(),
            "размер увеличить уменьшить zoom",
            zoom.into(),
        ),
        item(
            "Визуализатор",
            "Спектр звука под плеером".to_owned(),
            "спектр анимация эквалайзер",
            switch(prefs.visualizer, |_| App::ToggleVisualizer),
        ),
    ]
}

// ------------------------------------------------------------------ звук

/// Варианты качества звука: заголовок и описание.
const QUALITY_CHOICES: [(AudioQuality, &str, &str); 3] = [
    (
        AudioQuality::Lossless,
        "Превосходное",
        "Музыка в lossless и других высококачественных форматах для быстрого интернета \
         и хорошей акустики",
    ),
    (
        AudioQuality::Normal,
        "Оптимальное",
        "Сбалансированный звук для большинства устройств",
    ),
    (
        AudioQuality::Low,
        "Экономичное",
        "Стабильное звучание при медленном интернете",
    ),
];

const NORMALIZE_TITLE: &str = "Нормализация громкости";
const NORMALIZE_HINT: &str = "Выравнивает громкость треков";

/// Три уровня качества кнопками.
fn quality_list<'a>(prefs: Prefs) -> Element<'a, App> {
    let mut options = column![].spacing(2);
    for (quality, title, hint) in QUALITY_CHOICES {
        let selected = prefs.quality == quality;
        let mark: Element<'a, App> = if selected {
            icon(Icon::Check, 20.0, theme::accent()).into()
        } else {
            Space::new().width(20).into()
        };
        options = options.push(
            button(
                row![
                    column![
                        theme::line(title, Style::BodyStrong, theme::TEXT_PRIMARY),
                        theme::label(hint, Style::Secondary, theme::TEXT_SECONDARY),
                    ]
                    .spacing(2)
                    .width(Length::Fill),
                    mark,
                ]
                .spacing(16)
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .padding([12, 14])
            .style(|_, status| button::Style {
                background: match status {
                    button::Status::Hovered | button::Status::Pressed => {
                        Some(theme::SURFACE_3.into())
                    }
                    _ => None,
                },
                border: Border::default().rounded(12.0),
                ..button::Style::default()
            })
            .on_press(App::SetAudioQuality(quality)),
        );
    }
    options.into()
}

/// Содержимое панели у значка качества: три уровня и нормализация.
pub fn sound_options<'a>(prefs: &Prefs) -> Element<'a, App> {
    column![
        quality_list(*prefs),
        widgets::divider(),
        padded_row(
            NORMALIZE_TITLE,
            NORMALIZE_HINT.to_owned(),
            switch(prefs.normalize, App::SetNormalize),
            Padding::from([12, 14]),
        ),
    ]
    .spacing(8)
    .into()
}

/// «Звук»: качество потока и нормализация.
fn sound<'a>(prefs: Prefs) -> Vec<Entry<'a>> {
    let hint = "Превосходное, оптимальное или экономичное";
    vec![
        Entry::new(
            "Качество звука",
            hint,
            "поток битрейт lossless flac mp3 aac",
            column![
                theme::line("Качество звука", Style::BodyStrong, theme::TEXT_PRIMARY),
                quality_list(prefs),
            ]
            .spacing(8)
            .padding(Padding::from([16, 0]))
            .into(),
        ),
        item(
            NORMALIZE_TITLE,
            NORMALIZE_HINT.to_owned(),
            "громкость выравнивание",
            switch(prefs.normalize, App::SetNormalize),
        ),
    ]
}

// ----------------------------------------------------------- текст песни

/// «Текст песни»: запасной источник LRCLib.
fn lyrics<'a>(prefs: Prefs) -> Vec<Entry<'a>> {
    vec![item(
        "Искать текст в LRCLib",
        "Если у Яндекса текста нет. В lrclib.net уходят название и исполнитель трека".to_owned(),
        "тексты песен lyrics lrclib",
        switch(prefs.lyrics_lrclib, App::SetLyricsLrclib),
    )]
}

// --------------------------------------------------------------- discord

/// «Discord»: статус «Слушает» с названием трека и обложкой.
fn discord<'a>(prefs: Prefs) -> Vec<Entry<'a>> {
    use crate::discord::{AppName, Lang, Settings, StatusKind};
    let settings = prefs.discord;
    let ready = prefs.discord_ready;
    // Переключатели неактивны, пока нет Application ID.
    let toggle = move |on: bool, change: fn(Settings, bool) -> Settings| -> Element<'a, App> {
        if ready {
            switch(on, move |value| App::SetDiscord(change(settings, value)))
        } else {
            // Без Application ID переключатель приглушён и не отвечает.
            toggler(on)
                .size(22)
                .style(|_, status| {
                    let on = matches!(
                        status,
                        toggler::Status::Active { is_toggled: true }
                            | toggler::Status::Hovered { is_toggled: true }
                            | toggler::Status::Disabled { is_toggled: true }
                    );
                    toggler::Style {
                        background: theme::SURFACE_3.into(),
                        background_border_width: 0.0,
                        background_border_color: Color::TRANSPARENT,
                        foreground: theme::TEXT_DISABLED.into(),
                        foreground_border_width: 0.0,
                        foreground_border_color: Color::TRANSPARENT,
                        text_color: None,
                        border_radius: None,
                        padding_ratio: if on { 0.12 } else { 0.3 },
                    }
                })
                .into()
        }
    };
    // Выбор из нескольких значений — таблетки.
    let pills = |items: Vec<(String, bool, Settings, bool)>| -> Element<'a, App> {
        row(items.into_iter().map(|(title, selected, next, usable)| {
            let ready = ready && usable;
            let pill = button(theme::line(
                title,
                Style::CaptionStrong,
                match (selected, ready) {
                    (_, false) => theme::TEXT_DISABLED,
                    (true, true) => theme::on_accent(),
                    (false, true) => theme::TEXT_SECONDARY,
                },
            ))
            .padding([6, 14])
            .style(move |_, status| button::Style {
                background: Some(
                    match (selected, status) {
                        (true, _) if !ready => theme::SURFACE_4,
                        (true, _) => theme::accent(),
                        (false, button::Status::Hovered | button::Status::Pressed) if ready => {
                            theme::SURFACE_4
                        }
                        (false, _) => theme::SURFACE_3,
                    }
                    .into(),
                ),
                border: Border::default().rounded(widgets::ROUND),
                ..button::Style::default()
            });
            if ready {
                pill.on_press(App::SetDiscord(next)).into()
            } else {
                pill.into()
            }
        }))
        .spacing(8)
        .into()
    };
    let status = pills(
        StatusKind::ALL
            .into_iter()
            .map(|kind| {
                (
                    kind.title().to_owned(),
                    settings.status == kind,
                    Settings {
                        status: kind,
                        ..settings
                    },
                    true,
                )
            })
            .collect(),
    );
    let app_name = pills(
        AppName::ALL
            .into_iter()
            .map(|app| {
                (
                    app.title().to_owned(),
                    settings.app == app,
                    Settings { app, ..settings },
                    app.available(),
                )
            })
            .collect(),
    );
    let lang = pills(
        Lang::ALL
            .into_iter()
            .map(|lang| {
                (
                    lang.title().to_owned(),
                    settings.lang == lang,
                    Settings { lang, ..settings },
                    true,
                )
            })
            .collect(),
    );
    let mut entries = Vec::new();
    if !ready {
        entries.push(Entry::new(
            "Application ID Discord",
            "Не задан Application ID Discord",
            "",
            container(theme::line(
                "Не задан Application ID Discord",
                Style::Caption,
                theme::TEXT_TERTIARY,
            ))
            .padding(Padding::from([12, 0]))
            .into(),
        ));
    }
    entries.extend([
        item(
            "Показывать статус в Discord",
            "Что вы слушаете, на странице профиля".to_owned(),
            "дискорд rpc presence",
            toggle(settings.enabled, |s, on| Settings { enabled: on, ..s }),
        ),
        item(
            "Имя в статусе",
            "Заголовок «Слушает …» в карточке профиля".to_owned(),
            "название приложения",
            app_name,
        ),
        item(
            "Что показывать в строке статуса",
            "«Слушает …» под вашим именем".to_owned(),
            "исполнитель название трека",
            status,
        ),
        item(
            "Показывать на паузе",
            "Без таймера, с пометкой «пауза». Иначе на паузе статус убирается".to_owned(),
            "пауза таймер",
            toggle(settings.on_pause, |s, on| Settings { on_pause: on, ..s }),
        ),
        item(
            "Кнопка «Слушать»",
            "Ссылка на трек в Яндекс Музыке".to_owned(),
            "ссылка",
            toggle(settings.button, |s, on| Settings { button: on, ..s }),
        ),
        item(
            "Язык статуса",
            "Подпись кнопки и пометка паузы".to_owned(),
            "русский английский",
            lang,
        ),
    ]);
    entries
}

// ---------------------------------------------------------------- аккаунт

/// «Аккаунт»: имя, подписка, выход.
fn account<'a>(ctx: &Ctx<'a>) -> Vec<Entry<'a>> {
    let status = ctx.library.status();
    let account = status.account();
    let name = account.name();
    let initial = name
        .chars()
        .next()
        .map(|ch| ch.to_uppercase().to_string())
        .unwrap_or_default();
    let plus = match status.subscription() {
        api::Subscription::Active(details) => {
            let mut text = "Подписка Плюс активна".to_owned();
            if details.family {
                text.push_str(", семейная");
            }
            if let Some(until) = details.until {
                text.push_str(&format!(", до {until}"));
            }
            text
        }
        _ => "Подписка Плюс не подтверждена".to_owned(),
    };
    let view = row![
        avatar(initial, 44.0, ctx.library.avatar(), ctx.pixel_scale),
        column![
            theme::line(name.clone(), Style::BodyStrong, theme::TEXT_PRIMARY),
            theme::line(plus.clone(), Style::Secondary, theme::accent()),
        ]
        .spacing(2)
        .width(Length::Fill),
        widgets::action(
            Kind::Danger,
            Some(Icon::Logout),
            "Выйти",
            Some(App::OpenDialog(Dialog::ConfirmLogout)),
        ),
    ]
    .spacing(14)
    .align_y(Alignment::Center)
    .padding(Padding::from([16, 0]));
    vec![Entry::new(
        Cow::Owned(name),
        plus,
        "профиль подписка плюс выйти выход",
        view.into(),
    )]
}

// ------------------------------------------------------------ о программе

/// «О программе»: версия, обязательная строка правила 4, лицензии.
fn about<'a>() -> Vec<Entry<'a>> {
    const VERSION: &str = concat!("Версия ", env!("CARGO_PKG_VERSION"));
    const STREAMING: &str = "Музыка воспроизводится потоком через API Яндекс Музыки по вашей \
                             подписке. Аудио не сохраняется на диск.";
    let product = row![
        widgets::logo(44.0),
        column![
            theme::line(crate::APP_NAME, Style::Headline, theme::TEXT_PRIMARY),
            theme::line(VERSION, Style::Secondary, theme::TEXT_TERTIARY),
        ]
        .spacing(2)
        .width(Length::Fill),
        widgets::action(
            Kind::Secondary,
            None,
            "Сторонние лицензии",
            Some(App::OpenDialog(Dialog::Licenses)),
        ),
    ]
    .spacing(14)
    .align_y(Alignment::Center)
    .padding(Padding::from([16, 0]));
    let notice = column![
        theme::label(crate::DISCLAIMER, Style::BodyStrong, theme::TEXT_PRIMARY),
        theme::label(STREAMING, Style::Secondary, theme::TEXT_SECONDARY),
    ]
    .spacing(14)
    .padding(Padding::from([16, 0]));
    vec![
        Entry::new(
            crate::APP_NAME,
            VERSION,
            "версия лицензии лицензия",
            product.into(),
        ),
        Entry::new(
            "Неофициальный клиент",
            format!("{} {STREAMING}", crate::DISCLAIMER),
            "яндекс авторство поток диск",
            notice.into(),
        ),
    ]
}
