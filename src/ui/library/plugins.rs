//! Плагины в главном окне: FckCensor, Slopless, CopyTrackName.
//!
//! Модули плагинов (`crate::plugins`) знают только данные. Здесь — как окно
//! их загружает и показывает: раздел «Настройки → Плагины», меню трека,
//! диалог «Заменить на…», метки в строках, пропуск ИИ-треков и выбор звука.
//!
//! Звук берётся только с Яндекса или из файла, который выбрал пользователь
//! (правило 7). На диск пишутся только метаданные: список FckCensor, ответы
//! Slopless, найденные версии без цензуры и ручные замены.

use std::path::PathBuf;

use iced::widget::{
    Space, button, column, container, mouse_area, opaque, row, text_input, tooltip,
};
use iced::{Alignment, Border, Color, Element, Length, Padding, Point, Size, Task};

use super::{App, Effect, Lib, Library, Message, setting_row, switch};
use crate::api::{self, AlbumCard, ArtistRef, Found, SearchKind, Track};
use crate::player::{Command, Sound};
use crate::plugins::fckcensor::{
    self, AlbumPatch, Censor, Decision, ExplicitCache, ListState, Replacement, Replacements,
};
use crate::plugins::slopless::{self, Label};
use crate::plugins::{PluginSettings, copy_name, stale, unix_now};
use crate::texts::{plural, sentence};
use crate::ui::icons::{Icon, icon};
use crate::ui::queue::ListKey;
use crate::ui::theme::{self, Style};
use crate::ui::widgets::{self, Kind, Size as ButtonSize};
use crate::ui::{Dialog, Failure, FailureKind, anim};

/// Сколько ИИ-треков подряд пропускать, прежде чем сдаться.
pub(super) const MAX_AI_SKIPS: usize = 50;
/// Меню трека: ширина, высота пункта, поля.
const MENU_WIDTH: f32 = 280.0;
const MENU_ITEM: f32 = 36.0;
const MENU_PAD: f32 = 6.0;
const MENU_TITLE: f32 = 40.0;
/// Сколько результатов показывать в диалоге замены.
const REPLACE_RESULTS: usize = 8;
const REPLACE_ID: &str = "replace-search";
const FORMAT_ID: &str = "copy-format";

/// Что окно делает с плагинами в фоне.
#[derive(Debug, Default)]
pub(super) struct PluginIo {
    /// Данные FckCensor с диска уже запрошены.
    censor_started: bool,
    /// Идёт обновление списка с GitHub.
    pub censor_refreshing: bool,
    slop_started: bool,
    /// Кэш Slopless прочитан: можно спрашивать сервер.
    slop_ready: bool,
    slop_saving: bool,
    explicit_dirty: bool,
    explicit_saving: bool,
}

/// Меню трека у курсора.
pub(super) struct TrackMenu {
    track: Track,
    at: Point,
}

/// Диалог «Заменить на…»: поиск трека Яндекса или ссылка.
pub(super) struct Replace {
    track: Track,
    input: String,
    edit: u64,
    results: Results,
}

enum Results {
    Idle,
    Loading,
    Ready(Vec<Track>),
    Failed(String),
}

/// Данные FckCensor с диска.
#[derive(Debug, Clone)]
pub struct CensorDisk {
    cached: Option<(fckcensor::Loaded, fckcensor::CachedList)>,
    replacements: Result<Replacements, Failure>,
    explicit: ExplicitCache,
}

#[derive(Clone)]
pub enum Msg {
    CensorDisk(Box<CensorDisk>),
    CensorRefreshed(Result<fckcensor::Loaded, String>),
    /// «Обновить» или «Повторить» у списка FckCensor.
    CensorRefresh,
    /// Поиск версии без цензуры закончился: `found` — `None` при ошибке.
    ExplicitFound {
        session: u64,
        track: String,
        found: Option<Option<String>>,
    },
    ExplicitSaved,
    SlopCached(Box<slopless::Cache>),
    SlopFound(String, Result<slopless::Verdict, slopless::SlopError>),
    SlopSaved,
    Disliked(Result<(), Failure>),
    /// Треки альбома по составу из списка FckCensor.
    AlbumVolumes(u64, Result<Vec<Track>, Failure>),
    /// Где нажали мышь (см. `ui::pointer`).
    Pointer(Point),
    TrackMenu(ListKey, usize),
    PlayerMenu,
    CloseMenu,
    CopyName(Track),
    OpenReplace(Track),
    ReplaceInput(String),
    ReplaceSettled(u64),
    ReplaceFound(u64, Result<Vec<Track>, Failure>),
    ReplacePick(Track),
    CloseReplace,
    PickFile(Track),
    FilePicked(String, Option<PathBuf>),
    ClearReplacement(String),
    ReplacementsSaved(Result<(), Failure>),
}

/// Две метки одного трека (по исполнителям трека и по подменённым): ИИ
/// сильнее всего, неизвестное сильнее «человека».
fn merge_labels(own: Label, patched: Label) -> Label {
    match (own, patched) {
        (Label::Ai, _) | (_, Label::Ai) => Label::Ai,
        (Label::Pending, _) | (_, Label::Pending) => Label::Pending,
        _ => Label::Human,
    }
}

fn plugin(msg: Msg) -> Message {
    Message::Plugin(msg)
}

/// Можно ли включить трек: сам доступен или (с FckCensor) у него есть замена.
pub(super) fn playable(censor: Option<&Censor>, track: &Track) -> bool {
    censor.map_or(track.available, |censor| censor.playable(track))
}

async fn censor_disk() -> CensorDisk {
    CensorDisk {
        cached: fckcensor::cached().await,
        replacements: Replacements::load().await.map_err(Failure::from),
        explicit: fckcensor::load_explicit().await,
    }
}

impl Library {
    // ---------------------------------------------------------- показ

    /// FckCensor включён и список есть.
    fn censor_on(&self) -> Option<&Censor> {
        self.plugins.fckcensor.then_some(&self.censor)
    }

    /// Трек, как его показывать: с подменой FckCensor, если она есть.
    pub(super) fn shown<'a>(&'a self, track: &'a Track) -> &'a Track {
        if self.plugins.fckcensor {
            self.censor.patched.get(&track.id).unwrap_or(track)
        } else {
            track
        }
    }

