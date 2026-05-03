//! Slash-command dispatcher.

use std::collections::HashMap;
use std::sync::Arc;

use crate::host::CommandHost;
use crate::parser::{parse, ParsedInput};
use crate::template::render;
use crate::types::{Command, CommandKind, CommandResult, RouteOutcome};

pub struct Dispatcher {
    pub(crate) commands: HashMap<String, Arc<Command>>,
    pub(crate) aliases: HashMap<String, String>,
    pub(crate) host: Arc<dyn CommandHost>,
}

impl Dispatcher {
    pub async fn route(&self, line: &str) -> RouteOutcome {
        match parse(line) {
            ParsedInput::NotCommand => RouteOutcome::Passthrough(line.to_string()),
            ParsedInput::Command { name, args } => self.dispatch(name, args).await,
        }
    }

    async fn dispatch(&self, name: &str, args: &str) -> RouteOutcome {
        let canonical = self
            .aliases
            .get(name)
            .map(String::as_str)
            .unwrap_or(name);

        let Some(cmd) = self.commands.get(canonical) else {
            return RouteOutcome::Handled(CommandResult::Error(format!(
                "unknown command: /{name}    (type /help to see commands)"
            )));
        };

        match &cmd.kind {
            CommandKind::Builtin(handler) => {
                RouteOutcome::Handled(handler(args, self.host.as_ref()).await)
            }
            CommandKind::Yaml {
                prompt_template,
                allowed_tools,
            } => RouteOutcome::Rewrite {
                prompt: render(prompt_template, args),
                tool_filter: allowed_tools.clone(),
            },
        }
    }

    pub fn list_commands(&self) -> Vec<&Command> {
        let mut v: Vec<&Command> = self.commands.values().map(|a| a.as_ref()).collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::BuiltinHandler;
    use async_trait::async_trait;
    use futures::FutureExt;
    use gasket_types::{ModelSwitchInfo, SessionKey, SessionSummary};
    use std::sync::Mutex;

    pub struct MockCommandHost {
        pub clear_calls: Mutex<Vec<SessionKey>>,
    }

    impl MockCommandHost {
        pub fn new() -> Self {
            Self {
                clear_calls: Mutex::new(vec![]),
            }
        }
    }

    #[async_trait]
    impl CommandHost for MockCommandHost {
        async fn clear_session(&self, key: &SessionKey) {
            self.clear_calls.lock().unwrap().push(key.clone());
        }
        async fn list_sessions(&self) -> Vec<SessionSummary> {
            vec![]
        }
        async fn current_model(&self) -> String {
            "test-model".into()
        }
        async fn switch_model(&self, new: &str) -> Result<ModelSwitchInfo, String> {
            Ok(ModelSwitchInfo {
                previous: "test-model".into(),
                current: new.into(),
            })
        }
    }

    fn echo_handler() -> BuiltinHandler {
        Arc::new(|args: &str, _host: &dyn CommandHost| {
            let s = format!("echo: {}", args);
            async move { CommandResult::Print(s) }.boxed()
        })
    }

    fn make_dispatcher_with(commands: Vec<Command>) -> Dispatcher {
        let mut map: HashMap<String, Arc<Command>> = HashMap::new();
        for c in commands {
            map.insert(c.name.clone(), Arc::new(c));
        }
        Dispatcher {
            commands: map,
            aliases: HashMap::new(),
            host: Arc::new(MockCommandHost::new()),
        }
    }

    #[tokio::test]
    async fn builtin_match_invokes_handler() {
        let cmd = Command {
            name: "echo".into(),
            description: "echoes args".into(),
            aliases: vec![],
            kind: CommandKind::Builtin(echo_handler()),
        };
        let d = make_dispatcher_with(vec![cmd]);

        let outcome = d.route("/echo hello world").await;

        assert_eq!(
            outcome,
            RouteOutcome::Handled(CommandResult::Print("echo: hello world".into()))
        );
    }

    #[tokio::test]
    async fn unknown_command_returns_error() {
        let d = make_dispatcher_with(vec![]);
        let outcome = d.route("/whatisthis").await;
        match outcome {
            RouteOutcome::Handled(CommandResult::Error(msg)) => {
                assert!(msg.contains("/whatisthis"), "msg = {msg}");
                assert!(msg.contains("/help"), "should hint /help; msg = {msg}");
            }
            other => panic!("expected Error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn non_command_text_passes_through_verbatim() {
        let d = make_dispatcher_with(vec![]);
        let outcome = d.route("hello world").await;
        assert_eq!(outcome, RouteOutcome::Passthrough("hello world".into()));
    }

    #[tokio::test]
    async fn alias_resolves_to_canonical() {
        let cmd = Command {
            name: "exit".into(),
            description: "exit".into(),
            aliases: vec!["q".into(), "quit".into()],
            kind: CommandKind::Builtin(Arc::new(|_, _| {
                async { CommandResult::Quit }.boxed()
            })),
        };
        let mut map: HashMap<String, Arc<Command>> = HashMap::new();
        let arc = Arc::new(cmd);
        map.insert(arc.name.clone(), arc.clone());
        let mut aliases = HashMap::new();
        aliases.insert("q".into(), "exit".into());
        aliases.insert("quit".into(), "exit".into());
        let d = Dispatcher {
            commands: map,
            aliases,
            host: Arc::new(MockCommandHost::new()),
        };

        assert_eq!(
            d.route("/q").await,
            RouteOutcome::Handled(CommandResult::Quit)
        );
        assert_eq!(
            d.route("/quit").await,
            RouteOutcome::Handled(CommandResult::Quit)
        );
    }

    #[tokio::test]
    async fn yaml_kind_produces_rewrite_with_filter() {
        let cmd = Command {
            name: "translate".into(),
            description: "translate".into(),
            aliases: vec![],
            kind: CommandKind::Yaml {
                prompt_template: "Translate to Mandarin: {{user_input}}".into(),
                allowed_tools: Some(vec!["wiki_search".into()]),
            },
        };
        let d = make_dispatcher_with(vec![cmd]);

        let outcome = d.route("/translate Hello world").await;

        assert_eq!(
            outcome,
            RouteOutcome::Rewrite {
                prompt: "Translate to Mandarin: Hello world".into(),
                tool_filter: Some(vec!["wiki_search".into()]),
            }
        );
    }

    #[tokio::test]
    async fn yaml_with_no_tool_filter_passes_none() {
        let cmd = Command {
            name: "review".into(),
            description: "review".into(),
            aliases: vec![],
            kind: CommandKind::Yaml {
                prompt_template: "Review:\n{{user_input}}".into(),
                allowed_tools: None,
            },
        };
        let d = make_dispatcher_with(vec![cmd]);

        let outcome = d.route("/review my code").await;

        assert_eq!(
            outcome,
            RouteOutcome::Rewrite {
                prompt: "Review:\nmy code".into(),
                tool_filter: None,
            }
        );
    }
}
