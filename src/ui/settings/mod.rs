//! Экран «Настройки»: слева разделы с поиском и кнопкой «Сбросить всё»,
//! справа страница выбранного раздела из строк с разделителями. Состояние
//! экрана (раздел, запрос) живёт в окне и в `config.toml` не пишется.

pub mod controls;
mod eq_bands;
mod eq_page;
mod pages;

use std::borrow::Cow;

use iced::widget::{Space, button, column, container, row, scrollable, stack};
use iced::{Alignment, Border, Element, Length, Padding, Task};

use super::Message as App;
use super::icons::{Icon, icon};
use super::library::{Library, PAGE_SIDE, PAGE_TOP};
use super::palette;
use super::scroll;
use super::theme::{self, Style};
use super::widgets;
use super::{Dialog, Prefs};
use crate::discord;
use crate::player::eq::EqState;
use controls::Field;
pub use eq_page::{Msg as EqMsg, update as update_eq};
pub use pages::sound_options;

/// Прокручиваемая страница и поле поиска: для прокрутки в начало и фокуса.
const PAGE_ID: &str = "settings-page";
const SEARCH_ID: &str = "settings-search";
/// Ширина страницы, навигации (обычной и узкой) и промежуток между ними.
const PAGE_MAX: f32 = 920.0;
const NAV_WIDTH: f32 = 280.0;
const NAV_WIDTH_NARROW: f32 = 220.0;
const NAV_GAP: f32 = 32.0;
/// Окно уже этого — навигация 220, ещё уже — лента «таблеток» сверху.
const NARROW_BELOW: f32 = 900.0;
const RIBBON_BELOW: f32 = 720.0;
/// Высота пункта навигации и кнопки «Сбросить всё».
const NAV_ITEM: f32 = 44.0;
/// Поля страницы в режиме ленты.
const RIBBON_SIDE: f32 = 20.0;

/// Группа разделов в навигации.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Group {
    Main,
    Player,
    Extensions,
    Network,
    System,
}

impl Group {
    fn title(self) -> &'static str {
        match self {
            Self::Main => "Основное",
            Self::Player => "Плеер",
            Self::Extensions => "Расширения",
            Self::Network => "Сеть",
            Self::System => "Система",
        }
    }
}

/// Раздел настроек.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Appearance,
    Sound,
    Equalizer,
    Lyrics,
    Plugins,
    Discord,
    Together,
    Startup,
    Account,
    About,
}

impl Category {
    /// Разделы в порядке навигации.
    pub const ALL: [Self; 10] = [
        Self::Appearance,
        Self::Sound,
        Self::Equalizer,
        Self::Lyrics,
        Self::Plugins,
        Self::Discord,
        Self::Together,
        Self::Startup,
        Self::Account,
        Self::About,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::Appearance => "Оформление",
            Self::Sound => "Звук",
            Self::Equalizer => "Эквалайзер",
            Self::Lyrics => "Текст песни",
            Self::Plugins => "Плагины",
            Self::Discord => "Discord",
            Self::Together => "Слушать вместе",
            Self::Startup => "Запуск и трей",
            Self::Account => "Аккаунт",
            Self::About => "О программе",
        }
    }

    /// Пояснение под названием раздела в шапке страницы.
    pub fn summary(self) -> &'static str {
        match self {
            Self::Appearance => "Акцент, масштаб и визуализатор",
            Self::Sound => "Качество потока и громкость",
            Self::Equalizer => "Десять полос, пресеты и предусилитель",
            Self::Lyrics => "Откуда брать текст",
            Self::Plugins => "Встроенные модули, по умолчанию выключены",
            Self::Discord => "Статус «Слушает» в профиле Discord",
            Self::Together => "Один трек на двух ваших устройствах в одной сети",
            Self::Startup => "Сворачивание в трей и автозапуск",
            Self::Account => "Профиль и подписка",
            Self::About => "Версия, лицензии, авторство",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Self::Appearance => Icon::Palette,
            Self::Sound => Icon::Volume,
            Self::Equalizer => Icon::Equalizer,
            Self::Lyrics => Icon::Lyrics,
            Self::Plugins => Icon::Plug,
            Self::Discord => Icon::Chat,
            Self::Together => Icon::Devices,
            Self::Startup => Icon::MiniPlayer,
            Self::Account => Icon::Artist,
            Self::About => Icon::About,
        }
    }

    fn group(self) -> Group {
        match self {
            Self::Appearance => Group::Main,
            Self::Sound | Self::Equalizer | Self::Lyrics => Group::Player,
            Self::Plugins => Group::Extensions,
            Self::Discord | Self::Together => Group::Network,
            Self::Startup | Self::Account | Self::About => Group::System,
        }
    }
}

