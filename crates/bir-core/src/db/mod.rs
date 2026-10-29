//! Encrypted SQLite database for taxpayer data.
//!
//! Stores profiles, form data, submission history.
//! Data is transparently AES-256 encrypted using SQLCipher.
//! Master key stored in OS keychain.
//!
//! This module is split into domain-specific sub-modules:
//! - `profiles` — Taxpayer profile CRUD
//! - `jobs` — Background job queue
//! - `submissions` — Tax form submissions
//! - `drafts` — Form draft lifecycle
//! - `receipts` — Submission receipt tracking
//! - `notices` — BIR notices, announcements, deadlines, penalties

mod alerts;
mod amendment_snapshots;
pub use alerts::{AlertAction, AlertRecordOutcome, AlertSeverity, AppAlert, kinds as alert_kinds};
pub use amendment_snapshots::{AmendmentSnapshot, SnapshotPayloadSource};
mod drafts;
pub use drafts::{
    ABANDONED_CLAIM_RELEASE_REASON, AbandonedClaimRelease, Claim1601CSubmissionResult,
    Claim2551QSubmissionResult,
};
mod form_rule_state;
pub use form_rule_state::{FormFinalCopy, FormRuleIdentity, FormRuleState, FormRuleStateError};
mod forms_set;
mod jobs;
mod legacy_amendment;
mod legacy_filed_returns;
mod migrations;
mod notices;
mod post_commit;
mod profiles;
mod queueable;
mod receipts;
pub use queueable::{AmendmentBase, ClaimQueueableResult, RevertedForAmendment};
pub use receipts::ReceiptConfirmationOutcome;
mod submissions;
mod taxpayers;
pub use legacy_filed_returns::LegacyFiledReturn;
pub use submissions::SubmissionSummary;
pub use taxpayers::{BulkCopyReport, MissingFormPolicy, TemplateConflictPolicy};

use rusqlite::{Connection, ErrorCode, OpenFlags, params};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::info;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub use post_commit::{PostCommitRefreshStatus, PostCommitWrite};

