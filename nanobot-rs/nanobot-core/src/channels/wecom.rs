//! WeCom (企业微信) channel implementation
//!
//! Supports WeCom bot messaging via the Application Message API.
//! Uses corpid + corpsecret to obtain an access_token, then sends
//! messages to users/departments through agentid.
//!
//! Also supports receiving callback messages with signature verification
//! and AES-256-CBC decryption (using token + EncodingAESKey).

use aes::cipher::{block_padding::NoPadding, BlockDecryptMut, KeyIvInit};
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use tokio::sync::mpsc::Sender;
use tracing::{debug, error, info, instrument, warn};

use super::base::Channel;
use crate::bus::events::{InboundMessage, OutboundMessage};
use crate::bus::wecom;

type Aes256CbcDec = cbc::Decryptor<aes::Aes256>;

// ── Configuration ────────────────────────────────────────────

/// WeCom bot channel configuration
#[derive(Debug, Clone)]
pub struct WeComConfig {
    /// Corp ID
    pub corpid: String,

    /// Corp Secret
    pub corpsecret: String,

    /// Agent ID for the bot application
    pub agent_id: i64,

    /// Token for callback verification (optional)
    pub token: Option<String>,

    /// EncodingAESKey for callback message encryption/decryption (optional, 43 chars)
    pub encoding_aes_key: Option<String>,

    /// Allowed users (empty = allow all)
    pub allow_from: Vec<String>,
}

// ── Crypto helpers ───────────────────────────────────────────

