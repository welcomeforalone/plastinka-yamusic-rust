//! Вкладка «История»: что вы слушали, по данным сервера. Группы по дням,
//! внутри — контекст (альбом, плейлист, исполнитель, волна) и его треки.
//! Загружается при каждом открытии вкладки; показывается не больше
//! [`MAX_ROWS`] строк. Щелчок по треку играет с него в очереди из треков этой
//! группы, щелчок по контексту открывает его страницу, где она есть.

use std::sync::Arc;

use iced::widget::{Space, button, column, container, row, scrollable};
use iced::{Alignment, Border, Element, Length, Padding, Task};

use super::super::covers::Covers;
use super::super::icons::{Icon, spinner};
use super::super::queue::{ListKey, Queue};
use super::super::theme::{self, Style};
use super::super::widgets::{self, Kind};
use super::pages::{Target, no_network};
use super::{
    App, Effect, Failure, FailureKind, Lib, Library, Message, PAGE_SIDE, PAGE_TOP, Section,
    cover_art, state_block,
};
use crate::api::{self, HistoryContext, HistoryDay, Track};

/// Сколько строк показываем: длинная история окно не перегружает.
pub const MAX_ROWS: usize = 200;
/// Сторона обложки в строке.
const COVER: f32 = 40.0;

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
            Ok(days) => self.server_history = HistoryLoad::Ready(days),
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
        let HistoryLoad::Ready(days) = &self.server_history else {
            return None;
        };
        let tracks = days.get(day)?.items.get(item)?.tracks.clone();
        let id = tracks.get(index)?.id.clone();
        self.chosen = Some(id);
        self.player.skipped = 0;
        self.queue = Some(Queue::new(ListKey::Detached, Arc::new(tracks), index));
        self.start(index, 0.0)
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
        let HistoryLoad::Ready(days) = &self.server_history else {
            return Vec::new();
        };
        days.iter()
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
            HistoryLoad::Ready(days) if days.is_empty() => state_block(
                widgets::badge(Icon::History, theme::TEXT_SECONDARY, theme::SURFACE_2),
                "История пуста",
                "Здесь появятся треки, которые вы слушали".to_owned(),
                Space::new().into(),
            ),
            HistoryLoad::Ready(days) => self.history_list(covers, days),
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
    /// альбома, плейлиста или исполнителя.
    fn context_row<'a>(
        &'a self,
        covers: &'a Covers,
        context: &HistoryContext,
        first: Option<&'a Track>,
    ) -> Element<'a, App> {
        let target = match context {
            HistoryContext::Album { id, .. } => Some(Target::Album(id.clone())),
            HistoryContext::Artist { id, .. } => Some(Target::Artist(id.clone())),
            HistoryContext::Playlist { owner, kind, .. } => Some(Target::Playlist {
                owner: owner.clone(),
                kind: kind.clone(),
            }),
            HistoryContext::Wave | HistoryContext::Other(_) => None,
        };
        let cover: Element<'a, App> = match (context, first) {
            (HistoryContext::Wave, _) => widgets::badge(
                Icon::Wave,
                theme::accent(),
                theme::alpha(theme::accent(), 0.14),
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
        match target {
            Some(target) => button(content)
                .padding(0)
                .style(|_, _| button::Style::default())
                .on_press(Lib(Message::Open(target)))
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
