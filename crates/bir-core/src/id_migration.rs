//! macOS: v0.2.0 renamed the app id from `com.buwiz.oss` to
//! `com.buwiz.ebirforms`, which moves the data folder and the keychain
//! services ([`crate::app_id`]). On the first launch this brings v0.1.0's
//! data over, before the live database opens:
//!
//! 1. Only when the new folder has no `bir_data.db` and the old one has.
//!    Once the new database exists this never runs again.
//! 2. v0.1.0 must not be running: its owner lock (`bir_data.db.owner.lock`)
//!    is taken, and held while the folder is copied, so it cannot open the
//!    database meanwhile.
//! 3. The keychain items are copied to the new services (the master key,
//!    then bir-submit's secrets), each only when the new service has none,
//!    and read back. The master key is required: a failure stops here, so a
//!    copied database is never left without its key.
//! 4. The folder is copied to a sibling staging folder, checked (the same
//!    files at the same sizes, and the database opens with the key the app
//!    will use), then moved into the new folder, `bir_data.db` last.
//!
//! The old folder and the old keychain items are only read, never changed
//! or deleted; a later version can clean them up.

use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};

use tracing::{info, warn};

use crate::app_id;
use crate::keychain::{AddOutcome, KeychainStore, Lookup, read_with_fallback};

const DB_FILE: &str = "bir_data.db";
const LOCK_FILE: &str = "bir_data.db.owner.lock";
const OLD_APP: &str = "Buwiz 0.1";

/// What a run did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing to bring over: the new database exists, or there is no old one.
    NotNeeded,
    /// The old data is now in the new folder too.
    Migrated {
        files: usize,
        bytes: u64,
        /// `service/account` of each keychain item copied.
        keys: Vec<String>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum MigrationError {
    #[error(
        "Buwiz 0.1 is still running and has your data open ({}). Quit it, then open eBIRForms again to bring your data over.",
        .0.display()
    )]
    OldAppRunning(PathBuf),
    #[error(
        "Could not bring your data over from Buwiz 0.1: {detail}. Your data is still in {}, unchanged. Open eBIRForms again to retry.",
        .old.display()
    )]
    Failed { old: PathBuf, detail: String },
}

/// One keychain item to copy.
struct Item {
    old_service: &'static str,
    new_service: &'static str,
    account: &'static str,
    /// The master key: without it the copied database cannot open.
    required: bool,
}

fn items() -> Vec<Item> {
    let mut items = vec![Item {
        old_service: app_id::OLD_MASTER_KEY_SERVICE,
        new_service: app_id::MASTER_KEY_SERVICE,
        account: app_id::MASTER_KEY_ACCOUNT,
        required: true,
    }];
    items.extend(app_id::SUBMIT_SECRET_NAMES.into_iter().map(|account| Item {
        old_service: app_id::OLD_SUBMIT_SERVICE,
        new_service: app_id::SUBMIT_SERVICE,
        account,
        required: false,
    }));
    items
}

/// Bring v0.1.0's data over, as the module docs say. Uses the user's
/// `~/Library/Application Support`, the login keychain (none with
/// `BUWIZ_NO_KEYCHAIN`), and the app's own key lookup to check the copy.
pub fn migrate_from_old_app_id() -> Result<Outcome, MigrationError> {
    let Some(home) = std::env::var_os("HOME") else {
        return Ok(Outcome::NotNeeded);
    };
    let support = PathBuf::from(home).join("Library/Application Support");
    let keychain = (!crate::keychain_disabled()).then_some((
        &crate::keychain::NativeKeychain as &dyn KeychainStore,
        &crate::keychain::SecurityCli as &dyn KeychainStore,
    ));
    migrate(
        &support.join(app_id::OLD_APP_ID),
        &support.join(app_id::APP_ID),
        keychain,
        &|db| {
            crate::db::Database::open_existing_read_only(db)
                .map(drop)
                .map_err(|error| error.to_string())
        },
    )
}

