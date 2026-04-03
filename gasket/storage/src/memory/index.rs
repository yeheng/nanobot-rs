//! Index management for _INDEX.md files.
//!
//! Provides functionality to generate and parse _INDEX.md files that summarize
//! all memories in a scenario directory. The index is organized by frequency
//! (Hot, Warm, Cold, Archived) and includes token counts and metadata.

use super::types::*;
use super::frontmatter::*;
use super::path::*;
use anyhow::{Context, Result};
use std::path::PathBuf;
use chrono::Utc;

/// A single entry parsed from _INDEX.md.
#[derive(Debug, Clone)]
pub struct MemoryIndexEntry {
    pub id: String,
    pub title: String,
    pub memory_type: String,
    pub tags: Vec<String>,
    pub frequency: Frequency,
    pub tokens: u32,
    pub filename: String,
    pub updated: String,
}

/// Parsed result of a _INDEX.md file.
#[derive(Debug, Clone)]
pub struct MemoryIndex {
    pub scenario: Scenario,
    pub updated: String,
    pub total_memories: usize,
    pub total_tokens: u32,
    pub entries: Vec<MemoryIndexEntry>,
    pub human_notes: String,
}

/// Index manager for _INDEX.md files.
pub struct FileIndexManager {
    base_dir: PathBuf,
}

