//! Тексты песен в окне: панель 360 px справа и место в полноэкранном плеере.
//! Загрузка ленивая: текст запрашивается, только пока открыта панель или
//! полноэкранный плеер. Ответ про другой трек (или с другим переключателем
//! LRCLib) игнорируется. Сам поиск и кэш — модуль `crate::lyrics`.

use std::hash::{Hash, Hasher};

use iced::animation::Animation;
use iced::time::Instant;
use iced::widget::{Space, center, column, container, row};
use iced::{Alignment, Element, Length, Task};

use super::super::anim;
use super::super::covers::Covers;
use super::super::icons::{Icon, icon, spinner};
use super::super::lyrics_view::{Anchor, Colors, lyrics_view};
use super::super::theme::{self, Style};
use super::super::widgets::{self, Kind, Size};
use super::{App, Effect, Lib, Library, Message};
use crate::lyrics::{self, Lyrics, LyricsError, Outcome, Request};

/// Ширина панели.
pub const PANEL_WIDTH: f32 = 360.0;
/// Через сколько после открытия панели очереди её уже можно прокручивать.
const QUEUE_SCROLL_AFTER: std::time::Duration = std::time::Duration::from_millis(120);
/// Боковой отступ текста в панели.
const PAD: f32 = 24.0;

/// Для какого трека и с какими настройками загружен текст.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Key {
    /// Настоящий трек очереди (по нему текст Яндекса и кэш).
    track: String,
    /// Показанный трек: у замены FckCensor другие название и исполнитель
    /// (по ним ищет LRCLib).
    shown: String,
    lrclib: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Load {
    Idle,
    Loading(Key),
    Ready(Key, Outcome),
    Failed(Key, String),
}

impl Load {
    fn key(&self) -> Option<&Key> {
        match self {
            Self::Idle => None,
            Self::Loading(key) | Self::Ready(key, _) | Self::Failed(key, _) => Some(key),
        }
    }
}

/// Что показывает боковая панель справа: она одна, поэтому текст и очередь
/// не бывают открыты вместе.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Lyrics,
    Queue,
}

/// Кнопка в плеере: панель открыта или нет и что в ней. Кнопка той же
/// панели закрывает её, кнопка другой переключает содержимое (панель остаётся
/// открытой).
pub fn next_side(open: bool, current: Side, requested: Side) -> (bool, Side) {
    if open && current == requested {
        (false, current)
    } else {
        (true, requested)
    }
}

/// Сообщения боковой панели: текст песни и очередь.
#[derive(Clone)]
pub enum Msg {
    /// Кнопка «Текст» в плеере: открыть или закрыть панель текста.
    Toggle,
    /// Кнопка «Очередь» в плеере: открыть или закрыть панель очереди.
    ToggleQueue,
    Close,
    /// «Повторить» после ошибки.
    Retry,
    Loaded(Key, Result<Outcome, LyricsError>),
}

/// Состояние текстов.
pub struct State {
    /// Панель справа: открыта или закрыта (плавно).
    pub panel: Animation<bool>,
    /// Что в панели: текст песни или очередь.
    pub side: Side,
    load: Load,
    /// Искать в LRCLib, если у Яндекса текста нет (настройка).
    lrclib: bool,
    /// Панель «Очередь» открыли в этот момент: когда она появится на экране,
    /// её надо прокрутить к «Сейчас играет».
    queue_scroll: Option<Instant>,
}

impl State {
    pub fn new() -> Self {
        Self {
            panel: anim::settled(false, anim::PANEL),
            side: Side::Lyrics,
            queue_scroll: None,
            load: Load::Idle,
            lrclib: true,
        }
    }

    /// Нужно ли начинать загрузку для `key`: текст кому-то виден и он ещё не
    /// запрошен (идёт, готов или упал с этой же настройкой).
    fn needs_load(&self, wanted: bool, key: Option<&Key>) -> bool {
        wanted && key.is_some_and(|key| self.load.key() != Some(key))
    }

