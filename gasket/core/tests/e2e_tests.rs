//! End-to-end integration tests

use std::path::PathBuf;
use std::sync::Arc;

use gasket_core::bus::events::SessionKey;
use gasket_core::bus::ChannelType;
use gasket_core::providers::MessageRole;
use gasket_core::LlmProvider;
use gasket_core::Tool;

/// Create a test sender for inbound messages (middleware-aware).
/// The receiver is leaked to keep the channel open for the test duration.
#[allow(dead_code)]
fn create_test_sender() -> gasket_core::channels::InboundSender {
    let (tx, rx) = tokio::sync::mpsc::channel(100);
    std::mem::forget(rx);
    gasket_core::channels::InboundSender::new(tx)
}

/// Create a raw mpsc sender for channels not yet migrated to InboundSender.
#[allow(dead_code)]
fn create_raw_test_sender() -> tokio::sync::mpsc::Sender<gasket_core::bus::events::InboundMessage> {
    let (tx, rx) = tokio::sync::mpsc::channel(100);
    std::mem::forget(rx);
    tx
}

// =============================================================================
// Agent Tests
// =============================================================================

#[tokio::test]
async fn test_agent_initialization() {
    let workspace = PathBuf::from("/tmp/nanobot-test");

    let config = gasket_core::agent::AgentConfig {
        model: "gpt-4o".to_string(),
        max_iterations: 10,
        temperature: 0.7,
        max_tokens: 1024,
        memory_window: 20,
        max_tool_result_chars: 8000,
        thinking_enabled: false,
        streaming: false,
        subagent_timeout_secs: 10,
        session_idle_timeout_secs: 300,
    };

    let provider = gasket_core::providers::OpenAICompatibleProvider::from_name(
        "openai",
        "test-key",
        None,
        Some("gpt-4o".to_string()),
        true,
    )
    .expect("openai should be known provider");

    let tools = Arc::new(gasket_core::tools::ToolRegistry::new());
    let agent =
        gasket_core::agent::AgentLoop::new(Arc::new(provider), workspace.clone(), config, tools)
            .await
            .unwrap();

    assert_eq!(agent.model(), "gpt-4o");
    assert_eq!(agent.workspace(), &workspace);
}

#[tokio::test]
async fn test_agent_config_default() {
    use gasket_core::agent::AgentConfig;

    let config = AgentConfig::default();
    assert_eq!(config.model, "gpt-4o");
    assert_eq!(config.max_iterations, 20);
    assert_eq!(config.temperature, 1.0);
    assert_eq!(config.max_tokens, 65536);
    assert_eq!(config.memory_window, 50);
}

// =============================================================================
// Message Bus Tests
// =============================================================================

