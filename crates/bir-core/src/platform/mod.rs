//! Platform-specific implementations for OS services.
//!
//! This module hides all `#[cfg(target_os)]` gating behind a clean public API.
//! Consumers call `crate::platform::data_dir()`, `crate::platform::run_shell_command()`,
//! etc. without knowing which OS they are on.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::*;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;
