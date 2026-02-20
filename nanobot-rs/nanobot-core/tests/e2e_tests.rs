//! End-to-end integration tests

use std::path::PathBuf;
use std::sync::Arc;

use nanobot_core::LlmProvider;
use nanobot_core::Tool;

// =============================================================================
// Agent Tests
// =============================================================================

#[tokio::test]
async fn test_agent_initialization() {
    let workspace = PathBuf::from("/tmp/nanobot-test");

    let config = nanobot_core::agent::AgentConfig {
        model: "gpt-4o".to_string(),
        max_iterations: 10,
        temperature: 0.7,
        max_tokens: 1024,
        memory_window: 20,
        restrict_to_workspace: true,
        max_tool_result_chars: 8000,
    };

    let provider =
        nanobot_core::providers::OpenAICompatibleProvider::openai("test-key", None, "gpt-4o");

    let agent =
        nanobot_core::agent::AgentLoop::new(Arc::new(provider), workspace.clone(), config).unwrap();

    assert_eq!(agent.model(), "gpt-4o");
    assert_eq!(agent.workspace(), &workspace);
}

#[tokio::test]
async fn test_agent_config_default() {
    use nanobot_core::agent::AgentConfig;

    let config = AgentConfig::default();
    assert_eq!(config.model, "gpt-4o");
    assert_eq!(config.max_iterations, 20);
    assert_eq!(config.temperature, 0.7);
    assert_eq!(config.max_tokens, 4096);
    assert_eq!(config.memory_window, 50);
    assert!(!config.restrict_to_workspace);
}

// =============================================================================
// Message Bus Tests
// =============================================================================

#[tokio::test]
async fn test_message_bus() {
    use nanobot_core::bus::events::{ChannelType, InboundMessage};
    use nanobot_core::bus::MessageBus;

    let (bus, mut rx, _outbound_rx) = MessageBus::new(10);

    let inbound = InboundMessage {
        channel: ChannelType::Cli,
        sender_id: "user1".to_string(),
        chat_id: "chat1".to_string(),
        content: "Hello".to_string(),
        media: None,
        metadata: None,
        timestamp: chrono::Utc::now(),
    };

    bus.publish_inbound(inbound.clone()).await;

    let received = rx.recv().await;
    assert!(received.is_some());
    let received = received.unwrap();
    assert_eq!(received.content, "Hello");
    assert_eq!(received.channel, ChannelType::Cli);
}

#[tokio::test]
async fn test_message_bus_session_key() {
    use nanobot_core::bus::events::{ChannelType, InboundMessage};

    let msg = InboundMessage {
        channel: ChannelType::Telegram,
        sender_id: "user123".to_string(),
        chat_id: "chat456".to_string(),
        content: "Test".to_string(),
        media: None,
        metadata: None,
        timestamp: chrono::Utc::now(),
    };

    assert_eq!(msg.session_key(), "telegram:chat456");
}

#[tokio::test]
async fn test_outbound_message() {
    use nanobot_core::bus::events::{ChannelType, OutboundMessage};

    let outbound = OutboundMessage {
        channel: ChannelType::Discord,
        chat_id: "channel789".to_string(),
        content: "Response".to_string(),
        metadata: Some(serde_json::json!({"thread_ts": "12345"})),
    };

    assert_eq!(outbound.channel, ChannelType::Discord);
    assert_eq!(outbound.chat_id, "channel789");
    assert!(outbound.metadata.is_some());
}

// =============================================================================
// Session Tests
// =============================================================================

#[tokio::test]
async fn test_session_manager() {
    use nanobot_core::session::SessionManager;

    let workspace = PathBuf::from("/tmp/nanobot-test-sessions");
    let manager = SessionManager::new(workspace);

    let mut session = manager.get_or_create("test:session1").await;
    assert_eq!(session.key, "test:session1");

    session.add_message("user", "Hello", None);
    session.add_message("assistant", "Hi there!", None);

    let history = session.get_history(10);
    assert_eq!(history.len(), 2);
}

#[tokio::test]
async fn test_session_clear() {
    use nanobot_core::session::SessionManager;

    let workspace = PathBuf::from("/tmp/nanobot-test-clear");
    let manager = SessionManager::new(workspace);

    let mut session = manager.get_or_create("test:clear").await;
    session.add_message("user", "Hello", None);
    assert!(!session.messages.is_empty());

    session.clear();
    assert!(session.messages.is_empty());
}

#[tokio::test]
async fn test_session_tools_used() {
    use nanobot_core::session::SessionManager;

    let workspace = PathBuf::from("/tmp/nanobot-test-tools");
    let manager = SessionManager::new(workspace);

    let mut session = manager.get_or_create("test:tools").await;
    session.add_message(
        "assistant",
        "Done",
        Some(vec!["read_file".to_string(), "edit_file".to_string()]),
    );

    assert!(session.messages.last().unwrap().tools_used.is_some());
}

// =============================================================================
// Tool Tests
// =============================================================================