/// Состояние экрана: открытый раздел и запрос поиска.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub category: Category,
    pub query: String,
    /// Поле имени своего пресета эквалайзера.
    pub eq_name: eq_page::NameField,
    /// Обновление программы («О программе»).
    pub update: super::updates::State,
    /// Значок в трее создан (иначе строки «в трей» неактивны).
    pub tray: bool,
    /// Автозапуск включён в системе (в `config.toml` не пишется).
    pub autostart: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            category: Category::Appearance,
            query: String::new(),
            eq_name: eq_page::NameField::default(),
            update: super::updates::State::default(),
            tray: false,
            autostart: false,
        }
    }
}

/// Сообщения экрана настроек.
#[derive(Debug, Clone)]
pub enum Msg {
    /// Открыть раздел (и очистить запрос).
    Open(Category),
    Query(String),
    ClearQuery,
    /// Подтверждён диалог «Сбросить настройки?»: обрабатывает окно.
    ResetAll,
    /// Страница эквалайзера: разбирает окно (там состояние и плеер).
    Eq(eq_page::Msg),
}

impl State {
    /// Применяет сообщение экрана. `ResetAll` окно разбирает само.
    pub fn update(&mut self, msg: Msg) -> Task<App> {
        match msg {
            Msg::Open(category) => {
                self.category = category;
                self.query.clear();
                scroll_to_top()
            }
            Msg::Query(query) => {
                self.query = query;
                scroll_to_top()
            }
            Msg::ClearQuery => {
                self.query.clear();
                scroll_to_top()
            }
            Msg::ResetAll | Msg::Eq(_) => Task::none(),
        }
    }

    /// Фокус на поле имени пресета (после «Сохранить как…»).
    pub fn focus_eq_name() -> Task<App> {
        iced::widget::operation::focus(eq_page::NAME_ID)
    }

    /// Esc в поле поиска: очищает запрос. `false` — запрос и так пуст.
    pub fn clear_query(&mut self) -> bool {
        if self.query.is_empty() {
            return false;
        }
        self.query.clear();
        true
    }
}

fn scroll_to_top() -> Task<App> {
    iced::widget::operation::scroll_to(
        PAGE_ID,
        scrollable::AbsoluteOffset {
            x: None,
            y: Some(0.0),
        },
    )
}

/// Запись страницы: одна строка настройки (или блок строк) и то, по чему её
/// ищут.
pub struct Entry<'a> {
    /// Заголовок строки.
    pub title: Cow<'a, str>,
    /// Пояснение.
    pub hint: Cow<'a, str>,
    /// Дополнительные слова для поиска через пробел.
    pub keywords: &'static str,
    /// Готовая строка.
    pub view: Element<'a, App>,
}

impl<'a> Entry<'a> {
    pub fn new(
        title: impl Into<Cow<'a, str>>,
        hint: impl Into<Cow<'a, str>>,
        keywords: &'static str,
        view: Element<'a, App>,
    ) -> Self {
        Self {
            title: title.into(),
            hint: hint.into(),
            keywords,
            view,
        }
    }

    /// Подходит ли запись под запрос (см. [`matches`]); `category` — название
    /// раздела тоже участвует в поиске.
    fn found_by(&self, query: &str, category: Category) -> bool {
        matches(
            query,
            &[&self.title, &self.hint, self.keywords, category.title()],
        )
    }
}

