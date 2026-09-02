//! Shared subscriber contract between PulseBridge server and downstream clients.
//!
//! This crate deliberately contains data types only. Transport, storage and
//! application-specific behavior stay in their own projects.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Presence {
    Online,
    Stale,
    Offline,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "metric", rename_all = "snake_case")]
pub enum Metric {
    HeartRate { bpm: u8, contact_ok: bool },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MetricEvent {
    pub device_id: u32,
    pub timestamp_ms: u64,
    #[serde(flatten)]
    pub metric: Metric,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceSnapshot {
    pub device_id: u32,
    pub presence: Presence,
    pub age_ms: u64,
    pub heart_rate: Option<u8>,
    pub resting_hr: Option<u8>,
    pub contact_ok: bool,
    pub watch_connected: bool,
    pub phone_battery_pct: Option<u8>,
    pub session_id: u32,
    pub packets: u64,
    pub last_sequence: u32,
    pub gaps: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Snapshot { devices: Vec<DeviceSnapshot> },
    Metric { event: MetricEvent },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metric_json_shape_stays_compatible() {
        let message = ServerMessage::Metric {
            event: MetricEvent {
                device_id: 7,
                timestamp_ms: 1,
                metric: Metric::HeartRate {
                    bpm: 99,
                    contact_ok: true,
                },
            },
        };
        let json = serde_json::to_value(message).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "type": "metric",
                "event": {
                    "device_id": 7,
                    "timestamp_ms": 1,
                    "metric": "heart_rate",
                    "bpm": 99,
                    "contact_ok": true
                }
            })
        );
    }
}
