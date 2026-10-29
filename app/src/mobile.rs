//! Phone setup: where the database lives, and whether the phone layout
//! applies.
//!
//! [`prepare`] runs once, before the root mounts (the iOS `main`, the
//! Android library's `runRust`), and sets `BIR_DATABASE_PATH` to the app's
//! private data directory unless the launcher already set it (a test run
//! with a scratch database). bir-core keeps the key file fallback
//! (`bir_key.txt`) and its logs next to the database.

use std::path::PathBuf;

use nsh_core::element::Idiom;
use nsh_core::prelude::*;

/// The phone layout (a stack of pages, the form's floating bar) instead of
/// the desktop's three columns: a phone, iOS or Android.
pub(crate) fn phone() -> bool {
    !is_desktop() && idiom() == Idiom::Phone
}

/// Sets `BIR_DATABASE_PATH` (and on Android `HOME` and `TMPDIR`, which
/// the system leaves pointing nowhere the app may write) for this process.
pub fn prepare() {
    let Some(dir) = data_dir() else { return };
    let _ = std::fs::create_dir_all(&dir);
    let unset = |name: &str| std::env::var_os(name).is_none_or(|v| v.is_empty());
    // `launch.env` beside the database: `BUWIZ_*` launch hooks (and a
    // scratch `BIR_DATABASE_PATH` / `BIR_DB_KEY` for a test run), one
    // `NAME=value` a line, for a launcher that cannot pass the environment
    // (Android's `am start`; `adb shell run-as` writes it). Read once.
    if let Ok(text) = std::fs::read_to_string(dir.join("launch.env")) {
        for line in text.lines() {
            if let Some((name, value)) = line.trim().split_once('=')
                && (name.starts_with("BUWIZ_") || name == "BIR_DATABASE_PATH" || name == "BIR_DB_KEY")
                && unset(name)
            {
                // SAFETY: as below, before anything else reads the environment.
                unsafe { std::env::set_var(name, value) };
            }
        }
    }
    // SAFETY: runs before the engine starts, from the thread that starts
    // it; nothing else in the process reads the environment yet.
    unsafe {
        if unset("BIR_DATABASE_PATH") {
            std::env::set_var("BIR_DATABASE_PATH", dir.join("bir_data.db"));
        }
        #[cfg(target_os = "android")]
        {
            if unset("HOME") {
                std::env::set_var("HOME", &dir);
            }
            if let Some(cache) = dir.parent().and_then(|files| files.parent()).map(|app| app.join("cache")) {
                let _ = std::fs::create_dir_all(&cache);
                std::env::set_var("TMPDIR", cache);
            }
        }
    }
}

/// iOS: Application Support in the app's sandbox.
#[cfg(target_os = "ios")]
fn data_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/Application Support/Buwiz"))
}

/// Android: `files/buwiz` in the app's private directory, found from the
/// package name the process runs as.
#[cfg(target_os = "android")]
fn data_dir() -> Option<PathBuf> {
    let cmdline = std::fs::read("/proc/self/cmdline").ok()?;
    let package = String::from_utf8_lossy(cmdline.split(|b| *b == 0).next()?).into_owned();
    let package = package.split(':').next()?.to_owned();
    if package.is_empty() {
        return None;
    }
    Some(PathBuf::from("/data/data").join(package).join("files/buwiz"))
}

#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn data_dir() -> Option<PathBuf> {
    None
}

/// A phone's printing, given by the platform runner (the Android library
/// registers its JNI side here; iOS's lives in [`crate::ios_print`]). Every
/// call runs on a worker thread.
pub trait PhonePrint: Send + Sync {
    /// The system print dialog for the sheet `html` (it offers saving a
    /// PDF too).
    fn print(&self, html: &str, title: &str) -> Result<(), String>;
    /// The sheet as PDF bytes (bir-print's `PdfSink`).
    fn html_to_pdf(&self, html: &str) -> Result<Vec<u8>, String>;
    /// The share sheet for `pdf`, named after `title`.
    fn share_pdf(&self, pdf: &[u8], title: &str) -> Result<(), String>;
}

static PRINTER: std::sync::OnceLock<Box<dyn PhonePrint>> = std::sync::OnceLock::new();

/// Register the phone's printing (once, before the root mounts).
pub fn set_printer(printer: Box<dyn PhonePrint>) {
    let _ = PRINTER.set(printer);
}

/// The phone's printing, when the platform gave one.
pub(crate) fn printer() -> Option<&'static dyn PhonePrint> {
    #[cfg(target_os = "ios")]
    let _ = PRINTER.set(Box::new(crate::ios_print::IosPrint));
    PRINTER.get().map(|p| p.as_ref())
}
