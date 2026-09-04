use std::time::Duration;

use futures_util::StreamExt;
use pulsebridge_api::{DeviceSnapshot, Metric, MetricEvent, Presence, ServerMessage};
use reqwest::Client;
use serde_json::{json, Value};
use tokio::sync::watch;
use tokio::time::Instant;
use tokio_tungstenite::connect_async;
use tracing::{debug, info, warn};
use tracing_subscriber::EnvFilter;

use pulsebridge_napcat_bridge::avatar;

const DEFAULT_SERVER_WS: &str = "ws://127.0.0.1:8080/ws";
const DEFAULT_NAPCAT_API: &str = "http://127.0.0.1:3000";
const DEFAULT_STATUS_FORMAT: &str = "{heart} {zone} · {bpm} BPM";
const DEFAULT_NICKNAME_FORMAT: &str = "June - 💓{bpm}";
const DEFAULT_NICKNAME_IDLE: &str = "June";
const DEFAULT_MIN_INTERVAL_MS: u64 = 0;
const DEFAULT_NICKNAME_MIN_INTERVAL_MS: u64 = 60_000;
const DEFAULT_MAX_HR: u64 = 201;
const IDLE_RETRY: Duration = Duration::from_secs(10);
const WS_RECONNECT_INITIAL: Duration = Duration::from_secs(1);
const WS_RECONNECT_MAX: Duration = Duration::from_secs(30);
const WS_STABLE_AFTER: Duration = Duration::from_secs(30);
const DEFAULT_AVATAR_MIN_INTERVAL_MS: u64 = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Waiting,
    Live { device_id: u32, bpm: u8 },
    NoData { device_id: u32 },
    Offline { device_id: Option<u32> },
}

impl State {
    fn heart(beat: bool) -> &'static str {
        if beat { "💓" } else { "♥" }
    }

    /// Full status text used for the custom online status wording
    /// (legacy channel and fallback when nickname updates fail).
    fn status_text(self, format: &str, max_hr: u8, beat: bool) -> String {
        let heart = Self::heart(beat);
        match self {
            Self::Waiting => format!("{heart} -- BPM · connecting"),
            Self::Live { bpm, .. } => fill(format, heart, zone_for(bpm, max_hr), bpm),
            Self::NoData { .. } => format!("{heart} -- BPM · no data"),
            Self::Offline { .. } => format!("{heart} -- BPM · offline"),
        }
    }
}

