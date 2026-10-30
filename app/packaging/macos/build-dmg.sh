#!/usr/bin/env bash
# Packs the two macOS release builds as a universal Buwiz.app inside a DMG,
# signed with a Developer ID and notarized when the inputs are set.
# Run from app/ after:
#   cargo build --locked --release --target aarch64-apple-darwin
#   cargo build --locked --release --target x86_64-apple-darwin
#
# Environment:
#   VERSION               required, MAJOR.MINOR.PATCH
#   SIGNING_IDENTITY      codesign identity (name or SHA-1); default "-"
#                         (ad-hoc, local smoke tests only)
#   KEYCHAIN_PATH         optional keychain for codesign
#   NOTARY_KEY_PATH       App Store Connect API .p8 key file; all three
#   NOTARY_KEY_ID         NOTARY_* turn notarization on (CI)
#   NOTARY_ISSUER_ID
#   APPLE_ID              without the NOTARY_* key: an Apple ID, its team
#   APPLE_TEAM_ID         and an app-specific password; all three turn
#   APP_PASSWORD          notarization on (local, `just dmg`)
#   REQUIRE_NOTARIZATION  "1": fail unless a real identity and one of the
#                         two notarization inputs are set
#
# Prints the DMG's path: target/release-artifacts/Buwiz-$VERSION-macos-universal.dmg
set -euo pipefail
cd "$(dirname "$0")/../.."

fail() { echo "build-dmg: $*" >&2; exit 1; }

VERSION="${VERSION:-}"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "VERSION must be MAJOR.MINOR.PATCH (got '$VERSION')"
SIGNING_IDENTITY="${SIGNING_IDENTITY:--}"
NOTARY_KEY_PATH="${NOTARY_KEY_PATH:-}"
NOTARY_KEY_ID="${NOTARY_KEY_ID:-}"
NOTARY_ISSUER_ID="${NOTARY_ISSUER_ID:-}"

notary_set=0
for v in NOTARY_KEY_PATH NOTARY_KEY_ID NOTARY_ISSUER_ID; do
  if [ -n "${!v}" ]; then notary_set=$((notary_set + 1)); fi
done
[ "$notary_set" -eq 0 ] || [ "$notary_set" -eq 3 ] || fail "set all of NOTARY_KEY_PATH, NOTARY_KEY_ID, NOTARY_ISSUER_ID or none"
NOTARIZE=0
notary_auth=()
if [ "$notary_set" -eq 3 ]; then
  NOTARIZE=1
  notary_auth=(--key "$NOTARY_KEY_PATH" --key-id "$NOTARY_KEY_ID" --issuer "$NOTARY_ISSUER_ID")
else
  # The Apple ID alternative, only when the API key is absent.
  APPLE_ID="${APPLE_ID:-}"
  APPLE_TEAM_ID="${APPLE_TEAM_ID:-}"
  APP_PASSWORD="${APP_PASSWORD:-}"
  apple_set=0
  for v in APPLE_ID APPLE_TEAM_ID APP_PASSWORD; do
    if [ -n "${!v}" ]; then apple_set=$((apple_set + 1)); fi
  done
  [ "$apple_set" -eq 0 ] || [ "$apple_set" -eq 3 ] || fail "set all of APPLE_ID, APPLE_TEAM_ID, APP_PASSWORD or none"
  if [ "$apple_set" -eq 3 ]; then
    NOTARIZE=1
    notary_auth=(--apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" --password "$APP_PASSWORD")
  fi
fi
AD_HOC=0; [ "$SIGNING_IDENTITY" = "-" ] && AD_HOC=1
if [ "${REQUIRE_NOTARIZATION:-0}" = "1" ]; then
  [ "$AD_HOC" -eq 0 ] || fail "REQUIRE_NOTARIZATION=1 needs a Developer ID SIGNING_IDENTITY"
  [ "$NOTARIZE" -eq 1 ] || fail "REQUIRE_NOTARIZATION=1 needs NOTARY_KEY_PATH, NOTARY_KEY_ID and NOTARY_ISSUER_ID, or APPLE_ID, APPLE_TEAM_ID and APP_PASSWORD"
fi

APP_NAME=Buwiz
BUNDLE_ID=com.buwiz.oss
BIN=buwiz
OUT=target/release-artifacts
APP="$OUT/$APP_NAME.app"
DMG_NAME="$APP_NAME-$VERSION-macos-universal.dmg"
DMG="$OUT/$DMG_NAME"
keychain_args=()
[ -n "${KEYCHAIN_PATH:-}" ] && keychain_args=(--keychain "$KEYCHAIN_PATH")

for triple in aarch64-apple-darwin x86_64-apple-darwin; do
  [ -f "target/$triple/release/$BIN" ] || fail "missing target/$triple/release/$BIN; build it first"
done

notarize() { # file
  local result
  result=$(xcrun notarytool submit "$1" "${notary_auth[@]}" \
    --wait --timeout 1h --output-format json) || true
  echo "$result"
  local id status
  id=$(printf '%s' "$result" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("id",""))' 2>/dev/null || true)
  status=$(printf '%s' "$result" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("status",""))' 2>/dev/null || true)
  if [ "$status" != "Accepted" ]; then
    [ -n "$id" ] && xcrun notarytool log "$id" "${notary_auth[@]}" >&2 || true
    fail "notarization of $1 was not accepted (status: ${status:-none})"
  fi
}

# 1. The universal binary.
mkdir -p "$OUT"
rm -rf "$APP" "$DMG"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
lipo -create "target/aarch64-apple-darwin/release/$BIN" "target/x86_64-apple-darwin/release/$BIN" \
  -output "$APP/Contents/MacOS/$BIN"
archs=$(lipo -archs "$APP/Contents/MacOS/$BIN")
for a in arm64 x86_64; do
  case " $archs " in *" $a "*) ;; *) fail "universal binary lacks $a ($archs)" ;; esac
