//! Вид мини-плеера. Состояние берётся у той же `Library`, что у нижнего
//! плеера, и кнопки шлют те же сообщения (в «Моей волне» «Следующий» — тот
//! же пропуск). Раскладки: высокая 320 × 440 и низкая 420 × 132.

use iced::time::Instant;
use iced::widget::{Space, button, center, column, container, mouse_area, row};
use iced::{Alignment, Border, Element, Length, Padding};

use super::super::anim;
use super::super::covers::Covers;
use super::super::icons::{Icon, icon};
use super::super::marquee::marquee;
use super::super::mini;
use super::super::theme::{self, Style};
use super::super::widgets::{self, Size};
use super::{App, Lib, Library, Message, bar, clock, crossfade_cover};
use crate::api::Track;

/// Обложка в высокой раскладке и в низкой.
const TALL_COVER: f32 = 220.0;
const COMPACT_COVER: f32 = 108.0;
/// Высота полосы с кнопками окна.
const BAR: f32 = 36.0;
/// Боковой отступ шапки и ряда кнопок.
const SIDE: f32 = 8.0;
/// Боковой отступ названия, исполнителя и перемотки: одна ось и равные поля.
const TEXT_SIDE: f32 = 24.0;
/// Ширина подписей времени по сторонам перемотки: одинаковая.
const TIME_WIDTH: f32 = 40.0;

impl Library {
    /// Шаблон адреса большой обложки: из неё считается цвет фона.
    pub fn mini_cover(&self) -> Option<&str> {
        self.current_shown()
            .and_then(|track| track.cover_uri.as_deref())
    }

