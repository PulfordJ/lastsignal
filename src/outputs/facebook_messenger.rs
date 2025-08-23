use super::{Output, OutputResult};
use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde_json::json;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct FacebookMessengerOutput {
    user_id: String,
    access_token: String,
    client: Client,
}

impl FacebookMessengerOutput {
    pub fn new(config: &HashMap<String, String>) -> Result<Self> {
        let user_id = config
            .get("user_id")
            .context("Missing 'user_id' field in facebook_messenger config")?
            .clone();

        let access_token = config
            .get("access_token")
            .context("Missing 'access_token' field in facebook_messenger config")?
            .clone();

        let client = Client::new();

        Ok(FacebookMessengerOutput {
            user_id,
            access_token,
            client,
        })
    }

    fn get_send_url(&self) -> String {
        format!(
            "https://graph.facebook.com/v18.0/me/messages?access_token={}",
            self.access_token
        )
    }

    fn get_profile_url(&self) -> String {
        format!(
            "https://graph.facebook.com/v18.0/me?access_token={}",
            self.access_token
        )
    }
}

#[async_trait]
impl Output for FacebookMessengerOutput {
    async fn send_message(&self, message: &str) -> Result<OutputResult> {
        let payload = json!({
            "recipient": {
                "id": self.user_id
            },
            "message": {
                "text": message
            }
        });

        let response = match self
            .client
            .post(&self.get_send_url())
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(e) => {
                return Ok(OutputResult::Failed(format!("HTTP request failed: {}", e)));
            }
        };

        if response.status().is_success() {
            match response.json::<serde_json::Value>().await {
                Ok(json) => {
                    if json.get("error").is_some() {
                        let error_msg = json["error"]["message"]
                            .as_str()
                            .unwrap_or("Unknown Facebook API error");
                        Ok(OutputResult::Failed(format!("Facebook API error: {}", error_msg)))
                    } else {
                        Ok(OutputResult::Success)
                    }
                }
                Err(e) => Ok(OutputResult::Failed(format!("Failed to parse response: {}", e))),
            }
        } else {
            let status_code = response.status();
            match response.text().await {
                Ok(text) => Ok(OutputResult::Failed(format!("HTTP {}: {}", status_code, text))),
                Err(e) => Ok(OutputResult::Failed(format!("HTTP {} (failed to read response: {})", status_code, e))),
            }
        }
    }

    async fn health_check(&self) -> Result<bool> {
        let response = match self
            .client
            .get(&self.get_profile_url())
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(e) => {
                tracing::debug!("Facebook Messenger health check HTTP error: {}", e);
                return Ok(false);
            }
        };

        if response.status().is_success() {
            match response.json::<serde_json::Value>().await {
                Ok(json) => {
                    if json.get("error").is_some() {
                        tracing::debug!("Facebook Messenger health check API error: {:?}", json["error"]);
                        Ok(false)
                    } else if json.get("id").is_some() {
                        Ok(true)
                    } else {
                        tracing::debug!("Facebook Messenger health check: unexpected response format");
                        Ok(false)
                    }
                }
                Err(e) => {
                    tracing::debug!("Facebook Messenger health check parse error: {}", e);
                    Ok(false)
                }
            }
        } else {
            tracing::debug!("Facebook Messenger health check HTTP error: {}", response.status());
            Ok(false)
        }
    }

    fn get_name(&self) -> &str {
        "facebook_messenger"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_facebook_messenger_output_creation() {
        let mut config = HashMap::new();
        config.insert("user_id".to_string(), "123456789".to_string());
        config.insert("access_token".to_string(), "test_token".to_string());

        let output = FacebookMessengerOutput::new(&config).unwrap();
        assert_eq!(output.user_id, "123456789");
        assert_eq!(output.access_token, "test_token");
    }

    #[test]
    fn test_facebook_messenger_output_missing_config() {
        let config = HashMap::new();
        let result = FacebookMessengerOutput::new(&config);
        assert!(result.is_err());
    }

    #[test]
    fn test_facebook_messenger_urls() {
        let mut config = HashMap::new();
        config.insert("user_id".to_string(), "123456789".to_string());
        config.insert("access_token".to_string(), "test_token".to_string());

        let output = FacebookMessengerOutput::new(&config).unwrap();
        
        assert!(output.get_send_url().contains("test_token"));
        assert!(output.get_profile_url().contains("test_token"));
    }
}