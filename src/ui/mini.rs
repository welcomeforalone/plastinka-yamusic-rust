//! Мини-плеер: отдельное окно без рамки поверх остальных. Здесь только то,
//! что не зависит от окна iced: положение и раскладка в `config.toml`,
//! параметры окна, цвет фона по обложке и решение, открывать ли окно. Вид —
//! `library/mini.rs`, сообщения — в `ui/mod.rs`. Состояние плеера берётся из
//! той же `Library`, что у главного окна: ничего не дублируется.

use iced::{Color, Point, Size, window};

use crate::storage::config::MiniConfig;

/// Высокая раскладка: обложка сверху, кнопки снизу.
pub const TALL: Size = Size::new(320.0, 440.0);
/// Низкая раскладка: обложка слева, справа название и кнопки.
pub const COMPACT: Size = Size::new(420.0, 132.0);
/// Положения дальше этого от нуля считаем испорченными: окно не должно
/// оказаться вне всех мониторов.
const MAX_COORD: i64 = 20_000;

/// Что запоминается между запусками.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MiniPrefs {
    pub position: Option<Point>,
    pub compact: bool,
}

impl MiniPrefs {
    /// Из `config.toml`. Положение читается, только если заданы обе
    /// координаты и они разумны; иначе окно встанет, где решит система.
    pub fn from_config(config: &MiniConfig) -> Self {
        let position = match (config.x, config.y) {
            (Some(x), Some(y)) if x.abs() <= MAX_COORD && y.abs() <= MAX_COORD => {
                // Пределы выше: потери при переводе в f32 нет.
                Some(Point::new(x as f32, y as f32))
            }
            _ => None,
        };
        Self {
            position,
            compact: config.compact.unwrap_or(false),
        }
    }

    /// В `config.toml`: без положения координаты стираются.
    pub fn write(&self, config: &mut MiniConfig) {
        let rounded = |value: f32| i64::from(value.round() as i32);
        *config = MiniConfig {
            x: self.position.map(|point| rounded(point.x)),
            y: self.position.map(|point| rounded(point.y)),
            compact: self.compact.then_some(true),
        };
    }

    /// Запоминает новое положение окна; странные значения (NaN, далеко за
    /// экраном) отбрасываются. Свёрнутое окно Windows докладывает
    /// (-32000, -32000): его тоже не запоминаем.
    pub fn moved(&mut self, point: Point) {
        let sane = point.x.is_finite()
            && point.y.is_finite()
            && point.x.abs() <= MAX_COORD as f32
            && point.y.abs() <= MAX_COORD as f32
            && point.x > -30_000.0;
        if sane {
            self.position = Some(point);
        }
    }

    /// Размер окна выбранной раскладки.
    pub fn size(&self) -> Size {
        if self.compact { COMPACT } else { TALL }
    }
}

/// Уровень окна: закреплённое — поверх остальных.
pub fn level(pinned: bool) -> window::Level {
    if pinned {
        window::Level::AlwaysOnTop
    } else {
        window::Level::Normal
    }
}

/// Параметры окна мини-плеера.
pub fn settings(prefs: &MiniPrefs, pinned: bool) -> window::Settings {
    window::Settings {
        size: prefs.size(),
        position: prefs
            .position
            .map_or(window::Position::Default, window::Position::Specific),
        resizable: false,
        decorations: false,
        // Углы в Linux скругляем сами (см. `corners`).
        transparent: super::corners::ENABLED,
        level: level(pinned),
        // Закрытие обрабатываем сами: вместе с окном нужно вернуть главное.
        exit_on_close_request: false,
        icon: super::app_icon(),
        platform_specific: super::platform_specific(),
        ..window::Settings::default()
    }
}

/// Открывать ли мини-плеер: только в главном окне (после проверки Плюса,
/// правило 2) и если он ещё не открыт.
pub fn should_open(main_screen: bool, already_open: bool) -> bool {
    main_screen && !already_open
}

/// Закрыть ли мини-плеер без просьбы пользователя: главное окно ушло с
/// главного экрана (выход из аккаунта, подписка пропала).
pub fn should_close(main_screen: bool, open: bool) -> bool {
    open && !main_screen
}

