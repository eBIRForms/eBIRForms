//! Application log files on disk: naming, today's file, retention pruning.
//!
//! Painted `bir` and `bir-shell` both write one file per UTC day through
//! `tracing_appender::rolling` (`ebirforms.YYYY-MM-DD.log`) and prune older
//! days on a user-set retention. Before rotation the appender was
//! `rolling::never`: a single `ebirforms.log` that nothing ever truncated,
//! which on a busy machine grows by tens of megabytes a year.

use std::path::{Path, PathBuf};

use chrono::NaiveDate;

use crate::db::Database;

pub const LOG_FILE_PREFIX: &str = "ebirforms";
pub const LOG_FILE_SUFFIX: &str = "log";
/// The pre-rotation single file. Adopted into the dated scheme on first run.
pub const LEGACY_LOG_FILE: &str = "ebirforms.log";

/// Settings key. Value is a day count as decimal text.
pub const RETENTION_SETTING: &str = "log_retention_days";
pub const DEFAULT_RETENTION_DAYS: u32 = 7;
pub const MIN_RETENTION_DAYS: u32 = 1;
pub const MAX_RETENTION_DAYS: u32 = 365;
/// Offered in Settings. Any value in range is accepted from the database.
pub const RETENTION_CHOICES: [u32; 5] = [3, 7, 14, 30, 90];

/// `<data_dir>/logs`, the directory both binaries write under.
///
/// On macOS `data_dir` is the App Group container
/// (`~/Library/Group Containers/group.dev.goldcoders.bir`), so the files land
/// in that container's `logs` folder. `BIR_LOG_DIR`, when set and non-empty,
/// replaces the directory — tests point it at a scratch folder.
pub fn log_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("BIR_LOG_DIR")
        && !dir.is_empty()
    {
        return PathBuf::from(dir);
    }
    crate::platform::data_dir().join("logs")
}

/// `ebirforms.2026-09-12.log` — the name `tracing_appender` gives a daily
/// file with this prefix and suffix. The date is UTC, as the appender's is.
pub fn dated_file_name(date: NaiveDate) -> String {
    format!(
        "{LOG_FILE_PREFIX}.{}.{LOG_FILE_SUFFIX}",
        date.format("%Y-%m-%d")
    )
}

/// The file the appender is writing right now.
pub fn today_log_path(dir: &Path) -> PathBuf {
    dir.join(dated_file_name(chrono::Utc::now().date_naive()))
}

/// The date encoded in a rotated file name, or `None` for anything else.
pub fn rotated_log_date(file_name: &str) -> Option<NaiveDate> {
    let middle = file_name
        .strip_prefix(LOG_FILE_PREFIX)?
        .strip_prefix('.')?
        .strip_suffix(LOG_FILE_SUFFIX)?
        .strip_suffix('.')?;
    // The appender always writes the padded form; chrono would also accept
    // `2026-9-1`, which is not a file we wrote.
    if middle.len() != "2026-09-12".len() {
        return None;
    }
    NaiveDate::parse_from_str(middle, "%Y-%m-%d").ok()
}

/// Every rotated file in `dir`, oldest first.
pub fn list_rotated_logs(dir: &Path) -> Vec<(NaiveDate, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<(NaiveDate, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            rotated_log_date(name).map(|date| (date, path.clone()))
        })
        .collect();
    files.sort();
    files
}

/// Fold the legacy single `ebirforms.log` into the dated scheme so it is
/// pruned like everything else. It is filed under its last-modified date
/// (UTC); if that day's file already exists the legacy text is prepended to
/// it, since the legacy file is older. Returns the file it went into.
pub fn adopt_legacy_log(dir: &Path) -> Option<PathBuf> {
    let legacy = dir.join(LEGACY_LOG_FILE);
    let metadata = std::fs::metadata(&legacy).ok()?;
    let modified: chrono::DateTime<chrono::Utc> = metadata.modified().ok()?.into();
    let target = dir.join(dated_file_name(modified.date_naive()));
    if target.exists() {
        let mut merged = std::fs::read_to_string(&legacy).ok()?;
        if !merged.ends_with('\n') {
            merged.push('\n');
        }
        merged.push_str(&std::fs::read_to_string(&target).ok()?);
        std::fs::write(&target, merged).ok()?;
        std::fs::remove_file(&legacy).ok()?;
    } else {
        std::fs::rename(&legacy, &target).ok()?;
    }
    Some(target)
}

/// Delete rotated files older than the retention window. `keep_days = 7`
/// keeps today and the six days before it. Returns what was removed.
pub fn prune_rotated_logs(dir: &Path, keep_days: u32, today: NaiveDate) -> Vec<PathBuf> {
    let keep_days = keep_days.clamp(MIN_RETENTION_DAYS, MAX_RETENTION_DAYS);
    let oldest_kept = today - chrono::Duration::days(i64::from(keep_days) - 1);
    let mut removed = Vec::new();
    for (date, path) in list_rotated_logs(dir) {
        if date < oldest_kept && std::fs::remove_file(&path).is_ok() {
            removed.push(path);
        }
    }
    removed
}

/// The user's retention setting, clamped, defaulting to a week.
pub fn retention_days(db: &Database) -> u32 {
    db.get_setting(RETENTION_SETTING)
        .ok()
        .flatten()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .map(|days| days.clamp(MIN_RETENTION_DAYS, MAX_RETENTION_DAYS))
        .unwrap_or(DEFAULT_RETENTION_DAYS)
}

/// Adopt the legacy file, then prune to the stored retention. One call at
/// startup and one a few times a day is enough; rotation itself is the
/// appender's job.
pub fn maintain(dir: &Path, db: &Database) -> Vec<PathBuf> {
    let _ = adopt_legacy_log(dir);
    prune_rotated_logs(dir, retention_days(db), chrono::Utc::now().date_naive())
}