/// [`migrate_from_old_app_id`] with its folders, keychain (native store,
/// fallback store) and database check given.
pub(crate) fn migrate(
    old: &Path,
    new: &Path,
    keychain: Option<(&dyn KeychainStore, &dyn KeychainStore)>,
    open_db: &dyn Fn(&Path) -> Result<(), String>,
) -> Result<Outcome, MigrationError> {
    if new.join(DB_FILE).exists() || !old.join(DB_FILE).is_file() {
        return Ok(Outcome::NotNeeded);
    }
    let failed = |detail: String| MigrationError::Failed {
        old: old.to_path_buf(),
        detail,
    };
    info!(
        "bringing {OLD_APP}'s data over: {} -> {}",
        old.display(),
        new.display()
    );
    let _old_lock = lock_old(old)?;

    let keys = match keychain {
        Some((native, fallback)) => copy_keys(native, fallback).map_err(failed)?,
        None => {
            info!("BUWIZ_NO_KEYCHAIN is set: no keychain items copied");
            Vec::new()
        }
    };

    let staging = staging_dir(new);
    let copied = stage(old, &staging, open_db).and_then(|copied| {
        std::fs::create_dir_all(new).map_err(|error| format!("{}: {error}", new.display()))?;
        move_into(&staging, new, true).map_err(|error| format!("moving into {}: {error}", new.display()))?;
        Ok(copied)
    });
    let _ = std::fs::remove_dir_all(&staging);
    let (files, bytes) = copied.map_err(failed)?;
    info!(
        "brought {OLD_APP}'s data over: {files} files, {bytes} bytes, keychain items {keys:?}; {} is left as it was",
        old.display()
    );
    Ok(Outcome::Migrated { files, bytes, keys })
}

/// The old app's owner lock, held until dropped. `None` when the old app
/// never made one (it was not running then either). Opened read-only and
/// never created: the old folder is not written to.
fn lock_old(old: &Path) -> Result<Option<File>, MigrationError> {
    use std::os::fd::AsRawFd;
    let path = old.join(LOCK_FILE);
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(MigrationError::Failed {
                old: old.to_path_buf(),
                detail: format!("{}: {error}", path.display()),
            });
        }
    };
    // SAFETY: flock on a descriptor this function owns.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        warn!("{OLD_APP} holds {}: not migrating", path.display());
        return Err(MigrationError::OldAppRunning(old.to_path_buf()));
    }
    Ok(Some(file))
}

/// Copies each keychain item the new service lacks. Returns the copied
/// items. An error only for the master key; bir-submit's secrets that
/// cannot be copied are logged (the user signs in again).
fn copy_keys(native: &dyn KeychainStore, fallback: &dyn KeychainStore) -> Result<Vec<String>, String> {
    let mut copied = Vec::new();
    for item in items() {
        match copy_key(native, fallback, &item) {
            Ok(true) => copied.push(format!("{}/{}", item.new_service, item.account)),
            Ok(false) => {}
            Err(error) if item.required => return Err(error),
            Err(error) => warn!("{error}; signing in again restores it"),
        }
    }
    Ok(copied)
}

/// `Ok(true)` when copied, `Ok(false)` when there was nothing to copy.
fn copy_key(native: &dyn KeychainStore, fallback: &dyn KeychainStore, item: &Item) -> Result<bool, String> {
    let (old, new, account) = (item.old_service, item.new_service, item.account);
    match read_with_fallback(native, fallback, new, account) {
        Lookup::Found(_) => {
            info!("keychain {new}/{account} already exists: kept as it is");
            return Ok(false);
        }
        Lookup::Missing => {}
        Lookup::Unreadable(error) | Lookup::Unavailable(error) => {
            return Err(format!("could not read the keychain item {new}/{account} ({error})"));
        }
    }
    let value = match read_with_fallback(native, fallback, old, account) {
        Lookup::Found(value) => value,
        Lookup::Missing => {
            info!("keychain {old}/{account} does not exist: nothing to copy");
            return Ok(false);
        }
        Lookup::Unreadable(error) | Lookup::Unavailable(error) => {
            return Err(format!(
                "could not read {OLD_APP}'s keychain item {old}/{account} ({error}). If a keychain prompt appeared, allow it"
            ));
        }
    };
    let added = match native.add_new(new, account, &value) {
        AddOutcome::Added | AddOutcome::AlreadyExists => Ok(()),
        AddOutcome::Failed(native_error) => match fallback.add_new(new, account, &value) {
            AddOutcome::Added | AddOutcome::AlreadyExists => Ok(()),
            AddOutcome::Failed(error) => Err(format!(
                "could not store the keychain item {new}/{account} ({}: {native_error}; {}: {error})",
                native.name(),
                fallback.name()
            )),
        },
    };
    added?;
    match read_with_fallback(native, fallback, new, account) {
        Lookup::Found(read) if read == value => {
            info!("copied keychain {old}/{account} to {new}/{account}");
            Ok(true)
        }
        Lookup::Found(_) => Err(format!(
            "the keychain item {new}/{account} holds a different value than {old}/{account}"
        )),
        other => Err(format!(
            "the keychain item {new}/{account} could not be read back ({other:?})"
        )),
    }
}

