//! Pipeline hook trait and registry
//!
//! This module provides:
//! - `PipelineHook`: Trait for implementing lifecycle hooks
//! - `HookRegistry`: Registry for managing and executing hooks
//! - `HookBuilder`: Builder for convenient registry construction

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tracing::{debug, warn};

use super::{ExecutionStrategy, HookAction, HookPoint, MutableContext, ReadonlyContext};
use crate::error::AgentError;

// ── PipelineHook Trait ────────────────────────────────────

/// Trait for implementing lifecycle hooks in the agent pipeline.
///
/// Hooks can be attached at various points in the agent execution pipeline
/// to intercept, modify, or react to events.
///
/// # Execution Strategies
///
/// Hooks at different points have different execution strategies:
/// - **Sequential** (BeforeRequest, AfterHistory, BeforeLLM):
///   Hooks run one after another, can modify messages, can abort.
///   Use the `run()` method.
/// - **Parallel** (AfterToolCall, AfterResponse):
///   Hooks run concurrently with readonly access.
///   Use the `run_parallel()` method.
///
/// # Example
///
/// ```rust,ignore
/// use async_trait::async_trait;
/// use gasket_core::hooks::{PipelineHook, HookPoint, MutableContext, HookAction};
/// use gasket_core::error::AgentError;
///
/// struct LoggingHook;
///
/// #[async_trait]
/// impl PipelineHook for LoggingHook {
///     fn name(&self) -> &str { "logging" }
///     fn point(&self) -> HookPoint { HookPoint::BeforeRequest }
///
///     async fn run(&self, ctx: &mut MutableContext<'_>) -> Result<HookAction, AgentError> {
///         println!("Request received: {:?}", ctx.user_input);
///         Ok(HookAction::Continue)
///     }
/// }
/// ```
#[async_trait]
pub trait PipelineHook: Send + Sync {
    /// Returns the hook name for logging and debugging.
    fn name(&self) -> &str;

    /// Returns the execution point where this hook should be attached.
    fn point(&self) -> HookPoint;

    /// Sequential execution - can modify messages.
    ///
    /// Called for hooks at Sequential points (BeforeRequest, AfterHistory, BeforeLLM).
    /// The hook can modify the messages in the context and return an action
    /// to continue or abort processing.
    ///
    /// Default implementation returns `HookAction::Continue`.
    async fn run(&self, _ctx: &mut MutableContext<'_>) -> Result<HookAction, AgentError> {
        Ok(HookAction::Continue)
    }

    /// Parallel execution - readonly access.
    ///
    /// Called for hooks at Parallel points (AfterToolCall, AfterResponse).
    /// The hook receives readonly access to the context and cannot modify messages.
    /// Multiple hooks at the same point run concurrently.
    ///
    /// Default implementation returns `HookAction::Continue`.
    async fn run_parallel(&self, _ctx: &ReadonlyContext<'_>) -> Result<HookAction, AgentError> {
        Ok(HookAction::Continue)
    }
}

// ── HookRegistry ───────────────────────────────────────────

/// Registry for managing and executing lifecycle hooks.
///
/// The registry stores hooks organized by their execution point and
/// handles dispatching to the appropriate hooks during pipeline execution.
///
/// # Example
///
/// ```rust,ignore
/// use std::sync::Arc;
/// use gasket_core::hooks::{HookRegistry, HookBuilder, PipelineHook};
///
/// // Create a registry with hooks
/// let registry = HookBuilder::new()
///     .with_hook(Arc::new(MyHook))
///     .build_shared();
///
/// // Or create an empty registry for subagents
/// let empty = HookRegistry::empty();
/// ```
pub struct HookRegistry {
    hooks: HashMap<HookPoint, Vec<Arc<dyn PipelineHook>>>,
}

impl HookRegistry {
    /// Creates a new empty registry.
    pub fn new() -> Self {
        Self {
            hooks: HashMap::new(),
        }
    }

