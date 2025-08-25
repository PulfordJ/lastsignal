use super::{Output, OutputResult};
use crate::outputs::bidirectional::{BidirectionalOutput, CheckinResponse};
use crate::oauth::WhoopOAuth;
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;

/// WHOOP API client for checking device activity
#[derive(Debug)]
pub struct WhoopOutput {
    client: Client,
    oauth_client: WhoopOAuth,
    max_hours_since_activity: u64,
    name: String,
}

#[derive(Deserialize, Debug)]
struct WhoopCycleResponse {
    records: Vec<WhoopCycle>,
}

#[derive(Deserialize, Debug)]
struct WhoopCycle {
    #[allow(dead_code)]
    id: u32,
    #[allow(dead_code)]
    start: String,
    #[allow(dead_code)]
    end: Option<String>,
    #[allow(dead_code)]
    created_at: String,
    updated_at: String,
}

#[derive(Deserialize, Debug)]
struct WhoopSleepResponse {
    records: Vec<WhoopSleep>,
}

#[derive(Deserialize, Debug)]
struct WhoopSleep {
    #[allow(dead_code)]
    id: u32,
    #[allow(dead_code)]
    start: String,
    #[allow(dead_code)]
    end: String,
    #[allow(dead_code)]
    created_at: String,
    updated_at: String,
}

#[derive(Deserialize, Debug)]
struct WhoopRecoveryResponse {
    records: Vec<WhoopRecovery>,
}

#[derive(Deserialize, Debug)]
struct WhoopRecovery {
    #[allow(dead_code)]
    cycle_id: u32,
    #[allow(dead_code)]
    sleep_id: u32,
    #[allow(dead_code)]
    created_at: String,
    updated_at: String,
}

impl WhoopOutput {
    pub fn new(config: &HashMap<String, String>, data_directory: std::path::PathBuf) -> Result<Self> {
        let max_hours_since_activity = config
            .get("max_hours_since_activity")
            .unwrap_or(&"24".to_string())
            .parse::<u64>()
            .context("max_hours_since_activity must be a valid number")?;

        if max_hours_since_activity == 0 {
            anyhow::bail!("max_hours_since_activity must be greater than 0");
        }

        let client = Client::new();
        let name = "WHOOP".to_string();

        // Create OAuth client for token management
        // We use dummy client_id/secret since they're not needed for token refresh
        let oauth_client = WhoopOAuth::new(
            "dummy".to_string(),
            "dummy".to_string(),
            "dummy".to_string(),
            data_directory,
        );

        Ok(Self {
            client,
            oauth_client,
            max_hours_since_activity,
            name,
        })
    }

    async fn get_most_recent_activity_timestamp(&self) -> Result<DateTime<Utc>> {
        let mut most_recent: Option<DateTime<Utc>> = None;

        // Check most recent cycle data
        if let Ok(timestamp) = self.get_most_recent_cycle_timestamp().await {
            most_recent = Some(most_recent.map_or(timestamp, |existing| existing.max(timestamp)));
        }

        // Check most recent sleep data
        if let Ok(timestamp) = self.get_most_recent_sleep_timestamp().await {
            most_recent = Some(most_recent.map_or(timestamp, |existing| existing.max(timestamp)));
        }

        // Check most recent recovery data
        if let Ok(timestamp) = self.get_most_recent_recovery_timestamp().await {
            most_recent = Some(most_recent.map_or(timestamp, |existing| existing.max(timestamp)));
        }

        most_recent.context("No recent activity data found from WHOOP API")
    }

