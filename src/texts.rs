//! Общие тексты для командной строки и окна.

use std::time::Duration;

/// «трек», «трека» или «треков» для числа.
pub fn tracks_word(count: usize) -> &'static str {
    plural(count, ["трек", "трека", "треков"])
}

/// «прослушивание» в нужной форме: топ по истории.
pub fn plays_word(count: u32) -> &'static str {
    plural(
        usize::try_from(count).unwrap_or(usize::MAX),
        ["прослушивание", "прослушивания", "прослушиваний"],
    )
}

/// Форма слова для числа: [1, 2–4, 5–20].
pub fn plural(count: usize, forms: [&'static str; 3]) -> &'static str {
    let [one, few, many] = forms;
    match (count % 10, count % 100) {
        (1, n) if n != 11 => one,
        (2..=4, n) if !(12..=14).contains(&n) => few,
        _ => many,
    }
}

/// Общая длительность списка: «51 ч 12 мин», «12 мин», «меньше минуты».
pub fn total_duration(total: Duration) -> String {
    let minutes = total.as_secs() / 60;
    let (hours, minutes) = (minutes / 60, minutes % 60);
    match (hours, minutes) {
        (0, 0) => "меньше минуты".to_owned(),
        (0, minutes) => format!("{minutes} мин"),
        (hours, 0) => format!("{hours} ч"),
        (hours, minutes) => format!("{hours} ч {minutes} мин"),
    }
}

/// Сколько слушали исполнителя, как в топе месяца: «3,5 часа», «2 часа»,
/// «40 минут». Часы округляются до получаса. Ноль — `None`.
pub fn listened(total: Duration) -> Option<String> {
    let seconds = total.as_secs();
    if seconds == 0 {
        return None;
    }
    let halves = seconds.saturating_add(900) / 1800;
    if halves < 2 {
        let minutes = (seconds.saturating_add(30) / 60).max(1);
        let word = plural(count(minutes), ["минута", "минуты", "минут"]);
        return Some(format!("{minutes} {word}"));
    }
    let hours = halves / 2;
    Some(if halves.is_multiple_of(2) {
        format!("{hours} {}", plural(count(hours), ["час", "часа", "часов"]))
    } else {
        // С дробью — «часа»: «1,5 часа», «21,5 часа».
        format!("{hours},5 часа")
    })
}

/// Число для [`plural`]: огромное — как «много».
fn count(value: u64) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

/// Время `м:сс` для плеера.
pub fn clock(seconds: u64) -> String {
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// Текст ошибки как предложение: с большой буквы и с точкой в конце.
pub fn sentence(text: &str) -> String {
    let mut chars = text.trim().chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut out: String = first.to_uppercase().chain(chars).collect();
    if !out.ends_with(['.', '!', '?', '…']) {
        out.push('.');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_word_agrees_with_number() {
        for (count, word) in [
            (0, "треков"),
            (1, "трек"),
            (2, "трека"),
            (5, "треков"),
            (11, "треков"),
            (12, "треков"),
            (21, "трек"),
            (22, "трека"),
            (111, "треков"),
            (1001, "трек"),
        ] {
            assert_eq!(tracks_word(count), word, "{count}");
        }
    }

    #[test]
    fn durations_are_readable() {
        assert_eq!(total_duration(Duration::from_secs(30)), "меньше минуты");
        assert_eq!(total_duration(Duration::from_secs(12 * 60 + 5)), "12 мин");
        assert_eq!(total_duration(Duration::from_secs(2 * 3600)), "2 ч");
        assert_eq!(
            total_duration(Duration::from_secs(51 * 3600 + 12 * 60)),
            "51 ч 12 мин"
        );
        assert_eq!(clock(0), "0:00");
        assert_eq!(clock(467), "7:47");
        assert_eq!(clock(3725), "1:02:05");
    }

    #[test]
    fn listening_time_reads_like_the_top() {
        let hours = |seconds| listened(Duration::from_secs(seconds));
        assert_eq!(hours(0), None);
        assert_eq!(hours(13_110).as_deref(), Some("3,5 часа"));
        assert_eq!(hours(10_747).as_deref(), Some("3 часа"));
        assert_eq!(hours(8_823).as_deref(), Some("2,5 часа"));
        assert_eq!(hours(7_789).as_deref(), Some("2 часа"));
        assert_eq!(hours(3_600).as_deref(), Some("1 час"));
        assert_eq!(hours(5 * 3600).as_deref(), Some("5 часов"));
        assert_eq!(hours(2_400).as_deref(), Some("40 минут"));
        assert_eq!(hours(61).as_deref(), Some("1 минута"));
        assert_eq!(hours(10).as_deref(), Some("1 минута"));
        assert!(hours(u64::MAX).is_some());
    }

    #[test]
    fn sentence_capitalizes_and_ends_with_period() {
        assert_eq!(sentence("нет соединения"), "Нет соединения.");
        assert_eq!(sentence("Уже готово."), "Уже готово.");
        assert_eq!(sentence("  "), "");
    }
}
