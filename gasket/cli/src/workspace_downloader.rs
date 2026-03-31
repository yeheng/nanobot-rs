//! Workspace template downloader from GitHub
//!
//! Downloads the `workspace/` directory from the gasket repository
//! and extracts it to the user's `~/.gasket/` directory.

use std::io::Read;
use std::path::PathBuf;

use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use reqwest::Client;
use tracing::{debug, info};

/// GitHub repository information
const GITHUB_REPO: &str = "yeheng/gasket";
const GITHUB_BRANCH: &str = "main";

/// Result of the download operation
#[derive(Debug, Default)]
pub struct DownloadResult {
    /// Files that were successfully created
    pub created_files: Vec<String>,
    /// Files that were skipped (already exist)
    pub skipped_files: Vec<String>,
    /// Directories that were created
    pub created_dirs: Vec<String>,
}

/// Workspace template downloader
pub struct WorkspaceDownloader {
    client: Client,
    target_dir: PathBuf,
    overwrite_existing: bool,
}

impl WorkspaceDownloader {
    /// Create a new downloader with default target directory (~/.gasket)
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            target_dir: gasket_engine::config::config_dir(),
            overwrite_existing: false,
        }
    }

    /// Download and extract workspace templates from GitHub
    ///
    /// Downloads the repository tarball, extracts the `workspace/` directory,
    /// and copies files to the target directory.
    pub async fn download(&self) -> Result<DownloadResult> {
        let url = format!(
            "https://codeload.github.com/{}/tar.gz/refs/heads/{}",
            GITHUB_REPO, GITHUB_BRANCH
        );

        info!("Downloading workspace templates from {}", url);

        // Download tarball
        let response = self
            .client
            .get(&url)
            .header("User-Agent", "gasket-workspace-downloader/1.0")
            .send()
            .await
            .context("Failed to download workspace templates from GitHub")?;

        if !response.status().is_success() {
            anyhow::bail!(
                "GitHub returned HTTP {} when downloading workspace templates",
                response.status()
            );
        }

        let bytes = response
            .bytes()
            .await
            .context("Failed to read response body")?;

        debug!("Downloaded {} bytes", bytes.len());

        // Extract tarball
        self.extract_workspace(&bytes)
    }

    /// Extract the workspace directory from the tarball
    fn extract_workspace(&self, tarball: &[u8]) -> Result<DownloadResult> {
        let mut result = DownloadResult::default();

        // Create target directory if needed
        std::fs::create_dir_all(&self.target_dir)
            .with_context(|| format!("Failed to create directory: {:?}", self.target_dir))?;

        let decoder = GzDecoder::new(tarball);
        let mut archive = tar::Archive::new(decoder);

        // The tarball prefix: e.g., "gasket-rs-main/"
        let prefix = format!(
            "{}-{}/workspace/",
            GITHUB_REPO.split('/').next_back().unwrap_or("gasket-rs"),
            GITHUB_BRANCH
        );

        debug!("Looking for entries with prefix: {}", prefix);

        for entry in archive.entries()? {
            let mut entry = entry?;
            let path = entry.path()?.to_string_lossy().to_string();

            // Only process files in the workspace/ directory
            if let Some(relative) = path.strip_prefix(&prefix) {
                if relative.is_empty() {
                    continue; // Skip the workspace/ directory entry itself
                }

                let target_path = self.target_dir.join(relative);
                let relative_str = relative.to_string();

                // Check if it's a directory or file
                if path.ends_with('/') {
                    // Create directory
                    if !target_path.exists() {
                        std::fs::create_dir_all(&target_path).with_context(|| {
                            format!("Failed to create directory: {:?}", target_path)
                        })?;
                        // Remove trailing slash for display
                        let dir_name = relative_str.trim_end_matches('/');
                        result.created_dirs.push(dir_name.to_string());
                        debug!("Created directory: {:?}", target_path);
                    }
                } else {
                    // Ensure parent directory exists
                    if let Some(parent) = target_path.parent() {
                        if !parent.exists() {
                            std::fs::create_dir_all(parent).with_context(|| {
                                format!("Failed to create directory: {:?}", parent)
                            })?;
                        }
                    }

                    // Check if file already exists
                    if target_path.exists() && !self.overwrite_existing {
                        result.skipped_files.push(relative_str);
                        debug!("Skipped existing file: {:?}", target_path);
                        continue;
                    }

                    // Extract file
                    let mut content = Vec::new();
                    entry.read_to_end(&mut content)?;
                    std::fs::write(&target_path, &content)
                        .with_context(|| format!("Failed to write file: {:?}", target_path))?;

                    result.created_files.push(relative_str);
                    debug!("Created file: {:?}", target_path);
                }
            }
        }

        info!(
            "Workspace download complete: {} files created, {} files skipped",
            result.created_files.len(),
            result.skipped_files.len()
        );

        Ok(result)
    }
}

impl Default for WorkspaceDownloader {
    fn default() -> Self {
        Self::new()
    }
}