    /// Принимает ответ, только если ждали именно его: ответ про другой трек
    /// или со старой настройкой устарел. `true` — принят.
    fn accept(&mut self, key: &Key, result: Result<Outcome, LyricsError>) -> bool {
        if !matches!(&self.load, Load::Loading(waiting) if waiting == key) {
            return false;
        }
        self.load = match result {
            Ok(outcome) => Load::Ready(key.clone(), outcome),
            Err(error) => Load::Failed(key.clone(), error.to_string()),
        };
        true
    }

    /// Состояние для `key`; другой ключ — как «ещё грузится».
    fn view_for(&self, key: &Key) -> Option<&Load> {
        self.load.key().filter(|loaded| *loaded == key)?;
        Some(&self.load)
    }
}

impl Library {
    /// Настройка «искать в LRCLib» из `config.toml`.
    pub fn set_lyrics_lrclib(&mut self, on: bool) {
        self.lyrics.lrclib = on;
    }

    /// Панель текста открыта (или открывается).
    pub fn lyrics_open(&self) -> bool {
        self.lyrics.panel.value() && self.lyrics.side == Side::Lyrics
    }

    /// Панель очереди открыта (или открывается).
    pub fn queue_open(&self) -> bool {
        self.lyrics.panel.value() && self.lyrics.side == Side::Queue
    }

    /// Esc: закрыть боковую панель. `false` — она и не была открыта.
    pub fn close_lyrics(&mut self) -> bool {
        let open = self.lyrics.panel.value();
        if open {
            self.lyrics.panel.go_mut(false, Instant::now());
        }
        open
    }

    /// Идёт ли движение панели: ей нужны кадры.
    pub fn lyrics_transitioning(&self, now: Instant) -> bool {
        self.lyrics.panel.is_animating(now)
    }

    /// Крутится спиннер загрузки текста.
    pub fn lyrics_loading(&self) -> bool {
        (self.lyrics_open() || self.fullscreen.value())
            && match self.lyrics_key() {
                Some(key) => !matches!(
                    self.lyrics.view_for(&key),
                    Some(Load::Ready(..) | Load::Failed(..))
                ),
                None => false,
            }
    }

    fn lyrics_key(&self) -> Option<Key> {
        let current = self.current()?;
        let shown = self.current_shown()?;
        // У недоступного трека с копией текст Яндекса — копии.
        let (track, _) = self.lyrics_source(current);
        Some(Key {
            track,
            shown: shown.id.clone(),
            lrclib: self.lyrics.lrclib,
        })
    }

    fn lyrics_request(&self) -> Option<Request> {
        let current = self.current()?;
        let shown = self.current_shown()?;
        let (track_id, hint) = self.lyrics_source(current);
        Some(Request {
            track_id,
            title: shown.title.clone().unwrap_or_default(),
            // LRCLib сверяет одного исполнителя: берём первого.
            artist: shown
                .artists
                .first()
                .map(|artist| artist.name.clone())
                .unwrap_or_default(),
            album: shown.album_title.clone(),
            duration: shown.duration,
            hint,
        })
    }

    /// Запускает загрузку текста, если он виден и ещё не запрошен. Зовётся
    /// после каждого сообщения: смена трека при открытой панели сама
    /// запрашивает новый текст.
    pub fn lyrics_sync(&mut self) -> Task<Message> {
        Task::batch([self.lyrics_load(), self.queue_scroll_task()])
    }

    /// Прокрутка очереди к «Сейчас играет»: когда панель уже построена (через
    /// пару кадров после открытия).
    fn queue_scroll_task(&mut self) -> Task<Message> {
        let Some(opened) = self.lyrics.queue_scroll else {
            return Task::none();
        };
        if !self.queue_open() {
            self.lyrics.queue_scroll = None;
            return Task::none();
        }
        if Instant::now().saturating_duration_since(opened) < QUEUE_SCROLL_AFTER {
            return Task::none();
        }
        self.lyrics.queue_scroll = None;
        self.queue_scroll_to_current()
    }