#[tokio::test]
async fn test_message_bus() {
    use gasket_core::bus::events::InboundMessage;
    use gasket_core::bus::{ChannelType, MessageBus};

    let (bus, mut rx, _ox) = MessageBus::new(10);

    let inbound = InboundMessage {
        channel: ChannelType::Cli,
        sender_id: "user1".to_string(),
        chat_id: "chat1".to_string(),
        content: "Hello".to_string(),
        media: None,
        metadata: None,
        timestamp: chrono::Utc::now(),
        trace_id: None,
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
    use gasket_core::bus::events::InboundMessage;
    use gasket_core::bus::ChannelType;

    let msg = InboundMessage {
        channel: ChannelType::Telegram,
        sender_id: "user123".to_string(),
        chat_id: "chat456".to_string(),
        content: "Test".to_string(),
        media: None,
        metadata: None,
        timestamp: chrono::Utc::now(),
        trace_id: None,
    };

    assert_eq!(
        msg.session_key(),
        SessionKey::new(gasket_core::bus::ChannelType::Telegram, "chat456")
    );
}

#[tokio::test]
async fn test_outbound_message() {
    use gasket_core::bus::events::OutboundMessage;
    use gasket_core::bus::ChannelType;

    let outbound = OutboundMessage {
        channel: ChannelType::Discord,
        chat_id: "channel789".to_string(),
        content: "Response".to_string(),
        metadata: Some(serde_json::json!({"thread_ts": "12345"})),
        trace_id: None,
        ws_message: None,
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
    use gasket_core::memory::SqliteStore;
    use gasket_core::session::SessionManager;

    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::with_path(dir.path().join("test.db"))
        .await
        .unwrap();
    let manager = SessionManager::new(store);

    let mut session = manager
        .get_or_create(&SessionKey::from("test:session1"))
        .await;
    assert_eq!(session.key, "test:session1");

    session.add_message(MessageRole::User, "Hello", None);
    session.add_message(MessageRole::Assistant, "Hi there!", None);

    let history = session.get_history(10);
    assert_eq!(history.len(), 2);
}

#[tokio::test]
async fn test_session_clear() {
    use gasket_core::memory::SqliteStore;
    use gasket_core::session::SessionManager;

    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::with_path(dir.path().join("test.db"))
        .await
        .unwrap();
    let manager = SessionManager::new(store);

    let key = SessionKey {
        channel: ChannelType::Cli,
        chat_id: "clear".to_string(),
    };

    let mut session = manager.get_or_create(&key).await;
    session.add_message(MessageRole::User, "Hello", None);
    assert!(!session.messages.is_empty());

    session.clear();
    assert!(session.messages.is_empty());
}

#[tokio::test]
async fn test_session_tools_used() {
    use gasket_core::memory::SqliteStore;
    use gasket_core::session::SessionManager;

    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::with_path(dir.path().join("test.db"))
        .await
        .unwrap();
    let manager = SessionManager::new(store);

    let key = SessionKey {
        channel: ChannelType::Cli,
        chat_id: "tools".to_string(),
    };

    let mut session = manager.get_or_create(&key).await;
    session.add_message(
        MessageRole::Assistant,
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
    use gasket_core::tools::{ReadFileTool, ToolRegistry};

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
    use gasket_core::tools::{ExecTool, ReadFileTool, ToolRegistry, WriteFileTool};
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
    use gasket_core::tools::simple_schema;

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

#[test]
fn test_simple_schema_with_array() {
    use gasket_core::tools::simple_schema;

    // Test array type with default string items
    let schema = simple_schema(&[
        ("tags", "array", false, "List of tags"),
        ("ids", "array<integer>", false, "List of IDs"),
    ]);

    // Verify tags array has items with string type
    let tags_prop = &schema["properties"]["tags"];
    assert!(tags_prop.is_object());
    assert_eq!(tags_prop["type"], "array");
    assert_eq!(tags_prop["items"]["type"], "string");
    assert_eq!(tags_prop["description"], "List of tags");

    // Verify ids array has items with integer type
    let ids_prop = &schema["properties"]["ids"];
    assert!(ids_prop.is_object());
    assert_eq!(ids_prop["type"], "array");
    assert_eq!(ids_prop["items"]["type"], "integer");
}

// =============================================================================
// Config Tests
// =============================================================================

#[tokio::test]
async fn test_config_defaults() {
    use gasket_core::config::Config;

    let config = Config::default();

    // Agent defaults have sensible default values
    assert_eq!(config.agents.defaults.temperature, 0.7);
    assert_eq!(config.agents.defaults.max_tokens, 4096);
    assert_eq!(config.agents.defaults.max_iterations, 20);
    assert_eq!(config.agents.defaults.memory_window, 50);
    assert!(config.agents.defaults.streaming);
    assert!(!config.agents.defaults.thinking_enabled);
    assert!(!config.tools.restrict_to_workspace);
}

#[tokio::test]
async fn test_config_deserialization() {
    use gasket_core::config::Config;

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
    use gasket_core::config::Config;

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
    use gasket_core::cron::CronJob;

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
    use gasket_core::cron::CronJob;

    let mut job = CronJob::new("id1", "Name", "0 0 9 * * *", "Message");
    job.channel = Some("telegram".to_string());
    job.chat_id = Some("chat123".to_string());

    assert_eq!(job.channel, Some("telegram".to_string()));
    assert_eq!(job.chat_id, Some("chat123".to_string()));
}

// =============================================================================
// Web Tool Tests
// =============================================================================

#[tokio::test]
async fn test_web_search_tool_schema() {
    use gasket_core::tools::WebSearchTool;

    let tool = WebSearchTool::new(None);

    assert_eq!(tool.name(), "web_search");

    let params = tool.parameters();
    assert!(params["properties"]["query"].is_object());
    assert!(params["properties"]["count"].is_object());
}

#[tokio::test]
async fn test_web_fetch_tool_schema() {
    use gasket_core::tools::WebFetchTool;

    let tool = WebFetchTool::new();

    assert_eq!(tool.name(), "web_fetch");

    let params = tool.parameters();
    assert!(params["properties"]["url"].is_object());
    assert!(params["properties"]["prompt"].is_object());
}

#[tokio::test]
async fn test_web_fetch_tool_with_options() {
    use gasket_core::tools::WebFetchTool;

    let tool = WebFetchTool::new()
        .with_timeout(60)
        .with_max_size(5_000_000);

    assert_eq!(tool.name(), "web_fetch");
}

// =============================================================================
// Cron Tool Tests
// =============================================================================

#[tokio::test]
async fn test_cron_tool_schema() {
    use gasket_core::cron::CronService;
    use gasket_core::tools::CronTool;
    use std::sync::Arc;

    let service = Arc::new(CronService::new(PathBuf::from("/tmp/test-cron.json")).await);
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
    use gasket_core::providers::LlmProvider;
    use gasket_core::providers::OpenAICompatibleProvider;

    let provider = OpenAICompatibleProvider::from_name(
        "openai",
        "test-key",
        None,
        Some("gpt-4o".to_string()),
        true,
    )
    .expect("openai should be known provider");

    assert_eq!(provider.name(), "openai");
    assert_eq!(provider.default_model(), "gpt-4o");
}

#[tokio::test]
async fn test_openrouter_provider() {
    use gasket_core::providers::OpenAICompatibleProvider;

    let provider = OpenAICompatibleProvider::from_name(
        "openrouter",
        "sk-or-test",
        None,
        Some("anthropic/claude-sonnet-4".to_string()),
        true,
    )
    .expect("openrouter should be known provider");

    assert_eq!(provider.name(), "openrouter");
    assert_eq!(provider.default_model(), "anthropic/claude-sonnet-4");
}

#[tokio::test]
async fn test_anthropic_provider() {
    use gasket_core::providers::OpenAICompatibleProvider;

    let provider = OpenAICompatibleProvider::from_name(
        "anthropic",
        "sk-ant-test",
        None,
        Some("claude-sonnet-4-20250514".to_string()),
        true,
    )
    .expect("anthropic should be known provider");

    assert_eq!(provider.name(), "anthropic");
    assert_eq!(provider.default_model(), "claude-sonnet-4-20250514");
}

// =============================================================================
// Chat Message Tests
// =============================================================================

#[tokio::test]
async fn test_chat_message_user() {
    use gasket_core::providers::ChatMessage;

    let msg = ChatMessage::user("Hello");
    assert_eq!(msg.role, MessageRole::User);
    assert_eq!(msg.content, Some("Hello".to_string()));
}

#[tokio::test]
async fn test_chat_message_assistant() {
    use gasket_core::providers::ChatMessage;

    let msg = ChatMessage::assistant("Hi there!");
    assert_eq!(msg.role, MessageRole::Assistant);
    assert_eq!(msg.content, Some("Hi there!".to_string()));
}

#[tokio::test]
async fn test_chat_message_system() {
    use gasket_core::providers::ChatMessage;

    let msg = ChatMessage::system("You are a helpful assistant.");
    assert_eq!(msg.role, MessageRole::System);
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
    use gasket_core::heartbeat::HeartbeatService;

    let workspace = PathBuf::from("/tmp/nanobot-heartbeat-test");
    let service = HeartbeatService::new(workspace);

    // Just verify it can be created
    assert!(service.workspace().ends_with("nanobot-heartbeat-test"));
}

// =============================================================================
// Channel Error Tests
// =============================================================================

#[tokio::test]
async fn test_channel_error_types() {
    use gasket_core::channels::middleware::ChannelError;

    let err = ChannelError::NotConnected {
        channel: "telegram".to_string(),
    };
    assert_eq!(err.channel(), Some("telegram"));
    assert!(!err.is_retryable());

    let err = ChannelError::AuthError {
        channel: "discord".to_string(),
        message: "invalid token".to_string(),
    };
    assert_eq!(err.channel(), Some("discord"));
    assert!(!err.is_retryable());

    let err = ChannelError::RateLimited {
        channel: "slack".to_string(),
    };
    assert!(err.is_retryable());

    let err = ChannelError::DeliveryFailed {
        channel: "email".to_string(),
        message: "SMTP error".to_string(),
    };
    assert!(err.is_retryable());

    let err = ChannelError::InvalidFormat("bad format".to_string());
    assert!(err.channel().is_none());
}

// =============================================================================
// Channel Rate Limiter Tests
// =============================================================================

#[tokio::test]
async fn test_simple_rate_limiter() {
    use gasket_core::channels::middleware::SimpleRateLimiter;
    use std::time::Duration;

    let rl = SimpleRateLimiter::new(3, Duration::from_secs(60));

    // First 3 should pass
    assert!(rl.check("user1"));
    assert!(rl.check("user1"));
    assert!(rl.check("user1"));

    // 4th should be rate limited
    assert!(!rl.check("user1"));

    // Different user should still pass
    assert!(rl.check("user2"));
}

#[tokio::test]
async fn test_simple_auth_checker() {
    use gasket_core::channels::middleware::SimpleAuthChecker;

    let auth = SimpleAuthChecker::new(vec!["user1".to_string(), "user2".to_string()]);

    assert!(auth.is_allowed("user1"));
    assert!(auth.is_allowed("user2"));
    assert!(!auth.is_allowed("unknown"));
}

#[tokio::test]
async fn test_simple_auth_checker_empty_allows_all() {
    use gasket_core::channels::middleware::SimpleAuthChecker;

    let auth = SimpleAuthChecker::new(Vec::<String>::new());

    assert!(auth.is_allowed("anyone"));
    assert!(auth.is_allowed("user1"));
}

// =============================================================================
// Telegram Channel Tests
// =============================================================================

#[cfg(feature = "telegram")]
#[tokio::test]
async fn test_telegram_config_creation() {
    use gasket_core::channels::telegram::TelegramConfig;

    let config = TelegramConfig {
        token: "bot123456:ABC-DEF".to_string(),
        allow_from: vec!["user1".to_string(), "user2".to_string()],
    };

    assert_eq!(config.token, "bot123456:ABC-DEF");
    assert_eq!(config.allow_from.len(), 2);
}

#[cfg(feature = "telegram")]
#[tokio::test]
async fn test_telegram_channel_creation() {
    use gasket_core::channels::telegram::{TelegramChannel, TelegramConfig};
    use gasket_core::channels::Channel;

    let config = TelegramConfig {
        token: "test-token".to_string(),
        allow_from: vec![],
    };

    let channel = TelegramChannel::new(config, create_raw_test_sender());

    assert_eq!(channel.name(), "telegram");
}

#[cfg(feature = "telegram")]
#[tokio::test]
async fn test_telegram_channel_lifecycle() {
    use gasket_core::channels::base::Channel;
    use gasket_core::channels::telegram::{TelegramChannel, TelegramConfig};

    let config = TelegramConfig {
        token: "test-token".to_string(),
        allow_from: vec![],
    };

    let mut channel = TelegramChannel::new(config, create_raw_test_sender());

    // Test start (trait method)
    let start_result = Channel::start(&mut channel).await;
    assert!(start_result.is_ok());

    // Test stop
    let stop_result = channel.stop().await;
    assert!(stop_result.is_ok());

    // Test graceful shutdown
    let shutdown_result = channel.graceful_shutdown().await;
    assert!(shutdown_result.is_ok());
}

// =============================================================================
// Discord Channel Tests
// =============================================================================

#[cfg(feature = "discord")]
#[tokio::test]
async fn test_discord_config_creation() {
    use gasket_core::channels::discord::DiscordConfig;

    let config = DiscordConfig {
        token: "discord-bot-token".to_string(),
        allow_from: vec!["123456789".to_string()],
    };

    assert_eq!(config.token, "discord-bot-token");
    assert_eq!(config.allow_from, vec!["123456789"]);
}

#[cfg(feature = "discord")]
#[tokio::test]
async fn test_discord_channel_creation() {
    use gasket_core::channels::discord::{DiscordChannel, DiscordConfig};
    use gasket_core::channels::Channel;

    let config = DiscordConfig {
        token: "test-token".to_string(),
        allow_from: vec![],
    };

    let channel = DiscordChannel::new(config, create_raw_test_sender());

    assert_eq!(channel.name(), "discord");
}

#[cfg(feature = "discord")]
#[tokio::test]
async fn test_discord_channel_lifecycle() {
    use gasket_core::channels::base::Channel;
    use gasket_core::channels::discord::{DiscordChannel, DiscordConfig};

    let config = DiscordConfig {
        token: "test-token".to_string(),
        allow_from: vec![],
    };

    let mut channel = DiscordChannel::new(config, create_raw_test_sender());

    // Test start
    let start_result = channel.start().await;
    assert!(start_result.is_ok());

    // Test stop
    let stop_result = channel.stop().await;
    assert!(stop_result.is_ok());
}

// =============================================================================
// Slack Channel Tests
// =============================================================================

#[cfg(feature = "slack")]
#[tokio::test]
async fn test_slack_config_creation() {
    use gasket_core::channels::slack::SlackConfig;

    let config = SlackConfig {
        bot_token: "xoxb-test-token".to_string(),
        app_token: "xapp-test-token".to_string(),
        group_policy: Some("open".to_string()),
        allow_from: vec!["U12345".to_string()],
    };

    assert_eq!(config.bot_token, "xoxb-test-token");
    assert_eq!(config.app_token, "xapp-test-token");
    assert_eq!(config.group_policy, Some("open".to_string()));
}

#[cfg(feature = "slack")]
#[tokio::test]
async fn test_slack_channel_creation() {
    use gasket_core::channels::slack::{SlackChannel, SlackConfig};
    use gasket_core::channels::Channel;

    let config = SlackConfig {
        bot_token: "test-bot-token".to_string(),
        app_token: "test-app-token".to_string(),
        group_policy: None,
        allow_from: vec![],
    };

    let channel = SlackChannel::new(config, create_raw_test_sender());

    assert_eq!(channel.name(), "slack");
}

#[cfg(feature = "slack")]
#[tokio::test]
async fn test_slack_channel_lifecycle() {
    use gasket_core::channels::base::Channel;
    use gasket_core::channels::slack::{SlackChannel, SlackConfig};

    let config = SlackConfig {
        bot_token: "test-bot-token".to_string(),
        app_token: "test-app-token".to_string(),
        group_policy: None,
        allow_from: vec![],
    };

    let mut channel = SlackChannel::new(config, create_raw_test_sender());

    // Test start
    let start_result = channel.start().await;
    assert!(start_result.is_ok());

    // Test stop
    let stop_result = channel.stop().await;
    assert!(stop_result.is_ok());
}

#[cfg(feature = "slack")]
#[tokio::test]
async fn test_slack_message_serialization() {
    // Test Slack message JSON format
    use serde_json::json;

    let msg = json!({
        "text": "Hello Slack!",
        "user": "U12345",
        "channel": "C12345",
        "ts": "1234567890.123456",
        "thread_ts": "1234567890.000000"
    });

    assert!(msg["text"].as_str().unwrap() == "Hello Slack!");
    assert!(msg["user"].as_str().unwrap() == "U12345");
}

// =============================================================================
// Email Channel Tests
// =============================================================================

#[cfg(feature = "email")]
#[tokio::test]
async fn test_email_config_creation() {
    use gasket_core::channels::email::EmailConfig;

    let config = EmailConfig {
        imap_host: "imap.example.com".to_string(),
        imap_port: 993,
        imap_username: "user@example.com".to_string(),
        imap_password: "password".to_string(),
        smtp_host: "smtp.example.com".to_string(),
        smtp_port: 587,
        smtp_username: "user@example.com".to_string(),
        smtp_password: "password".to_string(),
        from_address: "bot@example.com".to_string(),
        allow_from: vec!["allowed@example.com".to_string()],
        consent_granted: true,
    };

    assert_eq!(config.imap_host, "imap.example.com");
    assert_eq!(config.imap_port, 993);
    assert_eq!(config.smtp_host, "smtp.example.com");
    assert!(config.consent_granted);
}

#[cfg(feature = "email")]
#[tokio::test]
async fn test_email_channel_creation() {
    use gasket_core::channels::email::{EmailChannel, EmailConfig};
    use gasket_core::channels::Channel;

    let config = EmailConfig {
        imap_host: "imap.example.com".to_string(),
        imap_port: 993,
        imap_username: "user@example.com".to_string(),
        imap_password: "password".to_string(),
        smtp_host: "smtp.example.com".to_string(),
        smtp_port: 587,
        smtp_username: "user@example.com".to_string(),
        smtp_password: "password".to_string(),
        from_address: "bot@example.com".to_string(),
        allow_from: vec![],
        consent_granted: false,
    };

    let channel = EmailChannel::new(config, create_raw_test_sender());

    assert_eq!(channel.name(), "email");
}

#[cfg(feature = "email")]
#[tokio::test]
async fn test_email_channel_lifecycle() {
    use gasket_core::channels::base::Channel;
    use gasket_core::channels::email::{EmailChannel, EmailConfig};

    let config = EmailConfig {
        imap_host: "imap.example.com".to_string(),
        imap_port: 993,
        imap_username: "user@example.com".to_string(),
        imap_password: "password".to_string(),
        smtp_host: "smtp.example.com".to_string(),
        smtp_port: 587,
        smtp_username: "user@example.com".to_string(),
        smtp_password: "password".to_string(),
        from_address: "bot@example.com".to_string(),
        allow_from: vec![],
        consent_granted: false,
    };

    let mut channel = EmailChannel::new(config, create_raw_test_sender());

    // Test start
    let start_result = channel.start().await;
    assert!(start_result.is_ok());

    // Test stop
    let stop_result = channel.stop().await;
    assert!(stop_result.is_ok());
}

#[cfg(feature = "email")]
#[tokio::test]
async fn test_email_channel_poll_without_consent() {
    use gasket_core::channels::email::{EmailChannel, EmailConfig};

    let config = EmailConfig {
        imap_host: "imap.example.com".to_string(),
        imap_port: 993,
        imap_username: "user@example.com".to_string(),
        imap_password: "password".to_string(),
        smtp_host: "smtp.example.com".to_string(),
        smtp_port: 587,
        smtp_username: "user@example.com".to_string(),
        smtp_password: "password".to_string(),
        from_address: "bot@example.com".to_string(),
        allow_from: vec![],
        consent_granted: false, // No consent
    };

    let channel = EmailChannel::new(config, create_raw_test_sender());

    // Without consent, poll should return empty vec
    let result = channel.poll().await.unwrap();
    assert!(result.is_empty());
}

// =============================================================================
// DingTalk Channel Tests
// =============================================================================

#[cfg(feature = "dingtalk")]
#[tokio::test]
async fn test_dingtalk_config_creation() {
    use gasket_core::channels::dingtalk::DingTalkConfig;

    let config = DingTalkConfig {
        webhook_url: "https://oapi.dingtalk.com/robot/send?access_token=test123".to_string(),
        secret: Some("SEC123".to_string()),
        access_token: None,
        allow_from: vec!["user1".to_string()],
    };

    assert_eq!(
        config.webhook_url,
        "https://oapi.dingtalk.com/robot/send?access_token=test123"
    );
    assert_eq!(config.secret, Some("SEC123".to_string()));
}

#[cfg(feature = "dingtalk")]
#[tokio::test]
async fn test_dingtalk_channel_creation() {
    use gasket_core::channels::dingtalk::{DingTalkChannel, DingTalkConfig};
    use gasket_core::channels::Channel;

    let config = DingTalkConfig {
        webhook_url: "https://oapi.dingtalk.com/robot/send?access_token=test".to_string(),
        secret: None,
        access_token: None,
        allow_from: vec![],
    };

    let channel = DingTalkChannel::new(config, create_test_sender());

    assert_eq!(channel.name(), "dingtalk");
}

#[cfg(feature = "dingtalk")]
#[tokio::test]
async fn test_dingtalk_channel_lifecycle() {
    use gasket_core::channels::base::Channel;
    use gasket_core::channels::dingtalk::{DingTalkChannel, DingTalkConfig};

    let config = DingTalkConfig {
        webhook_url: "https://oapi.dingtalk.com/robot/send?access_token=test".to_string(),
        secret: None,
        access_token: None,
        allow_from: vec![],
    };

    let mut channel = DingTalkChannel::new(config, create_test_sender());

    // Test start
    let start_result = channel.start().await;
    assert!(start_result.is_ok());

    // Test stop
    let stop_result = channel.stop().await;
    assert!(stop_result.is_ok());
}

#[cfg(feature = "dingtalk")]
#[tokio::test]
async fn test_dingtalk_callback_message_parsing() {
    use gasket_core::channels::dingtalk::DingTalkCallbackMessage;
    use serde_json;

    let json = r#"{
        "msgtype": "text",
        "text": {
            "content": "Hello from DingTalk!"
        },
        "msgid": "msg123",
        "createat": 1234567890000,
        "conversationId": "cid123",
        "conversationType": "1",
        "conversationTitle": "Test Chat",
        "senderId": "user123",
        "senderNick": "Test User",
        "chatbotUserId": "bot123",
        "atUsers": []
    }"#;

    let message: DingTalkCallbackMessage = serde_json::from_str(json).unwrap();
    assert_eq!(message.msgtype, "text");
    assert_eq!(message.text.content, "Hello from DingTalk!");
    assert_eq!(message.sender_id, "user123");
    assert_eq!(message.conversation_id, "cid123");
}

// =============================================================================
// Feishu Channel Tests
// =============================================================================

#[tokio::test]
#[cfg(feature = "feishu")]
async fn test_feishu_config_creation() {
    use gasket_core::channels::feishu::FeishuConfig;

    let config = FeishuConfig {
        app_id: "cli_test123".to_string(),
        app_secret: "secret123".to_string(),
        verification_token: Some("token123".to_string()),
        encrypt_key: None,
        allow_from: vec!["ou_user123".to_string()],
    };

    assert_eq!(config.app_id, "cli_test123");
    assert_eq!(config.app_secret, "secret123");
    assert_eq!(config.verification_token, Some("token123".to_string()));
}

#[tokio::test]
#[cfg(feature = "feishu")]
async fn test_feishu_channel_creation() {
    use gasket_core::channels::feishu::{FeishuChannel, FeishuConfig};
    use gasket_core::channels::Channel;

    let config = FeishuConfig {
        app_id: "cli_test".to_string(),
        app_secret: "secret".to_string(),
        verification_token: None,
        encrypt_key: None,
        allow_from: vec![],
    };

    let channel = FeishuChannel::new(config, create_test_sender());

    assert_eq!(channel.name(), "feishu");
}

#[tokio::test]
#[cfg(feature = "feishu")]
async fn test_feishu_channel_lifecycle() {
    use gasket_core::channels::base::Channel;
    use gasket_core::channels::feishu::{FeishuChannel, FeishuConfig};

    let config = FeishuConfig {
        app_id: "cli_test".to_string(),
        app_secret: "secret".to_string(),
        verification_token: None,
        encrypt_key: None,
        allow_from: vec![],
    };

    let mut channel = FeishuChannel::new(config, create_test_sender());

    // Test stop
    let stop_result = channel.stop().await;
    assert!(stop_result.is_ok());
}

#[tokio::test]
#[cfg(feature = "feishu")]
async fn test_feishu_text_content_parsing() {
    use gasket_core::channels::feishu::FeishuTextContent;
    use serde_json;

    let json = r#"{"text":"Hello from Feishu!"}"#;
    let content: FeishuTextContent = serde_json::from_str(json).unwrap();
    assert_eq!(content.text, "Hello from Feishu!");
}

#[tokio::test]
#[cfg(feature = "feishu")]
async fn test_feishu_message_parsing() {
    use gasket_core::channels::feishu::FeishuMessage;
    use serde_json;

    let json = r#"{
        "message_id": "om_msg123",
        "root_id": "om_root123",
        "parent_id": "om_parent123",
        "create_time": "1234567890",
        "chat_id": "oc_chat123",
        "message_type": "text",
        "content": "{\"text\":\"Hello!\"}"
    }"#;

    let message: FeishuMessage = serde_json::from_str(json).unwrap();
    assert_eq!(message.message_id, "om_msg123");
    assert_eq!(message.chat_id, "oc_chat123");
    assert_eq!(message.message_type, "text");
}

#[tokio::test]
#[cfg(feature = "feishu")]
async fn test_feishu_challenge_response() {
    use gasket_core::channels::feishu::{FeishuChallenge, FeishuChallengeResponse};
    use serde_json;

    let challenge = FeishuChallenge {
        challenge: "test_challenge_string".to_string(),
        token: "verification_token".to_string(),
        challenge_type: "url_verification".to_string(),
    };

    let response = FeishuChallengeResponse {
        challenge: challenge.challenge.clone(),
    };

    // Test serialization
    let json = serde_json::to_string(&response).unwrap();
    assert!(json.contains("test_challenge_string"));

    // Verify the JSON structure
    let json_value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(json_value["challenge"], "test_challenge_string");
}

// =============================================================================
// Inbound/Outbound Message Tests for Channels
// =============================================================================

#[tokio::test]
async fn test_inbound_message_with_all_channels() {
    use gasket_core::bus::events::InboundMessage;
    use gasket_core::bus::ChannelType;

    let channels = vec![
        ChannelType::Cli,
        ChannelType::Telegram,
        ChannelType::Discord,
        ChannelType::Slack,
        ChannelType::Email,
        ChannelType::Dingtalk,
        ChannelType::Feishu,
    ];

    for channel in channels {
        let msg = InboundMessage {
            channel: channel.clone(),
            sender_id: "user1".to_string(),
            chat_id: "chat1".to_string(),
            content: "Test message".to_string(),
            media: None,
            metadata: Some(serde_json::json!({"key": "value"})),
            timestamp: chrono::Utc::now(),
            trace_id: None,
        };

        assert_eq!(msg.channel, channel);
        assert!(msg.metadata.is_some());
    }
}

#[tokio::test]
async fn test_outbound_message_for_all_channels() {
    use gasket_core::bus::events::OutboundMessage;
    use gasket_core::bus::ChannelType;

    let channels = vec![
        ChannelType::Cli,
        ChannelType::Telegram,
        ChannelType::Discord,
        ChannelType::Slack,
        ChannelType::Email,
        ChannelType::Dingtalk,
        ChannelType::Feishu,
    ];

    for channel in channels {
        let msg = OutboundMessage {
            channel: channel.clone(),
            chat_id: "chat1".to_string(),
            content: "Response message".to_string(),
            metadata: Some(serde_json::json!({"thread_ts": "123456"})),
            trace_id: None,
            ws_message: None,
        };

        assert_eq!(msg.channel, channel);
        assert!(msg.metadata.is_some());
    }
}

// =============================================================================
// Channel Configuration Deserialization Tests
// =============================================================================

#[tokio::test]
async fn test_all_channels_config() {
    use gasket_core::config::Config;

    let json = r#"{
        "channels": {
            "telegram": {
                "enabled": true,
                "token": "bot123",
                "allow_from": ["user1"]
            },
            "discord": {
                "enabled": true,
                "token": "discord-token",
                "allow_from": []
            },
            "slack": {
                "enabled": true,
                "bot_token": "xoxb-test",
                "app_token": "xapp-test",
                "allow_from": []
            },
            "feishu": {
                "enabled": true,
                "app_id": "cli_test",
                "app_secret": "secret",
                "allow_from": []
            }
        }
    }"#;

    let config: Config = serde_json::from_str(json).unwrap();

    // Verify Telegram config
    assert!(config.channels.telegram.is_some());
    let tg = config.channels.telegram.unwrap();
    assert!(tg.enabled);
    assert_eq!(tg.token, "bot123");

    // Verify Discord config
    assert!(config.channels.discord.is_some());
    let dc = config.channels.discord.unwrap();
    assert!(dc.enabled);
    assert_eq!(dc.token, "discord-token");

    // Verify Slack config
    assert!(config.channels.slack.is_some());
    let sl = config.channels.slack.unwrap();
    assert!(sl.enabled);
    assert_eq!(sl.bot_token, "xoxb-test");
    assert_eq!(sl.app_token, "xapp-test");

    // Verify Feishu config
    assert!(config.channels.feishu.is_some());
    let fs = config.channels.feishu.unwrap();
    assert!(fs.enabled);
    assert_eq!(fs.app_id, "cli_test");
    assert_eq!(fs.app_secret, "secret");
}

#[tokio::test]
async fn test_channel_config_defaults() {
    use gasket_core::config::Config;

    // Test that missing channels default to None
    let json = r#"{}"#;
    let config: Config = serde_json::from_str(json).unwrap();

    assert!(config.channels.telegram.is_none());
    assert!(config.channels.discord.is_none());
    assert!(config.channels.slack.is_none());
    assert!(config.channels.feishu.is_none());
}

#[tokio::test]
async fn test_channel_config_with_group_policy() {
    use gasket_core::config::Config;

    let json = r#"{
        "channels": {
            "slack": {
                "enabled": true,
                "bot_token": "xoxb-test",
                "app_token": "xapp-test",
                "allow_from": [],
                "group_policy": "open"
            }
        }
    }"#;

    let config: Config = serde_json::from_str(json).unwrap();

    let sl = config.channels.slack.unwrap();
    assert_eq!(sl.group_policy, Some("open".to_string()));
}

#[tokio::test]
async fn test_feishu_config_with_optional_fields() {
    use gasket_core::config::Config;

    let json = r#"{
        "channels": {
            "feishu": {
                "enabled": true,
                "app_id": "cli_test",
                "app_secret": "secret",
                "verification_token": "token123",
                "encrypt_key": "key123",
                "allow_from": ["user1"]
            }
        }
    }"#;

    let config: Config = serde_json::from_str(json).unwrap();

    let fs = config.channels.feishu.unwrap();
    assert_eq!(fs.verification_token, Some("token123".to_string()));
    assert_eq!(fs.encrypt_key, Some("key123".to_string()));
    assert_eq!(fs.allow_from, vec!["user1"]);
}
