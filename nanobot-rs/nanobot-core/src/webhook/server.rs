//! Webhook HTTP server implementation
//!
//! Uses Axum's native routing with path-based handler dispatch.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
use tower_http::trace::TraceLayer;
use tracing::{debug, error, info};

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
    /// Map from path to handler
    handlers: Arc<HashMap<String, Arc<dyn super::types::WebhookHandler>>>,
}

/// Webhook HTTP server for handling callbacks from messaging platforms
pub struct WebhookServer {
    config: WebhookConfig,
    handlers: HashMap<String, BoxedWebhookHandler>,
}

impl WebhookServer {
    /// Create a new webhook server with the given configuration
    pub fn new(config: WebhookConfig) -> Self {
        Self {
            config,
            handlers: HashMap::new(),
        }
    }

    /// Add a webhook handler
    pub fn add_handler(mut self, handler: BoxedWebhookHandler) -> Self {
        let path = handler.path().to_string();
        self.handlers.insert(path, handler);
        self
    }

    /// Add multiple webhook handlers
    pub fn add_handlers(mut self, handlers: Vec<BoxedWebhookHandler>) -> Self {
        for handler in handlers {
            let path = handler.path().to_string();
            self.handlers.insert(path, handler);
        }
        self
    }

    /// Build the router with all registered handlers using native Axum routing
    fn build_router(self) -> Router {
        // Convert to Arc HashMap for shared state
        let handlers_map: HashMap<String, Arc<dyn super::types::WebhookHandler>> =
            self.handlers.into_iter().collect();

        let state = Arc::new(WebhookState {
            handlers: Arc::new(handlers_map),
        });

        // Build router with catch-all path for webhooks
        Router::new()
            .route("/health", get(health_check))
            .route("/webhook/{*path}", get(handle_get).post(handle_post))
            .route("/{*path}", get(handle_get).post(handle_post))
            .with_state(state)
            .layer(TraceLayer::new_for_http())
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

/// Find handler for the given path via exact match on the handlers map.
fn find_handler<'a>(
    state: &'a WebhookState,
    path: &str,
) -> Option<&'a Arc<dyn super::types::WebhookHandler>> {
    // Normalize path
    let normalized = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{}", path)
    };

    // Exact match
    if let Some(handler) = state.handlers.get(&normalized) {
        return Some(handler);
    }

    // Try without leading slash
    state.handlers.get(path)
}

/// Generic GET handler for webhooks
async fn handle_get(
    State(state): State<Arc<WebhookState>>,
    Path(path): Path<String>,
    Query(query): Query<serde_json::Value>,
) -> impl IntoResponse {
    debug!("Webhook GET request on /{}", path);

    match find_handler(&state, &path) {
        Some(handler) => match handler.handle_get(Query(query)).await {
            Ok(response) => response,
            Err(e) => {
                error!("Webhook GET error: {}", e);
                (StatusCode::INTERNAL_SERVER_ERROR, format!("Error: {}", e)).into_response()
            }
        },
        None => {
            debug!("No handler found for /{}", path);
            (StatusCode::NOT_FOUND, "Not Found").into_response()
        }
    }
}

/// Generic POST handler for webhooks
async fn handle_post(
    State(state): State<Arc<WebhookState>>,
    Path(path): Path<String>,
    headers: HeaderMap,
    Query(query): Query<serde_json::Value>,
    body: bytes::Bytes,
) -> impl IntoResponse {
    debug!("Webhook POST request on /{}", path);

    match find_handler(&state, &path) {
        Some(handler) => match handler.handle_post(headers, Query(query), body).await {
            Ok(response) => response,
            Err(e) => {
                error!("Webhook POST error: {}", e);
                (StatusCode::INTERNAL_SERVER_ERROR, format!("Error: {}", e)).into_response()
            }
        },
        None => {
            debug!("No handler found for /{}", path);
            (StatusCode::NOT_FOUND, "Not Found").into_response()
        }
    }
}