    async fn get_most_recent_cycle_timestamp(&self) -> Result<DateTime<Utc>> {
        let access_token = self.oauth_client.get_valid_access_token().await?;
        let url = "https://api.prod.whoop.com/developer/v1/cycle";
        let response = self
            .client
            .get(url)
            .bearer_auth(&access_token)
            .query(&[("limit", "1")])
            .send()
            .await
            .context("Failed to fetch cycle data from WHOOP API")?;

        if !response.status().is_success() {
            anyhow::bail!("WHOOP API returned error: {}", response.status());
        }

        let response_text = response.text().await
            .context("Failed to read response text from WHOOP API")?;
        
        tracing::debug!("WHOOP cycle API full response: {}", response_text);

        let cycle_response: WhoopCycleResponse = serde_json::from_str(&response_text)
            .context("Failed to parse cycle response from WHOOP API")?;

        if cycle_response.records.is_empty() {
            anyhow::bail!("No cycle data found");
        }

        let most_recent_cycle = &cycle_response.records[0];
        let timestamp = DateTime::parse_from_rfc3339(&most_recent_cycle.updated_at)
            .context("Failed to parse cycle updated_at timestamp")?
            .with_timezone(&Utc);

        Ok(timestamp)
    }

    async fn get_most_recent_sleep_timestamp(&self) -> Result<DateTime<Utc>> {
        let access_token = self.oauth_client.get_valid_access_token().await?;
        let url = "https://api.prod.whoop.com/developer/v1/activity/sleep";
        let response = self
            .client
            .get(url)
            .bearer_auth(&access_token)
            .query(&[("limit", "1")])
            .send()
            .await
            .context("Failed to fetch sleep data from WHOOP API")?;

        if !response.status().is_success() {
            anyhow::bail!("WHOOP API returned error: {}", response.status());
        }

        let response_text = response.text().await
            .context("Failed to read response text from WHOOP API")?;
        
        tracing::debug!("WHOOP sleep API full response: {}", response_text);

        let sleep_response: WhoopSleepResponse = serde_json::from_str(&response_text)
            .context("Failed to parse sleep response from WHOOP API")?;

        if sleep_response.records.is_empty() {
            anyhow::bail!("No sleep data found");
        }

        let most_recent_sleep = &sleep_response.records[0];
        let timestamp = DateTime::parse_from_rfc3339(&most_recent_sleep.updated_at)
            .context("Failed to parse sleep updated_at timestamp")?
            .with_timezone(&Utc);

        Ok(timestamp)
    }

    async fn get_most_recent_recovery_timestamp(&self) -> Result<DateTime<Utc>> {
        let access_token = self.oauth_client.get_valid_access_token().await?;
        let url = "https://api.prod.whoop.com/developer/v1/recovery";
        let response = self
            .client
            .get(url)
            .bearer_auth(&access_token)
            .query(&[("limit", "1")])
            .send()
            .await
            .context("Failed to fetch recovery data from WHOOP API")?;

        if !response.status().is_success() {
            anyhow::bail!("WHOOP API returned error: {}", response.status());
        }

        let response_text = response.text().await
            .context("Failed to read response text from WHOOP API")?;
        
        tracing::debug!("WHOOP recovery API full response: {}", response_text);

        let recovery_response: WhoopRecoveryResponse = serde_json::from_str(&response_text)
            .context("Failed to parse recovery response from WHOOP API")?;

        if recovery_response.records.is_empty() {
            anyhow::bail!("No recovery data found");
        }

        let most_recent_recovery = &recovery_response.records[0];
        let timestamp = DateTime::parse_from_rfc3339(&most_recent_recovery.updated_at)
            .context("Failed to parse recovery updated_at timestamp")?
            .with_timezone(&Utc);

        Ok(timestamp)
    }
}

#[async_trait]
impl Output for WhoopOutput {
    async fn send_message(&self, _message: &str) -> Result<OutputResult> {
        // WHOOP doesn't support sending messages, only checking activity
        // This adapter is used purely for checking if the user is alive via device activity
        Ok(OutputResult::Skipped("WHOOP is a check-only adapter".to_string()))
    }

