//! Автообновление: проверка свежего релиза на GitHub, скачивание файла,
//! сверка SHA-256 и замена программы. Логика без окна.
//!
//! Ходим только на GitHub (`api.github.com`, `github.com` и редиректы на
//! `*.githubusercontent.com`) и без какого-либо токена. Скачанное держим в
//! памяти, контрольная сумма проверяется до записи на диск. Спека запросов —
//! `docs/api/update.md`.

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::net::{self, NetError};

/// Репозиторий с релизами. Его могут переименовать: строка живёт только здесь.
pub const UPDATE_REPO: &str = "welcomeforalone/plastinka";

/// Версия этой сборки.
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Во сколько байт укладывается ответ API и файл с контрольной суммой.
const API_LIMIT: usize = 1024 * 1024;
const SHA_LIMIT: usize = 4096;
/// Предел размера скачиваемого файла: так чужой ответ не заполнит память.
const PACKAGE_LIMIT: usize = 300 * 1024 * 1024;
/// Сколько переадресаций разрешено.
const MAX_REDIRECTS: usize = 5;
/// Служебный флаг при перезапуске после обновления.
pub const AFTER_UPDATE_FLAG: &str = "--after-update";

/// Файлы релиза по платформам.
const WINDOWS_ASSET: &str = "Plastinka.exe";
const LINUX_ASSET: &str = "plastinka-linux-x86_64.tar.gz";
/// Путь бинарника внутри архива для Linux.
const LINUX_ENTRY: &str = "plastinka-linux-x86_64/Plastinka";
/// Имя установленного бинарника в `~/.local/bin` (его кладёт `install.sh`).
const LINUX_BIN_NAME: &str = "Plastinka";

/// Ошибки проверки и установки обновления. Тексты — для пользователя.
#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("{0}")]
    Net(#[from] NetError),
    #[error("Релиз не найден: репозиторий закрыт или в нём ещё нет релизов")]
    NotFound,
    #[error("GitHub временно ограничил число проверок. Повторите позже")]
    RateLimited,
    #[error("GitHub ответил ошибкой {0}")]
    Http(u16),
    #[error("Не удалось разобрать ответ GitHub")]
    BadResponse,
    #[error("Файл с контрольной суммой повреждён. Обновление не установлено")]
    BadChecksumFile,
    #[error("Файл обновления повреждён (контрольная сумма не совпала). Обновление не установлено")]
    Corrupt,
    #[error("Не удалось распаковать архив обновления: {0}")]
    Unpack(String),
    #[error("Не удалось заменить файл программы: {0}")]
    Replace(String),
    #[error("Эту установку нельзя обновить автоматически")]
    NoTarget,
}

/// Платформа, для которой выбираем файл релиза.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    LinuxX64,
    Other,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            Self::LinuxX64
        } else {
            Self::Other
        }
    }

    /// Имя файла релиза, который скачиваем. Для остальных платформ его нет.
    fn asset_name(self) -> Option<&'static str> {
        match self {
            Self::Windows => Some(WINDOWS_ASSET),
            Self::LinuxX64 => Some(LINUX_ASSET),
            Self::Other => None,
        }
    }
}

/// Файл, который можно скачать и установить.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    pub name: String,
    pub url: String,
    pub sha_url: String,
}

/// Найденное обновление. Без `package` — только уведомление со ссылкой.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Available {
    pub version: String,
    pub page_url: String,
    pub package: Option<Package>,
}

/// Итог проверки.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Check {
    /// Новее ничего нет; внутри — версия этой сборки.
    UpToDate(String),
    Available(Available),
}

/// Итог установки.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Installed {
    /// Файл заменён; `exe` — куда записана новая версия (её и запускаем).
    Replaced { exe: PathBuf },
    /// Отладочный хук без цели: скачано и проверено, ничего не заменено.
    Verified,
}

/// Куда ставить обновление.
enum Target {
    Replace(PathBuf),
    VerifyOnly,
}

// ------------------------------------------------------------ отладочные хуки

/// Отладочный хук: значение переменной окружения. В релизе кода хука нет.
#[cfg(debug_assertions)]
fn hook_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[cfg(not(debug_assertions))]
fn hook_env(_name: &str) -> Option<String> {
    None
}

/// Хук `PLASTINKA_TEST_UPDATE_URL` задан: адрес API и хосты любые, http можно.
fn hook_url() -> Option<String> {
    hook_env("PLASTINKA_TEST_UPDATE_URL")
}