#[tokio::test]
async fn test_tool_registry() {
    use nanobot_core::tools::{ReadFileTool, ToolRegistry};

    let mut registry = ToolRegistry::new();

    let tool = ReadFileTool::new(None);
    let name = tool.name().to_string();
    registry.register(Box::new(tool));

    assert!(registry.get(&name).is_some());

    let definitions = registry.get_definitions();
    assert!(!definitions.is_empty());
}

#[tokio::test]
async fn test_tool_registry_multiple() {
    use nanobot_core::tools::{ExecTool, ReadFileTool, ToolRegistry, WriteFileTool};
    use std::time::Duration;

    let mut registry = ToolRegistry::new();

    registry.register(Box::new(ReadFileTool::new(None)));
    registry.register(Box::new(WriteFileTool::new(None)));
    registry.register(Box::new(ExecTool::new(
        PathBuf::from("/tmp"),
        Duration::from_secs(60),
        false,
    )));

    assert!(registry.get("read_file").is_some());
    assert!(registry.get("write_file").is_some());
    assert!(registry.get("exec").is_some());
    assert!(registry.get("nonexistent").is_none());
}

#[tokio::test]
async fn test_simple_schema() {
    use nanobot_core::tools::simple_schema;

    let schema = simple_schema(&[
        ("path", "string", true, "File path"),
        ("limit", "number", false, "Max results"),
    ]);

    assert_eq!(schema["type"], "object");
    assert!(schema["properties"]["path"].is_object());
    assert!(schema["required"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("path")));
}

// =============================================================================
// Config Tests
// =============================================================================

#[tokio::test]
async fn test_config_defaults() {
    use nanobot_core::config::Config;

    let config = Config::default();

    assert_eq!(config.agents.defaults.temperature, 0.0);
    assert_eq!(config.agents.defaults.max_tokens, 0);
    assert_eq!(config.agents.defaults.max_iterations, 0);
    assert!(!config.tools.restrict_to_workspace);
}

#[tokio::test]
async fn test_config_deserialization() {
    use nanobot_core::config::Config;

    let json = r#"{
        "providers": {
            "openrouter": {
                "apiKey": "sk-or-test"
            }
        },
        "agents": {
            "defaults": {
                "model": "gpt-4o",
                "temperature": 0.5,
                "max_tokens": 2048,
                "max_iterations": 10,
                "memory_window": 30
            }
        },
        "tools": {
            "restrictToWorkspace": true
        }
    }"#;

    let config: Config = serde_json::from_str(json).unwrap();

    assert_eq!(
        config.providers.get("openrouter").unwrap().api_key,
        Some("sk-or-test".to_string())
    );
    assert_eq!(config.agents.defaults.model, Some("gpt-4o".to_string()));
    assert_eq!(config.agents.defaults.temperature, 0.5);
    assert_eq!(config.agents.defaults.max_tokens, 2048);
    assert_eq!(config.agents.defaults.max_iterations, 10);
    assert_eq!(config.agents.defaults.memory_window, 30);
    assert!(config.tools.restrict_to_workspace);
}

#[tokio::test]
async fn test_channels_config() {
    use nanobot_core::config::Config;

    let json = r#"{
        "channels": {
            "telegram": {
                "enabled": true,
                "token": "bot123",
                "allow_from": ["user1", "user2"]
            },
            "discord": {
                "enabled": true,
                "token": "discord-token",
                "allow_from": []
            }
        }
    }"#;

    let config: Config = serde_json::from_str(json).unwrap();

    assert!(config.channels.telegram.is_some());
    let tg = config.channels.telegram.unwrap();
    assert!(tg.enabled);
    assert_eq!(tg.token, "bot123");
    assert_eq!(tg.allow_from, vec!["user1", "user2"]);
}

// =============================================================================
// Cron Tests
// =============================================================================

#[tokio::test]
async fn test_cron_job_creation() {
    use nanobot_core::cron::CronJob;

    let job = CronJob::new("test-id", "Test Job", "0 0 * * * *", "Test message");

    assert_eq!(job.id, "test-id");
    assert_eq!(job.name, "Test Job");
    assert_eq!(job.cron, "0 0 * * * *");
    assert_eq!(job.message, "Test message");

    let schedule: cron::Schedule = job.cron.parse().expect("Invalid cron expression");
    assert!(schedule.upcoming(chrono::Utc).next().is_some());
}

#[tokio::test]
async fn test_cron_job_fields() {
    use nanobot_core::cron::CronJob;

    let mut job = CronJob::new("id1", "Name", "0 0 9 * * *", "Message");
    job.channel = Some("telegram".to_string());
    job.chat_id = Some("chat123".to_string());

    assert_eq!(job.channel, Some("telegram".to_string()));
    assert_eq!(job.chat_id, Some("chat123".to_string()));
}

// =============================================================================
// Memory Tests
// =============================================================================

#[tokio::test]
async fn test_memory_store() {
    use nanobot_core::agent::MemoryStore;

    let workspace = PathBuf::from("/tmp/nanobot-memory-test");
    let memory = MemoryStore::new(workspace);

    let _ = memory.write_long_term("User likes pizza.");

    let content = memory.read_long_term().unwrap_or_default();
    assert!(content.contains("pizza"));

    let _ = memory.append_history("[2024-01-01] User asked about pizza.");

    let history = memory.read_history().unwrap_or_default();
    assert!(history.contains("pizza"));
}

