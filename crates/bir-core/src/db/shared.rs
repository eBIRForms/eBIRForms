//! Several processes sharing one SQLCipher database.
//!
//! SQLite in WAL mode already lets several processes read and write one file;
//! each write is a short IMMEDIATE transaction that waits out the 5 s busy
//! timeout. What this module adds is the coordination SQLite does not do,
//! through sidecar lock files next to the database (std `File::lock`, so the
//! same code runs on macOS, Linux and Windows; the OS drops every lock when a
//! process exits or crashes):
//!
//! - `bir_data.db.maintenance.lock`: exclusive, for seconds at a time, while
//!   the file is created, the master key is read or created, migrations run,
//!   or an unreadable file is quarantined ([`with_maintenance_lock`]).
//! - `bir_data.db.open.lock`: shared for as long as a process has the file
//!   open. Quarantine and restore take it exclusively, so they never replace
//!   a file another process is using.
//! - `bir_data.db.worker.lock`: exclusive while one process runs the send
//!   queue and the cron ([`WorkerLease`]). The first process to take it runs
//!   the jobs; the others follow and take over when it quits.
//!
//! [`ChangeWatch`] tells a process that another process committed a change.

use std::cell::RefCell;
use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rusqlite::{Connection, ErrorCode};
use tracing::{info, warn};

use super::{Database, DbError};

/// How long a process waits for another one to finish creating, migrating
/// or quarantining the database before it gives up.
pub const MAINTENANCE_LOCK_DEADLINE: Duration = Duration::from_secs(30);

/// How often a follower tries to take the worker lease.
pub const WORKER_LEASE_RETRY: Duration = Duration::from_secs(30);

/// What a write that stayed busy past the timeout tells the person.
pub const BUSY_MESSAGE: &str = "Another app is saving to this database; try again";

fn sidecar_path(db_path: &Path, suffix: &str) -> PathBuf {
    let mut raw = db_path.as_os_str().to_os_string();
    raw.push(suffix);
    PathBuf::from(raw)
}

/// `bir_data.db.maintenance.lock`.
pub fn maintenance_lock_path(db_path: &Path) -> PathBuf {
    sidecar_path(db_path, ".maintenance.lock")
}

/// `bir_data.db.open.lock`.
pub fn open_lock_path(db_path: &Path) -> PathBuf {
    sidecar_path(db_path, ".open.lock")
}

/// `bir_data.db.worker.lock`.
pub fn worker_lock_path(db_path: &Path) -> PathBuf {
    sidecar_path(db_path, ".worker.lock")
}

fn open_sidecar(lock_path: &Path) -> std::io::Result<File> {
    if let Some(parent) = lock_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(lock_path)
}

/// Take `file`'s lock (exclusive or shared), waiting up to `deadline`.
/// `Ok(false)` when the deadline passed.
fn lock_within(file: &File, exclusive: bool, deadline: Duration) -> std::io::Result<bool> {
    let until = Instant::now() + deadline;
    let mut pause = Duration::from_millis(10);
    loop {
        let attempt = if exclusive {
            file.try_lock()
        } else {
            file.try_lock_shared()
        };
        match attempt {
            Ok(()) => return Ok(true),
            Err(TryLockError::WouldBlock) => {}
            Err(TryLockError::Error(error)) => return Err(error),
        }
        let now = Instant::now();
        if now >= until {
            return Ok(false);
        }
        std::thread::sleep(pause.min(until - now));
        pause = (pause * 2).min(Duration::from_millis(200));
    }
}

thread_local! {
    /// Maintenance locks this thread holds, so a nested call (open inside a
    /// quarantine, key and migrations inside a create) runs instead of
    /// waiting on itself.
    static MAINTENANCE_HELD: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
}

struct HeldMarker(PathBuf);

impl Drop for HeldMarker {
    fn drop(&mut self) {
        MAINTENANCE_HELD.with(|held| {
            let mut held = held.borrow_mut();
            if let Some(at) = held.iter().rposition(|path| *path == self.0) {
                held.remove(at);
            }
        });
    }
}

/// Run `f` holding the database's maintenance lock: exclusive, brief,
/// waiting up to [`MAINTENANCE_LOCK_DEADLINE`] for another process to finish.
/// Held only while the file is created, the key is read or created,
/// migrations run, or a file is quarantined; never for a process's life.
/// Re-entrant on one thread.
pub fn with_maintenance_lock<R>(
    db_path: &Path,
    f: impl FnOnce() -> Result<R, DbError>,
) -> Result<R, DbError> {
    with_maintenance_lock_within(db_path, MAINTENANCE_LOCK_DEADLINE, f)
}

