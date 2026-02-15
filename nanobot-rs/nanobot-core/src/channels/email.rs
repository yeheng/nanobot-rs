//! Email channel implementation using IMAP and SMTP

use async_trait::async_trait;
use tracing::{debug, info, warn};

use super::base::Channel;
use crate::bus::events::{InboundMessage, OutboundMessage};
use crate::bus::MessageBus;

/// Email channel configuration
#[derive(Debug, Clone)]
pub struct EmailConfig {
    pub imap_host: String,
    pub imap_port: u16,
    pub imap_username: String,
    pub imap_password: String,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_username: String,
    pub smtp_password: String,
    pub from_address: String,
    pub allow_from: Vec<String>,
    pub consent_granted: bool,
}

/// Email channel using IMAP/SMTP
pub struct EmailChannel {
    config: EmailConfig,
    bus: MessageBus,
}

impl EmailChannel {
    /// Create a new Email channel
    pub fn new(config: EmailConfig, bus: MessageBus) -> Self {
        Self { config, bus }
    }

    /// Poll for new emails
    pub async fn poll(&self) -> anyhow::Result<Vec<InboundMessage>> {
        if !self.config.consent_granted {
            return Ok(vec![]);
        }

        debug!("Polling IMAP for new emails");

        // Use async-imap for polling
        let messages = self.fetch_unread_emails().await?;

        for msg in &messages {
            self.bus.publish_inbound(msg.clone()).await;
        }

        Ok(messages)
    }

    async fn fetch_unread_emails(&self) -> anyhow::Result<Vec<InboundMessage>> {
        // Simplified IMAP fetch
        // In production, use async-imap or imap crate
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpStream;

        let addr = format!("{}:{}", self.config.imap_host, self.config.imap_port);

        match TcpStream::connect(&addr).await {
            Ok(mut stream) => {
                // Read greeting
                let mut greeting = vec![0u8; 1024];
                let _ = stream.read(&mut greeting).await;

                // Send CAPABILITY
                stream.write_all(b"A001 CAPABILITY\r\n").await?;
                let mut response = vec![0u8; 4096];
                let _ = stream.read(&mut response).await;

                // Send LOGIN
                let login = format!(
                    "A002 LOGIN {} {}\r\n",
                    self.config.imap_username, self.config.imap_password
                );
                stream.write_all(login.as_bytes()).await?;
                let _ = stream.read(&mut response).await;

                // Send SELECT INBOX
                stream.write_all(b"A003 SELECT INBOX\r\n").await?;
                let _ = stream.read(&mut response).await;

                // Search for UNSEEN messages
                stream.write_all(b"A004 SEARCH UNSEEN\r\n").await?;
                let mut search_response = vec![0u8; 4096];
                let n = stream.read(&mut search_response).await?;

                // Parse search results (simplified)
                let _search_str = String::from_utf8_lossy(&search_response[..n]);

                // Logout
                stream.write_all(b"A005 LOGOUT\r\n").await?;

                // For now, return empty - real implementation would parse emails
                Ok(vec![])
            }
            Err(e) => {
                warn!("Failed to connect to IMAP: {}", e);
                Ok(vec![])
            }
        }
    }

    /// Send an email
    pub async fn send_email(&self, to: &str, subject: &str, body: &str) -> anyhow::Result<()> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpStream;

        let addr = format!("{}:{}", self.config.smtp_host, self.config.smtp_port);

        let mut stream = TcpStream::connect(&addr).await?;

        // Read greeting
        let mut greeting = vec![0u8; 1024];
        let _ = stream.read(&mut greeting).await;

        // Send EHLO
        stream
            .write_all(format!("EHLO {}\r\n", self.config.smtp_host).as_bytes())
            .await?;
        let _ = stream.read(&mut greeting).await;

        // Send STARTTLS (if port 587)
        if self.config.smtp_port == 587 {
            stream.write_all(b"STARTTLS\r\n").await?;
            let _ = stream.read(&mut greeting).await;
        }

        // Send AUTH LOGIN (simplified)
        stream.write_all(b"AUTH LOGIN\r\n").await?;
        let _ = stream.read(&mut greeting).await;

        // Send username (base64)
        let username_b64 = base64_encode(&self.config.smtp_username);
        stream
            .write_all(format!("{}\r\n", username_b64).as_bytes())
            .await?;
        let _ = stream.read(&mut greeting).await;

        // Send password (base64)
        let password_b64 = base64_encode(&self.config.smtp_password);
        stream
            .write_all(format!("{}\r\n", password_b64).as_bytes())
            .await?;
        let _ = stream.read(&mut greeting).await;

        // Send MAIL FROM
        stream
            .write_all(format!("MAIL FROM:<{}>\r\n", self.config.from_address).as_bytes())
            .await?;
        let _ = stream.read(&mut greeting).await;

        // Send RCPT TO
        stream
            .write_all(format!("RCPT TO:<{}>\r\n", to).as_bytes())
            .await?;
        let _ = stream.read(&mut greeting).await;

        // Send DATA
        stream.write_all(b"DATA\r\n").await?;
        let _ = stream.read(&mut greeting).await;

        // Send email content
        let email = format!(
            "From: {}\r\nTo: {}\r\nSubject: {}\r\n\r\n{}\r\n.\r\n",
            self.config.from_address, to, subject, body
        );
        stream.write_all(email.as_bytes()).await?;
        let _ = stream.read(&mut greeting).await;

        // Send QUIT
        stream.write_all(b"QUIT\r\n").await?;

        info!("Email sent to {}", to);
        Ok(())
    }

    /// Start polling loop
    pub async fn start_polling(self) -> anyhow::Result<()> {
        info!("Starting Email polling");

        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));

        loop {
            interval.tick().await;

            if let Err(e) = self.poll().await {
                warn!("Email polling error: {}", e);
            }
        }
    }
}

#[async_trait]
impl Channel for EmailChannel {
    fn name(&self) -> &str {
        "email"
    }

    async fn start(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn stop(&mut self) -> anyhow::Result<()> {
        info!("Stopping Email channel");
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> anyhow::Result<()> {
        // Parse email address from chat_id
        let to = msg.chat_id.trim_start_matches("email:");
        self.send_email(to, "Re: Your message", &msg.content).await
    }
}

/// Simple base64 encoding (without external crate)
fn base64_encode(input: &str) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let bytes = input.as_bytes();
    let mut result = String::new();

    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = chunk.get(1).copied().unwrap_or(0) as usize;
        let b2 = chunk.get(2).copied().unwrap_or(0) as usize;

        result.push(ALPHABET[b0 >> 2] as char);
        result.push(ALPHABET[((b0 & 0x03) << 4) | (b1 >> 4)] as char);

        if chunk.len() > 1 {
            result.push(ALPHABET[((b1 & 0x0f) << 2) | (b2 >> 6)] as char);
        } else {
            result.push('=');
        }

        if chunk.len() > 2 {
            result.push(ALPHABET[b2 & 0x3f] as char);
        } else {
            result.push('=');
        }
    }

    result
}