/// Всё, что нужно страницам: окно главного экрана, настройки, палитра,
/// эквалайзер и ширина окна в единицах раскладки.
pub struct Ctx<'a> {
    pub library: &'a Library,
    pub prefs: Prefs,
    pub palette: &'a palette::State,
    pub eq: &'a EqState,
    pub eq_name: &'a eq_page::NameField,
    pub width: f32,
    /// Физических пикселей в логическом (для аватара).
    pub pixel_scale: f32,
    /// Состояние обновления программы.
    pub update: &'a super::updates::State,
    /// Значок в трее создан.
    pub tray: bool,
    /// Автозапуск включён в системе.
    pub autostart: bool,
}

// ------------------------------------------------------------------ поиск

/// Приведение текста к виду для сравнения: без регистра, «ё» = «е».
fn fold(text: &str) -> String {
    text.to_lowercase().replace('ё', "е")
}

/// Ищут ли сейчас: запрос из одних пробелов — это пустой запрос.
pub fn is_searching(query: &str) -> bool {
    !query.trim().is_empty()
}

/// Все слова запроса входят в один из `parts` (без учёта регистра, «ё» = «е»).
/// Пустой запрос подходит всему.
pub fn matches(query: &str, parts: &[&str]) -> bool {
    let haystack = parts
        .iter()
        .map(|part| fold(part))
        .collect::<Vec<_>>()
        .join(" ");
    fold(query)
        .split_whitespace()
        .all(|word| haystack.contains(word))
}

// ------------------------------------------------------------------ сброс

/// Эквалайзер после «Сбросить всё»: параметры и выбранный пресет по
/// умолчанию, свои пресеты остаются.
pub fn reset_eq(current: &EqState) -> EqState {
    EqState {
        custom: current.custom.clone(),
        ..EqState::default()
    }
}

/// Настройки после «Сбросить всё»: всё по умолчанию, кроме вида меню.
/// `discord_ready` пересчитывается по `discord_app_id` из `config.toml`.
pub fn reset_prefs(current: &Prefs, discord_override: Option<&str>) -> Prefs {
    Prefs {
        sidebar_collapsed: current.sidebar_collapsed,
        discord_ready: !discord::app_id(discord_override, discord::AppName::default()).is_empty(),
        ..Prefs::default()
    }
}

// ------------------------------------------------------------------- вид

/// Экран настроек. `width` — ширина окна в единицах раскладки.
pub fn view<'a>(ctx: Ctx<'a>, state: &'a State, width: f32) -> Element<'a, App> {
    let body = page(&ctx, state);
    if width < RIBBON_BELOW {
        return ribbon_layout(body, state);
    }
    let nav_width = if width < NARROW_BELOW {
        NAV_WIDTH_NARROW
    } else {
        NAV_WIDTH
    };
    let page = scroll::smooth(
        scrollable(
            container(body)
                .width(Length::Fill)
                .max_width(PAGE_MAX)
                .padding(Padding {
                    top: PAGE_TOP,
                    right: PAGE_SIDE,
                    bottom: PAGE_TOP,
                    left: 0.0,
                }),
        )
        .id(PAGE_ID)
        .width(Length::Fill)
        .height(Length::Fill),
    );
    row![navigation(state, nav_width), page]
        .spacing(NAV_GAP)
        .height(Length::Fill)
        .into()
}

/// Узкое окно: название, поиск и лента разделов сверху, страница под ними,
/// «Сбросить всё» в конце страницы.
fn ribbon_layout<'a>(body: Element<'a, App>, state: &'a State) -> Element<'a, App> {
    let top = column![
        theme::line("Настройки", Style::Display, theme::text_primary()),
        search_field(state),
        ribbon(state),
    ]
    .spacing(12)
    .padding(Padding {
        top: PAGE_TOP,
        right: RIBBON_SIDE,
        bottom: 0.0,
        left: RIBBON_SIDE,
    });
    let page = scroll::smooth(
        scrollable(
            container(column![body, Space::new().height(24), reset_button()])
                .width(Length::Fill)
                .max_width(PAGE_MAX)
                .padding(Padding {
                    top: 12.0,
                    right: RIBBON_SIDE,
                    bottom: PAGE_TOP,
                    left: RIBBON_SIDE,
                }),
        )
        .id(PAGE_ID)
        .width(Length::Fill)
        .height(Length::Fill),
    );
    column![top, page].height(Length::Fill).into()
}

