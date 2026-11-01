#!/usr/bin/env bash
# Fails if the tree holds anything that must never be published: BIR
# protocol constants or endpoints, the SFTP client, local secret files, or
# personal data copied over from the source repo. Runs in CI and as
# `just secrets`. Submodules and build output are skipped.
set -euo pipefail
cd "$(dirname "$0")/.."

fail=0
hit() { echo "secret-check: $1"; fail=1; }

files=$(git ls-files --cached --others --exclude-standard \
  | grep -v -E '^(nsh|blaze)/' \
  | grep -v -E '^scripts/check-secrets\.sh$' || true)

search() { # pattern, message
  [ -z "$files" ] && return
  # LICENSE carries the copyright holder's address on purpose.
  local found
  found=$(echo "$files" | grep -v -E '^LICENSE$' | xargs grep -n -I -E "$1" -- 2>/dev/null || true)
  if [ -n "$found" ]; then echo "$found"; hit "$2"; fi
}

search 'PASSPHRASE' 'a passphrase constant is in the tree (BIR secrets live on the server only)'
search 'birgovph\.com|ebf2\.bir\.gov\.ph|tinDispatcher' 'a BIR submission endpoint is in the tree'
search '^\s*(russh|russh-sftp)\s*=' 'an SFTP client dependency is in the tree'
search 'ftpTargetFolder|getFtpFolder' "eBIRForms' upload-folder table is named in the tree"
# Case-insensitive, outside comments: any passphrase identifier or string,
# and any SFTP identifier or string, in code, manifests, build and packaging
# files, and the HTML the crates embed (packed deflated, so a scan of the
# binary can't read it). Only prose (Markdown) may name SFTP.
code=$(echo "$files" | grep -E '\.(rs|toml|swift|java|kt|kts|gradle|sh|ps1|json|html|css|xml|plist|ya?ml|txt|iss|desktop|spec)$' | grep -v -E '^scripts/verify\.sh$' || true)
if [ -n "$code" ]; then
  found=$(echo "$code" | xargs grep -n -I -i -E 'passphrase|sftp' -- 2>/dev/null \
    | grep -v -E '^[^:]+:[0-9]+:[[:space:]]*(//|#|\*)' || true)
  if [ -n "$found" ]; then echo "$found"; hit 'a passphrase or SFTP code is in the tree'; fi
fi
search 'codeitlikemiley@' 'the maintainer email is in the tree (use fixtures)'
# Buwiz reads only its own BUWIZ_* environment names: eBIRForms (its shell,
# bir-desktop) reads these, so Buwiz reading one would let a setting made for
# the other app pick Buwiz's database, key or server. The list lives in
# crates/bir-core/src/foreign_env.rs, the one file allowed to name them.
foreign='(^|[^A-Z0-9_])(BIR_DATABASE_PATH|BIR_DB_KEY(_FILE)?|BIR_NO_KEYCHAIN|BIR_LOG_DIR|BIR_DATA_DIR|EBIR_TEST_ENV|EBIR_RELAXED_VALIDATION|DEVELOPER_MODE|BUWIZ_SERVER_URL|BUWIZ_ACCOUNT_URL|BUWIZ_ACCESS_TOKEN)([^A-Z0-9_]|$)'
envcode=$(echo "$files" | grep -E '\.(rs|sh|java|kt|swift|toml|ya?ml|json|py)$|(^|/)justfile$' | grep -v -E '^crates/bir-core/src/foreign_env\.rs$' || true)
if [ -n "$envcode" ]; then
  found=$(echo "$envcode" | xargs grep -n -I -E "$foreign" -- 2>/dev/null || true)
  if [ -n "$found" ]; then echo "$found"; hit "another app's environment name is in the tree (Buwiz reads only BUWIZ_* names)"; fi
fi
# A real TIN root from the source repo, kept here only as a SHA-256 so this
# script doesn't publish it: every 9-digit run (or NNN-NNN-NNN) in the tree
# is hashed and compared.
tin_hash=54769564c3351819e39114a34d0f1b52f5ddc479a736e24df3ffcae2e2c38e2b
if [ -n "$files" ]; then
  tins=$(echo "$files" | grep -v -E '^LICENSE$' | xargs grep -ohI -E '(^|[^0-9])[0-9]{3}-?[0-9]{3}-?[0-9]{3}' -- 2>/dev/null \
    | sed -E 's/^[^0-9]//; s/-//g' | sort -u || true)
  for t in $(echo "$tins" | perl -MDigest::SHA=sha256_hex -ne 'chomp; print "$_\n" if sha256_hex($_) eq "'"$tin_hash"'"'); do
    if [ -n "$t" ]; then
      echo "$files" | grep -v -E '^LICENSE$' | xargs grep -lI -E "${t:0:3}-?${t:3:3}-?${t:6:3}" -- 2>/dev/null | sed 's/^/  in /' || true
      hit 'a real-looking TIN from the source repo is in the tree'
    fi
  done
fi

if echo "$files" | grep -E '(^|/)\.env(\.|$)|(^|/)\.keys/|\.(pem|p8|p12|key)$' | grep -v '\.env\.example$'; then
  hit 'a local secret file is tracked'
fi

[ "$fail" -eq 0 ] && echo "secret-check: clean"
exit "$fail"
