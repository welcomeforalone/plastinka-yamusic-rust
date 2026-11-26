//! Скачивание треков в окне: «Скачать» в меню трека и плеера, «Скачать всё»
//! на страницах списков, раздел «Настройки → Скачивание».
//!
//! Только по явному действию пользователя и только в его папку. Папки по
//! умолчанию нет: первое «Скачать» открывает выбор папки. Звук берётся тем же
//! путём, что у плеера (`Library::decide`): копия недоступного трека, замена
//! FckCensor. Свой локальный файл FckCensor не копируется. Сама работа — в
//! `crate::download`; здесь очередь («не больше двух сразу»), диалог выбора
//! папки и сообщения.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use iced::widget::{Space, row};
use iced::{Alignment, Element, Task};

use super::{App, Effect, Lib, Library, Message};
use crate::api::{self, Track};
use crate::download::queue::{Done, Queue};
use crate::download::{self, Context, DownloadError, Job, Outcome, Source, name, tags};
use crate::net;
use crate::player::Sound;
use crate::plugins::fckcensor::Decision;
use crate::texts::sentence;
use crate::ui::icons::Icon;
use crate::ui::queue::ListKey;
use crate::ui::settings::Entry;
use crate::ui::settings::controls::setting_row;
use crate::ui::widgets::{self, Kind};

/// Состояние скачивания в окне.
#[derive(Default)]
pub(super) struct Downloads {
    /// Папка пользователя; `None` — ещё не выбрана.
    dir: Option<PathBuf>,
    queue: Queue<Job>,
    /// Номера треков в текущей очереди: один трек не качается дважды.
    known: HashSet<String>,
    /// Задания, которые ждут выбора папки или её проверки.
    pending: Vec<Job>,
    /// Сколько треков из запроса пропущено сразу (нет звука).
    pending_skipped: usize,
    /// Окно выбора папки открыто.
    picking: bool,
    /// Клиент для звука (без общего таймаута), создаётся при первом скачивании.
    audio_http: Option<reqwest::Client>,
}

/// Чем кончилось задание: то, что нужно окну (ошибка без токена и ссылок).
#[derive(Clone)]
pub struct Report {
    done: Done,
    error: Option<String>,
    unauthorized: bool,
}

#[derive(Clone)]
pub enum Msg {
    /// «Скачать» для одного трека.
    Track(Track),
    /// «Скачать всё» для списка.
    List(ListKey),
    /// Папка проверена: `Ok` — можно писать.
    Checked(Result<(), String>),
    /// «Изменить…»: открыть выбор папки.
    Pick,
    /// Выбор папки закончился (`None` — отмена).
    Picked(Option<PathBuf>),
    /// «Открыть папку».
    Open,
    /// Закончилось задание трека `id`.
    Done(String, Report),
}

/// Кнопка «Скачать всё». Нет подписки или треков для скачивания — кнопки нет
/// или она неактивна.
pub(super) fn download_all_action<'a>(
    key: &ListKey,
    shown: bool,
    enabled: bool,
) -> Element<'a, App> {
    if !shown {
        return Space::new().into();
    }
    widgets::action(
        Kind::Secondary,
        Some(Icon::Download),
        "Скачать всё",
        enabled.then(|| Lib(Message::Download(Msg::List(key.clone())))),
    )
}

/// Расширение списка кнопок страницы: «Слушать», «Перемешать» и «Скачать всё».
pub(super) fn with_download<'a>(
    buttons: Element<'a, App>,
    all: Element<'a, App>,
) -> Element<'a, App> {
    row![buttons, all]
        .spacing(10)
        .align_y(Alignment::Center)
        .into()
}

impl Library {
    /// Папка скачивания (`config.toml`); приходит от приложения.
    pub fn set_download_dir(&mut self, dir: Option<PathBuf>) {
        self.dl.dir = dir;
    }

    /// Можно ли скачивать: Плюс подтверждён (правило 2) и есть сеть.
    pub(super) fn can_download(&self) -> bool {
        matches!(self.status.subscription(), api::Subscription::Active(_)) && self.http.is_some()
    }

