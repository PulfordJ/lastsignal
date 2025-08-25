use anyhow::{Context, Result};
use tokio::time::{sleep, Duration};

use crate::config::Config;
use crate::message_adapter::{MessageAdapter, MessageAdapterFactory};
use crate::outputs::{
    process_outputs_to_all, process_last_signal_outputs, generate_recipient_id, Output, OutputFactory, OutputResult,
    bidirectional::{BidirectionalOutput, BidirectionalOutputFactory, process_bidirectional_outputs_for_checkins, mark_all_processed_until}
};
use crate::state::StateManager;

pub struct LastSignalApp {
    config: Config,
    state_manager: StateManager,
    message_adapter: Box<dyn MessageAdapter>,
    checkin_outputs: Vec<Box<dyn BidirectionalOutput>>,
    last_signal_outputs: Vec<Box<dyn Output>>,
    last_signal_output_configs: Vec<crate::config::OutputConfig>,
}

impl LastSignalApp {
    pub async fn new() -> Result<Self> {
        tracing::debug!("Loading configuration...");
        let config = Config::load()
            .context("Failed to load configuration. Make sure config.toml exists in ~/.lastsignal/")?;
        
        Self::from_config(config).await
    }

    pub async fn from_config(config: Config) -> Result<Self> {

        tracing::debug!("Getting data directory...");
        let data_directory = config.get_data_directory()
            .context("Failed to determine data directory")?;

        tracing::debug!("Creating state manager...");
        let state_manager = StateManager::new(&data_directory)
            .context("Failed to initialize state manager")?;

        tracing::debug!("Getting message file path...");
        let message_file_path = config.get_message_file_path()
            .context("Failed to determine message file path")?;

        tracing::debug!("Creating message adapter...");
        let message_adapter = MessageAdapterFactory::create_adapter(
            &config.last_signal.adapter_type,
            &message_file_path,
        ).context("Failed to create message adapter")?;

        tracing::debug!("Creating checkin outputs...");
        let mut checkin_outputs: Vec<Box<dyn BidirectionalOutput>> = Vec::new();
        for (i, output_config) in config.checkin.outputs.iter().enumerate() {
            tracing::debug!("Creating checkin output {} of type {}", i + 1, output_config.output_type);
            let output = BidirectionalOutputFactory::create_bidirectional_output(
                &output_config.output_type, 
                &output_config.config,
                output_config.bidirectional,
                Some(&data_directory)
            ).with_context(|| format!("Failed to create checkin output: {}", output_config.output_type))?;
            checkin_outputs.push(output);
            tracing::debug!("Successfully created checkin output {}", i + 1);
        }

        let mut last_signal_outputs: Vec<Box<dyn Output>> = Vec::new();
        for output_config in &config.recipient.last_signal_outputs {
            let output = OutputFactory::create_output(&output_config.output_type, &output_config.config, Some(&data_directory))
                .with_context(|| format!("Failed to create last signal output: {}", output_config.output_type))?;
            last_signal_outputs.push(output);
        }

        let last_signal_output_configs = config.recipient.last_signal_outputs.clone();

        tracing::debug!("App initialization complete");
        Ok(LastSignalApp {
            config,
            state_manager,
            message_adapter,
            checkin_outputs,
            last_signal_outputs,
            last_signal_output_configs,
        })
    }

    pub async fn run(&mut self) -> Result<()> {
        tracing::info!("Starting LastSignal application");
        tracing::info!("Configuration loaded: {} checkin outputs, {} last signal outputs", 
            self.checkin_outputs.len(), 
            self.last_signal_outputs.len());

        // Check for unsent last signal recipients on startup
        self.check_for_pending_last_signal_recipients().await?;

        tracing::debug!("Entering main loop");
        loop {
            tracing::debug!("About to run cycle");
            if let Err(e) = self.run_cycle().await {
                tracing::error!("Error in application cycle: {}", e);
                sleep(Duration::from_secs(300)).await; // Wait 5 minutes before retrying
                continue;
            }

            // Sleep for configured interval before next check
            let check_interval = self.config.app.check_interval.as_secs();
            tracing::debug!("Cycle complete, sleeping for {} seconds ({})", check_interval, self.config.app.check_interval);
            sleep(Duration::from_secs(check_interval)).await;
        }
    }

