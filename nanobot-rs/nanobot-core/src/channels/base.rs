//! Channel base types

use async_trait::async_trait;

/// Channel trait for implementing chat channel integrations.
///
/// Channels are **inbound-only**: they receive messages and push them to the
/// internal bus. All **outbound** sending is handled by the Outbound Actor,
/// which uses [`OutboundSenderRegistry`](super::outbound::OutboundSenderRegistry)
/// to route messages based on channel type.
///
/// Provides a unified lifecycle: `start` → `stop` → `graceful_shutdown`.
#[async_trait]
pub trait Channel: Send + Sync {
    /// Get the channel name
    fn name(&self) -> &str;

    /// Start the channel (begin receiving messages)
    async fn start(&mut self) -> anyhow::Result<()>;

    /// Stop the channel
    async fn stop(&mut self) -> anyhow::Result<()>;

    /// Graceful shutdown with optional timeout.
    ///
    /// Default implementation delegates to `stop()`.
    async fn graceful_shutdown(&mut self) -> anyhow::Result<()> {
        self.stop().await
    }
}
