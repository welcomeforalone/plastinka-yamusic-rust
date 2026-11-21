//! Две службы в одном процессе через 127.0.0.1, без mDNS (брандмауэр
//! Windows от таких сокетов не спрашивает). Везде таймауты: тесты не виснут.

use std::pin::Pin;

use iced::futures::StreamExt;
use keyring_core::CredentialStore;

use super::*;
use crate::sync::discovery::Visible;
use crate::sync::handshake::{commit_of, hello_initiator_frame};
use crate::sync::wire::write_frame;

const ID_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const ID_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const ID_C: &str = "cccccccccccccccccccccccccccccccc";
const WAIT: Duration = Duration::from_secs(8);

fn config(id: &str, port: u16, paired: Vec<PairedDevice>) -> ServiceConfig {
    let mut cfg = ServiceConfig::new(
        id.to_owned(),
        format!("Устройство {}", id.chars().next().unwrap_or('x')),
        paired,
    );
    cfg.bind = IpAddr::V4(Ipv4Addr::LOCALHOST);
    cfg.port = port;
    cfg.mdns = false;
    cfg.timeouts = Timeouts {
        frame: Duration::from_secs(2),
        pairing: Duration::from_secs(5),
    };
    cfg.connect_timeout = Duration::from_secs(1);
    cfg.burst_interval = Duration::from_millis(20);
    cfg.ping_interval = Duration::from_millis(50);
    cfg.liveness = Duration::from_millis(600);
    cfg.restore_interval = Duration::from_millis(100);
    cfg.restore_window = Duration::from_secs(5);
    cfg
}

fn mock_store() -> (SecretStore, Arc<CredentialStore>) {
    let raw: Arc<CredentialStore> = keyring_core::mock::Store::new().unwrap();
    (
        SecretStore::with_store(raw.clone(), "plastinka-sync-svc-test"),
        raw,
    )
}

struct Peer {
    svc: Service,
    events: Pin<Box<dyn Stream<Item = Event>>>,
    addr: SocketAddr,
}

impl Peer {
    async fn start(cfg: ServiceConfig, store: SecretStore) -> Self {
        let svc = Service::start(42, cfg, store).unwrap();
        let mut events: Pin<Box<dyn Stream<Item = Event>>> = Box::pin(svc.events().stream());
        let addresses = wait_for(&mut events, |event| match event {
            Event::Addresses(list) => list.first().cloned(),
            _ => None,
        })
        .await;
        Self {
            svc,
            events,
            addr: addresses.parse().unwrap(),
        }
    }

    /// Ждёт событие, подходящее под `pick`; лишние отбрасывает.
    async fn wait<T>(&mut self, pick: impl Fn(&Event) -> Option<T>) -> T {
        wait_for(&mut self.events, pick).await
    }

    /// Собирает события до подходящего (включительно).
    async fn until(&mut self, pick: impl Fn(&Event) -> bool) -> Vec<Event> {
        let mut seen = Vec::new();
        let result = tokio::time::timeout(WAIT, async {
            while let Some(event) = self.events.next().await {
                let done = pick(&event);
                seen.push(event);
                if done {
                    break;
                }
            }
        })
        .await;
        assert!(result.is_ok(), "ожидание события истекло, было: {seen:?}");
        seen
    }

    fn show_device(&self, other: &Peer, id: &str) {
        lock(&self.svc.shared.devices).push(Visible {
            id: id.to_owned(),
            name: "Другой".to_owned(),
            os: "linux".to_owned(),
            addrs: vec![other.addr],
            fullname: format!("{id}._plastinka._tcp.local."),
        });
    }

    async fn pair(&mut self, other: &mut Peer) {
        self.svc.send(Command::ConnectAddr(other.addr));
        let mine = self
            .wait(|e| match e {
                Event::Pairing { code, .. } => Some(code.clone()),
                _ => None,
            })
            .await;
        let theirs = other
            .wait(|e| match e {
                Event::Pairing { code, .. } => Some(code.clone()),
                _ => None,
            })
            .await;
        assert_eq!(mine, theirs);
        self.svc.send(Command::Decide(true));
        other.svc.send(Command::Decide(true));
        self.wait(|e| matches!(e, Event::Linked(_)).then_some(()))
            .await;
        other
            .wait(|e| matches!(e, Event::Linked(_)).then_some(()))
            .await;
    }
}