/// Версия этой сборки; при хуке её можно подменить `PLASTINKA_TEST_UPDATE_VERSION`.
fn current_version() -> String {
    if hook_url().is_some()
        && let Some(version) = hook_env("PLASTINKA_TEST_UPDATE_VERSION")
    {
        return version;
    }
    CURRENT_VERSION.to_owned()
}

// ------------------------------------------------------------------ версии

/// Разобранная версия: числовые части и признак пререлиза.
#[derive(Debug, PartialEq, Eq)]
struct Version {
    parts: Vec<u64>,
    pre: bool,
}

/// Версия без `v` в начале: так её показываем.
pub fn clean_version(tag: &str) -> &str {
    tag.trim().trim_start_matches(['v', 'V'])
}

fn parse_version(text: &str) -> Option<Version> {
    let text = clean_version(text);
    // Метаданные сборки (`+...`) на порядок не влияют.
    let text = text.split('+').next()?;
    let (core, pre) = match text.split_once('-') {
        Some((core, _)) => (core, true),
        None => (text, false),
    };
    let mut parts = Vec::new();
    for part in core.split('.') {
        if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        parts.push(part.parse::<u64>().ok()?);
    }
    (!parts.is_empty() && parts.len() <= 4).then_some(Version { parts, pre })
}

/// Новее ли `tag`, чем `current`. Пререлиз и неразобранная версия — «нет».
pub fn is_newer(current: &str, tag: &str) -> bool {
    let (Some(current), Some(new)) = (parse_version(current), parse_version(tag)) else {
        return false;
    };
    if new.pre {
        return false;
    }
    let len = current.parts.len().max(new.parts.len());
    for index in 0..len {
        let old = current.parts.get(index).copied().unwrap_or(0);
        let next = new.parts.get(index).copied().unwrap_or(0);
        if next != old {
            return next > old;
        }
    }
    false
}

// ------------------------------------------------------------- разбор ответа

#[derive(Debug, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

/// Нужные поля ответа `releases/latest`.
#[derive(Debug, Deserialize)]
pub struct Release {
    tag_name: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<Asset>,
}

pub fn parse_release(body: &[u8]) -> Result<Release, UpdateError> {
    serde_json::from_slice(body).map_err(|_| UpdateError::BadResponse)
}

/// Первое слово файла `.sha256` (формат `sha256sum`): ровно 64 hex-символа.
/// Результат — в нижнем регистре.
pub fn parse_sha256(text: &str) -> Option<String> {
    let word = text.split_whitespace().next()?;
    (word.len() == 64 && word.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| word.to_ascii_lowercase())
}

/// Совпадает ли SHA-256 байтов с ожидаемым (без учёта регистра).
pub fn verify_sha256(bytes: &[u8], expected: &str) -> bool {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex.eq_ignore_ascii_case(expected.trim())
}

// ------------------------------------------------------------------- адреса

/// Хост из списка разрешённых: GitHub и его хранилища файлов.
pub fn host_allowed(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    host == "github.com" || host == "api.github.com" || host.ends_with(".githubusercontent.com")
}

/// Можно ли ходить на адрес: https и разрешённый хост. При хуке — любой.
fn url_allowed(url: &reqwest::Url, hook: bool) -> bool {
    hook || (url.scheme() == "https" && url.host_str().is_some_and(host_allowed))
}

fn str_url_allowed(url: &str, hook: bool) -> bool {
    reqwest::Url::parse(url).is_ok_and(|url| url_allowed(&url, hook))
}

fn latest_page() -> String {
    format!("https://github.com/{UPDATE_REPO}/releases/latest")
}

fn api_url() -> String {
    format!("https://api.github.com/repos/{UPDATE_REPO}/releases/latest")
}

