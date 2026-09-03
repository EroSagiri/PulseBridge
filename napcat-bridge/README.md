# PulseBridge NapCat Bridge

Consumes the local PulseBridge WebSocket and updates NapCat's custom online
status with the current heart rate.

The default minimum interval is zero: every changed state received from
PulseBridge is sent to NapCat immediately. PulseBridge itself only emits a
metric event when the heart-rate value changes. The heart icon alternates on
each update to simulate a beat.

Environment variables:

| variable | default |
|---|---|
| `PB_SERVER_WS` | `ws://127.0.0.1:8087/ws` |
| `NAPCAT_API_URL` | `http://127.0.0.1:3000` |
| `NAPCAT_ACCESS_TOKEN` | unset |
| `PB_DEVICE_ID` | auto-select first online device |
| `PB_STATUS_MIN_INTERVAL_MS` | `0` |
| `PB_MAX_HR` | `201` |
| `PB_STATUS_FORMAT` | `{heart} {zone} · {bpm} BPM` |

Zones use maximum-heart-rate percentages: Z1 ≤60%, Z2 ≤70%, Z3 ≤80%,
Z4 ≤90%, and Z5 above 90%. With `PB_MAX_HR=201`, the upper boundaries are
121, 141, 161, and 181 BPM. Lactate threshold is not used in this mode.
| `NAPCAT_FACE_ID` | `0` |
| `NAPCAT_FACE_TYPE` | `1` |
