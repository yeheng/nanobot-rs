//! CLI 结构定义

use clap::{Args, Parser, Subcommand};

/// 🐈 nanobot - A lightweight AI assistant
#[derive(Parser)]
#[command(name = "nanobot")]
#[command(version = "2.0.0")]
#[command(about = "A lightweight personal AI assistant", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize configuration
    Onboard,

    /// Show status
    Status,

    /// Chat with the agent
    Agent(AgentOptions),

    /// Start the gateway (for chat channels)
    Gateway,

    /// Manage chat channels
    Channels {
        #[command(subcommand)]
        command: ChannelsCommands,
    },

    /// Authentication commands
    Auth {
        #[command(subcommand)]
        command: AuthCommands,
    },

    /// Manage scheduled tasks
    Cron {
        #[command(subcommand)]
        command: CronCommands,
    },

    /// Manage Tantivy search indexes
    Search {
        #[command(subcommand)]
        command: SearchCommands,
    },

    /// Show session token usage and cost statistics
    Stats,
}

/// Options for the `agent` command.
#[derive(Args, Debug)]
pub struct AgentOptions {
    /// Message to send (if not provided, enters interactive mode)
    #[arg(short, long)]
    pub message: Option<String>,

    /// Show logs during chat
    #[arg(long)]
    pub logs: bool,

    /// Disable Markdown rendering
    #[arg(long)]
    pub no_markdown: bool,

    /// Enable thinking/reasoning mode for deep reasoning models
    #[arg(long)]
    pub thinking: bool,

    /// Disable streaming output (stream is enabled by default)
    #[arg(long)]
    pub no_stream: bool,
}

#[derive(Subcommand)]
pub enum ChannelsCommands {
    /// Show status of all configured channels
    Status,
}

#[derive(Subcommand)]
pub enum AuthCommands {
    /// Login to GitHub Copilot using OAuth Device Flow
    Copilot {
        /// GitHub Personal Access Token (skip OAuth flow)
        #[arg(short, long)]
        pat: Option<String>,

        /// GitHub App Client ID (uses default if not specified)
        #[arg(short, long)]
        client_id: Option<String>,
    },

    /// Show authentication status for all providers
    Status,
}

#[derive(Subcommand)]
pub enum CronCommands {
    /// List all scheduled jobs
    List,

    /// Add a new scheduled job
    Add {
        /// Job name
        #[arg(short, long)]
        name: String,

        /// Cron expression (e.g., '0 9 * * *' for 9 AM daily)
        #[arg(short, long)]
        cron: String,

        /// Message to send at scheduled time
        #[arg(short, long)]
        message: String,
    },

    /// Remove a scheduled job
    Remove {
        /// Job ID to remove
        id: String,
    },

    /// Show details of a scheduled job
    Show {
        /// Job ID to show
        id: String,
    },

    /// Enable a scheduled job
    Enable {
        /// Job ID to enable
        id: String,
    },

    /// Disable a scheduled job
    Disable {
        /// Job ID to disable
        id: String,
    },
}

#[derive(Subcommand)]
pub enum SearchCommands {
    /// Rebuild search indexes from scratch
    Rebuild {
        /// Index type: "memory", "history", or "all" (default: all)
        #[arg(short, long, default_value = "all")]
        r#type: String,
    },

    /// Incrementally update search indexes
    Update {
        /// Index type: "memory", "history", or "all" (default: all)
        #[arg(short, long, default_value = "all")]
        r#type: String,
    },

    /// Show index statistics
    Status,
}
