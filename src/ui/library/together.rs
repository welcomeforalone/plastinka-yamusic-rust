//! «Слушать вместе» в главном окне: состояние связи, перевод своих изменений
//! в сообщения собеседнику и применение его состояния (§4.4 и §5 спеки
//! `docs/api/sync.md`). Сеть, шифрование и часы — в `crate::sync::service`;
//! здесь только поведение плеера.
//!
//! Свои изменения ловим не флагами в обработчиках, а снимком [`Snapshot`] в
//! конце каждого `Library::update`: что изменилось против базы (последнее
//! отправленное или применённое состояние), то и уходит собеседнику. Так
//! попадают все пути: кнопки, медиаклавиши, мини-плеер, системный плеер. После
//! применения чужого состояния база обновляется, поэтому эха нет.

use std::sync::Arc;
use std::time::Duration;

use iced::time::Instant;
use iced::widget::{column, container, mouse_area, row};
use iced::{Alignment, Element, Length, Padding, Task};

use super::{
    Effect, Lib, Library, Message, PLAYER_HEIGHT, Player, Repeat, SOUND_PANEL_MARGIN, anim, plugins,
};
use crate::api::Track;
use crate::player::Command as PlayerCommand;
use crate::storage::config::{PairedDevice, SyncConfig};
use crate::sync::handshake::Role;
use crate::sync::proto;
use crate::sync::service::{
    Action, Command, DeviceInfo, Event, LinkInfo, Outgoing, PlayState, PlayTick,
    Repeat as WireRepeat,
};
use crate::sync::state::{self, DriftCorrector, RevCounter, Winner};
use crate::texts::sentence;
use crate::ui::queue::{ListKey, Queue};
use crate::ui::settings::Entry;
use crate::ui::settings::controls::{Field, setting_row, switch, text_field, value_slider};
use crate::ui::theme::{self, Style};
use crate::ui::widgets::{self, Kind, Size};
use crate::ui::{Message as App, icons::Icon};

/// Сообщения окна «Слушать вместе».
#[derive(Clone)]
pub enum Msg {
    /// Событие службы.
    Event(Box<Event>),
    /// Раз в секунду, пока мы ведущие и играем: `tick`.
    Tick,
    TogglePanel,
    ClosePanel,
    /// «Подключить» устройство из списка.
    Connect(String),
    /// Ввод адреса `ip:порт` и «Подключить по адресу».
    AddressInput(String),
    ConnectAddr,
    Disconnect,
    /// Решение о коде сопряжения.
    Decide(bool),
    /// «Забыть» сопряжённое устройство (секрет удаляет служба, из конфига —
    /// приложение).
    Forget(String),
}

/// Что делает связь сейчас, пока её нет.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) enum Status {
    #[default]
    Idle,
    Connecting(String),
    Restoring(String),
}

/// Код сопряжения на экране.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Pairing {
    pub name: String,
    pub os: String,
    pub code: String,
    /// Свой «Коды совпадают» уже нажат: ждём собеседника.
    pub decided: bool,
}

/// Что ведущий показывает ведомому: режимы (очередь — в самой очереди).
#[derive(Debug, Clone, Default)]
pub(super) struct MasterView {
    pub shuffle: bool,
    pub repeat: WireRepeat,
    pub wave: bool,
}

/// Готовая связь.
pub(super) struct Link {
    pub peer: crate::sync::handshake::PeerInfo,
    pub role: Role,
    /// Кто ведущий (`owner`); пусто — общего состояния нет.
    pub owner: String,
    /// Версия общего состояния `(rev, by)`.
    pub version: (u64, String),
    /// `rev` общего состояния: по нему сверяются `tick`.
    pub state_rev: u64,
    /// Начальный обмен (§4.4) состоялся.
    pub resolved: bool,
    /// Своё начальное состояние и снимок на тот момент, пока ждём чужое.
    pending: Option<(PlayState, Snapshot)>,
    pub drift: DriftCorrector,
    pub master: MasterView,
}

/// Снимок того, что видит собеседник.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Snapshot {
    track: Option<String>,
    playing: bool,
    session: u64,
    seeks: u64,
    shuffle: bool,
    repeat: WireRepeat,
    wave: bool,
    next: Option<String>,
}

/// «Слушать вместе»: всё, что окно знает о службе и связи.
pub(super) struct Together {
    /// Идентификатор этого устройства, пока служба запущена.
    pub own: Option<String>,
    pub name: String,
    pub delay_ms: i64,
    pub enabled: bool,
    pub paired: Vec<PairedDevice>,
    pub devices: Vec<DeviceInfo>,
    pub addresses: Vec<String>,
    pub problem: Option<String>,
    pub last_failure: Option<String>,
    pub status: Status,
    pub link: Option<Link>,
    pub pairing: Option<Pairing>,
    pub address: String,
    pub address_error: Option<String>,
    pub panel: iced::animation::Animation<bool>,
    /// Команды службе: приложение забирает их после каждого сообщения.
    outbox: Vec<Command>,
    epoch: Instant,
    rev: RevCounter,
    base: Option<Snapshot>,
    /// После нового состояния ведущий шлёт `tick` сразу, как пойдёт звук.
    tick_due: bool,
    /// Ведущий до обрыва связи и когда это было: на восстановленной связи он
    /// остаётся ведущим (§4.4). Забывается через 10 минут и при `bye`.
    kept_owner: Option<(String, Instant)>,
}

impl Together {
    pub(super) fn new() -> Self {
        Self {
            own: None,
            name: String::new(),
            delay_ms: 0,
            enabled: false,
            paired: Vec::new(),
            devices: Vec::new(),
            addresses: Vec::new(),
            problem: None,
            last_failure: None,
            status: Status::Idle,
            link: None,
            pairing: None,
            address: String::new(),
            address_error: None,
            panel: anim::popover(false),
            outbox: Vec::new(),
            epoch: Instant::now(),
            rev: RevCounter::default(),
            base: None,
            tick_due: false,
            kept_owner: None,
        }
    }

    /// Мы ведомые: связь готова, ведущий — собеседник.
    pub(super) fn follower(&self) -> bool {
        let (Some(own), Some(link)) = (&self.own, &self.link) else {
            return false;
        };
        link.resolved && !link.owner.is_empty() && link.owner != *own
    }

    /// Мы ведущие.
    pub(super) fn leader(&self) -> bool {
        let (Some(own), Some(link)) = (&self.own, &self.link) else {
            return false;
        };
        link.resolved && link.owner == *own
    }

    pub(super) fn linked(&self) -> bool {
        self.link.is_some()
    }

    fn ms(&self, at: Instant) -> i64 {
        match at.checked_duration_since(self.epoch) {
            Some(after) => i64::try_from(after.as_millis()).unwrap_or(i64::MAX),
            None => -i64::try_from(self.epoch.duration_since(at).as_millis()).unwrap_or(i64::MAX),
        }
    }

    pub(super) fn now_ms(&self) -> i64 {
        self.ms(Instant::now())
    }

    /// Состояние из конфига (имя, задержка, сопряжённые, включено).
    pub(super) fn set_config(&mut self, config: &SyncConfig, default_name: &str) {
        self.enabled = config.enabled();
        self.delay_ms = config.delay_ms();
        self.paired = config.paired.clone();
        self.name = config
            .device_name
            .clone()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| default_name.to_owned());
    }

    fn push(&mut self, command: Command) {
        self.outbox.push(command);
    }
}

/// Повтор библиотеки в виде протокола.
fn wire_repeat(repeat: Repeat) -> WireRepeat {
    match repeat {
        Repeat::Off => WireRepeat::Off,
        Repeat::All => WireRepeat::All,
        Repeat::One => WireRepeat::One,
    }
}

/// Состояние для выбора по §4.4 (важны только трек, `playing`, `join`,
/// версия).
fn choice_view(state: &PlayState) -> proto::State {
    proto::State {
        rev: state.rev,
        by: state.by.clone(),
        owner: state.owner.clone(),
        track: state.track.as_ref().map(proto::WireTrack::from_track),
        playing: state.playing,
        position_ms: 0,
        at: 0,
        next: None,
        shuffle: false,
        repeat: state.repeat,
        wave: false,
        initial: state.initial,
        join: state.join,
    }
}

