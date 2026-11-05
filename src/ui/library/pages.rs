//! Страницы главного окна поверх разделов меню: альбом, плейлист,
//! исполнитель, все треки исполнителя. И разделы, которым нужны данные
//! каталога: поиск и коллекция (плейлисты, альбомы, исполнители).
//!
//! Открытые страницы лежат стопкой: «Назад» возвращает на предыдущую с той
//! же прокруткой. Раздел меню открывает свою стопку заново. Ответ для
//! страницы, с которой уже ушли, кладётся в её запись, если она ещё в
//! стопке, иначе отбрасывается.

use std::sync::Arc;
use std::time::Duration;

use iced::Task;
use iced::time::Instant;

use super::{Effect, Library, Message, Section};
use crate::api::{
    self, AlbumCard, ArtistCard, Found, PlaylistCard, SEARCH_LAST_PAGE, SearchKind, Track,
};
use crate::ui::queue::{ListKey, Queue};
use crate::ui::{Failure, FailureKind};

/// Через сколько после ввода искать: пока печатают, запросы не уходят.
const SEARCH_SETTLE: Duration = Duration::from_millis(350);
/// Сколько строк до конца списка оставить, чтобы догрузить следующую страницу.
const MORE_AHEAD: usize = 12;
/// Поле поиска по каталогу.
pub const CATALOG_SEARCH_ID: &str = "catalog-search";

/// Куда перейти.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Album(String),
    Artist(String),
    Playlist {
        owner: String,
        kind: String,
    },
    ArtistTracks {
        id: String,
        name: String,
    },
    /// Полный топ исполнителей по истории прослушивания.
    TopArtists,
}

/// Данные загружаются, пришли или не пришли.
pub enum Load<T> {
    Loading(Instant),
    Ready(T),
    Failed(Failure),
}

impl<T> Load<T> {
    pub fn ready(&self) -> Option<&T> {
        match self {
            Self::Ready(value) => Some(value),
            _ => None,
        }
    }

    pub fn is_loading(&self) -> bool {
        matches!(self, Self::Loading(_))
    }
}

/// Список, который приходит страницами.
pub struct Paged<T> {
    pub items: Arc<Vec<T>>,
    pub total: usize,
    /// Номер следующей страницы.
    pub next: u32,
    /// Следующая страница уже запрошена.
    pub fetching: bool,
    /// Следующая страница не пришла: дальше не грузим, пока не попросят.
    pub error: Option<String>,
    /// Последняя страница, которую можно запросить.
    pub last: u32,
}

impl<T> Paged<T> {
    /// Есть что догружать.
    pub fn more(&self) -> bool {
        self.items.len() < self.total && self.next <= self.last && self.error.is_none()
    }
}

impl<T: Clone> Paged<T> {
    fn first(items: Vec<T>, total: usize, last: u32) -> Self {
        Self {
            total: total.max(items.len()),
            items: Arc::new(items),
            next: 1,
            fetching: false,
            error: None,
            last,
        }
    }

    /// Пришла следующая страница. Пустая — список кончился.
    fn append(&mut self, items: Vec<T>, total: usize) {
        self.fetching = false;
        self.next = self.next.saturating_add(1);
        if items.is_empty() {
            self.total = self.items.len();
            return;
        }
        let mut all = Vec::with_capacity(self.items.len() + items.len());
        all.extend(self.items.iter().cloned());
        all.extend(items);
        self.total = total.max(all.len());
        self.items = Arc::new(all);
    }
}

pub struct AlbumView {
    pub card: AlbumCard,
    pub tracks: Arc<Vec<Track>>,
}

pub struct PlaylistView {
    pub card: PlaylistCard,
    pub description: Option<String>,
    pub tracks: Arc<Vec<Track>>,
}

pub struct ArtistView {
    pub card: ArtistCard,
    pub listeners: Option<u64>,
    pub popular: Arc<Vec<Track>>,
    pub albums: Arc<Vec<AlbumCard>>,
}

/// Страница поверх раздела.
pub enum Page {
    Album {
        id: String,
        load: Load<AlbumView>,
    },
    Playlist {
        owner: String,
        kind: String,
        load: Load<PlaylistView>,
    },
    Artist {
        id: String,
        load: Load<ArtistView>,
    },
    ArtistTracks {
        id: String,
        name: String,
        load: Load<Paged<Track>>,
    },
    TopArtists {
        load: Load<api::ListeningTop>,
    },
}

/// Запись в стопке: страница, её номер и прокрутка, когда с неё ушли.
pub struct Entry {
    pub serial: u64,
    pub page: Page,
    pub scroll: f32,
    /// Включить, как только загрузится (кнопка Play на карточке).
    pub autoplay: bool,
}

