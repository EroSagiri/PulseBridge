use std::time::Duration;

use chrono::{Local, Timelike};
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
const IDLE_RETRY: Duration = Duration::from_secs(10);
const WS_RECONNECT_INITIAL: Duration = Duration::from_secs(1);
const WS_RECONNECT_MAX: Duration = Duration::from_secs(30);
const WS_STABLE_AFTER: Duration = Duration::from_secs(30);
const DEFAULT_AVATAR_DAY_INTERVAL_SEC: u64 = 10;
const DEFAULT_AVATAR_NIGHT_INTERVAL_SEC: u64 = 30;
const DEFAULT_AVATAR_NIGHT_START_HOUR: u64 = 23;
const DEFAULT_AVATAR_NIGHT_END_HOUR: u64 = 7;
const DEFAULT_AVATAR_JUMP_THRESHOLD_BPM: u64 = 10;
const DEFAULT_AVATAR_JUMP_COOLDOWN_SEC: u64 = 5;

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
    fn status_text(self, format: &str, zones: &avatar::ZoneScheme, beat: bool) -> String {
        let heart = Self::heart(beat);
        match self {
            Self::Waiting => format!("{heart} -- BPM · connecting"),
            Self::Live { bpm, .. } => fill(format, heart, zone_for(bpm, zones), bpm),
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

fn zone_for(bpm: u8, zones: &avatar::ZoneScheme) -> &'static str {
    zones.zone_for(u16::from(bpm)).as_str()
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
    zone_scheme: avatar::ZoneScheme,
    face_id: u64,
    face_type: u64,
    avatar_enabled: bool,
    avatar_day_interval: Duration,
    avatar_night_interval: Duration,
    avatar_night_start_hour: u8,
    avatar_night_end_hour: u8,
    avatar_jump_threshold_bpm: u8,
    avatar_jump_cooldown: Duration,
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

fn env_positive_duration(key: &str, default: u64) -> Result<Duration, String> {
    let value = env_u64(key, default)?;
    if value == 0 {
        return Err(format!("{key} must be greater than zero"));
    }
    Ok(Duration::from_secs(value))
}

fn env_hour(key: &str, default: u64) -> Result<u8, String> {
    let value = env_u64(key, default)?;
    if value > 23 {
        return Err(format!("{key} must be between 0 and 23"));
    }
    Ok(value as u8)
}

fn env_device_id() -> Result<Option<u32>, String> {
    std::env::var("PB_DEVICE_ID").map_or(Ok(None), |v| v.parse().map(Some).map_err(|_| "PB_DEVICE_ID must be a u32".into()))
}

fn is_night_hour(hour: u8, start: u8, end: u8) -> bool {
    if start == end {
        return false;
    }
    if start < end {
        (start..end).contains(&hour)
    } else {
        hour >= start || hour < end
    }
}

fn avatar_is_night(cfg: &Config) -> bool {
    is_night_hour(
        Local::now().hour() as u8,
        cfg.avatar_night_start_hour,
        cfg.avatar_night_end_hour,
    )
}

fn avatar_window_interval(cfg: &Config) -> Duration {
    if avatar_is_night(cfg) {
        cfg.avatar_night_interval
    } else {
        cfg.avatar_day_interval
    }
}

struct HeartRateWindow {
    started_at: Instant,
    samples: Vec<u8>,
}

impl HeartRateWindow {
    fn new(now: Instant) -> Self {
        Self { started_at: now, samples: Vec::new() }
    }

    fn push(&mut self, bpm: u8) {
        self.samples.push(bpm);
    }

    fn average(&self) -> Option<u8> {
        if self.samples.is_empty() {
            return None;
        }
        let sum: u32 = self.samples.iter().map(|sample| u32::from(*sample)).sum();
        Some(((sum + self.samples.len() as u32 / 2) / self.samples.len() as u32) as u8)
    }

    fn median(&self) -> Option<u8> {
        if self.samples.is_empty() {
            return None;
        }
        let mut sorted = self.samples.clone();
        sorted.sort_unstable();
        let middle = sorted.len() / 2;
        if sorted.len() % 2 == 1 {
            Some(sorted[middle])
        } else {
            Some(((u16::from(sorted[middle - 1]) + u16::from(sorted[middle]) + 1) / 2) as u8)
        }
    }
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
        zone_scheme: avatar::default_zone_scheme().unwrap_or_else(|error| {
            warn!(%error, "using default max-heart-rate zone scheme");
            avatar::ZoneScheme::max_hr(200)
        }),
        face_id: env_u64("NAPCAT_FACE_ID", 0)?,
        face_type: env_u64("NAPCAT_FACE_TYPE", 1)?,
        avatar_enabled: env_or("PB_AVATAR_ENABLED", "true").parse().map_err(|_| "PB_AVATAR_ENABLED must be true or false")?,
        avatar_day_interval: env_positive_duration("PB_AVATAR_DAY_INTERVAL_SEC", DEFAULT_AVATAR_DAY_INTERVAL_SEC)?,
        avatar_night_interval: env_positive_duration("PB_AVATAR_NIGHT_INTERVAL_SEC", DEFAULT_AVATAR_NIGHT_INTERVAL_SEC)?,
        avatar_night_start_hour: env_hour("PB_AVATAR_NIGHT_START_HOUR", DEFAULT_AVATAR_NIGHT_START_HOUR)?,
        avatar_night_end_hour: env_hour("PB_AVATAR_NIGHT_END_HOUR", DEFAULT_AVATAR_NIGHT_END_HOUR)?,
        avatar_jump_threshold_bpm: env_u64("PB_AVATAR_JUMP_THRESHOLD_BPM", DEFAULT_AVATAR_JUMP_THRESHOLD_BPM)?
            .try_into()
            .map_err(|_| "PB_AVATAR_JUMP_THRESHOLD_BPM must be between 0 and 255".to_string())?,
        avatar_jump_cooldown: env_positive_duration("PB_AVATAR_JUMP_COOLDOWN_SEC", DEFAULT_AVATAR_JUMP_COOLDOWN_SEC)?,
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
    let mut live_device_id = None;
    let mut window = None;
    let mut last_published_bpm = None;
    let mut jump_cooldown_until = None;
    let mut last_non_live_state = None;
    let mut ticker = tokio::time::interval(Duration::from_millis(100));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            changed = state_rx.changed() => {
                if changed.is_err() { return Ok(()); }
                state = *state_rx.borrow_and_update();
                match state {
                    State::Live { device_id, bpm } => {
                        let now = Instant::now();
                        if live_device_id != Some(device_id) {
                            live_device_id = Some(device_id);
                            window = None;
                            last_published_bpm = None;
                            jump_cooldown_until = None;
                        }
                        window.get_or_insert_with(|| HeartRateWindow::new(now)).push(bpm);
                        last_non_live_state = None;

                        let jump_allowed = jump_cooldown_until.is_none_or(|until| now >= until);
                        let jumped = last_published_bpm.is_some_and(|last| bpm.abs_diff(last) > cfg.avatar_jump_threshold_bpm);
                        if jump_allowed && jumped {
                            let median = window.take().and_then(|samples| samples.median());
                            jump_cooldown_until = Some(now + cfg.avatar_jump_cooldown);
                            if let Some(median) = median {
                                if last_published_bpm != Some(median) {
                                    last_published_bpm = Some(median);
                                    match upload_avatar(&renderer, &client, &cfg, State::Live { device_id, bpm: median }).await {
                                        Ok(bytes) => info!(bpm = median, bytes, reason = "jump_median", "updated QQ avatar"),
                                        Err(error) => warn!(bpm = median, %error, reason = "jump_median", "QQ avatar update failed"),
                                    }
                                } else {
                                    debug!(bpm = median, reason = "jump_median", "skipped QQ avatar update because heart rate is unchanged");
                                }
                            }
                        }
                    }
                    non_live => {
                        live_device_id = None;
                        window = None;
                        last_published_bpm = None;
                        jump_cooldown_until = None;
                        if last_non_live_state != Some(non_live) {
                            last_non_live_state = Some(non_live);
                            match upload_avatar(&renderer, &client, &cfg, non_live).await {
                                Ok(bytes) => info!(?non_live, bytes, "updated QQ avatar"),
                                Err(error) => warn!(?non_live, %error, "QQ avatar update failed"),
                            }
                        }
                    }
                }
            },
            _ = ticker.tick() => {
                let now = Instant::now();
                if !matches!(state, State::Live { .. }) {
                    if last_non_live_state != Some(state) {
                        last_non_live_state = Some(state);
                        match upload_avatar(&renderer, &client, &cfg, state).await {
                            Ok(bytes) => info!(?state, bytes, "updated QQ avatar"),
                            Err(error) => warn!(?state, %error, "QQ avatar update failed"),
                        }
                    }
                    continue;
                }

                if let Some(samples) = window.as_ref() {
                    if now.duration_since(samples.started_at) >= avatar_window_interval(&cfg) {
                        let average = window.take().and_then(|samples| samples.average());
                        if let (Some(average), State::Live { device_id, .. }) = (average, state) {
                            if last_published_bpm != Some(average) {
                                last_published_bpm = Some(average);
                                match upload_avatar(&renderer, &client, &cfg, State::Live { device_id, bpm: average }).await {
                                    Ok(bytes) => info!(bpm = average, bytes, reason = "window_average", "updated QQ avatar"),
                                    Err(error) => warn!(bpm = average, %error, reason = "window_average", "QQ avatar update failed"),
                                }
                            } else {
                                debug!(bpm = average, reason = "window_average", "skipped QQ avatar update because heart rate is unchanged");
                            }
                        }
                    }
                }
            }
        }
    }
}

