//! Bridge trait between the dispatcher and the host application (CLI / Web).

use async_trait::async_trait;
use gasket_types::{ModelSwitchInfo, SessionKey, SessionSummary};

#[async_trait]
pub trait CommandHost: Send + Sync {
    async fn clear_session(&self, key: &SessionKey);
    async fn list_sessions(&self) -> Vec<SessionSummary>;
    async fn current_model(&self) -> String;
    async fn switch_model(&self, new: &str) -> Result<ModelSwitchInfo, String>;
}
