//! Токен пользователя в системном хранилище паролей: Windows Credential
//! Manager или Secret Service в Linux. Запасного хранения в файле нет.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use keyring_core::{CredentialStore, Entry};
use serde::{Deserialize, Serialize};

use super::StorageError;

/// Имя сервиса в хранилище паролей.
const DEFAULT_SERVICE: &str = "plastinka";
/// Имя записи внутри сервиса.
const ACCOUNT: &str = "oauth-token";

/// Подсказка, что делать, если хранилище недоступно.
#[cfg(all(unix, not(target_os = "macos")))]
pub const UNAVAILABLE_HINT: &str =
    "Нужен запущенный Secret Service: gnome-keyring, KWallet или KeePassXC";
#[cfg(target_os = "macos")]
pub const UNAVAILABLE_HINT: &str =
    "Разблокируйте «Связку ключей» (Программы → Утилиты → Связка ключей) и повторите";
#[cfg(not(any(all(unix, not(target_os = "macos")), target_os = "macos")))]
pub const UNAVAILABLE_HINT: &str = "Проверьте, что служба «Диспетчер учётных данных» запущена";

/// Токен OAuth и сведения для его обновления.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredToken {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Unix-время, когда истекает `access_token`, если сервер его сообщил.
    #[serde(default)]
    pub expires_at: Option<u64>,
}

// Сам токен никогда не попадает в отладочный вывод и логи.
impl fmt::Debug for StoredToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoredToken")
            .field("access_token", &"<скрыт>")
            .field("has_refresh_token", &self.refresh_token.is_some())
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// Доступ к записи с токеном. Все методы блокирующие: из async-кода их
/// вызывают через [`super::blocking`].
#[derive(Clone)]
pub struct TokenStore {
    backend: Backend,
}

#[derive(Clone)]
enum Backend {
    Keyring {
        store: Arc<CredentialStore>,
        service: String,
        modifiers: HashMap<&'static str, &'static str>,
    },
    /// Отладочный хук `PLASTINKA_TEST_TOKEN`: хранилище не открываем совсем,
    /// ничего не читаем, не сохраняем и не удаляем.
    #[cfg(debug_assertions)]
    Fake(String),
}

impl fmt::Debug for TokenStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.backend {
            Backend::Keyring { service, .. } => f
                .debug_struct("TokenStore")
                .field("service", service)
                .finish_non_exhaustive(),
            #[cfg(debug_assertions)]
            Backend::Fake(_) => f.write_str("TokenStore(фальшивый токен)"),
        }
    }
}

impl TokenStore {
    /// Подключается к хранилищу паролей ОС.
    pub fn open() -> Result<Self, StorageError> {
        #[cfg(debug_assertions)]
        if let Some(token) = env_hook("PLASTINKA_TEST_TOKEN") {
            return Ok(Self {
                backend: Backend::Fake(token),
            });
        }
        Ok(Self {
            backend: Backend::Keyring {
                store: platform_store()?,
                service: service_name(),
                modifiers: platform_modifiers(),
            },
        })
    }

    /// Хранилище для тестов: без модификаторов и без отладочных хуков.
    #[cfg(test)]
    fn with_store(store: Arc<CredentialStore>, service: &str) -> Self {
        Self {
            backend: Backend::Keyring {
                store,
                service: service.to_owned(),
                modifiers: HashMap::new(),
            },
        }
    }

    /// Читает токен. `None` — входа ещё не было.
    pub fn load(&self) -> Result<Option<StoredToken>, StorageError> {
        let entry = match &self.backend {
            Backend::Keyring { .. } => self.entry()?,
            #[cfg(debug_assertions)]
            Backend::Fake(token) => {
                return Ok(Some(StoredToken {
                    access_token: token.clone(),
                    refresh_token: None,
                    expires_at: None,
                }));
            }
        };
        match entry.get_password() {
            Ok(json) => serde_json::from_str(&json)
                .map(Some)
                .map_err(|_| StorageError::BadStoredToken),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(err) => Err(keyring_error(err)),
        }
    }

    /// Сохраняет токен, заменяя прежний.
    pub fn save(&self, token: &StoredToken) -> Result<(), StorageError> {
        let entry = match &self.backend {
            Backend::Keyring { .. } => self.entry()?,
            #[cfg(debug_assertions)]
            Backend::Fake(_) => return Ok(()),
        };
        let json = serde_json::to_string(token).map_err(|_| StorageError::BadStoredToken)?;
        entry.set_password(&json).map_err(keyring_error)
    }

    /// Удаляет токен, в том числе повреждённый и дубли. Возвращает `false`,
    /// если удалять было нечего.
    pub fn delete(&self) -> Result<bool, StorageError> {
        let entry = match &self.backend {
            Backend::Keyring { .. } => self.entry()?,
            #[cfg(debug_assertions)]
            Backend::Fake(_) => return Ok(false),
        };
        match entry.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring_core::Error::NoEntry) => Ok(false),
            Err(keyring_core::Error::Ambiguous(duplicates)) => {
                for duplicate in duplicates {
                    match duplicate.delete_credential() {
                        Ok(()) | Err(keyring_core::Error::NoEntry) => {}
                        Err(err) => return Err(keyring_error(err)),
                    }
                }
                Ok(true)
            }
            Err(err) => Err(keyring_error(err)),
        }
    }

    fn entry(&self) -> Result<Entry, StorageError> {
        match &self.backend {
            Backend::Keyring {
                store,
                service,
                modifiers,
            } => {
                let modifiers = (!modifiers.is_empty()).then_some(modifiers);
                store
                    .build(service, ACCOUNT, modifiers)
                    .map_err(keyring_error)
            }
            #[cfg(debug_assertions)]
            Backend::Fake(_) => Err(StorageError::SecretStore(
                "с фальшивым токеном хранилище не используется".to_owned(),
            )),
        }
    }
}

