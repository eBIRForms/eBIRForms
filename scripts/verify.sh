#!/usr/bin/env bash
# The finish line. One PASS/FAIL line per criterion; exits 1 if any fails.
#
#   ./scripts/verify.sh            every criterion
#   ./scripts/verify.sh V1 V3      just these
#
# The criteria are fixed: add new ones, never remove or loosen one.
#   V1  no secrets in the tree; built binaries carry no BIR secret strings
#   V2  locked builds: root (all targets), app, iOS simulator, Android
#   V3  workspace tests green, tests on in CI, no test removed or ignored vs main
#   V4  screenshots of every screen, light and dark, newer than the app source
#   V5  1702RT expand-all scroll bench: no frame over 8.3 ms (fresh log)
#   V6  submission e2e and attack tests pass (server repo)
#   V7  CI green on macOS, Windows and Linux for this commit
#   V8  SECURITY / PRIVACY / RELAY-PROTOCOL docs exist; README is run-only
#
# Environment: BUWIZ_SERVER_DIR (the buwiz-server worktree for V6, default
# ../buwiz-server-goal), VERIFY_BASE (the ref V3 compares tests against,
# default origin/main).
set -uo pipefail
cd "$(dirname "$0")/.."
export RUSTC_WRAPPER=

ROOT=$(pwd)
EVIDENCE="$ROOT/internal/evidence"
SERVER_DIR="${BUWIZ_SERVER_DIR:-$ROOT/../buwiz-server-goal}"
BASE="${VERIFY_BASE:-origin/main}"
LOGS=$(mktemp -d)
FAILED=0

