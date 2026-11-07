//! Буфер трека в памяти: файл целиком, скачанные куски отмечены отрезками.
//!
//! Загрузчик пишет байты на их место, читатель (symphonia) ждёт байты на
//! своей позиции. Если читатель ушёл далеко от того, что качается сейчас,
//! загрузчик начинает новый запрос с этой позиции (HTTP Range). На диск
//! ничего не пишется: буфер живёт, пока играет трек.

use std::io::{self, Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use symphonia::core::io::MediaSource;

/// Больше файл в память не берём.
pub const MAX_LEN: u64 = 512 * 1024 * 1024;
/// Запрос с новой позиции начинается с границы такого блока.
const BLOCK: u64 = 64 * 1024;
/// Если читателю нужны байты дальше этого от позиции загрузки, загрузка
/// перезапускается с его позиции.
const LOOKAHEAD: u64 = 256 * 1024;
/// Как часто читатель проверяет отмену, пока ждёт данные.
const WAIT_STEP: Duration = Duration::from_millis(100);

/// Почему поток прервался. Текст — для окна.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// Сеть не вернулась после повторов.
    Network(String),
    /// Получить ссылку заново не удалось.
    Link(String),
    /// Хранилище ответило не так, как ожидали.
    Storage(String),
    /// Файл больше [`MAX_LEN`].
    TooLarge,
    /// Ссылку на трек получить не удалось: в начале или посреди трека.
    NoLink(LinkFailure),
}

/// Почему не удалось получить ссылку на трек. Повторять незачем, кроме
/// сбоя сети посреди трека: его загрузчик повторяет сам.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkFailure {
    Unavailable,
    Unauthorized,
    Offline(String),
    Other(String),
}

#[derive(Default)]
struct State {
    data: Vec<u8>,
    total: Option<u64>,
    /// Скачанные отрезки `[начало, конец)`, по возрастанию, без пересечений.
    filled: Vec<(u64, u64)>,
    /// Где последний раз читал читатель.
    read_pos: u64,
    /// Позиция, на которой читатель ждёт данные.
    wanted: Option<u64>,
    failure: Option<Failure>,
    cancelled: bool,
    /// Прервать ожидание читателя один раз: плеер ждёт перемотку.
    interrupted: bool,
}

impl State {
    /// Конец скачанного отрезка, в котором лежит `pos`.
    fn filled_end(&self, pos: u64) -> Option<u64> {
        self.filled
            .iter()
            .find(|(start, end)| *start <= pos && pos < *end)
            .map(|(_, end)| *end)
    }

    /// Первый нескачанный байт не раньше `from`.
    fn first_gap(&self, from: u64) -> Option<u64> {
        let total = self.total?;
        let mut pos = from;
        for (start, end) in &self.filled {
            if *end <= pos {
                continue;
            }
            if *start > pos {
                break;
            }
            pos = *end;
        }
        (pos < total).then_some(pos)
    }

    fn mark(&mut self, start: u64, end: u64) {
        if start >= end {
            return;
        }
        self.filled.push((start, end));
        self.filled.sort_unstable();
        let mut merged: Vec<(u64, u64)> = Vec::with_capacity(self.filled.len());
        for (start, end) in self.filled.drain(..) {
            match merged.last_mut() {
                Some(last) if start <= last.1 => last.1 = last.1.max(end),
                _ => merged.push((start, end)),
            }
        }
        self.filled = merged;
    }
}

