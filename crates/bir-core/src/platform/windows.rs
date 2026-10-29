//! Windows-specific implementations for bir-core platform services.

use std::path::PathBuf;

/// Prevents console windows from flashing on screen when spawning subprocesses.
const CREATE_NO_WINDOW: u32 = 0x08000000;

// ── Data Directory ───────────────────────────────────────────────────────────

/// Returns the Windows application data directory.
///
/// Uses `%LOCALAPPDATA%\Taxman\eBIRForms` following Windows conventions.
pub fn data_dir() -> PathBuf {
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        let dir = PathBuf::from(local_app_data)
            .join("Taxman")
            .join("eBIRForms");
        let _ = std::fs::create_dir_all(&dir);
        return dir;
    }

    // Fallback
    if let Some(home) = std::env::var_os("USERPROFILE") {
        let dir = PathBuf::from(home).join(".taxman-ebir");
        let _ = std::fs::create_dir_all(&dir);
        return dir;
    }

    PathBuf::from(".taxman-ebir")
}

/// Returns the temporary directory.
pub fn temp_dir() -> PathBuf {
    std::env::temp_dir()
}

// ── Shell Execution ──────────────────────────────────────────────────────────

/// Execute a shell command using the platform's native shell.
pub async fn run_shell_command(cmd: &str) -> Result<std::process::Output, std::io::Error> {
    tokio::process::Command::new("cmd")
        .creation_flags(CREATE_NO_WINDOW)
        .arg("/c")
        .arg(cmd)
        .output()
        .await
}
