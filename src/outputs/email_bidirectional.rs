use super::bidirectional::{BidirectionalOutput, CheckinResponse};
use super::{Output, OutputResult};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use lettre::{
    message::header::ContentType,
    transport::smtp::authentication::Credentials,
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};
use std::collections::HashMap;

// For IMAP email checking
use async_imap::{Client, Session};
use async_native_tls::{TlsConnector, TlsStream};
use async_std::net::TcpStream;

#[derive(Debug, Clone)]
pub struct BidirectionalEmailOutput {
    // SMTP fields (for sending)
    to: String,
    from: String,
    smtp_host: String,
    smtp_port: u16,
    username: String,
    password: String,
    
    // IMAP fields (for receiving)
    imap_host: String,
    imap_port: u16,
    
    // Subject prefix to look for in replies
    subject_prefix: String,
}

impl BidirectionalEmailOutput {
    pub fn new(config: &HashMap<String, String>) -> Result<Self> {
        let to = config
            .get("to")
            .context("Missing 'to' field in email config")?
            .clone();

        let smtp_host = config
            .get("smtp_host")
            .context("Missing 'smtp_host' field in email config")?
            .clone();

        let smtp_port: u16 = config
            .get("smtp_port")
            .context("Missing 'smtp_port' field in email config")?
            .parse()
            .context("Invalid 'smtp_port' value in email config")?;

        let username = config
            .get("username")
            .context("Missing 'username' field in email config")?
            .clone();

        let password = config
            .get("password")
            .context("Missing 'password' field in email config")?
            .clone();

        let from = config
            .get("from")
            .unwrap_or(&username)
            .clone();

        // IMAP configuration - use defaults if not specified
        let imap_host = config
            .get("imap_host")
            .unwrap_or(&smtp_host.replace("smtp", "imap"))
            .clone();

        let imap_port: u16 = config
            .get("imap_port")
            .map_or("993", |v| v)
            .parse()
            .context("Invalid 'imap_port' value in email config")?;

        let subject_prefix = config
            .get("subject_prefix")
            .map_or("LastSignal", |v| v)
            .to_string();

        Ok(BidirectionalEmailOutput {
            to,
            from,
            smtp_host,
            smtp_port,
            username,
            password,
            imap_host,
            imap_port,
            subject_prefix,
        })
    }

    async fn create_smtp_transport(&self) -> Result<AsyncSmtpTransport<Tokio1Executor>> {
        let creds = Credentials::new(self.username.clone(), self.password.clone());

        let transport = AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&self.smtp_host)
            .context("Failed to create SMTP transport")?
            .port(self.smtp_port)
            .credentials(creds)
            .build();

