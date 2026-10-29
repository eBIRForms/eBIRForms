#!/usr/bin/env bash
# The desktop dev loop: the app runs under ra-jit, and every save of a .rs
# file under src/ is answered with a patch the running app applies.
#
#   ./dev.sh        first build, then the window opens and stays patched
#   save src/*.rs   cargo watch runs `cargo ra-jit build`, then patch, remount
#   Ctrl-C          everything stops
set -e
cd "$(dirname "$0")"

cargo ra-jit build &          # warm the daemon while the watcher comes up
BUILD=$!
cargo watch -w src -x 'ra-jit build' &
WATCH=$!
trap 'kill $WATCH 2>/dev/null' EXIT
wait $BUILD

cargo ra-jit run
