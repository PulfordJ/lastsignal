use super::{Output, OutputResult};
use crate::outputs::bidirectional::{BidirectionalOutput, CheckinResponse};
use crate::oauth::WhoopOAuth;
use crate::duration_parser::ConfigDuration;
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// WHOOP API client for checking device activity
#[derive(Debug)]
pub struct WhoopOutput {
    client: Client,
    oauth_client: Arc<RwLock<WhoopOAuth>>,
    max_time_since_last_checkin: ConfigDuration,
    name: String,
    _refresh_task_handle: tokio::task::JoinHandle<()>,
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
    pub fn new(_config: &HashMap<String, String>, data_directory: std::path::PathBuf, max_time_since_last_checkin: ConfigDuration) -> Result<Self> {

        let client = Client::new();
        let name = "WHOOP".to_string();

        // Create OAuth client for token management
        // We use dummy client_id/secret since they're not needed for token refresh
        let oauth_client = Arc::new(RwLock::new(WhoopOAuth::new(
            "dummy".to_string(),
            "dummy".to_string(),
            "dummy".to_string(),
            data_directory,
        )));

        // Spawn background task to refresh token every 30 minutes
        let oauth_client_clone = Arc::clone(&oauth_client);
        let refresh_task_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(30 * 60)); // 30 minutes
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            
            loop {
                interval.tick().await;
                
                // Attempt to refresh the token
                let oauth_client = oauth_client_clone.read().await;
                match oauth_client.load_tokens() {
                    Ok(tokens) => {
                        // Check if token needs refreshing (expires within next 35 minutes)
                        let now = Utc::now();
                        let buffer = chrono::Duration::minutes(35);
                        
                        if tokens.expires_at <= now + buffer {
                            tracing::info!("WHOOP: Proactively refreshing access token in background");
                            
                            match oauth_client.refresh_token(&tokens.refresh_token).await {
                                Ok(new_tokens) => {
                                    if let Err(e) = oauth_client.save_tokens(&new_tokens) {
                                        tracing::error!("WHOOP: Failed to save refreshed tokens: {}", e);
                                    } else {
                                        tracing::info!("WHOOP: Successfully refreshed access token in background");
                                    }
                                }
                                Err(e) => {
                                    tracing::error!("WHOOP: Failed to refresh token in background: {}", e);
                                }
                            }
                        } else {
                            tracing::debug!("WHOOP: Token still valid, no refresh needed");
                        }
                    }
                    Err(e) => {
                        tracing::warn!("WHOOP: Could not load tokens for background refresh: {}", e);
                    }
                }
            }
        });

        Ok(Self {
            client,
            oauth_client,
            max_time_since_last_checkin,
            name,
            _refresh_task_handle: refresh_task_handle,
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
        let oauth_client = self.oauth_client.read().await;
        let access_token = oauth_client.get_valid_access_token().await?;
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
        let oauth_client = self.oauth_client.read().await;
        let access_token = oauth_client.get_valid_access_token().await?;
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
        let oauth_client = self.oauth_client.read().await;
        let access_token = oauth_client.get_valid_access_token().await?;
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
                
                Ok(hours_since_activity <= self.max_time_since_last_checkin.as_hours() as i64)
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

    async fn check_for_responses(&self, _since: Option<DateTime<Utc>>) -> Result<Vec<CheckinResponse>> {
        // Check if there's been recent device activity that indicates the user is alive
        let most_recent_activity = self.get_most_recent_activity_timestamp().await?;
        
        // Always use our configured max_time_since_last_checkin window, not the 'since' parameter
        // WHOOP determines "aliveness" based on recent device activity within our configured window
        let cutoff_time = Utc::now() - chrono::Duration::hours(self.max_time_since_last_checkin.as_hours() as i64);

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
                "WHOOP: No recent activity within {} hours. Most recent activity was at {}",
                self.max_time_since_last_checkin.as_hours(),
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
    use std::time::Duration;

    #[tokio::test]
    async fn test_whoop_output_creation() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = HashMap::new();
        let max_time = ConfigDuration::from_hours(24);

        let output = WhoopOutput::new(&config, temp_dir.path().to_path_buf(), max_time);
        assert!(output.is_ok());
        
        let output = output.unwrap();
        assert_eq!(<dyn Output>::get_name(&output), "WHOOP");
        assert_eq!(output.max_time_since_last_checkin.as_hours(), 24);
        
        // Give the background task a moment to start
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    #[tokio::test]
    async fn test_whoop_output_creation_with_defaults() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = HashMap::new();
        let max_time = ConfigDuration::from_days(14); // Using system default
        let result = WhoopOutput::new(&config, temp_dir.path().to_path_buf(), max_time);
        assert!(result.is_ok());
        
        let output = result.unwrap();
        assert_eq!(output.max_time_since_last_checkin.as_days(), 14); // default value
        
        // Give the background task a moment to start
        tokio::time::sleep(Duration::from_millis(10)).await;
    }



    #[tokio::test]
    async fn test_whoop_send_message_returns_skipped() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = HashMap::new();
        let max_time = ConfigDuration::from_hours(24);

        let output = WhoopOutput::new(&config, temp_dir.path().to_path_buf(), max_time).unwrap();
        let result = <dyn Output>::send_message(&output, "test message").await.unwrap();

        match result {
            OutputResult::Skipped(reason) => {
                assert!(reason.contains("check-only adapter"));
            }
            _ => panic!("Expected Skipped result"),
        }
        
        // Give the background task a moment to start
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}