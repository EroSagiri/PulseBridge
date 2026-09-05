# PulseBridge NapCat Bridge

Consumes the local PulseBridge WebSocket and updates NapCat's QQ nickname with
the current heart rate. The default nickname is `June - 💓72`; it changes when
the heart-rate value changes and returns to `June` when the source is offline.
The custom online status remains available as a fallback.

The bridge can also render a JPEG heart-rate avatar in a dedicated renderer
thread. It reads the fixed asset manifest at
`/opt/pulsebridge/assets/heart-rate/avatar.json`, prepares a 1280×1280 master,
and caches rendered BPM values on demand. The manifest describes the background,
optional foreground, and heart-rate display. Heart-rate display supports text
or digit sprites and can be rendered as one combined value or as separate
hundreds/tens/ones positions. Relative asset paths are resolved beside the
manifest.

For local preview/testing, build and run the separate Rust CLI:

```text
cargo run --bin pulsebridge-avatar -- assets/heart-rate/avatar.json \
  --bpm 66,80,180 --count 60 --size 320 --quality 50 --output avatar-preview
cargo run --bin pulsebridge-avatar -- assets/heart-rate/avatar.json \
  --bpm 66,80,180 --count 3 --size 320 --max-bytes 10k --output avatar-preview-10k
cargo run --bin pulsebridge-avatar -- assets/heart-rate/avatar.json \
  --bpm 66,180 --zone-algorithm lactate_threshold --max-hr 200 \
  --lactate-threshold 170 --quality 50 --output avatar-preview-lthr
```

`--quality` and `--max-bytes` are mutually exclusive. If neither is supplied,
quality mode defaults to 50. In size mode the CLI chooses the highest JPEG
quality that stays at or below the requested limit. BPM values are cycled when
`--count` is larger than the list; every output is rendered from the master and
written as a uniquely numbered `.jpg`. `k`/`m` suffixes use decimal units;
`ki`/`mi` (or `kib`/`mib`) use binary units.

## Heart-rate display and per-zone artwork

Zone detection is supplied at runtime; `avatar.json` contains artwork only. A
minimal text configuration looks like this:

```json
{
  "background": "background.png",
  "foreground": null,
  "heart_rate": {
    "layout": "combined",
    "defaults": {
      "mode": "text",
      "common": {
        "region": { "cx": 420, "cy": 424, "width": 195, "height": 94, "rotation": 0 },
        "hide_leading_zeroes": true
      },
      "text": {
        "font": "DejaVuSansMono-Bold.ttf",
        "font_size": 80,
        "arc": { "curvature": 0, "x_scale": 1 },
        "effects": {
          "fill": "#FFFFFF",
          "highlight": "#FFFFFF88",
          "outline": { "color": "#702040", "width": 4 },
          "glow": { "color": "#FF80AA88", "radius": 4 },
          "inner_shadow": { "color": "#00000055", "offset_x": 2, "offset_y": 3, "blur": 3 }
        }
      }
    }
  }
}
```

Set `layout` to `individual` to use the three named positions
`hundreds`, `tens`, and `ones`. Each position inherits `heart_rate.defaults`
and may override its own region, mode, text style, or sprite style.

Sprite mode requires all ten digits after inheritance. A digit can be an entire
image or a rectangle from a sprite sheet:

```json
"sprite": {
  "spacing": 4,
  "digits": {
    "0": { "path": "digits.png", "rect": { "x": 0, "y": 0, "w": 80, "h": 120 } },
    "1": { "path": "digits/1.png" }
  }
}
```

The example omits digits `2` through `9`; a real sprite configuration must
provide them. Text effects are only read from `text.effects`; sprite settings
are only read from `sprite`, so the two modes do not accidentally share
outline/glow settings.

`foreground` is optional. A Zone can replace it with another image, adjust its
position, or disable it explicitly with `"foreground": null`:

```json
"zones": {
  "z4": {
    "foreground": { "path": "foreground-z4.png", "opacity": 0.9 },
    "heart_rate": {
      "defaults": {
        "text": { "effects": { "fill": "#FFB4B4" } }
      }
    }
  },
  "z5": { "foreground": null }
}
```

