#!/usr/bin/env bash
# The V4 evidence: every desktop and phone screen, light and dark, into
# internal/evidence/ (desktop-<screen>-<theme>.png, phone-<screen>-<theme>.png).
#
# - A throwaway database seeded by `seed_demo --full`: Juana Santos
#   (000-123-453-00000), drafts, and 1601C returns left Queued, Sending,
#   Failed and Sent.
# - Each screen opens through the app's BUWIZ_* launch hooks.
# - Desktop: the app's window goes to AeroSpace workspace $BUWIZ_WORKSPACE (2) for as long as
#   the app runs, `screencapture -l <window>` takes it.
# - Phone: its own iOS simulator ("Buwiz Evidence") is booted at the start and shut
#   down at the end (Simulator quit); `xcrun simctl io <udid> screenshot`.
# - Each shot must not be blank and must show the text its screen is known
#   by (Vision OCR, scripts/ocr.swift); a failed check fails the script.
#
#   ./scripts/evidence.sh            both
#   ./scripts/evidence.sh desktop    or phone
#   ONLY="form outline" ./scripts/evidence.sh phone    just those screens
set -euo pipefail
cd "$(dirname "$0")/.."
export RUSTC_WRAPPER=
ROOT=$(pwd)
# The AeroSpace workspace the app's windows run on while they are shot.
WORKSPACE="${BUWIZ_WORKSPACE:-2}"
OUT=$ROOT/internal/evidence
mkdir -p "$OUT"
TMP=$(mktemp -d)
KEY=0000000000000000000000000000000000000000000000000000000000000000
WHICH=${1:-all}
FAILED=""
# A simulator of its own (an iPhone 17 Pro, made on first use), so the
# script never boots, wipes or wedges a device someone else uses.
DEVICE_NAME="${BUWIZ_SIM:-Buwiz Evidence}"
UDID=""
APP_PID=""

cleanup() {
    [ -n "$APP_PID" ] && kill "$APP_PID" 2>/dev/null || true
    if [ -n "$UDID" ]; then
        timeout 60 xcrun simctl shutdown "$UDID" 2>/dev/null || true
        osascript -e 'tell application "Simulator" to quit' >/dev/null 2>&1 || true
    fi
    rm -rf "$TMP"
}
trap cleanup EXIT

# What the shots are of: a hash of every app/src source, taken before the
# builds below (verify.sh V4 requires it to match the current sources).
SRC=$(find app/src -type f -name '*.rs' | LC_ALL=C sort | xargs shasum -a 256 | shasum -a 256 | cut -c1-16)

# ---- the seeded database ---------------------------------------------------
cargo build -q --locked -p bir-app-core --example seed_demo
BIR_DATABASE_PATH="$TMP/seed/bir_data.db" BIR_DB_KEY=$KEY cargo run -q --locked -p bir-app-core --example seed_demo -- --full
swiftc -O scripts/ocr.swift -o "$TMP/ocr" 2>/dev/null
swiftc -O scripts/winid.swift -o "$TMP/winid" 2>/dev/null

# Copies the seeded database (its WAL too) into directory $1.
fresh_db() {
    rm -rf "$1"; mkdir -p "$1"
    for f in "$TMP"/seed/bir_data.db*; do
        case "$f" in *.owner.lock) ;; *) cp "$f" "$1/" ;; esac
    done
}

# check <png> <expected text>: not blank, and the text is on it.
# Each result goes to checks.txt (verify.sh V4 reads it): "ok <file>
# <sha256> src=<app/src hash> <text>" for a shot that passed. A shot that fails is renamed
# <name>.failed.png, so no failed shot keeps the name V4 looks for.
check() {
    local png=$1 want=$2 sd name why=""
    name=$(basename "$png")
    if [ ! -s "$png" ]; then
        why=missing
    else
        sd=$(magick "$png" -colorspace Gray -format '%[fx:standard_deviation]' info: 2>/dev/null || echo 0)
        if awk -v s="$sd" 'BEGIN{exit !(s < 0.02)}'; then why=blank
        elif ! "$TMP/ocr" "$png" | cut -f2- | grep -qiF -- "$want"; then why="no-'$want'"; fi
    fi
    if [ -n "$why" ]; then
        FAILED+=" $name:$why"
        [ -f "$png" ] && mv "$png" "${png%.png}.failed.png"
        echo "FAIL $name $why" >>"$TMP/checks"
    else
        rm -f "${png%.png}.failed.png"
        echo "ok $name $(shasum -a 256 "$png" | cut -d' ' -f1) src=$SRC $want" >>"$TMP/checks"
    fi
}