    /// Задание на трек или `None`, если звука нет: недоступен без замены или
    /// заменён своим файлом (его не копируем).
    fn download_job(&self, track: &Track) -> Option<Job> {
        let source = match self.decide(track) {
            Decision::Play(Sound::Track(id)) => Source::Track(id),
            Decision::Play(Sound::Url(url)) => Source::Url(url),
            Decision::Search => Source::Search(Box::new(track.clone())),
            Decision::Play(Sound::File(_)) | Decision::Silent => return None,
        };
        // Теги и имя — как трек показан: с названием без цензуры от FckCensor.
        let shown = self.shown(track);
        let artists = shown.artists_text();
        let title = shown.display_title();
        Some(Job {
            id: track.id.clone(),
            source,
            label: format!("{artists} — {title}"),
            stem: name::file_stem(&artists, &title),
            tags: tags::Tags {
                title,
                artist: artists,
                album: shown
                    .album_title
                    .clone()
                    .or_else(|| track.album_title.clone()),
                ..tags::Tags::default()
            },
            cover: shown.cover_uri.clone().or_else(|| track.cover_uri.clone()),
        })
    }

    /// Запрос «скачать эти треки».
    fn download_request(&mut self, tracks: &[Track]) -> (Task<Message>, Option<Effect>) {
        if !self.can_download() {
            return (Task::none(), None);
        }
        let mut jobs = Vec::new();
        let mut skipped = 0;
        for track in tracks {
            match self.download_job(track) {
                Some(job) => jobs.push(job),
                None => skipped += 1,
            }
        }
        if jobs.is_empty() {
            let text = match tracks {
                [track] => format!(
                    "«{}» нельзя скачать: у трека нет звука для скачивания",
                    self.shown(track).display_title()
                ),
                _ => "Нечего скачивать: у этих треков нет звука для скачивания".to_owned(),
            };
            return (Task::none(), Some(Effect::Toast(text)));
        }
        self.dl.pending.extend(jobs);
        self.dl.pending_skipped += skipped;
        if self.dl.picking {
            // Выбор папки уже открыт: задания дождутся его.
            return (Task::none(), None);
        }
        match self.dl.dir.clone() {
            None => (self.pick_dir(), None),
            Some(dir) => (check_dir(dir), None),
        }
    }

    /// Системное окно выбора папки.
    fn pick_dir(&mut self) -> Task<Message> {
        if self.dl.picking {
            return Task::none();
        }
        self.dl.picking = true;
        let start = self.dl.dir.clone();
        Task::perform(
            async move {
                let mut dialog =
                    rfd::AsyncFileDialog::new().set_title("Папка для скачанных треков");
                if let Some(start) = start.filter(|dir| dir.is_dir()) {
                    dialog = dialog.set_directory(start);
                }
                dialog
                    .pick_folder()
                    .await
                    .map(|folder| folder.path().to_path_buf())
            },
            |path| Message::Download(Msg::Picked(path)),
        )
    }

    /// Ждавшие задания идут в очередь.
    fn download_begin(&mut self) -> (Task<Message>, Option<Effect>) {
        let jobs = std::mem::take(&mut self.dl.pending);
        for _ in 0..std::mem::take(&mut self.dl.pending_skipped) {
            self.dl.queue.skip();
        }
        for job in jobs {
            if self.dl.known.insert(job.id.clone()) {
                self.dl.queue.push(job);
            }
        }
        let task = self.download_pump();
        let text = self.dl.queue.progress_text();
        (task, Some(Effect::Toast(text)))
    }

    /// Запускает задания, пока идёт меньше двух.
    fn download_pump(&mut self) -> Task<Message> {
        let ready = self.dl.queue.start_ready();
        if ready.is_empty() {
            return Task::none();
        }
        let (Some(http), Some(dir)) = (self.http.clone(), self.dl.dir.clone()) else {
            return Task::none();
        };
        let audio_http = match &self.dl.audio_http {
            Some(client) => client.clone(),
            None => match net::audio_client() {
                Ok(client) => {
                    self.dl.audio_http = Some(client.clone());
                    client
                }
                Err(_) => return Task::none(),
            },
        };
        let ctx = Arc::new(Context {
            http,
            audio_http,
            token: self.token.access_token.clone(),
            quality: self.quality,
            dir,
        });
        Task::batch(ready.into_iter().map(|job| {
            let ctx = ctx.clone();
            Task::perform(
                async move {
                    let outcome = download::run(&ctx, &job).await;
                    (job.id, report(outcome))
                },
                |(id, report)| Message::Download(Msg::Done(id, report)),
            )
        }))
    }

