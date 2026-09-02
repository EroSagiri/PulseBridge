# PulseBridge

Real-time heart rate from a Garmin watch to anything that wants it.

```
Garmin Forerunner 255
  │  standard BLE Heart Rate Service (0x180D / 0x2A37)
  ▼
Android bridge  ── foreground service, auto-reconnect, send-on-change
  │  UDP + ChaCha20-Poly1305
  ▼
Rust relay      ── replay window, device presence, metric bus
  │
  ├── WebSocket /ws   generic subscriber API
  ├── REST /api/...   point-in-time state
  └── web dashboard   live BPM
```

**v1 scope is heart rate only.** The transport, the state store and the
subscriber API are already metric-agnostic, so adding stress / HRV / pace later
is a new `Metric` variant rather than a rewrite. Everything else from the
original plan — Garmin private protocol, Connect IQ, VRChat, history — is
deliberately out.

## Why broadcast mode and not the Garmin private protocol

The watch is put into standard **Broadcast Heart Rate** mode, so it advertises
the Bluetooth SIG Heart Rate Service like any chest strap does. That buys:

* no reverse engineering, no Garmin Connect conflict, no Connect IQ app
* ~150 lines of completely standard Android BLE code
* the watch stays usable — nothing takes over the screen
* the same Android code works unchanged with a chest strap or an armband,
  which is the fallback if watch battery turns out to be unacceptable

The cost is that broadcast mode has to be switched on by hand on the watch and
that it drains the battery noticeably. See [docs/battery-test.md](docs/battery-test.md).

## Quick start

### 1. Generate a key

```bash
openssl rand -hex 32
```

The same 64-character value goes into the server environment and into the
Android app. It is the only thing standing between your heart rate and the open
internet, so do not ship the development default.

### 2. Run the server

```bash
cd server && PB_KEY=<your-64-hex-key> cargo run --release
```

| variable | default | meaning |
|---|---|---|
| `PB_UDP_ADDR` | `0.0.0.0:9999` | where telemetry arrives |
| `PB_HTTP_ADDR` | `0.0.0.0:8080` | dashboard + API |
| `PB_WEB_DIR` | `web` | static files |
| `PB_KEY` | dev default | 32-byte pre-shared key, hex |

Open <http://localhost:8080>.

### 3. Prove the pipeline without a watch

```bash
cd server && cargo run --bin simulator -- 127.0.0.1:9999
```

The dashboard should show a device whose heart rate wanders realistically. Kill
the simulator and the card must fall to `--` and `offline` within a minute —
never keep showing a stale number as if it were current.

### 4. Build and install the Android app

```bash
cd android && ./gradlew assembleDebug
```

`app/build/outputs/apk/debug/app-debug.apk`. Requires Android 9 (API 28) or
newer; `ChaCha20-Poly1305` in `javax.crypto` is what sets that floor.

In the app: paste the host, port and key, **Scan** while the watch is
broadcasting, tap the watch in the list, then **Start**. Grant the battery
optimisation exemption when it offers — without it the stream dies once the
screen has been off for a while.

### 5. On the watch

Hold **UP** → **Health & Wellness** → **Wrist Heart Rate** → **Broadcast Heart
Rate** → **START**.

## Subscriber API

Everything downstream — the bundled dashboard, a VRChat OSC bridge, Home
Assistant — is expected to use the WebSocket and nothing else.

```
ws://host:8080/ws
```

On connect, and every 2 s afterwards:

```json
{ "type": "snapshot",
  "devices": [ { "device_id": 1, "presence": "online", "age_ms": 340,
                 "heart_rate": 72, "contact_ok": true, "watch_connected": true,
                 "phone_battery_pct": 77, "packets": 812, "gaps": 3 } ] }
```

And on every change:

```json
{ "type": "metric",
  "event": { "device_id": 1, "timestamp_ms": 1756800000000,
             "metric": "heart_rate", "bpm": 73, "contact_ok": true } }
```

The periodic snapshot exists because presence decays with wall-clock time, not
with packets: without it a client could never learn that a device went away.

REST, for consumers that only want a point-in-time answer:

```
GET /api/devices
GET /api/device/:id
```

`presence` is `online` under 15 s, `stale` under 60 s, `offline` beyond that.
When a device is offline `heart_rate` is `null` — deliberately, so no consumer
can mistake the last known value for the current one.

## Layout

```
protocol/protocol.md   the wire format; the only contract between the two sides
server/src/protocol.rs codec + replay window, with the spec test vector
server/src/state.rs    device presence and the metric bus
server/src/http.rs     WebSocket and REST subscribers
server/src/bin/        simulator
android/               Kotlin bridge: pairing UI, BLE client, UDP sender
docs/battery-test.md   the measurement that decides whether this is viable
```

## Security notes

* The 24-byte header is plaintext but authenticated as AEAD associated data, so
  it is readable in a packet dump and still not forgeable.
* Nonces are derived, never random: `device_id || session_id || sequence`.
  `session_id` is re-randomised on app start, which is what makes a sequence
  reset after a crash safe.
* The server never trusts the UDP source address. A device is rebound to a new
  address only after a packet from it has passed authentication and the replay
  window, which is what makes Wi-Fi ↔ mobile handover work with no handshake.
* Replay protection is a 64-entry sliding window, not a monotonic counter —
  UDP reorders, and a monotonic check would drop legitimate packets.

## Status

Verified end to end with the simulator: UDP → server → WebSocket → dashboard,
including presence decay to offline. The Android app builds; it has **not**
been run against a real watch yet. That is the next step, together with the
battery measurement.