impl FileIndexManager {
    pub fn new(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    pub fn default_path() -> Self {
        Self::new(memory_base_dir())
    }

    /// Regenerate _INDEX.md for a scenario.
    /// Scans all .md files, parses frontmatter, writes atomic index.
    /// Preserves any existing HUMAN_NOTES section.
    pub async fn regenerate(&self, scenario: Scenario) -> Result<()> {
        let dir = self.base_dir.join(scenario.dir_name());
        if !dir.exists() {
            tokio::fs::create_dir_all(&dir).await?;
        }

        // Read existing human notes
        let index_file = dir.join("_INDEX.md");
        let human_notes = if index_file.exists() {
            let content = tokio::fs::read_to_string(&index_file).await?;
            extract_human_notes(&content)
        } else {
            String::new()
        };

        // Scan all memory files
        let mut entries: Vec<MemoryIndexEntry> = Vec::new();
        let mut read_dir = tokio::fs::read_dir(&dir).await?;
        while let Some(entry) = read_dir.next_entry().await? {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.ends_with(".md") || name == "_INDEX.md" || name.starts_with('.') {
                continue;
            }
            let path = entry.path();
            match tokio::fs::read_to_string(&path).await {
                Ok(content) => {
                    if let Ok((meta, _)) = parse_memory_file(&content) {
                        entries.push(MemoryIndexEntry {
                            id: meta.id,
                            title: meta.title,
                            memory_type: meta.r#type,
                            tags: meta.tags,
                            frequency: meta.frequency,
                            tokens: meta.tokens as u32,
                            filename: name,
                            updated: meta.updated,
                        });
                    }
                }
                Err(e) => {
                    tracing::warn!("Skipping unparseable memory file {}: {}", name, e);
                }
            }
        }

        // Sort: hot first, then warm, then cold, then archived
        entries.sort_by(|a, b| b.frequency.cmp(&a.frequency));

        let total_tokens: u32 = entries.iter().map(|e| e.tokens).sum();
        let now = Utc::now().to_rfc3339();

        // Generate index content
        let mut output = String::new();
        let title = format!("{} Index", capitalize_first(scenario.to_string()));
        output.push_str(&format!("# {}\n", title));
        output.push_str(&format!("<!-- scenario: {} -->\n", scenario));
        output.push_str(&format!("<!-- updated: {} -->\n", now));
        output.push_str(&format!("<!-- total_memories: {} -->\n", entries.len()));
        output.push_str(&format!("<!-- total_tokens: ~{} -->\n\n", total_tokens));

        // Group by frequency
        for freq in &[Frequency::Hot, Frequency::Warm, Frequency::Cold, Frequency::Archived] {
            let freq_entries: Vec<_> = entries.iter().filter(|e| e.frequency == *freq).collect();
            if freq_entries.is_empty() && *freq == Frequency::Archived {
                continue; // skip empty archived section
            }
            let desc = match freq {
                Frequency::Hot => "Hot (always loaded when scenario is active)",
                Frequency::Warm => "Warm (loaded on topic match)",
                Frequency::Cold => "Cold (loaded only on explicit search)",
                Frequency::Archived => "Archived (historical only)",
            };
            output.push_str(&format!("## {}\n", desc));
            output.push_str("| ID | Title | Type | Tags | Tokens | Updated |\n");
            output.push_str("|----|-------|------|------|--------|---------|\n");
            for entry in freq_entries {
                let tags_str = entry.tags.join(",");
                let date = entry.updated.get(..10).unwrap_or(&entry.updated);
                output.push_str(&format!(
                    "| {} | {} | {} | {} | ~{} | {} |\n",
                    entry.id.get(..12).unwrap_or(&entry.id),
                    entry.title,
                    entry.memory_type,
                    tags_str,
                    entry.tokens,
                    date,
                ));
            }
            output.push('\n');
        }

        // Preserve human notes
        output.push_str("<!-- HUMAN_NOTES_START -->\n");
        if !human_notes.is_empty() {
            output.push_str(&human_notes);
            output.push('\n');
        }
        output.push_str("<!-- HUMAN_NOTES_END -->\n");

        // Atomic write: .tmp → fsync → rename
        let tmp_path = dir.join("_INDEX.md.tmp");
        tokio::fs::write(&tmp_path, &output).await?;
        // fsync
        let file = tokio::fs::File::open(&tmp_path).await?;
        file.sync_all().await?;
        drop(file);
        // Atomic rename
        tokio::fs::rename(&tmp_path, &index_file).await?;

        Ok(())
    }

    /// Parse _INDEX.md into structured entries.
    pub async fn read_index(&self, scenario: Scenario) -> Result<MemoryIndex> {
        let index_file = self.base_dir.join(scenario.dir_name()).join("_INDEX.md");
        let content = tokio::fs::read_to_string(&index_file)
            .await
            .with_context(|| format!("No index file for scenario: {}", scenario))?;

        parse_index_content(scenario, &content)
    }
}

/// Capitalize the first character of a string.
fn capitalize_first(s: String) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

/// Extract human notes section from index content.
fn extract_human_notes(content: &str) -> String {
    if let Some(start) = content.find("<!-- HUMAN_NOTES_START -->") {
        let after_start = &content[start + "<!-- HUMAN_NOTES_START -->".len()..];
        if let Some(end) = after_start.find("<!-- HUMAN_NOTES_END -->") {
            return after_start[..end].trim().to_string();
        }
    }
    String::new()
}

/// Parse index content into MemoryIndex.
fn parse_index_content(scenario: Scenario, content: &str) -> Result<MemoryIndex> {
    let mut updated = String::new();
    let mut total_memories = 0;
    let mut total_tokens = 0;

    // Parse header comments
    for line in content.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("<!-- updated: ") {
            if let Some(val) = rest.strip_suffix(" -->") {
                updated = val.trim().to_string();
            }
        }
        if let Some(rest) = line.strip_prefix("<!-- total_memories: ") {
            if let Some(val) = rest.strip_suffix(" -->") {
                total_memories = val.trim().parse().unwrap_or(0);
            }
        }
        if let Some(rest) = line.strip_prefix("<!-- total_tokens: ~") {
            if let Some(val) = rest.strip_suffix(" -->") {
                total_tokens = val.trim().parse().unwrap_or(0);
            }
        }
    }

    // Parse table rows and track section headers for frequency
    let mut entries = Vec::new();
    let mut current_freq = Frequency::Warm;