/// Re-check retention every six hours so a long-running process crosses
/// midnight without growing past the setting. The caller owns `db` for the
/// life of the process; the thread sleeps, so only the real binaries should
/// start it (a test process would not exit).
pub fn spawn_retention(db: std::sync::Arc<std::sync::Mutex<Database>>) {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        let spawned = std::thread::Builder::new()
            .name("bir-log-retention".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(6 * 60 * 60));
                    if let Ok(guard) = db.lock() {
                        let _ = maintain(&log_dir(), &guard);
                    }
                }
            });
        if let Err(error) = spawned {
            eprintln!("log retention thread not started: {error}");
        }
    });
}

/// All kept days concatenated oldest → newest, for Export.
pub fn combined_log_text(dir: &Path) -> String {
    let mut out = String::new();
    for (_, path) in list_rotated_logs(dir) {
        if let Ok(text) = std::fs::read_to_string(&path) {
            out.push_str(&text);
            if !text.ends_with('\n') {
                out.push('\n');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn dated_names_round_trip() {
        let name = dated_file_name(day(2026, 9, 12));
        assert_eq!(name, "ebirforms.2026-09-12.log");
        assert_eq!(rotated_log_date(&name), Some(day(2026, 9, 12)));
        assert_eq!(rotated_log_date("ebirforms.log"), None);
        assert_eq!(rotated_log_date("bir-headless.log"), None);
        assert_eq!(rotated_log_date("ebirforms.2026-9-1.log"), None);
    }

    #[test]
    fn prune_keeps_today_and_the_window_before_it() {
        let dir = tempfile::tempdir().unwrap();
        for d in 1..=12 {
            std::fs::write(dir.path().join(dated_file_name(day(2026, 9, d))), "x\n").unwrap();
        }
        std::fs::write(dir.path().join("bir-headless.log"), "keep\n").unwrap();

        let removed = prune_rotated_logs(dir.path(), 7, day(2026, 9, 12));
        assert_eq!(removed.len(), 5, "{removed:?}");

        let kept: Vec<NaiveDate> = list_rotated_logs(dir.path())
            .into_iter()
            .map(|(date, _)| date)
            .collect();
        assert_eq!(kept.first(), Some(&day(2026, 9, 6)));
        assert_eq!(kept.last(), Some(&day(2026, 9, 12)));
        assert!(
            dir.path().join("bir-headless.log").exists(),
            "other files untouched"
        );
    }

    #[test]
    fn prune_clamps_absurd_retention() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(dated_file_name(day(2026, 9, 12))), "x\n").unwrap();
        assert!(prune_rotated_logs(dir.path(), 0, day(2026, 9, 12)).is_empty());
        assert!(dir.path().join(dated_file_name(day(2026, 9, 12))).exists());
    }

    #[test]
    fn legacy_file_is_adopted_under_its_modified_date() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join(LEGACY_LOG_FILE);
        std::fs::write(&legacy, "old line\n").unwrap();
        let target = adopt_legacy_log(dir.path()).expect("adopted");
        assert!(!legacy.exists());
        assert!(rotated_log_date(target.file_name().unwrap().to_str().unwrap()).is_some());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "old line\n");
        assert!(
            adopt_legacy_log(dir.path()).is_none(),
            "nothing left to adopt"
        );
    }

    #[test]
    fn legacy_text_goes_before_todays_when_the_day_file_exists() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join(LEGACY_LOG_FILE);
        std::fs::write(&legacy, "older").unwrap();
        let modified: chrono::DateTime<chrono::Utc> = std::fs::metadata(&legacy)
            .unwrap()
            .modified()
            .unwrap()
            .into();
        let today = dir.path().join(dated_file_name(modified.date_naive()));
        std::fs::write(&today, "newer\n").unwrap();
        adopt_legacy_log(dir.path()).unwrap();
        assert_eq!(std::fs::read_to_string(&today).unwrap(), "older\nnewer\n");
    }

    #[test]
    fn combined_text_is_oldest_first() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(dated_file_name(day(2026, 9, 12))), "b\n").unwrap();
        std::fs::write(dir.path().join(dated_file_name(day(2026, 9, 11))), "a").unwrap();
        assert_eq!(combined_log_text(dir.path()), "a\nb\n");
    }

    #[test]
    fn log_dir_honors_bir_log_dir() {
        let dir = tempfile::tempdir().unwrap();
        let expected = dir.path().join("custom-logs");
        temp_env::with_var("BIR_LOG_DIR", Some(expected.as_os_str()), || {
            assert_eq!(log_dir(), expected);
        });
        temp_env::with_var("BIR_LOG_DIR", Some(""), || {
            assert_ne!(log_dir(), PathBuf::from(""));
        });
    }

    #[test]
    fn retention_setting_is_parsed_clamped_and_defaulted() {
        let db = Database::open_ephemeral().unwrap();
        assert_eq!(retention_days(&db), DEFAULT_RETENTION_DAYS);
        db.set_setting(RETENTION_SETTING, "30").unwrap();
        assert_eq!(retention_days(&db), 30);
        db.set_setting(RETENTION_SETTING, "0").unwrap();
        assert_eq!(retention_days(&db), MIN_RETENTION_DAYS);
        db.set_setting(RETENTION_SETTING, "9999").unwrap();
        assert_eq!(retention_days(&db), MAX_RETENTION_DAYS);
        db.set_setting(RETENTION_SETTING, "soon").unwrap();
        assert_eq!(retention_days(&db), DEFAULT_RETENTION_DAYS);
    }
}