# ---- desktop -----------------------------------------------------------------
# name | hooks | text the screen shows | seconds to wait (default 9 / 12)
DESKTOP=(
    "home|BUWIZ_ROUTE=home|Due in the next 30 days"
    "library|BUWIZ_ROUTE=forms|Form library"
    "form|BUWIZ_OPEN=1701Q:2026:3|Quarterly Income Tax Return"
    "form-schedule|BUWIZ_OPEN=1700:2025:0 BUWIZ_PICK=schedule|Name of Employer"
    "profile-panel|BUWIZ_SHEET=profile|Tax profile"
    "new-taxpayer|BUWIZ_SHEET=new-taxpayer|Registered name"
    "filings|BUWIZ_ROUTE=filings|Sent returns stay exactly as sent"
    "queue|BUWIZ_ROUTE=queue|Waiting to send"
    "calendar|BUWIZ_ROUTE=calendar|Every deadline"
    "taxpayers|BUWIZ_ROUTE=taxpayers|Address and contact"
    "settings|BUWIZ_ROUTE=settings|Sign in to send"
    "palette|BUWIZ_SHEET=palette|Go to"
    "print-preview|BUWIZ_PRINT=1701Q|Quarterly Income Tax|22"
    "delete-confirm|BUWIZ_ROUTE=filings BUWIZ_SHEET=delete|Delete"
    "override-sheet|BUWIZ_SHEET=override|Applies to every taxpayer"
    "calendar-override|BUWIZ_ROUTE=calendar|Moved from"
    "amend|BUWIZ_AMEND=1601C|amends the return you sent|14"
    "edit-penalties|BUWIZ_OPEN=1701Q:2026:2 BUWIZ_UNLOCK=penalties|entering the penalties yourself|14"
    "edit-on-return|BUWIZ_OPEN=1701Q:2026:3 BUWIZ_UNLOCK=profile|On this return only|14"
    "toast|BUWIZ_ROUTE=home BUWIZ_TOAST=Taken_off_the_queue:_an_editable_draft_again.|editable draft again"
)

# Whether screen $1 is in this run (ONLY lists screens; empty: all).
wanted() { [ -z "${ONLY:-}" ] || [[ " $ONLY " == *" $1 "* ]]; }

desktop_shot() {
    local name=$1 theme=$2 hooks=$3 want=$4 wait=${5:-${DESK_WAIT:-9}} png wid=""
    png="$OUT/desktop-$name-$theme.png"
    fresh_db "$TMP/desk"
    # shellcheck disable=SC2086
    # BUWIZ_OFFSCREEN: the window stays off every screen even before the
    # window manager has moved it.
    env BIR_DATABASE_PATH="$TMP/desk/bir_data.db" BIR_DB_KEY=$KEY BUWIZ_THEME=$theme BUWIZ_OFFSCREEN=1 $hooks \
        "$ROOT/app/target/debug/buwiz" >"$TMP/desk.out" 2>&1 &
    APP_PID=$!
    # Every window of the app goes to the test workspace, for as long as it runs,
    # floating (so it keeps its own size whatever else shares the space).
    (
        while kill -0 "$APP_PID" 2>/dev/null; do
            aerospace list-windows --all --format '%{window-id}|%{app-pid}|%{workspace}' 2>/dev/null |
                awk -F'|' -v p="$APP_PID" -v w="$WORKSPACE" '$2==p && $3!=w{print $1}' |
                while read -r id; do
                    aerospace move-node-to-workspace --window-id "$id" "$WORKSPACE"
                    aerospace layout floating --window-id "$id" 2>/dev/null || true
                done
            sleep 0.2
        done
    ) &
    local mover=$!
    for _ in $(seq 1 150); do
        wid=$("$TMP/winid" "$APP_PID" | head -1)
        [ -n "$wid" ] && break
        kill -0 "$APP_PID" 2>/dev/null || break
        sleep 0.2
    done
    sleep "$wait"
    rm -f "$png"
    [ -n "$wid" ] && timeout 30 screencapture -x -o -l "$wid" "$png"
    kill "$APP_PID" 2>/dev/null || true
    wait "$APP_PID" 2>/dev/null || true
    APP_PID=""
    wait "$mover" 2>/dev/null || true
    check "$png" "$want"
    echo "desktop-$name-$theme"
}