    /// Трек в плеере, как его показывать.
    pub(super) fn current_shown(&self) -> Option<&Track> {
        self.current().map(|track| self.shown(track))
    }

    /// Подмена альбома из списка FckCensor.
    pub(super) fn album_patch(&self, id: &str) -> Option<&AlbumPatch> {
        self.censor_on()?.list()?.albums.get(id)
    }

    /// Обложка альбома с учётом подмены.
    pub(super) fn album_cover<'a>(&'a self, card: &'a AlbumCard) -> Option<&'a str> {
        self.album_patch(&card.id)
            .and_then(|patch| patch.cover_uri.as_deref())
            .or(card.cover_uri.as_deref())
    }

    /// Название альбома с учётом подмены.
    pub(super) fn album_title(&self, card: &AlbumCard) -> String {
        self.album_patch(&card.id)
            .and_then(|patch| patch.title.clone())
            .unwrap_or_else(|| card.display_title())
    }

    /// Исполнители альбома с учётом подмены.
    pub(super) fn album_artists<'a>(&'a self, card: &'a AlbumCard) -> &'a [ArtistRef] {
        self.album_patch(&card.id)
            .and_then(|patch| patch.artists.as_deref())
            .unwrap_or(&card.artists)
    }

    pub(super) fn track_playable(&self, track: &Track) -> bool {
        playable(self.censor_on(), track)
    }

    pub(super) fn any_playable(&self, tracks: &[Track]) -> bool {
        tracks.iter().any(|track| self.track_playable(track))
    }

    /// Метка Slopless для трека. Если FckCensor подменил исполнителей, учитываем
    /// и их: в списке у них настоящие номера.
    fn slop_label(&self, track: &Track) -> Label {
        let own = self.slop.track(track);
        let shown = self.shown(track);
        if std::ptr::eq(shown, track) {
            return own;
        }
        merge_labels(own, self.slop.track(shown))
    }

    /// Метка Slopless: трек — ИИ.
    pub(super) fn ai_track(&self, track: &Track) -> bool {
        self.plugins.slopless && self.slop_label(track) == Label::Ai
    }

    pub(super) fn ai_artist(&self, id: &str) -> bool {
        self.plugins.slopless && self.slop.artist(id) == Label::Ai
    }

    /// У трека ручная замена FckCensor.
    pub(super) fn replaced(&self, track: &Track) -> bool {
        self.plugins.fckcensor && self.censor.replacements.tracks.contains_key(&track.id)
    }

    /// Запомнить подмены FckCensor для всех треков в окне.
    pub(super) fn note_all(&mut self) {
        if !self.plugins.fckcensor || self.censor.list().is_none() {
            return;
        }
        let mut censor = std::mem::take(&mut self.censor);
        censor.note(self.tracks());
        for entry in &self.stack {
            match &entry.page {
                super::pages::Page::Album { load, .. } => {
                    if let Some(album) = load.ready() {
                        censor.note(album.tracks.iter());
                    }
                }
                super::pages::Page::Playlist { load, .. } => {
                    if let Some(playlist) = load.ready() {
                        censor.note(playlist.tracks.iter());
                    }
                }
                super::pages::Page::Artist { load, .. } => {
                    if let Some(artist) = load.ready() {
                        censor.note(artist.popular.iter());
                    }
                }
                super::pages::Page::ArtistTracks { load, .. } => {
                    if let Some(paged) = load.ready() {
                        censor.note(paged.items.iter());
                    }
                }
                super::pages::Page::TopArtists { .. } => {}
            }
        }
        if let Some(super::pages::Load::Ready(paged)) = &self.search.tracks {
            censor.note(paged.items.iter());
        }
        if let Some(queue) = &self.queue {
            censor.note(queue.tracks.iter());
        }
        self.censor = censor;
    }

    // ---------------------------------------------------------- настройки

    /// Новые настройки плагинов: включённым — загрузить данные.
    pub fn plugins_changed(&mut self, settings: &PluginSettings) -> Task<Message> {
        let before = std::mem::replace(&mut self.plugins, settings.clone());
        let mut tasks = Vec::new();
        if settings.fckcensor && !self.io.censor_started {
            self.io.censor_started = true;
            self.censor.state = ListState::Loading;
            tasks.push(Task::perform(censor_disk(), |disk| {
                plugin(Msg::CensorDisk(Box::new(disk)))
            }));
        }
        if settings.slopless && !self.io.slop_started {
            self.io.slop_started = true;
            tasks.push(Task::perform(slopless::load_cache(), |cache| {
                plugin(Msg::SlopCached(Box::new(cache)))
            }));
        }
        if settings.fckcensor && !before.fckcensor {
            self.note_all();
        }
        if !settings.copy_name || !settings.fckcensor {
            // Пункты меню выключенного плагина пропадают.
            self.menu = None;
        }
        if !settings.fckcensor {
            self.replace = None;
        }
        Task::batch(tasks)
    }

    fn refresh_censor(&mut self) {
        let Some(http) = self.http.clone() else {
            self.censor
                .failed("не удалось подготовить сетевое соединение".to_owned());
            return;
        };
        if self.io.censor_refreshing {
            return;
        }
        self.io.censor_refreshing = true;
        if !matches!(self.censor.state, ListState::Ready(_)) {
            self.censor.state = ListState::Loading;
        }
        let cache = self.censor.cache();
        self.tasks
            .push(Task::perform(fckcensor::refresh(http, cache), |result| {
                plugin(Msg::CensorRefreshed(result.map_err(|err| err.to_string())))
            }));
    }

    // ---------------------------------------------------------- звук

    /// Пропускать ли трек как ИИ: Slopless с автопропуском, трек не
    /// включён вручную.
    pub(super) fn skip_as_ai(&self, track: &Track) -> bool {
        self.plugins.slopless
            && self.plugins.autoskip
            && self.chosen.as_deref() != Some(track.id.as_str())
            && self.slop_label(track) == Label::Ai
    }

