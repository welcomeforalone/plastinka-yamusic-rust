//! Виды страниц и разделов каталога: общий виртуальный список, строки
//! треков со ссылками на исполнителя и альбом, сетка карточек, шапки
//! альбома, плейлиста и исполнителя, поиск и коллекция.
//!
//! Прокручиваемая часть любого экрана устроена одинаково ([`Body`]): блок
//! сверху известной высоты и одинаковые строки (треки или ряды карточек).
//! Поэтому высоту всего списка можно посчитать без раскладки, а строятся
//! только видимые строки.

use iced::widget::text::{Span, Wrapping};
use iced::widget::{
    Space, button, center, column, container, hover, mouse_area, rich_text, row, span, tooltip,
};
use iced::{Alignment, Border, Color, Element, Length, Padding, mouse};

use super::pages::{AlbumView, ArtistView, Entry, Load, Page, Paged, PlaylistView, Target};
use super::{
    App, LIST_BOTTOM, LIST_TOP, Lib, Library, Message, OVERSCAN, PAGE_SIDE, PAGE_TOP, ROW, Section,
    list_header, skeleton, state_block,
};
use crate::api::{
    self, AlbumCard, AlbumKind, ArtistCard, ArtistRef, PlaylistCard, SearchKind, Track,
};
use crate::texts::{self, plural, sentence};
use crate::ui::FailureKind;
use crate::ui::covers::{Art, Covers, Grade};
use crate::ui::icons::{Icon, icon};
use crate::ui::queue::ListKey;
use crate::ui::theme::{self, Style};
use crate::ui::widgets::{self, CoverArt, Kind, Size};
use crate::ui::{Failure, anim};
use iced::time::Instant;

/// Сетка карточек: промежуток, самая узкая карточка, подписи под обложкой
/// (отступ 10, название 20, промежуток 2, подпись 18) и отступ между рядами.
const GRID_GAP: f32 = 24.0;
const CARD_MIN: f32 = 168.0;
const CARD_TEXT: f32 = 50.0;
const GRID_ROW_GAP: f32 = 24.0;
const MAX_COLUMNS: usize = 8;
/// Блок сверху над первой строкой: отступ (с `LIST_TOP` выходит `PAGE_TOP`),
/// кнопка «Назад» и шапка.
const TOP_PAD: f32 = PAGE_TOP - LIST_TOP;
const BACK_ROW: f32 = 40.0;
const BACK_GAP: f32 = 12.0;
/// Шапка альбома, плейлиста и исполнителя: обложка 176 и текст рядом.
const HEADER: f32 = 184.0;
const HEADER_COVER: f32 = 176.0;
const HEADER_GAP: f32 = 28.0;
/// Заголовок таблицы треков с разделителем.
const LIST_HEADER: f32 = 37.0;
/// Заголовок блока («Популярные треки», «Альбомы»).
const SECTION_TITLE: f32 = 40.0;
const SECTION_GAP: f32 = 36.0;
/// Популярных треков на странице исполнителя.
const POPULAR: usize = 5;
/// Шапка «Все треки» исполнителя.
const TRACKS_HEADER: f32 = 140.0;
/// Шапка раздела коллекции: заголовок и подпись.
const COLLECTION_HEADER: f32 = 48.0 + 4.0 + 20.0;
/// Строка «Показаны результаты по запросу …».
const SEARCH_NOTE: f32 = 32.0;
/// Поле поиска по каталогу.
const SEARCH_FIELD: f32 = 48.0;
/// Через сколько показывать скелетон страницы.
const SKELETON_AFTER: f32 = 0.3;

/// Прокручиваемая часть экрана: блок сверху высотой `top` и `rows` строк
/// высотой `row`.
#[derive(Debug, Clone, Copy)]
pub struct Body {
    pub top: f32,
    pub rows: usize,
    pub row: f32,
}

impl Body {
    const EMPTY: Self = Self {
        top: 0.0,
        rows: 0,
        row: ROW,
    };

    pub fn content(&self) -> f32 {
        LIST_TOP + self.top + span_of(self.rows, self.row) + LIST_BOTTOM
    }
}

/// Высота `count` строк. Строк меньше 2^24, поэтому точность не теряется.
fn span_of(count: usize, row: f32) -> f32 {
    count as f32 * row
}

/// Сколько целых строк высотой `row` помещается в `px`. Отрицательное и не
/// число — ноль.
fn rows_at(px: f32, row: f32) -> usize {
    let rows = px / row;
    if rows.is_finite() && rows > 0.0 {
        // Значение неотрицательное и не больше высоты списка: потерь нет.
        rows as usize
    } else {
        0
    }
}

/// Колонки сетки и сторона карточки при ширине раздела `width`.
fn grid(width: f32) -> (usize, f32) {
    let inner = (width - 2.0 * PAGE_SIDE).max(CARD_MIN);
    let fit = rows_at(inner + GRID_GAP, CARD_MIN + GRID_GAP).clamp(2, MAX_COLUMNS);
    let side = (inner - GRID_GAP * (fit as f32 - 1.0)) / fit as f32;
    (fit, side.max(1.0))
}

fn grid_rows(items: usize, columns: usize) -> usize {
    items.div_ceil(columns.max(1))
}

/// Высота блока над альбомами исполнителя.
fn artist_top(artist: Option<&ArtistView>) -> f32 {
    let mut top = TOP_PAD + BACK_ROW + BACK_GAP + HEADER;
    if let Some(artist) = artist {
        let popular = artist.popular.len().min(POPULAR);
        if popular > 0 {
            top += SECTION_GAP + SECTION_TITLE + 8.0 + span_of(popular, ROW);
        }
        if !artist.albums.is_empty() {
            top += SECTION_GAP + SECTION_TITLE + 16.0;
        }
    }
    top
}

const DETAIL_TOP: f32 = TOP_PAD + BACK_ROW + BACK_GAP + HEADER + HEADER_GAP + LIST_HEADER;
const TRACKS_TOP: f32 = TOP_PAD + BACK_ROW + BACK_GAP + TRACKS_HEADER + HEADER_GAP + LIST_HEADER;
const COLLECTION_TOP: f32 = TOP_PAD + COLLECTION_HEADER + HEADER_GAP;

/// Строк в списке с догрузкой: треки и, пока есть что грузить, строка
/// «Загружаем ещё».
fn paged_rows<T>(paged: &Paged<T>) -> usize {
    let tail = paged.fetching || paged.error.is_some() || paged.more();
    paged.items.len() + usize::from(tail)
}

impl Library {
    // ---------------------------------------------------------- размеры

    pub(super) fn grid_columns(&self) -> usize {
        grid(self.panel_width).0
    }

    fn grid_body(&self, top: f32, items: usize) -> Body {
        let (columns, side) = grid(self.panel_width);
        Body {
            top,
            rows: grid_rows(items, columns),
            row: side + CARD_TEXT + GRID_ROW_GAP,
        }
    }