if [ "$WHICH" = all ] || [ "$WHICH" = desktop ]; then
    (cd app && cargo build -q --locked)
    for theme in light dark; do
        for spec in "${DESKTOP[@]}"; do
            IFS='|' read -r name hooks want wait <<<"$spec"
            wanted "$name" && desktop_shot "$name" "$theme" "$hooks" "$want" "$wait"
        done
    done
fi

# ---- phone (iOS simulator) ---------------------------------------------------
PHONE=(
    "home|BUWIZ_ROUTE=home|Due next"
    "forms|BUWIZ_ROUTE=forms|All forms"
    "filings|BUWIZ_ROUTE=filings|Drafts"
    "calendar|BUWIZ_ROUTE=calendar|Calendar"
    "form|BUWIZ_OPEN=1701Q:2026:3|Return details"
    "outline|BUWIZ_OPEN=1701Q:2026:3 BUWIZ_OUTLINE=1|Tax still due"
    "choice-sheet|BUWIZ_OPEN=1701Q:2026:3 BUWIZ_PICK=choice|Single Proprietor"
    "long-list-search|BUWIZ_OPEN=1606:2026:3 BUWIZ_PICK=list|province or RDO|20"
    "schedule|BUWIZ_OPEN=1601C:2026:10 BUWIZ_PICK=schedule|Add"
    "row-page|BUWIZ_OPEN=1601C:2026:10 BUWIZ_PICK=row|Delete row"
    "new-taxpayer|BUWIZ_SHEET=new-taxpayer|Registered name"
    "taxpayer-sheet|BUWIZ_SHEET=profile|Tax profile"
    "start-return|BUWIZ_SHEET=start:1701Q|Start return"
    "sent|BUWIZ_STATUS=sent|Where it is"
    "failed|BUWIZ_STATUS=failed|wasn't sent"
    "print-preview|BUWIZ_PRINT=1701Q|Print or save as PDF"
    "search|BUWIZ_SHEET=search|Cancel|20"
    "queue|BUWIZ_ROUTE=queue|Send queue"
    "queued|BUWIZ_STATUS=queued|Queued"
    "sending|BUWIZ_STATUS=sending|Sending"
    "attention|BUWIZ_STATUS=attention|confirmation"
    "how-sheet|BUWIZ_OPEN=1701Q:2026:3 BUWIZ_PICK=how|worked out|20"
    "payee-picker|BUWIZ_OPEN=1606:2026:3 BUWIZ_PICK=payee|payee|20"
    "delete-confirm|BUWIZ_ROUTE=filings BUWIZ_SHEET=delete|Delete"
    "settings|BUWIZ_ROUTE=settings|Sign in to send"
    "override-sheet|BUWIZ_SHEET=override|Applies to every taxpayer"
    "calendar-override|BUWIZ_ROUTE=calendar|moved from"
    "amend|BUWIZ_AMEND=1601C|amends the return you sent|20"
    "edit-penalties|BUWIZ_OPEN=1701Q:2026:2 BUWIZ_UNLOCK=penalties|entering this penalty yourself|20"
    "edit-on-return|BUWIZ_OPEN=1701Q:2026:3 BUWIZ_UNLOCK=profile|On this return only|20"
    "toast|BUWIZ_ROUTE=home BUWIZ_TOAST=Taken_off_the_queue:_an_editable_draft_again.|editable draft again"
)

