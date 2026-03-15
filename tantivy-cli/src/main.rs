//! Tantivy CLI - Full-text search command line tool
//!
//! A command-line interface for managing Tantivy full-text search indexes.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use directories::UserDirs;
use tantivy_cli::index::{Document, FieldDef, IndexConfig, IndexManager, SearchQuery};
use tantivy_cli::maintenance::rebuild_index;
use tracing::info;
use tracing_subscriber::EnvFilter;

/// Tantivy full-text search CLI tool
#[derive(Parser, Debug)]
#[command(author, version, about = "Tantivy full-text search CLI tool", long_about = None)]
struct Cli {
    /// Index storage directory
    #[arg(short, long)]
    index_dir: Option<PathBuf>,

    /// Log level (trace, debug, info, warn, error)
    #[arg(short, long, default_value = "info")]
    log_level: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Index management operations
    Index {
        #[command(subcommand)]
        subcommand: IndexCommands,
    },

    /// Document operations
    Doc {
        #[command(subcommand)]
        subcommand: DocCommands,
    },

    /// Search for documents
    Search {
        /// Index name
        #[arg(short, long)]
        index: String,

        /// Search query (JSON format)
        #[arg(short, long)]
        query: String,
    },

    /// Maintenance operations
    Maintain {
        #[command(subcommand)]
        subcommand: MaintainCommands,
    },
}

#[derive(Subcommand, Debug)]
enum IndexCommands {
    /// Create a new index with schema
    Create {
        /// Index name
        #[arg(short, long)]
        name: String,

        /// Field definitions (JSON array)
        #[arg(short, long)]
        fields: String,

        /// Default TTL for documents (e.g., "30d")
        #[arg(short, long)]
        default_ttl: Option<String>,
    },

    /// List all indexes
    List,

    /// Get index statistics
    Stats {
        /// Index name (optional, returns all if not specified)
        #[arg(short, long)]
        name: Option<String>,
    },

    /// Delete an index
    Drop {
        /// Index name
        #[arg(short, long)]
        name: String,
    },

    /// Compact an index
    Compact {
        /// Index name
        #[arg(short, long)]
        name: String,
    },

