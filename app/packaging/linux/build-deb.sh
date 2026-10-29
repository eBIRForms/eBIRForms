#!/usr/bin/env bash
# Packs the release binary (app/target/release/buwiz) as a .deb with its
# .desktop entry and icon. Run from app/ after `cargo build --release`.
set -euo pipefail

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
arch=$(dpkg --print-architecture)
root=target/deb/buwiz_${version}_${arch}
rm -rf "$root"
install -Dm755 target/release/buwiz "$root/usr/bin/buwiz"
install -Dm644 packaging/linux/com.buwiz.oss.desktop "$root/usr/share/applications/com.buwiz.oss.desktop"
install -Dm644 packaging/icon.svg "$root/usr/share/icons/hicolor/scalable/apps/com.buwiz.oss.svg"
if command -v rsvg-convert >/dev/null; then
  mkdir -p "$root/usr/share/icons/hicolor/256x256/apps"
  rsvg-convert -w 256 -h 256 packaging/icon.svg -o "$root/usr/share/icons/hicolor/256x256/apps/com.buwiz.oss.png"
fi
mkdir -p "$root/DEBIAN"
cat > "$root/DEBIAN/control" <<CONTROL
Package: buwiz
Version: $version
Architecture: $arch
Maintainer: Buwiz <noreply@buwiz.invalid>
Depends: libgtk-4-1 (>= 4.14), libwebkitgtk-6.0-4, libdbus-1-3
Section: utils
Priority: optional
Description: Offline filer for Philippine BIR tax returns
 Fill a form, check it, see deadlines and penalties, print it and queue
 it for sending. Every step works offline except sending.
CONTROL
dpkg-deb --root-owner-group --build "$root" "target/deb/buwiz_${version}_${arch}.deb"
echo "target/deb/buwiz_${version}_${arch}.deb"
