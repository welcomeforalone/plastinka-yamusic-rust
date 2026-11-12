//! Разбор текста с метками времени (LRC). Формат неформальный, разбор мягкий:
//! битая строка пропускается, остальные не страдают. Спека — раздел 3
//! `docs/api/stage-9.md`.

use super::Line;
use crate::api::clean;

/// Разбирает LRC. Строки без меток времени и метаданные (`[ar:…]`, `[ti:…]`,
/// `[offset:…]` и т. п.) пропускаются; порядок — по времени, при равенстве
/// как в тексте.
pub fn parse(text: &str) -> Vec<Line> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = Vec::new();
    for raw in text.lines() {
        parse_line(raw, &mut lines);
    }
    lines.sort_by_key(|line| line.time_ms);
    lines
}

/// Одна строка: одна или несколько меток подряд и текст после них.
fn parse_line(raw: &str, out: &mut Vec<Line>) {
    let mut rest = raw.trim_start();
    let mut times: Vec<u64> = Vec::new();
    while let Some(tail) = rest.strip_prefix('[') {
        let Some((tag, after)) = tail.split_once(']') else {
            break;
        };
        match timestamp(tag) {
            Some(ms) => {
                times.push(ms);
                rest = after.trim_start();
            }
            // Метаданные в начале строки: вся строка не текст песни.
            None if times.is_empty() => return,
            None => break,
        }
    }
    if times.is_empty() {
        return;
    }
    let text = clean(&strip_word_tags(rest)).unwrap_or_default();
    for time_ms in times {
        out.push(Line {
            time_ms,
            text: text.clone(),
        });
    }
}

/// Убирает посословные метки `<мм:сс.хх>` (расширенный LRC): строка
/// показывается целиком. Другие угловые скобки остаются текстом.
fn strip_word_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        let (before, tail) = rest.split_at(start);
        out.push_str(before);
        let tag_len = tail
            .strip_prefix('<')
            .and_then(|inner| inner.split_once('>'))
            .filter(|(tag, _)| timestamp(tag).is_some())
            .map(|(tag, _)| tag.len() + 2);
        match tag_len {
            Some(len) => rest = tail.get(len..).unwrap_or_default(),
            None => {
                out.push('<');
                rest = tail.get(1..).unwrap_or_default();
            }
        }
    }
    out.push_str(rest);
    out
}

/// `мм:сс`, `мм:сс.х`, `мм:сс.хх`, `мм:сс.ххх` и `мм:сс:хх` → миллисекунды.
/// Минуты — любое число (бывает больше 99). `ar`, `offset` и другие метаданные
/// сюда не проходят: до двоеточия должны быть цифры.
fn timestamp(tag: &str) -> Option<u64> {
    let (minutes, rest) = tag.split_once(':')?;
    let minutes = number(minutes, 6)?;
    let (seconds, fraction) = match rest.split_once(['.', ':']) {
        Some((seconds, fraction)) => (seconds, Some(fraction)),
        None => (rest, None),
    };
    let seconds = number(seconds, 4)?;
    let millis = match fraction {
        Some(fraction) => fraction_millis(fraction)?,
        None => 0,
    };
    minutes
        .checked_mul(60)?
        .checked_add(seconds)?
        .checked_mul(1000)?
        .checked_add(millis)
}

/// Целое из цифр, не длиннее `max_digits`.
fn number(text: &str, max_digits: usize) -> Option<u64> {
    let digits = !text.is_empty()
        && text.len() <= max_digits
        && text.bytes().all(|byte| byte.is_ascii_digit());
    if digits { text.parse().ok() } else { None }
}

/// Дробная часть секунды: `5` — 500 мс, `50` — 500, `500` — 500; лишние цифры
/// после третьей отбрасываются.
fn fraction_millis(fraction: &str) -> Option<u64> {
    if fraction.is_empty() || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let first: String = fraction.chars().take(3).collect();
    let value: u64 = first.parse().ok()?;
    let missing = 3 - u32::try_from(first.len()).ok()?;
    value.checked_mul(10u64.pow(missing))
}

/// Номер строки, которая звучит в `position_ms`: последняя с меткой не позже
/// позиции. До первой строки — `None`.
pub fn active_line(lines: &[Line], position_ms: u64) -> Option<usize> {
    lines
        .partition_point(|line| line.time_ms <= position_ms)
        .checked_sub(1)
}

