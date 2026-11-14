//! Позиция для MPRIS. Клиенты (апплеты KDE и GNOME, `playerctl`) читают
//! свойство `Position` когда хотят, а сигнала о его изменении по спецификации
//! нет. Поэтому позиция считается в момент чтения: последняя сообщённая плюс
//! прошедшее время, пока трек играет. Чистая логика: время приходит
//! параметром.

use std::time::{Duration, Instant};

use super::{Playback, Status};

/// Что играет с точки зрения системного плеера.
#[derive(Debug, Clone, Copy)]
pub struct Timeline {
    status: Status,
    /// Позиция в момент `at`.
    position: Duration,
    at: Instant,
    /// Длина трека: дальше неё позиция не уходит.
    length: Option<Duration>,
}

impl Timeline {
    pub fn new(now: Instant) -> Self {
        Self {
            status: Status::Stopped,
            position: Duration::ZERO,
            at: now,
            length: None,
        }
    }

    pub fn status(&self) -> Status {
        self.status
    }

    pub fn length(&self) -> Option<Duration> {
        self.length
    }

    /// Длина нового трека (`None` — неизвестна).
    pub fn set_length(&mut self, length: Option<Duration>) {
        self.length = length;
    }

    /// Новое состояние: старт, пауза, продолжение, перемотка, остановка.
    pub fn set(&mut self, playback: Playback, now: Instant) {
        self.status = playback.status;
        self.position = match playback.status {
            Status::Stopped => Duration::ZERO,
            Status::Paused | Status::Playing => playback.position,
        };
        self.at = now;
    }

    /// Позиция на момент `now`: на ходу течёт сама, на паузе стоит.
    pub fn position(&self, now: Instant) -> Duration {
        let position = match self.status {
            Status::Stopped => return Duration::ZERO,
            Status::Paused => self.position,
            Status::Playing => self.position + now.saturating_duration_since(self.at),
        };
        self.length.map_or(position, |length| position.min(length))
    }
}

/// Корень путей `mpris:trackid`. Свой путь, не `/org/mpris`: спецификация
/// оставляет его за собой.
const TRACK_ROOT: &str = "/org/plastinka/track/";

/// Путь D-Bus для `mpris:trackid` из номера трека. В пути разрешены только
/// латинские буквы, цифры и `_`, поэтому остальное (`:` у номеров вида
/// `123:456`, `-` у UUID) заменяется на `_`. У разных треков пути разные:
/// по ним клиенты понимают, что трек сменился, и проверяют `SetPosition`.
pub fn track_path(id: &str) -> String {
    let tail: String = id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if tail.is_empty() {
        format!("{TRACK_ROOT}_")
    } else {
        format!("{TRACK_ROOT}{tail}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playing(seconds: u64) -> Playback {
        Playback {
            status: Status::Playing,
            position: Duration::from_secs(seconds),
        }
    }

    fn paused(seconds: u64) -> Playback {
        Playback {
            status: Status::Paused,
            position: Duration::from_secs(seconds),
        }
    }

    #[test]
    fn position_runs_while_playing() {
        let start = Instant::now();
        let mut timeline = Timeline::new(start);
        timeline.set_length(Some(Duration::from_secs(200)));
        timeline.set(playing(10), start);
        assert_eq!(timeline.position(start), Duration::from_secs(10));
        assert_eq!(
            timeline.position(start + Duration::from_millis(2500)),
            Duration::from_millis(12_500)
        );
    }

    #[test]
    fn position_stands_on_pause() {
        let start = Instant::now();
        let mut timeline = Timeline::new(start);
        timeline.set(playing(0), start);
        // Пауза на 30-й секунде: дальше позиция не бежит.
        timeline.set(paused(30), start + Duration::from_secs(30));
        assert_eq!(
            timeline.position(start + Duration::from_secs(90)),
            Duration::from_secs(30)
        );
        // Продолжение: снова идёт от 30, а не от нуля.
        timeline.set(playing(30), start + Duration::from_secs(100));
        assert_eq!(
            timeline.position(start + Duration::from_secs(105)),
            Duration::from_secs(35)
        );
    }

    #[test]
    fn seek_moves_position() {
        let start = Instant::now();
        let mut timeline = Timeline::new(start);
        timeline.set(playing(0), start);
        timeline.set(playing(120), start + Duration::from_secs(5));
        assert_eq!(
            timeline.position(start + Duration::from_secs(6)),
            Duration::from_secs(121)
        );
        // Перемотка на паузе тоже двигает позицию.
        timeline.set(paused(40), start + Duration::from_secs(7));
        assert_eq!(
            timeline.position(start + Duration::from_secs(60)),
            Duration::from_secs(40)
        );
    }

    #[test]
    fn position_stops_at_track_end() {
        let start = Instant::now();
        let mut timeline = Timeline::new(start);
        timeline.set_length(Some(Duration::from_secs(60)));
        assert_eq!(timeline.length(), Some(Duration::from_secs(60)));
        timeline.set(playing(55), start);
        assert_eq!(
            timeline.position(start + Duration::from_secs(30)),
            Duration::from_secs(60)
        );
    }

    #[test]
    fn stopped_is_zero() {
        let start = Instant::now();
        let mut timeline = Timeline::new(start);
        assert_eq!(timeline.position(start), Duration::ZERO);
        timeline.set(playing(50), start);
        timeline.set(
            Playback {
                status: Status::Stopped,
                position: Duration::from_secs(50),
            },
            start,
        );
        assert_eq!(timeline.status(), Status::Stopped);
        assert_eq!(
            timeline.position(start + Duration::from_secs(5)),
            Duration::ZERO
        );
    }

    #[test]
    fn earlier_now_does_not_go_back() {
        let start = Instant::now() + Duration::from_secs(10);
        let mut timeline = Timeline::new(start);
        timeline.set(playing(20), start);
        assert_eq!(
            timeline.position(start - Duration::from_secs(5)),
            Duration::from_secs(20)
        );
    }

    #[test]
    fn track_paths_are_valid_and_distinct() {
        assert_eq!(track_path("12345"), "/org/plastinka/track/12345");
        assert_eq!(track_path("123:456"), "/org/plastinka/track/123_456");
        assert_eq!(track_path("a-b c"), "/org/plastinka/track/a_b_c");
        assert_eq!(track_path("трек"), "/org/plastinka/track/____");
        assert_eq!(track_path(""), "/org/plastinka/track/_");
        assert_ne!(track_path("1"), track_path("2"));
    }
}
