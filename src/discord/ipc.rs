//! Локальный IPC Discord: кадры и поиск канала или сокета.
//!
//! Кадр: код операции (u32, little-endian), длина тела (u32, little-endian),
//! затем JSON в UTF-8.

use std::path::PathBuf;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Коды операций.
pub const OP_HANDSHAKE: u32 = 0;
pub const OP_FRAME: u32 = 1;
pub const OP_CLOSE: u32 = 2;
pub const OP_PING: u32 = 3;
pub const OP_PONG: u32 = 4;

/// Тело кадра не больше этого при чтении: чужой процесс не заставит нас
/// выделить гигабайт.
pub const MAX_BODY: usize = 64 * 1024;
/// Сколько каналов `discord-ipc-N` пробуем.
pub const SLOTS: u32 = 10;

/// Собирает кадр.
pub fn encode(op: u32, body: &[u8]) -> Vec<u8> {
    let len = u32::try_from(body.len()).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(8 + body.len());
    out.extend_from_slice(&op.to_le_bytes());
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(body);
    out
}

/// Ошибки разбора кадра из буфера (разбор потока — `read_frame`).
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    /// Заголовок короче 8 байт или тело неполное.
    Short,
    /// Заявленная длина больше предела.
    TooLong,
}

#[cfg(test)]
/// Разбирает один кадр из начала `data`: код, тело и сколько байт занял.
pub fn decode(data: &[u8]) -> Result<(u32, Vec<u8>, usize), FrameError> {
    let header = data.get(..8).ok_or(FrameError::Short)?;
    let op = u32::from_le_bytes(
        header
            .get(..4)
            .and_then(|b| b.try_into().ok())
            .ok_or(FrameError::Short)?,
    );
    let len = u32::from_le_bytes(
        header
            .get(4..8)
            .and_then(|b| b.try_into().ok())
            .ok_or(FrameError::Short)?,
    );
    let len = usize::try_from(len).map_err(|_| FrameError::TooLong)?;
    if len > MAX_BODY {
        return Err(FrameError::TooLong);
    }
    let body = data.get(8..8 + len).ok_or(FrameError::Short)?;
    Ok((op, body.to_vec(), 8 + len))
}

/// Читает кадр из потока. `None` — поток закрыт, оборван или прислал
/// слишком длинный кадр: соединение больше не годится.
pub async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> Option<(u32, Vec<u8>)> {
    let mut header = [0_u8; 8];
    reader.read_exact(&mut header).await.ok()?;
    let op = u32::from_le_bytes(header.get(..4)?.try_into().ok()?);
    let len = usize::try_from(u32::from_le_bytes(header.get(4..8)?.try_into().ok()?)).ok()?;
    if len > MAX_BODY {
        return None;
    }
    let mut body = vec![0_u8; len];
    reader.read_exact(&mut body).await.ok()?;
    Some((op, body))
}

/// Пишет кадр. `false` — запись не удалась.
pub async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, op: u32, body: &[u8]) -> bool {
    let frame = encode(op, body);
    writer.write_all(&frame).await.is_ok() && writer.flush().await.is_ok()
}

/// Двусторонний поток к Discord.
pub trait Duplex: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> Duplex for T {}

/// Имя канала или сокета с номером `slot`.
fn socket_name(slot: u32) -> String {
    format!("discord-ipc-{slot}")
}

/// Каталоги Linux, где Discord кладёт сокет, по переменным окружения.
/// Для каждого базового каталога (`XDG_RUNTIME_DIR`, иначе `TMPDIR`, `TMP`,
/// `TEMP`, иначе `/tmp`) — он сам и подкаталоги Flatpak и Snap.
pub fn unix_dirs(get: impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let base = ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"]
        .into_iter()
        .find_map(|name| get(name).filter(|value| !value.is_empty()))
        .map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
    [
        "",
        "app/com.discordapp.Discord",
        "snap.discord",
        ".flatpak/dev.vencord.Vesktop/xdg-run",
    ]
    .into_iter()
    .map(|sub| {
        if sub.is_empty() {
            base.clone()
        } else {
            base.join(sub)
        }
    })
    .collect()
}

