use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use pulsebridge_api::{DeviceSnapshot, Metric, MetricEvent, Presence};
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
            // A reading is current only while packets are arriving within the
            // online window. Once the stream is stale, expose no heart rate so
            // consumers cannot mistake the last sample for a live value.
            heart_rate: if presence == Presence::Online { self.heart_rate } else { None },
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
    Ok(IngestReport),
    Replay,
    ClockSkew,
}

/// Internal ingest evidence for logging. This never crosses the REST or
/// WebSocket boundary.
pub struct IngestReport {
    pub event: Option<MetricEvent>,
    pub received_at_ms: u64,
    pub ingest_lag_ms: i64,
    pub new_device: bool,
    pub new_session: bool,
    pub gap_count: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{
        Header, Telemetry, FLAG_CONTACT_OK, FLAG_HR_VALID, FLAG_WATCH_CONNECTED,
        PACKET_TELEMETRY,
    };

    fn header(session_id: u32, sequence: u32) -> Header {
        Header {
            packet_type: PACKET_TELEMETRY,
            device_id: 42,
            session_id,
            sequence,
            timestamp_ms: now_ms(),
        }
    }

    fn telemetry() -> Telemetry {
        Telemetry {
            flags: FLAG_HR_VALID | FLAG_CONTACT_OK | FLAG_WATCH_CONNECTED,
            heart_rate: 72,
            battery_pct: 85,
            resting_hr: 51,
        }
    }

    #[test]
    fn ingest_report_contains_device_session_lag_and_gap_metadata() {
        let store = Store::new();
        let addr = "192.0.2.10:9999".parse().unwrap();

        let first = match store.ingest(&header(100, 1), &telemetry(), addr) {
            Accepted::Ok(report) => report,
            _ => panic!("expected accepted packet"),
        };
        assert!(first.new_device);
        assert!(!first.new_session);
        assert_eq!(first.gap_count, 0);
        assert!(first.event.is_some());
        assert!(first.ingest_lag_ms.abs() < 2_000);

        let gap = match store.ingest(&header(100, 3), &telemetry(), addr) {
            Accepted::Ok(report) => report,
            _ => panic!("expected accepted packet"),
        };
        assert!(!gap.new_device);
        assert!(!gap.new_session);
        assert_eq!(gap.gap_count, 1);
        assert!(gap.event.is_none());

        let session = match store.ingest(&header(200, 1), &telemetry(), addr) {
            Accepted::Ok(report) => report,
            _ => panic!("expected accepted packet"),
        };
        assert!(!session.new_device);
        assert!(session.new_session);
        assert_eq!(session.gap_count, 0);
    }
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
        let received_at_ms = now_ms();
        if h.timestamp_ms.abs_diff(received_at_ms) > 120_000 {
            return Accepted::ClockSkew;
        }

        let mut devices = self.devices.lock().unwrap();
        let new_device = !devices.contains_key(&h.device_id);
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
        let new_session = dev.session_id != h.session_id;
        if new_session {
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

        let mut gap_count = 0;
        if h.sequence > dev.highest_seq {
            if dev.highest_seq != 0 {
                gap_count = (h.sequence - dev.highest_seq - 1) as u64;
                dev.gaps += gap_count;
            }
            dev.highest_seq = h.sequence;
        }

        dev.last_seen_ms = received_at_ms;
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
        let ingest_lag_ms = if received_at_ms >= h.timestamp_ms {
            (received_at_ms - h.timestamp_ms) as i64
        } else {
            -((h.timestamp_ms - received_at_ms) as i64)
        };
        Accepted::Ok(IngestReport {
            event,
            received_at_ms,
            ingest_lag_ms,
            new_device,
            new_session,
            gap_count,
        })
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