/// Что пришло для страницы.
#[derive(Clone)]
pub enum Loaded {
    Album(Result<api::Album, Failure>),
    Playlist(Result<api::Playlist, Failure>),
    Artist(Result<api::Artist, Failure>),
    Tracks(Result<api::Page<Track>, Failure>),
    Top(Result<api::ListeningTop, Failure>),
}

/// Что пришло для раздела коллекции.
#[derive(Clone)]
pub enum Collected {
    Playlists(Result<api::MyPlaylists, Failure>),
    Albums(Result<api::MyAlbums, Failure>),
    Artists(Result<api::MyArtists, Failure>),
}

/// Вкладка списка в разделе «Плейлисты».
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlaylistTab {
    #[default]
    Own,
    Liked,
}

/// Коллекция: загружается при первом открытии раздела.
#[derive(Default)]
pub struct Collections {
    pub playlists: Option<Load<Arc<api::MyPlaylists>>>,
    pub albums: Option<Load<Arc<api::MyAlbums>>>,
    pub artists: Option<Load<Arc<api::MyArtists>>>,
    pub playlist_tab: PlaylistTab,
    /// Полки, раскрытые целиком.
    pub expanded: Vec<ShelfId>,
}

/// Полка раздела коллекции.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShelfId {
    Top,
    Met,
    Releases,
    Personal,
}

impl Collections {
    pub fn is_expanded(&self, shelf: ShelfId) -> bool {
        self.expanded.contains(&shelf)
    }

    /// Раскрыть полку или свернуть обратно.
    pub fn toggle(&mut self, shelf: ShelfId) {
        if self.is_expanded(shelf) {
            self.expanded.retain(|open| *open != shelf);
        } else {
            self.expanded.push(shelf);
        }
    }

    /// Плейлисты выбранной вкладки.
    pub fn playlist_list(&self) -> Option<&[PlaylistCard]> {
        let playlists = self.playlists.as_ref()?.ready()?;
        Some(match self.playlist_tab {
            PlaylistTab::Own => &playlists.own,
            PlaylistTab::Liked => &playlists.liked,
        })
    }
}

/// Поиск по каталогу.
pub struct SearchState {
    /// Текст в поле.
    pub input: String,
    /// Запрос, по которому показаны результаты.
    pub query: String,
    /// Номер правки поля: искать, только если после неё ничего не меняли.
    edit: u64,
    /// Номер запроса: ответы на прежние запросы отбрасываются.
    pub generation: u64,
    pub tab: SearchKind,
    /// Сервис исправил опечатку: по какому запросу показаны результаты.
    pub corrected: Option<String>,
    pub tracks: Option<Load<Paged<Track>>>,
    pub albums: Option<Load<Paged<AlbumCard>>>,
    pub artists: Option<Load<Paged<ArtistCard>>>,
    pub playlists: Option<Load<Paged<PlaylistCard>>>,
}

impl SearchState {
    pub fn new() -> Self {
        Self {
            input: String::new(),
            query: String::new(),
            edit: 0,
            generation: 0,
            tab: SearchKind::Tracks,
            corrected: None,
            tracks: None,
            albums: None,
            artists: None,
            playlists: None,
        }
    }

    fn clear_results(&mut self) {
        self.corrected = None;
        self.tracks = None;
        self.albums = None;
        self.artists = None;
        self.playlists = None;
    }

    /// Вкладка ещё не запрашивалась для текущего запроса.
    fn untouched(&self, kind: SearchKind) -> bool {
        match kind {
            SearchKind::Tracks => self.tracks.is_none(),
            SearchKind::Albums => self.albums.is_none(),
            SearchKind::Artists => self.artists.is_none(),
            SearchKind::Playlists => self.playlists.is_none(),
        }
    }

    fn set_loading(&mut self, kind: SearchKind, now: Instant) {
        match kind {
            SearchKind::Tracks => self.tracks = Some(Load::Loading(now)),
            SearchKind::Albums => self.albums = Some(Load::Loading(now)),
            SearchKind::Artists => self.artists = Some(Load::Loading(now)),
            SearchKind::Playlists => self.playlists = Some(Load::Loading(now)),
        }
    }

    /// Идёт ли загрузка первой страницы текущей вкладки.
    pub fn loading(&self) -> bool {
        match self.tab {
            SearchKind::Tracks => self.tracks.as_ref().is_some_and(Load::is_loading),
            SearchKind::Albums => self.albums.as_ref().is_some_and(Load::is_loading),
            SearchKind::Artists => self.artists.as_ref().is_some_and(Load::is_loading),
            SearchKind::Playlists => self.playlists.as_ref().is_some_and(Load::is_loading),
        }
    }
}