    /// Прокручиваемая часть экрана, который сейчас открыт.
    pub(super) fn body(&self) -> Body {
        if let Some(entry) = self.stack.last() {
            return match &entry.page {
                Page::Album { load, .. } => Body {
                    top: DETAIL_TOP,
                    rows: load.ready().map_or(0, |album| album.tracks.len()),
                    row: ROW,
                },
                Page::Playlist { load, .. } => Body {
                    top: DETAIL_TOP,
                    rows: load.ready().map_or(0, |playlist| playlist.tracks.len()),
                    row: ROW,
                },
                Page::Artist { load, .. } => {
                    let artist = load.ready();
                    self.grid_body(
                        artist_top(artist),
                        artist.map_or(0, |artist| artist.albums.len()),
                    )
                }
                Page::ArtistTracks { load, .. } => Body {
                    top: TRACKS_TOP,
                    rows: load.ready().map_or(0, paged_rows),
                    row: ROW,
                },
            };
        }
        match self.section {
            Section::Liked => Body {
                top: 0.0,
                rows: self.shown_count(),
                row: ROW,
            },
            Section::Search => {
                let note = if self.search.corrected.is_some() {
                    SEARCH_NOTE
                } else {
                    0.0
                };
                match self.search.tab {
                    SearchKind::Tracks => Body {
                        top: note + LIST_HEADER,
                        rows: ready_paged(self.search.tracks.as_ref()).map_or(0, paged_rows),
                        row: ROW,
                    },
                    SearchKind::Albums => self.grid_body(
                        note,
                        ready_paged(self.search.albums.as_ref()).map_or(0, paged_rows),
                    ),
                    SearchKind::Artists => self.grid_body(
                        note,
                        ready_paged(self.search.artists.as_ref()).map_or(0, paged_rows),
                    ),
                    SearchKind::Playlists => self.grid_body(
                        note,
                        ready_paged(self.search.playlists.as_ref()).map_or(0, paged_rows),
                    ),
                }
            }
            Section::Playlists => self.grid_body(
                COLLECTION_TOP,
                ready_len(self.collections.playlists.as_ref()),
            ),
            Section::Albums => {
                self.grid_body(COLLECTION_TOP, ready_len(self.collections.albums.as_ref()))
            }
            Section::Artists => {
                self.grid_body(COLLECTION_TOP, ready_len(self.collections.artists.as_ref()))
            }
            _ => Body::EMPTY,
        }
    }

    /// Высота видимой части списка.
    fn list_view_height(&self) -> f32 {
        self.list_height.unwrap_or(self.viewport)
    }

    /// Диапазон строк, которые надо построить.
    pub(super) fn visible_range(&self) -> (usize, usize) {
        let body = self.body();
        let from = self.scroll_y - LIST_TOP - body.top;
        let first = rows_at(from, body.row).saturating_sub(OVERSCAN);
        let last = (rows_at(from + self.viewport, body.row) + 1 + OVERSCAN).min(body.rows);
        (first.min(last), last)
    }

    /// До какой строки список виден сейчас (без запаса).
    pub(super) fn last_visible_row(&self) -> usize {
        let body = self.body();
        let from = self.scroll_y - LIST_TOP - body.top;
        rows_at(from + self.list_view_height(), body.row) + 1
    }

    /// Дальше этого список не прокрутить. Пока `scrollable` не сообщил свою
    /// высоту, берём половину окна: цель поправится при первом сообщении.
    pub(super) fn max_scroll(&self) -> f32 {
        let height = self.list_height.unwrap_or(self.viewport * 0.5);
        (self.list_content() - height).max(0.0)
    }

    /// Что-то на экране загружается: нужны кадры для скелетона и спиннера.
    pub(super) fn loading_on_screen(&self) -> bool {
        if let Some(entry) = self.stack.last() {
            return match &entry.page {
                Page::Album { load, .. } => load.is_loading(),
                Page::Playlist { load, .. } => load.is_loading(),
                Page::Artist { load, .. } => load.is_loading(),
                Page::ArtistTracks { load, .. } => {
                    load.is_loading() || load.ready().is_some_and(|paged| paged.fetching)
                }
            };
        }
        let fetching = |paged: Option<&Paged<_>>| paged.is_some_and(|paged| paged.fetching);
        match self.section {
            Section::Search => {
                self.search.loading()
                    || match self.search.tab {
                        SearchKind::Tracks => fetching(ready_paged(self.search.tracks.as_ref())),
                        SearchKind::Albums => {
                            ready_paged(self.search.albums.as_ref()).is_some_and(|p| p.fetching)
                        }
                        SearchKind::Artists => {
                            ready_paged(self.search.artists.as_ref()).is_some_and(|p| p.fetching)
                        }
                        SearchKind::Playlists => {
                            ready_paged(self.search.playlists.as_ref()).is_some_and(|p| p.fetching)
                        }
                    }
            }
            Section::Playlists => loading(self.collections.playlists.as_ref()),
            Section::Albums => loading(self.collections.albums.as_ref()),
            Section::Artists => loading(self.collections.artists.as_ref()),
            _ => false,
        }
    }

    /// Какой экран на виду: раздел и номер открытой страницы. Сменился —
    /// экран проявляется.
    pub fn view_key(&self) -> (Section, u64) {
        (
            self.section,
            self.stack.last().map_or(0, |entry| entry.serial),
        )
    }

