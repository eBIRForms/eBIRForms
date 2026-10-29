#!/usr/bin/env bash
# The 120 Hz scroll bench behind verify.sh V5: a release build opens 1702RT
# with every part expanded, scrolls to the bottom and back three times at
# 40 and at 160 points per frame, and nsh's frame monitor logs every frame
# that ran over budget. The result is internal/evidence/bench-1702rt.log.
#
# It uses a throwaway database with the demo taxpayer, puts the window on
# AeroSpace workspace 3, and quits the app it started. macOS only.
set -euo pipefail
cd "$(dirname "$0")/.."
export RUSTC_WRAPPER=

OUT=internal/evidence/bench-1702rt.log
TMP=$(mktemp -d)
mkdir -p internal/evidence
export BIR_DATABASE_PATH="$TMP/bir_data.db"
export BIR_DB_KEY=0000000000000000000000000000000000000000000000000000000000000000

# Every display must run at 120 Hz: at 60 Hz the budget is 16.7 ms and the
# frames between 8.3 and 16.7 ms would never be logged.
rates=$(system_profiler SPDisplaysDataType -json | python3 -c '
import json, sys
d = json.load(sys.stdin)
for g in d["SPDisplaysDataType"]:
    for s in g.get("spdisplays_ndrvs", []):
        if s.get("spdisplays_online") == "spdisplays_yes":
            print(s.get("_spdisplays_resolution", "").rsplit("@", 1)[-1].strip())')
if echo "$rates" | grep -qv '^120'; then
    echo "a display is not at 120 Hz: $rates" >&2
    exit 1
fi

# The sources the bench measures (verify.sh V5 requires a match).
SRC=$(find app/src -type f -name '*.rs' | LC_ALL=C sort | xargs shasum -a 256 | shasum -a 256 | cut -c1-16)
cargo build -q --locked -p bir-app-core --example seed_demo
cargo run -q --locked -p bir-app-core --example seed_demo
(cd app && cargo build -q --locked --release)
BIN=app/target/release/buwiz

: >"$OUT"
for speed in 40 160; do
    log="$TMP/jank-$speed.log"
    BUWIZ_JANK_LOG="$log" BUWIZ_FPS=1 BUWIZ_OPEN=1702RT BUWIZ_SCROLL_BENCH=$speed "$BIN" >"$TMP/app-$speed.out" 2>&1 &
    pid=$!
    moved=0
    for _ in $(seq 1 240); do
        if [ $moved = 0 ]; then
            id=$(aerospace list-windows --all --format '%{window-id}|%{app-pid}' 2>/dev/null | awk -F'|' -v p="$pid" '$2==p{print $1; exit}')
            [ -n "$id" ] && aerospace move-node-to-workspace --window-id "$id" 3 && moved=1
        fi
        grep -q 'bench end' "$log" 2>/dev/null && break
        kill -0 "$pid" 2>/dev/null || break
        sleep 1
    done
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    { echo "speed $speed pt/frame"; cat "$log" 2>/dev/null; } >>"$OUT"
done
{
    echo "refresh 120"
    echo "src $SRC"
    echo "commit $(git rev-parse --short HEAD) $(git diff --quiet -- app/src || echo '+uncommitted')"
    echo "date $(date -u +%FT%TZ)"
} >>"$OUT"
rm -rf "$TMP"

over=$(awk '/mounted; scrolling/{on=1;next} /bench end/{on=0} on && match($0,/frame [0-9]+(\.[0-9]+)?ms/){ms=substr($0,RSTART+6,RLENGTH-8); if (ms+0 > 8.3) n++} END{print n+0}' "$OUT")
echo "bench: $(grep -c 'bench end' "$OUT") runs finished, $over scroll frames over 8.3 ms -> $OUT"