    /// «Не нравится» пропущенному ИИ-треку, если пользователь согласился.
    /// Каждому треку — не больше раза за сеанс.
    pub(super) fn dislike_ai(&mut self, track: &Track) {
        if !(self.plugins.slopless && self.plugins.dislike) {
            return;
        }
        let Some(http) = self.http.clone() else {
            return;
        };
        if !self.slop.disliked.insert(track.id.clone()) {
            return;
        }
        let token = self.token.clone();
        let uid = self.status.account().uid;
        let ids = vec![track.id.clone()];
        self.tasks.push(Task::perform(
            async move {
                api::add_disliked_tracks(&http, &token.access_token, uid, &ids)
                    .await
                    .map_err(Failure::from)
            },
            |result| plugin(Msg::Disliked(result)),
        ));
    }

    /// Откуда брать звук трека.
    pub(super) fn decide(&self, track: &Track) -> Decision {
        super::queue::sound(track, self.censor_on(), self.plugins.explicit_search)
    }

    /// Поиск версии без цензуры перед запуском трека. Пока он идёт, звука
    /// нет; ответ запускает трек в том же запуске (`session`).
    pub(super) fn search_explicit(&mut self, track: &Track) {
        let session = self.player.session;
        let id = track.id.clone();
        let Some(http) = self.http.clone() else {
            self.tasks.push(Task::done(plugin(Msg::ExplicitFound {
                session,
                track: id,
                found: None,
            })));
            return;
        };
        let token = self.token.clone();
        let original = track.clone();
        self.tasks.push(Task::perform(
            async move {
                let search = fckcensor::find_explicit(&http, &token.access_token, &original);
                match tokio::time::timeout(fckcensor::SEARCH_TIMEOUT, search).await {
                    Ok(Ok(found)) => Some(found),
                    // Ошибка или долго — играет сам трек, без сообщения.
                    _ => None,
                }
            },
            move |found| {
                plugin(Msg::ExplicitFound {
                    session,
                    track: id.clone(),
                    found,
                })
            },
        ));
    }

    /// Пришёл ответ о версии без цензуры.
    fn explicit_found(&mut self, session: u64, track: String, found: Option<Option<String>>) {
        if let Some(found) = &found {
            self.censor.explicit.insert(track.clone(), found.clone());
            self.io.explicit_dirty = true;
        }
        let current = self.current().is_some_and(|current| current.id == track);
        if session != self.player.session || !current || !self.player.loaded {
            return;
        }
        if !self.player.playing {
            // Пока искали, поставили на паузу: запустим, когда нажмут Play.
            self.player.loaded = false;
            self.player.buffering = false;
            return;
        }
        let id = found.flatten().unwrap_or(track);
        self.send(Command::Play {
            id: session,
            sound: Sound::Track(id),
            start: super::seconds(self.player.position),
        });
    }

    // ---------------------------------------------------------- фон

    /// После каждого сообщения: вопросы Slopless о видимых исполнителях и
    /// запись кэшей, когда в них что-то изменилось.
    pub(super) fn pump(&mut self) -> Task<Message> {
        let mut tasks = Vec::new();
        if self.plugins.slopless && self.io.slop_ready {
            let wanted = self.wanted_artists();
            self.slop.want(
                wanted.iter().map(String::as_str),
                iced::time::Instant::now(),
            );
            if let Some(http) = &self.http {
                for artist in self.slop.next_batch() {
                    let id = artist.clone();
                    tasks.push(Task::perform(
                        slopless::fetch_artist(http.clone(), artist),
                        move |result| plugin(Msg::SlopFound(id.clone(), result)),
                    ));
                }
            }
            if self.slop.dirty && !self.slop.busy() && !self.io.slop_saving {
                self.slop.dirty = false;
                self.io.slop_saving = true;
                tasks.push(Task::perform(
                    slopless::save_cache(self.slop.cache.clone()),
                    |()| plugin(Msg::SlopSaved),
                ));
            }
        }
        if self.io.explicit_dirty && !self.io.explicit_saving {
            self.io.explicit_dirty = false;
            self.io.explicit_saving = true;
            tasks.push(Task::perform(
                fckcensor::save_explicit(self.censor.explicit.clone()),
                |()| plugin(Msg::ExplicitSaved),
            ));
        }
        Task::batch(tasks)
    }

    /// Исполнители, о которых стоит спросить Slopless: в видимых строках, в
    /// плеере, следующий трек, открытая страница исполнителя.
    fn wanted_artists(&self) -> Vec<String> {
        let mut tracks = self.visible_tracks();
        if let Some(queue) = &self.queue
            && let Some(next) = queue.tracks.get(queue.current.saturating_add(1))
        {
            tracks.push(next);
        }
        let mut ids: Vec<String> = tracks
            .iter()
            .flat_map(|track| track.artists.iter().chain(self.shown(track).artists.iter()))
            .filter_map(|artist| artist.id.clone())
            .collect();
        if let Some(entry) = self.stack.last()
            && let super::pages::Page::Artist { load, .. } = &entry.page
            && let Some(artist) = load.ready()
        {
            ids.push(artist.card.id.clone());
        }
        ids
    }

    // ---------------------------------------------------------- сообщения

