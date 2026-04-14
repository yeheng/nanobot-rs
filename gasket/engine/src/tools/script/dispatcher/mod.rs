//! RPC dispatcher for script tools.
//!
//! This module provides the dispatcher that routes JSON-RPC method calls to
//! registered handlers with permission enforcement. It's the core routing layer
//! that connects external script processes to Gasket's engine capabilities.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::tools::ToolRegistry;
use gasket_providers::LlmProvider;

use super::manifest::Permission;
use super::rpc::{RpcError, RpcRequest, RpcResponse};

/// Trait for RPC method handlers.
///
/// Handlers are registered with the dispatcher and invoked when matching
/// method names are called. Each handler declares its required permission
/// which is checked before execution.
#[async_trait]
pub trait RpcHandler: Send + Sync {
    /// Get the method name this handler responds to.
    fn method(&self) -> &str;

    /// Get the permission required to call this handler.
    fn required_permission(&self) -> Permission;

    /// Handle an RPC request.
    ///
    /// # Arguments
    ///
    /// * `params` - The parameters from the RPC request (can be null/array/object)
    /// * `ctx` - The dispatcher context with engine capabilities
    ///
    /// # Returns
    ///
    /// - `Ok(Value)` - Successful result (will be wrapped in RpcResponse)
    /// - `Err(RpcError)` - Error response
    async fn handle(&self, params: Value, ctx: &DispatcherContext) -> Result<Value, RpcError>;
}

/// Context provided to RPC handlers during execution.
///
/// Contains references to engine capabilities that handlers may need.
/// All fields are optional to allow flexible handler registration.
#[derive(Clone)]
pub struct DispatcherContext {
    /// Session identifier for the current session
    pub session_key: Option<gasket_types::events::SessionKey>,

    /// Channel for sending outbound messages
    pub outbound_tx: Option<tokio::sync::mpsc::Sender<gasket_types::events::OutboundMessage>>,

    /// Subagent spawner for delegating to specialized agents
    pub spawner: Option<Arc<dyn gasket_types::SubagentSpawner>>,

    /// Token usage tracker for LLM calls
    pub token_tracker: Option<Arc<gasket_types::token_tracker::TokenTracker>>,

    /// Tool registry for executing engine tools
    pub tool_registry: Option<Arc<ToolRegistry>>,

    /// LLM provider for direct chat completions
    pub provider: Option<Arc<dyn LlmProvider>>,
}

// Manual Default implementation - Arc<dyn Trait> cannot derive Default
impl Default for DispatcherContext {
    fn default() -> Self {
        Self {
            session_key: None,
            outbound_tx: None,
            spawner: None,
            token_tracker: None,
            tool_registry: None,
            provider: None,
        }
    }
}

/// RPC dispatcher that routes method calls to handlers with permission checks.
///
/// The dispatcher maintains a registry of method handlers and provides:
/// - Method registration and lookup
/// - Permission validation before execution
/// - Standard error responses for common failures
pub struct RpcDispatcher {
    /// Registered handlers indexed by method name
    handlers: HashMap<String, Arc<dyn RpcHandler>>,
}

impl RpcDispatcher {
    /// Create a new empty dispatcher.
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    /// Register a handler for a specific method.
    ///
    /// # Arguments
    ///
    /// * `handler` - The handler to register
    ///
    /// # Panics
    ///
    /// Panics if a handler is already registered for the same method name.
    pub fn register(&mut self, handler: Arc<dyn RpcHandler>) {
        let method = handler.method().to_string();
        if self.handlers.contains_key(&method) {
            panic!("Handler already registered for method: {}", method);
        }
        self.handlers.insert(method, handler);
    }

    /// Dispatch an RPC request to the appropriate handler.
    ///
    /// # Arguments
    ///
    /// * `request` - The RPC request to dispatch
    /// * `permissions` - Permissions granted to the calling script
    /// * `ctx` - The dispatcher context
    ///
    /// # Returns
    ///
    /// An RPC response. For notifications (id=None), the response id will be Value::Null.
    ///
    /// # Error Handling
    ///
    /// - Method not found: returns error code -32601
    /// - Permission denied: returns error code -32000
    /// - Handler error: returns error from handler
    pub async fn dispatch(
        &self,
        request: RpcRequest,
        permissions: &[Permission],
        ctx: &DispatcherContext,
    ) -> RpcResponse {
        // Extract request id (use Value::Null for notifications)
        let id = request.id.clone().unwrap_or(Value::Null);

        // Find handler by method name
        let handler = match self.handlers.get(&request.method) {
            Some(h) => h,
            None => {
                return RpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id,
                    result: None,
                    error: Some(RpcError::method_not_found(&request.method)),
                };
            }
        };