fn millis(seconds: f32) -> u64 {
    Duration::try_from_secs_f32(seconds.max(0.0))
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn seconds(ms: u64) -> f32 {
    ms as f32 / 1000.0
}

impl Library {
    // ---------------------------------------------------------- настройка

    /// Приложение сообщает, что служба запущена (`Some(id)`) или остановлена.
    pub fn together_started(&mut self, id: Option<String>) {
        if id.is_none() {
            self.together_reset();
        }
        self.together.own = id;
    }

    /// Настройки из `config.toml`.
    pub fn together_config(&mut self, config: &SyncConfig, default_name: &str) {
        self.together.set_config(config, default_name);
    }

    /// Команды службе, накопившиеся за сообщение.
    pub fn together_take(&mut self) -> Vec<Command> {
        std::mem::take(&mut self.together.outbox)
    }

    pub fn together_open(&self) -> bool {
        self.together.panel.value()
    }

    /// Закрывает карточку «Слушать вместе». `true`, если она была открыта.
    pub fn close_together_panel(&mut self) -> bool {
        let open = self.together.panel.value();
        if open {
            self.together.panel.go_mut(false, Instant::now());
        }
        open
    }

    pub(super) fn together_animating(&self, now: Instant) -> bool {
        self.together.panel.is_animating(now)
    }

    /// Режимы ведущего для показа ведомому. `None` — мы не ведомые.
    pub(super) fn together_master(&self) -> Option<&MasterView> {
        if !self.together.follower() {
            return None;
        }
        self.together.link.as_ref().map(|link| &link.master)
    }

    /// Связь оборвана или службы нет: возвращаемся к своему плееру. Ведомый
    /// доигрывает текущий трек и останавливается.
    fn together_reset(&mut self) {
        let was_follower = self.together.follower();
        self.together.link = None;
        self.together.pairing = None;
        self.together.status = Status::Idle;
        self.together.base = None;
        self.together.devices.clear();
        self.together.addresses.clear();
        if was_follower {
            self.together_trim_queue();
        }
    }

    /// Очередь ведомого после разрыва: только текущий трек (своей очереди у
    /// него нет).
    fn together_trim_queue(&mut self) {
        if let Some(queue) = &mut self.queue
            && queue.key == ListKey::Remote
            && let Some(track) = queue.track().cloned()
        {
            queue.tracks = Arc::new(vec![track]);
            queue.current = 0;
        }
    }

    // ------------------------------------------------------- перехват кнопок

    /// Кнопки ведомого, которые решает ведущий: уходят ему командой, а не
    /// выполняются у себя. `true` — сообщение забрано.
    pub(super) fn together_intercept(&mut self, message: &Message) -> bool {
        if !self.together.follower() {
            return false;
        }
        let action = match message {
            Message::Next => Action::Next,
            Message::Previous => Action::Previous,
            Message::ToggleShuffle => Action::Shuffle,
            Message::CycleRepeat => Action::Repeat,
            Message::Wave(super::WaveMsg::Dislike)
                if self.together.link.as_ref().is_some_and(|l| l.master.wave) =>
            {
                Action::Dislike
            }
            _ => return false,
        };
        self.together.push(Command::Send(Outgoing::Command(action)));
        true
    }

    // ------------------------------------------------------------ сообщения

    pub(super) fn together_msg(
        &mut self,
        msg: Msg,
        http: Option<&reqwest::Client>,
    ) -> (Task<Message>, Option<Effect>) {
        let now = Instant::now();
        let effect = match msg {
            Msg::Event(event) => return self.together_event(*event, http),
            Msg::Tick => {
                self.together_send_tick();
                None
            }
            Msg::TogglePanel => {
                let open = !self.together.panel.value();
                self.together.panel.go_mut(open, now);
                None
            }
            Msg::ClosePanel => {
                self.together.panel.go_mut(false, now);
                None
            }
            Msg::Connect(id) => {
                let name = self
                    .together
                    .devices
                    .iter()
                    .find(|device| device.id == id)
                    .map_or_else(|| "устройству".to_owned(), |device| device.name.clone());
                self.together.status = Status::Connecting(name);
                self.together.last_failure = None;
                self.together.push(Command::Connect { id });
                None
            }
            Msg::AddressInput(text) => {
                self.together.address = text;
                self.together.address_error = None;
                None
            }
            Msg::ConnectAddr => {
                match self.together.address.trim().parse::<std::net::SocketAddr>() {
                    Ok(addr) => {
                        self.together.address_error = None;
                        self.together.status = Status::Connecting(addr.to_string());
                        self.together.last_failure = None;
                        self.together.push(Command::ConnectAddr(addr));
                    }
                    Err(_) => {
                        self.together.address_error =
                            Some("Введите адрес вида 192.168.0.5:47800".to_owned());
                    }
                }
                None
            }
            Msg::Disconnect => {
                self.together.status = Status::Idle;
                self.together.push(Command::Disconnect);
                None
            }
            Msg::Decide(accept) => {
                let decided = self
                    .together
                    .pairing
                    .as_ref()
                    .is_some_and(|pairing| pairing.decided);
                // Своё «да» уже ушло: «Отмена» обрывает подключение, второй
                // ответ служба не примет.
                let command = if !accept && decided {
                    Command::Disconnect
                } else {
                    Command::Decide(accept)
                };
                self.together.push(command);
                if accept {
                    if let Some(pairing) = &mut self.together.pairing {
                        pairing.decided = true;
                    }
                } else {
                    self.together.pairing = None;
                    self.together.status = Status::Idle;
                }
                None
            }
            Msg::Forget(id) => {
                self.together.push(Command::Forget { id: id.clone() });
                self.together.paired.retain(|device| device.id != id);
                None
            }
        };
        (Task::none(), effect)
    }

    fn together_event(
        &mut self,
        event: Event,
        http: Option<&reqwest::Client>,
    ) -> (Task<Message>, Option<Effect>) {
        let effect = match event {
            Event::Devices(list) => {
                self.together.devices = list;
                None
            }
            Event::Addresses(list) => {
                self.together.addresses = list;
                None
            }
            Event::Pairing {
                peer,
                code,
                initiator: _,
            } => {
                self.together.status = Status::Idle;
                self.together.pairing = Some(Pairing {
                    name: peer.name,
                    os: peer.os,
                    code,
                    decided: false,
                });
                // Карточка «Слушать вместе» не остаётся под диалогом.
                self.close_together_panel();
                None
            }
            Event::Paired { peer, .. } => {
                self.together.pairing = None;
                let device = PairedDevice {
                    id: peer.id,
                    name: peer.name,
                    os: peer.os,
                };
                match self
                    .together
                    .paired
                    .iter_mut()
                    .find(|known| known.id == device.id)
                {
                    Some(known) => *known = device.clone(),
                    None => self.together.paired.push(device.clone()),
                }
                Some(Effect::TogetherPaired(device))
            }
            Event::Linked(info) => {
                self.together_linked(info);
                None
            }
            Event::State(state) => self.together_remote_state(*state),
            Event::Tick(tick) => {
                self.together_remote_tick(tick);
                None
            }
            Event::Command(action) => return self.together_remote_command(action, http),
            Event::Liked { track, liked } => {
                self.together_liked(&track, liked);
                None
            }
            Event::Disconnected { peer, text, lost } => {
                // Оборвалась сама: ведущего запоминаем для восстановления.
                self.together.kept_owner = self
                    .together
                    .link
                    .as_ref()
                    .filter(|link| lost && !link.owner.is_empty())
                    .map(|link| (link.owner.clone(), Instant::now()));
                self.together_reset();
                self.together.status = if lost {
                    Status::Restoring(peer.name)
                } else {
                    Status::Idle
                };
                Some(Effect::Toast(text))
            }
            Event::Failed { text } => {
                self.together.pairing = None;
                self.together.status = Status::Idle;
                self.together.last_failure = Some(text.clone());
                Some(Effect::Toast(text))
            }
            Event::Problem(text) => {
                self.together.problem = Some(text.clone());
                Some(Effect::Toast(text))
            }
        };
        (Task::none(), effect)
    }

    // ----------------------------------------------------------- снимок

    fn together_repeat(&self) -> WireRepeat {
        if self.in_wave() {
            if self.repeat_one() {
                WireRepeat::One
            } else {
                WireRepeat::Off
            }
        } else {
            wire_repeat(self.player.repeat)
        }
    }

    /// Следующий трек очереди для показа. Очередь не меняется.
    fn together_next(&self) -> Option<Track> {
        let queue = self.queue.as_ref()?;
        let censor = self.plugins.fckcensor.then_some(&self.censor);
        let relinks = &self.relinks;
        let index = queue
            .upcoming(
                self.player.shuffle && !self.in_wave(),
                &|track| plugins::playable(censor, relinks, track),
                1,
            )
            .first()
            .copied()?;
        queue.get(index).cloned()
    }

    fn together_snapshot(&self) -> Snapshot {
        Snapshot {
            track: self.now_playing.as_ref().map(|track| track.id.clone()),
            playing: self.player.playing,
            session: self.player.session,
            seeks: self.player.seeks,
            shuffle: self.player.shuffle,
            repeat: self.together_repeat(),
            wave: self.in_wave(),
            next: self.together_next().map(|track| track.id),
        }
    }

    /// Позиция сейчас, мс: из последнего события плеера и времени с него.
    fn together_position_ms(&self) -> u64 {
        let player: &Player = &self.player;
        let mut position = player.position;
        if player.playing && !player.buffering {
            position += player.position_at.elapsed().as_secs_f32();
        }
        millis(position)
    }

    /// Своё состояние для отправки.
    fn together_state(&self, owner: &str, rev: u64, initial: bool, join: bool) -> PlayState {
        // Позиция в состоянии синхронная: у ведомого своя поправка задержки
        // вычитается (она нужна только при применении у себя).
        let correction = if self.together.own.as_deref() == Some(owner) {
            0
        } else {
            self.together.delay_ms
        };
        let local = i64::try_from(self.together_position_ms()).unwrap_or(i64::MAX);
        let position_ms = u64::try_from(local.saturating_sub(correction).max(0)).unwrap_or(0);
        PlayState {
            rev,
            by: self.together.own.clone().unwrap_or_default(),
            owner: owner.to_owned(),
            track: self.now_playing.clone(),
            playing: self.player.playing,
            position: Duration::from_millis(position_ms),
            at: Instant::now(),
            next: self.together_next(),
            shuffle: self.player.shuffle,
            repeat: self.together_repeat(),
            wave: self.in_wave(),
            initial,
            join,
        }
    }

    /// База разницы — то, что собеседник уже знает.
    fn together_rebase(&mut self) {
        if self.together.link.is_some() {
            self.together.base = Some(self.together_snapshot());
        }
    }

    /// Конец `Library::update`: что изменилось у нас против базы, уходит
    /// собеседнику новым состоянием.
    pub(super) fn together_sync(&mut self) {
        let Some(me) = self.together.own.clone() else {
            return;
        };
        let Some(link) = &self.together.link else {
            return;
        };
        // До решения §4.4 своих состояний не шлём: база останется прежней, и
        // изменение уйдёт после.
        if !link.resolved {
            return;
        }
        let owner = link.owner.clone();
        let now = self.together_snapshot();
        let Some(base) = self.together.base.clone() else {
            self.together.base = Some(now);
            return;
        };
        if now == base {
            return;
        }
        // Сменился трек — его выбрали у нас: мы ведущие. Иначе (пауза,
        // перемотка) ведущий прежний.
        let owner = if now.track != base.track || owner.is_empty() {
            me.clone()
        } else {
            owner
        };
        let rev = self.together.rev.bump();
        let state = self.together_state(&owner, rev, false, false);
        if let Some(link) = &mut self.together.link {
            link.owner = owner;
            link.version = (rev, me);
            link.state_rev = rev;
        }
        self.together.base = Some(now);
        self.together.tick_due = true;
        self.together
            .push(Command::Send(Outgoing::State(Box::new(state))));
    }

    // ------------------------------------------------------------ связь

    fn together_linked(&mut self, info: LinkInfo) {
        let Some(me) = self.together.own.clone() else {
            return;
        };
        self.together.status = Status::Idle;
        self.together.pairing = None;
        self.together.last_failure = None;
        // Восстановленная связь: ведущий — каким он был до обрыва.
        let owner = match self.together.kept_owner.take() {
            Some((owner, at)) if info.restored && at.elapsed() < RESTORE_WINDOW => owner,
            _ => me.clone(),
        };
        let initial = self.together_state(&owner, self.together.rev.current(), true, info.join);
        let base = self.together_snapshot();
        self.together.link = Some(Link {
            peer: info.peer,
            role: info.role,
            owner: String::new(),
            version: (0, String::new()),
            state_rev: 0,
            resolved: false,
            pending: Some((initial.clone(), base)),
            drift: DriftCorrector::new(),
            master: MasterView::default(),
        });
        self.together
            .push(Command::Send(Outgoing::State(Box::new(initial))));
    }

    fn together_remote_state(&mut self, state: PlayState) -> Option<Effect> {
        let me = self.together.own.clone()?;
        let link = self.together.link.as_mut()?;
        if state.initial {
            // Начальный обмен §4.4: одинаковый выбор у обеих сторон.
            let (mine, base) = link.pending.take()?;
            let winner = state::choose_initial(
                &choice_view(&mine),
                &choice_view(&state),
                link.role == Role::Initiator,
            );
            link.resolved = true;
            self.together.rev.observe(state.rev);
            let rev_now = self.together.rev.current();
            let _ = rev_now;
            return match winner {
                Winner::Local => {
                    if let Some(link) = &mut self.together.link {
                        link.owner = me.clone();
                        link.version = (mine.rev, me);
                        link.state_rev = mine.rev;
                    }
                    self.together.base = Some(base);
                    None
                }
                Winner::Remote => {
                    let effect = self.together_apply(&state);
                    self.together_rebase();
                    effect
                }
                Winner::Nobody => {
                    if let Some(link) = &mut self.together.link {
                        link.owner = String::new();
                        link.version = (0, String::new());
                    }
                    self.together.base = Some(base);
                    None
                }
            };
        }
        if !link.resolved {
            return None;
        }
        let newer = state::is_newer(
            (state.rev, state.by.as_str()),
            (link.version.0, link.version.1.as_str()),
        );
        if !newer {
            return None;
        }
        self.together.rev.observe(state.rev);
        let effect = self.together_apply(&state);
        self.together_rebase();
        effect
    }

    /// Применяет чужое состояние: пауза, перемотка, новый трек.
    fn together_apply(&mut self, state: &PlayState) -> Option<Effect> {
        let me = self.together.own.clone()?;
        let leader = state.owner == me;
        let correction = if leader { 0 } else { self.together.delay_ms };
        if let Some(link) = &mut self.together.link {
            link.owner = state.owner.clone();
            link.version = (state.rev, state.by.clone());
            link.state_rev = state.rev;
            if !leader {
                link.master = MasterView {
                    shuffle: state.shuffle,
                    repeat: state.repeat,
                    wave: state.wave,
                };
            }
        }
        let Some(track) = &state.track else {
            // Ведущий ничего не играет: ведомый останавливается.
            if !leader && (self.player.loaded || self.player.playing) {
                self.send(PlayerCommand::Stop);
                self.halt();
            }
            return None;
        };
        let same = self
            .now_playing
            .as_ref()
            .is_some_and(|current| current.id == track.id);
        if !leader {
            self.together_follower_queue(state);
        }
        if same {
            return self.together_same_track(state, correction);
        }
        if leader {
            // Ведомый прислал не наш трек (гонка): ждём своего состояния.
            return None;
        }
        self.together_launch(state, correction)
    }

    /// Очередь ведомого: текущий трек и «Далее» ведущего.
    fn together_follower_queue(&mut self, state: &PlayState) {
        let Some(track) = &state.track else {
            return;
        };
        let mut list = vec![track.clone()];
        if let Some(next) = &state.next {
            list.push(next.clone());
        }
        let same_queue = self
            .queue
            .as_ref()
            .is_some_and(|queue| queue.key == ListKey::Remote);
        if same_queue
            && self
                .now_playing
                .as_ref()
                .is_some_and(|current| current.id == track.id)
            && let Some(queue) = &mut self.queue
        {
            queue.tracks = Arc::new(list);
            queue.current = 0;
        }
    }

    fn together_target_ms(&self, state: &PlayState, correction: i64) -> u64 {
        state::position_now(
            u64::try_from(state.position.as_millis()).unwrap_or(u64::MAX),
            self.together.ms(state.at),
            state.playing,
            0,
            self.together.now_ms(),
            correction,
        )
    }

    /// Тот же трек: пауза или продолжение и перемотка при расхождении
    /// больше 120 мс.
    fn together_same_track(&mut self, state: &PlayState, correction: i64) -> Option<Effect> {
        let target = self.together_target_ms(state, correction);
        let now_ms = self.together.now_ms();
        if self.player.loaded {
            if state.playing != self.player.playing {
                self.player.playing = state.playing;
                self.send(if state.playing {
                    PlayerCommand::Resume
                } else {
                    PlayerCommand::Pause
                });
            }
            let local = i64::try_from(self.together_position_ms()).unwrap_or(i64::MAX);
            let target_signed = i64::try_from(target).unwrap_or(i64::MAX);
            if (local - target_signed).abs() > state::DRIFT_THRESHOLD_MS {
                self.seek_to(seconds(target));
                self.together_quiet(now_ms);
                // Ведущий перемотал: первый его `tick` исправит позицию сразу.
                if let Some(link) = &mut self.together.link {
                    link.drift.expect_first_tick();
                }
            }
            return None;
        }
        // Трек у нас кончился или не включился, а ведущий играет: запускаем.
        if state.playing {
            let index = self.queue.as_ref().map_or(0, |queue| queue.current);
            self.player.skipped = 0;
            self.together_quiet(now_ms);
            return self.start(index, seconds(target));
        }
        None
    }

    /// Новый трек ведущего: у ведомого очередь из него и «Далее».
    fn together_launch(&mut self, state: &PlayState, correction: i64) -> Option<Effect> {
        let track = state.track.clone()?;
        let mut list = vec![track];
        if let Some(next) = &state.next {
            list.push(next.clone());
        }
        self.queue = Some(Queue::new(ListKey::Remote, Arc::new(list), 0));
        // Таблетка источника на экране волны: «Слушаю с …», а не «Очередь».
        self.wave.source = match &self.together.link {
            Some(link) => format!("Слушаю с {}", link.peer.name),
            None => "Слушаю вместе".to_owned(),
        };
        self.player.skipped = 0;
        let target = self.together_target_ms(state, correction);
        let now_ms = self.together.now_ms();
        let effect = self.start(0, seconds(target));
        if !state.playing && self.player.loaded {
            self.player.playing = false;
            self.send(PlayerCommand::Pause);
        }
        self.together_quiet(now_ms);
        // Новый трек ведущего: первый его `tick` исправит позицию сразу.
        if let Some(link) = &mut self.together.link {
            link.drift.expect_first_tick();
        }
        effect
    }

    fn together_quiet(&mut self, now_ms: i64) {
        if let Some(link) = &mut self.together.link {
            link.drift.on_local_start(now_ms);
        }
    }

    /// `tick` ведущего: подстройка позиции ведомого (§5.2).
    fn together_remote_tick(&mut self, tick: PlayTick) {
        let Some(me) = self.together.own.clone() else {
            return;
        };
        let delay = self.together.delay_ms;
        let now_ms = self.together.now_ms();
        let Some(link) = &self.together.link else {
            return;
        };
        if !link.resolved || link.owner == me || link.owner.is_empty() || tick.rev != link.state_rev
        {
            return;
        }
        if !self.player.loaded {
            return;
        }
        let target = state::position_now(
            u64::try_from(tick.position.as_millis()).unwrap_or(u64::MAX),
            self.together.ms(tick.at),
            true,
            0,
            now_ms,
            delay,
        );
        let local = self.together_position_ms();
        let buffering = self.player.buffering;
        let seek = self.together.link.as_mut().and_then(|link| {
            link.drift
                .on_tick(now_ms, local, target, buffering, tick.buffering)
        });
        if let Some(target) = seek {
            self.seek_to(seconds(target));
            self.together_rebase();
        }
    }

    /// Ведущий шлёт `tick` по таймеру и сразу после запуска или перемотки.
    fn together_send_tick(&mut self) {
        if !self.together.leader() || !self.player.playing {
            return;
        }
        let rev = self.together.link.as_ref().map_or(0, |link| link.state_rev);
        let tick = PlayTick {
            rev,
            position: Duration::from_millis(self.together_position_ms()),
            at: Instant::now(),
            buffering: self.player.buffering,
        };
        self.together.push(Command::Send(Outgoing::Tick(tick)));
    }

    /// Пошёл звук: после нового состояния ведущий сразу шлёт `tick`.
    pub(super) fn together_progress(&mut self) {
        if self.together.tick_due && !self.player.buffering {
            self.together.tick_due = false;
            self.together_send_tick();
        }
    }

    /// Нужен ли таймер `tick`: мы ведущие и играем.
    pub(super) fn together_ticking(&self) -> bool {
        self.together.leader() && self.player.playing
    }

    /// Ведомый: трек кончился или не включился — своей очереди нет, ждём
    /// состояние ведущего.
    pub(super) fn together_follower(&self) -> bool {
        self.together.follower()
    }

    pub(super) fn together_settled(&mut self) {
        self.together_rebase();
    }

    /// Ведомый не смог включить трек: сообщение без пропуска дальше.
    pub(super) fn together_failed(&mut self, title: &str, error: &str) -> Option<Effect> {
        self.together_rebase();
        Some(Effect::Toast(sentence(&format!(
            "Не удалось включить «{title}» на этом устройстве: {error}"
        ))))
    }

    // ------------------------------------------------------ команды, лайки

    /// Ведомый просит ведущего выполнить действие как свою кнопку.
    fn together_remote_command(
        &mut self,
        action: Action,
        http: Option<&reqwest::Client>,
    ) -> (Task<Message>, Option<Effect>) {
        if !self.together.leader() {
            return (Task::none(), None);
        }
        let message = match action {
            Action::Next => Message::Next,
            Action::Previous => Message::Previous,
            Action::Shuffle => Message::ToggleShuffle,
            Action::Repeat => Message::CycleRepeat,
            Action::Dislike => Message::Wave(super::WaveMsg::Dislike),
        };
        self.handle(message, http)
    }

    /// Мы поставили или сняли сердце: собеседник обновит своё.
    pub(super) fn together_notify_liked(&mut self, track: &str, liked: bool) {
        if self.together.link.is_some() {
            self.together.push(Command::Send(Outgoing::Liked {
                track: track.to_owned(),
                liked,
            }));
        }
    }

    /// Собеседник поставил или снял сердце: обновляем своё без запроса.
    fn together_liked(&mut self, id: &str, liked: bool) {
        let known = self
            .queue
            .as_ref()
            .and_then(|queue| queue.tracks.iter().find(|track| track.id == id))
            .or(self.now_playing.as_ref().filter(|track| track.id == id))
            .cloned();
        match known {
            Some(track) => self.set_liked(&track, liked),
            None if liked => {
                self.liked.insert(id.to_owned());
            }
            None => {
                self.liked.remove(id);
            }
        }
    }
}

// ------------------------------------------------------------------- вид

/// Сколько после обрыва помнится прежний ведущий (§5.3).
const RESTORE_WINDOW: Duration = Duration::from_secs(600);

/// Ширина карточки «Слушать вместе».
const PANEL_WIDTH: f32 = 420.0;

fn os_title(os: &str) -> &'static str {
    match os {
        "windows" => "Windows",
        "linux" => "Linux",
        "macos" => "macOS",
        "android" => "Android",
        _ => "устройство",
    }
}

