use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::duration_parser::ConfigDuration;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub checkin: CheckinConfig,
    pub recipient: RecipientConfig,
    pub last_signal: LastSignalConfig,
    pub app: AppConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CheckinConfig {
    pub duration_between_checkins: ConfigDuration,
    pub output_retry_delay: ConfigDuration,
    pub outputs: Vec<OutputConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RecipientConfig {
    pub duration_before_last_signal: ConfigDuration,
    pub output_retry_delay: ConfigDuration,
    pub last_signal_outputs: Vec<OutputConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OutputConfig {
    #[serde(rename = "type")]
    pub output_type: String,
    pub config: HashMap<String, String>,
    #[serde(default = "default_false")]
    pub bidirectional: bool,
}

fn default_false() -> bool {
    false
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LastSignalConfig {
    pub adapter_type: String,
    pub message_file: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AppConfig {
    pub data_directory: String,
    pub log_level: String,
    #[serde(default = "default_check_interval")]
    pub check_interval: ConfigDuration,
}

fn default_check_interval() -> ConfigDuration {
    ConfigDuration::from_hours(1)
}

impl Config {
    pub fn load() -> Result<Self> {
        let config_path = Self::get_config_path()?;
        Self::load_from_path(&config_path)
    }

    pub fn load_from_path<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = std::fs::read_to_string(path.as_ref())
            .with_context(|| format!("Failed to read config file: {:?}", path.as_ref()))?;
        
        let config: Config = toml::from_str(&content)
            .with_context(|| "Failed to parse config file as TOML")?;
        
        config.validate()?;
        Ok(config)
    }

    pub fn get_config_path() -> Result<PathBuf> {
        let home_dir = dirs::home_dir()
            .context("Could not determine home directory")?;
        
        Ok(home_dir.join(".lastsignal").join("config.toml"))
    }

    pub fn get_data_directory(&self) -> Result<PathBuf> {
        let data_dir = if self.app.data_directory.starts_with('~') {
            let home_dir = dirs::home_dir()
                .context("Could not determine home directory")?;
            home_dir.join(self.app.data_directory.strip_prefix("~/").unwrap_or(&self.app.data_directory))
        } else {
            PathBuf::from(&self.app.data_directory)
        };

        if !data_dir.exists() {
            std::fs::create_dir_all(&data_dir)
                .with_context(|| format!("Failed to create data directory: {:?}", data_dir))?;
        }

        Ok(data_dir)
    }

    pub fn get_message_file_path(&self) -> Result<PathBuf> {
        let message_file = if self.last_signal.message_file.starts_with('/') {
            PathBuf::from(&self.last_signal.message_file)
        } else if self.last_signal.message_file.starts_with('~') {
            let home_dir = dirs::home_dir()
                .context("Could not determine home directory")?;
            home_dir.join(self.last_signal.message_file.strip_prefix("~/").unwrap_or(&self.last_signal.message_file))
        } else {
            self.get_data_directory()?.join(&self.last_signal.message_file)
        };

        Ok(message_file)
    }

    fn validate(&self) -> Result<()> {
        if self.checkin.duration_between_checkins.as_secs() == 0 {
            anyhow::bail!("duration_between_checkins must be greater than 0");
        }

        if self.recipient.duration_before_last_signal.as_secs() == 0 {
            anyhow::bail!("duration_before_last_signal must be greater than 0");
        }

        if self.checkin.output_retry_delay.as_secs() == 0 {
            anyhow::bail!("checkin output_retry_delay must be greater than 0");
        }

        if self.recipient.output_retry_delay.as_secs() == 0 {
            anyhow::bail!("recipient output_retry_delay must be greater than 0");
        }

        if self.app.check_interval.as_secs() == 0 {
            anyhow::bail!("app check_interval must be greater than 0");
        }

        if self.checkin.outputs.is_empty() {
            anyhow::bail!("At least one checkin output must be configured");
        }

        if self.recipient.last_signal_outputs.is_empty() {
            anyhow::bail!("At least one last signal output must be configured");
        }

        for output in &self.checkin.outputs {
            self.validate_output(output, "checkin")?;
        }

        for output in &self.recipient.last_signal_outputs {
            self.validate_output(output, "last_signal")?;
        }

        let valid_log_levels = ["trace", "debug", "info", "warn", "error"];
        if !valid_log_levels.contains(&self.app.log_level.as_str()) {
            anyhow::bail!("Invalid log level: {}. Must be one of: {}", 
                self.app.log_level, valid_log_levels.join(", "));
        }

        Ok(())
    }

    fn validate_output(&self, output: &OutputConfig, context: &str) -> Result<()> {
        match output.output_type.as_str() {
            "facebook_messenger" => {
                if !output.config.contains_key("user_id") {
                    anyhow::bail!("facebook_messenger output in {} missing 'user_id'", context);
                }
                if !output.config.contains_key("access_token") {
                    anyhow::bail!("facebook_messenger output in {} missing 'access_token'", context);
                }
            }
            "email" => {
                let required_fields = ["to", "smtp_host", "smtp_port", "username", "password"];
                for field in &required_fields {
                    if !output.config.contains_key(*field) {
                        anyhow::bail!("email output in {} missing '{}'", context, field);
                    }
                }
                
                if let Some(port_str) = output.config.get("smtp_port") {
                    port_str.parse::<u16>()
                        .with_context(|| format!("Invalid SMTP port '{}' in {} output", port_str, context))?;
                }

                // Validate IMAP settings for bidirectional email
                if output.bidirectional {
                    if let Some(imap_port_str) = output.config.get("imap_port") {
                        imap_port_str.parse::<u16>()
                            .with_context(|| format!("Invalid IMAP port '{}' in {} output", imap_port_str, context))?;
                    }
                }
            }
            _ => {
                anyhow::bail!("Unknown output type '{}' in {}", output.output_type, context);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;
    use std::io::Write;

    #[test]
    fn test_config_validation() {
        let config_content = r#"
[checkin]
duration_between_checkins = "7d"
output_retry_delay = "24h"

[[checkin.outputs]]
type = "email"
config = { to = "admin@example.com", smtp_host = "smtp.gmail.com", smtp_port = "587", username = "sender@example.com", password = "password" }

[recipient]
duration_before_last_signal = "14d"
output_retry_delay = "12h"

[[recipient.last_signal_outputs]]
type = "email"
config = { to = "recipient@example.com", smtp_host = "smtp.gmail.com", smtp_port = "587", username = "sender@example.com", password = "password" }

[last_signal]
adapter_type = "file"
message_file = "message.txt"

[app]
data_directory = "~/.lastsignal/"
log_level = "info"
check_interval = "1h"
        "#;

        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(config_content.as_bytes()).unwrap();
        
        let config = Config::load_from_path(temp_file.path()).unwrap();
        assert_eq!(config.checkin.duration_between_checkins.as_days(), 7);
        assert_eq!(config.recipient.duration_before_last_signal.as_days(), 14);
        assert_eq!(config.app.check_interval.as_hours(), 1);
    }

    #[test]
    fn test_config_duration_formats() {
        // Test various valid formats
        let config_content = r#"
[checkin]
duration_between_checkins = "168hours"
output_retry_delay = "30minutes"

[[checkin.outputs]]
type = "email"
config = { to = "admin@example.com", smtp_host = "smtp.gmail.com", smtp_port = "587", username = "sender@example.com", password = "password" }

[recipient]
duration_before_last_signal = "336h"
output_retry_delay = "12hours"

[[recipient.last_signal_outputs]]
type = "email"
config = { to = "recipient@example.com", smtp_host = "smtp.gmail.com", smtp_port = "587", username = "sender@example.com", password = "password" }

[last_signal]
adapter_type = "file"
message_file = "message.txt"

[app]
data_directory = "~/.lastsignal/"
log_level = "info"
check_interval = "30m"
        "#;

        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(config_content.as_bytes()).unwrap();
        
        let config = Config::load_from_path(temp_file.path()).unwrap();
        // 168 hours = 7 days
        assert_eq!(config.checkin.duration_between_checkins.as_hours(), 168);
        assert_eq!(config.checkin.duration_between_checkins.as_days(), 7);
        // 30 minutes
        assert_eq!(config.checkin.output_retry_delay.as_minutes(), 30);
        // 336 hours = 14 days  
        assert_eq!(config.recipient.duration_before_last_signal.as_hours(), 336);
        assert_eq!(config.recipient.duration_before_last_signal.as_days(), 14);
        // 30 minutes
        assert_eq!(config.app.check_interval.as_minutes(), 30);
    }

    #[test]
    fn test_config_rejects_pure_numbers() {
        // Test that config rejects pure numbers and requires explicit units
        let config_content = r#"
[checkin]
duration_between_checkins = 604800
output_retry_delay = 86400

[[checkin.outputs]]
type = "email"
config = { to = "admin@example.com", smtp_host = "smtp.gmail.com", smtp_port = "587", username = "sender@example.com", password = "password" }

[recipient]
duration_before_last_signal = 1209600
output_retry_delay = 43200

[[recipient.last_signal_outputs]]
type = "email"
config = { to = "recipient@example.com", smtp_host = "smtp.gmail.com", smtp_port = "587", username = "sender@example.com", password = "password" }

[last_signal]
adapter_type = "file"
message_file = "message.txt"

[app]
data_directory = "~/.lastsignal/"
log_level = "info"
check_interval = 3600
        "#;

        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(config_content.as_bytes()).unwrap();
        
        // This should fail because pure numbers are not allowed
        let result = Config::load_from_path(temp_file.path());
        assert!(result.is_err());
        
        // Test the corrected version with explicit units
        let config_content_fixed = r#"
[checkin]
duration_between_checkins = "604800s"
output_retry_delay = "86400s"

[[checkin.outputs]]
type = "email"
config = { to = "admin@example.com", smtp_host = "smtp.gmail.com", smtp_port = "587", username = "sender@example.com", password = "password" }

[recipient]
duration_before_last_signal = "1209600s"
output_retry_delay = "43200s"

[[recipient.last_signal_outputs]]
type = "email"
config = { to = "recipient@example.com", smtp_host = "smtp.gmail.com", smtp_port = "587", username = "sender@example.com", password = "password" }

[last_signal]
adapter_type = "file"
message_file = "message.txt"

[app]
data_directory = "~/.lastsignal/"
log_level = "info"
check_interval = "3600s"
        "#;

        let mut temp_file_fixed = NamedTempFile::new().unwrap();
        temp_file_fixed.write_all(config_content_fixed.as_bytes()).unwrap();
        
        let config = Config::load_from_path(temp_file_fixed.path()).unwrap();
        // 604800 seconds = 7 days
        assert_eq!(config.checkin.duration_between_checkins.as_days(), 7);
        // 86400 seconds = 1 day  
        assert_eq!(config.checkin.output_retry_delay.as_hours(), 24);
        // 1209600 seconds = 14 days
        assert_eq!(config.recipient.duration_before_last_signal.as_days(), 14);
        // 43200 seconds = 12 hours
        assert_eq!(config.recipient.output_retry_delay.as_hours(), 12);
        // 3600 seconds = 1 hour
        assert_eq!(config.app.check_interval.as_hours(), 1);
    }
}