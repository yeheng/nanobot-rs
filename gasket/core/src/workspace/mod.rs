//! Workspace management module
//!
//! Provides functionality for downloading and managing workspace templates
//! from GitHub repository.

mod downloader;

pub use downloader::{DownloadResult, WorkspaceDownloader};