    async fn run_cycle(&mut self) -> Result<()> {
        tracing::debug!("Running application cycle");

        // First, check for any bidirectional responses that could be check-ins
        tracing::debug!("About to check bidirectional responses...");
        self.process_bidirectional_checkins().await?;
        tracing::debug!("Finished checking bidirectional responses");

        // Check if we need to request a checkin
        tracing::debug!("Checking if we should request checkin...");
        if self.should_request_checkin().await? {
            tracing::info!("Time to request checkin");
            self.request_checkin().await?;
        } else {
            tracing::debug!("No checkin request needed");
        }

        // Check if we need to fire the last signal
        tracing::debug!("Checking if we should fire last signal...");
        if self.should_fire_last_signal().await? {
            tracing::warn!("Time to fire last signal");
            self.fire_last_signal().await?;
        } else {
            tracing::debug!("No last signal needed");
        }

        tracing::debug!("Application cycle completed");
        Ok(())
    }

    async fn should_request_checkin(&self) -> Result<bool> {
        let state = self.state_manager.get_state();
        Ok(state.should_request_checkin(self.config.checkin.duration_between_checkins))
    }

    async fn should_fire_last_signal(&self) -> Result<bool> {
        let state = self.state_manager.get_state();
        
        // Don't fire if we've already fired recently
        if state.has_fired_last_signal_recently(self.config.recipient.max_time_since_last_checkin) {
            return Ok(false);
        }

        Ok(state.should_fire_last_signal(self.config.recipient.max_time_since_last_checkin))
    }

    async fn request_checkin(&mut self) -> Result<()> {
        tracing::info!("Requesting checkin from admin");

        let message = self.message_adapter.generate_checkin_message()
            .context("Failed to generate checkin message")?;

        let result = self.send_message_via_bidirectional_outputs(&message).await?;

        match result {
            OutputResult::Success => {
                tracing::info!("Checkin request sent successfully");
                self.state_manager.record_checkin_request()
                    .context("Failed to record checkin request")?;
            }
            OutputResult::Failed(error) => {
                tracing::error!("Failed to send checkin request: {}", error);
                self.state_manager.record_checkin_request()
                    .context("Failed to send checkin request")?;
            }
            OutputResult::Skipped(reason) => {
                tracing::info!("Checkin request skipped: {}", reason);
                self.state_manager.record_checkin_request()
                    .context("Failed to record checkin request")?;
            }
        }

        Ok(())
    }

    async fn fire_last_signal(&mut self) -> Result<()> {
        tracing::warn!("Firing last signal to recipients");

        let message = self.message_adapter.generate_last_signal_message()
            .context("Failed to generate last signal message")?;

        let results = process_last_signal_outputs(
            &self.last_signal_output_configs,
            &self.last_signal_outputs,
            &message,
            &mut self.state_manager,
        ).await?;

        let mut success_count = 0;
        let mut failure_count = 0;
        let mut skip_count = 0;
        let mut already_notified_count = 0;

        for (output_name, recipient_id, result) in results {
            match result {
                OutputResult::Success => {
                    success_count += 1;
                    tracing::info!("Last signal sent successfully to {} ({})", output_name, recipient_id);
                }
                OutputResult::Failed(error) => {
                    failure_count += 1;
                    tracing::error!("Failed to send last signal to {} ({}): {}", output_name, recipient_id, error);
                }
                OutputResult::Skipped(reason) => {
                    if reason.contains("already notified") {
                        already_notified_count += 1;
                    } else {
                        skip_count += 1;
                    }
                    tracing::warn!("Last signal skipped for {} ({}): {}", output_name, recipient_id, reason);
                }
            }
        }

        if success_count > 0 {
            tracing::warn!("Last signal sent successfully to {} recipient(s)", success_count);
            if failure_count > 0 || skip_count > 0 {
                tracing::warn!("Some last signal deliveries failed or were skipped: {} failed, {} health/other skipped", failure_count, skip_count);
            }
            if already_notified_count > 0 {
                tracing::info!("{} recipient(s) already notified, skipped to prevent spam", already_notified_count);
            }
            self.state_manager.record_last_signal_fired()
                .context("Failed to record last signal fired")?;
        } else if already_notified_count > 0 {
            tracing::info!("All {} recipient(s) already notified - no new notifications sent", already_notified_count);
        } else {
            let error_msg = format!("All {} last signal output(s) failed or were skipped", failure_count + skip_count);
            tracing::error!("{}", error_msg);
            anyhow::bail!("{}", error_msg);
        }

        Ok(())
    }

