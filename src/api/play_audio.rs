//! Сообщения о прослушивании: `POST /play-audio`, как делает официальный
//! клиент (решение пользователя, 2026-11-12: это согласованная запись в
//! аккаунт, только она; от неё зависит серверная «История»). Крейт
//! `yandex-music` 0.7 описывает поля (`PlayAudioOptions`), но тело запроса у
//! него не реализовано (`todo`), поэтому имена полей формы здесь **по памяти
//! (справка MarshalX), вживую не проверены**. Спека — раздел «История»
//! `docs/api/stage-9.md`.
//!
//! Шлём дважды за трек: когда он зазвучал (сыграно 0 с) и когда сменился или
//! кончился (сколько реально сыграно). Свой файл FckCensor и трек, который не
//! зазвучал, не шлём. Ошибки не мешают игре.

use std::io::Write as _;
use std::time::{Duration, SystemTime};

use reqwest::header::AUTHORIZATION;

use super::rotor::rfc3339_utc;
use super::{API_BASE, ApiError, MUSIC_CLIENT, auth_header, check_status};
use crate::net::NetError;

/// Откуда играем (поле `from`): наш клиент, без личных данных.
const FROM: &str = "plastinka";

/// Одно сообщение о прослушивании.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayReport {
    pub track_id: String,
    pub album_id: Option<String>,
    pub play_id: String,
    pub uid: u64,
    pub length_seconds: Option<u32>,
    pub total_played_seconds: u32,
    pub end_position_seconds: u32,
    /// Когда трек начался.
    pub started: SystemTime,
}

impl PlayReport {
    /// Поля формы `POST /play-audio` (только метаданные, токена здесь нет).
    pub fn form(&self, now: SystemTime) -> Vec<(&'static str, String)> {
        let mut form = vec![
            ("track-id", self.track_id.clone()),
            ("from-cache", "false".to_owned()),
            ("from", FROM.to_owned()),
            ("play-id", self.play_id.clone()),
            ("uid", self.uid.to_string()),
            ("timestamp", rfc3339_utc(self.started)),
            ("client-now", rfc3339_utc(now)),
            (
                "total-played-seconds",
                self.total_played_seconds.to_string(),
            ),
            (
                "end-position-seconds",
                self.end_position_seconds.to_string(),
            ),
        ];
        if let Some(album) = &self.album_id {
            form.push(("album-id", album.clone()));
        }
        if let Some(length) = self.length_seconds {
            form.push(("track-length-seconds", length.to_string()));
        }
        form
    }
}

/// Отправляет сообщение. С тестовым токеном (`PLASTINKA_TEST_TOKEN`, только
/// отладка) в сеть не идёт: одна строка в stderr.
pub async fn send_play_audio(
    http: &reqwest::Client,
    token: &str,
    report: &PlayReport,
) -> Result<(), ApiError> {
    let form = report.form(SystemTime::now());
    #[cfg(debug_assertions)]
    if crate::storage::test_hooks_active() {
        let line: Vec<String> = form
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect();
        let _ = writeln!(std::io::stderr(), "play-audio (тест) {}", line.join("&"));
        return Ok(());
    }
    let response = http
        .post(format!("{API_BASE}/play-audio"))
        .header(AUTHORIZATION, auth_header(token)?)
        .header("X-Yandex-Music-Client", MUSIC_CLIENT)
        .form(&form)
        .send()
        .await
        .map_err(NetError::from)?;
    let status = response.status();
    // Отладочная сборка: ответ сервиса одной строкой, чтобы вживую проверить,
    // принимает ли он отчёт (в ответе нет токена, только `result`).
    #[cfg(debug_assertions)]
    {
        let body = response.text().await.unwrap_or_default();
        let short: String = body.chars().take(200).collect();
        let _ = writeln!(
            std::io::stderr(),
            "play-audio track-id={} played={} → HTTP {}: {short}",
            report.track_id,
            report.total_played_seconds,
            status.as_u16()
        );
    }
    check_status(status)
}

/// Отправка в фоне: ошибка — одна строка в stderr, без токена и адресов.
pub async fn send_quietly(http: reqwest::Client, token: String, report: PlayReport) {
    if let Err(err) = send_play_audio(&http, &token, &report).await {
        let _ = writeln!(
            std::io::stderr(),
            "Предупреждение: не удалось сообщить о прослушивании: {err}"
        );
    }
}

/// Трек, который сейчас считается играющим.
struct Active {
    track_id: String,
    album_id: Option<String>,
    length_seconds: Option<u32>,
    play_id: String,
    started: SystemTime,
    /// Сообщение о начале уже ушло.
    started_sent: bool,
}

/// Правила «слать или не слать». Каждое событие даёт не больше одного
/// сообщения.
#[derive(Default)]
pub struct Tracker {
    active: Option<Active>,
}

impl Tracker {
    /// Запущен трек. `reportable` — слать о нём можно (поток Яндекса, не
    /// свой файл). Прежний трек должен быть закрыт через [`Tracker::finish`]
    /// до этого.
    pub fn begin(
        &mut self,
        track_id: &str,
        album_id: Option<&str>,
        length: Option<Duration>,
        reportable: bool,
        play_id: String,
    ) {
        self.active = reportable.then(|| Active {
            track_id: track_id.to_owned(),
            album_id: album_id.map(str::to_owned),
            length_seconds: length.and_then(|length| u32::try_from(length.as_secs()).ok()),
            play_id,
            started: SystemTime::now(),
            started_sent: false,
        });
    }

