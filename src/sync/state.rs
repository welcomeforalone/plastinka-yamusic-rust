//! Чистая логика без сети: версии состояния, выбор общего состояния при
//! начальном обмене (§4.4), позиция «сейчас» и подстройка ведомого (§5.2).
//! Время — свои часы в миллисекундах (`i64`).

use super::proto::State;

/// Расхождение, меньше которого позицию не трогаем.
pub const DRIFT_THRESHOLD_MS: i64 = 120;
/// Подстраиваться не чаще.
pub const MIN_ADJUST_INTERVAL_MS: i64 = 4000;
/// Пауза после своего запуска или перемотки.
pub const SETTLE_MS: i64 = 1500;
/// Сколько `tick` подряд должны разойтись.
const STRIKES_NEEDED: u32 = 2;

/// Версия `(rev, by)` новее текущей: сначала `rev`, потом `by` как строка.
pub fn is_newer(new: (u64, &str), current: (u64, &str)) -> bool {
    new > current
}

/// Счётчик ревизий устройства.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RevCounter {
    current: u64,
}

impl RevCounter {
    pub fn new(current: u64) -> Self {
        Self { current }
    }

    pub fn current(&self) -> u64 {
        self.current
    }

    /// Своё изменение: `rev = текущий + 1`.
    pub fn bump(&mut self) -> u64 {
        self.current = self.current.saturating_add(1);
        self.current
    }

    /// Принятое: `текущий = max(текущий, rev)`.
    pub fn observe(&mut self, rev: u64) {
        self.current = self.current.max(rev);
    }
}

/// Чьё состояние становится общим.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Winner {
    /// Своё: чужая сторона станет ведомой.
    Local,
    /// Чужое: мы ведомые.
    Remote,
    /// Трека нет ни у кого: общего состояния и ведущего нет.
    Nobody,
}

/// Выбор по правилам §4.4. Обе стороны вызывают с одной и той же парой
/// начальных состояний и приходят к одному выводу. `local_is_initiator` —
/// мы подключались сами.
pub fn choose_initial(local: &State, remote: &State, local_is_initiator: bool) -> Winner {
    let (local_track, remote_track) = (local.track.is_some(), remote.track.is_some());
    match (local_track, remote_track) {
        (false, false) => return Winner::Nobody,
        (true, false) => return Winner::Local,
        (false, true) => return Winner::Remote,
        (true, true) => {}
    }
    // Связь восстановилась: у обоих один и тот же ведущий, и это одна из
    // сторон — он остаётся ведущим, его очередь не теряется.
    if local.owner == remote.owner {
        if local.owner == local.by {
            return Winner::Local;
        }
        if local.owner == remote.by {
            return Winner::Remote;
        }
    }
    match (local.playing, remote.playing) {
        (true, false) => return Winner::Local,
        (false, true) => return Winner::Remote,
        _ => {}
    }
    let initiator_joined = if local_is_initiator {
        local.join
    } else {
        remote.join
    };
    if initiator_joined {
        // Пользователь нажал «Подключить»: побеждает ответчик.
        return if local_is_initiator {
            Winner::Remote
        } else {
            Winner::Local
        };
    }
    let local_key = (local.rev, local.by.as_str());
    let remote_key = (remote.rev, remote.by.as_str());
    if local_key > remote_key {
        Winner::Local
    } else if remote_key > local_key {
        Winner::Remote
    } else if local_is_initiator {
        // Равные версии невозможны (разные `by`); при равенстве — ответчик.
        Winner::Remote
    } else {
        Winner::Local
    }
}

/// Позиция сейчас (мс) по состоянию или `tick` собеседника: `position_ms`
/// на момент `at` его часов; `offset` — оценка смещения; `correction_ms` —
/// своя поправка задержки. Не меньше нуля.
pub fn position_now(
    position_ms: u64,
    at: i64,
    playing: bool,
    offset: i64,
    now: i64,
    correction_ms: i64,
) -> u64 {
    let base = i64::try_from(position_ms).unwrap_or(i64::MAX);
    let elapsed = if playing {
        now.saturating_sub(at.saturating_sub(offset))
    } else {
        0
    };
    let position = base
        .saturating_add(elapsed)
        .saturating_add(correction_ms)
        .max(0);
    u64::try_from(position).unwrap_or(0)
}