impl Library {
    /// Подпись «Вместе с <имя>» для ведомого (в плеере).
    pub(super) fn together_caption(&self) -> Option<String> {
        let link = self.together.link.as_ref()?;
        Some(format!("Вместе с {}", link.peer.name))
    }

    /// Связь есть (значок «Устройства» горит акцентом).
    pub(super) fn together_is_linked(&self) -> bool {
        self.together.linked()
    }

    /// Карточка «Слушать вместе» над плеером. `None` — закрыта.
    pub fn together_panel(&self, now: Instant) -> Option<Element<'_, App>> {
        let frame = anim::Popover::at(&self.together.panel, now);
        if !frame.visible() {
            return None;
        }
        let header = row![
            container(theme::line(
                "Слушать вместе",
                Style::Title,
                theme::TEXT_PRIMARY
            ))
            .width(Length::Fill),
            widgets::icon_button(
                Icon::Close,
                Size::S,
                false,
                Some(Lib(Message::Together(Msg::ClosePanel)))
            ),
        ]
        .align_y(Alignment::Center);
        let content = anim::motion(column![header, self.together_body()].spacing(12))
            .reveal(theme::SURFACE_2, frame.content);
        let card = container(content)
            .width(Length::Fill)
            .max_width(PANEL_WIDTH)
            .padding(20)
            .style(move |_| widgets::popover_card(frame.shell, 16.0));
        let hidden = 1.0 - frame.motion;
        let card = anim::motion(iced::widget::opaque(card))
            .offset(0.0, 12.0 * hidden)
            .scale(0.97 + 0.03 * frame.motion);
        let placed = container(card)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::End)
            .align_y(Alignment::End)
            .padding(Padding {
                right: SOUND_PANEL_MARGIN,
                bottom: PLAYER_HEIGHT + 1.0 + SOUND_PANEL_MARGIN,
                left: SOUND_PANEL_MARGIN,
                top: SOUND_PANEL_MARGIN,
            });
        Some(iced::widget::opaque(
            mouse_area(placed).on_press(Lib(Message::Together(Msg::ClosePanel))),
        ))
    }

    /// Содержимое карточки по состоянию.
    fn together_body(&self) -> Element<'_, App> {
        let together = &self.together;
        let note = |text: String| -> Element<'_, App> {
            theme::label(text, Style::Secondary, theme::TEXT_SECONDARY).into()
        };
        if !together.enabled {
            return column![
                note(
                    "Слушайте один и тот же трек на двух своих устройствах в одной сети: \
                     звук каждое устройство берёт у Яндекса само. Функция выключена."
                        .to_owned()
                ),
                widgets::action(
                    Kind::Primary,
                    None,
                    "Включить",
                    Some(App::SetSyncEnabled(true))
                ),
            ]
            .spacing(12)
            .into();
        }
        let mut body = column![].spacing(10);
        if let Some(link) = &together.link {
            body = body
                .push(theme::label(
                    format!("Вместе с {}", link.peer.name),
                    Style::BodyStrong,
                    theme::TEXT_PRIMARY,
                ))
                .push(note(format!(
                    "{}. {}",
                    os_title(&link.peer.os),
                    if together.follower() {
                        "Играет то, что включено на нём"
                    } else if together.leader() {
                        "Играет то, что включено здесь"
                    } else {
                        "Включите трек на любом из устройств"
                    }
                )))
                .push(widgets::action(
                    Kind::Secondary,
                    None,
                    "Отключить",
                    Some(Lib(Message::Together(Msg::Disconnect))),
                ));
        } else {
            match &together.status {
                Status::Connecting(name) => {
                    body = body.push(note(format!("Подключаемся к {name}…")));
                }
                Status::Restoring(name) => {
                    body = body.push(note(format!("Восстанавливаем связь с {name}…")));
                }
                Status::Idle => {}
            }
            body = body.push(self.together_devices());
        }
        if let Some(text) = &together.last_failure {
            body = body.push(theme::label(text.clone(), Style::Secondary, theme::ERROR));
        }
        if let Some(text) = &together.problem {
            body = body.push(theme::label(text.clone(), Style::Secondary, theme::ERROR));
        }
        body = body.push(widgets::action(
            Kind::Secondary,
            Some(Icon::Settings),
            "Настройки",
            Some(App::OpenSyncSettings),
        ));
        body.into()
    }

    /// Найденные устройства.
    fn together_devices(&self) -> Element<'_, App> {
        let together = &self.together;
        if together.devices.is_empty() {
            return theme::label(
                "Ищем устройства в сети…".to_owned(),
                Style::Secondary,
                theme::TEXT_TERTIARY,
            )
            .into();
        }
        let mut list = column![].spacing(8);
        for device in &together.devices {
            let paired = if device.paired {
                " · сопряжено"
            } else {
                ""
            };
            let info = column![
                theme::line(device.name.clone(), Style::BodyStrong, theme::TEXT_PRIMARY),
                theme::line(
                    format!("{}{paired}", os_title(&device.os)),
                    Style::Secondary,
                    theme::TEXT_TERTIARY
                ),
            ]
            .spacing(2)
            .width(Length::Fill);
            list = list.push(
                row![
                    info,
                    widgets::action(
                        Kind::Primary,
                        None,
                        "Подключить",
                        Some(Lib(Message::Together(Msg::Connect(device.id.clone()))))
                    ),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
            );
        }
        list.into()
    }

    /// Диалог сопряжения: код и «Коды совпадают». `None` — кода нет.
    /// Вид как у остальных диалогов окна (выход из аккаунта, сброс
    /// настроек): заголовок слева, крестик справа, текст, кнопки слева.
    pub fn together_pairing_dialog(&self) -> Option<Element<'_, App>> {
        let pairing = self.together.pairing.as_ref()?;
        let cancel = || Some(Lib(Message::Together(Msg::Decide(false))));
        // После «Коды совпадают» ждём второе устройство: остаётся только «Отмена».
        let text = if pairing.decided {
            format!("Ждём подтверждения на {}…", pairing.name)
        } else {
            format!(
                "Проверьте, что на {} ({}) тот же код.",
                pairing.name,
                os_title(&pairing.os)
            )
        };
        // Кнопки поровну на всю ширину карточки; пока ждём — одна «Отмена» во всю ширину.
        let mut actions = row![].spacing(12).width(Length::Fill);
        if !pairing.decided {
            actions = actions.push(widgets::wide_action(
                Kind::Primary,
                None,
                "Коды совпадают",
                Some(Lib(Message::Together(Msg::Decide(true)))),
            ));
        }
        actions = actions.push(widgets::wide_action(
            Kind::Secondary,
            None,
            "Отмена",
            cancel(),
        ));
        // Код — крупно в плашке на всю ширину, по центру.
        let code = container(theme::line(
            pairing.code.clone(),
            Style::Display,
            theme::TEXT_PRIMARY,
        ))
        .center_x(Length::Fill)
        .padding([16, 0])
        .style(|_| container::Style {
            background: Some(theme::SURFACE_3.into()),
            border: iced::Border::default().rounded(widgets::RADIUS_M),
            ..container::Style::default()
        });
        let body = column![
            theme::line("Сопряжение", Style::Title, theme::TEXT_PRIMARY),
            theme::label(text, Style::Body, theme::TEXT_SECONDARY),
            code,
            actions,
        ]
        .spacing(16)
        .padding(Padding::default().top(4));
        let close =
            iced::widget::right(widgets::icon_button(Icon::Close, Size::S, false, cancel()));
        let card = container(iced::widget::stack![body, close])
            .width(460)
            .padding(24)
            .style(|_| container::Style {
                background: Some(theme::SURFACE_2.into()),
                border: iced::Border {
                    color: theme::OUTLINE,
                    width: 1.0,
                    radius: widgets::RADIUS_XL.into(),
                },
                shadow: widgets::popover_shadow(),
                ..container::Style::default()
            });
        let scrim = container(iced::widget::opaque(card))
            .center(Length::Fill)
            .style(|_| container::Style {
                background: Some(theme::SCRIM.into()),
                ..container::Style::default()
            });
        Some(iced::widget::opaque(mouse_area(scrim)))
    }
}

