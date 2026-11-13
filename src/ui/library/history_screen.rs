//! Вкладка «История»: что вы слушали, по данным сервера. Группы по дням,
//! внутри — контекст (альбом, плейлист, исполнитель, волна) и его треки.
//! Загружается при каждом открытии вкладки; показывается не больше
//! [`MAX_ROWS`] строк. Сервер показывает прослушивания с задержкой (а свои
//! отчёты `play-audio` он может и не учесть), поэтому всё, что зазвучало за
//! сеанс, сразу встаёт в начало своего дня ([`history::merge_days`]). Щелчок по треку играет с него в очереди из треков этой
//! группы, щелчок по контексту открывает его страницу, где она есть.

use std::sync::Arc;

use iced::widget::{Space, button, column, container, row, scrollable};
use iced::{Alignment, Background, Border, Color, Element, Length, Padding, Task};

use super::super::covers::Covers;
use super::super::history::{self, LocalPlay};
use super::super::icons::{Icon, icon, spinner};
use super::super::queue::{ListKey, Queue};
use super::super::theme::{self, Style};
use super::super::widgets::{self, Kind};
use super::pages::{Page, Target, no_network};
use super::{
    App, Effect, Failure, FailureKind, Lib, Library, Message, PAGE_SIDE, PAGE_TOP, Section,
    cover_art, state_block,
};
use crate::api::{self, HistoryContext, HistoryDay, Track};

/// Сколько строк показываем: длинная история окно не перегружает.
pub const MAX_ROWS: usize = 200;
/// Сторона обложки в строке.
const COVER: f32 = 40.0;

/// Контекст очереди из «Мне нравится» (и по умолчанию).
pub(super) fn liked_context() -> HistoryContext {
    HistoryContext::Liked
}

/// Свой плейлист «Мне нравится» с сервера (`kind` 3 у владельца `uid`)
/// становится контекстом «Мне нравится»: своя плитка, и на стыке с
/// прослушиваниями сеанса это одна группа.
fn mark_liked(days: &[HistoryDay], uid: u64) -> Vec<HistoryDay> {
    let uid = uid.to_string();
    let mut days = days.to_vec();
    for item in days.iter_mut().flat_map(|day| &mut day.items) {
        if matches!(&item.context, HistoryContext::Playlist { owner, kind, .. }
            if *owner == uid && kind == "3")
        {
            item.context = HistoryContext::Liked;
        }
    }
    days
}

/// Плитка контекста без обложки: значок на фоне (цвета — от акцента).
fn context_tile<'a>(
    name: Icon,
    color: Color,
    background: Background,
    radius: f32,
) -> Element<'a, App> {
    container(iced::widget::center(icon(name, COVER * 0.5, color)))
        .width(COVER)
        .height(COVER)
        .style(move |_| container::Style {
            background: Some(background),
            border: Border::default().rounded(radius),
            ..container::Style::default()
        })
        .into()
}

/// Состояние вкладки.
pub(super) enum HistoryLoad {
    Idle,
    Loading,
    Ready(Arc<Vec<HistoryDay>>),
    Failed(Failure),
}

impl HistoryLoad {
    /// Что делать с состоянием при новой загрузке: готовое остаётся на экране
    /// (оно обновится), остальное — «загрузка».
    fn reloading(self) -> Self {
        match self {
            Self::Ready(days) => Self::Ready(days),
            _ => Self::Loading,
        }
    }
}

impl Library {
    /// Открытие вкладки и «Повторить»: запрашивает историю заново.
    pub(super) fn history_load(&mut self, http: Option<&reqwest::Client>) -> Task<Message> {
        let state = std::mem::replace(&mut self.server_history, HistoryLoad::Idle);
        self.server_history = state.reloading();
        self.history_serial = self.history_serial.wrapping_add(1);
        let serial = self.history_serial;
        let Some(http) = http.cloned() else {
            return Task::done(Message::HistoryLoaded(serial, Err(no_network())));
        };
        let token = self.token.clone();
        Task::perform(
            async move { api::fetch_history(&http, &token.access_token).await },
            move |result| {
                Message::HistoryLoaded(serial, result.map(Arc::new).map_err(Failure::from))
            },
        )
    }