    /// Обложки, которые сейчас на экране: видимые строки, карточки, шапка и
    /// трек в плеере.
    pub fn visible_covers(&self) -> Vec<(Grade, &str)> {
        let (first, last) = self.visible_range();
        let mut uris: Vec<(Grade, &str)> = self
            .current()
            .and_then(|track| track.cover_uri.as_deref())
            .map(|uri| (Grade::Small, uri))
            .into_iter()
            .collect();
        let columns = self.grid_columns();
        let cards = |range: std::ops::Range<usize>| (range.start * columns)..(range.end * columns);
        if let Some(entry) = self.stack.last() {
            match &entry.page {
                Page::Album { load, .. } => {
                    if let Some(album) = load.ready() {
                        uris.extend(medium(album.card.cover_uri.as_deref()));
                        uris.extend(small(&album.tracks, first..last));
                    }
                }
                Page::Playlist { load, .. } => {
                    if let Some(playlist) = load.ready() {
                        uris.extend(medium(playlist.card.cover_uri.as_deref()));
                        uris.extend(small(&playlist.tracks, first..last));
                    }
                }
                Page::Artist { load, .. } => {
                    if let Some(artist) = load.ready() {
                        uris.extend(medium(artist.card.cover_uri.as_deref()));
                        uris.extend(small(&artist.popular, 0..POPULAR));
                        uris.extend(cards(first..last).filter_map(|index| {
                            artist
                                .albums
                                .get(index)
                                .and_then(|album| medium(album.cover_uri.as_deref()))
                        }));
                    }
                }
                Page::ArtistTracks { load, .. } => {
                    if let Some(paged) = load.ready() {
                        uris.extend(small(&paged.items, first..last));
                    }
                }
            }
            return uris;
        }
        match self.section {
            Section::Liked => {
                let tracks = self.tracks();
                uris.extend(
                    (first..last)
                        .filter_map(|row| self.shown_at(row))
                        .filter_map(|index| tracks.get(index))
                        .filter_map(|track| track.cover_uri.as_deref())
                        .map(|uri| (Grade::Small, uri)),
                );
            }
            Section::Search => match self.search.tab {
                SearchKind::Tracks => {
                    if let Some(paged) = ready_paged(self.search.tracks.as_ref()) {
                        uris.extend(small(&paged.items, first..last));
                    }
                }
                SearchKind::Albums => {
                    if let Some(paged) = ready_paged(self.search.albums.as_ref()) {
                        uris.extend(cards(first..last).filter_map(|index| {
                            paged
                                .items
                                .get(index)
                                .and_then(|card| medium(card.cover_uri.as_deref()))
                        }));
                    }
                }
                SearchKind::Artists => {
                    if let Some(paged) = ready_paged(self.search.artists.as_ref()) {
                        uris.extend(cards(first..last).filter_map(|index| {
                            paged
                                .items
                                .get(index)
                                .and_then(|card| medium(card.cover_uri.as_deref()))
                        }));
                    }
                }
                SearchKind::Playlists => {
                    if let Some(paged) = ready_paged(self.search.playlists.as_ref()) {
                        uris.extend(cards(first..last).filter_map(|index| {
                            paged
                                .items
                                .get(index)
                                .and_then(|card| medium(card.cover_uri.as_deref()))
                        }));
                    }
                }
            },
            Section::Playlists => {
                if let Some(Load::Ready(items)) = &self.collections.playlists {
                    uris.extend(cards(first..last).filter_map(|index| {
                        items
                            .get(index)
                            .and_then(|card| medium(card.cover_uri.as_deref()))
                    }));
                }
            }
            Section::Albums => {
                if let Some(Load::Ready(items)) = &self.collections.albums {
                    uris.extend(cards(first..last).filter_map(|index| {
                        items
                            .get(index)
                            .and_then(|card| medium(card.cover_uri.as_deref()))
                    }));
                }
            }
            Section::Artists => {
                if let Some(Load::Ready(items)) = &self.collections.artists {
                    uris.extend(cards(first..last).filter_map(|index| {
                        items
                            .get(index)
                            .and_then(|card| medium(card.cover_uri.as_deref()))
                    }));
                }
            }
            _ => {}
        }
        uris
    }

    // ---------------------------------------------------------- общий список

