use std::sync::Arc;

use futures::FutureExt;

use crate::host::CommandHost;
use crate::types::{Command, CommandKind, CommandResult};

pub fn exit() -> Command {
    Command {
        name: "exit".into(),
        description: "Exit the REPL".into(),
        aliases: vec!["quit".into(), "q".into(), ":q".into()],
        kind: CommandKind::Builtin(Arc::new(|_args: &str, _host: &dyn CommandHost| {
            async { CommandResult::Quit }.boxed()
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::DispatcherBuilder;
    use crate::types::RouteOutcome;
    use std::sync::Arc;

    use async_trait::async_trait;
    use gasket_types::{ModelSwitchInfo, SessionKey, SessionSummary};

    struct H;

    #[async_trait]
    impl CommandHost for H {
        async fn clear_session(&self, _: &SessionKey) {}
        async fn list_sessions(&self) -> Vec<SessionSummary> {
            vec![]
        }
        async fn current_model(&self) -> String {
            "m".into()
        }
        async fn switch_model(&self, _new: &str) -> Result<ModelSwitchInfo, String> {
            Ok(ModelSwitchInfo {
                previous: "m".into(),
                current: "m".into(),
            })
        }
    }

    #[tokio::test]
    async fn exit_canonical_returns_quit() {
        let d = DispatcherBuilder::new()
            .host(Arc::new(H))
            .register_builtin(exit())
            .build()
            .await
            .unwrap();
        assert_eq!(
            d.route("/exit").await,
            RouteOutcome::Handled(CommandResult::Quit)
        );
    }

    #[tokio::test]
    async fn exit_aliases_resolve() {
        let d = DispatcherBuilder::new()
            .host(Arc::new(H))
            .register_builtin(exit())
            .build()
            .await
            .unwrap();
        for s in &["/quit", "/q", "/:q"] {
            assert_eq!(
                d.route(s).await,
                RouteOutcome::Handled(CommandResult::Quit),
                "alias {s}"
            );
        }
    }
}