/// Кладёт ответ поиска в вкладку: первую страницу или следующую.
fn store<T: Clone>(
    slot: &mut Option<Load<Paged<T>>>,
    page: u32,
    result: Result<api::Page<T>, Failure>,
) -> Option<Failure> {
    match (slot.as_mut(), result) {
        (Some(Load::Ready(paged)), Ok(found)) if page > 0 => {
            paged.append(found.items, found.total);
            None
        }
        (Some(Load::Ready(paged)), Err(failure)) if page > 0 => {
            paged.fetching = false;
            paged.error = Some(failure.text.clone());
            Some(failure)
        }
        (_, Ok(found)) => {
            *slot = Some(Load::Ready(Paged::first(
                found.items,
                found.total,
                SEARCH_LAST_PAGE,
            )));
            None
        }
        (_, Err(failure)) => {
            *slot = Some(Load::Failed(failure.clone()));
            Some(failure)
        }
    }
}

/// Запросить следующую страницу, если она нужна: `shown` — до какой строки
/// список уже виден.
fn wants_more<T>(slot: Option<&mut Load<Paged<T>>>, shown: usize) -> Option<u32> {
    let Some(Load::Ready(paged)) = slot else {
        return None;
    };
    if paged.fetching || !paged.more() || shown + MORE_AHEAD < paged.items.len() {
        return None;
    }
    paged.fetching = true;
    Some(paged.next)
}

impl Library {
    // ---------------------------------------------------------- переходы

    pub(super) fn next_serial(&mut self) -> u64 {
        self.serial = self.serial.wrapping_add(1);
        self.serial
    }

    /// Открыть страницу поверх текущей.
    pub(super) fn open(
        &mut self,
        target: Target,
        autoplay: bool,
        http: Option<&reqwest::Client>,
    ) -> Task<Message> {
        // Та же страница уже открыта: только включить, если просили.
        if let Some(entry) = self.stack.last_mut()
            && same_target(&entry.page, &target)
        {
            if autoplay
                && entry.page_ready()
                && let Some(key) = entry_key(entry)
            {
                return Task::done(Message::PlayList(key));
            }
            entry.autoplay |= autoplay;
            return Task::none();
        }
        self.remember_scroll();
        let serial = self.next_serial();
        let now = Instant::now();
        let page = match &target {
            Target::Album(id) => Page::Album {
                id: id.clone(),
                load: Load::Loading(now),
            },
            Target::Artist(id) => Page::Artist {
                id: id.clone(),
                load: Load::Loading(now),
            },
            Target::Playlist { owner, kind } => Page::Playlist {
                owner: owner.clone(),
                kind: kind.clone(),
                load: Load::Loading(now),
            },
            Target::ArtistTracks { id, name } => Page::ArtistTracks {
                id: id.clone(),
                name: name.clone(),
                load: Load::Loading(now),
            },
            Target::TopArtists => Page::TopArtists {
                load: Load::Loading(now),
            },
        };
        self.stack.push(Entry {
            serial,
            page,
            scroll: 0.0,
            autoplay,
        });
        self.selected = None;
        let scroll = self.jump(0.0);
        Task::batch([self.load_target(serial, &target, 0, http), scroll])
    }

    /// Назад: на предыдущую страницу или в раздел. `None` — некуда.
    pub fn back(&mut self) -> Option<Task<Message>> {
        self.stack.pop()?;
        self.selected = None;
        let scroll = self
            .stack
            .last()
            .map_or(self.root_scroll, |entry| entry.scroll);
        Some(self.jump(scroll))
    }

    /// Раздел меню: стопка страниц закрывается.
    pub(super) fn nav(
        &mut self,
        section: Section,
        http: Option<&reqwest::Client>,
    ) -> Task<Message> {
        let changed = section != self.section || !self.stack.is_empty();
        self.stack.clear();
        self.section = section;
        let mut tasks = Vec::new();
        if changed {
            self.selected = None;
            tasks.push(self.jump(0.0));
        }
        tasks.push(self.load_collection(section, false, http));
        if section == Section::Search {
            tasks.push(iced::widget::operation::focus(CATALOG_SEARCH_ID));
        }
        Task::batch(tasks)
    }

    /// Прокрутка текущей страницы запоминается в её записи.
    fn remember_scroll(&mut self) {
        let scroll = self.scroll_y;
        match self.stack.last_mut() {
            Some(entry) => entry.scroll = scroll,
            None => self.root_scroll = scroll,
        }
    }

    /// Список сразу на `offset`, без плавной прокрутки.
    pub(super) fn jump(&mut self, offset: f32) -> Task<Message> {
        self.scroll_y = offset;
        self.glide = None;
        self.list_height = None;
        super::scroll_list(offset)
    }

    pub(super) fn find_entry(&mut self, serial: u64) -> Option<&mut Entry> {
        self.stack.iter_mut().find(|entry| entry.serial == serial)
    }

