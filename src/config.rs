use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub checkin: CheckinConfig,
    pub recipient: RecipientConfig,
    pub last_signal: LastSignalConfig,
    pub app: AppConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CheckinConfig {
    pub days_between_checkins: u32,
    pub output_retry_delay_hours: u32,
    pub outputs: Vec<OutputConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RecipientConfig {
    pub days_before_last_signal: u32,
    pub output_retry_delay_hours: u32,
    pub last_signal_outputs: Vec<OutputConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OutputConfig {
    #[serde(rename = "type")]
    pub output_type: String,
    pub config: HashMap<String, String>,
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
        if self.checkin.days_between_checkins == 0 {
            anyhow::bail!("days_between_checkins must be greater than 0");
        }

        if self.recipient.days_before_last_signal == 0 {
            anyhow::bail!("days_before_last_signal must be greater than 0");
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
data_directory = "~/.lastsignal/"
log_level = "info"
        "#;

        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(config_content.as_bytes()).unwrap();
        
        let config = Config::load_from_path(temp_file.path()).unwrap();
        assert_eq!(config.checkin.days_between_checkins, 7);
        assert_eq!(config.recipient.days_before_last_signal, 14);
    }
}