// ------------------------------------------------------------- настройки

/// Предел поправки задержки и шаг ползунка, мс.
const DELAY_LIMIT: f32 = 500.0;
const DELAY_STEP: f32 = 10.0;

impl Library {
    /// Раздел настроек «Слушать вместе».
    pub(in crate::ui) fn together_entries(&self) -> Vec<Entry<'_>> {
        let together = &self.together;
        let mut entries = Vec::new();

        let hint = "Один трек играет на этом и другом вашем устройстве с тем же аккаунтом, \
                    пока оба в одной сети. Звук каждое устройство берёт у Яндекса само, по \
                    сети идут только команды и позиция. Включите функцию на обоих устройствах"
            .to_owned();
        entries.push(Entry::new(
            "Слушать вместе",
            hint.clone(),
            "устройства синхронизация телефон android",
            setting_row(
                "Слушать вместе",
                hint,
                switch(together.enabled, App::SetSyncEnabled),
            ),
        ));

        let name_hint =
            "Так это устройство видно другим. Применится при следующем включении".to_owned();
        let name_field = text_field(
            Field {
                id: None,
                placeholder: "Имя устройства",
                value: &together.name,
                icon: None,
                width: Length::Fixed(260.0),
            },
            App::SetSyncName,
            None,
        );
        entries.push(Entry::new(
            "Имя устройства",
            name_hint.clone(),
            "название компьютера",
            setting_row("Имя устройства", name_hint, name_field),
        ));

