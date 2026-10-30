#!/usr/bin/env bash
# cargo-generate-rpm's --auto-req program: reads file paths on stdin and
# prints, for each ELF file, what rpmbuild's elfdeps would: every library
# it links directly (DT_NEEDED) and every symbol version it needs from
# them, e.g. libgtk-4.so.1()(64bit) and libc.so.6(GLIBC_2.34)(64bit).
# The builtin mode follows ldd into the build machine's own libraries
# (ICU, libjpeg), whose sonames other distributions don't share.
set -euo pipefail

while IFS= read -r path; do
  [ -f "$path" ] || continue
  readelf -h "$path" >/dev/null 2>&1 || continue
  marker=
  if readelf -h "$path" | grep -q 'Class:[[:space:]]*ELF64'; then marker='(64bit)'; fi
  readelf -d --wide "$path" | sed -n 's/.*(NEEDED).*\[\(.*\)\].*/\1/p' | while IFS= read -r soname; do
    echo "$soname()$marker"
  done
  readelf -V --wide "$path" | awk -v marker="$marker" '
    /^Version needs section/ { inside = 1; next }
    /^Version (symbols|definition) section/ { inside = 0 }
    inside {
      for (i = 1; i < NF; i++) {
        if ($i == "File:") file = $(i + 1)
        if ($i == "Name:") print file "(" $(i + 1) ")" marker
      }
    }'
done | sort -u