    /// Прокручиваемый список: блок `top` сверху и видимые строки `row(i)`,
    /// выше и ниже — пустое место нужной высоты. Колесо прокручивает плавно,
    /// полоса прокрутки своя.
    pub(super) fn scroller<'a>(
        &'a self,
        body: Body,
        top: Option<Element<'a, App>>,
        row: impl Fn(usize) -> Option<Element<'a, App>>,
    ) -> Element<'a, App> {
        let (first, last) = self.visible_range();
        let mut list = column![Space::new().height(LIST_TOP)];
        if let Some(top) = top {
            list = list.push(container(top).height(body.top).clip(true));
        }
        list = list.push(Space::new().height(span_of(first, body.row)));
        for index in first..last {
            if let Some(element) = row(index) {
                list = list.push(container(element).height(body.row));
            }
        }
        list = list.push(
            Space::new().height(span_of(body.rows.saturating_sub(last), body.row) + LIST_BOTTOM),
        );
        super::scrolling(self, list.into(), body.content())
    }

    // ---------------------------------------------------------- строки

    /// Строка трека `index` списка `key`, с номером `number`.
    pub(super) fn track_row<'a>(
        &'a self,
        key: &ListKey,
        index: usize,
        number: usize,
        track: &'a Track,
        covers: &'a Covers,
    ) -> Element<'a, App> {
        let current = self.is_current(key, index, track);
        let selected = self
            .selected
            .as_ref()
            .is_some_and(|(selected, at)| selected == key && *at == index);
        let liked = self.liked.contains(&track.id);
        let make = |hovered: bool| -> Element<'a, App> {
            container(track_cells(
                key,
                index,
                number,
                track,
                covers,
                Flags {
                    current,
                    playing: self.player.playing,
                    buffering: self.player.buffering,
                    hovered,
                    liked,
                },
            ))
            .height(ROW)
            .padding(Padding {
                top: 0.0,
                right: 16.0,
                bottom: 0.0,
                left: 8.0,
            })
            .center_y(ROW)
            .into()
        };
        // Фон строки проявляется плавно; выбранная строка подсвечена всегда.
        let (idle, hovered) = if selected {
            (theme::SURFACE_3, theme::SURFACE_3)
        } else {
            (theme::clear(theme::SURFACE_2), theme::SURFACE_2)
        };
        mouse_area(anim::hover_fill(
            anim::swap(make(false), make(true)),
            idle,
            hovered,
            widgets::RADIUS_M,
        ))
        .on_press(Lib(Message::Select(key.clone(), index)))
        .on_double_click(Lib(Message::PlayFrom(key.clone(), index)))
        .into()
    }

    /// Строка в конце списка с догрузкой: спиннер или ошибка с кнопкой.
    fn tail_row<'a>(&self, error: Option<&'a str>, retry: Message, turn: f32) -> Element<'a, App> {
        let content: Element<'a, App> = match error {
            Some(error) => row![
                icon(Icon::Error, 18.0, theme::ERROR),
                theme::line(
                    sentence(&format!("Не удалось догрузить: {error}")),
                    Style::Secondary,
                    theme::TEXT_SECONDARY
                ),
                widgets::action(
                    Kind::Secondary,
                    Some(Icon::Retry),
                    "Повторить",
                    Some(Lib(retry))
                ),
            ]
            .spacing(12)
            .align_y(Alignment::Center)
            .into(),
            None => widgets::waiting("Загружаем ещё…".to_owned(), turn),
        };
        container(content)
            .center_x(Length::Fill)
            .center_y(ROW)
            .into()
    }

    // ---------------------------------------------------------- страницы

    /// Страница из стопки.
    pub(super) fn page_view<'a>(
        &'a self,
        entry: &'a Entry,
        covers: &'a Covers,
        turn: f32,
        now: Instant,
    ) -> Element<'a, App> {
        match &entry.page {
            Page::Album { load, .. } => match load {
                Load::Ready(album) => self.album_page(entry.serial, album, covers),
                Load::Loading(since) => loading_page(*since, now, turn),
                Load::Failed(failure) => failed_page("Не удалось открыть альбом", failure),
            },
            Page::Playlist { load, .. } => match load {
                Load::Ready(playlist) => self.playlist_page(entry.serial, playlist, covers),
                Load::Loading(since) => loading_page(*since, now, turn),
                Load::Failed(failure) => failed_page("Не удалось открыть плейлист", failure),
            },
            Page::Artist { load, .. } => match load {
                Load::Ready(artist) => self.artist_page(entry.serial, artist, covers),
                Load::Loading(since) => loading_page(*since, now, turn),
                Load::Failed(failure) => failed_page("Не удалось открыть исполнителя", failure),
            },
            Page::ArtistTracks { id, name, load } => match load {
                Load::Ready(paged) => {
                    self.artist_tracks_page(entry.serial, id, name, paged, covers, turn)
                }
                Load::Loading(since) => loading_page(*since, now, turn),
                Load::Failed(failure) => failed_page("Не удалось загрузить треки", failure),
            },
        }
    }

    fn album_page<'a>(
        &'a self,
        serial: u64,
        album: &'a AlbumView,
        covers: &'a Covers,
    ) -> Element<'a, App> {
        let key = ListKey::Album(serial);
        let card = &album.card;
        let mut caption = card.kind.title().to_owned();
        if let Some(year) = card.year {
            caption.push_str(&format!(" · {year}"));
        }
        let header = detail_header(
            header_cover(covers, card.cover_uri.as_deref(), &card.id, false),
            caption,
            card.display_title(),
            Some(artist_links(
                &card.artists,
                Style::BodyStrong,
                theme::TEXT_PRIMARY,
            )),
            list_stats(&album.tracks),
            list_actions(&key, &album.tracks),
        );
        self.track_page(key, header, &album.tracks, covers, "В альбоме нет треков")
    }

    fn playlist_page<'a>(
        &'a self,
        serial: u64,
        playlist: &'a PlaylistView,
        covers: &'a Covers,
    ) -> Element<'a, App> {
        let key = ListKey::Playlist(serial);
        let card = &playlist.card;
        let byline: Element<'a, App> = theme::line(
            card.owner_name.clone().unwrap_or_default(),
            Style::BodyStrong,
            theme::TEXT_PRIMARY,
        )
        .into();
        let mut stats = list_stats(&playlist.tracks);
        if let Some(description) = &playlist.description {
            stats = format!("{stats} · {description}");
        }
        let header = detail_header(
            header_cover(
                covers,
                card.cover_uri.as_deref(),
                &format!("{}:{}", card.owner, card.kind),
                false,
            ),
            "Плейлист".to_owned(),
            card.title.clone(),
            Some(byline),
            stats,
            list_actions(&key, &playlist.tracks),
        );
        self.track_page(
            key,
            header,
            &playlist.tracks,
            covers,
            "В плейлисте нет треков",
        )
    }

    /// Страница со списком треков: шапка и строки под ней.
    fn track_page<'a>(
        &'a self,
        key: ListKey,
        header: Element<'a, App>,
        tracks: &'a [Track],
        covers: &'a Covers,
        empty: &'a str,
    ) -> Element<'a, App> {
        let top = column![
            Space::new().height(TOP_PAD),
            back_row(),
            Space::new().height(BACK_GAP),
            container(header).height(HEADER),
            Space::new().height(HEADER_GAP),
            list_header(),
        ];
        if tracks.is_empty() {
            return column![
                container(top).padding([0.0, PAGE_SIDE]),
                state_block(
                    widgets::badge(Icon::Empty, theme::TEXT_SECONDARY, theme::SURFACE_2),
                    empty,
                    "Здесь появятся треки, когда их добавят.".to_owned(),
                    Space::new().into(),
                ),
            ]
            .into();
        }
        self.scroller(self.body(), Some(top.into()), move |index| {
            let track = tracks.get(index)?;
            Some(self.track_row(&key, index, index + 1, track, covers))
        })
    }

    fn artist_page<'a>(
        &'a self,
        serial: u64,
        artist: &'a ArtistView,
        covers: &'a Covers,
    ) -> Element<'a, App> {
        let key = ListKey::ArtistPopular(serial);
        let card = &artist.card;
        let mut stats = Vec::new();
        if let Some(tracks) = card.tracks {
            stats.push(format!("{tracks} {}", texts::tracks_word(count(tracks))));
        }
        if let Some(albums) = card.albums {
            stats.push(format!(
                "{albums} {}",
                plural(count(albums), ["альбом", "альбома", "альбомов"])
            ));
        }
        if let Some(listeners) = artist.listeners {
            stats.push(format!(
                "{} {} в месяц",
                api::compact_number(listeners),
                if listeners < 10_000 {
                    plural(
                        usize::try_from(listeners).unwrap_or(usize::MAX),
                        ["слушатель", "слушателя", "слушателей"],
                    )
                } else {
                    "слушателей"
                }
            ));
        }
        let header = detail_header(
            header_cover(covers, card.cover_uri.as_deref(), &card.id, true),
            "Исполнитель".to_owned(),
            card.name.clone(),
            None,
            stats.join(" · "),
            list_actions(&key, &artist.popular),
        );
        let mut top = column![
            Space::new().height(TOP_PAD),
            back_row(),
            Space::new().height(BACK_GAP),
            container(header).height(HEADER),
        ];
        let popular = artist.popular.len().min(POPULAR);
        if popular > 0 {
            let all = Lib(Message::Open(Target::ArtistTracks {
                id: card.id.clone(),
                name: card.name.clone(),
            }));
            top = top
                .push(Space::new().height(SECTION_GAP))
                .push(section_title("Популярные треки", Some(("Все треки", all))))
                .push(Space::new().height(8));
            for (index, track) in artist.popular.iter().enumerate().take(POPULAR) {
                top = top.push(self.track_row(&key, index, index + 1, track, covers));
            }
        }
        if !artist.albums.is_empty() {
            top = top
                .push(Space::new().height(SECTION_GAP))
                .push(section_title("Альбомы", None))
                .push(Space::new().height(16));
        }
        let (columns, side) = grid(self.panel_width);
        let albums = &artist.albums;
        self.scroller(self.body(), Some(top.into()), move |row| {
            Some(cards_row(row, columns, side, albums, |album| {
                album_card(covers, album, side)
            }))
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn artist_tracks_page<'a>(
        &'a self,
        serial: u64,
        id: &'a str,
        name: &'a str,
        paged: &'a Paged<Track>,
        covers: &'a Covers,
        turn: f32,
    ) -> Element<'a, App> {
        let key = ListKey::ArtistTracks(serial);
        let header = column![
            widgets::link(
                name.to_owned(),
                Style::Body,
                Lib(Message::Open(Target::Artist(id.to_owned())))
            ),
            theme::line("Все треки", Style::Display, theme::TEXT_PRIMARY),
            theme::line(
                format!("{} {}", paged.total, texts::tracks_word(paged.total)),
                Style::Body,
                theme::TEXT_SECONDARY
            ),
            Space::new().height(10),
            list_actions(&key, &paged.items),
        ]
        .spacing(4);
        let top = column![
            Space::new().height(TOP_PAD),
            back_row(),
            Space::new().height(BACK_GAP),
            container(header)
                .height(TRACKS_HEADER)
                .center_y(TRACKS_HEADER),
            Space::new().height(HEADER_GAP),
            list_header(),
        ];
        let tracks = &paged.items;
        let error = paged.error.as_deref();
        self.scroller(self.body(), Some(top.into()), move |index| {
            match tracks.get(index) {
                Some(track) => Some(self.track_row(&key, index, index + 1, track, covers)),
                None => Some(self.tail_row(error, Message::RetryPage, turn)),
            }
        })
    }

    // ---------------------------------------------------------- коллекция

    pub(super) fn collection_view<'a>(
        &'a self,
        covers: &'a Covers,
        turn: f32,
        now: Instant,
    ) -> Element<'a, App> {
        let (columns, side) = grid(self.panel_width);
        match self.section {
            Section::Playlists => self.collection(
                self.collections.playlists.as_ref(),
                "Плейлисты",
                ["плейлист", "плейлиста", "плейлистов"],
                "Здесь появятся ваши плейлисты и те, что вам понравились.",
                (turn, now),
                move |row, items| {
                    cards_row(row, columns, side, items, |card| {
                        playlist_card(covers, card, side)
                    })
                },
            ),
            Section::Albums => self.collection(
                self.collections.albums.as_ref(),
                "Альбомы",
                ["альбом", "альбома", "альбомов"],
                "Отмечайте альбомы сердцем — они появятся здесь.",
                (turn, now),
                move |row, items| {
                    cards_row(row, columns, side, items, |card| {
                        album_card(covers, card, side)
                    })
                },
            ),
            _ => self.collection(
                self.collections.artists.as_ref(),
                "Исполнители",
                ["исполнитель", "исполнителя", "исполнителей"],
                "Отмечайте исполнителей сердцем — они появятся здесь.",
                (turn, now),
                move |row, items| {
                    cards_row(row, columns, side, items, |card| {
                        artist_card(covers, card, side)
                    })
                },
            ),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn collection<'a, T>(
        &'a self,
        load: Option<&'a Load<std::sync::Arc<Vec<T>>>>,
        title: &'a str,
        forms: [&'static str; 3],
        empty: &'a str,
        (turn, now): (f32, Instant),
        row: impl Fn(usize, &'a [T]) -> Element<'a, App> + 'a,
    ) -> Element<'a, App> {
        let items = match load {
            None => return Space::new().into(),
            Some(Load::Loading(since)) => return loading_grid(*since, now, turn),
            Some(Load::Failed(failure)) => {
                return failure_block(
                    &format!("Не удалось загрузить «{title}»"),
                    failure,
                    Some(Lib(Message::RetryCollection)),
                );
            }
            Some(Load::Ready(items)) => items,
        };
        let header = column![
            theme::line(title, Style::Display, theme::TEXT_PRIMARY),
            theme::line(
                format!("{} {}", items.len(), plural(items.len(), forms)),
                Style::Body,
                theme::TEXT_SECONDARY
            ),
        ]
        .spacing(4);
        let top = column![
            Space::new().height(TOP_PAD),
            container(header).height(COLLECTION_HEADER),
            Space::new().height(HEADER_GAP),
        ];
        if items.is_empty() {
            return column![
                container(top).padding([0.0, PAGE_SIDE]),
                state_block(
                    widgets::badge(Icon::Heart, theme::TEXT_SECONDARY, theme::SURFACE_2),
                    "Здесь пока пусто",
                    empty.to_owned(),
                    widgets::action(
                        Kind::Secondary,
                        Some(Icon::Search),
                        "Найти музыку",
                        Some(Lib(Message::Nav(Section::Search))),
                    ),
                ),
            ]
            .into();
        }
        let items: &'a [T] = items;
        self.scroller(self.body(), Some(top.into()), move |index| {
            Some(row(index, items))
        })
    }

    // ---------------------------------------------------------- поиск

    pub(super) fn search_view<'a>(
        &'a self,
        covers: &'a Covers,
        turn: f32,
        now: Instant,
    ) -> Element<'a, App> {
        let search = &self.search;
        let field = super::search_field(
            super::SearchField {
                id: super::pages::CATALOG_SEARCH_ID,
                placeholder: "Трек, альбом, исполнитель или плейлист",
                height: SEARCH_FIELD,
            },
            &search.input,
            |text| Lib(Message::SearchInput(text)),
            Some(Lib(Message::SearchSubmit)),
            Lib(Message::ClearCatalogSearch),
        );
        let tabs = row(SearchKind::ALL.map(|kind| tab_pill(kind, kind == search.tab)))
            .spacing(8)
            .align_y(Alignment::Center);
        let head = column![field, tabs].spacing(16).padding(Padding {
            top: PAGE_TOP,
            right: PAGE_SIDE,
            bottom: 8.0,
            left: PAGE_SIDE,
        });
        let body: Element<'a, App> = if search.query.is_empty() {
            state_block(
                widgets::badge(Icon::Search, theme::TEXT_SECONDARY, theme::SURFACE_2),
                "Поиск",
                "Найдите треки, альбомы, исполнителей и плейлисты.".to_owned(),
                Space::new().into(),
            )
        } else {
            self.search_results(covers, turn, now)
        };
        column![head, body].into()
    }

    fn search_results<'a>(
        &'a self,
        covers: &'a Covers,
        turn: f32,
        now: Instant,
    ) -> Element<'a, App> {
        let search = &self.search;
        let note: Option<Element<'a, App>> = search.corrected.as_ref().map(|corrected| {
            container(theme::line(
                format!("Показаны результаты по запросу «{corrected}»"),
                Style::Secondary,
                theme::TEXT_SECONDARY,
            ))
            .height(SEARCH_NOTE)
            .center_y(SEARCH_NOTE)
            .into()
        });
        let (columns, side) = grid(self.panel_width);
        let retry = Message::RetrySearch;
        match search.tab {
            SearchKind::Tracks => {
                let paged = match self.search_state(search.tracks.as_ref(), now, turn) {
                    Ok(paged) => paged,
                    Err(element) => return element,
                };
                let key = ListKey::SearchTracks(search.generation);
                let top = column![].extend(note).push(list_header());
                let tracks = &paged.items;
                let error = paged.error.as_deref();
                self.scroller(self.body(), Some(top.into()), move |index| {
                    match tracks.get(index) {
                        Some(track) => Some(self.track_row(&key, index, index + 1, track, covers)),
                        None => Some(self.tail_row(error, retry.clone(), turn)),
                    }
                })
            }
            SearchKind::Albums => {
                let paged = match self.search_state(search.albums.as_ref(), now, turn) {
                    Ok(paged) => paged,
                    Err(element) => return element,
                };
                let items = &paged.items;
                let error = paged.error.as_deref();
                self.scroller(self.body(), note, move |row| {
                    Some(
                        paged_cards_row(row, columns, side, items, |card| {
                            album_card(covers, card, side)
                        })
                        .unwrap_or_else(|| self.tail_row(error, retry.clone(), turn)),
                    )
                })
            }
            SearchKind::Artists => {
                let paged = match self.search_state(search.artists.as_ref(), now, turn) {
                    Ok(paged) => paged,
                    Err(element) => return element,
                };
                let items = &paged.items;
                let error = paged.error.as_deref();
                self.scroller(self.body(), note, move |row| {
                    Some(
                        paged_cards_row(row, columns, side, items, |card| {
                            artist_card(covers, card, side)
                        })
                        .unwrap_or_else(|| self.tail_row(error, retry.clone(), turn)),
                    )
                })
            }
            SearchKind::Playlists => {
                let paged = match self.search_state(search.playlists.as_ref(), now, turn) {
                    Ok(paged) => paged,
                    Err(element) => return element,
                };
                let items = &paged.items;
                let error = paged.error.as_deref();
                self.scroller(self.body(), note, move |row| {
                    Some(
                        paged_cards_row(row, columns, side, items, |card| {
                            playlist_card(covers, card, side)
                        })
                        .unwrap_or_else(|| self.tail_row(error, retry.clone(), turn)),
                    )
                })
            }
        }
    }

    /// Вкладка поиска: готовые результаты или то, что показать вместо них.
    fn search_state<'a, T>(
        &self,
        load: Option<&'a Load<Paged<T>>>,
        now: Instant,
        turn: f32,
    ) -> Result<&'a Paged<T>, Element<'a, App>> {
        match load {
            None => Err(Space::new().into()),
            Some(Load::Loading(since)) => Err(if elapsed(*since, now) >= SKELETON_AFTER {
                center(widgets::waiting("Ищем…".to_owned(), turn)).into()
            } else {
                Space::new().into()
            }),
            Some(Load::Failed(failure)) => Err(failure_block(
                "Поиск не удался",
                failure,
                Some(Lib(Message::RetrySearch)),
            )),
            Some(Load::Ready(paged)) if paged.items.is_empty() => Err(state_block(
                widgets::badge(Icon::Search, theme::TEXT_SECONDARY, theme::SURFACE_2),
                "Ничего не нашлось",
                format!(
                    "По запросу «{}» нет: {}.",
                    self.search.query,
                    self.search.tab.title().to_lowercase()
                ),
                Space::new().into(),
            )),
            Some(Load::Ready(paged)) => Ok(paged),
        }
    }
}

