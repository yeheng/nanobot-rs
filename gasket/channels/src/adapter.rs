//! IM adapter trait — unified inbound/outbound interface for messaging platforms.
//!
//! Replaces the old split design (Channel for inbound + OutboundSender for outbound)
//! with a single trait per platform.

use async_trait::async_trait;

/// Unified adapter for a single messaging platform.
///
/// Each platform implements this trait to handle both message ingestion
/// (inbound) and message delivery (outbound).
#[async_trait]
pub trait ImAdapter: Send + Sync {
    /// Platform name, e.g. "telegram".
    fn name(&self) -> &str;

    /// Start the inbound message loop.
    ///
    /// For bot-based platforms (Telegram, Discord, Slack) this blocks and
    /// pushes incoming messages into the supplied sender.
    /// For webhook-based platforms this is typically a no-op because inbound
    /// messages arrive via HTTP callbacks.
    async fn start(&self, inbound: crate::middleware::InboundSender) -> anyhow::Result<()>;

    /// Send an outbound message.
    async fn send(&self, msg: &crate::events::OutboundMessage) -> anyhow::Result<()>;
}
