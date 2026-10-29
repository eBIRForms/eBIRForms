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
# and any SFTP identifier or string, in code or manifests.
code=$(echo "$files" | grep -E '\.(rs|toml|swift|java|kt|sh|json)$' | grep -v -E '^scripts/verify\.sh$' || true)
if [ -n "$code" ]; then
  found=$(echo "$code" | xargs grep -n -I -i -E 'passphrase|sftp' -- 2>/dev/null \
    | grep -v -E '^[^:]+:[0-9]+:[[:space:]]*(//|#|\*)' || true)
  if [ -n "$found" ]; then echo "$found"; hit 'a passphrase or SFTP code is in the tree'; fi
fi
search 'codeitlikemiley@' 'the maintainer email is in the tree (use fixtures)'
search '\b261708015' 'a real-looking TIN from the source repo is in the tree'

if echo "$files" | grep -E '(^|/)\.env(\.|$)|(^|/)\.keys/|\.(pem|p8|p12|key)$' | grep -v '\.env\.example$'; then
  hit 'a local secret file is tracked'
fi

[ "$fail" -eq 0 ] && echo "secret-check: clean"
exit "$fail"
