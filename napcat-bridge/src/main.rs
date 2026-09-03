use std::time::Duration;

use futures_util::StreamExt;
use pulsebridge_api::{DeviceSnapshot, Metric, MetricEvent, Presence, ServerMessage};
use reqwest::Client;
use serde_json::json;
use tokio::sync::watch;
use tokio::time::Instant;
use tokio_tungstenite::connect_async;
use tracing::{debug, info, warn};
use tracing_subscriber::EnvFilter;

const DEFAULT_SERVER_WS: &str = "ws://127.0.0.1:8080/ws";
const DEFAULT_NAPCAT_API: &str = "http://127.0.0.1:3000";
const DEFAULT_FORMAT: &str = "{heart} {zone} · {bpm} BPM";
const DEFAULT_MIN_INTERVAL_MS: u64 = 0;
const DEFAULT_MAX_HR: u64 = 201;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Waiting,
    Live { device_id: u32, bpm: u8 },
    NoSignal { device_id: Option<u32> },
}

impl State {
    fn text(self, format: &str, max_hr: u8, beat: bool) -> String {
        let heart = if beat { "💓" } else { "♥" };
        match self {
            Self::Waiting => format!("{heart} -- BPM · connecting"),
            Self::Live { bpm, .. } => format
                .replace("{heart}", heart)
                .replace("{zone}", zone_for(bpm, max_hr))
                .replace("{bpm}", &bpm.to_string())
                .replace("{}", &bpm.to_string()),
            Self::NoSignal { .. } => format!("{heart} -- BPM · no signal"),
        }
    }
}

fn zone_for(bpm: u8, max_hr: u8) -> &'static str {
    let boundary = |percent: u16| -> u8 {
        ((u16::from(max_hr) * percent + 99) / 100) as u8
    };
    let thresholds = [boundary(60), boundary(70), boundary(80), boundary(90)];
    match bpm {
        value if value <= thresholds[0] => "Z1",
        value if value <= thresholds[1] => "Z2",
        value if value <= thresholds[2] => "Z3",
        value if value <= thresholds[3] => "Z4",
        _ => "Z5",
    }
}

struct Selector {
    configured: Option<u32>,
    active: Option<u32>,
}

impl Selector {
    fn new(configured: Option<u32>) -> Self {
        Self { configured, active: configured }
    }

    fn snapshot(&mut self, devices: &[DeviceSnapshot]) -> State {
        let selected = if let Some(id) = self.configured {
            Some(id)
        } else if self.active.is_some_and(|id| devices.iter().any(|d|
            d.device_id == id && d.presence == Presence::Online && d.heart_rate.is_some())) {
            self.active
        } else {
            devices.iter()
                .filter(|d| d.presence == Presence::Online && d.heart_rate.is_some())
                .map(|d| d.device_id)
                .min()
        };
        self.active = selected;
        match selected.and_then(|id| devices.iter().find(|d| d.device_id == id)) {
            Some(d) if d.presence == Presence::Online => d.heart_rate
                .map(|bpm| State::Live { device_id: d.device_id, bpm })
                .unwrap_or(State::NoSignal { device_id: selected }),
            _ => State::NoSignal { device_id: selected },
        }
    }