/// Отладка: атомы mp4 верхнего уровня — первые по порядку и сколько каких.
#[cfg(debug_assertions)]
fn layout(data: &[u8]) -> String {
    if data.get(..4) == Some(b"fLaC".as_slice()) {
        return "нативный FLAC".to_owned();
    }
    let be32 = |at: usize| -> Option<u64> {
        let bytes: [u8; 4] = data.get(at..at.checked_add(4)?)?.try_into().ok()?;
        Some(u64::from(u32::from_be_bytes(bytes)))
    };
    let be64 = |at: usize| -> Option<u64> {
        let bytes: [u8; 8] = data.get(at..at.checked_add(8)?)?.try_into().ok()?;
        Some(u64::from_be_bytes(bytes))
    };
    let mut first = Vec::new();
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    let mut pos: usize = 0;
    while let (Some(size), Some(kind)) = (be32(pos), data.get(pos + 4..pos + 8)) {
        let kind = String::from_utf8_lossy(kind).into_owned();
        let size = match size {
            0 => (data.len() - pos) as u64,
            1 => be64(pos + 8).unwrap_or(0),
            size => size,
        };
        if size < 8 {
            break;
        }
        if first.len() < 8 {
            first.push(format!("{kind}@{pos}({size})"));
        }
        *counts.entry(kind).or_default() += 1;
        let Some(next) = usize::try_from(size)
            .ok()
            .and_then(|size| pos.checked_add(size))
        else {
            break;
        };
        pos = next;
    }
    if counts.is_empty() {
        return "не mp4 и не FLAC (MP3/ADTS?)".to_owned();
    }
    format!("первые атомы: {}; всего: {counts:?}", first.join(" "))
}

/// Общий буфер загрузчика и читателя.
#[derive(Default)]
pub struct StreamBuffer {
    state: Mutex<State>,
    changed: Condvar,
    /// Отладка: начало трассировки потока (`PLASTINKA_TRACE_STREAM=1`).
    #[cfg(debug_assertions)]
    trace: Option<std::time::Instant>,
}