/// Compute WeCom msg_signature.
///
/// `dev_msg_signature = SHA1(sort([token, timestamp, nonce, msg_encrypt]))`
fn compute_signature(token: &str, timestamp: &str, nonce: &str, msg_encrypt: &str) -> String {
    let mut params = [token, timestamp, nonce, msg_encrypt];
    params.sort();
    let joined: String = params.concat();

    let mut hasher = Sha1::new();
    hasher.update(joined.as_bytes());
    let hash = hasher.finalize();

    // hex-encode (lowercase)
    hash.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Decode an EncodingAESKey (43-char base64 without padding) into a 32-byte AES key.
fn decode_aes_key(encoding_aes_key: &str) -> anyhow::Result<Vec<u8>> {
    // EncodingAESKey is 43 chars of base64; add `=` padding to make it valid.
    let padded = format!("{}=", encoding_aes_key);
    let key = BASE64.decode(&padded)?;
    if key.len() != 32 {
        anyhow::bail!("EncodingAESKey decoded to {} bytes, expected 32", key.len());
    }
    Ok(key)
}

/// Decrypt a WeCom encrypted message.
///
/// Algorithm: AES-256-CBC, IV = AESKey\[0..16\], PKCS#7 padding.
/// Plaintext layout: `random(16B) + msg_len(4B, big-endian) + msg + receiveid`
fn decrypt_message(
    aes_key: &[u8],
    ciphertext_b64: &str,
    expected_receiveid: &str,
) -> anyhow::Result<String> {
    let ciphertext = BASE64.decode(ciphertext_b64)?;

    if ciphertext.len() < 32 || ciphertext.len() % 16 != 0 {
        anyhow::bail!(
            "Invalid ciphertext length: {} (must be >= 32 and multiple of 16)",
            ciphertext.len()
        );
    }

    // IV = first 16 bytes of AES key
    let iv: &[u8; 16] = aes_key[..16]
        .try_into()
        .map_err(|_| anyhow::anyhow!("AES key too short for IV"))?;
    let key: &[u8; 32] = aes_key
        .try_into()
        .map_err(|_| anyhow::anyhow!("AES key must be 32 bytes"))?;

    // Decrypt with NoPadding — WeCom uses PKCS#7 padding to a 32-byte boundary
    // (not the standard 16-byte AES block size), so we must handle it manually.
    let mut buf = ciphertext.clone();
    let decrypted = Aes256CbcDec::new(key.into(), iv.into())
        .decrypt_padded_mut::<NoPadding>(&mut buf)
        .map_err(|e| anyhow::anyhow!("AES decryption failed: {}", e))?;

    // Remove PKCS#7 padding (32-byte boundary, per WeCom spec)
    if decrypted.is_empty() {
        anyhow::bail!("Decrypted data is empty");
    }
    let pad_len = *decrypted.last().unwrap() as usize;
    if pad_len == 0 || pad_len > 32 || pad_len > decrypted.len() {
        anyhow::bail!("Invalid PKCS#7 padding value: {}", pad_len);
    }
    let decrypted = &decrypted[..decrypted.len() - pad_len];

    // Parse: random(16) + msg_len(4) + msg + receiveid
    if decrypted.len() < 20 {
        anyhow::bail!(
            "Decrypted data too short: {} bytes (need at least 20)",
            decrypted.len()
        );
    }

    let msg_len = u32::from_be_bytes(decrypted[16..20].try_into().unwrap()) as usize;

    if 20 + msg_len > decrypted.len() {
        anyhow::bail!(
            "msg_len={} exceeds available data ({})",
            msg_len,
            decrypted.len() - 20
        );
    }

    let msg = &decrypted[20..20 + msg_len];
    let receiveid = &decrypted[20 + msg_len..];

    // Verify receiveid matches corpid
    if receiveid != expected_receiveid.as_bytes() {
        anyhow::bail!(
            "ReceiveId mismatch: expected '{}', got '{}'",
            expected_receiveid,
            String::from_utf8_lossy(receiveid)
        );
    }

    String::from_utf8(msg.to_vec())
        .map_err(|e| anyhow::anyhow!("Decrypted message is not valid UTF-8: {}", e))
}

// ── Callback types ───────────────────────────────────────────

/// WeCom callback request query parameters (shared by URL verify & message callback).
#[derive(Debug, Clone, Deserialize)]
pub struct WeComCallbackQuery {
    pub msg_signature: String,
    pub timestamp: String,
    pub nonce: String,
    /// Only present in URL verification (GET).
    pub echostr: Option<String>,
}

/// WeCom callback POST body (JSON format).
#[derive(Debug, Clone, Deserialize)]
pub struct WeComCallbackBody {
    #[serde(rename = "Encrypt")]
    pub encrypt: String,
}

/// Parsed WeCom callback message (after decryption).
///
/// WeCom sends messages in XML format. This struct represents a text message.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename = "xml")]
pub struct WeComCallbackMessage {
    /// Target Corp ID
    #[serde(rename = "ToUserName")]
    pub to_user_name: String,

    /// Sender user ID
    #[serde(rename = "FromUserName")]
    pub from_user_name: String,

    /// Create timestamp
    #[serde(rename = "CreateTime")]
    pub create_time: i64,

    /// Message type (text, image, voice, video, etc.)
    #[serde(rename = "MsgType")]
    pub msg_type: String,

    /// Text content (only present for text messages)
    #[serde(rename = "Content")]
    pub content: Option<String>,

    /// Message ID
    #[serde(rename = "MsgId")]
    pub msg_id: Option<String>,

    /// Agent ID
    #[serde(rename = "AgentID")]
    pub agent_id: Option<i64>,

    /// Event type (only present for event messages)
    #[serde(rename = "Event")]
    pub event: Option<String>,
}

// ── API response ─────────────────────────────────────────────

/// WeCom API response envelope
#[derive(Debug, Deserialize)]
struct WeComApiResponse {
    errcode: i32,
    errmsg: String,
}

// ── Channel ──────────────────────────────────────────────────

/// WeCom bot channel.
///
/// Sends incoming messages directly to the message bus via `Sender<InboundMessage>`.
pub struct WeComChannel {
    config: WeComConfig,
    inbound_sender: Sender<InboundMessage>,
    client: Client,
    access_token: Option<String>,
    /// Cached decoded AES key (32 bytes), derived from `encoding_aes_key`.
    aes_key: Option<Vec<u8>>,
}

impl WeComChannel {
    /// Create a new WeCom bot channel with an inbound message sender.
    pub fn new(config: WeComConfig, inbound_sender: Sender<InboundMessage>) -> Self {
        Self {
            config,
            inbound_sender,
            client: Client::new(),
            access_token: None,
            aes_key: None,
        }
    }

    // ── Token management ─────────────────────────────────────

    /// Get access_token via corpid + corpsecret.
    ///
    /// Caches the token in `self.access_token`. Called automatically during `start()`.
    #[instrument(name = "channel.wecom.get_token", skip_all)]
    pub async fn get_access_token(&mut self) -> anyhow::Result<&str> {
        if let Some(ref token) = self.access_token {
            return Ok(token);
        }

        let url = format!(
            "https://qyapi.weixin.qq.com/cgi-bin/gettoken?corpid={}&corpsecret={}",
            self.config.corpid, self.config.corpsecret
        );

        let response = self.client.get(&url).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await?;
            anyhow::bail!("Failed to get WeCom access token: {} - {}", status, body);
        }

        #[derive(Deserialize)]
        struct TokenResponse {
            errcode: i32,
            errmsg: String,
            access_token: Option<String>,
            #[allow(dead_code)]
            expires_in: Option<i64>,
        }

        let token_resp: TokenResponse = response.json().await?;
        if token_resp.errcode != 0 {
            anyhow::bail!(
                "WeCom gettoken error (errcode={}): {}",
                token_resp.errcode,
                token_resp.errmsg
            );
        }

        let token = token_resp.access_token.ok_or_else(|| {
            anyhow::anyhow!("WeCom gettoken returned errcode=0 but no access_token")
        })?;

        self.access_token = Some(token);
        info!("Obtained WeCom access token");

        Ok(self
            .access_token
            .as_ref()
            .expect("access_token was just set"))
    }

    // ── Sending ──────────────────────────────────────────────

    /// Send a POST to the message/send API and check the response.
    async fn post_message<T: Serialize>(&self, body: &T) -> anyhow::Result<()> {
        let token = self
            .access_token
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No access token. Call get_access_token first."))?;

        let url = format!(
            "https://qyapi.weixin.qq.com/cgi-bin/message/send?access_token={}",
            token
        );

        let response = self.client.post(&url).json(body).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await?;
            anyhow::bail!("Failed to send WeCom message: {} - {}", status, body);
        }

        let result: WeComApiResponse = response.json().await?;
        if result.errcode != 0 {
            anyhow::bail!(
                "WeCom message/send error (errcode={}): {}",
                result.errcode,
                result.errmsg
            );
        }
        Ok(())
    }

    /// Send a text message to users.
    ///
    /// `to_user` — pipe-separated user IDs, e.g. `"UserID1|UserID2"` or `"@all"`.
    #[instrument(name = "channel.wecom.send_text", skip(self, text), fields(to = %to_user))]
    pub async fn send_text(&self, to_user: &str, text: &str) -> anyhow::Result<()> {
        #[derive(Serialize)]
        struct Msg {
            touser: String,
            msgtype: String,
            agentid: i64,
            text: Content,
        }
        #[derive(Serialize)]
        struct Content {
            content: String,
        }

        self.post_message(&Msg {
            touser: to_user.to_string(),
            msgtype: "text".to_string(),
            agentid: self.config.agent_id,
            text: Content {
                content: text.to_string(),
            },
        })
        .await?;

        debug!("Sent WeCom text message to {}", to_user);
        Ok(())
    }

    /// Send a markdown message to users.
    ///
    /// `to_user` — pipe-separated user IDs, e.g. `"UserID1|UserID2"` or `"@all"`.
    #[instrument(name = "channel.wecom.send_markdown", skip(self, content), fields(to = %to_user))]
    pub async fn send_markdown(&self, to_user: &str, content: &str) -> anyhow::Result<()> {
        #[derive(Serialize)]
        struct Msg {
            touser: String,
            msgtype: String,
            agentid: i64,
            markdown: Content,
        }
        #[derive(Serialize)]
        struct Content {
            content: String,
        }

        self.post_message(&Msg {
            touser: to_user.to_string(),
            msgtype: "markdown".to_string(),
            agentid: self.config.agent_id,
            markdown: Content {
                content: content.to_string(),
            },
        })
        .await?;

        debug!("Sent WeCom markdown message to {}", to_user);
        Ok(())
    }

    // ── Callback receiving ───────────────────────────────────

    /// Get or compute the cached AES key.
    fn get_aes_key(&self) -> anyhow::Result<&[u8]> {
        self.aes_key
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("No AES key. encoding_aes_key not configured."))
    }

    /// Verify callback URL (handles the GET verification request from WeCom).
    ///
    /// WeCom sends: `GET /callback?msg_signature=...&timestamp=...&nonce=...&echostr=...`
    ///
    /// Returns the decrypted echostr that should be sent back as the HTTP response body.
    pub fn verify_url(&self, query: &WeComCallbackQuery) -> anyhow::Result<String> {
        let token = self
            .config
            .token
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Token not configured for callback verification"))?;

        let echostr = query
            .echostr
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Missing echostr in URL verification request"))?;

        // Verify signature
        let expected_sig = compute_signature(token, &query.timestamp, &query.nonce, echostr);
        if expected_sig != query.msg_signature {
            anyhow::bail!(
                "Signature mismatch in URL verification: expected={}, got={}",
                expected_sig,
                query.msg_signature
            );
        }

        // Decrypt echostr
        let aes_key = self.get_aes_key()?;
        let plaintext = decrypt_message(aes_key, echostr, &self.config.corpid)?;

        debug!("WeCom URL verification succeeded");
        Ok(plaintext)
    }

    /// Handle an incoming callback message (POST from WeCom).
    ///
    /// Verifies signature, decrypts the message, parses it, and publishes
    /// through the inbound processor middleware.
    #[instrument(name = "channel.wecom.handle_callback", skip_all)]
    pub async fn handle_callback_message(
        &self,
        query: &WeComCallbackQuery,
        body: &WeComCallbackBody,
    ) -> anyhow::Result<()> {
        let token = self
            .config
            .token
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Token not configured for callback"))?;

        // Verify signature
        let expected_sig = compute_signature(token, &query.timestamp, &query.nonce, &body.encrypt);
        if expected_sig != query.msg_signature {
            error!(
                "WeCom callback signature mismatch: expected={}, got={}",
                expected_sig, query.msg_signature
            );
            anyhow::bail!("Signature verification failed for WeCom callback");
        }

        // Decrypt
        let aes_key = self.get_aes_key()?;
        let xml_str = decrypt_message(aes_key, &body.encrypt, &self.config.corpid)?;
        debug!("Decrypted WeCom callback message: {}", xml_str);

        // Parse the XML message
        let message = parse_callback_xml(&xml_str)?;

        // Check allowlist
        if !self.config.allow_from.is_empty()
            && !self.config.allow_from.contains(&message.from_user_name)
        {
            debug!(
                "Ignoring message from unauthorized WeCom user: {}",
                message.from_user_name
            );
            return Ok(());
        }

        // Handle by message type
        match message.msg_type.as_str() {
            "text" => {
                let content = message.content.as_deref().unwrap_or("");
                if content.is_empty() {
                    debug!("Ignoring empty WeCom text message");
                    return Ok(());
                }

                debug!(
                    "Received WeCom text message from {}: {}",
                    message.from_user_name, content
                );

                let ctx_trace_id = None;

                let inbound = InboundMessage {
                    channel: wecom(),
                    sender_id: message.from_user_name.clone(),
                    chat_id: message.from_user_name.clone(),
                    content: content.to_string(),
                    media: None,
                    metadata: serde_json::to_value(&message).ok(),
                    timestamp: chrono::Utc::now(),
                    trace_id: ctx_trace_id,
                };

                self.inbound_sender.send(inbound).await?;
            }
            "event" => {
                debug!(
                    "Received WeCom event: {:?} from {}",
                    message.event, message.from_user_name
                );
                // Events (subscribe, enter_agent, etc.) are logged but not published
            }
            other => {
                warn!(
                    "Ignoring unsupported WeCom message type: {} from {}",
                    other, message.from_user_name
                );
            }
        }

        Ok(())
    }
}