/// Левая колонка: название, поиск, разделы по группам и «Сбросить всё».
fn navigation<'a>(state: &'a State, width: f32) -> Element<'a, App> {
    let mut groups = column![].width(Length::Fill);
    let mut last = None;
    for category in Category::ALL {
        let group = category.group();
        if last != Some(group) {
            last = Some(group);
            groups = groups.push(
                container(theme::line(
                    group.title().to_uppercase(),
                    Style::Caption,
                    theme::text_tertiary(),
                ))
                .padding(Padding {
                    top: 20.0,
                    right: 0.0,
                    bottom: 6.0,
                    left: 14.0,
                }),
            );
        }
        groups = groups.push(nav_item(category, state.category == category));
    }
    let footer = column![
        Space::new().height(12),
        widgets::divider(),
        Space::new().height(12),
        reset_button(),
    ];
    column![
        theme::line("Настройки", Style::Display, theme::text_primary()),
        Space::new().height(16),
        search_field(state),
        scroll::smooth(scrollable(groups).height(Length::Fill)),
        footer,
    ]
    .width(width)
    .height(Length::Fill)
    .padding(Padding {
        top: PAGE_TOP,
        right: 0.0,
        bottom: PAGE_TOP,
        left: PAGE_SIDE,
    })
    .into()
}

fn search_field(state: &State) -> Element<'_, App> {
    controls::text_field(
        Field {
            id: Some(SEARCH_ID),
            placeholder: "Найти настройку",
            value: &state.query,
            icon: Some(Icon::Search),
            width: Length::Fill,
        },
        |query| App::Settings(Msg::Query(query)),
        Some(App::Settings(Msg::ClearQuery)),
    )
}