fn fill(template: &str, heart: &str, zone: &str, bpm: u8) -> String {
    template
        .replace("{heart}", heart)
        .replace("{zone}", zone)
        .replace("{bpm}", &bpm.to_string())
        .replace("{}", &bpm.to_string())
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
            d.device_id == id && d.presence == Presence::Online)) {
            self.active
        } else {
            devices.iter()
                .filter(|d| d.presence == Presence::Online && d.heart_rate.is_some())
                .map(|d| d.device_id)
                .min()
                .or_else(|| devices.iter()
                    .filter(|d| d.presence == Presence::Online)
                    .map(|d| d.device_id)
                    .min())
        };
        self.active = selected;
        match selected.and_then(|id| devices.iter().find(|d| d.device_id == id)) {
            Some(d) if d.presence == Presence::Online => d.heart_rate
                .map(|bpm| State::Live { device_id: d.device_id, bpm })
                .unwrap_or(State::NoData { device_id: d.device_id }),
            _ => State::Offline { device_id: selected },
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

#[derive(Clone)]
struct Config {
    api: String,
    token: Option<String>,
    status_format: String,
    nickname_format: String,
    nickname_idle: String,
    min_interval: Duration,
    nickname_min_interval: Duration,
    max_hr: u8,
    face_id: u64,
    face_type: u64,
    avatar_enabled: bool,
    avatar_min_interval: Duration,
}

impl Config {
    fn status_enabled(&self) -> bool {
        !self.status_format.is_empty()
    }

    fn nickname_enabled(&self) -> bool {
        !self.nickname_format.is_empty()
    }

    fn nickname_idle_enabled(&self) -> bool {
        self.nickname_enabled() && !self.nickname_idle.is_empty()
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
    let cfg = Config {
        api: env_or("NAPCAT_API_URL", DEFAULT_NAPCAT_API).trim_end_matches('/').to_string(),
        token: std::env::var("NAPCAT_ACCESS_TOKEN").ok(),
        status_format: env_or("PB_STATUS_FORMAT", DEFAULT_STATUS_FORMAT),
        nickname_format: env_or("PB_NICKNAME_FORMAT", DEFAULT_NICKNAME_FORMAT),
        nickname_idle: env_or("PB_NICKNAME_IDLE", DEFAULT_NICKNAME_IDLE),
        min_interval: Duration::from_millis(env_u64("PB_STATUS_MIN_INTERVAL_MS", DEFAULT_MIN_INTERVAL_MS)?),
        nickname_min_interval: Duration::from_millis(env_u64("PB_NICKNAME_MIN_INTERVAL_MS", DEFAULT_NICKNAME_MIN_INTERVAL_MS)?),
        max_hr: env_u64("PB_MAX_HR", DEFAULT_MAX_HR)?.try_into().map_err(|_| "PB_MAX_HR must be between 1 and 255")?,
        face_id: env_u64("NAPCAT_FACE_ID", 0)?,
        face_type: env_u64("NAPCAT_FACE_TYPE", 1)?,
        avatar_enabled: env_or("PB_AVATAR_ENABLED", "true").parse().map_err(|_| "PB_AVATAR_ENABLED must be true or false")?,
        avatar_min_interval: Duration::from_millis(env_u64("PB_AVATAR_MIN_INTERVAL_MS", DEFAULT_AVATAR_MIN_INTERVAL_MS)?),
    };
    let device_id = env_device_id()?;

    let client = Client::builder().timeout(Duration::from_secs(10)).build()?;
    let (state_tx, state_rx) = watch::channel(State::Waiting);
    let source = tokio::spawn(source_loop(server_ws, device_id, state_tx));
    let avatar_rx = state_rx.clone();
    let updater = tokio::spawn(updater_loop(client.clone(), cfg.clone(), state_rx));
    let avatar_updater = tokio::spawn(avatar_loop(client, cfg, avatar_rx));

    tokio::select! {
        result = source => result??,
        result = updater => result??,
        result = avatar_updater => result??,
        result = tokio::signal::ctrl_c() => { result?; info!("shutdown requested"); }
    }
    Ok(())
}

async fn avatar_loop(client: Client, cfg: Config, mut state_rx: watch::Receiver<State>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if !cfg.avatar_enabled { return Ok(()); }
    let avatar_config = match avatar::default_config() {
        Ok(config) => config,
        Err(error) => {
            warn!(%error, "avatar updater disabled because avatar.json could not be loaded");
            return Ok(());
        }
    };
    let renderer = match avatar::AvatarRenderer::start(avatar_config).await {
        Ok(renderer) => renderer,
        Err(error) => {
            warn!(%error, "avatar updater disabled because renderer could not start");
            return Ok(());
        }
    };
    let mut state = *state_rx.borrow_and_update();
    let mut last_state = None;
    let mut last_attempt = Instant::now().checked_sub(cfg.avatar_min_interval).unwrap_or_else(Instant::now);
    let mut ticker = tokio::time::interval(Duration::from_millis(100));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            changed = state_rx.changed() => { if changed.is_err() { return Ok(()); } state = *state_rx.borrow_and_update(); },
            _ = ticker.tick() => {
                let now = Instant::now();
                if last_state == Some(state) || now.duration_since(last_attempt) < cfg.avatar_min_interval { continue; }
                last_state = Some(state);
                last_attempt = now;
                let rendered = match state {
                    State::Live { bpm, .. } => renderer.render(bpm).await,
                    State::NoData { .. } => renderer.render_no_data().await,
                    State::Offline { .. } | State::Waiting => renderer.render_offline().await,
                };
                match rendered {
                    Ok(bytes) => match set_avatar(&client, &cfg, &bytes).await {
                        Ok(()) => info!(?state, bytes = bytes.len(), "updated QQ avatar"),
                        Err(error) => warn!(?state, %error, "QQ avatar update failed; newer state will supersede it"),
                    },
                    Err(error) => warn!(?state, %error, "heart-rate avatar render failed"),
                }
            }
        }
    }
}

async fn source_loop(server_ws: String, device_id: Option<u32>, state_tx: watch::Sender<State>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut retry_delay = WS_RECONNECT_INITIAL;
    loop {
        match connect_async(&server_ws).await {
            Ok((stream, _)) => {
                info!(%server_ws, "connected to PulseBridge WebSocket");
                let connected_at = Instant::now();
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
                if connected_at.elapsed() >= WS_STABLE_AFTER {
                    retry_delay = WS_RECONNECT_INITIAL;
                }
            }
            Err(error) => warn!(%error, "cannot connect to PulseBridge WebSocket"),
        }
        state_tx.send_replace(State::Offline { device_id });
        info!(delay = ?retry_delay, "PulseBridge WebSocket disconnected; reconnecting");
        tokio::time::sleep(retry_delay).await;
        retry_delay = (retry_delay * 2).min(WS_RECONNECT_MAX);
    }
}

