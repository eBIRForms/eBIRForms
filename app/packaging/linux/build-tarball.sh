#!/usr/bin/env bash
# Packs the release binary (app/target/release/buwiz) as a .tar.gz with its
# .desktop entry, icons and install-desktop-entry.sh, for a per-user
# install without root. Run from app/ after `cargo build --release`.
set -euo pipefail

fail() { echo "build-tarball: $*" >&2; exit 1; }

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
case "$(uname -m)" in
  x86_64) arch=x64 ;;
  aarch64) arch=arm64 ;;
  *) fail "unsupported machine $(uname -m)" ;;
esac
[ -x target/release/buwiz ] || fail "missing target/release/buwiz; build it first"

name=Buwiz-${version}-linux-${arch}
root=target/tarball/$name
rm -rf "$root" "$root.tar.gz"
install -Dm755 target/release/buwiz "$root/buwiz"
install -Dm755 packaging/linux/install-desktop-entry.sh "$root/install-desktop-entry.sh"
install -Dm644 packaging/linux/com.buwiz.oss.desktop "$root/com.buwiz.oss.desktop"
install -Dm644 packaging/icon.svg "$root/icons/hicolor/scalable/apps/com.buwiz.oss.svg"
mkdir -p "$root/icons/hicolor/256x256/apps"
rsvg-convert -w 256 -h 256 packaging/icon.svg -o "$root/icons/hicolor/256x256/apps/com.buwiz.oss.png"
tar -czf "$root.tar.gz" --owner=0 --group=0 -C target/tarball "$name"
echo "$root.tar.gz"
