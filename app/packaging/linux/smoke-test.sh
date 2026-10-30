#!/usr/bin/env bash
# Installs the Linux packages in clean containers of this machine's
# architecture, through install-linux.sh as a user would:
#   ubuntu:24.04   the .deb (apt), then the .tar.gz for a non-root user
#   fedora:latest  the .rpm (dnf), whose requires must name GTK 4 and
#                  WebKitGTK 6.0 by soname
# Each checks the binary, the .desktop entry and the icons land, and that
# every shared library the binary needs resolves.
#
#   app/packaging/linux/smoke-test.sh <deb> <rpm> <tar.gz>
#
# Environment: VERSION. Needs docker.
set -euo pipefail

fail() { echo "smoke-test: $*" >&2; exit 1; }

[ "$#" -eq 3 ] || fail "usage: smoke-test.sh <deb> <rpm> <tar.gz>"
: "${VERSION:?VERSION is required}"
root="$(cd "$(dirname "$0")/../../.." && pwd)"
case "$(uname -m)" in
  x86_64) arch=x64 ;;
  aarch64) arch=arm64 ;;
  *) fail "unsupported machine $(uname -m)" ;;
esac

# The files and a manifest.json for them, as downloads.buwiz.com serves them.
packages=$(mktemp -d)
trap 'rm -rf "$packages"' EXIT
installer="$packages/Buwiz-$VERSION-linux-install.sh"
cp "$1" "$2" "$3" "$packages/"
cp "$root/app/packaging/linux/install-linux.sh" "$installer"
"$root/scripts/release/file-meta.sh" "$1" linux "$arch" deb "$packages/meta"
"$root/scripts/release/file-meta.sh" "$2" linux "$arch" rpm "$packages/meta"
"$root/scripts/release/file-meta.sh" "$3" linux "$arch" targz "$packages/meta"
"$root/scripts/release/file-meta.sh" "$installer" linux universal sh "$packages/meta"
python3 "$root/scripts/release/manifest.py" "$packages/meta" "$VERSION" "$packages" >/dev/null
chmod -R a+rX "$packages"
install_sh="sh /packages/$(basename "$installer") --from-dir /packages"

check_installed=$(cat <<'CHECK'
check_files() { # bin, data dir
  test -x "$1"
  test -f "$2/applications/com.buwiz.oss.desktop"
  test -f "$2/icons/hicolor/scalable/apps/com.buwiz.oss.svg"
  test -f "$2/icons/hicolor/256x256/apps/com.buwiz.oss.png"
  ldd "$1" > /tmp/ldd.txt
  if grep -q "not found" /tmp/ldd.txt; then cat /tmp/ldd.txt >&2; exit 1; fi
  echo "PASS: $1, its .desktop entry and icons, every library resolves"
}
CHECK
)

# A stalled mirror fails the step instead of holding the release.
in_time=(timeout --kill-after=30 900)

"${in_time[@]}" docker run --rm -v "$packages:/packages:ro" ubuntu:24.04 bash -euc "
  printf '%s\n' 'Acquire::Retries \"5\";' 'Acquire::http::Timeout \"30\";' > /etc/apt/apt.conf.d/80ci-network
  $check_installed
  $install_sh
  check_files /usr/bin/buwiz /usr/share
  dpkg -s buwiz | grep -qx 'Version: $VERSION'

  useradd --create-home tester
  su tester -c '$install_sh --tarball'
  check_files /home/tester/.local/opt/buwiz/buwiz /home/tester/.local/share
  test -L /home/tester/.local/bin/buwiz
  grep -qx 'Exec=\"/home/tester/.local/opt/buwiz/buwiz\"' /home/tester/.local/share/applications/com.buwiz.oss.desktop

  $install_sh --uninstall
  test ! -e /usr/bin/buwiz
  su tester -c '$install_sh --uninstall'
  test ! -e /home/tester/.local/opt/buwiz
  test ! -e /home/tester/.local/share/applications/com.buwiz.oss.desktop
"
echo "PASS: ubuntu:24.04 installed the .deb and the .tar.gz"

"${in_time[@]}" docker run --rm -v "$packages:/packages:ro" fedora:latest bash -euc "
  $check_installed
  command -v awk >/dev/null || dnf install -y gawk
  $install_sh
  check_files /usr/bin/buwiz /usr/share
  rpm -q --qf '%{VERSION}\n' buwiz | grep -qx '$VERSION'
  rpm -qR buwiz | tee /tmp/requires.txt
  grep -q '^libgtk-4\.so\.1' /tmp/requires.txt
  grep -q '^libwebkitgtk-6\.0\.so\.4' /tmp/requires.txt
"
echo "PASS: fedora:latest installed the .rpm"