    pub(super) fn entry(&self, serial: u64) -> Option<&Entry> {
        self.stack.iter().find(|entry| entry.serial == serial)
    }

    // ---------------------------------------------------------- загрузка

    fn load_target(
        &self,
        serial: u64,
        target: &Target,
        page: u32,
        http: Option<&reqwest::Client>,
    ) -> Task<Message> {
        let Some(http) = http.cloned() else {
            let failure = no_network();
            let loaded = match target {
                Target::Album(_) => Loaded::Album(Err(failure)),
                Target::Artist(_) => Loaded::Artist(Err(failure)),
                Target::Playlist { .. } => Loaded::Playlist(Err(failure)),
                Target::ArtistTracks { .. } => Loaded::Tracks(Err(failure)),
                Target::TopArtists => Loaded::Top(Err(failure)),
            };
            return Task::done(Message::PageLoaded(serial, page, loaded));
        };
        let token = self.token.clone();
        let target = target.clone();
        Task::perform(
            async move {
                let token = &token.access_token;
                match target {
                    Target::Album(id) => Loaded::Album(
                        api::fetch_album(&http, token, &id)
                            .await
                            .map_err(Failure::from),
                    ),
                    Target::Artist(id) => Loaded::Artist(
                        api::fetch_artist(&http, token, &id)
                            .await
                            .map_err(Failure::from),
                    ),
                    Target::Playlist { owner, kind } => Loaded::Playlist(
                        api::fetch_playlist(&http, token, &owner, &kind)
                            .await
                            .map_err(Failure::from),
                    ),
                    Target::ArtistTracks { id, .. } => Loaded::Tracks(
                        api::fetch_artist_tracks(&http, token, &id, page)
                            .await
                            .map_err(Failure::from),
                    ),
                    Target::TopArtists => Loaded::Top(
                        api::fetch_listening_top(&http, token)
                            .await
                            .map_err(Failure::from),
                    ),
                }
            },
            move |loaded| Message::PageLoaded(serial, page, loaded),
        )
    }

    /// «Повторить» на странице с ошибкой.
    pub(super) fn retry_page(&mut self, http: Option<&reqwest::Client>) -> Task<Message> {
        let Some(entry) = self.stack.last_mut() else {
            return Task::none();
        };
        let target = entry_target(&entry.page);
        let serial = entry.serial;
        let now = Instant::now();
        let page = match &mut entry.page {
            Page::Album { load, .. } => reload(load, now),
            Page::Playlist { load, .. } => reload(load, now),
            Page::Artist { load, .. } => reload(load, now),
            // Ошибка догрузки: продолжаем с той же страницы.
            Page::ArtistTracks {
                load: Load::Ready(paged),
                ..
            } => {
                paged.error = None;
                paged.fetching = true;
                paged.next
            }
            Page::ArtistTracks { load, .. } => reload(load, now),
            Page::TopArtists { load } => reload(load, now),
        };
        self.load_target(serial, &target, page, http)
    }

    /// Пришли данные страницы `serial`.
    pub(super) fn page_loaded(
        &mut self,
        serial: u64,
        page: u32,
        loaded: Loaded,
        http: Option<&reqwest::Client>,
    ) -> (Task<Message>, Option<Effect>) {
        let unauthorized = match &loaded {
            Loaded::Album(Err(failure))
            | Loaded::Playlist(Err(failure))
            | Loaded::Artist(Err(failure))
            | Loaded::Tracks(Err(failure))
            | Loaded::Top(Err(failure)) => failure.kind == FailureKind::Unauthorized,
            _ => false,
        };
        if unauthorized {
            return (Task::none(), Some(Effect::Relogin));
        }
        let mut grown: Option<Arc<Vec<Track>>> = None;
        let mut toast = None;
        let Some(entry) = self.find_entry(serial) else {
            return (Task::none(), None);
        };
        match (&mut entry.page, loaded) {
            (Page::Album { load, .. }, Loaded::Album(result)) => {
                *load = match result {
                    Ok(album) => Load::Ready(AlbumView {
                        card: album.card,
                        tracks: Arc::new(album.tracks),
                    }),
                    Err(failure) => Load::Failed(failure),
                };
            }
            (Page::Playlist { load, .. }, Loaded::Playlist(result)) => {
                *load = match result {
                    Ok(playlist) => Load::Ready(PlaylistView {
                        card: playlist.card,
                        description: playlist.description,
                        tracks: Arc::new(playlist.tracks),
                    }),
                    Err(failure) => Load::Failed(failure),
                };
            }
            (Page::Artist { load, .. }, Loaded::Artist(result)) => {
                *load = match result {
                    Ok(artist) => Load::Ready(ArtistView {
                        card: artist.card,
                        listeners: artist.listeners,
                        popular: Arc::new(artist.popular),
                        albums: Arc::new(artist.albums),
                    }),
                    Err(failure) => Load::Failed(failure),
                };
            }
            (Page::ArtistTracks { load, .. }, Loaded::Tracks(result)) => match (load, result) {
                (Load::Ready(paged), Ok(found)) if page > 0 => {
                    paged.append(found.items, found.total);
                    grown = Some(paged.items.clone());
                }
                (Load::Ready(paged), Err(failure)) if page > 0 => {
                    paged.fetching = false;
                    paged.error = Some(failure.text.clone());
                    toast = Some(failure.text);
                }
                (load, Ok(found)) => {
                    *load = Load::Ready(Paged::first(found.items, found.total, u32::MAX));
                }
                (load, Err(failure)) => *load = Load::Failed(failure),
            },
            (Page::TopArtists { load }, Loaded::Top(result)) => {
                *load = match result {
                    Ok(top) => Load::Ready(top),
                    Err(failure) => Load::Failed(failure),
                };
            }
            _ => {}
        }
        let autoplay = std::mem::take(&mut entry.autoplay) && entry.page_ready();
        let key = entry_key(entry);
        if let Some(tracks) = grown
            && let Some(queue) = &mut self.queue
            && key.as_ref() == Some(&queue.key)
        {
            queue.extend(tracks, &mut self.rng);
        }
        let mut effect = toast.map(|text| {
            Effect::Toast(crate::texts::sentence(&format!(
                "Не удалось догрузить треки: {text}"
            )))
        });
        if autoplay && let Some(key) = key {
            effect = self.play_list(key, false).or(effect);
        }
        (self.more(http), effect)
    }