// =============================================================================
// Web Tool Tests
// =============================================================================

#[tokio::test]
async fn test_web_search_tool_schema() {
    use nanobot_core::tools::WebSearchTool;

    let tool = WebSearchTool::new(None);

    assert_eq!(tool.name(), "web_search");

    let params = tool.parameters();
    assert!(params["properties"]["query"].is_object());
    assert!(params["properties"]["count"].is_object());
}

#[tokio::test]
async fn test_web_fetch_tool_schema() {
    use nanobot_core::tools::WebFetchTool;

    let tool = WebFetchTool::new();

    assert_eq!(tool.name(), "web_fetch");

    let params = tool.parameters();
    assert!(params["properties"]["url"].is_object());
    assert!(params["properties"]["prompt"].is_object());
}

#[tokio::test]
async fn test_web_fetch_tool_with_options() {
    use nanobot_core::tools::WebFetchTool;

    let tool = WebFetchTool::new()
        .with_timeout(60)
        .with_max_size(5_000_000);

    assert_eq!(tool.name(), "web_fetch");
}

// =============================================================================
// Spawn Tool Tests
// =============================================================================

#[tokio::test]
async fn test_spawn_tool() {
    use nanobot_core::tools::SpawnTool;

    let tool = SpawnTool::new();

    assert_eq!(tool.name(), "spawn");

    let params = tool.parameters();
    assert!(params["properties"]["task"].is_object());
    assert!(params["properties"]["timeout"].is_object());
}

#[tokio::test]
async fn test_subagent_task() {
    use nanobot_core::agent::subagent::{SubagentTask, TaskStatus};

    let task = SubagentTask::new(
        "Test task",
        "telegram",
        "chat123",
        "session:telegram:chat123",
    );

    assert!(!task.id.is_empty());
    assert_eq!(task.prompt, "Test task");
    assert_eq!(task.status, TaskStatus::Pending);
    assert!(!task.is_finished());
}

// =============================================================================
// Cron Tool Tests
// =============================================================================

#[tokio::test]
async fn test_cron_tool_schema() {
    use nanobot_core::cron::CronService;
    use nanobot_core::tools::CronTool;
    use std::sync::Arc;

    let service = Arc::new(CronService::new(PathBuf::from("/tmp/test-cron.json")));
    let tool = CronTool::new(service);

    assert_eq!(tool.name(), "cron");

    let params = tool.parameters();
    assert!(params["properties"]["action"].is_object());
}

// =============================================================================
// Provider Tests
// =============================================================================

#[tokio::test]
async fn test_provider_trait() {
    use nanobot_core::providers::LlmProvider;
    use nanobot_core::providers::OpenAICompatibleProvider;

    let provider = OpenAICompatibleProvider::openai("test-key", None, "gpt-4o");

    assert_eq!(provider.name(), "openai");
    assert_eq!(provider.default_model(), "gpt-4o");
}

#[tokio::test]
async fn test_openrouter_provider() {
    use nanobot_core::providers::OpenAICompatibleProvider;

    let provider =
        OpenAICompatibleProvider::openrouter("sk-or-test", None, "anthropic/claude-sonnet-4");

    assert_eq!(provider.name(), "openrouter");
    assert_eq!(provider.default_model(), "anthropic/claude-sonnet-4");
}

#[tokio::test]
async fn test_anthropic_provider() {
    use nanobot_core::providers::OpenAICompatibleProvider;

    let provider =
        OpenAICompatibleProvider::anthropic("sk-ant-test", None, "claude-sonnet-4-20250514");

    assert_eq!(provider.name(), "anthropic");
    assert_eq!(provider.default_model(), "claude-sonnet-4-20250514");
}

// =============================================================================
// Chat Message Tests
// =============================================================================

#[tokio::test]
async fn test_chat_message_user() {
    use nanobot_core::providers::ChatMessage;

    let msg = ChatMessage::user("Hello");
    assert_eq!(msg.role, "user");
    assert_eq!(msg.content, Some("Hello".to_string()));
}

#[tokio::test]
async fn test_chat_message_assistant() {
    use nanobot_core::providers::ChatMessage;

    let msg = ChatMessage::assistant("Hi there!");
    assert_eq!(msg.role, "assistant");
    assert_eq!(msg.content, Some("Hi there!".to_string()));
}

#[tokio::test]
async fn test_chat_message_system() {
    use nanobot_core::providers::ChatMessage;

    let msg = ChatMessage::system("You are a helpful assistant.");
    assert_eq!(msg.role, "system");
    assert_eq!(
        msg.content,
        Some("You are a helpful assistant.".to_string())
    );
}

// =============================================================================
// Heartbeat Tests
// =============================================================================

#[tokio::test]
async fn test_heartbeat_service_creation() {
    use nanobot_core::heartbeat::HeartbeatService;

    let workspace = PathBuf::from("/tmp/nanobot-heartbeat-test");
    let service = HeartbeatService::new(workspace);

    // Just verify it can be created
    assert!(service.workspace().ends_with("nanobot-heartbeat-test"));
}

// =============================================================================
// MCP Tests
// =============================================================================

