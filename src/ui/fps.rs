//! Отладочный счётчик кадров (`PLASTINKA_TEST_FPS=1`, только отладочная сборка
//! с тестовыми хуками): раз в секунду пишет в stderr, сколько перерисовок
//! главного окна и сообщений `App::update` было за секунду и свёрнуто ли окно.
//! Для замеров нагрузки: свёрнутое окно должно рисовать 0 кадров в секунду.

use std::io::Write;
use std::sync::Once;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

static FRAMES: AtomicU32 = AtomicU32::new(0);
static MESSAGES: AtomicU32 = AtomicU32::new(0);
static MINIMIZED: AtomicBool = AtomicBool::new(false);
static START: Once = Once::new();

/// Включён ли счётчик.
fn enabled() -> bool {
    crate::storage::test_hooks_active() && std::env::var_os("PLASTINKA_TEST_FPS").is_some()
}

/// Одна перерисовка (`RedrawRequested`), в том числе спрятанная от детей.
pub fn frame() {
    FRAMES.fetch_add(1, Ordering::Relaxed);
}

/// Одно сообщение, прошедшее через `App::update`. Первый вызов запускает
/// поток, который печатает счётчики.
pub fn message() {
    if !enabled() {
        return;
    }
    MESSAGES.fetch_add(1, Ordering::Relaxed);
    START.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("fps".into())
            .spawn(report_loop);
    });
}

/// Запоминает, свёрнуто ли окно.
pub fn set_minimized(minimized: bool) {
    MINIMIZED.store(minimized, Ordering::Relaxed);
}

fn report_loop() {
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let frames = FRAMES.swap(0, Ordering::Relaxed);
        let messages = MESSAGES.swap(0, Ordering::Relaxed);
        let minimized = if MINIMIZED.load(Ordering::Relaxed) {
            "да"
        } else {
            "нет"
        };
        let _ = writeln!(
            std::io::stderr(),
            "кадров: {frames}, сообщений: {messages}, свёрнуто: {minimized}"
        );
    }
}
