#!/usr/bin/env bash
# Packs and launches Buwiz in the iOS simulator: a debug
# aarch64-apple-ios-sim build of `buwiz`, a minimal .app around it,
# install + launch (rox's app/ios.sh).
#
#   ./ios.sh [device name]     default: iPhone 17 Pro
#   BUWIZ_FRESH=1 ./ios.sh     uninstall first: an empty database
#   BUWIZ_LIVE=1 ./ios.sh      build through `cargo ra-jit` (ios-dev.sh sets it)
#   BUWIZ_DETACH=1 ./ios.sh    launch and return instead of following the log
#   BUWIZ_OPEN=1701Q:2026:3    open that return once the app is up
#                              (BUWIZ_FOCUS=<n> then focuses the n-th box, BUWIZ_OUTLINE=1
#                              opens the outline: screenshots without taps)
#
# The database lives in the app's sandbox (Library/Application Support/
# Buwiz, set by `buwiz::mobile::prepare`); its key in the simulator's
# keychain.
set -e
cd "$(dirname "$0")"
export RUSTC_WRAPPER=

TARGET=aarch64-apple-ios-sim
APP="Buwiz"
ID=com.buwiz.oss
BUNDLE="target/ios-sim/$APP.app"

if [ -n "$BUWIZ_LIVE" ]; then
    # The daemon must know the binary it patches; ra-jit-live's macOS path
    # covers the simulator (a sim process is a macOS arm64 process).
    cargo ra-jit build --target "$TARGET"
else
    cargo build --target "$TARGET"
fi
BIN="target/$TARGET/debug/buwiz"

rm -rf "$BUNDLE"
mkdir -p "$BUNDLE"
cp "$BIN" "$BUNDLE/$APP"
cat > "$BUNDLE/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key><string>$APP</string>
    <key>CFBundleIdentifier</key><string>$ID</string>
    <key>CFBundleName</key><string>$APP</string>
    <key>CFBundleDisplayName</key><string>$APP</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleVersion</key><string>1</string>
    <key>CFBundleShortVersionString</key><string>0.1.0</string>
    <key>MinimumOSVersion</key><string>15.0</string>
    <key>UILaunchScreen</key><dict/>
    <key>UIDeviceFamily</key><array><integer>1</integer><integer>2</integer></array>
    <key>UISupportedInterfaceOrientations</key>
    <array>
        <string>UIInterfaceOrientationPortrait</string>
        <string>UIInterfaceOrientationLandscapeLeft</string>
        <string>UIInterfaceOrientationLandscapeRight</string>
    </array>
</dict>
</plist>
PLIST
codesign --force --sign - "$BUNDLE/$APP" 2>/dev/null || true

NAME="${1:-iPhone 17 Pro}"
DEVICE="$(xcrun simctl list devices available | grep -F "$NAME (" | head -1 | sed -E 's/.*\(([0-9A-F-]+)\).*/\1/')"
if [ -z "$DEVICE" ]; then
    echo "no available device named '$NAME'; pick one:" >&2
    xcrun simctl list devices available | grep -E "iPhone|iPad" >&2
    exit 1
fi

xcrun simctl boot "$DEVICE" 2>/dev/null || true
open -a Simulator --args -CurrentDeviceUDID "$DEVICE" 2>/dev/null || true
xcrun simctl bootstatus "$DEVICE" -b >/dev/null
if [ -n "$BUWIZ_FRESH" ]; then
    xcrun simctl uninstall "$DEVICE" "$ID" 2>/dev/null || true
fi
xcrun simctl install "$DEVICE" "$BUNDLE"
# The app's log in this terminal until it quits (BUWIZ_DETACH=1: launch
# and return).
CONSOLE=--console-pty
[ -n "$BUWIZ_DETACH" ] && CONSOLE=
SIMCTL_CHILD_BUWIZ_OPEN="${BUWIZ_OPEN:-}" \
SIMCTL_CHILD_BUWIZ_FOCUS="${BUWIZ_FOCUS:-}" \
SIMCTL_CHILD_BUWIZ_OUTLINE="${BUWIZ_OUTLINE:-}" \
SIMCTL_CHILD_NSH_DEVELOPMENT=1 \
SIMCTL_CHILD_RA_JIT_HOME="$HOME/.cache/ra-jit" \
    xcrun simctl launch --terminate-running-process $CONSOLE "$DEVICE" "$ID"
