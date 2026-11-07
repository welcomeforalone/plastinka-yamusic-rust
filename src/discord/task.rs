//! Фоновая задача Discord RPC: владеет соединением, подключается заново,
//! обновляет и очищает статус. Окно только кладёт состояние в `watch` и
//! ответов от задачи не ждёт и не получает.

use std::time::Duration;

use tokio::io::{ReadHalf, WriteHalf};
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

use super::ipc::{self, Duplex, OP_CLOSE, OP_FRAME, OP_HANDSHAKE, OP_PING, OP_PONG};
use super::{Activity, START_SHIFT_MS, Settings, State, activity, should_send};

/// Через сколько пробовать подключиться снова, если Discord не запущен или
/// соединение оборвалось.
const RECONNECT: Duration = Duration::from_secs(20);
/// Не чаще раза в секунду обновляем статус.
const MIN_INTERVAL: Duration = Duration::from_secs(1);
/// Сколько ждём ответ Discord на рукопожатие.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
/// Сколько даём задаче очистить статус при выходе.
const SHUTDOWN: Duration = Duration::from_millis(400);

/// Куда уходят кадры: в Discord или (в отладке) в stderr.
enum Out {
    Pipe(WriteHalf<Box<dyn Duplex>>),
    #[cfg(debug_assertions)]
    Log,
}

impl Out {
    async fn send(&mut self, op: u32, body: &[u8]) -> bool {
        match self {
            Self::Pipe(writer) => ipc::write_frame(writer, op, body).await,
            #[cfg(debug_assertions)]
            Self::Log => {
                use std::io::Write as _;
                let _ = writeln!(
                    std::io::stderr(),
                    "discord op={op} {}",
                    String::from_utf8_lossy(body)
                );
                true
            }
        }
    }
}

/// Хозяин фоновой задачи. Пока он жив, статус обновляется; когда его
/// бросают (выход, смена экрана), статус очищается.
pub struct Rpc {
    tx: Option<watch::Sender<State>>,
    runtime: Option<tokio::runtime::Runtime>,
    /// Задача сообщает, что закончила (после очистки статуса).
    done: std::sync::mpsc::Receiver<()>,
}

