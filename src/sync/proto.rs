//! Сообщения связи (§4 спеки): JSON с полем `t`. Разбор мягкий: неизвестный
//! `t` и битые поля дают «пропустить», а не обрыв связи.

use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::api::{ArtistRef, Loudness, Track, Warning};

/// Длина идентификатора устройства: 32 символа hex.
pub const DEVICE_ID_LEN: usize = 32;
const MAX_TEXT_CHARS: usize = 512;
const MAX_ARTISTS: usize = 20;
const MAX_DURATION_MS: u64 = 86_400_000;
const COVER_HOSTS: [&str; 3] = [
    "avatars.yandex.net/",
    "avatars.mds.yandex.net/",
    "music-avatars.music.yandex.net/",
];

/// Идентификатор устройства: 32 hex-символа в нижнем регистре.
pub fn is_device_id(id: &str) -> bool {
    id.len() == DEVICE_ID_LEN && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Причина закрытия связи.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ByeReason {
    User,
    Shutdown,
    Busy,
    Version,
    Account,
    Auth,
    Rejected,
    /// Причина, которую эта версия не знает.
    #[serde(other)]
    Unknown,
}

impl fmt::Display for ByeReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::User => "устройство отключено пользователем",
            Self::Shutdown => "устройство закрыло программу или выключило функцию",
            Self::Busy => "уже связано с другим устройством",
            Self::Version => "версии программы несовместимы, обновите Plastinka",
            Self::Account => "другой аккаунт Яндекса",
            Self::Auth => "устройство не узнано",
            Self::Rejected => "сопряжение отменено",
            Self::Unknown => "причина неизвестна",
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Next,
    Previous,
    Shuffle,
    Repeat,
    Dislike,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireArtist {
    pub id: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WireWarning {
    Explicit,
    Clean,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireLoudness {
    pub integrated: f64,
    pub true_peak: f64,
}

/// Трек протокола (§4.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireTrack {
    pub id: String,
    pub album_id: Option<String>,
    pub album_title: Option<String>,
    pub title: Option<String>,
    pub version: Option<String>,
    #[serde(default)]
    pub artists: Vec<WireArtist>,
    pub duration_ms: Option<u64>,
    pub cover_uri: Option<String>,
    #[serde(default)]
    pub available: bool,
    pub warning: Option<WireWarning>,
    pub loudness: Option<WireLoudness>,
}

fn is_track_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn is_numeric_id(id: &str) -> bool {
    (1..=20).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit())
}

fn text_ok(text: &Option<String>) -> bool {
    text.as_ref()
        .is_none_or(|text| text.chars().count() <= MAX_TEXT_CHARS)
}

fn cut(text: &str) -> String {
    text.chars().take(MAX_TEXT_CHARS).collect()
}

/// Шаблон обложки допустим: только хосты аватарок Яндекса, есть `%%`,
/// только видимые ASCII-символы.
fn cover_ok(cover: &str) -> bool {
    COVER_HOSTS.iter().any(|host| cover.starts_with(host))
        && cover.contains("%%")
        && cover.bytes().all(|b| b.is_ascii_graphic())
}

impl WireTrack {
    /// Проверки §4.1. `coverUri` с чужого хоста становится `None`; всё
    /// остальное неверное — `false`: сообщение со таким треком пропускают.
    pub fn validate(&mut self) -> bool {
        if self.cover_uri.as_deref().is_some_and(|c| !cover_ok(c)) {
            self.cover_uri = None;
        }
        is_track_id(&self.id)
            && self.album_id.as_deref().is_none_or(is_numeric_id)
            && text_ok(&self.album_title)
            && text_ok(&self.title)
            && text_ok(&self.version)
            && self.artists.len() <= MAX_ARTISTS
            && self.artists.iter().all(|artist| {
                artist.id.as_deref().is_none_or(is_numeric_id) && text_ok(&artist.name)
            })
            && self.duration_ms.is_none_or(|ms| ms <= MAX_DURATION_MS)
            && self
                .loudness
                .is_none_or(|l| l.integrated.is_finite() && l.true_peak.is_finite())
    }

    /// Из трека каталога. Строки длиннее предела обрезаются, лишние
    /// исполнители отбрасываются, чтобы получатель не пропустил состояние.
    pub fn from_track(track: &Track) -> Self {
        let mut wire = Self {
            id: track.id.clone(),
            album_id: track.album_id.clone(),
            album_title: track.album_title.as_deref().map(cut),
            title: track.title.as_deref().map(cut),
            version: track.version.as_deref().map(cut),
            artists: track
                .artists
                .iter()
                .take(MAX_ARTISTS)
                .map(|artist| WireArtist {
                    id: artist.id.clone(),
                    name: Some(cut(&artist.name)),
                })
                .collect(),
            duration_ms: track
                .duration
                .map(|d| u64::try_from(d.as_millis()).unwrap_or(MAX_DURATION_MS)),
            cover_uri: track.cover_uri.clone(),
            available: track.available,
            warning: match track.warning {
                Warning::None => None,
                Warning::Explicit => Some(WireWarning::Explicit),
                Warning::Clean => Some(WireWarning::Clean),
            },
            loudness: track.loudness.map(|l| WireLoudness {
                integrated: l.integrated,
                true_peak: l.true_peak,
            }),
        };
        // Наружу уходит только то, что получатель примет.
        if wire.cover_uri.as_deref().is_some_and(|c| !cover_ok(c)) {
            wire.cover_uri = None;
        }
        wire
    }

    /// В трек каталога; тексты и поля, которых в протоколе нет, пустые.
    pub fn to_track(&self) -> Track {
        Track {
            id: self.id.clone(),
            title: self.title.clone(),
            version: self.version.clone(),
            artists: self
                .artists
                .iter()
                .filter_map(|artist| {
                    Some(ArtistRef {
                        id: artist.id.clone(),
                        name: artist.name.clone()?,
                    })
                })
                .collect(),
            album_id: self.album_id.clone(),
            album_title: self.album_title.clone(),
            duration: self.duration_ms.map(Duration::from_millis),
            available: self.available,
            cover_uri: self.cover_uri.clone(),
            warning: match self.warning {
                None => Warning::None,
                Some(WireWarning::Explicit) => Warning::Explicit,
                Some(WireWarning::Clean) => Warning::Clean,
            },
            loudness: self.loudness.map(|l| Loudness {
                integrated: l.integrated,
                true_peak: l.true_peak,
            }),
            lyrics: None,
        }
    }
}

/// Состояние воспроизведения (§4.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub rev: u64,
    pub by: String,
    pub owner: String,
    pub track: Option<WireTrack>,
    pub playing: bool,
    pub position_ms: u64,
    /// Часы отправителя.
    pub at: i64,
    pub next: Option<WireTrack>,
    #[serde(default)]
    pub shuffle: bool,
    #[serde(default)]
    pub repeat: Repeat,
    #[serde(default)]
    pub wave: bool,
    #[serde(default)]
    pub initial: bool,
    #[serde(default)]
    pub join: bool,
}

