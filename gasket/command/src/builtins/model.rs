use std::sync::Arc;

use futures::FutureExt;

use crate::host::CommandHost;
use crate::types::{Command, CommandKind, CommandResult};

pub fn model() -> Command {
    Command {
        name: "model".into(),
        description: "Show or switch the active model".into(),
        aliases: vec![],
        kind: CommandKind::Builtin(Arc::new(|args: &str, host: &dyn CommandHost| {
            let target = args.trim().to_string();
            async move {
                if target.is_empty() {
                    let id = host.current_model().await;
                    return CommandResult::Print(format!("Current model: {id}"));
                }
                match host.switch_model(&target).await {
                    Ok(info) => CommandResult::Print(format!(
                        "Switched: {} → {}",
                        info.previous, info.current
                    )),
                    Err(e) => CommandResult::Error(format!("model switch failed: {e}")),
                }
            }
            .boxed()
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use gasket_types::{ModelSwitchInfo, SessionKey, SessionSummary};
    use std::sync::{Arc, Mutex};

    use crate::dispatcher::DispatcherBuilder;
    use crate::types::RouteOutcome;

    struct H {
        current: Mutex<String>,
        switch: Result<(), &'static str>,
    }
    #[async_trait]
    impl CommandHost for H {
        async fn clear_session(&self, _: &SessionKey) {}
        async fn list_sessions(&self) -> Vec<SessionSummary> {
            vec![]
        }
        async fn current_model(&self) -> String {
            self.current.lock().unwrap().clone()
        }
        async fn switch_model(&self, new: &str) -> Result<ModelSwitchInfo, String> {
            match self.switch {
                Ok(()) => {
                    let mut g = self.current.lock().unwrap();
                    let prev = g.clone();
                    *g = new.to_string();
                    Ok(ModelSwitchInfo {
                        previous: prev,
                        current: new.into(),
                    })
                }
                Err(msg) => Err(msg.into()),
            }
        }
    }

    fn host_ok(initial: &str) -> Arc<H> {
        Arc::new(H {
            current: Mutex::new(initial.into()),
            switch: Ok(()),
        })
    }

    #[tokio::test]
    async fn no_args_shows_current() {
        let d = DispatcherBuilder::new()
            .host(host_ok("openai/gpt-4.1"))
            .register_builtin(model())
            .build()
            .await
            .unwrap();
        match d.route("/model").await {
            RouteOutcome::Handled(CommandResult::Print(s)) => {
                assert!(s.contains("openai/gpt-4.1"));
            }
            other => panic!("{:?}", other),
        }
    }

    #[tokio::test]
    async fn args_switches_model() {
        let d = DispatcherBuilder::new()
            .host(host_ok("openai/gpt-4.1"))
            .register_builtin(model())
            .build()
            .await
            .unwrap();
        match d.route("/model anthropic/claude-4.5-sonnet").await {
            RouteOutcome::Handled(CommandResult::Print(s)) => {
                assert!(s.contains("openai/gpt-4.1"));
                assert!(s.contains("anthropic/claude-4.5-sonnet"));
                assert!(s.contains("→"));
            }
            other => panic!("{:?}", other),
        }
    }

    #[tokio::test]
    async fn switch_error_yields_error_result() {
        let host = Arc::new(H {
            current: Mutex::new("a".into()),
            switch: Err("unknown model"),
        });
        let d = DispatcherBuilder::new()
            .host(host)
            .register_builtin(model())
            .build()
            .await
            .unwrap();
        match d.route("/model bogus").await {
            RouteOutcome::Handled(CommandResult::Error(s)) => {
                assert!(s.contains("unknown model"));
            }
            other => panic!("{:?}", other),
        }
    }
}