    /// Ответ сервера. Ответ на прежний запрос (вкладку открыли заново)
    /// игнорируется; ошибка при обновлении готовой истории её не стирает.
    pub(super) fn history_loaded(
        &mut self,
        serial: u64,
        result: Result<Arc<Vec<HistoryDay>>, Failure>,
    ) -> Option<Effect> {
        if serial != self.history_serial {
            return None;
        }
        match result {
            Ok(days) => {
                self.server_history = HistoryLoad::Ready(days);
                self.refresh_history_shown();
            }
            Err(failure) if failure.kind == FailureKind::Unauthorized => {
                self.server_history = HistoryLoad::Idle;
                return Some(Effect::Relogin);
            }
            Err(failure) => {
                if !matches!(self.server_history, HistoryLoad::Ready(_)) {
                    self.server_history = HistoryLoad::Failed(failure);
                }
            }
        }
        None
    }

    /// Играть трек `index` группы `item` дня `day`: очередь становится из
    /// треков этой группы.
    pub(super) fn play_history_item(
        &mut self,
        day: usize,
        item: usize,
        index: usize,
    ) -> Option<Effect> {
        if !matches!(self.server_history, HistoryLoad::Ready(_)) {
            return None;
        }
        let group = self.history_shown.get(day)?.items.get(item)?;
        let (tracks, context) = (group.tracks.clone(), group.context.clone());
        let id = tracks.get(index)?.id.clone();
        self.play_context = context;
        self.chosen = Some(id);
        self.player.skipped = 0;
        self.queue = Some(Queue::new(ListKey::Detached, Arc::new(tracks), index));
        self.start(index, 0.0)
    }

    /// Запоминает, откуда построена очередь `key`: страница альбома,
    /// плейлиста или исполнителя берётся из стопки, пока она там есть.
    pub(super) fn remember_context(&mut self, key: &ListKey) {
        self.play_context = match key {
            ListKey::Liked | ListKey::LikedFound(_) | ListKey::Detached => liked_context(),
            ListKey::Wave => HistoryContext::Wave,
            ListKey::TopTracks(_) => HistoryContext::Other("Топ месяца".to_owned()),
            ListKey::SearchTracks(_) => HistoryContext::Other("Поиск".to_owned()),
            ListKey::Album(serial)
            | ListKey::Playlist(serial)
            | ListKey::ArtistPopular(serial)
            | ListKey::ArtistTracks(serial) => self
                .page_context(*serial)
                .unwrap_or_else(|| HistoryContext::Other("Подборка".to_owned())),
        };
    }

    /// Контекст страницы `serial` из стопки.
    fn page_context(&self, serial: u64) -> Option<HistoryContext> {
        let entry = self.stack.iter().find(|entry| entry.serial == serial)?;
        Some(match &entry.page {
            Page::Album { id, load } => HistoryContext::Album {
                id: id.clone(),
                title: load
                    .ready()
                    .map_or_else(|| "Альбом".to_owned(), |view| view.card.title.clone()),
            },
            Page::Playlist { owner, kind, load } => HistoryContext::Playlist {
                owner: owner.clone(),
                kind: kind.clone(),
                title: load
                    .ready()
                    .map_or_else(|| "Плейлист".to_owned(), |view| view.card.title.clone()),
            },
            Page::Artist { id, load } => HistoryContext::Artist {
                id: id.clone(),
                name: load
                    .ready()
                    .map_or_else(|| "Исполнитель".to_owned(), |view| view.card.name.clone()),
            },
            Page::ArtistTracks { id, name, .. } => HistoryContext::Artist {
                id: id.clone(),
                name: name.clone(),
            },
            Page::TopArtists { .. } => return None,
        })
    }

    /// Трек зазвучал: сразу в «Историю», не дожидаясь сервера.
    pub(super) fn history_note_play(&mut self) {
        let Some(track) = self.now_playing.clone() else {
            return;
        };
        let context = if self.in_wave() {
            HistoryContext::Wave
        } else {
            self.play_context.clone()
        };
        self.local_plays.record(LocalPlay {
            track,
            context,
            date: api::today_utc(),
        });
        self.refresh_history_shown();
    }

    /// Пересобирает вкладку «История»: сервер плюс сеанс.
    fn refresh_history_shown(&mut self) {
        let server = match &self.server_history {
            HistoryLoad::Ready(days) => mark_liked(days, self.status.account().uid),
            _ => Vec::new(),
        };
        self.history_shown = Arc::new(history::merge_days(&server, self.local_plays.plays()));
    }

