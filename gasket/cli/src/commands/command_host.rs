//! Bridge from `gasket-command::CommandHost` to `AgentSession`.

use std::sync::Arc;

use async_trait::async_trait;
use gasket_command::CommandHost;
use gasket_engine::session::AgentSession;
use gasket_types::{ModelSwitchInfo, SessionKey, SessionSummary};

#[allow(dead_code)] // Used by agent.rs in Task 21 (next commit).
pub struct CliCommandHost {
    pub agent: Arc<AgentSession>,
}

#[allow(dead_code)] // Used by agent.rs in Task 21 (next commit).
impl CliCommandHost {
    pub fn new(agent: Arc<AgentSession>) -> Self {
        Self { agent }
    }
}

#[async_trait]
impl CommandHost for CliCommandHost {
    async fn clear_session(&self, key: &SessionKey) {
        self.agent.clear_session(key).await;
    }

    async fn list_sessions(&self) -> Vec<SessionSummary> {
        self.agent.list_sessions().await
    }

    async fn current_model(&self, _key: &SessionKey) -> String {
        self.agent.model().to_string()
    }

    async fn switch_model(&self, _key: &SessionKey, new: &str) -> Result<ModelSwitchInfo, String> {
        self.agent.switch_model(new).await
    }
}
