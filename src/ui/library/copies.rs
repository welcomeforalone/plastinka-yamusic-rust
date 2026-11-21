//! Копии недоступных треков в окне: очередь поиска, откуда брать звук и
//! текст, метки в строках.
//!
//! Модуль `crate::relink` знает только данные. Здесь — какие треки окно
//! просит искать (видимые строки и следующие в очереди), как строка трека
//! показывает «Копия» и «Недоступен». Звук копии — только поток Яндекса
//! (правило 1): копия — обычный трек каталога, играет по своему номеру.

use iced::widget::tooltip;
use iced::{Element, Task};

use super::{App, Library, Message, plugins};
use crate::api::{LyricsInfo, Track};
use crate::player::Sound;
use crate::plugins::fckcensor::Decision;
use crate::plugins::unix_now;
use crate::relink::{self, CopyRef};
use crate::ui::theme;
use crate::ui::widgets;

/// Сколько следующих треков очереди готовим заранее.
const AHEAD: usize = 3;

/// Как строка трека показывает доступность.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Link<'a> {
    /// Играет сам трек (или замена FckCensor).
    Plain,
    /// Играет копия из другого релиза; название её альбома, если известно.
    Copy(Option<&'a str>),
    /// Играть нечем.
    Gone,
}

impl Library {
    /// Копия, которая реально играет вместо недоступного трека: FckCensor
    /// не дал звук, а поиск копию нашёл.
    pub(super) fn playing_copy(&self, track: &Track) -> Option<&CopyRef> {
        if track.available {
            return None;
        }
        let copy = self.relinks.copy_of(&track.id)?;
        let Decision::Play(Sound::Track(id)) = self.decide(track) else {
            return None;
        };
        (id == copy.id).then_some(copy)
    }

    /// Состояние строки трека.
    pub(super) fn link<'a>(&'a self, track: &'a Track) -> Link<'a> {
        if let Some(copy) = self.playing_copy(track) {
            let album = self
                .relinks
                .track(&copy.id)
                .and_then(|found| found.album_title.as_deref());
            return Link::Copy(album);
        }
        if self.track_playable(track) {
            Link::Plain
        } else {
            Link::Gone
        }
    }

    /// Трек, чью обложку показывать в строке: показанный трек, а если у него
    /// обложки нет — найденная копия.
    pub(super) fn art_track<'a>(&'a self, track: &'a Track) -> &'a Track {
        let shown = self.shown(track);
        if shown.cover_uri.is_some() {
            return shown;
        }
        self.relinks
            .copy_of(&track.id)
            .and_then(|copy| self.relinks.track(&copy.id))
            .filter(|copy| copy.cover_uri.is_some())
            .unwrap_or(shown)
    }

    /// Номер трека и подсказка о тексте песни Яндекса: у трека с копией —
    /// копии (у оригинала текста нет, как и звука).
    pub(super) fn lyrics_source(&self, current: &Track) -> (String, Option<LyricsInfo>) {
        match self.playing_copy(current) {
            Some(copy) => (
                copy.id.clone(),
                self.relinks.track(&copy.id).and_then(|found| found.lyrics),
            ),
            None => (current.id.clone(), current.lyrics),
        }
    }

    /// Какие недоступные треки сейчас стоит искать: видимые строки и
    /// следующие в очереди, без тех, кого уже выручил FckCensor.
    fn relink_wanted(&self) -> Vec<Track> {
        let mut tracks = self.visible_tracks();
        if let Some(queue) = &self.queue {
            tracks.extend(
                queue
                    .tracks
                    .iter()
                    .skip(queue.current.saturating_add(1))
                    .take(AHEAD),
            );
        }
        let censor = self.censor_on();
        tracks
            .into_iter()
            .filter(|track| {
                !track.available && !censor.is_some_and(|censor| censor.playable(track))
            })
            .cloned()
            .collect()
    }

    /// После каждого сообщения: запросы поиска копий и запись кэша.
    pub(super) fn relink_pump(&mut self) -> Vec<Task<Message>> {
        let mut tasks = Vec::new();
        if let Some(http) = self.http.clone()
            && !self.token.access_token.is_empty()
            && self.relinks.ready
        {
            let wanted = self.relink_wanted();
            let batch = self
                .relinks
                .next_batch(&wanted, std::time::Instant::now(), unix_now());
            for original in batch {
                let (http, token) = (http.clone(), self.token.access_token.clone());
                let id = original.id.clone();
                tasks.push(Task::perform(
                    async move { relink::find_copy(&http, &token, &original).await },
                    move |lookup| {
                        Message::Plugin(plugins::Msg::RelinkFound {
                            track: id.clone(),
                            lookup,
                        })
                    },
                ));
            }
        }
        if self.relinks.ready && self.relinks.dirty && !self.relinks.saving {
            self.relinks.dirty = false;
            self.relinks.saving = true;
            tasks.push(Task::perform(
                relink::save(self.relinks.cache().clone()),
                |()| Message::Plugin(plugins::Msg::RelinkSaved),
            ));
        }
        tasks
    }
}

/// Метка перед названием: «Копия» акцентом или серая «Недоступен».
pub(super) fn tag<'a>(link: Link<'_>) -> Option<Element<'a, App>> {
    match link {
        Link::Plain => None,
        Link::Copy(album) => {
            let tip = match album {
                Some(album) => {
                    format!("Оригинал недоступен — играет тот же трек из релиза «{album}»")
                }
                None => "Оригинал недоступен — играет тот же трек из другого релиза".to_owned(),
            };
            Some(widgets::with_tooltip(
                plugins::tag("Копия", theme::accent()),
                tip,
                tooltip::Position::Top,
            ))
        }
        Link::Gone => Some(plugins::tag("Недоступен", theme::TEXT_DISABLED)),
    }
}
