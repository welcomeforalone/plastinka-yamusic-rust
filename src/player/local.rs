//! Свой файл пользователя (замена звука в плагине FckCensor): файл только
//! читается в буфер в памяти. На диск ничего не пишется (правило 1).

use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use super::buffer::{Failure, StreamBuffer};

/// Сколько читать за раз.
const CHUNK: usize = 256 * 1024;

/// Читает файл `path` в `buffer`, пока его не отменят. Ошибки остаются в
/// буфере. Блокирует поток: запускать через `spawn_blocking`.
pub fn run(buffer: &Arc<StreamBuffer>, path: &Path) {
    if let Err(failure) = read(buffer, path) {
        buffer.fail(failure);
    }
}

fn read(buffer: &Arc<StreamBuffer>, path: &Path) -> Result<(), Failure> {
    let unreadable =
        |err: std::io::Error| Failure::Storage(format!("не удалось прочитать файл: {err}"));
    let mut file = std::fs::File::open(path).map_err(unreadable)?;
    let total = file.metadata().map_err(unreadable)?.len();
    if total == 0 {
        return Err(Failure::Storage("файл пустой".to_owned()));
    }
    buffer.set_total(total)?;
    let mut chunk = vec![0; CHUNK];
    let mut offset = 0u64;
    while offset < total {
        if buffer.is_cancelled() {
            return Ok(());
        }
        let read = file.read(&mut chunk).map_err(unreadable)?;
        if read == 0 {
            return Err(Failure::Storage(
                "файл оказался короче, чем при открытии".to_owned(),
            ));
        }
        let bytes = chunk.get(..read).unwrap_or_default();
        buffer.write(offset, bytes);
        offset = offset.saturating_add(read as u64);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_and_empty_files_fail_softly() {
        let buffer = StreamBuffer::new();
        run(&buffer, Path::new("нет-такого-файла.mp3"));
        assert!(matches!(buffer.failure(), Some(Failure::Storage(_))));

        let dir = std::env::temp_dir().join(format!("plastinka-local-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let empty = dir.join("empty.mp3");
        std::fs::write(&empty, b"").unwrap();
        let buffer = StreamBuffer::new();
        run(&buffer, &empty);
        assert!(matches!(buffer.failure(), Some(Failure::Storage(_))));

        let full = dir.join("full.mp3");
        std::fs::write(&full, vec![7u8; CHUNK * 2 + 5]).unwrap();
        let buffer = StreamBuffer::new();
        run(&buffer, &full);
        assert_eq!(buffer.failure(), None);
        assert_eq!(buffer.next_start(), None, "файл прочитан целиком");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
