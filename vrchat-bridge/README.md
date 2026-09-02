# PulseBridge VRChat Bridge

Local adapter from the PulseBridge WebSocket API to VRChat's OSC Chatbox input.
It is a separate process from `pulsebridge-server`: the two projects share only
the data contract in `../shared/pulsebridge-api`.

## Run

1. Start `pulsebridge-server` and confirm its dashboard shows a live heart rate.
2. In VRChat, open **Action Menu → Options → OSC → Enabled**.
3. On the same PC as VRChat, run:

```powershell
cd vrchat-bridge
cargo run --release
```

The default route is:

```text
PulseBridge ws://127.0.0.1:8080/ws
  → pulsebridge-vrchat-bridge
  → OSC UDP 127.0.0.1:9000
  → VRChat /chatbox/input
```

The bridge shows `♥ 72 BPM` above the avatar, sends without notification sound,
refreshes the bubble every 5 seconds, and replaces stale or disconnected data
with `♥ -- BPM · no signal`. Heart-rate changes are throttled to at most one
chatbox update every 1.1 seconds.

## Configuration

| variable | default | meaning |
|---|---|---|
| `PB_SERVER_WS` | `ws://127.0.0.1:8080/ws` | PulseBridge subscriber endpoint; `wss://` is supported |
| `PB_DEVICE_ID` | auto | fixed source device; otherwise the first online device is selected and kept |
| `PB_VRCHAT_OSC_ADDR` | `127.0.0.1:9000` | VRChat OSC input address |
| `PB_VRCHAT_TEXT_FORMAT` | `♥ {} BPM` | live Chatbox text containing exactly one BPM placeholder |
| `PB_VRCHAT_MIN_INTERVAL_MS` | `1100` | minimum interval between Chatbox sends; cannot be below 1000 |
| `PB_VRCHAT_REFRESH_MS` | `5000` | resend interval so the Chatbox remains visible |
| `RUST_LOG` | `info` | log filter, such as `debug` |

Example for a PulseBridge server on another machine:

```powershell
$env:PB_SERVER_WS = "ws://192.168.1.20:8080/ws"
$env:PB_DEVICE_ID = "1"
$env:PB_VRCHAT_TEXT_FORMAT = "[HR] {}BPM"
cargo run --release
```

VRChat receives OSC over UDP and does not acknowledge messages. The bridge can
confirm that packets were sent, but the visible bubble is the final end-to-end
check.

### Heart-rate text formatting

`PB_VRCHAT_TEXT_FORMAT` supports a small, validated subset of Rust-style number
formatting:

| placeholder | BPM 55 | meaning |
|---|---|---|
| `{}` | `55` | no padding |
| `{:3}` or `{:>3}` | ` 55` | left-pad with spaces to width 3 |
| `{:03}` or `{:0>3}` | `055` | left-pad with zeroes to width 3 |
| `{:<3}` | `55 ` | right-pad with spaces to width 3 |
| `{:^4}` | ` 55 ` | center with spaces in width 4 |

For a stable-width Garmin label in VRChat:

```powershell
$env:PB_VRCHAT_TEXT_FORMAT = "♥ {:03} BPM · Garmin Watch"
cargo run --release
```
