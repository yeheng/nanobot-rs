//! Memory and Wiki knowledge system commands.

use anyhow::Result;
use gasket_engine::memory::{
    memory_base_dir, AutoIndexHandler, EmbeddingStore, FileMemoryStore, FrequencyManager,
    MetadataStore, SqliteStore,
};
use gasket_engine::wiki::{slugify, PageFilter, PageStore, PageType, WikiLinter, WikiPage};
#[cfg(feature = "local-embedding")]
use gasket_engine::TextEmbedder;
use gasket_engine::{Embedder, NoopEmbedder};
use std::path::PathBuf;
use tracing::info;
#[cfg(feature = "local-embedding")]
use tracing::warn;

fn wiki_base_dir() -> PathBuf {
    dirs::home_dir()
        .map(|p| p.join(".gasket/wiki"))
        .expect("Could not find home directory")
}

/// Manually refresh memory files from disk, comparing mtime and size to detect changes.
///
/// Only processes files that have changed since the last sync.
/// This is the unified memory refresh command (formerly `reindex` and `refresh`).
pub async fn cmd_memory_refresh() -> Result<()> {
    println!("Refreshing memory files from disk...");

    let base_dir = memory_base_dir();
    if !base_dir.exists() {
        println!("Memory directory does not exist: {}", base_dir.display());
        println!("Nothing to refresh.");
        return Ok(());
    }

    let store = SqliteStore::new().await?;
    let pool = store.pool();

    // Use TextEmbedder if local-embedding feature is enabled, otherwise use NoopEmbedder
    let embedder: Box<dyn Embedder> = {
        #[cfg(feature = "local-embedding")]
        {
            match TextEmbedder::new() {
                Ok(embedder) => {
                    info!("Refresh using TextEmbedder (local-embedding enabled)");
                    Box::new(embedder) as Box<dyn Embedder>
                }
                Err(e) => {
                    warn!(
                        "Failed to initialize TextEmbedder, falling back to NoopEmbedder: {}",
                        e
                    );
                    Box::new(NoopEmbedder::new(384)) as Box<dyn Embedder>
                }
            }
        }
        #[cfg(not(feature = "local-embedding"))]
        {
            info!("Refresh using NoopEmbedder (local-embedding disabled)");
            Box::new(NoopEmbedder::new(384)) as Box<dyn Embedder>
        }
    };

    let metadata_store = MetadataStore::new(pool.clone());
    let embedding_store = EmbeddingStore::new(pool.clone());
    let auto_index = AutoIndexHandler::new(
        metadata_store,
        embedding_store,
        base_dir.clone(),
        embedder.into(),
    );

    let report = auto_index.refresh_all_files().await?;

    println!("Refresh complete:");
    println!("  {} files processed", report.processed);
    println!("  {} files updated", report.updated);
    println!("  {} files skipped (unchanged)", report.skipped);
    if report.errors > 0 {
        println!("  {} files with errors", report.errors);
    }

    Ok(())
}

/// Manually run memory frequency decay.
///
/// Scans all memories and demotes stale entries:
/// - Hot → Warm (7 days without access)
/// - Warm → Cold (30 days without access)
/// - Cold → Archived (90 days without access)
///
/// Useful for manual maintenance or non-Gateway (CLI-only) usage.
pub async fn cmd_memory_decay() -> Result<()> {
    println!("Running memory frequency decay...");

    let base_dir = memory_base_dir();
    if !base_dir.exists() {
        println!("Memory directory does not exist: {}", base_dir.display());
        println!("Nothing to decay.");
        return Ok(());
    }

    let store = FileMemoryStore::new(base_dir);
    let sqlite = SqliteStore::new().await?;
    let metadata_store = MetadataStore::new(sqlite.pool().clone());

    let report = FrequencyManager::run_decay_batch(&store, &metadata_store).await?;

    println!("Decay complete:");
    println!("  {} candidates scanned", report.total_scanned);
    println!("  {} memories decayed", report.decayed);
    if report.errors > 0 {
        println!("  {} errors", report.errors);
    }
    if report.decayed == 0 {
        println!("  All memories are fresh — no decay needed.");
    }

    Ok(())
}

