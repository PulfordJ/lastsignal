use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;

pub mod email;
pub mod email_bidirectional;
pub mod facebook_messenger;
pub mod bidirectional;

#[derive(Debug, Clone)]
pub enum OutputResult {
    Success,
    Failed(String),
}

impl OutputResult {
    pub fn is_success(&self) -> bool {
        matches!(self, OutputResult::Success)
    }

    pub fn error_message(&self) -> Option<&str> {
        match self {
            OutputResult::Success => None,
            OutputResult::Failed(msg) => Some(msg),
        }
    }
}

#[async_trait]
pub trait Output: Send + Sync {
    async fn send_message(&self, message: &str) -> Result<OutputResult>;
    async fn health_check(&self) -> Result<bool>;
    fn get_name(&self) -> &str;
}

pub struct OutputFactory;

impl OutputFactory {
    pub fn create_output(
        output_type: &str,
        config: &HashMap<String, String>,
    ) -> Result<Box<dyn Output>> {
        match output_type {
            "email" => {
                let output = email::EmailOutput::new(config)?;
                Ok(Box::new(output))
            }
            "facebook_messenger" => {
                let output = facebook_messenger::FacebookMessengerOutput::new(config)?;
                Ok(Box::new(output))
            }
            _ => anyhow::bail!("Unknown output type: {}", output_type),
        }
    }
}

pub async fn process_outputs_with_fallback(
    outputs: &[Box<dyn Output>],
    message: &str,
    _retry_delay_hours: u32,
) -> Result<OutputResult> {
    if outputs.is_empty() {
        return Ok(OutputResult::Failed("No outputs configured".to_string()));
    }

    for (i, output) in outputs.iter().enumerate() {
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
            Err(e) => {
                tracing::error!("Error sending message via {}: {}", output.get_name(), e);
            }
        }

        if i < outputs.len() - 1 {
            tracing::info!("Trying next output immediately due to failure");
        }
    }

    Ok(OutputResult::Failed("All outputs failed".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockOutput {
        name: String,
        should_succeed: bool,
        health_check_result: bool,
    }

    impl MockOutput {
        fn new(name: &str, should_succeed: bool, health_check_result: bool) -> Self {
            Self {
                name: name.to_string(),
                should_succeed,
                health_check_result,
            }
        }
    }

    #[async_trait]
    impl Output for MockOutput {
        async fn send_message(&self, _message: &str) -> Result<OutputResult> {
            if self.should_succeed {
                Ok(OutputResult::Success)
            } else {
                Ok(OutputResult::Failed("Mock failure".to_string()))
            }
        }

        async fn health_check(&self) -> Result<bool> {
            Ok(self.health_check_result)
        }

        fn get_name(&self) -> &str {
            &self.name
        }
    }

    #[tokio::test]
    async fn test_process_outputs_success_on_first() {
        let outputs: Vec<Box<dyn Output>> = vec![
            Box::new(MockOutput::new("first", true, true)),
            Box::new(MockOutput::new("second", false, true)),
        ];

        let result = process_outputs_with_fallback(&outputs, "test message", 1).await.unwrap();
        assert!(result.is_success());
    }

    #[tokio::test]
    async fn test_process_outputs_fallback_to_second() {
        let outputs: Vec<Box<dyn Output>> = vec![
            Box::new(MockOutput::new("first", false, true)),
            Box::new(MockOutput::new("second", true, true)),
        ];

        let result = process_outputs_with_fallback(&outputs, "test message", 1).await.unwrap();
        assert!(result.is_success());
    }

    #[tokio::test]
    async fn test_process_outputs_skip_unhealthy() {
        let outputs: Vec<Box<dyn Output>> = vec![
            Box::new(MockOutput::new("unhealthy", true, false)),
            Box::new(MockOutput::new("healthy", true, true)),
        ];

        let result = process_outputs_with_fallback(&outputs, "test message", 1).await.unwrap();
        assert!(result.is_success());
    }

    #[tokio::test]
    async fn test_process_outputs_all_fail() {
        let outputs: Vec<Box<dyn Output>> = vec![
            Box::new(MockOutput::new("first", false, true)),
            Box::new(MockOutput::new("second", false, true)),
        ];

        let result = process_outputs_with_fallback(&outputs, "test message", 1).await.unwrap();
        assert!(!result.is_success());
        assert!(result.error_message().unwrap().contains("All outputs failed"));
    }
}