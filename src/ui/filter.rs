//! Поиск трека в списке «Мне нравится»: фильтр по названию и исполнителю,
//! прямо в памяти, без запросов к API.

#[cfg(test)]
use crate::api::ArtistRef;
use crate::api::Track;

/// Строка трека, в которой ищем: название с версией и исполнители, в
/// нижнем регистре и с «е» вместо «ё».
pub fn haystack(track: &Track) -> String {
    let mut text = track.display_title();
    for artist in &track.artists {
        text.push(' ');
        text.push_str(&artist.name);
    }
    normalize(&text)
}

/// Нижний регистр, «ё» → «е»: «Ёлка» находится по «елка».
pub fn normalize(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .map(|ch| if ch == 'ё' { 'е' } else { ch })
        .collect()
}

/// Номера треков, у которых в строке есть все слова запроса, по порядку.
/// `None` — запрос пустой, показываем всё.
pub fn matching(haystacks: &[String], query: &str) -> Option<Vec<usize>> {
    let query = normalize(query);
    let words: Vec<&str> = query.split_whitespace().collect();
    if words.is_empty() {
        return None;
    }
    Some(
        haystacks
            .iter()
            .enumerate()
            .filter(|(_, text)| words.iter().all(|word| text.contains(word)))
            .map(|(index, _)| index)
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(title: &str, version: Option<&str>, artists: &[&str]) -> Track {
        Track {
            id: "1".to_owned(),
            title: Some(title.to_owned()),
            version: version.map(str::to_owned),
            artists: artists
                .iter()
                .map(|artist| ArtistRef {
                    id: None,
                    name: (*artist).to_owned(),
                })
                .collect(),
            album_id: None,
            album_title: None,
            duration: None,
            available: true,
            cover_uri: None,
            warning: crate::api::Warning::None,
        }
    }

    #[test]
    fn finds_by_title_artist_and_words_in_any_order() {
        let haystacks: Vec<String> = [
            track("Karma Police", None, &["Radiohead"]),
            track("Ёлка", Some("Live"), &["Кто-то"]),
            track("Bitter Sweet Symphony", Some("Radio Edit"), &["The Verve"]),
        ]
        .iter()
        .map(haystack)
        .collect();
        assert_eq!(matching(&haystacks, ""), None);
        assert_eq!(matching(&haystacks, "   "), None);
        assert_eq!(matching(&haystacks, "karma"), Some(vec![0]));
        assert_eq!(matching(&haystacks, "RADIO"), Some(vec![0, 2]));
        assert_eq!(matching(&haystacks, "police radiohead"), Some(vec![0]));
        assert_eq!(matching(&haystacks, "елка live"), Some(vec![1]));
        assert_eq!(matching(&haystacks, "ЁЛКА"), Some(vec![1]));
        assert_eq!(matching(&haystacks, "нет такого"), Some(vec![]));
    }
}
