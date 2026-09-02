# What the Multi-Link channel actually carries

Every service the FR255 advertises was registered on lane 0 and observed for 60 s at rest
on 2026-09-02. Field meanings were established by matching live frame bytes against the
same day's Garmin Connect figures — not guessed from names.

All 19 advertised services registered with `SUCCESS`. None required authentication.
Frames arrive as `<ml_handle> <payload>`; the handle comes from the registration response.

## Confirmed

| svc | name | payload | rate | cross-check |
|---|---|---|---|---|
| 6 | REAL_TIME_HR | `03 <hr> <resting_hr> ff ff` | ~0.5–1 Hz | resting 51 = Connect's `resting_heart_rate_bpm` |
| 12 | REAL_TIME_HRV | `<rr_ms u16> <ts u32>` | **one frame per heartbeat** | 78 frames/60 s at 78 bpm; RR 722–793 ms |
| 13 | REAL_TIME_STRESS | `<stress u16> <? u16>` | ~0.3 Hz | 51–65, within Connect's avg 30 / max 90 |
| 19 | REAL_TIME_SPO2 | `<spo2> <seq> <ts u24>` | 1 Hz | 99 %, plausible vs avg 95 / low 86 |
| 20 | REAL_TIME_BODY_BATTERY | `<bb>` | on change | 0x25 = 37 vs Connect's `body_battery_current` 38 |
| 21 | REAL_TIME_RESPIRATION | `<brpm>` | ~0.13 Hz | 15–17 vs `avg_waking_respiration` 15.0 |
| 7 | REAL_TIME_STEPS | `<steps u32> <goal u32>` | on change | **6444 / 14030 — exact match** |
| 8 | REAL_TIME_CALORIES | `<total u32> <active u32>` | ~1/min | active **76 exact**; total runs ahead of the last sync |
| 9 | REAL_TIME_FLOORS | `<up u16> <down u16> <goal u16>` | on change | 1 / 4 / 10 vs 1.2 / 4.2 ascended/descended |
| 10 | REAL_TIME_INTENSITY | `<u16> <u32> <weekly u32> <goal u32>` | on change | goal **150 exact** |

Worked example, HR: `24 03 49 33 ff ff` → handle `0x24`, flags `0x03`, HR 73, resting 51,
`ffff` = field not available.

`REAL_TIME_HRV` is the standout. It emits one frame per beat carrying the raw RR interval
in milliseconds plus a timestamp — that is genuine beat-to-beat data, not a smoothed
number. The frame count over a minute equals the heart rate, which is how the meaning was
pinned down. Timestamp deltas run ~4x the wall clock, so the unit looks like quarter-
milliseconds; confirm before relying on it.

## Registered but silent at rest

`2`, `4` (REGISTRATION itself), `15`, `16`, `22`, `24`, `28`.

They accept registration and emit nothing while the watch is idle. Several are probably
activity-scoped (speed, cadence, running dynamics, GPS) — **re-run `mode all` during a
recorded run to find out.** That is the single most informative follow-up test.

Service 24 was closed by the watch itself ~30 s after registration, via an unsolicited
management frame `00 03 <client_uuid> 18 00 4d 00` (type `0x03`, service 24, handle
`0x4d`). Type `0x03` is not in the Gadgetbridge notes.

## Do not register: service 17

**36219 frames in 60 seconds** — roughly 600/s, ~11 kB/s sustained. Frames look like
`<index u16> <14 zero bytes> <value u16> 00` with the index sweeping monotonically
upward, i.e. a paged bulk dump rather than telemetry. It flushed the entire logcat ring
buffer twice during testing. Whatever it is, it is not something an always-on bridge
should ever subscribe to.

## What to actually subscribe to

**All-day, effectively free** — these read values the watch already computes, so there is
no extra sensor cost and the frame rates are trivial:

    6  HR              21 respiration     20 body battery    13 stress
    7  steps           8  calories         9  floors         10 intensity minutes

**Worth it but heavier** — 12 (HRV), at one frame per beat. This is the highest-value data
on the channel and the only source of real RR intervals.

**Cheap but low value** — 19 (SpO2) repeats the last spot reading at 1 Hz with a
measurement timestamp; the value did not change across 61 frames. It does *not* appear to
force a new measurement, so it does not violate the "never switch on a sensor the user had
off" rule — but it also tells you nothing a once-a-minute poll would not.

## Consequence for the wire protocol

`protocol/protocol.md` §3 defines a fixed 4-byte TELEMETRY payload with a single `u8`
heart rate. The channel offers ten live metrics, several of which are `u16`/`u32` and
update on change rather than on a clock. A fixed struct means either padding every packet
with nine unchanged fields or minting a new packet type per metric.

Worth reshaping the payload into a present-fields bitmask plus packed values, so one
datagram carries whatever changed since the last one. That keeps the "latest state, lossy,
no retransmit" model intact and stays small — HR alone would still be a 4-byte payload.
This is the Metric Bus abstraction showing up in the wire format, and it is cheaper to do
now than after the Kotlin encoder exists.

## Reproducing

```bash
adb shell am start -n me.sagiri.mltest/.MainActivity -e mode all
adb logcat -s MLTEST:I
```

`mode all` reads the supported-service bitmap, registers everything except GFDI, logs up
to 8 distinct frames per service, and prints a per-service frame tally after 60 s.
Registering everything includes service 17, so kill the app when the tally prints:

```bash
adb shell am force-stop me.sagiri.mltest
```
