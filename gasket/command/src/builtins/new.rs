use std::sync::Arc;

use futures::FutureExt;
use gasket_types::SessionKey;

use crate::host::CommandHost;
use crate::types::{Command, CommandKind, CommandResult};

pub fn new(session_key: Arc<SessionKey>) -> Command {
    Command {
        name: "new".into(),
        description: "Start a new conversation".into(),
        aliases: vec![],
        kind: CommandKind::Builtin(Arc::new(move |_args: &str, host: &dyn CommandHost| {
            let key = session_key.clone();
            async move {
                host.clear_session(&key).await;
                CommandResult::Print(format!("✓ Session cleared ({})", key))
            }
            .boxed()
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use gasket_types::{ChannelType, ModelSwitchInfo, SessionKey, SessionSummary};
    use std::sync::Mutex;

    use crate::dispatcher::DispatcherBuilder;
    use crate::types::RouteOutcome;

    struct H {
        cleared: Mutex<Vec<SessionKey>>,
    }
    #[async_trait]
    impl CommandHost for H {
        async fn clear_session(&self, k: &SessionKey) {
            self.cleared.lock().unwrap().push(k.clone());
        }
        async fn list_sessions(&self) -> Vec<SessionSummary> {
            vec![]
        }
        async fn current_model(&self) -> String {
            "m".into()
        }
        async fn switch_model(&self, _: &str) -> Result<ModelSwitchInfo, String> {
            Ok(ModelSwitchInfo {
                previous: "m".into(),
                current: "m".into(),
            })
        }
    }

    #[tokio::test]
    async fn new_calls_clear_session_once_with_correct_key() {
        let key = SessionKey::new(ChannelType::Cli, "interactive");
        let host = Arc::new(H {
            cleared: Mutex::new(vec![]),
        });
        let d = DispatcherBuilder::new()
            .host(host.clone())
            .register_builtin(new(Arc::new(key.clone())))
            .build()
            .await
            .unwrap();

        let outcome = d.route("/new").await;

        match outcome {
            RouteOutcome::Handled(CommandResult::Print(msg)) => {
                assert!(msg.contains("Session cleared"));
            }
            other => panic!("{:?}", other),
        }
        let calls = host.cleared.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0], key);
    }
}
