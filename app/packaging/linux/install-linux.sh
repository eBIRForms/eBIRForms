#!/bin/sh
# Installs Buwiz on Linux from downloads.buwiz.com:
#
#   curl -fsSL https://downloads.buwiz.com/releases/v<VERSION>/Buwiz-<VERSION>-linux-install.sh | sh
#   sh install-linux.sh [--version X.Y.Z] [--tarball] [--uninstall]
#   sh install-linux.sh --from-dir <dir>    (tests: <dir>/manifest.json and the files)
#
# Reads latest.json (or releases/vX.Y.Z/manifest.json with --version),
# picks the file for this machine and package manager (apt: .deb;
# dnf, zypper or rpm: .rpm; otherwise the .tar.gz), checks its sha256
# against the manifest, then installs it. The .tar.gz installs for this
# user only, under ~/.local/opt/buwiz, without root.
set -eu

BASE="https://downloads.buwiz.com"
APP_VERSION=""
USE_TARBALL=0
UNINSTALL=0
FROM_DIR=""
TEMP_DIR=""

fail() { echo "install-linux: $*" >&2; exit 1; }

usage() {
    cat <<'USAGE'
Usage: install-linux.sh [--version X.Y.Z] [--tarball] [--uninstall]
       install-linux.sh --from-dir <directory> [--tarball]
USAGE
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --version)
            [ "$#" -ge 2 ] || fail "--version needs a value"
            APP_VERSION=${2#v}
            shift 2
            ;;
        --tarball) USE_TARBALL=1; shift ;;
        --uninstall) UNINSTALL=1; shift ;;
        --from-dir)
            [ "$#" -ge 2 ] || fail "--from-dir needs a directory"
            FROM_DIR=$2
            shift 2
            ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; fail "unknown option: $1" ;;
    esac
done

run_privileged() {
    if [ "$(id -u)" -eq 0 ]; then
        "$@"
    elif command -v sudo >/dev/null 2>&1; then
        sudo "$@"
    else
        fail "run as root, or install sudo, to install a system package"
    fi
}

# The tarball install; app data lives elsewhere (~/.taxman-ebir).
USER_ROOT="$HOME/.local/opt/buwiz"
USER_BIN="$HOME/.local/bin"

if [ "$UNINSTALL" -eq 1 ]; then
    if command -v dpkg-query >/dev/null 2>&1 &&
        dpkg-query -W -f='${Status}' buwiz 2>/dev/null | grep -q 'install ok installed'; then
        run_privileged apt-get remove -y buwiz
    elif command -v rpm >/dev/null 2>&1 && rpm -q buwiz >/dev/null 2>&1; then
        if command -v dnf >/dev/null 2>&1; then
            run_privileged dnf remove -y buwiz
        elif command -v zypper >/dev/null 2>&1; then
            run_privileged zypper --non-interactive remove buwiz
        else
            run_privileged rpm -e buwiz
        fi
    fi
    if [ -x "$USER_ROOT/install-desktop-entry.sh" ]; then
        "$USER_ROOT/install-desktop-entry.sh" --remove
    fi
    if [ -L "$USER_BIN/buwiz" ]; then rm -f "$USER_BIN/buwiz"; fi
    rm -rf "$USER_ROOT"
    echo "Removed Buwiz."
    exit 0
fi

case "$(uname -m)" in
    x86_64|amd64) ARCH=x64 ;;
    aarch64|arm64) ARCH=arm64 ;;
    *) fail "no Buwiz build for $(uname -m); there are x86_64 and aarch64 builds" ;;
esac

if [ "$USE_TARBALL" -eq 1 ]; then
    KIND=targz
elif command -v apt-get >/dev/null 2>&1 && command -v dpkg >/dev/null 2>&1; then
    KIND=deb
elif command -v dnf >/dev/null 2>&1 || command -v zypper >/dev/null 2>&1 || command -v rpm >/dev/null 2>&1; then
    KIND=rpm
else
    KIND=targz
fi

if command -v sha256sum >/dev/null 2>&1; then
    hash_file() { sha256sum "$1" | awk '{print $1}'; }
elif command -v shasum >/dev/null 2>&1; then
    hash_file() { shasum -a 256 "$1" | awk '{print $1}'; }
else
    fail "needs sha256sum or shasum to check the download"
fi

fetch() { # url, output
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL --retry 3 -o "$2" "$1"
    elif command -v wget >/dev/null 2>&1; then
        wget -q -O "$2" "$1"
    else
        fail "needs curl or wget to download Buwiz"
    fi
}

cleanup() {
    if [ -n "$TEMP_DIR" ] && [ -d "$TEMP_DIR" ]; then rm -rf "$TEMP_DIR"; fi
}
trap cleanup 0
trap 'exit 1' HUP INT TERM
TEMP_DIR=$(mktemp -d "${TMPDIR:-/tmp}/buwiz-install.XXXXXX")