async fn wait_for<T>(
    events: &mut Pin<Box<dyn Stream<Item = Event>>>,
    pick: impl Fn(&Event) -> Option<T>,
) -> T {
    let found = tokio::time::timeout(WAIT, async {
        while let Some(event) = events.next().await {
            if let Some(value) = pick(&event) {
                return Some(value);
            }
        }
        None
    })
    .await;
    found.unwrap().expect("поток событий закрылся")
}

fn play_state(by: &str, position_ms: u64) -> PlayState {
    PlayState {
        rev: 7,
        by: by.to_owned(),
        owner: by.to_owned(),
        track: Some(Track {
            id: "123".into(),
            title: Some("Название".into()),
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
        }),
        playing: true,
        position: Duration::from_millis(position_ms),
        at: Instant::now(),
        next: None,
        shuffle: false,
        repeat: Repeat::Off,
        wave: false,
        initial: false,
        join: false,
    }
}

#[tokio::test]
async fn pairing_then_recognition_and_messages() {
    let (store_a, _) = mock_store();
    let (store_b, _) = mock_store();
    let mut a = Peer::start(config(ID_A, 0, vec![]), store_a).await;
    let mut b = Peer::start(config(ID_B, 0, vec![]), store_b).await;

    a.svc.send(Command::ConnectAddr(b.addr));
    let (peer, initiator) = a
        .wait(|e| match e {
            Event::Pairing {
                peer, initiator, ..
            } => Some((peer.clone(), *initiator)),
            _ => None,
        })
        .await;
    assert_eq!(peer.id, ID_B);
    assert!(initiator);
    let responder_side = b
        .wait(|e| match e {
            Event::Pairing {
                peer, initiator, ..
            } => Some((peer.id.clone(), *initiator)),
            _ => None,
        })
        .await;
    assert_eq!(responder_side, (ID_A.to_owned(), false));
    a.svc.send(Command::Decide(true));
    b.svc.send(Command::Decide(true));

    let linked_a = a
        .wait(|e| match e {
            Event::Linked(info) => Some(info.clone()),
            _ => None,
        })
        .await;
    let linked_b = b
        .wait(|e| match e {
            Event::Linked(info) => Some(info.clone()),
            _ => None,
        })
        .await;
    assert_eq!(
        (linked_a.role, linked_a.join, linked_a.restored),
        (Role::Initiator, true, false)
    );
    assert_eq!(
        (linked_b.role, linked_b.join, linked_b.restored),
        (Role::Responder, false, false)
    );
    assert_eq!(linked_a.peer.id, ID_B);

    // Обмен: `state` с переводом времени и `tick`, `command`, `liked`.
    let sent = play_state(ID_A, 61_000);
    a.svc
        .send(Command::Send(Outgoing::State(Box::new(sent.clone()))));
    let got = b
        .wait(|e| match e {
            Event::State(state) => Some(state.clone()),
            _ => None,
        })
        .await;
    assert_eq!(got.rev, 7);
    assert_eq!(got.position, Duration::from_millis(61_000));
    assert_eq!(got.track, sent.track);
    let skew = if got.at > sent.at {
        got.at - sent.at
    } else {
        sent.at - got.at
    };
    assert!(skew < Duration::from_millis(200), "смещение {skew:?}");

    let tick = PlayTick {
        rev: 7,
        position: Duration::from_millis(62_000),
        at: Instant::now(),
        buffering: true,
    };
    b.svc.send(Command::Send(Outgoing::Tick(tick)));
    let got = a
        .wait(|e| match e {
            Event::Tick(tick) => Some(*tick),
            _ => None,
        })
        .await;
    assert!(got.buffering && got.position == tick.position);
    b.svc.send(Command::Send(Outgoing::Command(Action::Next)));
    a.wait(|e| matches!(e, Event::Command(Action::Next)).then_some(()))
        .await;
    a.svc.send(Command::Send(Outgoing::Liked {
        track: "123".into(),
        liked: true,
    }));
    b.wait(|e| matches!(e, Event::Liked { liked: true, .. }).then_some(()))
        .await;

    // `bye user`: у обеих сторон разрыв не «сам».
    a.svc.send(Command::Disconnect);
    for peer in [&mut a, &mut b] {
        let lost = peer
            .wait(|e| match e {
                Event::Disconnected { lost, .. } => Some(*lost),
                _ => None,
            })
            .await;
        assert!(!lost);
    }

    // Второе подключение: секрет уже есть у обеих сторон — узнавание без кода.
    a.show_device(&b, ID_B);
    a.svc.send(Command::Connect {
        id: ID_B.to_owned(),
    });
    let seen = a.until(|e| matches!(e, Event::Linked(_))).await;
    assert!(
        !seen.iter().any(|e| matches!(e, Event::Pairing { .. })),
        "{seen:?}"
    );
    let seen = b.until(|e| matches!(e, Event::Linked(_))).await;
    assert!(
        !seen.iter().any(|e| matches!(e, Event::Pairing { .. })),
        "{seen:?}"
    );
}

