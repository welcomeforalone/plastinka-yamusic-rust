//! Оценка смещения часов собеседника (§4.2). Часы — монотонные миллисекунды
//! устройства с произвольной точкой отсчёта.

use std::collections::VecDeque;
use std::time::Duration;

/// Сколько последних замеров храним.
pub const MAX_SAMPLES: usize = 8;
/// Замер с большим `rtt` (мс) отбрасываем.
pub const MAX_RTT_MS: i64 = 1000;
/// Сколько пингов подряд сразу после готовности связи.
pub const BURST_PINGS: usize = 5;
/// Пауза между пингами в начале.
pub const BURST_INTERVAL: Duration = Duration::from_millis(200);
/// Пауза между пингами потом.
pub const PING_INTERVAL: Duration = Duration::from_secs(3);
/// Не реже этого каждая сторона шлёт хоть что-то.
pub const SEND_INTERVAL: Duration = Duration::from_secs(5);
/// Ничего не пришло столько — связь потеряна.
pub const LIVENESS_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy)]
struct Sample {
    rtt: i64,
    offset: i64,
}

/// Хранит замеры и выдаёт оценку смещения: на сколько часы собеседника
/// впереди своих.
#[derive(Debug, Default)]
pub struct ClockEstimator {
    samples: VecDeque<Sample>,
}

impl ClockEstimator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Новый замер: `t0` — свои часы при отправке `ping`, `t1` — часы
    /// собеседника при ответе, `t2` — свои часы при получении `pong`.
    /// Возвращает `false`, если замер отброшен (отрицательный или слишком
    /// большой `rtt`).
    pub fn record(&mut self, t0: i64, t1: i64, t2: i64) -> bool {
        let rtt = t2.saturating_sub(t0);
        if !(0..=MAX_RTT_MS).contains(&rtt) {
            return false;
        }
        let offset = t1.saturating_sub(t0.saturating_add(rtt / 2));
        if self.samples.len() == MAX_SAMPLES {
            self.samples.pop_front();
        }
        self.samples.push_back(Sample { rtt, offset });
        true
    }

    /// Смещение замера с наименьшим `rtt` (при равенстве — более свежего).
    pub fn offset(&self) -> Option<i64> {
        self.samples
            .iter()
            .rev()
            .min_by_key(|sample| sample.rtt)
            .map(|sample| sample.offset)
    }

    /// Наименьший `rtt` среди замеров.
    pub fn best_rtt(&self) -> Option<i64> {
        self.samples.iter().map(|sample| sample.rtt).min()
    }

    /// Момент собеседника в своих часах: `remote - offset`. Без оценки —
    /// смещение считается нулевым.
    pub fn to_local(&self, remote: i64) -> i64 {
        remote.saturating_sub(self.offset().unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_from_symmetric_exchange() {
        let mut clock = ClockEstimator::new();
        // Собеседник впереди на 5000: ping в 100, ответ в его 5060, pong в 120.
        assert!(clock.record(100, 5060, 120));
        // rtt 20, offset = 5060 - (100 + 10) = 4950.
        assert_eq!(clock.offset(), Some(4950));
        assert_eq!(clock.to_local(5110), 160);
    }

    #[test]
    fn best_rtt_wins_and_bad_samples_dropped() {
        let mut clock = ClockEstimator::new();
        assert_eq!(clock.offset(), None);
        assert_eq!(clock.to_local(77), 77);
        assert!(clock.record(0, 500, 100));
        assert!(clock.record(1000, 1500, 1010));
        assert_eq!(clock.offset(), Some(495));
        assert!(!clock.record(0, 0, 1001));
        assert!(!clock.record(10, 0, 5));
        assert_eq!(clock.best_rtt(), Some(10));
    }

    #[test]
    fn only_last_eight_samples_are_kept() {
        let mut clock = ClockEstimator::new();
        // Первый замер самый точный, но он вытесняется.
        assert!(clock.record(0, 1000, 2));
        for i in 1..=8 {
            assert!(clock.record(i * 100, i * 100 + 40, i * 100 + 20));
        }
        // rtt 20, offset 40 - 10 = 30; замера с offset 999 уже нет.
        assert_eq!(clock.offset(), Some(30));
    }
}