done

# 2. The bundle: Info.plist, and the icon when rsvg-convert is installed.
icon_key=
if command -v rsvg-convert >/dev/null; then
  iconset=$(mktemp -d)/AppIcon.iconset
  mkdir -p "$iconset"
  for s in 16 32 128 256 512; do
    rsvg-convert -w "$s" -h "$s" packaging/icon.svg -o "$iconset/icon_${s}x${s}.png"
    rsvg-convert -w $((s * 2)) -h $((s * 2)) packaging/icon.svg -o "$iconset/icon_${s}x${s}@2x.png"
  done
  iconutil -c icns "$iconset" -o "$APP/Contents/Resources/AppIcon.icns"
  icon_key="<key>CFBundleIconFile</key><string>AppIcon</string>"
else
  echo "build-dmg: rsvg-convert not found; the app ships without an icon" >&2
fi
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key><string>$BIN</string>
    <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
    <key>CFBundleName</key><string>$APP_NAME</string>
    <key>CFBundleDisplayName</key><string>$APP_NAME</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleVersion</key><string>$VERSION</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.finance</string>
    <key>NSHighResolutionCapable</key><true/>
    $icon_key
</dict>
</plist>
PLIST
plutil -lint "$APP/Contents/Info.plist"
xattr -cr "$APP"

# Staple and validate. Apple's ticket service can lag the notary's
# "Accepted" and drops connections now and then (error 68), so a failure
# gets four more tries before the build fails.
staple() {
  for attempt in 1 2 3 4 5; do
    if xcrun stapler staple "$1" && xcrun stapler validate "$1"; then
      return 0
    fi
    [ "$attempt" -lt 5 ] || break
    echo "stapling $1 failed (attempt $attempt); retrying in $((attempt * 20))s" >&2
    sleep $((attempt * 20))
  done
  fail "stapling $1 failed five times"
}

# 3. Sign (hardened runtime), notarize and staple the app.
if [ "$AD_HOC" -eq 1 ]; then
  codesign --force --options runtime --timestamp=none --sign - "$APP"
else
  codesign --force --options runtime --timestamp --sign "$SIGNING_IDENTITY" \
    ${keychain_args[@]+"${keychain_args[@]}"} "$APP"
fi
codesign --verify --strict --deep --verbose=2 "$APP"
if [ "$NOTARIZE" -eq 1 ]; then
  zip_dir=$(mktemp -d)
  ditto -c -k --keepParent "$APP" "$zip_dir/$APP_NAME.zip"
  notarize "$zip_dir/$APP_NAME.zip"
  rm -rf "$zip_dir"
  staple "$APP"
  spctl --assess --type execute --verbose=2 "$APP"
fi

# 4. The DMG: the app and an Applications link; signed, notarized, stapled.
stage=$(mktemp -d)
ditto "$APP" "$stage/$APP_NAME.app"
ln -s /Applications "$stage/Applications"
created=0
for attempt in 1 2 3; do
  if hdiutil create -volname "$APP_NAME" -srcfolder "$stage" -ov -format UDZO -fs HFS+ "$DMG"; then
    created=1; break
  fi
  echo "hdiutil create attempt $attempt failed; retrying" >&2
  sleep 5
done
rm -rf "$stage"
[ "$created" -eq 1 ] || fail "hdiutil create failed three times"
if [ "$AD_HOC" -eq 0 ]; then
  codesign --force --timestamp --sign "$SIGNING_IDENTITY" ${keychain_args[@]+"${keychain_args[@]}"} "$DMG"
fi
if [ "$NOTARIZE" -eq 1 ]; then
  notarize "$DMG"
  staple "$DMG"
  spctl --assess --type open --context context:primary-signature --verbose=2 "$DMG"
fi

# 5. The mounted DMG holds a valid, universal app.
mnt=$(mktemp -d)
trap 'hdiutil detach "$mnt" >/dev/null 2>&1 || true' EXIT
hdiutil attach "$DMG" -nobrowse -readonly -mountpoint "$mnt" >/dev/null
[ -d "$mnt/$APP_NAME.app" ] || fail "the DMG has no $APP_NAME.app"
[ -L "$mnt/Applications" ] || fail "the DMG has no Applications link"
codesign --verify --strict --deep --verbose=2 "$mnt/$APP_NAME.app"
lipo -archs "$mnt/$APP_NAME.app/Contents/MacOS/$BIN" | grep -q arm64
lipo -archs "$mnt/$APP_NAME.app/Contents/MacOS/$BIN" | grep -q x86_64
[ "$NOTARIZE" -eq 0 ] || xcrun stapler validate "$mnt/$APP_NAME.app"
hdiutil detach "$mnt" >/dev/null
trap - EXIT

echo "$DMG"