// ------------------------------------------------------------- помощники

fn ready_paged<T>(load: Option<&Load<Paged<T>>>) -> Option<&Paged<T>> {
    load.and_then(Load::ready)
}

fn ready_len<T>(load: Option<&Load<std::sync::Arc<Vec<T>>>>) -> usize {
    load.and_then(Load::ready).map_or(0, |items| items.len())
}

fn loading<T>(load: Option<&Load<T>>) -> bool {
    load.is_some_and(Load::is_loading)
}

/// Маленькие обложки треков `range` списка.
fn small(tracks: &[Track], range: std::ops::Range<usize>) -> Vec<(Grade, &str)> {
    range
        .filter_map(|index| tracks.get(index))
        .filter_map(|track| track.cover_uri.as_deref())
        .map(|uri| (Grade::Small, uri))
        .collect()
}

fn medium(uri: Option<&str>) -> Option<(Grade, &str)> {
    uri.map(|uri| (Grade::Medium, uri))
}

fn count(value: u32) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

fn elapsed(since: Instant, now: Instant) -> f32 {
    now.saturating_duration_since(since).as_secs_f32()
}

/// Обложка размера `grade` или заглушка, цвет которой задаёт `seed`.
fn art<'a>(covers: &'a Covers, grade: Grade, uri: Option<&str>, seed: &str) -> CoverArt<'a> {
    match covers.art_of(grade, uri) {
        Art::Ready(handle, opacity) => CoverArt::Image(handle, opacity),
        Art::Loading => CoverArt::Loading,
        Art::Missing => CoverArt::Placeholder(theme::cover_placeholder(seed)),
    }
}