    pub(super) fn plugin_update(&mut self, msg: Msg) -> (Task<Message>, Option<Effect>) {
        let effect = match msg {
            Msg::CensorDisk(disk) => {
                let disk = *disk;
                let effect = match disk.replacements {
                    Ok(replacements) => {
                        self.censor.replacements = replacements;
                        None
                    }
                    Err(failure) => Some(Effect::Toast(sentence(&format!(
                        "FckCensor: не удалось прочитать ручные замены: {}",
                        failure.text
                    )))),
                };
                for (id, found) in disk.explicit.found {
                    self.censor.explicit.found.entry(id).or_insert(found);
                }
                let fresh = match disk.cached {
                    Some((loaded, cache)) => {
                        let fresh = !stale(loaded.fetched, fckcensor::LIST_FRESH);
                        self.censor.loaded(loaded, Some(cache));
                        fresh
                    }
                    None => false,
                };
                if !fresh {
                    self.refresh_censor();
                }
                effect
            }
            Msg::CensorRefreshed(result) => {
                self.io.censor_refreshing = false;
                match result {
                    Ok(loaded) => self.censor.loaded(loaded, None),
                    Err(error) => self.censor.failed(error),
                }
                None
            }
            Msg::CensorRefresh => {
                self.refresh_censor();
                None
            }
            Msg::ExplicitFound {
                session,
                track,
                found,
            } => {
                self.explicit_found(session, track, found);
                None
            }
            Msg::ExplicitSaved => {
                self.io.explicit_saving = false;
                None
            }
            Msg::SlopCached(cache) => {
                for (id, verdict) in cache.artists {
                    self.slop.cache.artists.entry(id).or_insert(verdict);
                }
                self.io.slop_ready = true;
                self.skip_current_ai()
            }
            Msg::SlopFound(artist, result) => {
                self.slop
                    .finished(artist, result, iced::time::Instant::now());
                self.skip_current_ai()
            }
            Msg::SlopSaved => {
                self.io.slop_saving = false;
                None
            }
            Msg::Disliked(result) => match result {
                Ok(()) => None,
                Err(failure) if failure.kind == FailureKind::Unauthorized => Some(Effect::Relogin),
                Err(failure) => Some(Effect::Toast(sentence(&format!(
                    "Slopless: не удалось поставить «Не нравится»: {}",
                    failure.text
                )))),
            },
            Msg::AlbumVolumes(serial, result) => {
                self.album_volumes_loaded(serial, result);
                None
            }
            Msg::Pointer(at) => {
                self.pointer = at;
                None
            }
            Msg::TrackMenu(key, index) => {
                let track = self
                    .list_tracks(&key)
                    .and_then(|tracks| tracks.get(index).cloned());
                if let Some(track) = track {
                    self.selected = Some((key, index));
                    self.menu = Some(TrackMenu {
                        track,
                        at: self.pointer,
                    });
                }
                None
            }
            Msg::PlayerMenu => {
                if let Some(track) = self.current().cloned() {
                    self.menu = Some(TrackMenu {
                        track,
                        at: self.pointer,
                    });
                }
                None
            }
            Msg::CloseMenu => {
                self.menu = None;
                None
            }
            Msg::CopyName(track) => {
                self.menu = None;
                let text = copy_name::format(&self.plugins.copy_format, self.shown(&track));
                return (
                    iced::clipboard::write(text.clone()),
                    Some(Effect::Toast(format!("Скопировано: {text}"))),
                );
            }
            Msg::OpenReplace(track) => {
                self.menu = None;
                let shown = self.shown(&track);
                let input = format!("{} {}", shown.artists_text(), shown.display_title());
                self.replace = Some(Replace {
                    track,
                    input: input.clone(),
                    edit: 0,
                    results: Results::Idle,
                });
                let search = self.replace_input(input);
                return (
                    Task::batch([search, iced::widget::operation::focus(REPLACE_ID)]),
                    None,
                );
            }
            Msg::ReplaceInput(text) => return (self.replace_input(text), None),
            Msg::ReplaceSettled(edit) => return (self.replace_settled(edit), None),
            Msg::ReplaceFound(edit, result) => {
                if let Some(replace) = &mut self.replace
                    && replace.edit == edit
                {
                    if let Err(failure) = &result
                        && failure.kind == FailureKind::Unauthorized
                    {
                        return (Task::none(), Some(Effect::Relogin));
                    }
                    replace.results = match result {
                        Ok(tracks) => Results::Ready(tracks),
                        Err(failure) => Results::Failed(sentence(&failure.text)),
                    };
                }
                None
            }
            Msg::ReplacePick(chosen) => {
                let Some(replace) = self.replace.take() else {
                    return (Task::none(), None);
                };
                let label = format!("{} — {}", chosen.artists_text(), chosen.display_title());
                let effect = self.set_replacement(&replace.track.id, Replacement::track(&chosen));
                return (
                    self.save_replacements(),
                    effect.or(Some(Effect::Toast(format!("Звук заменён: {label}")))),
                );
            }
            Msg::CloseReplace => {
                self.replace = None;
                None
            }
            Msg::PickFile(track) => {
                self.menu = None;
                self.replace = None;
                let id = track.id.clone();
                let title = format!("Свой файл для «{}»", self.shown(&track).display_title());
                return (
                    Task::perform(
                        async move {
                            rfd::AsyncFileDialog::new()
                                .set_title(title)
                                .add_filter("Аудио", &fckcensor::AUDIO_EXTENSIONS)
                                .pick_file()
                                .await
                                .map(|file| file.path().to_path_buf())
                        },
                        move |path| plugin(Msg::FilePicked(id.clone(), path)),
                    ),
                    None,
                );
            }
            Msg::FilePicked(id, path) => {
                let Some(path) = path else {
                    return (Task::none(), None);
                };
                let known = path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .map(str::to_lowercase)
                    .is_some_and(|ext| fckcensor::AUDIO_EXTENSIONS.contains(&ext.as_str()));
                if !known {
                    return (
                        Task::none(),
                        Some(Effect::Toast(
                            "Этот файл не подходит: нужен MP3, FLAC, OGG, WAV, M4A или AAC"
                                .to_owned(),
                        )),
                    );
                }
                let name = Replacement::file(path.clone()).label();
                let effect = self.set_replacement(&id, Replacement::file(path));
                return (
                    self.save_replacements(),
                    effect.or(Some(Effect::Toast(format!("Звук заменён файлом {name}")))),
                );
            }
            Msg::ClearReplacement(id) => {
                self.menu = None;
                if self.censor.replacements.tracks.remove(&id).is_some() {
                    let effect = self.restart_if_current(&id);
                    return (self.save_replacements(), effect);
                }
                None
            }
            Msg::ReplacementsSaved(result) => result.err().map(|failure| {
                Effect::Toast(sentence(&format!(
                    "Не удалось сохранить замену: {}",
                    failure.text
                )))
            }),
        };
        (Task::none(), effect)
    }

    /// Играющий трек оказался ИИ — пропускаем, как в очереди.
    fn skip_current_ai(&mut self) -> Option<Effect> {
        let track = self.current()?.clone();
        if !self.player.loaded || !self.skip_as_ai(&track) {
            return None;
        }
        let index = self.queue.as_ref()?.current;
        self.dislike_ai(&track);
        self.player.skipped = 0;
        match self.step(index, true) {
            Some(effect) => Some(effect),
            None => Some(Effect::Toast(sentence(&format!(
                "«{}» — ИИ-трек по данным slopless.art, пропускаем",
                self.shown(&track).display_title()
            )))),
        }
    }