impl StreamBuffer {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            #[cfg(debug_assertions)]
            trace: std::env::var("PLASTINKA_TRACE_STREAM")
                .is_ok_and(|value| value == "1")
                .then(std::time::Instant::now),
            ..Self::default()
        })
    }

    /// Отладка: включена ли трассировка потока.
    #[cfg(debug_assertions)]
    pub fn tracing(&self) -> bool {
        self.trace.is_some()
    }

    /// Отладка: строка трассировки с временем от начала трека.
    #[cfg(debug_assertions)]
    pub fn trace(&self, message: std::fmt::Arguments<'_>) {
        use std::io::Write;
        if let Some(start) = self.trace {
            let _ = writeln!(
                std::io::stderr(),
                "[поток {:>6} мс] {message}",
                start.elapsed().as_millis()
            );
        }
    }

    /// Отладка: где читатель и сколько скачано.
    #[cfg(debug_assertions)]
    pub fn trace_state(&self) -> String {
        let state = self.lock();
        let filled: u64 = state.filled.iter().map(|(start, end)| end - start).sum();
        format!(
            "читатель на {}, ждёт {:?}, скачано {} из {:?} байт ({} отрезков)",
            state.read_pos,
            state.wanted,
            filled,
            state.total,
            state.filled.len()
        )
    }

    /// Отладка: устройство скачанного файла (атомы mp4 верхнего уровня).
    #[cfg(debug_assertions)]
    pub fn trace_layout(&self) -> String {
        layout(&self.lock().data)
    }

    /// Мьютекс отравляется только паникой в другом потоке. Данные при этом
    /// целы: продолжаем с ними, а не роняем плеер.
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Размер файла из первого ответа. Буфер выделяется один раз.
    pub fn set_total(&self, total: u64) -> Result<(), Failure> {
        let mut state = self.lock();
        match state.total {
            Some(known) if known == total => Ok(()),
            Some(_) => Err(Failure::Storage(
                "размер файла изменился во время загрузки".to_owned(),
            )),
            None if total > MAX_LEN => Err(Failure::TooLarge),
            None => {
                let len = usize::try_from(total).map_err(|_| Failure::TooLarge)?;
                state.data = vec![0; len];
                state.total = Some(total);
                drop(state);
                self.changed.notify_all();
                Ok(())
            }
        }
    }

    /// Кладёт байты, пришедшие с позиции `offset`. Лишнее за концом файла
    /// отбрасывается. Возвращает, сколько байт легло.
    pub fn write(&self, offset: u64, bytes: &[u8]) -> usize {
        let mut state = self.lock();
        let Some(total) = state.total else {
            return 0;
        };
        let (Ok(start), Ok(limit)) = (usize::try_from(offset), usize::try_from(total)) else {
            return 0;
        };
        let end = start.saturating_add(bytes.len()).min(limit);
        let len = end.saturating_sub(start);
        let (Some(target), Some(source)) = (state.data.get_mut(start..end), bytes.get(..len))
        else {
            return 0;
        };
        target.copy_from_slice(source);
        state.mark(offset, offset.saturating_add(len as u64));
        drop(state);
        self.changed.notify_all();
        len
    }

    /// С какого байта качать дальше: с позиции, которую ждёт читатель, потом
    /// от позиции чтения до конца, потом пропуски с начала. `None` — файл
    /// скачан целиком (или размер ещё не известен: тогда с нуля).
    pub fn next_start(&self) -> Option<u64> {
        let state = self.lock();
        if state.total.is_none() {
            return Some(0);
        }
        if let Some(wanted) = state.wanted
            && let Some(gap) = state.first_gap(wanted - wanted % BLOCK)
        {
            return Some(gap);
        }
        state
            .first_gap(state.read_pos)
            .or_else(|| state.first_gap(0))
    }

    /// Пора ли бросить текущий запрос, который дошёл до `pos`: читатель ждёт
    /// далеко от него, или дальше уже скачано.
    pub fn should_restart(&self, pos: u64) -> bool {
        let state = self.lock();
        if state.cancelled || state.filled_end(pos).is_some() {
            return true;
        }
        // Читатель, чьи байты уже легли, ждать не будет: его `wanted` просто
        // не успел сброситься. Иначе каждый запрос отдавал бы один кусок и
        // сразу перезапускался (на каждый кусок — задержка ответа).
        state.wanted.is_some_and(|wanted| {
            state.filled_end(wanted).is_none()
                && (wanted < pos || wanted > pos.saturating_add(LOOKAHEAD))
        })
    }

    pub fn fail(&self, failure: Failure) {
        let mut state = self.lock();
        state.failure.get_or_insert(failure);
        drop(state);
        self.changed.notify_all();
    }

    pub fn failure(&self) -> Option<Failure> {
        self.lock().failure.clone()
    }

    pub fn cancel(&self) {
        self.lock().cancelled = true;
        self.changed.notify_all();
    }

    /// Будит читателя, который ждёт данные: ему пора перематывать.
    pub fn interrupt(&self) {
        let mut state = self.lock();
        if state.wanted.is_some() {
            state.interrupted = true;
        }
        drop(state);
        self.changed.notify_all();
    }

    /// Сбрасывает прерывание, которое читатель не успел получить.
    pub fn clear_interrupt(&self) {
        self.lock().interrupted = false;
    }

    pub fn is_cancelled(&self) -> bool {
        self.lock().cancelled
    }

    /// Читает с позиции `pos`, при необходимости ждёт загрузку.
    fn read_at(&self, pos: u64, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut state = self.lock();
        state.read_pos = pos;
        loop {
            // Не `ErrorKind::Interrupted`: на нём `read_exact` повторяет чтение
            // и ждал бы вечно.
            if state.cancelled {
                return Err(io::Error::other("поток остановлен"));
            }
            if state.interrupted {
                state.interrupted = false;
                state.wanted = None;
                return Err(io::Error::other("чтение прервано перемоткой"));
            }
            if let Some(total) = state.total
                && pos >= total
            {
                state.wanted = None;
                return Ok(0);
            }
            if let Some(end) = state.filled_end(pos) {
                state.wanted = None;
                let available = usize::try_from(end - pos).unwrap_or(usize::MAX);
                let len = buf.len().min(available);
                let start = usize::try_from(pos).map_err(io::Error::other)?;
                let source = state
                    .data
                    .get(start..start.saturating_add(len))
                    .ok_or_else(|| io::Error::other("позиция за пределами буфера"))?;
                let target = buf
                    .get_mut(..len)
                    .ok_or_else(|| io::Error::other("позиция за пределами буфера"))?;
                target.copy_from_slice(source);
                return Ok(len);
            }
            if state.failure.is_some() {
                state.wanted = None;
                return Err(io::Error::other("поток прервался"));
            }
            state.wanted = Some(pos);
            state = self
                .changed
                .wait_timeout(state, WAIT_STEP)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
    }
}

