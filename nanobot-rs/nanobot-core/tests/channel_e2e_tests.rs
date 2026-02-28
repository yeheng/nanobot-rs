//! Real E2E tests for each channel.
//!
//! These tests make actual API calls to external services.
//! They require valid credentials in a `.env` file (see `.env.example`).
//!
//! Run all channels:
//!   cargo test --test channel_e2e_tests --features all-channels -- --ignored
//!
//! Run a single channel:
//!   cargo test --test channel_e2e_tests --features dingtalk  -- --ignored test_dingtalk
//!   cargo test --test channel_e2e_tests --features feishu    -- --ignored test_feishu
//!   cargo test --test channel_e2e_tests --features slack     -- --ignored test_slack
//!   cargo test --test channel_e2e_tests --features email     -- --ignored test_email
//!   cargo test --test channel_e2e_tests --features telegram  -- --ignored test_telegram
//!   cargo test --test channel_e2e_tests --features discord   -- --ignored test_discord

/// Load .env file and install the rustls CryptoProvider.
///
/// Rustls 0.23+ requires an explicit crypto provider when both `ring` and
/// `aws-lc-rs` features are present in the dependency tree. We pick `ring`
/// here. The call is idempotent — `install_default` returns `Err` if a
/// provider was already installed, so we just ignore that.
#[allow(dead_code)]
fn load_env() -> bool {
    let _ = rustls::crypto::ring::default_provider().install_default();

    // Try project root, then workspace root
    dotenvy::from_filename(".env").is_ok()
        || dotenvy::from_filename("../../.env").is_ok()
        || dotenvy::from_filename("../.env").is_ok()
}

/// Helper: read an env var or skip the test
macro_rules! env_or_skip {
    ($key:expr) => {
        match std::env::var($key) {
            Ok(v) if !v.is_empty() => v,
            _ => {
                eprintln!("SKIP: env var {} not set, skipping test", $key);
                return;
            }
        }
    };
}

/// Create a test sender for inbound messages (middleware-aware).
/// The receiver is leaked to keep the channel open for the test duration.
#[allow(dead_code)]
fn create_test_sender() -> nanobot_core::channels::InboundSender {
    let (tx, rx) = tokio::sync::mpsc::channel(100);
    std::mem::forget(rx);
    nanobot_core::channels::InboundSender::new(tx)
}

/// Create a raw mpsc sender for channels not yet migrated to InboundSender.
#[allow(dead_code)]
fn create_raw_test_sender() -> tokio::sync::mpsc::Sender<nanobot_core::bus::events::InboundMessage>
{
    let (tx, rx) = tokio::sync::mpsc::channel(100);
    std::mem::forget(rx);
    tx
}

// =============================================================================
// DingTalk E2E Tests
// =============================================================================

#[cfg(feature = "dingtalk")]
mod dingtalk_e2e {
    use super::*;
    use nanobot_core::channels::dingtalk::{DingTalkChannel, DingTalkConfig};

    macro_rules! make_config {
        () => {{
            let webhook_url = env_or_skip!("DINGTALK_WEBHOOK_URL");
            let secret = std::env::var("DINGTALK_SECRET")
                .ok()
                .filter(|s| !s.is_empty());
            DingTalkConfig {
                webhook_url,
                secret,
                access_token: None,
                allow_from: vec![],
            }
        }};
    }