Zone overrides are merged recursively. The selected Zone's background,
heart-rate display, and foreground are resolved independently, then rendered
on the 1280×1280 master in the order background, heart rate, foreground.

The CLI accepts the following runtime parameters:

```text
--zone-algorithm max_hr --max-hr 200
--zone-algorithm lactate_threshold --max-hr 200 --lactate-threshold 170
--zone-algorithm custom --max-hr 200 \
  --custom-zones 50-100,101-140,141-160,161-180,181-200
```

The three detection algorithms are:

- `max_hr`: default. With `max_hr: 200`, the thresholds are 120, 140, 160,
  and 180 BPM; the five finite ranges are 100–120, 121–140, 141–160,
  161–180, and 181–200 BPM. Values below 100 or above 200 are out of range.
- `lactate_threshold`: requires `lactate_threshold`, in BPM. The default
  thresholds are 85%, 90%, 95%, and 100% of that threshold; Z5 ends at
  `max_hr`, and values above `max_hr` are out of range.
- `custom`: requires `custom.z1` through `custom.z5`, each with inclusive
  `min` and `max` BPM values. Ranges may have intentional gaps; values in a
  gap or outside all five ranges are out of range.

Out-of-range is an internal default state and does not need a JSON entry. Its
artwork uses the base configuration, while status text shows `--`; it is
different from `NoData` and `Offline`.

The live bridge and `pulsebridge-avatar` CLI use the same zone resolver, so a
preview generated with the CLI uses the same artwork that the service will
upload. For the service, these user/runtime values come from environment
variables, never from `avatar.json`.

Avatar updates use a sampling window rather than uploading every changed
metric. During the day the default window is 10 seconds; during the night it
is 30 seconds (23:00–07:00 in the machine's local timezone). The avatar uses
the rounded average BPM in that window, and skips the upload when it equals
the previous avatar BPM. Each uploaded window starts a new sampling window.

If a newly received BPM differs from the previous uploaded BPM by more than
10 BPM, the avatar updates immediately using the median of the current
window. That window is consumed and restarted, and the immediate-jump path
then enters a 5-second cooldown. The normal day/night window is still allowed
to complete during that cooldown.

The custom online status still updates on each changed state subject to its
own interval. PulseBridge itself only emits a metric event when the heart-rate
value changes. The heart icon alternates on each update to simulate a beat.

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
| `PB_ZONE_ALGORITHM` | `max_hr` |
| `PB_MAX_HR` | `200` |
| `PB_LACTATE_THRESHOLD` | unset; required for `lactate_threshold` |
| `PB_CUSTOM_ZONES` | unset; required for `custom`, format `MIN-MAX,...` |
| `PB_STATUS_FORMAT` | `{heart} {zone} · {bpm} BPM` |
| `PB_NICKNAME_FORMAT` | `June - 💓{bpm}` |
| `PB_NICKNAME_IDLE` | `June` |
| `PB_AVATAR_ENABLED` | `true` |
| `PB_AVATAR_DAY_INTERVAL_SEC` | `10` |
| `PB_AVATAR_NIGHT_INTERVAL_SEC` | `30` |
| `PB_AVATAR_NIGHT_START_HOUR` | `23` |
| `PB_AVATAR_NIGHT_END_HOUR` | `7` |
| `PB_AVATAR_JUMP_THRESHOLD_BPM` | `10` (strictly greater than) |
| `PB_AVATAR_JUMP_COOLDOWN_SEC` | `5` |
| `PB_AVATAR_SIZE` | `320` |
| `PB_AVATAR_JPEG_QUALITY` | `50` |
| `PB_AVATAR_MAX_BYTES` | unset; e.g. `10k` |

Zones use maximum-heart-rate percentages: Z1 50–60%, Z2 60–70%, Z3 70–80%,
Z4 80–90%, and Z5 90–100%. With `PB_MAX_HR=200`, the ranges are 100–120,
121–140, 141–160, 161–180, and 181–200 BPM. Values outside the selected
algorithm's ranges are out of range and use the base artwork.
| `NAPCAT_FACE_ID` | `0` |
| `NAPCAT_FACE_TYPE` | `1` |

When the selected device is online without a heart-rate sample, the avatar
uses a cached `--` image. When the device is offline or the PulseBridge
WebSocket is disconnected, it uses a cached `OFF` image.