        Ok(transport)
    }

    async fn create_imap_session(&self) -> Result<Session<TlsStream<TcpStream>>> {
        use tokio::time::{timeout, Duration};
        
        let addr = format!("{}:{}", self.imap_host, self.imap_port);
        tracing::debug!("Connecting to IMAP server: {}", addr);
        
        let tcp_stream = timeout(Duration::from_secs(30), TcpStream::connect(&addr)).await
            .context("IMAP connection timed out")?
            .context("Failed to connect to IMAP server")?;
        
        tracing::debug!("Establishing TLS connection to {}", self.imap_host);
        let tls = TlsConnector::new();
        let tls_stream = timeout(Duration::from_secs(30), tls.connect(&self.imap_host, tcp_stream)).await
            .context("TLS connection timed out")?
            .context("Failed to establish TLS connection")?;

        tracing::debug!("Logging in to IMAP as {}", self.username);
        let client = Client::new(tls_stream);
        let session = timeout(Duration::from_secs(30), client.login(&self.username, &self.password)).await
            .context("IMAP login timed out")?
            .map_err(|e| anyhow::anyhow!("Failed to login to IMAP: {}", e.0))?;

        tracing::debug!("IMAP session established successfully");
        Ok(session)
    }

    async fn check_inbox_for_replies(&self, since: Option<DateTime<Utc>>) -> Result<Vec<CheckinResponse>> {
        use tokio::time::{timeout, Duration};
        
        tracing::debug!("Checking inbox for replies since: {:?}", since);
        let mut session = self.create_imap_session().await?;
        
        // Select INBOX
        tracing::debug!("Selecting INBOX");
        timeout(Duration::from_secs(30), session.select("INBOX")).await
            .context("INBOX select timed out")?
            .context("Failed to select INBOX")?;

        // Build search criteria - only look for replies (RE: prefix)
        let search_criteria = if let Some(since_date) = since {
            // Search for emails since the given date that are replies to our subject
            format!("SINCE {} SUBJECT \"RE: {} Notification\"", 
                since_date.format("%d-%b-%Y"), 
                self.subject_prefix)
        } else {
            // Just search for reply emails to our subject
            format!("SUBJECT \"RE: {} Notification\"", self.subject_prefix)
        };

        tracing::debug!("Searching with criteria: {}", search_criteria);
        let message_ids = timeout(Duration::from_secs(30), session.search(&search_criteria)).await
            .context("Email search timed out")?
            .context("Failed to search emails")?;

        if message_ids.is_empty() {
            tracing::debug!("No messages found matching search criteria");
            timeout(Duration::from_secs(10), session.logout()).await.ok();
            return Ok(vec![]);
        }
        
        tracing::debug!("Found {} messages matching search criteria", message_ids.len());

        // Fetch the messages  
        let message_ids_str = message_ids.iter()
            .map(|id| id.to_string())
            .collect::<Vec<String>>()
            .join(",");
        use futures_util::stream::StreamExt;
        
        let mut message_stream = timeout(Duration::from_secs(30), session.fetch(&message_ids_str, "ENVELOPE")).await
            .context("Message fetch timed out")?
            .context("Failed to fetch messages")?;

        let mut responses = Vec::new();
        
        while let Some(message_result) = message_stream.next().await {
            let message = match message_result {
                Ok(msg) => msg,
                Err(e) => {
                    tracing::warn!("Failed to fetch message: {}", e);
                    continue;
                }
            };
            if let Some(envelope) = message.envelope() {
                if let (Some(date), Some(subject), Some(from)) = (
                    envelope.date.as_ref(),
                    envelope.subject.as_ref(),
                    envelope.from.as_ref().and_then(|f| f.first())
                ) {
                    // Parse the date
                    if let Ok(parsed_date) = chrono::DateTime::parse_from_rfc2822(
                        &String::from_utf8_lossy(date)
                    ) {
                        let timestamp = parsed_date.with_timezone(&Utc);
                        
                        // Check if this is after our 'since' timestamp
                        if let Some(since_time) = since {
                            if timestamp <= since_time {
                                continue;
                            }
                        }
                        
                        let subject_str = String::from_utf8_lossy(subject);
                        let from_str = if let (Some(name), Some(email)) = (from.name.as_ref(), from.mailbox.as_ref()) {
                            format!("{} <{}@{}>", 
                                String::from_utf8_lossy(name),
                                String::from_utf8_lossy(email),
                                from.host.as_ref().map(|h| String::from_utf8_lossy(h)).unwrap_or_default()
                            )
                        } else if let Some(email) = from.mailbox.as_ref() {
                            format!("{}@{}", 
                                String::from_utf8_lossy(email),
                                from.host.as_ref().map(|h| String::from_utf8_lossy(h)).unwrap_or_default()
                            )
                        } else {
                            "Unknown".to_string()
                        };
                        
                        responses.push(CheckinResponse::Found {
                            timestamp,
                            subject: subject_str.to_string(),
                            from: from_str,
                        });
                    }
                }
            }
        }

        // Explicitly drop the message stream to release the session borrow
        drop(message_stream);
        
        tracing::debug!("Processed {} email responses, logging out", responses.len());
        timeout(Duration::from_secs(10), session.logout()).await.ok();
        Ok(responses)
    }
}

