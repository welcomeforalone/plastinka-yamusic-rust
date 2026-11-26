//! Имя файла скачанного трека: «Исполнитель — Название.расширение».

/// Символы, которые нельзя в имени файла в Windows (в Linux запрещён только
/// «/», но имена должны открываться на обеих системах).
const FORBIDDEN: [char; 9] = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
/// Предел имени без расширения: в символах и в байтах UTF-8. Вместе с папкой
/// и расширением путь остаётся в пределах 260 знаков Windows и 255 байт имени
/// в Linux.
const MAX_CHARS: usize = 120;
const MAX_BYTES: usize = 200;
/// Разделитель исполнителя и названия.
const SEPARATOR: &str = " — ";

/// Часть имени без запрещённых символов: они и управляющие знаки заменены на
/// «_», подряд идущие пробелы слиты.
fn clean_part(text: &str) -> String {
    let replaced: String = text
        .chars()
        .map(|ch| {
            if ch.is_control() || FORBIDDEN.contains(&ch) {
                '_'
            } else {
                ch
            }
        })
        .collect();
    replaced.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Обрезает по символам и байтам, не разрывая символ, и убирает пробелы и
/// точки в конце (Windows их не принимает).
fn limit(text: &str) -> String {
    let mut bytes = 0;
    let cut: String = text
        .chars()
        .take(MAX_CHARS)
        .take_while(|ch| {
            bytes += ch.len_utf8();
            bytes <= MAX_BYTES
        })
        .collect();
    cut.trim_end_matches([' ', '.']).to_owned()
}

/// Имя без расширения: «Исполнитель — Название». Пустое — «Трек».
pub fn file_stem(artists: &str, title: &str) -> String {
    let joined = format!("{}{SEPARATOR}{}", clean_part(artists), clean_part(title));
    let stem = limit(joined.trim_start_matches([' ', '.']));
    // Всегда есть « — », так что зарезервированное имя Windows («CON») не выйдет.
    if stem.is_empty() {
        "Трек".to_owned()
    } else {
        stem
    }
}

/// Полное имя: `stem` и расширение без точки.
pub fn file_name(stem: &str, extension: &str) -> String {
    format!("{stem}.{extension}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_artist_and_title() {
        assert_eq!(
            file_stem("Король и Шут", "Дурак и молния"),
            "Король и Шут — Дурак и молния"
        );
        assert_eq!(file_name("A — B", "mp3"), "A — B.mp3");
    }

    #[test]
    fn replaces_forbidden_characters() {
        assert_eq!(file_stem("AC/DC", "Who Made Who?"), "AC_DC — Who Made Who_");
        assert_eq!(
            file_stem("A", r#"a<b>c:d"e/f\g|h?i*j"#),
            "A — a_b_c_d_e_f_g_h_i_j"
        );
        // Управляющие знаки и лишние пробелы.
        assert_eq!(file_stem("A\u{0}\n", "  B   C "), "A__ — B C");
    }

    #[test]
    fn trims_trailing_dots_and_spaces() {
        assert_eq!(file_stem("A", "B..."), "A — B");
        assert_eq!(file_stem("A", "B . "), "A — B");
    }

    #[test]
    fn long_names_are_cut_on_char_boundary() {
        let long = "ж".repeat(500);
        let stem = file_stem("Исполнитель", &long);
        assert!(stem.chars().count() <= MAX_CHARS);
        assert!(stem.len() <= MAX_BYTES);
        assert!(stem.starts_with("Исполнитель — ж"));
        // Лишь латиница: упирается в предел символов, а не байтов.
        let ascii = file_stem("A", &"x".repeat(500));
        assert_eq!(ascii.chars().count(), MAX_CHARS);
    }

    #[test]
    fn empty_input_still_gives_a_name() {
        let stem = file_stem("", "");
        assert!(!stem.is_empty());
        assert!(!stem.ends_with(' '));
    }
}