    /// Creates an empty registry wrapped in Arc (for subagents).
    pub fn empty() -> Arc<Self> {
        Arc::new(Self::new())
    }

    /// Registers a hook at its designated execution point.
    pub fn register(&mut self, hook: Arc<dyn PipelineHook>) {
        self.hooks.entry(hook.point()).or_default().push(hook);
    }

    /// Returns the hooks registered at a specific point.
    pub fn get_hooks(&self, point: HookPoint) -> &[Arc<dyn PipelineHook>] {
        self.hooks.get(&point).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Returns true if there are no hooks registered.
    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }

    /// Returns the total number of hooks registered.
    pub fn len(&self) -> usize {
        self.hooks.values().map(|v| v.len()).sum()
    }

    /// Executes hooks at a specific point.
    ///
    /// For Sequential points, hooks are executed one after another,
    /// and each hook can modify the context. If any hook returns
    /// `HookAction::Abort`, execution stops immediately.
    ///
    /// For Parallel points, hooks are executed concurrently with
    /// readonly access to the context.
    pub async fn execute(
        &self,
        point: HookPoint,
        ctx: &mut MutableContext<'_>,
    ) -> Result<HookAction, AgentError> {
        let hooks = self.get_hooks(point);
        if hooks.is_empty() {
            return Ok(HookAction::Continue);
        }

        match point.default_strategy() {
            ExecutionStrategy::Sequential => self.execute_sequential(hooks, ctx).await,
            ExecutionStrategy::Parallel => {
                // For parallel execution, we need a readonly view
                // Create a readonly context from the mutable one
                let session_key = ctx.session_key;
                let messages: &[crate::providers::ChatMessage] = ctx.messages;
                let user_input = ctx.user_input;
                let response = ctx.response;
                let tool_calls = ctx.tool_calls;
                let token_usage = ctx.token_usage;

                let readonly = ReadonlyContext {
                    session_key,
                    messages,
                    user_input,
                    response,
                    tool_calls,
                    token_usage,
                };

                self.execute_parallel(hooks, &readonly).await
            }
        }
    }

    /// Execute hooks sequentially, allowing modifications.
    async fn execute_sequential(
        &self,
        hooks: &[Arc<dyn PipelineHook>],
        ctx: &mut MutableContext<'_>,
    ) -> Result<HookAction, AgentError> {
        for hook in hooks {
            debug!("[Hook] Running {} at {:?}", hook.name(), hook.point());
            let action = hook.run(ctx).await?;
            if let HookAction::Abort(msg) = action {
                warn!("[Hook] {} aborted: {}", hook.name(), msg);
                return Ok(HookAction::Abort(msg));
            }
        }
        Ok(HookAction::Continue)
    }

    /// Execute hooks in parallel with readonly access.
    async fn execute_parallel(
        &self,
        hooks: &[Arc<dyn PipelineHook>],
        ctx: &ReadonlyContext<'_>,
    ) -> Result<HookAction, AgentError> {
        let results = futures::future::join_all(
            hooks
                .iter()
                .map(|h| {
                    debug!("[Hook] Running {} at {:?}", h.name(), h.point());
                    h.run_parallel(ctx)
                }),
        )
        .await;

        for result in results {
            if let Ok(HookAction::Abort(msg)) = result {
                return Ok(HookAction::Abort(msg));
            }
        }
        Ok(HookAction::Continue)
    }
}

impl Default for HookRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ── HookBuilder ────────────────────────────────────────────

/// Builder for creating a HookRegistry with a fluent API.
///
/// # Example
///
/// ```rust,ignore
/// use std::sync::Arc;
/// use gasket_core::hooks::HookBuilder;
///
/// let registry = HookBuilder::new()
///     .with_hook(Arc::new(Hook1))
///     .with_hook(Arc::new(Hook2))
///     .build_shared();
/// ```
pub struct HookBuilder {
    hooks: Vec<Arc<dyn PipelineHook>>,
}

