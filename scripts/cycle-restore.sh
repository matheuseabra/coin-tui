#!/usr/bin/env bash
# G4: ten consecutive start/refresh/quit cycles against the fixture server.
# Each cycle starts the release binary in a fresh detached tmux pane, waits for
# a successful refresh, sends `r` then `q`, and verifies the shell resumed
# (a `cycle-done` marker and no leftover app UI in the restored pane).
#
# Usage:
#   scripts/cycle-restore.sh              # 10 cycles on port 8137
set -euo pipefail

cd "$(dirname "$0")/.."

PORT="${FIXTURE_PORT:-8137}"
CYCLES="${CYCLES:-10}"
SESSION="coin-cycles"

cleanup() {
    kill "${SERVER_PID:-}" 2>/dev/null || true
    tmux kill-session -t "$SESSION" 2>/dev/null || true
    wait "${SERVER_PID:-}" 2>/dev/null || true
}
trap cleanup EXIT

echo "==> building release binary (locked)"
cargo build --release --locked

echo "==> starting fixture server on 127.0.0.1:$PORT"
python3 scripts/fixture-server.py --port "$PORT" >/dev/null &
SERVER_PID=$!
for _ in $(seq 1 50); do
    curl -sf "http://127.0.0.1:$PORT/api/v3/global" >/dev/null && break
    sleep 0.1
done

pass=0
fail=0
for cycle in $(seq 1 "$CYCLES"); do
    LOG="/tmp/coin-tui-cycle-$$-$cycle.log"
    tmux kill-session -t "$SESSION" 2>/dev/null || true
    tmux new-session -d -s "$SESSION" -x 80 -y 24
    tmux send-keys -t "$SESSION" \
        "(${PWD}/target/release/coin-tui --base-url http://127.0.0.1:$PORT/ --log-file $LOG; echo cycle-done-$cycle) 2>&1" \
        Enter

    APP_OK=0
    for _ in $(seq 1 50); do
        grep -q "refresh ok" "$LOG" 2>/dev/null && { APP_OK=1; break; }
        sleep 0.2
    done
    if [ "$APP_OK" -ne 1 ]; then
        echo "cycle $cycle: FAIL (no successful refresh)"
        fail=$((fail + 1))
        tmux kill-session -t "$SESSION" 2>/dev/null || true
        continue
    fi

    tmux send-keys -t "$SESSION" r
    sleep 1
    tmux send-keys -t "$SESSION" q
    sleep 1

    PANE=$(tmux capture-pane -p -t "$SESSION" || true)
    if echo "$PANE" | grep -q "cycle-done-$cycle" &&
        ! echo "$PANE" | grep -q "q quit"; then
        echo "cycle $cycle: PASS (quit restored the shell)"
        pass=$((pass + 1))
    else
        echo "cycle $cycle: FAIL (no marker or UI leftovers)"
        fail=$((fail + 1))
    fi
    tmux kill-session -t "$SESSION" 2>/dev/null || true
done

kill "$SERVER_PID" 2>/dev/null || true
wait "$SERVER_PID" 2>/dev/null || true

echo "================================================================"
echo "G4-2 cycles: $pass passed, $fail failed (of $CYCLES)"
echo "================================================================"
[ "$fail" -eq 0 ]