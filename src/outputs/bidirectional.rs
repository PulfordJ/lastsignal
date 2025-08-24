use super::{Output, OutputResult};
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};

/// Represents the result of checking for incoming responses
#[derive(Debug, Clone)]
pub enum CheckinResponse {
    /// No new responses found
    None,
    /// Found a valid checkin response
    Found {
        /// Timestamp when the response was received
        timestamp: DateTime<Utc>,
        /// Subject of the response message
        subject: String,
        /// Sender of the response
        from: String,
    },
}

/// Trait for outputs that can both send messages and receive responses
/// This extends the basic Output functionality with bidirectional communication
#[async_trait]
pub trait BidirectionalOutput: Send + Sync {
    /// Send a message (delegated to underlying Output)
    async fn send_message(&self, message: &str) -> Result<OutputResult>;
    
    /// Health check (delegated to underlying Output)
    async fn health_check(&self) -> Result<bool>;
    
    /// Get the name of this output
    fn get_name(&self) -> &str;
    
    /// Check for new responses/replies that could count as check-ins
    /// This should only return responses received after the last check
    async fn check_for_responses(&self, since: Option<DateTime<Utc>>) -> Result<Vec<CheckinResponse>>;
    
    /// Mark responses as processed up to the given timestamp
    /// This prevents re-processing the same responses
    async fn mark_processed_until(&self, timestamp: DateTime<Utc>) -> Result<()>;
}

/// Wrapper that makes any Output into a BidirectionalOutput by composition
/// This allows us to add bidirectional capabilities to existing outputs
pub struct BidirectionalWrapper<T: Output> {
    inner: T,
}

impl<T: Output> BidirectionalWrapper<T> {
    pub fn new(inner: T) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl<T: Output> BidirectionalOutput for BidirectionalWrapper<T> {
    async fn send_message(&self, message: &str) -> Result<OutputResult> {
        self.inner.send_message(message).await
    }
    
    async fn health_check(&self) -> Result<bool> {
        self.inner.health_check().await
    }
    
    fn get_name(&self) -> &str {
        self.inner.get_name()
    }
    
    async fn check_for_responses(&self, _since: Option<DateTime<Utc>>) -> Result<Vec<CheckinResponse>> {
        // Default implementation for non-bidirectional outputs
        Ok(vec![])
    }
    
    async fn mark_processed_until(&self, _timestamp: DateTime<Utc>) -> Result<()> {
        // Default implementation - no-op
        Ok(())
    }
}

/// Factory for creating bidirectional outputs
pub struct BidirectionalOutputFactory;

impl BidirectionalOutputFactory {
    pub fn create_bidirectional_output(
        output_type: &str,
        config: &std::collections::HashMap<String, String>,
        is_bidirectional: bool,
    ) -> Result<Box<dyn BidirectionalOutput>> {
        tracing::debug!("Creating bidirectional output: type={}, is_bidirectional={}", output_type, is_bidirectional);
        match output_type {
            "email" => {
                if is_bidirectional {
                    // Create the specialized bidirectional email output
                    tracing::info!("Creating true bidirectional email output with IMAP support");
                    let output = super::email_bidirectional::BidirectionalEmailOutput::new(config)?;
                    Ok(Box::new(output))
                } else {
                    // Wrap the regular email output
                    tracing::info!("Creating regular email output (wrapped for bidirectional compatibility)");
                    let output = super::email::EmailOutput::new(config)?;
                    Ok(Box::new(BidirectionalWrapper::new(output)))
                }
            }
            "facebook_messenger" => {
                // Facebook Messenger could potentially be bidirectional too
                let output = super::facebook_messenger::FacebookMessengerOutput::new(config)?;
                Ok(Box::new(BidirectionalWrapper::new(output)))
            }
            _ => anyhow::bail!("Unknown output type: {}", output_type),
        }
    }
}

/// Helper function to process bidirectional outputs and collect any check-ins
pub async fn process_bidirectional_outputs_for_checkins(
    outputs: &[Box<dyn BidirectionalOutput>],
    since: Option<DateTime<Utc>>,
) -> Result<Vec<CheckinResponse>> {
    let mut all_responses = Vec::new();
    
    for output in outputs {
        tracing::debug!("Checking output: {}", output.get_name());
        match output.check_for_responses(since).await {
            Ok(mut responses) => {
                tracing::debug!("Found {} responses from {}", responses.len(), output.get_name());
                all_responses.append(&mut responses);
            }
            Err(e) => {
                tracing::warn!("Error checking for responses from {}: {}", output.get_name(), e);
                // Continue with other outputs even if one fails
            }
        }
    }
    
    Ok(all_responses)
}

/// Helper function to mark all outputs as processed up to a certain timestamp
pub async fn mark_all_processed_until(
    outputs: &[Box<dyn BidirectionalOutput>],
    timestamp: DateTime<Utc>,
) -> Result<()> {
    for output in outputs {
        if let Err(e) = output.mark_processed_until(timestamp).await {
            tracing::warn!("Error marking {} processed until {}: {}", 
                output.get_name(), timestamp, e);
        }
    }
    Ok(())
}