/// Parse WeCom callback XML into a `WeComCallbackMessage`.
///
/// WeCom sends XML like:
/// ```xml
/// <xml>
///   <ToUserName><![CDATA[corpid]]></ToUserName>
///   <FromUserName><![CDATA[userid]]></FromUserName>
///   <CreateTime>1234567890</CreateTime>
///   <MsgType><![CDATA[text]]></MsgType>
///   <Content><![CDATA[hello]]></Content>
///   <MsgId>12345</MsgId>
///   <AgentID>1000002</AgentID>
/// </xml>
/// ```
///
/// Uses `quick-xml` for robust XML parsing.
fn parse_callback_xml(xml: &str) -> anyhow::Result<WeComCallbackMessage> {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let mut reader = Reader::from_str(xml);

    let mut to_user_name: Option<String> = None;
    let mut from_user_name: Option<String> = None;
    let mut create_time: Option<i64> = None;
    let mut msg_type: Option<String> = None;
    let mut content: Option<String> = None;
    let mut msg_id: Option<String> = None;
    let mut agent_id: Option<i64> = None;
    let mut event: Option<String> = None;

    let mut current_tag: Option<String> = None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                current_tag = Some(tag);
            }
            Ok(Event::Text(e)) => {
                if let Some(ref tag) = current_tag {
                    let text = e.unescape().map_err(|err| {
                        anyhow::anyhow!("Failed to unescape XML text in <{}>: {}", tag, err)
                    })?;
                    let text = text.trim().to_string();
                    if !text.is_empty() {
                        match tag.as_str() {
                            "ToUserName" => to_user_name = Some(text),
                            "FromUserName" => from_user_name = Some(text),
                            "CreateTime" => {
                                create_time = Some(text.parse().map_err(|e| {
                                    anyhow::anyhow!("Invalid CreateTime '{}': {}", text, e)
                                })?)
                            }
                            "MsgType" => msg_type = Some(text),
                            "Content" => content = Some(text),
                            "MsgId" => msg_id = Some(text),
                            "AgentID" => {
                                agent_id = Some(text.parse().map_err(|e| {
                                    anyhow::anyhow!("Invalid AgentID '{}': {}", text, e)
                                })?)
                            }
                            "Event" => event = Some(text),
                            _ => {}
                        }
                    }
                }
            }
            Ok(Event::CData(e)) => {
                if let Some(ref tag) = current_tag {
                    let text = String::from_utf8(e.into_inner().to_vec()).map_err(|err| {
                        anyhow::anyhow!("Invalid UTF-8 in CDATA for <{}>: {}", tag, err)
                    })?;
                    match tag.as_str() {
                        "ToUserName" => to_user_name = Some(text),
                        "FromUserName" => from_user_name = Some(text),
                        "MsgType" => msg_type = Some(text),
                        "Content" => content = Some(text),
                        "MsgId" => msg_id = Some(text),
                        "Event" => event = Some(text),
                        _ => {}
                    }
                }
            }
            Ok(Event::End(_)) => {
                current_tag = None;
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "Error parsing WeCom callback XML at position {}: {}",
                    reader.error_position(),
                    e
                ));
            }
            _ => {}
        }
    }

    Ok(WeComCallbackMessage {
        to_user_name: to_user_name
            .ok_or_else(|| anyhow::anyhow!("Missing ToUserName in WeCom callback XML"))?,
        from_user_name: from_user_name
            .ok_or_else(|| anyhow::anyhow!("Missing FromUserName in WeCom callback XML"))?,
        create_time: create_time
            .ok_or_else(|| anyhow::anyhow!("Missing CreateTime in WeCom callback XML"))?,
        msg_type: msg_type
            .ok_or_else(|| anyhow::anyhow!("Missing MsgType in WeCom callback XML"))?,
        content,
        msg_id,
        agent_id,
        event,
    })
}