impl Rpc {
    /// Запускает задачу для приложения `app_id`. `None` — ID пуст или
    /// рантайм не запустился: RPC молча отключён.
    pub fn start(app_id: &str) -> Option<Self> {
        #[cfg(debug_assertions)]
        let log = std::env::var("PLASTINKA_TEST_DISCORD").is_ok_and(|mode| mode == "log");
        #[cfg(not(debug_assertions))]
        let log = false;
        if app_id.is_empty() && !log {
            return None;
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("plastinka-discord")
            .enable_all()
            .build()
            .ok()?;
        // Пока окно не прислало настройки, статус выключен.
        let idle = State {
            settings: Settings {
                enabled: false,
                ..Settings::default()
            },
            ..State::default()
        };
        let (tx, rx) = watch::channel(idle);
        let (done_tx, done) = std::sync::mpsc::channel();
        let app_id = app_id.to_owned();
        runtime.spawn(async move {
            run(app_id, rx, log).await;
            let _ = done_tx.send(());
        });
        Some(Self {
            tx: Some(tx),
            runtime: Some(runtime),
            done,
        })
    }

    /// Новое состояние. Сообщение уходит, только если что-то изменилось
    /// по сути: ход времени задачу не будит.
    pub fn set(&self, state: State) {
        if let Some(tx) = &self.tx {
            tx.send_if_modified(|current| {
                if same(current, &state) {
                    false
                } else {
                    *current = state;
                    true
                }
            });
        }
    }
}

/// Состояния без существенной разницы: то же содержимое, начало трека
/// сдвинулось меньше чем на полпорога перемотки (или трек на паузе).
fn same(a: &State, b: &State) -> bool {
    a.settings == b.settings
        && a.track == b.track
        && a.playing == b.playing
        && (!a.playing || (a.start_ms - b.start_ms).abs() <= START_SHIFT_MS / 2)
}

impl Drop for Rpc {
    fn drop(&mut self) {
        // Закрытый канал — сигнал задаче очистить статус и выйти.
        self.tx.take();
        // Ждём очистку статуса, но недолго: выход не должен зависать.
        let _ = self.done.recv_timeout(SHUTDOWN);
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

/// Ждёт `duration`; `true`, если канал закрыли (пора выходить).
async fn wait_or_closed(rx: &mut watch::Receiver<State>, duration: Duration) -> bool {
    let deadline = Instant::now() + duration;
    loop {
        tokio::select! {
            () = tokio::time::sleep_until(deadline) => return false,
            changed = rx.changed() => {
                if changed.is_err() {
                    return true;
                }
            }
        }
    }
}

async fn run(app_id: String, mut rx: watch::Receiver<State>, log: bool) {
    loop {
        // Выключено в настройках — не подключаемся.
        let enabled = rx.borrow().settings.enabled;
        if !enabled {
            if rx.changed().await.is_err() {
                return;
            }
            continue;
        }
        let ended = if log {
            #[cfg(debug_assertions)]
            {
                session(Out::Log, None, &mut rx).await
            }
            #[cfg(not(debug_assertions))]
            {
                true
            }
        } else {
            match connect(&app_id).await {
                Some((out, frames)) => session(out, Some(frames), &mut rx).await,
                None => false,
            }
        };
        // `true` — приложение закрылось (канал закрыт).
        if ended {
            return;
        }
        // Выключили — ждём включения в начале цикла, а не таймер.
        let enabled = rx.borrow().settings.enabled;
        if enabled && wait_or_closed(&mut rx, RECONNECT).await {
            return;
        }
    }
}

/// Подключается и проходит рукопожатие. Читатель кадров — отдельная задача:
/// чтение кадра не отменяемо, а в основном цикле мы ждём несколько событий.
async fn connect(app_id: &str) -> Option<(Out, mpsc::UnboundedReceiver<(u32, Vec<u8>)>)> {
    let stream = ipc::connect().await?;
    let (reader, mut writer): (ReadHalf<Box<dyn Duplex>>, _) = tokio::io::split(stream);
    let hello = serde_json::json!({ "v": 1, "client_id": app_id }).to_string();
    if !ipc::write_frame(&mut writer, OP_HANDSHAKE, hello.as_bytes()).await {
        return None;
    }
    let (frames_tx, mut frames_rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut reader = reader;
        while let Some(frame) = ipc::read_frame(&mut reader).await {
            if frames_tx.send(frame).is_err() {
                break;
            }
        }
    });
    // Ждём READY.
    let ready = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
        while let Some((op, body)) = frames_rx.recv().await {
            if op == OP_CLOSE {
                return false;
            }
            if op == OP_FRAME && is_event(&body, "READY") {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false);
    ready.then_some((Out::Pipe(writer), frames_rx))
}

/// Это кадр `DISPATCH` с событием `evt`.
fn is_event(body: &[u8], evt: &str) -> bool {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .is_some_and(|value| value.get("evt").and_then(serde_json::Value::as_str) == Some(evt))
}

/// Тело `SET_ACTIVITY`; `None` вместо статуса — очистка.
fn set_activity_body(activity: Option<&Activity>) -> Vec<u8> {
    serde_json::json!({
        "cmd": "SET_ACTIVITY",
        "args": {
            "pid": std::process::id(),
            "activity": activity.map(Activity::to_json),
        },
        "nonce": uuid::Uuid::new_v4().to_string(),
    })
    .to_string()
    .into_bytes()
}

/// Работа на готовом соединении. `true` — канал окна закрыт (выход),
/// `false` — соединение потеряно, надо подключаться заново.
async fn session(
    mut out: Out,
    frames: Option<mpsc::UnboundedReceiver<(u32, Vec<u8>)>>,
    rx: &mut watch::Receiver<State>,
) -> bool {
    // Без читателя (отладочный вывод в stderr) держим пустой приёмник.
    let (_keep, mut frames) = match frames {
        Some(frames) => (None, frames),
        None => {
            let (tx, frames) = mpsc::unbounded_channel();
            (Some(tx), frames)
        }
    };
    // То, что Discord показывает сейчас (после подключения — ничего).
    let mut last: Option<Activity> = None;
    let mut last_sent: Option<Instant> = None;
    loop {
        let state = rx.borrow_and_update().clone();
        let wanted = activity(&state);
        let mut wait = None;
        if !state.settings.enabled {
            // Выключили: очищаем и выходим из сеанса, подключимся позже.
            if last.is_some() {
                out.send(OP_FRAME, &set_activity_body(None)).await;
            }
            return false;
        }
        if should_send(&last, &wanted) {
            let due = last_sent.map_or(Duration::ZERO, |at| {
                MIN_INTERVAL.saturating_sub(at.elapsed())
            });
            if due.is_zero() {
                if !out
                    .send(OP_FRAME, &set_activity_body(wanted.as_ref()))
                    .await
                {
                    return false;
                }
                // Ответ с ошибкой статус не повторяет: он считается отправленным.
                last = wanted;
                last_sent = Some(Instant::now());
                continue;
            }
            wait = Some(due);
        }
        tokio::select! {
            changed = rx.changed() => {
                if changed.is_err() {
                    // Окно закрылось: убираем статус.
                    if last.is_some() {
                        out.send(OP_FRAME, &set_activity_body(None)).await;
                    }
                    return true;
                }
            }
            frame = frames.recv() => match frame {
                None => return false,
                Some((OP_PING, body)) => {
                    if !out.send(OP_PONG, &body).await {
                        return false;
                    }
                }
                Some((OP_CLOSE, _)) => return false,
                // Ответы на SET_ACTIVITY (в том числе ошибки) не нужны.
                Some(_) => {}
            },
            () = async { tokio::time::sleep(wait.unwrap_or_default()).await }, if wait.is_some() => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discord::Track;

    fn state(title: &str, playing: bool, start_ms: i64) -> State {
        State {
            settings: Settings::default(),
            track: Some(Track {
                id: "1".to_owned(),
                title: title.to_owned(),
                artists: "Артист".to_owned(),
                album_id: None,
                album: None,
                cover: None,
                duration_ms: Some(60_000),
                local: false,
            }),
            playing,
            start_ms,
        }
    }

    #[test]
    fn time_drift_does_not_wake_the_task() {
        let a = state("Песня", true, 1_000_000);
        assert!(same(&a, &state("Песня", true, 1_000_800)));
        assert!(!same(&a, &state("Песня", true, 1_002_000)));
        assert!(!same(&a, &state("Другая", true, 1_000_000)));
        // На паузе начало не важно.
        let p = state("Песня", false, 1);
        assert!(same(&p, &state("Песня", false, 99_999_999)));
        assert!(!same(&a, &state("Песня", false, 1_000_000)));
    }

    #[test]
    fn set_activity_body_has_pid_and_nonce() {
        let body = set_activity_body(activity(&state("Песня", true, 5)).as_ref());
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["cmd"], "SET_ACTIVITY");
        assert_eq!(value["args"]["pid"], std::process::id());
        assert_eq!(value["args"]["activity"]["details"], "Песня");
        assert!(value["nonce"].as_str().is_some_and(|n| n.len() == 36));
        let clear: serde_json::Value = serde_json::from_slice(&set_activity_body(None)).unwrap();
        assert!(clear["args"]["activity"].is_null());
    }

    /// Задача целиком, на искусственном соединении: рукопожатие, статус,
    /// очистка при выходе, ответ на PING.
    #[tokio::test]
    async fn session_sets_and_clears_activity() {
        let (client, server) = tokio::io::duplex(8192);
        let boxed: Box<dyn Duplex> = Box::new(client);
        let (reader, writer) = tokio::io::split(boxed);
        let (frames_tx, frames_rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut reader = reader;
            while let Some(frame) = ipc::read_frame(&mut reader).await {
                if frames_tx.send(frame).is_err() {
                    break;
                }
            }
        });
        let (state_tx, mut state_rx) = watch::channel(state("Песня", true, 5));
        let task = tokio::spawn(async move {
            session(Out::Pipe(writer), Some(frames_rx), &mut state_rx).await
        });
        let (mut server_read, mut server_write) = tokio::io::split(server);
        let (op, body) = ipc::read_frame(&mut server_read).await.unwrap();
        assert_eq!(op, OP_FRAME);
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["args"]["activity"]["details"], "Песня");
        // PING получает PONG с тем же телом.
        assert!(ipc::write_frame(&mut server_write, OP_PING, b"x").await);
        let (op, body) = ipc::read_frame(&mut server_read).await.unwrap();
        assert_eq!((op, body.as_slice()), (OP_PONG, &b"x"[..]));
        // Окно закрылось: статус очищается, задача выходит.
        drop(state_tx);
        let (_, body) = ipc::read_frame(&mut server_read).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(value["args"]["activity"].is_null());
        assert!(task.await.unwrap());
    }
}
