//! Таймер на время перетаскивания окна (только Windows).
//!
//! Перетаскивание и растягивание окна идут через модальный цикл Windows.
//! Внутри него winit не выдаёт `AboutToWait` и `ResumeTimeReached`, поэтому
//! таймеры и подписки iced стоят; доходят только `WM_PAINT` и события
//! `Moved`/`Resized`, а когда мышь неподвижна, нет и их. Зато модальный цикл
//! диспетчеризует `WM_TIMER`, так что колбэк таймера потока срабатывает и
//! внутри него. Колбэк просит у окна внутреннюю перерисовку (`WM_PAINT`), а
//! кадр спектра публикует виджет `heartbeat` по своему сроку.
//!
//! Таймер запускается при начале перетаскивания и гасит сам себя, когда
//! основная кнопка мыши отпущена или окна уже нет. Здесь единственное в
//! проекте место с `unsafe`: вызовы системного API.
//!
//! На других платформах модального цикла нет, функции пустые.

#[cfg(windows)]
pub use imp::{active, set_wanted, set_window, start};

#[cfg(not(windows))]
pub fn set_window(_hwnd: isize) {}

#[cfg(not(windows))]
pub fn set_wanted(_wanted: bool) {}

#[cfg(not(windows))]
pub fn start() {}

/// Идёт ли перетаскивание (таймер ещё жив). Вне Windows никогда.
#[cfg(not(windows))]
pub fn active() -> bool {
    false
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod imp {
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};
    use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicUsize, Ordering};

    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::Graphics::Gdi::{RDW_INTERNALPAINT, RedrawWindow};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, IsWindow, KillTimer, SM_SWAPBUTTON, SetTimer,
    };

    /// Период таймера, мс. Таймер Windows с разрешением ~15,6 мс тикает раз в
    /// ~31 мс, что не меньше периода кадра `heartbeat` (30 мс).
    const PERIOD_MS: u32 = 30;

    /// HWND окна как число; 0 — окна нет.
    static WINDOW: AtomicIsize = AtomicIsize::new(0);
    /// Нужны ли сейчас кадры.
    static WANTED: AtomicBool = AtomicBool::new(false);
    /// Идентификатор запущенного таймера; 0 — не запущен.
    static TIMER: AtomicUsize = AtomicUsize::new(0);

    /// Что сделать на тике.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Action {
        /// Погасить таймер.
        Stop,
        /// Попросить перерисовку окна.
        Redraw,
        /// Ничего.
        Idle,
    }

    /// Какая кнопка мыши основная: при обмене кнопок в системе — правая.
    fn primary_button(swapped: bool) -> i32 {
        i32::from(if swapped { VK_RBUTTON } else { VK_LBUTTON })
    }

    /// Решение тика: кнопка отпущена или окна нет — остановить; нужны кадры —
    /// перерисовать; иначе ничего.
    fn decide(button_down: bool, window_ok: bool, wanted: bool) -> Action {
        if !button_down || !window_ok {
            Action::Stop
        } else if wanted {
            Action::Redraw
        } else {
            Action::Idle
        }
    }

    /// Запоминает окно для перерисовки.
    pub fn set_window(hwnd: isize) {
        WINDOW.store(hwnd, Ordering::Relaxed);
    }

    /// Нужны ли сейчас кадры (спектр, спиннеры, переходы).
    pub fn set_wanted(wanted: bool) {
        WANTED.store(wanted, Ordering::Relaxed);
    }

    /// Идёт ли перетаскивание: таймер запущен и ещё не погасил себя.
    pub fn active() -> bool {
        TIMER.load(Ordering::Relaxed) != 0
    }

    /// Запускает таймер, если он ещё не запущен.
    pub fn start() {
        if TIMER.load(Ordering::Relaxed) != 0 {
            return;
        }
        // SAFETY: таймер потока (окно null, идентификатор 0 — система выдаст
        // свой); `tick` — функция с нужной сигнатурой, живёт всё время работы.
        let id = unsafe { SetTimer(null_mut(), 0, PERIOD_MS, Some(tick)) };
        TIMER.store(id, Ordering::Relaxed);
    }

    /// Колбэк таймера: вызывается на потоке окна, в том числе внутри
    /// модального цикла.
    unsafe extern "system" fn tick(_hwnd: HWND, _msg: u32, id: usize, _time: u32) {
        // SAFETY: простой запрос настройки системы, без указателей.
        let swapped = unsafe { GetSystemMetrics(SM_SWAPBUTTON) } != 0;
        // SAFETY: простой запрос состояния клавиши; отрицательный результат
        // (старший бит) означает «нажата».
        let button_down = unsafe { GetAsyncKeyState(primary_button(swapped)) } < 0;
        let raw = WINDOW.load(Ordering::Relaxed);
        // HWND — указатель; число получено из `raw_window_handle` и переводится
        // обратно без потери (знак просто переинтерпретируется).
        let hwnd: HWND = std::ptr::with_exposed_provenance_mut::<c_void>(raw.cast_unsigned());
        // SAFETY: `IsWindow` принимает любое значение, в том числе недействительное.
        let window_ok = raw != 0 && unsafe { IsWindow(hwnd) } != 0;
        match decide(button_down, window_ok, WANTED.load(Ordering::Relaxed)) {
            Action::Stop => {
                // SAFETY: гасим собственный таймер потока по выданному id.
                unsafe { KillTimer(null_mut(), id) };
                TIMER.store(0, Ordering::Relaxed);
            }
            Action::Redraw => {
                // SAFETY: окно проверено `IsWindow`; прямоугольник и регион не
                // нужны (null), флаг только ставит `WM_PAINT` в очередь.
                unsafe { RedrawWindow(hwnd, null(), null_mut(), RDW_INTERNALPAINT) };
            }
            Action::Idle => {}
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn swapped_buttons_use_right_button() {
            assert_eq!(primary_button(false), i32::from(VK_LBUTTON));
            assert_eq!(primary_button(true), i32::from(VK_RBUTTON));
        }

        #[test]
        fn tick_decision() {
            assert_eq!(decide(false, true, true), Action::Stop);
            assert_eq!(decide(true, false, true), Action::Stop);
            assert_eq!(decide(true, true, true), Action::Redraw);
            assert_eq!(decide(true, true, false), Action::Idle);
        }
    }
}