#[async_trait]
impl Channel for WeComChannel {
    fn name(&self) -> &str {
        "wecom"
    }

    async fn start(&mut self) -> anyhow::Result<()> {
        info!("Starting WeCom channel");
        self.get_access_token().await?;

        // Pre-decode AES key if configured
        if let Some(ref encoding_aes_key) = self.config.encoding_aes_key {
            let key = decode_aes_key(encoding_aes_key)?;
            self.aes_key = Some(key);
            info!("WeCom callback decryption key loaded");
        }

        Ok(())
    }

    async fn stop(&mut self) -> anyhow::Result<()> {
        info!("Stopping WeCom channel");
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> anyhow::Result<()> {
        self.send_text(&msg.chat_id, &msg.content).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    fn create_test_sender() -> Sender<InboundMessage> {
        let (tx, rx) = mpsc::channel(100);
        // Leak the receiver to keep the channel open for tests
        std::mem::forget(rx);
        tx
    }

    #[test]
    fn test_wecom_config_creation() {
        let config = WeComConfig {
            corpid: "ww1234567890".to_string(),
            corpsecret: "secret".to_string(),
            agent_id: 1000002,
            token: Some("token123".to_string()),
            encoding_aes_key: Some("abcdefghijklmnopqrstuvwxyz01234567890ABCDEF".to_string()),
            allow_from: vec![],
        };

        assert_eq!(config.corpid, "ww1234567890");
        assert_eq!(config.agent_id, 1000002);
        assert_eq!(config.token.as_deref(), Some("token123"));
        assert_eq!(
            config.encoding_aes_key.as_deref(),
            Some("abcdefghijklmnopqrstuvwxyz01234567890ABCDEF")
        );
    }

    #[test]
    fn test_wecom_channel_creation() {
        let config = WeComConfig {
            corpid: "ww_test".to_string(),
            corpsecret: "secret".to_string(),
            agent_id: 1000002,
            token: None,
            encoding_aes_key: None,
            allow_from: vec![],
        };

        let channel = WeComChannel::new(config, create_test_sender());
        assert_eq!(channel.name(), "wecom");
    }

    #[test]
    fn test_wecom_text_message_serialization() {
        #[derive(Serialize)]
        struct Msg {
            touser: String,
            msgtype: String,
            agentid: i64,
            text: Content,
        }
        #[derive(Serialize)]
        struct Content {
            content: String,
        }

        let message = Msg {
            touser: "UserID1|UserID2".to_string(),
            msgtype: "text".to_string(),
            agentid: 1000002,
            text: Content {
                content: "Hello".to_string(),
            },
        };

        let json = serde_json::to_string(&message).unwrap();
        assert!(json.contains("\"touser\":\"UserID1|UserID2\""));
        assert!(json.contains("\"agentid\":1000002"));
        assert!(json.contains("\"msgtype\":\"text\""));
        assert!(json.contains("\"content\":\"Hello\""));
    }

    #[test]
    fn test_wecom_markdown_message_serialization() {
        #[derive(Serialize)]
        struct Msg {
            touser: String,
            msgtype: String,
            agentid: i64,
            markdown: Content,
        }
        #[derive(Serialize)]
        struct Content {
            content: String,
        }

        let message = Msg {
            touser: "@all".to_string(),
            msgtype: "markdown".to_string(),
            agentid: 1000002,
            markdown: Content {
                content: "# Title\nBody".to_string(),
            },
        };

        let json = serde_json::to_string(&message).unwrap();
        assert!(json.contains("\"touser\":\"@all\""));
        assert!(json.contains("\"msgtype\":\"markdown\""));
        assert!(json.contains("\"agentid\":1000002"));
        assert!(json.contains("# Title\\nBody"));
    }

    // ── Crypto tests ─────────────────────────────────────────

    #[test]
    fn test_compute_signature() {
        // Known values: SHA1(sort(["token", "1234567890", "nonce", "encrypt"]))
        let sig = compute_signature("token", "1234567890", "nonce", "encrypt");
        // The sort order is: ["1234567890", "encrypt", "nonce", "token"]
        // SHA1("1234567890encryptnoncetoken")
        let mut hasher = Sha1::new();
        hasher.update(b"1234567890encryptnoncetoken");
        let expected: String = hasher
            .finalize()
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect();
        assert_eq!(sig, expected);
    }

    #[test]
    fn test_decode_aes_key_valid() {
        // 43-char base64-without-padding that decodes to 32 bytes
        // Base64("0123456789abcdef0123456789abcdef") = "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY="
        // Without trailing '=' that's 43 chars: "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY"
        let encoding_aes_key = "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY";
        let key = decode_aes_key(encoding_aes_key).unwrap();
        assert_eq!(key.len(), 32);
        assert_eq!(&key, b"0123456789abcdef0123456789abcdef");
    }

    #[test]
    fn test_decode_aes_key_invalid_length() {
        let result = decode_aes_key("dG9vc2hvcnQ"); // "tooshort" in base64
        assert!(result.is_err());
    }

    #[test]
    fn test_decrypt_message_roundtrip() {
        use aes::cipher::{block_padding::NoPadding, BlockEncryptMut, KeyIvInit};

        type Aes256CbcEnc = cbc::Encryptor<aes::Aes256>;

        let aes_key = b"0123456789abcdef0123456789abcdef"; // 32 bytes
        let iv: &[u8; 16] = &aes_key[..16].try_into().unwrap();
        let receiveid = "ww1234567890";
        let msg = "Hello, WeCom!";

        // Build plaintext: random(16) + msg_len(4) + msg + receiveid
        let mut plaintext = Vec::new();
        plaintext.extend_from_slice(b"0123456789abcdef"); // 16 random bytes
        plaintext.extend_from_slice(&(msg.len() as u32).to_be_bytes());
        plaintext.extend_from_slice(msg.as_bytes());
        plaintext.extend_from_slice(receiveid.as_bytes());

        // PKCS#7 padding to 32-byte boundary
        let pad_len = 32 - (plaintext.len() % 32);
        plaintext.extend(std::iter::repeat(pad_len as u8).take(pad_len));

        // Encrypt
        let mut buf = plaintext.clone();
        let buf_len = buf.len();
        Aes256CbcEnc::new(aes_key.into(), iv.into())
            .encrypt_padded_mut::<NoPadding>(&mut buf, buf_len)
            .unwrap();

        let ciphertext_b64 = BASE64.encode(&buf);

        // Decrypt
        let decrypted = decrypt_message(aes_key, &ciphertext_b64, receiveid).unwrap();
        assert_eq!(decrypted, "Hello, WeCom!");
    }

    #[test]
    fn test_decrypt_message_wrong_receiveid() {
        use aes::cipher::{block_padding::NoPadding, BlockEncryptMut, KeyIvInit};

        type Aes256CbcEnc = cbc::Encryptor<aes::Aes256>;

        let aes_key = b"0123456789abcdef0123456789abcdef";
        let iv: &[u8; 16] = &aes_key[..16].try_into().unwrap();
        let receiveid = "ww1234567890";
        let msg = "Test";

        let mut plaintext = Vec::new();
        plaintext.extend_from_slice(b"0123456789abcdef");
        plaintext.extend_from_slice(&(msg.len() as u32).to_be_bytes());
        plaintext.extend_from_slice(msg.as_bytes());
        plaintext.extend_from_slice(receiveid.as_bytes());

        let pad_len = 32 - (plaintext.len() % 32);
        plaintext.extend(std::iter::repeat(pad_len as u8).take(pad_len));

        let mut buf = plaintext;
        let buf_len = buf.len();
        Aes256CbcEnc::new(aes_key.into(), iv.into())
            .encrypt_padded_mut::<NoPadding>(&mut buf, buf_len)
            .unwrap();

        let ciphertext_b64 = BASE64.encode(&buf);

        let result = decrypt_message(aes_key, &ciphertext_b64, "wrong_corpid");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("ReceiveId mismatch"),
            "Expected ReceiveId mismatch error"
        );
    }

    // ── XML parsing tests ────────────────────────────────────

    #[test]
    fn test_parse_callback_xml_text_message() {
        let xml = r#"<xml>
            <ToUserName><![CDATA[ww1234567890]]></ToUserName>
            <FromUserName><![CDATA[user001]]></FromUserName>
            <CreateTime>1348831860</CreateTime>
            <MsgType><![CDATA[text]]></MsgType>
            <Content><![CDATA[Hello from WeCom!]]></Content>
            <MsgId>1234567890123456</MsgId>
            <AgentID>1000002</AgentID>
        </xml>"#;

        let msg = parse_callback_xml(xml).unwrap();
        assert_eq!(msg.to_user_name, "ww1234567890");
        assert_eq!(msg.from_user_name, "user001");
        assert_eq!(msg.create_time, 1348831860);
        assert_eq!(msg.msg_type, "text");
        assert_eq!(msg.content.as_deref(), Some("Hello from WeCom!"));
        assert_eq!(msg.msg_id.as_deref(), Some("1234567890123456"));
        assert_eq!(msg.agent_id, Some(1000002));
    }

    #[test]
    fn test_parse_callback_xml_event_message() {
        let xml = r#"<xml>
            <ToUserName><![CDATA[ww1234567890]]></ToUserName>
            <FromUserName><![CDATA[user001]]></FromUserName>
            <CreateTime>1348831860</CreateTime>
            <MsgType><![CDATA[event]]></MsgType>
            <Event><![CDATA[enter_agent]]></Event>
            <AgentID>1000002</AgentID>
        </xml>"#;

        let msg = parse_callback_xml(xml).unwrap();
        assert_eq!(msg.msg_type, "event");
        assert_eq!(msg.event.as_deref(), Some("enter_agent"));
        assert!(msg.content.is_none());
    }

    #[test]
    fn test_parse_callback_xml_missing_required_field() {
        let xml = r#"<xml>
            <ToUserName><![CDATA[ww1234567890]]></ToUserName>
            <CreateTime>1348831860</CreateTime>
            <MsgType><![CDATA[text]]></MsgType>
        </xml>"#;

        let result = parse_callback_xml(xml);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("FromUserName"));
    }

