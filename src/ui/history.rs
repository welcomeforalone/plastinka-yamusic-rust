//! История прослушивания за сеанс, только в памяти. [`History`] — последние
//! треки, которые зазвучали и сменились другим; её показывает панель
//! «Очередь» («Недавно играли»). [`LocalPlays`] — всё, что зазвучало, с
//! контекстом и днём: вкладка «История» сливает это с ответом сервера
//! ([`merge_days`]), потому что сервер показывает прослушивания не сразу.

use std::collections::{HashSet, VecDeque};

use crate::api::{HistoryContext, HistoryDay, HistoryItem, Track, Ymd};

/// Сколько треков помним.
pub const LIMIT: usize = 20;
/// Сколько прослушиваний сеанса держим для вкладки «История».
pub const LOCAL_LIMIT: usize = 200;

/// Прослушивание за сеанс: трек, откуда играл, день (UTC, как подписи дней).
#[derive(Debug, Clone)]
pub struct LocalPlay {
    pub track: Track,
    pub context: HistoryContext,
    pub date: Ymd,
}

/// Прослушивания сеанса, от старых к новым.
#[derive(Default)]
pub struct LocalPlays {
    plays: VecDeque<LocalPlay>,
}

impl LocalPlays {
    /// Трек зазвучал. Прежняя запись того же трека за тот же день уходит:
    /// он поднимается наверх, а не повторяется.
    pub fn record(&mut self, play: LocalPlay) {
        self.plays
            .retain(|old| old.track.id != play.track.id || old.date != play.date);
        self.plays.push_back(play);
        while self.plays.len() > LOCAL_LIMIT {
            self.plays.pop_front();
        }
    }

    /// От старых к новым.
    pub fn plays(&self) -> impl DoubleEndedIterator<Item = &LocalPlay> + Clone {
        self.plays.iter()
    }
}

/// Вкладка «История»: серверные дни (свежие первыми) плюс прослушивания
/// сеанса (`local`, от старых к новым). Прослушивания сеанса встают в начало
/// своего дня, свежие первыми; подряд идущие из одного контекста — одна
/// группа. Тот же трек в тот же день с сервера убирается (дубль), опустевшие
/// группы тоже. Дня нет у сервера — он появляется на своём месте по дате.
pub fn merge_days<'a>(
    server: &[HistoryDay],
    local: impl DoubleEndedIterator<Item = &'a LocalPlay> + Clone,
) -> Vec<HistoryDay> {
    let mut days = server.to_vec();
    let mut dates: Vec<Ymd> = Vec::new();
    for play in local.clone().rev() {
        if !dates.contains(&play.date) {
            dates.push(play.date);
        }
    }
    for date in dates {
        let groups = local_groups(local.clone(), date);
        let ids: HashSet<String> = groups
            .iter()
            .flat_map(|item| &item.tracks)
            .map(|track| track.id.clone())
            .collect();
        match days.iter_mut().find(|day| day.date == Some(date)) {
            Some(day) => {
                let rest = without(std::mem::take(&mut day.items), &ids);
                day.items = join(groups, rest);
            }
            None => {
                // Перед первым днём старше этого (или без даты).
                let at = days
                    .iter()
                    .position(|day| day.date.is_none_or(|other| other < date))
                    .unwrap_or(days.len());
                days.insert(
                    at,
                    HistoryDay {
                        date: Some(date),
                        items: groups,
                    },
                );
            }
        }
    }
    days
}

/// Группы прослушиваний дня `date`: свежие первыми, без повторов трека.
fn local_groups<'a>(
    local: impl DoubleEndedIterator<Item = &'a LocalPlay>,
    date: Ymd,
) -> Vec<HistoryItem> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut groups: Vec<HistoryItem> = Vec::new();
    for play in local.rev().filter(|play| play.date == date) {
        if !seen.insert(play.track.id.as_str()) {
            continue;
        }
        match groups.last_mut() {
            Some(last) if last.context == play.context => last.tracks.push(play.track.clone()),
            _ => groups.push(HistoryItem {
                context: play.context.clone(),
                tracks: vec![play.track.clone()],
            }),
        }
    }
    groups
}

/// Группы без треков `ids`; опустевшие отбрасываются.
fn without(items: Vec<HistoryItem>, ids: &HashSet<String>) -> Vec<HistoryItem> {
    items
        .into_iter()
        .filter_map(|mut item| {
            item.tracks.retain(|track| !ids.contains(&track.id));
            (!item.tracks.is_empty()).then_some(item)
        })
        .collect()
}

