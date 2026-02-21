//! Webhook HTTP server implementation

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    extract::State,
    http::{Method, StatusCode, Uri},
    response::IntoResponse,
    routing::get,
    Router,
};
use tower_http::trace::TraceLayer;
use tracing::{debug, error, info, warn};

use super::types::{BoxedWebhookHandler, WebhookError, WebhookResult};

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

/// State shared across all request handlers
#[derive(Clone)]
pub struct WebhookState {
    pub handlers: Vec<BoxedWebhookHandler>,
}

/// Webhook HTTP server for handling callbacks from messaging platforms
pub struct WebhookServer {
    config: WebhookConfig,
    handlers: Vec<BoxedWebhookHandler>,
}

impl WebhookServer {
    /// Create a new webhook server with the given configuration
    pub fn new(config: WebhookConfig) -> Self {
        Self {
            config,
            handlers: Vec::new(),
        }
    }

    /// Add a webhook handler
    pub fn add_handler(mut self, handler: BoxedWebhookHandler) -> Self {
        self.handlers.push(handler);
        self
    }

    /// Add multiple webhook handlers
    pub fn add_handlers(mut self, handlers: Vec<BoxedWebhookHandler>) -> Self {
        self.handlers.extend(handlers);
        self
    }

    /// Build the router with all registered handlers
    fn build_router(&self) -> Router {
        let state = Arc::new(WebhookState {
            handlers: self.handlers.clone(),
        });

        let router = Router::new()
            .route("/health", get(health_check))
            .fallback(handle_request)
            .with_state(state)
            .layer(TraceLayer::new_for_http());

        router
    }

    /// Start the webhook server
    ///
    /// This method blocks until the server is shut down.
    pub async fn start(self) -> WebhookResult<()> {
        let addr: SocketAddr = format!("{}:{}", self.config.host, self.config.port)
            .parse()
            .map_err(|e| WebhookError::ConfigError(format!("Invalid address: {}", e)))?;

        let router = self.build_router();

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
    ) -> WebhookResult<()> {
        let addr: SocketAddr = format!("{}:{}", self.config.host, self.config.port)
            .parse()
            .map_err(|e| WebhookError::ConfigError(format!("Invalid address: {}", e)))?;

        let router = self.build_router();

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
    (StatusCode::OK, "OK")
}

/// Fallback handler that routes to the appropriate platform handler
async fn handle_request(
    State(state): State<Arc<WebhookState>>,
    method: Method,
    uri: Uri,
    headers: axum::http::HeaderMap,
    query: axum::extract::Query<serde_json::Value>,
    body: bytes::Bytes,
) -> impl IntoResponse {
    let path = uri.path();

    debug!("Received {} request on {}", method, path);

    // Find a handler that matches this path
    let handler = state.handlers.iter().find(|h| path.starts_with(h.path()));

    match handler {
        Some(h) => {
            let result = match method {
                Method::GET => h.handle_get(query).await,
                Method::POST => h.handle_post(headers, query, body).await,
                _ => {
                    warn!("Unsupported method {} for path {}", method, path);
                    Err(WebhookError::InvalidBody(format!(
                        "Unsupported method: {}",
                        method
                    )))
                }
            };

            match result {
                Ok(response) => response,
                Err(e) => {
                    error!("Webhook handler error for {}: {}", path, e);
                    (StatusCode::INTERNAL_SERVER_ERROR, format!("Error: {}", e)).into_response()
                }
            }
        }
        None => {
            warn!("No handler found for path {}", path);
            (StatusCode::NOT_FOUND, "Not Found").into_response()
        }
    }
}