/// `com.buwiz.ebirforms.migrating`, beside the new folder (same volume, so
/// the moves out of it are renames).
fn staging_dir(new: &Path) -> PathBuf {
    let mut name = new.file_name().unwrap_or_default().to_os_string();
    name.push(".migrating");
    new.with_file_name(name)
}

/// Copies `old` into a fresh `staging` and checks the copy. Returns the
/// file count and bytes.
fn stage(old: &Path, staging: &Path, open_db: &dyn Fn(&Path) -> Result<(), String>) -> Result<(usize, u64), String> {
    if staging.exists() {
        // Left by an attempt that stopped part way; it holds only copies.
        std::fs::remove_dir_all(staging).map_err(|error| format!("{}: {error}", staging.display()))?;
    }
    copy_tree(old, staging).map_err(|error| format!("copying to {}: {error}", staging.display()))?;
    let want = listing(old).map_err(|error| format!("listing {}: {error}", old.display()))?;
    let have = listing(staging).map_err(|error| format!("listing {}: {error}", staging.display()))?;
    if want != have {
        return Err(format!(
            "the copy in {} differs from the original (file list or sizes)",
            staging.display()
        ));
    }
    open_db(&staging.join(DB_FILE)).map_err(|error| format!("the copied database does not open: {error}"))?;
    Ok((want.len(), want.values().sum::<u64>()))
}

/// Skipped by the copy: the old app's lock file, and anything neither a
/// file nor a folder (a symlink).
fn skipped(path: &Path, top: bool, file_type: std::fs::FileType) -> bool {
    (top && path.file_name().is_some_and(|name| name == LOCK_FILE)) || !(file_type.is_file() || file_type.is_dir())
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    fn walk(from: &Path, to: &Path, top: bool) -> std::io::Result<()> {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            let (path, file_type) = (entry.path(), entry.file_type()?);
            if skipped(&path, top, file_type) {
                continue;
            }
            let target = to.join(entry.file_name());
            if file_type.is_dir() {
                walk(&path, &target, false)?;
            } else {
                std::fs::copy(&path, &target)?;
            }
        }
        Ok(())
    }
    walk(from, to, true)
}

