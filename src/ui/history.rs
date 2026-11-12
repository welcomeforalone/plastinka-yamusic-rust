//! История прослушивания за сеанс: последние треки, которые зазвучали и
//! сменились другим. Только в памяти. Её показывает панель «Очередь»
//! («Недавно играли»), позже понадобится вкладке «История».

use std::collections::VecDeque;

use crate::api::Track;

/// Сколько треков помним.
pub const LIMIT: usize = 20;

/// «Недавно играли»: сборка из трёх источников. Порядок важности: история
/// сеанса (`session`, от старых к новым), треки очереди перед текущим
/// (`queue_before`, ближайший первым), серверная история (`server`, свежие
/// первыми). Повторы по id отбрасываются: остаётся вхождение, ближайшее к
/// текущему; сам текущий трек не показывается. Не больше `limit` треков;
/// результат от старых к новым, самые свежие рядом с «Сейчас играет».
pub fn merge_recent<'a>(
    current: Option<&str>,
    session: &[&'a Track],
    queue_before: &[&'a Track],
    server: &[&'a Track],
    limit: usize,
) -> Vec<&'a Track> {
    let mut seen: std::collections::HashSet<&str> = current.into_iter().collect();
    let mut out: Vec<&Track> = Vec::new();
    let candidates = session
        .iter()
        .rev()
        .chain(queue_before.iter())
        .chain(server.iter());
    for track in candidates {
        if out.len() >= limit {
            break;
        }
        if seen.insert(track.id.as_str()) {
            out.push(track);
        }
    }
    out.reverse();
    out
}

/// Последние прослушанные треки, новые в конце.
#[derive(Default)]
pub struct History {
    tracks: VecDeque<Track>,
}

impl History {
    /// Трек сменился другим. `sounded` — он успел зазвучать: трек, который
    /// не запустился или его пропустили сразу же, в историю не идёт. Подряд
    /// идущие повторы одного трека не дублируются.
    pub fn record(&mut self, track: Track, sounded: bool) {
        if !sounded || self.tracks.back().is_some_and(|last| last.id == track.id) {
            return;
        }
        self.tracks.push_back(track);
        while self.tracks.len() > LIMIT {
            self.tracks.pop_front();
        }
    }

    /// От старых к новым.
    pub fn tracks(&self) -> impl Iterator<Item = &Track> {
        self.tracks.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: &str) -> Track {
        Track {
            id: id.to_owned(),
            title: None,
            version: None,
            artists: Vec::new(),
            album_id: None,
            album_title: None,
            duration: None,
            available: true,
            cover_uri: None,
            warning: crate::api::Warning::None,
            loudness: None,
            lyrics: None,
        }
    }

    fn ids(history: &History) -> Vec<String> {
        history.tracks().map(|track| track.id.clone()).collect()
    }

    fn refs(list: &[Track]) -> Vec<&Track> {
        list.iter().collect()
    }

    fn merged(
        current: Option<&str>,
        session: &[&str],
        before: &[&str],
        server: &[&str],
        limit: usize,
    ) -> Vec<String> {
        let make = |ids: &[&str]| ids.iter().map(|id| track(id)).collect::<Vec<_>>();
        let (session, before, server) = (make(session), make(before), make(server));
        merge_recent(
            current,
            &refs(&session),
            &refs(&before),
            &refs(&server),
            limit,
        )
        .into_iter()
        .map(|track| track.id.clone())
        .collect()
    }

    #[test]
    fn merge_puts_the_freshest_next_to_current() {
        // Сеанс: a, b (b свежее). Очередь перед текущим: c (ближайший), d.
        // Сервер: e (самый свежий), f.
        assert_eq!(
            merged(Some("x"), &["a", "b"], &["c", "d"], &["e", "f"], 20),
            ["f", "e", "d", "c", "a", "b"]
        );
    }

    #[test]
    fn merge_drops_duplicates_keeping_the_closest() {
        // «b» есть и в сеансе, и на сервере: остаётся сеансовое (ближе).
        assert_eq!(
            merged(Some("x"), &["a", "b"], &[], &["b", "z"], 20),
            ["z", "a", "b"]
        );
        // «c» в очереди и на сервере.
        assert_eq!(merged(Some("x"), &[], &["c"], &["c", "z"], 20), ["z", "c"]);
    }

    #[test]
    fn merge_never_shows_the_current_track() {
        assert_eq!(
            merged(Some("b"), &["a", "b"], &["b"], &["b", "z"], 20),
            ["z", "a"]
        );
        assert_eq!(merged(None, &["a"], &[], &[], 20), ["a"]);
    }

    #[test]
    fn merge_keeps_the_freshest_within_limit() {
        let session: Vec<String> = (0..5).map(|n| format!("s{n}")).collect();
        let session: Vec<&str> = session.iter().map(String::as_str).collect();
        // Лимит 3: берутся самые свежие, порядок от старых к новым.
        assert_eq!(
            merged(Some("x"), &session, &["q"], &["z"], 3),
            ["s2", "s3", "s4"]
        );
        // Сеанс короткий: недостающее добирается из очереди, потом с сервера.
        assert_eq!(
            merged(Some("x"), &["s"], &["q1", "q2"], &["z1", "z2"], 4),
            ["z1", "q2", "q1", "s"]
        );
        assert!(merged(Some("x"), &[], &[], &[], 5).is_empty());
    }

    #[test]
    fn played_tracks_land_in_order() {
        let mut history = History::default();
        assert!(ids(&history).is_empty());
        history.record(track("1"), true);
        history.record(track("2"), true);
        assert_eq!(ids(&history), ["1", "2"]);
    }

    #[test]
    fn silent_tracks_are_not_recorded() {
        let mut history = History::default();
        history.record(track("1"), false);
        assert!(ids(&history).is_empty());
    }

    #[test]
    fn repeats_in_a_row_are_not_duplicated() {
        let mut history = History::default();
        history.record(track("1"), true);
        history.record(track("1"), true);
        history.record(track("2"), true);
        history.record(track("1"), true);
        // Не подряд — можно.
        assert_eq!(ids(&history), ["1", "2", "1"]);
    }

    #[test]
    fn limit_drops_the_oldest() {
        let mut history = History::default();
        for n in 0..(LIMIT + 5) {
            history.record(track(&n.to_string()), true);
        }
        let all = ids(&history);
        assert_eq!(all.len(), LIMIT);
        assert_eq!(all.first().map(String::as_str), Some("5"));
        assert_eq!(all.last(), Some(&(LIMIT + 4).to_string()));
    }
}