        // Check permissions
        let required = handler.required_permission();
        if !permissions.contains(&required) {
            return RpcResponse {
                jsonrpc: "2.0".to_string(),
                id,
                result: None,
                error: Some(RpcError::permission_denied(&request.method)),
            };
        }

        // Execute handler
        let params = request.params.unwrap_or(Value::Null);
        match handler.handle(params, ctx).await {
            Ok(result) => RpcResponse {
                jsonrpc: "2.0".to_string(),
                id,
                result: Some(result),
                error: None,
            },
            Err(error) => RpcResponse {
                jsonrpc: "2.0".to_string(),
                id,
                result: None,
                error: Some(error),
            },
        }
    }
}

impl Default for RpcDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

/// Build and return an empty RPC dispatcher.
///
/// This is a placeholder function that will be expanded in future tasks
/// to register default handlers (llm/chat, memory/search, etc.).
pub fn build_dispatcher() -> RpcDispatcher {
    RpcDispatcher::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Mock handler for testing that echoes back the params.
    struct EchoHandler {
        method: String,
        permission: Permission,
    }

    #[async_trait]
    impl RpcHandler for EchoHandler {
        fn method(&self) -> &str {
            &self.method
        }

        fn required_permission(&self) -> Permission {
            self.permission.clone()
        }

        async fn handle(&self, params: Value, _ctx: &DispatcherContext) -> Result<Value, RpcError> {
            Ok(json!({"echo": params}))
        }
    }

    #[tokio::test]
    async fn test_dispatch_success() {
        let mut dispatcher = RpcDispatcher::new();
        let handler = Arc::new(EchoHandler {
            method: "test/echo".to_string(),
            permission: Permission::LlmChat,
        });
        dispatcher.register(handler);

        let request = RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "test/echo".to_string(),
            params: Some(json!({"hello": "world"})),
        };

        let permissions = vec![Permission::LlmChat];
        let ctx = DispatcherContext::default();
        let response = dispatcher.dispatch(request, &permissions, &ctx).await;

        assert_eq!(response.id, json!(1));
        assert!(response.result.is_some());
        assert!(response.error.is_none());
        assert_eq!(response.result.unwrap(), json!({"echo": {"hello": "world"}}));
    }

    #[tokio::test]
    async fn test_dispatch_permission_denied() {
        let mut dispatcher = RpcDispatcher::new();
        let handler = Arc::new(EchoHandler {
            method: "test/echo".to_string(),
            permission: Permission::LlmChat,
        });
        dispatcher.register(handler);

        let request = RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(2)),
            method: "test/echo".to_string(),
            params: Some(json!({"test": "data"})),
        };

        // Empty permissions = deny all
        let permissions = vec![];
        let ctx = DispatcherContext::default();
        let response = dispatcher.dispatch(request, &permissions, &ctx).await;

        assert_eq!(response.id, json!(2));
        assert!(response.result.is_none());
        assert!(response.error.is_some());
        let error = response.error.unwrap();
        assert_eq!(error.code, -32000);
        assert!(error.message.contains("Permission denied"));
    }

    #[tokio::test]
    async fn test_dispatch_method_not_found() {
        let dispatcher = RpcDispatcher::new();

        let request = RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(3)),
            method: "unknown/method".to_string(),
            params: None,
        };

        let permissions = vec![Permission::LlmChat];
        let ctx = DispatcherContext::default();
        let response = dispatcher.dispatch(request, &permissions, &ctx).await;

        assert_eq!(response.id, json!(3));
        assert!(response.result.is_none());
        assert!(response.error.is_some());
        let error = response.error.unwrap();
        assert_eq!(error.code, -32601);
        assert!(error.message.contains("Method not found"));
    }

    #[tokio::test]
    async fn test_dispatch_no_id() {
        let mut dispatcher = RpcDispatcher::new();
        let handler = Arc::new(EchoHandler {
            method: "test/notify".to_string(),
            permission: Permission::MemorySearch,
        });
        dispatcher.register(handler);

        // Notification: no id field
        let request = RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: None,
            method: "test/notify".to_string(),
            params: Some(json!({"event": "test"})),
        };

        let permissions = vec![Permission::MemorySearch];
        let ctx = DispatcherContext::default();
        let response = dispatcher.dispatch(request, &permissions, &ctx).await;

        // Notifications should get Value::Null as response id
        assert_eq!(response.id, Value::Null);
        assert!(response.result.is_some());
        assert!(response.error.is_none());
        assert_eq!(response.result.unwrap(), json!({"echo": {"event": "test"}}));
    }
}
