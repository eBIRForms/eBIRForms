//! Linux-specific implementations for bir-core platform services.

use std::path::PathBuf;

// ── Data Directory ───────────────────────────────────────────────────────────

/// Returns the Linux application data directory.
///
/// Uses `$XDG_DATA_HOME/buwiz`, else `$HOME/.local/share/buwiz`.
pub fn data_dir() -> PathBuf {
    let set = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(dir) = set("XDG_DATA_HOME")
        .or_else(|| set("HOME").map(|home| home.join(".local/share")))
        .map(|base| base.join("buwiz"))
    {
        let _ = std::fs::create_dir_all(&dir);
        return dir;
    }

    PathBuf::from(".buwiz")
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