/// Initialize wiki directory structure and SQLite tables.
pub async fn cmd_wiki_init() -> Result<()> {
    let wiki_root = wiki_base_dir();
    let store = SqliteStore::new().await?;
    let ps = PageStore::new(store.pool(), wiki_root.clone());
    ps.init_dirs().await?;

    // Create wiki tables
    gasket_engine::create_wiki_tables(&store.pool()).await?;

    println!("Wiki initialized at {}", wiki_root.display());
    Ok(())
}

/// Migrate existing memory files (~/.gasket/memory/) to wiki pages.
///
/// Reads each .md file from memory directories, creates a WikiPage with
/// appropriate path mapping, and writes to the wiki PageStore.
pub async fn cmd_wiki_migrate() -> Result<()> {
    let memory_dir = dirs::home_dir()
        .map(|p| p.join(".gasket/memory"))
        .expect("Could not find home directory");

    if !memory_dir.exists() {
        println!(
            "No memory directory found at {}. Nothing to migrate.",
            memory_dir.display()
        );
        return Ok(());
    }

    let wiki_root = wiki_base_dir();
    let store = SqliteStore::new().await?;

    // Ensure wiki tables exist
    gasket_engine::create_wiki_tables(&store.pool()).await?;

    let ps = PageStore::new(store.pool(), wiki_root.clone());
    ps.init_dirs().await?;

    let mut migrated = 0;
    let mut errors = 0;

    // Map scenario directories to wiki path prefixes
    let scenario_map = [
        ("profile", "entities/people"),
        ("active", "topics"),
        ("knowledge", "topics"),
        ("decisions", "topics"),
        ("episodes", "topics"),
        ("reference", "sources"),
    ];

    for (scenario, prefix) in &scenario_map {
        let dir = memory_dir.join(scenario);
        if !dir.exists() {
            continue;
        }

        let mut entries = std::fs::read_dir(&dir)?;
        while let Some(entry) = entries.next().transpose()? {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "md") {
                continue;
            }

            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("Failed to read {}: {}", path.display(), e);
                    errors += 1;
                    continue;
                }
            };

            // Extract title from frontmatter or filename
            let title = extract_title(&content).unwrap_or_else(|| {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("untitled")
                    .to_string()
            });

            let slug = slugify(&title);
            let page_path = format!("{}/{}", prefix, slug);

            // Extract body (skip frontmatter)
            let body = skip_frontmatter(&content);

            let mut page = WikiPage::new(page_path, title, PageType::Topic, body.to_string());

            // Extract tags from frontmatter if present
            if let Some(tags) = extract_tags(&content) {
                page.tags = tags;
            }

            match ps.write(&page).await {
                Ok(_) => migrated += 1,
                Err(e) => {
                    tracing::warn!("Failed to write page {}: {}", page.path, e);
                    errors += 1;
                }
            }
        }
    }

    println!("Migration complete:");
    println!("  {} pages migrated", migrated);
    if errors > 0 {
        println!("  {} errors", errors);
    }

    // Rebuild disk cache
    let cached = ps.rebuild_disk_cache().await?;
    println!("  {} disk cache files written", cached);

    Ok(())
}

/// Show wiki statistics.
pub async fn cmd_wiki_stats() -> Result<()> {
    let wiki_root = wiki_base_dir();
    if !wiki_root.exists() {
        println!("Wiki not initialized. Run 'gasket wiki init' first.");
        return Ok(());
    }

    let store = SqliteStore::new().await?;
    let ps = PageStore::new(store.pool(), wiki_root);

    let all = ps.list(PageFilter::default()).await?;
    let entities = all
        .iter()
        .filter(|p| p.page_type == PageType::Entity)
        .count();
    let topics = all
        .iter()
        .filter(|p| p.page_type == PageType::Topic)
        .count();
    let sources = all
        .iter()
        .filter(|p| p.page_type == PageType::Source)
        .count();

    println!("Wiki Statistics:");
    println!("  Total pages: {}", all.len());
    println!("  Entities: {}", entities);
    println!("  Topics: {}", topics);
    println!("  Sources: {}", sources);

    Ok(())
}