/// Обычный текст из строк: по строке на запись, пустые паузы остаются.
pub fn plain_text(lines: &[Line]) -> String {
    let texts: Vec<&str> = lines.iter().map(|line| line.text.as_str()).collect();
    texts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn times(lines: &[Line]) -> Vec<u64> {
        lines.iter().map(|line| line.time_ms).collect()
    }

    fn texts(lines: &[Line]) -> Vec<&str> {
        lines.iter().map(|line| line.text.as_str()).collect()
    }

    #[test]
    fn basic_lines() {
        let lines = parse("[00:12.00] Первая\n[01:02.50]Вторая  \n");
        assert_eq!(times(&lines), vec![12_000, 62_500]);
        assert_eq!(texts(&lines), vec!["Первая", "Вторая"]);
    }

    #[test]
    fn fraction_forms() {
        assert_eq!(timestamp("00:01"), Some(1000));
        assert_eq!(timestamp("00:01.5"), Some(1500));
        assert_eq!(timestamp("00:01.05"), Some(1050));
        assert_eq!(timestamp("00:01.005"), Some(1005));
        assert_eq!(timestamp("00:01.9999"), Some(1999));
        assert_eq!(timestamp("01:02:34"), Some(62_340));
        assert_eq!(timestamp("100:05.00"), Some(6_005_000));
    }

    #[test]
    fn not_a_timestamp() {
        for tag in [
            "ar:Исполнитель",
            "ti:Название",
            "offset:+500",
            "00:xx",
            "a:01",
            ":01",
            "00:",
            "00:01.",
            "00:01.ab",
            "",
            "1234567:00",
        ] {
            assert_eq!(timestamp(tag), None, "{tag}");
        }
    }

    #[test]
    fn several_tags_share_text() {
        let lines = parse("[00:12.00][01:30.50] Припев");
        assert_eq!(times(&lines), vec![12_000, 90_500]);
        assert_eq!(texts(&lines), vec!["Припев", "Припев"]);
    }

    #[test]
    fn empty_text_is_kept_as_pause() {
        let lines = parse("[00:10.00] a\n[00:45.00]\n[00:50.00]   \n");
        assert_eq!(texts(&lines), vec!["a", "", ""]);
    }

    #[test]
    fn metadata_and_garbage_are_skipped() {
        let lines = parse(
            "[ar:Исполнитель]\n[ti:Название]\n[length: 03:45]\n[offset:+500]\n\
             просто текст\n\n[00:01.00] строка\n[00:02.00 сломана\n",
        );
        assert_eq!(texts(&lines), vec!["строка"]);
    }

    #[test]
    fn bom_and_crlf() {
        let lines = parse("\u{feff}[00:01.00] a\r\n[00:02.00] b\r\n");
        assert_eq!(texts(&lines), vec!["a", "b"]);
    }

    #[test]
    fn unsorted_input_is_sorted_stably() {
        let lines = parse("[00:20.00] c\n[00:10.00] a\n[00:10.00] b\n");
        assert_eq!(texts(&lines), vec!["a", "b", "c"]);
    }

    #[test]
    fn word_level_tags_are_removed() {
        let lines = parse("[00:01.00]<00:01.00>Раз <00:01.50>два <b> три");
        assert_eq!(texts(&lines), vec!["Раз два <b> три"]);
    }

    #[test]
    fn non_time_tag_after_time_stays_in_text() {
        let lines = parse("[00:01.00][x] текст");
        assert_eq!(texts(&lines), vec!["[x] текст"]);
    }

    #[test]
    fn no_timestamps_gives_nothing() {
        assert!(parse("строка\nещё строка").is_empty());
        assert!(parse("").is_empty());
    }

    #[test]
    fn huge_numbers_do_not_overflow() {
        assert!(parse("[999999:9999.999] x").len() <= 1);
        assert_eq!(timestamp("99999999999999999999:00"), None);
    }

    #[test]
    fn active_line_follows_position() {
        let lines = parse("[00:10.00] a\n[00:20.00] b\n[00:20.00] c\n[00:30.00] d");
        assert_eq!(active_line(&lines, 0), None);
        assert_eq!(active_line(&lines, 9_999), None);
        assert_eq!(active_line(&lines, 10_000), Some(0));
        assert_eq!(active_line(&lines, 19_999), Some(0));
        // Две строки с одним временем: звучит последняя.
        assert_eq!(active_line(&lines, 20_000), Some(2));
        assert_eq!(active_line(&lines, 999_999), Some(3));
        assert_eq!(active_line(&[], 5), None);
    }

    #[test]
    fn plain_text_joins_lines() {
        let lines = parse("[00:01.00] a\n[00:02.00]\n[00:03.00] b");
        assert_eq!(plain_text(&lines), "a\n\nb");
    }
}