fn header_cover<'a>(
    covers: &'a Covers,
    uri: Option<&str>,
    seed: &str,
    round: bool,
) -> Element<'a, App> {
    let radius = if round { HEADER_COVER / 2.0 } else { 12.0 };
    container(widgets::cover(
        art(covers, Grade::Medium, uri, seed),
        HEADER_COVER,
        radius,
    ))
    .style(move |_| container::Style {
        border: Border::default().rounded(radius),
        shadow: widgets::cover_shadow(),
        ..container::Style::default()
    })
    .into()
}

/// Исполнители через запятую, каждый с идентификатором — ссылка на его
/// страницу.
pub(super) fn artist_links<'a>(
    artists: &'a [ArtistRef],
    style: Style,
    color: Color,
) -> Element<'a, App> {
    if artists.is_empty() {
        return theme::line("Неизвестный исполнитель", style, color).into();
    }
    let mut spans: Vec<Span<'a, App, iced::Font>> = Vec::with_capacity(artists.len() * 2);
    for (index, artist) in artists.iter().enumerate() {
        if index > 0 {
            spans.push(span(", ").color(color));
        }
        let name = span(artist.name.as_str()).color(color);
        spans.push(match &artist.id {
            Some(id) => name.link(Lib(Message::Open(Target::Artist(id.clone())))),
            None => name,
        });
    }
    linked(spans, style)
}

/// Текст со ссылками в стиле `style`, одной строкой.
fn linked<'a>(spans: Vec<Span<'a, App, iced::Font>>, style: Style) -> Element<'a, App> {
    let (size, line, font) = style.metrics();
    rich_text(spans)
        .size(size)
        .line_height(iced::widget::text::LineHeight::Absolute(iced::Pixels(line)))
        .font(font)
        .wrapping(Wrapping::None)
        .on_link_click(|message| message)
        .into()
}

/// Название альбома — ссылка на его страницу.
fn album_link<'a>(track: &'a Track, color: Color) -> Element<'a, App> {
    let title = track.album_title.as_deref().unwrap_or_default();
    let text = span(title).color(color);
    let text = match &track.album_id {
        Some(id) => text.link(Lib(Message::Open(Target::Album(id.clone())))),
        None => text,
    };
    linked(vec![text], Style::Secondary)
}

/// Состояние строки трека.
#[derive(Clone, Copy)]
struct Flags {
    current: bool,
    playing: bool,
    buffering: bool,
    hovered: bool,
    liked: bool,
}

