//! CopyTrackName: строка «Исполнитель — Название» или свой формат для
//! буфера обмена. В сеть не ходит.

use crate::api::Track;

/// Формат по умолчанию.
pub const DEFAULT_FORMAT: &str = "{artist} — {title}";
/// Длиннее формат не нужен: это одна строка.
const MAX_FORMAT_CHARS: usize = 200;

/// Формат не длиннее [`MAX_FORMAT_CHARS`] символов.
pub fn limit(format: &str) -> String {
    format.chars().take(MAX_FORMAT_CHARS).collect()
}

/// Текст для буфера: `{artist}` — исполнители через запятую, `{title}` —
/// название с версией, `{album}` — альбом. Остальное — как есть. Пустой
/// формат — [`DEFAULT_FORMAT`].
pub fn format(format: &str, track: &Track) -> String {
    let format = if format.trim().is_empty() {
        DEFAULT_FORMAT
    } else {
        format
    };
    // Подстановка за один проход: название с «{artist}» внутри не
    // раскрывается второй раз.
    let mut out = String::with_capacity(format.len() + 32);
    let mut rest = format;
    while let Some(open) = rest.find('{') {
        let (before, tail) = rest.split_at(open);
        out.push_str(before);
        let value = [
            ("{artist}", track.artists_text()),
            ("{title}", track.display_title()),
            ("{album}", track.album_title.clone().unwrap_or_default()),
        ]
        .into_iter()
        .find(|(key, _)| tail.starts_with(key));
        match value {
            Some((key, value)) => {
                out.push_str(&value);
                rest = tail.get(key.len()..).unwrap_or_default();
            }
            None => {
                out.push('{');
                rest = tail.get(1..).unwrap_or_default();
            }
        }
    }
    out.push_str(rest);
    out.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{ArtistRef, Warning};

    fn track() -> Track {
        Track {
            id: "1".to_owned(),
            title: Some("Кукла колдуна {title}".to_owned()),
            version: Some("Live".to_owned()),
            artists: vec![
                ArtistRef {
                    id: None,
                    name: "Король и Шут".to_owned(),
                },
                ArtistRef {
                    id: None,
                    name: "Оркестр".to_owned(),
                },
            ],
            album_id: None,
            album_title: Some("Акустический альбом".to_owned()),
            duration: None,
            available: true,
            cover_uri: None,
            warning: Warning::None,
        }
    }

    #[test]
    fn default_and_custom_formats() {
        let track = track();
        assert_eq!(
            format("", &track),
            "Король и Шут, Оркестр — Кукла колдуна {title} (Live)"
        );
        assert_eq!(
            format("{title} / {album} {unknown} {", &track),
            "Кукла колдуна {title} (Live) / Акустический альбом {unknown} {"
        );
        let nobody = Track {
            artists: Vec::new(),
            ..track
        };
        assert_eq!(format("{artist}", &nobody), "Неизвестный исполнитель");
    }

    #[test]
    fn format_is_limited() {
        assert_eq!(limit(&"я".repeat(500)).chars().count(), 200);
    }
}
