//! Один экземпляр программы. Первый запуск держит блокировку на файле
//! `instance.lock` в каталоге кэша и слушает канал «показать»; второй
//! запуск не открывает окно, а просит первый показать своё и выходит.
//!
//! Блокировка — `File::try_lock` из std: система снимает её сама, когда
//! процесс завершился (в том числе аварийно), поэтому «застрявшего» замка
//! не бывает. Канал — именованный канал Windows или сокет Unix; по нему
//! передаётся одна строка `show`, остальное игнорируется.
//!
//! Если блокировку поставить нельзя (нет каталога кэша, ошибка ввода-вывода),
//! программа работает как единственная и канал не открывает: проверка не
//! должна мешать запуску.

use std::fs::{File, TryLockError};
use std::io;
use std::path::Path;
#[cfg(any(unix, test))]
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use iced::futures::{SinkExt, Stream};

use crate::storage::{self, AppDirs};

/// Единственная строка, которую принимает канал.
const SHOW: &str = "show";
/// Сколько ждать блокировку после обновления: старый процесс ещё выходит.
const AFTER_UPDATE_WAIT: Duration = Duration::from_secs(10);
/// Как часто повторять попытку блокировки и подключения к каналу.
const RETRY_EVERY: Duration = Duration::from_millis(200);
/// Сколько пробовать подключиться к первому экземпляру, пока он не открыл канал.
const CONNECT_WAIT: Duration = Duration::from_secs(2);
/// Сколько читать строку у подключившегося: канал не должен висеть.
const READ_LIMIT: Duration = Duration::from_secs(2);

/// Файл блокировки живёт, пока работает программа.
static LOCK: OnceLock<File> = OnceLock::new();

/// Итог попытки стать первым экземпляром.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Claim {
    /// Работаем одни (блокировка взята или проверить не удалось).
    First,
    /// Уже запущен другой экземпляр.
    Running,
}

/// Имя файла блокировки. С тестовыми хуками оно своё, чтобы тестовый запуск
/// не трогал настоящее окно пользователя.
fn lock_file_name(test: bool) -> &'static str {
    if test {
        "instance-test.lock"
    } else {
        "instance.lock"
    }
}

/// Имя именованного канала Windows для пользователя `user`.
#[cfg(any(windows, test))]
fn pipe_name(user: &str, test: bool) -> String {
    let user: String = user
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let suffix = if test { "-test" } else { "" };
    format!(r"\\.\pipe\plastinka-{user}{suffix}")
}

/// Путь сокета Unix: в каталоге выполнения пользователя, а если его нет — в
/// каталоге кэша.
#[cfg(any(unix, test))]
fn socket_path(runtime_dir: Option<&Path>, cache_dir: &Path, test: bool) -> PathBuf {
    let name = if test {
        "plastinka-test.sock"
    } else {
        "plastinka.sock"
    };
    runtime_dir.unwrap_or(cache_dir).join(name)
}

/// Разбор строки канала: только `show`, остальное игнорируется.
fn is_show(line: &str) -> bool {
    line.trim() == SHOW
}

/// Пытается стать первым экземпляром. `wait` — после обновления: ждёт до
/// 10 секунд, пока старый процесс отпустит блокировку.
pub fn claim(wait: bool) -> Claim {
    let Ok(dirs) = AppDirs::discover() else {
        return Claim::First;
    };
    let test = storage::test_hooks_active();
    let path = dirs.cache_dir().join(lock_file_name(test));
    let deadline = Instant::now()
        + if wait {
            AFTER_UPDATE_WAIT
        } else {
            Duration::ZERO
        };
    loop {
        match try_claim(&path) {
            Ok(Some(file)) => {
                // Файл живёт до конца процесса: без него блокировки нет.
                let _ = LOCK.set(file);
                return Claim::First;
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    return Claim::Running;
                }
                std::thread::sleep(RETRY_EVERY);
            }
            Err(_) => return Claim::First,
        }
    }
}

/// `Ok(Some)` — блокировка взята, `Ok(None)` — файл занят другим процессом.
fn try_claim(path: &Path) -> io::Result<Option<File>> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(err)) => Err(err),
    }
}

/// Слушает ли этот экземпляр канал «показать» (блокировка взята).
pub fn serving() -> bool {
    LOCK.get().is_some()
}

/// Просит первый экземпляр показать окно. Первый мог ещё не открыть канал,
/// поэтому подключение повторяется до двух секунд. `true` — просьба передана.
pub fn ask_to_show() -> bool {
    let Ok(dirs) = AppDirs::discover() else {
        return false;
    };
    let test = storage::test_hooks_active();
    let deadline = Instant::now() + CONNECT_WAIT;
    loop {
        if send_show(&dirs, test).is_ok() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(RETRY_EVERY);
    }
}

#[cfg(windows)]
fn send_show(_dirs: &AppDirs, test: bool) -> io::Result<()> {
    use std::io::Write as _;
    let user = std::env::var("USERNAME").unwrap_or_default();
    let mut pipe = File::options().write(true).open(pipe_name(&user, test))?;
    pipe.write_all(format!("{SHOW}\n").as_bytes())
}

#[cfg(unix)]
fn send_show(dirs: &AppDirs, test: bool) -> io::Result<()> {
    use std::io::Write as _;
    let mut stream = std::os::unix::net::UnixStream::connect(socket_path(
        runtime_dir().as_deref(),
        dirs.cache_dir(),
        test,
    ))?;
    stream.write_all(format!("{SHOW}\n").as_bytes())
}

/// `$XDG_RUNTIME_DIR`, если задан и не пуст.
#[cfg(unix)]
fn runtime_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
}

