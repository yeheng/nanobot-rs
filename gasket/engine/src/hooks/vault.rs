//! VaultHook - Hook for injecting vault secrets into messages
//!
//! This hook wraps `VaultInjector` to inject secrets from the vault into
//! messages before sending to LLM. The injected values are written into
//! the per-request `HookContext::vault_values` for later redaction from
//! logs and saved history.

use async_trait::async_trait;
use tracing::debug;

use super::{HookAction, HookPoint, MutableContext, PipelineHook, ReadonlyContext};
use crate::error::AgentError;
use crate::vault::VaultInjector;

/// Hook for injecting vault secrets into messages.
///
/// This hook executes at `BeforeLLM` point and replaces `{{vault:key}}`
/// placeholders with actual secret values from the vault.
///
/// # Per-Request Safety
///
/// Injected values are written directly into the per-request
/// `HookContext::vault_values`, eliminating shared mutable state and
/// preventing cross-request secret leakage under concurrent load.
pub struct VaultHook {
    injector: VaultInjector,
}

impl VaultHook {
    /// Create a new VaultHook with the given injector.
    pub fn new(injector: VaultInjector) -> Self {
        Self { injector }
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

        // Write injected values into per-request context for redaction
        ctx.vault_values.clear();
        ctx.vault_values.extend(report.injected_values);
        ctx.vault_values.sort();
        ctx.vault_values.dedup();

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
    use std::sync::Arc;

    #[test]
    fn test_vault_hook_point() {
        let store = Arc::new(crate::vault::VaultStore::new_in_memory());
        let injector = VaultInjector::new(store);
        let hook = VaultHook::new(injector);

        assert_eq!(hook.name(), "vault_injector");
        assert_eq!(hook.point(), HookPoint::BeforeLLM);
    }
}
