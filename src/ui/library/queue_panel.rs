//! Панель «Очередь» справа (360 px, на месте панели текста): что играет и
//! что дальше. Строки те же, что в списках (с заменами FckCensor), в
//! компактном виде. Щелчок по строке играет с этого трека, очередь не
//! пересоздаётся. Как в официальном клиенте: «Недавно играли» (история
//! сеанса, до 20), «Сейчас играет» и один следующий трек.

use std::sync::Arc;

use iced::widget::{Space, button, column, container, row, scrollable};
use iced::{Alignment, Border, Color, Element, Length, Padding, Task};

use super::super::covers::Covers;
use super::super::icons::Icon;
use super::super::queue::{ListKey, Queue};
use super::super::scroll;
use super::super::theme::{self, Style};
use super::super::widgets;
use super::lyrics::note;
use super::{App, Lib, Library, Message, cover_art, plugins};
use crate::api::Track;

/// Сколько следующих треков показываем в «Далее»: один, как в официальном
/// клиенте.
pub const UPCOMING: usize = 1;
/// Прокручиваемый список панели.
const QUEUE_ID: &str = "queue-panel";
/// Промежуток между разделами.
const SECTION_GAP: f32 = 12.0;
/// Высота строки, заголовка раздела и промежутка вместе с отступом 2 px
/// между элементами списка: по ним считается прокрутка к «Сейчас играет».
const ROW_STRIDE: f32 = 54.0;
const HEADER_STRIDE: f32 = 34.0;
const GAP_STRIDE: f32 = SECTION_GAP + 2.0;
/// Боковой отступ списка.
const PAD: f32 = 16.0;
/// Сторона обложки в строке.
const COVER: f32 = 40.0;

impl Library {
    /// Номера треков, которые идут дальше (до [`UPCOMING`]): с учётом
    /// перемешивания и недоступных треков. В «Моей волне» — показанные
    /// вперёд треки волны.
    pub fn queue_upcoming(&self) -> Vec<usize> {
        let Some(queue) = &self.queue else {
            return Vec::new();
        };
        let censor = self.plugins.fckcensor.then_some(&self.censor);
        let shuffle = !self.in_wave() && self.player.shuffle;
        queue.upcoming(shuffle, &|track| plugins::playable(censor, track), UPCOMING)
    }

    /// Обложки строк панели (для загрузки), пока она открыта.
    pub fn queue_covers(&self) -> Vec<&str> {
        if !self.queue_open() {
            return Vec::new();
        }
        let next = self.queue.as_ref().into_iter().flat_map(|queue| {
            self.queue_upcoming()
                .into_iter()
                .filter_map(|index| queue.get(index))
        });
        self.recent_tracks()
            .into_iter()
            .chain(next)
            .filter_map(|track| self.shown(track).cover_uri.as_deref())
            .collect()
    }

    /// «Недавно играли»: история сеанса, уже сыгранные треки этой очереди и
    /// серверная история, без повторов, до 20, свежие рядом с текущим.
    /// Ошибка загрузки серверной истории не мешает: тогда её просто нет.
    pub fn recent_tracks(&self) -> Vec<&Track> {
        let session: Vec<&Track> = self.history.tracks().collect();
        let censor = self.plugins.fckcensor.then_some(&self.censor);
        let shuffle = !self.in_wave() && self.player.shuffle;
        let before: Vec<&Track> = self
            .queue
            .as_ref()
            .map(|queue| {
                queue
                    .before(
                        shuffle,
                        &|track| plugins::playable(censor, track),
                        crate::ui::history::LIMIT,
                    )
                    .into_iter()
                    .filter_map(|index| queue.get(index))
                    .collect()
            })
            .unwrap_or_default();
        let server = self.server_recent();
        crate::ui::history::merge_recent(
            self.current().map(|track| track.id.as_str()),
            &session,
            &before,
            &server,
            crate::ui::history::LIMIT,
        )
    }

    /// Прокручивает панель так, чтобы «Сейчас играет» было сверху.
    pub(super) fn queue_scroll_to_current(&self) -> Task<Message> {
        iced::widget::operation::scroll_to(
            QUEUE_ID,
            scrollable::AbsoluteOffset {
                x: None,
                y: Some(current_offset(self.recent_tracks().len())),
            },
        )
    }