    /// Треки серверной истории подряд, свежие первыми (как пришли). Пока она
    /// не загрузилась или не загрузилась вовсе — пусто.
    pub(super) fn server_recent(&self) -> Vec<&Track> {
        let HistoryLoad::Ready(days) = &self.server_history else {
            return Vec::new();
        };
        days.iter()
            .flat_map(|day| &day.items)
            .flat_map(|item| &item.tracks)
            .collect()
    }

    /// Крутится спиннер загрузки истории.
    pub fn history_loading(&self) -> bool {
        self.section == Section::History
            && self.stack.is_empty()
            && matches!(
                self.server_history,
                HistoryLoad::Idle | HistoryLoad::Loading
            )
    }

    /// Обложки видимых строк (для загрузки).
    pub fn history_covers(&self) -> Vec<&str> {
        if !matches!(self.server_history, HistoryLoad::Ready(_)) {
            return Vec::new();
        }
        self.history_shown
            .iter()
            .flat_map(|day| &day.items)
            .flat_map(|item| &item.tracks)
            .take(MAX_ROWS)
            .filter_map(|track| self.shown(track).cover_uri.as_deref())
            .collect()
    }

    pub(super) fn history_view<'a>(&'a self, covers: &'a Covers, turn: f32) -> Element<'a, App> {
        match &self.server_history {
            HistoryLoad::Idle | HistoryLoad::Loading => container(iced::widget::center(
                column![
                    spinner(40.0, theme::accent(), turn),
                    theme::line("Загружаем историю…", Style::Body, theme::TEXT_SECONDARY),
                ]
                .spacing(14)
                .align_x(Alignment::Center),
            ))
            .into(),
            HistoryLoad::Failed(failure) => state_block(
                widgets::badge(Icon::Offline, theme::ERROR, theme::ERROR_SOFT),
                "Не удалось загрузить историю",
                failure.text.clone(),
                widgets::action(
                    Kind::Primary,
                    Some(Icon::Retry),
                    "Повторить",
                    Some(Lib(Message::HistoryReload)),
                ),
            ),
            HistoryLoad::Ready(_) if self.history_shown.is_empty() => state_block(
                widgets::badge(Icon::History, theme::TEXT_SECONDARY, theme::SURFACE_2),
                "История пуста",
                "Здесь появятся треки, которые вы слушали".to_owned(),
                Space::new().into(),
            ),
            HistoryLoad::Ready(_) => self.history_list(covers, &self.history_shown),
        }
    }