/// Every copied file under `root`, by relative path, with its size.
fn listing(root: &Path) -> std::io::Result<BTreeMap<PathBuf, u64>> {
    fn walk(root: &Path, dir: &Path, top: bool, out: &mut BTreeMap<PathBuf, u64>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let (path, file_type) = (entry.path(), entry.file_type()?);
            if skipped(&path, top, file_type) {
                continue;
            }
            if file_type.is_dir() {
                walk(root, &path, false, out)?;
            } else {
                let relative = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
                out.insert(relative, entry.metadata()?.len());
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    walk(root, root, true, &mut out)?;
    Ok(out)
}

/// Moves the staged copy into `to`, which may already hold the new app's
/// own files (its logs). A file already there is kept, except the
/// database's own (`bir_data.db-wal`, `-shm`), which the copy replaces so
/// they match it. Folders merge. `bir_data.db` moves last: it is what marks
/// the migration done.
fn move_into(from: &Path, to: &Path, top: bool) -> std::io::Result<()> {
    let mut database = None;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        let (source, target) = (entry.path(), to.join(&name));
        if top && name == DB_FILE {
            database = Some(source);
            continue;
        }
        if entry.file_type()?.is_dir() {
            if target.is_dir() {
                move_into(&source, &target, false)?;
            } else if target.exists() {
                warn!("kept {} (the copy is a folder)", target.display());
            } else {
                std::fs::rename(&source, &target)?;
            }
        } else if !target.exists() || (top && name.to_string_lossy().starts_with(DB_FILE)) {
            std::fs::rename(&source, &target)?;
        } else {
            info!("kept the new {}", target.display());
        }
    }
    if let Some(source) = database {
        std::fs::rename(source, to.join(DB_FILE))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::{HashMap, HashSet};

    const KEY: &str = "0000000000000000000000000000000000000000000000000000000000000000";

    /// An in-memory keychain: items by (service, account), with services
    /// that cannot be read and an add that fails.
    #[derive(Default)]
    struct Store {
        items: RefCell<HashMap<(String, String), String>>,
        unreadable: HashSet<&'static str>,
        add_fails: bool,
    }

    impl Store {
        fn with(items: &[(&str, &str, &str)]) -> Self {
            let store = Self::default();
            for (service, account, value) in items {
                store
                    .items
                    .borrow_mut()
                    .insert((service.to_string(), account.to_string()), value.to_string());
            }
            store
        }
        fn get(&self, service: &str, account: &str) -> Option<String> {
            self.items
                .borrow()
                .get(&(service.to_string(), account.to_string()))
                .cloned()
        }
    }

    impl KeychainStore for Store {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn read(&self, service: &str, account: &str) -> Lookup {
            if self.unreadable.contains(service) {
                return Lookup::Unreadable("denied".into());
            }
            self.get(service, account).map_or(Lookup::Missing, Lookup::Found)
        }
        fn add_new(&self, service: &str, account: &str, value: &str) -> AddOutcome {
            if self.add_fails {
                return AddOutcome::Failed("refused".into());
            }
            let mut items = self.items.borrow_mut();
            let key = (service.to_string(), account.to_string());
            if items.contains_key(&key) {
                return AddOutcome::AlreadyExists;
            }
            items.insert(key, value.to_string());
            AddOutcome::Added
        }
    }

    /// The fallback store (`security` CLI) finding nothing.
    fn none() -> Store {
        Store::default()
    }

    fn v01_keychain() -> Store {
        Store::with(&[
            (app_id::OLD_MASTER_KEY_SERVICE, app_id::MASTER_KEY_ACCOUNT, KEY),
            (app_id::OLD_SUBMIT_SERVICE, "session", "{\"token\":\"s\"}"),
            (app_id::OLD_SUBMIT_SERVICE, "device_key", "device"),
        ])
    }

    /// A v0.1.0 folder: an encrypted database (the test key), its owner
    /// lock, logs and a submit secret file.
    fn v01_folder(support: &Path) -> PathBuf {
        let old = support.join(app_id::OLD_APP_ID);
        std::fs::create_dir_all(old.join("logs")).unwrap();
        drop(crate::db::Database::open(old.join(DB_FILE)).unwrap());
        std::fs::write(old.join(LOCK_FILE), "").unwrap();
        std::fs::write(old.join("logs/ebirforms.2026-04-01.log"), "old log").unwrap();
        std::fs::write(old.join("buwiz_submit_session.txt"), "s").unwrap();
        old
    }

    fn opens(db: &Path) -> Result<(), String> {
        crate::db::Database::open_existing_read_only(db)
            .map(drop)
            .map_err(|error| error.to_string())
    }

    fn pair<'a>(native: &'a Store, fallback: &'a Store) -> Option<(&'a dyn KeychainStore, &'a dyn KeychainStore)> {
        let native: &dyn KeychainStore = native;
        let fallback: &dyn KeychainStore = fallback;
        Some((native, fallback))
    }

    fn run(old: &Path, new: &Path, native: &Store) -> Result<Outcome, MigrationError> {
        let fallback = none();
        migrate(old, new, pair(native, &fallback), &opens)
    }

    /// Every file under `dir` with its contents, to show it unchanged.
    fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        listing(dir)
            .unwrap()
            .into_keys()
            .map(|relative| {
                let bytes = std::fs::read(dir.join(&relative)).unwrap();
                (relative, bytes)
            })
            .collect()
    }

    #[test]
    fn copies_the_folder_and_the_keys_and_leaves_the_old_ones() {
        let support = tempfile::tempdir().unwrap();
        let old = v01_folder(support.path());
        let new = support.path().join(app_id::APP_ID);
        // The new app already wrote its own log before the database opened.
        std::fs::create_dir_all(new.join("logs")).unwrap();
        std::fs::write(new.join("logs/ebirforms.2026-05-01.log"), "new log").unwrap();
        let before = snapshot(&old);
        let keychain = v01_keychain();

        let outcome = run(&old, &new, &keychain).unwrap();

        let Outcome::Migrated { files, keys, .. } = outcome.clone() else {
            panic!("expected a migration, got {outcome:?}")
        };
        assert_eq!(files, before.len());
        assert_eq!(keys.len(), 3, "the master key and both submit secrets: {keys:?}");
        assert_eq!(
            keychain.get(app_id::MASTER_KEY_SERVICE, app_id::MASTER_KEY_ACCOUNT).as_deref(),
            Some(KEY)
        );
        assert_eq!(keychain.get(app_id::SUBMIT_SERVICE, "device_key").as_deref(), Some("device"));
        // The old items and the old folder are untouched.
        assert_eq!(
            keychain.get(app_id::OLD_MASTER_KEY_SERVICE, app_id::MASTER_KEY_ACCOUNT).as_deref(),
            Some(KEY)
        );
        assert_eq!(snapshot(&old), before);
        // The new folder has the old files, keeps its own, and opens.
        opens(&new.join(DB_FILE)).unwrap();
        assert_eq!(std::fs::read_to_string(new.join("buwiz_submit_session.txt")).unwrap(), "s");
        assert_eq!(std::fs::read_to_string(new.join("logs/ebirforms.2026-04-01.log")).unwrap(), "old log");
        assert_eq!(std::fs::read_to_string(new.join("logs/ebirforms.2026-05-01.log")).unwrap(), "new log");
        assert!(!new.join(LOCK_FILE).exists(), "the old lock file is not copied");
        assert!(!staging_dir(&new).exists());
    }

    #[test]
    fn runs_once() {
        let support = tempfile::tempdir().unwrap();
        let old = v01_folder(support.path());
        let new = support.path().join(app_id::APP_ID);
        let keychain = v01_keychain();
        assert!(matches!(run(&old, &new, &keychain), Ok(Outcome::Migrated { .. })));
        std::fs::write(old.join("buwiz_submit_session.txt"), "changed later").unwrap();
        assert_eq!(run(&old, &new, &keychain).unwrap(), Outcome::NotNeeded);
        assert_eq!(std::fs::read_to_string(new.join("buwiz_submit_session.txt")).unwrap(), "s");
    }

    #[test]
    fn nothing_to_do_without_an_old_database_or_with_a_new_one() {
        let support = tempfile::tempdir().unwrap();
        let old = support.path().join(app_id::OLD_APP_ID);
        let new = support.path().join(app_id::APP_ID);
        let keychain = v01_keychain();
        assert_eq!(run(&old, &new, &keychain).unwrap(), Outcome::NotNeeded);

        let old = v01_folder(support.path());
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(new.join(DB_FILE), "the new app's own").unwrap();
        assert_eq!(run(&old, &new, &keychain).unwrap(), Outcome::NotNeeded);
        assert_eq!(std::fs::read_to_string(new.join(DB_FILE)).unwrap(), "the new app's own");
        assert_eq!(keychain.get(app_id::MASTER_KEY_SERVICE, app_id::MASTER_KEY_ACCOUNT), None);
    }

    #[test]
    fn waits_while_the_old_app_holds_its_lock() {
        let support = tempfile::tempdir().unwrap();
        let old = v01_folder(support.path());
        let new = support.path().join(app_id::APP_ID);
        let keychain = v01_keychain();
        let held = crate::db::try_acquire_live_owner_lock(&old.join(DB_FILE)).unwrap();

        let error = run(&old, &new, &keychain).unwrap_err();
        assert!(matches!(error, MigrationError::OldAppRunning(_)), "{error}");
        assert!(error.to_string().contains("Quit it"));
        assert!(!new.join(DB_FILE).exists());
        assert_eq!(keychain.get(app_id::MASTER_KEY_SERVICE, app_id::MASTER_KEY_ACCOUNT), None);

        drop(held);
        assert!(matches!(run(&old, &new, &keychain), Ok(Outcome::Migrated { .. })));
    }

    #[test]
    fn an_unreadable_master_key_stops_before_the_folder_is_copied() {
        let support = tempfile::tempdir().unwrap();
        let old = v01_folder(support.path());
        let new = support.path().join(app_id::APP_ID);
        let mut keychain = v01_keychain();
        keychain.unreadable.insert(app_id::OLD_MASTER_KEY_SERVICE);

        let error = run(&old, &new, &keychain).unwrap_err();
        assert!(matches!(error, MigrationError::Failed { .. }), "{error}");
        assert!(!new.join(DB_FILE).exists(), "no database without its key");

        // Allowed on the next launch: it goes through.
        keychain.unreadable.clear();
        assert!(matches!(run(&old, &new, &keychain), Ok(Outcome::Migrated { .. })));
    }

    #[test]
    fn a_key_that_cannot_be_stored_stops_the_migration() {
        let support = tempfile::tempdir().unwrap();
        let old = v01_folder(support.path());
        let new = support.path().join(app_id::APP_ID);
        // Both stores refuse the write: the native one falls back to the
        // `security` CLI, so a refusal by the native store alone is not one.
        let mut keychain = v01_keychain();
        keychain.add_fails = true;
        let fallback = Store { add_fails: true, ..Store::default() };
        assert!(migrate(&old, &new, pair(&keychain, &fallback), &opens).is_err());
        assert!(!new.join(DB_FILE).exists());
    }

    #[test]
    fn an_existing_new_key_is_never_overwritten() {
        let support = tempfile::tempdir().unwrap();
        let old = v01_folder(support.path());
        let new = support.path().join(app_id::APP_ID);
        let other = "1111111111111111111111111111111111111111111111111111111111111111";
        let keychain = v01_keychain();
        keychain.items.borrow_mut().insert(
            (app_id::MASTER_KEY_SERVICE.into(), app_id::MASTER_KEY_ACCOUNT.into()),
            other.into(),
        );
        // The database check is what fails when the kept key is the wrong one.
        let wrong_key = |_: &Path| -> Result<(), String> { Err("file is not a database".to_string()) };
        let fallback = none();
        assert!(migrate(&old, &new, pair(&keychain, &fallback), &wrong_key).is_err());
        assert_eq!(
            keychain.get(app_id::MASTER_KEY_SERVICE, app_id::MASTER_KEY_ACCOUNT).as_deref(),
            Some(other)
        );
        assert!(!new.join(DB_FILE).exists());
        assert!(!staging_dir(&new).exists());
    }

    #[test]
    fn unreadable_submit_secrets_do_not_stop_the_migration() {
        let support = tempfile::tempdir().unwrap();
        let old = v01_folder(support.path());
        let new = support.path().join(app_id::APP_ID);
        let mut keychain = v01_keychain();
        keychain.unreadable.insert(app_id::OLD_SUBMIT_SERVICE);
        let Outcome::Migrated { keys, .. } = run(&old, &new, &keychain).unwrap() else {
            panic!("expected a migration")
        };
        assert_eq!(keys, [format!("{}/{}", app_id::MASTER_KEY_SERVICE, app_id::MASTER_KEY_ACCOUNT)]);
        assert!(new.join(DB_FILE).exists());
    }

    #[test]
    fn without_the_keychain_only_the_folder_is_copied() {
        let support = tempfile::tempdir().unwrap();
        let old = v01_folder(support.path());
        let new = support.path().join(app_id::APP_ID);
        let Outcome::Migrated { keys, .. } = migrate(&old, &new, None, &opens).unwrap() else {
            panic!("expected a migration")
        };
        assert!(keys.is_empty());
        opens(&new.join(DB_FILE)).unwrap();
    }

    #[test]
    fn a_leftover_staging_folder_is_replaced() {
        let support = tempfile::tempdir().unwrap();
        let old = v01_folder(support.path());
        let new = support.path().join(app_id::APP_ID);
        let staging = staging_dir(&new);
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("stray.txt"), "half a copy").unwrap();
        assert!(matches!(run(&old, &new, &v01_keychain()), Ok(Outcome::Migrated { .. })));
        assert!(!new.join("stray.txt").exists());
        assert!(!staging.exists());
    }
}
