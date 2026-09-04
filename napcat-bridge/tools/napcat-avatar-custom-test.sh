#!/usr/bin/env bash
set -u

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  cat <<'EOF'
Usage:
  NAPCAT_RATE_TEST_CONFIRM=I_UNDERSTAND_QQ_RATE_LIMIT_RISK \
  tools/napcat-avatar-custom-test.sh [interval_ms] [duration_seconds] [start_bpm] [end_bpm]

Example:
  ... tools/napcat-avatar-custom-test.sh 1000 60 71 79

Rotates the generated heart-rate PNGs from assets/heart-rate through NapCat's
set_qq_avatar endpoint. This changes the real QQ avatar.
EOF
  exit 0
fi

if [[ "${NAPCAT_RATE_TEST_CONFIRM:-}" != "I_UNDERSTAND_QQ_RATE_LIMIT_RISK" ]]; then
  echo "Refusing to run. Set NAPCAT_RATE_TEST_CONFIRM=I_UNDERSTAND_QQ_RATE_LIMIT_RISK" >&2
  exit 2
fi

api="${NAPCAT_API_URL:-http://127.0.0.1:3000}"
api="${api%/}"
interval_ms="${1:-2000}"
duration="${2:-10}"
start_bpm="${3:-71}"
end_bpm="${4:-79}"

if ! [[ "$interval_ms" =~ ^[0-9]+([.][0-9]+)?$ ]] || ! [[ "$duration" =~ ^[0-9]+([.][0-9]+)?$ ]] || ! [[ "$start_bpm" =~ ^[0-9]+$ ]] || ! [[ "$end_bpm" =~ ^[0-9]+$ ]]; then
  echo "interval_ms/duration must be positive numbers; BPM range must be integers" >&2
  exit 2
fi
if ! awk -v interval_ms="$interval_ms" -v duration="$duration" -v start_bpm="$start_bpm" -v end_bpm="$end_bpm" 'BEGIN { exit !(interval_ms > 0 && duration > 0 && start_bpm >= 0 && end_bpm <= 999 && start_bpm <= end_bpm) }'; then
  echo "invalid interval, duration, or BPM range (expected 0 <= start <= end <= 999)" >&2
  exit 2
fi

interval="$(awk -v interval_ms="$interval_ms" 'BEGIN { printf "%.9f", interval_ms / 1000 }')"
request_target="$(awk -v interval_ms="$interval_ms" -v duration="$duration" 'BEGIN { printf "%.0f", 1000 / interval_ms * duration }')"
avatar_dir="assets/heart-rate"
avatar_count=$((end_bpm - start_bpm + 1))
for bpm in $(seq "$start_bpm" "$end_bpm"); do
  image="$avatar_dir/heart-rate-$(printf '%03d' "$bpm").jpg"
  if [[ ! -f "$image" ]]; then
    echo "missing generated image: $image" >&2
    exit 2
  fi
done

echo "Testing heart-rate ${start_bpm}-${end_bpm} every ${interval_ms} ms for ${duration}s (${request_target} requests)"
echo "heart_rate, http_status, onebot_status"

for ((request = 0; request < request_target; request++)); do
  bpm=$((start_bpm + request % avatar_count))
  image="$avatar_dir/heart-rate-$(printf '%03d' "$bpm").png"
  response="$({
    printf '%s' '{"file":"base64://'
    base64 -w0 "$image"
    printf '%s' '"}'
  } | curl -sS --max-time 10 -w $'\n__HTTP_STATUS:%{http_code}' \
    -H 'Content-Type: application/json' \
    --data-binary @- \
    "$api/set_qq_avatar" 2>/dev/null || true)"
  http_status="${response##*__HTTP_STATUS:}"
  body="${response%$'\n'__HTTP_STATUS:*}"
  onebot_status="$(grep -o '"status"[[:space:]]*:[[:space:]]*"[^"]*"' <<<"$body" | head -1 || true)"
  echo "$bpm, $http_status, ${onebot_status:-missing}"
  sleep "$interval"
done
