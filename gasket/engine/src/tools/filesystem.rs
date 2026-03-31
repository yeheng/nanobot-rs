//! File system tools

use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::fs;
use tracing::{debug, instrument};

use super::base::{simple_schema, ToolContext};
use super::{Tool, ToolError, ToolResult};

/// Secure path validator that canonicalizes allowed directory at initialization
/// to prevent symlink attacks and path traversal vulnerabilities.
#[derive(Clone)]
pub struct PathValidator {
    /// Canonicalized allowed directory (all symlinks resolved)
    pub allowed_dir: Option<PathBuf>,
}

impl PathValidator {
    /// Create a new path validator with the given allowed directory.
    pub fn new(allowed_dir: Option<PathBuf>) -> Self {
        let canonical_allowed = allowed_dir.as_ref().and_then(|p| p.canonicalize().ok());
        Self {
            allowed_dir: canonical_allowed,
        }
    }

    /// Validate that a path is within the allowed directory.
    pub fn validate(&self, path: &str) -> Result<PathBuf, ToolError> {
        let path = PathBuf::from(path);

        if let Some(allowed) = &self.allowed_dir {
            if !path.exists() {
                return Err(ToolError::NotFound(format!(
                    "Path not found: {}",
                    path.display()
                )));
            }

            let canonical = path.canonicalize().map_err(|e| {
                ToolError::NotFound(format!("Cannot resolve path: {} - {}", path.display(), e))
            })?;

            if !canonical.starts_with(allowed) {
                return Err(ToolError::PermissionDenied(format!(
                    "Path outside workspace: {}",
                    path.display()
                )));
            }
        }

        Ok(path)
    }

    /// Validate a path for write operations (file may not exist yet).
    pub fn validate_for_write(&self, path: &str) -> Result<PathBuf, ToolError> {
        let path = PathBuf::from(path);

        if let Some(allowed) = &self.allowed_dir {
            let parent = path.parent().unwrap_or(&path);

            if parent.exists() {
                let canonical_parent = parent.canonicalize().map_err(|e| {
                    ToolError::NotFound(format!(
                        "Cannot resolve parent path: {} - {}",
                        parent.display(),
                        e
                    ))
                })?;

                if !canonical_parent.starts_with(allowed) {
                    return Err(ToolError::PermissionDenied(format!(
                        "Path outside workspace: {}",
                        path.display()
                    )));
                }
            } else {
                return Err(ToolError::NotFound(format!(
                    "Parent directory not found: {}",
                    parent.display()
                )));
            }
        }

        Ok(path)
    }
}

/// Read file tool
pub struct ReadFileTool {
    validator: PathValidator,
}

impl ReadFileTool {
    pub fn new(allowed_dir: Option<PathBuf>) -> Self {
        Self {
            validator: PathValidator::new(allowed_dir),
        }
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read the contents of a file from the filesystem"
    }

    fn parameters(&self) -> Value {
        simple_schema(&[
            (
                "absolute_path",
                "string",
                true,
                "Absolute path to the file to read",
            ),
            (
                "offset",
                "number",
                false,
                "Line offset to start reading from (0-based)",
            ),
            ("limit", "number", false, "Maximum number of lines to read"),
        ])
    }

    #[instrument(name = "tool.read_file", skip_all)]
    async fn execute(&self, args: Value, _ctx: &ToolContext) -> ToolResult {
        #[derive(Deserialize)]
        struct Args {
            absolute_path: String,
            #[serde(default)]
            offset: Option<usize>,
            #[serde(default)]
            limit: Option<usize>,
        }

        let args: Args =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        let path = self.validator.validate(&args.absolute_path)?;
        debug!("Reading file: {:?}", path);

        let content = fs::read_to_string(&path).await.map_err(|e| {
            ToolError::ExecutionError(format!("Failed to read file '{}': {}", path.display(), e))
        })?;

        // Handle offset and limit
        let result = if args.offset.is_some() || args.limit.is_some() {
            let lines: Vec<&str> = content.lines().collect();
            let offset = args.offset.unwrap_or(0);
            let limit = args.limit.unwrap_or(lines.len());

            lines
                .iter()
                .skip(offset)
                .take(limit)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            content
        };

        Ok(result)
    }
}

/// Write file tool
#[allow(dead_code)]
pub struct WriteFileTool {
    validator: PathValidator,
}

impl WriteFileTool {
    pub fn new(allowed_dir: Option<PathBuf>) -> Self {
        Self {
            validator: PathValidator::new(allowed_dir),
        }
    }
}

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Write content to a file, creating it if it doesn't exist"
    }

    fn parameters(&self) -> Value {
        simple_schema(&[
            ("file_path", "string", true, "Path to the file to write"),
            ("content", "string", true, "Content to write to the file"),
        ])
    }

    #[instrument(name = "tool.write_file", skip_all)]
    async fn execute(&self, args: Value, _ctx: &ToolContext) -> ToolResult {
        #[derive(Deserialize)]
        struct Args {
            file_path: String,
            content: String,
        }

        let args: Args =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        let path = self.validator.validate_for_write(&args.file_path)?;
        debug!("Writing file: {:?}", path);

        // Create parent directories if needed
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await.map_err(|e| {
                ToolError::ExecutionError(format!(
                    "Failed to create directories for '{}': {}",
                    parent.display(),
                    e
                ))
            })?;
        }

        fs::write(&path, &args.content).await.map_err(|e| {
            ToolError::ExecutionError(format!("Failed to write file '{}': {}", path.display(), e))
        })?;

        Ok(format!(
            "Successfully wrote {} bytes to {}",
            args.content.len(),
            args.file_path
        ))
    }
}

