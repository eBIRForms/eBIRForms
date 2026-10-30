#!/usr/bin/env bash
# Checks one finished release file and writes its metadata, with no
# network access:
#
#   scripts/release/file-meta.sh <file> <platform> <arch> <kind> <meta-dir>
#
# <meta-dir>/<basename>.json gets {file, size, sha256, platform, arch, kind},
# which scripts/release/manifest.py turns into manifest.json and SHA256SUMS.
# r2-upload.sh runs it before uploading; the release dry run runs it alone.
#
# Environment: VERSION. Runs in bash on the macOS, Linux and Windows (Git
# Bash) runners.
set -euo pipefail

fail() { echo "file-meta: $*" >&2; exit 1; }

[ "$#" -eq 5 ] || fail "usage: file-meta.sh <file> <platform> <arch> <kind> <meta-dir>"
path=$1 platform=$2 arch=$3 kind=$4 meta_dir=$5

: "${VERSION:?VERSION is required}"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "VERSION must be MAJOR.MINOR.PATCH (got '$VERSION')"

# Kinds never hold a "-": publish splits artifact names on it.
case "$platform" in macos|windows|linux|android) ;; *) fail "unknown platform '$platform'" ;; esac
case "$arch" in arm64|x64|universal) ;; *) fail "unknown arch '$arch'" ;; esac
case "$kind" in dmg|msi|exe|appimage|deb|rpm|targz|sh|apk) ;; *) fail "unknown kind '$kind'" ;; esac

[ -f "$path" ] || fail "no such file: $path"
file=$(basename "$path")
[[ "$file" =~ ^[A-Za-z0-9._+-]+$ ]] || fail "file name has characters outside [A-Za-z0-9._+-]: $file"
case "$file" in *"$VERSION"*) ;; *) fail "file name does not carry the version $VERSION: $file" ;; esac

size=$(wc -c < "$path" | tr -d '[:space:]')
[ "$size" -gt 0 ] || fail "$file is empty"
if command -v sha256sum >/dev/null; then
  sha256=$(sha256sum "$path" | awk '{print $1}')
else
  sha256=$(shasum -a 256 "$path" | awk '{print $1}')
fi
sha256=${sha256#\\}
[[ "$sha256" =~ ^[0-9a-f]{64}$ ]] || fail "could not hash $file"

mkdir -p "$meta_dir"
printf '{"file":"%s","size":%s,"sha256":"%s","platform":"%s","arch":"%s","kind":"%s"}\n' \
  "$file" "$size" "$sha256" "$platform" "$arch" "$kind" > "$meta_dir/$file.json"
echo "$file: $size bytes, sha256 $sha256 ($platform $arch $kind)"