    // ---------------------------------------------------------- коллекция

    /// Загрузить раздел коллекции, если его ещё нет (или `again` — заново).
    pub(super) fn load_collection(
        &mut self,
        section: Section,
        again: bool,
        http: Option<&reqwest::Client>,
    ) -> Task<Message> {
        let now = Instant::now();
        let fresh = match section {
            Section::Playlists => replace_if(&mut self.collections.playlists, again, now),
            Section::Albums => replace_if(&mut self.collections.albums, again, now),
            Section::Artists => replace_if(&mut self.collections.artists, again, now),
            _ => false,
        };
        if !fresh {
            return Task::none();
        }
        let Some(http) = http.cloned() else {
            let collected = match section {
                Section::Playlists => Collected::Playlists(Err(no_network())),
                Section::Albums => Collected::Albums(Err(no_network())),
                _ => Collected::Artists(Err(no_network())),
            };
            return Task::done(Message::CollectionLoaded(collected));
        };
        let token = self.token.clone();
        let uid = self.status.account().uid;
        Task::perform(
            async move {
                let token = &token.access_token;
                match section {
                    Section::Playlists => Collected::Playlists(
                        api::fetch_my_playlists(&http, token, uid)
                            .await
                            .map_err(Failure::from),
                    ),
                    Section::Albums => Collected::Albums(
                        api::fetch_my_albums(&http, token, uid)
                            .await
                            .map_err(Failure::from),
                    ),
                    _ => Collected::Artists(
                        api::fetch_my_artists(&http, token, uid)
                            .await
                            .map_err(Failure::from),
                    ),
                }
            },
            Message::CollectionLoaded,
        )
    }

    pub(super) fn collection_loaded(&mut self, collected: Collected) -> Option<Effect> {
        let relogin = |failure: &Failure| failure.kind == FailureKind::Unauthorized;
        match collected {
            Collected::Playlists(Err(failure))
            | Collected::Albums(Err(failure))
            | Collected::Artists(Err(failure))
                if relogin(&failure) =>
            {
                Some(Effect::Relogin)
            }
            Collected::Playlists(result) => {
                let (load, partial) = match result {
                    Ok(mut found) => {
                        let partial = found.partial.take();
                        // Своих нет, а понравившиеся есть — сразу их вкладка.
                        if found.own.is_empty() && !found.liked.is_empty() {
                            self.collections.playlist_tab = PlaylistTab::Liked;
                        }
                        (Load::Ready(Arc::new(found)), partial)
                    }
                    Err(failure) => (Load::Failed(failure), None),
                };
                self.collections.playlists = Some(load);
                partial.map(|text| Effect::Toast(crate::texts::sentence(&text)))
            }
            Collected::Albums(result) => {
                self.collections.albums = Some(match result {
                    Ok(albums) => Load::Ready(Arc::new(albums)),
                    Err(failure) => Load::Failed(failure),
                });
                None
            }
            Collected::Artists(result) => {
                self.collections.artists = Some(match result {
                    Ok(artists) => Load::Ready(Arc::new(artists)),
                    Err(failure) => Load::Failed(failure),
                });
                None
            }
        }
    }

    // ---------------------------------------------------------- поиск

