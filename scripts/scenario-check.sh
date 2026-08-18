#!/usr/bin/env bash
# G4: failure-scenario state check against the documented PRODUCT.md states.
# Each scenario runs the release binary in a detached 80x24 tmux pane with the
# right provider behavior, waits for its marker text, captures the pane, and
# reports PASS/FAIL for every documented state it proves.
#
# Usage:
#   scripts/scenario-check.sh
set -euo pipefail

cd "$(dirname "$0")/.."

PORT="${FIXTURE_PORT:-8139}"
SESSION="coin-scenarios"
RESULT="/tmp/coin-tui-scenarios-$$.txt"

cleanup() {
    kill "${SERVER_PID:-}" 2>/dev/null || true
    tmux kill-session -t "$SESSION" 2>/dev/null || true
    wait "${SERVER_PID:-}" 2>/dev/null || true
}
trap cleanup EXIT

# Clear any straggler fixture server still bound to the scenario port so the
# offline-startup scenario really sees a closed loopback port.
lsof -ti tcp:"$PORT" 2>/dev/null | xargs kill 2>/dev/null || true

wait_for_text() {
    local marker="$1" timeout="${2:-15}"
    for _ in $(seq 1 "$timeout"); do
        tmux capture-pane -p -t "$SESSION" 2>/dev/null | grep -q "$marker" && return 0
        sleep 1
    done
    return 1
}

report=""
run_scenario() {
    local name="$1" base_url="$2" marker="$3"
    local extra="${4:-}" timeout="${5:-15}"
    tmux kill-session -t "$SESSION" 2>/dev/null || true
    tmux new-session -d -s "$SESSION" -x 80 -y 24
    # shellcheck disable=SC2086
    tmux send-keys -t "$SESSION" \
        "(${PWD}/target/release/coin-tui --base-url $base_url $extra; echo end-$name) 2>&1" \
        Enter
    if wait_for_text "$marker" "$timeout"; then
        report+="  $name: PASS ($marker)\n"
    else
        report+="  $name: FAIL (missing $marker)\n"
    fi
    tmux kill-session -t "$SESSION" 2>/dev/null || true
}

echo "==> building release binary (locked)"
cargo build --release --locked

run_scenario "offline-startup" \
    "http://127.0.0.1:$PORT/" \
    "Offline: no market data is available; press r to retry"

run_scenario "dns-failure" \
    "https://does-not-exist.invalid/" \
    "Offline: no market data is available; press r to retry"

python3 scripts/fixture-server.py --port "$PORT" --mode malformed >/dev/null &
SERVER_PID=$!
for _ in $(seq 1 50); do curl -sf "http://127.0.0.1:$PORT/api/v3/global" >/dev/null && break; sleep 0.1; done
run_scenario "malformed" \
    "http://127.0.0.1:$PORT/" \
    "Error: invalid provider response; press r to retry"

kill "$SERVER_PID" 2>/dev/null || true
wait "$SERVER_PID" 2>/dev/null || true

python3 scripts/fixture-server.py --port "$PORT" --mode rate-limited >/dev/null &
SERVER_PID=$!
for _ in $(seq 1 50); do curl -sf "http://127.0.0.1:$PORT/api/v3/global" >/dev/null && break; sleep 0.1; done
run_scenario "rate-limited" \
    "http://127.0.0.1:$PORT/" \
    "Rate limited"

kill "$SERVER_PID" 2>/dev/null || true
wait "$SERVER_PID" 2>/dev/null || true

python3 scripts/fixture-server.py --port "$PORT" --mode server-error >/dev/null &
SERVER_PID=$!
for _ in $(seq 1 50); do curl -sf "http://127.0.0.1:$PORT/api/v3/global" >/dev/null && break; sleep 0.1; done
run_scenario "server-error" \
    "http://127.0.0.1:$PORT/" \
    "Error: provider request failed; press r to retry"

kill "$SERVER_PID" 2>/dev/null || true
wait "$SERVER_PID" 2>/dev/null || true

python3 scripts/fixture-server.py --port "$PORT" --mode timeout >/dev/null &
SERVER_PID=$!
for _ in $(seq 1 50); do curl -sf "http://127.0.0.1:$PORT/api/v3/global" >/dev/null && break; sleep 0.1; done
run_scenario "timeout" \
    "http://127.0.0.1:$PORT/" \
    "Offline: no market data is available; press r to retry" \
    "" 45

kill "$SERVER_PID" 2>/dev/null || true
wait "$SERVER_PID" 2>/dev/null || true

printf "============================================================\nG4-3 scenario report\n%b============================================================\n" "$report"