/// Ingest a file into the wiki.
pub async fn cmd_wiki_ingest(path: &str, tier: &str) -> Result<()> {
    let file_path = PathBuf::from(path);
    if !file_path.exists() {
        anyhow::bail!("File not found: {}", path);
    }

    let wiki_root = wiki_base_dir();
    let store = SqliteStore::new().await?;
    gasket_engine::create_wiki_tables(&store.pool()).await?;

    let ps = PageStore::new(store.pool().clone(), wiki_root);
    ps.init_dirs().await?;

    let content = std::fs::read_to_string(&file_path)?;
    let title = file_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled")
        .to_string();

    let slug = slugify(&title);
    let page_path = format!("topics/{}", slug);

    let mut page = WikiPage::new(page_path.clone(), title.clone(), PageType::Topic, content);

    // Detect type from path
    if path.contains("entity") || path.contains("entities") {
        page.page_type = PageType::Entity;
        page.path = format!("entities/{}", slug);
    } else if path.contains("source") || path.contains("sources") {
        page.page_type = PageType::Source;
        page.path = format!("sources/{}", slug);
    }

    if tier == "deep" {
        println!("Deep ingest requires LLM provider. Falling back to quick ingest.");
    }

    ps.write(&page).await?;
    println!("Ingested '{}' → '{}'", path, page.path);
    Ok(())
}

/// Search wiki pages.
pub async fn cmd_wiki_search(query: &str, limit: usize) -> Result<()> {
    let wiki_root = wiki_base_dir();
    if !wiki_root.exists() {
        println!("Wiki not initialized. Run 'gasket wiki init' first.");
        return Ok(());
    }

    let store = SqliteStore::new().await?;
    let ps = PageStore::new(store.pool(), wiki_root);

    // Try Tantivy search first
    let tantivy_dir = dirs::home_dir()
        .map(|p| p.join(".gasket/wiki/.tantivy"))
        .unwrap_or_else(|| PathBuf::from("~/.gasket/wiki/.tantivy"));

    if tantivy_dir.exists() {
        match gasket_engine::wiki::PageIndex::open(tantivy_dir) {
            Ok(index) => {
                let hits = index.search_raw(query, limit)?;
                if hits.is_empty() {
                    println!("No results for '{}'", query);
                    return Ok(());
                }
                println!("Search results for '{}' ({} hits):\n", query, hits.len());
                for (i, hit) in hits.iter().enumerate() {
                    println!(
                        "  {}. {} ({}) — score: {:.2}",
                        i + 1,
                        hit.title,
                        hit.path,
                        hit.score
                    );
                }
                return Ok(());
            }
            Err(e) => {
                tracing::debug!("Tantivy search failed, falling back to list: {}", e);
            }
        }
    }

    // Fallback: list all pages and do simple matching
    let pages = ps.list(PageFilter::default()).await?;
    let query_lower = query.to_lowercase();
    let matches: Vec<_> = pages
        .iter()
        .filter(|p| {
            p.title.to_lowercase().contains(&query_lower)
                || p.path.to_lowercase().contains(&query_lower)
        })
        .take(limit)
        .collect();

    if matches.is_empty() {
        println!("No results for '{}'", query);
    } else {
        println!("Search results for '{}' ({} hits):\n", query, matches.len());
        for (i, page) in matches.iter().enumerate() {
            println!("  {}. {} ({})", i + 1, page.title, page.path);
        }
    }
    Ok(())
}

