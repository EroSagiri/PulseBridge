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

| observation | reading |
|---|---|
| uptime matches wall clock, reconnects low | the service survived; done |
| uptime reset | the service was killed and restarted |
| uptime fine, samples far below uptime seconds | link suspended while asleep |
| reconnects in the dozens | link churning, probably the power manager |

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
|      | C    | ML     |         |       |       |     |            |       |
|      | A    | ML     |         |       |       |     |            |       |
|      | B    | none   |         |       |       |     |            |       |
|      | D    | BC     |         |       |       |     |            |       |

## If the answer is "too expensive"

The broadcast path is a plain Heart Rate Service client, so a chest strap or an
optical armband works with **exactly the same code and no changes** — a coin
cell runs one for months at 1 Hz because that hardware is built for it. The
watch then keeps doing what it is good at, and the bridge keeps working.