    /// Новый текст в поле: искать, когда ввод замрёт.
    pub(super) fn search_input(&mut self, text: String) -> Task<Message> {
        self.search.input = text;
        self.search.edit = self.search.edit.wrapping_add(1);
        let edit = self.search.edit;
        if self.search.input.trim().is_empty() {
            // Пустое поле — сразу подсказка, без ожидания.
            return Task::done(Message::SearchSettled(edit));
        }
        Task::perform(tokio::time::sleep(SEARCH_SETTLE), move |()| {
            Message::SearchSettled(edit)
        })
    }

    /// Ввод замер (`edit` — номер правки) или нажали Enter (`None`).
    pub(super) fn search_settled(
        &mut self,
        edit: Option<u64>,
        http: Option<&reqwest::Client>,
    ) -> Task<Message> {
        if edit.is_some_and(|edit| edit != self.search.edit) {
            return Task::none();
        }
        let query = self.search.input.trim().to_owned();
        if query == self.search.query && edit.is_some() {
            return Task::none();
        }
        self.search.query = query;
        self.search.generation = self.search.generation.wrapping_add(1);
        self.search.clear_results();
        let scroll = self.jump(0.0);
        if self.search.query.is_empty() {
            return scroll;
        }
        let tab = self.search.tab;
        Task::batch([scroll, self.search_page(tab, 0, http)])
    }

    /// Другая вкладка поиска: запросить, если её ещё не искали.
    pub(super) fn search_tab(
        &mut self,
        tab: SearchKind,
        http: Option<&reqwest::Client>,
    ) -> Task<Message> {
        if tab == self.search.tab {
            return Task::none();
        }
        self.search.tab = tab;
        self.selected = None;
        let scroll = self.jump(0.0);
        if self.search.query.is_empty() || !self.search.untouched(tab) {
            return scroll;
        }
        Task::batch([scroll, self.search_page(tab, 0, http)])
    }

    /// «Повторить» в поиске.
    pub(super) fn retry_search(&mut self, http: Option<&reqwest::Client>) -> Task<Message> {
        let tab = self.search.tab;
        if self.search.query.is_empty() {
            return Task::none();
        }
        let ready = match tab {
            SearchKind::Tracks => forgive(&mut self.search.tracks),
            SearchKind::Albums => forgive(&mut self.search.albums),
            SearchKind::Artists => forgive(&mut self.search.artists),
            SearchKind::Playlists => forgive(&mut self.search.playlists),
        };
        if ready {
            self.more(http)
        } else {
            self.search_page(tab, 0, http)
        }
    }

    fn search_page(
        &mut self,
        kind: SearchKind,
        page: u32,
        http: Option<&reqwest::Client>,
    ) -> Task<Message> {
        if page == 0 {
            self.search.set_loading(kind, Instant::now());
        }
        let generation = self.search.generation;
        let Some(http) = http.cloned() else {
            return Task::done(Message::SearchLoaded(
                generation,
                kind,
                page,
                Err(no_network()),
            ));
        };
        let token = self.token.clone();
        let query = self.search.query.clone();
        Task::perform(
            async move {
                api::search(&http, &token.access_token, &query, kind, page)
                    .await
                    .map_err(Failure::from)
            },
            move |result| Message::SearchLoaded(generation, kind, page, result),
        )
    }

    pub(super) fn search_loaded(
        &mut self,
        generation: u64,
        kind: SearchKind,
        page: u32,
        result: Result<api::SearchResult, Failure>,
        http: Option<&reqwest::Client>,
    ) -> (Task<Message>, Option<Effect>) {
        if generation != self.search.generation {
            return (Task::none(), None);
        }
        if let Err(failure) = &result
            && failure.kind == FailureKind::Unauthorized
        {
            return (Task::none(), Some(Effect::Relogin));
        }
        if let Ok(found) = &result
            && page == 0
        {
            self.search.corrected = found.corrected.clone();
        }
        let found = result.map(|found| found.found);
        // Ответ не той вкладки (так не бывает) — пустая страница.
        let failure = match kind {
            SearchKind::Tracks => {
                let failure = store(
                    &mut self.search.tracks,
                    page,
                    found.map(|found| match found {
                        Found::Tracks(page) => page,
                        _ => empty_page(),
                    }),
                );
                // Очередь из этих же треков растёт вместе со списком.
                if let Some(Load::Ready(paged)) = &self.search.tracks
                    && let Some(queue) = &mut self.queue
                    && queue.key == ListKey::SearchTracks(generation)
                    && queue.tracks.len() < paged.items.len()
                {
                    queue.extend(paged.items.clone(), &mut self.rng);
                }
                failure
            }
            SearchKind::Albums => store(
                &mut self.search.albums,
                page,
                found.map(|found| match found {
                    Found::Albums(page) => page,
                    _ => empty_page(),
                }),
            ),
            SearchKind::Artists => store(
                &mut self.search.artists,
                page,
                found.map(|found| match found {
                    Found::Artists(page) => page,
                    _ => empty_page(),
                }),
            ),
            SearchKind::Playlists => store(
                &mut self.search.playlists,
                page,
                found.map(|found| match found {
                    Found::Playlists(page) => page,
                    _ => empty_page(),
                }),
            ),
        };
        // Ошибка первой страницы видна на экране, следующей — сообщением.
        let effect = failure.filter(|_| page > 0).map(|failure| {
            Effect::Toast(crate::texts::sentence(&format!(
                "Не удалось догрузить результаты: {}",
                failure.text
            )))
        });
        (self.more(http), effect)
    }