    fn lyrics_load(&mut self) -> Task<Message> {
        let wanted = self.lyrics_open() || self.fullscreen.value();
        let (Some(key), Some(request)) = (self.lyrics_key(), self.lyrics_request()) else {
            return Task::none();
        };
        if !self.lyrics.needs_load(wanted, Some(&key)) {
            return Task::none();
        }
        let Some(http) = self.http.clone() else {
            self.lyrics.load =
                Load::Failed(key, "Не удалось подготовить сетевое соединение".to_owned());
            return Task::none();
        };
        self.lyrics.load = Load::Loading(key.clone());
        let token = self.token.clone();
        let lrclib = key.lrclib;
        Task::perform(
            async move { lyrics::load(&http, &token.access_token, &request, lrclib).await },
            move |result| Message::Lyrics(Msg::Loaded(key.clone(), result)),
        )
    }

    pub(super) fn lyrics_msg(&mut self, msg: Msg) -> (Task<Message>, Option<Effect>) {
        match msg {
            Msg::Toggle => self.toggle_side(Side::Lyrics),
            Msg::ToggleQueue => self.toggle_side(Side::Queue),
            Msg::Close => {
                self.close_lyrics();
            }
            Msg::Retry => self.lyrics.load = Load::Idle,
            Msg::Loaded(key, result) => {
                let unauthorized = matches!(result, Err(LyricsError::Unauthorized));
                if self.lyrics.accept(&key, result) && unauthorized {
                    return (Task::none(), Some(Effect::Relogin));
                }
            }
        }
        (Task::none(), None)
    }

    /// Открывает, закрывает или переключает боковую панель.
    fn toggle_side(&mut self, requested: Side) {
        let (open, side) = next_side(self.lyrics.panel.value(), self.lyrics.side, requested);
        // Без трека открывать нечего.
        if !open || self.current().is_some() {
            self.lyrics.side = side;
            if open != self.lyrics.panel.value() {
                self.lyrics.panel.go_mut(open, Instant::now());
            }
            self.lyrics.queue_scroll = (open && side == Side::Queue).then(Instant::now);
        }
    }

    /// Последняя известная позиция для виджета текста: он сам ведёт её вперёд
    /// по часам. Пока ползунок тянут, позиция стоит.
    fn lyrics_anchor(&self) -> Anchor {
        let player = &self.player;
        let dragging = player.seeking.is_some();
        let seconds = player.seeking.unwrap_or(player.position).max(0.0);
        Anchor {
            position_ms: (seconds * 1000.0) as u64,
            at: if dragging {
                Instant::now()
            } else {
                player.position_at
            },
            playing: player.playing && player.loaded && !player.buffering && !dragging,
        }
    }

