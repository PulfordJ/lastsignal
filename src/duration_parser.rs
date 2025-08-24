use anyhow::{bail, Context, Result};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigDuration(Duration);

impl ConfigDuration {
    pub fn as_duration(&self) -> Duration {
        self.0
    }

    pub fn as_secs(&self) -> u64 {
        self.0.as_secs()
    }

    pub fn as_days(&self) -> u64 {
        self.0.as_secs() / (24 * 60 * 60)
    }

    pub fn as_hours(&self) -> u64 {
        self.0.as_secs() / (60 * 60)
    }

    pub fn as_minutes(&self) -> u64 {
        self.0.as_secs() / 60
    }

    pub fn from_days(days: u64) -> Self {
        Self(Duration::from_secs(days * 24 * 60 * 60))
    }

    pub fn from_hours(hours: u64) -> Self {
        Self(Duration::from_secs(hours * 60 * 60))
    }

    pub fn from_minutes(minutes: u64) -> Self {
        Self(Duration::from_secs(minutes * 60))
    }

    pub fn from_seconds(seconds: u64) -> Self {
        Self(Duration::from_secs(seconds))
    }
}

impl FromStr for ConfigDuration {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let s = s.trim();
        
        if s.is_empty() {
            bail!("Duration cannot be empty");
        }


        // Parse with units
        let (number_part, unit_part) = split_number_and_unit(s)?;
        let value = number_part.parse::<u64>()
            .with_context(|| format!("Invalid number in duration: '{}'", number_part))?;

        if value == 0 {
            bail!("Duration must be greater than 0");
        }

        match unit_part {
            "s" | "sec" | "secs" | "second" | "seconds" => Ok(ConfigDuration::from_seconds(value)),
            "m" | "min" | "mins" | "minute" | "minutes" => Ok(ConfigDuration::from_minutes(value)),
            "h" | "hr" | "hrs" | "hour" | "hours" => Ok(ConfigDuration::from_hours(value)),
            "d" | "day" | "days" => Ok(ConfigDuration::from_days(value)),
            _ => bail!("Invalid duration unit '{}'. Valid units: s, m, h, d (or their full names)", unit_part),
        }
    }
}

fn split_number_and_unit(s: &str) -> Result<(&str, &str)> {
    let mut split_pos = 0;
    
    for (i, c) in s.char_indices() {
        if c.is_ascii_digit() {
            split_pos = i + 1;
        } else {
            break;
        }
    }
    
    if split_pos == 0 {
        bail!("Duration must start with a number");
    }
    
    if split_pos == s.len() {
        bail!("Duration must include a unit (s, m, h, d)");
    }
    
    let number_part = &s[..split_pos];
    let unit_part = s[split_pos..].trim();
    
    if unit_part.is_empty() {
        bail!("Duration must include a unit (s, m, h, d)");
    }
    
    Ok((number_part, unit_part))
}

impl fmt::Display for ConfigDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let secs = self.0.as_secs();
        
        if secs % (24 * 60 * 60) == 0 {
            write!(f, "{}d", secs / (24 * 60 * 60))
        } else if secs % (60 * 60) == 0 {
            write!(f, "{}h", secs / (60 * 60))
        } else if secs % 60 == 0 {
            write!(f, "{}m", secs / 60)
        } else {
            write!(f, "{}s", secs)
        }
    }
}

impl Serialize for ConfigDuration {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ConfigDuration {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::{Unexpected, Visitor};
        use std::fmt;

        struct DurationVisitor;

        impl<'de> Visitor<'de> for DurationVisitor {
            type Value = ConfigDuration;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a string with duration format (e.g., '7d', '24h', '30m', '3600s')")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                value.parse().map_err(serde::de::Error::custom)
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Err(E::invalid_type(Unexpected::Unsigned(value), &self))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Err(E::invalid_type(Unexpected::Signed(value), &self))
            }
        }

        deserializer.deserialize_any(DurationVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_seconds() {
        assert_eq!("30s".parse::<ConfigDuration>().unwrap().as_secs(), 30);
        assert_eq!("45sec".parse::<ConfigDuration>().unwrap().as_secs(), 45);
        assert_eq!("60seconds".parse::<ConfigDuration>().unwrap().as_secs(), 60);
    }

    #[test]
    fn test_parse_minutes() {
        assert_eq!("5m".parse::<ConfigDuration>().unwrap().as_secs(), 300);
        assert_eq!("10min".parse::<ConfigDuration>().unwrap().as_secs(), 600);
        assert_eq!("15minutes".parse::<ConfigDuration>().unwrap().as_secs(), 900);
    }

    #[test]
    fn test_parse_hours() {
        assert_eq!("2h".parse::<ConfigDuration>().unwrap().as_secs(), 7200);
        assert_eq!("3hr".parse::<ConfigDuration>().unwrap().as_secs(), 10800);
        assert_eq!("4hours".parse::<ConfigDuration>().unwrap().as_secs(), 14400);
    }

    #[test]
    fn test_parse_days() {
        assert_eq!("1d".parse::<ConfigDuration>().unwrap().as_secs(), 86400);
        assert_eq!("7day".parse::<ConfigDuration>().unwrap().as_secs(), 604800);
        assert_eq!("30days".parse::<ConfigDuration>().unwrap().as_secs(), 2592000);
    }

    #[test]
    fn test_serde_rejects_pure_numbers() {
        // Test that serde deserialization rejects pure numbers
        use serde_json;
        
        // Test integer parsing via serde should fail
        assert!(serde_json::from_str::<ConfigDuration>("3600").is_err());
        assert!(serde_json::from_str::<ConfigDuration>("86400").is_err());
        
        // But string versions should work
        assert!(serde_json::from_str::<ConfigDuration>("\"3600s\"").is_ok());
        assert!(serde_json::from_str::<ConfigDuration>("\"1d\"").is_ok());
    }

    #[test]
    fn test_display() {
        assert_eq!(ConfigDuration::from_seconds(30).to_string(), "30s");
        assert_eq!(ConfigDuration::from_minutes(5).to_string(), "5m");
        assert_eq!(ConfigDuration::from_hours(2).to_string(), "2h");
        assert_eq!(ConfigDuration::from_days(7).to_string(), "7d");
    }

    #[test]
    fn test_display_prefers_larger_units() {
        assert_eq!(ConfigDuration::from_seconds(3600).to_string(), "1h");
        assert_eq!(ConfigDuration::from_seconds(86400).to_string(), "1d");
        assert_eq!(ConfigDuration::from_seconds(604800).to_string(), "7d");
    }

    #[test]
    fn test_invalid_durations() {
        assert!("".parse::<ConfigDuration>().is_err());
        assert!("0s".parse::<ConfigDuration>().is_err());
        assert!("5x".parse::<ConfigDuration>().is_err());
        assert!("abc".parse::<ConfigDuration>().is_err());
        assert!("5".parse::<ConfigDuration>().is_err()); // No unit - should fail
    }

    #[test]
    fn test_conversion_methods() {
        let dur = ConfigDuration::from_days(2);
        assert_eq!(dur.as_days(), 2);
        assert_eq!(dur.as_hours(), 48);
        assert_eq!(dur.as_minutes(), 2880);
        assert_eq!(dur.as_secs(), 172800);
    }

    #[test]
    fn test_serde() {
        use serde_json;
        
        let duration = ConfigDuration::from_hours(24);
        let json = serde_json::to_string(&duration).unwrap();
        assert_eq!(json, "\"1d\"");
        
        let deserialized: ConfigDuration = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, duration);
    }
}