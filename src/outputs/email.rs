use super::{Output, OutputResult};
use anyhow::{Context, Result};
use async_trait::async_trait;
use lettre::{
    message::header::ContentType,
    transport::smtp::authentication::Credentials,
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct EmailOutput {
    to: String,
    from: String,
    smtp_host: String,
    smtp_port: u16,
    username: String,
    password: String,
}

impl EmailOutput {
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

        Ok(EmailOutput {
            to,
            from,
            smtp_host,
            smtp_port,
            username,
            password,
        })
    }

    async fn create_transport(&self) -> Result<AsyncSmtpTransport<Tokio1Executor>> {
        let creds = Credentials::new(self.username.clone(), self.password.clone());

        let transport = AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&self.smtp_host)
            .context("Failed to create SMTP transport")?
            .port(self.smtp_port)
            .credentials(creds)
            .build();

        Ok(transport)
    }
}

#[async_trait]
impl Output for EmailOutput {
    async fn send_message(&self, message: &str) -> Result<OutputResult> {
        let email = Message::builder()
            .from(self.from.parse().context("Invalid from email address")?)
            .to(self.to.parse().context("Invalid to email address")?)
            .subject("LastSignal Notification")
            .header(ContentType::TEXT_PLAIN)
            .body(message.to_string())
            .context("Failed to build email message")?;

        let transport = match self.create_transport().await {
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
        match self.create_transport().await {
            Ok(transport) => {
                match transport.test_connection().await {
                    Ok(_) => Ok(true),
                    Err(e) => {
                        tracing::debug!("Email health check failed: {}", e);
                        Ok(false)
                    }
                }
            }
            Err(e) => {
                tracing::debug!("Email transport creation failed during health check: {}", e);
                Ok(false)
            }
        }
    }

    fn get_name(&self) -> &str {
        "email"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_email_output_creation() {
        let mut config = HashMap::new();
        config.insert("to".to_string(), "test@example.com".to_string());
        config.insert("smtp_host".to_string(), "smtp.example.com".to_string());
        config.insert("smtp_port".to_string(), "587".to_string());
        config.insert("username".to_string(), "user@example.com".to_string());
        config.insert("password".to_string(), "password".to_string());

        let output = EmailOutput::new(&config).unwrap();
        assert_eq!(output.to, "test@example.com");
        assert_eq!(output.smtp_host, "smtp.example.com");
        assert_eq!(output.smtp_port, 587);
        assert_eq!(output.username, "user@example.com");
        assert_eq!(output.from, "user@example.com"); // defaults to username
    }

    #[test]
    fn test_email_output_creation_with_from() {
        let mut config = HashMap::new();
        config.insert("to".to_string(), "test@example.com".to_string());
        config.insert("from".to_string(), "from@example.com".to_string());
        config.insert("smtp_host".to_string(), "smtp.example.com".to_string());
        config.insert("smtp_port".to_string(), "587".to_string());
        config.insert("username".to_string(), "user@example.com".to_string());
        config.insert("password".to_string(), "password".to_string());

        let output = EmailOutput::new(&config).unwrap();
        assert_eq!(output.from, "from@example.com");
    }

    #[test]
    fn test_email_output_missing_config() {
        let config = HashMap::new();
        let result = EmailOutput::new(&config);
        assert!(result.is_err());
    }
}