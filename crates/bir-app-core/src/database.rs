//! The process database handle the ported functions reach through
//! [`with_database`], and the JSON conversions they share.
//!
//! The old shell kept the handle in a UI-thread local behind an account
//! gate. Here [`crate::BirApp`] installs it once when it opens the database,
//! and any thread may use it: the handle is an `Arc<Mutex<Database>>`, so
//! calls serialize on the lock. There is no account gate.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use bir_core::db::Database;

use crate::host::{HostError, HostValue};

pub(crate) type DatabaseHandle = Arc<Mutex<Database>>;

static DATABASE: RwLock<Option<(DatabaseHandle, PathBuf)>> = RwLock::new(None);

/// Make `handle` (the database file at `path`) the database every ported
/// function uses.
pub(crate) fn install(handle: DatabaseHandle, path: PathBuf) {
    let mut slot = DATABASE
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *slot = Some((handle, path));
}

/// The installed database's file, else the app's default location.
pub(crate) fn database_path() -> PathBuf {
    DATABASE
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .map(|(_, path)| path.clone())
        .unwrap_or_else(bir_core::db::app_database_path)
}

/// The installed handle, or an error when no `BirApp` has opened one.
pub(crate) fn database_handle() -> Result<DatabaseHandle, HostError> {
    DATABASE
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .map(|(handle, _)| handle.clone())
        .ok_or_else(|| HostError::new("the database is not open"))
}

/// Run `f` on the locked process database.
pub(crate) fn with_database<R>(
    f: impl FnOnce(&Database) -> Result<R, HostError>,
) -> Result<R, HostError> {
    let handle = database_handle()?;
    let guard = handle
        .lock()
        .map_err(|_| HostError::new("database lock poisoned"))?;
    f(&guard)
}

/// Tell the submission worker the queue changed, so a queued return goes
/// out now rather than on the worker's next one-minute tick.
pub(crate) fn wake_submission_worker() {
    bir_core::background_cron::wake();
}

/// `serde_json::Value` -> `HostValue`.
pub(crate) fn json(value: serde_json::Value) -> HostValue {
    match value {
        serde_json::Value::Null => HostValue::Null,
        serde_json::Value::Bool(v) => HostValue::Bool(v),
        serde_json::Value::Number(v) => HostValue::Number(v.as_f64().unwrap_or(0.0)),
        serde_json::Value::String(v) => HostValue::Str(v),
        serde_json::Value::Array(v) => HostValue::Array(v.into_iter().map(json).collect()),
        serde_json::Value::Object(v) => HostValue::Object(
            v.into_iter()
                .map(|(key, value)| (key, json(value)))
                .collect(),
        ),
    }
}

pub(crate) fn serialize<T: serde::Serialize>(value: &T) -> Result<HostValue, HostError> {
    serde_json::to_value(value)
        .map(json)
        .map_err(|error| HostError::new(error.to_string()))
}

/// `HostValue` -> `serde_json::Value`: the inverse of [`json`]. An integral
/// number comes back as a JSON integer, so typed drafts' integer fields read.
pub(crate) fn to_json(value: &HostValue) -> serde_json::Value {
    match value {
        HostValue::Null => serde_json::Value::Null,
        HostValue::Bool(v) => serde_json::Value::Bool(*v),
        HostValue::Number(v) => {
            if v.fract() == 0.0 && *v >= i64::MIN as f64 && *v <= i64::MAX as f64 {
                serde_json::json!(*v as i64)
            } else {
                serde_json::json!(v)
            }
        }
        HostValue::Str(v) => serde_json::Value::String(v.clone()),
        HostValue::Array(v) => serde_json::Value::Array(v.iter().map(to_json).collect()),
        HostValue::Object(v) => serde_json::Value::Object(
            v.iter()
                .map(|(key, value)| (key.clone(), to_json(value)))
                .collect(),
        ),
    }
}
