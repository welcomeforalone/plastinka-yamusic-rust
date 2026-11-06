//! VibeColorizer: цвет «Моей волны» под обложку текущего трека.
//!
//! Модуль только считает цвет, без окна и сети. Источники по порядку:
//! `derivedColors.accent`, `derivedColors.average`, оттенок `hue` из
//! параметров трека, средний цвет обложки. Любой цвет приводится к читаемому
//! на чёрном фоне ([`lift`]).

use iced::Color;

/// Насыщенность и светлота цвета из оттенка: умеренные, чтобы на чёрном
/// выглядело как акцент, а не кислотно.
const HUE_SATURATION: f32 = 0.55;
const HUE_LIGHTNESS: f32 = 0.5;
/// Пределы светлоты читаемого цвета и минимальная насыщенность у цветных.
const MIN_LIGHTNESS: f32 = 0.42;
const MAX_LIGHTNESS: f32 = 0.68;
const MIN_SATURATION: f32 = 0.35;
/// Ниже этой насыщенности цвет считаем серым: оттенка у него нет, и
/// «насыщать» его нельзя (серое стало бы красным).
const GRAY_BELOW: f32 = 0.08;

/// Откуда взять цвет. Порядок полей — порядок выбора.
#[derive(Debug, Clone, Copy, Default)]
pub struct Sources<'a> {
    pub accent: Option<&'a str>,
    pub average: Option<&'a str>,
    pub hue: Option<f64>,
    /// Средний цвет обложки (RGB).
    pub cover: Option<[u8; 3]>,
}

/// Цвет трека. `None` — ни один источник не дал цвета.
pub fn pick(sources: Sources<'_>) -> Option<Color> {
    let color = sources
        .accent
        .and_then(parse_hex)
        .or_else(|| sources.average.and_then(parse_hex))
        .or_else(|| sources.hue.and_then(from_hue))
        .or_else(|| sources.cover.map(rgb8))?;
    Some(lift(Color { a: 1.0, ..color }))
}

/// `#RRGGBB` или `#RRGGBBAA`.
pub fn parse_hex(text: &str) -> Option<Color> {
    let digits = text.strip_prefix('#')?;
    if !matches!(digits.len(), 6 | 8) || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |at: usize| {
        digits
            .get(at..at + 2)
            .and_then(|pair| u8::from_str_radix(pair, 16).ok())
    };
    let alpha = if digits.len() == 8 { byte(6)? } else { u8::MAX };
    Some(Color::from_rgba8(
        byte(0)?,
        byte(2)?,
        byte(4)?,
        f32::from(alpha) / 255.0,
    ))
}

/// Цвет из оттенка в градусах.
pub fn from_hue(hue: f64) -> Option<Color> {
    if !hue.is_finite() {
        return None;
    }
    let degrees = hue.rem_euclid(360.0) as f32;
    Some(from_hsl(degrees, HUE_SATURATION, HUE_LIGHTNESS))
}

/// Приводит цвет к читаемому на чёрном: светлота в пределах, у цветных
/// насыщенность не ниже минимума; серый остаётся серым.
pub fn lift(color: Color) -> Color {
    let (hue, saturation, lightness) = to_hsl(color);
    let saturation = if saturation < GRAY_BELOW {
        saturation
    } else {
        saturation.max(MIN_SATURATION)
    };
    let lightness = lightness.clamp(MIN_LIGHTNESS, MAX_LIGHTNESS);
    Color {
        a: color.a,
        ..from_hsl(hue, saturation, lightness)
    }
}

/// Средний цвет по пикселям RGBA. Полностью прозрачные не считаются.
pub fn average_rgb(pixels: &[u8]) -> Option<[u8; 3]> {
    let (mut r, mut g, mut b, mut count) = (0u64, 0u64, 0u64, 0u64);
    for [red, green, blue, alpha] in pixels.as_chunks::<4>().0 {
        if *alpha == 0 {
            continue;
        }
        r += u64::from(*red);
        g += u64::from(*green);
        b += u64::from(*blue);
        count += 1;
    }
    if count == 0 {
        return None;
    }
    let mean = |total: u64| u8::try_from(total / count).unwrap_or(u8::MAX);
    Some([mean(r), mean(g), mean(b)])
}

fn rgb8([r, g, b]: [u8; 3]) -> Color {
    Color::from_rgb8(r, g, b)
}

/// Цвет в HSL: оттенок в градусах, остальное от 0 до 1.
fn to_hsl(color: Color) -> (f32, f32, f32) {
    let (r, g, b) = (color.r, color.g, color.b);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let lightness = (max + min) / 2.0;
    let delta = max - min;
    if delta <= f32::EPSILON {
        return (0.0, 0.0, lightness);
    }
    let saturation = delta / (1.0 - (2.0 * lightness - 1.0).abs()).max(f32::EPSILON);
    let hue = if (max - r).abs() <= f32::EPSILON {
        ((g - b) / delta).rem_euclid(6.0)
    } else if (max - g).abs() <= f32::EPSILON {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    };
    (hue * 60.0, saturation.clamp(0.0, 1.0), lightness)
}

