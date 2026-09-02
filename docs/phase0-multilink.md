# Phase 0 — Garmin Multi-Link go/no-go

**Verdict: GREEN. Coexistence with Garmin Connect works.**

A second GATT client registered `REAL_TIME_HR` on a Forerunner 255 while the official
Garmin Connect app stayed connected and undisturbed, and 1 Hz heart rate streamed out
immediately. No pairing, no authentication, no reverse-engineered handshake.

Run on 2026-09-02. Probe source: `tools/mltest/`.

## Setup

| | |
|---|---|
| Watch | Forerunner 255, `C4:CB:11:53:B5:58`, product number 3992 |
| Phone | OPPO PKB110, Android 16 (ColorOS) |
| Other GATT clients on the link | Garmin Connect, Keep, nRF Connect |

## 1. Multiple GATT clients on one link — proven

Android multiplexes app-level GATT clients over a single ACL link. During the test
four clients were attached to the watch simultaneously:

```
id: 1  address: c4:cb:11:53:b5:58  transport: BT_TRANSPORT_LE  ch_state: GATT_CH_OPEN,
  ACL holders gatt_if: no.nordicsemi.android.mcp (110), com.gotokeep.keep (108),
                       me.sagiri.mltest (112), com.garmin.android.apps.connectmobile (107),
```

Garmin Connect kept `gatt_if 107` from before the probe started until after it exited —
it never dropped or reconnected.

**This only holds for two apps on the same phone.** A second phone would need the watch
to accept a second ACL connection as peripheral, which is a different and much harder
question. Do not design around it.

## 2. GATT table of the FR255

```
6a4e2800  Multi-Link
   6a4e2803   READ WRITE WRITE_NR    registration characteristic (single-byte queries)
   6a4e2810   READ WRITE WRITE_NR NOTIFY  \ lane 0
   6a4e2820   WRITE WRITE_NR              /
   6a4e2811   READ WRITE WRITE_NR NOTIFY  \ lane 1  <- Garmin Connect
   6a4e2821   WRITE WRITE_NR              /
   6a4e2830   READ WRITE WRITE_NR NOTIFY  \ lane 2
   6a4e2840   WRITE WRITE_NR              /
6a4e8022  legacy GFDI-over-dedicated-characteristic (4c80 write / cd28 notify)
0000180d  Heart Rate            (2a37 notify)  -- standard broadcast HRS is present
00001814  Running Speed/Cadence (2a53 notify, 2a54 feature)
00003802, cc353442-…  vendor
```

The third lane is **2830/2840**, not 2822/2812 as the Gadgetbridge notes imply. Do not
hardcode the documented triple; enumerate what the device actually exposes.

Garmin Connect was identified as the owner of lane 1 from the Bluetooth stack's own
notification counters before any write was attempted:

```
ATT NTF INFO: char[6a4e2811-667b-11e3-949a-0800200c9a66],cnt/duration_ms/freq_p_cnt=373/7167167/4
```

The probe used lane 0 and never touched lane 1.

## 3. Capability queries (`6a4e2803`, read back after each write)

| query | raw reply | meaning |
|---|---|---|
| `0x00` SUPPORTED_PROTOCOLS | `00 d6 b7 7b 11` | bitmap `0x117bb7d6` |
| `0x02` MULTI_LINK_VERSION | `02 01 02 02` | `01 02 02` |
| `0x03` PRODUCT_NUMBER | `03 98 0f 59 0b 44 73 ac d6` | product `0x0f98` = 3992, `0x0b59` = 2905, unit id `0xd6ac7344` |

Byte 0 echoes the query type; the rest is the payload. The service bitmap decodes to:

```
1  GFDI            9  ?              17 ?
2  ?              10  ?              19 REAL_TIME_SPO2
4  REGISTRATION   12  REAL_TIME_HRV  20 REAL_TIME_BODY_BATTERY
6  REAL_TIME_HR   13  REAL_TIME_STRESS  21 ?
7  ?              15  ?              22 ?
8  ?              16  ?              24 ?  28 ?
```

Superset of the FR245 list in the Gadgetbridge docs (which adds 2, 9, 15, 17, 24, 28).
Names for the unlabelled ids are not established — don't guess them into code.