    pub(super) fn download_update(&mut self, msg: Msg) -> (Task<Message>, Option<Effect>) {
        match msg {
            Msg::Track(track) => {
                self.menu = None;
                self.download_request(std::slice::from_ref(&track))
            }
            Msg::List(key) => {
                let Some(tracks) = self.list_tracks(&key) else {
                    return (Task::none(), None);
                };
                self.download_request(&tracks)
            }
            Msg::Checked(result) => {
                if self.dl.pending.is_empty() {
                    return (Task::none(), None);
                }
                match result {
                    Ok(()) => self.download_begin(),
                    Err(text) => {
                        let toast =
                            format!("Папка для скачивания недоступна: {text}. Выберите другую");
                        (self.pick_dir(), Some(Effect::Toast(toast)))
                    }
                }
            }
            Msg::Pick => (self.pick_dir(), None),
            Msg::Picked(path) => {
                self.dl.picking = false;
                let Some(path) = path else {
                    // Отмена: ничего не скачивается.
                    self.dl.pending.clear();
                    self.dl.pending_skipped = 0;
                    return (Task::none(), None);
                };
                self.dl.dir = Some(path.clone());
                let task = if self.dl.pending.is_empty() {
                    Task::none()
                } else {
                    check_dir(path.clone())
                };
                (task, Some(Effect::DownloadDir(path)))
            }
            Msg::Open => {
                let Some(dir) = self.dl.dir.clone() else {
                    return (Task::none(), None);
                };
                let effect = open::that_detached(&dir).err().map(|_| {
                    Effect::Toast("Не удалось открыть папку. Откройте её в проводнике".to_owned())
                });
                (Task::none(), effect)
            }
            Msg::Done(_id, report) => {
                self.dl.queue.finish(report.done, report.error);
                let mut effect = None;
                if report.unauthorized {
                    self.dl.queue.clear_waiting();
                    effect = Some(Effect::Relogin);
                }
                let task = self.download_pump();
                if self.dl.queue.is_idle() {
                    if effect.is_none() {
                        effect = Some(Effect::Toast(sentence(&self.dl.queue.summary())));
                    }
                    self.dl.queue.reset_if_idle();
                    self.dl.known.clear();
                } else if effect.is_none() {
                    effect = Some(Effect::Toast(self.dl.queue.progress_text()));
                }
                (task, effect)
            }
        }
    }

    /// Раздел «Настройки → Скачивание».
    pub(in crate::ui) fn download_entries(&self) -> Vec<Entry<'_>> {
        let dir_hint = match &self.dl.dir {
            Some(dir) => dir.display().to_string(),
            None => "Не выбрана. Выбрать папку предложат при первом «Скачать»".to_owned(),
        };
        let dir_control: Element<'_, App> = row![
            widgets::action(
                Kind::Secondary,
                None,
                "Изменить…",
                Some(Lib(Message::Download(Msg::Pick))),
            ),
            widgets::action(
                Kind::Secondary,
                None,
                "Открыть папку",
                self.dl
                    .dir
                    .is_some()
                    .then(|| Lib(Message::Download(Msg::Open))),
            ),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into();
        let status = if self.dl.queue.is_idle() {
            "Скачать можно из меню трека (правый щелчок или «⋯» в плеере) и кнопкой \
             «Скачать всё» на странице альбома, плейлиста и в «Мне нравится». Файл получает \
             теги и обложку; уже скачанные треки пропускаются"
                .to_owned()
        } else {
            self.dl.queue.progress_text()
        };
        vec![
            Entry::new(
                "Папка для скачивания",
                dir_hint.clone(),
                "скачать загрузка папка каталог сохранить",
                setting_row("Папка для скачивания", dir_hint, dir_control),
            ),
            Entry::new(
                "Скачивание треков",
                status.clone(),
                "скачать загрузка трек альбом плейлист теги обложка flac mp3",
                setting_row("Скачивание треков", status, Space::new().into()),
            ),
        ]
    }
}

/// Проверка папки (запись пробного файла) вне основного потока.
fn check_dir(dir: PathBuf) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || download::check_dir(&dir))
                .await
                .unwrap_or_else(|_| Err("проверка прервалась".to_owned()))
        },
        |result| Message::Download(Msg::Checked(result)),
    )
}

fn report(outcome: Outcome) -> Report {
    match outcome {
        Outcome::Saved { .. } => Report {
            done: Done::Saved,
            error: None,
            unauthorized: false,
        },
        Outcome::Exists => Report {
            done: Done::Existed,
            error: None,
            unauthorized: false,
        },
        Outcome::Failed(DownloadError::Unauthorized) => Report {
            done: Done::Failed,
            error: None,
            unauthorized: true,
        },
        // Трек без звука пропускаем, а не считаем ошибкой.
        Outcome::Failed(DownloadError::Unavailable) => Report {
            done: Done::Skipped,
            error: None,
            unauthorized: false,
        },
        Outcome::Failed(err) => Report {
            done: Done::Failed,
            error: Some(err.to_string()),
            unauthorized: false,
        },
    }
}