pass() { printf 'PASS  %-3s %s\n' "$1" "$2"; }
fail() { printf 'FAIL  %-3s %s\n' "$1" "$2"; FAILED=1; }
run() { local log="$LOGS/$1.log"; shift; "$@" >"$log" 2>&1; }
SELECTED=("$@")
selected() { [ ${#SELECTED[@]} -eq 0 ] || [[ " ${SELECTED[*]} " == *" $1 "* ]]; }

# The newest modification time under app/src, in seconds: evidence older
# than this was taken of code that has since changed.
newest_src() { find app/src -type f -name '*.rs' -exec stat -f '%m' {} + | sort -n | tail -1; }

android_build() {
    local sdk="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Library/Android/sdk}}"
    local ndk="${ANDROID_NDK_HOME:-$(ls -d "$sdk"/ndk/* 2>/dev/null | sort -V | tail -1)}"
    local tc="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin" triple=aarch64-linux-android min=29
    [ -d "$tc" ] || { echo "no Android NDK under $sdk"; return 1; }
    (cd app && ANDROID_NDK_HOME="$ndk" ANDROID_NDK_ROOT="$ndk" PATH="$tc:$PATH" \
        CC_aarch64_linux_android="$tc/${triple}${min}-clang" AR_aarch64_linux_android="$tc/llvm-ar" \
        CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$tc/${triple}${min}-clang" \
        cargo build --locked -p buwiz-android --target "$triple")
}

# ---- V2 first: V1 scans what it builds -------------------------------------
V2_OK=1
if selected V2 || selected V1; then
    run v2-root cargo build --locked --all-targets || { V2_OK=0; V2_WHY="root build (see $LOGS/v2-root.log)"; }
    [ $V2_OK = 1 ] && { run v2-app sh -c 'cd app && cargo build --locked' || { V2_OK=0; V2_WHY="app build ($LOGS/v2-app.log)"; }; }
    [ $V2_OK = 1 ] && { run v2-ios sh -c 'cd app && cargo build --locked --target aarch64-apple-ios-sim' || { V2_OK=0; V2_WHY="iOS build ($LOGS/v2-ios.log)"; }; }
    [ $V2_OK = 1 ] && { run v2-android android_build || { V2_OK=0; V2_WHY="Android build ($LOGS/v2-android.log)"; }; }
fi

# ---- V1 ---------------------------------------------------------------------
if selected V1; then
    if ! run v1-secrets ./scripts/check-secrets.sh; then
        fail V1 "check-secrets found something ($LOGS/v1-secrets.log)"
    elif [ $V2_OK = 0 ]; then
        fail V1 "binaries not built: $V2_WHY"
    else
        bins=(app/target/debug/buwiz app/target/aarch64-apple-ios-sim/debug/buwiz app/target/aarch64-linux-android/debug/libbuwiz_android.so)
        hits=""
        for b in "${bins[@]}"; do
            [ -f "$b" ] || { hits+="missing $b; "; continue; }
            # No exceptions: any passphrase or SFTP text at all, and BIR's
            # dispatcher and upload hosts. (The printed forms' footer names
            # BIR's public website, so the bare domain is not on the list.)
            found=$(strings -a "$b" | grep -i -E 'passphrase|tindispatcher|dispatcher\.php|birgovph|ebf2\.bir|ws2\.bir|s[f]tp' | head -3)
            [ -n "$found" ] && hits+="$b: $(echo "$found" | tr '\n' '|'); "
        done
        [ -z "$hits" ] && pass V1 "tree clean; ${#bins[@]} binaries carry no BIR secret strings" || fail V1 "$hits"
    fi
fi

if selected V2; then
    [ $V2_OK = 1 ] && pass V2 "locked builds: root all-targets, app, iOS sim, Android" || fail V2 "$V2_WHY"
fi

# ---- V3 ---------------------------------------------------------------------
if selected V3; then
    why=""
    run v3-test cargo test --locked --workspace || why+="cargo test --workspace failed ($LOGS/v3-test.log); "
    grep -q 'cargo test' .github/workflows/ci.yml || why+="CI does not run cargo test; "
    if git rev-parse -q --verify "$BASE" >/dev/null; then
        count() { git grep -h -E "$1" "$2" -- '*.rs' ':!nsh' ':!blaze' 2>/dev/null | wc -l | tr -d ' '; }
        count_wt() { grep -r -h -E "$1" --include='*.rs' crates app/src app/android 2>/dev/null | wc -l | tr -d ' '; }
        t0=$(count '#\[(tokio::)?test' "$BASE"); t1=$(count_wt '#\[(tokio::)?test')
        i0=$(count '#\[ignore' "$BASE"); i1=$(count_wt '#\[ignore')
        [ "$t1" -ge "$t0" ] || why+="tests went from $t0 to $t1; "
        [ "$i1" -le "$i0" ] || why+="#[ignore] went from $i0 to $i1; "
        deleted=$(git diff --name-status "$BASE" -- 'crates/*/tests/*' | awk '$1=="D"{print $2}' | head -3)
        [ -z "$deleted" ] || why+="deleted test files: $deleted; "
    else
        why+="base $BASE not found (git fetch); "
    fi
    [ -z "$why" ] && pass V3 "workspace tests green; CI runs them; tests $t0 -> $t1, ignored $i0 -> $i1" || fail V3 "$why"
fi

# ---- V4 ---------------------------------------------------------------------
DESKTOP_SCREENS="home library form form-schedule profile-panel new-taxpayer filings queue calendar taxpayers settings palette print-preview"
PHONE_SCREENS="home forms filings calendar form outline choice-sheet long-list-search schedule row-page new-taxpayer taxpayer-sheet start-return sent failed print-preview search settings queue"
if selected V4; then
    src=$(newest_src); missing=""; stale=""; unchecked=""; n=0
    for kind in desktop phone; do
        list=$DESKTOP_SCREENS; [ $kind = phone ] && list=$PHONE_SCREENS
        for s in $list; do for t in light dark; do
            f="$EVIDENCE/$kind-$s-$t.png"; n=$((n + 1))
            if [ ! -s "$f" ]; then missing+="$kind-$s-$t "
            elif [ "$(stat -f '%m' "$f")" -lt "$src" ]; then stale+="$kind-$s-$t "
            # evidence.sh's own check: not blank, and OCR found the screen's
            # text, recorded against this exact file's hash.
            elif ! grep -q -F "ok $kind-$s-$t.png $(shasum -a 256 "$f" | cut -d' ' -f1) " "$EVIDENCE/checks.txt" 2>/dev/null; then
                unchecked+="$kind-$s-$t "
            fi
        done; done
    done
    if [ -z "$missing$stale$unchecked" ]; then pass V4 "$n screenshots present, newer than app/src, each checked (not blank, screen text found)"
    else fail V4 "missing: ${missing:-none} | stale: ${stale:-none} | unchecked: ${unchecked:-none}"; fi
fi

# ---- V5 ---------------------------------------------------------------------
if selected V5; then
    log="$EVIDENCE/bench-1702rt.log"
    if [ ! -s "$log" ]; then fail V5 "no bench log; run scripts/bench-scroll.sh"
    elif [ "$(stat -f '%m' "$log")" -lt "$(newest_src)" ]; then fail V5 "bench log is older than app/src; rerun scripts/bench-scroll.sh"
    elif ! grep -q 'refresh 120' "$log"; then fail V5 "bench did not run at 120 Hz"
    else
        # nsh logs each over-budget frame as "frame 9.1ms (budget 8.3ms): ...";
        # count those between "mounted; scrolling" and "bench end".
        over=$(awk '/mounted; scrolling/{on=1;next} /bench end/{on=0} on && match($0,/frame [0-9]+(\.[0-9]+)?ms/){ms=substr($0,RSTART+6,RLENGTH-8); if (ms+0 > 8.3) n++} END{print n+0}' "$log")
        ended=$(grep -c 'bench end' "$log")
        # Each run must really have scrolled: the list's own scroll events
        # reached (within 5%) the bottom of a long form, many times over.
        short=$(awk '/bench end/{ if (match($0,/reached [0-9]+pt of [0-9]+pt, [0-9]+ scroll events/)) { split(substr($0,RSTART,RLENGTH),a,/[^0-9]+/); far=a[2]; max=a[3]; ev=a[4]; if (max < 5000 || far < 0.95*max || ev < 200) n++ } else n++ } END{print n+0}' "$log")
        if [ "$ended" -lt 2 ]; then fail V5 "bench finished $ended of 2 runs (40 and 160 pt/frame)"
        elif [ "$short" -gt 0 ]; then fail V5 "$short run(s) did not scroll the whole form (see 'reached' in $log)"
        elif [ "$over" -eq 0 ]; then pass V5 "1702RT expand-all scroll: 0 frames over 8.3 ms at 120 Hz, both runs scrolled the whole form"
        else fail V5 "$over scroll frames over 8.3 ms"; fi
    fi
fi

# ---- V6 ---------------------------------------------------------------------
if selected V6; then
    if [ ! -d "$SERVER_DIR" ]; then fail V6 "no server worktree at $SERVER_DIR"
    # The server's v6.sh builds its e2e workspace against this checkout
    # (uncommitted changes included), with a throwaway Postgres.
    elif ! run v6-e2e env BUWIZ_OSS_PATH="$ROOT" "$SERVER_DIR/scripts/v6.sh"; then
        fail V6 "server e2e/attack tests failed ($LOGS/v6-e2e.log)"
    else
        # Each required case must have run and passed, not been filtered out.
        missing=""
        for t in a_queued_1701q_is_sent_through_the_gatekeeper_and_sender \
                 tin_not_active ticket_works_once replay expired tamper revoked; do
            grep -E "test [a-z0-9_]*$t[a-z0-9_]* \.\.\. ok" "$LOGS/v6-e2e.log" >/dev/null || missing="$missing $t"
        done
        if [ -n "$missing" ]; then fail V6 "cases did not run and pass:$missing ($LOGS/v6-e2e.log)"
        else pass V6 "e2e (1701Q queue -> gatekeeper -> sender -> mock SFTP -> signed receipt -> Sent) and attack tests pass"; fi
    fi
fi

# ---- V7 ---------------------------------------------------------------------
if selected V7; then
    head=$(git rev-parse HEAD)
    if ! git diff --quiet HEAD -- . ':!internal'; then fail V7 "uncommitted changes: CI has not seen this tree"
    else
        jobs=$(gh run list --commit "$head" --workflow ci --json databaseId --jq '.[0].databaseId' 2>/dev/null)
        if [ -z "$jobs" ]; then fail V7 "no CI run for $head (push the branch)"
        else
            res=$(gh run view "$jobs" --json jobs --jq '.jobs[] | "\(.name)=\(.conclusion)"')
            bad=$(echo "$res" | grep -v '=success$')
            oses=$(echo "$res" | grep -c -i -E 'macos|windows|ubuntu')
            if [ -n "$bad" ]; then fail V7 "CI not green: $(echo "$bad" | tr '\n' ' ')"
            elif ! echo "$res" | grep -qi windows || ! echo "$res" | grep -qi ubuntu || ! echo "$res" | grep -qi macos; then fail V7 "CI lacks a macOS, Windows or Linux job"
            else pass V7 "CI run $jobs green ($oses OS jobs) for $head"; fi
        fi
    fi
fi

# ---- V8 ---------------------------------------------------------------------
if selected V8; then
    why=""
    for d in SECURITY.md PRIVACY.md RELAY-PROTOCOL.md; do
        [ -s "$d" ] || [ -s "docs/$d" ] || why+="$d missing; "
    done
    grep -n -i -E 'deploy|gcloud|google cloud|buwiz-server|\bserver\b' README.md >"$LOGS/v8.txt" && why+="README mentions: $(head -2 "$LOGS/v8.txt" | tr '\n' ' ')"
    [ -z "$why" ] && pass V8 "docs present; README is run-only" || fail V8 "$why"
fi

[ $FAILED = 0 ] && echo "ALL PASS" || echo "logs: $LOGS"
exit $FAILED