// =========================================================================
// Data types
// =========================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: Option<i64>,
    pub name: String,
    pub job_type: String, // "System" or "Custom"
    pub cron_expr: Option<String>,
    pub command: Option<String>,
    pub status: String,
    pub retries: i64,
    pub last_run_at: Option<String>,
    pub next_run_at: Option<String>,
    pub created_at: String,
    pub output_log: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Submission {
    pub id: Option<i64>,
    pub tin: String,
    pub form_type: String,
    pub period: String,
    pub status: String,
    pub form_data: BTreeMap<String, String>,
    pub submitted_at: Option<String>,
    pub filename: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmissionReceipt {
    pub id: Option<i64>,
    pub filename: String,
    pub tin: String,
    pub form_type: String,
    pub period: String,
    pub received_date: String,
    pub received_time: String,
    pub source_from: Option<String>,
    pub raw_text: String,
    pub raw_html: Option<String>,
    /// The confirmation email's own `Date` header (RFC 3339), when known.
    pub email_received_at: Option<String>,
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaxDeadline {
    pub id: Option<i64>,
    pub form_type: String,
    pub due_date: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Announcement {
    pub id: Option<i64>,
    pub source: String,
    pub title: String,
    pub content: String,
    pub published_at: String,
    pub read_status: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BirNotice {
    pub id: Option<i64>,
    pub external_id: String,
    pub source: String,
    pub source_kind: NoticeSourceKind,
    pub source_url: Option<String>,
    pub title: String,
    pub body: String,
    pub notice_type: NoticeType,
    pub rdo_code: Option<String>,
    pub form_code: Option<String>,
    pub deadline: Option<String>, // NaiveDate format YYYY-MM-DD
    pub image_url: Option<String>,
    pub posted_at: Option<String>,
    pub fetched_at: String,
    pub raw_json: Option<String>,
    pub read_status: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum NoticeSourceKind {
    BirCms,
    Rss,
    Manual,
    FacebookGraph,
}

impl NoticeSourceKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            NoticeSourceKind::BirCms => "BirCms",
            NoticeSourceKind::Rss => "Rss",
            NoticeSourceKind::Manual => "Manual",
            NoticeSourceKind::FacebookGraph => "FacebookGraph",
        }
    }

    pub fn from_string(s: &str) -> Self {
        match s {
            "BirCms" => NoticeSourceKind::BirCms,
            "Manual" => NoticeSourceKind::Manual,
            "FacebookGraph" => NoticeSourceKind::FacebookGraph,
            "Rss" => NoticeSourceKind::Rss,
            unknown => {
                tracing::warn!("Unknown NoticeSourceKind '{}', defaulting to Rss", unknown);
                NoticeSourceKind::Rss
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum NoticeType {
    EbirFormsVersion,
    Deadline,
    TaxCalendar,
    RdoAdvisory,
    SystemAdvisory,
    General,
}

impl NoticeType {
    pub fn as_str(&self) -> &'static str {
        match self {
            NoticeType::EbirFormsVersion => "EbirFormsVersion",
            NoticeType::Deadline => "Deadline",
            NoticeType::TaxCalendar => "TaxCalendar",
            NoticeType::RdoAdvisory => "RdoAdvisory",
            NoticeType::SystemAdvisory => "SystemAdvisory",
            NoticeType::General => "General",
        }
    }

    pub fn from_string(s: &str) -> Self {
        match s {
            "EbirFormsVersion" => NoticeType::EbirFormsVersion,
            "Deadline" => NoticeType::Deadline,
            "TaxCalendar" => NoticeType::TaxCalendar,
            "RdoAdvisory" => NoticeType::RdoAdvisory,
            "SystemAdvisory" => NoticeType::SystemAdvisory,
            _ => NoticeType::General,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PenaltyCache {
    pub id: Option<i64>,
    pub tin: String,
    pub form_type: String,
    pub period: String,
    pub penalty_amount: f64,
    pub reason: String,
    pub is_high_risk: bool,
    pub calculated_at: String,
}

// =========================================================================
// Error type
// =========================================================================

/// `security(1)` exit code for "the specified item could not be found".
///
/// macOS-only: the `security` CLI, and the read-then-create dance this guards,
/// exist only on that platform.
#[cfg(target_os = "macos")]
const SECURITY_ERR_ITEM_NOT_FOUND: i32 = 44;

/// Keychain service and account of the SQLCipher master key. Unchanged since
/// the first release: existing installs find their key under these names.
#[cfg(all(target_os = "macos", not(test)))]
const MASTER_KEY_SERVICE: &str = "com.ebir.rust";
#[cfg(all(target_os = "macos", not(test)))]
const MASTER_KEY_ACCOUNT: &str = "sqlcipher_master_key";

/// The master key from the environment, for installs that keep it out of
/// the keychain: `BIR_DB_KEY` (64 hex digits) or `BIR_DB_KEY_FILE` (a file
/// holding them). `None` when neither is set. A set but unusable value is an
/// error, never a fall back to the keychain: a run meant to skip the keychain
/// must not stop on a keychain prompt.
fn env_master_key(var: impl Fn(&str) -> Option<String>) -> Option<Result<String, DbError>> {
    let set = |name: &str| var(name).filter(|value| !value.is_empty());
    let (text, from) = match set("BIR_DB_KEY") {
        Some(key) => (key, "BIR_DB_KEY"),
        None => match std::fs::read_to_string(set("BIR_DB_KEY_FILE")?) {
            Ok(text) => (text, "BIR_DB_KEY_FILE"),
            Err(error) => {
                return Some(Err(DbError::Other(format!(
                    "BIR_DB_KEY_FILE is unreadable: {error}"
                ))));
            }
        },
    };
    let key = text.trim();
    if key.len() == 64 && key.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(Ok(key.to_owned()))
    } else {
        Some(Err(DbError::Other(format!(
            "{from} must hold the 64 hex digit database key"
        ))))
    }
}

/// [`env_master_key`] read from the process environment, logging its source.
#[cfg(not(test))]
fn process_env_master_key() -> Option<Result<String, DbError>> {
    let key = env_master_key(|name| std::env::var(name).ok())?;
    if key.is_ok() {
        info!("Loaded SQLCipher master key from the environment");
    }
    Some(key)
}

/// Whether a failed keychain read means the master key is genuinely absent.
///
/// This single predicate is what stands between a first run and destroying an
/// existing database. `security find-generic-password` fails for several
/// reasons, and only one of them means "no key here":
///
/// - exit 44 - the item is not in the keychain. First run; safe to create one.
/// - anything else - authorization denied, keychain locked, `securityd`
///   unavailable, killed by a signal. A key may well exist and simply not be
///   readable at this moment. Generating a replacement would overwrite it and
///   leave the encrypted database unopenable forever.
///
/// Defaults to `false` for `None` (terminated by a signal), because an unknown
/// failure must never be read as "absent".
#[cfg(target_os = "macos")]
pub(crate) fn keychain_miss_is_genuine(exit_code: Option<i32>) -> bool {
    exit_code == Some(SECURITY_ERR_ITEM_NOT_FOUND)
}

#[derive(thiserror::Error, Debug)]
pub enum DbError {
    #[error("Database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Keychain error: {0}")]
    Keychain(#[from] keyring::Error),
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("Encryption key invalid or database corrupted")]
    Encryption,
    #[error("Keychain CLI error: {0}")]
    KeychainCli(String),
    #[error("Zip error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("Other error: {0}")]
    Other(String),
    /// save-009: Amended Return = No while a filed original exists.
    #[error("{}", crate::forms::queueable::AMENDED_RETURN_ALERT)]
    AmendedReturnRequired,
    #[error(
        "live database {} is already open (painted bir or bir-headless). \
         Single live-DB owner: quit the other process. There is no protocol Op::Yield.",
        .0.display()
    )]
    LiveDatabaseInUse(PathBuf),
    /// Exporting a profile whose taxpayer has a PIN, without it. The text is
    /// the refusal a caller surfaces as-is.
    #[error("This profile's PIN is required to export it.")]
    PinRequired,
    /// The PIN given for a profile export is not the taxpayer's. The text is
    /// the refusal a caller surfaces as-is.
    #[error("Wrong PIN.")]
    PinMismatch,
}

// =========================================================================
// Core Database struct and lifecycle
// =========================================================================

pub struct Database {
    pub(crate) conn: Connection,
    /// Sidecar exclusive lock for painted `bir` / `bir-headless serve`.
    /// `None` for ephemeral fixtures and `Database::open` test helpers.
    _owner_lock: Option<File>,
}

/// The filing period a Tax Form Library selection (`FileForm { form_code,
/// year, quarter }`) names, with the same slot semantics the draft store uses.
pub(crate) fn period_from_library_slot(form_code: &str, slot: u8) -> crate::forms::FilingPeriod {
    drafts::period_from_legacy_slot(form_code, Some(slot), 1)
}

pub fn default_database_path() -> std::path::PathBuf {
    crate::platform::data_dir().join("bir_data.db")
}

/// Live DB file painted `bir` and `bir-headless serve` open.
///
/// `BIR_DATABASE_PATH` overrides for tests / CI. Empty or unset uses
/// [`default_database_path`] (`platform::data_dir()/bir_data.db`).
pub fn app_database_path() -> PathBuf {
    match std::env::var_os("BIR_DATABASE_PATH") {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => default_database_path(),
    }
}

/// Create the parent directory and copy a non-empty cwd `bir_data.db` when
/// the target is missing (same bootstrap as painted `bir`).
pub fn prepare_app_database_file(path: &Path) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let legacy_db_path = std::env::current_dir()
        .unwrap_or_default()
        .join("bir_data.db");
    if !path.exists()
        && legacy_db_path.exists()
        && legacy_db_path.metadata().map(|m| m.len()).unwrap_or(0) > 0
        && legacy_db_path != path
    {
        let _ = std::fs::copy(&legacy_db_path, path);
    }
}

/// Sidecar lock next to the SQLCipher file (`bir_data.db.owner.lock`).
pub fn live_owner_lock_path(db_path: &Path) -> PathBuf {
    let mut raw = db_path.as_os_str().to_os_string();
    raw.push(".owner.lock");
    PathBuf::from(raw)
}

/// Exclusive non-blocking lock so painted `bir` and `bir-headless` cannot
/// both own the live file. There is no protocol `Op::Yield`.
pub fn try_acquire_live_owner_lock(db_path: &Path) -> Result<File, DbError> {
    let lock_path = live_owner_lock_path(db_path);
    if let Some(parent) = lock_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let file = open_live_owner_lock_file(&lock_path, db_path)?;
    acquire_exclusive_owner_lock(&file, db_path)?;
    Ok(file)
}

fn open_live_owner_lock_file(lock_path: &Path, db_path: &Path) -> Result<File, DbError> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        match OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .share_mode(0)
            .open(lock_path)
        {
            Ok(file) => Ok(file),
            Err(error)
                if error.kind() == std::io::ErrorKind::PermissionDenied
                    || error.raw_os_error() == Some(32) =>
            {
                Err(DbError::LiveDatabaseInUse(db_path.to_path_buf()))
            }
            Err(error) => Err(DbError::Io(error)),
        }
    }
    #[cfg(not(windows))]
    {
        let _ = db_path;
        Ok(OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(lock_path)?)
    }
}

fn acquire_exclusive_owner_lock(file: &File, db_path: &Path) -> Result<(), DbError> {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc != 0 {
            return Err(DbError::LiveDatabaseInUse(db_path.to_path_buf()));
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (file, db_path);
        Ok(())
    }
}

/// Open the live SQLCipher database (keychain / file-fallback key), not an
/// ephemeral in-memory fixture. Painted `bir` uses [`Database::open_or_recreate`].
pub fn open_live_database() -> Result<(Database, PathBuf, Option<PathBuf>), DbError> {
    let path = app_database_path();
    prepare_app_database_file(&path);
    let lock = try_acquire_live_owner_lock(&path)?;
    let (db, recovered) = Database::open_or_recreate(&path)?;
    // D8a: the shipped holiday lists, on first run and for every new year
    // an update ships. Never fatal: the app runs without them.
    if let Err(error) = db.seed_official_holidays() {
        tracing::warn!(%error, "official holidays not seeded");
    }
    Ok((db.with_owner_lock(lock), path, recovered))
}

/// Open the live SQLCipher file without quarantining. `bir-headless serve`
/// must not recreate a taxpayer DB on a transient key/WAL error.
pub fn open_serve_database() -> Result<(Database, PathBuf), DbError> {
    let path = app_database_path();
    prepare_app_database_file(&path);
    let lock = try_acquire_live_owner_lock(&path)?;
    let db = Database::open(&path)?;
    Ok((db.with_owner_lock(lock), path))
}

/// Setting key for [`Database::list_send_grants`].
pub const SEND_GRANTS_KEY: &str = "agent.send_grants";

/// One standing OK for an assistant to file `form` for `tin`.
/// The Queue button does not read this. An assistant send does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendGrant {
    pub tin: String,
    pub form: String,
    pub granted_at: String,
}

fn grant_tin(tin: &str) -> String {
    tin.chars().filter(|c| c.is_ascii_digit()).collect()
}

fn grant_form(form: &str) -> String {
    form.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_uppercase()
}

impl Database {
    fn with_owner_lock(mut self, lock: File) -> Self {
        self._owner_lock = Some(lock);
        self
    }

    /// Unencrypted in-memory SQLite for isolated tests and the agent host fixture.
    ///
    /// Does not open the live app-group database path and does not touch the OS
    /// keychain, so automated runs cannot mutate a real user profile.
    pub fn open_ephemeral() -> Result<Self, DbError> {
        let conn = Connection::open_in_memory()?;
        migrations::migrate_database(&conn)?;
        Ok(Self {
            conn,
            _owner_lock: None,
        })
    }

    #[cfg(test)]
    pub(crate) fn open_in_memory_for_tests() -> Result<Self, DbError> {
        Self::open_ephemeral()
    }

    /// Returns the current SQLite `data_version`, which increments whenever another connection
    /// commits a write to the WAL. Used by the db-watcher in `bir-desktop` to detect external
    /// database changes without requiring direct access to the raw `conn` field.
    pub fn data_version(&self) -> Option<i32> {
        self.conn
            .query_row("PRAGMA data_version;", [], |row| row.get(0))
            .ok()
    }

    /// Opens the database, quarantining an unreadable existing file and
    /// recreating it when the on-disk contents are not a usable SQLCipher DB.
    pub fn open_or_recreate<P: AsRef<Path>>(path: P) -> Result<(Self, Option<PathBuf>), DbError> {
        let path = path.as_ref();

        match Self::open(path) {
            Ok(db) => Ok((db, None)),
            Err(err) if path.exists() && Self::should_recreate_after_open_error(&err) => {
                let backup_path = Self::quarantine_database_file(path)?;
                Self::cleanup_old_backups(path, 3);
                let db = Self::open(path)?;
                Ok((db, Some(backup_path)))
            }
            Err(err) => Err(err),
        }
    }

    /// Opens the database and initializes SQLCipher using the OS keychain.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, DbError> {
        let conn = Connection::open(path)?;

        let key_hex = Self::get_or_create_master_key()?;

        // Initialize SQLCipher transparent encryption
        conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", key_hex))?;

        // Improve concurrency and wait for locks instead of failing immediately (fixes UI freezes)
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;",
        )?;

        // Verify the key by trying to read from sqlite_master
        {
            let mut check_stmt = conn.prepare("SELECT count(*) FROM sqlite_master")?;
            let _: i64 = check_stmt.query_row([], |r| r.get(0)).map_err(|e| {
                if let rusqlite::Error::SqliteFailure(err, _) = &e {
                    // Only treat it as an encryption error if it's explicitly NotADatabase
                    if err.code == ErrorCode::NotADatabase {
                        return DbError::Encryption;
                    }
                }
                DbError::Sqlite(e)
            })?;
        }

        // Apply database schema migrations
        migrations::migrate_database(&conn)?;

        // Sync legacy announcements to bir_notices table
        let _ = conn.execute_batch("
            INSERT INTO bir_notices (external_id, source, source_kind, title, body, notice_type, posted_at, read_status)
            SELECT
                'legacy-' || id,
                source,
                'Rss',
                title,
                content,
                'General',
                published_at,
                read_status
            FROM announcements
            WHERE NOT EXISTS (SELECT 1 FROM bir_notices WHERE external_id = 'legacy-' || announcements.id);
        ");

        Ok(Self {
            conn,
            _owner_lock: None,
        })
    }

    /// Opens an existing SQLCipher database without creating, migrating, or recovering it.
    ///
    /// This is intentionally narrower than [`Self::open`]. It exists for read-only diagnostics
    /// and backup export, where a failed audit must never mutate or replace the source database.
    /// The returned connection cannot write to the main database, although SQLite may still
    /// attach a separate output database for `sqlcipher_export`.
    pub fn open_existing_read_only<P: AsRef<Path>>(path: P) -> Result<Self, DbError> {
        let path = path.as_ref();
        if !path.is_file() {
            return Err(DbError::Other(format!(
                "Database does not exist: {}",
                path.display()
            )));
        }

        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let key_hex = Self::get_existing_master_key()?;
        conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", key_hex))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;

        let mut check_stmt = conn.prepare("SELECT count(*) FROM sqlite_master")?;
        let _: i64 = check_stmt
            .query_row([], |row| row.get(0))
            .map_err(|error| {
                if let rusqlite::Error::SqliteFailure(sqlite_error, _) = &error {
                    if sqlite_error.code == ErrorCode::NotADatabase {
                        return DbError::Encryption;
                    }
                }
                DbError::Sqlite(error)
            })?;
        drop(check_stmt);

        Ok(Self {
            conn,
            _owner_lock: None,
        })
    }

    fn should_recreate_after_open_error(err: &DbError) -> bool {
        match err {
            DbError::Encryption => true,
            DbError::Sqlite(rusqlite::Error::SqliteFailure(sql_err, _)) => {
                sql_err.code == ErrorCode::NotADatabase
            }
            _ => false,
        }
    }

    pub fn get_or_create_master_key() -> Result<String, DbError> {
        #[cfg(test)]
        return Ok("0000000000000000000000000000000000000000000000000000000000000000".to_string());

        #[cfg(not(test))]
        {
            if std::env::var("EBIR_TEST_ENV").is_ok() {
                return Ok(
                    "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
                );
            }
            if let Some(key) = process_env_master_key() {
                return key;
            }
            if crate::keychain_disabled() {
                return Err(DbError::Other(
                    "BIR_NO_KEYCHAIN is set: give the database key in BIR_DB_KEY or BIR_DB_KEY_FILE"
                        .into(),
                ));
            }

            // Platform dispatch: macOS uses Security.framework (off the main
            // thread) with a `security` CLI fallback; Linux/Windows use the
            // `keyring` crate directly.
            #[cfg(target_os = "macos")]
            return Self::get_or_create_master_key_macos();

            #[cfg(not(target_os = "macos"))]
            return Self::get_or_create_master_key_keyring();
        }
    }

    pub(crate) fn get_existing_master_key() -> Result<String, DbError> {
        #[cfg(test)]
        return Ok("0000000000000000000000000000000000000000000000000000000000000000".to_string());

        #[cfg(not(test))]
        {
            if std::env::var("EBIR_TEST_ENV").is_ok() {
                return Ok(
                    "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
                );
            }
            if let Some(key) = process_env_master_key() {
                return key;
            }
            if crate::keychain_disabled() {
                return Err(DbError::Other(
                    "BIR_NO_KEYCHAIN is set: give the database key in BIR_DB_KEY or BIR_DB_KEY_FILE"
                        .into(),
                ));
            }

            #[cfg(target_os = "macos")]
            return Self::get_existing_master_key_macos();

            #[cfg(not(target_os = "macos"))]
            return Self::get_existing_master_key_keyring();
        }
    }

    #[cfg(all(target_os = "macos", not(test)))]
    fn get_existing_master_key_macos() -> Result<String, DbError> {
        crate::keychain::resolve_master_key(
            &crate::keychain::NativeKeychain,
            &crate::keychain::SecurityCli,
            MASTER_KEY_SERVICE,
            MASTER_KEY_ACCOUNT,
            crate::keychain::KeyMode::ExistingOnly,
            || unreachable!("ExistingOnly never generates a key"),
        )
        .map_err(DbError::KeychainCli)
    }

    #[cfg(all(not(target_os = "macos"), not(test)))]
    fn get_existing_master_key_keyring() -> Result<String, DbError> {
        let keyring_error = match keyring::Entry::new("com.ebir.rust", "sqlcipher_master_key") {
            Ok(entry) => match entry.get_password() {
                Ok(hex_key) if hex_key.len() == 64 => return Ok(hex_key),
                Ok(_) => "stored key has an invalid length".to_string(),
                Err(error) => error.to_string(),
            },
            Err(error) => error.to_string(),
        };

        let key_path = crate::platform::data_dir().join("bir_key.txt");
        if let Ok(hex_key) = std::fs::read_to_string(&key_path) {
            if hex_key.len() == 64 {
                return Ok(hex_key);
            }
        }

        Err(DbError::Other(format!(
            "Existing SQLCipher master key was not available: {keyring_error}"
        )))
    }

    /// macOS: Security.framework in-process, falling back to the `security`
    /// CLI that earlier releases used exclusively.
    ///
    /// Native first because the App Store sandbox may not let a child
    /// `security` process reach the keychain, and an unreadable key means the
    /// encrypted database cannot open. The CLI fallback keeps existing installs
    /// working: their item was created by `security`, so its ACL trusts that
    /// tool and a native read may be refused. See `crate::keychain` for the
    /// lookup order and the rules that stop an existing key from ever being
    /// overwritten or replaced by a freshly generated one.
    ///
    /// Native calls run on a helper thread with a timeout: earlier releases
    /// observed `SecKeychainFindGenericPassword` deadlocking GPUI's main run
    /// loop when called on it directly.
    #[cfg(all(target_os = "macos", not(test)))]
    fn get_or_create_master_key_macos() -> Result<String, DbError> {
        let hex_key = crate::keychain::resolve_master_key(
            &crate::keychain::NativeKeychain,
            &crate::keychain::SecurityCli,
            MASTER_KEY_SERVICE,
            MASTER_KEY_ACCOUNT,
            crate::keychain::KeyMode::CreateIfMissing,
            || {
                info!("No master key in keychain; generating and storing one");
                hex::encode(rand::random::<[u8; 32]>())
            },
        )
        .map_err(DbError::KeychainCli)?;
        info!("Loaded SQLCipher master key from keychain");
        Ok(hex_key)
    }

    /// Linux/Windows: Use the `keyring` crate directly (no RunLoop deadlock risk).
    ///
    /// A valid `bir_key.txt` in the data dir wins: it only exists because an
    /// earlier launch could not use the OS store, so it holds the key the
    /// database was encrypted with. On a headless Linux box with no Secret
    /// Service on D-Bus, `get_password` fails with `PlatformFailure`, which
    /// routes to the file fallback instead of refusing to start.
    #[cfg(not(target_os = "macos"))]
    fn get_or_create_master_key_keyring() -> Result<String, DbError> {
        use keyring::Entry;
        use tracing::info;

        if let Some(hex_key) = Self::read_fallback_key_file() {
            info!("Loaded master key from fallback file");
            return Ok(hex_key);
        }

        let entry = match Entry::new("com.ebir.rust", "sqlcipher_master_key") {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("Failed to access OS keyring: {e}. Falling back to file-based key.");
                return Self::get_or_create_master_key_file_fallback();
            }
        };

        match classify_keyring_read(entry.get_password()) {
            KeyringRead::Found(hex_key) => {
                info!("Loaded existing master key from native keychain");
                Ok(hex_key)
            }
            // Only NoEntry is a genuine miss: no key exists yet, so
            // generating one is safe.
            KeyringRead::Missing => {
                info!("Generating new master key and storing in native keychain");
                let key: [u8; 32] = rand::random();
                let hex_key = hex::encode(key);
                if let Err(e) = entry.set_password(&hex_key) {
                    tracing::warn!(
                        "Failed to save key to OS keyring: {e}. Falling back to file-based key."
                    );
                    return Self::get_or_create_master_key_file_fallback();
                }
                Ok(hex_key)
            }
            // The credential store itself is unreachable (e.g. no Secret
            // Service on the session bus), so it cannot be holding a key we
            // created: use the file-based key instead.
            KeyringRead::Unavailable(e) => {
                tracing::warn!("OS keyring unavailable: {e}. Falling back to file-based key.");
                Self::get_or_create_master_key_file_fallback()
            }
            // Locked/denied or anything unexpected: the store may still hold
            // a live key we failed to read. Generating a replacement would
            // make the existing database permanently undecryptable - the
            // same failure mode the macOS keychain path refuses to risk.
            KeyringRead::Refuse(e) => Err(DbError::KeychainCli(format!(
                "Could not read the existing SQLCipher master key: {e}. \
                 Refusing to generate a replacement, because that would \
                 make the database permanently unreadable. Fix keyring \
                 access and retry."
            ))),
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn read_fallback_key_file() -> Option<String> {
        let key_path = crate::platform::data_dir().join("bir_key.txt");
        match std::fs::read_to_string(key_path) {
            Ok(hex_key) if hex_key.len() == 64 => Some(hex_key),
            _ => None,
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn get_or_create_master_key_file_fallback() -> Result<String, DbError> {
        use tracing::info;
        let key_path = crate::platform::data_dir().join("bir_key.txt");

        if key_path.exists() {
            match std::fs::read_to_string(&key_path) {
                Ok(hex_key) if hex_key.len() == 64 => {
                    info!("Loaded master key from fallback file");
                    return Ok(hex_key);
                }
                _ => {
                    tracing::warn!("Fallback key file invalid or unreadable. Generating new key.");
                }
            }
        }

        info!("Generating new master key and storing in fallback file");
        let key: [u8; 32] = rand::random();
        let hex_key = hex::encode(key);
        std::fs::write(&key_path, &hex_key)?;

        // Try to secure the file on Linux by restricting permissions
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(mut perms) = std::fs::metadata(&key_path).map(|m| m.permissions()) {
                perms.set_mode(0o600);
                let _ = std::fs::set_permissions(&key_path, perms);
            }
        }

        Ok(hex_key)
    }

    fn quarantine_database_file(path: &Path) -> Result<PathBuf, DbError> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let file_name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "bir_data.db".to_string());
        let backup_path = path.with_file_name(format!("{file_name}.corrupt-{timestamp}.bak"));

        std::fs::rename(path, &backup_path)?;

        Ok(backup_path)
    }

    /// Checkpoint WAL without consuming self.
    /// Flushes all data from the WAL file into the main database file.
    pub fn checkpoint(&self) -> Result<(), DbError> {
        self.conn
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }

    /// Consume self and close cleanly after a WAL checkpoint.
    pub fn close(self) -> Result<(), DbError> {
        let Database { conn, _owner_lock } = self;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        let result = conn.close().map_err(|(_, e)| DbError::Sqlite(e));
        drop(_owner_lock);
        result
    }

    /// Remove old `.corrupt-*.bak` files, keeping only the most recent `keep`.
    fn cleanup_old_backups(db_path: &Path, keep: usize) {
        let Some(dir) = db_path.parent() else {
            return;
        };
        let db_name = db_path.file_name().unwrap_or_default().to_string_lossy();
        let prefix = format!("{}.corrupt-", db_name);

        let mut backups: Vec<_> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
            .collect();

        // Sort by name (timestamp is embedded) — oldest first
        backups.sort_by_key(|e| e.file_name());

        if backups.len() > keep {
            for entry in &backups[..backups.len() - keep] {
                let _ = std::fs::remove_file(entry.path());
                info!("Cleaned up old backup: {}", entry.path().display());
            }
        }
    }

    /// Factory Reset: Deletes all data from the database.
    pub fn factory_reset(&self) -> Result<(), DbError> {
        self.conn.execute_batch(
            "DELETE FROM tax_deadlines;
             DELETE FROM announcements;
             DELETE FROM bir_notices;
             DELETE FROM penalties_cache;
             DELETE FROM profiles;
             DELETE FROM submissions;
             DELETE FROM legacy_filed_returns;
             DELETE FROM form_drafts;
             DELETE FROM submission_receipts;
             DELETE FROM job_queue;
             DELETE FROM settings;",
        )?;
        // Also vacuum to reclaim space and shrink file
        self.conn.execute_batch("VACUUM;")?;

        // Clean up temporary PDF directory
        let temp_pdf_dir = std::env::temp_dir().join("taxman-ebir-pdf");
        if temp_pdf_dir.exists() {
            let _ = std::fs::remove_dir_all(&temp_pdf_dir);
        }

        Ok(())
    }

    /// Retrieve a global setting value by key.
    pub fn get_setting(&self, key: &str) -> Result<Option<String>, DbError> {
        let mut stmt = self
            .conn
            .prepare("SELECT value FROM settings WHERE key = ?1")?;
        let mut rows = stmt.query(params![key])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    /// Set a global setting value.
    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), DbError> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = ?2",
            params![key, value],
        )?;
        Ok(())
    }

    /// Standing send grants, oldest first.
    pub fn list_send_grants(&self) -> Result<Vec<SendGrant>, DbError> {
        let Some(stored) = self.get_setting(SEND_GRANTS_KEY)? else {
            return Ok(Vec::new());
        };
        if stored.trim().is_empty() {
            return Ok(Vec::new());
        }
        serde_json::from_str(&stored).map_err(|error| {
            DbError::Other(format!("agent.send_grants is not a JSON array: {error}"))
        })
    }

    fn write_send_grants(&self, grants: &[SendGrant]) -> Result<(), DbError> {
        let stored = serde_json::to_string(grants)?;
        self.set_setting(SEND_GRANTS_KEY, &stored)
    }

    /// Record a standing OK. A repeat grant for the same taxpayer and form
    /// keeps the original time.
    pub fn grant_send(&self, tin: &str, form: &str) -> Result<SendGrant, DbError> {
        let tin = grant_tin(tin);
        let form = grant_form(form);
        if tin.len() != 14 {
            return Err(DbError::Other("a send grant needs the 14-digit TIN".into()));
        }
        if form.is_empty() {
            return Err(DbError::Other("a send grant needs a form code".into()));
        }
        let mut grants = self.list_send_grants()?;
        if let Some(existing) = grants
            .iter()
            .find(|grant| grant.tin == tin && grant.form == form)
        {
            return Ok(existing.clone());
        }
        let grant = SendGrant {
            tin,
            form,
            granted_at: chrono::Utc::now().to_rfc3339(),
        };
        grants.push(grant.clone());
        self.write_send_grants(&grants)?;
        Ok(grant)
    }

    /// Remove one standing OK. Missing grants are fine.
    pub fn revoke_send(&self, tin: &str, form: &str) -> Result<(), DbError> {
        let tin = grant_tin(tin);
        let form = grant_form(form);
        let grants = self.list_send_grants()?;
        let kept: Vec<SendGrant> = grants
            .into_iter()
            .filter(|grant| !(grant.tin == tin && grant.form == form))
            .collect();
        self.write_send_grants(&kept)
    }

    /// Whether the assistant may file `form` for `tin` without asking again.
    pub fn has_send_grant(&self, tin: &str, form: &str) -> Result<bool, DbError> {
        let tin = grant_tin(tin);
        let form = grant_form(form);
        Ok(self
            .list_send_grants()?
            .iter()
            .any(|grant| grant.tin == tin && grant.form == form))
    }

    /// Set a global setting only if it is not set yet, atomically; whether
    /// this call set it (a one-time notice's seen flag: of two callers, one
    /// gets `true`).
    pub fn insert_setting_if_absent(&self, key: &str, value: &str) -> Result<bool, DbError> {
        let inserted = self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO NOTHING",
            params![key, value],
        )?;
        Ok(inserted == 1)
    }

    /// Delete a global setting value.
    pub fn delete_setting(&self, key: &str) -> Result<(), DbError> {
        self.conn
            .execute("DELETE FROM settings WHERE key = ?1", params![key])?;
        Ok(())
    }

    /// The stored override entries, each as its own stored text (`None` when
    /// none are stored; `Err` when the setting is not a JSON array).
    fn stored_override_entries(&self) -> Result<Option<Vec<String>>, DbError> {
        let Some(stored) = self.get_setting("deadline_overrides")? else {
            return Ok(None);
        };
        split_json_array(&stored)
            .map(|entries| Some(entries.into_iter().map(str::to_string).collect()))
            .ok_or_else(|| DbError::Other("stored deadline overrides are not a JSON array".into()))
    }

    /// Retrieve deadline overrides stored in the settings table. An entry
    /// that cannot be read is skipped (and logged), never the whole list
    /// (decision P5-10); [`Self::damaged_deadline_overrides`] lists it.
    pub fn get_deadline_overrides(&self) -> Vec<crate::calendar_rules::DeadlineOverride> {
        let entries = match self.stored_override_entries() {
            Ok(Some(entries)) => entries,
            Ok(None) => return Vec::new(),
            Err(error) => {
                tracing::warn!(%error, "deadline overrides unreadable");
                return Vec::new();
            }
        };
        entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| match serde_json::from_str(entry) {
                Ok(read) => Some(read),
                Err(error) => {
                    tracing::warn!(index, %error, "deadline override entry unreadable; skipped, kept as stored");
                    None
                }
            })
            .collect()
    }

    /// The stored `deadline_overrides` value exists but is not a JSON array
    /// at all: no entry can be listed, and [`Self::set_deadline_overrides`]
    /// refuses to write over it (decision P5-10).
    pub fn deadline_overrides_unreadable(&self) -> bool {
        self.stored_override_entries().is_err()
    }

    /// The stored override entries that cannot be read, each as its stored
    /// text (decision P5-10: shown, and kept as stored by every save).
    pub fn damaged_deadline_overrides(&self) -> Vec<String> {
        self.damaged_deadline_overrides_checked()
            .unwrap_or_default()
    }

    /// Persist deadline overrides to the settings table. An entry stored
    /// before that cannot be read is kept, byte for byte, ahead of the
    /// written ones: a save never drops or rewrites it (decision P5-10). A
    /// stored value that is not a JSON array at all is not overwritten.
    pub fn set_deadline_overrides(
        &self,
        overrides: &[crate::calendar_rules::DeadlineOverride],
    ) -> Result<(), DbError> {
        let mut entries = self.damaged_deadline_overrides_checked()?;
        for entry in overrides {
            entries.push(serde_json::to_string(entry).map_err(|e| DbError::Other(e.to_string()))?);
        }
        self.set_setting("deadline_overrides", &format!("[{}]", entries.join(",")))?;
        self.request_post_commit_refresh()
    }

    /// [`Self::damaged_deadline_overrides`], failing when the stored value is
    /// not an array (so a save refuses rather than overwrite it).
    fn damaged_deadline_overrides_checked(&self) -> Result<Vec<String>, DbError> {
        Ok(self
            .stored_override_entries()?
            .unwrap_or_default()
            .into_iter()
            .filter(|entry| {
                serde_json::from_str::<crate::calendar_rules::DeadlineOverride>(entry).is_err()
            })
            .collect())
    }

    /// Seed the official non-working holidays the app ships
    /// ([`crate::official_holidays::OFFICIAL_HOLIDAY_YEARS`]) into the stored
    /// deadline overrides (decision D8a). Returns how many were added.
    pub fn seed_official_holidays(&self) -> Result<usize, DbError> {
        self.seed_holidays_from(crate::official_holidays::OFFICIAL_HOLIDAY_YEARS)
    }

    /// [`Self::seed_official_holidays`] for an explicit list of years.
    /// Returns how many overrides were appended.
    ///
    /// Per holiday date and form code:
    /// - a code an existing override already covers on that date (its
    ///   `original_deadline` and `affected_form_codes`) keeps that override
    ///   and is not seeded, so one saved override for one form blocks the
    ///   holiday for that form only;
    /// - each (date, code) is offered once, recorded under
    ///   [`HOLIDAY_SEED_LEDGER_KEY`] as `YYYY-MM-DD|CODE`, so a re-run adds
    ///   only new years, dates and codes, and a seed the user deleted is not
    ///   brought back. A pair the user's own override covers is recorded as
    ///   offered too, so deleting that override later does not restore the
    ///   holiday for it. A ledger entry of the earlier format (`YYYY-MM-DD`
    ///   alone) counts as offered for every form on that date;
    /// - what is left for a date is appended as one override.
    ///
    /// Stored overrides are never modified, removed or re-identified: the
    /// stored JSON text is kept byte for byte (unknown keys included) and
    /// the new entries are appended to it. Overrides or a ledger that do
    /// not parse stop the seeding. Both settings are read and written in one
    /// immediate transaction.
    pub fn seed_holidays_from(
        &self,
        years: &[crate::official_holidays::HolidayYear],
    ) -> Result<usize, DbError> {
        use std::collections::BTreeSet;
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        let stored = self.get_setting("deadline_overrides")?;
        let existing: Vec<serde_json::Value> = match stored.as_deref() {
            None => Vec::new(),
            Some(json) => match serde_json::from_str::<serde_json::Value>(json) {
                Ok(serde_json::Value::Array(entries)) => entries,
                _ => {
                    tracing::warn!("stored deadline overrides do not parse; holidays not seeded");
                    return Ok(0);
                }
            },
        };
        let ledger: BTreeSet<String> = match self.get_setting(HOLIDAY_SEED_LEDGER_KEY)? {
            None => BTreeSet::new(),
            Some(json) => match serde_json::from_str(&json) {
                Ok(ledger) => ledger,
                Err(error) => {
                    tracing::warn!(%error, "the holiday seed ledger does not parse; holidays not seeded");
                    return Ok(0);
                }
            },
        };
        let mut covered: BTreeSet<(String, String)> = BTreeSet::new();
        let mut ids: BTreeSet<String> = BTreeSet::new();
        for entry in &existing {
            if let Some(id) = entry.get("id").and_then(serde_json::Value::as_str) {
                ids.insert(id.to_string());
            }
            let Some(date) = entry
                .get("original_deadline")
                .and_then(serde_json::Value::as_str)
            else {
                continue;
            };
            for code in entry
                .get("affected_form_codes")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
            {
                covered.insert((date.to_string(), code.to_string()));
            }
        }
        let mut offered = ledger.clone();
        let mut appended = Vec::new();
        for mut seed in crate::official_holidays::holiday_overrides(years) {
            let date = seed.original_deadline.to_string();
            let legacy = ledger.contains(&date);
            seed.affected_form_codes.retain(|code| {
                let new = offered.insert(format!("{date}|{code}"));
                new && !legacy && !covered.contains(&(date.clone(), code.clone()))
            });
            if seed.affected_form_codes.is_empty() {
                continue;
            }
            let base = seed.id.clone();
            let mut n = 2;
            while ids.contains(&seed.id) {
                seed.id = format!("{base}-{n}");
                n += 1;
            }
            ids.insert(seed.id.clone());
            appended.push(seed);
        }
        if appended.is_empty() && offered == ledger {
            return Ok(0);
        }
        if !appended.is_empty() {
            let json = append_json_entries(stored.as_deref(), existing.is_empty(), &appended)?;
            self.set_setting("deadline_overrides", &json)?;
        }
        if offered != ledger {
            let json =
                serde_json::to_string(&offered).map_err(|e| DbError::Other(e.to_string()))?;
            self.set_setting(HOLIDAY_SEED_LEDGER_KEY, &json)?;
        }
        tx.commit()?;
        if !appended.is_empty() {
            let _ = self.request_post_commit_refresh();
        }
        Ok(appended.len())
    }
}

