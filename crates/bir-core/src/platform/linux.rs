//! Linux-specific implementations for bir-core platform services.

use std::path::PathBuf;

// ── Data Directory ───────────────────────────────────────────────────────────

/// Returns the Linux application data directory.
///
/// Uses `$HOME/.taxman-ebir` following XDG-compatible conventions.
pub fn data_dir() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
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
    tokio::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .output()
        .await
}