    #[tokio::test]
    #[ignore]
    async fn test_dingtalk_send_text_real_api() {
        load_env();
        let config = make_config!();
        let channel = DingTalkChannel::new(config, create_test_sender());

        let result = channel
            .send_text("[E2E Test] DingTalk send_text - nanobot channel test")
            .await;

        assert!(
            result.is_ok(),
            "DingTalk send_text failed: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_dingtalk_send_markdown_real_api() {
        load_env();
        let config = make_config!();
        let channel = DingTalkChannel::new(config, create_test_sender());

        let result = channel
            .send_markdown(
                "E2E Test",
                "### Nanobot E2E Test\n\n- Channel: **DingTalk**\n- Type: Markdown\n- Status: OK",
            )
            .await;

        assert!(
            result.is_ok(),
            "DingTalk send_markdown failed: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_dingtalk_send_via_channel_trait_real_api() {
        load_env();
        use nanobot_core::bus::events::OutboundMessage;
        use nanobot_core::bus::ChannelType;
        use nanobot_core::channels::base::Channel;

        let config = make_config!();
        let channel = DingTalkChannel::new(config, create_test_sender());

        let msg = OutboundMessage {
            channel: ChannelType::Dingtalk,
            chat_id: "unused".to_string(), // DingTalk send() uses webhook, not chat_id
            content: "[E2E Test] DingTalk Channel::send trait - nanobot".to_string(),
            metadata: None,
            trace_id: None,
        };

        let result = channel.send(msg).await;
        assert!(
            result.is_ok(),
            "DingTalk Channel::send failed: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_dingtalk_start_stop_lifecycle_real_api() {
        load_env();
        use nanobot_core::channels::base::Channel;

        let config = make_config!();
        let mut channel = DingTalkChannel::new(config, create_test_sender());

        let start_result = Channel::start(&mut channel).await;
        assert!(
            start_result.is_ok(),
            "DingTalk start failed: {:?}",
            start_result.err()
        );

        let stop_result = channel.stop().await;
        assert!(
            stop_result.is_ok(),
            "DingTalk stop failed: {:?}",
            stop_result.err()
        );
    }
}

// =============================================================================
// Feishu E2E Tests
// =============================================================================

#[cfg(feature = "feishu")]
mod feishu_e2e {
    use super::*;
    use nanobot_core::channels::feishu::{FeishuChannel, FeishuConfig};

    macro_rules! make_config {
        () => {{
            let app_id = env_or_skip!("FEISHU_APP_ID");
            let app_secret = env_or_skip!("FEISHU_APP_SECRET");
            FeishuConfig {
                app_id,
                app_secret,
                verification_token: None,
                encrypt_key: None,
                allow_from: vec![],
            }
        }};
    }

    #[tokio::test]
    #[ignore]
    async fn test_feishu_get_access_token_real_api() {
        load_env();
        let config = make_config!();
        let mut channel = FeishuChannel::new(config, create_test_sender());

        // start() internally calls get_access_token()
        let result = nanobot_core::channels::base::Channel::start(&mut channel).await;
        assert!(
            result.is_ok(),
            "Feishu get_access_token failed: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_feishu_send_text_real_api() {
        load_env();
        let config = make_config!();
        let chat_id = env_or_skip!("FEISHU_CHAT_ID");

        let mut channel = FeishuChannel::new(config, create_test_sender());

        // Must get access token first
        let start_result = nanobot_core::channels::base::Channel::start(&mut channel).await;
        assert!(
            start_result.is_ok(),
            "Feishu start failed: {:?}",
            start_result.err()
        );

        let result = channel
            .send_text(
                &chat_id,
                "[E2E Test] Feishu send_text - nanobot channel test",
            )
            .await;

        assert!(
            result.is_ok(),
            "Feishu send_text failed: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_feishu_send_via_channel_trait_real_api() {
        load_env();
        use nanobot_core::bus::events::OutboundMessage;
        use nanobot_core::bus::ChannelType;
        use nanobot_core::channels::base::Channel;

        let config = make_config!();
        let chat_id = env_or_skip!("FEISHU_CHAT_ID");

        let mut channel = FeishuChannel::new(config, create_test_sender());

        // Must start to get access token
        Channel::start(&mut channel)
            .await
            .expect("Feishu start failed");

        let msg = OutboundMessage {
            channel: ChannelType::Feishu,
            chat_id,
            content: "[E2E Test] Feishu Channel::send trait - nanobot".to_string(),
            metadata: None,
            trace_id: None,
        };

        let result = channel.send(msg).await;
        assert!(
            result.is_ok(),
            "Feishu Channel::send failed: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_feishu_start_stop_lifecycle_real_api() {
        load_env();
        use nanobot_core::channels::base::Channel;

        let config = make_config!();
        let mut channel = FeishuChannel::new(config, create_test_sender());

        let start_result = Channel::start(&mut channel).await;
        assert!(
            start_result.is_ok(),
            "Feishu start failed: {:?}",
            start_result.err()
        );

        let stop_result = channel.stop().await;
        assert!(
            stop_result.is_ok(),
            "Feishu stop failed: {:?}",
            stop_result.err()
        );
    }
}

// =============================================================================
// Slack E2E Tests
// =============================================================================

#[cfg(feature = "slack")]
mod slack_e2e {
    use super::*;
    use nanobot_core::channels::slack::{SlackChannel, SlackConfig};

    macro_rules! make_config {
        () => {{
            let bot_token = env_or_skip!("SLACK_BOT_TOKEN");
            let app_token = env_or_skip!("SLACK_APP_TOKEN");
            SlackConfig {
                bot_token,
                app_token,
                group_policy: None,
                allow_from: vec![],
            }
        }};
    }

    #[tokio::test]
    #[ignore]
    async fn test_slack_send_message_real_api() {
        load_env();
        let config = make_config!();
        let channel_id = env_or_skip!("SLACK_CHANNEL_ID");

        let channel = SlackChannel::new(config, create_raw_test_sender());

        let result = channel
            .send_message(
                &channel_id,
                "[E2E Test] Slack send_message - nanobot channel test",
                None,
            )
            .await;

        assert!(
            result.is_ok(),
            "Slack send_message failed: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_slack_send_message_with_thread_real_api() {
        load_env();
        let config = make_config!();
        let channel_id = env_or_skip!("SLACK_CHANNEL_ID");

        let channel = SlackChannel::new(config, create_raw_test_sender());

        // Send a parent message first
        let result = channel
            .send_message(
                &channel_id,
                "[E2E Test] Slack thread parent - nanobot",
                None,
            )
            .await;

        assert!(
            result.is_ok(),
            "Slack send parent message failed: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_slack_send_via_channel_trait_real_api() {
        load_env();
        use nanobot_core::bus::events::OutboundMessage;
        use nanobot_core::bus::ChannelType;
        use nanobot_core::channels::base::Channel;

        let config = make_config!();
        let channel_id = env_or_skip!("SLACK_CHANNEL_ID");

        let channel = SlackChannel::new(config, create_raw_test_sender());

        let msg = OutboundMessage {
            channel: ChannelType::Slack,
            chat_id: channel_id,
            content: "[E2E Test] Slack Channel::send trait - nanobot".to_string(),
            metadata: None,
            trace_id: None,
        };

        let result = channel.send(msg).await;
        assert!(
            result.is_ok(),
            "Slack Channel::send failed: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_slack_send_via_channel_trait_with_thread_metadata_real_api() {
        load_env();
        use nanobot_core::bus::events::OutboundMessage;
        use nanobot_core::bus::ChannelType;
        use nanobot_core::channels::base::Channel;

        let config = make_config!();
        let channel_id = env_or_skip!("SLACK_CHANNEL_ID");

        let channel = SlackChannel::new(config, create_raw_test_sender());

        // Send with thread_ts metadata (thread_ts is null, but the code path is exercised)
        let msg = OutboundMessage {
            channel: ChannelType::Slack,
            chat_id: channel_id,
            content: "[E2E Test] Slack Channel::send with metadata - nanobot".to_string(),
            metadata: Some(serde_json::json!({
                "thread_ts": null
            })),
            trace_id: None,
        };

        let result = channel.send(msg).await;
        assert!(
            result.is_ok(),
            "Slack Channel::send with metadata failed: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_slack_start_stop_lifecycle_real_api() {
        load_env();
        use nanobot_core::channels::base::Channel;

        let config = make_config!();
        let mut channel = SlackChannel::new(config, create_raw_test_sender());

        let start_result = Channel::start(&mut channel).await;
        assert!(
            start_result.is_ok(),
            "Slack start failed: {:?}",
            start_result.err()
        );

        let stop_result = channel.stop().await;
        assert!(
            stop_result.is_ok(),
            "Slack stop failed: {:?}",
            stop_result.err()
        );
    }
}

// =============================================================================
// Email E2E Tests
// =============================================================================

#[cfg(feature = "email")]
mod email_e2e {
    use super::*;
    use nanobot_core::channels::email::{EmailChannel, EmailConfig};

    /// Build an SMTP-only config (IMAP fields set to placeholders).
    macro_rules! make_smtp_config {
        () => {{
            let smtp_host = env_or_skip!("EMAIL_SMTP_HOST");
            let smtp_port: u16 = env_or_skip!("EMAIL_SMTP_PORT")
                .parse()
                .expect("EMAIL_SMTP_PORT must be a number");
            let smtp_username = env_or_skip!("EMAIL_SMTP_USERNAME");
            let smtp_password = env_or_skip!("EMAIL_SMTP_PASSWORD");
            let from_address = env_or_skip!("EMAIL_FROM_ADDRESS");
            EmailConfig {
                imap_host: "unused".to_string(),
                imap_port: 993,
                imap_username: "unused".to_string(),
                imap_password: "unused".to_string(),
                smtp_host,
                smtp_port,
                smtp_username,
                smtp_password,
                from_address,
                allow_from: vec![],
                consent_granted: true,
            }
        }};
    }

    /// Build an IMAP-only config (SMTP fields set to placeholders).
    macro_rules! make_imap_config {
        () => {{
            let imap_host = env_or_skip!("EMAIL_IMAP_HOST");
            let imap_port: u16 = env_or_skip!("EMAIL_IMAP_PORT")
                .parse()
                .expect("EMAIL_IMAP_PORT must be a number");
            let imap_username = env_or_skip!("EMAIL_IMAP_USERNAME");
            let imap_password = env_or_skip!("EMAIL_IMAP_PASSWORD");
            let from_address = env_or_skip!("EMAIL_FROM_ADDRESS");
            EmailConfig {
                imap_host,
                imap_port,
                imap_username,
                imap_password,
                smtp_host: "unused".to_string(),
                smtp_port: 587,
                smtp_username: "unused".to_string(),
                smtp_password: "unused".to_string(),
                from_address,
                allow_from: vec![],
                consent_granted: true,
            }
        }};
    }

    #[tokio::test]
    #[ignore]
    async fn test_email_send_smtp_real_api() {
        load_env();
        let config = make_smtp_config!();
        let to_address = env_or_skip!("EMAIL_TO_ADDRESS");

        let channel = EmailChannel::new(config, create_raw_test_sender());

        let result = channel
            .send_email(
                &to_address,
                "[E2E Test] Nanobot Email Channel",
                "This is an automated E2E test from nanobot email channel.",
            )
            .await;

        assert!(
            result.is_ok(),
            "Email send_email failed: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_email_imap_poll_real_api() {
        load_env();
        let config = make_imap_config!();

        let channel = EmailChannel::new(config, create_raw_test_sender());

        // poll() connects to IMAP and fetches unread emails
        let result = channel.poll().await;

        assert!(result.is_ok(), "Email IMAP poll failed: {:?}", result.err());

        let messages = result.unwrap();
        println!("Fetched {} unread emails via IMAP", messages.len());
    }

    #[tokio::test]
    #[ignore]
    async fn test_email_send_via_channel_trait_real_api() {
        load_env();
        use nanobot_core::bus::events::OutboundMessage;
        use nanobot_core::bus::ChannelType;
        use nanobot_core::channels::base::Channel;

        let config = make_smtp_config!();
        let to_address = env_or_skip!("EMAIL_TO_ADDRESS");

        let channel = EmailChannel::new(config, create_raw_test_sender());

        // Channel::send parses chat_id as "email:recipient@example.com"
        let msg = OutboundMessage {
            channel: ChannelType::Email,
            chat_id: format!("email:{}", to_address),
            content: "[E2E Test] Email Channel::send trait - nanobot".to_string(),
            metadata: None,
            trace_id: None,
        };

        let result = channel.send(msg).await;
        assert!(
            result.is_ok(),
            "Email Channel::send failed: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_email_start_stop_lifecycle_real_api() {
        load_env();
        use nanobot_core::channels::base::Channel;

        let config = make_smtp_config!();
        let mut channel = EmailChannel::new(config, create_raw_test_sender());

        let start_result = Channel::start(&mut channel).await;
        assert!(
            start_result.is_ok(),
            "Email start failed: {:?}",
            start_result.err()
        );

        let stop_result = channel.stop().await;
        assert!(
            stop_result.is_ok(),
            "Email stop failed: {:?}",
            stop_result.err()
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_email_poll_without_consent_returns_empty() {
        load_env();
        let mut config = make_imap_config!();
        config.consent_granted = false;

        let channel = EmailChannel::new(config, create_raw_test_sender());

        let result = channel.poll().await;
        assert!(result.is_ok(), "poll() should succeed even without consent");

        let messages = result.unwrap();
        assert!(
            messages.is_empty(),
            "poll() should return empty when consent_granted=false, got {} messages",
            messages.len()
        );
    }
}

// =============================================================================
// Telegram E2E Tests
// =============================================================================

#[cfg(feature = "telegram")]
mod telegram_e2e {
    use super::*;
    use nanobot_core::channels::telegram::{TelegramChannel, TelegramConfig};

    macro_rules! make_config {
        () => {{
            let token = env_or_skip!("TELEGRAM_BOT_TOKEN");
            TelegramConfig {
                token,
                allow_from: vec![],
            }
        }};
    }

    #[tokio::test]
    #[ignore]
    async fn test_telegram_send_message_real_api() {
        load_env();
        use nanobot_core::bus::events::OutboundMessage;
        use nanobot_core::bus::ChannelType;
        use nanobot_core::channels::base::Channel;

        let config = make_config!();
        let chat_id = env_or_skip!("TELEGRAM_CHAT_ID");

        let channel = TelegramChannel::new(config, create_raw_test_sender());

        let msg = OutboundMessage {
            channel: ChannelType::Telegram,
            chat_id,
            content: "[E2E Test] Telegram Channel::send - nanobot channel test".to_string(),
            metadata: None,
            trace_id: None,
        };

        let result = channel.send(msg).await;
        assert!(result.is_ok(), "Telegram send failed: {:?}", result.err());
    }

    #[tokio::test]
    #[ignore]
    async fn test_telegram_bot_token_validation_real_api() {
        load_env();
        let config = make_config!();

        // Validate the bot token by calling Telegram's getMe endpoint
        let client = reqwest::Client::new();
        let url = format!("https://api.telegram.org/bot{}/getMe", config.token);

        let response = client.get(&url).send().await;

        assert!(
            response.is_ok(),
            "HTTP request failed: {:?}",
            response.err()
        );

        let resp = response.unwrap();
        assert!(
            resp.status().is_success(),
            "Telegram token validation failed with status: {}",
            resp.status()
        );

        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(
            body["ok"].as_bool(),
            Some(true),
            "Telegram getMe response not ok: {:?}",
            body
        );

        println!(
            "Telegram bot validated: {} (id: {})",
            body["result"]["username"].as_str().unwrap_or("unknown"),
            body["result"]["id"].as_i64().unwrap_or(0)
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_telegram_send_long_message_real_api() {
        load_env();
        use nanobot_core::bus::events::OutboundMessage;
        use nanobot_core::bus::ChannelType;
        use nanobot_core::channels::base::Channel;

        let config = make_config!();
        let chat_id = env_or_skip!("TELEGRAM_CHAT_ID");

        let channel = TelegramChannel::new(config, create_raw_test_sender());

        let long_content = format!(
            "[E2E Test] Telegram long message test - nanobot\n\n{}\n\nEnd of message.",
            "This is a line of test content. ".repeat(20)
        );

        let msg = OutboundMessage {
            channel: ChannelType::Telegram,
            chat_id,
            content: long_content,
            metadata: None,
            trace_id: None,
        };

        let result = channel.send(msg).await;
        assert!(
            result.is_ok(),
            "Telegram send long message failed: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_telegram_start_stop_lifecycle_real_api() {
        load_env();
        use nanobot_core::channels::base::Channel;

        let config = make_config!();
        let mut channel = TelegramChannel::new(config, create_raw_test_sender());

        let start_result = Channel::start(&mut channel).await;
        assert!(
            start_result.is_ok(),
            "Telegram start failed: {:?}",
            start_result.err()
        );

        let stop_result = channel.stop().await;
        assert!(
            stop_result.is_ok(),
            "Telegram stop failed: {:?}",
            stop_result.err()
        );
    }
}

// =============================================================================
// Discord E2E Tests
// =============================================================================

#[cfg(feature = "discord")]
mod discord_e2e {
    use super::*;
    use nanobot_core::channels::discord::{DiscordChannel, DiscordConfig};

    macro_rules! make_config {
        () => {{
            let token = env_or_skip!("DISCORD_BOT_TOKEN");
            DiscordConfig {
                token,
                allow_from: vec![],
            }
        }};
    }

    /// Validate that the bot token is accepted by Discord's API.
    /// Discord's Channel::send is a stub, so we test via HTTP directly.
    #[tokio::test]
    #[ignore]
    async fn test_discord_bot_token_validation_real_api() {
        load_env();
        let config = make_config!();

        let client = reqwest::Client::new();
        let response = client
            .get("https://discord.com/api/v10/users/@me")
            .header("Authorization", format!("Bot {}", config.token))
            .send()
            .await;

        assert!(
            response.is_ok(),
            "HTTP request failed: {:?}",
            response.err()
        );

        let resp = response.unwrap();
        assert!(
            resp.status().is_success(),
            "Discord token validation failed with status: {}",
            resp.status()
        );

        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(
            body.get("id").is_some(),
            "Discord response missing 'id' field: {:?}",
            body
        );

        println!(
            "Discord bot validated: {} ({})",
            body["username"].as_str().unwrap_or("unknown"),
            body["id"].as_str().unwrap_or("unknown")
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_discord_send_message_via_http_real_api() {
        load_env();
        let config = make_config!();
        let channel_id = env_or_skip!("DISCORD_CHANNEL_ID");

        // Since Channel::send is a stub, test sending via Discord REST API directly
        let client = reqwest::Client::new();
        let url = format!(
            "https://discord.com/api/v10/channels/{}/messages",
            channel_id
        );

        let body = serde_json::json!({
            "content": "[E2E Test] Discord HTTP send - nanobot channel test"
        });

        let response = client
            .post(&url)
            .header("Authorization", format!("Bot {}", config.token))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await;

        assert!(
            response.is_ok(),
            "HTTP request failed: {:?}",
            response.err()
        );

        let resp = response.unwrap();
        assert!(
            resp.status().is_success(),
            "Discord send message failed with status: {} - {}",
            resp.status(),
            resp.text().await.unwrap_or_default()
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_discord_send_embed_via_http_real_api() {
        load_env();
        let config = make_config!();
        let channel_id = env_or_skip!("DISCORD_CHANNEL_ID");

        let client = reqwest::Client::new();
        let url = format!(
            "https://discord.com/api/v10/channels/{}/messages",
            channel_id
        );

        let body = serde_json::json!({
            "content": "[E2E Test] Discord embed message",
            "embeds": [{
                "title": "Nanobot E2E Test",
                "description": "This is a test embed sent from nanobot channel e2e tests.",
                "color": 5814783,
                "fields": [
                    { "name": "Channel", "value": "Discord", "inline": true },
                    { "name": "Type", "value": "Embed", "inline": true }
                ]
            }]
        });

        let response = client
            .post(&url)
            .header("Authorization", format!("Bot {}", config.token))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await;

        assert!(
            response.is_ok(),
            "HTTP request failed: {:?}",
            response.err()
        );

        let resp = response.unwrap();
        assert!(
            resp.status().is_success(),
            "Discord send embed failed with status: {} - {}",
            resp.status(),
            resp.text().await.unwrap_or_default()
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_discord_channel_lifecycle_real_api() {
        load_env();
        use nanobot_core::channels::base::Channel;

        let config = make_config!();
        let mut channel = DiscordChannel::new(config, create_raw_test_sender());

        let start_result = Channel::start(&mut channel).await;
        assert!(
            start_result.is_ok(),
            "Discord start failed: {:?}",
            start_result.err()
        );

        let stop_result = channel.stop().await;
        assert!(
            stop_result.is_ok(),
            "Discord stop failed: {:?}",
            stop_result.err()
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_discord_get_channel_info_real_api() {
        load_env();
        let config = make_config!();
        let channel_id = env_or_skip!("DISCORD_CHANNEL_ID");

        let client = reqwest::Client::new();
        let url = format!("https://discord.com/api/v10/channels/{}", channel_id);

        let response = client
            .get(&url)
            .header("Authorization", format!("Bot {}", config.token))
            .send()
            .await;

        assert!(
            response.is_ok(),
            "HTTP request failed: {:?}",
            response.err()
        );

        let resp = response.unwrap();
        assert!(
            resp.status().is_success(),
            "Discord get channel info failed with status: {}",
            resp.status()
        );

        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(
            body.get("id").is_some(),
            "Discord channel response missing 'id': {:?}",
            body
        );

        println!(
            "Discord channel info: name={}, type={}",
            body["name"].as_str().unwrap_or("unknown"),
            body["type"].as_i64().unwrap_or(-1)
        );
    }
}

// =============================================================================
// WeCom (企业微信) E2E Tests
// =============================================================================

#[cfg(feature = "wecom")]
mod wecom_e2e {
    use super::*;
    use nanobot_core::channels::wecom::{WeComChannel, WeComConfig};

    macro_rules! make_config {
        () => {{
            let corpid = env_or_skip!("WECOM_CORP_ID");
            let corpsecret = env_or_skip!("WECOM_CORP_SECRET");
            let agent_id: i64 = env_or_skip!("WECOM_AGENT_ID")
                .parse()
                .expect("WECOM_AGENT_ID must be a number");
            WeComConfig {
                corpid,
                corpsecret,
                agent_id,
                token: None,
                encoding_aes_key: None,
                allow_from: vec![],
            }
        }};
    }

    #[tokio::test]
    #[ignore]
    async fn test_wecom_get_access_token_real_api() {
        load_env();
        let config = make_config!();
        let mut channel = WeComChannel::new(config, create_test_sender());

        let result = channel.get_access_token().await;
        assert!(
            result.is_ok(),
            "WeCom get_access_token failed: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_wecom_send_text_real_api() {
        load_env();
        let config = make_config!();
        let to_user = env_or_skip!("WECOM_TO_USER");

        let mut channel = WeComChannel::new(config, create_test_sender());
        nanobot_core::channels::base::Channel::start(&mut channel)
            .await
            .expect("WeCom start failed");

        let result = channel
            .send_text(
                &to_user,
                "[E2E Test] WeCom send_text - nanobot channel test",
            )
            .await;

        assert!(result.is_ok(), "WeCom send_text failed: {:?}", result.err());
    }

    #[tokio::test]
    #[ignore]
    async fn test_wecom_send_markdown_real_api() {
        load_env();
        let config = make_config!();
        let to_user = env_or_skip!("WECOM_TO_USER");

        let mut channel = WeComChannel::new(config, create_test_sender());
        nanobot_core::channels::base::Channel::start(&mut channel)
            .await
            .expect("WeCom start failed");

        let result = channel
            .send_markdown(
                &to_user,
                "### Nanobot E2E Test\n\n- Channel: **WeCom**\n- Type: Markdown\n- Status: OK",
            )
            .await;

        assert!(
            result.is_ok(),
            "WeCom send_markdown failed: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_wecom_send_via_channel_trait_real_api() {
        load_env();
        use nanobot_core::bus::events::OutboundMessage;
        use nanobot_core::bus::ChannelType;
        use nanobot_core::channels::base::Channel;

        let config = make_config!();
        let to_user = env_or_skip!("WECOM_TO_USER");

        let mut channel = WeComChannel::new(config, create_test_sender());
        Channel::start(&mut channel)
            .await
            .expect("WeCom start failed");

        let msg = OutboundMessage {
            channel: ChannelType::Wecom,
            chat_id: to_user,
            content: "[E2E Test] WeCom Channel::send trait - nanobot".to_string(),
            metadata: None,
            trace_id: None,
        };

        let result = channel.send(msg).await;
        assert!(
            result.is_ok(),
            "WeCom Channel::send failed: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_wecom_start_stop_lifecycle_real_api() {
        load_env();
        use nanobot_core::channels::base::Channel;

        let config = make_config!();
        let mut channel = WeComChannel::new(config, create_test_sender());

        let start_result = Channel::start(&mut channel).await;
        assert!(
            start_result.is_ok(),
            "WeCom start failed: {:?}",
            start_result.err()
        );

        let stop_result = channel.stop().await;
        assert!(
            stop_result.is_ok(),
            "WeCom stop failed: {:?}",
            stop_result.err()
        );
    }
}
