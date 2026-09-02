use std::net::SocketAddr;
use std::time::Duration;

use futures_util::StreamExt;
use pulsebridge_api::{DeviceSnapshot, Metric, MetricEvent, Presence, ServerMessage};
use tokio::net::UdpSocket;
use tokio::sync::watch;
use tokio::time::Instant;
use tokio_tungstenite::connect_async;
use tracing::{debug, info, warn};
use tracing_subscriber::EnvFilter;

const DEFAULT_SERVER_WS: &str = "ws://127.0.0.1:8080/ws";
const DEFAULT_OSC_ADDR: &str = "127.0.0.1:9000";
const DEFAULT_MIN_INTERVAL_MS: u64 = 1_100;
const DEFAULT_REFRESH_MS: u64 = 5_000;
const DEFAULT_TEXT_FORMAT: &str = "♥ {} BPM";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Alignment {
    Left,
    Right,
    Center,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LiveTextFormat {
    prefix: String,
    suffix: String,
    width: Option<usize>,
    fill: char,
    alignment: Alignment,
}

impl LiveTextFormat {
    fn parse(template: &str) -> Result<Self, String> {
        let open = template
            .find('{')
            .ok_or_else(|| "PB_VRCHAT_TEXT_FORMAT must contain a BPM placeholder".to_string())?;
        let close = template[open + 1..]
            .find('}')
            .map(|offset| open + 1 + offset)
            .ok_or_else(|| "PB_VRCHAT_TEXT_FORMAT has an unclosed placeholder".to_string())?;

        let prefix = &template[..open];
        let field = &template[open + 1..close];
        let suffix = &template[close + 1..];
        if prefix.contains(['{', '}']) || suffix.contains(['{', '}']) {
            return Err(
                "PB_VRCHAT_TEXT_FORMAT must contain exactly one BPM placeholder".to_string(),
            );
        }

        let (width, fill, alignment) = match field {
            "" => (None, ' ', Alignment::Right),
            _ if field.starts_with(':') => parse_field_spec(&field[1..])?,
            _ => {
                return Err(format!(
                    "unsupported PB_VRCHAT_TEXT_FORMAT placeholder {{{field}}}"
                ))
            }
        };

        Ok(Self {
            prefix: prefix.to_string(),
            suffix: suffix.to_string(),
            width,
            fill,
            alignment,
        })
    }

    fn render(&self, bpm: u8) -> String {
        let value = bpm.to_string();
        let Some(width) = self.width else {
            return format!("{}{}{}", self.prefix, value, self.suffix);
        };
        let padding = width.saturating_sub(value.len());
        let (left, right) = match self.alignment {
            Alignment::Left => (0, padding),
            Alignment::Right => (padding, 0),
            Alignment::Center => (padding / 2, padding - padding / 2),
        };
        format!(
            "{}{}{}{}{}",
            self.prefix,
            self.fill.to_string().repeat(left),
            value,
            self.fill.to_string().repeat(right),
            self.suffix
        )
    }
}

fn parse_field_spec(spec: &str) -> Result<(Option<usize>, char, Alignment), String> {
    if spec.is_empty() {
        return Err("PB_VRCHAT_TEXT_FORMAT width is missing after ':'".to_string());
    }

    let (fill, alignment, width_text) = if let Some(width) = spec.strip_prefix("0>") {
        ('0', Alignment::Right, width)
    } else if let Some(width) = spec.strip_prefix('>') {
        (' ', Alignment::Right, width)
    } else if let Some(width) = spec.strip_prefix('<') {
        (' ', Alignment::Left, width)
    } else if let Some(width) = spec.strip_prefix('^') {
        (' ', Alignment::Center, width)
    } else if spec.starts_with('0') {
        ('0', Alignment::Right, spec)
    } else {
        (' ', Alignment::Right, spec)
    };

    let width = width_text
        .parse::<usize>()
        .map_err(|_| format!("unsupported PB_VRCHAT_TEXT_FORMAT field specifier {{:{spec}}}"))?;
    if !(1..=32).contains(&width) {
        return Err("PB_VRCHAT_TEXT_FORMAT width must be between 1 and 32".to_string());
    }
    Ok((Some(width), fill, alignment))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DisplayState {
    Waiting,
    Live { device_id: u32, bpm: u8 },
    NoSignal { device_id: Option<u32> },
}

impl DisplayState {
    fn text(self, live_format: &LiveTextFormat) -> String {
        match self {
            DisplayState::Waiting => "♥ -- BPM · connecting".to_string(),
            DisplayState::Live { bpm, .. } => live_format.render(bpm),
            DisplayState::NoSignal { .. } => "♥ -- BPM · no signal".to_string(),
        }
    }
}

struct DeviceSelector {
    configured: Option<u32>,
    active: Option<u32>,
}

impl DeviceSelector {
    fn new(configured: Option<u32>) -> Self {
        Self {
            configured,
            active: configured,
        }
    }

    fn snapshot(&mut self, devices: &[DeviceSnapshot]) -> DisplayState {
        let selected = if let Some(device_id) = self.configured {
            Some(device_id)
        } else {
            let active_is_online = self.active.is_some_and(|active| {
                devices.iter().any(|device| {
                    device.device_id == active
                        && device.presence == Presence::Online
                        && device.heart_rate.is_some()
                })
            });

            if active_is_online {
                self.active
            } else {
                devices
                    .iter()
                    .filter(|device| {
                        device.presence == Presence::Online && device.heart_rate.is_some()
                    })
                    .map(|device| device.device_id)
                    .min()
            }
        };

        self.active = selected;
        let Some(device_id) = selected else {
            return DisplayState::NoSignal { device_id: None };
        };

        match devices.iter().find(|device| device.device_id == device_id) {
            Some(device) if device.presence == Presence::Online => match device.heart_rate {
                Some(bpm) => DisplayState::Live { device_id, bpm },
                None => DisplayState::NoSignal {
                    device_id: Some(device_id),
                },
            },
            _ => DisplayState::NoSignal {
                device_id: Some(device_id),
            },
        }
    }

    fn metric(&mut self, event: &MetricEvent) -> Option<DisplayState> {
        if self
            .configured
            .is_some_and(|device_id| device_id != event.device_id)
        {
            return None;
        }
        if self.configured.is_none() {
            match self.active {
                Some(active) if active != event.device_id => return None,
                None => self.active = Some(event.device_id),
                _ => {}
            }
        }

        match &event.metric {
            Metric::HeartRate { bpm, .. } => Some(DisplayState::Live {
                device_id: event.device_id,
                bpm: *bpm,
            }),
        }
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn env_u64(key: &str, default: u64) -> Result<u64, String> {
    match std::env::var(key) {
        Ok(value) => value
            .parse::<u64>()
            .map_err(|_| format!("{key} must be a non-negative integer, got {value:?}")),
        Err(_) => Ok(default),
    }
}

fn env_device_id() -> Result<Option<u32>, String> {
    match std::env::var("PB_DEVICE_ID") {
        Ok(value) => value
            .parse::<u32>()
            .map(Some)
            .map_err(|_| format!("PB_DEVICE_ID must be a u32, got {value:?}")),
        Err(_) => Ok(None),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    // tokio-tungstenite intentionally leaves Rustls' process-wide crypto
    // provider unselected. Install one explicitly before the first wss://
    // connection so TLS never depends on feature-unification accidents.
    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| "failed to install the Rustls ring crypto provider")?;

    let server_ws = env_or("PB_SERVER_WS", DEFAULT_SERVER_WS);
    let osc_addr: SocketAddr = env_or("PB_VRCHAT_OSC_ADDR", DEFAULT_OSC_ADDR).parse()?;
    let text_format = LiveTextFormat::parse(&env_or("PB_VRCHAT_TEXT_FORMAT", DEFAULT_TEXT_FORMAT))?;
    let device_id = env_device_id()?;
    let min_interval_ms = env_u64("PB_VRCHAT_MIN_INTERVAL_MS", DEFAULT_MIN_INTERVAL_MS)?;
    let refresh_ms = env_u64("PB_VRCHAT_REFRESH_MS", DEFAULT_REFRESH_MS)?;
    if min_interval_ms < 1_000 {
        return Err("PB_VRCHAT_MIN_INTERVAL_MS must be at least 1000".into());
    }
    if refresh_ms < min_interval_ms {
        return Err("PB_VRCHAT_REFRESH_MS must be >= PB_VRCHAT_MIN_INTERVAL_MS".into());
    }

    info!(
        server_ws,
        %osc_addr,
        device_id = ?device_id,
        "VRChat heart-rate bridge starting"
    );

    let (state_tx, state_rx) = watch::channel(DisplayState::Waiting);
    let source = tokio::spawn(source_loop(server_ws, device_id, state_tx));
    let osc = tokio::spawn(osc_loop(
        state_rx,
        osc_addr,
        Duration::from_millis(min_interval_ms),
        Duration::from_millis(refresh_ms),
        text_format,
    ));

    tokio::select! {
        result = source => {
            result?;
            Err("heart-rate source task stopped unexpectedly".into())
        }
        result = osc => {
            result??;
            Err("OSC sender task stopped unexpectedly".into())
        }
        result = tokio::signal::ctrl_c() => {
            result?;
            info!("shutdown requested");
            Ok(())
        }
    }
}

async fn source_loop(
    server_ws: String,
    device_id: Option<u32>,
    state_tx: watch::Sender<DisplayState>,
) {
    let mut retry_secs = 1u64;
    loop {
        match connect_async(&server_ws).await {
            Ok((stream, _)) => {
                info!(%server_ws, "connected to PulseBridge WebSocket");
                retry_secs = 1;
                let mut selector = DeviceSelector::new(device_id);
                let (_, mut incoming) = stream.split();

                while let Some(message) = incoming.next().await {
                    match message {
                        Ok(message) if message.is_text() => {
                            let text = match message.to_text() {
                                Ok(text) => text,
                                Err(error) => {
                                    debug!(%error, "ignored invalid WebSocket text frame");
                                    continue;
                                }
                            };
                            match serde_json::from_str::<ServerMessage>(text) {
                                Ok(ServerMessage::Snapshot { devices }) => {
                                    state_tx.send_replace(selector.snapshot(&devices));
                                }
                                Ok(ServerMessage::Metric { event }) => {
                                    if let Some(state) = selector.metric(&event) {
                                        state_tx.send_replace(state);
                                    }
                                }
                                Err(error) => debug!(%error, "ignored unknown server message"),
                            }
                        }
                        Ok(message) if message.is_close() => break,
                        Ok(_) => {}
                        Err(error) => {
                            warn!(%error, "PulseBridge WebSocket receive failed");
                            break;
                        }
                    }
                }
                warn!(%server_ws, "PulseBridge WebSocket disconnected");
            }
            Err(error) => warn!(%server_ws, %error, "cannot connect to PulseBridge WebSocket"),
        }

        state_tx.send_replace(DisplayState::NoSignal { device_id });
        tokio::time::sleep(Duration::from_secs(retry_secs)).await;
        retry_secs = (retry_secs * 2).min(10);
    }
}

async fn osc_loop(
    mut state_rx: watch::Receiver<DisplayState>,
    osc_addr: SocketAddr,
    min_interval: Duration,
    refresh: Duration,
    text_format: LiveTextFormat,
) -> Result<(), std::io::Error> {
    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    socket.connect(osc_addr).await?;
    info!(%osc_addr, "OSC sender ready; enable OSC in VRChat's Action Menu");

    let mut state = *state_rx.borrow_and_update();
    let mut last_sent_state = None;
    let mut last_sent_at = Instant::now()
        .checked_sub(refresh)
        .unwrap_or_else(Instant::now);
    let mut ticker = tokio::time::interval(Duration::from_millis(100));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            changed = state_rx.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
                state = *state_rx.borrow_and_update();
            }
            _ = ticker.tick() => {
                let now = Instant::now();
                let changed = last_sent_state != Some(state);
                let needs_refresh = now.duration_since(last_sent_at) >= refresh;
                let allowed = now.duration_since(last_sent_at) >= min_interval;
                if allowed && (changed || needs_refresh) {
                    let text = state.text(&text_format);
                    let packet = encode_chatbox_input(&text);
                    socket.send(&packet).await?;
                    info!(message = %text, "sent VRChat chatbox update");
                    last_sent_state = Some(state);
                    last_sent_at = now;
                }
            }
        }
    }
}

/// OSC `/chatbox/input s b n`: text, send immediately, no notification sound.
fn encode_chatbox_input(text: &str) -> Vec<u8> {
    let text: String = text.chars().filter(|ch| *ch != '\0').take(144).collect();
    let mut packet = Vec::with_capacity(64);
    push_osc_string(&mut packet, "/chatbox/input");
    // OSC bool values are encoded in the type tag and carry no value bytes.
    push_osc_string(&mut packet, ",sTF");
    push_osc_string(&mut packet, &text);
    packet
}

fn push_osc_string(packet: &mut Vec<u8>, value: &str) {
    packet.extend_from_slice(value.as_bytes());
    packet.push(0);
    while packet.len() % 4 != 0 {
        packet.push(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(device_id: u32, presence: Presence, heart_rate: Option<u8>) -> DeviceSnapshot {
        DeviceSnapshot {
            device_id,
            presence,
            age_ms: 0,
            heart_rate,
            resting_hr: None,
            contact_ok: true,
            watch_connected: true,
            phone_battery_pct: None,
            session_id: 1,
            packets: 1,
            last_sequence: 1,
            gaps: 0,
        }
    }

    #[test]
    fn chatbox_packet_has_expected_osc_layout() {
        let packet = encode_chatbox_input("♥ 72 BPM");
        assert_eq!(&packet[0..16], b"/chatbox/input\0\0");
        assert_eq!(&packet[16..24], b",sTF\0\0\0\0");
        assert_eq!(&packet[24..], "♥ 72 BPM\0\0".as_bytes());
        assert_eq!(packet.len() % 4, 0);
    }

    #[test]
    fn live_text_uses_runtime_format() {
        let state = DisplayState::Live {
            device_id: 1,
            bpm: 72,
        };
        assert_eq!(
            state.text(&LiveTextFormat::parse("{}BPM").unwrap()),
            "72BPM"
        );
        assert_eq!(
            state.text(&LiveTextFormat::parse("[HR] ♥ {} BPM").unwrap()),
            "[HR] ♥ 72 BPM"
        );
    }

    #[test]
    fn live_text_supports_space_and_zero_padding() {
        let state = DisplayState::Live {
            device_id: 1,
            bpm: 55,
        };
        assert_eq!(
            state.text(&LiveTextFormat::parse("♥ {:3} BPM").unwrap()),
            "♥  55 BPM"
        );
        assert_eq!(
            state.text(&LiveTextFormat::parse("♥ {:03} BPM").unwrap()),
            "♥ 055 BPM"
        );
        assert_eq!(
            state.text(&LiveTextFormat::parse("♥ {:0>3} BPM").unwrap()),
            "♥ 055 BPM"
        );
    }

    #[test]
    fn live_text_supports_left_and_center_alignment() {
        let state = DisplayState::Live {
            device_id: 1,
            bpm: 5,
        };
        assert_eq!(
            state.text(&LiveTextFormat::parse("[{:<3}]").unwrap()),
            "[5  ]"
        );
        assert_eq!(
            state.text(&LiveTextFormat::parse("[{:^3}]").unwrap()),
            "[ 5 ]"
        );
    }

    #[test]
    fn invalid_runtime_formats_are_rejected() {
        assert!(LiveTextFormat::parse("no bpm here").is_err());
        assert!(LiveTextFormat::parse("{} {}").is_err());
        assert!(LiveTextFormat::parse("{:abc}").is_err());
        assert!(LiveTextFormat::parse("{:0}").is_err());
    }

    #[test]
    fn configured_device_does_not_fall_through_to_another_device() {
        let devices = vec![
            snapshot(1, Presence::Online, Some(70)),
            snapshot(2, Presence::Offline, None),
        ];
        let mut selector = DeviceSelector::new(Some(2));
        assert_eq!(
            selector.snapshot(&devices),
            DisplayState::NoSignal { device_id: Some(2) }
        );
    }

    #[test]
    fn auto_selection_is_sticky_while_device_is_online() {
        let mut selector = DeviceSelector::new(None);
        let first = vec![
            snapshot(2, Presence::Online, Some(82)),
            snapshot(3, Presence::Online, Some(93)),
        ];
        assert_eq!(
            selector.snapshot(&first),
            DisplayState::Live {
                device_id: 2,
                bpm: 82
            }
        );

        let next = vec![
            snapshot(1, Presence::Online, Some(71)),
            snapshot(2, Presence::Online, Some(83)),
        ];
        assert_eq!(
            selector.snapshot(&next),
            DisplayState::Live {
                device_id: 2,
                bpm: 83
            }
        );
    }

    #[test]
    fn parses_the_shared_metric_contract() {
        let message = r#"{"type":"metric","event":{"device_id":7,"timestamp_ms":1,"metric":"heart_rate","bpm":99,"contact_ok":true}}"#;
        match serde_json::from_str::<ServerMessage>(message).unwrap() {
            ServerMessage::Metric { event } => {
                assert_eq!(event.device_id, 7);
                assert_eq!(
                    event.metric,
                    Metric::HeartRate {
                        bpm: 99,
                        contact_ok: true
                    }
                );
            }
            _ => panic!("wrong message variant"),
        }
    }
}
