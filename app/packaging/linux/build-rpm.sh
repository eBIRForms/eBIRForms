#!/usr/bin/env bash
# Packs the release binary (app/target/release/buwiz) as an .rpm with its
# .desktop entry and icons, through cargo-generate-rpm and the
# [package.metadata.generate-rpm] table in app/Cargo.toml. Run from app/
# after `cargo build --release`, with cargo-generate-rpm 0.21.0 and
# rsvg-convert installed.
#
# Requires come from the binary's shared libraries (libgtk-4.so.1,
# libwebkitgtk-6.0.so.4, libdbus-1.so.3, ...), so one package installs on
# Fedora and openSUSE alike, whatever each names its GTK packages.
set -euo pipefail

fail() { echo "build-rpm: $*" >&2; exit 1; }

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
case "$(uname -m)" in
  x86_64) arch=x64 ;;
  aarch64) arch=arm64 ;;
  *) fail "unsupported machine $(uname -m)" ;;
esac
[ -x target/release/buwiz ] || fail "missing target/release/buwiz; build it first"
want=0.21.0
have=$(cargo generate-rpm --version 2>/dev/null | awk '{print $NF}' || true)
[ "$have" = "$want" ] || fail "needs cargo-generate-rpm $want (have ${have:-none}): cargo install --locked cargo-generate-rpm --version $want"

# The 256 px icon Cargo.toml's assets list.
mkdir -p target/linux-icons
rsvg-convert -w 256 -h 256 packaging/icon.svg -o target/linux-icons/com.buwiz.oss.png

out=target/rpm/Buwiz-${version}-linux-${arch}.rpm
mkdir -p target/rpm
rm -f "$out"
cargo generate-rpm --auto-req builtin -o "$out"
echo "$out"