    /// Панель справа. `None` — закрыта.
    pub(super) fn lyrics_panel<'a>(
        &'a self,
        covers: &'a Covers,
        now: Instant,
        turn: f32,
    ) -> Option<Element<'a, App>> {
        let shown = anim::progress(&self.lyrics.panel, now);
        if shown <= 0.0 {
            return None;
        }
        let (title, (body, footer)) = match self.lyrics.side {
            Side::Lyrics => ("Текст песни", self.lyrics_content(Style::Title, turn)),
            Side::Queue => ("Очередь", (self.queue_content(covers), None)),
        };
        let header = row![
            container(theme::line(title, Style::Headline, theme::text_primary()))
                .width(Length::Fill),
            widgets::icon_button(
                Icon::Close,
                Size::S,
                false,
                Some(Lib(Message::Lyrics(Msg::Close)))
            ),
        ]
        .align_y(Alignment::Center)
        .padding([0.0, PAD]);
        let mut inner = column![
            Space::new().height(16),
            header,
            Space::new().height(8),
            body
        ]
        .width(PANEL_WIDTH)
        .height(Length::Fill);
        inner = inner.push(
            container(match footer {
                Some(text) => theme::line(text, Style::Caption, theme::text_tertiary()),
                None => theme::line("", Style::Caption, theme::text_tertiary()),
            })
            .padding([12.0, PAD]),
        );
        Some(
            row![
                widgets::vertical_divider(),
                container(inner)
                    .width(Length::Fixed(PANEL_WIDTH * shown))
                    .height(Length::Fill)
                    .clip(true),
            ]
            .height(Length::Fill)
            .into(),
        )
    }

    /// Текст для полноэкранного плеера.
    pub(super) fn lyrics_fullscreen(&self, turn: f32) -> Element<'_, App> {
        let (body, footer) = self.lyrics_content(Style::Display, turn);
        let mut column = column![body].height(Length::Fill);
        if let Some(text) = footer {
            column = column.push(
                container(theme::line(text, Style::Caption, theme::text_tertiary()))
                    .padding([8.0, 0.0]),
            );
        }
        container(column)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// Содержимое по состоянию и подпись источника внизу. `style` — размер
    /// строк текста.
    fn lyrics_content(&self, style: Style, turn: f32) -> (Element<'_, App>, Option<String>) {
        let Some(key) = self.lyrics_key() else {
            return (
                note(Icon::Lyrics, "Ничего не играет", "Включите трек", None),
                None,
            );
        };
        match self.lyrics.view_for(&key) {
            Some(Load::Ready(_, Outcome::Found(found))) => {
                let footer = footer(found);
                (self.lyrics_lines(found, &key, style), Some(footer))
            }
            Some(Load::Ready(_, Outcome::Instrumental)) => (
                note(Icon::Lyrics, "Инструментал", "В этом треке нет слов", None),
                None,
            ),
            Some(Load::Ready(_, Outcome::Missing)) => (
                note(
                    Icon::Lyrics,
                    "Текста нет",
                    "Для этого трека текст не нашёлся",
                    None,
                ),
                None,
            ),
            Some(Load::Failed(_, text)) => (
                note(
                    Icon::Offline,
                    "Не удалось получить текст",
                    text,
                    Some(widgets::action(
                        Kind::Primary,
                        Some(Icon::Retry),
                        "Повторить",
                        Some(Lib(Message::Lyrics(Msg::Retry))),
                    )),
                ),
                None,
            ),
            // Идёт загрузка (или ответ про прошлый трек): спиннер.
            _ => (
                center(spinner(28.0, theme::text_secondary(), turn)).into(),
                None,
            ),
        }
    }

    fn lyrics_lines<'a>(&'a self, found: &'a Lyrics, key: &Key, style: Style) -> Element<'a, App> {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut hasher);
        let id = hasher.finish();
        let colors = Colors {
            past: theme::text_tertiary(),
            active: theme::text_primary(),
            future: theme::text_secondary(),
        };
        if let Some(lines) = &found.synced {
            let texts: Vec<&str> = lines.iter().map(|line| line.text.as_str()).collect();
            let times: Vec<u64> = lines.iter().map(|line| line.time_ms).collect();
            let seek = times.clone();
            return container(
                lyrics_view(texts, times, Some(self.lyrics_anchor()), style, colors, id).on_press(
                    move |index| {
                        let ms = seek.get(index).copied().unwrap_or(0);
                        Lib(Message::SeekTo(ms as f32 / 1000.0))
                    },
                ),
            )
            .padding([0.0, PAD])
            .into();
        }
        let plain = found.plain.as_deref().unwrap_or_default();
        let texts: Vec<&str> = plain.lines().map(str::trim).collect();
        container(lyrics_view(texts, Vec::new(), None, style, colors, id))
            .padding([0.0, PAD])
            .into()
    }
}

/// Подпись источника внизу.
fn footer(found: &Lyrics) -> String {
    let kind = if found.synced.is_some() {
        "Синхронный текст"
    } else {
        "Текст"
    };
    format!("{kind} · {}", found.source.title())
}