    async fn health_check(&self) -> Result<bool> {
        // Health check by verifying we can fetch recent activity
        match self.get_most_recent_activity_timestamp().await {
            Ok(timestamp) => {
                let now = Utc::now();
                let hours_since_activity = (now - timestamp).num_hours();
                
                tracing::info!(
                    "WHOOP health check: most recent activity was {} hours ago",
                    hours_since_activity
                );
                
                Ok(hours_since_activity <= self.max_hours_since_activity as i64)
            }
            Err(e) => {
                tracing::warn!("WHOOP health check failed: {}", e);
                Ok(false)
            }
        }
    }

    fn get_name(&self) -> &str {
        &self.name
    }
}

#[async_trait]
impl BidirectionalOutput for WhoopOutput {
    async fn send_message(&self, message: &str) -> Result<OutputResult> {
        <Self as Output>::send_message(self, message).await
    }

    async fn health_check(&self) -> Result<bool> {
        <Self as Output>::health_check(self).await
    }

    fn get_name(&self) -> &str {
        <Self as Output>::get_name(self)
    }

    async fn check_for_responses(&self, since: Option<DateTime<Utc>>) -> Result<Vec<CheckinResponse>> {
        // Check if there's been recent device activity that indicates the user is alive
        let most_recent_activity = self.get_most_recent_activity_timestamp().await?;
        
        // If no "since" timestamp provided, use a default lookback period
        let cutoff_time = since.unwrap_or_else(|| {
            Utc::now() - chrono::Duration::hours(self.max_hours_since_activity as i64)
        });

        if most_recent_activity > cutoff_time {
            // Found recent activity - this counts as a "check-in"
            let response = CheckinResponse::Found {
                timestamp: most_recent_activity,
                subject: "WHOOP Device Activity Detected".to_string(),
                from: "WHOOP Device".to_string(),
            };
            
            tracing::info!(
                "WHOOP detected recent activity at {}, treating as check-in",
                most_recent_activity
            );
            
            Ok(vec![response])
        } else {
            tracing::debug!(
                "WHOOP: No recent activity since {}. Most recent activity was at {}",
                cutoff_time,
                most_recent_activity
            );
            Ok(vec![])
        }
    }

    async fn mark_processed_until(&self, _timestamp: DateTime<Utc>) -> Result<()> {
        // No need to persist anything for WHOOP - we always check recent activity
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_whoop_output_creation() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut config = HashMap::new();
        config.insert("max_hours_since_activity".to_string(), "24".to_string());

        let output = WhoopOutput::new(&config, temp_dir.path().to_path_buf());
        assert!(output.is_ok());
        
        let output = output.unwrap();
        assert_eq!(<dyn Output>::get_name(&output), "WHOOP");
        assert_eq!(output.max_hours_since_activity, 24);
    }

    #[test]
    fn test_whoop_output_creation_with_defaults() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = HashMap::new();
        let result = WhoopOutput::new(&config, temp_dir.path().to_path_buf());
        assert!(result.is_ok());
        
        let output = result.unwrap();
        assert_eq!(output.max_hours_since_activity, 24); // default value
    }

    #[test]
    fn test_whoop_output_invalid_max_hours() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut config = HashMap::new();
        config.insert("max_hours_since_activity".to_string(), "0".to_string());

        let result = WhoopOutput::new(&config, temp_dir.path().to_path_buf());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("must be greater than 0"));
    }

    #[test]
    fn test_whoop_output_default_max_hours() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = HashMap::new();

        let output = WhoopOutput::new(&config, temp_dir.path().to_path_buf()).unwrap();
        assert_eq!(output.max_hours_since_activity, 24);
    }

    #[tokio::test]
    async fn test_whoop_send_message_returns_skipped() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = HashMap::new();

        let output = WhoopOutput::new(&config, temp_dir.path().to_path_buf()).unwrap();
        let result = <dyn Output>::send_message(&output, "test message").await.unwrap();

        match result {
            OutputResult::Skipped(reason) => {
                assert!(reason.contains("check-only adapter"));
            }
            _ => panic!("Expected Skipped result"),
        }
    }
}