/// List wiki pages.
pub async fn cmd_wiki_list(page_type: Option<&str>) -> Result<()> {
    let wiki_root = wiki_base_dir();
    if !wiki_root.exists() {
        println!("Wiki not initialized. Run 'gasket wiki init' first.");
        return Ok(());
    }

    let store = SqliteStore::new().await?;
    let ps = PageStore::new(store.pool(), wiki_root);

    let filter = match page_type {
        Some("entity") => PageFilter {
            page_type: Some(PageType::Entity),
            ..Default::default()
        },
        Some("topic") => PageFilter {
            page_type: Some(PageType::Topic),
            ..Default::default()
        },
        Some("source") => PageFilter {
            page_type: Some(PageType::Source),
            ..Default::default()
        },
        _ => PageFilter::default(),
    };

    let pages = ps.list(filter).await?;
    if pages.is_empty() {
        println!("No wiki pages found.");
        return Ok(());
    }

    println!("Wiki pages ({} total):\n", pages.len());
    for page in &pages {
        let type_tag = match page.page_type {
            PageType::Entity => "[E]",
            PageType::Topic => "[T]",
            PageType::Source => "[S]",
            PageType::Sop => "[P]",
        };
        let tags = if page.tags.is_empty() {
            String::new()
        } else {
            format!(" [{}]", page.tags.join(", "))
        };
        println!("  {} {} ({}){}", type_tag, page.title, page.path, tags);
    }
    Ok(())
}

/// Run wiki lint checks.
pub async fn cmd_wiki_lint(auto_fix: bool) -> Result<()> {
    let wiki_root = wiki_base_dir();
    if !wiki_root.exists() {
        println!("Wiki not initialized. Run 'gasket wiki init' first.");
        return Ok(());
    }

    let store = SqliteStore::new().await?;
    let ps = std::sync::Arc::new(PageStore::new(store.pool(), wiki_root));

    let linter = WikiLinter::new(ps);

    println!("Running wiki lint...");
    let report = linter.lint().await?;

    println!("{}", report.summary());

    if auto_fix && report.total_issues() > 0 {
        let fix_report = linter.auto_fix(&report).await?;
        if fix_report.total_fixes() > 0 {
            println!("\nAuto-fixed {} issues.", fix_report.total_fixes());
        }
    }

    Ok(())
}

// ── Helpers ──────────────────────────────────────────────────

fn extract_title(content: &str) -> Option<String> {
    if !content.trim_start().starts_with("---") {
        return None;
    }
    let rest = &content.trim_start()[3..];
    let end = rest.find("\n---")?;
    let yaml = &rest[..end];
    for line in yaml.lines() {
        if line.trim().starts_with("title:") {
            return Some(line.trim().trim_start_matches("title:").trim().to_string());
        }
    }
    None
}

fn extract_tags(content: &str) -> Option<Vec<String>> {
    if !content.trim_start().starts_with("---") {
        return None;
    }
    let rest = &content.trim_start()[3..];
    let end = rest.find("\n---")?;
    let yaml = &rest[..end];
    // Look for tags: line
    for line in yaml.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("tags:") {
            // Simple extraction: tags:\n  - tag1\n  - tag2
            let tags_str = trimmed.trim_start_matches("tags:").trim();
            if tags_str.is_empty() || tags_str == "[]" {
                // Multi-line tags follow
                let mut tags = Vec::new();
                let yaml_lines: Vec<&str> = yaml.lines().collect();
                if let Some(idx) = yaml_lines
                    .iter()
                    .position(|l| l.trim().starts_with("tags:"))
                {
                    for following in yaml_lines[idx + 1..].iter() {
                        let f = following.trim();
                        if f.starts_with("- ") {
                            tags.push(f.trim_start_matches("- ").trim().to_string());
                        } else if !f.starts_with("- ") && !f.is_empty() {
                            break;
                        }
                    }
                }
                if !tags.is_empty() {
                    return Some(tags);
                }
            } else {
                // Inline: tags: [tag1, tag2]
                let tags: Vec<String> = tags_str
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                if !tags.is_empty() {
                    return Some(tags);
                }
            }
        }
    }
    None
}

fn skip_frontmatter(content: &str) -> &str {
    let content = content.trim_start();
    if content.starts_with("---") {
        if let Some(end) = content[3..].find("\n---") {
            return content[end + 7..].trim();
        }
    }
    content.trim()
}
