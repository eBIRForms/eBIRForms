#!/usr/bin/env bash
# SQLCipher file compatibility between the builds Buwiz ships: macOS's
# (bundled SQLCipher, CommonCrypto, through bir-core's Database::open) and
# Android's (crates/sqlcipher-tomcrypt, LibTomCrypt). Each writes a database
# the other then opens, checks and writes to, and their file-format settings
# must agree. Runs on macOS (the CommonCrypto side needs it).
#
#   crates/sqlcipher-tomcrypt/compat/check.sh
set -euo pipefail
cd "$(dirname "$0")"
export RUSTC_WRAPPER=
# Both sides take the database key from here, as bir-core does with
# BUWIZ_DB_KEY; never the keychain.
export BUWIZ_NO_KEYCHAIN=1 BUWIZ_DB_KEY=5f4dcc3b5aa765d61d8327deb882cf995f4dcc3b5aa765d61d8327deb882cf99

# Built one at a time: in one build rusqlite's features would unify.
cargo build --locked -q -p compat-commoncrypto
cargo build --locked -q -p compat-libtomcrypt
CC=target/debug/compat-commoncrypto
LTC=target/debug/compat-libtomcrypt

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
# bir-core keeps its logs and data under here, not the user's.
export HOME="$work/home" BUWIZ_DATABASE_PATH="$work/unused.db"
mkdir -p "$HOME"
step() { echo "== $*"; "$@"; }

# 1. bir-core on macOS creates a database; Android's build opens it.
step $CC write "$work/a.db"
step $LTC read "$work/a.db" commoncrypto
step $LTC write "$work/a.db"
step $CC read "$work/a.db" commoncrypto libtomcrypt

# 2. Android's build creates one; bir-core opens it (and migrates it).
step $LTC write "$work/b.db"
step $CC read "$work/b.db" libtomcrypt
step $CC write "$work/b.db"
out=$(step $LTC read "$work/b.db" libtomcrypt commoncrypto)
echo "$out"
grep -q '^user_version=[1-9]' <<<"$out" || { echo "FAIL: bir-core's migrations not visible to LibTomCrypt"; exit 1; }

# Encrypted for real: no plaintext header, and a wrong key opens nothing.
for f in a b; do
    [ "$(head -c 15 "$work/$f.db")" != "SQLite format 3" ] || { echo "FAIL: $f.db is plaintext"; exit 1; }
    for side in $CC $LTC; do
        if BUWIZ_DB_KEY=$(printf '0%.0s' {1..63})1 $side read "$work/$f.db" 2>/dev/null >&2; then
            echo "FAIL: $side opened $f.db with the wrong key"; exit 1
        fi
    done
done
echo "== wrong key refused on both sides; files carry no plaintext header"

# 3. The same file-format settings on both sides.
format='^(cipher_page_size|kdf_iter|cipher_hmac_algorithm|cipher_kdf_algorithm|cipher_use_hmac|cipher_plaintext_header_size)='
$CC settings "$work/a.db" >"$work/cc.txt"
$LTC settings "$work/b.db" >"$work/ltc.txt"
paste -d'|' "$work/cc.txt" "$work/ltc.txt" | sed 's/|/   |   /'
if ! diff <(grep -E "$format" "$work/cc.txt") <(grep -E "$format" "$work/ltc.txt"); then
    echo "FAIL: SQLCipher settings differ"; exit 1
fi
echo "PASS: CommonCrypto and LibTomCrypt SQLCipher databases open on both sides"