    // ── Verify URL test ──────────────────────────────────────

    #[test]
    fn test_verify_url_signature_mismatch() {
        let config = WeComConfig {
            corpid: "ww1234567890".to_string(),
            corpsecret: "secret".to_string(),
            agent_id: 1000002,
            token: Some("test_token".to_string()),
            encoding_aes_key: Some("MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY".to_string()),
            allow_from: vec![],
        };

        let mut channel = WeComChannel::new(config, create_test_sender());
        channel.aes_key = Some(b"0123456789abcdef0123456789abcdef".to_vec());

        let query = WeComCallbackQuery {
            msg_signature: "wrong_signature".to_string(),
            timestamp: "1234567890".to_string(),
            nonce: "nonce123".to_string(),
            echostr: Some("encrypted_echostr".to_string()),
        };

        let result = channel.verify_url(&query);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Signature mismatch"));
    }

    #[test]
    fn test_verify_url_no_token_configured() {
        let config = WeComConfig {
            corpid: "ww1234567890".to_string(),
            corpsecret: "secret".to_string(),
            agent_id: 1000002,
            token: None,
            encoding_aes_key: None,
            allow_from: vec![],
        };

        let channel = WeComChannel::new(config, create_test_sender());

        let query = WeComCallbackQuery {
            msg_signature: "sig".to_string(),
            timestamp: "123".to_string(),
            nonce: "nonce".to_string(),
            echostr: Some("enc".to_string()),
        };

        let result = channel.verify_url(&query);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Token not configured"));
    }

