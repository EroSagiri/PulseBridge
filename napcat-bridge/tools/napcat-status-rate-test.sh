#!/usr/bin/env bash
set -u

# Destructive-ish integration test: this sends real QQ online-status updates.
# Keep the default duration short and require an explicit confirmation.
if [[ "${NAPCAT_RATE_TEST_CONFIRM:-}" != "I_UNDERSTAND_QQ_RATE_LIMIT_RISK" ]]; then
  echo "Refusing to run. Set NAPCAT_RATE_TEST_CONFIRM=I_UNDERSTAND_QQ_RATE_LIMIT_RISK" >&2
  exit 2
fi

api="${NAPCAT_API_URL:-http://127.0.0.1:3000}"
api="${api%/}"
duration="${NAPCAT_RATE_TEST_DURATION_SECONDS:-1}"
levels=(60 30 15 8 4 2 1)

if ! [[ "$duration" =~ ^[0-9]+([.][0-9]+)?$ ]] || [[ "$duration" == 0* && "$duration" != 0.* ]]; then
  echo "NAPCAT_RATE_TEST_DURATION_SECONDS must be a positive number" >&2
  exit 2
fi

echo "Testing $api/set_diy_online_status for ${duration}s per level"
echo "level_hz,requests,http_ok,onebot_ok,business_ok"

for hz in "${levels[@]}"; do
  interval="$(awk -v hz="$hz" 'BEGIN { printf "%.9f", 1 / hz }')"
  requests_target="$(awk -v hz="$hz" -v duration="$duration" 'BEGIN { printf "%.0f", hz * duration }')"
  result_dir="$(mktemp -d)"
  pids=()
  http_ok=0
  onebot_ok=0
  business_ok=0

  for ((request = 0; request < requests_target; request++)); do
    wording="RateTest-${hz}Hz-${request}"
    (
      curl -sS --max-time 3 -w $'\n__HTTP_STATUS:%{http_code}' \
      -H 'Content-Type: application/json' \
      --data "{\"face_id\":0,\"face_type\":1,\"wording\":\"$wording\"}" \
      "$api/set_diy_online_status" >"$result_dir/$request" 2>/dev/null || true
    ) &
    pids+=("$!")
    sleep "$interval"
  done
  for pid in "${pids[@]}"; do wait "$pid" || true; done

  for result in "$result_dir"/*; do
    status="$(sed -n '$s/^__HTTP_STATUS://p' "$result")"
    body="$(sed '$d' "$result")"
    [[ "$status" == 2* ]] && ((http_ok += 1))
    grep -q '"status"[[:space:]]*:[[:space:]]*"ok"' <<<"$body" && ((onebot_ok += 1))
    grep -q '"retcode"[[:space:]]*:[[:space:]]*0' <<<"$body" && \
      ! grep -q '"result"[[:space:]]*:[[:space:]]*[1-9]' <<<"$body" && ((business_ok += 1))
  done

  rm -rf "$result_dir"
  echo "$hz,$requests_target,$http_ok,$onebot_ok,$business_ok"
done
