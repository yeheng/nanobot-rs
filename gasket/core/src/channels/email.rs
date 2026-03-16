//! Email channel implementation using IMAP and SMTP
//!
//! Uses well-tested crates for email operations:
//! - `lettre` for SMTP (sending emails)
//! - `async-imap` for IMAP (receiving emails)

use async_trait::async_trait;
use lettre::{
    message::{header::ContentType, Mailbox},
    transport::smtp::authentication::Credentials,
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};
use tokio::sync::mpsc::Sender;
use tracing::{debug, info, instrument, warn};

use super::base::Channel;
use crate::bus::events::InboundMessage;
use crate::bus::ChannelType;

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

/// Email channel.
///
/// Sends incoming messages directly to the message bus via `Sender<InboundMessage>`.
pub struct EmailChannel {
    config: EmailConfig,
    inbound_sender: Sender<InboundMessage>,
}

impl EmailChannel {
    /// Create a new Email channel with an inbound message sender.
    pub fn new(config: EmailConfig, inbound_sender: Sender<InboundMessage>) -> Self {
        Self {
            config,
            inbound_sender,
        }
    }

    /// Poll for new emails
    #[instrument(name = "channel.email.poll", skip_all)]
    pub async fn poll(&self) -> anyhow::Result<Vec<InboundMessage>> {
        if !self.config.consent_granted {
            return Ok(vec![]);
        }

        debug!("Polling IMAP for new emails");

        let messages = self.fetch_unread_emails().await?;

        for msg in &messages {
            let inbound = InboundMessage {
                channel: ChannelType::Email,
                sender_id: msg.sender_id.clone(),
                chat_id: msg.chat_id.clone(),
                content: msg.content.clone(),
                media: None,
                metadata: None,
                timestamp: chrono::Utc::now(),
                trace_id: None,
            };

            if let Err(e) = self.inbound_sender.send(inbound).await {
                warn!("Failed to send inbound email: {}", e);
            }
        }

        Ok(messages)
    }

    async fn fetch_unread_emails(&self) -> anyhow::Result<Vec<InboundMessage>> {
        use async_imap::Client;
        use futures_util::StreamExt;
        use std::sync::Arc;
        use tokio::net::TcpStream;
        use tokio_rustls::{
            rustls::{ClientConfig, RootCertStore},
            TlsConnector,
        };

        let addr = format!("{}:{}", self.config.imap_host, self.config.imap_port);
        let username = &self.config.imap_username;
        let password = &self.config.imap_password;

        // Build TLS connector with native roots
        let mut root_store = RootCertStore::empty();
        let result = rustls_native_certs::load_native_certs();
        for cert in result.certs {
            let _ = root_store.add(cert);
        }
        if !result.errors.is_empty() {
            warn!("Some errors loading native certs: {:?}", result.errors);
        }

        let config = ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();
        let connector = TlsConnector::from(Arc::new(config));

        // Connect via TCP
        let tcp_stream = match TcpStream::connect(&addr).await {
            Ok(s) => s,
            Err(e) => {
                warn!("Failed to connect to IMAP server: {}", e);
                return Ok(vec![]);
            }
        };

        // Upgrade to TLS
        let server_name =
            tokio_rustls::rustls::pki_types::ServerName::try_from(&*self.config.imap_host)
                .map_err(|e| anyhow::anyhow!("Invalid server name: {}", e))?;
        let tls_stream = match connector.connect(server_name.to_owned(), tcp_stream).await {
            Ok(s) => s,
            Err(e) => {
                warn!("TLS handshake failed: {}", e);
                return Ok(vec![]);
            }
        };

        // Create IMAP client
        let client = Client::new(tls_stream);

        // Authenticate
        let mut session = match client.login(username, password).await {
            Ok(s) => s,
            Err((e, _)) => {
                warn!("IMAP login failed: {}", e);
                return Ok(vec![]);
            }
        };

        debug!("Connected to IMAP server");

        // Select inbox
        match session.select("INBOX").await {
            Ok(_) => debug!("Selected INBOX"),
            Err(e) => {
                warn!("Failed to select INBOX: {}", e);
                let _ = session.logout().await;
                return Ok(vec![]);
            }
        }

        // Search for unseen messages
        let unseen = match session.search("UNSEEN").await {
            Ok(ids) => ids,
            Err(e) => {
                warn!("Failed to search for unseen messages: {}", e);
                let _ = session.logout().await;
                return Ok(vec![]);
            }
        };

        let mut messages = Vec::new();

        for seq in unseen.iter() {
            // Fetch the message
            let fetch_result = session.fetch(seq.to_string(), "RFC822").await;

            if let Ok(mut fetches) = fetch_result {
                while let Some(fetch) = fetches.next().await {
                    if let Ok(fetch) = fetch {
                        if let Some(email_data) = self.parse_fetch(&fetch) {
                            messages.push(email_data);
                        }
                    }
                }
            }
        }

        // Logout
        let _ = session.logout().await;

        info!("Fetched {} unread emails", messages.len());
        Ok(messages)
    }