    fn metric(&mut self, event: &MetricEvent) -> Option<State> {
        if self.configured.is_some_and(|id| id != event.device_id) {
            return None;
        }
        if self.configured.is_none() {
            match self.active {
                Some(id) if id != event.device_id => return None,
                None => self.active = Some(event.device_id),
                _ => {}
            }
        }
        match event.metric {
            Metric::HeartRate { bpm, .. } => Some(State::Live { device_id: event.device_id, bpm }),
        }
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn env_u64(key: &str, default: u64) -> Result<u64, String> {
    std::env::var(key).map_or(Ok(default), |v| v.parse().map_err(|_| format!("{key} must be an integer")))
}

fn env_device_id() -> Result<Option<u32>, String> {
    std::env::var("PB_DEVICE_ID").map_or(Ok(None), |v| v.parse().map(Some).map_err(|_| "PB_DEVICE_ID must be a u32".into()))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let server_ws = env_or("PB_SERVER_WS", DEFAULT_SERVER_WS);
    let api = env_or("NAPCAT_API_URL", DEFAULT_NAPCAT_API).trim_end_matches('/').to_string();
    let token = std::env::var("NAPCAT_ACCESS_TOKEN").ok();
    let format = env_or("PB_STATUS_FORMAT", DEFAULT_FORMAT);
    let min_interval = Duration::from_millis(env_u64("PB_STATUS_MIN_INTERVAL_MS", DEFAULT_MIN_INTERVAL_MS)?);
    let max_hr = env_u64("PB_MAX_HR", DEFAULT_MAX_HR)?.try_into().map_err(|_| "PB_MAX_HR must be between 1 and 255")?;
    let face_id = env_u64("NAPCAT_FACE_ID", 0)?;
    let face_type = env_u64("NAPCAT_FACE_TYPE", 1)?;
    let device_id = env_device_id()?;

    let client = Client::builder().timeout(Duration::from_secs(10)).build()?;
    let (state_tx, state_rx) = watch::channel(State::Waiting);
    let source = tokio::spawn(source_loop(server_ws, device_id, state_tx));
    let updater = tokio::spawn(status_loop(client, api, token, format, min_interval, max_hr, face_id, face_type, state_rx));

    tokio::select! {
        result = source => result??,
        result = updater => result??,
        result = tokio::signal::ctrl_c() => { result?; info!("shutdown requested"); }
    }
    Ok(())
}

async fn source_loop(server_ws: String, device_id: Option<u32>, state_tx: watch::Sender<State>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut retry_secs = 1;
    loop {
        match connect_async(&server_ws).await {
            Ok((stream, _)) => {
                info!(%server_ws, "connected to PulseBridge WebSocket");
                retry_secs = 1;
                let mut selector = Selector::new(device_id);
                let (_, mut incoming) = stream.split();
                while let Some(message) = incoming.next().await {
                    match message {
                        Ok(message) if message.is_text() => match serde_json::from_str::<ServerMessage>(message.to_text()?) {
                            Ok(ServerMessage::Snapshot { devices }) => { state_tx.send_replace(selector.snapshot(&devices)); }
                            Ok(ServerMessage::Metric { event }) => if let Some(state) = selector.metric(&event) { state_tx.send_replace(state); },
                            Err(error) => debug!(%error, "ignored unknown server message"),
                        },
                        Ok(message) if message.is_close() => break,
                        Err(error) => { warn!(%error, "PulseBridge WebSocket receive failed"); break; }
                        _ => {}
                    }
                }
            }
            Err(error) => warn!(%error, "cannot connect to PulseBridge WebSocket"),
        }
        state_tx.send_replace(State::NoSignal { device_id });
        tokio::time::sleep(Duration::from_secs(retry_secs)).await;
        retry_secs = (retry_secs * 2).min(10);
    }
}

async fn status_loop(client: Client, api: String, token: Option<String>, format: String, min_interval: Duration, max_hr: u8, face_id: u64, face_type: u64, mut state_rx: watch::Receiver<State>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut state = *state_rx.borrow_and_update();
    let mut last_state = None;
    let mut beat = false;
    let mut last_sent_at = Instant::now().checked_sub(min_interval).unwrap_or_else(Instant::now);
    let mut ticker = tokio::time::interval(Duration::from_millis(250));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            changed = state_rx.changed() => { if changed.is_err() { return Ok(()); } state = *state_rx.borrow_and_update(); },
            _ = ticker.tick() => {
                let now = Instant::now();
                let usable = !matches!(state, State::Waiting);
                if usable && last_state != Some(state) && now.duration_since(last_sent_at) >= min_interval {
                    let next_beat = !beat;
                    let text = state.text(&format, max_hr, next_beat);
                    match set_status(&client, &api, token.as_deref(), face_id, face_type, &text).await {
                        Ok(()) => {
                            info!(message = %text, "updated NapCat custom online status");
                            last_state = Some(state);
                            beat = next_beat;
                            last_sent_at = now;
                        }
                        Err(error) => warn!(%error, "NapCat status update failed; will retry"),
                    }
                }
            }
        }
    }
}

async fn set_status(client: &Client, api: &str, token: Option<&str>, face_id: u64, face_type: u64, wording: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut request = client.post(format!("{api}/set_diy_online_status")).json(&json!({ "face_id": face_id, "face_type": face_type, "wording": wording }));
    if let Some(token) = token { request = request.bearer_auth(token); }
    let response = request.send().await?;
    let status = response.status();
    let body: serde_json::Value = response.json().await.unwrap_or_else(|_| json!({}));
    if !status.is_success() || body.get("status").and_then(|v| v.as_str()) == Some("failed") {
        return Err(format!("NapCat status update failed ({status}): {body}").into());
    }
    Ok(())
}
