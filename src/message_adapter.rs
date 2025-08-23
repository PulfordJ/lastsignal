use anyhow::{Context, Result};
use chrono::Utc;
use std::path::Path;

pub trait MessageAdapter: Send + Sync {
    fn generate_checkin_message(&self) -> Result<String>;
    fn generate_last_signal_message(&self) -> Result<String>;
}

pub struct FileMessageAdapter {
    message_file_path: std::path::PathBuf,
}

impl FileMessageAdapter {
    pub fn new<P: AsRef<Path>>(message_file_path: P) -> Self {
        Self {
            message_file_path: message_file_path.as_ref().to_path_buf(),
        }
    }

    fn load_message_from_file(&self) -> Result<String> {
        if !self.message_file_path.exists() {
            let default_message = self.get_default_message();
            
            if let Some(parent) = self.message_file_path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("Failed to create directory for message file: {:?}", parent))?;
            }
            
            std::fs::write(&self.message_file_path, &default_message)
                .with_context(|| format!("Failed to create default message file: {:?}", self.message_file_path))?;
            
            tracing::info!("Created default message file at: {:?}", self.message_file_path);
            return Ok(default_message);
        }

        let content = std::fs::read_to_string(&self.message_file_path)
            .with_context(|| format!("Failed to read message file: {:?}", self.message_file_path))?;
        
        Ok(content.trim().to_string())
    }

    fn get_default_message(&self) -> String {
        r#"This is an automated message from LastSignal.

I have not received a check-in from my designated contact within the expected timeframe. 
This message is being sent as a precautionary measure to ensure my wellbeing.

If you are receiving this message, please:
1. Try to contact me through normal means
2. If you cannot reach me, consider checking on me in person
3. Contact emergency services if necessary

This system was set up to ensure my safety and peace of mind.

Generated at: {timestamp}

LastSignal - Automated Safety System"#.to_string()
    }
}

impl MessageAdapter for FileMessageAdapter {
    fn generate_checkin_message(&self) -> Result<String> {
        let base_message = "Hello! This is your scheduled check-in reminder from LastSignal.\n\nPlease respond to confirm you're okay. If you don't respond within the configured timeframe, the emergency contacts will be notified.\n\nTo check in, you can reply to this message or use any of the configured response methods.";
        Ok(base_message.to_string())
    }

    fn generate_last_signal_message(&self) -> Result<String> {
        let template = self.load_message_from_file()?;
        let timestamp = Utc::now().format("%Y-%m-%d %H:%M:%S UTC");
        
        let message = template.replace("{timestamp}", &timestamp.to_string());
        
        Ok(message)
    }
}

pub struct MessageAdapterFactory;

impl MessageAdapterFactory {
    pub fn create_adapter(
        adapter_type: &str,
        message_file_path: &Path,
    ) -> Result<Box<dyn MessageAdapter>> {
        match adapter_type {
            "file" => {
                let adapter = FileMessageAdapter::new(message_file_path);
                Ok(Box::new(adapter))
            }
            _ => anyhow::bail!("Unknown message adapter type: {}", adapter_type),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::{tempdir, NamedTempFile};
    use std::io::Write;

    #[test]
    fn test_file_message_adapter_default_message() {
        let temp_dir = tempdir().unwrap();
        let message_path = temp_dir.path().join("message.txt");
        
        let adapter = FileMessageAdapter::new(&message_path);
        let message = adapter.generate_last_signal_message().unwrap();
        
        assert!(message.contains("LastSignal"));
        assert!(message.contains("{timestamp}") == false); // Should be replaced
        assert!(std::fs::exists(&message_path).unwrap());
    }

    #[test]
    fn test_file_message_adapter_existing_file() {
        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(b"Custom message with {timestamp}").unwrap();
        
        let adapter = FileMessageAdapter::new(temp_file.path());
        let message = adapter.generate_last_signal_message().unwrap();
        
        assert!(message.contains("Custom message"));
        assert!(message.contains("{timestamp}") == false); // Should be replaced with actual timestamp
    }

    #[test]
    fn test_file_message_adapter_checkin_message() {
        let temp_dir = tempdir().unwrap();
        let message_path = temp_dir.path().join("message.txt");
        
        let adapter = FileMessageAdapter::new(&message_path);
        let message = adapter.generate_checkin_message().unwrap();
        
        assert!(message.contains("check-in reminder"));
        assert!(message.contains("LastSignal"));
    }

    #[test]
    fn test_message_adapter_factory() {
        let temp_dir = tempdir().unwrap();
        let message_path = temp_dir.path().join("message.txt");
        
        let adapter = MessageAdapterFactory::create_adapter("file", &message_path).unwrap();
        let message = adapter.generate_checkin_message().unwrap();
        
        assert!(message.contains("check-in reminder"));
    }

    #[test]
    fn test_message_adapter_factory_unknown_type() {
        let temp_dir = tempdir().unwrap();
        let message_path = temp_dir.path().join("message.txt");
        
        let result = MessageAdapterFactory::create_adapter("unknown", &message_path);
        assert!(result.is_err());
    }

    #[test]
    fn test_timestamp_replacement() {
        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(b"Message sent at: {timestamp}").unwrap();
        
        let adapter = FileMessageAdapter::new(temp_file.path());
        let message = adapter.generate_last_signal_message().unwrap();
        
        assert!(message.contains("Message sent at: "));
        assert!(message.contains("UTC"));
        assert!(!message.contains("{timestamp}"));
    }
}