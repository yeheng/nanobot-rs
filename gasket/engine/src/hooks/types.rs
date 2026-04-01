//! Core types for lifecycle hooks
//!
//! This module defines the fundamental types used in the hook system:
//! - `HookPoint`: Execution points in the agent pipeline
//! - `ExecutionStrategy`: How hooks are executed (Sequential vs Parallel)
//! - `HookAction`: Result of hook execution (Continue vs Abort)
//! - `HookContext`: Context passed to hooks with message access

use crate::token_tracker::TokenUsage;
use gasket_providers::ChatMessage;
use serde::{Deserialize, Serialize};

// ── HookPoint ─────────────────────────────────────────────

/// Execution points in the agent pipeline where hooks can be attached.
///
/// Each hook point has a default execution strategy that determines
/// whether hooks run sequentially (can modify messages) or in parallel
/// (readonly access).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookPoint {
    /// Before the request is processed - can modify user input
    BeforeRequest,
    /// After history is loaded - can add context messages
    AfterHistory,
    /// Before sending to LLM - last chance to modify messages
    BeforeLLM,
    /// After a tool call completes - logging, auditing
    AfterToolCall,
    /// After the response is generated - logging, notifications
    AfterResponse,
}

// ── ExecutionStrategy ─────────────────────────────────────

/// How hooks at a given point are executed.
///
/// - `Sequential`: Hooks run one after another, each receiving the
///   potentially modified messages from the previous hook. Can abort.
/// - `Parallel`: Hooks run concurrently with readonly access. Abort
///   is not supported (best-effort execution).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionStrategy {
    /// Hooks run one after another, can modify messages
    Sequential,
    /// Hooks run concurrently, readonly access only
    Parallel,
}

impl HookPoint {
    /// Returns the default execution strategy for this hook point.
    ///
    /// - `BeforeRequest`, `AfterHistory`, `BeforeLLM`: Sequential
    ///   (may modify messages, abort is meaningful)
    /// - `AfterToolCall`, `AfterResponse`: Parallel
    ///   (readonly, fire-and-forget style)
    pub fn default_strategy(&self) -> ExecutionStrategy {
        match self {
            Self::BeforeRequest => ExecutionStrategy::Sequential,
            Self::AfterHistory => ExecutionStrategy::Sequential,
            Self::BeforeLLM => ExecutionStrategy::Sequential,
            Self::AfterToolCall => ExecutionStrategy::Parallel,
            Self::AfterResponse => ExecutionStrategy::Parallel,
        }
    }
}

// ── HookAction ────────────────────────────────────────────

/// Result returned by a hook after execution.
#[derive(Debug, Clone)]
pub enum HookAction {
    /// Continue processing normally
    Continue,
    /// Abort the request with an error message
    Abort(String),
}

// ── HookContext (Generic) ─────────────────────────────────

/// Context passed to hooks during execution.
///
/// The generic parameter `M` allows for different message access patterns:
/// - `&'a mut Vec<ChatMessage>` for mutable access (Sequential hooks)
/// - `&'a [ChatMessage]` for readonly access (Parallel hooks)
pub struct HookContext<'a, M> {
    /// Session identifier (e.g., "telegram:12345")
    pub session_key: &'a str,
    /// Message list (mutable or readonly depending on hook point)
    pub messages: M,
    /// Original user input (if available)
    pub user_input: Option<&'a str>,
    /// Agent response (for AfterResponse hooks)
    pub response: Option<&'a str>,
    /// Tool calls made during this request
    pub tool_calls: Option<&'a [ToolCallInfo]>,
    /// Token usage statistics
    pub token_usage: Option<&'a TokenUsage>,
    /// Vault values collected during this request (for log redaction).
    ///
    /// Per-request owned storage — hooks (e.g., VaultHook) push plaintext
    /// secrets here so the caller can snapshot them after hook execution.
    /// This replaces the previous `Arc<RwLock<Vec<String>>>` shared state
    /// which was susceptible to cross-request race conditions.
    pub vault_values: Vec<String>,
}

/// Tool call information for hooks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallInfo {
    /// Unique identifier for the tool call
    pub id: String,
    /// Name of the tool that was called
    pub name: String,
    /// JSON-encoded arguments passed to the tool
    pub arguments: Option<String>,
}

/// Mutable context for Sequential hooks
pub type MutableContext<'a> = HookContext<'a, &'a mut Vec<ChatMessage>>;

/// Readonly context for Parallel hooks
pub type ReadonlyContext<'a> = HookContext<'a, &'a [ChatMessage]>;

impl<'a> MutableContext<'a> {
    /// Convert to a readonly context for parallel execution.
    ///
    /// This method consumes the mutable context and returns a readonly view
    /// of the same messages, allowing parallel hook execution.
    pub fn into_readonly(self) -> ReadonlyContext<'a> {
        HookContext {
            session_key: self.session_key,
            messages: self.messages,
            user_input: self.user_input,
            response: self.response,
            tool_calls: self.tool_calls,
            token_usage: self.token_usage,
            vault_values: self.vault_values,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hook_point_strategy() {
        assert_eq!(
            HookPoint::BeforeRequest.default_strategy(),
            ExecutionStrategy::Sequential
        );
        assert_eq!(
            HookPoint::AfterHistory.default_strategy(),
            ExecutionStrategy::Sequential
        );
        assert_eq!(
            HookPoint::BeforeLLM.default_strategy(),
            ExecutionStrategy::Sequential
        );
        assert_eq!(
            HookPoint::AfterToolCall.default_strategy(),
            ExecutionStrategy::Parallel
        );
        assert_eq!(
            HookPoint::AfterResponse.default_strategy(),
            ExecutionStrategy::Parallel
        );
    }

    #[test]
    fn test_hook_action_is_abort() {
        let action = HookAction::Abort("error".to_string());
        assert!(matches!(action, HookAction::Abort(_)));

        let action = HookAction::Continue;
        assert!(matches!(action, HookAction::Continue));
    }

    #[test]
    fn test_mutable_context_into_readonly() {
        let mut messages = vec![ChatMessage::user("test")];
        let ctx = MutableContext {
            session_key: "test:123",
            messages: &mut messages,
            user_input: Some("input"),
            response: None,
            tool_calls: None,
            token_usage: None,
            vault_values: Vec::new(),
        };

        let readonly = ctx.into_readonly();
        assert_eq!(readonly.session_key, "test:123");
        assert_eq!(readonly.messages.len(), 1);
    }

    #[test]
    fn test_tool_call_info_serialization() {
        let info = ToolCallInfo {
            id: "call_123".to_string(),
            name: "read_file".to_string(),
            arguments: Some(r#"{"path": "/test.txt"}"#.to_string()),
        };

        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("call_123"));
        assert!(json.contains("read_file"));

        let decoded: ToolCallInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.id, "call_123");
        assert_eq!(decoded.name, "read_file");
    }

    #[test]
    fn test_hook_point_equality() {
        assert_eq!(HookPoint::BeforeRequest, HookPoint::BeforeRequest);
        assert_ne!(HookPoint::BeforeRequest, HookPoint::AfterResponse);
    }

    #[test]
    fn test_execution_strategy_equality() {
        assert_eq!(ExecutionStrategy::Sequential, ExecutionStrategy::Sequential);
        assert_ne!(ExecutionStrategy::Sequential, ExecutionStrategy::Parallel);
    }
}