    // ── Full verify_url roundtrip ────────────────────────────

    #[test]
    fn test_verify_url_full_roundtrip() {
        use aes::cipher::{block_padding::NoPadding, BlockEncryptMut, KeyIvInit};

        type Aes256CbcEnc = cbc::Encryptor<aes::Aes256>;

        let aes_key_bytes = b"0123456789abcdef0123456789abcdef";
        let encoding_aes_key = "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY";
        let corpid = "ww1234567890";
        let token = "test_token";
        let echostr_plaintext = "random_echostr_123";

        // Build encrypted echostr
        let iv: &[u8; 16] = &aes_key_bytes[..16].try_into().unwrap();
        let mut plaintext = Vec::new();
        plaintext.extend_from_slice(b"0123456789abcdef"); // 16 random
        plaintext.extend_from_slice(&(echostr_plaintext.len() as u32).to_be_bytes());
        plaintext.extend_from_slice(echostr_plaintext.as_bytes());
        plaintext.extend_from_slice(corpid.as_bytes());

        let pad_len = 32 - (plaintext.len() % 32);
        plaintext.extend(std::iter::repeat(pad_len as u8).take(pad_len));

        let mut buf = plaintext;
        let buf_len = buf.len();
        Aes256CbcEnc::new(aes_key_bytes.into(), iv.into())
            .encrypt_padded_mut::<NoPadding>(&mut buf, buf_len)
            .unwrap();
        let echostr_encrypted = BASE64.encode(&buf);

        // Compute correct signature
        let timestamp = "1234567890";
        let nonce = "nonce123";
        let sig = compute_signature(token, timestamp, nonce, &echostr_encrypted);

        // Create channel and verify
        let config = WeComConfig {
            corpid: corpid.to_string(),
            corpsecret: "secret".to_string(),
            agent_id: 1000002,
            token: Some(token.to_string()),
            encoding_aes_key: Some(encoding_aes_key.to_string()),
            allow_from: vec![],
        };

        let mut channel = WeComChannel::new(config, create_test_sender());
        channel.aes_key = Some(aes_key_bytes.to_vec());

        let query = WeComCallbackQuery {
            msg_signature: sig,
            timestamp: timestamp.to_string(),
            nonce: nonce.to_string(),
            echostr: Some(echostr_encrypted),
        };

        let result = channel.verify_url(&query);
        assert!(result.is_ok(), "verify_url failed: {:?}", result.err());
        assert_eq!(result.unwrap(), echostr_plaintext);
    }

