//! Готовая связь: читатель и писатель в отдельных задачах, пинги и оценка
//! часов (§4.2), живость, перевод моментов между часами окна и собеседника.

use std::time::Duration;

use tokio::time::{Instant as TokioInstant, sleep_until, timeout};

use super::*;
use crate::sync::clock::ClockEstimator;
use crate::sync::handshake::Established;
use crate::sync::proto::Message;

/// Сколько ждём отправку `bye` при закрытии связи.
const FLUSH_WAIT: Duration = Duration::from_millis(500);
/// Сколько дочитываем входящее после своего `bye`.
const LINGER_WAIT: Duration = Duration::from_millis(300);
/// Сколько пингов идёт частым темпом после готовности связи.
const BURST_PINGS: u64 = 5;

/// Чем кончилась связь.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LinkEnd {
    /// Пользователь нажал «Отключить» у нас.
    User,
    /// Закрываем службу.
    Shutdown,
    /// Собеседник прислал `bye`.
    PeerBye(ByeReason),
    /// Оборвалась сама: ошибка чтения или записи, молчание.
    Lost,
    /// Слот занят другой сессией.
    Replaced,
}

enum Internal {
    Frame(Option<Message>),
    Failed,
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn state_to_wire(shared: &Shared, state: &PlayState) -> State {
    State {
        rev: state.rev,
        by: state.by.clone(),
        owner: state.owner.clone(),
        track: state.track.as_ref().map(WireTrack::from_track),
        playing: state.playing,
        position_ms: millis(state.position),
        at: shared.ms(state.at),
        next: state.next.as_ref().map(WireTrack::from_track),
        shuffle: state.shuffle,
        repeat: state.repeat,
        wave: state.wave,
        initial: state.initial,
        join: state.join,
    }
}

fn state_from_wire(shared: &Shared, clock: &ClockEstimator, state: State) -> PlayState {
    PlayState {
        rev: state.rev,
        by: state.by,
        owner: state.owner,
        track: state.track.as_ref().map(WireTrack::to_track),
        playing: state.playing,
        position: Duration::from_millis(state.position_ms),
        at: shared.instant(clock.to_local(state.at)),
        next: state.next.as_ref().map(WireTrack::to_track),
        shuffle: state.shuffle,
        repeat: state.repeat,
        wave: state.wave,
        initial: state.initial,
        join: state.join,
    }
}

fn outgoing_to_wire(shared: &Shared, outgoing: &Outgoing) -> Message {
    match outgoing {
        Outgoing::State(state) => Message::State(Box::new(state_to_wire(shared, state))),
        Outgoing::Tick(tick) => Message::Tick(Tick {
            rev: tick.rev,
            position_ms: millis(tick.position),
            at: shared.ms(tick.at),
            buffering: tick.buffering,
        }),
        Outgoing::Command(action) => Message::Command { action: *action },
        Outgoing::Liked { track, liked } => Message::Liked {
            track: track.clone(),
            liked: *liked,
        },
    }
}

/// Сообщение собеседника → событие окну (в часах окна).
fn deliver(shared: &Shared, clock: &ClockEstimator, message: Message) -> Option<Event> {
    match message {
        Message::State(state) => Some(Event::State(Box::new(state_from_wire(
            shared, clock, *state,
        )))),
        Message::Tick(tick) => Some(Event::Tick(PlayTick {
            rev: tick.rev,
            position: Duration::from_millis(tick.position_ms),
            at: shared.instant(clock.to_local(tick.at)),
            buffering: tick.buffering,
        })),
        Message::Command { action } => Some(Event::Command(action)),
        Message::Liked { track, liked } => Some(Event::Liked { track, liked }),
        _ => None,
    }
}

/// Ведёт готовую связь до конца.
pub(super) async fn run(
    shared: &Arc<Shared>,
    est: Established<TcpStream>,
    parts: &mut SessionParts,
    info: LinkInfo,
) -> LinkEnd {
    let Established {
        peer,
        mut reader,
        mut writer,
        ..
    } = est;
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Message>();
    let (in_tx, mut in_rx) = mpsc::unbounded_channel::<Internal>();

    let reader_tx = in_tx.clone();
    let mut reader_task = tokio::spawn(async move {
        loop {
            match reader.recv().await {
                Ok(message) => {
                    if reader_tx.send(Internal::Frame(message)).is_err() {
                        break;
                    }
                }
                Err(_) => {
                    let _ = reader_tx.send(Internal::Failed);
                    break;
                }
            }
        }
    });
    let frame_limit = shared.cfg.timeouts.frame;
    let writer_task = tokio::spawn(async move {
        while let Some(message) = out_rx.recv().await {
            match timeout(frame_limit, writer.send(&message)).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) => {
                    let _ = in_tx.send(Internal::Failed);
                    break;
                }
                Err(_) => {
                    let _ = in_tx.send(Internal::Failed);
                    break;
                }
            }
        }
    });

    let mut clock = ClockEstimator::new();
    let mut announced = false;
    let mut pending: Vec<Message> = Vec::new();
    let mut pings: u64 = 0;
    let mut next_ping = TokioInstant::now() + shared.cfg.burst_interval;
    let mut last_rx = TokioInstant::now();

    let end = loop {
        tokio::select! {
            () = sleep_until(next_ping) => {
                let _ = out_tx.send(Message::Ping { n: pings, t0: shared.now_ms() });
                pings += 1;
                let pause = if pings < BURST_PINGS {
                    shared.cfg.burst_interval
                } else {
                    shared.cfg.ping_interval
                };
                next_ping = TokioInstant::now() + pause;
            }
            () = sleep_until(last_rx + shared.cfg.liveness) => break LinkEnd::Lost,
            internal = in_rx.recv() => match internal {
                Some(Internal::Frame(frame)) => {
                    last_rx = TokioInstant::now();
                    let Some(message) = frame else { continue };
                    match message {
                        Message::Ping { n, t0 } => {
                            let _ = out_tx.send(Message::Pong { n, t0, t1: shared.now_ms() });
                        }
                        Message::Pong { t0, t1, .. } => {
                            clock.record(t0, t1, shared.now_ms());
                            if !announced {
                                announced = true;
                                shared.emit(Event::Linked(info.clone()));
                                for held in pending.drain(..) {
                                    if let Some(event) = deliver(shared, &clock, held) {
                                        shared.emit(event);
                                    }
                                }
                            }
                        }
                        Message::Bye { reason } => break LinkEnd::PeerBye(reason),
                        Message::Auth { .. } | Message::Confirm => {}
                        other if announced => {
                            if let Some(event) = deliver(shared, &clock, other) {
                                shared.emit(event);
                            }
                        }
                        other => pending.push(other),
                    }
                }
                Some(Internal::Failed) | None => break LinkEnd::Lost,
            },
            command = parts.cmds.recv() => match command {
                Some(SessionCmd::Send(outgoing)) => {
                    let _ = out_tx.send(outgoing_to_wire(shared, &outgoing));
                }
                Some(SessionCmd::Disconnect) => break LinkEnd::User,
                Some(SessionCmd::Shutdown) => break LinkEnd::Shutdown,
                Some(SessionCmd::Decide(_)) => {}
                None => break LinkEnd::Replaced,
            },
            () = parts.cancel.wait() => break LinkEnd::Replaced,
        }
    };

    // Закрытие: `bye` дописываем и ждём отправку, остальное просто закрываем.
    let reason = match end {
        LinkEnd::User => Some(ByeReason::User),
        LinkEnd::Shutdown => Some(ByeReason::Shutdown),
        _ => None,
    };
    match reason {
        Some(reason) => {
            let _ = out_tx.send(Message::Bye { reason });
            drop(out_tx);
            let _ = timeout(FLUSH_WAIT, writer_task).await;
            // Мягкое закрытие: читатель дочитывает входящее до закрытия
            // собеседником, иначе сброс (RST) съест наш `bye`.
            let _ = timeout(LINGER_WAIT, &mut reader_task).await;
            reader_task.abort();
        }
        None => {
            drop(out_tx);
            reader_task.abort();
            writer_task.abort();
        }
    }

    if !shared.stopping.load(Ordering::SeqCst) && end != LinkEnd::Replaced {
        if announced {
            let lost = end == LinkEnd::Lost;
            let how = if lost {
                "потеряна"
            } else {
                "разорвана"
            };
            shared.emit(Event::Disconnected {
                text: format!("Связь с {} {how}", peer.name),
                peer,
                lost,
            });
        } else {
            shared.emit(Event::Failed {
                text: format!("Связь с {} прервалась при подключении", peer.name),
            });
        }
    }
    end
}
