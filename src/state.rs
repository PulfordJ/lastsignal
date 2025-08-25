use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::duration_parser::ConfigDuration;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AppState {
    pub last_checkin: Option<DateTime<Utc>>,
    pub last_checkin_request: Option<DateTime<Utc>>,
    pub last_signal_fired: Option<DateTime<Utc>>,
    pub checkin_request_count: u32,
    pub version: String,
    /// Tracks which recipients have successfully received the last signal
    /// Key is recipient identifier (e.g., "email:emergency@example.com"), 
    /// Value is timestamp when successfully sent
    #[serde(default)]
    pub last_signal_recipients_notified: HashMap<String, DateTime<Utc>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            last_checkin: None,
            last_checkin_request: None,
            last_signal_fired: None,
            checkin_request_count: 0,
            version: env!("CARGO_PKG_VERSION").to_string(),
            last_signal_recipients_notified: HashMap::new(),
        }
    }
}

impl AppState {
    pub fn load_from_path<P: AsRef<Path>>(path: P) -> Result<Self> {
        if !path.as_ref().exists() {
            tracing::info!("State file does not exist, creating new state");
            return Ok(Self::default());
        }

        let content = std::fs::read_to_string(path.as_ref())
            .with_context(|| format!("Failed to read state file: {:?}", path.as_ref()))?;

        let state: AppState = serde_json::from_str(&content)
            .with_context(|| "Failed to parse state file as JSON")?;

        Ok(state)
    }

    pub fn save_to_path<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create state directory: {:?}", parent))?;
        }

        let content = serde_json::to_string_pretty(self)
            .context("Failed to serialize state to JSON")?;

        std::fs::write(path.as_ref(), content)
            .with_context(|| format!("Failed to write state file: {:?}", path.as_ref()))?;

        Ok(())
    }

    pub fn record_checkin(&mut self) {
        tracing::info!("Recording checkin at {}", Utc::now());
        self.last_checkin = Some(Utc::now());
        self.checkin_request_count = 0;
    }

    pub fn record_checkin_request(&mut self) {
        tracing::info!("Recording checkin request at {}", Utc::now());
        self.last_checkin_request = Some(Utc::now());
        self.checkin_request_count += 1;
    }

    pub fn record_last_signal_fired(&mut self) {
        tracing::info!("Recording last signal fired at {}", Utc::now());
        self.last_signal_fired = Some(Utc::now());
    }

    pub fn record_last_signal_recipient_notified(&mut self, recipient_id: &str) {
        let now = Utc::now();
        tracing::info!("Recording last signal sent to recipient {} at {}", recipient_id, now);
        self.last_signal_recipients_notified.insert(recipient_id.to_string(), now);
    }

    pub fn is_last_signal_recipient_already_notified(&self, recipient_id: &str) -> bool {
        self.last_signal_recipients_notified.contains_key(recipient_id)
    }

    pub fn get_pending_last_signal_recipients(&self, all_recipient_ids: &[String]) -> Vec<String> {
        all_recipient_ids.iter()
            .filter(|id| !self.last_signal_recipients_notified.contains_key(*id))
            .cloned()
            .collect()
    }

    pub fn clear_last_signal_recipient_tracking(&mut self) {
        tracing::info!("Clearing last signal recipient tracking");
        self.last_signal_recipients_notified.clear();
        self.last_signal_fired = None;
    }

    pub fn days_since_last_checkin(&self) -> Option<i64> {
        self.last_checkin.map(|checkin_time| {
            let duration = Utc::now().signed_duration_since(checkin_time);
            duration.num_days()
        })
    }

    pub fn days_since_last_checkin_request(&self) -> Option<i64> {
        self.last_checkin_request.map(|request_time| {
            let duration = Utc::now().signed_duration_since(request_time);
            duration.num_days()
        })
    }

    pub fn days_since_last_signal_fired(&self) -> Option<i64> {
        self.last_signal_fired.map(|signal_time| {
            let duration = Utc::now().signed_duration_since(signal_time);
            duration.num_days()
        })
    }

    pub fn should_request_checkin(&self, duration_between_checkins: ConfigDuration) -> bool {
        match self.last_checkin {
            None => true, // Never checked in before
            Some(_) => {
                let days_since = self.days_since_last_checkin().unwrap_or(0);
                days_since >= duration_between_checkins.as_days() as i64
            }
        }
    }

    pub fn should_fire_last_signal(&self, max_time_since_last_checkin: ConfigDuration) -> bool {
        match self.last_checkin {
            None => {
                // If we've never had a checkin, we need to look at how long we've been running
                // For now, we'll be conservative and only fire if we've explicitly been requesting checkins
                match self.last_checkin_request {
                    None => false,
                    Some(_) => {
                        let days_since_request = self.days_since_last_checkin_request().unwrap_or(0);
                        days_since_request >= max_time_since_last_checkin.as_days() as i64
                    }
                }
            }
            Some(_) => {
                let days_since_checkin = self.days_since_last_checkin().unwrap_or(0);
                days_since_checkin >= max_time_since_last_checkin.as_days() as i64
            }
        }
    }

    pub fn has_fired_last_signal_recently(&self, max_time_since_last_checkin: ConfigDuration) -> bool {
        match self.last_signal_fired {
            None => false,
            Some(_) => {
                let days_since_signal = self.days_since_last_signal_fired().unwrap_or(i64::MAX);
                days_since_signal < max_time_since_last_checkin.as_days() as i64
            }
        }
    }
}