async fn upload_avatar(
    renderer: &avatar::AvatarRenderer,
    client: &Client,
    cfg: &Config,
    state: State,
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    let bytes = match state {
        State::Live { bpm, .. } => renderer.render(bpm).await?,
        State::NoData { .. } => renderer.render_no_data().await?,
        State::Offline { .. } | State::Waiting => renderer.render_offline().await?,
    };
    set_avatar(client, cfg, &bytes).await?;
    Ok(bytes.len())
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
                            let text = state.status_text(&cfg.status_format, &cfg.zone_scheme, next_beat);
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
                            let nickname = fill(&cfg.nickname_format, State::heart(beat), zone_for(bpm, &cfg.zone_scheme), bpm);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_calculates_rounded_average_and_median() {
        let mut window = HeartRateWindow::new(Instant::now());
        for bpm in [60, 61, 80] {
            window.push(bpm);
        }
        assert_eq!(window.average(), Some(67));
        assert_eq!(window.median(), Some(61));

        let mut even_window = HeartRateWindow::new(Instant::now());
        even_window.push(60);
        even_window.push(61);
        assert_eq!(even_window.median(), Some(61));
    }

    #[test]
    fn night_window_supports_midnight_wrap() {
        assert!(is_night_hour(23, 23, 7));
        assert!(is_night_hour(0, 23, 7));
        assert!(is_night_hour(6, 23, 7));
        assert!(!is_night_hour(7, 23, 7));
        assert!(!is_night_hour(12, 23, 7));
    }
}