    /// Догрузить следующую страницу списка на экране, если его прокрутили к
    /// концу.
    pub(super) fn more(&mut self, http: Option<&reqwest::Client>) -> Task<Message> {
        let Some(http) = http else {
            return Task::none();
        };
        let shown = self.last_visible_row();
        if let Some(entry) = self.stack.last_mut() {
            let serial = entry.serial;
            if let Page::ArtistTracks { id, name, load } = &mut entry.page
                && let Some(next) = wants_more(Some(load), shown)
            {
                let target = Target::ArtistTracks {
                    id: id.clone(),
                    name: name.clone(),
                };
                return self.load_target(serial, &target, next, Some(http));
            }
            return Task::none();
        }
        if self.section != Section::Search || self.search.query.is_empty() {
            return Task::none();
        }
        let tab = self.search.tab;
        // В сетке строка — это ряд карточек: переводим в число карточек.
        let shown_items = match tab {
            SearchKind::Tracks => shown,
            _ => shown.saturating_add(1).saturating_mul(self.grid_columns()),
        };
        let next = match tab {
            SearchKind::Tracks => wants_more(self.search.tracks.as_mut(), shown_items),
            SearchKind::Albums => wants_more(self.search.albums.as_mut(), shown_items),
            SearchKind::Artists => wants_more(self.search.artists.as_mut(), shown_items),
            SearchKind::Playlists => wants_more(self.search.playlists.as_mut(), shown_items),
        };
        match next {
            Some(page) => self.search_page(tab, page, Some(http)),
            None => Task::none(),
        }
    }

    // ---------------------------------------------------------- списки

    /// Треки списка по ключу — для очереди.
    pub(super) fn list_tracks(&self, key: &ListKey) -> Option<Arc<Vec<Track>>> {
        let page_tracks = |serial: u64| -> Option<Arc<Vec<Track>>> {
            match &self.entry(serial)?.page {
                Page::Album { load, .. } => load.ready().map(|album| album.tracks.clone()),
                Page::Playlist { load, .. } => load.ready().map(|playlist| playlist.tracks.clone()),
                Page::Artist { load, .. } => load.ready().map(|artist| artist.popular.clone()),
                Page::ArtistTracks { load, .. } => load.ready().map(|paged| paged.items.clone()),
                Page::TopArtists { .. } => None,
            }
        };
        match key {
            ListKey::Liked => self.liked_tracks(),
            ListKey::LikedFound(query) => {
                let tracks = self.liked_tracks()?;
                let shown = super::filter::matching(&self.haystacks, query)?;
                Some(Arc::new(
                    shown
                        .into_iter()
                        .filter_map(|index| tracks.get(index).cloned())
                        .collect(),
                ))
            }
            ListKey::Album(serial)
            | ListKey::Playlist(serial)
            | ListKey::ArtistPopular(serial)
            | ListKey::ArtistTracks(serial) => page_tracks(*serial),
            ListKey::SearchTracks(generation) => match &self.search.tracks {
                Some(Load::Ready(paged)) if *generation == self.search.generation => {
                    Some(paged.items.clone())
                }
                _ => None,
            },
        }
    }

    /// Включить трек `index` списка `key`. Тот же список уже в очереди —
    /// очередь (и порядок перемешивания) остаётся.
    pub(super) fn play_from(&mut self, key: ListKey, index: usize) -> Option<Effect> {
        let tracks = self.list_tracks(&key)?;
        let same = self
            .queue
            .as_ref()
            .is_some_and(|queue| queue.key == key && Arc::ptr_eq(&queue.tracks, &tracks));
        if same {
            if let Some(queue) = &mut self.queue {
                queue.current = index;
            }
        } else {
            let mut queue = Queue::new(key, tracks, index);
            if self.player.shuffle {
                queue.shuffle(Some(index), &mut self.rng);
            }
            self.queue = Some(queue);
        }
        self.player.skipped = 0;
        self.start(index, 0.0)
    }