    /// Содержимое панели: «Недавно играли», «Сейчас играет», «Далее» (один
    /// следующий трек).
    pub(super) fn queue_content<'a>(&'a self, covers: &'a Covers) -> Element<'a, App> {
        let (Some(queue), Some(current)) = (&self.queue, self.current()) else {
            return note(
                Icon::Queue,
                "Очередь пуста",
                "Включите трек из списка",
                None,
            );
        };
        let mut list = column![].spacing(2);
        let recent = self.recent_tracks();
        if !recent.is_empty() {
            list = list.push(section("Недавно играли"));
            for (index, track) in recent.into_iter().enumerate() {
                list = list.push(self.history_row(covers, index, track));
            }
            list = list.push(Space::new().height(SECTION_GAP));
        }
        list = list.push(section("Сейчас играет"));
        list = list.push(self.queue_row(covers, queue.current, current, true));
        list = list
            .push(Space::new().height(SECTION_GAP))
            .push(section("Далее"));
        match self
            .queue_upcoming()
            .first()
            .and_then(|index| queue.get(*index).map(|track| (*index, track)))
        {
            Some((index, track)) => {
                list = list.push(self.queue_row(covers, index, track, false));
            }
            None => {
                list = list.push(
                    container(theme::line(
                        "Дальше ничего нет",
                        Style::Secondary,
                        theme::TEXT_TERTIARY,
                    ))
                    .padding([12, 8]),
                );
            }
        }
        scroll::smooth(
            scrollable(list.padding(Padding::default().left(PAD).right(PAD)))
                .id(QUEUE_ID)
                .width(Length::Fill)
                .height(Length::Fill),
        )
    }

    /// Строка истории: приглушённая, щелчок играет этот трек.
    fn history_row<'a>(
        &'a self,
        covers: &'a Covers,
        index: usize,
        original: &'a Track,
    ) -> Element<'a, App> {
        self.row_content(
            covers,
            original,
            (theme::TEXT_SECONDARY, theme::TEXT_TERTIARY),
            Some(Lib(Message::PlayHistory(index))),
        )
    }

    /// Строка очереди: играющая — акцентом и без щелчка.
    fn queue_row<'a>(
        &'a self,
        covers: &'a Covers,
        index: usize,
        original: &'a Track,
        playing: bool,
    ) -> Element<'a, App> {
        if playing {
            return self.row_content(
                covers,
                original,
                (theme::accent(), theme::TEXT_SECONDARY),
                None,
            );
        }
        self.row_content(
            covers,
            original,
            (theme::TEXT_PRIMARY, theme::TEXT_SECONDARY),
            Some(Lib(Message::PlayQueueAt(index))),
        )
    }

    /// Строка: обложка 40, название, исполнитель, длительность. Показывается
    /// то, что слышно (с заменой FckCensor). Без `on_press` строка не нажимается.
    fn row_content<'a>(
        &'a self,
        covers: &'a Covers,
        original: &'a Track,
        (title_color, other_color): (Color, Color),
        on_press: Option<App>,
    ) -> Element<'a, App> {
        let track = self.shown(original);
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
            theme::line(track.duration_text(), Style::Caption, theme::TEXT_TERTIARY),
        ]
        .spacing(12)
        .align_y(Alignment::Center);
        let Some(message) = on_press else {
            return container(content).padding(6).width(Length::Fill).into();
        };
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
            .on_press(message)
            .into()
    }

    /// Играть трек из истории: он есть в текущей очереди — как `PlayQueueAt`;
    /// иначе очередь становится из одного этого трека (проще и надёжнее, чем
    /// вставлять в чужой порядок и перемешивание).
    pub(super) fn play_history(&mut self, index: usize) -> Option<super::Effect> {
        let track = (*self.recent_tracks().get(index)?).clone();
        let found = self
            .queue
            .as_ref()
            .and_then(|queue| queue_position(&queue.tracks, &track.id));
        if let Some(at) = found {
            return self.play_queue_at(at);
        }
        self.chosen = Some(track.id.clone());
        self.player.skipped = 0;
        let queue = Queue::new(ListKey::Detached, Arc::new(vec![track]), 0);
        self.queue = Some(queue);
        self.start(0, 0.0)
    }

    /// Играть с трека `index` текущей очереди: очередь та же, перемешивание
    /// и список не трогаются. Включённый вручную трек Slopless не пропускает.
    pub(super) fn play_queue_at(&mut self, index: usize) -> Option<super::Effect> {
        let id = self.queue.as_ref()?.get(index)?.id.clone();
        self.chosen = Some(id);
        self.player.skipped = 0;
        self.start(index, 0.0)
    }
}

fn section<'a>(title: &'a str) -> Element<'a, App> {
    container(theme::line(
        title,
        Style::CaptionStrong,
        theme::TEXT_TERTIARY,
    ))
    .padding([8, 8])
    .into()
}

/// На сколько прокрутить список, чтобы «Сейчас играет» оказалось сверху:
/// над ним заголовок «Недавно играли», строки истории и промежуток.
/// Без истории — 0.
pub fn current_offset(history: usize) -> f32 {
    if history == 0 {
        return 0.0;
    }
    HEADER_STRIDE + history as f32 * ROW_STRIDE + GAP_STRIDE
}

/// Номер трека `id` в списке очереди.
fn queue_position(tracks: &[Track], id: &str) -> Option<usize> {
    tracks.iter().position(|track| track.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_one_track_follows() {
        assert_eq!(UPCOMING, 1);
    }

    #[test]
    fn offset_skips_the_history_block() {
        assert_eq!(current_offset(0), 0.0);
        assert_eq!(current_offset(1), HEADER_STRIDE + ROW_STRIDE + GAP_STRIDE);
        assert!(current_offset(5) > current_offset(4));
    }
}
