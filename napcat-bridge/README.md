# PulseBridge NapCat Bridge

Consumes the local PulseBridge WebSocket and updates NapCat's QQ nickname with
the current heart rate. The default nickname is `June - 💓72`; it changes when
the heart-rate value changes and returns to `June` when the source is offline.
The custom online status remains available as a fallback.

The bridge can also render a JPEG heart-rate avatar in a dedicated renderer
thread. It reads the fixed asset manifest at
`/opt/pulsebridge/assets/heart-rate/avatar.json`, prepares a 1280×1280 master,
and caches rendered BPM values on demand. The manifest describes the background,
font, text region, rotation, fill, outline, and shadow. Its relative
`background.png` path is resolved beside the manifest.

For local preview/testing, build and run the separate Rust CLI:

```text
cargo run --bin pulsebridge-avatar -- assets/heart-rate/avatar.json \
  --bpm 66,80,180 --count 60 --size 320 --quality 50 --output avatar-preview
cargo run --bin pulsebridge-avatar -- assets/heart-rate/avatar.json \
  --bpm 66,80,180 --count 3 --size 320 --max-bytes 10k --output avatar-preview-10k
```

`--quality` and `--max-bytes` are mutually exclusive. If neither is supplied,
quality mode defaults to 50. In size mode the CLI chooses the highest JPEG
quality that stays at or below the requested limit. BPM values are cycled when
`--count` is larger than the list; every output is rendered from the master and
written as a uniquely numbered `.jpg`. `k`/`m` suffixes use decimal units;
`ki`/`mi` (or `kib`/`mib`) use binary units.

The default minimum interval is zero: every changed state received from
PulseBridge is sent to NapCat immediately. PulseBridge itself only emits a
metric event when the heart-rate value changes. The heart icon alternates on
each update to simulate a beat.

QQ profile/nickname changes are rate-limited by default to once per minute.
Only the newest heart-rate state is considered when that interval expires; a
failed nickname request is not retried for the same state. Configure this with
`PB_NICKNAME_MIN_INTERVAL_MS` if needed. NapCat does not document a fixed QQ
nickname-change quota, so aggressive intervals may trigger QQ account risk
controls.

Environment variables:

| variable | default |
|---|---|
| `PB_SERVER_WS` | `ws://127.0.0.1:8087/ws` |
| `NAPCAT_API_URL` | `http://127.0.0.1:3000` |
| `NAPCAT_ACCESS_TOKEN` | unset |
| `PB_DEVICE_ID` | auto-select first online device |
| `PB_STATUS_MIN_INTERVAL_MS` | `0` |
| `PB_NICKNAME_MIN_INTERVAL_MS` | `60000` |
| `PB_MAX_HR` | `201` |
| `PB_STATUS_FORMAT` | `{heart} {zone} · {bpm} BPM` |
| `PB_NICKNAME_FORMAT` | `June - 💓{bpm}` |
| `PB_NICKNAME_IDLE` | `June` |
| `PB_AVATAR_ENABLED` | `true` |
| `PB_AVATAR_MIN_INTERVAL_MS` | `1000` |
| `PB_AVATAR_SIZE` | `320` |
| `PB_AVATAR_JPEG_QUALITY` | `50` |
| `PB_AVATAR_MAX_BYTES` | unset; e.g. `10k` |

Zones use maximum-heart-rate percentages: Z1 ≤60%, Z2 ≤70%, Z3 ≤80%,
Z4 ≤90%, and Z5 above 90%. With `PB_MAX_HR=201`, the upper boundaries are
121, 141, 161, and 181 BPM. Lactate threshold is not used in this mode.
| `NAPCAT_FACE_ID` | `0` |
| `NAPCAT_FACE_TYPE` | `1` |

When the selected device is online without a heart-rate sample, the avatar
uses a cached `--` image. When the device is offline or the PulseBridge
WebSocket is disconnected, it uses a cached `OFF` image.
