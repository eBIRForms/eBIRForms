//! macOS-specific implementations for bir-core platform services.
#![allow(unexpected_cfgs)]

use std::path::PathBuf;

// ── Data Directory ───────────────────────────────────────────────────────────

/// Returns the macOS application data directory:
/// `~/Library/Application Support/com.buwiz.oss`, named for the bundle id so
/// no other app shares it (other tools already use `buwiz`, and the file
/// system ignores case).
pub fn data_dir() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        let dir = PathBuf::from(home).join("Library/Application Support/com.buwiz.oss");
        let _ = std::fs::create_dir_all(&dir);
        return dir;
    }

    // Fallback (should never happen on macOS)
    PathBuf::from(".buwiz")
}

/// Returns the temporary directory.
pub fn temp_dir() -> PathBuf {
    std::env::temp_dir()
}

// ── Shell Execution ──────────────────────────────────────────────────────────

/// Execute a shell command using the platform's native shell.
///
/// Absent from the Mac App Store build (`mas_build`): App Review rejects
/// arbitrary code execution and the sandbox would block it anyway.
#[cfg(not(feature = "mas_build"))]
pub async fn run_shell_command(cmd: &str) -> Result<std::process::Output, std::io::Error> {
    tokio::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .output()
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_data_dir_resolution() {
        // Use temp_env to safely set HOME without unsafe — also auto-restores on panic.
        temp_env::with_var("HOME", Some("/Users/testuser"), || {
            let path = data_dir();
            assert_eq!(path, PathBuf::from("/Users/testuser/Library/Application Support/com.buwiz.oss"));
        });
    }
}