# The manifest: {"version", "published_at", "files": [{"platform", "arch",
# "kind", "file", "size", "sha256"}]}.
manifest="$TEMP_DIR/manifest.json"
if [ -n "$FROM_DIR" ]; then
    [ -f "$FROM_DIR/manifest.json" ] || fail "no manifest.json in $FROM_DIR"
    cp "$FROM_DIR/manifest.json" "$manifest"
elif [ -n "$APP_VERSION" ]; then
    fetch "$BASE/releases/v$APP_VERSION/manifest.json" "$manifest" || fail "no release v$APP_VERSION on $BASE"
else
    fetch "$BASE/latest.json" "$manifest" || fail "could not read $BASE/latest.json"
fi

# One JSON object per line, then a field's string value from it.
objects() { tr -d '\r\n' < "$manifest" | tr '{}' '\n\n'; }
field_awk='function field(name,   r) {
    if (!match($0, "\"" name "\"[ \t]*:[ \t]*\"[^\"]*\"")) return ""
    r = substr($0, RSTART, RLENGTH)
    sub(/^"[^"]*"[ \t]*:[ \t]*"/, "", r)
    sub(/"$/, "", r)
    return r
}'

VERSION=$(objects | awk "$field_awk"' { v = field("version"); if (v != "") { print v; exit } }')
case "$VERSION" in
    [0-9]*.[0-9]*.[0-9]*) ;;
    *) fail "the manifest has no usable version (got '$VERSION')" ;;
esac
case "$VERSION" in *[!0-9.]*) fail "bad version in the manifest: $VERSION" ;; esac
if [ -n "$APP_VERSION" ] && [ "$APP_VERSION" != "$VERSION" ]; then
    fail "the manifest is for $VERSION, not $APP_VERSION"
fi

entry=$(objects | awk -v arch="$ARCH" -v kind="$KIND" "$field_awk"'
    field("platform") == "linux" && field("arch") == arch && field("kind") == kind {
        print field("file"), field("sha256"); exit
    }')
[ -n "$entry" ] || fail "Buwiz $VERSION has no linux $ARCH $KIND file"
FILE=${entry% *}
SHA256=${entry#* }
case "$FILE" in
    *[!A-Za-z0-9._+-]*|"") fail "bad file name in the manifest: $FILE" ;;
    *"$VERSION"*) ;;
    *) fail "$FILE does not carry the version $VERSION" ;;
esac
case "$SHA256" in
    *[!0-9a-f]*|"") fail "bad sha256 in the manifest for $FILE" ;;
esac
[ "${#SHA256}" -eq 64 ] || fail "bad sha256 in the manifest for $FILE"

package="$TEMP_DIR/$FILE"
if [ -n "$FROM_DIR" ]; then
    [ -f "$FROM_DIR/$FILE" ] || fail "no $FILE in $FROM_DIR"
    cp "$FROM_DIR/$FILE" "$package"
else
    echo "Downloading Buwiz $VERSION ($FILE)"
    fetch "$BASE/releases/v$VERSION/$FILE" "$package" || fail "could not download $FILE"
fi
actual=$(hash_file "$package")
[ "$actual" = "$SHA256" ] || fail "$FILE has sha256 $actual, the manifest says $SHA256"
echo "Checked $FILE (sha256 $SHA256)"

case "$KIND" in
    deb)
        run_privileged apt-get update
        run_privileged apt-get install -y "$package"
        ;;
    rpm)
        if command -v dnf >/dev/null 2>&1; then
            run_privileged dnf install -y "$package"
        elif command -v zypper >/dev/null 2>&1; then
            run_privileged zypper --non-interactive install --allow-unsigned-rpm "$package"
        else
            run_privileged rpm -U --replacepkgs "$package"
        fi
        ;;
    targz)
        rm -rf "$USER_ROOT"
        mkdir -p "$USER_ROOT" "$USER_BIN"
        tar -xzf "$package" -C "$USER_ROOT" --strip-components=1
        ln -sf "$USER_ROOT/buwiz" "$USER_BIN/buwiz"
        "$USER_ROOT/install-desktop-entry.sh"
        echo "Installed Buwiz $VERSION in $USER_ROOT"
        case ":$PATH:" in
            *":$USER_BIN:"*) ;;
            *) echo "Add $USER_BIN to PATH to run buwiz from a terminal." ;;
        esac
        echo "The app needs GTK 4.14 or later and WebKitGTK 6.0 from your distribution."
        exit 0
        ;;
esac
echo "Installed Buwiz $VERSION ($FILE)"