/// Edit file tool (string replacement)
#[allow(dead_code)]
pub struct EditFileTool {
    validator: PathValidator,
}

impl EditFileTool {
    pub fn new(allowed_dir: Option<PathBuf>) -> Self {
        Self {
            validator: PathValidator::new(allowed_dir),
        }
    }
}

#[async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn description(&self) -> &str {
        "Edit a file by replacing a specific string with new content"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file to edit"
                },
                "old_string": {
                    "type": "string",
                    "description": "The exact text to replace (must be unique in the file)"
                },
                "new_string": {
                    "type": "string",
                    "description": "The text to replace with"
                },
                "instruction": {
                    "type": "string",
                    "description": "Brief description of the change"
                }
            },
            "required": ["file_path", "old_string", "new_string", "instruction"]
        })
    }

    #[instrument(name = "tool.edit_file", skip_all)]
    async fn execute(&self, args: Value, _ctx: &ToolContext) -> ToolResult {
        #[derive(Deserialize)]
        struct Args {
            file_path: String,
            old_string: String,
            new_string: String,
            instruction: String,
        }

        let args: Args =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        let path = self.validator.validate(&args.file_path)?;
        debug!("Editing file: {:?} - {}", path, args.instruction);

        let content = fs::read_to_string(&path).await.map_err(|e| {
            ToolError::ExecutionError(format!(
                "Failed to read file '{}' for editing: {}",
                path.display(),
                e
            ))
        })?;

        // Check uniqueness
        let count = content.matches(&args.old_string).count();
        if count == 0 {
            return Err(ToolError::ExecutionError(
                "old_string not found in file".to_string(),
            ));
        }
        if count > 1 {
            return Err(ToolError::ExecutionError(format!(
                "old_string appears {} times - must be unique",
                count
            )));
        }

        let new_content = content.replace(&args.old_string, &args.new_string);

        fs::write(&path, new_content).await.map_err(|e| {
            ToolError::ExecutionError(format!(
                "Failed to write edited file '{}': {}",
                path.display(),
                e
            ))
        })?;

        Ok(format!("Successfully edited {}", args.file_path))
    }
}

/// List directory tool
#[allow(dead_code)]
pub struct ListDirTool {
    validator: PathValidator,
}

impl ListDirTool {
    pub fn new(allowed_dir: Option<PathBuf>) -> Self {
        Self {
            validator: PathValidator::new(allowed_dir),
        }
    }
}

#[async_trait]
impl Tool for ListDirTool {
    fn name(&self) -> &str {
        "list_directory"
    }

    fn description(&self) -> &str {
        "List the contents of a directory"
    }

    fn parameters(&self) -> Value {
        simple_schema(&[(
            "path",
            "string",
            true,
            "Absolute path to the directory to list",
        )])
    }

    #[instrument(name = "tool.list_dir", skip_all)]
    async fn execute(&self, args: Value, _ctx: &ToolContext) -> ToolResult {
        #[derive(Deserialize)]
        struct Args {
            path: String,
        }

        let args: Args =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        let path = self.validator.validate(&args.path)?;
        debug!("Listing directory: {:?}", path);

        let mut entries = fs::read_dir(&path).await.map_err(|e| {
            ToolError::ExecutionError(format!(
                "Failed to read directory '{}': {}",
                path.display(),
                e
            ))
        })?;

        let mut result = String::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            let file_type = entry
                .file_type()
                .await
                .map_err(|e| ToolError::ExecutionError(e.to_string()))?;

            if file_type.is_dir() {
                result.push_str(&format!("[DIR]  {}\n", name));
            } else {
                result.push_str(&format!("       {}\n", name));
            }
        }

        Ok(result)
    }
}

// Implement Default for tools
impl Default for ReadFileTool {
    fn default() -> Self {
        Self::new(None)
    }
}

impl Default for WriteFileTool {
    fn default() -> Self {
        Self::new(None)
    }
}

impl Default for EditFileTool {
    fn default() -> Self {
        Self::new(None)
    }
}

