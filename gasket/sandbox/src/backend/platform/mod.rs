//! Platform-specific sandbox implementations
//!
//! This module re-exports platform-specific backends based on the target OS.

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
pub use linux::LinuxBwrapBackend;

#[cfg(target_os = "macos")]
pub use macos::MacOsSandboxBackend;

#[cfg(target_os = "windows")]
pub use windows::WindowsFallbackBackend;