impl HookBuilder {
    /// Creates a new empty builder.
    pub fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    /// Adds a hook to the builder.
    pub fn with_hook(mut self, hook: Arc<dyn PipelineHook>) -> Self {
        self.hooks.push(hook);
        self
    }

    /// Builds the HookRegistry from the registered hooks.
    pub fn build(self) -> HookRegistry {
        let mut registry = HookRegistry::new();
        for hook in self.hooks {
            registry.register(hook);
        }
        registry
    }

    /// Builds the HookRegistry wrapped in Arc for sharing.
    pub fn build_shared(self) -> Arc<HookRegistry> {
        Arc::new(self.build())
    }
}

impl Default for HookBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ChatMessage;

    // Test hook that records execution
    struct TestHook {
        name: String,
        point: HookPoint,
    }

    #[async_trait]
    impl PipelineHook for TestHook {
        fn name(&self) -> &str {
            &self.name
        }
        fn point(&self) -> HookPoint {
            self.point
        }

        async fn run(&self, ctx: &mut MutableContext<'_>) -> Result<HookAction, AgentError> {
            ctx.messages.push(ChatMessage::assistant(format!(
                "hook {} executed",
                self.name
            )));
            Ok(HookAction::Continue)
        }
    }

    // Test hook that aborts
    struct AbortHook {
        name: String,
    }

    #[async_trait]
    impl PipelineHook for AbortHook {
        fn name(&self) -> &str {
            &self.name
        }
        fn point(&self) -> HookPoint {
            HookPoint::BeforeRequest
        }

        async fn run(&self, _ctx: &mut MutableContext<'_>) -> Result<HookAction, AgentError> {
            Ok(HookAction::Abort("test abort".to_string()))
        }
    }

    // Test hook for parallel execution
    struct ParallelHook {
        name: String,
    }

    #[async_trait]
    impl PipelineHook for ParallelHook {
        fn name(&self) -> &str {
            &self.name
        }
        fn point(&self) -> HookPoint {
            HookPoint::AfterResponse
        }

        async fn run_parallel(&self, _ctx: &ReadonlyContext<'_>) -> Result<HookAction, AgentError> {
            Ok(HookAction::Continue)
        }
    }

    #[test]
    fn test_registry_empty() {
        let registry = HookRegistry::new();
        assert!(registry.get_hooks(HookPoint::BeforeRequest).is_empty());
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn test_registry_register() {
        let mut registry = HookRegistry::new();
        registry.register(Arc::new(TestHook {
            name: "test".to_string(),
            point: HookPoint::BeforeRequest,
        }));
        assert_eq!(registry.get_hooks(HookPoint::BeforeRequest).len(), 1);
        assert!(!registry.is_empty());
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn test_registry_multiple_hooks_same_point() {
        let mut registry = HookRegistry::new();
        registry.register(Arc::new(TestHook {
            name: "hook1".to_string(),
            point: HookPoint::BeforeRequest,
        }));
        registry.register(Arc::new(TestHook {
            name: "hook2".to_string(),
            point: HookPoint::BeforeRequest,
        }));
        assert_eq!(registry.get_hooks(HookPoint::BeforeRequest).len(), 2);
    }

    #[test]
    fn test_registry_hooks_different_points() {
        let mut registry = HookRegistry::new();
        registry.register(Arc::new(TestHook {
            name: "before".to_string(),
            point: HookPoint::BeforeRequest,
        }));
        registry.register(Arc::new(TestHook {
            name: "after".to_string(),
            point: HookPoint::AfterResponse,
        }));
        assert_eq!(registry.get_hooks(HookPoint::BeforeRequest).len(), 1);
        assert_eq!(registry.get_hooks(HookPoint::AfterResponse).len(), 1);
        assert_eq!(registry.len(), 2);
    }

    #[tokio::test]
    async fn test_execute_sequential() {
        let mut registry = HookRegistry::new();
        registry.register(Arc::new(TestHook {
            name: "hook1".to_string(),
            point: HookPoint::BeforeRequest,
        }));
        registry.register(Arc::new(TestHook {
            name: "hook2".to_string(),
            point: HookPoint::BeforeRequest,
        }));

        let mut messages = vec![];
        let mut ctx = MutableContext {
            session_key: "test:123",
            messages: &mut messages,
            user_input: Some("test"),
            response: None,
            tool_calls: None,
            token_usage: None,
        };

        let result = registry
            .execute(HookPoint::BeforeRequest, &mut ctx)
            .await;
        assert!(matches!(result, Ok(HookAction::Continue)));
        assert_eq!(ctx.messages.len(), 2);
    }

    #[tokio::test]
    async fn test_execute_abort() {
        let mut registry = HookRegistry::new();
        registry.register(Arc::new(AbortHook {
            name: "abort".to_string(),
        }));

        let mut messages = vec![];
        let mut ctx = MutableContext {
            session_key: "test:123",
            messages: &mut messages,
            user_input: Some("test"),
            response: None,
            tool_calls: None,
            token_usage: None,
        };

        let result = registry
            .execute(HookPoint::BeforeRequest, &mut ctx)
            .await;
        assert!(matches!(result, Ok(HookAction::Abort(_))));
        if let Ok(HookAction::Abort(msg)) = result {
            assert_eq!(msg, "test abort");
        }
    }

    #[tokio::test]
    async fn test_execute_parallel() {
        let mut registry = HookRegistry::new();
        registry.register(Arc::new(ParallelHook {
            name: "parallel1".to_string(),
        }));
        registry.register(Arc::new(ParallelHook {
            name: "parallel2".to_string(),
        }));

        let mut messages = vec![ChatMessage::user("test")];
        let mut ctx = MutableContext {
            session_key: "test:123",
            messages: &mut messages,
            user_input: Some("test"),
            response: Some("response"),
            tool_calls: None,
            token_usage: None,
        };

        let result = registry
            .execute(HookPoint::AfterResponse, &mut ctx)
            .await;
        assert!(matches!(result, Ok(HookAction::Continue)));
    }

    #[test]
    fn test_hook_builder() {
        let registry = HookBuilder::new()
            .with_hook(Arc::new(TestHook {
                name: "hook1".to_string(),
                point: HookPoint::BeforeRequest,
            }))
            .with_hook(Arc::new(TestHook {
                name: "hook2".to_string(),
                point: HookPoint::AfterResponse,
            }))
            .build();

        assert_eq!(registry.len(), 2);
        assert_eq!(registry.get_hooks(HookPoint::BeforeRequest).len(), 1);
        assert_eq!(registry.get_hooks(HookPoint::AfterResponse).len(), 1);
    }

    #[test]
    fn test_hook_builder_shared() {
        let registry = HookBuilder::new()
            .with_hook(Arc::new(TestHook {
                name: "hook1".to_string(),
                point: HookPoint::BeforeRequest,
            }))
            .build_shared();

        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn test_registry_default() {
        let registry = HookRegistry::default();
        assert!(registry.is_empty());
    }

    #[test]
    fn test_hook_builder_default() {
        let builder = HookBuilder::default();
        let registry = builder.build();
        assert!(registry.is_empty());
    }

    #[tokio::test]
    async fn test_empty_registry_execute() {
        let registry = HookRegistry::new();

        let mut messages = vec![];
        let mut ctx = MutableContext {
            session_key: "test:123",
            messages: &mut messages,
            user_input: Some("test"),
            response: None,
            tool_calls: None,
            token_usage: None,
        };

        let result = registry
            .execute(HookPoint::BeforeRequest, &mut ctx)
            .await;
        assert!(matches!(result, Ok(HookAction::Continue)));
    }

    #[test]
    fn test_empty_arc() {
        let registry = HookRegistry::empty();
        assert!(registry.is_empty());
    }
}