impl State {
    fn validate(&mut self) -> bool {
        is_device_id(&self.by)
            && is_device_id(&self.owner)
            && self.track.as_mut().is_none_or(WireTrack::validate)
            && self.next.as_mut().is_none_or(WireTrack::validate)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tick {
    pub rev: u64,
    pub position_ms: u64,
    pub at: i64,
    #[serde(default)]
    pub buffering: bool,
}

/// Сообщение шифрованного канала.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum Message {
    Auth { mac: String },
    Confirm,
    Bye { reason: ByeReason },
    Ping { n: u64, t0: i64 },
    Pong { n: u64, t0: i64, t1: i64 },
    State(Box<State>),
    Tick(Tick),
    Command { action: Action },
    Liked { track: String, liked: bool },
}

/// Разбор открытого текста кадра. `None` — пропустить: неизвестный `t`,
/// битый JSON, неверные поля или не прошедший проверку трек.
pub fn parse(bytes: &[u8]) -> Option<Message> {
    let mut message: Message = serde_json::from_slice(bytes).ok()?;
    let valid = match &mut message {
        Message::State(state) => state.validate(),
        Message::Liked { track, .. } => is_track_id(track),
        _ => true,
    };
    valid.then_some(message)
}

/// JSON сообщения.
pub fn encode(message: &Message) -> Option<Vec<u8>> {
    serde_json::to_vec(message).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const ID_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn sample_track() -> Track {
        Track {
            id: "12345".into(),
            title: Some("Название".into()),
            version: Some("Live".into()),
            artists: vec![ArtistRef {
                id: Some("9".into()),
                name: "Исполнитель".into(),
            }],
            album_id: Some("678".into()),
            album_title: Some("Альбом".into()),
            duration: Some(Duration::from_millis(215_000)),
            available: true,
            cover_uri: Some("avatars.yandex.net/get-music-content/1/abc/%%".into()),
            warning: Warning::Explicit,
            loudness: Some(Loudness {
                integrated: -9.1,
                true_peak: 0.3,
            }),
            lyrics: None,
        }
    }

    fn state_json(track: &str) -> String {
        format!(
            r#"{{"t":"state","rev":42,"by":"{ID_A}","owner":"{ID_B}","track":{track},
            "playing":true,"positionMs":61234,"at":123456789,"next":null,
            "shuffle":false,"repeat":"off","wave":false,"initial":false,"join":false}}"#
        )
    }

    #[test]
    fn track_roundtrip() {
        let track = sample_track();
        let wire = WireTrack::from_track(&track);
        assert_eq!(wire.to_track(), track);
        let json = serde_json::to_string(&wire).unwrap();
        assert!(json.contains("\"albumTitle\":\"Альбом\""));
        assert!(json.contains("\"truePeak\":0.3"));
        assert!(json.contains("\"warning\":\"explicit\""));
    }

    #[test]
    fn spec_state_parses() {
        let json = state_json(
            r#"{"id":"12345","albumId":"678","albumTitle":"Альбом","title":"Название","version":"Live",
            "artists":[{"id":"9","name":"Исполнитель"}],"durationMs":215000,
            "coverUri":"avatars.yandex.net/get-music-content/x/%%","available":true,
            "warning":"explicit","loudness":{"integrated":-9.1,"truePeak":0.3}}"#,
        );
        let Some(Message::State(state)) = parse(json.as_bytes()) else {
            panic!("состояние должно разобраться");
        };
        assert_eq!(state.rev, 42);
        assert_eq!(state.position_ms, 61234);
        assert_eq!(state.track.unwrap().to_track(), sample_track_like_spec());
    }

    fn sample_track_like_spec() -> Track {
        let mut track = sample_track();
        track.cover_uri = Some("avatars.yandex.net/get-music-content/x/%%".into());
        track
    }

    #[test]
    fn unknown_and_broken_are_skipped() {
        assert_eq!(parse(br#"{"t":"future","x":1}"#), None);
        assert_eq!(parse(b"not json"), None);
        assert_eq!(parse(br#"{"t":"ping","n":"x","t0":1}"#), None);
        assert_eq!(parse(br#"{"t":"command","action":"explode"}"#), None);
        // Лишние поля игнорируются.
        assert_eq!(
            parse(br#"{"t":"ping","n":1,"t0":2,"extra":true}"#),
            Some(Message::Ping { n: 1, t0: 2 })
        );
        assert_eq!(
            parse(br#"{"t":"bye","reason":"new-thing"}"#),
            Some(Message::Bye {
                reason: ByeReason::Unknown
            })
        );
    }

    #[test]
    fn track_checks() {
        let bad = |track: &str| parse(state_json(track).as_bytes());
        assert!(bad(r#"{"id":"1","available":true}"#).is_some());
        for track in [
            r#"{"id":"","available":true}"#,
            r#"{"id":"a b","available":true}"#,
            r#"{"id":"1","albumId":"12x"}"#,
            r#"{"id":"1","durationMs":86400001}"#,
            r#"{"id":"1","artists":[{"id":"abc","name":"x"}]}"#,
            r#"{"id":"1","warning":"weird"}"#,
        ] {
            assert!(bad(track).is_none(), "{track}");
        }
        let long = "я".repeat(513);
        assert!(bad(&format!(r#"{{"id":"1","title":"{long}"}}"#)).is_none());
        let ok = "я".repeat(512);
        assert!(bad(&format!(r#"{{"id":"1","title":"{ok}"}}"#)).is_some());
        let artists = vec![r#"{"id":null,"name":"a"}"#; 21].join(",");
        assert!(bad(&format!(r#"{{"id":"1","artists":[{artists}]}}"#)).is_none());
        let artists = vec![r#"{"id":null,"name":"a"}"#; 20].join(",");
        assert!(bad(&format!(r#"{{"id":"1","artists":[{artists}]}}"#)).is_some());
    }

    #[test]
    fn foreign_cover_becomes_none() {
        let take = |cover: &str| {
            let json = state_json(&format!(r#"{{"id":"1","coverUri":"{cover}"}}"#));
            let Some(Message::State(state)) = parse(json.as_bytes()) else {
                panic!("состояние должно разобраться");
            };
            state.track.unwrap().cover_uri
        };
        assert!(take("avatars.yandex.net/a/%%").is_some());
        assert!(take("avatars.mds.yandex.net/a/%%").is_some());
        assert!(take("music-avatars.music.yandex.net/a/%%").is_some());
        assert_eq!(take("evil.example/avatars.yandex.net/%%"), None);
        assert_eq!(take("avatars.yandex.net.evil.com/a/%%"), None);
        assert_eq!(take("https://avatars.yandex.net/a/%%"), None);
        assert_eq!(take("avatars.yandex.net/a/200x200"), None);
    }

    #[test]
    fn bad_device_ids_skip_state() {
        let json = state_json("null").replace(ID_A, "short");
        assert_eq!(parse(json.as_bytes()), None);
    }

    #[test]
    fn outgoing_track_fits_limits() {
        let mut track = sample_track();
        track.title = Some("ы".repeat(2000));
        track.cover_uri = Some("evil/%%".into());
        track.artists = (0..30)
            .map(|i| ArtistRef {
                id: None,
                name: format!("a{i}"),
            })
            .collect();
        let wire = WireTrack::from_track(&track);
        assert_eq!(wire.cover_uri, None);
        assert_eq!(wire.artists.len(), 20);
        let mut again = wire.clone();
        assert!(again.validate());
    }

    #[test]
    fn messages_roundtrip() {
        for message in [
            Message::Confirm,
            Message::Auth { mac: "AAAA".into() },
            Message::Ping { n: 1, t0: 5 },
            Message::Pong { n: 1, t0: 5, t1: 9 },
            Message::Tick(Tick {
                rev: 1,
                position_ms: 2,
                at: 3,
                buffering: true,
            }),
            Message::Command {
                action: Action::Dislike,
            },
            Message::Liked {
                track: "12345".into(),
                liked: true,
            },
            Message::Bye {
                reason: ByeReason::Auth,
            },
        ] {
            let bytes = encode(&message).unwrap();
            assert_eq!(parse(&bytes), Some(message));
        }
        let tick = encode(&Message::Tick(Tick {
            rev: 42,
            position_ms: 62240,
            at: 123457795,
            buffering: false,
        }))
        .unwrap();
        assert_eq!(
            String::from_utf8(tick).unwrap(),
            r#"{"t":"tick","rev":42,"positionMs":62240,"at":123457795,"buffering":false}"#
        );
    }

    #[test]
    fn device_ids() {
        assert!(is_device_id(ID_A));
        assert!(!is_device_id(&ID_A.to_uppercase()));
        assert!(!is_device_id("abc"));
    }
}