    /// Звук пошёл: сообщение о начале (один раз за трек).
    pub fn sounded(&mut self, uid: u64) -> Option<PlayReport> {
        let active = self.active.as_mut().filter(|active| !active.started_sent)?;
        active.started_sent = true;
        Some(report(active, uid, 0.0))
    }

    /// Трек сменился или кончился: сообщение о сыгранном. Идёт, только если
    /// трек зазвучал; повторный вызов ничего не даёт.
    pub fn finish(&mut self, uid: u64, played_seconds: f32) -> Option<PlayReport> {
        let active = self.active.take().filter(|active| active.started_sent)?;
        Some(report(&active, uid, played_seconds))
    }
}

fn report(active: &Active, uid: u64, played: f32) -> PlayReport {
    // Секунды целые: дробную часть и отрицательное отбрасываем.
    let played = if played.is_finite() {
        played.max(0.0) as u32
    } else {
        0
    };
    PlayReport {
        track_id: active.track_id.clone(),
        album_id: active.album_id.clone(),
        play_id: active.play_id.clone(),
        uid,
        length_seconds: active.length_seconds,
        total_played_seconds: played,
        end_position_seconds: played,
        started: active.started,
    }
}

/// Случайный идентификатор прослушивания (`playId`).
pub fn new_play_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn begin(tracker: &mut Tracker, reportable: bool) {
        tracker.begin(
            "42",
            Some("7"),
            Some(Duration::from_secs(200)),
            reportable,
            "play-1".to_owned(),
        );
    }

    #[test]
    fn start_goes_once_and_finish_reports_played() {
        let mut tracker = Tracker::default();
        begin(&mut tracker, true);
        // Звука ещё не было: сообщать нечего, закрытие тоже ничего не шлёт.
        let started = tracker.sounded(9).unwrap();
        assert_eq!(started.total_played_seconds, 0);
        assert_eq!(started.track_id, "42");
        assert_eq!(started.length_seconds, Some(200));
        // Повторное «зазвучал» (после перемотки, буферизации) — не шлём.
        assert_eq!(tracker.sounded(9), None);
        let finished = tracker.finish(9, 61.8).unwrap();
        assert_eq!(finished.total_played_seconds, 61);
        assert_eq!(finished.end_position_seconds, 61);
        assert_eq!(finished.play_id, started.play_id);
        // Одно событие — одно сообщение.
        assert_eq!(tracker.finish(9, 70.0), None);
    }

    #[test]
    fn track_that_never_sounded_is_not_reported() {
        let mut tracker = Tracker::default();
        begin(&mut tracker, true);
        assert_eq!(tracker.finish(9, 3.0), None);
        // Ни начала, ни конца.
        assert_eq!(tracker.sounded(9), None);
    }

    #[test]
    fn own_file_is_never_reported() {
        let mut tracker = Tracker::default();
        begin(&mut tracker, false);
        assert_eq!(tracker.sounded(9), None);
        assert_eq!(tracker.finish(9, 30.0), None);
    }

    #[test]
    fn nonsense_seconds_become_zero() {
        let mut tracker = Tracker::default();
        begin(&mut tracker, true);
        tracker.sounded(1);
        assert_eq!(tracker.finish(1, f32::NAN).unwrap().total_played_seconds, 0);
        begin(&mut tracker, true);
        tracker.sounded(1);
        assert_eq!(tracker.finish(1, -5.0).unwrap().total_played_seconds, 0);
    }

    #[test]
    fn form_has_the_expected_fields() {
        let mut tracker = Tracker::default();
        begin(&mut tracker, true);
        let report = tracker.sounded(1234).unwrap();
        let form = report.form(SystemTime::UNIX_EPOCH);
        let get = |key: &str| {
            form.iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| value.as_str())
        };
        assert_eq!(get("track-id"), Some("42"));
        assert_eq!(get("album-id"), Some("7"));
        assert_eq!(get("play-id"), Some("play-1"));
        assert_eq!(get("uid"), Some("1234"));
        assert_eq!(get("from"), Some("plastinka"));
        assert_eq!(get("from-cache"), Some("false"));
        assert_eq!(get("track-length-seconds"), Some("200"));
        assert_eq!(get("total-played-seconds"), Some("0"));
        assert_eq!(get("end-position-seconds"), Some("0"));
        assert_eq!(get("client-now"), Some("1970-01-01T00:00:00.000Z"));
        // Время начала в формате RFC 3339 с «Z».
        assert!(get("timestamp").is_some_and(|stamp| stamp.ends_with('Z')));
        // Токена в форме нет.
        assert!(form.iter().all(|(name, _)| !name.contains("token")));
    }

    #[test]
    fn optional_fields_are_left_out() {
        let mut tracker = Tracker::default();
        tracker.begin("5", None, None, true, "p".to_owned());
        let report = tracker.sounded(1).unwrap();
        let form = report.form(SystemTime::UNIX_EPOCH);
        assert!(form.iter().all(|(name, _)| *name != "album-id"));
        assert!(form.iter().all(|(name, _)| *name != "track-length-seconds"));
    }

    #[test]
    fn play_ids_are_unique() {
        assert_ne!(new_play_id(), new_play_id());
    }
}
