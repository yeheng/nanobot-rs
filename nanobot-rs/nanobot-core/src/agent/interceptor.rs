//! Message interceptor middleware for pre-LLM processing.
//!
//! This module provides a middleware pattern for intercepting and modifying
//! messages before they are sent to the LLM. Interceptors are called in order
//! and can modify the message list in place.
//!
//! # Example
//!
//! ```ignore
//! use nanobot_core::agent::interceptor::{MessageInterceptor, InterceptReport};
//! use nanobot_core::providers::ChatMessage;
//!
//! struct MyInterceptor;
//!
//! impl MessageInterceptor for MyInterceptor {
//!     fn intercept(&self, messages: &mut Vec<ChatMessage>) -> Option<InterceptReport> {
//!         // Modify messages...
//!         Some(InterceptReport {
//!             name: "my_interceptor".to_string(),
//!             messages_modified: 1,
//!             details: "Added context".to_string(),
//!         })
//!     }
//! }
//! ```

use crate::providers::ChatMessage;

/// Report from an interceptor operation.
#[derive(Debug, Clone)]
pub struct InterceptReport {
    /// Name of the interceptor for logging
    pub name: String,
    /// Number of messages modified
    pub messages_modified: usize,
    /// Human-readable details about what was done
    pub details: String,
}

/// Trait for message interceptors that modify messages before LLM processing.
///
/// Interceptors are called in order before messages are sent to the LLM.
/// They can modify the message list in place, for example:
/// - Inject secrets/placeholders (VaultInjector)
/// - Add context or system messages
/// - Filter or transform content
///
/// Implementations must be `Send + Sync` for thread safety.
pub trait MessageInterceptor: Send + Sync {
    /// Intercept and optionally modify the message list.
    ///
    /// Returns `Some(InterceptReport)` if any modifications were made,
    /// or `None` if the interceptor is a no-op for this invocation.
    fn intercept(&self, messages: &mut Vec<ChatMessage>) -> Option<InterceptReport>;

    /// Get the name of this interceptor for logging.
    fn name(&self) -> &str;
}

/// A chain of interceptors that are executed in order.
pub struct InterceptorChain {
    interceptors: Vec<Box<dyn MessageInterceptor>>,
}

impl InterceptorChain {
    /// Create a new empty interceptor chain.
    pub fn new() -> Self {
        Self {
            interceptors: Vec::new(),
        }
    }

    /// Add an interceptor to the chain.
    pub fn add(&mut self, interceptor: Box<dyn MessageInterceptor>) {
        self.interceptors.push(interceptor);
    }

    /// Run all interceptors in order.
    ///
    /// Returns a vector of reports from interceptors that made modifications.
    pub fn run(&self, messages: &mut Vec<ChatMessage>) -> Vec<InterceptReport> {
        let mut reports = Vec::new();
        for interceptor in &self.interceptors {
            if let Some(report) = interceptor.intercept(messages) {
                tracing::debug!(
                    "[Interceptor] {} modified {} messages: {}",
                    report.name,
                    report.messages_modified,
                    report.details
                );
                reports.push(report);
            }
        }
        reports
    }

    /// Check if the chain is empty.
    pub fn is_empty(&self) -> bool {
        self.interceptors.is_empty()
    }

    /// Get the number of interceptors in the chain.
    pub fn len(&self) -> usize {
        self.interceptors.len()
    }
}

impl Default for InterceptorChain {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestInterceptor;

    impl MessageInterceptor for TestInterceptor {
        fn intercept(&self, messages: &mut Vec<ChatMessage>) -> Option<InterceptReport> {
            if messages.is_empty() {
                return None;
            }
            // Add a prefix to the first message
            if let Some(first) = messages.first_mut() {
                if let Some(ref content) = first.content {
                    first.content = Some(format!("[Modified] {}", content));
                    return Some(InterceptReport {
                        name: "test".to_string(),
                        messages_modified: 1,
                        details: "Added prefix".to_string(),
                    });
                }
            }
            None
        }

        fn name(&self) -> &str {
            "test_interceptor"
        }
    }

    struct NoOpInterceptor;

    impl MessageInterceptor for NoOpInterceptor {
        fn intercept(&self, _messages: &mut Vec<ChatMessage>) -> Option<InterceptReport> {
            None
        }

        fn name(&self) -> &str {
            "no_op"
        }
    }

    #[test]
    fn test_interceptor_chain_empty() {
        let chain = InterceptorChain::new();
        assert!(chain.is_empty());
        assert_eq!(chain.len(), 0);
    }

    #[test]
    fn test_interceptor_chain_add() {
        let mut chain = InterceptorChain::new();
        chain.add(Box::new(TestInterceptor));
        assert!(!chain.is_empty());
        assert_eq!(chain.len(), 1);
    }

    #[test]
    fn test_interceptor_chain_run() {
        let mut chain = InterceptorChain::new();
        chain.add(Box::new(TestInterceptor));

        let mut messages = vec![ChatMessage::user("Hello")];
        let reports = chain.run(&mut messages);

        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].name, "test");
        assert!(messages[0]
            .content
            .as_ref()
            .unwrap()
            .starts_with("[Modified]"));
    }

    #[test]
    fn test_interceptor_chain_no_op() {
        let mut chain = InterceptorChain::new();
        chain.add(Box::new(NoOpInterceptor));

        let mut messages = vec![ChatMessage::user("Hello")];
        let reports = chain.run(&mut messages);

        assert!(reports.is_empty());
        assert_eq!(messages[0].content, Some("Hello".to_string()));
    }

    #[test]
    fn test_interceptor_chain_multiple() {
        let mut chain = InterceptorChain::new();
        chain.add(Box::new(NoOpInterceptor));
        chain.add(Box::new(TestInterceptor));
        chain.add(Box::new(NoOpInterceptor));

        let mut messages = vec![ChatMessage::user("Hello")];
        let reports = chain.run(&mut messages);

        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].name, "test");
    }
}
