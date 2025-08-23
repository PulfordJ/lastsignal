use anyhow::{Context, Result};
use tokio::time::{sleep, Duration};

use crate::config::Config;
use crate::message_adapter::{MessageAdapter, MessageAdapterFactory};
use crate::outputs::{process_outputs_with_fallback, Output, OutputFactory, OutputResult};
use crate::state::StateManager;

pub struct LastSignalApp {
    config: Config,
    state_manager: StateManager,
    message_adapter: Box<dyn MessageAdapter>,
    checkin_outputs: Vec<Box<dyn Output>>,
    last_signal_outputs: Vec<Box<dyn Output>>,
}

impl LastSignalApp {
    pub async fn new() -> Result<Self> {
        let config = Config::load()
            .context("Failed to load configuration. Make sure config.toml exists in ~/.lastsignal/")?;

        let data_directory = config.get_data_directory()
            .context("Failed to determine data directory")?;

        let state_manager = StateManager::new(&data_directory)
            .context("Failed to initialize state manager")?;

        let message_file_path = config.get_message_file_path()
            .context("Failed to determine message file path")?;

        let message_adapter = MessageAdapterFactory::create_adapter(
            &config.last_signal.adapter_type,
            &message_file_path,
        ).context("Failed to create message adapter")?;

        let mut checkin_outputs: Vec<Box<dyn Output>> = Vec::new();
        for output_config in &config.checkin.outputs {
            let output = OutputFactory::create_output(&output_config.output_type, &output_config.config)
                .with_context(|| format!("Failed to create checkin output: {}", output_config.output_type))?;
            checkin_outputs.push(output);
        }

        let mut last_signal_outputs: Vec<Box<dyn Output>> = Vec::new();
        for output_config in &config.recipient.last_signal_outputs {
            let output = OutputFactory::create_output(&output_config.output_type, &output_config.config)
                .with_context(|| format!("Failed to create last signal output: {}", output_config.output_type))?;
            last_signal_outputs.push(output);
        }

        Ok(LastSignalApp {
            config,
            state_manager,
            message_adapter,
            checkin_outputs,
            last_signal_outputs,
        })
    }

    pub async fn run(&mut self) -> Result<()> {
        tracing::info!("Starting LastSignal application");
        tracing::info!("Configuration loaded: {} checkin outputs, {} last signal outputs", 
            self.checkin_outputs.len(), 
            self.last_signal_outputs.len());

        loop {
            if let Err(e) = self.run_cycle().await {
                tracing::error!("Error in application cycle: {}", e);
                sleep(Duration::from_secs(300)).await; // Wait 5 minutes before retrying
                continue;
            }

            // Sleep for 1 hour before next check
            tracing::debug!("Cycle complete, sleeping for 1 hour");
            sleep(Duration::from_secs(3600)).await; // 1 hour
        }
    }

    async fn run_cycle(&mut self) -> Result<()> {
        tracing::debug!("Running application cycle");

        // Check if we need to request a checkin
        if self.should_request_checkin().await? {
            tracing::info!("Time to request checkin");
            self.request_checkin().await?;
        }

        // Check if we need to fire the last signal
        if self.should_fire_last_signal().await? {
            tracing::warn!("Time to fire last signal");
            self.fire_last_signal().await?;
        }

        Ok(())
    }

    async fn should_request_checkin(&self) -> Result<bool> {
        let state = self.state_manager.get_state();
        Ok(state.should_request_checkin(self.config.checkin.days_between_checkins))
    }

    async fn should_fire_last_signal(&self) -> Result<bool> {
        let state = self.state_manager.get_state();
        
        // Don't fire if we've already fired recently
        if state.has_fired_last_signal_recently(self.config.recipient.days_before_last_signal) {
            return Ok(false);
        }

        Ok(state.should_fire_last_signal(self.config.recipient.days_before_last_signal))
    }

    async fn request_checkin(&mut self) -> Result<()> {
        tracing::info!("Requesting checkin from admin");

        let message = self.message_adapter.generate_checkin_message()
            .context("Failed to generate checkin message")?;

        let result = process_outputs_with_fallback(
            &self.checkin_outputs,
            &message,
            self.config.checkin.output_retry_delay_hours,
        ).await?;

        match result {
            OutputResult::Success => {
                tracing::info!("Checkin request sent successfully");
                self.state_manager.record_checkin_request()
                    .context("Failed to record checkin request")?;
            }
            OutputResult::Failed(error) => {
                tracing::error!("Failed to send checkin request: {}", error);
                anyhow::bail!("All checkin outputs failed: {}", error);
            }
        }

        Ok(())
    }

    async fn fire_last_signal(&mut self) -> Result<()> {
        tracing::warn!("Firing last signal to recipients");

        let message = self.message_adapter.generate_last_signal_message()
            .context("Failed to generate last signal message")?;

        let result = process_outputs_with_fallback(
            &self.last_signal_outputs,
            &message,
            self.config.recipient.output_retry_delay_hours,
        ).await?;

        match result {
            OutputResult::Success => {
                tracing::warn!("Last signal sent successfully");
                self.state_manager.record_last_signal_fired()
                    .context("Failed to record last signal fired")?;
            }
            OutputResult::Failed(error) => {
                tracing::error!("Failed to send last signal: {}", error);
                anyhow::bail!("All last signal outputs failed: {}", error);
            }
        }

        Ok(())
    }

    pub async fn checkin(&mut self) -> Result<()> {
        tracing::info!("Recording manual checkin");
        self.state_manager.record_checkin()
            .context("Failed to record checkin")?;
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
        println!("  Days between checkins: {}", self.config.checkin.days_between_checkins);
        println!("  Days before last signal: {}", self.config.recipient.days_before_last_signal);
        println!("  Checkin outputs: {}", self.checkin_outputs.len());
        println!("  Last signal outputs: {}", self.last_signal_outputs.len());
        
        println!();
        
        // Show what actions would be taken
        if self.state_manager.get_state().should_request_checkin(self.config.checkin.days_between_checkins) {
            println!("⚠️  Checkin request would be sent if running");
        } else {
            println!("✅ Checkin is up to date");
        }

        if self.state_manager.get_state().should_fire_last_signal(self.config.recipient.days_before_last_signal) 
            && !self.state_manager.get_state().has_fired_last_signal_recently(self.config.recipient.days_before_last_signal) {
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
days_between_checkins = 7
output_retry_delay_hours = 24

[[checkin.outputs]]
type = "email"
config = { to = "admin@example.com", smtp_host = "smtp.gmail.com", smtp_port = "587", username = "sender@example.com", password = "password" }

[recipient]
days_before_last_signal = 14
output_retry_delay_hours = 12

[[recipient.last_signal_outputs]]
type = "email"
config = { to = "recipient@example.com", smtp_host = "smtp.gmail.com", smtp_port = "587", username = "sender@example.com", password = "password" }

[last_signal]
adapter_type = "file"
message_file = "message.txt"

[app]
data_directory = "{}"
log_level = "info"
        "#;

        let config_path = config_dir.join("config.toml");
        std::fs::write(&config_path, config_content.replace("{}", &config_dir.to_string_lossy()))?;

        // Temporarily set the config path for testing
        std::env::set_var("HOME", temp_dir.path());
        
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