/// Значение отладочного хука из переменной окружения, если оно непустое.
#[cfg(debug_assertions)]
pub(crate) fn env_hook(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

/// Имя сервиса. В отладочной сборке его можно подменить, чтобы тесты не
/// трогали настоящую запись пользователя.
pub(crate) fn service_name() -> String {
    #[cfg(debug_assertions)]
    if let Some(name) = env_hook("PLASTINKA_KEYRING_SERVICE") {
        return name;
    }
    DEFAULT_SERVICE.to_owned()
}

#[cfg(windows)]
pub(crate) fn platform_store() -> Result<Arc<CredentialStore>, StorageError> {
    let store: Arc<CredentialStore> =
        windows_native_keyring_store::Store::new().map_err(keyring_error)?;
    Ok(store)
}

#[cfg(all(unix, not(target_os = "macos")))]
pub(crate) fn platform_store() -> Result<Arc<CredentialStore>, StorageError> {
    let store: Arc<CredentialStore> =
        zbus_secret_service_keyring_store::Store::new().map_err(keyring_error)?;
    Ok(store)
}

#[cfg(target_os = "macos")]
pub(crate) fn platform_store() -> Result<Arc<CredentialStore>, StorageError> {
    let store: Arc<CredentialStore> =
        apple_native_keyring_store::keychain::Store::new().map_err(keyring_error)?;
    Ok(store)
}

#[cfg(not(any(windows, all(unix, not(target_os = "macos")), target_os = "macos")))]
pub(crate) fn platform_store() -> Result<Arc<CredentialStore>, StorageError> {
    Err(StorageError::SecretStoreUnavailable(
        "эта ОС пока не поддерживается".to_owned(),
    ))
}

/// На Windows запись по умолчанию может ходить между компьютерами домена,
/// а токену место только на этом компьютере.
#[cfg(windows)]
pub(crate) fn platform_modifiers() -> HashMap<&'static str, &'static str> {
    HashMap::from([("persistence", "Local")])
}

#[cfg(not(windows))]
pub(crate) fn platform_modifiers() -> HashMap<&'static str, &'static str> {
    HashMap::new()
}

/// Переводит ошибку хранилища в нашу. Варианты с содержимым записи
/// (`BadEncoding`, `BadDataFormat`) не выводим: там может быть токен.
pub(crate) fn keyring_error(err: keyring_core::Error) -> StorageError {
    use keyring_core::Error as K;
    match err {
        K::NoStorageAccess(_) => StorageError::SecretStoreLocked,
        K::PlatformFailure(source) => StorageError::SecretStoreUnavailable(source.to_string()),
        K::NoDefaultStore => {
            StorageError::SecretStoreUnavailable("хранилище не выбрано".to_owned())
        }
        K::BadEncoding(_) | K::BadDataFormat(..) => StorageError::BadStoredToken,
        K::Ambiguous(_) => StorageError::SecretStoreDuplicates,
        K::TooLong(..) => {
            StorageError::SecretStore("токен слишком длинный для хранилища".to_owned())
        }
        err @ (K::Invalid(..) | K::NotSupportedByStore(_) | K::BadStoreFormat(_)) => {
            StorageError::SecretStore(err.to_string())
        }
        _ => StorageError::SecretStore("неизвестная ошибка".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_store() -> TokenStore {
        let store: Arc<CredentialStore> = keyring_core::mock::Store::new().unwrap();
        TokenStore::with_store(store, "plastinka-unit-test")
    }

    fn sample_token() -> StoredToken {
        StoredToken {
            access_token: "y0_secret-access".to_owned(),
            refresh_token: Some("1:secret-refresh".to_owned()),
            expires_at: Some(1_800_000_000),
        }
    }

    #[test]
    fn save_load_delete_round_trip() {
        let store = mock_store();
        assert_eq!(store.load().unwrap(), None);
        assert!(!store.delete().unwrap());

        store.save(&sample_token()).unwrap();
        assert_eq!(store.load().unwrap(), Some(sample_token()));

        assert!(store.delete().unwrap());
        assert_eq!(store.load().unwrap(), None);
    }

    #[test]
    fn broken_record_is_reported_without_contents_and_deletable() {
        let store = mock_store();
        store.entry().unwrap().set_password("не json").unwrap();
        let err = store.load().unwrap_err();
        assert!(matches!(err, StorageError::BadStoredToken));
        assert!(!err.to_string().contains("не json"));
        assert!(store.delete().unwrap());
        assert_eq!(store.load().unwrap(), None);
    }

    #[test]
    fn debug_output_hides_tokens() {
        let debug = format!("{:?}", sample_token());
        assert!(!debug.contains("secret"), "{debug}");
    }

    #[cfg(debug_assertions)]
    #[test]
    fn fake_token_hook_never_touches_store() {
        let store = TokenStore {
            backend: Backend::Fake("fake-token".to_owned()),
        };
        assert_eq!(store.load().unwrap().unwrap().access_token, "fake-token");
        store.save(&sample_token()).unwrap();
        assert!(!store.delete().unwrap());
        assert!(!format!("{store:?}").contains("fake-token"));
    }
}