#[async_trait]
impl Output for BidirectionalEmailOutput {
    async fn send_message(&self, message: &str) -> Result<OutputResult> {
        let email = Message::builder()
            .from(self.from.parse().context("Invalid from email address")?)
            .to(self.to.parse().context("Invalid to email address")?)
            .subject(&format!("{} Notification", self.subject_prefix))
            .header(ContentType::TEXT_PLAIN)
            .body(message.to_string())
            .context("Failed to build email message")?;

        let transport = match self.create_smtp_transport().await {
            Ok(t) => t,
            Err(e) => {
                return Ok(OutputResult::Failed(format!("Failed to create transport: {}", e)));
            }
        };

        match transport.send(email).await {
            Ok(_) => Ok(OutputResult::Success),
            Err(e) => Ok(OutputResult::Failed(format!("Failed to send email: {}", e))),
        }
    }

    async fn health_check(&self) -> Result<bool> {
        // Check both SMTP (sending) and IMAP (receiving) connectivity
        let smtp_ok = match self.create_smtp_transport().await {
            Ok(transport) => {
                match transport.test_connection().await {
                    Ok(_) => true,
                    Err(e) => {
                        tracing::debug!("SMTP health check failed: {}", e);
                        false
                    }
                }
            }
            Err(e) => {
                tracing::debug!("SMTP transport creation failed during health check: {}", e);
                false
            }
        };

        let imap_ok = match self.create_imap_session().await {
            Ok(mut session) => {
                let result = session.logout().await.is_ok();
                result
            }
            Err(e) => {
                tracing::debug!("IMAP health check failed: {}", e);
                false
            }
        };

        Ok(smtp_ok && imap_ok)
    }

    fn get_name(&self) -> &str {
        "bidirectional_email"
    }
}

#[async_trait]
impl BidirectionalOutput for BidirectionalEmailOutput {
    async fn send_message(&self, message: &str) -> Result<OutputResult> {
        Output::send_message(self, message).await
    }
    
    async fn health_check(&self) -> Result<bool> {
        Output::health_check(self).await
    }
    
    fn get_name(&self) -> &str {
        Output::get_name(self)
    }
    
    async fn check_for_responses(&self, since: Option<DateTime<Utc>>) -> Result<Vec<CheckinResponse>> {
        self.check_inbox_for_replies(since).await
    }
    
    async fn mark_processed_until(&self, _timestamp: DateTime<Utc>) -> Result<()> {
        // For email, we don't need to mark as processed since we use timestamp-based filtering
        // The IMAP search with SINCE handles this automatically
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bidirectional_email_output_creation() {
        let mut config = HashMap::new();
        config.insert("to".to_string(), "test@example.com".to_string());
        config.insert("smtp_host".to_string(), "smtp.example.com".to_string());
        config.insert("smtp_port".to_string(), "587".to_string());
        config.insert("username".to_string(), "user@example.com".to_string());
        config.insert("password".to_string(), "password".to_string());

        let output = BidirectionalEmailOutput::new(&config).unwrap();
        assert_eq!(output.to, "test@example.com");
        assert_eq!(output.smtp_host, "smtp.example.com");
        assert_eq!(output.smtp_port, 587);
        assert_eq!(output.username, "user@example.com");
        assert_eq!(output.from, "user@example.com");
        assert_eq!(output.imap_host, "imap.example.com"); // auto-converted
        assert_eq!(output.imap_port, 993); // default IMAP SSL port
        assert_eq!(output.subject_prefix, "LastSignal"); // default
    }

    #[test]
    fn test_bidirectional_email_output_with_custom_imap() {
        let mut config = HashMap::new();
        config.insert("to".to_string(), "test@example.com".to_string());
        config.insert("smtp_host".to_string(), "smtp.example.com".to_string());
        config.insert("smtp_port".to_string(), "587".to_string());
        config.insert("imap_host".to_string(), "mail.example.com".to_string());
        config.insert("imap_port".to_string(), "143".to_string());
        config.insert("username".to_string(), "user@example.com".to_string());
        config.insert("password".to_string(), "password".to_string());
        config.insert("subject_prefix".to_string(), "MyApp".to_string());

        let output = BidirectionalEmailOutput::new(&config).unwrap();
        assert_eq!(output.imap_host, "mail.example.com");
        assert_eq!(output.imap_port, 143);
        assert_eq!(output.subject_prefix, "MyApp");
    }
}