/// Читатель буфера для symphonia: `Read + Seek` поверх памяти.
pub struct Reader {
    buffer: Arc<StreamBuffer>,
    pos: u64,
    total: u64,
    /// Что отвечает `is_seekable`. Читатель mp4 в режиме «перематывается»
    /// обходит все атомы файла до конца: у фрагментированного mp4 это сотни
    /// прыжков и столько же запросов Range. Поэтому на открытии и при
    /// перемотке вперёд режим выключают (см. `decode`): чтение идёт подряд, а
    /// загрузка не прыгает.
    seekable: Arc<AtomicBool>,
}

impl Reader {
    /// Ждёт, пока станет известен размер файла. `None` — поток прервался
    /// раньше.
    pub fn open(buffer: Arc<StreamBuffer>) -> Option<Self> {
        let mut state = buffer.lock();
        loop {
            if state.cancelled || state.failure.is_some() {
                return None;
            }
            if let Some(total) = state.total {
                drop(state);
                return Some(Self {
                    buffer,
                    pos: 0,
                    total,
                    seekable: Arc::new(AtomicBool::new(true)),
                });
            }
            state.wanted = Some(0);
            state = buffer
                .changed
                .wait_timeout(state, WAIT_STEP)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
    }
}

impl Reader {
    /// Переключатель «перематывается» для `MediaSource`.
    pub fn seekable_switch(&self) -> Arc<AtomicBool> {
        self.seekable.clone()
    }

    /// Файл начинается как mp4 (`ftyp` или `styp`). Позиция после проверки
    /// возвращается в начало.
    pub fn sniff_mp4(&mut self) -> bool {
        let mut head = [0_u8; 8];
        let mut filled = 0;
        while let Some(rest) = head.get_mut(filled..).filter(|rest| !rest.is_empty()) {
            match self.read(rest) {
                Ok(read) if read > 0 => filled += read,
                _ => break,
            }
        }
        self.pos = 0;
        matches!(head.get(4..8), Some(b"ftyp" | b"styp")) && filled == head.len()
    }
}

impl Read for Reader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.buffer.read_at(self.pos, buf)?;
        self.pos = self.pos.saturating_add(read as u64);
        Ok(read)
    }
}

impl Seek for Reader {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let target = match to {
            SeekFrom::Start(pos) => Some(pos),
            SeekFrom::End(delta) => self.total.checked_add_signed(delta),
            SeekFrom::Current(delta) => self.pos.checked_add_signed(delta),
        };
        let pos = target.ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "позиция до начала файла")
        })?;
        self.pos = pos;
        Ok(pos)
    }
}