    // ── Callback message handling test ───────────────────────

    #[tokio::test]
    async fn test_handle_callback_message_full_roundtrip() {
        use aes::cipher::{block_padding::NoPadding, BlockEncryptMut, KeyIvInit};

        type Aes256CbcEnc = cbc::Encryptor<aes::Aes256>;

        let aes_key_bytes = b"0123456789abcdef0123456789abcdef";
        let corpid = "ww1234567890";
        let token = "test_token";

        // Build XML message
        let xml = r#"<xml>
            <ToUserName><![CDATA[ww1234567890]]></ToUserName>
            <FromUserName><![CDATA[user001]]></FromUserName>
            <CreateTime>1348831860</CreateTime>
            <MsgType><![CDATA[text]]></MsgType>
            <Content><![CDATA[Hello callback!]]></Content>
            <MsgId>123456</MsgId>
            <AgentID>1000002</AgentID>
        </xml>"#;

        // Encrypt the XML
        let iv: &[u8; 16] = &aes_key_bytes[..16].try_into().unwrap();
        let mut plaintext = Vec::new();
        plaintext.extend_from_slice(b"0123456789abcdef");
        plaintext.extend_from_slice(&(xml.len() as u32).to_be_bytes());
        plaintext.extend_from_slice(xml.as_bytes());
        plaintext.extend_from_slice(corpid.as_bytes());

        let pad_len = 32 - (plaintext.len() % 32);
        plaintext.extend(std::iter::repeat(pad_len as u8).take(pad_len));

        let mut buf = plaintext;
        let buf_len = buf.len();
        Aes256CbcEnc::new(aes_key_bytes.into(), iv.into())
            .encrypt_padded_mut::<NoPadding>(&mut buf, buf_len)
            .unwrap();
        let encrypted = BASE64.encode(&buf);

        let timestamp = "1234567890";
        let nonce = "nonce123";
        let sig = compute_signature(token, timestamp, nonce, &encrypted);

        let config = WeComConfig {
            corpid: corpid.to_string(),
            corpsecret: "secret".to_string(),
            agent_id: 1000002,
            token: Some(token.to_string()),
            encoding_aes_key: None,
            allow_from: vec![],
        };

        let mut channel = WeComChannel::new(config, create_test_sender());
        channel.aes_key = Some(aes_key_bytes.to_vec());

        let query = WeComCallbackQuery {
            msg_signature: sig,
            timestamp: timestamp.to_string(),
            nonce: nonce.to_string(),
            echostr: None,
        };

        let body = WeComCallbackBody { encrypt: encrypted };

        let result = channel.handle_callback_message(&query, &body).await;
        assert!(
            result.is_ok(),
            "handle_callback_message failed: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    async fn test_handle_callback_message_allowlist_rejection() {
        use aes::cipher::{block_padding::NoPadding, BlockEncryptMut, KeyIvInit};

        type Aes256CbcEnc = cbc::Encryptor<aes::Aes256>;

        let aes_key_bytes = b"0123456789abcdef0123456789abcdef";
        let corpid = "ww1234567890";
        let token = "test_token";

        let xml = r#"<xml>
            <ToUserName><![CDATA[ww1234567890]]></ToUserName>
            <FromUserName><![CDATA[unauthorized_user]]></FromUserName>
            <CreateTime>1348831860</CreateTime>
            <MsgType><![CDATA[text]]></MsgType>
            <Content><![CDATA[Should be blocked]]></Content>
            <MsgId>123456</MsgId>
            <AgentID>1000002</AgentID>
        </xml>"#;

        let iv: &[u8; 16] = &aes_key_bytes[..16].try_into().unwrap();
        let mut plaintext = Vec::new();
        plaintext.extend_from_slice(b"0123456789abcdef");
        plaintext.extend_from_slice(&(xml.len() as u32).to_be_bytes());
        plaintext.extend_from_slice(xml.as_bytes());
        plaintext.extend_from_slice(corpid.as_bytes());

        let pad_len = 32 - (plaintext.len() % 32);
        plaintext.extend(std::iter::repeat(pad_len as u8).take(pad_len));

        let mut buf = plaintext;
        let buf_len = buf.len();
        Aes256CbcEnc::new(aes_key_bytes.into(), iv.into())
            .encrypt_padded_mut::<NoPadding>(&mut buf, buf_len)
            .unwrap();
        let encrypted = BASE64.encode(&buf);

        let timestamp = "1234567890";
        let nonce = "nonce123";
        let sig = compute_signature(token, timestamp, nonce, &encrypted);

        let config = WeComConfig {
            corpid: corpid.to_string(),
            corpsecret: "secret".to_string(),
            agent_id: 1000002,
            token: Some(token.to_string()),
            encoding_aes_key: None,
            allow_from: vec!["allowed_user".to_string()],
        };

        let mut channel = WeComChannel::new(config, create_test_sender());
        channel.aes_key = Some(aes_key_bytes.to_vec());

        let query = WeComCallbackQuery {
            msg_signature: sig,
            timestamp: timestamp.to_string(),
            nonce: nonce.to_string(),
            echostr: None,
        };

        let body = WeComCallbackBody { encrypt: encrypted };

        // Should succeed (returns Ok) but not process the message
        let result = channel.handle_callback_message(&query, &body).await;
        assert!(result.is_ok());
    }
}
