#!/usr/bin/env bash
# Uploads one finished, signed release file to Cloudflare R2 and writes its
# metadata for the manifest job.
#
#   scripts/release/r2-upload.sh <file> <platform> <arch> <kind> <meta-dir>
#
# The object lands at s3://$R2_BUCKET/releases/v$VERSION/<basename of file>
# with an immutable cache-control. The script fails, before uploading, if
# that key already exists: a published version is never overwritten.
# scripts/release/file-meta.sh checks the file first; its metadata,
# {file, size, sha256, platform, arch, kind}, lands in
# <meta-dir>/<basename>.json once the upload is verified, and
# scripts/release/manifest.py turns it into manifest.json and SHA256SUMS.
#
# Environment: VERSION, R2_BUCKET, R2_ENDPOINT and the aws CLI's
# credentials (AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY, AWS_DEFAULT_REGION).
# Runs in bash on the macOS, Linux and Windows (Git Bash) runners.
set -euo pipefail

fail() { echo "r2-upload: $*" >&2; exit 1; }

[ "$#" -eq 5 ] || fail "usage: r2-upload.sh <file> <platform> <arch> <kind> <meta-dir>"
path=$1 platform=$2 arch=$3 kind=$4 meta_dir=$5

: "${VERSION:?VERSION is required}"
: "${R2_BUCKET:?R2_BUCKET is required}"
: "${R2_ENDPOINT:?R2_ENDPOINT is required}"

case "$kind" in
  dmg) content_type=application/x-apple-diskimage ;;
  msi) content_type=application/x-msi ;;
  exe) content_type=application/vnd.microsoft.portable-executable ;;
  appimage) content_type=application/vnd.appimage ;;
  deb) content_type=application/vnd.debian.binary-package ;;
  rpm) content_type=application/x-rpm ;;
  targz) content_type=application/gzip ;;
  sh) content_type="text/x-shellscript; charset=utf-8" ;;
  apk) content_type=application/vnd.android.package-archive ;;
  *) fail "unknown kind '$kind'" ;;
esac

# Validates the file and its name, and hashes it.
checked=$(mktemp -d)
trap 'rm -rf "$checked"' EXIT
"$(dirname "$0")/file-meta.sh" "$path" "$platform" "$arch" "$kind" "$checked"
file=$(basename "$path")
size=$(wc -c < "$path" | tr -d '[:space:]')

key="releases/v$VERSION/$file"

# Never overwrite: the exact key must not exist yet.
existing=$(aws s3api list-objects-v2 --endpoint-url "$R2_ENDPOINT" --bucket "$R2_BUCKET" \
  --prefix "$key" --query 'Contents[].Key' --output text)
if printf '%s\n' "$existing" | tr '\t' '\n' | grep -Fxq -- "$key"; then
  fail "s3://$R2_BUCKET/$key already exists; a published version is never overwritten"
fi

aws s3 cp --endpoint-url "$R2_ENDPOINT" --no-progress \
  --cache-control "public, max-age=31536000, immutable" \
  --content-type "$content_type" \
  "$path" "s3://$R2_BUCKET/$key"

remote_size=$(aws s3api head-object --endpoint-url "$R2_ENDPOINT" --bucket "$R2_BUCKET" \
  --key "$key" --query ContentLength --output text)
[ "$remote_size" = "$size" ] || fail "uploaded $key is $remote_size bytes, expected $size"

mkdir -p "$meta_dir"
mv "$checked/$file.json" "$meta_dir/$file.json"
echo "uploaded s3://$R2_BUCKET/$key ($size bytes)"
