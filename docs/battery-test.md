# Battery and background-survival measurement

Phase 0 settled the protocol question — Multi-Link works and coexists with
Garmin Connect. What it did not touch is whether an all-day subscription is
affordable, and whether ColorOS lets the bridge live through a night. Those are
now the two things that can still sink this.

**Test C is the more urgent one.** A stream that dies at 02:00 because the
vendor power manager killed the service is a worse failure than one that costs
2 %/hour, and it is also the more likely one.

The bridge now holds a partial CPU wake lock for the lifetime of the foreground
service and reconnects if a connected GATT client produces no heart-rate sample
for 30 seconds. Test C must verify both survival and the resulting phone battery
cost; a foreground-service notification by itself does not keep the CPU awake.

## Test C — does the phone survive the night

Leave the app streaming overnight, screen off, phone not on charge.

In the morning record: phone battery delta, app **uptime**, **ble reconnects**,
**samples**, and whether the dashboard shows an unbroken stream.

For each run also record the App diagnostics: last sample age, last source
event, watchdog recoveries and reason, last successful UDP send, UDP send
failures, and last UDP error. Save the filtered Logcat output for the same
time window. These fields are local diagnostics only and do not change the
telemetry protocol.

| observation | reading |
|---|---|
| uptime matches wall clock, reconnects low | the service survived; done |
| uptime reset | the service was killed and restarted |
| uptime fine, samples far below uptime seconds | link suspended while asleep |
| reconnects in the dozens | link churning, probably the power manager |

Interpret the diagnostic evidence before changing code: a GATT status on a
link-down event indicates a BLE/ColorOS path; a watchdog recovery indicates a
connected GATT link that stopped delivering notifications; a UDP error
indicates DNS/socket/send failure; and a reset uptime with no orderly stop
indicates process or service termination. A UDP receiver restart by itself is
not proof of a sender error because UDP has no delivery handshake.

`samples` should be close to `uptime` in seconds, since the watch streams at
roughly 1 Hz. A large gap is the tell that something throttled the link without
tearing it down.

If it fails, in order: battery optimisation exemption, then ColorOS **App
battery usage → Allow background activity** plus locking the app in recents,
then the vendor auto-start list. Note which one fixed it — that is the
instruction the app should surface.

## Test A — cost of an all-day Multi-Link subscription

1. Charge the watch to 100 %, let the reading settle.
2. Note watch battery % and clock time.
3. Start the bridge on Multi-Link. Confirm the dashboard is live.
4. Two sedentary hours. No activity recording, no GPS.
5. Note watch battery % and time again.

`drain_per_hour = (start − end) / hours`

| measured | verdict |
|---|---|
| ≤ 1.5 %/h | all-day is fine, ship it |
| 1.5–2.5 %/h | ~16 h/day costs 24–40 %; viable if you charge daily |
| > 2.5 %/h | on-demand only, or move all-day duty to a chest strap |

The FR255 baseline is 14 days in smartwatch mode, about **0.3 %/h**.

## Test B — baseline control

The same two hours, ideally the same day, with the bridge stopped. This is what
the watch costs you anyway; subtract it, and the difference is the true price of
the bridge.

Skipping this is the most common way to over-attribute drain to the bridge.

## Test D — Multi-Link versus broadcast

Only worth running once A and C pass. Same two-hour protocol with the source
switched to `BROADCAST`.

The interesting question is whether Multi-Link is *cheaper* than broadcast.
Broadcast forces continuous 1 Hz optical sampling plus a dedicated advertising
mode; Multi-Link rides a connection the watch is already maintaining for Garmin
Connect, so it may well cost less. Nobody has measured this.

## Log

| date | test | source | start % | end % | hours | %/h | reconnects | notes |
|------|------|--------|---------|-------|-------|-----|------------|-------|
| 2026-09-03 | C (partial) | ML | n/a | n/a | 12.06 | n/a | 150 | uptime 12:03:37; samples 17,136; packets 21,795; contact ok; resting HR 52; not a pass |
|      | A    | ML     |         |       |       |     |            |       |
|      | B    | none   |         |       |       |     |            |       |
|      | D    | BC     |         |       |       |     |            |       |

The 2026-09-03 Test C entry is a partial result. The phone's system battery
percentage at the beginning and end was not recorded, so no battery verdict is
possible. OPPO's preceding 24-hour app attribution was 41 mAh foreground
(11:47) and 48 mAh background (17:06); treat those as comparative estimates,
not as a replacement for system battery start/end readings. The observed
sample rate was about 0.395 Hz and reconnects were about 12.4/hour, so this
run remains failed/inconclusive for the P1 stability target and should trigger
ColorOS background, BLE watchdog, and Multi-Link investigation before P3.

## P1 real-device link smoke check — 2026-09-03

This short LAN validation is not a battery or overnight acceptance run. The
phone was observed streaming through Multi-Link with system battery readings
of 43% at the first observation and 46% later; charging was present, so the
delta is not meaningful. Over the observed interval the app showed uptime
`0:00:17` to `0:03:28`, samples `7` to `76`, packets `13` to `81`, and BLE
reconnects remained `1`. Server observations reached `packets=92`,
`last_sequence=92`, and `gaps=0`, with the device online. Dashboard and Live
Embed requests over `192.168.1.3:8080` returned HTTP 200.

The diagnostics-enabled short run started at 21:21:13 and ended at 21:33:02.
The phone remained USB-powered (`level=54%` at the start and `61%` at the
end), so it is not a battery test. The foreground service remained alive while
the App was backgrounded: Android reported `isForeground=true`, BluetoothGatt
notifications continued, and no watchdog, BLE reconnect, or UDP error event
was observed. Server packets advanced from 61 to 396 and the sequence advanced
from 89 to 425; the final snapshot was online with `gaps=1`. Record this as a
successful service-survival/link smoke check with one observed UDP sequence
gap, not as a zero-loss P1 pass.

## Public server log correlation — 2026-09-03

The release Server was built on the public host with its existing Rust 1.97.1
toolchain and deployed to `/opt/pulsebridge`. Caddy remained on
`https://pulse.sighjune.com`, HTTP stayed on `127.0.0.1:8087`, and UDP stayed on
`0.0.0.0:9999`. The service was active after restart; the dedicated log was
`/var/log/pulsebridge/server.log` with daily/100 MB rotation, seven compressed
copies and `copytruncate`. The existing `/etc/pulsebridge/env` was preserved.

The Android client was pointed at `s2.sighjune.com:9999` and produced
`device_id=1552271651`, `session_id=602974534`, `sequence=1..3` in Logcat. The
Server API then reported the same unsigned session and a live snapshot with
`session_id=602974534`, `last_sequence=6`, `gaps=0`; later public snapshots
continued to advance. Server `info` logs also recorded startup, websocket
connect/disconnect, new device/session and a real sequence gap. This was a
short interoperability check, not the planned 10-minute or 24-hour Test C;
the service was restored to `RUST_LOG=pulsebridge_server=info` afterward.

For a packet-level comparison, temporarily override the env-file log level
with a systemd drop-in rather than editing or printing the PSK. Compare the
Android `UdpSender` fields (`session_id`, `sequence`, `timestamp_ms`) with the
Server debug fields (`session_id`, `sequence`, `timestamp_ms`,
`received_at_ms`, `ingest_lag_ms`).

## If the answer is "too expensive"

The broadcast path is a plain Heart Rate Service client, so a chest strap or an
optical armband works with **exactly the same code and no changes** — a coin
cell runs one for months at 1 Hz because that hardware is built for it. The
watch then keeps doing what it is good at, and the bridge keeps working.