impl Default for ListDirTool {
    fn default() -> Self {
        Self::new(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_read_file_tool_name() {
        let tool = ReadFileTool::new(None);
        assert_eq!(tool.name(), "read_file");
    }

    #[tokio::test]
    async fn test_write_file_tool() {
        let tool = WriteFileTool::new(None);
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join("gasket_test_write.txt");

        let args = serde_json::json!({
            "file_path": test_file.to_str().unwrap(),
            "content": "Hello, World!"
        });

        let result = tool.execute(args, &ToolContext::default()).await;
        assert!(result.is_ok());

        // Verify file was written
        let content = fs::read_to_string(&test_file).await.unwrap();
        assert_eq!(content, "Hello, World!");

        // Cleanup
        let _ = fs::remove_file(&test_file).await;
    }

    #[tokio::test]
    async fn test_edit_file_tool() {
        let tool = EditFileTool::new(None);
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join("gasket_test_edit.txt");

        // Create initial file
        fs::write(&test_file, "Hello, World!").await.unwrap();

        let args = serde_json::json!({
            "file_path": test_file.to_str().unwrap(),
            "old_string": "World",
            "new_string": "Rust",
            "instruction": "Replace World with Rust"
        });

        let result = tool.execute(args, &ToolContext::default()).await;
        assert!(result.is_ok());

        // Verify edit
        let content = fs::read_to_string(&test_file).await.unwrap();
        assert_eq!(content, "Hello, Rust!");

        // Cleanup
        let _ = fs::remove_file(&test_file).await;
    }

    #[tokio::test]
    async fn test_edit_file_not_found() {
        let tool = EditFileTool::new(None);
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join("gasket_test_not_exist.txt");

        let _ = fs::remove_file(&test_file).await;

        let args = serde_json::json!({
            "file_path": test_file.to_str().unwrap(),
            "old_string": "old",
            "new_string": "new",
            "instruction": "test"
        });

        let result = tool.execute(args, &ToolContext::default()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_list_directory_tool() {
        let tool = ListDirTool::new(None);
        let temp_dir = std::env::temp_dir();

        let args = serde_json::json!({
            "path": temp_dir.to_str().unwrap()
        });

        let result = tool.execute(args, &ToolContext::default()).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_read_file_with_offset_limit() {
        let tool = ReadFileTool::new(None);
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join("gasket_test_read_offset.txt");

        // Create test file
        let mut file = fs::File::create(&test_file).await.unwrap();
        for i in 0..10 {
            use tokio::io::AsyncWriteExt;
            file.write_all(format!("Line {}\n", i).as_bytes())
                .await
                .unwrap();
            file.flush().await.unwrap();
        }

        let args = serde_json::json!({
            "absolute_path": test_file.to_str().unwrap(),
            "offset": 2,
            "limit": 3
        });

        let result = tool.execute(args, &ToolContext::default()).await.unwrap();
        assert!(result.contains("Line 2"));
        assert!(result.contains("Line 3"));
        assert!(result.contains("Line 4"));
        assert!(!result.contains("Line 5"));

        // Cleanup
        let _ = fs::remove_file(&test_file).await;
    }

    // === Security Tests ===

    mod security_tests {
        use super::*;
        use std::os::unix::fs::symlink;
        use tempfile::TempDir;

        #[tokio::test]
        async fn test_symlink_escape_attack() {
            let temp_workspace = TempDir::new().unwrap();
            let temp_external = TempDir::new().unwrap();
            let external_file = temp_external.path().join("secret.txt");
            tokio::fs::write(&external_file, "secret data")
                .await
                .unwrap();

            let symlink_path = temp_workspace.path().join("malicious_link");
            symlink(&external_file, &symlink_path).unwrap();

            let tool = ReadFileTool::new(Some(temp_workspace.path().to_path_buf()));
            let args = serde_json::json!({
                "absolute_path": symlink_path.to_str().unwrap()
            });

            let result = tool.execute(args, &ToolContext::default()).await;
            assert!(
                result.is_err(),
                "Should reject symlink pointing outside workspace"
            );
        }

        #[tokio::test]
        async fn test_path_traversal_attack() {
            let temp_workspace = TempDir::new().unwrap();
            let temp_external = TempDir::new().unwrap();

            let external_file = temp_external.path().join("secret.txt");
            tokio::fs::write(&external_file, "secret data")
                .await
                .unwrap();

            let workspace_canonical = temp_workspace.path().canonicalize().unwrap();
            let external_canonical = external_file.canonicalize().unwrap();

            let tool = ReadFileTool::new(Some(workspace_canonical));
            let args = serde_json::json!({
                "absolute_path": external_canonical.to_str().unwrap()
            });

            let result = tool.execute(args, &ToolContext::default()).await;
            assert!(result.is_err(), "Should reject path outside workspace");
        }

        #[tokio::test]
        async fn test_legitimate_path_in_workspace() {
            let temp_workspace = TempDir::new().unwrap();
            let test_file = temp_workspace.path().join("test.txt");
            tokio::fs::write(&test_file, "legitimate content")
                .await
                .unwrap();

            let tool = ReadFileTool::new(Some(temp_workspace.path().to_path_buf()));
            let args = serde_json::json!({
                "absolute_path": test_file.to_str().unwrap()
            });

            let result = tool.execute(args, &ToolContext::default()).await;
            assert!(result.is_ok(), "Should allow legitimate path");
            assert_eq!(result.unwrap(), "legitimate content");
        }
    }
}