        let delay_hint = "Для Bluetooth-наушников: если звук отстаёт от другого устройства, \
                          сдвиньте позицию этого"
            .to_owned();
        let delay = value_slider(
            format!("{:+} мс", together.delay_ms),
            -DELAY_LIMIT..=DELAY_LIMIT,
            together.delay_ms as f32,
            DELAY_STEP,
            |value| App::SetSyncDelay(value.round() as i64),
            None,
        );
        entries.push(Entry::new(
            "Задержка звука на этом устройстве",
            delay_hint.clone(),
            "bluetooth наушники сдвиг",
            setting_row(
                "Задержка звука на этом устройстве",
                delay_hint,
                container(delay).width(Length::Fixed(300.0)).into(),
            ),
        ));

        // Сопряжённые устройства.
        let paired_hint = if together.paired.is_empty() {
            "Пока нет: при первом подключении оба устройства покажут одинаковый код".to_owned()
        } else {
            "«Забыть» удаляет сопряжение только на этом устройстве".to_owned()
        };
        let mut paired = column![].spacing(8).width(Length::Fill);
        for device in &together.paired {
            paired = paired.push(
                row![
                    column![
                        theme::line(device.name.clone(), Style::Body, theme::TEXT_PRIMARY),
                        theme::line(os_title(&device.os), Style::Caption, theme::TEXT_TERTIARY),
                    ]
                    .width(Length::Fill),
                    widgets::action(
                        Kind::Secondary,
                        None,
                        "Забыть",
                        Some(Lib(Message::Together(Msg::Forget(device.id.clone())))),
                    ),
                ]
                .align_y(Alignment::Center),
            );
        }
        entries.push(Entry::new(
            "Сопряжённые устройства",
            paired_hint.clone(),
            "забыть сопряжение код",
            column![
                theme::line(
                    "Сопряжённые устройства",
                    Style::BodyStrong,
                    theme::TEXT_PRIMARY
                ),
                theme::label(paired_hint, Style::Secondary, theme::TEXT_TERTIARY),
                paired,
            ]
            .spacing(6)
            .padding([16, 0])
            .into(),
        ));