fn from_hsl(hue: f32, saturation: f32, lightness: f32) -> Color {
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let sector = hue.rem_euclid(360.0) / 60.0;
    let x = chroma * (1.0 - (sector.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match sector {
        s if s < 1.0 => (chroma, x, 0.0),
        s if s < 2.0 => (x, chroma, 0.0),
        s if s < 3.0 => (0.0, chroma, x),
        s if s < 4.0 => (0.0, x, chroma),
        s if s < 5.0 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = lightness - chroma / 2.0;
    Color::from_rgb(r + m, g + m, b + m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Color, b: Color) -> bool {
        (a.r - b.r).abs() < 0.01 && (a.g - b.g).abs() < 0.01 && (a.b - b.b).abs() < 0.01
    }

    #[test]
    fn hex_with_and_without_alpha() {
        assert!(close(
            parse_hex("#3A7BD5").unwrap(),
            Color::from_rgb8(0x3A, 0x7B, 0xD5)
        ));
        let with_alpha = parse_hex("#FF000080").unwrap();
        assert!((with_alpha.a - 128.0 / 255.0).abs() < 0.001);
        assert_eq!(parse_hex("#FF0000").unwrap().a, 1.0);
        for bad in ["", "3A7BD5", "#12345", "#GGGGGG", "#1234567", "rgb(1,2,3)"] {
            assert_eq!(parse_hex(bad), None, "{bad}");
        }
    }

    #[test]
    fn hue_gives_moderate_color() {
        let red = from_hue(0.0).unwrap();
        assert!(red.r > red.g && red.r > red.b);
        let blue = from_hue(240.0).unwrap();
        assert!(blue.b > blue.r && blue.b > blue.g);
        // Оттенок по кругу: 360 — как 0, отрицательный — тоже.
        assert!(close(from_hue(360.0).unwrap(), red));
        assert!(close(from_hue(-120.0).unwrap(), blue));
        assert_eq!(from_hue(f64::NAN), None);
        // Не кислотный: ни один канал не выходит в 1.0 целиком.
        assert!(red.r < 0.95);
    }

    #[test]
    fn dark_and_gray_colors_are_lifted() {
        let dark = lift(Color::from_rgb8(0x10, 0x18, 0x20));
        let (_, saturation, lightness) = to_hsl(dark);
        assert!(lightness >= MIN_LIGHTNESS - 0.01, "{lightness}");
        assert!(saturation >= MIN_SATURATION - 0.01, "{saturation}");
        // Слишком светлый — темнее.
        let (_, _, light) = to_hsl(lift(Color::WHITE));
        assert!(light <= MAX_LIGHTNESS + 0.01);
        // Серый остаётся серым (не краснеет), но светлеет.
        let gray = lift(Color::from_rgb8(0x20, 0x20, 0x20));
        assert!((gray.r - gray.g).abs() < 0.01 && (gray.g - gray.b).abs() < 0.01);
        assert!(gray.r > 0.3);
        // Читаемый цвет не меняется.
        let good = Color::from_rgb8(0x3A, 0x7B, 0xD5);
        assert!(close(lift(good), good));
    }

    #[test]
    fn sources_are_tried_in_order() {
        let all = Sources {
            accent: Some("#FF0000"),
            average: Some("#0000FF"),
            hue: Some(120.0),
            cover: Some([0, 255, 0]),
        };
        let red = pick(all).unwrap();
        assert!(red.r > red.b);
        let no_accent = pick(Sources {
            accent: None,
            ..all
        })
        .unwrap();
        assert!(no_accent.b > no_accent.r);
        // Плохой hex пропускается, берётся следующий источник.
        let bad = pick(Sources {
            accent: Some("красный"),
            average: None,
            ..all
        })
        .unwrap();
        assert!(bad.g > bad.r && bad.g > bad.b);
        let cover = pick(Sources {
            accent: None,
            average: None,
            hue: None,
            cover: Some([0, 0, 255]),
        })
        .unwrap();
        assert!(cover.b > cover.r);
        assert_eq!(pick(Sources::default()), None);
    }

    #[test]
    fn average_of_pixels() {
        let pixels = [255, 0, 0, 255, 0, 0, 255, 255, 9, 9, 9, 0];
        // Прозрачный пиксель не считается.
        assert_eq!(average_rgb(&pixels), Some([127, 0, 127]));
        assert_eq!(average_rgb(&[]), None);
        assert_eq!(average_rgb(&[1, 2, 3, 0]), None);
    }
}