    /// «Слушать» (`shuffle` — «Перемешать»): весь список с начала.
    pub(super) fn play_list(&mut self, key: ListKey, shuffle: bool) -> Option<Effect> {
        let tracks = self.list_tracks(&key)?;
        if shuffle {
            self.player.shuffle = true;
        }
        let mut queue = Queue::new(key, tracks, 0);
        let first = queue.first(self.player.shuffle, &mut self.rng)?;
        queue.current = first;
        self.queue = Some(queue);
        self.player.skipped = 0;
        self.start(first, 0.0)
    }
}

impl Entry {
    fn page_ready(&self) -> bool {
        match &self.page {
            Page::Album { load, .. } => load.ready().is_some(),
            Page::Playlist { load, .. } => load.ready().is_some(),
            Page::Artist { load, .. } => load.ready().is_some(),
            Page::ArtistTracks { load, .. } => load.ready().is_some(),
            Page::TopArtists { load } => load.ready().is_some(),
        }
    }
}

/// Ключ списка треков страницы. У топа исполнителей треков нет.
pub fn entry_key(entry: &Entry) -> Option<ListKey> {
    Some(match entry.page {
        Page::Album { .. } => ListKey::Album(entry.serial),
        Page::Playlist { .. } => ListKey::Playlist(entry.serial),
        Page::Artist { .. } => ListKey::ArtistPopular(entry.serial),
        Page::ArtistTracks { .. } => ListKey::ArtistTracks(entry.serial),
        Page::TopArtists { .. } => return None,
    })
}

fn entry_target(page: &Page) -> Target {
    match page {
        Page::Album { id, .. } => Target::Album(id.clone()),
        Page::Artist { id, .. } => Target::Artist(id.clone()),
        Page::Playlist { owner, kind, .. } => Target::Playlist {
            owner: owner.clone(),
            kind: kind.clone(),
        },
        Page::ArtistTracks { id, name, .. } => Target::ArtistTracks {
            id: id.clone(),
            name: name.clone(),
        },
        Page::TopArtists { .. } => Target::TopArtists,
    }
}

fn same_target(page: &Page, target: &Target) -> bool {
    entry_target(page) == *target
}

/// Раздел коллекции ещё не загружен (или не загрузился, или `again`):
/// ставим «загружается» и возвращаем `true`.
fn replace_if<T>(slot: &mut Option<Load<T>>, again: bool, now: Instant) -> bool {
    let needed = again || matches!(slot, None | Some(Load::Failed(_)));
    if needed && !matches!(slot, Some(Load::Loading(_))) {
        *slot = Some(Load::Loading(now));
        return true;
    }
    false
}

/// Страница загружается заново с начала.
fn reload<T>(load: &mut Load<T>, now: Instant) -> u32 {
    *load = Load::Loading(now);
    0
}

/// Ошибка догрузки забыта: список можно догружать снова. `false` — первой
/// страницы нет, искать надо заново.
fn forgive<T>(slot: &mut Option<Load<Paged<T>>>) -> bool {
    match slot {
        Some(Load::Ready(paged)) => {
            paged.error = None;
            true
        }
        _ => false,
    }
}

fn empty_page<T>() -> api::Page<T> {
    api::Page {
        items: Vec::new(),
        total: 0,
    }
}

fn no_network() -> Failure {
    Failure {
        kind: FailureKind::Other,
        text: "не удалось подготовить сетевое соединение".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paged_stops_at_total_last_page_and_error() {
        let mut paged = Paged::first(vec![1, 2], 5, 2);
        assert!(paged.more());
        paged.append(vec![3, 4], 5);
        assert_eq!(paged.items.as_slice(), [1, 2, 3, 4]);
        assert_eq!(paged.next, 2);
        assert!(paged.more());
        // Пустая страница — список кончился, сколько бы ни обещал сервис.
        paged.append(Vec::new(), 5);
        assert_eq!(paged.total, 4);
        assert!(!paged.more());

        let mut capped = Paged::first(vec![1], 100, 0);
        assert!(!capped.more(), "последняя страница уже получена");
        capped.next = 0;
        capped.error = Some("сбой".to_owned());
        assert!(!capped.more(), "после ошибки не догружаем сами");
    }

    #[test]
    fn more_is_requested_only_near_the_end() {
        let mut slot = Some(Load::Ready(Paged::first((0..40).collect(), 100, 4)));
        assert_eq!(wants_more(slot.as_mut(), 10), None);
        assert_eq!(wants_more(slot.as_mut(), 30), Some(1));
        // Уже запрошена — второй раз не просим.
        assert_eq!(wants_more(slot.as_mut(), 39), None);
        let mut loading: Option<Load<Paged<u32>>> = Some(Load::Loading(Instant::now()));
        assert_eq!(wants_more(loading.as_mut(), 1000), None);
        assert_eq!(wants_more::<u32>(None, 1000), None);
    }
}