    fn history_list<'a>(&'a self, covers: &'a Covers, days: &'a [HistoryDay]) -> Element<'a, App> {
        let today = api::today_utc();
        let mut list =
            column![theme::line("История", Style::Display, theme::TEXT_PRIMARY)].spacing(6);
        let mut rows = 0;
        'days: for (day_index, day) in days.iter().enumerate() {
            list = list.push(Space::new().height(18)).push(theme::line(
                api::day_label(day.date, today),
                Style::Title,
                theme::TEXT_PRIMARY,
            ));
            for (item_index, item) in day.items.iter().enumerate() {
                list = list.push(self.context_row(covers, &item.context, item.tracks.first()));
                for (index, track) in item.tracks.iter().enumerate() {
                    if rows >= MAX_ROWS {
                        break 'days;
                    }
                    rows += 1;
                    list = list.push(self.history_track_row(
                        covers,
                        track,
                        Lib(Message::PlayHistoryItem(day_index, item_index, index)),
                    ));
                }
            }
        }
        scrollable(list.max_width(760).padding(Padding {
            top: PAGE_TOP,
            right: PAGE_SIDE,
            bottom: PAGE_TOP,
            left: PAGE_SIDE,
        }))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    /// Подпись контекста: обложка первого трека и название; щелчок — страница
    /// альбома, плейлиста или исполнителя, «Мне нравится». У волны и «Мне
    /// нравится» вместо обложки трека своя плитка в цвет акцента.
    fn context_row<'a>(
        &'a self,
        covers: &'a Covers,
        context: &HistoryContext,
        first: Option<&'a Track>,
    ) -> Element<'a, App> {
        let open = match context {
            HistoryContext::Album { id, .. } => Some(Message::Open(Target::Album(id.clone()))),
            HistoryContext::Artist { id, .. } => Some(Message::Open(Target::Artist(id.clone()))),
            HistoryContext::Playlist { owner, kind, .. } => Some(Message::Open(Target::Playlist {
                owner: owner.clone(),
                kind: kind.clone(),
            })),
            HistoryContext::Liked => Some(Message::Nav(Section::Liked)),
            HistoryContext::Wave | HistoryContext::Other(_) => None,
        };
        let cover: Element<'a, App> = match (context, first) {
            (HistoryContext::Wave, _) => context_tile(
                Icon::Wave,
                theme::accent(),
                theme::alpha(theme::accent(), 0.14).into(),
                widgets::ROUND,
            ),
            (HistoryContext::Liked, _) => context_tile(
                Icon::HeartFilled,
                theme::on_accent(),
                theme::liked_tile(),
                widgets::RADIUS_S,
            ),
            (_, Some(track)) => widgets::cover(
                cover_art(covers, self.shown(track)),
                COVER,
                widgets::RADIUS_S,
            ),
            _ => widgets::block(COVER, COVER, theme::SURFACE_3, widgets::RADIUS_S),
        };
        let content = row![
            container(cover).width(COVER).height(COVER).clip(true),
            theme::line(context.label(), Style::BodyStrong, theme::TEXT_PRIMARY),
        ]
        .spacing(12)
        .align_y(Alignment::Center);
        let content = container(content).padding(Padding {
            top: 10.0,
            right: 6.0,
            bottom: 4.0,
            left: 6.0,
        });
        match open {
            Some(open) => button(content)
                .padding(0)
                .style(|_, _| button::Style::default())
                .on_press(Lib(open))
                .into(),
            None => content.into(),
        }
    }

    /// Строка трека: обложка, название, исполнитель, сердце, длительность.
    fn history_track_row<'a>(
        &'a self,
        covers: &'a Covers,
        original: &'a Track,
        play: App,
    ) -> Element<'a, App> {
        let track = self.shown(original);
        let liked = self.liked.contains(&original.id);
        let heart = widgets::icon_button(
            if liked {
                Icon::HeartFilled
            } else {
                Icon::Heart
            },
            widgets::Size::S,
            liked,
            Some(Lib(Message::ToggleLike(original.clone()))),
        );
        let (title_color, other_color) = if original.available || track.id != original.id {
            (theme::TEXT_PRIMARY, theme::TEXT_SECONDARY)
        } else {
            (theme::TEXT_DISABLED, theme::TEXT_DISABLED)
        };
        let content = row![
            widgets::cover(cover_art(covers, track), COVER, widgets::RADIUS_S),
            column![
                container(theme::line(track.display_title(), Style::Body, title_color)).clip(true),
                container(theme::line(
                    track.artists_text(),
                    Style::Secondary,
                    other_color
                ))
                .clip(true),
            ]
            .spacing(2)
            .width(Length::Fill),
            heart,
            theme::line(track.duration_text(), Style::Caption, theme::TEXT_TERTIARY)
                .width(56)
                .align_x(Alignment::End),
        ]
        .spacing(12)
        .align_y(Alignment::Center);
        button(content)
            .width(Length::Fill)
            .padding(6)
            .style(|_, status| button::Style {
                background: match status {
                    button::Status::Hovered | button::Status::Pressed => {
                        Some(theme::SURFACE_3.into())
                    }
                    _ => None,
                },
                border: Border::default().rounded(10.0),
                ..button::Style::default()
            })
            .on_press(play)
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playlist(owner: &str, kind: &str) -> HistoryContext {
        HistoryContext::Playlist {
            owner: owner.to_owned(),
            kind: kind.to_owned(),
            title: "Плейлист".to_owned(),
        }
    }

    #[test]
    fn own_liked_playlist_becomes_liked_context() {
        let item = |context| api::HistoryItem {
            context,
            tracks: Vec::new(),
        };
        let days = [HistoryDay {
            date: Some((2026, 11, 14)),
            items: vec![
                item(playlist("42", "3")),
                // Чужой «Мне нравится» и свой другой плейлист — как есть.
                item(playlist("7", "3")),
                item(playlist("42", "1000")),
                item(HistoryContext::Wave),
            ],
        }];
        let contexts: Vec<HistoryContext> = mark_liked(&days, 42)
            .into_iter()
            .flat_map(|day| day.items)
            .map(|item| item.context)
            .collect();
        assert_eq!(
            contexts,
            [
                HistoryContext::Liked,
                playlist("7", "3"),
                playlist("42", "1000"),
                HistoryContext::Wave,
            ]
        );
        assert_eq!(liked_context().label(), "Мне нравится");
    }
}
