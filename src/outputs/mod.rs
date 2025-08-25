use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use crate::config::OutputConfig;
use crate::state::StateManager;

pub mod email;
pub mod email_bidirectional;
pub mod facebook_messenger;
pub mod whoop;
pub mod bidirectional;

#[derive(Debug, Clone)]
pub enum OutputResult {
    Success,
    Failed(String),
    Skipped(String),
}

impl OutputResult {
    pub fn is_success(&self) -> bool {
        matches!(self, OutputResult::Success)
    }

    pub fn error_message(&self) -> Option<&str> {
        match self {
            OutputResult::Success => None,
            OutputResult::Failed(msg) => Some(msg),
            OutputResult::Skipped(msg) => Some(msg),
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
        data_directory: Option<&std::path::Path>,
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
            "whoop" => {
                let data_dir = data_directory
                    .ok_or_else(|| anyhow::anyhow!("Data directory required for WHOOP output"))?
                    .to_path_buf();
                let output = whoop::WhoopOutput::new(config, data_dir)?;
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
            Ok(OutputResult::Skipped(reason)) => {
                tracing::info!("Message sending skipped via {}: {}", output.get_name(), reason);
                return Ok(OutputResult::Skipped(reason));
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

/// Processes all outputs, sending the message to every configured recipient.
/// Unlike process_outputs_with_fallback, this continues after the first success
/// to ensure all recipients receive the message (used for emergency last signals).
pub async fn process_outputs_to_all(
    outputs: &[Box<dyn Output>],
    message: &str,
) -> Result<Vec<(String, OutputResult)>> {
    if outputs.is_empty() {
        return Ok(vec![]);
    }

    let mut results = Vec::new();
    
    for output in outputs.iter() {
        let output_name = output.get_name().to_string();
        tracing::info!("Attempting to send message via {}", output_name);
        
        // Check health first
        let health_ok = match output.health_check().await {
            Ok(healthy) => {
                if !healthy {
                    tracing::warn!("Health check failed for {}, skipping", output_name);
                    false
                } else {
                    true
                }
            }
            Err(e) => {
                tracing::warn!("Health check error for {}: {}, skipping", output_name, e);
                false
            }
        };

        let result = if !health_ok {
            OutputResult::Skipped("Health check failed".to_string())
        } else {
            match output.send_message(message).await {
                Ok(result) => {
                    match &result {
                        OutputResult::Success => {
                            tracing::info!("Message sent successfully via {}", output_name);
                        }
                        OutputResult::Failed(error) => {
                            tracing::warn!("Failed to send message via {}: {}", output_name, error);
                        }
                        OutputResult::Skipped(reason) => {
                            tracing::info!("Message sending skipped via {}: {}", output_name, reason);
                        }
                    }
                    result
                }
                Err(e) => {
                    let error_msg = format!("Error sending message: {}", e);
                    tracing::error!("Error sending message via {}: {}", output_name, e);
                    OutputResult::Failed(error_msg)
                }
            }
        };
        
        results.push((output_name, result));
    }

    Ok(results)
}

/// Generates a unique identifier for an output recipient based on type and config.
/// This is used to track which recipients have already been successfully notified.
pub fn generate_recipient_id(output_config: &OutputConfig) -> String {
    match output_config.output_type.as_str() {
        "email" => {
            if let Some(to) = output_config.config.get("to") {
                format!("email:{}", to)
            } else {
                format!("email:unknown")
            }
        }
        "facebook_messenger" => {
            if let Some(user_id) = output_config.config.get("user_id") {
                format!("facebook_messenger:{}", user_id)
            } else {
                format!("facebook_messenger:unknown")
            }
        }
        "whoop" => {
            // WHOOP doesn't send messages, but include for completeness
            "whoop:device".to_string()
        }
        _ => format!("{}:unknown", output_config.output_type)
    }
}

/// Processes last signal outputs with recipient tracking to prevent duplicate notifications.
/// Only sends to recipients who haven't already been successfully notified.
pub async fn process_last_signal_outputs(
    output_configs: &[OutputConfig],
    outputs: &[Box<dyn Output>],
    message: &str,
    state_manager: &mut StateManager,
) -> Result<Vec<(String, String, OutputResult)>> {
    if outputs.is_empty() {
        return Ok(vec![]);
    }

    let mut results = Vec::new();
    
    for (i, (output_config, output)) in output_configs.iter().zip(outputs.iter()).enumerate() {
        let recipient_id = generate_recipient_id(output_config);
        let output_name = output.get_name().to_string();
        
        // Skip if already notified
        if state_manager.get_state().is_last_signal_recipient_already_notified(&recipient_id) {
            tracing::info!("Skipping {} - recipient {} already notified", output_name, recipient_id);
            results.push((
                output_name,
                recipient_id,
                OutputResult::Skipped("Recipient already notified".to_string())
            ));
            continue;
        }
        
        tracing::info!("Attempting to send last signal via {} to {}", output_name, recipient_id);
        
        // Check health first
        let health_ok = match output.health_check().await {
            Ok(healthy) => {
                if !healthy {
                    tracing::warn!("Health check failed for {}, skipping", output_name);
                    false
                } else {
                    true
                }
            }
            Err(e) => {
                tracing::warn!("Health check error for {}: {}, skipping", output_name, e);
                false
            }
        };

        let result = if !health_ok {
            OutputResult::Skipped("Health check failed".to_string())
        } else {
            match output.send_message(message).await {
                Ok(result) => {
                    match &result {
                        OutputResult::Success => {
                            tracing::info!("Last signal sent successfully via {} to {}", output_name, recipient_id);
                            // Record successful notification
                            if let Err(e) = state_manager.record_last_signal_recipient_notified(&recipient_id) {
                                tracing::error!("Failed to record recipient notification: {}", e);
                            }
                        }
                        OutputResult::Failed(error) => {
                            tracing::warn!("Failed to send last signal via {} to {}: {}", output_name, recipient_id, error);
                        }
                        OutputResult::Skipped(reason) => {
                            tracing::info!("Last signal sending skipped via {} to {}: {}", output_name, recipient_id, reason);
                        }
                    }
                    result
                }
                Err(e) => {
                    let error_msg = format!("Error sending last signal: {}", e);
                    tracing::error!("Error sending last signal via {} to {}: {}", output_name, recipient_id, e);
                    OutputResult::Failed(error_msg)
                }
            }
        };
        
        results.push((output_name, recipient_id, result));
    }

    Ok(results)
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

    #[tokio::test]
    async fn test_process_outputs_to_all_sends_to_all_recipients() {
        let outputs: Vec<Box<dyn Output>> = vec![
            Box::new(MockOutput {
                name: "Output1".to_string(),
                should_succeed: true,
                health_check_result: true,
            }),
            Box::new(MockOutput {
                name: "Output2".to_string(),
                should_succeed: true,
                health_check_result: true,
            }),
            Box::new(MockOutput {
                name: "Output3".to_string(),
                should_succeed: false,
                health_check_result: true,
            }),
        ];

        let results = process_outputs_to_all(&outputs, "test message").await.unwrap();
        
        assert_eq!(results.len(), 3);
        
        // Check each result
        assert_eq!(results[0].0, "Output1");
        assert!(matches!(results[0].1, OutputResult::Success));
        
        assert_eq!(results[1].0, "Output2");
        assert!(matches!(results[1].1, OutputResult::Success));
        
        assert_eq!(results[2].0, "Output3");
        assert!(matches!(results[2].1, OutputResult::Failed(_)));
    }

    #[tokio::test]
    async fn test_process_outputs_to_all_handles_health_check_failures() {
        let outputs: Vec<Box<dyn Output>> = vec![
            Box::new(MockOutput {
                name: "HealthyOutput".to_string(),
                should_succeed: true,
                health_check_result: true,
            }),
            Box::new(MockOutput {
                name: "UnhealthyOutput".to_string(),
                should_succeed: true,
                health_check_result: false,
            }),
        ];

        let results = process_outputs_to_all(&outputs, "test message").await.unwrap();
        
        assert_eq!(results.len(), 2);
        assert!(matches!(results[0].1, OutputResult::Success));
        assert!(matches!(results[1].1, OutputResult::Skipped(_)));
    }
}