    fn set_replacement(&mut self, id: &str, replacement: Replacement) -> Option<Effect> {
        self.censor
            .replacements
            .tracks
            .insert(id.to_owned(), replacement);
        self.restart_if_current(id)
    }

    /// Замену поменяли у играющего трека — он начинается заново с новым звуком.
    fn restart_if_current(&mut self, id: &str) -> Option<Effect> {
        let index = self.queue.as_ref()?.current;
        let current = self.current().is_some_and(|track| track.id == id);
        if current && self.player.loaded {
            self.player.skipped = 0;
            return self.start(index, 0.0);
        }
        None
    }

    fn save_replacements(&self) -> Task<Message> {
        Task::perform(self.censor.replacements.clone().save(), |result| {
            plugin(Msg::ReplacementsSaved(result.map_err(Failure::from)))
        })
    }

    fn replace_input(&mut self, text: String) -> Task<Message> {
        let Some(replace) = &mut self.replace else {
            return Task::none();
        };
        replace.input = text;
        replace.edit = replace.edit.wrapping_add(1);
        let edit = replace.edit;
        if replace.input.trim().is_empty() {
            replace.results = Results::Idle;
            return Task::none();
        }
        Task::perform(tokio::time::sleep(super::pages::SEARCH_SETTLE), move |()| {
            plugin(Msg::ReplaceSettled(edit))
        })
    }

    fn replace_settled(&mut self, edit: u64) -> Task<Message> {
        let Some(replace) = &mut self.replace else {
            return Task::none();
        };
        if replace.edit != edit {
            return Task::none();
        }
        let Some(http) = self.http.clone() else {
            replace.results = Results::Failed("Нет сетевого соединения".to_owned());
            return Task::none();
        };
        replace.results = Results::Loading;
        let token = self.token.clone();
        let query = replace.input.trim().to_owned();
        let link = fckcensor::track_from_link(&query);
        Task::perform(
            async move {
                let token = &token.access_token;
                let tracks = match link {
                    Some(id) => api::fetch_tracks(&http, token, &[id], |_| {}).await?,
                    None => match api::search(&http, token, &query, SearchKind::Tracks, 0)
                        .await?
                        .found
                    {
                        Found::Tracks(page) => page.items,
                        _ => Vec::new(),
                    },
                };
                Ok(tracks.into_iter().take(REPLACE_RESULTS).collect())
            },
            move |result: Result<Vec<Track>, api::ApiError>| {
                plugin(Msg::ReplaceFound(edit, result.map_err(Failure::from)))
            },
        )
    }

    // ---------------------------------------------------------- альбомы

    /// Альбом открылся: если в списке FckCensor есть его состав, догружаем
    /// треки по нему.
    pub(super) fn album_volumes(&mut self, serial: u64, album: &str) {
        let Some(volumes) = self
            .album_patch(album)
            .map(|patch| patch.volumes.clone())
            .filter(|volumes| !volumes.is_empty())
        else {
            return;
        };
        let Some(http) = self.http.clone() else {
            return;
        };
        let token = self.token.clone();
        self.tasks.push(Task::perform(
            async move {
                api::fetch_tracks(&http, &token.access_token, &volumes, |_| {})
                    .await
                    .map(|tracks| {
                        // Порядок — как в составе.
                        volumes
                            .iter()
                            .filter_map(|id| tracks.iter().find(|track| track.id == *id).cloned())
                            .collect()
                    })
                    .map_err(Failure::from)
            },
            move |result| plugin(Msg::AlbumVolumes(serial, result)),
        ));
    }

    fn album_volumes_loaded(&mut self, serial: u64, result: Result<Vec<Track>, Failure>) {
        // Не вышло — остаётся состав от Яндекса.
        let Ok(mut tracks) = result else {
            return;
        };
        if tracks.is_empty() {
            return;
        }
        let Some(entry) = self.find_entry(serial) else {
            return;
        };
        let super::pages::Page::Album { load, .. } = &mut entry.page else {
            return;
        };
        let super::pages::Load::Ready(album) = load else {
            return;
        };
        let id = album.card.id.clone();
        let artists = self
            .album_patch(&id)
            .filter(|patch| patch.replace_artists)
            .and_then(|patch| patch.artists.clone());
        if let Some(artists) = artists {
            for track in &mut tracks {
                track.artists = artists.clone();
            }
        }
        if let Some(entry) = self.find_entry(serial)
            && let super::pages::Page::Album {
                load: super::pages::Load::Ready(album),
                ..
            } = &mut entry.page
        {
            album.tracks = std::sync::Arc::new(tracks);
        }
    }

    // ---------------------------------------------------------- поверх окна

    /// Esc: закрыть меню или диалог замены. `false` — их не было.
    pub fn close_overlay(&mut self) -> bool {
        if self.replace.take().is_some() {
            return true;
        }
        self.menu.take().is_some()
    }