/// Подстройка позиции ведомого по `tick` (§5.2).
#[derive(Debug, Default)]
pub struct DriftCorrector {
    /// Ждём первый `tick` без буферизации после нового трека или перемотки
    /// ведущего: он исправляет расхождение сразу.
    fresh: bool,
    strikes: u32,
    last_adjust: Option<i64>,
    quiet_until: Option<i64>,
}

impl DriftCorrector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Свой запуск или перемотка в момент `now`: 1,5 с не подстраиваемся.
    pub fn on_local_start(&mut self, now: i64) {
        self.quiet_until = Some(now.saturating_add(SETTLE_MS));
        self.strikes = 0;
    }

    /// Ведущий начал новый трек или перемотал: первый `tick` без буферизации
    /// исправляет расхождение сразу (§5.2).
    pub fn expect_first_tick(&mut self) {
        self.fresh = true;
    }

    /// Разбирает очередной `tick`. `local_position` — своя позиция сейчас,
    /// `target` — позиция ведущего сейчас плюс поправка. Возвращает позицию,
    /// на которую надо перемотать.
    pub fn on_tick(
        &mut self,
        now: i64,
        local_position: u64,
        target: u64,
        local_buffering: bool,
        tick_buffering: bool,
    ) -> Option<u64> {
        if self.fresh && !tick_buffering {
            self.fresh = false;
            if !local_buffering {
                self.strikes = 0;
                let drift = i128::from(local_position).abs_diff(i128::from(target));
                if drift <= DRIFT_THRESHOLD_MS.unsigned_abs().into() {
                    return None;
                }
                self.last_adjust = Some(now);
                self.quiet_until = Some(now.saturating_add(SETTLE_MS));
                return Some(target);
            }
        }
        let quiet = self.quiet_until.is_some_and(|until| now < until);
        if local_buffering || tick_buffering || quiet {
            self.strikes = 0;
            return None;
        }
        let drift = i128::from(local_position).abs_diff(i128::from(target));
        if drift <= DRIFT_THRESHOLD_MS.unsigned_abs().into() {
            self.strikes = 0;
            return None;
        }
        self.strikes = self.strikes.saturating_add(1);
        let allowed = self
            .last_adjust
            .is_none_or(|last| now.saturating_sub(last) >= MIN_ADJUST_INTERVAL_MS);
        if self.strikes < STRIKES_NEEDED || !allowed {
            return None;
        }
        self.strikes = 0;
        self.last_adjust = Some(now);
        self.quiet_until = Some(now.saturating_add(SETTLE_MS));
        Some(target)
    }
}

#[cfg(test)]
mod tests {
    use super::super::proto::{Repeat, WireTrack};
    use super::*;

    fn track() -> WireTrack {
        WireTrack {
            id: "1".into(),
            album_id: None,
            album_title: None,
            title: None,
            version: None,
            artists: Vec::new(),
            duration_ms: None,
            cover_uri: None,
            available: true,
            warning: None,
            loudness: None,
        }
    }

    fn state(by: char, rev: u64, has_track: bool, playing: bool, join: bool) -> State {
        let id: String = std::iter::repeat_n(by, 32).collect();
        State {
            rev,
            owner: id.clone(),
            by: id,
            track: has_track.then(track),
            playing,
            position_ms: 0,
            at: 0,
            next: None,
            shuffle: false,
            repeat: Repeat::Off,
            wave: false,
            initial: true,
            join,
        }
    }

    /// Выбор с обеих сторон должен совпасть: `a` — инициатор, `b` — ответчик.
    fn both(a: &State, b: &State) -> (Winner, Winner) {
        (choose_initial(a, b, true), choose_initial(b, a, false))
    }

