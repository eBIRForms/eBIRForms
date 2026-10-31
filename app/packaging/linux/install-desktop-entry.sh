#!/usr/bin/env bash
# Adds eBIRForms to the current user's app menu when it runs from the Linux
# tarball (the .deb and .rpm install these files system-wide instead).
#
#   ./install-desktop-entry.sh            install for this user
#   ./install-desktop-entry.sh --remove   undo
set -euo pipefail

APP_ID=com.buwiz.ebirforms
# 0.1.0's id (the app was Buwiz then): its launcher is replaced.
OLD_APP_ID=com.buwiz.oss
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN="$HERE/buwiz"
DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
ENTRY="$DATA/applications/$APP_ID.desktop"
ICONS=("scalable/apps/$APP_ID.svg" "256x256/apps/$APP_ID.png")

remove_old() {
  rm -f "$DATA/applications/$OLD_APP_ID.desktop"
  rm -f "$DATA/icons/hicolor/scalable/apps/$OLD_APP_ID.svg" "$DATA/icons/hicolor/256x256/apps/$OLD_APP_ID.png"
}

refresh() {
  if command -v update-desktop-database >/dev/null; then update-desktop-database "$DATA/applications" 2>/dev/null || true; fi
  if command -v gtk-update-icon-cache >/dev/null; then gtk-update-icon-cache -q -t "$DATA/icons/hicolor" 2>/dev/null || true; fi
}

if [ "${1:-}" = "--remove" ]; then
  rm -f "$ENTRY"
  for icon in "${ICONS[@]}"; do rm -f "$DATA/icons/hicolor/$icon"; done
  remove_old
  refresh
  echo "Removed the eBIRForms launcher."
  exit 0
fi

[ -x "$BIN" ] || { echo "No eBIRForms binary at $BIN" >&2; exit 1; }
for icon in "${ICONS[@]}"; do
  mkdir -p "$(dirname "$DATA/icons/hicolor/$icon")"
  install -m644 "$HERE/icons/hicolor/$icon" "$DATA/icons/hicolor/$icon"
done
mkdir -p "$(dirname "$ENTRY")"
# Exec points at this copy of the binary, quoted in case the path has spaces.
{ sed '/^Exec=/d' "$HERE/$APP_ID.desktop"; printf 'Exec="%s"\n' "$BIN"; } > "$ENTRY"
chmod 644 "$ENTRY"
remove_old
refresh
echo "Installed the eBIRForms launcher: $ENTRY"