    /// Меню трека или диалог замены поверх окна размером `window`.
    pub fn overlay(&self, window: Size) -> Option<Element<'_, App>> {
        if let Some(replace) = &self.replace {
            return Some(self.replace_view(replace));
        }
        self.menu.as_ref().map(|menu| self.menu_view(menu, window))
    }

    fn menu_view<'a>(&'a self, menu: &'a TrackMenu, window: Size) -> Element<'a, App> {
        let track = &menu.track;
        let shown = self.shown(track);
        let mut items: Vec<Element<'a, App>> = Vec::new();
        if self.plugins.copy_name {
            items.push(menu_item(
                Icon::Copy,
                "Скопировать название".to_owned(),
                Lib(plugin(Msg::CopyName(track.clone()))),
            ));
        }
        if let Some(album) = &shown.album_id {
            items.push(menu_item(
                Icon::Album,
                "Перейти к альбому".to_owned(),
                Lib(Message::Open(super::Target::Album(album.clone()))),
            ));
        }
        if let Some((id, name)) = shown
            .artists
            .iter()
            .find_map(|artist| artist.id.as_ref().map(|id| (id, &artist.name)))
        {
            items.push(menu_item(
                Icon::Artist,
                format!("К исполнителю: {name}"),
                Lib(Message::Open(super::Target::Artist(id.clone()))),
            ));
        }
        if self.plugins.fckcensor {
            items.push(menu_item(
                Icon::Search,
                "Заменить на…".to_owned(),
                Lib(plugin(Msg::OpenReplace(track.clone()))),
            ));
            items.push(menu_item(
                Icon::Plus,
                "Свой файл…".to_owned(),
                Lib(plugin(Msg::PickFile(track.clone()))),
            ));
            if let Some(replacement) = self.censor.replacements.tracks.get(&track.id) {
                items.push(menu_item(
                    Icon::Close,
                    format!("Убрать замену ({})", replacement.label()),
                    Lib(plugin(Msg::ClearReplacement(track.clone().id))),
                ));
            }
        }
        let count = items.len() as f32;
        let height = MENU_TITLE + count * MENU_ITEM + 2.0 * MENU_PAD;
        let title = container(theme::line(
            format!("{} — {}", shown.artists_text(), shown.display_title()),
            Style::Caption,
            theme::TEXT_TERTIARY,
        ))
        .height(MENU_TITLE)
        .center_y(MENU_TITLE)
        .width(Length::Fill)
        .padding([0.0, 12.0])
        .clip(true);
        let card = container(column![title].extend(items).width(Length::Fill))
            .width(MENU_WIDTH)
            .padding(MENU_PAD)
            .style(|_| popover());
        // У края окна меню отодвигается внутрь, у нижнего — раскрывается вверх.
        let x = menu.at.x.min(window.width - MENU_WIDTH - 8.0).max(8.0);
        let y = if menu.at.y + height + 8.0 > window.height {
            (menu.at.y - height).max(8.0)
        } else {
            menu.at.y
        };
        let placed = container(opaque(card))
            .padding(Padding {
                top: y,
                left: x,
                ..Padding::ZERO
            })
            .width(Length::Fill)
            .height(Length::Fill);
        opaque(
            mouse_area(placed)
                .on_press(Lib(plugin(Msg::CloseMenu)))
                .on_right_press(Lib(plugin(Msg::CloseMenu))),
        )
    }

    fn replace_view<'a>(&'a self, replace: &'a Replace) -> Element<'a, App> {
        let shown = self.shown(&replace.track);
        let input = text_input(
            "Название трека или ссылка на music.yandex.ru",
            &replace.input,
        )
        .id(REPLACE_ID)
        .on_input(|text| Lib(plugin(Msg::ReplaceInput(text))))
        .size(14)
        .padding([10, 16])
        .style(|_, _| text_input::Style {
            background: Color::TRANSPARENT.into(),
            border: Border {
                color: theme::OUTLINE,
                width: 1.0,
                radius: widgets::ROUND.into(),
            },
            icon: theme::TEXT_TERTIARY,
            placeholder: theme::TEXT_TERTIARY,
            value: theme::TEXT_PRIMARY,
            selection: theme::alpha(theme::accent(), 0.35),
        });
        let results: Element<'a, App> = match &replace.results {
            Results::Idle => note("Найдите трек на Яндексе или вставьте ссылку на него."),
            Results::Loading => note("Ищем…"),
            Results::Failed(text) => note_owned(text.clone()),
            Results::Ready(tracks) if tracks.is_empty() => note("Ничего не нашлось."),
            Results::Ready(tracks) => column(
                tracks
                    .iter()
                    .map(|track| replace_row(track, &replace.track.id)),
            )
            .spacing(2)
            .into(),
        };
        let body = column![
            theme::line("Заменить звук", Style::Title, theme::TEXT_PRIMARY),
            theme::label(
                format!(
                    "Вместо «{}» будет играть выбранный трек Яндекса. Замена действует везде, \
                     в том числе в очереди.",
                    shown.display_title()
                ),
                Style::Secondary,
                theme::TEXT_SECONDARY,
            ),
            input,
            container(results).height(Length::Fixed(REPLACE_RESULTS as f32 * 48.0)),
            row![
                widgets::action(
                    Kind::Secondary,
                    Some(Icon::Plus),
                    "Свой файл…",
                    Some(Lib(plugin(Msg::PickFile(replace.track.clone())))),
                ),
                Space::new().width(Length::Fill),
                widgets::action(
                    Kind::Secondary,
                    None,
                    "Отмена",
                    Some(Lib(plugin(Msg::CloseReplace))),
                ),
            ]
            .align_y(Alignment::Center),
        ]
        .spacing(16);
        let card = container(body)
            .width(560)
            .padding(24)
            .style(|_| container::Style {
                background: Some(theme::SURFACE_2.into()),
                border: Border {
                    color: theme::OUTLINE,
                    width: 1.0,
                    radius: widgets::RADIUS_XL.into(),
                },
                shadow: widgets::popover_shadow(),
                ..container::Style::default()
            });
        let scrim = container(opaque(card))
            .center(Length::Fill)
            .style(|_| container::Style {
                background: Some(theme::SCRIM.into()),
                ..container::Style::default()
            });
        opaque(mouse_area(scrim).on_press(Lib(plugin(Msg::CloseReplace))))
    }

    // ---------------------------------------------------------- настройки

    /// «Настройки → Плагины».
    pub(super) fn plugins_block(&self) -> Element<'_, App> {
        let settings = &self.plugins;
        let mut rows: Vec<Element<'_, App>> = vec![setting_row(
            "FckCensor",
            "Названия, обложки и исполнители без цензуры из списка FckCensorData. \
             Звук — только с Яндекса или из вашего файла"
                .to_owned(),
            switch(
                settings.fckcensor,
                change(settings, |s, on| s.fckcensor = on),
            ),
        )];
        if settings.fckcensor {
            rows.push(self.censor_status());
            rows.push(sub_row(
                "Искать версию без цензуры",
                "Если на Яндексе есть explicit-версия трека, играет она".to_owned(),
                switch(
                    settings.explicit_search,
                    change(settings, |s, on| s.explicit_search = on),
                ),
            ));
            let count = self.censor.replacements.tracks.len();
            rows.push(sub_row(
                "Ручные замены",
                format!(
                    "{count} {}. Правый щелчок по треку → «Заменить на…» или «Свой файл…»",
                    plural(count, ["замена", "замены", "замен"])
                ),
                Space::new().into(),
            ));
        }
        rows.push(widgets::divider());
        rows.push(setting_row(
            "Slopless",
            "Метки ИИ-исполнителей и ИИ-треков по базе slopless.art".to_owned(),
            switch(settings.slopless, change(settings, |s, on| s.slopless = on)),
        ));
        if settings.slopless {
            rows.push(sub_row(
                "Пропускать ИИ-треки",
                "В очереди. Трек, включённый вручную, играет".to_owned(),
                switch(settings.autoskip, change(settings, |s, on| s.autoskip = on)),
            ));
            let base = settings.clone();
            rows.push(sub_row(
                "Ставить «Не нравится»",
                "Пропущенным ИИ-трекам. Это запись в ваш аккаунт: включается после \
                 подтверждения"
                    .to_owned(),
                switch(settings.dislike, move |on| {
                    if on {
                        App::OpenDialog(Dialog::SloplessConsent)
                    } else {
                        App::SetPlugins(PluginSettings {
                            dislike: false,
                            ..base.clone()
                        })
                    }
                }),
            ));
            let known = self.slop.cache.artists.len();
            rows.push(sub_row(
                "База",
                format!(
                    "Проверено {known} {}",
                    plural(known, ["исполнитель", "исполнителя", "исполнителей"])
                ),
                widgets::action(
                    Kind::Secondary,
                    Some(Icon::ExternalLink),
                    "slopless.art",
                    Some(App::OpenUrl(slopless::SITE_URL.to_owned())),
                ),
            ));
        }
        rows.push(widgets::divider());
        rows.push(setting_row(
            "CopyTrackName",
            "Пункт «Скопировать название» в меню трека (правый щелчок) и плеера".to_owned(),
            switch(
                settings.copy_name,
                change(settings, |s, on| s.copy_name = on),
            ),
        ));
        if settings.copy_name {
            rows.push(self.copy_format_row());
        }
        rows.push(widgets::divider());
        rows.push(setting_row(
            "VibeColorizer",
            "Цвет «Моей волны» под обложку текущего трека".to_owned(),
            switch(settings.vibe, change(settings, |s, on| s.vibe = on)),
        ));
        container(column(rows))
            .width(Length::Fill)
            .style(|_| super::card_style())
            .into()
    }

    fn censor_status(&self) -> Element<'_, App> {
        let refreshing = self.io.censor_refreshing;
        let (hint, retry) = match &self.censor.state {
            ListState::Off | ListState::Loading => ("Загружаем список…".to_owned(), false),
            ListState::Ready(loaded) => {
                let count = loaded.list.tracks.len();
                let mut text = format!(
                    "{count} {} · обновлён {}",
                    plural(count, ["трек", "трека", "треков"]),
                    ago(loaded.fetched)
                );
                if let Some(error) = &loaded.stale {
                    text.push_str(&format!(". Обновить не удалось: {error}"));
                }
                (sentence(&text), false)
            }
            ListState::Failed(error) => (
                sentence(&format!("Не удалось загрузить список: {error}")),
                true,
            ),
        };
        let caption = if retry {
            "Повторить"
        } else {
            "Обновить"
        };
        let action = row![
            widgets::action(
                Kind::Secondary,
                Some(Icon::Retry),
                caption,
                (!refreshing).then(|| Lib(plugin(Msg::CensorRefresh))),
            ),
            widgets::with_tooltip(
                widgets::icon_button(
                    Icon::ExternalLink,
                    ButtonSize::S,
                    false,
                    Some(App::OpenUrl(fckcensor::PROJECT_URL.to_owned())),
                ),
                "Список на GitHub",
                tooltip::Position::Top,
            ),
        ]
        .spacing(6)
        .align_y(Alignment::Center);
        sub_row("Список", hint, action.into())
    }

    fn copy_format_row(&self) -> Element<'_, App> {
        let base = self.plugins.clone();
        let input = text_input(copy_name::DEFAULT_FORMAT, &self.plugins.copy_format)
            .id(FORMAT_ID)
            .on_input(move |value| {
                App::SetPlugins(PluginSettings {
                    copy_format: copy_name::limit(&value),
                    ..base.clone()
                })
            })
            .size(14)
            .padding([8, 14])
            .width(260)
            .style(|_, _| text_input::Style {
                background: Color::TRANSPARENT.into(),
                border: Border {
                    color: theme::OUTLINE,
                    width: 1.0,
                    radius: widgets::ROUND.into(),
                },
                icon: theme::TEXT_TERTIARY,
                placeholder: theme::TEXT_TERTIARY,
                value: theme::TEXT_PRIMARY,
                selection: theme::alpha(theme::accent(), 0.35),
            });
        let example = self
            .current_shown()
            .map(|track| copy_name::format(&self.plugins.copy_format, track))
            .unwrap_or_else(|| {
                self.plugins
                    .copy_format
                    .replace("{artist}", "Исполнитель")
                    .replace("{title}", "Название")
                    .replace("{album}", "Альбом")
            });
        sub_row(
            "Формат",
            format!(
                "{{artist}} — исполнители, {{title}} — название, {{album}} — альбом. Сейчас: {example}"
            ),
            input.into(),
        )
    }
}