pub(crate) fn with_maintenance_lock_within<R>(
    db_path: &Path,
    deadline: Duration,
    f: impl FnOnce() -> Result<R, DbError>,
) -> Result<R, DbError> {
    let lock_path = maintenance_lock_path(db_path);
    if MAINTENANCE_HELD.with(|held| held.borrow().contains(&lock_path)) {
        return f();
    }
    let file = open_sidecar(&lock_path)?;
    if !lock_within(&file, true, deadline)? {
        return Err(DbError::Other(format!(
            "another app has been preparing the database {} for over {} s; \
             try again when it has finished",
            db_path.display(),
            deadline.as_secs()
        )));
    }
    MAINTENANCE_HELD.with(|held| held.borrow_mut().push(lock_path.clone()));
    let marker = HeldMarker(lock_path);
    let result = f();
    drop(marker);
    // Closing the file releases the lock.
    drop(file);
    result
}

/// The shared "this process has the file open" lock. Waits only while a
/// quarantine or restore holds it exclusively.
pub(crate) fn hold_open_lock(db_path: &Path) -> Result<File, DbError> {
    let file = open_sidecar(&open_lock_path(db_path))?;
    if !lock_within(&file, false, MAINTENANCE_LOCK_DEADLINE)? {
        return Err(DbError::Other(format!(
            "the database {} is being replaced by another app; try again",
            db_path.display()
        )));
    }
    Ok(file)
}

/// Run `f` only when no other process (and no other connection of this
/// one) has the database open: for replacing or renaming the file. Does not
/// wait; fails with a clear message instead.
pub fn with_sole_access<R>(
    db_path: &Path,
    f: impl FnOnce() -> Result<R, DbError>,
) -> Result<R, DbError> {
    let file = open_sidecar(&open_lock_path(db_path))?;
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            return Err(DbError::Other(format!(
                "another app has the database {} open; quit it and try again",
                db_path.display()
            )));
        }
        Err(TryLockError::Error(error)) => return Err(DbError::Io(error)),
    }
    let result = f();
    drop(file);
    result
}

impl DbError {
    /// SQLITE_BUSY or SQLITE_LOCKED: another connection holds the lock.
    /// Transient, never a reason to quarantine.
    pub fn is_busy(&self) -> bool {
        matches!(
            self,
            DbError::Sqlite(rusqlite::Error::SqliteFailure(error, _))
                if matches!(error.code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
        )
    }
}

/// The text of a SQLite error; a busy one says what happened in words.
pub(crate) fn describe_sqlite(error: &rusqlite::Error) -> String {
    match error {
        rusqlite::Error::SqliteFailure(failure, _)
            if matches!(
                failure.code,
                ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked
            ) =>
        {
            BUSY_MESSAGE.to_string()
        }
        other => other.to_string(),
    }
}

/// Run `f` again with backoff while it fails busy (another process is
/// writing or migrating). Any other error returns at once.
pub(crate) fn retry_busy<T>(mut f: impl FnMut() -> Result<T, DbError>) -> Result<T, DbError> {
    // Each attempt already waits out the 5 s busy timeout.
    let mut pause = Duration::from_millis(200);
    for _ in 0..3 {
        match f() {
            Err(error) if error.is_busy() => {
                warn!(
                    "database busy on open; retrying in {} ms",
                    pause.as_millis()
                );
                std::thread::sleep(pause);
                pause *= 2;
            }
            other => return other,
        }
    }
    f()
}

/// Run the migrations once across processes: the caller holds the
/// maintenance lock; inside `BEGIN EXCLUSIVE` the schema version is read
/// again (another process may have migrated already) and only what is
/// missing runs. A failure rolls the whole step back.
pub(crate) fn migrate_exclusive(conn: &Connection) -> Result<(), DbError> {
    conn.execute_batch("BEGIN EXCLUSIVE")?;
    match super::migrations::migrate_database(conn) {
        Ok(()) => {
            conn.execute_batch("COMMIT")?;
            Ok(())
        }
        Err(error) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

/// Whether one process runs the background jobs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerRole {
    /// Holds the lease: runs the send queue and the cron.
    Holder,
    /// Another process holds it: this one only reads and writes data.
    Follower,
}

/// The worker lease: an exclusive lock on `bir_data.db.worker.lock`, held for
/// as long as this process runs the send queue and the cron. Dropping it (or
/// the process exiting or crashing) lets a follower take over.
pub struct WorkerLease {
    _file: File,
    path: PathBuf,
}

impl WorkerLease {
    /// Take the lease if no other process holds it. `Ok(None)`: follower.
    pub fn try_acquire(db_path: &Path) -> Result<Option<WorkerLease>, DbError> {
        let path = worker_lock_path(db_path);
        let file = open_sidecar(&path)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(WorkerLease { _file: file, path })),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(error)) => Err(DbError::Io(error)),
        }
    }

    /// Take the lease, following until the holder quits: tries now, then
    /// every `retry`. `None` once `stop` answers true (the process is
    /// quitting). Logs each role change.
    pub fn acquire_or_follow(
        db_path: &Path,
        retry: Duration,
        stop: impl Fn() -> bool,
    ) -> Option<WorkerLease> {
        let mut followed = false;
        loop {
            if stop() {
                return None;
            }
            match Self::try_acquire(db_path) {
                Ok(Some(lease)) => {
                    if followed {
                        info!(lock = %lease.path.display(), "worker lease: the holder quit; this process takes over the background jobs");
                    } else {
                        info!(lock = %lease.path.display(), "worker lease: holder; this process runs the background jobs");
                    }
                    return Some(lease);
                }
                Ok(None) => {
                    if !followed {
                        info!(
                            "worker lease: follower; another process runs the background jobs (retrying every {} s)",
                            retry.as_secs()
                        );
                        followed = true;
                    }
                }
                Err(error) => {
                    if !followed {
                        warn!(%error, "worker lease could not be taken; following");
                        followed = true;
                    }
                }
            }
            let until = Instant::now() + retry;
            while Instant::now() < until {
                if stop() {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(250).min(retry));
            }
        }
    }

    pub fn role(lease: &Option<WorkerLease>) -> WorkerRole {
        if lease.is_some() {
            WorkerRole::Holder
        } else {
            WorkerRole::Follower
        }
    }
}