/// Settings key of the (holiday date, form code) pairs
/// [`Database::seed_holidays_from`] has offered: a JSON array of
/// `YYYY-MM-DD|CODE`.
pub const HOLIDAY_SEED_LEDGER_KEY: &str = "official_holidays_seeded";

/// The top-level elements of a JSON array, each as its exact text (trimmed
/// of the whitespace around it). `None` unless `text` is one well-formed
/// JSON array.
fn split_json_array(text: &str) -> Option<Vec<&str>> {
    if !matches!(
        serde_json::from_str::<serde_json::Value>(text),
        Ok(serde_json::Value::Array(_))
    ) {
        return None;
    }
    let body = text.trim();
    let inner = &body[1..body.len() - 1];
    let (mut depth, mut in_string, mut escaped, mut start) = (0i32, false, false, 0usize);
    let mut out = Vec::new();
    for (index, c) in inner.char_indices() {
        if in_string {
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '[' | '{' => depth += 1,
            ']' | '}' => depth -= 1,
            ',' if depth == 0 => {
                out.push(inner[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }
    let last = inner[start..].trim();
    if !last.is_empty() {
        out.push(last);
    }
    Some(out)
}

/// `stored` (a JSON array, or nothing) with `entries` appended, every
/// stored byte before the closing bracket kept as it is.
fn append_json_entries<T: serde::Serialize>(
    stored: Option<&str>,
    stored_is_empty: bool,
    entries: &[T],
) -> Result<String, DbError> {
    let items = entries
        .iter()
        .map(serde_json::to_string)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| DbError::Other(e.to_string()))?
        .join(",");
    let Some(stored) = stored else {
        return Ok(format!("[{items}]"));
    };
    // The array's closing bracket is its last one (only whitespace can
    // follow it in JSON that parsed); the bytes after it are kept too.
    let close = stored
        .rfind(']')
        .ok_or_else(|| DbError::Other("stored deadline overrides are not an array".into()))?;
    let (head, tail) = (&stored[..close], &stored[close + 1..]);
    let separator = if stored_is_empty { "" } else { "," };
    Ok(format!("{head}{separator}{items}]{tail}"))
}

// =========================================================================
// Utility functions
// =========================================================================

pub fn parse_2551q_period(period: &str) -> Option<(u16, u8)> {
    let q_pos = period.rfind('Q')?;
    let quarter = period[q_pos + 1..].parse::<u8>().ok()?;
    let before_q = &period[..q_pos];
    let year_str = before_q.get(before_q.len().saturating_sub(4)..)?;
    let year = year_str.parse::<u16>().ok()?;
    Some((year, quarter))
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(all(test, target_os = "macos"))]
mod keychain_miss_tests {
    use super::{SECURITY_ERR_ITEM_NOT_FOUND, keychain_miss_is_genuine};

    // This predicate decides whether the app may generate a new SQLCipher
    // master key. Say yes when a key actually exists but is merely unreadable
    // and the app overwrites it, making the encrypted database permanently
    // unopenable. Every case below is a real failure mode of `security(1)`.

    #[test]
    fn exit_44_is_the_only_genuine_miss() {
        assert!(keychain_miss_is_genuine(Some(SECURITY_ERR_ITEM_NOT_FOUND)));
        assert_eq!(SECURITY_ERR_ITEM_NOT_FOUND, 44);
    }

    #[test]
    fn authorization_denied_is_not_a_miss() {
        // The user dismissed or denied the keychain prompt. A key exists.
        assert!(!keychain_miss_is_genuine(Some(51)));
        assert!(!keychain_miss_is_genuine(Some(1)));
    }

    #[test]
    fn success_is_not_a_miss() {
        assert!(!keychain_miss_is_genuine(Some(0)));
    }

    #[test]
    fn killed_by_signal_is_not_a_miss() {
        // No exit code at all. An unknown failure must never read as "absent".
        assert!(!keychain_miss_is_genuine(None));
    }

    #[test]
    fn no_other_code_in_a_wide_sweep_is_treated_as_a_miss() {
        for code in -128..=255 {
            if code == SECURITY_ERR_ITEM_NOT_FOUND {
                continue;
            }
            assert!(
                !keychain_miss_is_genuine(Some(code)),
                "exit {code} must not be treated as a missing key"
            );
        }
    }
}

/// Outcome of reading the SQLCipher master key from the OS credential store.
#[cfg(not(target_os = "macos"))]
#[derive(Debug, PartialEq, Eq)]
enum KeyringRead {
    Found(String),
    Missing,
    Unavailable(String),
    Refuse(String),
}

#[cfg(not(target_os = "macos"))]
fn classify_keyring_read(result: Result<String, keyring::Error>) -> KeyringRead {
    match result {
        Ok(hex_key) => KeyringRead::Found(hex_key),
        Err(keyring::Error::NoEntry) => KeyringRead::Missing,
        // keyring maps D-Bus connection/service errors to PlatformFailure;
        // a locked collection or dismissed prompt is NoStorageAccess and
        // must not be treated as "no store".
        Err(e @ keyring::Error::PlatformFailure(_)) => KeyringRead::Unavailable(e.to_string()),
        Err(e) => KeyringRead::Refuse(e.to_string()),
    }
}

#[cfg(feature = "test-support")]
impl Database {
    /// The raw connection, for another crate's test fixtures only (see the
    /// `test-support` feature).
    pub fn conn_for_tests(&self) -> &Connection {
        &self.conn
    }
}

#[cfg(test)]
mod tests {
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn keyring_read_falls_back_only_when_store_is_unreachable() {
        use super::{KeyringRead, classify_keyring_read};
        let key = "ab".repeat(32);
        assert_eq!(
            classify_keyring_read(Ok(key.clone())),
            KeyringRead::Found(key)
        );
        assert_eq!(
            classify_keyring_read(Err(keyring::Error::NoEntry)),
            KeyringRead::Missing
        );
        // No Secret Service on the bus surfaces as PlatformFailure.
        let unreachable =
            keyring::Error::PlatformFailure("org.freedesktop.DBus.Error.ServiceUnknown".into());
        assert!(matches!(
            classify_keyring_read(Err(unreachable)),
            KeyringRead::Unavailable(_)
        ));
        // A locked collection may still hold the live key: never regenerate.
        let locked = keyring::Error::NoStorageAccess("locked".into());
        assert!(matches!(
            classify_keyring_read(Err(locked)),
            KeyringRead::Refuse(_)
        ));
    }

    use super::*;
    use crate::naming::Tin;
    use crate::profile::TaxpayerProfile;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn the_environment_supplies_the_database_key() {
        let key = "ab".repeat(32);
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("db.key");
        std::fs::write(&path, format!("{key}\n")).expect("key file");
        let path = path.to_string_lossy().into_owned();
        let only = |name: &'static str, value: String| {
            move |asked: &str| (asked == name).then(|| value.clone())
        };

        assert!(env_master_key(|_| None).is_none());
        assert!(env_master_key(only("BIR_DB_KEY", String::new())).is_none());
        assert_eq!(
            env_master_key(only("BIR_DB_KEY", key.clone()))
                .unwrap()
                .unwrap(),
            key
        );
        assert_eq!(
            env_master_key(only("BIR_DB_KEY_FILE", path))
                .unwrap()
                .unwrap(),
            key
        );
        assert!(
            env_master_key(only("BIR_DB_KEY", "abc".into()))
                .unwrap()
                .is_err()
        );
        let missing = "/nonexistent/db.key".to_string();
        assert!(
            env_master_key(only("BIR_DB_KEY_FILE", missing))
                .unwrap()
                .is_err()
        );
    }

    // A mock keyring can be tricky in CI, so we might encounter issues depending on the runner.
    // Assuming macOS with user session or a mock environment.
    #[test]
    fn test_profile_crud() {
        let db_file = NamedTempFile::new().unwrap();
        // Since keyring depends on OS, this might fail in headless environments without a keychain.
        // For the sake of the test, we'll try to open it and skip if the OS keychain isn't available.
        let db = match Database::open(db_file.path()) {
            Ok(db) => db,
            Err(e) => {
                println!("Skipping test due to keyring unavailability: {:?}", e);
                return;
            }
        };

        let profile = TaxpayerProfile {
            id: None,
            full_name: "Code It Like Miley".into(),
            tin: Tin {
                segment1: "000".into(),
                segment2: "234".into(),
                segment3: "564".into(),
                branch: "000".into(),
            },
            rdo_code: "039".into(),
            line_of_business: "Software".into(),
            registered_address: "QC".into(),
            is_archived: false,
            email_tracking_enabled: false,
            test_notification_enabled: false,
            profile_versions: vec![],
            compliance_source_mode: Default::default(),
            per_year_forms: Default::default(),
            zip_code: "1103".into(),
            phone: "0999".into(),
            email: "tester@example.com".into(),
            default_form_type: "2551Qv2018".into(),
            taxpayer_type: crate::profile::TaxpayerType::Individual,
            is_vat_registered: false,
            business_start_date: None,
            birth_date: None,
            tax_classification: None,
            eopt_tier: None,
            is_bmbe: false,
            is_gpp_partner: false,
            is_create_msme: false,
            is_expanded_withholding_agent: false,
            atc_codes: vec![],
            excise_tax_categories: vec![],
            tax_elections: vec![],
            has_employees: false,
            is_dormant: false,
            has_single_employer: false,
            withholds_compensation: false,
            withholds_expanded: false,
            withholds_final: false,
            is_top_withholding_agent: false,
            is_government_withholding_entity: false,
            registration_activity_status: Default::default(),
            profile_pin_hash: None,
        };

        db.save_profile(profile).expect("Failed to save profile");

        let retrieved = db
            .get_profile("000234564000")
            .expect("Failed to query")
            .expect("Profile not found");
        assert_eq!(retrieved.full_name, "Code It Like Miley");

        let list = db.list_profiles().expect("Failed to list");
        assert_eq!(list.len(), 1);

        db.delete_profile("000234564000").expect("Failed to delete");
        let list_after = db.list_profiles().expect("Failed to list");
        assert_eq!(list_after.len(), 0);
    }

    #[test]
    fn test_open_or_recreate_quarantines_unreadable_db() {
        let mut db_file = NamedTempFile::new().unwrap();
        db_file.write_all(b"not-a-sqlite-database").unwrap();

        let (db, backup_path) = match Database::open_or_recreate(db_file.path()) {
            Ok(result) => result,
            Err(e) => {
                println!("Skipping test due to keyring unavailability: {:?}", e);
                return;
            }
        };

        let backup_path = backup_path.expect("expected unreadable db to be quarantined");
        assert!(backup_path.exists(), "backup file should exist");
        assert!(db_file.path().exists(), "database file should be recreated");
        assert!(
            db.list_profiles().is_ok(),
            "recreated database should be usable"
        );
    }

    #[test]
    fn open_existing_read_only_never_creates_a_missing_database() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database_path = directory.path().join("missing.db");

        let error = match Database::open_existing_read_only(&database_path) {
            Ok(_) => panic!("a missing source database must fail closed"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("does not exist"));
        assert!(!database_path.exists());
    }

    #[test]
    fn open_existing_read_only_rejects_source_writes() {
        let database_file = NamedTempFile::new().expect("temporary database");
        let database = Database::open(database_file.path()).expect("create encrypted database");
        database
            .set_setting("read_only_probe", "preserved")
            .expect("seed source database");
        database.close().expect("close source database");

        let read_only = Database::open_existing_read_only(database_file.path())
            .expect("open existing database read-only");
        assert_eq!(
            read_only
                .get_setting("read_only_probe")
                .expect("read setting"),
            Some("preserved".to_string())
        );
        assert!(
            read_only.set_setting("read_only_probe", "changed").is_err(),
            "the diagnostic connection must not modify the source database"
        );
    }

    #[test]
    fn app_database_path_defaults_to_data_dir_bir_data() {
        temp_env::with_var("BIR_DATABASE_PATH", None::<&str>, || {
            assert_eq!(app_database_path(), default_database_path());
            assert_eq!(
                default_database_path(),
                crate::platform::data_dir().join("bir_data.db")
            );
        });
        temp_env::with_var("BIR_DATABASE_PATH", Some(""), || {
            assert_eq!(app_database_path(), default_database_path());
        });
    }

    #[test]
    fn app_database_path_honors_bir_database_path_override() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let override_path = directory.path().join("ci-bir.db");
        temp_env::with_var("BIR_DATABASE_PATH", Some(&override_path), || {
            assert_eq!(app_database_path(), override_path);
            assert_ne!(app_database_path(), default_database_path());
        });
    }

    #[test]
    fn open_live_database_is_file_backed_not_ephemeral() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let db_path = directory.path().join("live.db");
        temp_env::with_var("BIR_DATABASE_PATH", Some(db_path.as_os_str()), || {
            let (db, opened, recovered) = open_live_database().expect("open live file db");
            assert_eq!(opened, db_path);
            assert!(recovered.is_none());
            assert!(db_path.is_file());
            db.set_setting("probe", "visible")
                .expect("write through the file-backed connection");
            drop(db);
            let reopened = Database::open(&db_path).expect("reopen file db");
            assert_eq!(
                reopened.get_setting("probe").expect("read"),
                Some("visible".into())
            );
        });
    }

    const EXTRA_YEAR: crate::official_holidays::HolidayYear =
        crate::official_holidays::HolidayYear {
            year: 2027,
            proclamation: Some("Test proclamation 2027"),
            days: &[
                crate::official_holidays::Holiday {
                    month: 1,
                    day: 1,
                    name: "New Year's Day",
                },
                crate::official_holidays::Holiday {
                    month: 6,
                    day: 14,
                    name: "Test Monday holiday",
                },
            ],
        };

    fn ymd(y: i32, m: u32, d: u32) -> chrono::NaiveDate {
        chrono::NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn first_run_seeds_every_shipped_holiday() {
        let db = Database::open_in_memory_for_tests().unwrap();
        let shipped: usize = crate::official_holidays::OFFICIAL_HOLIDAY_YEARS
            .iter()
            .map(|year| year.days.len())
            .sum();
        assert!(shipped > 0);
        assert_eq!(db.seed_official_holidays().unwrap(), shipped);
        let overrides = db.get_deadline_overrides();
        assert_eq!(overrides.len(), shipped);
        let christmas = overrides
            .iter()
            .find(|o| o.original_deadline == ymd(2026, 12, 25))
            .expect("Christmas 2026 seeded");
        assert_eq!(christmas.id, "official-holiday-2026-12-25");
        assert_eq!(christmas.adjusted_deadline, ymd(2026, 12, 28));
        assert!(!christmas.source_reference.trim().is_empty());
        // A seeded holiday moves a deadline that lands on it: 1702Q Q3 (60
        // days after September 30) is due Sunday November 29, 2026, moved
        // to Monday the 30th, Bonifacio Day, and so on to December 1.
        let rows = crate::calendar_rules::DeadlineResolver::deadlines_for_forms(
            &["1702Q".to_string()],
            2026,
            &overrides,
        );
        let q3 = rows
            .iter()
            .find(|d| d.period.quarter() == Some(3))
            .expect("1702Q Q3");
        assert_eq!(q3.original_deadline_date(), Some(ymd(2026, 11, 29)));
        assert_eq!(q3.final_deadline_date(), Some(ymd(2026, 12, 1)));
    }

    #[test]
    fn a_second_run_adds_nothing() {
        let db = Database::open_in_memory_for_tests().unwrap();
        db.seed_official_holidays().unwrap();
        let first = db.get_setting("deadline_overrides").unwrap();
        assert_eq!(db.seed_official_holidays().unwrap(), 0);
        assert_eq!(db.get_setting("deadline_overrides").unwrap(), first);
    }

    #[test]
    fn a_saved_override_on_a_holiday_is_kept() {
        let db = Database::open_in_memory_for_tests().unwrap();
        let saved = crate::calendar_rules::DeadlineOverride {
            id: "user-christmas".to_string(),
            title: "Saved by the user".to_string(),
            source_reference: "RMC 99-2026".to_string(),
            affected_form_codes: vec!["1601C".to_string()],
            original_deadline: ymd(2026, 12, 25),
            adjusted_deadline: ymd(2027, 1, 4),
            affected_regions: Vec::new(),
            affected_taxpayer_types: Vec::new(),
            effective_from: None,
            effective_until: None,
            expires_at: None,
        };
        db.set_deadline_overrides(std::slice::from_ref(&saved))
            .unwrap();
        let stored = db.get_setting("deadline_overrides").unwrap().unwrap();
        let shipped = crate::official_holidays::OFFICIAL_HOLIDAY_YEARS[0]
            .days
            .len();
        // Every date is seeded: Christmas too, for every form but 1601C.
        assert_eq!(db.seed_official_holidays().unwrap(), shipped);
        let overrides = db.get_deadline_overrides();
        let christmas: Vec<_> = overrides
            .iter()
            .filter(|o| o.original_deadline == ymd(2026, 12, 25))
            .collect();
        assert_eq!(christmas.len(), 2);
        assert_eq!(
            christmas[0].id, "user-christmas",
            "saved overrides stay first"
        );
        assert_eq!(christmas[0].affected_form_codes, ["1601C"]);
        assert_eq!(christmas[0].adjusted_deadline, ymd(2027, 1, 4));
        assert_eq!(christmas[0].source_reference, "RMC 99-2026");
        assert_eq!(christmas[1].id, "official-holiday-2026-12-25");
        assert!(
            !christmas[1]
                .affected_form_codes
                .iter()
                .any(|c| c == "1601C")
        );
        assert!(
            christmas[1]
                .affected_form_codes
                .iter()
                .any(|c| c == "0619E")
        );
        let after = db.get_setting("deadline_overrides").unwrap().unwrap();
        assert!(
            after.starts_with(stored.trim_end().strip_suffix(']').unwrap()),
            "the saved entry's stored text is kept"
        );
        // The user's date moves 1601C; the seed moves the other forms.
        let codes = ["1601C".to_string(), "0619E".to_string()];
        let rows =
            crate::calendar_rules::DeadlineResolver::deadlines_for_forms(&codes, 2026, &overrides);
        assert!(
            rows.iter()
                .all(|d| d.final_deadline_date() != Some(ymd(2026, 12, 25)))
        );
    }

    /// The desktop's former 2026 seed (13 form codes, random ids, cited
    /// "RR 2026-001") as an install holds it: kept byte for byte, and the
    /// holiday added for every form it did not cover.
    #[test]
    fn the_former_desktop_seed_is_kept_and_completed_for_the_other_forms() {
        let db = Database::open_in_memory_for_tests().unwrap();
        let old_codes = [
            "2000", "0619E", "0619F", "1601C", "0620", "1606", "2551Q", "2550Q", "1701Q", "1601EQ",
            "1601FQ", "1602Q", "1603Q",
        ];
        let old = |id: &str, title: &str, original: &str, adjusted: &str| {
            serde_json::json!({
                "id": id, "title": title, "source_reference": "RR 2026-001",
                "affected_form_codes": old_codes, "original_deadline": original,
                "adjusted_deadline": adjusted, "affected_regions": [],
                "affected_taxpayer_types": [], "effective_from": null,
                "effective_until": null, "expires_at": null
            })
        };
        let stored = serde_json::to_string(&[
            old(
                "9b1c0f7e-0000-4000-8000-000000000001",
                "New Year's Day 2026",
                "2026-01-01",
                "2026-01-02",
            ),
            old(
                "9b1c0f7e-0000-4000-8000-000000000002",
                "Maundy Thursday 2026",
                "2026-04-02",
                "2026-04-03",
            ),
            old(
                "9b1c0f7e-0000-4000-8000-000000000003",
                "Good Friday 2026",
                "2026-04-03",
                "2026-04-06",
            ),
        ])
        .unwrap();
        db.set_setting("deadline_overrides", &stored).unwrap();
        let shipped = crate::official_holidays::OFFICIAL_HOLIDAY_YEARS[0]
            .days
            .len();
        assert_eq!(db.seed_official_holidays().unwrap(), shipped);
        let after = db.get_setting("deadline_overrides").unwrap().unwrap();
        assert!(
            after.starts_with(&stored[..stored.len() - 1]),
            "kept byte for byte"
        );
        let overrides = db.get_deadline_overrides();
        let jan1: Vec<_> = overrides
            .iter()
            .filter(|o| o.original_deadline == ymd(2026, 1, 1))
            .collect();
        assert_eq!(jan1.len(), 2);
        assert_eq!(jan1[0].affected_form_codes, old_codes);
        assert!(
            jan1[1]
                .affected_form_codes
                .iter()
                .all(|c| !old_codes.contains(&c.as_str()))
        );
        assert!(jan1[1].affected_form_codes.iter().any(|c| c == "2200C"));
        assert_eq!(db.seed_official_holidays().unwrap(), 0);
    }

    #[test]
    fn one_saved_override_for_one_form_blocks_the_holiday_for_that_form_only() {
        let db = Database::open_in_memory_for_tests().unwrap();
        db.set_setting(
            "deadline_overrides",
            r#"[{"id":"mine","title":"Mine","source_reference":"RMC","affected_form_codes":["2551Q"],"original_deadline":"2026-01-01","adjusted_deadline":"2026-01-05","affected_regions":[],"affected_taxpayer_types":[],"effective_from":null,"effective_until":null,"expires_at":null}]"#,
        )
        .unwrap();
        db.seed_official_holidays().unwrap();
        let overrides = db.get_deadline_overrides();
        let seeded = overrides
            .iter()
            .find(|o| o.id == "official-holiday-2026-01-01")
            .unwrap();
        assert!(!seeded.affected_form_codes.iter().any(|c| c == "2551Q"));
        assert!(seeded.affected_form_codes.iter().any(|c| c == "1601C"));
        let ledger: std::collections::BTreeSet<String> =
            serde_json::from_str(&db.get_setting(HOLIDAY_SEED_LEDGER_KEY).unwrap().unwrap())
                .unwrap();
        assert!(
            ledger.contains("2026-01-01|2551Q"),
            "offered, kept as the user's"
        );
        assert!(ledger.contains("2026-01-01|1601C"));
    }

    /// Review F21: an entry's keys this build does not know survive.
    #[test]
    fn an_override_with_an_extra_key_is_kept_exactly() {
        let db = Database::open_in_memory_for_tests().unwrap();
        let stored = "[ {\"id\":\"x\", \"note\": \"from a newer build\", \"original_deadline\":\"2026-03-03\",\"affected_form_codes\":[\"1601C\"]} ]\n";
        db.set_setting("deadline_overrides", stored).unwrap();
        assert!(db.seed_official_holidays().unwrap() > 0);
        let after = db.get_setting("deadline_overrides").unwrap().unwrap();
        assert!(after.starts_with("[ {\"id\":\"x\", \"note\": \"from a newer build\", \"original_deadline\":\"2026-03-03\",\"affected_form_codes\":[\"1601C\"]} ,{"));
        assert!(after.ends_with("]\n"), "the bytes after the array are kept");
        let parsed: serde_json::Value = serde_json::from_str(&after).unwrap();
        assert_eq!(parsed[0]["note"], "from a newer build");
    }

    /// Round 2, item 4: a ledger of the earlier format (dates alone) counts
    /// a date as offered for every form, so a seed deleted under it stays
    /// deleted, and a date it never offered is still seeded.
    #[test]
    fn a_legacy_ledger_date_keeps_a_deleted_seed_deleted() {
        let db = Database::open_in_memory_for_tests().unwrap();
        db.set_setting(HOLIDAY_SEED_LEDGER_KEY, r#"["2026-06-12"]"#)
            .unwrap();
        db.seed_official_holidays().unwrap();
        let overrides = db.get_deadline_overrides();
        assert!(
            overrides
                .iter()
                .all(|o| o.original_deadline != ymd(2026, 6, 12)),
            "Independence Day stays deleted"
        );
        assert!(
            overrides
                .iter()
                .any(|o| o.original_deadline == ymd(2026, 12, 25))
        );
        let ledger: std::collections::BTreeSet<String> =
            serde_json::from_str(&db.get_setting(HOLIDAY_SEED_LEDGER_KEY).unwrap().unwrap())
                .unwrap();
        assert!(ledger.contains("2026-06-12"), "the old entry is kept");
        assert!(ledger.contains("2026-06-12|1601C"));
        assert_eq!(db.seed_official_holidays().unwrap(), 0);
    }

    /// Round 2, item 13: a pair covered by the user's own override is
    /// recorded as offered, so deleting that override later does not bring
    /// the holiday back for that form.
    #[test]
    fn deleting_a_users_own_override_does_not_restore_the_holiday() {
        let db = Database::open_in_memory_for_tests().unwrap();
        db.set_setting(
            "deadline_overrides",
            r#"[{"id":"mine","title":"Mine","source_reference":"RMC","affected_form_codes":["2551Q"],"original_deadline":"2026-01-01","adjusted_deadline":"2026-01-05","affected_regions":[],"affected_taxpayer_types":[],"effective_from":null,"effective_until":null,"expires_at":null}]"#,
        )
        .unwrap();
        db.seed_official_holidays().unwrap();
        let mut overrides = db.get_deadline_overrides();
        overrides.retain(|o| o.id != "mine");
        db.set_deadline_overrides(&overrides).unwrap();
        assert_eq!(db.seed_official_holidays().unwrap(), 0);
        assert!(db.get_deadline_overrides().iter().all(|o| {
            o.original_deadline != ymd(2026, 1, 1)
                || !o.affected_form_codes.iter().any(|c| c == "2551Q")
        }));
    }

    #[test]
    fn a_ledger_that_does_not_parse_stops_the_seeding() {
        let db = Database::open_in_memory_for_tests().unwrap();
        db.set_setting(HOLIDAY_SEED_LEDGER_KEY, "{not a list")
            .unwrap();
        assert_eq!(db.seed_official_holidays().unwrap(), 0);
        assert_eq!(db.get_setting("deadline_overrides").unwrap(), None);
        assert_eq!(
            db.get_setting(HOLIDAY_SEED_LEDGER_KEY).unwrap().as_deref(),
            Some("{not a list"),
            "not reset to empty"
        );
    }

    /// The overrides and the ledger are written together or not at all.
    #[test]
    fn the_seed_writes_both_settings_in_one_transaction() {
        let db = Database::open_in_memory_for_tests().unwrap();
        db.conn
            .execute_batch(&format!(
                "CREATE TEMP TRIGGER refuse_ledger BEFORE INSERT ON settings
                 WHEN NEW.key = '{HOLIDAY_SEED_LEDGER_KEY}'
                 BEGIN SELECT RAISE(ABORT, 'ledger refused'); END;"
            ))
            .unwrap();
        assert!(db.seed_official_holidays().is_err());
        assert_eq!(
            db.get_setting("deadline_overrides").unwrap(),
            None,
            "the overrides write rolled back with the ledger's"
        );
        db.conn.execute_batch("DROP TRIGGER refuse_ledger").unwrap();
        assert!(db.seed_official_holidays().unwrap() > 0);
    }

    #[test]
    fn a_missing_year_is_added_and_nothing_else() {
        let db = Database::open_in_memory_for_tests().unwrap();
        let shipped = crate::official_holidays::OFFICIAL_HOLIDAY_YEARS;
        db.seed_holidays_from(shipped).unwrap();
        let before = db.get_deadline_overrides();
        let mut with_extra = shipped.to_vec();
        with_extra.push(EXTRA_YEAR);
        assert_eq!(db.seed_holidays_from(&with_extra).unwrap(), 2);
        let after = db.get_deadline_overrides();
        assert_eq!(after.len(), before.len() + 2);
        assert_eq!(
            after[..before.len()]
                .iter()
                .map(|o| (&o.id, o.original_deadline, o.adjusted_deadline))
                .collect::<Vec<_>>(),
            before
                .iter()
                .map(|o| (&o.id, o.original_deadline, o.adjusted_deadline))
                .collect::<Vec<_>>(),
            "the seeded years are unchanged"
        );
        let added: Vec<_> = after[before.len()..]
            .iter()
            .map(|o| (o.original_deadline, o.source_reference.as_str()))
            .collect();
        assert_eq!(
            added,
            [
                (ymd(2027, 1, 1), "Test proclamation 2027"),
                (ymd(2027, 6, 14), "Test proclamation 2027"),
            ]
        );
        assert_eq!(db.seed_holidays_from(&with_extra).unwrap(), 0);
    }

    #[test]
    fn a_seeded_holiday_the_user_deleted_is_not_brought_back() {
        let db = Database::open_in_memory_for_tests().unwrap();
        db.seed_official_holidays().unwrap();
        let mut overrides = db.get_deadline_overrides();
        overrides.retain(|o| o.original_deadline != ymd(2026, 6, 12));
        db.set_deadline_overrides(&overrides).unwrap();
        assert_eq!(db.seed_official_holidays().unwrap(), 0);
        assert!(
            db.get_deadline_overrides()
                .iter()
                .all(|o| o.original_deadline != ymd(2026, 6, 12))
        );
    }

    #[test]
    fn stored_overrides_that_do_not_parse_are_left_alone() {
        let db = Database::open_in_memory_for_tests().unwrap();
        db.set_setting("deadline_overrides", "not json").unwrap();
        assert_eq!(db.seed_official_holidays().unwrap(), 0);
        assert_eq!(
            db.get_setting("deadline_overrides").unwrap().as_deref(),
            Some("not json")
        );
    }

    /// Decision P5-10: three stored entries, the middle one damaged (no
    /// `title`). Two apply and the damaged one is listed; a save adding a
    /// fourth keeps the damaged one's exact bytes; the Calendar and the
    /// penalty clock see the two good ones.
    #[test]
    fn a_damaged_override_is_skipped_listed_and_kept() {
        let db = Database::open_in_memory_for_tests().unwrap();
        let good = |id: &str, code: &str, from: &str, to: &str| {
            format!(
                r#"{{"id":"{id}","title":"Good {id}","source_reference":"RMC","affected_form_codes":["{code}"],"original_deadline":"{from}","adjusted_deadline":"{to}","affected_regions":[],"affected_taxpayer_types":[],"effective_from":null,"effective_until":null,"expires_at":null}}"#
            )
        };
        let damaged = r#"{"id":"broken", "original_deadline":"2026-10-12",  "note":"no title"}"#;
        let stored = format!(
            "[{}, {damaged}, {}]",
            good("a", "1601C", "2026-10-12", "2026-10-20"),
            good("b", "0619E", "2026-11-10", "2026-11-12")
        );
        db.set_setting("deadline_overrides", &stored).unwrap();
        let read = db.get_deadline_overrides();
        assert_eq!(
            read.iter().map(|o| o.id.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(db.damaged_deadline_overrides(), [damaged]);
        // The Calendar sees the good entry: 1601C September moves to Oct 20.
        let rows = crate::calendar_rules::DeadlineResolver::deadlines_for_forms(
            &["1601C".to_string()],
            2026,
            &read,
        );
        let september = rows.iter().find(|d| d.period.month() == Some(9)).unwrap();
        assert_eq!(september.final_deadline_date(), Some(ymd(2026, 10, 20)));
        // And so does the penalty clock, which resolves from the same list.
        assert_eq!(
            crate::forms::penalty_items::resolve_due_date(
                "1601C",
                &crate::calendar_rules::DeadlinePeriod::Monthly {
                    taxable_year: 2026,
                    month: 9
                },
                &read
            ),
            Some(ymd(2026, 10, 20))
        );
        // A save adding a fourth keeps the damaged entry's bytes.
        let mut more = read.clone();
        let mut fourth = more[0].clone();
        fourth.id = "d".into();
        more.push(fourth);
        db.set_deadline_overrides(&more).unwrap();
        let after = db.get_setting("deadline_overrides").unwrap().unwrap();
        assert!(after.contains(damaged), "{after}");
        assert_eq!(db.get_deadline_overrides().len(), 3);
        assert_eq!(db.damaged_deadline_overrides(), [damaged]);
    }

    #[test]
    fn a_stored_value_that_is_not_an_array_is_not_overwritten() {
        let db = Database::open_in_memory_for_tests().unwrap();
        assert!(!db.deadline_overrides_unreadable());
        db.set_setting("deadline_overrides", "{not an array")
            .unwrap();
        assert!(db.get_deadline_overrides().is_empty());
        assert!(db.deadline_overrides_unreadable());
        assert!(db.damaged_deadline_overrides().is_empty());
        assert!(db.set_deadline_overrides(&[]).is_err());
        assert_eq!(
            db.get_setting("deadline_overrides").unwrap().as_deref(),
            Some("{not an array")
        );
        // An array again — even a damaged entry inside it — reads fine.
        db.set_setting("deadline_overrides", r#"[{"note":"x"}]"#)
            .unwrap();
        assert!(!db.deadline_overrides_unreadable());
        assert_eq!(db.damaged_deadline_overrides(), [r#"{"note":"x"}"#]);
    }

    #[test]
    fn open_live_database_seeds_the_shipped_holidays() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let db_path = directory.path().join("seeded.db");
        temp_env::with_var("BIR_DATABASE_PATH", Some(db_path.as_os_str()), || {
            let (db, _, _) = open_live_database().expect("open live file db");
            assert!(
                db.get_deadline_overrides()
                    .iter()
                    .any(|o| o.id == "official-holiday-2026-01-01")
            );
        });
    }

    #[test]
    fn second_open_live_database_is_refused_until_first_drops() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let db_path = directory.path().join("owned.db");
        temp_env::with_var("BIR_DATABASE_PATH", Some(db_path.as_os_str()), || {
            let (first, opened, _) = open_live_database().expect("first live owner");
            assert_eq!(opened, db_path);
            let error = match open_live_database() {
                Ok(_) => panic!("second live owner must fail"),
                Err(error) => error,
            };
            assert!(matches!(error, DbError::LiveDatabaseInUse(_)), "{error}");
            let serve_error = match open_serve_database() {
                Ok(_) => panic!("serve while GUI holds lock must fail"),
                Err(error) => error,
            };
            assert!(
                matches!(serve_error, DbError::LiveDatabaseInUse(_)),
                "{serve_error}"
            );
            drop(first);
            let (second, _, _) = open_live_database().expect("owner after drop");
            drop(second);
            open_serve_database().expect("serve after GUI drop");
        });
    }

    #[test]
    fn open_serve_database_does_not_quarantine() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let db_path = directory.path().join("serve.db");
        temp_env::with_var("BIR_DATABASE_PATH", Some(db_path.as_os_str()), || {
            {
                let db = Database::open(&db_path).expect("seed");
                db.set_setting("probe", "keep").expect("write");
            }
            let (db, opened) = open_serve_database().expect("serve open");
            assert_eq!(opened, db_path);
            assert_eq!(db.get_setting("probe").expect("read"), Some("keep".into()));
            let corrupt = std::fs::read_dir(directory.path())
                .expect("dir")
                .filter_map(|entry| entry.ok())
                .any(|entry| entry.file_name().to_string_lossy().contains("corrupt"));
            assert!(!corrupt);
        });
    }
}