fn track_cells<'a>(
    key: &ListKey,
    index: usize,
    number: usize,
    track: &'a Track,
    covers: &'a Covers,
    flags: Flags,
) -> Element<'a, App> {
    let available = track.available;
    let (title_color, other_color) = match (available, flags.current) {
        (false, _) => (theme::TEXT_DISABLED, theme::TEXT_DISABLED),
        (true, true) => (theme::accent(), theme::TEXT_SECONDARY),
        (true, false) => (theme::TEXT_PRIMARY, theme::TEXT_SECONDARY),
    };
    let play = Message::PlayFrom(key.clone(), index);
    // Играющий: столбики, при наведении — пауза; на паузе — play акцентом.
    let index_cell: Element<'a, App> =
        match (available, flags.current, flags.playing, flags.hovered) {
            (true, true, true, true) => {
                super::play_cell(Icon::PauseFilled, theme::TEXT_PRIMARY, Message::TogglePlay)
            }
            (true, true, true, false) => anim::equalizer(theme::accent(), !flags.buffering),
            (true, true, false, _) => {
                super::play_cell(Icon::PlayFilled, theme::accent(), Message::TogglePlay)
            }
            (true, false, _, true) => super::play_cell(Icon::PlayFilled, theme::TEXT_PRIMARY, play),
            _ => theme::line(number.to_string(), Style::Secondary, theme::TEXT_TERTIARY).into(),
        };
    let cover = if available {
        widgets::cover(super::cover_art(covers, track), 40.0, widgets::RADIUS_S)
    } else {
        widgets::block(40, 40, theme::SURFACE_3, widgets::RADIUS_S)
    };
    let title = column![
        theme::line(track.display_title(), Style::Body, title_color),
        artist_links(&track.artists, Style::Secondary, other_color),
    ]
    .spacing(2);
    let like: Element<'a, App> = if flags.liked {
        icon(
            Icon::HeartFilled,
            18.0,
            if available {
                theme::accent()
            } else {
                theme::TEXT_DISABLED
            },
        )
        .into()
    } else {
        Space::new().into()
    };
    let duration = theme::line(
        track.duration_text(),
        Style::Secondary,
        theme::TEXT_TERTIARY,
    );
    let cells = super::columns(
        index_cell,
        cover,
        title.into(),
        album_link(track, other_color),
        like,
        duration.into(),
    );
    if available {
        cells
    } else {
        widgets::with_tooltip(
            cells,
            "Трек недоступен: его убрали из каталога или он недоступен в вашем регионе",
            tooltip::Position::FollowCursor,
        )
    }
}

/// Шапка страницы: обложка слева, справа подпись, название, автор,
/// сведения и кнопки.
fn detail_header<'a>(
    cover: Element<'a, App>,
    caption: String,
    title: String,
    byline: Option<Element<'a, App>>,
    stats: String,
    actions: Element<'a, App>,
) -> Element<'a, App> {
    let info = column![
        theme::line(caption, Style::BodyStrong, theme::TEXT_SECONDARY),
        theme::line(title, Style::Display, theme::TEXT_PRIMARY),
    ]
    .extend(byline)
    .push(theme::line(stats, Style::Body, theme::TEXT_SECONDARY))
    .push(Space::new().height(10))
    .push(actions)
    .spacing(4)
    .width(Length::Fill);
    let info = container(info).width(Length::Fill).clip(true);
    row![cover, info]
        .spacing(28)
        .align_y(Alignment::Center)
        .height(HEADER)
        .into()
}

/// «21 трек · 51 мин».
fn list_stats(tracks: &[Track]) -> String {
    let count = tracks.len();
    let mut text = format!("{count} {}", texts::tracks_word(count));
    if count > 0 {
        text.push_str(&format!(
            " · {}",
            texts::total_duration(api::total_duration(tracks))
        ));
    }
    text
}

/// «Слушать» и «Перемешать». Нет доступных треков — кнопки недоступны.
fn list_actions<'a>(key: &ListKey, tracks: &[Track]) -> Element<'a, App> {
    let playable = tracks.iter().any(|track| track.available);
    row![
        widgets::action(
            Kind::Primary,
            Some(Icon::PlayFilled),
            "Слушать",
            playable.then(|| Lib(Message::PlayList(key.clone()))),
        ),
        widgets::action(
            Kind::Secondary,
            Some(Icon::Shuffle),
            "Перемешать",
            playable.then(|| Lib(Message::ShuffleList(key.clone()))),
        ),
    ]
    .spacing(10)
    .align_y(Alignment::Center)
    .into()
}

/// Кнопка «Назад».
fn back_row<'a>() -> Element<'a, App> {
    container(widgets::with_tooltip(
        widgets::icon_button(Icon::Back, Size::M, false, Some(Lib(Message::Back))),
        "Назад",
        tooltip::Position::Right,
    ))
    .height(BACK_ROW)
    .into()
}

/// Заголовок блока и ссылка справа («Все треки»).
fn section_title<'a>(title: &'a str, link: Option<(&'a str, App)>) -> Element<'a, App> {
    let mut line = row![
        theme::line(title, Style::Title, theme::TEXT_PRIMARY),
        Space::new().width(Length::Fill),
    ]
    .align_y(Alignment::Center);
    if let Some((caption, message)) = link {
        line = line.push(widgets::link(
            caption.to_owned(),
            Style::BodyStrong,
            message,
        ));
    }
    container(line)
        .height(SECTION_TITLE)
        .center_y(SECTION_TITLE)
        .into()
}

/// Вкладка поиска — «таблетка».
fn tab_pill<'a>(kind: SearchKind, selected: bool) -> Element<'a, App> {
    let color = if selected {
        theme::TEXT_PRIMARY
    } else {
        theme::TEXT_SECONDARY
    };
    let pill = button(theme::line(kind.title(), Style::BodyStrong, color))
        .padding([8, 16])
        .style(move |_, status| button::Style {
            background: match (selected, status) {
                (true, _) | (false, button::Status::Pressed) => Some(theme::SURFACE_3.into()),
                _ => None,
            },
            border: Border::default().rounded(widgets::ROUND),
            ..button::Style::default()
        })
        .on_press(Lib(Message::SearchTab(kind)));
    if selected {
        pill.into()
    } else {
        anim::hover_fill(
            pill,
            theme::clear(theme::SURFACE_2),
            theme::SURFACE_2,
            widgets::ROUND,
        )
        .into()
    }
}

/// Ряд `row` сетки: карточки `columns` штук шириной `side`.
fn cards_row<'a, T>(
    row: usize,
    columns: usize,
    side: f32,
    items: &'a [T],
    card: impl Fn(&'a T) -> Element<'a, App>,
) -> Element<'a, App> {
    let start = row.saturating_mul(columns);
    let cards = items
        .iter()
        .skip(start)
        .take(columns)
        .map(card)
        .collect::<Vec<_>>();
    let _ = side;
    iced::widget::Row::with_children(cards)
        .spacing(GRID_GAP)
        .into()
}

/// Ряд сетки с догрузкой: `None` — ряд за последней карточкой (место для
/// строки «Загружаем ещё»).
fn paged_cards_row<'a, T>(
    row: usize,
    columns: usize,
    side: f32,
    items: &'a [T],
    card: impl Fn(&'a T) -> Element<'a, App>,
) -> Option<Element<'a, App>> {
    (row.saturating_mul(columns) < items.len()).then(|| cards_row(row, columns, side, items, card))
}