    async fn check_for_pending_last_signal_recipients(&self) -> Result<()> {
        let state = self.state_manager.get_state();
        
        // Only check if last signal was previously fired
        if state.last_signal_fired.is_none() {
            return Ok(());
        }
        
        // Generate list of all recipient IDs
        let all_recipient_ids: Vec<String> = self.last_signal_output_configs
            .iter()
            .map(|config| generate_recipient_id(config))
            .collect();
            
        let pending_recipients = state.get_pending_last_signal_recipients(&all_recipient_ids);
        
        if pending_recipients.is_empty() {
            // All recipients have been notified - nothing left to do
            eprintln!("🚨 ERROR: LastSignal has already completed all emergency notifications.");
            eprintln!("   All configured recipients have been successfully notified.");
            eprintln!();
            eprintln!("LastSignal has nothing to do and should not be running.");
            eprintln!("If you want to restart LastSignal for future monitoring:");
            eprintln!("   1. Delete the state.json file: rm ~/.lastsignal/state.json");
            eprintln!("   2. Re-run LastSignal");
            eprintln!();
            eprintln!("WARNING: This will reset all tracking and start fresh monitoring.");
            
            anyhow::bail!(
                "Cannot start: All {} recipient(s) already notified - emergency process complete.",
                all_recipient_ids.len()
            );
        }
        
        Ok(())
    }

    pub async fn checkin(&mut self) -> Result<()> {
        tracing::info!("Recording manual checkin");
        self.state_manager.record_checkin()
            .context("Failed to record checkin")?;
        
        // Clear last signal recipient tracking since user is now alive
        self.state_manager.clear_last_signal_recipient_tracking()
            .context("Failed to clear last signal recipient tracking")?;
        
        println!("Checkin recorded successfully!");
        Ok(())
    }

    pub async fn status(&self) -> Result<()> {
        let state = self.state_manager.get_state();
        
        println!("LastSignal Status:");
        println!("==================");
        
        match state.last_checkin {
            Some(checkin_time) => {
                let days_since = state.days_since_last_checkin().unwrap_or(0);
                println!("Last checkin: {} ({} days ago)", checkin_time.format("%Y-%m-%d %H:%M:%S UTC"), days_since);
            }
            None => println!("Last checkin: Never"),
        }

        match state.last_checkin_request {
            Some(request_time) => {
                let days_since = state.days_since_last_checkin_request().unwrap_or(0);
                println!("Last checkin request: {} ({} days ago)", request_time.format("%Y-%m-%d %H:%M:%S UTC"), days_since);
            }
            None => println!("Last checkin request: Never"),
        }

        match state.last_signal_fired {
            Some(signal_time) => {
                let days_since = state.days_since_last_signal_fired().unwrap_or(0);
                println!("Last signal fired: {} ({} days ago)", signal_time.format("%Y-%m-%d %H:%M:%S UTC"), days_since);
            }
            None => println!("Last signal fired: Never"),
        }

        println!("Checkin request count: {}", state.checkin_request_count);
        println!();
        
        println!("Configuration:");
        println!("  Duration between checkins: {}", self.config.checkin.duration_between_checkins);
        println!("  Output retry delay (checkin): {}", self.config.checkin.output_retry_delay);
        println!("  Max time since last checkin: {}", self.config.recipient.max_time_since_last_checkin);
        println!("  Output retry delay (last signal): {}", self.config.recipient.output_retry_delay);
        println!("  Checkin outputs: {}", self.checkin_outputs.len());
        println!("  Last signal outputs: {}", self.last_signal_outputs.len());
        
        println!();
        
        // Show what actions would be taken
        if self.state_manager.get_state().should_request_checkin(self.config.checkin.duration_between_checkins) {
            println!("⚠️  Checkin request would be sent if running");
        } else {
            println!("✅ Checkin is up to date");
        }

        if self.state_manager.get_state().should_fire_last_signal(self.config.recipient.max_time_since_last_checkin) 
            && !self.state_manager.get_state().has_fired_last_signal_recently(self.config.recipient.max_time_since_last_checkin) {
            println!("🚨 Last signal would be fired if running");
        } else {
            println!("✅ Last signal not needed");
        }

        Ok(())
    }

    pub async fn test_outputs(&self) -> Result<()> {
        println!("Testing checkin outputs...");
        for (i, output) in self.checkin_outputs.iter().enumerate() {
            print!("  {} ({}): ", i + 1, output.get_name());
            match output.health_check().await {
                Ok(true) => println!("✅ Healthy"),
                Ok(false) => println!("❌ Unhealthy"),
                Err(e) => println!("💥 Error: {}", e),
            }
        }

        println!("\nTesting last signal outputs...");
        for (i, output) in self.last_signal_outputs.iter().enumerate() {
            print!("  {} ({}): ", i + 1, output.get_name());
            match output.health_check().await {
                Ok(true) => println!("✅ Healthy"),
                Ok(false) => println!("❌ Unhealthy"),
                Err(e) => println!("💥 Error: {}", e),
            }
        }

        Ok(())
    }