    #[test]
    fn version_order() {
        assert!(is_newer((5, "a"), (4, "z")));
        assert!(is_newer((5, "b"), (5, "a")));
        assert!(!is_newer((5, "a"), (5, "a")));
        assert!(!is_newer((4, "z"), (5, "a")));
    }

    #[test]
    fn rev_counter() {
        let mut counter = RevCounter::new(3);
        assert_eq!(counter.bump(), 4);
        counter.observe(2);
        assert_eq!(counter.current(), 4);
        counter.observe(10);
        assert_eq!(counter.bump(), 11);
    }

    #[test]
    fn rule_1_track_only_on_one_side() {
        let (a, b) = (
            state('a', 9, true, false, true),
            state('b', 1, false, false, false),
        );
        assert_eq!(both(&a, &b), (Winner::Local, Winner::Remote));
        assert_eq!(both(&b, &a), (Winner::Remote, Winner::Local));
    }

    #[test]
    fn rule_2_only_one_plays() {
        let (a, b) = (
            state('a', 1, true, false, true),
            state('b', 1, true, true, false),
        );
        // Даже при join побеждает играющая сторона.
        assert_eq!(both(&a, &b), (Winner::Remote, Winner::Local));
    }

    #[test]
    fn restored_link_keeps_the_previous_leader() {
        // Ведущий до обрыва — ответчик «b»: у него очередь, у нас (инициатор
        // «a») трек тоже играет, но побеждает ведущий.
        let mut a = state('a', 9, true, true, false);
        let mut b = state('b', 1, true, true, false);
        a.owner = b.by.clone();
        b.owner = b.by.clone();
        assert_eq!(both(&a, &b), (Winner::Remote, Winner::Local));
        // Ведущий — мы.
        a.owner = a.by.clone();
        b.owner = a.by.clone();
        assert_eq!(both(&a, &b), (Winner::Local, Winner::Remote));
        // Мнения о ведущем разные — обычные правила.
        a.owner = a.by.clone();
        b.owner = b.by.clone();
        assert_eq!(both(&a, &b), (Winner::Local, Winner::Remote));
        // Ведущий — чужой id: обычные правила.
        let stranger = "c".repeat(32);
        a.owner = stranger.clone();
        b.owner = stranger;
        assert_eq!(both(&a, &b), (Winner::Local, Winner::Remote));
        // Трек только у одной стороны: правило 1 сильнее.
        let mut a = state('a', 1, true, false, false);
        let mut b = state('b', 9, false, false, false);
        a.owner = b.by.clone();
        b.owner = b.by.clone();
        assert_eq!(both(&a, &b), (Winner::Local, Winner::Remote));
    }

    #[test]
    fn first_tick_after_new_track_fixes_drift_at_once() {
        let mut fix = DriftCorrector::new();
        fix.on_local_start(10_000);
        fix.expect_first_tick();
        // Тик с буферизацией ведущего не считается первым.
        assert_eq!(fix.on_tick(10_100, 0, 900, false, true), None);
        // Первый тик без буферизации: сразу, несмотря на тишину 1,5 с.
        assert_eq!(fix.on_tick(10_200, 0, 900, false, false), Some(900));
        // Дальше обычные правила (тишина после перемотки).
        assert_eq!(fix.on_tick(10_300, 0, 900, false, false), None);
        // Ведомый сам буферизуется — исключения нет, и «первый» израсходован.
        let mut fix = DriftCorrector::new();
        fix.expect_first_tick();
        assert_eq!(fix.on_tick(20_000, 0, 900, true, false), None);
        assert_eq!(fix.on_tick(21_000, 0, 900, false, false), None);
        // Малое расхождение первый тик не трогает.
        let mut fix = DriftCorrector::new();
        fix.expect_first_tick();
        assert_eq!(fix.on_tick(30_000, 1000, 1050, false, false), None);
    }

