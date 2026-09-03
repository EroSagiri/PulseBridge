# PulseBridge

Real-time heart rate from a Garmin watch to anything that wants it.

```
Garmin Forerunner 255
  │  Multi-Link REAL_TIME_HR   (default, alongside Garmin Connect)
  │  or standard HRS 0x180D    (fallback, needs broadcast mode)
  ▼
Android bridge  ── foreground service, auto-reconnect, send-on-change
  │  UDP + ChaCha20-Poly1305
  ▼
Rust relay      ── replay window, device presence, metric bus
  │
  ├── WebSocket /ws   generic subscriber API
  ├── REST /api/...   point-in-time state
  ├── web dashboard   live BPM
  └── local VRChat bridge ── OSC /chatbox/input ── avatar Chatbox
```

**v1 scope is heart rate only.** The transport, the state store and the
subscriber API are already metric-agnostic, so adding stress / HRV / pace later
is a new `Metric` variant rather than a rewrite. The VRChat integration is kept
as a separate local adapter; Connect IQ, FIT sync and stored history are out.

## Two sources, selectable at runtime

### Multi-Link (default)

Garmin's private channel, measured rather than assumed — see
[docs/phase0-multilink.md](docs/phase0-multilink.md). The bridge attaches a
second GATT client to the link **Garmin Connect is already holding**, registers
the `REAL_TIME_HR` service on an unclaimed lane, and 1 Hz heart rate starts
flowing. No pairing, no authentication, no handshake to reverse engineer.

* nothing to switch on at the watch, and nothing to remember to switch off
* Garmin Connect keeps running and was never observed to drop
* carries resting heart rate as well as the live value
* the same registration mechanism reaches HRV, stress, SpO2 and body battery,
  all of which the FR255 advertises as supported

Two constraints worth knowing. Handles are assigned **at registration time**, so
the decoder dispatches on the handle the watch returned and never on a constant.
And this only covers two apps on one phone — a second phone would need the watch
to accept a second ACL connection, which is a different problem.

### Broadcast (fallback)

Standard Bluetooth SIG Heart Rate Service, which means the watch has to be put
into **Broadcast Heart Rate** mode by hand and the battery cost is real. It is
kept for two reasons: it is the escape hatch if Multi-Link ever stops working,
and it is the same code path a chest strap or an optical armband would use, so
switching to dedicated hardware needs no new code at all.

Pick the source in the app. `MULTILINK` is the default.

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

In the app: paste the host, port and key, pick the source, choose the watch,
then **Start**. Grant the battery optimisation exemption when it offers —
without it the stream dies once the screen has been off for a while.

* **Multi-Link** — press **List paired devices** and pick the watch. It is
  already bonded, so there is nothing to scan for and nothing to do on the
  watch itself.
* **Broadcast** — turn on broadcast at the watch first (hold **UP** →
  **Health & Wellness** → **Wrist Heart Rate** → **Broadcast Heart Rate** →
  **START**), then press **Scan**.

The status card shows the Multi-Link registration state, so a lane conflict
(`already in use`) or an authentication demand is visible instead of looking
like a dead link. If lane 0 is taken, raise the lane number — but note that
Garmin Connect was seen holding lane 1, and writing into its lane is the one
thing that could disturb it.

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
                 "resting_hr": 51, "phone_battery_pct": 77,
                 "packets": 812, "gaps": 3 } ] }
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
When a device is stale or offline `heart_rate` is `null` — deliberately, so no
consumer can mistake the last known value for the current one. `resting_hr` is `null`
when the source does not report one, and unlike the live value it survives the
device going quiet, because it describes the wearer rather than the link.

## Embed Kit

The first Embed Kit layouts reuse the same `/ws` contract and filter one
controlled device in the browser:

```
/embed/{device_id}/minimal
/embed/{device_id}/compact
/embed/{device_id}/card
/embed/{device_id}/live
```

`minimal` is a small heart-rate readout, `compact` is a widget-sized card,
`card` is a larger status card, and `live` is a transparent overlay suitable
for an OBS Browser Source. The Live layout estimates a pulse animation from
`60 / BPM`; it is not synchronized to real RR intervals.

Optional query parameters include `theme=auto|dark|light`,
`transparent=1`, `show_name=0`, `show_status=0`, and `animate=0`. The current
`show_name` value is the temporary label `Device {id}`. Avatar, profile and
heart-rate-zone data are not present yet, so `show_avatar` and `show_zone` do
not invent or display those fields.

The current target is a numeric `device_id` for local or otherwise controlled
use. It is not a public identity or privacy boundary; do not expose arbitrary
device IDs on the public internet until Profile and Visibility are implemented.

## VRChat local bridge

The VRChat adapter is an independent project in `vrchat-bridge/`. It consumes
the same typed WebSocket contract as any other subscriber and sends OSC only to
the local VRChat client. The relay server has no VRChat-specific dependency or
behavior.

```powershell
# First enable OSC in VRChat: Action Menu → Options → OSC → Enabled
cd vrchat-bridge
cargo run --release
```

Defaults connect to `ws://127.0.0.1:8080/ws` and send to VRChat at
`127.0.0.1:9000`. See [vrchat-bridge/README.md](vrchat-bridge/README.md) for
remote-server, device-selection, custom text and refresh settings. For example,
set `PB_VRCHAT_TEXT_FORMAT="{}BPM"` to display `72BPM`, or use `{:03}`
instead of `{}` to keep the BPM field three characters wide with leading zeroes.

## Layout

```
protocol/protocol.md      the wire format; the contract between the two sides
server/src/protocol.rs    codec + replay window, with the spec test vector
server/src/state.rs       device presence and the metric bus
server/src/http.rs        WebSocket and REST subscribers
server/src/bin/           simulator
shared/pulsebridge-api/   typed subscriber contract shared by server and clients
vrchat-bridge/            standalone WebSocket → local VRChat OSC adapter
android/…/garmin/         Multi-Link framing and GATT client
android/…/ble/            standard Heart Rate Service client
android/…/service/        foreground service, source selection, UDP
tools/mltest/             Phase 0 probe: dumps the GATT table, runs registration
docs/phase0-multilink.md  the coexistence experiment and the raw captures
docs/battery-test.md      the measurement still outstanding
```

The Multi-Link frame formats are pinned by unit tests against the exact bytes
captured from the watch (`android/app/src/test/…/MultiLinkTest.kt`), so a
regression in the parser or a change on the watch fails the build rather than
showing up as a silently wrong heart rate.

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

* Multi-Link coexistence with Garmin Connect: **proven on hardware**, see
  docs/phase0-multilink.md. Registration and frame parsing are covered by unit
  tests against the captured bytes.
* Server pipeline: verified end to end with the simulator, UDP → WebSocket →
  dashboard, including decay to offline.
* The Android bridge uses a `connectedDevice` foreground service, holds a
  partial CPU wake lock while streaming, and force-reconnects a GATT link that
  stays connected but delivers no heart-rate notification for 30 seconds.
  Unit tests and a successful build cover the recovery policy, but a full
  screen-off overnight run is still required.

Outstanding, in order of how much they can still sink this:

1. Whether the wake-lock plus silent-stream watchdog survives screen-off on
   ColorOS for 24 h without an unacceptable phone battery cost.
2. All-day battery cost of a Multi-Link subscription — unmeasured.
3. Close-handle message format; the client currently just detaches.
4. Whether lane 0 stays free after a watch reboot or a Connect firmware sync.
