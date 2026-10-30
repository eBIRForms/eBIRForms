#!/usr/bin/env bash
# The iOS-simulator dev loop (rox's app/ios-dev.sh): packs and launches the
# app (./ios.sh), while a watcher answers each save of a .rs file with a
# daemon build for aarch64-apple-ios-sim, which the running sim app applies
# as a live patch.
#
#   ./ios-dev.sh [device name]   default: iPhone 17 Pro
#   save src/*.rs                -> cargo watch -> patch -> remount in the sim
#   Ctrl-C                       -> everything stops
set -e
cd "$(dirname "$0")"
export RUSTC_WRAPPER=

# `--features live` as ios.sh's BUWIZ_LIVE build: ra-jit-live is in only then.
cargo watch -w src -x 'ra-jit build --target aarch64-apple-ios-sim --features live' &
WATCH=$!
trap 'kill $WATCH 2>/dev/null' EXIT

BUWIZ_LIVE=1 ./ios.sh "$@"