async fn updater_loop(client: Client, cfg: Config, mut state_rx: watch::Receiver<State>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut state = *state_rx.borrow_and_update();
    // last_nickname_state: last state for which we attempted a nickname update. This
    // deliberately advances even on failure so a failed old state is never
    // retried; a later heart-rate state supersedes it.
    // nick_is_idle: the QQ nickname currently reflects the idle/rollback name.
    let mut last_nickname_state = None;
    let mut last_status_state = None;
    let mut nick_is_idle = true;
    let mut beat = false;
    let mut last_sent_at = Instant::now().checked_sub(cfg.min_interval).unwrap_or_else(Instant::now);
    let mut last_nickname_attempt = Instant::now().checked_sub(cfg.nickname_min_interval).unwrap_or_else(Instant::now);
    let mut last_idle_attempt = Instant::now().checked_sub(IDLE_RETRY).unwrap_or_else(Instant::now);
    let mut ticker = tokio::time::interval(Duration::from_millis(250));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            changed = state_rx.changed() => { if changed.is_err() { return Ok(()); } state = *state_rx.borrow_and_update(); },
            _ = ticker.tick() => {
                let now = Instant::now();
                match state {
                    State::Live { bpm, .. } => {
                        // Keep custom online status on the heart-rate cadence.
                        // Nickname changes have a separate, much slower quota.
                        if cfg.status_enabled()
                            && last_status_state != Some(state)
                            && now.duration_since(last_sent_at) >= cfg.min_interval
                        {
                            let next_beat = !beat;
                            let text = state.status_text(&cfg.status_format, cfg.max_hr, next_beat);
                            // A failed update is not retried for this exact
                            // state; the next heart-rate state is authoritative.
                            last_status_state = Some(state);
                            match set_status(&client, &cfg, &text).await {
                                Ok(()) => {
                                    info!(message = %text, "updated NapCat custom online status");
                                    beat = next_beat;
                                    last_sent_at = now;
                                }
                                Err(error) => warn!(%error, "NapCat status update failed; will retry"),
                            }
                        }

                        if cfg.nickname_enabled()
                            && last_nickname_state != Some(state)
                            && now.duration_since(last_nickname_attempt) >= cfg.nickname_min_interval
                        {
                            let nickname = fill(&cfg.nickname_format, State::heart(beat), zone_for(bpm, cfg.max_hr), bpm);
                            // Mark before the request: a failed old state is not
                            // retried, and a newer state supersedes it.
                            last_nickname_state = Some(state);
                            last_nickname_attempt = now;
                            match set_nickname(&client, &cfg, &nickname).await {
                                Ok(()) => {
                                    info!(nickname = %nickname, "updated QQ nickname");
                                    nick_is_idle = false;
                                }
                                Err(error) => warn!(%error, "QQ nickname update failed; latest state will be used on the next nickname window"),
                            }
                        }
                    }
                    _ => {
                        // Waiting / NoData / Offline: roll the nickname back to the idle
                        // name once per transition so a stale live value never
                        // lingers. Rate-limited so a dead NapCat cannot spam.
                        if cfg.nickname_idle_enabled() && !nick_is_idle && now.duration_since(last_idle_attempt) >= IDLE_RETRY {
                            last_idle_attempt = now;
                            // Do not retry the same failed rollback. A future
                            // live state will create a new nickname transition.
                            nick_is_idle = true;
                            match set_nickname(&client, &cfg, &cfg.nickname_idle).await {
                                Ok(()) => {
                                    info!(nickname = %cfg.nickname_idle, "QQ nickname rolled back to idle");
                                    last_nickname_state = None;
                                }
                                Err(error) => warn!(%error, "QQ nickname rollback failed; will retry"),
                            }
                        }
                    }
                }
            }
        }
    }
}

/// POST a JSON-RPC-style action to NapCat and validate the response envelope.
/// Returns the response body once status/retcode look successful.
async fn napcat_post(client: &Client, cfg: &Config, action: &str, body: Value) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let mut request = client.post(format!("{}/{}", cfg.api, action)).json(&body);
    if let Some(token) = &cfg.token { request = request.bearer_auth(token); }
    let response = request.send().await?;
    let status = response.status();
    let body: Value = response.json().await.unwrap_or_else(|_| json!({}));
    let failed = body.get("status").and_then(|v| v.as_str()) == Some("failed");
    let bad_result = body.get("data")
        .and_then(|d| d.get("result"))
        .and_then(|r| r.as_i64())
        .map(|r| r != 0)
        .unwrap_or(false);
    if !status.is_success() || failed || bad_result {
        let message = body.get("message").or_else(|| body.get("wording"))
            .or_else(|| body.get("data").and_then(|d| d.get("errMsg")))
            .and_then(|v| v.as_str())
            .filter(|value| !value.is_empty())
            .unwrap_or("unknown error");
        let retcode = body.get("retcode").and_then(Value::as_i64);
        return Err(format!("NapCat {action} failed ({status}, retcode={retcode:?}): {message}; response={body}").into());
    }
    Ok(body)
}

async fn set_status(client: &Client, cfg: &Config, wording: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    napcat_post(client, cfg, "set_diy_online_status", json!({
        "face_id": cfg.face_id,
        "face_type": cfg.face_type,
        "wording": wording,
    })).await.map(|_| ())
}

async fn set_nickname(client: &Client, cfg: &Config, nickname: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    napcat_post(client, cfg, "set_qq_profile", json!({
        "nickname": nickname,
    })).await.map(|_| ())
}

async fn set_avatar(client: &Client, cfg: &Config, bytes: &[u8]) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    napcat_post(client, cfg, "set_qq_avatar", json!({
        "file": format!("base64://{}", STANDARD.encode(bytes)),
    })).await.map(|_| ())
}
