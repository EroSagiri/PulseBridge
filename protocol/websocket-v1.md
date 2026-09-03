# PulseBridge WebSocket subscriber contract v1

The subscriber transport is a WebSocket carrying UTF-8 JSON text frames:

```text
ws://host:8080/ws
```

The server sends one `snapshot` immediately after connection, repeats a
snapshot every two seconds so presence decay is observable, and sends a
`metric` message when a metric value changes.

## Compatibility rules

- The current `/ws` endpoint is the v1 contract and has no version-negotiation
  handshake.
- Existing message types, field names, field meanings, and JSON types are
  stable. A breaking change requires a new versioned endpoint or an explicit
  protocol migration; it must not silently change `/ws` v1.
- Consumers must dispatch on the top-level `type` field and ignore unknown
  message types when possible.
- Consumers should ignore unknown object fields. New fields are add-only and
  must be optional for older consumers.
- A new metric variant is not a change to the meaning of `heart_rate`. Clients
  that do not understand the new variant must ignore that metric rather than
  treating it as heart rate.
- A message with a known type but missing or invalid required fields is
  malformed and should be ignored or reported without terminating the
  subscription loop.
- Snapshot data is authoritative for the complete current device list and
  presence. A `metric` event is an immediate update for its `device_id`; the
  next snapshot remains authoritative for all other fields.

## v1 message shapes

```json
{
  "type": "snapshot",
  "devices": [
    {
      "device_id": 1,
      "presence": "online",
      "age_ms": 340,
      "heart_rate": 72,
      "resting_hr": 51,
      "contact_ok": true,
      "watch_connected": true,
      "phone_battery_pct": 77,
      "session_id": 12,
      "packets": 812,
      "last_sequence": 812,
      "gaps": 3
    }
  ]
}
```

```json
{
  "type": "metric",
  "event": {
    "device_id": 1,
    "timestamp_ms": 1756800000000,
    "metric": "heart_rate",
    "bpm": 73,
    "contact_ok": true
  }
}
```

`presence` is `online` within 15 seconds, `stale` within 60 seconds, and
`offline` after that. `heart_rate` is null unless the device is online and the
reading is current. The server does not backfill metrics after a disconnect.