    for line in content.lines() {
        let line = line.trim();

        // Update current frequency based on section headers
        if line.starts_with("## Hot") {
            current_freq = Frequency::Hot;
            continue;
        } else if line.starts_with("## Warm") {
            current_freq = Frequency::Warm;
            continue;
        } else if line.starts_with("## Cold") {
            current_freq = Frequency::Cold;
            continue;
        } else if line.starts_with("## Archived") {
            current_freq = Frequency::Archived;
            continue;
        }

        // Parse table rows (lines starting with | and not separator lines)
        if !line.starts_with('|') || line.contains("----|") || line.contains("ID |") {
            continue;
        }

        let cols: Vec<&str> = line.split('|').filter(|c| !c.trim().is_empty()).collect();
        if cols.len() >= 5 {
            let tokens_str = cols.get(4).unwrap_or(&"~0").trim();
            let tokens = tokens_str.trim_start_matches('~').parse().unwrap_or(0);

            entries.push(MemoryIndexEntry {
                id: cols.first().unwrap_or(&"").trim().to_string(),
                title: cols.get(1).unwrap_or(&"").trim().to_string(),
                memory_type: cols.get(2).unwrap_or(&"").trim().to_string(),
                tags: cols
                    .get(3)
                    .unwrap_or(&"")
                    .trim()
                    .split(',')
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .collect(),
                frequency: current_freq,
                tokens,
                filename: String::new(), // not stored in index
                updated: cols.get(5).unwrap_or(&"").trim().to_string(),
            });
        }
    }

    let human_notes = extract_human_notes(content);

    Ok(MemoryIndex {
        scenario,
        updated,
        total_memories,
        total_tokens,
        entries,
        human_notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_store(temp_dir: &PathBuf) -> FileIndexManager {
        FileIndexManager::new(temp_dir.clone())
    }

    #[tokio::test]
    async fn test_regenerate_creates_index() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_test_store(&temp_dir.path().to_path_buf());

        // Create scenario directory
        let dir = temp_dir.path().join("knowledge");
        tokio::fs::create_dir_all(&dir).await.unwrap();

        // Create a test memory file
        let memory_content = r#"---
id: mem_test123
title: Test Memory
type: note
scenario: knowledge
tags:
  - test
frequency: hot
created: 2026-04-03T00:00:00Z
updated: 2026-04-03T00:00:00Z
last_accessed: 2026-04-03T00:00:00Z
tokens: 100
---

# Test Content
"#;
        tokio::fs::write(dir.join("mem_test123.md"), memory_content)
            .await
            .unwrap();

        // Regenerate index
        store.regenerate(Scenario::Knowledge).await.unwrap();

        // Verify index exists
        let index_path = dir.join("_INDEX.md");
        assert!(index_path.exists(), "Index file should be created");

        // Read and verify content
        let content = tokio::fs::read_to_string(&index_path).await.unwrap();
        assert!(content.contains("# Knowledge Index"));
        assert!(content.contains("mem_test12"));
        assert!(content.contains("Test Memory"));
        assert!(content.contains("## Hot"));
    }

    #[tokio::test]
    async fn test_regenerate_groups_by_frequency() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_test_store(&temp_dir.path().to_path_buf());

        // Create scenario directory
        let dir = temp_dir.path().join("knowledge");
        tokio::fs::create_dir_all(&dir).await.unwrap();

        // Create memories with different frequencies
        for (freq, title) in &[
            (Frequency::Hot, "Hot Memory"),
            (Frequency::Warm, "Warm Memory"),
            (Frequency::Cold, "Cold Memory"),
        ] {
            let memory_content = format!(
                r#"---
id: mem_{}
title: {}
type: note
scenario: knowledge
frequency: {}
created: 2026-04-03T00:00:00Z
updated: 2026-04-03T00:00:00Z
last_accessed: 2026-04-03T00:00:00Z
tokens: 50
---
"#,
                title, title, freq
            );
            let filename = format!("mem_{}.md", title);
            tokio::fs::write(dir.join(&filename), memory_content)
                .await
                .unwrap();
        }

        // Regenerate index
        store.regenerate(Scenario::Knowledge).await.unwrap();

        // Read index
        let index_path = dir.join("_INDEX.md");
        let content = tokio::fs::read_to_string(&index_path).await.unwrap();

        // Verify sections exist in correct order
        let hot_pos = content.find("## Hot").unwrap();
        let warm_pos = content.find("## Warm").unwrap();
        let cold_pos = content.find("## Cold").unwrap();

        assert!(hot_pos < warm_pos, "Hot should come before Warm");
        assert!(warm_pos < cold_pos, "Warm should come before Cold");

        // Verify memories are in correct sections
        let lines: Vec<&str> = content.lines().collect();
        let mut in_hot = false;
        let mut in_warm = false;
        let mut in_cold = false;

        for line in lines {
            if line.contains("## Hot") {
                in_hot = true;
                in_warm = false;
                in_cold = false;
            } else if line.contains("## Warm") {
                in_hot = false;
                in_warm = true;
                in_cold = false;
            } else if line.contains("## Cold") {
                in_hot = false;
                in_warm = false;
                in_cold = true;
            }

            if line.contains("Hot Memory") {
                assert!(in_hot, "Hot Memory should be in Hot section");
            }
            if line.contains("Warm Memory") {
                assert!(in_warm, "Warm Memory should be in Warm section");
            }
            if line.contains("Cold Memory") {
                assert!(in_cold, "Cold Memory should be in Cold section");
            }
        }
    }