impl Drop for WorkerLease {
    fn drop(&mut self) {
        info!(lock = %self.path.display(), "worker lease released");
    }
}

/// Notices commits by other connections, through `PRAGMA data_version`
/// (cheap: no table is read). The value does not move for this
/// connection's own writes, so a process that keeps one connection sees
/// only other processes' changes.
#[derive(Debug)]
pub struct ChangeWatch {
    last: Option<i32>,
}

impl ChangeWatch {
    pub fn new(db: &Database) -> Self {
        Self {
            last: db.data_version(),
        }
    }

    /// `true` when another connection committed since the last poll.
    pub fn poll(&mut self, db: &Database) -> bool {
        let now = db.data_version();
        let changed = matches!((self.last, now), (Some(before), Some(after)) if before != after);
        if now.is_some() {
            self.last = now;
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maintenance_lock_is_reentrant_on_one_thread() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("bir_data.db");
        let inner = with_maintenance_lock(&db, || {
            with_maintenance_lock_within(&db, Duration::from_millis(50), || Ok(7))
        })
        .unwrap();
        assert_eq!(inner, 7);
        assert!(maintenance_lock_path(&db).exists());
    }

    #[test]
    fn maintenance_lock_times_out_while_another_holder_keeps_it() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("bir_data.db");
        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let holder = {
            let db = db.clone();
            std::thread::spawn(move || {
                with_maintenance_lock(&db, || {
                    held_tx.send(()).unwrap();
                    let _ = done_rx.recv();
                    Ok(())
                })
                .unwrap();
            })
        };
        held_rx.recv().unwrap();
        let error = with_maintenance_lock_within(&db, Duration::from_millis(100), || Ok(()))
            .expect_err("a second holder must wait, then give up");
        assert!(
            error.to_string().contains("preparing the database"),
            "{error}"
        );
        done_tx.send(()).unwrap();
        holder.join().unwrap();
        with_maintenance_lock_within(&db, Duration::from_millis(100), || Ok(())).unwrap();
    }

    #[test]
    fn worker_lease_has_one_holder_until_it_drops() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("bir_data.db");
        let first = WorkerLease::try_acquire(&db).unwrap();
        assert_eq!(WorkerLease::role(&first), WorkerRole::Holder);
        let second = WorkerLease::try_acquire(&db).unwrap();
        assert_eq!(WorkerLease::role(&second), WorkerRole::Follower);
        drop(first);
        let third = WorkerLease::acquire_or_follow(&db, Duration::from_millis(20), || false);
        assert!(third.is_some());
    }

    #[test]
    fn acquire_or_follow_stops_when_asked() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("bir_data.db");
        let _holder = WorkerLease::try_acquire(&db).unwrap().unwrap();
        let stop = std::sync::atomic::AtomicBool::new(false);
        let started = Instant::now();
        std::thread::scope(|scope| {
            let follower = scope.spawn(|| {
                WorkerLease::acquire_or_follow(&db, Duration::from_secs(30), || {
                    stop.load(std::sync::atomic::Ordering::SeqCst)
                })
            });
            std::thread::sleep(Duration::from_millis(100));
            stop.store(true, std::sync::atomic::Ordering::SeqCst);
            assert!(follower.join().unwrap().is_none());
        });
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn sole_access_is_refused_while_the_file_is_open() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("bir_data.db");
        let open = hold_open_lock(&db).unwrap();
        let error = with_sole_access(&db, || Ok(())).expect_err("refused while open");
        assert!(error.to_string().contains("open"), "{error}");
        drop(open);
        with_sole_access(&db, || Ok(())).unwrap();
    }

    #[test]
    fn a_busy_error_reads_as_another_app_saving() {
        let busy = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
            None,
        );
        assert_eq!(describe_sqlite(&busy), BUSY_MESSAGE);
        let error = DbError::from(busy);
        assert!(error.is_busy());
        assert!(error.to_string().contains(BUSY_MESSAGE), "{error}");
    }
}
