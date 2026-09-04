#!/usr/bin/env bash
set -u

usage() {
  cat <<'EOF'
Usage:
  NAPCAT_RATE_TEST_CONFIRM=I_UNDERSTAND_QQ_RATE_LIMIT_RISK \
  tools/napcat-status-custom-test.sh [interval_ms] [duration_seconds] [prefix] [start_number]

Examples:
  ... tools/napcat-status-custom-test.sh 1000 30 PulseTest 1
  ... tools/napcat-status-custom-test.sh 500 20 "HR Demo" 100

The wording becomes: "<prefix> <number>" and increments on every request.
This sends real set_diy_online_status requests to NapCat.
EOF
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

if [[ "${NAPCAT_RATE_TEST_CONFIRM:-}" != "I_UNDERSTAND_QQ_RATE_LIMIT_RISK" ]]; then
  echo "Refusing to run. Set NAPCAT_RATE_TEST_CONFIRM=I_UNDERSTAND_QQ_RATE_LIMIT_RISK" >&2
  exit 2
fi

api="${NAPCAT_API_URL:-http://127.0.0.1:3000}"
api="${api%/}"
interval_ms="${1:-1000}"
duration="${2:-10}"
prefix="${3:-PulseTest}"
number="${4:-1}"

if ! [[ "$interval_ms" =~ ^[0-9]+([.][0-9]+)?$ ]] || ! [[ "$duration" =~ ^[0-9]+([.][0-9]+)?$ ]]; then
  echo "interval_ms and duration_seconds must be positive numbers" >&2
  exit 2
fi
if awk -v interval_ms="$interval_ms" -v duration="$duration" 'BEGIN { exit !(interval_ms > 0 && duration > 0) }'; then :; else
  echo "interval_ms and duration_seconds must be greater than zero" >&2
  exit 2
fi

interval="$(awk -v interval_ms="$interval_ms" 'BEGIN { printf "%.9f", interval_ms / 1000 }')"
request_target="$(awk -v interval_ms="$interval_ms" -v duration="$duration" 'BEGIN { printf "%.0f", 1000 / interval_ms * duration }')"
echo "Testing $api/set_diy_online_status every ${interval_ms} ms for ${duration}s (${request_target} requests)"
echo "wording, http_status, onebot_status, business_result"

for ((request = 0; request < request_target; request++)); do
  wording="${prefix} ${number}"
  response="$(curl -sS --max-time 3 -w $'\n__HTTP_STATUS:%{http_code}' \
    -H 'Content-Type: application/json' \
    --data "{\"face_id\":0,\"face_type\":1,\"wording\":\"$wording\"}" \
    "$api/set_diy_online_status" 2>/dev/null || true)"
  http_status="${response##*__HTTP_STATUS:}"
  body="${response%$'\n'__HTTP_STATUS:*}"
  onebot_status="$(grep -o '"status"[[:space:]]*:[[:space:]]*"[^"]*"' <<<"$body" | head -1 || true)"
  result="$(grep -o '"result"[[:space:]]*:[[:space:]]*[^,}]*' <<<"$body" | head -1 || true)"
  echo "$wording, $http_status, ${onebot_status:-missing}, ${result:-missing}"
  number=$((number + 1))
  sleep "$interval"
done
