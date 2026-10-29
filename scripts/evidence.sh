#!/usr/bin/env bash
# The V4 evidence: every desktop and phone screen, light and dark, into
# internal/evidence/ (desktop-<screen>-<theme>.png, phone-<screen>-<theme>.png).
#
# - A throwaway database seeded by `seed_demo --full`: Juana Santos
#   (000-123-453-00000), drafts, and 1601C returns left Queued, Sending,
#   Failed and Sent.
# - Each screen opens through the app's BUWIZ_* launch hooks.
# - Desktop: the app's window goes to AeroSpace workspace 3 for as long as
#   the app runs, `screencapture -l <window>` takes it.
# - Phone: the iOS simulator (iPhone 17 Pro) is booted at the start and shut
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
OUT=$ROOT/internal/evidence
mkdir -p "$OUT"
TMP=$(mktemp -d)
KEY=0000000000000000000000000000000000000000000000000000000000000000
WHICH=${1:-all}
FAILED=""
DEVICE_NAME="iPhone 17 Pro"
UDID=""
APP_PID=""

cleanup() {
    [ -n "$APP_PID" ] && kill "$APP_PID" 2>/dev/null || true
    if [ -n "$UDID" ]; then
        xcrun simctl shutdown "$UDID" 2>/dev/null || true
        osascript -e 'tell application "Simulator" to quit' >/dev/null 2>&1 || true
    fi
    rm -rf "$TMP"
}
trap cleanup EXIT

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
check() {
    local png=$1 want=$2 sd
    if [ ! -s "$png" ]; then FAILED+=" $(basename "$png"):missing"; return; fi
    sd=$(magick "$png" -colorspace Gray -format '%[fx:standard_deviation]' info: 2>/dev/null || echo 0)
    if awk -v s="$sd" 'BEGIN{exit !(s < 0.02)}'; then FAILED+=" $(basename "$png"):blank"; return; fi
    if ! "$TMP/ocr" "$png" | cut -f2- | grep -qiF -- "$want"; then
        FAILED+=" $(basename "$png"):no-'$want'"
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
    # Every window of the app goes to workspace 3, for as long as it runs,
    # floating (so it keeps its own size whatever else shares the space).
    (
        while kill -0 "$APP_PID" 2>/dev/null; do
            aerospace list-windows --all --format '%{window-id}|%{app-pid}|%{workspace}' 2>/dev/null |
                awk -F'|' -v p="$APP_PID" '$2==p && $3!="3"{print $1}' |
                while read -r id; do
                    aerospace move-node-to-workspace --window-id "$id" 3
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
    "long-list-search|BUWIZ_OPEN=1601EQ:2026:3 BUWIZ_PICK=list|Search"
    "schedule|BUWIZ_OPEN=1601C:2026:10 BUWIZ_PICK=schedule|Add"
    "row-page|BUWIZ_OPEN=1601C:2026:10 BUWIZ_PICK=row|Delete row"
    "new-taxpayer|BUWIZ_SHEET=new-taxpayer|Registered name"
    "taxpayer-sheet|BUWIZ_SHEET=profile|Tax profile"
    "start-return|BUWIZ_SHEET=start:1701Q|Start return"
    "sent|BUWIZ_STATUS=sent|Where it is"
    "failed|BUWIZ_STATUS=failed|wasn't sent"
    "print-preview|BUWIZ_PRINT=1701Q|Print or save as PDF"
    "search|BUWIZ_SHEET=search|Cancel"
    "settings|BUWIZ_ROUTE=settings|Sign in to send"
)

phone_shot() {
    local name=$1 theme=$2 hooks=$3 want=$4 wait=${5:-${PHONE_WAIT:-12}} png dir
    png="$OUT/phone-$name-$theme.png"
    dir="$(xcrun simctl get_app_container "$UDID" com.buwiz.oss data)/Library/Application Support/Buwiz"
    fresh_db "$dir"
    mv "$dir/bir_data.db" "$dir/bir_data.db.tmp" && mv "$dir/bir_data.db.tmp" "$dir/bir_data.db"
    { echo "BIR_DB_KEY=$KEY"; echo "BUWIZ_THEME=$theme"; for kv in $hooks; do echo "$kv"; done; } >"$dir/launch.env"
    xcrun simctl ui "$UDID" appearance "$theme" 2>/dev/null || true
    xcrun simctl terminate "$UDID" com.buwiz.oss 2>/dev/null || true
    xcrun simctl launch "$UDID" com.buwiz.oss >/dev/null
    sleep "$wait"
    xcrun simctl io "$UDID" screenshot "$png" >/dev/null 2>&1
    check "$png" "$want"
    echo "phone-$name-$theme"
}

if [ "$WHICH" = all ] || [ "$WHICH" = phone ]; then
    (cd app && cargo build -q --locked --target aarch64-apple-ios-sim)
    UDID=$(xcrun simctl list devices available | grep -F "$DEVICE_NAME (" | head -1 | sed -E 's/.*\(([0-9A-F-]+)\).*/\1/')
    xcrun simctl boot "$UDID" 2>/dev/null || true
    xcrun simctl bootstatus "$UDID" -b >/dev/null
    # The app bundle, as app/ios.sh packs it.
    BUNDLE="$TMP/Buwiz.app"
    mkdir -p "$BUNDLE"
    cp app/target/aarch64-apple-ios-sim/debug/buwiz "$BUNDLE/Buwiz"
    sed -n '/^cat > "$BUNDLE\/Info.plist" <<PLIST/,/^PLIST/p' app/ios.sh | sed '1d;$d' |
        sed 's/\$APP/Buwiz/g; s/\$ID/com.buwiz.oss/g' >"$BUNDLE/Info.plist"
    codesign --force --sign - "$BUNDLE/Buwiz" 2>/dev/null || true
    xcrun simctl install "$UDID" "$BUNDLE"
    for theme in light dark; do
        for spec in "${PHONE[@]}"; do
            IFS='|' read -r name hooks want wait <<<"$spec"
            wanted "$name" && phone_shot "$name" "$theme" "$hooks" "$want" "$wait"
        done
    done
    xcrun simctl ui "$UDID" appearance light 2>/dev/null || true
fi

if [ -n "$FAILED" ]; then
    echo "evidence: checks failed:$FAILED" >&2
    exit 1
fi
echo "evidence: every shot taken and checked -> $OUT"
