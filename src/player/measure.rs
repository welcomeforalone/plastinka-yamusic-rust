//! Замер запуска и перемотки на имитации сети: каждый Range-запрос отвечает
//! с задержкой, скорость ограничена. Файлы лежат вне репозитория (их делает
//! ffmpeg), каталог — в `PLASTINKA_TEST_AUDIO_DIR`. Запуск:
//! `cargo test measure_formats -- --ignored --nocapture`. Ничего не пишется
//! на диск: файл читается в память и отдаётся из памяти.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::num::NonZero;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::*;

/// Задержка ответа на каждый запрос.
const DELAY: Duration = Duration::from_millis(80);
/// Скорость отдачи по умолчанию, МБ/с (около 200 Мбит/с); меняется
/// `PLASTINKA_TEST_RATE`.
fn rate() -> f64 {
    std::env::var("PLASTINKA_TEST_RATE")
        .ok()
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(25.0)
        * 1024.0
        * 1024.0
}

struct Sim {
    port: u16,
    starts: Arc<Mutex<Vec<u64>>>,
    stop: Arc<AtomicBool>,
}

impl Sim {
    fn start(data: Arc<Vec<u8>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let starts = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (log, halt) = (starts.clone(), stop.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if halt.load(Ordering::SeqCst) {
                    return;
                }
                let Ok(stream) = stream else { continue };
                let (data, log) = (data.clone(), log.clone());
                std::thread::spawn(move || serve(stream, &data, &log));
            }
        });
        Self { port, starts, stop }
    }

    fn requests(&self) -> usize {
        self.starts.lock().unwrap().len()
    }
}

impl Drop for Sim {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = std::net::TcpStream::connect(("127.0.0.1", self.port));
    }
}

fn serve(stream: std::net::TcpStream, data: &[u8], log: &Mutex<Vec<u64>>) {
    let Ok(clone) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(clone);
    let mut start = 0_u64;
    let mut line = String::new();
    while reader.read_line(&mut line).is_ok() {
        let lower = line.to_ascii_lowercase();
        if let Some(range) = lower.strip_prefix("range: bytes=") {
            start = range.trim().trim_end_matches('-').parse().unwrap_or(0);
        }
        if line.trim().is_empty() {
            break;
        }
        line.clear();
    }
    log.lock().unwrap().push(start);
    std::thread::sleep(DELAY);
    let total = data.len() as u64;
    let body = data
        .get(usize::try_from(start).unwrap_or(0)..)
        .unwrap_or(&[]);
    let mut stream = stream;
    let head = format!(
        "HTTP/1.1 206 Partial Content\r\nContent-Type: audio/mp4\r\nContent-Range: bytes {start}-{}/{total}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        total - 1,
        body.len()
    );
    if stream.write_all(head.as_bytes()).is_err() {
        return;
    }
    for chunk in body.chunks(64 * 1024) {
        if stream.write_all(chunk).is_err() {
            return;
        }
        std::thread::sleep(Duration::from_secs_f64(chunk.len() as f64 / rate()));
    }
}

/// Ждёт, пока `done` станет истиной; возвращает, сколько ушло времени.
fn wait(limit: Duration, mut done: impl FnMut() -> bool) -> Option<Duration> {
    let started = Instant::now();
    while started.elapsed() < limit {
        if done() {
            return Some(started.elapsed());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    None
}

/// `resume` — с какой секунды запускать (как смена качества посреди трека).
fn measure(path: &std::path::Path, resume: u64) {
    let data = Arc::new(std::fs::read(path).unwrap());
    let sim = Sim::start(data);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let volume = Arc::new(Volume::default());
    volume.set(1.0);
    let shared = Shared::new(
        StreamBuffer::new(),
        volume,
        Arc::new(Tap::default()),
        Duration::from_secs(resume),
    );
    let url = format!("http://127.0.0.1:{}/track", sim.port);
    let relink: download::Relink = Box::new(move || {
        let url = url.clone();
        Box::pin(async move {
            Ok(api::StreamSource::Url {
                url,
                kbps: 0,
                codec: Codec::Flac,
            })
        })
    });
    let buffer = shared.buffer.clone();
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let started = Instant::now();
    runtime.spawn(download::run(buffer, http, relink, |_| {}));
    let (mixer, mut source) =
        rodio::mixer::mixer(NonZero::new(2).unwrap(), NonZero::new(44_100).unwrap());
    let decoder = shared.clone();
    std::thread::spawn(move || decode::run(decoder, mixer, None));
    // Забираем звук быстрее, чем он играет (как в `run_fake`).
    let drain = shared.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let halt = stop.clone();
    let drainer = std::thread::spawn(move || {
        // Звук забирается в реальном времени, как при игре.
        while !halt.load(Ordering::SeqCst) && !drain.finished() {
            for _ in 0..1764 {
                let _ = source.next();
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    });
    let limit = Duration::from_secs(
        std::env::var("PLASTINKA_TEST_LIMIT")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(30),
    );
    let first = wait(limit, || {
        !shared.seek_pending() && shared.position() > Duration::from_secs(resume)
    });
    let open_requests = sim.requests();
    let line = |name: &str, took: Option<Duration>, requests: usize| {
        eprintln!(
            "  {name:<22} {:>8}  запросов: {requests}",
            took.map_or("не дождались".to_owned(), |took| format!(
                "{:.2} с",
                took.as_secs_f64()
            ))
        );
    };
    eprintln!(
        "{} (старт с {resume} с)",
        path.file_name().unwrap().to_string_lossy()
    );
    line(
        "до первого звука",
        first.map(|_| started.elapsed()),
        open_requests,
    );
    if first.is_none() {
        eprintln!(
            "  ошибка декодера: {:?}, очередь {}, конец {}",
            shared.decode_error(),
            shared.queued(),
            shared.finished()
        );
    }
    if open_requests > 30 || first.is_none() {
        let starts = sim.starts.lock().unwrap();
        eprintln!(
            "  первые смещения: {:?}",
            starts.iter().take(25).collect::<Vec<_>>()
        );
    }
    if resume > 0 {
        shared.stop();
        stop.store(true, Ordering::SeqCst);
        let _ = drainer.join();
        return;
    }
    for (name, to) in [("вперёд на 3:00", 180), ("назад на 0:30", 30)] {
        let before = sim.requests();
        let asked = Instant::now();
        shared.request_seek(Duration::from_secs(to));
        // Перемотка кончилась, и позиция пошла от цели.
        let took = wait(limit, || {
            let position = shared.position().as_secs();
            !shared.seek_pending() && position >= to && position <= to + 5
        });
        line(name, took.map(|_| asked.elapsed()), sim.requests() - before);
    }
    shared.stop();
    stop.store(true, Ordering::SeqCst);
    let _ = drainer.join();
}

#[test]
#[ignore = "нужны файлы из PLASTINKA_TEST_AUDIO_DIR"]
fn measure_formats() {
    let Ok(dir) = std::env::var("PLASTINKA_TEST_AUDIO_DIR") else {
        return;
    };
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    files.sort();
    let only = std::env::var("PLASTINKA_TEST_ONLY").unwrap_or_default();
    for path in files {
        if !path.to_string_lossy().contains(&only) {
            continue;
        }
        measure(&path, 0);
        measure(&path, 90);
    }
}