## 4. Handle registration — all three succeeded

Frame: `00 00 | client_uuid[8] | service_id[2 LE] | 00`, written to `6a4e2820`.
Reply arrives as a notification on `6a4e2810`.

`client_uuid` was `50 42 00 00 00 00 00 00` ("PB"). Garmin Connect uses `0x01`.

| service | request | response | status | handle |
|---|---|---|---|---|
| 4 REGISTRATION | `00 00 5042…00 04 00 00` | `00 01 5042…00 04 00 00 23 00 02` | SUCCESS | `0x23` |
| 6 REAL_TIME_HR | `00 00 5042…00 06 00 00` | `00 01 5042…00 06 00 00 24 00 00` | SUCCESS | `0x24` |
| 1 GFDI         | `00 00 5042…00 01 00 00` | `00 01 5042…00 01 00 00 25 00 00` | SUCCESS | `0x25` |

No `PENDING_AUTH`, no `ALREADY_IN_USE`, no `REJECTED`. **Registration on an unclaimed
lane with a fresh `client_uuid` needs no authentication.** This was the gate most likely
to kill the project and it is not a gate at all.

Handles are assigned dynamically (`0x23`, `0x24`, `0x25` in registration order), so the
decoder must dispatch on the handle returned at registration time, never a constant.

## 5. Real-time HR frames

Arriving on lane 0 at roughly 1 Hz:

```
24 03 42 33 ff ff
24 03 41 33 ff ff
24 03 40 33 ff ff
24 03 43 33 ff ff
```

| offset | value | reading |
|---|---|---|
| 0 | `0x24` | ML handle for REAL_TIME_HR (from registration) |
| 1 | `0x03` | constant; message type or flags |
| 2 | `0x40`–`0x43` | **current heart rate, bpm** (64–67) |
| 3 | `0x33` | **resting heart rate**, 51 |
| 4–5 | `ff ff` | 16-bit field, all-ones = not available |

Cross-checked against Garmin Connect for the same day: average HR 65.8 bpm, resting HR
**51**. Both the live value and the constant match. Offsets 1 and 4–5 are inferred from
a single resting session and need a workout to confirm.

Registering GFDI (handle `0x25`) also produced a repeating device-information message
containing the ASCII strings `Forerunner 255` / `Forerunner` / `255` — it repeats
because the probe never answers the handshake. There is no reason to register GFDI for
this project; drop it.

## 6. What this changes

- **`REAL_TIME_HR` as its own Multi-Link service is real.** Registering by service id and
  dispatching on the returned handle is the right shape after all. It is not GFDI protobuf.
- **All-day real-time HR does not require displacing Garmin Connect**, so the fallback
  branch "give up Connect on this phone" is dead, and with it the conflict against the
  Connect IQ route (which needs Connect Mobile running). Both can coexist.
- **Standard HR broadcast is no longer needed as a safety net.**
- Metrics 12 (HRV), 13 (stress), 19 (SpO2), 20 (body battery) are all advertised as
  supported and register the same way — the Metric Bus abstraction has somewhere to go.

## 7. Still open

- Whether the watch keeps `REAL_TIME_HR` streaming through screen-off / Doze on ColorOS,
  and for how long. Battery cost of an all-day subscription is unmeasured.
- The close-handle message format (`00 02 …`) is untested; the probe just detaches its
  GATT client.
- What happens on a Garmin Connect firmware sync, or when Connect itself wants lane 0.
- Whether lane 0 stays free after a watch reboot, and what `ALREADY_IN_USE` actually
  returns in the "free char uuid" field.
- Byte 1 and bytes 4–5 of the HR frame.

## Reproducing

```bash
cd tools/mltest && bash build.sh
adb install -r -t build/mltest.apk
adb shell am start -n me.sagiri.mltest/.MainActivity -e mode scan   # read-only
adb shell am start -n me.sagiri.mltest/.MainActivity -e mode reg    # writes
adb logcat -s MLTEST:I
```

`scan` dumps the GATT table and writes nothing. `reg` runs the capability queries and the
registration sequence. Both target lane 0 only.