/// Пункт навигации. Выбранный — с фоном, иконкой акцента и полоской слева.
fn nav_item<'a>(category: Category, selected: bool) -> Element<'a, App> {
    let (icon_color, style, color) = if selected {
        (theme::accent(), Style::BodyStrong, theme::text_primary())
    } else {
        (
            theme::text_secondary(),
            Style::Body,
            theme::text_secondary(),
        )
    };
    let content = row![
        icon(category.icon(), 18.0, icon_color),
        theme::line(category.title(), style, color),
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    let item = button(container(content).center_y(Length::Fill))
        .width(Length::Fill)
        .height(NAV_ITEM)
        .padding([0, 14])
        .style(move |_, status| button::Style {
            background: match (selected, status) {
                (true, _) => Some(theme::surface_2().into()),
                (false, button::Status::Hovered | button::Status::Pressed) => {
                    Some(theme::surface_1().into())
                }
                _ => None,
            },
            border: Border::default().rounded(12),
            ..button::Style::default()
        })
        .on_press(App::Settings(Msg::Open(category)));
    if !selected {
        return item.into();
    }
    let mark = container(widgets::block(3, 20, theme::accent(), 2.0))
        .height(NAV_ITEM)
        .center_y(NAV_ITEM);
    stack![item, mark].width(Length::Fill).into()
}

/// Лента разделов для узкого окна: «таблетки» с горизонтальной прокруткой.
fn ribbon(state: &State) -> Element<'_, App> {
    let pills = row(Category::ALL.into_iter().map(|category| {
        let selected = state.category == category;
        let (icon_color, color) = if selected {
            (theme::accent(), theme::text_primary())
        } else {
            (theme::text_secondary(), theme::text_secondary())
        };
        button(
            row![
                icon(category.icon(), 16.0, icon_color),
                theme::line(category.title(), Style::BodyStrong, color),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .padding([10, 16])
        .style(move |_, status| button::Style {
            background: Some(
                match (selected, status) {
                    (true, _) => theme::surface_3(),
                    (false, button::Status::Hovered | button::Status::Pressed) => {
                        theme::surface_2()
                    }
                    _ => theme::surface_1(),
                }
                .into(),
            ),
            border: Border::default().rounded(widgets::ROUND),
            ..button::Style::default()
        })
        .on_press(App::Settings(Msg::Open(category)))
        .into()
    }))
    .spacing(8);
    scrollable(container(pills).padding(Padding::default().bottom(8)))
        .direction(scrollable::Direction::Horizontal(
            scrollable::Scrollbar::new().width(4).scroller_width(4),
        ))
        .into()
}

/// «Сбросить всё»: рамка и текст цвета ошибки, диалог подтверждения.
fn reset_button<'a>() -> Element<'a, App> {
    button(
        container(theme::line(
            "Сбросить всё",
            Style::BodyStrong,
            theme::error(),
        ))
        .center(Length::Fill),
    )
    .width(Length::Fill)
    .height(NAV_ITEM)
    .padding(0)
    .style(|_, status| button::Style {
        background: match status {
            button::Status::Hovered | button::Status::Pressed => Some(theme::error_soft().into()),
            _ => None,
        },
        border: Border {
            color: theme::error(),
            width: 1.0,
            radius: 12.0.into(),
        },
        ..button::Style::default()
    })
    .on_press(App::OpenDialog(Dialog::ConfirmResetSettings))
    .into()
}

/// Страница: раздел или, при запросе, найденные записи всех разделов.
fn page<'a>(ctx: &Ctx<'a>, state: &'a State) -> Element<'a, App> {
    if is_searching(&state.query) {
        return search_results(ctx, &state.query);
    }
    let category = state.category;
    column![
        controls::header(category.title(), category.summary()),
        with_dividers(pages::entries(category, ctx)),
    ]
    .into()
}

/// Строки одна под другой, между ними разделители.
fn with_dividers<'a>(entries: Vec<Entry<'a>>) -> Element<'a, App> {
    let mut rows = column![];
    for (index, entry) in entries.into_iter().enumerate() {
        if index > 0 {
            rows = rows.push(widgets::divider());
        }
        rows = rows.push(entry.view);
    }
    rows.into()
}