/// Переключатель настройки плагина: меняет одно поле копии настроек.
fn change(
    settings: &PluginSettings,
    apply: impl Fn(&mut PluginSettings, bool) + 'static,
) -> impl Fn(bool) -> App + 'static {
    let base = settings.clone();
    move |on| {
        let mut next = base.clone();
        apply(&mut next, on);
        App::SetPlugins(next)
    }
}

/// Вложенная строка настройки: с отступом слева.
fn sub_row<'a>(title: &'a str, hint: String, control: Element<'a, App>) -> Element<'a, App> {
    container(setting_row(title, hint, control))
        .padding(Padding {
            left: 20.0,
            ..Padding::ZERO
        })
        .into()
}

/// «5 мин назад».
fn ago(at: u64) -> String {
    let secs = unix_now().saturating_sub(at);
    let minutes = usize::try_from(secs / 60).unwrap_or(usize::MAX);
    let hours = minutes / 60;
    let days = hours / 24;
    if minutes < 1 {
        "только что".to_owned()
    } else if hours < 1 {
        format!(
            "{minutes} {} назад",
            plural(minutes, ["минуту", "минуты", "минут"])
        )
    } else if days < 1 {
        format!("{hours} {} назад", plural(hours, ["час", "часа", "часов"]))
    } else {
        format!("{days} {} назад", plural(days, ["день", "дня", "дней"]))
    }
}

