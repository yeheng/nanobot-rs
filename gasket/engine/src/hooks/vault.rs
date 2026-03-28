//! VaultHook - Hook for injecting vault secrets into messages
//!
//! This hook wraps `VaultInjector` to inject secrets from the vault into
//! messages before sending to LLM. The injected values are stored for
//! later redaction from logs and saved history.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;
use tracing::debug;

use super::{HookAction, HookPoint, MutableContext, PipelineHook, ReadonlyContext};
use crate::error::AgentError;
use gasket_core::vault::VaultInjector;

/// Hook for injecting vault secrets into messages.
///
/// This hook executes at `BeforeLLM` point and replaces `{{vault:key}}`
/// placeholders with actual secret values from the vault.
///
/// # Thread Safety
///
/// The injected values are stored in an `Arc<RwLock<Vec<String>>>` so they
/// can be accessed by other components (e.g., for log redaction) after
/// the hook completes.
///
/// # Example
///
/// ```rust,ignore
/// use std::sync::Arc;
/// use gasket_core::hooks::VaultHook;
/// use gasket_core::vault::{VaultStore, VaultInjector};
///
/// // Create vault store
/// let store = Arc::new(VaultStore::new()?);
/// store.set("api_key", "secret123", None)?;
///
/// // Create hook
/// let injector = VaultInjector::new(store);
/// let hook = VaultHook::new(injector);
///
/// // Get injected values for later redaction
/// let injected_values = hook.injected_values();
/// ```
pub struct VaultHook {
    injector: VaultInjector,
    /// Stored injected values for later redaction
    injected_values: Arc<RwLock<Vec<String>>>,
}

impl VaultHook {
    /// Create a new VaultHook with the given injector.
    pub fn new(injector: VaultInjector) -> Self {
        Self {
            injector,
            injected_values: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Get a handle to the injected values for redaction.
    ///
    /// This returns an `Arc<RwLock<Vec<String>>>` that can be cloned
    /// and shared with other components that need to redact secrets
    /// from logs or saved history.
    pub fn injected_values(&self) -> Arc<RwLock<Vec<String>>> {
        self.injected_values.clone()
    }
}

#[async_trait]
impl PipelineHook for VaultHook {
    fn name(&self) -> &str {
        "vault_injector"
    }

    fn point(&self) -> HookPoint {
        HookPoint::BeforeLLM
    }

    async fn run(&self, ctx: &mut MutableContext<'_>) -> Result<HookAction, AgentError> {
        let report = self.injector.inject(ctx.messages);

        if !report.keys_used.is_empty() {
            debug!(
                "[VaultHook] Injected {} keys into {} messages",
                report.keys_used.len(),
                report.messages_modified
            );
        }

        // Store injected values for redaction
        let mut values = self.injected_values.write().await;
        values.clear();
        values.extend(report.injected_values);
        values.sort();
        values.dedup();

        Ok(HookAction::Continue)
    }

    async fn run_parallel(&self, _ctx: &ReadonlyContext<'_>) -> Result<HookAction, AgentError> {
        // VaultHook is Sequential only, this shouldn't be called
        Ok(HookAction::Continue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vault_hook_point() {
        // Compile-time check that the trait is implemented correctly
        let store = Arc::new(gasket_core::vault::VaultStore::new_in_memory());
        let injector = VaultInjector::new(store);
        let hook = VaultHook::new(injector);

        assert_eq!(hook.name(), "vault_injector");
        assert_eq!(hook.point(), HookPoint::BeforeLLM);
    }

    #[tokio::test]
    async fn test_injected_values_empty() {
        let store = Arc::new(gasket_core::vault::VaultStore::new_in_memory());
        let injector = VaultInjector::new(store);
        let hook = VaultHook::new(injector);

        let values_handle = hook.injected_values();
        let values = values_handle.read().await;
        assert!(values.is_empty());
    }
}