impl MediaSource for Reader {
    fn is_seekable(&self) -> bool {
        self.seekable.load(Ordering::SeqCst)
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled(buffer: &StreamBuffer) -> Vec<(u64, u64)> {
        buffer.lock().filled.clone()
    }

    #[test]
    fn merges_ranges() {
        let buffer = StreamBuffer::new();
        buffer.set_total(100).unwrap();
        buffer.write(10, &[1; 10]);
        buffer.write(40, &[1; 10]);
        buffer.write(20, &[1; 20]);
        assert_eq!(filled(&buffer), vec![(10, 50)]);
        buffer.write(95, &[1; 10]);
        assert_eq!(filled(&buffer), vec![(10, 50), (95, 100)]);
    }

    #[test]
    fn next_start_follows_reader_then_gaps() {
        let buffer = StreamBuffer::new();
        assert_eq!(buffer.next_start(), Some(0));
        buffer.set_total(1_000_000).unwrap();
        buffer.write(0, &[0; 1000]);
        assert_eq!(buffer.next_start(), Some(1000));
        // Читатель ждёт далеко впереди: начинаем с границы блока.
        buffer.lock().wanted = Some(700_000);
        assert_eq!(buffer.next_start(), Some(655_360));
        assert!(buffer.should_restart(2000));
        assert!(!buffer.should_restart(600_000));
        // Дочитали до конца — возвращаемся к пропуску в начале.
        {
            let mut state = buffer.lock();
            state.wanted = None;
            state.read_pos = 655_360;
        }
        buffer.write(655_360, &vec![0; 344_640]);
        assert_eq!(buffer.next_start(), Some(1000));
        buffer.write(1000, &vec![0; 654_360]);
        assert_eq!(buffer.next_start(), None);
    }

    /// Читатель, чьи байты уже легли, не повод перезапускать запрос: иначе
    /// каждый запрос отдавал бы один кусок (на каждый — задержка ответа).
    #[test]
    fn satisfied_reader_does_not_restart_download() {
        let buffer = StreamBuffer::new();
        buffer.set_total(10_000_000).unwrap();
        buffer.write(0, &[0; 8192]);
        // Читатель ждал позицию 0, байты легли, `wanted` ещё не сброшен.
        buffer.lock().wanted = Some(0);
        assert!(!buffer.should_restart(8192));
        // Ждёт нескачанное позади — это уже повод.
        buffer.lock().wanted = Some(100_000);
        assert!(buffer.should_restart(8192 + 1_000_000));
        // Ждёт нескачанное далеко впереди — тоже.
        buffer.lock().wanted = Some(5_000_000);
        assert!(buffer.should_restart(8192));
    }

    #[test]
    fn sniff_finds_mp4_and_resets_position() {
        let buffer = StreamBuffer::new();
        let mut data = vec![0, 0, 0, 28];
        data.extend_from_slice(b"ftypisom");
        data.resize(64, 0);
        buffer.set_total(data.len() as u64).unwrap();
        buffer.write(0, &data);
        let mut reader = Reader::open(buffer).unwrap();
        assert!(reader.sniff_mp4());
        let mut out = [0; 4];
        assert_eq!(reader.read(&mut out).unwrap(), 4);
        assert_eq!(out, [0, 0, 0, 28]);
        // Переключатель «перематывается» доходит до `MediaSource`.
        let switch = reader.seekable_switch();
        assert!(reader.is_seekable());
        switch.store(false, Ordering::SeqCst);
        assert!(!reader.is_seekable());

        let other = StreamBuffer::new();
        other.set_total(16).unwrap();
        other.write(0, b"fLaC0000abcdefgh");
        assert!(!Reader::open(other).unwrap().sniff_mp4());
    }

    #[test]
    fn reader_reads_and_seeks() {
        let buffer = StreamBuffer::new();
        buffer.set_total(6).unwrap();
        buffer.write(0, b"abcdef");
        let mut reader = Reader::open(buffer).unwrap();
        let mut out = [0; 4];
        assert_eq!(reader.read(&mut out).unwrap(), 4);
        assert_eq!(&out, b"abcd");
        reader.seek(SeekFrom::End(-1)).unwrap();
        assert_eq!(reader.read(&mut out).unwrap(), 1);
        assert_eq!(out[0], b'f');
        assert_eq!(reader.read(&mut out).unwrap(), 0);
        assert!(reader.seek(SeekFrom::Current(-100)).is_err());
    }

    #[test]
    fn reader_waits_for_writer() {
        let buffer = StreamBuffer::new();
        buffer.set_total(4).unwrap();
        let writer = buffer.clone();
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            writer.write(0, b"wxyz");
        });
        let mut reader = Reader::open(buffer).unwrap();
        let mut out = [0; 4];
        assert_eq!(reader.read(&mut out).unwrap(), 4);
        assert_eq!(&out, b"wxyz");
        handle.join().unwrap();
    }

    #[test]
    fn failure_and_cancel_wake_reader() {
        let buffer = StreamBuffer::new();
        buffer.set_total(4).unwrap();
        buffer.fail(Failure::Network("нет сети".to_owned()));
        let mut reader = Reader::open(buffer.clone());
        assert!(reader.is_none());
        let buffer = StreamBuffer::new();
        buffer.set_total(4).unwrap();
        reader = Reader::open(buffer.clone());
        buffer.cancel();
        let mut out = [0; 4];
        assert!(reader.unwrap().read(&mut out).is_err());
    }

    #[test]
    fn rejects_huge_and_changed_size() {
        let buffer = StreamBuffer::new();
        assert_eq!(buffer.set_total(MAX_LEN + 1), Err(Failure::TooLarge));
        buffer.set_total(10).unwrap();
        assert!(buffer.set_total(11).is_err());
        assert_eq!(buffer.write(8, b"abcd"), 2);
        assert_eq!(buffer.write(20, b"abcd"), 0);
    }
}
