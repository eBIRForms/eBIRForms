//! Where the sign-in tokens and the device key live: the OS credential
//! store (Keychain, Credential Manager, Secret Service) through `keyring`,
//! or, where there is none (Android, a headless Linux box), a file in the
//! app's private data dir, as bir-core does for its database key.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

/// Service name of the credential-store entries: `com.buwiz.ebirforms.submit`
/// on macOS and iOS, `com.buwiz.oss.submit` on Windows and Linux (see
/// `bir_core::app_id`).
pub const SERVICE: &str = bir_core::app_id::SUBMIT_SERVICE;

#[derive(Debug, thiserror::Error)]
#[error("secret store: {0}")]
pub struct StoreError(pub String);

pub trait SecretStore: Send + Sync {
    fn get(&self, name: &str) -> Result<Option<String>, StoreError>;
    fn set(&self, name: &str, value: &str) -> Result<(), StoreError>;
    fn delete(&self, name: &str) -> Result<(), StoreError>;
}

/// The store for this platform: the OS credential store, falling back to
/// files in bir-core's data dir when the OS store cannot be used.
pub fn platform_store() -> Box<dyn SecretStore> {
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "linux"
    ))]
    {
        Box::new(KeyringStore {
            fallback: FileStore::new(bir_core::platform::data_dir()),
        })
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "linux"
    )))]
    {
        Box::new(FileStore::new(bir_core::platform::data_dir()))
    }
}

/// The OS credential store. When it is unavailable (no Secret Service on
/// D-Bus, for one), secrets go to the fallback file store, and a secret
/// already in that file wins, as bir-core does with `bir_key.txt`.
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "linux"
))]
pub struct KeyringStore {
    fallback: FileStore,
}

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "linux"
))]
impl KeyringStore {
    fn entry(name: &str) -> Result<keyring::Entry, keyring::Error> {
        keyring::Entry::new(SERVICE, name)
    }

    fn unavailable(error: &keyring::Error) -> bool {
        matches!(
            error,
            keyring::Error::PlatformFailure(_) | keyring::Error::NoStorageAccess(_)
        )
    }
}

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "linux"
))]
impl SecretStore for KeyringStore {
    fn get(&self, name: &str) -> Result<Option<String>, StoreError> {
        if let Some(value) = self.fallback.get(name)? {
            return Ok(Some(value));
        }
        match Self::entry(name).and_then(|entry| entry.get_password()) {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) if Self::unavailable(&error) => Ok(None),
            Err(error) => Err(StoreError(error.to_string())),
        }
    }

    fn set(&self, name: &str, value: &str) -> Result<(), StoreError> {
        match Self::entry(name).and_then(|entry| entry.set_password(value)) {
            Ok(()) => self.fallback.delete(name),
            Err(error) if Self::unavailable(&error) => {
                tracing::warn!(
                    "credential store unavailable ({error}); keeping {name} in a private file"
                );
                self.fallback.set(name, value)
            }
            Err(error) => Err(StoreError(error.to_string())),
        }
    }

    fn delete(&self, name: &str) -> Result<(), StoreError> {
        self.fallback.delete(name)?;
        match Self::entry(name).and_then(|entry| entry.delete_credential()) {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) if Self::unavailable(&error) => Ok(()),
            Err(error) => Err(StoreError(error.to_string())),
        }
    }
}

/// One file per secret, readable by this user only.
pub struct FileStore {
    dir: PathBuf,
}

impl FileStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(format!("buwiz_submit_{name}.txt"))
    }
}

impl SecretStore for FileStore {
    fn get(&self, name: &str) -> Result<Option<String>, StoreError> {
        match std::fs::read_to_string(self.path(name)) {
            Ok(value) => Ok(Some(value)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(StoreError(error.to_string())),
        }
    }

    fn set(&self, name: &str, value: &str) -> Result<(), StoreError> {
        std::fs::create_dir_all(&self.dir).map_err(|error| StoreError(error.to_string()))?;
        let path = self.path(name);
        let temporary = path.with_extension("tmp");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options
            .open(&temporary)
            .map_err(|error| StoreError(error.to_string()))?;
        std::io::Write::write_all(&mut file, value.as_bytes())
            .map_err(|error| StoreError(error.to_string()))?;
        drop(file);
        std::fs::rename(&temporary, &path).map_err(|error| StoreError(error.to_string()))
    }

    fn delete(&self, name: &str) -> Result<(), StoreError> {
        match std::fs::remove_file(self.path(name)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(StoreError(error.to_string())),
        }
    }
}

/// In memory, for tests and tools.
#[derive(Default)]
pub struct MemoryStore(Mutex<HashMap<String, String>>);

impl SecretStore for MemoryStore {
    fn get(&self, name: &str) -> Result<Option<String>, StoreError> {
        Ok(self.0.lock().expect("memory store").get(name).cloned())
    }

    fn set(&self, name: &str, value: &str) -> Result<(), StoreError> {
        self.0
            .lock()
            .expect("memory store")
            .insert(name.to_owned(), value.to_owned());
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<(), StoreError> {
        self.0.lock().expect("memory store").remove(name);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_store_round_trips_privately() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileStore::new(dir.path().to_owned());
        assert_eq!(store.get("session").unwrap(), None);
        store.set("session", "{\"a\":1}").unwrap();
        assert_eq!(store.get("session").unwrap().as_deref(), Some("{\"a\":1}"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(store.path("session"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0, "only the owner may read it");
        }
        store.delete("session").unwrap();
        store.delete("session").unwrap();
        assert_eq!(store.get("session").unwrap(), None);
    }
}
