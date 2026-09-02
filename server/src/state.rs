use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tokio::sync::broadcast;

use crate::protocol::{Header, ReplayWindow, Telemetry};

/// How stale a sample may be before the device stops counting as live.
pub const ONLINE_MS: u64 = 15_000;
pub const STALE_MS: u64 = 60_000;

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Presence {
    Online,
    Stale,
    Offline,
}

/// A single metric reading on the bus. Adding stress / HRV / pace later means
/// adding variants here, not touching the transport or the subscribers.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "metric", rename_all = "snake_case")]
pub enum Metric {
    HeartRate { bpm: u8, contact_ok: bool },
}

#[derive(Debug, Clone, Serialize)]
pub struct MetricEvent {
    pub device_id: u32,
    pub timestamp_ms: u64,
    #[serde(flatten)]
    pub metric: Metric,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceSnapshot {
    pub device_id: u32,
    pub presence: Presence,
    /// Milliseconds since the last accepted packet.
    pub age_ms: u64,
    pub heart_rate: Option<u8>,
    pub resting_hr: Option<u8>,
    pub contact_ok: bool,
    pub watch_connected: bool,
    pub phone_battery_pct: Option<u8>,
    pub session_id: u32,
    pub packets: u64,
    pub last_sequence: u32,
    /// Sequence numbers we never saw, a rough packet-loss indicator.
    pub gaps: u64,
}

struct Device {
    session_id: u32,
    replay: ReplayWindow,
    addr: SocketAddr,
    last_seen_ms: u64,
    last_sender_ts_ms: u64,
    heart_rate: Option<u8>,
    resting_hr: Option<u8>,
    contact_ok: bool,
    watch_connected: bool,
    phone_battery_pct: Option<u8>,
    packets: u64,
    highest_seq: u32,
    gaps: u64,
}

impl Device {
    fn snapshot(&self, device_id: u32, now: u64) -> DeviceSnapshot {
        let age = now.saturating_sub(self.last_seen_ms);
        let presence = if age <= ONLINE_MS {
            Presence::Online
        } else if age <= STALE_MS {
            Presence::Stale
        } else {
            Presence::Offline
        };
        DeviceSnapshot {
            device_id,
            presence,
            age_ms: age,
            // A device that has gone quiet must not keep reporting its last
            // reading as if it were current.
            heart_rate: if presence == Presence::Offline { None } else { self.heart_rate },
            // Resting rate is a property of the wearer, not of the live link,
            // so it survives the device going quiet.
            resting_hr: self.resting_hr,
            contact_ok: self.contact_ok,
            watch_connected: self.watch_connected && presence != Presence::Offline,
            phone_battery_pct: self.phone_battery_pct,
            session_id: self.session_id,
            packets: self.packets,
            last_sequence: self.highest_seq,
            gaps: self.gaps,
        }
    }
}

pub struct Store {
    devices: Mutex<HashMap<u32, Device>>,
    events: broadcast::Sender<MetricEvent>,
}

pub enum Accepted {
    /// Packet was accepted; carries an event if a metric actually changed.
    Ok(Option<MetricEvent>),
    Replay,
    ClockSkew,
}

impl Store {
    pub fn new() -> Self {
        let (events, _) = broadcast::channel(256);
        Store { devices: Mutex::new(HashMap::new()), events }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<MetricEvent> {
        self.events.subscribe()
    }

    pub fn ingest(&self, h: &Header, t: &Telemetry, addr: SocketAddr) -> Accepted {
        let now = now_ms();
        if h.timestamp_ms.abs_diff(now) > 120_000 {
            return Accepted::ClockSkew;
        }

        let mut devices = self.devices.lock().unwrap();
        let dev = devices.entry(h.device_id).or_insert_with(|| Device {
            session_id: h.session_id,
            replay: ReplayWindow::default(),
            addr,
            last_seen_ms: 0,
            last_sender_ts_ms: 0,
            heart_rate: None,
            resting_hr: None,
            contact_ok: false,
            watch_connected: false,
            phone_battery_pct: None,
            packets: 0,
            highest_seq: 0,
            gaps: 0,
        });

        // A new session means the client restarted: drop the old replay state
        // instead of rejecting every packet until the sequence catches up.
        if dev.session_id != h.session_id {
            dev.session_id = h.session_id;
            dev.replay = ReplayWindow::default();
            dev.highest_seq = 0;
            dev.gaps = 0;
        }

        if !dev.replay.accept(h.sequence) {
            return Accepted::Replay;
        }

        // Only rebind the address after the packet has authenticated, so a
        // spoofed source address cannot hijack the device. See protocol.md 5.
        dev.addr = addr;

        if h.sequence > dev.highest_seq {
            if dev.highest_seq != 0 {
                dev.gaps += (h.sequence - dev.highest_seq - 1) as u64;
            }
            dev.highest_seq = h.sequence;
        }

        dev.last_seen_ms = now;
        dev.last_sender_ts_ms = h.timestamp_ms;
        dev.packets += 1;
        dev.contact_ok = t.contact_ok();
        dev.watch_connected = t.watch_connected();
        dev.phone_battery_pct = if t.battery_pct == 0xFF { None } else { Some(t.battery_pct) };
        if t.resting_hr != 0 {
            dev.resting_hr = Some(t.resting_hr);
        }

        let previous = dev.heart_rate;
        let new_hr = if t.hr_valid() { Some(t.heart_rate) } else { None };
        dev.heart_rate = new_hr;
        drop(devices);

        let event = match new_hr {
            Some(bpm) if previous != Some(bpm) => {
                let ev = MetricEvent {
                    device_id: h.device_id,
                    timestamp_ms: h.timestamp_ms,
                    metric: Metric::HeartRate { bpm, contact_ok: t.contact_ok() },
                };
                let _ = self.events.send(ev.clone());
                Some(ev)
            }
            _ => None,
        };
        Accepted::Ok(event)
    }

    pub fn snapshot_all(&self) -> Vec<DeviceSnapshot> {
        let now = now_ms();
        let devices = self.devices.lock().unwrap();
        let mut v: Vec<_> = devices.iter().map(|(id, d)| d.snapshot(*id, now)).collect();
        v.sort_by_key(|d| d.device_id);
        v
    }

    pub fn snapshot_one(&self, device_id: u32) -> Option<DeviceSnapshot> {
        let now = now_ms();
        let devices = self.devices.lock().unwrap();
        devices.get(&device_id).map(|d| d.snapshot(device_id, now))
    }
}
