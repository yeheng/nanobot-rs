//! Tantivy MCP - Standalone MCP Index Server
//!
//! A Model Context Protocol (MCP) server providing full-text search capabilities
//! using the Tantivy search engine.

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use directories::ProjectDirs;
use tantivy_mcp::index::IndexManager;
use tantivy_mcp::maintenance::{MaintenanceConfig, MaintenanceScheduler};
use tantivy_mcp::mcp::{McpHandler, ToolRegistry};
use tantivy_mcp::register_tools;
use tokio::signal;
use tokio::sync::RwLock;
use tracing::info;
use tracing_subscriber::EnvFilter;

/// Tantivy MCP Index Server
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Index storage directory
    #[arg(short, long)]
    index_dir: Option<PathBuf>,

    /// Configuration file path
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Log level (trace, debug, info, warn, error)
    #[arg(short, long, default_value = "info")]
    log_level: String,

    /// Enable automatic maintenance (compaction, expiration)
    #[arg(long, default_value = "true")]
    auto_maintain: bool,

    /// Maintenance interval in seconds
    #[arg(long, default_value = "3600")]
    maintenance_interval: u64,
}

#[tokio::main]
async fn main() -> tantivy_mcp::Result<()> {
    let args = Args::parse();

    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&args.log_level)),
        )
        .with_writer(std::io::stderr) // Log to stderr, stdout is for MCP
        .init();

    info!("Starting tantivy-mcp {}", env!("CARGO_PKG_VERSION"));

    // Determine index directory
    let index_dir = args.index_dir.unwrap_or_else(|| {
        ProjectDirs::from("com", "nanobot", "tantivy-mcp")
            .map(|d| d.data_dir().join("indexes"))
            .unwrap_or_else(|| PathBuf::from(".tantivy-mcp/indexes"))
    });

    info!("Index directory: {:?}", index_dir);

    // Create index manager
    let mut manager = IndexManager::new(&index_dir);

    // Load existing indexes
    manager.load_indexes()?;

    let manager = Arc::new(RwLock::new(manager));

    // Start maintenance scheduler if enabled
    let _scheduler_handle = if args.auto_maintain {
        let config = MaintenanceConfig {
            auto_compact: true,
            deleted_ratio_threshold: 0.2,
            max_segments: 10,
            auto_expire: true,
            expire_interval_secs: args.maintenance_interval,
        };
        let scheduler = MaintenanceScheduler::new(manager.clone(), config);
        let handle = scheduler.start();
        info!(
            "Maintenance scheduler started (interval: {}s)",
            args.maintenance_interval
        );
        Some(handle)
    } else {
        None
    };

    // Create tool registry and register tools
    let mut tools = ToolRegistry::new();
    register_tools(&mut tools, manager.clone());

    // Create MCP handler
    let mut handler = McpHandler::new(tools);

    // Set up graceful shutdown
    let shutdown = setup_shutdown_handler();

    // Run MCP server in a separate task
    let server_task = tokio::task::spawn_blocking(move || handler.run());

    // Wait for either server completion or shutdown signal
    tokio::select! {
        result = server_task => {
            match result {
                Ok(Ok(())) => info!("MCP server completed normally"),
                Ok(Err(e)) => {
                    tracing::error!("MCP server error: {}", e);
                    return Err(e);
                }
                Err(e) => {
                    tracing::error!("MCP server task panicked: {}", e);
                    return Err(tantivy_mcp::Error::McpError(format!("Server panic: {}", e)));
                }
            }
        }
        _ = shutdown => {
            info!("Received shutdown signal");
        }
    }

    info!("Shutting down tantivy-mcp");

    Ok(())
}

/// Setup graceful shutdown handler.
fn setup_shutdown_handler() -> impl std::future::Future<Output = ()> {
    async {
        #[cfg(unix)]
        {
            let ctrl_c = async {
                signal::ctrl_c()
                    .await
                    .expect("Failed to install Ctrl+C handler");
            };

            let terminate = async {
                signal::unix::signal(signal::unix::SignalKind::terminate())
                    .expect("Failed to install signal handler")
                    .recv()
                    .await;
            };

            tokio::select! {
                _ = ctrl_c => {},
                _ = terminate => {},
            }
        }

        #[cfg(not(unix))]
        {
            signal::ctrl_c()
                .await
                .expect("Failed to install Ctrl+C handler");
        }
    }
}