#[tokio::test]
async fn rejection_by_one_side() {
    let (store_a, _) = mock_store();
    let (store_b, _) = mock_store();
    let mut a = Peer::start(config(ID_A, 0, vec![]), store_a).await;
    let mut b = Peer::start(config(ID_B, 0, vec![]), store_b).await;
    a.svc.send(Command::ConnectAddr(b.addr));
    a.wait(|e| matches!(e, Event::Pairing { .. }).then_some(()))
        .await;
    b.wait(|e| matches!(e, Event::Pairing { .. }).then_some(()))
        .await;
    a.svc.send(Command::Decide(false));
    b.svc.send(Command::Decide(true));
    let text = a
        .wait(|e| match e {
            Event::Failed { text } => Some(text.clone()),
            _ => None,
        })
        .await;
    assert_eq!(text, "Сопряжение отменено");
    let text = b
        .wait(|e| match e {
            Event::Failed { text } => Some(text.clone()),
            _ => None,
        })
        .await;
    assert_eq!(text, "Сопряжение отменено");
    // Занятость снята.
    assert!(
        !a.svc.shared.is_busy()
            || tokio::time::timeout(WAIT, async {
                while a.svc.shared.is_busy() {
                    sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn third_device_gets_busy() {
    let (store_a, _) = mock_store();
    let (store_b, _) = mock_store();
    let (store_c, _) = mock_store();
    let mut a = Peer::start(config(ID_A, 0, vec![]), store_a).await;
    let mut b = Peer::start(config(ID_B, 0, vec![]), store_b).await;
    let mut c = Peer::start(config(ID_C, 0, vec![]), store_c).await;
    a.pair(&mut b).await;
    c.svc.send(Command::ConnectAddr(b.addr));
    let text = c
        .wait(|e| match e {
            Event::Failed { text } => Some(text.clone()),
            _ => None,
        })
        .await;
    assert!(text.contains("уже связано с другим устройством"), "{text}");
}

#[tokio::test]
async fn failed_handshake_releases_busy_guard() {
    let (store_b, _) = mock_store();
    let b = Peer::start(config(ID_B, 0, vec![]), store_b).await;
    // Сырое соединение: верный `hello`, потом обрыв до `reveal`.
    let mut raw = TcpStream::connect(b.addr).await.unwrap();
    let me = Local {
        id: ID_A.to_owned(),
        name: "Сырой".to_owned(),
        os: "linux".to_owned(),
        acc: account_tag(42),
    };
    let hello = hello_initiator_frame(&me, false, &commit_of(&[4; 65], &[1; 16])).unwrap();
    write_frame(&mut raw, &hello).await.unwrap();
    let deadline = Instant::now() + WAIT;
    while !b.svc.shared.is_busy() && Instant::now() < deadline {
        sleep(Duration::from_millis(5)).await;
    }
    assert!(b.svc.shared.is_busy(), "отметка должна быть взята");
    drop(raw);
    while b.svc.shared.is_busy() && Instant::now() < deadline {
        sleep(Duration::from_millis(5)).await;
    }
    assert!(
        !b.svc.shared.is_busy(),
        "отметка должна сняться после обрыва"
    );
}

#[tokio::test]
async fn crash_is_lost_and_restore_reconnects_without_code() {
    let (store_a, _) = mock_store();
    let (store_b, raw_b) = mock_store();
    let mut a = Peer::start(config(ID_A, 0, vec![]), store_a).await;
    let mut b = Peer::start(config(ID_B, 0, vec![]), store_b).await;
    a.pair(&mut b).await;
    let port = b.addr.port();
    // B «падает» без `bye`: рантайм останавливается, сокеты закрываются.
    let Peer { svc, .. } = b;
    drop_without_bye(svc);
    let lost = a
        .wait(|e| match e {
            Event::Disconnected { lost, .. } => Some(*lost),
            _ => None,
        })
        .await;
    assert!(lost, "обрыв должен считаться «сам»");

    // B возвращается (те же id, порт и секрет) — A восстанавливает связь.
    let paired = vec![PairedDevice {
        id: ID_A.to_owned(),
        name: "A".to_owned(),
        os: "linux".to_owned(),
    }];
    let store_b2 = SecretStore::with_store(raw_b, "plastinka-sync-svc-test");
    let mut b2 = Peer::start(config(ID_B, port, paired), store_b2).await;
    let seen = a.until(|e| matches!(e, Event::Linked(_))).await;
    assert!(
        !seen.iter().any(|e| matches!(e, Event::Pairing { .. })),
        "{seen:?}"
    );
    let linked = seen.iter().find_map(|e| match e {
        Event::Linked(info) => Some(info.clone()),
        _ => None,
    });
    assert!(linked.is_some_and(|info| info.restored && !info.join));
    b2.wait(|e| matches!(e, Event::Linked(_)).then_some(()))
        .await;
}

/// Остановка службы без `bye` (как при вылете программы).
fn drop_without_bye(mut svc: Service) {
    svc.shared.stopping.store(true, Ordering::SeqCst);
    if let Some(runtime) = svc.runtime.take() {
        runtime.shutdown_background();
    }
    // Drop ниже увидит `stopping`, но `bye` уже некому отправить.
    std::mem::drop(svc);
}

#[tokio::test]
async fn gives_up_restoring_after_window() {
    let (store_a, _) = mock_store();
    let (store_b, _) = mock_store();
    let mut cfg_a = config(ID_A, 0, vec![]);
    cfg_a.restore_window = Duration::from_millis(500);
    let mut a = Peer::start(cfg_a, store_a).await;
    let mut b = Peer::start(config(ID_B, 0, vec![]), store_b).await;
    a.pair(&mut b).await;
    let Peer { svc, .. } = b;
    drop_without_bye(svc);
    let text = a
        .wait(|e| match e {
            Event::Failed { text } => Some(text.clone()),
            _ => None,
        })
        .await;
    assert!(text.contains("восстановить связь"), "{text}");
}

#[test]
fn crossing_connections_keep_smaller_initiator() {
    // Мы «b», встречное входящее от «a»: инициатор «a» меньше — входящее.
    assert!(incoming_wins_crossing("b", "a"));
    // Мы «a», входящее от «b»: остаётся своё исходящее.
    assert!(!incoming_wins_crossing("a", "b"));
    // Обе стороны приходят к одному выводу.
    for (x, y) in [("a", "b"), ("m", "z"), ("0", "f")] {
        assert_ne!(incoming_wins_crossing(x, y), incoming_wins_crossing(y, x));
    }
}

#[tokio::test]
async fn claim_follows_crossing_rule() {
    let (store, _) = mock_store();
    // Мы «b»: встречное входящее от «a» вытесняет нашу исходящую попытку.
    let svc = Service::start(42, config(ID_B, 0, vec![]), store).unwrap();
    let shared = svc.shared.clone();
    let (_claim_out, parts_out) = shared.claim(Some(ID_A), true).unwrap();
    assert!(
        matches!(shared.claim(Some(ID_C), false), Err(ClaimDenied::Busy)),
        "чужой — занято"
    );
    let (claim_in, _parts_in) = shared.claim(Some(ID_A), false).unwrap();
    parts_out.cancel.wait().await;
    // Старая отметка холостая: занятость держит новая.
    drop(_claim_out);
    assert!(shared.is_busy());
    drop(claim_in);
    assert!(!shared.is_busy());

    // Мы «a»: входящее от «b» отклоняется, своя попытка остаётся.
    let (store, _) = mock_store();
    let svc2 = Service::start(42, config(ID_A, 0, vec![]), store).unwrap();
    let (_claim, _parts) = svc2.shared.claim(Some(ID_B), true).unwrap();
    // Входящее уступает молча (без `bye busy`), не «занято».
    assert!(matches!(
        svc2.shared.claim(Some(ID_B), false),
        Err(ClaimDenied::Crossing)
    ));
}

#[test]
fn failure_texts_follow_spec() {
    let text = |err: SyncError| failure_text(&err, "Телефон");
    assert_eq!(
        text(SyncError::PeerBye(ByeReason::Busy)),
        "Телефон уже связано с другим устройством"
    );
    assert_eq!(
        text(SyncError::PeerBye(ByeReason::Version)),
        "Обновите Plastinka на Телефон"
    );
    assert_eq!(
        text(SyncError::PeerBye(ByeReason::Account)),
        "На Телефон другой аккаунт Яндекса"
    );
    assert_eq!(
        text(SyncError::Declined(ByeReason::Rejected)),
        "Сопряжение отменено"
    );
    assert!(text(SyncError::AuthFailed).contains("Подтвердите код заново"));
    assert!(is_fatal(&SyncError::AuthFailed));
    assert!(!is_fatal(&SyncError::PeerBye(ByeReason::Busy)));
    assert!(!is_fatal(&SyncError::Timeout));
}

#[tokio::test]
async fn clock_conversion_roundtrips() {
    let (store, _) = mock_store();
    let svc = Service::start(42, config(ID_A, 0, vec![]), store).unwrap();
    let now = Instant::now();
    let back = svc.shared.instant(svc.shared.ms(now));
    let diff = if back > now { back - now } else { now - back };
    assert!(diff < Duration::from_millis(2));
    let before = svc.shared.epoch.checked_sub(Duration::from_millis(500));
    if let Some(before) = before {
        assert_eq!(svc.shared.ms(before), -500);
        assert_eq!(svc.shared.instant(-500), before);
    }
}

/// Ждёт событие `pick` до `deadline`; на провале печатает, что ждали и что
/// пришло.
async fn interop_wait<T>(
    events: &mut Pin<Box<dyn Stream<Item = Event>>>,
    deadline: tokio::time::Instant,
    expected: &str,
    pick: impl Fn(&Event) -> Option<T>,
) -> T {
    let mut seen = Vec::new();
    let found = tokio::time::timeout_at(deadline, async {
        while let Some(event) = events.next().await {
            if let Some(value) = pick(&event) {
                return Some(value);
            }
            seen.push(format!("{event:?}"));
        }
        None
    })
    .await;
    match found {
        Ok(Some(value)) => value,
        other => panic!(
            "INTEROP: ПРОВАЛ. Ждали: {expected}. Пришло: {seen:?}. Исход: {}",
            if other.is_err() {
                "таймаут"
            } else {
                "поток закрыт"
            }
        ),
    }
}

/// Ответная сторона для живой сверки с Android по 127.0.0.1. Запуск:
/// `cargo test interop_responder -- --ignored --nocapture`. Порт —
/// `PLASTINKA_INTEROP_PORT` (47999 по умолчанию).
#[tokio::test]
#[ignore = "ручная сверка с Android"]
async fn interop_responder() {
    let port: u16 = std::env::var("PLASTINKA_INTEROP_PORT")
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(47999);
    let total = tokio::time::Instant::now() + Duration::from_secs(120);
    let mut cfg = ServiceConfig::new(ID_B.to_owned(), "Interop Desktop".to_owned(), vec![]);
    cfg.bind = IpAddr::V4(Ipv4Addr::LOCALHOST);
    cfg.port = port;
    cfg.mdns = false;
    let (store, _) = mock_store();
    let svc = Service::start(42, cfg, store).unwrap();
    let mut events: Pin<Box<dyn Stream<Item = Event>>> = Box::pin(svc.events().stream());
    let addresses = interop_wait(&mut events, total, "Addresses", |e| match e {
        Event::Addresses(list) => list.first().cloned(),
        _ => None,
    })
    .await;
    println!("INTEROP: слушаю {addresses}, жду подключение до 90 с");

    let connect_by = (tokio::time::Instant::now() + Duration::from_secs(90)).min(total);
    let code = interop_wait(
        &mut events,
        connect_by,
        "Pairing (входящее подключение)",
        |e| match e {
            Event::Pairing { code, .. } => Some(code.clone()),
            _ => None,
        },
    )
    .await;
    println!("INTEROP: код сопряжения {code}, подтверждаю");
    svc.send(Command::Decide(true));
    let link = interop_wait(&mut events, total, "Linked", |e| match e {
        Event::Linked(info) => Some(info.clone()),
        _ => None,
    })
    .await;
    println!(
        "INTEROP: связь готова: {} ({}), роль {:?}, join {}",
        link.peer.name, link.peer.os, link.role, link.join
    );

    // Начальный state: трек из задания.
    let mut state = play_state(ID_B, 10_000);
    state.rev = 0;
    state.initial = true;
    state.join = false;
    state.track = Some(Track {
        id: "12345".into(),
        title: Some("Interop".into()),
        version: None,
        artists: vec![crate::api::ArtistRef {
            id: Some("9".into()),
            name: "Тест".into(),
        }],
        album_id: Some("678".into()),
        album_title: None,
        duration: Some(Duration::from_millis(200_000)),
        available: true,
        cover_uri: None,
        warning: crate::api::Warning::None,
        loudness: None,
        lyrics: None,
    });
    state.at = Instant::now();
    svc.send(Command::Send(Outgoing::State(Box::new(state))));
    println!("INTEROP: отправлен начальный state (трек 12345, 10 с)");

    let theirs = interop_wait(
        &mut events,
        total,
        "начальный state Android",
        |e| match e {
            Event::State(state) if state.initial => Some(state.clone()),
            _ => None,
        },
    )
    .await;
    assert!(
        theirs.track.is_none() && theirs.join,
        "INTEROP: ПРОВАЛ. Ждали начальный state без трека и join=true, пришло: {theirs:?}"
    );
    println!("INTEROP: начальный state Android получен (без трека, join=true)");

    interop_wait(&mut events, total, "liked {12345, true}", |e| match e {
        Event::Liked { track, liked } if track == "12345" && *liked => Some(()),
        _ => None,
    })
    .await;
    println!("INTEROP: liked 12345 получен");
    interop_wait(&mut events, total, "command next", |e| {
        matches!(e, Event::Command(Action::Next)).then_some(())
    })
    .await;
    println!("INTEROP: command next получен");
    let (text, lost) = interop_wait(&mut events, total, "разрыв bye user", |e| match e {
        Event::Disconnected { text, lost, .. } => Some((text.clone(), *lost)),
        _ => None,
    })
    .await;
    assert!(
        !lost,
        "INTEROP: ПРОВАЛ. Ждали разрыв не «сам» (bye user), пришло: {text} (lost)"
    );
    println!("INTEROP: разрыв: {text} (lost: false) — ВСЁ ПРОШЛО");
}