#[tokio::test]
async fn test_mcp_manager_creation() {
    use nanobot_core::mcp::McpManager;

    let manager = McpManager::new();

    assert!(manager.get_all_tools().is_empty());
}

#[tokio::test]
async fn test_mcp_tool_definition() {
    use nanobot_core::mcp::McpTool;

    let tool = McpTool {
        name: "test_tool".to_string(),
        description: Some("A test tool".to_string()),
        input_schema: Some(serde_json::json!({"type": "object"})),
    };

    assert_eq!(tool.name, "test_tool");
    assert_eq!(tool.description, Some("A test tool".to_string()));
}

// =============================================================================
// Tool Registry Tests (Extended)
// =============================================================================

#[tokio::test]
async fn test_tool_registry_execute() {
    use nanobot_core::tools::{ReadFileTool, ToolRegistry};
    use std::io::Write;

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ReadFileTool::new(None)));

    let temp_dir = std::env::temp_dir();
    let test_file = temp_dir.join("nanobot_registry_test.txt");
    let mut file = std::fs::File::create(&test_file).unwrap();
    file.write_all(b"Test content for registry").unwrap();

    let args = serde_json::json!({
        "absolute_path": test_file.to_str().unwrap()
    });

    let result = registry.execute("read_file", args).await;
    assert!(result.is_ok());
    assert!(result.unwrap().contains("Test content for registry"));

    let _ = std::fs::remove_file(&test_file);
}

#[tokio::test]
async fn test_tool_registry_execute_not_found() {
    use nanobot_core::tools::ToolRegistry;

    let registry = ToolRegistry::new();

    let args = serde_json::json!({});
    let result = registry.execute("nonexistent_tool", args).await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("Tool not found"));
}

#[tokio::test]
async fn test_tool_registry_list() {
    use nanobot_core::tools::{ExecTool, ReadFileTool, ToolRegistry, WriteFileTool};
    use std::time::Duration;

    let mut registry = ToolRegistry::new();
    assert!(registry.list().is_empty());

    registry.register(Box::new(ReadFileTool::new(None)));
    registry.register(Box::new(WriteFileTool::new(None)));
    registry.register(Box::new(ExecTool::new(
        PathBuf::from("/tmp"),
        Duration::from_secs(60),
        false,
    )));

    let tools = registry.list();
    assert_eq!(tools.len(), 3);
    assert!(tools.contains(&"read_file"));
    assert!(tools.contains(&"write_file"));
    assert!(tools.contains(&"exec"));
}

#[tokio::test]
async fn test_tool_registry_get_definitions() {
    use nanobot_core::tools::{ReadFileTool, ToolRegistry};

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ReadFileTool::new(None)));

    let definitions = registry.get_definitions();
    assert_eq!(definitions.len(), 1);
    assert_eq!(definitions[0].function.name, "read_file");
}

// =============================================================================
// Shell Tool Tests
// =============================================================================

#[tokio::test]
async fn test_exec_tool_echo() {
    use nanobot_core::tools::ExecTool;
    use nanobot_core::Tool;
    use std::time::Duration;

    let tool = ExecTool::new("/tmp", Duration::from_secs(30), false);

    assert_eq!(tool.name(), "exec");
    assert!(tool
        .description()
        .contains("Execute an arbitrary shell command"));

    let args = serde_json::json!({
        "command": "echo 'Hello from exec tool'"
    });

    let result = tool.execute(args).await;
    assert!(result.is_ok());
    assert!(result.unwrap().contains("Hello from exec tool"));
}