/// Все пути сокетов, которые стоит попробовать, по порядку.
#[cfg_attr(windows, allow(dead_code))]
pub fn unix_paths(get: impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let dirs = unix_dirs(get);
    (0..SLOTS)
        .flat_map(|slot| {
            let name = socket_name(slot);
            dirs.iter()
                .map(move |dir| dir.join(&name))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Подключается к первому доступному `discord-ipc-N`.
#[cfg(windows)]
pub async fn connect() -> Option<Box<dyn Duplex>> {
    use tokio::net::windows::named_pipe::ClientOptions;
    for slot in 0..SLOTS {
        let name = format!(r"\\.\pipe\{}", socket_name(slot));
        if let Ok(pipe) = ClientOptions::new().open(&name) {
            return Some(Box::new(pipe));
        }
    }
    None
}

/// Подключается к первому доступному сокету `discord-ipc-N`.
#[cfg(unix)]
pub async fn connect() -> Option<Box<dyn Duplex>> {
    use tokio::net::UnixStream;
    for path in unix_paths(|name| std::env::var(name).ok()) {
        if let Ok(stream) = UnixStream::connect(&path).await {
            return Some(Box::new(stream));
        }
    }
    None
}

#[cfg(not(any(windows, unix)))]
pub async fn connect() -> Option<Box<dyn Duplex>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trip() {
        let frame = encode(OP_FRAME, br#"{"cmd":"X"}"#);
        assert_eq!(frame.get(..4), Some(&[1, 0, 0, 0][..]));
        assert_eq!(frame.get(4..8), Some(&[11, 0, 0, 0][..]));
        let (op, body, used) = decode(&frame).unwrap();
        assert_eq!(
            (op, body.as_slice(), used),
            (OP_FRAME, &br#"{"cmd":"X"}"#[..], frame.len())
        );
        // Следующий кадр в том же буфере не мешает.
        let mut two = frame.clone();
        two.extend(encode(OP_PING, b"1"));
        let (_, _, used) = decode(&two).unwrap();
        let (op, body, _) = decode(two.get(used..).unwrap()).unwrap();
        assert_eq!((op, body.as_slice()), (OP_PING, &b"1"[..]));
    }

    #[test]
    fn short_and_long_frames_are_rejected() {
        assert_eq!(decode(&[1, 0, 0]), Err(FrameError::Short));
        // Заголовок есть, тела не хватает.
        let mut frame = encode(OP_FRAME, b"abcd");
        frame.truncate(10);
        assert_eq!(decode(&frame), Err(FrameError::Short));
        // Длина больше предела: тело не читаем вовсе.
        let mut huge = Vec::new();
        huge.extend_from_slice(&1_u32.to_le_bytes());
        huge.extend_from_slice(&(MAX_BODY as u32 + 1).to_le_bytes());
        assert_eq!(decode(&huge), Err(FrameError::TooLong));
        let mut max = Vec::new();
        max.extend_from_slice(&1_u32.to_le_bytes());
        max.extend_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(decode(&max), Err(FrameError::TooLong));
    }

    #[tokio::test]
    async fn read_and_write_through_a_stream() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        assert!(write_frame(&mut a, OP_HANDSHAKE, br#"{"v":1}"#).await);
        let (op, body) = read_frame(&mut b).await.unwrap();
        assert_eq!((op, body.as_slice()), (OP_HANDSHAKE, &br#"{"v":1}"#[..]));
        // Обрыв и слишком длинный кадр — конец соединения.
        drop(a);
        assert!(read_frame(&mut b).await.is_none());
        let (mut c, mut d) = tokio::io::duplex(1024);
        let mut huge = Vec::new();
        huge.extend_from_slice(&1_u32.to_le_bytes());
        huge.extend_from_slice(&(MAX_BODY as u32 + 1).to_le_bytes());
        c.write_all(&huge).await.unwrap();
        assert!(read_frame(&mut d).await.is_none());
    }

    #[test]
    fn unix_paths_follow_environment() {
        let env = |name: &str| match name {
            "XDG_RUNTIME_DIR" => Some("/run/user/1000".to_owned()),
            "TMPDIR" => Some("/ignored".to_owned()),
            _ => None,
        };
        let dirs = unix_dirs(env);
        assert_eq!(dirs.first(), Some(&PathBuf::from("/run/user/1000")));
        assert_eq!(
            dirs.get(1),
            Some(&PathBuf::from("/run/user/1000/app/com.discordapp.Discord"))
        );
        assert_eq!(
            dirs.get(2),
            Some(&PathBuf::from("/run/user/1000/snap.discord"))
        );
        assert_eq!(
            dirs.get(3),
            Some(&PathBuf::from(
                "/run/user/1000/.flatpak/dev.vencord.Vesktop/xdg-run"
            ))
        );
        // Без XDG — TMPDIR, потом TMP, TEMP; совсем без — /tmp.
        let tmpdir = |name: &str| (name == "TMPDIR").then(|| "/var/tmp".to_owned());
        assert_eq!(unix_dirs(tmpdir).first(), Some(&PathBuf::from("/var/tmp")));
        let temp = |name: &str| (name == "TEMP").then(|| "/t".to_owned());
        assert_eq!(unix_dirs(temp).first(), Some(&PathBuf::from("/t")));
        assert_eq!(unix_dirs(|_| None).first(), Some(&PathBuf::from("/tmp")));
        // Пустая переменная не считается.
        let empty = |name: &str| (name == "XDG_RUNTIME_DIR").then(String::new);
        assert_eq!(unix_dirs(empty).first(), Some(&PathBuf::from("/tmp")));
        // Номера идут по порядку, внутри — каталоги по порядку.
        let paths = unix_paths(|_| None);
        assert_eq!(paths.len(), 40);
        assert_eq!(paths.first(), Some(&PathBuf::from("/tmp/discord-ipc-0")));
        assert_eq!(paths.get(4), Some(&PathBuf::from("/tmp/discord-ipc-1")));
    }
}