    #[tokio::test]
    async fn test_human_notes_preserved() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_test_store(&temp_dir.path().to_path_buf());

        // Create scenario directory
        let dir = temp_dir.path().join("knowledge");
        tokio::fs::create_dir_all(&dir).await.unwrap();

        // Create initial index with human notes
        let initial_index = r#"# Knowledge Index
<!-- scenario: knowledge -->
<!-- updated: 2026-04-03T00:00:00Z -->
<!-- total_memories: 0 -->
<!-- total_tokens: ~0 -->

## Hot (always loaded when scenario is active)
| ID | Title | Type | Tags | Tokens | Updated |
|----|-------|------|------|--------|---------|

<!-- HUMAN_NOTES_START -->
This is a personal note
that spans multiple lines
<!-- HUMAN_NOTES_END -->
"#;
        tokio::fs::write(dir.join("_INDEX.md"), initial_index)
            .await
            .unwrap();

        // Create a memory file
        let memory_content = r#"---
id: mem_test456
title: Test Memory
type: note
scenario: knowledge
frequency: hot
created: 2026-04-03T00:00:00Z
updated: 2026-04-03T00:00:00Z
last_accessed: 2026-04-03T00:00:00Z
tokens: 50
---
"#;
        tokio::fs::write(dir.join("mem_test456.md"), memory_content)
            .await
            .unwrap();

        // Regenerate index
        store.regenerate(Scenario::Knowledge).await.unwrap();

        // Verify human notes are preserved
        let content = tokio::fs::read_to_string(&dir.join("_INDEX.md"))
            .await
            .unwrap();
        assert!(content.contains("This is a personal note"));
        assert!(content.contains("that spans multiple lines"));
        assert!(content.contains("<!-- HUMAN_NOTES_START -->"));
        assert!(content.contains("<!-- HUMAN_NOTES_END -->"));
    }

    #[tokio::test]
    async fn test_read_index_parses_correctly() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_test_store(&temp_dir.path().to_path_buf());

        // Create scenario directory
        let dir = temp_dir.path().join("knowledge");
        tokio::fs::create_dir_all(&dir).await.unwrap();

        // Create index with known content
        let index_content = r#"# Knowledge Index
<!-- scenario: knowledge -->
<!-- updated: 2026-04-03T12:00:00Z -->
<!-- total_memories: 2 -->
<!-- total_tokens: ~150 -->

## Hot (always loaded when scenario is active)
| ID | Title | Type | Tags | Tokens | Updated |
|----|-------|------|------|--------|---------|
| mem_hot01 | Hot Memory | note | test,important | ~100 | 2026-04-03 |

## Warm (loaded on topic match)
| ID | Title | Type | Tags | Tokens | Updated |
|----|-------|------|------|--------|---------|
| mem_warm01 | Warm Memory | concept | reference | ~50 | 2026-04-02 |