/// Найденные записи, сгруппированные по разделам; название раздела — ссылка.
fn search_results<'a>(ctx: &Ctx<'a>, query: &str) -> Element<'a, App> {
    let mut found = column![];
    let mut count = 0usize;
    for category in Category::ALL {
        let entries: Vec<Entry<'a>> = pages::entries(category, ctx)
            .into_iter()
            .filter(|entry| entry.found_by(query, category))
            .collect();
        if entries.is_empty() {
            continue;
        }
        count += entries.len();
        let heading = button(theme::line(
            category.title(),
            Style::Headline,
            theme::text_primary(),
        ))
        .padding(Padding::default().top(24).bottom(4))
        .style(|_, status| button::Style {
            text_color: if status == button::Status::Hovered {
                theme::accent_hover()
            } else {
                theme::text_primary()
            },
            ..button::Style::default()
        })
        .on_press(App::Settings(Msg::Open(category)));
        found = found.push(heading).push(widgets::divider());
        found = found.push(with_dividers(entries));
    }
    if count == 0 {
        return container(
            column![
                theme::line("Ничего не нашлось", Style::Headline, theme::text_primary()),
                theme::label(
                    "Проверьте запрос или попробуйте другое слово",
                    Style::Secondary,
                    theme::text_tertiary(),
                ),
            ]
            .spacing(6)
            .align_x(Alignment::Center),
        )
        .center_x(Length::Fill)
        .padding(Padding::default().top(48))
        .into();
    }
    found.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::AudioQuality;

    #[test]
    fn empty_query_matches_nothing_in_particular() {
        assert!(!is_searching(""));
        assert!(!is_searching("   \t "));
        assert!(is_searching(" а "));
        // Пустой запрос подходит любой записи: страница просто не фильтруется.
        assert!(matches("", &["Акцент"]));
        assert!(matches("   ", &["Акцент"]));
    }

    #[test]
    fn ignores_case() {
        assert!(matches("АКЦЕНТ", &["Акцент"]));
        assert!(matches("discord", &["Показывать статус в Discord"]));
        assert!(matches("DISCORD", &["показывать статус в discord"]));
    }

    #[test]
    fn yo_equals_ye() {
        assert!(matches("елка", &["Ёлка"]));
        assert!(matches("ёлка", &["елка"]));
        assert!(matches("ЕЛКА", &["ёлка"]));
    }

    #[test]
    fn every_word_must_be_found() {
        let parts = ["Масштаб", "Ctrl + = меняет масштаб", "размер zoom"];
        assert!(matches("масштаб zoom", &parts));
        assert!(matches("  zoom   размер ", &parts));
        assert!(!matches("масштаб громкость", &parts));
        // Слова могут быть в разных частях записи.
        assert!(matches("ctrl размер", &parts));
    }

    #[test]
    fn category_title_is_searchable() {
        let found = Category::Discord.title();
        assert!(matches(
            "discord статус",
            &["Показывать статус", "", "", found]
        ));
        assert!(!matches(
            "плагины статус",
            &["Показывать статус", "", "", found]
        ));
    }

    #[test]
    fn nothing_found() {
        assert!(!matches("эквалайзер", &["Акцент", "цвет"]));
    }

    #[test]
    fn every_category_has_unique_title() {
        for (index, category) in Category::ALL.into_iter().enumerate() {
            for other in Category::ALL.into_iter().skip(index + 1) {
                assert_ne!(category.title(), other.title());
            }
        }
    }

    #[test]
    fn reset_restores_defaults_but_keeps_sidebar() {
        let current = Prefs {
            sidebar_collapsed: true,
            accent: theme::Accent::Blue,
            theme: theme::Family::Midnight,
            theme_mode: theme::ThemeMode::Light,
            scale: 150,
            visualizer: false,
            auto_update: true,
            close_to_tray: false,
            quality: AudioQuality::Low,
            normalize: true,
            lyrics_lrclib: false,
            discord: discord::Settings {
                enabled: false,
                ..discord::Settings::default()
            },
            discord_ready: false,
        };
        let reset = reset_prefs(&current, None);
        let expected = Prefs {
            sidebar_collapsed: true,
            discord_ready: !discord::app_id(None, discord::AppName::default()).is_empty(),
            ..Prefs::default()
        };
        assert_eq!(reset, expected);
        // Вид меню не сбрасывается и в обратную сторону.
        let open = Prefs {
            sidebar_collapsed: false,
            ..current
        };
        assert!(!reset_prefs(&open, None).sidebar_collapsed);
    }

    #[test]
    fn reset_recomputes_discord_ready_from_config() {
        // Свой Application ID из конфига не теряется при сбросе.
        let current = Prefs::default();
        let with_id = reset_prefs(&current, Some("1234567890"));
        assert_eq!(
            with_id.discord_ready,
            !discord::app_id(Some("1234567890"), discord::AppName::default()).is_empty()
        );
    }

    #[test]
    fn reset_eq_keeps_custom_presets_only() {
        use crate::player::eq::{BANDS, PresetId};
        let mut current = EqState::default();
        current.settings.enabled = true;
        current.settings.auto_preamp = false;
        current.settings.preamp = -30;
        current.settings.gains = [40; BANDS];
        assert!(current.add_custom("Мой", -10, [20; BANDS]).is_ok());
        current.preset = Some(PresetId::Custom("Мой".to_owned()));
        let reset = reset_eq(&current);
        assert_eq!(reset.settings, EqState::default().settings);
        assert_eq!(reset.preset, None);
        assert_eq!(reset.custom, current.custom);
        assert_eq!(reset.custom.len(), 1);
    }

    #[test]
    fn state_clears_query() {
        let mut state = State {
            query: "акцент".to_owned(),
            ..State::default()
        };
        assert!(state.clear_query());
        assert!(state.query.is_empty());
        assert!(!state.clear_query());
    }
}
