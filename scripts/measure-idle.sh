#!/usr/bin/env bash
# M4-04 idle and refresh measurement harness.
#
# Runs the release binary inside a detached 120x30 tmux pane against the local
# fixture server, samples its CPU every second for a window, then quits with
# `q` and reports the idle CPU and the traced render/refresh timings.
#
# Usage:
#   scripts/measure-idle.sh              # 60s idle CPU with an instant fixture
#   FIXTURE_DELAY_MS=250 IDLE_WINDOW=3 scripts/measure-idle.sh   # delayed mock timing
#
# Env: FIXTURE_PORT (default 8137), FIXTURE_DELAY_MS (default 0),
#      IDLE_WINDOW (default 60).
set -euo pipefail

cd "$(dirname "$0")/.."

PORT="${FIXTURE_PORT:-8137}"
DELAY_MS="${FIXTURE_DELAY_MS:-0}"
WINDOW="${IDLE_WINDOW:-60}"
LOG_FILE="/tmp/coin-tui-measure-$$.log"
SAMPLES="/tmp/coin-tui-samples-$$.txt"
SESSION="coin-measure"

cleanup() {
    kill "${SERVER_PID:-}" 2>/dev/null || true
    tmux kill-session -t "$SESSION" 2>/dev/null || true
    wait "${SERVER_PID:-}" 2>/dev/null || true
}
trap cleanup EXIT

echo "==> building release binary (locked)"
cargo build --release --locked

echo "==> starting fixture server on 127.0.0.1:$PORT (delay ${DELAY_MS}ms)"
python3 scripts/fixture-server.py --port "$PORT" --delay-ms "$DELAY_MS" &
SERVER_PID=$!
for _ in $(seq 1 50); do
    curl -sf "http://127.0.0.1:$PORT/api/v3/global" >/dev/null && break
    sleep 0.1
done

echo "==> starting app in tmux $SESSION (120x30)"
tmux kill-session -t "$SESSION" 2>/dev/null || true
tmux new-session -d -s "$SESSION" -x 120 -y 30
tmux send-keys -t "$SESSION" \
    "exec ${PWD}/target/release/coin-tui --base-url http://127.0.0.1:$PORT/ --log-file $LOG_FILE" \
    Enter

APP_PID=""
for _ in $(seq 1 100); do
    APP_PID=$(pgrep -f "coin-tui --base-url http://127.0.0.1:$PORT" | head -1 || true)
    [ -n "$APP_PID" ] && break
    sleep 0.1
done
[ -n "$APP_PID" ] || { echo "ERROR: app did not start"; exit 1; }

echo "==> waiting for the first successful refresh"
for _ in $(seq 1 50); do
    grep -q "refresh ok" "$LOG_FILE" 2>/dev/null && break
    sleep 0.2
done

echo "==> sampling CPU of pid $APP_PID every second for ${WINDOW}s"
: > "$SAMPLES"
for _ in $(seq 1 "$WINDOW"); do
    ps -p "$APP_PID" -o %cpu= >> "$SAMPLES" || true
    sleep 1
done

echo "==> quitting"
tmux send-keys -t "$SESSION" q
sleep 1
tmux kill-session -t "$SESSION" 2>/dev/null || true
kill "$SERVER_PID" 2>/dev/null || true
wait "$SERVER_PID" 2>/dev/null || true

AVG=$(awk '{ sum += $1; count += 1 } END { if (count > 0) printf "%.2f", sum / count; else print "n/a" }' "$SAMPLES")
MAX=$(awk 'BEGIN { max = 0 } { if ($1 > max) max = $1 } END { printf "%.1f", max }' "$SAMPLES")
RENDERS=$(grep -c "render ok" "$LOG_FILE" || true)
REFRESH_OK=$(grep "refresh ok" "$LOG_FILE" | head -1 || true)

echo "================================================================"
echo "M4-04 measurement report"
echo "  window seconds:        ${WINDOW}"
echo "  fixture delay (ms):    ${DELAY_MS}"
echo "  samples:               $(wc -l < "$SAMPLES" | tr -d ' ')"
echo "  avg CPU %:             ${AVG}"
echo "  max CPU %:             ${MAX}"
echo "  render lines traced:   ${RENDERS}  (~1 Hz tick expected)"
echo "  first refresh trace:   ${REFRESH_OK}"
echo "  trace log:             ${LOG_FILE}"
echo "================================================================"