    /// Rebuild an index
    Rebuild {
        /// Index name
        #[arg(short, long)]
        name: String,

        /// New field definitions for schema migration (JSON array)
        #[arg(short, long)]
        fields: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
enum DocCommands {
    /// Add a document
    Add {
        /// Index name
        #[arg(short, long)]
        index: String,

        /// Document ID
        #[arg(short, long)]
        id: String,

        /// Field values (JSON object)
        #[arg(short, long)]
        fields: String,

        /// Optional TTL (e.g., "7d")
        #[arg(short, long)]
        ttl: Option<String>,
    },

    /// Add multiple documents in batch
    AddBatch {
        /// Index name
        #[arg(short, long)]
        index: String,

        /// Path to JSON file containing array of documents
        #[arg(short, long)]
        file: Option<PathBuf>,

        /// Documents as JSON array string (alternative to --file)
        #[arg(short, long, conflicts_with = "file")]
        documents: Option<String>,

        /// Optional default TTL for all documents (e.g., "7d")
        #[arg(short, long)]
        ttl: Option<String>,

        /// Number of parallel workers (default: 4, ignored in sync mode)
        #[arg(short, long, default_value = "4")]
        parallel: usize,
    },

    /// Delete a document
    Delete {
        /// Index name
        #[arg(short, long)]
        index: String,

        /// Document ID
        #[arg(short, long)]
        id: String,
    },

    /// Commit pending changes
    Commit {
        /// Index name
        #[arg(short, long)]
        index: String,
    },
}

#[derive(Subcommand, Debug)]
enum MaintainCommands {
    /// Get maintenance status
    Status {
        /// Index name (optional, returns all if not specified)
        #[arg(short, long)]
        index: Option<String>,
    },
}

fn main() -> tantivy_cli::Result<()> {
    let cli = Cli::parse();

    // Initialize logging
    init_logging(&cli.log_level)?;

    // Determine index directory
    let index_dir = cli.index_dir.clone().unwrap_or_else(|| {
        UserDirs::new()
            .map(|dirs| dirs.home_dir().join(".nanobot/tantivy"))
            .unwrap_or_else(|| PathBuf::from(".nanobot/tantivy"))
    });

    info!("Index directory: {:?}", index_dir);

    // Create index manager
    let mut manager = IndexManager::new(&index_dir);
    manager.load_indexes()?;

    // Execute command
    match cli.command {
        Commands::Index { subcommand } => execute_index_command(&mut manager, subcommand)?,
        Commands::Doc { subcommand } => execute_doc_command(&mut manager, subcommand)?,
        Commands::Search { index, query } => execute_search_command(&manager, &index, &query)?,
        Commands::Maintain { subcommand } => execute_maintain_command(&manager, subcommand)?,
    }

    Ok(())
}

/// Initialize logging
fn init_logging(log_level: &str) -> tantivy_cli::Result<()> {
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(log_level));

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(std::io::stderr)
        .init();

    Ok(())
}

fn execute_index_command(
    manager: &mut IndexManager,
    subcommand: IndexCommands,
) -> tantivy_cli::Result<()> {
    match subcommand {
        IndexCommands::Create {
            name,
            fields,
            default_ttl,
        } => {
            let field_defs: Vec<FieldDef> = serde_json::from_str(&fields).map_err(|e| {
                tantivy_cli::Error::ParseError(format!("Invalid fields JSON: {}", e))
            })?;

            let config = default_ttl.map(|ttl| IndexConfig {
                default_ttl: Some(ttl),
                auto_compact: None,
            });

            let schema = manager.create_index(&name, field_defs, config)?;
            println!(
                "Created index '{}' with {} fields",
                schema.name,
                schema.fields.len()
            );
        }
        IndexCommands::List => {
            let indexes = manager.list_indexes();
            if indexes.is_empty() {
                println!("No indexes found");
            } else {
                println!("Indexes:");
                for name in indexes {
                    println!("  - {}", name);
                }
            }
        }
        IndexCommands::Stats { name } => {
            if let Some(index_name) = name {
                let stats = manager.get_stats(&index_name)?;
                println!(
                    "Index '{}': {} docs, {} segments, {} deleted, {} bytes",
                    stats.name,
                    stats.doc_count,
                    stats.segment_count,
                    stats.deleted_count,
                    stats.size_bytes
                );
            } else {
                let indexes = manager.list_indexes();
                for index_name in indexes {
                    if let Ok(stats) = manager.get_stats(&index_name) {
                        println!(
                            "Index '{}': {} docs, {} segments, {} deleted, {} bytes",
                            stats.name,
                            stats.doc_count,
                            stats.segment_count,
                            stats.deleted_count,
                            stats.size_bytes
                        );
                    }
                }
            }
        }
        IndexCommands::Drop { name } => {
            manager.drop_index(&name)?;
            println!("Deleted index '{}'", name);
        }
        IndexCommands::Compact { name } => {
            manager.compact(&name)?;
            println!("Compaction completed for index '{}'", name);
        }
        IndexCommands::Rebuild { name, fields } => {
            let new_fields: Option<Vec<FieldDef>> = if let Some(fields_json) = fields {
                Some(serde_json::from_str(&fields_json).map_err(|e| {
                    tantivy_cli::Error::ParseError(format!("Invalid fields JSON: {}", e))
                })?)
            } else {
                None
            };

            let result = rebuild_index(manager, &name, new_fields, 1000)?;
            println!(
                "Rebuilt index '{}': {} documents reindexed, schema changed: {}",
                result.index_name, result.docs_reindexed, result.schema_changed
            );
        }
    }
    Ok(())
}

fn execute_doc_command(
    manager: &mut IndexManager,
    subcommand: DocCommands,
) -> tantivy_cli::Result<()> {
    match subcommand {
        DocCommands::Add {
            index,
            id,
            fields,
            ttl,
        } => {
            let field_map: serde_json::Map<String, serde_json::Value> =
                serde_json::from_str(&fields).map_err(|e| {
                    tantivy_cli::Error::ParseError(format!("Invalid fields JSON: {}", e))
                })?;

            let mut doc = Document::new(id.clone(), field_map);

            if let Some(ttl_str) = ttl {
                let ttl_duration = parse_ttl(&ttl_str)?;
                let expires_at = chrono::Utc::now() + ttl_duration;
                doc = doc.with_expiry(expires_at);
            }

            manager.add_document(&index, doc)?;
            manager.commit(&index)?;

            println!("Document '{}' added successfully", id);
        }
        DocCommands::AddBatch {
            index,
            file,
            documents,
            ttl,
            parallel,
        } => {
            // Parse documents from file or command line
            let doc_inputs: Vec<tantivy_cli::index::BatchDocumentInput> =
                if let Some(file_path) = file {
                    let content = std::fs::read_to_string(&file_path)
                        .map_err(|e| tantivy_cli::Error::PathError(file_path, e.to_string()))?;
                    serde_json::from_str(&content).map_err(|e| {
                        tantivy_cli::Error::ParseError(format!("Invalid JSON file: {}", e))
                    })?
                } else if let Some(documents_json) = documents {
                    serde_json::from_str(&documents_json).map_err(|e| {
                        tantivy_cli::Error::ParseError(format!("Invalid documents JSON: {}", e))
                    })?
                } else {
                    return Err(tantivy_cli::Error::ParseError(
                        "Either --file or --documents must be provided".to_string(),
                    ));
                };

            if doc_inputs.is_empty() {
                println!("No documents to add");
                return Ok(());
            }

            println!(
                "Adding {} documents to index '{}'...",
                doc_inputs.len(),
                index
            );

            // Add documents in batch
            let result = manager.add_documents_batch(&index, doc_inputs, ttl, parallel)?;

            // Commit to make all documents searchable at once
            println!("Committing changes...");
            manager.commit(&index)?;

            // Print results
            println!("\nBatch add completed:");
            println!("  Total: {}", result.total);
            println!("  Success: {}", result.success);
            println!("  Failed: {}", result.failed);

            if !result.errors.is_empty() {
                println!("\nErrors:");
                for error in result.errors {
                    println!("  - Document '{}': {}", error.id, error.error);
                }
            }
        }
        DocCommands::Delete { index, id } => {
            manager.delete_document(&index, &id)?;
            manager.commit(&index)?;

            println!("Document '{}' deleted successfully", id);
        }
        DocCommands::Commit { index } => {
            manager.commit(&index)?;
            println!("Index '{}' committed successfully", index);
        }
    }
    Ok(())
}

fn execute_search_command(
    manager: &IndexManager,
    index: &str,
    query_json: &str,
) -> tantivy_cli::Result<()> {
    let query: SearchQuery = serde_json::from_str(query_json)
        .map_err(|e| tantivy_cli::Error::ParseError(format!("Invalid query JSON: {}", e)))?;

    let results = manager.search(index, &query)?;

    println!("Found {} results:", results.len());
    for result in results {
        println!("  [{}] Score: {:.4}", result.id, result.score);
        if let Some(highlight) = result.highlight {
            println!("    {}", highlight);
        }
    }

    Ok(())
}

fn execute_maintain_command(
    manager: &IndexManager,
    subcommand: MaintainCommands,
) -> tantivy_cli::Result<()> {
    match subcommand {
        MaintainCommands::Status { index } => {
            if let Some(index_name) = index {
                let stats = manager.get_stats(&index_name)?;
                let config_opt = manager.get_config(&index_name)?;
                println!("Index '{}':", index_name);
                println!("  Docs: {}", stats.doc_count);
                println!("  Segments: {}", stats.segment_count);
                println!("  Deleted: {}", stats.deleted_count);
                println!("  Size: {} bytes", stats.size_bytes);
                println!("  Health: {:?}", stats.health);
                if let Some(config) = config_opt {
                    println!("  Config: {:?}", config);
                }
            } else {
                let indexes = manager.list_indexes();
                for index_name in indexes {
                    if let Ok(stats) = manager.get_stats(&index_name) {
                        println!(
                            "Index '{}': {} docs, health: {:?}",
                            index_name, stats.doc_count, stats.health
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

/// Parse a TTL string into a duration
fn parse_ttl(ttl: &str) -> tantivy_cli::Result<chrono::Duration> {
    let ttl = ttl.trim();

    if ttl.is_empty() {
        return Err(tantivy_cli::Error::ParseError("Empty TTL".to_string()));
    }

    let numeric_end = ttl.find(|c: char| !c.is_ascii_digit()).unwrap_or(ttl.len());

    if numeric_end == 0 {
        return Err(tantivy_cli::Error::ParseError(format!(
            "Invalid TTL: {}",
            ttl
        )));
    }

    let number: i64 = ttl[..numeric_end]
        .parse()
        .map_err(|_| tantivy_cli::Error::ParseError(format!("Invalid TTL number: {}", ttl)))?;

    let unit = &ttl[numeric_end..];

    let duration = match unit {
        "s" | "sec" | "seconds" => chrono::Duration::seconds(number),
        "m" | "min" | "minutes" => chrono::Duration::minutes(number),
        "h" | "hour" | "hours" => chrono::Duration::hours(number),
        "d" | "day" | "days" => chrono::Duration::days(number),
        "w" | "week" | "weeks" => chrono::Duration::weeks(number),
        _ => {
            return Err(tantivy_cli::Error::ParseError(format!(
                "Unknown TTL unit: {}",
                unit
            )))
        }
    };

    Ok(duration)
}