phone_shot() {
    local name=$1 theme=$2 hooks=$3 want=$4 wait=${5:-${PHONE_WAIT:-12}} png dir
    png="$OUT/phone-$name-$theme.png"
    dir="$(xcrun simctl get_app_container "$UDID" com.buwiz.oss data)/Library/Application Support/Buwiz"
    fresh_db "$dir"
    mv "$dir/bir_data.db" "$dir/bir_data.db.tmp" && mv "$dir/bir_data.db.tmp" "$dir/bir_data.db"
    { echo "BIR_DB_KEY=$KEY"; echo "BUWIZ_THEME=$theme"; for kv in $hooks; do echo "$kv"; done; } >"$dir/launch.env"
    timeout 60 xcrun simctl ui "$UDID" appearance "$theme" 2>/dev/null || true
    timeout 60 xcrun simctl terminate "$UDID" com.buwiz.oss 2>/dev/null || true
    # A wedged simulator never answers a launch: fail the shot, don't hang.
    if ! timeout 180 xcrun simctl launch "$UDID" com.buwiz.oss >/dev/null; then
        FAILED+=" phone-$name-$theme.png:launch"
        return
    fi
    sleep "$wait"
    rm -f "$png"
    # The simulator writes the file itself, and macOS may not let it write
    # to an external volume: shoot into $TMP, then move it. A just-booted
    # simulator may not have its display yet: try again, for up to a minute.
    for _ in 1 2 3 4 5 6 7 8 9 10 11 12; do
        if timeout 60 xcrun simctl io "$UDID" screenshot "$TMP/shot.png" >/dev/null 2>"$TMP/shot.err"; then
            mv "$TMP/shot.png" "$png"
            break
        fi
        sleep 5
    done
    [ -s "$png" ] || sed 's/^/  simctl: /' "$TMP/shot.err" >&2
    check "$png" "$want"
    echo "phone-$name-$theme"
}

if [ "$WHICH" = all ] || [ "$WHICH" = phone ]; then
    (cd app && cargo build -q --locked --target aarch64-apple-ios-sim)
    UDID=$(xcrun simctl list devices available | grep -F "$DEVICE_NAME (" | head -1 | sed -E 's/.*\(([0-9A-F-]+)\).*/\1/')
    if [ -z "$UDID" ]; then
        runtime=$(xcrun simctl list runtimes -j | python3 -c 'import json,sys; print([r for r in json.load(sys.stdin)["runtimes"] if r["platform"]=="iOS" and r["isAvailable"]][-1]["identifier"])')
        UDID=$(xcrun simctl create "$DEVICE_NAME" "iPhone 17 Pro" "$runtime")
    fi
    xcrun simctl boot "$UDID" 2>/dev/null || true
    timeout 300 xcrun simctl bootstatus "$UDID" -b >/dev/null
    # The app bundle, as app/ios.sh packs it.
    BUNDLE="$TMP/Buwiz.app"
    mkdir -p "$BUNDLE"
    cp app/target/aarch64-apple-ios-sim/debug/buwiz "$BUNDLE/Buwiz"
    sed -n '/^cat > "$BUNDLE\/Info.plist" <<PLIST/,/^PLIST/p' app/ios.sh | sed '1d;$d' |
        sed 's/\$APP/Buwiz/g; s/\$ID/com.buwiz.oss/g' >"$BUNDLE/Info.plist"
    codesign --force --sign - "$BUNDLE/Buwiz" 2>/dev/null || true
    timeout 600 xcrun simctl install "$UDID" "$BUNDLE"
    # The first launch after an install is the slow one (the system prepares
    # the app): launch it once and let it settle before the first shot.
    timeout 180 xcrun simctl launch "$UDID" com.buwiz.oss >/dev/null 2>&1 || true
    sleep 20
    timeout 60 xcrun simctl terminate "$UDID" com.buwiz.oss 2>/dev/null || true
    for theme in light dark; do
        for spec in "${PHONE[@]}"; do
            IFS='|' read -r name hooks want wait <<<"$spec"
            wanted "$name" && phone_shot "$name" "$theme" "$hooks" "$want" "$wait"
        done
    done
    timeout 60 xcrun simctl ui "$UDID" appearance light 2>/dev/null || true
fi

# Merge this run's results into checks.txt: a retaken shot replaces its line.
touch "$OUT/checks.txt" "$TMP/checks"
awk 'NR==FNR{new[$2]=$0; next} !($2 in new)' "$TMP/checks" "$OUT/checks.txt" >"$TMP/merged"
cat "$TMP/checks" >>"$TMP/merged"
sort -k2 "$TMP/merged" >"$OUT/checks.txt"

if [ -n "$FAILED" ]; then
    echo "evidence: checks failed:$FAILED" >&2
    exit 1
fi
echo "evidence: every shot taken and checked -> $OUT"