/// Фон по среднему цвету обложки: насыщенность не больше 20 %, яркость
/// 18 % (HSV), оттенок прежний. Без обложки — цвет подложки.
pub fn background(average: Option<[u8; 3]>) -> Color {
    let Some([r, g, b]) = average else {
        return super::theme::SURFACE_2;
    };
    let (r, g, b) = (
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
    );
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let hue = if delta <= f32::EPSILON {
        0.0
    } else if max == r {
        ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    };
    // Серый остаётся серым: насыщенность только убавляется.
    let saturation = if max <= f32::EPSILON {
        0.0
    } else {
        delta / max
    };
    hsv(hue, saturation.min(0.2), 0.18)
}

/// HSV (оттенок в шестых долях круга, 0…6) → цвет.
fn hsv(hue: f32, saturation: f32, value: f32) -> Color {
    let chroma = value * saturation;
    let x = chroma * (1.0 - (hue.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match hue.floor() as i32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = value - chroma;
    Color::from_rgb(r + m, g + m, b + m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_roundtrip() {
        let prefs = MiniPrefs {
            position: Some(Point::new(120.0, -40.0)),
            compact: true,
        };
        let mut config = MiniConfig::default();
        prefs.write(&mut config);
        assert_eq!(config.x, Some(120));
        assert_eq!(config.y, Some(-40));
        assert_eq!(MiniPrefs::from_config(&config), prefs);
        // По умолчанию: ничего не пишется.
        MiniPrefs::default().write(&mut config);
        assert_eq!(config, MiniConfig::default());
    }

    #[test]
    fn broken_config_gives_defaults() {
        let half = MiniConfig {
            x: Some(10),
            y: None,
            compact: None,
        };
        assert_eq!(MiniPrefs::from_config(&half), MiniPrefs::default());
        let far = MiniConfig {
            x: Some(1_000_000),
            y: Some(5),
            compact: Some(true),
        };
        let prefs = MiniPrefs::from_config(&far);
        assert_eq!(prefs.position, None);
        assert!(prefs.compact);
        // Размеры раскладок.
        assert_eq!(MiniPrefs::default().size(), TALL);
        assert_eq!(prefs.size(), COMPACT);
    }

    #[test]
    fn moved_ignores_nonsense() {
        let mut prefs = MiniPrefs::default();
        prefs.moved(Point::new(f32::NAN, 1.0));
        assert_eq!(prefs.position, None);
        prefs.moved(Point::new(-32_000.0, -32_000.0));
        assert_eq!(prefs.position, None);
        prefs.moved(Point::new(300.0, 200.0));
        assert_eq!(prefs.position, Some(Point::new(300.0, 200.0)));
        // Плохое значение прежнее положение не стирает.
        prefs.moved(Point::new(900_000.0, 0.0));
        assert_eq!(prefs.position, Some(Point::new(300.0, 200.0)));
    }

    #[test]
    fn open_and_close_rules() {
        // Открывается только в главном окне и только один раз.
        assert!(should_open(true, false));
        assert!(!should_open(true, true));
        assert!(!should_open(false, false));
        // Закрывается само, когда главное окно ушло с главного экрана.
        assert!(should_close(false, true));
        assert!(!should_close(true, true));
        assert!(!should_close(false, false));
    }

    #[test]
    fn window_is_on_top_when_pinned() {
        assert_eq!(level(true), window::Level::AlwaysOnTop);
        assert_eq!(level(false), window::Level::Normal);
        let s = settings(&MiniPrefs::default(), true);
        assert!(!s.decorations);
        assert!(!s.resizable);
        assert_eq!(s.transparent, super::super::corners::ENABLED);
        assert_eq!(s.size, TALL);
        assert_eq!(s.level, window::Level::AlwaysOnTop);
        let placed = settings(
            &MiniPrefs {
                position: Some(Point::new(5.0, 6.0)),
                compact: false,
            },
            false,
        );
        assert!(matches!(placed.position, window::Position::Specific(_)));
    }

    #[test]
    fn background_is_muted_cover_color() {
        // Яркость 18 % у самого яркого канала, насыщенность 20 %.
        let color = background(Some([255, 0, 0]));
        assert!((color.r - 0.18).abs() < 0.01, "{color:?}");
        assert!((color.g - 0.144).abs() < 0.01, "{color:?}");
        // Серый остаётся серым, белый становится тёмно-серым.
        let white = background(Some([255, 255, 255]));
        assert!((white.r - 0.18).abs() < 0.01 && (white.r - white.b).abs() < 0.001);
        assert_eq!(background(None), super::super::theme::SURFACE_2);
    }
}