/// Карточка: обложка, название, подпись. Щелчок открывает страницу, кнопка
/// Play при наведении — сразу включает.
fn card<'a>(
    cover: Element<'a, App>,
    side: f32,
    title: String,
    subtitle: String,
    open: Target,
) -> Element<'a, App> {
    let base = column![
        cover,
        Space::new().height(10),
        theme::line(title, Style::BodyStrong, theme::TEXT_PRIMARY),
        Space::new().height(2),
        theme::line(subtitle, Style::Secondary, theme::TEXT_SECONDARY),
    ]
    .width(side)
    .clip(true);
    let play = button(center(icon(Icon::PlayFilled, 20.0, theme::on_accent())))
        .width(44)
        .height(44)
        .padding(0)
        .style(|_, status| button::Style {
            background: Some(
                match status {
                    button::Status::Hovered => theme::accent_hover(),
                    button::Status::Pressed => theme::accent_pressed(),
                    _ => theme::accent(),
                }
                .into(),
            ),
            border: Border::default().rounded(widgets::ROUND),
            shadow: widgets::raised_shadow(),
            ..button::Style::default()
        })
        .on_press(Lib(Message::PlayCard(open.clone())));
    let overlay = column![
        container(widgets::with_tooltip(
            play,
            "Слушать",
            tooltip::Position::Top
        ))
        .width(side)
        .height(side)
        .padding(10)
        .align_x(Alignment::End)
        .align_y(Alignment::End),
    ]
    .width(side);
    mouse_area(hover(base, overlay))
        .interaction(mouse::Interaction::Pointer)
        .on_press(Lib(Message::Open(open)))
        .into()
}

fn album_card<'a>(covers: &'a Covers, album: &'a AlbumCard, side: f32) -> Element<'a, App> {
    let mut parts = Vec::new();
    if album.kind != AlbumKind::Album {
        parts.push(album.kind.title().to_owned());
    }
    if let Some(year) = album.year {
        parts.push(year.to_string());
    }
    if !album.artists.is_empty() {
        parts.push(api::artists_text(&album.artists));
    }
    card(
        widgets::cover(
            art(covers, Grade::Medium, album.cover_uri.as_deref(), &album.id),
            side,
            12.0,
        ),
        side,
        album.display_title(),
        parts.join(" · "),
        Target::Album(album.id.clone()),
    )
}

fn artist_card<'a>(covers: &'a Covers, artist: &'a ArtistCard, side: f32) -> Element<'a, App> {
    card(
        widgets::cover(
            art(
                covers,
                Grade::Medium,
                artist.cover_uri.as_deref(),
                &artist.id,
            ),
            side,
            side / 2.0,
        ),
        side,
        artist.name.clone(),
        "Исполнитель".to_owned(),
        Target::Artist(artist.id.clone()),
    )
}

fn playlist_card<'a>(
    covers: &'a Covers,
    playlist: &'a PlaylistCard,
    side: f32,
) -> Element<'a, App> {
    let mut parts = Vec::new();
    if let Some(owner) = &playlist.owner_name {
        parts.push(owner.clone());
    }
    if let Some(count) = playlist.track_count {
        let count = usize::try_from(count).unwrap_or(usize::MAX);
        parts.push(format!("{count} {}", texts::tracks_word(count)));
    }
    let seed = format!("{}:{}", playlist.owner, playlist.kind);
    card(
        widgets::cover(
            art(covers, Grade::Medium, playlist.cover_uri.as_deref(), &seed),
            side,
            12.0,
        ),
        side,
        playlist.title.clone(),
        parts.join(" · "),
        Target::Playlist {
            owner: playlist.owner.clone(),
            kind: playlist.kind.clone(),
        },
    )
}

/// Страница загружается: кнопка «Назад» сразу, скелетон — если ответ
/// задерживается.
fn loading_page<'a>(since: Instant, now: Instant, turn: f32) -> Element<'a, App> {
    let back = container(back_row()).padding(Padding {
        top: PAGE_TOP,
        right: PAGE_SIDE,
        bottom: 0.0,
        left: PAGE_SIDE,
    });
    if elapsed(since, now) < SKELETON_AFTER {
        return back.into();
    }
    column![back, skeleton(turn)].into()
}

fn loading_grid<'a>(since: Instant, now: Instant, turn: f32) -> Element<'a, App> {
    if elapsed(since, now) < SKELETON_AFTER {
        return Space::new().into();
    }
    center(widgets::waiting("Загружаем…".to_owned(), turn)).into()
}

fn failed_page<'a>(title: &'a str, failure: &Failure) -> Element<'a, App> {
    column![
        container(back_row()).padding(Padding {
            top: PAGE_TOP,
            right: PAGE_SIDE,
            bottom: 0.0,
            left: PAGE_SIDE,
        }),
        failure_block(title, failure, Some(Lib(Message::RetryPage))),
    ]
    .into()
}

/// Блок ошибки: без сети — свой значок и текст, «не найдено» — без
/// повтора.
fn failure_block<'a>(title: &str, failure: &Failure, retry: Option<App>) -> Element<'a, App> {
    let (badge, text) = if failure.kind == FailureKind::Offline {
        (
            Icon::Offline,
            "Нет соединения с сервером. Проверьте интернет или прокси и попробуйте ещё раз."
                .to_owned(),
        )
    } else {
        (Icon::Error, sentence(&failure.text))
    };
    let gone = failure.text.starts_with("не найдено") || failure.text.starts_with("нет доступа");
    let action = match retry.filter(|_| !gone) {
        Some(retry) => widgets::action(Kind::Primary, Some(Icon::Retry), "Повторить", Some(retry)),
        None => widgets::action(
            Kind::Secondary,
            Some(Icon::Back),
            "Назад",
            Some(Lib(Message::Back)),
        ),
    };
    super::state_block(
        widgets::badge(badge, theme::ERROR, theme::ERROR_SOFT),
        title.to_owned(),
        text,
        action,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_ignore_nonsense() {
        assert_eq!(rows_at(f32::NAN, ROW), 0);
        assert_eq!(rows_at(-100.0, ROW), 0);
        assert_eq!(rows_at(100.0, 0.0), 0);
        assert_eq!(rows_at(ROW * 3.5, ROW), 3);
    }

    #[test]
    fn grid_fits_any_width() {
        for width in [0.0, 1.0, 400.0, 728.0, 1048.0, 4000.0, f32::NAN] {
            let (columns, side) = grid(width);
            assert!((2..=MAX_COLUMNS).contains(&columns), "{width}: {columns}");
            assert!(side >= 1.0 || side.is_nan(), "{width}: {side}");
        }
        // Окно 1280 с развёрнутым меню: пять колонок, как на макете.
        assert_eq!(grid(1280.0 - super::super::SIDEBAR - 1.0).0, 5);
        assert_eq!(grid_rows(11, 5), 3);
        assert_eq!(grid_rows(3, 0), 3);
    }
}
