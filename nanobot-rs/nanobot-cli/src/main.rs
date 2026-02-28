//! nanobot CLI

use anyhow::Result;
use clap::Parser;
use opentelemetry::trace::TracerProvider;
use opentelemetry_otlp::WithExportConfig;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, EnvFilter};

mod cli;
mod commands;
mod provider;

use cli::{AuthCommands, ChannelsCommands, Cli, Commands};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging and OpenTelemetry
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));

    // Try to initialize OpenTelemetry, fall back to plain logging if unavailable
    if !init_telemetry(env_filter.clone()) {
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .with_level(true)
            .with_ansi(true)
            .init();
    }

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Onboard) => commands::cmd_onboard().await,
        Some(Commands::Status) => commands::cmd_status().await,
        Some(Commands::Agent {
            message,
            logs,
            no_markdown,
            thinking,
            no_stream,
        }) => commands::cmd_agent(message, logs, no_markdown, thinking, no_stream).await,
        Some(Commands::Gateway) => commands::cmd_gateway().await,
        Some(Commands::Channels { command }) => match command {
            ChannelsCommands::Status => commands::cmd_channels_status().await,
        },
        Some(Commands::Auth { command }) => match command {
            AuthCommands::Copilot { pat, client_id } => {
                commands::cmd_auth_copilot(pat, client_id).await
            }
            AuthCommands::Status => commands::cmd_auth_status().await,
        },
        None => {
            // No command - show help
            println!("🐈 nanobot v2.0.0 - A lightweight AI assistant\n");
            println!("Usage: nanobot <COMMAND>\n");
            println!("Commands:");
            println!("  onboard   Initialize configuration");
            println!("  status    Show status");
            println!("  agent     Chat with the agent");
            println!("  channels  Manage chat channels");
            println!("  gateway   Start the gateway");
            println!("  auth      Authentication commands\n");
            println!("Run 'nanobot --help' for more information.");
            Ok(())
        }
    }
}

/// Initialize OpenTelemetry tracing (optional).
///
/// Only initializes OpenTelemetry when explicitly configured via environment
/// variables. Defaults to no exporter (logging only).
///
/// Environment variables:
/// - `OTEL_EXPORTER_OTLP_ENDPOINT`: OTLP endpoint URL (e.g., http://localhost:4317)
/// - `OTEL_SDK_DISABLED=true`: Disable OpenTelemetry completely
fn init_telemetry(env_filter: EnvFilter) -> bool {
    use tracing_subscriber::util::SubscriberInitExt;

    // Check if OTEL is disabled
    if std::env::var("OTEL_SDK_DISABLED").is_ok_and(|v| v == "true") {
        return false;
    }

    // Only initialize if endpoint is explicitly configured
    let endpoint = match std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT") {
        Ok(e) => e,
        Err(_) => return false, // No endpoint configured, skip OpenTelemetry
    };

    // Try to create OTLP exporter
    let exporter = match opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(format!("{}/v1/traces", endpoint))
        .build()
    {
        Ok(e) => e,
        Err(_) => return false,
    };

    // Create tracer provider
    let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .build();

    let tracer = provider.tracer("nanobot");

    // Set global tracer provider
    opentelemetry::global::set_tracer_provider(provider);

    // Create tracing layer and initialize
    let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

    tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer())
        .with(otel_layer)
        .init();

    info!("OpenTelemetry tracing enabled: {}", endpoint);
    true
}
