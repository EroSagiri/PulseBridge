# PulseBridge Wire Protocol v1

Transport: **UDP**, single datagram per packet, no fragmentation, no retransmission.
This is a *latest-state* telemetry bus: losing a packet is fine, the next one supersedes it.

## 1. Packet layout

```
offset size field         notes
------ ---- ------------- -------------------------------------------
     0    2 magic         0x5042 ("PB"), big-endian
     2    1 version       1
     3    1 packet_type   1 = TELEMETRY
     4    4 device_id     little-endian u32
     8    4 session_id    little-endian u32, random per app start
    12    4 sequence      little-endian u32, +1 per packet within session
    16    8 timestamp_ms  little-endian u64, sender's Unix epoch ms
------ ---- ------------- -------------------------------------------
    24    N ciphertext    AEAD output
  24+N   16 tag           Poly1305 tag
```

Header (bytes 0..24) is **plaintext** and is used verbatim as the AEAD
associated data, so every field above is authenticated and cannot be tampered
with even though it is readable.

## 2. Cryptography

* Algorithm: **ChaCha20-Poly1305 (IETF, RFC 8439)** — 32-byte key, 12-byte nonce.
* Key: 32 bytes, pre-shared per device, distributed out of band (hex in config).
* Nonce is **deterministic**, never random:

```
nonce = device_id (4, LE) || session_id (4, LE) || sequence (4, LE)
```

Uniqueness holds as long as `(session_id, sequence)` never repeats under the
same key. `session_id` is re-randomised on every app start, which is what makes
a sequence reset after a crash safe.

> Why not XChaCha20? The extended nonce exists to make *random* nonces safe.
> We derive nonces deterministically, so the 12-byte IETF nonce is sufficient
> and is available in `javax.crypto` on Android API 28+ with no native library.

## 3. TELEMETRY payload (plaintext, 4 bytes)

```
offset size field       notes
------ ---- ----------- ------------------------------------------
     0    1 flags       bit0 hr_valid
                        bit1 sensor_contact_ok
                        bit2 watch_connected  (BLE link is up)
                        bit3 heartbeat        (no change, keepalive)
     1    1 heart_rate  bpm, 0 when hr_valid = 0
     2    1 battery_pct phone battery 0..100, 0xFF = unknown
     3    1 resting_hr  bpm, 0 = unknown
```

`resting_hr` occupies what v1 originally reserved. A sender that does not know
it writes 0, which is what the reserved byte already carried, so the two are
wire-compatible and the version stays 1. Only the Garmin Multi-Link source
supplies it; the standard Heart Rate Service does not carry resting rate.

`heart_rate` is `u8`: the BLE Heart Rate Measurement characteristic may report
16-bit values, but any value > 255 bpm is clamped and `hr_valid` cleared.

Never encode "no data" as `heart_rate = 0` with `hr_valid` set. Consumers must
check `hr_valid` first.

## 4. Replay and session handling (server side)

* Reject packets whose `magic`/`version` do not match, or whose AEAD tag fails.
* Reject `timestamp_ms` more than **120 s** away from server time in either
  direction (clock skew tolerance).
* Per `(device_id, session_id)` keep `highest_seq` and a **64-bit sliding window
  bitmap** of already-seen sequences below it. UDP reorders, so a strictly
  increasing check would drop legitimate packets.
* A packet older than `highest_seq - 64` is dropped.
* A **new `session_id`** for a known device replaces the old session state.
  This is how an app restart recovers.

## 5. Address rebinding

The server MUST NOT trust the UDP source address. The remote address stored for
a device is updated **only after** a packet from that address has passed AEAD
verification and the replay window check. This is what makes Wi-Fi <-> mobile
handover safe without any reconnection handshake.

## 6. Send policy (client side)

| condition                       | action                     |
|---------------------------------|----------------------------|
| heart rate value changed        | send immediately           |
| value unchanged                 | heartbeat every **10 s**   |
| BLE link state changed          | send immediately           |

10 s keeps carrier NAT bindings alive (they can expire at 15-30 s) and bounds
idle traffic to ~44 bytes / 10 s ≈ 0.4 kB/min.

There is **no retransmission and no backfill**. After a network outage the
client sends current state only. Historical data is Garmin's job, not ours.

## 7. Test vectors

```
key        = 000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f
device_id  = 0x00000001
session_id = 0x11223344
sequence   = 0x00000001
timestamp  = 1700000000000  (0x18BCFE56800)
payload    = flags 0x07, hr 72, battery 85, resting_hr 51
```

The complete canonical packet, including nonce, ciphertext and tag, is stored
in `protocol/test-vectors/telemetry-v1.json`. The Rust and Kotlin codec tests
must remain byte-compatible with that fixture.