    /// Содержимое окна мини-плеера. `average` — средний цвет обложки (фон),
    /// `pinned` — окно закреплено поверх других.
    pub fn mini_view<'a>(
        &'a self,
        covers: &'a Covers,
        compact: bool,
        pinned: bool,
        average: Option<[u8; 3]>,
        now: Instant,
    ) -> Element<'a, App> {
        let content = if compact {
            self.mini_compact(covers, pinned, now)
        } else {
            self.mini_tall(covers, pinned, now)
        };
        let background = mini::background(average);
        // Перетаскивание за фон: кнопки забирают нажатие себе.
        let draggable = mouse_area(content).on_press(App::MiniDrag);
        container(draggable)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_| container::Style {
                background: Some(background.into()),
                border: Border {
                    color: theme::border(),
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..container::Style::default()
            })
            .into()
    }

    /// Название и исполнитель; без трека — подсказка.
    fn mini_texts(&self) -> (String, String) {
        match self.current_shown() {
            Some(track) => (track.display_title(), track.artists_text()),
            None => ("Ничего не играет".to_owned(), "Выберите трек".to_owned()),
        }
    }

    /// Ползунок перемотки с временем: слева текущее, справа общее (если
    /// `with_total`).
    fn mini_seek<'a>(&'a self, with_total: bool, now: Instant) -> Element<'a, App> {
        let has_track = self.current_shown().is_some();
        let total = self.current_duration();
        let seekable = has_track && total.is_some();
        let shown = self.player.seeking.unwrap_or(self.player.position);
        let position = total.map_or(shown, |total| shown.min(total));
        let range_end = total.unwrap_or(0.0).max(1.0);
        let total_text = match (has_track, total) {
            (true, None) => "-:--".to_owned(),
            (_, total) => clock(total.unwrap_or(0.0)),
        };
        let slider = mouse_area(bar(
            0.0..=range_end,
            position.min(range_end),
            seekable,
            anim::progress(&self.motion.seek_hover, now),
            |value| Lib(Message::SeekDrag(value)),
            Some(Lib(Message::SeekCommit)),
        ))
        .on_enter(Lib(Message::SeekHover(true)))
        .on_exit(Lib(Message::SeekHover(false)));
        let mut line = row![
            theme::line(clock(position), Style::Caption, theme::text_tertiary())
                .width(TIME_WIDTH)
                .align_x(Alignment::End),
            container(slider).width(Length::Fill),
        ]
        .spacing(10)
        .align_y(Alignment::Center);
        if with_total {
            line = line.push(
                theme::line(total_text, Style::Caption, theme::text_tertiary()).width(TIME_WIDTH),
            );
        }
        line.into()
    }

    /// Кнопка play/pause: круг в цвете акцента.
    fn mini_play<'a>(&'a self, side: f32) -> Element<'a, App> {
        let has_track = self.current_shown().is_some();
        let playing = self.player.playing;
        let color = theme::on_accent();
        let glyph = if playing {
            icon(Icon::PauseFilled, side * 0.46, color)
        } else {
            icon(Icon::PlayFilled, side * 0.46, color)
        };
        let play = button(center(glyph))
            .width(side)
            .height(side)
            .padding(0)
            .style(|_, status| button::Style {
                background: match status {
                    button::Status::Pressed => Some(theme::accent_pressed().into()),
                    button::Status::Disabled => Some(theme::surface_3().into()),
                    _ => None,
                },
                border: Border::default().rounded(widgets::ROUND),
                ..button::Style::default()
            })
            .on_press_maybe(has_track.then_some(Lib(Message::TogglePlay)));
        if has_track {
            anim::hover_fill(play, theme::accent(), theme::accent_hover(), widgets::ROUND).into()
        } else {
            play.into()
        }
    }

    /// Булавка: держать окно поверх остальных.
    fn mini_pin_button<'a>(&self, pinned: bool) -> Element<'a, App> {
        widgets::with_tooltip(
            widgets::icon_button(Icon::Pin, Size::S, pinned, Some(App::MiniPin)),
            if pinned {
                "Не держать поверх окон"
            } else {
                "Держать поверх окон"
            },
            iced::widget::tooltip::Position::Bottom,
        )
    }

    /// «Вернуть главное окно» и крестик.
    fn mini_exit_buttons<'a>(&self) -> Element<'a, App> {
        row![
            widgets::with_tooltip(
                widgets::icon_button(Icon::WindowMaximize, Size::S, false, Some(App::MiniExpand)),
                "Вернуть главное окно",
                iced::widget::tooltip::Position::Bottom,
            ),
            widgets::icon_button(Icon::Close, Size::S, false, Some(App::MiniClose)),
        ]
        .spacing(2)
        .align_y(Alignment::Center)
        .into()
    }

    /// Обложка в мини-плеере: большая (600×600), когда загрузилась, иначе
    /// 300×300 (она же под большой, пока та проявляется). Размер с запасом
    /// под масштаб экрана: картинка не растягивается из маленькой.
    fn mini_art<'a>(
        &'a self,
        covers: &'a Covers,
        track: &'a Track,
        size: f32,
        radius: f32,
        now: Instant,
    ) -> Element<'a, App> {
        let base = crossfade_cover(
            covers,
            track,
            self.motion.cover_from.as_ref(),
            anim::progress(&self.motion.cover_fade, now),
            size,
            radius,
        );
        match covers.big(track.cover_uri.as_deref(), now) {
            Some((big, opacity)) if opacity >= 1.0 => {
                widgets::picture(&big.cover, size, radius, 1.0)
            }
            Some((big, opacity)) => {
                iced::widget::stack![base, widgets::picture(&big.cover, size, radius, opacity)]
                    .into()
            }
            None => base,
        }
    }

    fn mini_tall<'a>(&'a self, covers: &'a Covers, pinned: bool, now: Instant) -> Element<'a, App> {
        let (title, artist) = self.mini_texts();
        let has_track = self.current_shown().is_some();
        let liked = self
            .current_shown()
            .is_some_and(|track| self.liked.contains(&track.id));
        let like = self
            .current()
            .map(|track| Lib(Message::ToggleLike(track.clone())));
        let on = |message: Message| has_track.then_some(Lib(message));
        let cover: Element<'a, App> = match self.current_shown() {
            Some(track) => self.mini_art(covers, track, TALL_COVER, 16.0, now),
            None => container(center(icon(Icon::Empty, 40.0, theme::text_secondary())))
                .width(TALL_COVER)
                .height(TALL_COVER)
                .style(|_| super::surface_2(16.0))
                .into(),
        };
        let volume = if self.player.muted {
            Icon::VolumeMute
        } else {
            Icon::Volume
        };
        // Шапка симметрична: булавка слева, «вернуть» и крестик справа,
        // одинаковые отступы от краёв.
        let top = row![
            self.mini_pin_button(pinned),
            Space::new().width(Length::Fill),
            self.mini_exit_buttons(),
        ]
        .height(BAR)
        .align_y(Alignment::Center)
        .padding([0.0, SIDE]);
        // Лайк и громкость одной ширины, «Предыдущий» и «Следующий» тоже:
        // play/pause стоит ровно по центру окна.
        let left = row![
            widgets::icon_button(
                if liked {
                    Icon::HeartFilled
                } else {
                    Icon::Heart
                },
                Size::S,
                liked,
                like,
            ),
            widgets::icon_button(Icon::PreviousFilled, Size::M, false, on(Message::Previous)),
        ]
        .spacing(10)
        .align_y(Alignment::Center);
        let right = row![
            widgets::icon_button(Icon::NextFilled, Size::M, false, on(Message::Next)),
            widgets::icon_button(volume, Size::S, false, Some(Lib(Message::ToggleMute))),
        ]
        .spacing(10)
        .align_y(Alignment::Center);
        let controls = row![
            container(left).width(Length::Fill).align_x(Alignment::End),
            self.mini_play(48.0),
            container(right)
                .width(Length::Fill)
                .align_x(Alignment::Start),
        ]
        .spacing(10)
        .align_y(Alignment::Center);
        column![
            top,
            container(cover).center_x(Length::Fill).padding([8.0, 0.0]),
            column![
                container(marquee(title, Style::Headline, theme::text_primary()).centered())
                    .padding([0.0, TEXT_SIDE]),
                container(marquee(artist, Style::Secondary, theme::text_secondary()).centered())
                    .padding([0.0, TEXT_SIDE]),
            ]
            .spacing(2),
            Space::new().height(Length::Fill),
            container(self.mini_seek(true, now)).padding([0.0, TEXT_SIDE]),
            container(controls).padding([10.0, SIDE]),
        ]
        .into()
    }

    fn mini_compact<'a>(
        &'a self,
        covers: &'a Covers,
        pinned: bool,
        now: Instant,
    ) -> Element<'a, App> {
        let (title, artist) = self.mini_texts();
        let has_track = self.current_shown().is_some();
        let on = |message: Message| has_track.then_some(Lib(message));
        let cover: Element<'a, App> = match self.current_shown() {
            Some(track) => self.mini_art(covers, track, COMPACT_COVER, 12.0, now),
            None => container(center(icon(Icon::Empty, 28.0, theme::text_secondary())))
                .width(COMPACT_COVER)
                .height(COMPACT_COVER)
                .style(|_| super::surface_2(12.0))
                .into(),
        };
        let total = self.current_duration();
        let shown = self.player.seeking.unwrap_or(self.player.position);
        let time = format!(
            "{} / {}",
            clock(total.map_or(shown, |total| shown.min(total))),
            total.map_or_else(|| "-:--".to_owned(), clock)
        );
        let head = row![
            column![
                marquee(title, Style::Headline, theme::text_primary()),
                marquee(artist, Style::Secondary, theme::text_secondary()),
            ]
            .spacing(2)
            .width(Length::Fill),
            self.mini_pin_button(pinned),
            self.mini_exit_buttons(),
        ]
        .spacing(2)
        .align_y(Alignment::Start);
        let controls = row![
            widgets::icon_button(Icon::PreviousFilled, Size::M, false, on(Message::Previous)),
            self.mini_play(40.0),
            widgets::icon_button(Icon::NextFilled, Size::M, false, on(Message::Next)),
            container(theme::line(time, Style::Caption, theme::text_tertiary()))
                .width(Length::Fill)
                .align_x(Alignment::End),
        ]
        .spacing(10)
        .align_y(Alignment::Center);
        row![
            container(cover).padding(12),
            column![head, self.mini_seek(false, now), controls]
                .spacing(4)
                .width(Length::Fill)
                .padding(Padding {
                    top: 8.0,
                    right: 8.0,
                    bottom: 8.0,
                    left: 0.0,
                }),
        ]
        .into()
    }
}