    fn parse_fetch(&self, fetch: &async_imap::types::Fetch) -> Option<InboundMessage> {
        let body = fetch.body()?;
        let body_str = String::from_utf8_lossy(body);

        // Simple email parsing (extract From and Subject)
        let sender_id = self
            .extract_header(&body_str, "From")
            .unwrap_or_else(|| "unknown@unknown".to_string());

        let subject = self
            .extract_header(&body_str, "Subject")
            .unwrap_or_else(|| "(no subject)".to_string());

        // Extract plain text body (very basic)
        let content = self.extract_body(&body_str);

        Some(InboundMessage {
            channel: ChannelType::Email,
            sender_id: sender_id.clone(),
            chat_id: format!("email:{}", sender_id),
            content: format!("Subject: {}\n\n{}", subject, content),
            media: None,
            metadata: None,
            timestamp: chrono::Utc::now(),
            trace_id: None,
        })
    }

    fn extract_header(&self, email: &str, header: &str) -> Option<String> {
        for line in email.lines() {
            if line.starts_with(header) {
                return Some(
                    line.trim_start_matches(&format!("{}:", header))
                        .trim()
                        .to_string(),
                );
            }
            if line.is_empty() {
                break;
            }
        }
        None
    }

    fn extract_body(&self, email: &str) -> String {
        // Find empty line that separates headers from body
        if let Some(pos) = email.find("\r\n\r\n") {
            email[pos + 4..].to_string()
        } else if let Some(pos) = email.find("\n\n") {
            email[pos + 2..].to_string()
        } else {
            String::new()
        }
    }

    /// Send an email using lettre
    #[instrument(name = "channel.email.send_email", skip(self, body), fields(to = %to))]
    pub async fn send_email(&self, to: &str, subject: &str, body: &str) -> anyhow::Result<()> {
        let from: Mailbox = self.config.from_address.parse()?;
        let to_mailbox: Mailbox = to.parse()?;

        let email = Message::builder()
            .from(from)
            .to(to_mailbox)
            .subject(subject)
            .header(ContentType::TEXT_PLAIN)
            .body(body.to_string())?;

        let creds = Credentials::new(
            self.config.smtp_username.clone(),
            self.config.smtp_password.clone(),
        );

        // Build TLS transport
        let mailer: AsyncSmtpTransport<Tokio1Executor> = if self.config.smtp_port == 465 {
            AsyncSmtpTransport::<Tokio1Executor>::relay(&self.config.smtp_host)?
                .credentials(creds)
                .build()
        } else {
            // Port 587 - use STARTTLS
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&self.config.smtp_host)?
                .credentials(creds)
                .port(self.config.smtp_port)
                .build()
        };

        mailer.send(email).await?;
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
}

/// Stateless send: send an email without needing an `EmailChannel` instance.
#[allow(clippy::too_many_arguments)]
pub async fn send_email_stateless(
    smtp_host: &str,
    smtp_port: u16,
    smtp_username: &str,
    smtp_password: &str,
    from_address: &str,
    to: &str,
    subject: &str,
    body: &str,
) -> anyhow::Result<()> {
    let from: Mailbox = from_address.parse()?;
    let to_mailbox: Mailbox = to.parse()?;

    let email = Message::builder()
        .from(from)
        .to(to_mailbox)
        .subject(subject)
        .header(ContentType::TEXT_PLAIN)
        .body(body.to_string())?;

    let creds = Credentials::new(smtp_username.to_string(), smtp_password.to_string());

    let mailer: AsyncSmtpTransport<Tokio1Executor> = if smtp_port == 465 {
        AsyncSmtpTransport::<Tokio1Executor>::relay(smtp_host)?
            .credentials(creds)
            .build()
    } else {
        AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(smtp_host)?
            .credentials(creds)
            .port(smtp_port)
            .build()
    };

    mailer.send(email).await?;
    Ok(())
}