    async fn send_message_via_bidirectional_outputs(&self, message: &str) -> Result<OutputResult> {
        if self.checkin_outputs.is_empty() {
            return Ok(OutputResult::Failed("No checkin outputs configured".to_string()));
        }

        for (i, output) in self.checkin_outputs.iter().enumerate() {
            tracing::info!("Attempting to send message via {}", output.get_name());
            
            let health_ok = match output.health_check().await {
                Ok(healthy) => {
                    if !healthy {
                        tracing::warn!("Health check failed for {}, skipping", output.get_name());
                        false
                    } else {
                        true
                    }
                }
                Err(e) => {
                    tracing::warn!("Health check error for {}: {}, skipping", output.get_name(), e);
                    false
                }
            };

            if !health_ok {
                continue;
            }

            match output.send_message(message).await {
                Ok(OutputResult::Success) => {
                    tracing::info!("Message sent successfully via {}", output.get_name());
                    return Ok(OutputResult::Success);
                }
                Ok(OutputResult::Failed(error)) => {
                    tracing::warn!("Failed to send message via {}: {}", output.get_name(), error);
                }
                Ok(OutputResult::Skipped(reason)) => {
                    tracing::info!("Message sending skipped via {}: {}", output.get_name(), reason);
                    return Ok(OutputResult::Skipped(reason));
                }
                Err(e) => {
                    tracing::error!("Error sending message via {}: {}", output.get_name(), e);
                }
            }

            if i < self.checkin_outputs.len() - 1 {
                tracing::info!("Trying next output immediately due to failure");
            }
        }

        Ok(OutputResult::Failed("All checkin outputs failed".to_string()))
    }

    async fn process_bidirectional_checkins(&mut self) -> Result<()> {
        tracing::debug!("Starting process_bidirectional_checkins");
        let state = self.state_manager.get_state();
        
        // Only check since the last successful checkin or checkin request
        let since = state.last_checkin.or(state.last_checkin_request);
        
        tracing::debug!("Checking for bidirectional responses since: {:?}", since);
        tracing::debug!("Number of checkin outputs: {}", self.checkin_outputs.len());
        
        match process_bidirectional_outputs_for_checkins(&self.checkin_outputs, since).await {
            Ok(responses) => {
                if !responses.is_empty() {
                    tracing::info!("Found {} potential checkin responses", responses.len());
                    
                    // Find the most recent response
                    let mut sorted_responses = responses;
                    sorted_responses.sort_by_key(|r| {
                        match r {
                            crate::outputs::bidirectional::CheckinResponse::Found { timestamp, .. } => *timestamp,
                            crate::outputs::bidirectional::CheckinResponse::None => chrono::Utc::now(),
                        }
                    });
                    
                    if let Some(latest_response) = sorted_responses.last() {
                        if let crate::outputs::bidirectional::CheckinResponse::Found { timestamp, subject, from } = latest_response {
                            tracing::info!("Processing checkin response from {} at {}: {}", from, timestamp, subject);
                            
                            // Record the checkin
                            self.state_manager.record_checkin()
                                .context("Failed to record checkin from bidirectional response")?;
                            
                            // Mark all responses as processed up to this timestamp
                            mark_all_processed_until(&self.checkin_outputs, *timestamp).await?;
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Error checking for bidirectional responses: {}", e);
            }
        }
        
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use std::io::Write;

    async fn create_test_app() -> Result<LastSignalApp> {
        let temp_dir = tempdir()?;
        let config_dir = temp_dir.path().join(".lastsignal");
        std::fs::create_dir_all(&config_dir)?;

        let config_content = r#"
[checkin]
duration_between_checkins = "7d"
output_retry_delay = "24h"

[[checkin.outputs]]
type = "email"
config = { to = "admin@example.com", smtp_host = "smtp.gmail.com", smtp_port = "587", username = "sender@example.com", password = "password" }

[recipient]
max_time_since_last_checkin = "14d"
output_retry_delay = "12h"

[[recipient.last_signal_outputs]]
type = "email"
config = { to = "recipient@example.com", smtp_host = "smtp.gmail.com", smtp_port = "587", username = "sender@example.com", password = "password" }

[last_signal]
adapter_type = "file"
message_file = "message.txt"

[app]
data_directory = "{}"
log_level = "info"
check_interval = "1h"
        "#;

        let config_path = config_dir.join("config.toml");
        std::fs::write(&config_path, config_content.replace("{}", &config_dir.to_string_lossy()))?;

        // Temporarily set the config path for testing
        unsafe { std::env::set_var("HOME", temp_dir.path()); }
        
        let app = LastSignalApp::new().await?;
        Ok(app)
    }

    #[tokio::test]
    async fn test_app_initialization() {
        let result = create_test_app().await;
        assert!(result.is_ok());
        
        let app = result.unwrap();
        assert_eq!(app.checkin_outputs.len(), 1);
        assert_eq!(app.last_signal_outputs.len(), 1);
    }
}
