//! Выбор видеокарты для рендера окна.
//!
//! Окно рисует только wgpu. По умолчанию iced просит у системы видеокарту
//! `HighPerformance`, и на ноутбуках с двумя видеокартами это будит
//! дискретную. Для интерфейса хватает встроенной, поэтому до запуска iced
//! ставим `WGPU_POWER_PREF=low` (значения: `low`, `high`, `none`), если
//! пользователь не задал её сам. Записывать в окружение приходится через
//! `unsafe`: это единственное место (см. [`prefer_low_power`]).

/// Имя переменной окружения, по которой wgpu выбирает видеокарту.
const VAR: &str = "WGPU_POWER_PREF";

/// Ставит `WGPU_POWER_PREF=low`, если переменная не задана. Вызывается один
/// раз в начале [`super::run`], до запуска iced и tokio.
#[allow(unsafe_code)]
pub fn prefer_low_power() {
    if !needs_default(std::env::var(VAR).ok().as_deref()) {
        return;
    }
    // SAFETY: функция вызывается из главного потока до запуска iced и
    // tokio. В `main` до `ui::run()` разбираются только аргументы
    // командной строки, потоков никто не создаёт, поэтому одновременного
    // чтения окружения из другого потока нет.
    unsafe {
        std::env::set_var(VAR, "low");
    }
}

/// Нужно ли подставить значение по умолчанию: переменная не задана или пуста.
fn needs_default(value: Option<&str>) -> bool {
    value.is_none_or(str::is_empty)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_set_only_when_variable_is_missing() {
        assert!(needs_default(None));
        assert!(needs_default(Some("")));
        assert!(!needs_default(Some("high")));
        assert!(!needs_default(Some("low")));
    }
}
