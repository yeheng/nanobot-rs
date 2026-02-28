//! CLI 结构定义

use clap::{Parser, Subcommand};

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
    Agent {
        /// Message to send (if not provided, enters interactive mode)
        #[arg(short, long)]
        message: Option<String>,

        /// Show logs during chat
        #[arg(long)]
        logs: bool,

        /// Disable Markdown rendering
        #[arg(long)]
        no_markdown: bool,

        /// Enable thinking/reasoning mode for deep reasoning models
        #[arg(long)]
        thinking: bool,

        /// Disable streaming output (stream is enabled by default)
        #[arg(long)]
        no_stream: bool,
    },

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
