//! Секреты сопряжения в системном хранилище паролей (§7 спеки): тот же сервис,
//! что и у токена, учётная запись `sync-<id>`, значение — base64. Запасного
//! хранения в файле нет. Все методы блокирующие: из async-кода их зовут через
//! `spawn_blocking`.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use keyring_core::{CredentialStore, Entry};

use super::handshake::PairSecret;
use super::proto::is_device_id;
use crate::storage::StorageError;
use crate::storage::token::{keyring_error, platform_modifiers, platform_store, service_name};

/// Имя учётной записи секрета для устройства `id`.
fn account(id: &str) -> String {
    format!("sync-{id}")
}

#[derive(Clone)]
enum Backend {
    Keyring {
        store: Arc<CredentialStore>,
        service: String,
        modifiers: HashMap<&'static str, &'static str>,
    },
    /// Фальшивый токен (`PLASTINKA_TEST_TOKEN`): хранилище не открываем,
    /// ничего не читаем, не сохраняем и не удаляем.
    #[cfg(debug_assertions)]
    Memory,
}

/// Доступ к секретам сопряжения.
#[derive(Clone)]
pub struct SecretStore {
    backend: Backend,
}

impl fmt::Debug for SecretStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretStore(..)")
    }
}

impl SecretStore {
    /// Подключается к хранилищу ОС. С фальшивым токеном хранилище не
    /// открывается вовсе.
    pub fn open() -> Result<Self, StorageError> {
        #[cfg(debug_assertions)]
        if crate::storage::test_hooks_active() {
            return Ok(Self {
                backend: Backend::Memory,
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

    /// Только память: ничего не читает и не пишет (тесты, фальшивый токен).
    #[cfg(debug_assertions)]
    pub fn memory() -> Self {
        Self {
            backend: Backend::Memory,
        }
    }

    /// Хранилище для тестов: чужой `CredentialStore` (например, mock).
    #[cfg(test)]
    pub(crate) fn with_store(store: Arc<CredentialStore>, service: &str) -> Self {
        Self {
            backend: Backend::Keyring {
                store,
                service: service.to_owned(),
                modifiers: HashMap::new(),
            },
        }
    }

    fn entry(&self, id: &str) -> Result<Option<Entry>, StorageError> {
        match &self.backend {
            Backend::Keyring {
                store,
                service,
                modifiers,
            } => {
                let modifiers = (!modifiers.is_empty()).then_some(modifiers);
                store
                    .build(service, &account(id), modifiers)
                    .map(Some)
                    .map_err(keyring_error)
            }
            #[cfg(debug_assertions)]
            Backend::Memory => Ok(None),
        }
    }

    /// Секрет для `id`. `None` — сопряжения не было.
    pub fn load(&self, id: &str) -> Result<Option<PairSecret>, StorageError> {
        if !is_device_id(id) {
            return Ok(None);
        }
        let Some(entry) = self.entry(id)? else {
            return Ok(None);
        };
        match entry.get_password() {
            Ok(text) => {
                let bytes = STANDARD
                    .decode(text.trim())
                    .ok()
                    .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok());
                // Повреждённая запись — как будто сопряжения нет: заменится новым.
                Ok(bytes.map(PairSecret::new))
            }
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(err) => Err(keyring_error(err)),
        }
    }

    /// Сохраняет секрет, заменяя прежний.
    pub fn save(&self, id: &str, secret: &PairSecret) -> Result<(), StorageError> {
        if !is_device_id(id) {
            return Err(StorageError::SecretStore(
                "неверный идентификатор".to_owned(),
            ));
        }
        let Some(entry) = self.entry(id)? else {
            return Ok(());
        };
        entry
            .set_password(&STANDARD.encode(secret.as_bytes()))
            .map_err(keyring_error)
    }

    /// Удаляет секрет. Нечего удалять — не ошибка.
    pub fn delete(&self, id: &str) -> Result<(), StorageError> {
        if !is_device_id(id) {
            return Ok(());
        }
        let Some(entry) = self.entry(id)? else {
            return Ok(());
        };
        match entry.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(keyring_core::Error::Ambiguous(duplicates)) => {
                for duplicate in duplicates {
                    match duplicate.delete_credential() {
                        Ok(()) | Err(keyring_core::Error::NoEntry) => {}
                        Err(err) => return Err(keyring_error(err)),
                    }
                }
                Ok(())
            }
            Err(err) => Err(keyring_error(err)),
        }
    }
}

/// Метка аккаунта для TXT `acc` (§2): первые 12 символов hex от
/// `SHA-256("plastinka/1 account " + uid)`.
pub fn account_tag(uid: u64) -> String {
    let digest = super::crypto::sha256(&[b"plastinka/1 account ", uid.to_string().as_bytes()]);
    digest
        .iter()
        .take(6)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Имя устройства по умолчанию: имя компьютера (`COMPUTERNAME`,
/// `HOSTNAME`, `/etc/hostname`), запасное — «Plastinka на ПК».
pub fn default_device_name() -> String {
    const FALLBACK: &str = "Plastinka на ПК";
    let from_env = ["COMPUTERNAME", "HOSTNAME"]
        .into_iter()
        .find_map(|name| std::env::var(name).ok());
    let name = from_env.or_else(|| std::fs::read_to_string("/etc/hostname").ok());
    name.and_then(|name| crate::api::clean(&name))
        .map(|name| name.chars().take(64).collect())
        .unwrap_or_else(|| FALLBACK.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "abababababababababababababababab";

    fn mock() -> SecretStore {
        let store: Arc<CredentialStore> = keyring_core::mock::Store::new().unwrap();
        SecretStore::with_store(store, "plastinka-sync-unit-test")
    }

    #[test]
    fn save_load_delete_round_trip() {
        let store = mock();
        assert_eq!(store.load(ID).unwrap(), None);
        store.delete(ID).unwrap();
        let secret = PairSecret::new([7; 32]);
        store.save(ID, &secret).unwrap();
        assert_eq!(store.load(ID).unwrap(), Some(secret));
        store.delete(ID).unwrap();
        assert_eq!(store.load(ID).unwrap(), None);
    }

    #[test]
    fn broken_record_counts_as_absent_and_account_is_per_device() {
        let store = mock();
        store
            .entry(ID)
            .unwrap()
            .unwrap()
            .set_password("не base64")
            .unwrap();
        assert_eq!(store.load(ID).unwrap(), None);
        assert_eq!(account(ID), format!("sync-{ID}"));
        assert_eq!(store.load("bad").unwrap(), None);
        assert!(store.save("bad", &PairSecret::new([0; 32])).is_err());
    }

    #[cfg(debug_assertions)]
    #[test]
    fn memory_backend_never_touches_keyring() {
        let store = SecretStore::memory();
        store.save(ID, &PairSecret::new([1; 32])).unwrap();
        assert_eq!(store.load(ID).unwrap(), None);
        store.delete(ID).unwrap();
    }

    #[test]
    fn account_tag_shape_and_stability() {
        let tag = account_tag(42);
        assert_eq!(tag.len(), 12);
        assert!(tag.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(tag, account_tag(42));
        assert_ne!(tag, account_tag(43));
    }

    #[test]
    fn device_name_is_not_empty() {
        let name = default_device_name();
        assert!(!name.is_empty() && name.chars().count() <= 64);
    }
}
