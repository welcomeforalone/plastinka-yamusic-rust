//! Кадры: 4 байта длины (`u32`, big-endian) и содержимое длиной 1..=65536.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::SyncError;

/// Наибольшая длина содержимого кадра.
pub const MAX_FRAME: usize = 65536;

/// Кадр целиком, как он уходит в сеть: длина и содержимое.
pub fn encode_frame(payload: &[u8]) -> Result<Vec<u8>, SyncError> {
    let len = check_len(payload.len())?;
    let mut out = Vec::with_capacity(payload.len().saturating_add(4));
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(payload);
    Ok(out)
}

fn check_len(len: usize) -> Result<u32, SyncError> {
    if (1..=MAX_FRAME).contains(&len) {
        u32::try_from(len).map_err(|_| SyncError::FrameLength(len))
    } else {
        Err(SyncError::FrameLength(len))
    }
}

/// Пишет кадр и сбрасывает буфер.
pub async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    payload: &[u8],
) -> Result<(), SyncError> {
    let frame = encode_frame(payload)?;
    writer.write_all(&frame).await?;
    writer.flush().await?;
    Ok(())
}

/// Читатель кадров. Отмена `read` безопасна (её можно бросить в `select!`):
/// прочитанное хранится в самом читателе и не теряется.
pub struct FrameReader<R> {
    inner: R,
    header: [u8; 4],
    header_filled: usize,
    body: Vec<u8>,
    body_filled: usize,
}

impl<R: AsyncRead + Unpin> FrameReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            header: [0; 4],
            header_filled: 0,
            body: Vec::new(),
            body_filled: 0,
        }
    }

    /// Содержимое следующего кадра. Неверная длина — ошибка, память под
    /// неё не выделяется.
    pub async fn read(&mut self) -> Result<Vec<u8>, SyncError> {
        while self.header_filled < 4 {
            let Some(rest) = self.header.get_mut(self.header_filled..) else {
                return Err(SyncError::Protocol("сбой разбора кадра"));
            };
            let n = self.inner.read(rest).await?;
            if n == 0 {
                return Err(SyncError::Io(std::io::ErrorKind::UnexpectedEof.into()));
            }
            self.header_filled += n;
            if self.header_filled == 4 {
                let len = usize::try_from(u32::from_be_bytes(self.header)).unwrap_or(usize::MAX);
                check_len(len)?;
                self.body = vec![0; len];
                self.body_filled = 0;
            }
        }
        while self.body_filled < self.body.len() {
            let Some(rest) = self.body.get_mut(self.body_filled..) else {
                return Err(SyncError::Protocol("сбой разбора кадра"));
            };
            let n = self.inner.read(rest).await?;
            if n == 0 {
                return Err(SyncError::Io(std::io::ErrorKind::UnexpectedEof.into()));
            }
            self.body_filled += n;
        }
        self.header_filled = 0;
        self.body_filled = 0;
        Ok(std::mem::take(&mut self.body))
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncWriteExt, duplex};

    use super::*;

    #[tokio::test]
    async fn frame_roundtrip() {
        let (a, b) = duplex(200_000);
        let (mut a, mut reader) = (a, FrameReader::new(b));
        write_frame(&mut a, b"x").await.unwrap();
        let big = vec![7u8; MAX_FRAME];
        write_frame(&mut a, &big).await.unwrap();
        assert_eq!(reader.read().await.unwrap(), b"x");
        assert_eq!(reader.read().await.unwrap(), big);
    }

    #[tokio::test]
    async fn bad_lengths_are_errors() {
        assert!(matches!(encode_frame(&[]), Err(SyncError::FrameLength(0))));
        assert!(encode_frame(&vec![0; MAX_FRAME + 1]).is_err());
        for len in [0u32, 65537, u32::MAX] {
            let (mut a, b) = duplex(64);
            a.write_all(&len.to_be_bytes()).await.unwrap();
            let mut reader = FrameReader::new(b);
            assert!(matches!(
                reader.read().await,
                Err(SyncError::FrameLength(_))
            ));
        }
    }

    #[tokio::test]
    async fn partial_read_survives_cancellation() {
        let (mut a, b) = duplex(64);
        let mut reader = FrameReader::new(b);
        a.write_all(&[0, 0, 0, 3, 1]).await.unwrap();
        let early = tokio::time::timeout(std::time::Duration::from_millis(30), reader.read()).await;
        assert!(early.is_err());
        a.write_all(&[2, 3]).await.unwrap();
        assert_eq!(reader.read().await.unwrap(), [1, 2, 3]);
    }

    #[tokio::test]
    async fn eof_is_an_error() {
        let (mut a, b) = duplex(64);
        a.write_all(&[0, 0, 0, 5, 1]).await.unwrap();
        drop(a);
        let mut reader = FrameReader::new(b);
        assert!(matches!(reader.read().await, Err(SyncError::Io(_))));
    }
}
