//! Очередь скачивания: не больше [`PARALLEL`] треков одновременно, счётчики
//! для строки «Скачивается N из M…» и итога «Скачано N, пропущено K».

use std::collections::VecDeque;

use crate::texts::plural;

/// Сколько треков качается одновременно.
pub const PARALLEL: usize = 2;

/// Чем кончилось скачивание одного трека.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Done {
    Saved,
    /// Файл уже был в папке.
    Existed,
    /// Звука нет: трек недоступен или это свой файл FckCensor.
    Skipped,
    Failed,
}

#[derive(Debug)]
pub struct Queue<T> {
    waiting: VecDeque<T>,
    running: usize,
    total: usize,
    saved: usize,
    existed: usize,
    skipped: usize,
    failed: usize,
    /// Первая ошибка пачки: попадает в итог.
    first_error: Option<String>,
}

impl<T> Default for Queue<T> {
    fn default() -> Self {
        Self {
            waiting: VecDeque::new(),
            running: 0,
            total: 0,
            saved: 0,
            existed: 0,
            skipped: 0,
            failed: 0,
            first_error: None,
        }
    }
}

impl<T> Queue<T> {
    /// Добавляет трек в конец очереди.
    pub fn push(&mut self, item: T) {
        self.total += 1;
        self.waiting.push_back(item);
    }

    /// Трек пропущен сразу, до очереди (нет звука).
    pub fn skip(&mut self) {
        self.total += 1;
        self.skipped += 1;
    }

    /// Какие треки пора запускать: столько, чтобы шло не больше `PARALLEL`.
    pub fn start_ready(&mut self) -> Vec<T> {
        let mut started = Vec::new();
        while self.running < PARALLEL
            && let Some(item) = self.waiting.pop_front()
        {
            self.running += 1;
            started.push(item);
        }
        started
    }

    /// Один трек закончился. `error` — текст ошибки при `Done::Failed`.
    pub fn finish(&mut self, done: Done, error: Option<String>) {
        self.running = self.running.saturating_sub(1);
        match done {
            Done::Saved => self.saved += 1,
            Done::Existed => self.existed += 1,
            Done::Skipped => self.skipped += 1,
            Done::Failed => {
                self.failed += 1;
                if self.first_error.is_none() {
                    self.first_error = error;
                }
            }
        }
    }

    /// Ничего не идёт и не ждёт.
    pub fn is_idle(&self) -> bool {
        self.running == 0 && self.waiting.is_empty()
    }

    /// Закончено и всего: для «Скачивается N из M».
    pub fn counts(&self) -> (usize, usize) {
        let done = self.saved + self.existed + self.skipped + self.failed;
        (done, self.total)
    }

    /// «Скачивается N из M…» (N — номер трека, который идёт).
    pub fn progress_text(&self) -> String {
        let (done, total) = self.counts();
        format!("Скачивается {} из {total}…", (done + 1).min(total))
    }

    /// Итог пачки: «Скачано N, пропущено K» (и ошибки, если были).
    pub fn summary(&self) -> String {
        let skipped = self.existed + self.skipped;
        let mut text = format!("Скачано {}, пропущено {skipped}", self.saved);
        if self.failed > 0 {
            text.push_str(&format!(
                ", не удалось {} {}",
                self.failed,
                plural(self.failed, ["трек", "трека", "треков"])
            ));
            if let Some(error) = &self.first_error {
                text.push_str(&format!(": {error}"));
            }
        }
        text
    }

    /// Сбрасывает счётчики после показа итога, если очередь пуста.
    pub fn reset_if_idle(&mut self) {
        if self.is_idle() {
            *self = Self::default();
        }
    }

    /// Очередь отменена (например, токен перестал действовать): ожидающие
    /// треки выброшены, идущие закончатся сами.
    pub fn clear_waiting(&mut self) {
        let dropped = self.waiting.len();
        self.waiting.clear();
        self.total = self.total.saturating_sub(dropped);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_two_at_a_time() {
        let mut queue = Queue::default();
        for number in 0..5 {
            queue.push(number);
        }
        assert_eq!(queue.start_ready(), vec![0, 1]);
        // Пока оба идут, новых не запускаем.
        assert!(queue.start_ready().is_empty());
        queue.finish(Done::Saved, None);
        assert_eq!(queue.start_ready(), vec![2]);
        queue.finish(Done::Existed, None);
        queue.finish(Done::Saved, None);
        assert_eq!(queue.start_ready(), vec![3, 4]);
        assert!(!queue.is_idle());
        for _ in 0..2 {
            queue.finish(Done::Saved, None);
        }
        assert!(queue.is_idle());
        assert_eq!(queue.counts(), (5, 5));
    }

    #[test]
    fn skipped_tracks_never_start() {
        let mut queue = Queue::default();
        queue.skip();
        queue.push("a");
        queue.skip();
        assert_eq!(queue.start_ready(), vec!["a"]);
        assert_eq!(queue.counts(), (2, 3));
        assert_eq!(queue.progress_text(), "Скачивается 3 из 3…");
        queue.finish(Done::Saved, None);
        assert_eq!(queue.summary(), "Скачано 1, пропущено 2");
    }

    #[test]
    fn summary_counts_existing_files_as_skipped() {
        let mut queue = Queue::default();
        for number in 0..4 {
            queue.push(number);
        }
        queue.start_ready();
        queue.finish(Done::Saved, None);
        queue.finish(Done::Existed, None);
        queue.start_ready();
        queue.finish(Done::Failed, Some("нет сети".to_owned()));
        queue.finish(Done::Failed, Some("другая".to_owned()));
        assert_eq!(
            queue.summary(),
            "Скачано 1, пропущено 1, не удалось 2 трека: нет сети"
        );
    }

    #[test]
    fn progress_starts_at_one() {
        let mut queue = Queue::default();
        queue.push(1);
        queue.push(2);
        assert_eq!(queue.progress_text(), "Скачивается 1 из 2…");
    }

    #[test]
    fn tracks_can_join_a_running_queue() {
        let mut queue = Queue::default();
        queue.push(1);
        assert_eq!(queue.start_ready(), vec![1]);
        queue.push(2);
        queue.push(3);
        assert_eq!(queue.start_ready(), vec![2]);
        queue.reset_if_idle();
        assert_eq!(queue.counts().1, 3);
        queue.finish(Done::Saved, None);
        queue.finish(Done::Saved, None);
        assert_eq!(queue.start_ready(), vec![3]);
        queue.finish(Done::Saved, None);
        queue.reset_if_idle();
        assert_eq!(queue.counts(), (0, 0));
    }

    #[test]
    fn clearing_drops_only_waiting() {
        let mut queue = Queue::default();
        for number in 0..4 {
            queue.push(number);
        }
        queue.start_ready();
        queue.clear_waiting();
        assert_eq!(queue.counts().1, 2);
        queue.finish(Done::Saved, None);
        queue.finish(Done::Saved, None);
        assert!(queue.is_idle());
    }
}