<!-- HUMAN_NOTES_START -->
Test notes
<!-- HUMAN_NOTES_END -->
"#;
        tokio::fs::write(dir.join("_INDEX.md"), index_content)
            .await
            .unwrap();

        // Read index
        let index = store.read_index(Scenario::Knowledge).await.unwrap();

        // Verify metadata
        assert_eq!(index.scenario, Scenario::Knowledge);
        assert_eq!(index.updated, "2026-04-03T12:00:00Z");
        assert_eq!(index.total_memories, 2);
        assert_eq!(index.total_tokens, 150);
        assert_eq!(index.entries.len(), 2);

        // Verify hot entry
        let hot_entry = &index.entries[0];
        assert_eq!(hot_entry.frequency, Frequency::Hot);
        assert_eq!(hot_entry.id, "mem_hot01");
        assert_eq!(hot_entry.title, "Hot Memory");
        assert_eq!(hot_entry.tags, vec!["test", "important"]);
        assert_eq!(hot_entry.tokens, 100);

        // Verify warm entry
        let warm_entry = &index.entries[1];
        assert_eq!(warm_entry.frequency, Frequency::Warm);
        assert_eq!(warm_entry.id, "mem_warm01");
        assert_eq!(warm_entry.title, "Warm Memory");
        assert_eq!(warm_entry.tokens, 50);

        // Verify human notes
        assert_eq!(index.human_notes, "Test notes");
    }

    #[tokio::test]
    async fn test_atomic_write_no_tmp_left() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_test_store(&temp_dir.path().to_path_buf());

        // Create scenario directory
        let dir = temp_dir.path().join("knowledge");
        tokio::fs::create_dir_all(&dir).await.unwrap();

        // Create a memory file
        let memory_content = r#"---
id: mem_atomic
title: Atomic Test
type: test
scenario: knowledge
frequency: warm
created: 2026-04-03T00:00:00Z
updated: 2026-04-03T00:00:00Z
last_accessed: 2026-04-03T00:00:00Z
tokens: 25
---
"#;
        tokio::fs::write(dir.join("mem_atomic.md"), memory_content)
            .await
            .unwrap();

        // Regenerate index
        store.regenerate(Scenario::Knowledge).await.unwrap();

        // Verify no .tmp file remains
        let tmp_path = dir.join("_INDEX.md.tmp");
        assert!(!tmp_path.exists(), "No .tmp file should remain after atomic write");

        // Verify the actual index exists
        let index_path = dir.join("_INDEX.md");
        assert!(index_path.exists(), "Index file should exist");
    }

    #[test]
    fn test_capitalize_first() {
        assert_eq!(capitalize_first("hello".to_string()), "Hello");
        assert_eq!(capitalize_first("Hello".to_string()), "Hello");
        assert_eq!(capitalize_first("".to_string()), "");
        assert_eq!(capitalize_first("a".to_string()), "A");
        assert_eq!(capitalize_first("ABC".to_string()), "ABC");
    }

    #[test]
    fn test_extract_human_notes() {
        let content = r#"# Index

<!-- HUMAN_NOTES_START -->
Line 1
Line 2
Line 3
<!-- HUMAN_NOTES_END -->
"#;
        let notes = extract_human_notes(content);
        assert_eq!(notes.trim(), "Line 1\nLine 2\nLine 3");
    }

    #[test]
    fn test_extract_human_notes_missing() {
        let content = r#"# Index

No notes here
"#;
        let notes = extract_human_notes(content);
        assert_eq!(notes, "");
    }

    #[tokio::test]
    async fn test_parse_index_content_handles_frequencies() {
        let content = r#"# Knowledge Index
<!-- scenario: knowledge -->
<!-- updated: 2026-04-03T00:00:00Z -->
<!-- total_memories: 3 -->
<!-- total_tokens: ~150 -->

## Hot (always loaded when scenario is active)
| ID | Title | Type | Tags | Tokens | Updated |
|----|-------|------|------|--------|---------|
| mem_001 | Hot Item | note | tag1 | ~50 | 2026-04-03 |

## Warm (loaded on topic match)
| ID | Title | Type | Tags | Tokens | Updated |
|----|-------|------|------|--------|---------|
| mem_002 | Warm Item | concept | tag2 | ~50 | 2026-04-02 |

## Cold (loaded only on explicit search)
| ID | Title | Type | Tags | Tokens | Updated |
|----|-------|------|------|--------|---------|
| mem_003 | Cold Item | reference | tag3 | ~50 | 2026-04-01 |

<!-- HUMAN_NOTES_START -->
Notes here
<!-- HUMAN_NOTES_END -->
"#;

        let result = parse_index_content(Scenario::Knowledge, content).unwrap();

        assert_eq!(result.entries.len(), 3);
        assert_eq!(result.entries[0].frequency, Frequency::Hot);
        assert_eq!(result.entries[1].frequency, Frequency::Warm);
        assert_eq!(result.entries[2].frequency, Frequency::Cold);
    }
}
