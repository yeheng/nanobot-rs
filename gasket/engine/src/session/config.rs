//! Session configuration — re-exports AgentConfig with kernel conversion.

// Re-export the existing AgentConfig from agent/core/config.rs
// (will be moved here in cleanup phase)
pub use crate::agent::core::config::AgentConfig;

use crate::kernel::KernelConfig;

/// Extension trait to convert AgentConfig → KernelConfig.
pub trait AgentConfigExt {
    fn to_kernel_config(&self) -> KernelConfig;
}

impl AgentConfigExt for AgentConfig {
    fn to_kernel_config(&self) -> KernelConfig {
        KernelConfig::new(self.model.clone())
            .with_max_iterations(self.max_iterations)
            .with_temperature(self.temperature)
            .with_max_tokens(self.max_tokens)
            .with_thinking(self.thinking_enabled)
    }
}
