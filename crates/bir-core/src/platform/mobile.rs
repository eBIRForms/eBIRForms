//! iOS and Android implementations for bir-core platform services.
//!
//! A phone app has no home directory of its own to pick from: the app
//! knows where its private data lives and says so before it opens the
//! database, through `BUWIZ_DATABASE_PATH` (the database file) or
//! `BIR_DATA_DIR` (the directory). The key file fallback (`bir_key.txt`)
//! and the logs go next to the database.

use std::path::PathBuf;

/// The app's private data directory: `BIR_DATA_DIR`, else the directory of
/// `BUWIZ_DATABASE_PATH`, else `$HOME/Library/Application Support/Buwiz`
/// (the iOS sandbox), else the working directory.
pub fn data_dir() -> PathBuf {
    let set = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    let dir = set("BIR_DATA_DIR")
        .or_else(|| set("BUWIZ_DATABASE_PATH").and_then(|db| db.parent().map(PathBuf::from)))
        .or_else(|| set("HOME").map(|home| home.join("Library/Application Support/Buwiz")))
        .unwrap_or_else(|| PathBuf::from("."));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Returns the temporary directory.
pub fn temp_dir() -> PathBuf {
    std::env::temp_dir()
}