fn popover() -> container::Style {
    container::Style {
        background: Some(theme::SURFACE_2.into()),
        border: Border {
            color: theme::OUTLINE,
            width: 1.0,
            radius: widgets::RADIUS_M.into(),
        },
        shadow: widgets::popover_shadow(),
        ..container::Style::default()
    }
}

fn menu_item<'a>(name: Icon, caption: String, message: App) -> Element<'a, App> {
    let content = row![
        icon(name, 16.0, theme::TEXT_SECONDARY),
        theme::line(caption, Style::Body, theme::TEXT_PRIMARY),
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    // Длинная подпись («Убрать замену (…)») обрезается краем пункта.
    let item = button(container(content).center_y(Length::Fill).clip(true))
        .width(Length::Fill)
        .height(MENU_ITEM)
        .padding([0, 12])
        .style(|_, status| button::Style {
            background: match status {
                button::Status::Pressed => Some(theme::SURFACE_4.into()),
                _ => None,
            },
            border: Border::default().rounded(widgets::RADIUS_S),
            ..button::Style::default()
        })
        .on_press(message);
    anim::hover_fill(
        item,
        theme::clear(theme::SURFACE_3),
        theme::SURFACE_3,
        widgets::RADIUS_S,
    )
    .into()
}

fn note<'a>(text: &'a str) -> Element<'a, App> {
    container(theme::label(text, Style::Secondary, theme::TEXT_SECONDARY))
        .padding([12, 4])
        .into()
}

fn note_owned<'a>(text: String) -> Element<'a, App> {
    container(theme::label(text, Style::Secondary, theme::TEXT_SECONDARY))
        .padding([12, 4])
        .into()
}

/// Строка результата в диалоге замены. Сам трек и недоступные не выбрать.
fn replace_row<'a>(track: &'a Track, original: &str) -> Element<'a, App> {
    let usable = track.available && track.id != original;
    let color = if usable {
        theme::TEXT_PRIMARY
    } else {
        theme::TEXT_DISABLED
    };
    let mut title = row![].spacing(6).align_y(Alignment::Center);
    if track.warning == api::Warning::Explicit {
        title = title.push(tag("E", theme::TEXT_SECONDARY));
    }
    let title = title.push(theme::line(track.display_title(), Style::Body, color));
    let content = row![
        column![
            title,
            theme::line(
                track.artists_text(),
                Style::Secondary,
                theme::TEXT_SECONDARY
            ),
        ]
        .spacing(2)
        .width(Length::Fill),
        theme::line(
            track.duration_text(),
            Style::Secondary,
            theme::TEXT_TERTIARY
        ),
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    let item = button(container(content).center_y(Length::Fill).clip(true))
        .width(Length::Fill)
        .height(46)
        .padding([0, 12])
        .style(|_, status| button::Style {
            background: match status {
                button::Status::Pressed => Some(theme::SURFACE_4.into()),
                _ => None,
            },
            border: Border::default().rounded(widgets::RADIUS_M),
            ..button::Style::default()
        })
        .on_press_maybe(usable.then(|| Lib(plugin(Msg::ReplacePick(track.clone())))));
    if usable {
        anim::hover_fill(
            item,
            theme::clear(theme::SURFACE_3),
            theme::SURFACE_3,
            widgets::RADIUS_M,
        )
        .into()
    } else {
        item.into()
    }
}

/// Метка в строке трека: «ИИ», «Замена», «E».
pub(super) fn tag<'a>(text: &'a str, color: Color) -> Element<'a, App> {
    container(theme::line(text, Style::CaptionStrong, color))
        .padding([0, 5])
        .style(move |_| container::Style {
            border: Border::default()
                .rounded(widgets::RADIUS_S)
                .width(1)
                .color(theme::alpha(color, 0.6)),
            ..container::Style::default()
        })
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::fckcensor::{Loaded, SAMPLE_LIST, parse_list};
    use std::sync::Arc;

    fn track(id: &str, available: bool) -> Track {
        Track {
            id: id.to_owned(),
            title: Some("Название".to_owned()),
            version: None,
            artists: Vec::new(),
            album_id: None,
            album_title: None,
            duration: None,
            available,
            cover_uri: None,
            warning: api::Warning::None,
        }
    }

    fn censor() -> Censor {
        let mut censor = Censor::default();
        let list = parse_list(SAMPLE_LIST.as_bytes()).unwrap();
        censor.loaded(
            Loaded {
                list: Arc::new(list),
                fetched: 0,
                stale: None,
            },
            None,
        );
        censor
    }

    #[test]
    fn labels_merge_in_priority_order() {
        use Label::{Ai, Human, Pending};
        assert_eq!(merge_labels(Human, Human), Human);
        assert_eq!(merge_labels(Human, Pending), Pending);
        assert_eq!(merge_labels(Pending, Human), Pending);
        assert_eq!(merge_labels(Pending, Ai), Ai);
        assert_eq!(merge_labels(Human, Ai), Ai);
        assert_eq!(merge_labels(Ai, Human), Ai);
    }

    #[test]
    fn unavailable_track_plays_only_with_a_replacement() {
        let gone = track("99", false);
        assert!(!playable(None, &gone));
        assert!(playable(None, &track("1", true)));

        let mut censor = censor();
        assert!(!playable(Some(&censor), &gone));
        // «Снят с каталога» (2): в списке есть трек Яндекса вместо него.
        assert!(playable(Some(&censor), &track("2", false)));
        // Ручная замена открывает и недоступный трек.
        censor.replacements.tracks.insert(
            "99".to_owned(),
            Replacement::file(PathBuf::from("мой-файл.flac")),
        );
        assert!(playable(Some(&censor), &gone));
    }
}