/// Поток просьб «показать» для подписки окна. Пока блокировка не взята или
/// канал открыть не вышло, поток пуст и сразу заканчивается.
pub fn requests() -> impl Stream<Item = ()> {
    iced::stream::channel(
        8,
        async |mut output: iced::futures::channel::mpsc::Sender<()>| {
            let Ok(dirs) = AppDirs::discover() else {
                return;
            };
            let test = storage::test_hooks_active();
            let mut listener = match Listener::open(&dirs, test) {
                Ok(listener) => listener,
                Err(_) => return,
            };
            loop {
                match listener.accept().await {
                    Ok(Some(())) => {
                        if output.send(()).await.is_err() {
                            return;
                        }
                    }
                    // Чужая или пустая строка.
                    Ok(None) => {}
                    // Канал сломался: повторять бесполезно.
                    Err(_) => return,
                }
            }
        },
    )
}

/// Читает одну строку у подключившегося; `Some(())` — это `show`.
async fn read_request<R>(reader: R) -> Option<()>
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
    // Строка короткая: длиннее 64 байт не читаем.
    let mut reader = BufReader::new(reader.take(64));
    let mut line = String::new();
    let read = tokio::time::timeout(READ_LIMIT, reader.read_line(&mut line)).await;
    matches!(read, Ok(Ok(count)) if count > 0)
        .then(|| is_show(&line))
        .filter(|show| *show)
        .map(|_| ())
}

#[cfg(windows)]
struct Listener {
    name: String,
    server: tokio::net::windows::named_pipe::NamedPipeServer,
}

#[cfg(windows)]
impl Listener {
    fn open(_dirs: &AppDirs, test: bool) -> io::Result<Self> {
        use tokio::net::windows::named_pipe::ServerOptions;
        let user = std::env::var("USERNAME").unwrap_or_default();
        let name = pipe_name(&user, test);
        let server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&name)?;
        Ok(Self { name, server })
    }

    /// Ждёт подключения; `Some(())` — пришло `show`.
    async fn accept(&mut self) -> io::Result<Option<()>> {
        use tokio::net::windows::named_pipe::ServerOptions;
        self.server.connect().await?;
        // Следующий экземпляр канала создаём сразу: между двумя подключениями
        // канала не должно быть «дыры».
        let next = ServerOptions::new().create(&self.name)?;
        let connected = std::mem::replace(&mut self.server, next);
        Ok(read_request(connected).await)
    }
}

#[cfg(unix)]
struct Listener {
    listener: tokio::net::UnixListener,
}

#[cfg(unix)]
impl Listener {
    fn open(dirs: &AppDirs, test: bool) -> io::Result<Self> {
        let path = socket_path(runtime_dir().as_deref(), dirs.cache_dir(), test);
        // Блокировку держим мы, значит старый файл сокета ничей.
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
        Ok(Self {
            listener: tokio::net::UnixListener::bind(&path)?,
        })
    }

    /// Ждёт подключения; `Some(())` — пришло `show`.
    async fn accept(&mut self) -> io::Result<Option<()>> {
        let (stream, _) = self.listener.accept().await?;
        Ok(read_request(stream).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Каталог теста во временной папке; удаляется при выходе из теста.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "plastinka-instance-{name}-{}",
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(&dir).expect("каталог создаётся");
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn test_names_differ_from_real_ones() {
        assert_ne!(lock_file_name(true), lock_file_name(false));
        assert_ne!(pipe_name("anna", true), pipe_name("anna", false));
        let cache = Path::new("/cache");
        assert_ne!(
            socket_path(None, cache, true),
            socket_path(None, cache, false)
        );
    }

    #[test]
    fn pipe_name_keeps_user_and_drops_odd_characters() {
        assert_eq!(pipe_name("anna", false), r"\\.\pipe\plastinka-anna");
        assert_eq!(
            pipe_name("Анна Иванова", false),
            r"\\.\pipe\plastinka-Анна_Иванова"
        );
        assert_eq!(pipe_name(r"a\b/c", true), r"\\.\pipe\plastinka-a_b_c-test");
    }

    #[test]
    fn socket_lives_in_runtime_dir_or_cache() {
        let cache = Path::new("/cache");
        let runtime = Path::new("/run/user/1000");
        assert_eq!(
            socket_path(Some(runtime), cache, false),
            runtime.join("plastinka.sock")
        );
        assert_eq!(
            socket_path(None, cache, false),
            cache.join("plastinka.sock")
        );
    }

    #[test]
    fn only_show_is_accepted() {
        assert!(is_show("show\n"));
        assert!(is_show("show"));
        assert!(is_show("  show \r\n"));
        assert!(!is_show("quit\n"));
        assert!(!is_show(""));
        assert!(!is_show("show me"));
    }

    #[test]
    fn second_claim_of_same_file_is_refused() {
        let dir = TempDir::new("lock");
        let path = dir.0.join("nested").join("instance.lock");
        let first = try_claim(&path).expect("первая попытка без ошибок");
        assert!(first.is_some(), "первый берёт блокировку");
        let second = try_claim(&path).expect("вторая попытка без ошибок");
        assert!(second.is_none(), "второй видит занятый файл");
        drop(first);
        let third = try_claim(&path).expect("третья попытка без ошибок");
        assert!(third.is_some(), "после выхода первого замок свободен");
    }

    #[tokio::test]
    async fn request_reader_accepts_show_only() {
        assert_eq!(read_request(&b"show\n"[..]).await, Some(()));
        assert_eq!(read_request(&b"quit\n"[..]).await, None);
        assert_eq!(read_request(&b""[..]).await, None);
    }
}