pub struct StateManager {
    state_file_path: PathBuf,
    state: AppState,
}

impl StateManager {
    pub fn new(data_directory: &Path) -> Result<Self> {
        let state_file_path = data_directory.join("state.json");
        let state = AppState::load_from_path(&state_file_path)?;

        Ok(StateManager {
            state_file_path,
            state,
        })
    }

    pub fn get_state(&self) -> &AppState {
        &self.state
    }

    pub fn get_state_mut(&mut self) -> &mut AppState {
        &mut self.state
    }

    pub fn save(&self) -> Result<()> {
        self.state.save_to_path(&self.state_file_path)
    }

    pub fn record_checkin(&mut self) -> Result<()> {
        self.state.record_checkin();
        self.save()
    }

    pub fn record_checkin_request(&mut self) -> Result<()> {
        self.state.record_checkin_request();
        self.save()
    }

    pub fn record_last_signal_fired(&mut self) -> Result<()> {
        self.state.record_last_signal_fired();
        self.save()
    }

    pub fn record_last_signal_recipient_notified(&mut self, recipient_id: &str) -> Result<()> {
        self.state.record_last_signal_recipient_notified(recipient_id);
        self.save()
    }

    pub fn clear_last_signal_recipient_tracking(&mut self) -> Result<()> {
        self.state.clear_last_signal_recipient_tracking();
        self.save()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use tempfile::tempdir;

    #[test]
    fn test_app_state_default() {
        let state = AppState::default();
        assert!(state.last_checkin.is_none());
        assert!(state.last_checkin_request.is_none());
        assert!(state.last_signal_fired.is_none());
        assert_eq!(state.checkin_request_count, 0);
    }

    #[test]
    fn test_app_state_record_checkin() {
        let mut state = AppState::default();
        state.record_checkin();
        
        assert!(state.last_checkin.is_some());
        assert_eq!(state.checkin_request_count, 0);
    }

    #[test]
    fn test_app_state_record_checkin_request() {
        let mut state = AppState::default();
        state.record_checkin_request();
        
        assert!(state.last_checkin_request.is_some());
        assert_eq!(state.checkin_request_count, 1);
        
        state.record_checkin_request();
        assert_eq!(state.checkin_request_count, 2);
    }

    #[test]
    fn test_should_request_checkin() {
        let mut state = AppState::default();
        let seven_days = ConfigDuration::from_days(7);
        
        // Should request checkin if never checked in
        assert!(state.should_request_checkin(seven_days));
        
        // Record a checkin
        state.record_checkin();
        
        // Should not request immediately after checkin
        assert!(!state.should_request_checkin(seven_days));
        
        // Simulate 8 days ago
        state.last_checkin = Some(Utc::now() - Duration::days(8));
        
        // Should request checkin after 7 days
        assert!(state.should_request_checkin(seven_days));
    }

    #[test]
    fn test_should_fire_last_signal() {
        let mut state = AppState::default();
        let fourteen_days = ConfigDuration::from_days(14);
        
        // Should not fire if no checkin requests made
        assert!(!state.should_fire_last_signal(fourteen_days));
        
        // Record a checkin request 15 days ago
        state.last_checkin_request = Some(Utc::now() - Duration::days(15));
        
        // Should fire after 14 days of no checkin
        assert!(state.should_fire_last_signal(fourteen_days));
        
        // Record a checkin 
        state.record_checkin();
        
        // Should not fire immediately after checkin
        assert!(!state.should_fire_last_signal(fourteen_days));
        
        // Simulate 15 days since last checkin
        state.last_checkin = Some(Utc::now() - Duration::days(15));
        
        // Should fire after 14 days
        assert!(state.should_fire_last_signal(fourteen_days));
    }

    #[test]
    fn test_state_persistence() {
        let temp_dir = tempdir().unwrap();
        let state_path = temp_dir.path().join("state.json");
        
        let mut state = AppState::default();
        state.record_checkin();
        
        state.save_to_path(&state_path).unwrap();
        
        let loaded_state = AppState::load_from_path(&state_path).unwrap();
        assert!(loaded_state.last_checkin.is_some());
    }

    #[test]
    fn test_state_manager() {
        let temp_dir = tempdir().unwrap();
        let mut manager = StateManager::new(temp_dir.path()).unwrap();
        
        assert!(manager.get_state().last_checkin.is_none());
        
        manager.record_checkin().unwrap();
        assert!(manager.get_state().last_checkin.is_some());
        
        // Create a new manager to test persistence
        let manager2 = StateManager::new(temp_dir.path()).unwrap();
        assert!(manager2.get_state().last_checkin.is_some());
    }
}