/// Решает, есть ли обновление и что можно скачать. `can_install` — эту
/// установку можно обновить сами; иначе только ссылка на страницу релиза.
pub fn evaluate(
    release: &Release,
    current: &str,
    platform: Platform,
    can_install: bool,
    hook: bool,
) -> Check {
    if release.draft || release.prerelease || !is_newer(current, &release.tag_name) {
        return Check::UpToDate(current.to_owned());
    }
    let page_url = if str_url_allowed(&release.html_url, hook) {
        release.html_url.clone()
    } else {
        latest_page()
    };
    let package = can_install
        .then(|| platform.asset_name())
        .flatten()
        .and_then(|name| {
            let find = |wanted: &str| {
                release
                    .assets
                    .iter()
                    .find(|asset| asset.name == wanted)
                    .map(|asset| asset.browser_download_url.clone())
                    .filter(|url| str_url_allowed(url, hook))
            };
            let url = find(name)?;
            let sha_url = find(&format!("{name}.sha256"))?;
            Some(Package {
                name: name.to_owned(),
                url,
                sha_url,
            })
        });
    Check::Available(Available {
        version: clean_version(&release.tag_name).to_owned(),
        page_url,
        package,
    })
}

// -------------------------------------------------------------------- сеть

/// HTTP-клиент обновлений: https, редиректы только на GitHub, без общего
/// таймаута (файл большой). При хуке разрешены http и любые хосты.
fn client(hook: bool) -> Result<reqwest::Client, NetError> {
    let policy = reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= MAX_REDIRECTS {
            attempt.error("слишком много переадресаций")
        } else if url_allowed(attempt.url(), hook) {
            attempt.follow()
        } else {
            attempt.error("переадресация на посторонний адрес запрещена")
        }
    });
    reqwest::Client::builder()
        .user_agent(concat!("Plastinka/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(30))
        .redirect(policy)
        .https_only(!hook)
        .build()
        .map_err(NetError::from)
}

/// GET без токена; тело не больше `limit` байт.
async fn fetch(
    client: &reqwest::Client,
    url: &str,
    accept: &str,
    limit: usize,
) -> Result<Vec<u8>, UpdateError> {
    let response = client
        .get(url)
        .header("Accept", accept)
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .map_err(NetError::from)?;
    let status = response.status();
    match status.as_u16() {
        404 => return Err(UpdateError::NotFound),
        403 | 429 => return Err(UpdateError::RateLimited),
        _ if !status.is_success() => return Err(UpdateError::Http(status.as_u16())),
        _ => {}
    }
    Ok(net::read_limited(response, limit).await?)
}

/// Спрашивает GitHub о последнем релизе.
pub async fn check() -> Result<Check, UpdateError> {
    let hook = hook_url();
    let client = client(hook.is_some())?;
    let url = hook.clone().unwrap_or_else(api_url);
    let body = fetch(&client, &url, "application/vnd.github+json", API_LIMIT).await?;
    let release = parse_release(&body)?;
    let platform = Platform::current();
    Ok(evaluate(
        &release,
        &current_version(),
        platform,
        target(platform).is_some(),
        hook.is_some(),
    ))
}

/// Скачивает файл, сверяет SHA-256 и только потом пишет его на диск.
pub async fn install(package: Package) -> Result<Installed, UpdateError> {
    let hook = hook_url().is_some();
    let platform = Platform::current();
    let target = target(platform).ok_or(UpdateError::NoTarget)?;
    let client = client(hook)?;
    let sums = fetch(
        &client,
        &package.sha_url,
        "application/octet-stream",
        SHA_LIMIT,
    )
    .await?;
    let expected =
        parse_sha256(&String::from_utf8_lossy(&sums)).ok_or(UpdateError::BadChecksumFile)?;
    let bytes = fetch(
        &client,
        &package.url,
        "application/octet-stream",
        PACKAGE_LIMIT,
    )
    .await?;
    if !verify_sha256(&bytes, &expected) {
        return Err(UpdateError::Corrupt);
    }
    match target {
        Target::VerifyOnly => Ok(Installed::Verified),
        Target::Replace(path) => {
            let exe = path.clone();
            tokio::task::spawn_blocking(move || write_update(platform, &path, &bytes))
                .await
                .map_err(|_| UpdateError::Replace("операция прервана".to_owned()))??;
            Ok(Installed::Replaced { exe })
        }
    }
}

// ------------------------------------------------------------ куда ставить

/// Что обновляем. `None` — эту установку сами не обновляем (macOS, Linux не из
/// `~/.local/bin`). При хуке цель — файл из `PLASTINKA_TEST_UPDATE_TARGET`,
/// без него замены нет.
fn target(platform: Platform) -> Option<Target> {
    if hook_url().is_some() {
        return Some(
            hook_env("PLASTINKA_TEST_UPDATE_TARGET").map_or(Target::VerifyOnly, |path| {
                Target::Replace(PathBuf::from(path))
            }),
        );
    }
    match platform {
        Platform::Windows => std::env::current_exe().ok().map(Target::Replace),
        Platform::LinuxX64 => linux_target().map(Target::Replace),
        Platform::Other => None,
    }
}

/// Каталог `${XDG_BIN_HOME:-$HOME/.local/bin}`, куда ставит `install.sh`.
fn bin_dir(xdg_bin_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    if let Some(dir) = xdg_bin_home.filter(|dir| !dir.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    let home = home.filter(|home| !home.is_empty())?;
    Some(PathBuf::from(home).join(".local").join("bin"))
}

/// Запущенный бинарник — тот, что поставил `install.sh`.
fn linux_target() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let installed =
        bin_dir(std::env::var_os("XDG_BIN_HOME"), std::env::var_os("HOME"))?.join(LINUX_BIN_NAME);
    let same = fs::canonicalize(&exe).ok()? == fs::canonicalize(&installed).ok()?;
    same.then_some(exe)
}

// ------------------------------------------------------------------ замена

/// `путь` + суффикс в имени файла (`Plastinka.exe` → `Plastinka.exe.new`).
fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn replace_error(err: &io::Error) -> UpdateError {
    UpdateError::Replace(err.to_string())
}

/// Записывает файл целиком и сбрасывает его на диск; в Unix — права 755.
fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    // Старый остаток мог остаться с другими правами.
    let _ = fs::remove_file(path);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o755);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Windows: запущенный exe можно переименовать, но не перезаписать. Новый файл