/// Сначала `first`, потом `rest`; на стыке один контекст — одна группа.
fn join(mut first: Vec<HistoryItem>, rest: Vec<HistoryItem>) -> Vec<HistoryItem> {
    let mut rest = rest.into_iter();
    if let Some(next) = rest.next() {
        match first.last_mut() {
            Some(last) if last.context == next.context => last.tracks.extend(next.tracks),
            _ => first.push(next),
        }
    }
    first.extend(rest);
    first
}

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

    const TODAY: Ymd = (2026, 11, 14);
    const YESTERDAY: Ymd = (2026, 11, 13);

    fn album(id: &str) -> HistoryContext {
        HistoryContext::Album {
            id: id.to_owned(),
            title: format!("Альбом {id}"),
        }
    }

    fn play(id: &str, context: HistoryContext, date: Ymd) -> LocalPlay {
        LocalPlay {
            track: track(id),
            context,
            date,
        }
    }

    fn day(date: Option<Ymd>, items: Vec<(HistoryContext, &[&str])>) -> HistoryDay {
        HistoryDay {
            date,
            items: items
                .into_iter()
                .map(|(context, ids)| HistoryItem {
                    context,
                    tracks: ids.iter().map(|id| track(id)).collect(),
                })
                .collect(),
        }
    }

    /// Дни как (дата, [(подпись контекста, [id])]).
    type Shape = Vec<(Option<Ymd>, Vec<(String, Vec<String>)>)>;

    fn shape(days: &[HistoryDay]) -> Shape {
        days.iter()
            .map(|day| {
                let items = day
                    .items
                    .iter()
                    .map(|item| {
                        let ids = item.tracks.iter().map(|t| t.id.clone()).collect();
                        (item.context.label(), ids)
                    })
                    .collect();
                (day.date, items)
            })
            .collect()
    }

    fn group(label: &str, ids: &[&str]) -> (String, Vec<String>) {
        (
            label.to_owned(),
            ids.iter().map(|id| (*id).to_owned()).collect(),
        )
    }

    fn merge(server: &[HistoryDay], plays: &[LocalPlay]) -> Shape {
        let mut local = LocalPlays::default();
        for play in plays {
            local.record(play.clone());
        }
        shape(&merge_days(server, local.plays()))
    }

    #[test]
    fn local_plays_go_on_top_of_today_freshest_first() {
        // Сервер знает только волну; сеанс: альбом 1 (a, b), потом поиск (c).
        let server = [day(
            Some(TODAY),
            vec![(HistoryContext::Wave, &["w1", "w2"])],
        )];
        let search = HistoryContext::Other("Поиск".to_owned());
        let plays = [
            play("a", album("1"), TODAY),
            play("b", album("1"), TODAY),
            play("c", search, TODAY),
        ];
        assert_eq!(
            merge(&server, &plays),
            vec![(
                Some(TODAY),
                vec![
                    group("Поиск", &["c"]),
                    group("Альбом «Альбом 1»", &["b", "a"]),
                    group("Моя волна", &["w1", "w2"]),
                ]
            )]
        );
    }

    #[test]
    fn server_duplicates_of_today_are_dropped() {
        // Сервер уже учёл «a»: остаётся одна запись, сеансовая (свежее);
        // опустевшая группа сервера пропадает. Вчерашнее «a» не трогаем.
        let server = [
            day(
                Some(TODAY),
                vec![(album("1"), &["a"]), (album("2"), &["x", "a"])],
            ),
            day(Some(YESTERDAY), vec![(album("1"), &["a"])]),
        ];
        let plays = [play("a", HistoryContext::Wave, TODAY)];
        assert_eq!(
            merge(&server, &plays),
            vec![
                (
                    Some(TODAY),
                    vec![
                        group("Моя волна", &["a"]),
                        group("Альбом «Альбом 2»", &["x"]),
                    ]
                ),
                (Some(YESTERDAY), vec![group("Альбом «Альбом 1»", &["a"])]),
            ]
        );
    }

    #[test]
    fn same_context_at_the_seam_is_one_group() {
        // Сеанс и сервер играли одну волну: один заголовок «Моя волна».
        let server = [day(Some(TODAY), vec![(HistoryContext::Wave, &["w1"])])];
        let plays = [play("w2", HistoryContext::Wave, TODAY)];
        assert_eq!(
            merge(&server, &plays),
            vec![(Some(TODAY), vec![group("Моя волна", &["w2", "w1"])])]
        );
    }

    #[test]
    fn replay_moves_the_track_up_without_duplicates() {
        // a, b, a: «a» один раз и наверху; группы по контексту подряд.
        let plays = [
            play("a", album("1"), TODAY),
            play("b", album("2"), TODAY),
            play("a", album("1"), TODAY),
        ];
        assert_eq!(
            merge(&[], &plays),
            vec![(
                Some(TODAY),
                vec![
                    group("Альбом «Альбом 1»", &["a"]),
                    group("Альбом «Альбом 2»", &["b"]),
                ]
            )]
        );
    }

    #[test]
    fn missing_day_is_inserted_by_date() {
        // Сервер ещё не знает «сегодня»: день встаёт первым, перед вчера и
        // днями без даты. Пустой сервер — только сеанс.
        let server = [
            day(Some(YESTERDAY), vec![(album("1"), &["y"])]),
            day(None, vec![(album("9"), &["z"])]),
        ];
        let plays = [play("a", album("1"), TODAY)];
        let merged = merge(&server, &plays);
        let dates: Vec<Option<Ymd>> = merged.iter().map(|(date, _)| *date).collect();
        assert_eq!(dates, [Some(TODAY), Some(YESTERDAY), None]);
        // Прослушивание через полночь (UTC) — в свой день, после сегодняшних.
        let plays = [
            play("old", album("1"), YESTERDAY),
            play("new", album("1"), TODAY),
        ];
        let merged = merge(&server, &plays);
        assert_eq!(
            merged,
            vec![
                (Some(TODAY), vec![group("Альбом «Альбом 1»", &["new"])]),
                (
                    Some(YESTERDAY),
                    vec![group("Альбом «Альбом 1»", &["old", "y"])]
                ),
                (None, vec![group("Альбом «Альбом 9»", &["z"])]),
            ]
        );
        assert!(merge(&[], &[]).is_empty());
        // Без сеанса — ответ сервера как есть.
        assert_eq!(merge(&server, &[]), shape(&server));
    }

    #[test]
    fn local_plays_are_limited() {
        let mut local = LocalPlays::default();
        for n in 0..(LOCAL_LIMIT + 3) {
            local.record(play(&n.to_string(), HistoryContext::Wave, TODAY));
        }
        assert_eq!(local.plays().count(), LOCAL_LIMIT);
        assert_eq!(local.plays().next().map(|p| p.track.id.as_str()), Some("3"));
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