    #[test]
    fn rule_3_join_picks_responder() {
        let (a, b) = (
            state('a', 9, true, true, true),
            state('b', 1, true, true, false),
        );
        assert_eq!(both(&a, &b), (Winner::Remote, Winner::Local));
    }

    #[test]
    fn rule_4_bigger_version() {
        let (a, b) = (
            state('a', 5, true, true, false),
            state('b', 4, true, true, false),
        );
        assert_eq!(both(&a, &b), (Winner::Local, Winner::Remote));
        let (a, b) = (
            state('a', 5, true, false, false),
            state('b', 5, true, false, false),
        );
        // Ревизии равны: больше `by`.
        assert_eq!(both(&a, &b), (Winner::Remote, Winner::Local));
    }

    #[test]
    fn rule_5_nobody() {
        let (a, b) = (
            state('a', 5, false, false, true),
            state('b', 4, false, false, false),
        );
        assert_eq!(both(&a, &b), (Winner::Nobody, Winner::Nobody));
    }

    #[test]
    fn position_now_uses_offset() {
        // Собеседник впереди на 1000; его `at` = 5000 — это наши 4000.
        assert_eq!(position_now(10_000, 5000, true, 1000, 4500, 0), 10_500);
        assert_eq!(position_now(10_000, 5000, false, 1000, 4500, 0), 10_000);
        assert_eq!(position_now(10_000, 5000, true, 1000, 4500, 30), 10_530);
        assert_eq!(position_now(100, 5000, false, 0, 0, -500), 0);
    }

    #[test]
    fn small_drift_is_ignored() {
        let mut fix = DriftCorrector::new();
        for t in 0..5 {
            assert_eq!(
                fix.on_tick(10_000 + t * 1000, 5000, 5120, false, false),
                None
            );
        }
    }

    #[test]
    fn two_ticks_in_a_row_needed() {
        let mut fix = DriftCorrector::new();
        assert_eq!(fix.on_tick(10_000, 5000, 5500, false, false), None);
        assert_eq!(fix.on_tick(11_000, 6000, 6500, false, false), Some(6500));
        // Один выброс между нормальными ничего не меняет.
        let mut fix = DriftCorrector::new();
        assert_eq!(fix.on_tick(10_000, 5000, 5500, false, false), None);
        assert_eq!(fix.on_tick(11_000, 6000, 6050, false, false), None);
        assert_eq!(fix.on_tick(12_000, 7000, 7500, false, false), None);
    }

    #[test]
    fn adjust_not_more_often_than_every_four_seconds() {
        let mut fix = DriftCorrector::new();
        fix.on_tick(10_000, 0, 500, false, false);
        assert_eq!(fix.on_tick(11_000, 0, 500, false, false), Some(500));
        assert_eq!(fix.on_tick(12_000, 0, 500, false, false), None);
        assert_eq!(fix.on_tick(13_000, 0, 500, false, false), None);
        assert_eq!(fix.on_tick(14_000, 0, 500, false, false), None);
        assert_eq!(fix.on_tick(15_000, 0, 500, false, false), Some(500));
    }

    #[test]
    fn settle_and_buffering_pauses() {
        let mut fix = DriftCorrector::new();
        fix.on_local_start(10_000);
        assert_eq!(fix.on_tick(10_500, 0, 900, false, false), None);
        assert_eq!(fix.on_tick(11_000, 0, 900, false, false), None);
        // После 1,5 с — два расхождения подряд.
        assert_eq!(fix.on_tick(11_600, 0, 900, false, false), None);
        assert_eq!(fix.on_tick(12_600, 0, 900, false, false), Some(900));
        // Своя буферизация и буферизация ведущего: ничего.
        let mut fix = DriftCorrector::new();
        for _ in 0..4 {
            assert_eq!(fix.on_tick(20_000, 0, 900, true, false), None);
            assert_eq!(fix.on_tick(20_000, 0, 900, false, true), None);
        }
    }
}