#[tokio::test]
async fn test_exec_tool_with_description() {
    use nanobot_core::tools::ExecTool;
    use nanobot_core::Tool;
    use std::time::Duration;

    let tool = ExecTool::new("/tmp", Duration::from_secs(30), false);

    let args = serde_json::json!({
        "command": "echo 'test'",
        "description": "A simple test command"
    });

    let result = tool.execute(args).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_exec_tool_failed_command() {
    use nanobot_core::tools::ExecTool;
    use nanobot_core::Tool;
    use std::time::Duration;

    let tool = ExecTool::new("/tmp", Duration::from_secs(30), false);

    let args = serde_json::json!({
        "command": "ls /nonexistent_directory_12345"
    });

    let result = tool.execute(args).await;
    assert!(result.is_ok()); // Command runs but returns non-zero exit
    let output = result.unwrap();
    assert!(output.contains("exit") || output.contains("No such file"));
}

#[tokio::test]
async fn test_exec_tool_default() {
    use nanobot_core::tools::ExecTool;

    let tool = ExecTool::default();
    assert_eq!(tool.name(), "exec");
}

// =============================================================================
// Spawn Tool Tests (Extended)
// =============================================================================

#[tokio::test]
async fn test_spawn_tool_execute_no_manager() {
    use nanobot_core::tools::SpawnTool;
    use nanobot_core::Tool;

    let tool = SpawnTool::new();

    let args = serde_json::json!({
        "action": "run",
        "task": "Test background task"
    });

    // Without a manager, spawn should return an error
    let result = tool.execute(args).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not available"));
}

#[tokio::test]
async fn test_spawn_tool_list_no_manager() {
    use nanobot_core::tools::SpawnTool;
    use nanobot_core::Tool;

    let tool = SpawnTool::new();

    let args = serde_json::json!({
        "action": "list"
    });

    let result = tool.execute(args).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_spawn_tool_empty_task_no_manager() {
    use nanobot_core::tools::SpawnTool;
    use nanobot_core::Tool;

    let tool = SpawnTool::new();

    let args = serde_json::json!({
        "action": "run",
        "task": ""
    });

    // Without manager it hits the "not available" error first
    let result = tool.execute(args).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_spawn_tool_without_manager() {
    use nanobot_core::tools::SpawnTool;
    use nanobot_core::Tool;

    let tool = SpawnTool::new();

    // Spawn should fail gracefully without a manager
    let result = tool
        .execute(serde_json::json!({
            "action": "run",
            "task": "do something"
        }))
        .await;

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not available"));
}

// =============================================================================
// Cron Tool Tests (Extended)
// =============================================================================

#[tokio::test]
async fn test_cron_tool_add_job() {
    use nanobot_core::cron::CronService;
    use nanobot_core::tools::CronTool;
    use nanobot_core::Tool;
    use std::sync::Arc;

    let temp_dir = std::env::temp_dir().join("nanobot-cron-add-test");
    let _ = std::fs::create_dir_all(&temp_dir);

    let service = Arc::new(CronService::new(temp_dir.clone()));
    let tool = CronTool::new(service);
    tool.set_context("telegram", "chat123");

    let args = serde_json::json!({
        "action": "add",
        "name": "Daily Reminder",
        "cron": "0 9 * * * *",
        "message": "Time for daily standup!"
    });

    let result = tool.execute(args).await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(output.contains("Scheduled job"));
    assert!(output.contains("Daily Reminder"));

    // Cleanup
    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_cron_tool_add_missing_name() {
    use nanobot_core::cron::CronService;
    use nanobot_core::tools::CronTool;
    use nanobot_core::Tool;
    use std::sync::Arc;

    let temp_dir = std::env::temp_dir().join("nanobot-cron-missing-test");
    let _ = std::fs::create_dir_all(&temp_dir);

    let service = Arc::new(CronService::new(temp_dir.clone()));
    let tool = CronTool::new(service);

    let args = serde_json::json!({
        "action": "add",
        "cron": "0 9 * * * *",
        "message": "Test"
    });

    let result = tool.execute(args).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("name is required"));

    // Cleanup
    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_cron_tool_add_invalid_cron() {
    use nanobot_core::cron::CronService;
    use nanobot_core::tools::CronTool;
    use nanobot_core::Tool;
    use std::sync::Arc;

    let temp_dir = std::env::temp_dir().join("nanobot-cron-invalid-test");
    let _ = std::fs::create_dir_all(&temp_dir);

    let service = Arc::new(CronService::new(temp_dir.clone()));
    let tool = CronTool::new(service);

    let args = serde_json::json!({
        "action": "add",
        "name": "Bad Job",
        "cron": "not a valid cron",
        "message": "Test"
    });

    let result = tool.execute(args).await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("Invalid cron expression"));

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_cron_tool_unknown_action() {
    use nanobot_core::cron::CronService;
    use nanobot_core::tools::CronTool;
    use nanobot_core::Tool;
    use std::sync::Arc;

    let temp_dir = std::env::temp_dir().join("nanobot-cron-unknown-test");
    let _ = std::fs::create_dir_all(&temp_dir);

    let service = Arc::new(CronService::new(temp_dir.clone()));
    let tool = CronTool::new(service);

    let args = serde_json::json!({
        "action": "unknown_action"
    });

    let result = tool.execute(args).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Unknown action"));

    // Cleanup
    let _ = std::fs::remove_dir_all(&temp_dir);
}

// =============================================================================
// Message Bus Tests (Extended)
// =============================================================================

#[tokio::test]
async fn test_message_bus_outbound() {
    use nanobot_core::bus::events::{ChannelType, OutboundMessage};
    use nanobot_core::bus::MessageBus;

    let (bus, _inbound_rx, mut rx) = MessageBus::new(10);

    let outbound = OutboundMessage {
        channel: ChannelType::Cli,
        chat_id: "chat1".to_string(),
        content: "Response".to_string(),
        metadata: None,
    };

    bus.publish_outbound(outbound.clone()).await;

    let received = rx.recv().await;
    assert!(received.is_some());
    let received = received.unwrap();
    assert_eq!(received.content, "Response");
}

#[tokio::test]
async fn test_message_bus_senders() {
    use nanobot_core::bus::events::{ChannelType, InboundMessage};
    use nanobot_core::bus::MessageBus;

    let (bus, mut rx, _outbound_rx) = MessageBus::new(10);

    let inbound_sender = bus.inbound_sender();
    let _ = bus.outbound_sender();

    let msg = InboundMessage {
        channel: ChannelType::Cli,
        sender_id: "user1".to_string(),
        chat_id: "chat1".to_string(),
        content: "Hello".to_string(),
        media: None,
        metadata: None,
        timestamp: chrono::Utc::now(),
    };

    inbound_sender.send(msg).await.unwrap();

    let received = rx.recv().await;
    assert!(received.is_some());
}

#[tokio::test]
async fn test_message_bus_default() {
    use nanobot_core::bus::MessageBus;

    let (bus, _inbound_rx, _outbound_rx) = MessageBus::new(100);
    let sender = bus.inbound_sender();
    assert!(sender.capacity() > 0);
}

// =============================================================================
// Provider Base Tests (Extended)
// =============================================================================

#[tokio::test]
async fn test_chat_message_tool_result() {
    use nanobot_core::providers::ChatMessage;

    let msg = ChatMessage::tool_result("call_123", "read_file", "File content here");
    assert_eq!(msg.role, "tool");
    assert_eq!(msg.tool_call_id, Some("call_123".to_string()));
    assert_eq!(msg.name, Some("read_file".to_string()));
    assert_eq!(msg.content, Some("File content here".to_string()));
}

#[tokio::test]
async fn test_chat_message_assistant_with_tools() {
    use nanobot_core::providers::{ChatMessage, ToolCall};

    let tool_calls = vec![ToolCall::new(
        "call_1",
        "read_file",
        serde_json::json!({"path": "/tmp/test.txt"}),
    )];

    let msg =
        ChatMessage::assistant_with_tools(Some("I'll read the file.".to_string()), tool_calls);
    assert_eq!(msg.role, "assistant");
    assert!(msg.tool_calls.is_some());
    assert_eq!(msg.tool_calls.as_ref().unwrap().len(), 1);
}

#[tokio::test]
async fn test_tool_definition() {
    use nanobot_core::providers::ToolDefinition;

    let def = ToolDefinition::function(
        "read_file",
        "Read a file from the filesystem",
        serde_json::json!({"type": "object"}),
    );

    assert_eq!(def.tool_type, "function");
    assert_eq!(def.function.name, "read_file");
    assert_eq!(def.function.description, "Read a file from the filesystem");
}

#[tokio::test]
async fn test_tool_call() {
    use nanobot_core::providers::ToolCall;

    let call = ToolCall::new(
        "call_abc123",
        "write_file",
        serde_json::json!({"path": "/tmp/test.txt", "content": "Hello"}),
    );

    assert_eq!(call.id, "call_abc123");
    assert_eq!(call.tool_type, "function");
    assert_eq!(call.function.name, "write_file");
    assert_eq!(call.function.arguments["path"], "/tmp/test.txt");
}

#[tokio::test]
async fn test_chat_response_text() {
    use nanobot_core::providers::ChatResponse;

    let response = ChatResponse::text("Hello, I'm the assistant.");
    assert_eq!(
        response.content,
        Some("Hello, I'm the assistant.".to_string())
    );
    assert!(response.tool_calls.is_empty());
    assert!(!response.has_tool_calls());
}

#[tokio::test]
async fn test_chat_response_tool_calls() {
    use nanobot_core::providers::{ChatResponse, ToolCall};

    let tool_calls = vec![ToolCall::new(
        "call_1",
        "exec",
        serde_json::json!({"command": "ls"}),
    )];

    let response = ChatResponse::tool_calls(tool_calls);
    assert!(response.content.is_none());
    assert!(!response.tool_calls.is_empty());
    assert!(response.has_tool_calls());
}

// =============================================================================
// Config Loader Tests (Extended)
// =============================================================================

#[tokio::test]
async fn test_config_loader_with_dir() {
    use nanobot_core::config::ConfigLoader;

    let temp_dir = std::env::temp_dir().join("nanobot-config-test-dir");
    let _ = std::fs::create_dir_all(&temp_dir);

    let loader = ConfigLoader::with_dir(temp_dir.clone());
    assert!(loader.config_path().ends_with("config.yaml"));
    assert!(!loader.exists());

    // Cleanup
    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_config_loader_save_and_load() {
    use nanobot_core::config::{Config, ConfigLoader};

    let temp_dir = std::env::temp_dir().join("nanobot-config-save-test");
    let _ = std::fs::create_dir_all(&temp_dir);

    let loader = ConfigLoader::with_dir(temp_dir.clone());

    // Create and save a config
    let mut config = Config::default();
    config.providers.insert(
        "test_provider".to_string(),
        nanobot_core::config::ProviderConfig {
            api_key: Some("test-key".to_string()),
            api_base: None,
        },
    );

    let save_result = loader.save(&config);
    assert!(save_result.is_ok());
    assert!(loader.exists());

    // Load it back
    let loaded = loader.load().unwrap();
    assert!(loaded.providers.contains_key("test_provider"));

    // Cleanup
    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_config_loader_load_nonexistent() {
    use nanobot_core::config::ConfigLoader;

    let temp_dir = std::env::temp_dir().join("nanobot-config-nonexistent-test");
    let _ = std::fs::remove_dir_all(&temp_dir);
    let _ = std::fs::create_dir_all(&temp_dir);

    let loader = ConfigLoader::with_dir(temp_dir.clone());

    // Should return default config when file doesn't exist
    let config = loader.load().unwrap();
    assert!(config.providers.is_empty());

    // Cleanup
    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_config_loader_init_default() {
    use nanobot_core::config::ConfigLoader;

    let temp_dir = std::env::temp_dir().join("nanobot-config-init-test");
    let _ = std::fs::remove_dir_all(&temp_dir);
    let _ = std::fs::create_dir_all(&temp_dir);

    let loader = ConfigLoader::with_dir(temp_dir.clone());

    let _ = loader.init_default().unwrap();
    assert!(loader.exists());

    // Cleanup
    let _ = std::fs::remove_dir_all(&temp_dir);
}

// =============================================================================
// Session Manager Tests (Extended)
// =============================================================================

#[tokio::test]
async fn test_session_manager_save() {
    use nanobot_core::session::{Session, SessionManager};

    let temp_dir = std::env::temp_dir().join("nanobot-session-save-test");
    let _ = std::fs::remove_dir_all(&temp_dir);
    let _ = std::fs::create_dir_all(&temp_dir);

    let manager = SessionManager::new(temp_dir.clone());

    let mut session = Session::new("test:save");
    session.add_message("user", "Hello", None);
    session.add_message("assistant", "Hi!", None);

    manager.save(&session).await;

    // Invalidate and reload
    manager.invalidate("test:save").await;
    let loaded = manager.get_or_create("test:save").await;
    assert_eq!(loaded.messages.len(), 2);

    // Cleanup
    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_session_manager_invalidate() {
    use nanobot_core::session::SessionManager;

    let temp_dir = std::env::temp_dir().join("nanobot-session-invalidate-test");
    let _ = std::fs::remove_dir_all(&temp_dir);
    let _ = std::fs::create_dir_all(&temp_dir);

    let manager = SessionManager::new(temp_dir.clone());

    let mut session = manager.get_or_create("test:invalidate").await;
    session.add_message("user", "Test", None);
    manager.save(&session).await;

    manager.invalidate("test:invalidate").await;

    // Get again - should create new (empty) session
    let session = manager.get_or_create("test:invalidate").await;
    // After invalidation, it loads from disk if exists, or creates new
    assert!(!session.messages.is_empty()); // Loaded from disk

    // Cleanup
    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_session_get_history() {
    use nanobot_core::session::Session;

    let mut session = Session::new("test:history");
    for i in 0..10 {
        session.add_message("user", &format!("Message {}", i), None);
    }

    // Get last 5 messages
    let history = session.get_history(5);
    assert_eq!(history.len(), 5);
    assert_eq!(history[0].content, "Message 5");
    assert_eq!(history[4].content, "Message 9");
}

// =============================================================================
// Context Builder Tests
// =============================================================================

#[tokio::test]
async fn test_context_builder_new() {
    use nanobot_core::agent::context::ContextBuilder;

    let builder = ContextBuilder::new(PathBuf::from("/tmp/workspace")).unwrap();
    let messages = builder.build_messages(vec![], "test", None, "test", "chat1");
    assert!(!messages.is_empty());
}

#[tokio::test]
async fn test_context_builder_with_system_prompt() {
    use nanobot_core::agent::context::ContextBuilder;

    let builder = ContextBuilder::new(PathBuf::from("/tmp"))
        .unwrap()
        .with_system_prompt("Custom system prompt");

    let messages = builder.build_messages(vec![], "Hello", None, "test", "chat1");
    assert_eq!(messages[0].role, "system");
    assert!(messages[0]
        .content
        .as_ref()
        .unwrap()
        .contains("Custom system prompt"));
}

#[tokio::test]
async fn test_context_builder_build_messages() {
    use nanobot_core::agent::context::ContextBuilder;
    use nanobot_core::session::SessionMessage;

    let builder = ContextBuilder::new(PathBuf::from("/tmp")).unwrap();

    let history = vec![
        SessionMessage {
            role: "user".to_string(),
            content: "Previous question".to_string(),
            timestamp: chrono::Utc::now(),
            tools_used: None,
        },
        SessionMessage {
            role: "assistant".to_string(),
            content: "Previous answer".to_string(),
            timestamp: chrono::Utc::now(),
            tools_used: None,
        },
    ];

    let messages = builder.build_messages(
        history,
        "Current question",
        Some("Long term memory"),
        "test",
        "chat1",
    );

    assert_eq!(messages.len(), 4); // system + 2 history + current
    assert_eq!(messages[0].role, "system");
    assert!(messages[0]
        .content
        .as_ref()
        .unwrap()
        .contains("Long term memory"));
    assert_eq!(messages[1].role, "user");
    assert_eq!(messages[1].content.as_ref().unwrap(), "Previous question");
    assert_eq!(messages[2].role, "assistant");
    assert_eq!(messages[3].role, "user");
    assert_eq!(messages[3].content.as_ref().unwrap(), "Current question");
}

#[tokio::test]
async fn test_context_builder_add_assistant_message() {
    use nanobot_core::agent::context::ContextBuilder;
    use nanobot_core::providers::ChatMessage;

    let builder = ContextBuilder::new(PathBuf::from("/tmp")).unwrap();

    let mut messages = vec![ChatMessage::user("Hello")];
    builder.add_assistant_message(&mut messages, Some("Hi there!".to_string()), vec![], None);

    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1].role, "assistant");
}

#[tokio::test]
async fn test_context_builder_add_tool_result() {
    use nanobot_core::agent::context::ContextBuilder;
    use nanobot_core::providers::ChatMessage;

    let builder = ContextBuilder::new(PathBuf::from("/tmp")).unwrap();

    let mut messages = vec![ChatMessage::user("Read the file")];
    builder.add_tool_result(
        &mut messages,
        "call_123".to_string(),
        "read_file".to_string(),
        "File content".to_string(),
    );

    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1].role, "tool");
    assert_eq!(messages[1].tool_call_id, Some("call_123".to_string()));
}

// =============================================================================
// Cron Service Tests (Extended)
// =============================================================================

#[tokio::test]
async fn test_cron_service_add_and_get() {
    use nanobot_core::cron::{CronJob, CronService};

    let temp_dir = std::env::temp_dir().join("nanobot-cron-service-add-test");
    let _ = std::fs::remove_dir_all(&temp_dir);
    let _ = std::fs::create_dir_all(&temp_dir);

    let service = CronService::new(temp_dir.clone());

    let job = CronJob::new("job-1", "Test Job", "0 9 * * * *", "Test message");
    service.add_job(job.clone()).await.unwrap();

    let retrieved = service.get_job("job-1").await;
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().name, "Test Job");

    // Cleanup
    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_cron_service_remove() {
    use nanobot_core::cron::{CronJob, CronService};

    let temp_dir = std::env::temp_dir().join("nanobot-cron-service-remove-test");
    let _ = std::fs::remove_dir_all(&temp_dir);
    let _ = std::fs::create_dir_all(&temp_dir);

    let service = CronService::new(temp_dir.clone());

    let job = CronJob::new("job-to-remove", "Test", "0 9 * * * *", "Message");
    service.add_job(job).await.unwrap();

    let removed = service.remove_job("job-to-remove").await.unwrap();
    assert!(removed);

    let removed_again = service.remove_job("job-to-remove").await.unwrap();
    assert!(!removed_again);

    // Cleanup
    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_cron_service_list() {
    use nanobot_core::cron::{CronJob, CronService};

    let temp_dir = std::env::temp_dir().join("nanobot-cron-service-list-test");
    let _ = std::fs::remove_dir_all(&temp_dir);
    let _ = std::fs::create_dir_all(&temp_dir);

    let service = CronService::new(temp_dir.clone());

    // Initially empty
    let jobs = service.list_jobs().await;
    assert!(jobs.is_empty());

    // Add jobs
    service
        .add_job(CronJob::new("job-1", "Job 1", "0 9 * * * *", "M1"))
        .await
        .unwrap();
    service
        .add_job(CronJob::new("job-2", "Job 2", "0 10 * * * *", "M2"))
        .await
        .unwrap();

    let jobs = service.list_jobs().await;
    assert_eq!(jobs.len(), 2);

    // Cleanup
    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_cron_job_update_next_run() {
    use nanobot_core::cron::CronJob;

    let mut job = CronJob::new("id", "Test", "* * * * * *", "Message");

    let _ = job.next_run.clone();
    job.update_next_run();

    assert!(job.last_run.is_some());
    // next_run should be recalculated
    assert!(job.next_run.is_some());
}

#[tokio::test]
async fn test_cron_service_mark_job_run() {
    use nanobot_core::cron::{CronJob, CronService};

    let temp_dir = std::env::temp_dir().join("nanobot-cron-service-mark-test");
    let _ = std::fs::remove_dir_all(&temp_dir);
    let _ = std::fs::create_dir_all(&temp_dir);

    let service = CronService::new(temp_dir.clone());

    let job = CronJob::new("job-to-mark", "Test", "* * * * * *", "Message");
    service.add_job(job).await.unwrap();

    service.mark_job_run("job-to-mark").await;

    let job = service.get_job("job-to-mark").await.unwrap();
    assert!(job.last_run.is_some());

    // Cleanup
    let _ = std::fs::remove_dir_all(&temp_dir);
}

// =============================================================================
// Memory Store Tests (Extended)
// =============================================================================

#[tokio::test]
async fn test_memory_store_read_empty() {
    use nanobot_core::agent::MemoryStore;

    let temp_dir = std::env::temp_dir().join("nanobot-memory-empty-test");
    let _ = std::fs::remove_dir_all(&temp_dir);
    let _ = std::fs::create_dir_all(&temp_dir);

    let memory = MemoryStore::new(temp_dir.clone());

    // Reading non-existent memory should return empty string
    let content = memory.read_long_term().unwrap();
    assert!(content.is_empty());

    let history = memory.read_history().unwrap();
    assert!(history.is_empty());

    // Cleanup
    let _ = std::fs::remove_dir_all(&temp_dir);
}

// =============================================================================
// Heartbeat Service Tests (Extended)
// =============================================================================

#[tokio::test]
async fn test_heartbeat_service_workspace() {
    use nanobot_core::heartbeat::HeartbeatService;

    let workspace = PathBuf::from("/tmp/nanobot-heartbeat-workspace");
    let service = HeartbeatService::new(workspace.clone());

    assert_eq!(*service.workspace(), workspace);
}