/// пишем рядом (`.new`), старый уводим в `.old`, новый ставим на место. Любая
/// ошибка откатывает всё назад.
fn replace_with_backup(target: &Path, bytes: &[u8]) -> Result<(), UpdateError> {
    let new = suffixed(target, ".new");
    let old = suffixed(target, ".old");
    // Остаток прошлого обновления.
    let _ = fs::remove_file(&old);
    if let Err(err) = write_new(&new, bytes) {
        let _ = fs::remove_file(&new);
        return Err(replace_error(&err));
    }
    if let Err(err) = fs::rename(target, &old) {
        let _ = fs::remove_file(&new);
        return Err(replace_error(&err));
    }
    if let Err(err) = fs::rename(&new, target) {
        // Возвращаем прежний файл на место.
        let _ = fs::rename(&old, target);
        let _ = fs::remove_file(&new);
        return Err(replace_error(&err));
    }
    Ok(())
}

/// Linux: новый файл рядом (`.new`, права 755), затем `rename` поверх. Замена
/// запущенного файла в Linux безопасна.
fn replace_in_place(target: &Path, bytes: &[u8]) -> Result<(), UpdateError> {
    let new = suffixed(target, ".new");
    if let Err(err) = write_new(&new, bytes) {
        let _ = fs::remove_file(&new);
        return Err(replace_error(&err));
    }
    if let Err(err) = fs::rename(&new, target) {
        let _ = fs::remove_file(&new);
        return Err(replace_error(&err));
    }
    Ok(())
}

/// Достаёт бинарник из `.tar.gz` системной командой `tar`: архив подаётся в
/// stdin, файл читается из stdout, на диск ничего не пишется.
fn extract_linux_binary(archive: &[u8]) -> Result<Vec<u8>, UpdateError> {
    use std::process::{Command, Stdio};
    let unpack = |err: io::Error| UpdateError::Unpack(err.to_string());
    let mut child = Command::new("tar")
        .args(["-xzOf", "-", LINUX_ENTRY])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(unpack)?;
    let stdin = child.stdin.take();
    // Запись в отдельном потоке: иначе tar и мы ждали бы друг друга.
    let output = std::thread::scope(|scope| {
        if let Some(mut stdin) = stdin {
            scope.spawn(move || {
                let _ = stdin.write_all(archive);
            });
        }
        child.wait_with_output()
    })
    .map_err(unpack)?;
    if !output.status.success() || output.stdout.is_empty() {
        return Err(UpdateError::Unpack(
            "в архиве нет файла программы".to_owned(),
        ));
    }
    Ok(output.stdout)
}

fn write_update(platform: Platform, target: &Path, bytes: &[u8]) -> Result<(), UpdateError> {
    match platform {
        Platform::Windows => replace_with_backup(target, bytes),
        Platform::LinuxX64 => replace_in_place(target, &extract_linux_binary(bytes)?),
        Platform::Other => Err(UpdateError::NoTarget),
    }
}

