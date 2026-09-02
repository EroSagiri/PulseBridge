# Broadcast mode battery measurement

The whole approach lives or dies on this number, and the only published figures
are from a Forerunner 230/235 — a 2015 watch with an older Bluetooth radio and
a smaller battery. Those measured roughly **2–3 %/hour** (22–24 % across an
8–12 hour night). The FR255 baseline is 14 days in smartwatch mode, about
**0.3 %/hour**, so broadcast would be 5–10× normal drain.

Do not design around the old number. Measure the actual watch.

## Test A — the number that decides everything

1. Charge to 100 %, wait until the reading settles.
2. Note watch battery % and the clock time.
3. Turn on Broadcast HR, start the app, confirm the dashboard is live.
4. Go about a normal sedentary two hours. No activity recording, no GPS.
5. Note watch battery % and time again.

`drain_per_hour = (start − end) / hours`

Read the app's **uptime**, **samples from watch** and **ble reconnects** at the
end. If samples is far below `uptime_seconds`, the link was dropping and the
drain figure is not comparable.

## How to read the result

| measured | verdict |
|---|---|
| ≤ 1.5 %/h | All-day broadcast is fine. Ship it as-is. |
| 1.5–2.5 %/h | ~16 h/day costs 24–40 %. Viable if you charge daily. |
| > 2.5 %/h | On-demand only, or move all-day duty to a chest strap. |

## Test B — baseline control

Same two hours, same day if possible, with broadcast **off** and the app
stopped. This is what the watch costs you anyway. Subtract it: the difference
is the true price of the bridge, and it is the only fair comparison.

Skipping this test is the most common way to over-attribute drain to
broadcasting.

## Test C — does the phone survive the night

Separate question, separate run. Leave the app streaming overnight with the
screen off.

Record in the morning: phone battery delta, app uptime, `ble reconnects`, and
whether the dashboard shows an unbroken stream.

What this is really testing is Doze and the OEM power manager, not Bluetooth.
If uptime resets or reconnects climb into the dozens, the foreground service
was killed — check the battery optimisation exemption first, then whatever
extra allow-list the phone vendor ships.

## Log

| date | test | start % | end % | hours | %/h | reconnects | notes |
|------|------|---------|-------|-------|-----|------------|-------|
|      | A    |         |       |       |     |            |       |
|      | B    |         |       |       |     |            |       |
|      | C    |         |       |       |     |            |       |

## If the answer is "too expensive"

The Android side is a plain Heart Rate Service client, so a chest strap or an
optical armband pairs with **exactly the same code and no changes** — a coin
cell runs one for months at 1 Hz because that hardware is built for it. The
watch then goes back to what it is good at, and the bridge keeps working.
