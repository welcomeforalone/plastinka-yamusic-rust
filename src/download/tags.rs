//! Теги скачанного файла через lofty: название, исполнители, альбом, год,
//! номер трека, обложка. Теги пишутся в память до сохранения на диск: не
//! вышло — файл сохраняется без них, а не портится.

use std::io::Cursor;

use lofty::config::WriteOptions;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::tag::{Accessor, ItemKey, Tag, TagExt, TagType};

use super::Format;

/// Обложка: байты и тип картинки.
#[derive(Debug, Clone)]
pub struct Cover {
    pub data: Vec<u8>,
    pub mime: MimeType,
}

impl Cover {
    /// Картинка JPEG или PNG; остальное в тег не кладём.
    pub fn from_bytes(data: Vec<u8>) -> Option<Self> {
        let mime = if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
            MimeType::Jpeg
        } else if data.starts_with(b"\x89PNG") {
            MimeType::Png
        } else {
            return None;
        };
        Some(Self { data, mime })
    }
}

/// Что записать в теги.
#[derive(Debug, Clone, Default)]
pub struct Tags {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub year: Option<u32>,
    pub number: Option<u32>,
    pub cover: Option<Cover>,
}

fn tag_type(format: Format) -> TagType {
    match format {
        Format::Mp3 | Format::Aac => TagType::Id3v2,
        Format::M4a => TagType::Mp4Ilst,
        Format::Flac => TagType::VorbisComments,
    }
}

/// Тег нужного вида для `format`.
pub fn build(format: Format, tags: &Tags) -> Tag {
    let mut tag = Tag::new(tag_type(format));
    tag.set_title(tags.title.clone());
    tag.set_artist(tags.artist.clone());
    if let Some(album) = &tags.album {
        tag.set_album(album.clone());
    }
    if let Some(number) = tags.number {
        tag.set_track(number);
    }
    if let Some(year) = tags.year {
        tag.insert_text(ItemKey::RecordingDate, year.to_string());
    }
    if let Some(cover) = &tags.cover {
        tag.push_picture(
            Picture::unchecked(cover.data.clone())
                .pic_type(PictureType::CoverFront)
                .mime_type(cover.mime.clone())
                .build(),
        );
    }
    tag
}

/// Файл с тегами и признак «теги записаны». Не получилось — исходные байты.
pub fn apply(format: Format, data: Vec<u8>, tags: &Tags) -> (Vec<u8>, bool) {
    let tag = build(format, tags);
    let options = WriteOptions::default().use_id3v23(true);
    let mut cursor = Cursor::new(data.clone());
    match tag.save_to(&mut cursor, options) {
        Ok(()) => (cursor.into_inner(), true),
        Err(_) => (data, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lofty::file::TaggedFileExt;
    use lofty::probe::Probe;

    fn sample() -> Tags {
        Tags {
            title: "Дурак и молния".to_owned(),
            artist: "Король и Шут".to_owned(),
            album: Some("Камнем по голове".to_owned()),
            year: Some(1996),
            number: Some(13),
            cover: Cover::from_bytes([&[0xFF, 0xD8, 0xFF, 0xE0][..], &[0; 64][..]].concat()),
        }
    }

    fn mp3() -> Vec<u8> {
        let mut data = Vec::new();
        for _ in 0..20 {
            data.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x00]);
            data.resize(data.len() + 413, 0);
        }
        data
    }

    fn read_back(data: Vec<u8>) -> Tag {
        let file = Probe::new(Cursor::new(data))
            .guess_file_type()
            .unwrap()
            .read()
            .unwrap();
        file.primary_tag().unwrap().clone()
    }

    #[test]
    fn mp3_gets_all_tags() {
        let (data, tagged) = apply(Format::Mp3, mp3(), &sample());
        assert!(tagged);
        assert!(data.starts_with(b"ID3"));
        let tag = read_back(data);
        assert_eq!(tag.title().as_deref(), Some("Дурак и молния"));
        assert_eq!(tag.artist().as_deref(), Some("Король и Шут"));
        assert_eq!(tag.album().as_deref(), Some("Камнем по голове"));
        assert_eq!(tag.track(), Some(13));
        assert_eq!(tag.pictures().len(), 1);
        assert_eq!(
            tag.get_string(ItemKey::RecordingDate)
                .map(|date| date.get(..4)),
            Some(Some("1996"))
        );
    }

    #[test]
    fn flac_gets_tags_and_keeps_frames() {
        let info = super::super::flac::StreamInfo::from_params(44_100, 2, 16, 0)
            .to_bytes()
            .unwrap();
        let mut data = super::super::flac::header(&info);
        let frames = vec![0xFF, 0xF8, 0xC9, 0x18, 1, 2, 3, 4, 5, 6, 7, 8];
        data.extend_from_slice(&frames);
        let (tagged_data, tagged) = apply(Format::Flac, data, &sample());
        assert!(tagged);
        assert!(tagged_data.starts_with(b"fLaC"));
        assert!(tagged_data.ends_with(&frames));
        let tag = read_back(tagged_data);
        assert_eq!(tag.title().as_deref(), Some("Дурак и молния"));
        assert_eq!(tag.pictures().len(), 1);
    }

    #[test]
    fn unknown_data_stays_untouched() {
        let junk = "это не звук".repeat(10).into_bytes();
        let (data, tagged) = apply(Format::Mp3, junk.clone(), &sample());
        assert!(!tagged || data.starts_with(b"ID3"));
        let (data, tagged) = apply(Format::Flac, junk.clone(), &sample());
        assert!(!tagged);
        assert_eq!(data, junk);
    }

    #[test]
    fn only_images_become_covers() {
        assert!(Cover::from_bytes(b"<html>".to_vec()).is_none());
        assert!(Cover::from_bytes(b"\x89PNG\r\n".to_vec()).is_some());
    }
}
