//! Выбор рендера окна: на видеокарте (wgpu, по умолчанию) или
//! программный (tiny-skia).
//!
//! iced 0.14 собран с обоими рендерами, а выбирает между ними только по
//! переменной окружения `ICED_BACKEND` (`tiny-skia` или `wgpu`): без неё берётся
//! wgpu, а при его сбое — tiny-skia. Поля в настройках окна для этого нет, а
//! `iced::daemon` и весь интерфейс привязаны к `iced::Renderer`, поэтому
//! переменную ставим сами в самом начале запуска окна (см. [`choose`]).
//! Записывать в окружение приходится через `unsafe`: это единственное место.

use std::sync::atomic::{AtomicBool, Ordering};

/// Имя переменной окружения, по которой iced выбирает рендер.
const VAR: &str = "ICED_BACKEND";

/// Работает ли окно на программном рендере. Если ускорение включено, а wgpu
/// не запустился, iced сам перейдёт на tiny-skia, но здесь останется `false`.
static SOFTWARE: AtomicBool = AtomicBool::new(false);

/// С каким значением настройки запущено окно.
static BOOT_GPU: AtomicBool = AtomicBool::new(false);

/// Выбирает рендер по настройке `gpu`. Вызывается один раз в начале
/// [`super::run`], до запуска iced. Уже заданная пользователем `ICED_BACKEND`
/// важнее настройки.
#[allow(unsafe_code)]
pub fn choose(gpu: bool) {
    BOOT_GPU.store(gpu, Ordering::Relaxed);
    let current = std::env::var(VAR).ok();
    if current.as_deref().is_none_or(str::is_empty) && !gpu {
        // SAFETY: функция вызывается из главного потока до запуска iced и
        // tokio. В `main` до `ui::run()` разбираются только аргументы
        // командной строки, потоков никто не создаёт, поэтому одновременного
        // чтения окружения из другого потока нет.
        unsafe {
            std::env::set_var(VAR, "tiny-skia");
        }
    }
    let value = std::env::var(VAR).ok();
    SOFTWARE.store(is_software(value.as_deref()), Ordering::Relaxed);
}

/// Работает ли окно на программном рендере (см. [`SOFTWARE`]).
pub fn software() -> bool {
    SOFTWARE.load(Ordering::Relaxed)
}

/// С каким значением настройки «Использовать аппаратное ускорение» окно запущено.
pub fn boot_acceleration() -> bool {
    BOOT_GPU.load(Ordering::Relaxed)
}

/// Значит ли значение `ICED_BACKEND` программный рендер.
fn is_software(value: Option<&str>) -> bool {
    matches!(value, Some("tiny-skia" | "tiny_skia"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn software_backend_is_recognized() {
        assert!(is_software(Some("tiny-skia")));
        assert!(is_software(Some("tiny_skia")));
        assert!(!is_software(Some("wgpu")));
        assert!(!is_software(Some("")));
        assert!(!is_software(None));
    }
}