        // Свои адреса и подключение по адресу.
        let addresses = if together.addresses.is_empty() {
            "Адреса появятся, когда функция включена".to_owned()
        } else {
            together.addresses.join("   ")
        };
        let address_hint =
            "Если устройства не видят друг друга (гостевая сеть, брандмауэр), введите на \
             другом устройстве один из этих адресов"
                .to_owned();
        entries.push(Entry::new(
            "Адреса этого устройства",
            address_hint.clone(),
            "ip порт подключить вручную",
            column![
                theme::line(
                    "Адреса этого устройства",
                    Style::BodyStrong,
                    theme::TEXT_PRIMARY
                ),
                theme::label(address_hint, Style::Secondary, theme::TEXT_TERTIARY),
                theme::label(addresses, Style::Body, theme::TEXT_PRIMARY),
            ]
            .spacing(6)
            .padding([16, 0])
            .into(),
        ));
        let mut connect = column![
            theme::line(
                "Подключить по адресу",
                Style::BodyStrong,
                theme::TEXT_PRIMARY
            ),
            row![
                text_field(
                    Field {
                        id: None,
                        placeholder: "192.168.0.5:47800",
                        value: &together.address,
                        icon: None,
                        width: Length::Fill,
                    },
                    |text| Lib(Message::Together(Msg::AddressInput(text))),
                    None,
                ),
                widgets::action(
                    Kind::Primary,
                    None,
                    "Подключить",
                    together
                        .own
                        .is_some()
                        .then_some(Lib(Message::Together(Msg::ConnectAddr))),
                ),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
        ]
        .spacing(6)
        .padding([16, 0]);
        if let Some(error) = &together.address_error {
            connect = connect.push(theme::label(error.clone(), Style::Secondary, theme::ERROR));
        }
        entries.push(Entry::new(
            "Подключить по адресу",
            "ip порт".to_owned(),
            "ip порт вручную адрес",
            connect.into(),
        ));

        let firewall = "При первом включении Windows спросит разрешение для сети: разрешите \
                        для частных сетей. Звук не передаётся между устройствами, каждое берёт \
                        его у Яндекса само"
            .to_owned();
        entries.push(Entry::new(
            "Брандмауэр",
            firewall.clone(),
            "windows firewall разрешение сеть",
            container(theme::label(
                firewall,
                Style::Secondary,
                theme::TEXT_TERTIARY,
            ))
            .padding([16, 0])
            .into(),
        ));
        entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::{Event as AudioEvent, PlayError};
    use crate::sync::handshake::PeerInfo;
    use crate::ui::library::tests::test_library;

    const ME: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const PEER: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn track(id: &str) -> Track {
        Track {
            id: id.to_owned(),
            title: Some(format!("Трек {id}")),
            version: None,
            artists: Vec::new(),
            album_id: None,
            album_title: None,
            duration: Some(Duration::from_secs(200)),
            available: true,
            cover_uri: None,
            warning: crate::api::Warning::None,
            loudness: None,
            lyrics: None,
        }
    }

    fn peer_info() -> PeerInfo {
        PeerInfo {
            id: PEER.to_owned(),
            name: "Телефон".to_owned(),
            os: "android".to_owned(),
        }
    }

    fn feed(library: &mut Library, event: Event) -> Option<Effect> {
        library
            .update(Message::Together(Msg::Event(Box::new(event))), None)
            .1
    }

    fn start_link(library: &mut Library, role: Role, join: bool) {
        library.together_started(Some(ME.to_owned()));
        feed(
            library,
            Event::Linked(LinkInfo {
                peer: peer_info(),
                role,
                join,
                restored: false,
            }),
        );
    }

    /// Состояние собеседника.
    fn remote(rev: u64, by: &str, owner: &str, track_id: Option<&str>, playing: bool) -> PlayState {
        PlayState {
            rev,
            by: by.to_owned(),
            owner: owner.to_owned(),
            track: track_id.map(track),
            playing,
            position: Duration::from_secs(10),
            at: Instant::now(),
            next: None,
            shuffle: false,
            repeat: WireRepeat::Off,
            wave: false,
            initial: false,
            join: false,
        }
    }

    fn states(library: &mut Library) -> Vec<PlayState> {
        library
            .together_take()
            .into_iter()
            .filter_map(|command| match command {
                Command::Send(Outgoing::State(state)) => Some(*state),
                _ => None,
            })
            .collect()
    }

    /// Свой трек играет: плеер «загружен» (звука в тестах нет).
    fn play_own(library: &mut Library, id: &str) {
        library.queue = Some(Queue::new(
            ListKey::Liked,
            Arc::new(vec![track(id), track("next")]),
            0,
        ));
        library.now_playing = Some(track(id));
        library.player.loaded = true;
        library.player.playing = true;
        library.player.session = 1;
        library.player.position = 5.0;
        library.player.position_at = Instant::now();
    }

    fn initial_of(mut state: PlayState) -> Event {
        state.initial = true;
        Event::State(Box::new(state))
    }

    #[test]
    fn link_sends_initial_state_with_join_and_waits_for_the_peer() {
        let mut library = test_library();
        play_own(&mut library, "1");
        start_link(&mut library, Role::Initiator, true);
        let sent = states(&mut library);
        assert_eq!(sent.len(), 1);
        let initial = &sent[0];
        assert!(initial.initial && initial.join);
        assert_eq!((initial.by.as_str(), initial.owner.as_str()), (ME, ME));
        assert_eq!(initial.track.as_ref().map(|t| t.id.as_str()), Some("1"));
        assert_eq!(initial.next.as_ref().map(|t| t.id.as_str()), Some("next"));
        // Пока обмен не состоялся, обычные состояния собеседника игнорируются.
        feed(
            &mut library,
            Event::State(Box::new(remote(5, PEER, PEER, Some("9"), true))),
        );
        assert_eq!(
            library.now_playing.as_ref().map(|t| t.id.as_str()),
            Some("1")
        );
        assert!(!library.together.follower());
    }

    #[test]
    fn initial_exchange_picks_the_same_winner_as_the_peer() {
        // У нас играет трек, у собеседника ничего: ведущие мы, ничего не шлём.
        let mut library = test_library();
        play_own(&mut library, "1");
        start_link(&mut library, Role::Responder, false);
        let _ = states(&mut library);
        feed(&mut library, initial_of(remote(3, PEER, PEER, None, false)));
        assert!(library.together.leader());
        assert!(states(&mut library).is_empty());
        // Счётчик ревизий — не меньше чужого.
        assert!(library.together.rev.current() >= 3);

        // Наоборот: трек только у собеседника — мы ведомые, играем его.
        let mut library = test_library();
        start_link(&mut library, Role::Responder, false);
        let _ = states(&mut library);
        feed(
            &mut library,
            initial_of(remote(3, PEER, PEER, Some("7"), true)),
        );
        assert!(library.together.follower());
        assert_eq!(
            library.now_playing.as_ref().map(|t| t.id.as_str()),
            Some("7")
        );
        assert_eq!(
            library.queue.as_ref().map(|queue| queue.key.clone()),
            Some(ListKey::Remote)
        );
        // Применение чужого состояния не рождает эха.
        assert!(states(&mut library).is_empty());

        // «Подключить» у собеседника (join) отдаёт победу ответчику, то есть нам.
        let mut library = test_library();
        play_own(&mut library, "1");
        start_link(&mut library, Role::Responder, false);
        let _ = states(&mut library);
        library.together.base = Some(library.together_snapshot());
        let mut peer = remote(9, PEER, PEER, Some("7"), true);
        peer.join = true;
        feed(&mut library, initial_of(peer));
        assert!(library.together.leader());

        // Трека нет ни у кого: ведущего нет.
        let mut library = test_library();
        start_link(&mut library, Role::Initiator, false);
        let _ = states(&mut library);
        feed(&mut library, initial_of(remote(0, PEER, PEER, None, false)));
        assert!(library.together.link.as_ref().is_some_and(|l| l.resolved));
        assert!(!library.together.leader() && !library.together.follower());
    }

    /// Связь готова, ведущий — мы, играет трек «1».
    fn leader_playing() -> Library {
        let mut library = test_library();
        play_own(&mut library, "1");
        start_link(&mut library, Role::Initiator, false);
        let _ = states(&mut library);
        feed(&mut library, initial_of(remote(0, PEER, PEER, None, false)));
        assert!(library.together.leader());
        library
    }

    /// Связь готова, ведущий — собеседник, играет трек «7».
    fn follower_playing() -> Library {
        let mut library = test_library();
        start_link(&mut library, Role::Initiator, false);
        let _ = states(&mut library);
        let mut peer = remote(4, PEER, PEER, Some("7"), true);
        peer.next = Some(track("8"));
        feed(&mut library, initial_of(peer));
        assert!(library.together.follower());
        // В тестах звука нет, поэтому трек «играет» вручную.
        library.player.loaded = true;
        library.player.playing = true;
        library.player.session = 1;
        library.player.position = 10.0;
        library.player.position_at = Instant::now();
        library.together_rebase();
        library
    }

    #[test]
    fn own_changes_go_out_as_new_state_and_bump_the_revision() {
        let mut library = leader_playing();
        let before = library.together.rev.current();
        // Пауза.
        let _ = library.update(Message::TogglePlay, None);
        let sent = states(&mut library);
        assert_eq!(sent.len(), 1);
        assert!(!sent[0].playing && !sent[0].initial);
        assert_eq!(sent[0].rev, before + 1);
        assert_eq!(sent[0].owner, ME);
        // Без изменений ничего не уходит.
        let _ = library.update(Message::Noop, None);
        assert!(states(&mut library).is_empty());
        // Перемотка.
        let _ = library.update(Message::SeekTo(100.0), None);
        let sent = states(&mut library);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].rev, before + 2);
        assert_eq!(sent[0].position, Duration::from_secs(100));
    }

    #[test]
    fn new_track_makes_us_the_leader() {
        let mut library = follower_playing();
        // Выбрали свой трек (смена трека и запуска): owner — мы.
        library.now_playing = Some(track("42"));
        library.player.session += 1;
        let _ = library.update(Message::Noop, None);
        let sent = states(&mut library);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].owner, ME);
        assert_eq!(sent[0].track.as_ref().map(|t| t.id.as_str()), Some("42"));
        assert!(library.together.leader());
    }

    #[test]
    fn follower_buttons_become_commands_for_the_leader() {
        let mut library = follower_playing();
        let queue_before = library.queue.as_ref().map(|queue| queue.current);
        for (message, action) in [
            (Message::Next, Action::Next),
            (Message::Previous, Action::Previous),
            (Message::ToggleShuffle, Action::Shuffle),
            (Message::CycleRepeat, Action::Repeat),
        ] {
            let _ = library.update(message, None);
            let commands = library.together_take();
            assert!(
                matches!(commands.as_slice(), [Command::Send(Outgoing::Command(a))] if *a == action),
                "{action:?}: {commands:?}"
            );
        }
        // Своё действие не выполнено.
        assert_eq!(
            library.queue.as_ref().map(|queue| queue.current),
            queue_before
        );
        assert!(!library.player.shuffle && library.player.repeat == Repeat::Off);
        // Пауза у ведомого — своё изменение с прежним ведущим.
        let _ = library.update(Message::TogglePlay, None);
        let sent = states(&mut library);
        assert_eq!(sent.len(), 1);
        assert_eq!((sent[0].owner.as_str(), sent[0].playing), (PEER, false));
        // Без связи кнопки работают как обычно.
        let mut alone = test_library();
        let _ = alone.update(Message::ToggleShuffle, None);
        assert!(alone.player.shuffle);
        assert!(alone.together_take().is_empty());
    }

    #[test]
    fn leader_applies_followers_pause_and_seek_but_not_old_states() {
        let mut library = leader_playing();
        let rev = library.together.rev.current();
        let mut pause = remote(rev + 1, PEER, ME, Some("1"), false);
        pause.position = Duration::from_secs(50);
        feed(&mut library, Event::State(Box::new(pause)));
        assert!(!library.player.playing);
        // Перемотка ведомого: расхождение больше 120 мс.
        assert!((library.player.position - 50.0).abs() < 0.5);
        assert!(library.together.leader());
        assert!(states(&mut library).is_empty(), "эха нет");
        // Старое состояние (меньшая ревизия) игнорируется.
        let old = remote(1, PEER, ME, Some("1"), true);
        feed(&mut library, Event::State(Box::new(old)));
        assert!(!library.player.playing);
        // Продолжение.
        let resume = remote(rev + 2, PEER, ME, Some("1"), true);
        feed(&mut library, Event::State(Box::new(resume)));
        assert!(library.player.playing);
    }

    #[test]
    fn small_difference_is_not_a_seek() {
        let mut library = leader_playing();
        let rev = library.together.rev.current();
        let seeks = library.player.seeks;
        let mut same = remote(rev + 1, PEER, ME, Some("1"), true);
        // Наша позиция 5 с, у собеседника 5,05 с: меньше порога.
        same.position = Duration::from_millis(5050);
        feed(&mut library, Event::State(Box::new(same)));
        assert_eq!(library.player.seeks, seeks);
    }

    #[test]
    fn follower_follows_new_tracks_with_delay_and_keeps_next() {
        let mut library = follower_playing();
        library.together.delay_ms = 200;
        let mut state = remote(5, PEER, PEER, Some("9"), true);
        state.position = Duration::from_secs(30);
        state.next = Some(track("10"));
        feed(&mut library, Event::State(Box::new(state)));
        assert_eq!(
            library.now_playing.as_ref().map(|t| t.id.as_str()),
            Some("9")
        );
        let queue = library.queue.as_ref().unwrap();
        assert_eq!(queue.key, ListKey::Remote);
        assert_eq!(queue.tracks.len(), 2);
        assert_eq!(queue.get(1).map(|t| t.id.as_str()), Some("10"));
        // На экране волны источник — устройство, а не «Очередь».
        assert_eq!(library.wave.source, "Слушаю с Телефон");
        // Позиция: 30 с, поправка +0,2 с, плюс ничтожное время теста.
        assert!(library.player.position >= 30.19 && library.player.position < 31.0);
        assert!(states(&mut library).is_empty());
    }

    #[test]
    fn follower_does_not_continue_on_its_own() {
        let mut library = follower_playing();
        let session = library.player.session;
        let current = library.queue.as_ref().map(|queue| queue.current);
        let effect = library
            .update(Message::Audio(AudioEvent::Ended { id: session }), None)
            .1;
        assert!(effect.is_none());
        assert!(!library.player.playing);
        assert_eq!(library.queue.as_ref().map(|queue| queue.current), current);
        assert!(
            states(&mut library).is_empty(),
            "конец трека ведущему не шлём"
        );
        // Ведущий начал заново — снова играем.
        let mut again = remote(6, PEER, PEER, Some("7"), true);
        again.position = Duration::ZERO;
        feed(&mut library, Event::State(Box::new(again)));
        assert!(library.together.follower());
    }

    #[test]
    fn follower_failure_is_reported_without_skipping() {
        let mut library = follower_playing();
        let session = library.player.session;
        let effect = library
            .update(
                Message::Audio(AudioEvent::Failed {
                    id: session,
                    error: PlayError::Unauthorized,
                }),
                None,
            )
            .1;
        // Вход больше не действует — это не про трек, обычный путь.
        assert!(matches!(effect, Some(Effect::Relogin)));
        let mut library = follower_playing();
        let session = library.player.session;
        let current = library.queue.as_ref().map(|queue| queue.current);
        let effect = library
            .update(
                Message::Audio(AudioEvent::Failed {
                    id: session,
                    error: PlayError::Network("обрыв".to_owned()),
                }),
                None,
            )
            .1;
        match effect {
            Some(Effect::Toast(text)) => {
                assert!(
                    text.contains("Не удалось включить «Трек 7» на этом устройстве"),
                    "{text}"
                );
            }
            _ => panic!("ожидалось сообщение"),
        }
        assert_eq!(library.queue.as_ref().map(|queue| queue.current), current);
        assert!(states(&mut library).is_empty());
    }

    #[test]
    fn tick_corrects_follower_after_two_in_a_row_only() {
        let mut library = follower_playing();
        // Прошло больше 1,5 с после запуска: подстройка разрешена.
        library.together.link.as_mut().unwrap().drift = DriftCorrector::new();
        let rev = library.together.link.as_ref().unwrap().state_rev;
        library.player.position = 10.0;
        library.player.position_at = Instant::now();
        let tick = |position_ms: u64, rev: u64| PlayTick {
            rev,
            position: Duration::from_millis(position_ms),
            at: Instant::now(),
            buffering: false,
        };
        let seeks = library.player.seeks;
        // Чужой `rev` игнорируется.
        for _ in 0..3 {
            feed(&mut library, Event::Tick(tick(20_000, rev + 5)));
        }
        assert_eq!(library.player.seeks, seeks);
        // Один выброс — ещё не перемотка.
        feed(&mut library, Event::Tick(tick(12_000, rev)));
        assert_eq!(library.player.seeks, seeks);
        // Второй подряд — перемотка на цель.
        feed(&mut library, Event::Tick(tick(12_000, rev)));
        assert_eq!(library.player.seeks, seeks + 1);
        assert!((library.player.position - 12.0).abs() < 0.5);
        // Перемотка по `tick` не уходит собеседнику как своя.
        assert!(states(&mut library).is_empty());
        // Ведущему `tick` не нужен.
        let mut leader = leader_playing();
        let seeks = leader.player.seeks;
        feed(&mut leader, Event::Tick(tick(90_000, 0)));
        feed(&mut leader, Event::Tick(tick(90_000, 0)));
        assert_eq!(leader.player.seeks, seeks);
    }

    #[test]
    fn first_tick_after_a_new_leader_track_corrects_immediately() {
        let mut library = follower_playing();
        let mut state = remote(7, PEER, PEER, Some("9"), true);
        state.position = Duration::from_secs(30);
        feed(&mut library, Event::State(Box::new(state)));
        let rev = library.together.link.as_ref().unwrap().state_rev;
        // Ведущий ушёл вперёд (его звук пошёл позже прикидки): первый же
        // `tick` без буферизации исправляет, без второго и без паузы 1,5 с.
        // (В тестах звука нет: трек «загружен» вручную.)
        library.player.loaded = true;
        library.player.playing = true;
        library.player.buffering = false;
        let seeks = library.player.seeks;
        let tick = PlayTick {
            rev,
            position: Duration::from_secs(33),
            at: Instant::now(),
            buffering: false,
        };
        feed(&mut library, Event::Tick(tick));
        assert_eq!(library.player.seeks, seeks + 1);
        assert!((library.player.position - 33.0).abs() < 0.5);
    }

    #[test]
    fn repeat_one_at_the_end_sends_the_same_track_from_zero() {
        let mut library = leader_playing();
        library.player.repeat = Repeat::One;
        let session = library.player.session;
        let rev = library.together.rev.current();
        let _ = library.update(Message::Audio(AudioEvent::Ended { id: session }), None);
        let sent = states(&mut library);
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert_eq!(sent[0].track.as_ref().map(|t| t.id.as_str()), Some("1"));
        assert_eq!(sent[0].position, Duration::ZERO);
        assert_eq!(sent[0].rev, rev + 1);
        assert_eq!(sent[0].owner, ME);
    }

    #[test]
    fn restored_link_starts_with_the_leader_from_before_the_break() {
        // Мы были ведомыми; связь оборвалась сама и восстановилась.
        let mut library = follower_playing();
        let lost = |lost: bool| Event::Disconnected {
            peer: peer_info(),
            text: "Связь с Телефон потеряна".to_owned(),
            lost,
        };
        feed(&mut library, lost(true));
        assert!(library.together.kept_owner.is_some());
        let _ = library.together_take();
        feed(
            &mut library,
            Event::Linked(LinkInfo {
                peer: peer_info(),
                role: Role::Initiator,
                join: false,
                restored: true,
            }),
        );
        let sent = states(&mut library);
        assert_eq!(sent.len(), 1);
        // По-прежнему ведущий — собеседник; `by` — мы.
        assert_eq!((sent[0].owner.as_str(), sent[0].by.as_str()), (PEER, ME));
        assert!(sent[0].initial && !sent[0].join);
        // `bye` ведущего не помнится.
        let mut library = follower_playing();
        feed(&mut library, lost(false));
        assert!(library.together.kept_owner.is_none());
        // Обычная новая связь — owner свой.
        let _ = library.together_take();
        feed(
            &mut library,
            Event::Linked(LinkInfo {
                peer: peer_info(),
                role: Role::Initiator,
                join: true,
                restored: false,
            }),
        );
        assert_eq!(states(&mut library)[0].owner, ME);
    }

    #[test]
    fn follower_sends_synchronous_position_without_own_delay() {
        let mut library = follower_playing();
        library.together.delay_ms = 200;
        library.player.position = 10.5;
        library.player.position_at = Instant::now();
        library.player.playing = false;
        let _ = library.update(Message::Noop, None);
        let sent = states(&mut library);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].owner, PEER);
        assert_eq!(sent[0].position, Duration::from_millis(10_300));
    }

    #[test]
    fn leader_ticks_only_while_playing_and_runs_commands() {
        let mut library = leader_playing();
        assert!(library.together_ticking());
        let _ = library.update(Message::Together(Msg::Tick), None);
        let commands = library.together_take();
        assert!(matches!(
            commands.as_slice(),
            [Command::Send(Outgoing::Tick(tick))] if !tick.buffering
        ));
        // Пауза — таймер не нужен.
        library.player.playing = false;
        assert!(!library.together_ticking());
        // Команда ведомого выполняется как своя кнопка: «Следующий».
        library.player.playing = true;
        library.queue.as_mut().unwrap().current = 0;
        let before = library.player.session;
        feed(&mut library, Event::Command(Action::Next));
        assert!(library.player.session > before, "запущен следующий трек");
        // Команда не ведущему игнорируется.
        let mut follower = follower_playing();
        let session = follower.player.session;
        feed(&mut follower, Event::Command(Action::Next));
        assert_eq!(follower.player.session, session);
    }

    #[test]
    fn disconnect_returns_to_own_player_and_follower_stops_after_the_track() {
        let mut library = follower_playing();
        assert_eq!(library.queue.as_ref().map(|q| q.tracks.len()), Some(2));
        let effect = feed(
            &mut library,
            Event::Disconnected {
                peer: peer_info(),
                text: "Связь с Телефон разорвана".to_owned(),
                lost: false,
            },
        );
        assert!(matches!(effect, Some(Effect::Toast(text)) if text.contains("разорвана")));
        assert!(!library.together.linked());
        // Очередь — только текущий трек: свою он не продолжит.
        assert_eq!(library.queue.as_ref().map(|q| q.tracks.len()), Some(1));
        // Состояния собеседника больше не применяются, кнопки снова свои.
        let _ = library.update(Message::ToggleShuffle, None);
        assert!(library.player.shuffle);
        assert!(library.together_take().is_empty());
    }

    #[test]
    fn likes_are_shared_without_extra_requests() {
        let mut library = leader_playing();
        let _ = library.update(Message::LikeDone(track("1"), true, Ok(())), None);
        let commands = library.together_take();
        assert!(matches!(
            commands.as_slice(),
            [Command::Send(Outgoing::Liked { track, liked: true })] if track == "1"
        ));
        // Пришло от собеседника: сердце обновилось, запроса нет.
        feed(
            &mut library,
            Event::Liked {
                track: "55".to_owned(),
                liked: true,
            },
        );
        assert!(library.liked.contains("55"));
        assert!(library.like_busy.is_empty());
        assert!(library.together_take().is_empty());
    }

    #[test]
    fn pairing_dialog_and_decisions() {
        let mut library = test_library();
        library.together_started(Some(ME.to_owned()));
        feed(
            &mut library,
            Event::Pairing {
                peer: peer_info(),
                code: "123 456".to_owned(),
                initiator: true,
            },
        );
        assert!(library.together_pairing_dialog().is_some());
        let _ = library.update(Message::Together(Msg::Decide(true)), None);
        assert!(matches!(
            library.together_take().as_slice(),
            [Command::Decide(true)]
        ));
        // После своего подтверждения ждём собеседника.
        assert!(library.together.pairing.as_ref().is_some_and(|p| p.decided));
        // «Отмена» после своего «да» обрывает подключение и закрывает диалог.
        let _ = library.update(Message::Together(Msg::Decide(false)), None);
        assert!(library.together_pairing_dialog().is_none());
        assert!(matches!(
            library.together_take().as_slice(),
            [Command::Disconnect]
        ));
        // «Отмена» до решения — отказ от кода.
        feed(
            &mut library,
            Event::Pairing {
                peer: peer_info(),
                code: "654 321".to_owned(),
                initiator: false,
            },
        );
        let _ = library.update(Message::Together(Msg::Decide(false)), None);
        assert!(library.together_pairing_dialog().is_none());
        assert!(matches!(
            library.together_take().as_slice(),
            [Command::Decide(false)]
        ));
    }

    #[test]
    fn paired_event_asks_the_window_to_remember_and_forget_goes_to_the_service() {
        let mut library = test_library();
        library.together_started(Some(ME.to_owned()));
        let effect = feed(
            &mut library,
            Event::Paired {
                peer: peer_info(),
                saved: true,
            },
        );
        assert!(matches!(effect, Some(Effect::TogetherPaired(d)) if d.id == PEER));
        assert_eq!(library.together.paired.len(), 1);
        let _ = library.update(Message::Together(Msg::Forget(PEER.to_owned())), None);
        assert!(library.together.paired.is_empty());
        assert!(matches!(
            library.together_take().as_slice(),
            [Command::Forget { id }] if id == PEER
        ));
    }

    #[test]
    fn address_input_is_validated() {
        let mut library = test_library();
        let _ = library.update(Message::Together(Msg::AddressInput("abc".to_owned())), None);
        let _ = library.update(Message::Together(Msg::ConnectAddr), None);
        assert!(library.together.address_error.is_some());
        assert!(library.together_take().is_empty());
        let _ = library.update(
            Message::Together(Msg::AddressInput("127.0.0.1:47800".to_owned())),
            None,
        );
        let _ = library.update(Message::Together(Msg::ConnectAddr), None);
        assert!(matches!(
            library.together_take().as_slice(),
            [Command::ConnectAddr(addr)] if addr.port() == 47800
        ));
        assert!(matches!(library.together.status, Status::Connecting(_)));
    }
}
