//! Webhook HTTP server implementation
//!
//! Uses Axum's native routing directly for each platform.

use std::net::SocketAddr;

use axum::{response::IntoResponse, routing::get, Router};
use tower_http::trace::TraceLayer;
use tracing::info;

use super::types::WebhookError;

/// Configuration for the webhook server
#[derive(Debug, Clone)]
pub struct WebhookConfig {
    /// Port to listen on
    pub port: u16,

    /// Host to bind to
    pub host: String,
}

impl Default for WebhookConfig {
    fn default() -> Self {
        Self {
            port: 3000,
            host: "0.0.0.0".to_string(),
        }
    }
}

/// Webhook HTTP server for handling callbacks from messaging platforms
pub struct WebhookServer {
    config: WebhookConfig,
    router: Router,
}

impl WebhookServer {
    /// Create a new webhook server with the given configuration
    pub fn new(config: WebhookConfig) -> Self {
        Self {
            config,
            router: Router::new().route("/health", get(health_check)),
        }
    }

    /// Add a router for a specific platform
    pub fn add_router(mut self, router: Router) -> Self {
        self.router = self.router.merge(router);
        self
    }

    /// Start the webhook server
    ///
    /// This method blocks until the server is shut down.
    pub async fn start(self) -> Result<(), WebhookError> {
        let addr: SocketAddr = format!("{}:{}", self.config.host, self.config.port)
            .parse()
            .map_err(|e| WebhookError::ConfigError(format!("Invalid address: {}", e)))?;

        let router = self.router.layer(TraceLayer::new_for_http());

        info!("Starting webhook server on {}", addr);

        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, router).await?;

        Ok(())
    }

    /// Start the webhook server with graceful shutdown
    ///
    /// Returns a handle that can be used to trigger shutdown.
    pub async fn start_with_shutdown(
        self,
        mut shutdown_rx: tokio::sync::broadcast::Receiver<()>,
    ) -> Result<(), WebhookError> {
        let addr: SocketAddr = format!("{}:{}", self.config.host, self.config.port)
            .parse()
            .map_err(|e| WebhookError::ConfigError(format!("Invalid address: {}", e)))?;

        let router = self.router.layer(TraceLayer::new_for_http());

        info!("Starting webhook server on {}", addr);

        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.recv().await;
                info!("Webhook server shutting down");
            })
            .await?;

        Ok(())
    }
}

/// Health check endpoint
async fn health_check() -> impl IntoResponse {
    (axum::http::StatusCode::OK, "OK")
}