/// При запуске: убирает `Plastinka.exe.old` (и `.new`) после прошлого
/// обновления. Тихо, ошибки игнорируются.
pub fn cleanup() {
    if !cfg!(windows) {
        return;
    }
    if let Ok(exe) = std::env::current_exe() {
        let _ = fs::remove_file(suffixed(&exe, ".old"));
        let _ = fs::remove_file(suffixed(&exe, ".new"));
    }
}

/// Запускает новую версию с `--after-update` и теми же аргументами. Окно
/// после этого закрывается обычным выходом.
pub fn restart(exe: &Path) -> io::Result<()> {
    let args = std::env::args_os()
        .skip(1)
        .filter(|arg| arg != AFTER_UPDATE_FLAG);
    std::process::Command::new(exe)
        .args(args)
        .arg(AFTER_UPDATE_FLAG)
        .spawn()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "tag_name": "v0.0.6",
        "name": "Версия 0.0.6",
        "html_url": "https://github.com/welcomeforalone/plastinka/releases/tag/v0.0.6",
        "draft": false,
        "prerelease": false,
        "assets": [
            {"name": "Plastinka.exe", "size": 36000000,
             "browser_download_url": "https://github.com/o/r/releases/download/v0.0.6/Plastinka.exe"},
            {"name": "Plastinka.exe.sha256", "size": 90,
             "browser_download_url": "https://github.com/o/r/releases/download/v0.0.6/Plastinka.exe.sha256"},
            {"name": "plastinka-linux-x86_64.tar.gz", "size": 1,
             "browser_download_url": "https://github.com/o/r/releases/download/v0.0.6/plastinka-linux-x86_64.tar.gz"},
            {"name": "plastinka-linux-x86_64.tar.gz.sha256", "size": 1,
             "browser_download_url": "https://github.com/o/r/releases/download/v0.0.6/plastinka-linux-x86_64.tar.gz.sha256"},
            {"name": "plastinka-android.apk", "size": 1,
             "browser_download_url": "https://github.com/o/r/releases/download/v0.0.6/plastinka-android.apk"}
        ]
    }"#;

    fn release(json: &str) -> Release {
        parse_release(json.as_bytes()).expect("ответ разбирается")
    }

    /// Каталог теста во временной папке; удаляется при выходе из теста.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let dir =
                std::env::temp_dir().join(format!("plastinka-update-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&dir).expect("каталог создаётся");
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn newer_versions_compare_numerically() {
        assert!(is_newer("0.0.5", "v0.0.6"));
        assert!(is_newer("0.0.9", "0.0.10"));
        assert!(is_newer("0.0.5", "0.1"));
        assert!(is_newer("0.0.5", "V1.0.0"));
        assert!(!is_newer("0.0.5", "0.0.5"));
        assert!(!is_newer("0.0.6", "0.0.5"));
        assert!(!is_newer("0.1", "0.1.0"));
    }

    #[test]
    fn prerelease_and_garbage_are_not_updates() {
        assert!(!is_newer("0.0.5", "v0.0.6-beta.1"));
        assert!(!is_newer("0.0.5", "nightly"));
        assert!(!is_newer("0.0.5", ""));
        assert!(!is_newer("0.0.5", "0..6"));
        assert!(!is_newer("0.0.5", "1.2.3.4.5"));
        assert!(!is_newer("0.0.5", "v-"));
        assert!(!is_newer("мусор", "0.0.6"));
    }

    #[test]
    fn build_metadata_is_ignored() {
        assert!(is_newer("0.0.5", "0.0.6+abc"));
        assert!(!is_newer("0.0.6", "0.0.6+abc"));
    }

    #[test]
    fn release_parses_and_picks_files_per_platform() {
        let release = release(SAMPLE);
        let Check::Available(win) = evaluate(&release, "0.0.5", Platform::Windows, true, false)
        else {
            panic!("обновление есть");
        };
        assert_eq!(win.version, "0.0.6");
        let package = win.package.expect("для Windows есть файл");
        assert_eq!(package.name, "Plastinka.exe");
        assert!(package.sha_url.ends_with("Plastinka.exe.sha256"));
        let Check::Available(linux) = evaluate(&release, "0.0.5", Platform::LinuxX64, true, false)
        else {
            panic!("обновление есть");
        };
        assert_eq!(
            linux.package.expect("для Linux есть файл").name,
            "plastinka-linux-x86_64.tar.gz"
        );
        let Check::Available(other) = evaluate(&release, "0.0.5", Platform::Other, true, false)
        else {
            panic!("обновление есть");
        };
        assert!(other.package.is_none());
        assert!(other.page_url.contains("/releases/tag/v0.0.6"));
    }

    #[test]
    fn not_installable_gives_link_only() {
        let release = release(SAMPLE);
        let Check::Available(found) = evaluate(&release, "0.0.5", Platform::Windows, false, false)
        else {
            panic!("обновление есть");
        };
        assert!(found.package.is_none());
    }

    #[test]
    fn same_or_older_version_is_up_to_date() {
        let release = release(SAMPLE);
        assert_eq!(
            evaluate(&release, "0.0.6", Platform::Windows, true, false),
            Check::UpToDate("0.0.6".to_owned())
        );
    }

    #[test]
    fn draft_and_prerelease_are_ignored() {
        let draft = SAMPLE.replace("\"draft\": false", "\"draft\": true");
        let pre = SAMPLE.replace("\"prerelease\": false", "\"prerelease\": true");
        for json in [draft, pre] {
            let release = release(&json);
            assert!(matches!(
                evaluate(&release, "0.0.5", Platform::Windows, true, false),
                Check::UpToDate(_)
            ));
        }
    }

    #[test]
    fn missing_file_or_checksum_gives_link_only() {
        let no_sha = SAMPLE.replace("Plastinka.exe.sha256", "Plastinka.exe.sum");
        let release = release(&no_sha);
        let Check::Available(found) = evaluate(&release, "0.0.5", Platform::Windows, true, false)
        else {
            panic!("обновление есть");
        };
        assert!(found.package.is_none());

        let empty = release_json_without_assets();
        let Check::Available(found) = evaluate(
            &self::release(&empty),
            "0.0.5",
            Platform::Windows,
            true,
            false,
        ) else {
            panic!("обновление есть");
        };
        assert!(found.package.is_none());
    }

    fn release_json_without_assets() -> String {
        r#"{"tag_name": "v0.0.6", "html_url": "https://github.com/o/r/releases/tag/v0.0.6", "assets": []}"#
            .to_owned()
    }

    #[test]
    fn foreign_asset_host_is_rejected() {
        let json = SAMPLE.replace("https://github.com/o/r/", "https://evil.example/o/r/");
        let release = release(&json);
        let Check::Available(found) = evaluate(&release, "0.0.5", Platform::Windows, true, false)
        else {
            panic!("обновление есть");
        };
        assert!(found.package.is_none());
        // С хуком любой хост разрешён.
        let Check::Available(hooked) = evaluate(&release, "0.0.5", Platform::Windows, true, true)
        else {
            panic!("обновление есть");
        };
        assert!(hooked.package.is_some());
    }

    #[test]
    fn foreign_page_url_is_replaced() {
        let json = SAMPLE.replace("https://github.com/welcomeforalone", "http://evil.example");
        let release = release(&json);
        let Check::Available(found) = evaluate(&release, "0.0.5", Platform::Windows, true, false)
        else {
            panic!("обновление есть");
        };
        assert_eq!(found.page_url, latest_page());
    }

    #[test]
    fn broken_json_is_an_error() {
        assert!(matches!(
            parse_release(b"{not json"),
            Err(UpdateError::BadResponse)
        ));
        assert!(matches!(
            parse_release(b"{}"),
            Err(UpdateError::BadResponse)
        ));
        let empty = release(r#"{"tag_name": "v9.0.0"}"#);
        assert!(matches!(
            evaluate(&empty, "0.0.5", Platform::Windows, true, false),
            Check::Available(Available { package: None, .. })
        ));
    }

    #[test]
    fn sha256_file_is_parsed() {
        let hex = "a".repeat(64);
        assert_eq!(
            parse_sha256(&format!("{hex}  Plastinka.exe\n")),
            Some(hex.clone())
        );
        assert_eq!(
            parse_sha256(&format!("{}  x", hex.to_uppercase())),
            Some(hex.clone())
        );
        assert_eq!(parse_sha256(&hex), Some(hex));
        assert_eq!(parse_sha256(""), None);
        assert_eq!(parse_sha256("не хеш  файл"), None);
        assert_eq!(parse_sha256(&"a".repeat(63)), None);
        assert_eq!(parse_sha256(&"a".repeat(65)), None);
        assert_eq!(parse_sha256(&format!("{}g", "a".repeat(63))), None);
    }

    #[test]
    fn hash_is_verified() {
        // SHA-256 строки «abc».
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert!(verify_sha256(b"abc", abc));
        assert!(verify_sha256(b"abc", &abc.to_uppercase()));
        assert!(!verify_sha256(b"abd", abc));
        assert!(!verify_sha256(b"abc", ""));
    }

    #[test]
    fn redirect_hosts_are_limited() {
        assert!(host_allowed("github.com"));
        assert!(host_allowed("api.github.com"));
        assert!(host_allowed("objects.githubusercontent.com"));
        assert!(host_allowed("GitHub.com"));
        assert!(!host_allowed("evilgithub.com"));
        assert!(!host_allowed("githubusercontent.com.evil.example"));
        assert!(!host_allowed("evilgithubusercontent.com"));
        assert!(!host_allowed("example.com"));

        let https = reqwest::Url::parse("https://objects.githubusercontent.com/x").expect("url");
        let http = reqwest::Url::parse("http://github.com/x").expect("url");
        let local = reqwest::Url::parse("http://127.0.0.1:8000/x").expect("url");
        assert!(url_allowed(&https, false));
        assert!(!url_allowed(&http, false));
        assert!(!url_allowed(&local, false));
        assert!(url_allowed(&local, true));
    }

    #[test]
    fn linux_bin_dir_follows_xdg() {
        assert_eq!(
            bin_dir(Some("/x/bin".into()), Some("/home/u".into())),
            Some(PathBuf::from("/x/bin"))
        );
        assert_eq!(
            bin_dir(Some("".into()), Some("/home/u".into())),
            Some(PathBuf::from("/home/u/.local/bin"))
        );
        assert_eq!(
            bin_dir(None, Some("/home/u".into())),
            Some(PathBuf::from("/home/u/.local/bin"))
        );
        assert_eq!(bin_dir(None, None), None);
    }

    #[test]
    fn backup_replace_swaps_file_and_keeps_old() {
        let dir = TempDir::new();
        let target = dir.0.join("Plastinka.exe");
        fs::write(&target, b"old").expect("запись");
        replace_with_backup(&target, b"new").expect("замена");
        assert_eq!(fs::read(&target).expect("чтение"), b"new");
        assert_eq!(fs::read(suffixed(&target, ".old")).expect("чтение"), b"old");
        assert!(!suffixed(&target, ".new").exists());
    }

    #[test]
    fn backup_replace_rolls_back_on_error() {
        let dir = TempDir::new();
        let target = dir.0.join("Plastinka.exe");
        fs::write(&target, b"old").expect("запись");
        // На месте `.old` лежит непустой каталог: переименовать файл туда нельзя.
        let old = suffixed(&target, ".old");
        fs::create_dir_all(old.join("inner")).expect("каталог");
        fs::write(old.join("inner").join("f"), b"x").expect("запись");
        let err = replace_with_backup(&target, b"new").expect_err("замена не удалась");
        assert!(
            err.to_string()
                .starts_with("Не удалось заменить файл программы")
        );
        assert_eq!(fs::read(&target).expect("чтение"), b"old");
        assert!(!suffixed(&target, ".new").exists());
    }

    #[test]
    fn in_place_replace_swaps_file() {
        let dir = TempDir::new();
        let target = dir.0.join("Plastinka");
        fs::write(&target, b"old").expect("запись");
        replace_in_place(&target, b"new").expect("замена");
        assert_eq!(fs::read(&target).expect("чтение"), b"new");
        assert!(!suffixed(&target, ".new").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&target)
                .expect("метаданные")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o755);
        }
    }

    #[test]
    fn in_place_replace_fails_cleanly_without_target_dir() {
        let dir = TempDir::new();
        let target = dir.0.join("нет").join("Plastinka");
        let err = replace_in_place(&target, b"new").expect_err("каталога нет");
        assert!(matches!(err, UpdateError::Replace(_)));
    }

    #[test]
    fn error_texts_are_russian() {
        assert!(
            UpdateError::Corrupt
                .to_string()
                .contains("контрольная сумма")
        );
        assert!(
            UpdateError::NotFound
                .to_string()
                .starts_with("Релиз не найден")
        );
    }
}
