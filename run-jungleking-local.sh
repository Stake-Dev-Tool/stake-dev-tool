#!/usr/bin/env bash
set -Eeuo pipefail
set -m

TOOL_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
FRONTEND_DIR="${JUNGLE_KING_DIR:-$TOOL_DIR/../jungle-king/web-sdk}"
TEST_URL='https://localhost:3001/test/?gameUrl=http%3A%2F%2Flocalhost%3A5174&gameSlug=jungleking'
PIDS=()

frontend_ready() { curl -fsS --max-time 2 http://127.0.0.1:5174/ >/dev/null 2>&1; }
lgs_ready() { curl -kfsS --max-time 2 https://127.0.0.1:3001/api/devtool/status >/dev/null 2>&1; }

wait_for() {
  local label="$1" check="$2"
  for ((i = 0; i < 120; i++)); do "$check" && return; sleep 1; done
  echo "Timed out waiting for $label." >&2
  exit 1
}

cleanup() {
  local status=$?
  trap - EXIT INT TERM
  for pid in "${PIDS[@]}"; do kill -TERM -- "-$pid" 2>/dev/null || true; done
  wait 2>/dev/null || true
  exit "$status"
}
trap cleanup EXIT INT TERM

[[ -d "$FRONTEND_DIR" ]] || {
  echo 'Set JUNGLE_KING_DIR to the Jungle King web-sdk directory.' >&2
  exit 1
}
[[ -f "$TOOL_DIR/math/jungleking/index.json" ]] || {
  echo "Missing published math: $TOOL_DIR/math/jungleking" >&2
  exit 1
}
command -v pnpm >/dev/null || { echo 'pnpm is required.' >&2; exit 1; }
command -v rustup >/dev/null || { echo 'rustup is required.' >&2; exit 1; }

if ! frontend_ready; then
  echo 'Starting Jungle King frontend on :5174...'
  (cd "$FRONTEND_DIR" && exec pnpm --filter jungleking exec vite dev --host --port 5174 --strictPort) &
  PIDS+=("$!")
fi
wait_for 'Jungle King frontend' frontend_ready

if ! lgs_ready; then
  echo 'Starting Stake Dev Tool LGS on :3001...'
  (
    cd "$TOOL_DIR"
    export LGS_BIND_ADDR='127.0.0.1:3001'
    export LGS_MATH_DIR="$TOOL_DIR/math"
    export RUST_LOG='info'
    exec rustup run 1.90.0 cargo run -p lgs --release
  ) &
  PIDS+=("$!")
fi
wait_for 'Stake Dev Tool LGS' lgs_ready

if command -v open >/dev/null; then open "$TEST_URL"; else xdg-open "$TEST_URL"; fi
echo "Stack ready: $TEST_URL"

if ((${#PIDS[@]})); then
  echo 'Press Ctrl-C to stop the services started by this script.'
  while :; do
    for pid in "${PIDS[@]}"; do
      kill -0 "$pid" 2>/dev/null || { echo 'A stack service exited unexpectedly.' >&2; exit 1; }
    done
    sleep 2
  done
fi