/// Значок, заголовок, пояснение и, если нужно, кнопка — по центру.
pub(super) fn note<'a>(
    name: Icon,
    title: &'a str,
    text: impl iced::widget::text::IntoFragment<'a>,
    action: Option<Element<'a, App>>,
) -> Element<'a, App> {
    let mut content = column![
        icon(name, 40.0, theme::text_tertiary()),
        theme::line(title, Style::Title, theme::text_primary()),
        theme::label(text, Style::Secondary, theme::text_secondary())
            .center()
            .width(260),
    ]
    .spacing(12)
    .align_x(Alignment::Center);
    if let Some(action) = action {
        content = content.push(Space::new().height(4)).push(action);
    }
    center(content).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(track: &str, lrclib: bool) -> Key {
        Key {
            track: track.to_owned(),
            shown: track.to_owned(),
            lrclib,
        }
    }

    #[test]
    fn side_panels_exclude_each_other() {
        // Закрыта: любая кнопка открывает свою панель.
        assert_eq!(
            next_side(false, Side::Lyrics, Side::Queue),
            (true, Side::Queue)
        );
        assert_eq!(
            next_side(false, Side::Queue, Side::Lyrics),
            (true, Side::Lyrics)
        );
        // Открыта своя: кнопка закрывает.
        assert_eq!(
            next_side(true, Side::Lyrics, Side::Lyrics),
            (false, Side::Lyrics)
        );
        assert_eq!(
            next_side(true, Side::Queue, Side::Queue),
            (false, Side::Queue)
        );
        // Открыта чужая: переключается на свою, остаётся открытой.
        assert_eq!(
            next_side(true, Side::Lyrics, Side::Queue),
            (true, Side::Queue)
        );
        assert_eq!(
            next_side(true, Side::Queue, Side::Lyrics),
            (true, Side::Lyrics)
        );
    }

    #[test]
    fn closed_panel_loads_nothing() {
        let state = State::new();
        assert!(!state.needs_load(false, Some(&key("1", true))));
        // Нет трека — грузить нечего.
        assert!(!state.needs_load(true, None));
        // Открыли — грузим.
        assert!(state.needs_load(true, Some(&key("1", true))));
    }

    #[test]
    fn same_key_is_not_loaded_twice() {
        let mut state = State::new();
        let one = key("1", true);
        state.load = Load::Loading(one.clone());
        assert!(!state.needs_load(true, Some(&one)));
        assert!(state.accept(&one, Ok(Outcome::Missing)));
        assert!(!state.needs_load(true, Some(&one)));
        // Упало — само не повторяется, только по «Повторить».
        state.load = Load::Failed(one.clone(), "x".to_owned());
        assert!(!state.needs_load(true, Some(&one)));
        state.load = Load::Idle;
        assert!(state.needs_load(true, Some(&one)));
    }

    #[test]
    fn track_or_setting_change_reloads() {
        let mut state = State::new();
        state.load = Load::Ready(key("1", true), Outcome::Missing);
        assert!(state.needs_load(true, Some(&key("2", true))));
        assert!(state.needs_load(true, Some(&key("1", false))));
    }

    #[test]
    fn stale_answer_is_ignored() {
        let mut state = State::new();
        let (old, new) = (key("1", true), key("2", true));
        state.load = Load::Loading(old.clone());
        // Трек сменился, пошла загрузка нового.
        state.load = Load::Loading(new.clone());
        assert!(!state.accept(&old, Ok(Outcome::Instrumental)));
        assert_eq!(state.load, Load::Loading(new.clone()));
        assert!(state.accept(&new, Err(LyricsError::Unavailable("нет сети".to_owned()))));
        assert_eq!(state.load, Load::Failed(new, "нет сети".to_owned()));
        // Ответ без запроса тоже не принимается.
        state.load = Load::Idle;
        assert!(!state.accept(&old, Ok(Outcome::Missing)));
        assert_eq!(state.load, Load::Idle);
    }

    #[test]
    fn view_for_other_key_is_loading() {
        let mut state = State::new();
        state.load = Load::Ready(key("1", true), Outcome::Missing);
        assert!(state.view_for(&key("1", true)).is_some());
        assert!(state.view_for(&key("2", true)).is_none());
    }

    #[test]
    fn footer_names_source_and_kind() {
        let found = Lyrics {
            source: lyrics::Source::LrcLib,
            synced: Some(vec![lyrics::Line {
                time_ms: 0,
                text: "a".to_owned(),
            }]),
            plain: None,
        };
        assert_eq!(footer(&found), "Синхронный текст · LRCLib");
        let plain = Lyrics {
            source: lyrics::Source::Yandex,
            synced: None,
            plain: Some("a".to_owned()),
        };
        assert_eq!(footer(&plain), "Текст · Яндекс");
    }
}
