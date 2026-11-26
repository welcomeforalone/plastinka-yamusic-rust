//! Данные трека для тегов скачанного файла: год и номер в альбоме. В списки
//! они не попадают, поэтому запрашиваются один раз при скачивании.

use reqwest::StatusCode;
use serde_json::Value;

use super::tracks::{plausible_id, post_tracks};
use super::{ApiError, check_status};

/// Год выхода и номер трека в альбоме (с единицы). Чего сервис не сказал —
/// `None`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TrackMeta {
    pub year: Option<u32>,
    pub number: Option<u32>,
}

/// Год и номер трека `id` из `POST /tracks`.
pub async fn fetch_track_meta(
    http: &reqwest::Client,
    token: &str,
    id: &str,
) -> Result<TrackMeta, ApiError> {
    // С фальшивым токеном в сеть не ходим.
    if crate::storage::test_hooks_active() || !plausible_id(id) {
        return Ok(TrackMeta::default());
    }
    let (status, body) = post_tracks(http, token, &[id.to_owned()]).await?;
    parse_meta(status, &body)
}

fn parse_meta(status: StatusCode, body: &str) -> Result<TrackMeta, ApiError> {
    check_status(status)?;
    let envelope: Value = serde_json::from_str(body).map_err(|_| ApiError::InvalidResponse)?;
    let album = envelope
        .get("result")
        .and_then(Value::as_array)
        .and_then(|tracks| tracks.first())
        .and_then(|track| track.get("albums"))
        .and_then(Value::as_array)
        .and_then(|albums| albums.first());
    let number = |value: Option<&Value>| {
        value
            .and_then(Value::as_u64)
            .and_then(|number| u32::try_from(number).ok())
            .filter(|number| *number > 0)
    };
    Ok(TrackMeta {
        // Годы вне разумных границ в тег не пишем.
        year: number(album.and_then(|album| album.get("year")))
            .filter(|year| (1000..=9999).contains(year)),
        number: number(
            album
                .and_then(|album| album.get("trackPosition"))
                .and_then(|position| position.get("index")),
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_year_and_position() {
        let body = r#"{"result":[{"id":"1","albums":[{"id":2,"year":1996,
            "trackPosition":{"volume":1,"index":13}}]}]}"#;
        assert_eq!(
            parse_meta(StatusCode::OK, body).unwrap(),
            TrackMeta {
                year: Some(1996),
                number: Some(13)
            }
        );
    }

    #[test]
    fn missing_fields_are_none() {
        let body = r#"{"result":[{"id":"1","albums":[{"id":2,"year":0}]}]}"#;
        assert_eq!(
            parse_meta(StatusCode::OK, body).unwrap(),
            TrackMeta::default()
        );
        assert_eq!(
            parse_meta(StatusCode::OK, r#"{"result":[]}"#).unwrap(),
            TrackMeta::default()
        );
        assert!(parse_meta(StatusCode::OK, "<html>").is_err());
        assert!(parse_meta(StatusCode::SERVICE_UNAVAILABLE, "").is_err());
    }
}
