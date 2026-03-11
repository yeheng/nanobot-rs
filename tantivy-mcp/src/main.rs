//! Tantivy MCP - Standalone MCP Index Server
//!
//! A Model Context Protocol (MCP) server providing full-text search capabilities
//! using the Tantivy search engine.

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use directories::UserDirs;
use tantivy_mcp::index::IndexManager;
use tantivy_mcp::maintenance::{MaintenanceConfig, MaintenanceScheduler};
use tantivy_mcp::mcp::{McpHandler, ToolRegistry};
use tantivy_mcp::register_tools;
use tokio::signal;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
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
        UserDirs::new()
            .map(|dirs| dirs.home_dir().join(".nanobot/tantivy"))
            .unwrap_or_else(|| PathBuf::from(".nanobot/tantivy"))
    });

    info!("Index directory: {:?}", index_dir);

    // Create index manager
    let mut manager = IndexManager::new(&index_dir);

    // Load existing indexes
    manager.load_indexes()?;

    let manager = Arc::new(RwLock::new(manager));

    // Create cancellation token for graceful shutdown
    let cancel_token = CancellationToken::new();

    // Start maintenance scheduler if enabled
    let (scheduler_handle, scheduler_token) = if args.auto_maintain {
        let config = MaintenanceConfig {
            auto_compact: true,
            deleted_ratio_threshold: 0.2,
            max_segments: 10,
            auto_expire: true,
            expire_interval_secs: args.maintenance_interval,
        };
        let scheduler = MaintenanceScheduler::new(manager.clone(), config);
        let (handle, token) = scheduler.start();
        info!(
            "Maintenance scheduler started (interval: {}s)",
            args.maintenance_interval
        );
        (Some(handle), Some(token))
    } else {
        (None, None)
    };

    // Create tool registry and register tools
    let mut tools = ToolRegistry::new();
    register_tools(&mut tools, manager.clone());

    // Create MCP handler
    let mut handler = McpHandler::new(tools);

    // Set up graceful shutdown
    let shutdown_token = cancel_token.clone();
    let shutdown = setup_shutdown_handler();

    // Run MCP server in a separate task
    let server_token = cancel_token.clone();
    let server_task = tokio::spawn(async move {
        handler.run(server_token).await
    });

    // Wait for either server completion or shutdown signal
    tokio::select! {
        result = server_task => {
            // Cancel scheduler when server completes
            if let Some(ref token) = scheduler_token {
                token.cancel();
            }
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
            // Cancel all tasks
            shutdown_token.cancel();
            if let Some(token) = scheduler_token {
                token.cancel();
            }
        }
    }

    // Wait for server task to finish (with timeout)
    if let Ok(_handle) = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        async {
            // The server task should have already finished due to cancellation
            // but we wait for it to clean up properly
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    ).await {
        info!("Server shutdown complete");
    } else {
        tracing::warn!("Server shutdown timed out");
    }

    // Wait for maintenance scheduler to stop
    if let Some(handle) = scheduler_handle {
        // Scheduler should already be stopped via cancellation
        if let Ok(_) = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            handle
        ).await {
            info!("Maintenance scheduler stopped");
        } else {
            tracing::warn!("Maintenance scheduler stop timed out");
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
