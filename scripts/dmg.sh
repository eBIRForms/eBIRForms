#!/usr/bin/env bash
# Builds a signed, notarized and stapled DMG on this Mac, notarizing with
# an Apple ID instead of CI's API key (`just dmg`):
#
#   BUWIZ_SIGNING_ENV=../buwiz-forms/.env just dmg
#
# The env file is never sourced: only these exact KEY=value lines are read
# from it, and nothing else in it enters this process.
#   APPLE_ID, APPLE_TEAM_ID, APP_PASSWORD   notarytool's Apple ID login
#   RELEASE_SIGNING_IDENTITY                Developer ID Application identity,
#   or SIGNING_IDENTITY                     preferred in that order
# The identity must be in the login keychain. Writes
# app/target/release-artifacts/Buwiz-$VERSION-macos-universal.dmg.
set -euo pipefail
cd "$(dirname "$0")/.."

fail() { echo "dmg: $*" >&2; exit 1; }

[ "$(uname -s)" = Darwin ] || fail "builds on macOS only"
env_file="${BUWIZ_SIGNING_ENV:-}"
[ -n "$env_file" ] || fail "set BUWIZ_SIGNING_ENV to the env file holding the signing keys"
[ -f "$env_file" ] || fail "no such file: $env_file"

# The value of the last exact KEY= line, without surrounding quotes.
read_key() {
  local value
  value=$(grep -E "^$1=" "$env_file" | tail -1 | cut -d= -f2- | tr -d '\r' || true)
  case "$value" in
    \"*\") value=${value#\"}; value=${value%\"} ;;
    \'*\') value=${value#\'}; value=${value%\'} ;;
  esac
  printf '%s' "$value"
}

apple_id=$(read_key APPLE_ID)
apple_team_id=$(read_key APPLE_TEAM_ID)
app_password=$(read_key APP_PASSWORD)
identity=$(read_key RELEASE_SIGNING_IDENTITY)
[ -n "$identity" ] || identity=$(read_key SIGNING_IDENTITY)
for pair in "APPLE_ID:$apple_id" "APPLE_TEAM_ID:$apple_team_id" "APP_PASSWORD:$app_password"; do
  [ -n "${pair#*:}" ] || fail "$env_file has no ${pair%%:*}= line"
done
[ -n "$identity" ] && [ "$identity" != "-" ] || fail "$env_file has no RELEASE_SIGNING_IDENTITY= or SIGNING_IDENTITY= line"

version="$(awk -F '"' '
  /^\[package\]$/ { in_package = 1; next }
  in_package && /^\[/ { exit }
  in_package && /^version[[:space:]]*=/ { print $2; exit }
' app/Cargo.toml)"
echo "Buwiz $version, signing as $identity, notarizing as $apple_id (team $apple_team_id)"

cd app
export MACOSX_DEPLOYMENT_TARGET=11.0 RUSTC_WRAPPER=''
rustup target add aarch64-apple-darwin x86_64-apple-darwin
cargo build --locked --release --target aarch64-apple-darwin
cargo build --locked --release --target x86_64-apple-darwin

# Only build-dmg.sh sees the login; the API key, if set in this shell,
# would win over it, so it is dropped.
env -u NOTARY_KEY_PATH -u NOTARY_KEY_ID -u NOTARY_ISSUER_ID \
  VERSION="$version" SIGNING_IDENTITY="$identity" REQUIRE_NOTARIZATION=1 \
  APPLE_ID="$apple_id" APPLE_TEAM_ID="$apple_team_id" APP_PASSWORD="$app_password" \
  ./packaging/macos/build-dmg.sh
