#!/usr/bin/env bash
# Builds the release APK: the same steps as app/android.sh (libnsh.so from
# the `android` crate, nsh's Java runtime and Buwiz's classes, an APK
# assembled without Gradle), but a release build, not debuggable, carrying
# the version, and signed with the release keystore. Runs on Linux or macOS.
#
# Environment:
#   VERSION                    required, MAJOR.MINOR.PATCH
#   ANDROID_KEYSTORE_PATH      the release keystore (JKS or PKCS12)
#   ANDROID_KEYSTORE_PASSWORD  its store password
#   ANDROID_KEY_ALIAS          the signing key's alias
#   ANDROID_KEY_PASSWORD       the key's password (default: the store's)
#   ANDROID_HOME / ANDROID_NDK_HOME   the SDK and NDK (defaults as android.sh)
#
# Prints the APK's path: target/android-release/Buwiz-$VERSION-android-arm64.apk
set -euo pipefail
cd "$(dirname "$0")/../.."
export RUSTC_WRAPPER=

fail() { echo "build-apk: $*" >&2; exit 1; }

VERSION="${VERSION:-}"
[[ "$VERSION" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]] || fail "VERSION must be MAJOR.MINOR.PATCH (got '$VERSION')"
# Grows with every release: 1.2.3 -> 1002003.
VERSION_CODE=$((BASH_REMATCH[1] * 1000000 + BASH_REMATCH[2] * 1000 + BASH_REMATCH[3]))
[ "$VERSION_CODE" -gt 0 ] || fail "version code must be positive"
for v in ANDROID_KEYSTORE_PATH ANDROID_KEYSTORE_PASSWORD ANDROID_KEY_ALIAS; do
  [ -n "${!v:-}" ] || fail "$v is required"
done
[ -f "$ANDROID_KEYSTORE_PATH" ] || fail "no keystore at $ANDROID_KEYSTORE_PATH"
ANDROID_KEY_PASSWORD="${ANDROID_KEY_PASSWORD:-$ANDROID_KEYSTORE_PASSWORD}"

SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Library/Android/sdk}}"
NDK="${ANDROID_NDK_HOME:-$(ls -d "$SDK"/ndk/* | sort -V | tail -1)}"
BUILD_TOOLS="$(ls -d "$SDK"/build-tools/* | sort -V | tail -1)"
PLATFORM="$SDK/platforms/android-35/android.jar"
[ -f "$PLATFORM" ] || fail "missing $PLATFORM (sdkmanager \"platforms;android-35\")"
case "$(uname -s)" in
  Linux) HOST=linux-x86_64 ;;
  Darwin) HOST=darwin-x86_64 ;;
  *) fail "unsupported host $(uname -s)" ;;
esac
TC="$NDK/toolchains/llvm/prebuilt/$HOST/bin"
[ -d "$TC" ] || fail "no NDK toolchain at $TC"
MIN_SDK=29
TRIPLE=aarch64-linux-android
ABI=arm64-v8a
PACKAGE=com.buwiz.oss
OUT=target/android-release
APK="$OUT/Buwiz-$VERSION-android-arm64.apk"
NSH_JAVA=../nsh/crates/nsh-platform-android/java
APP_JAVA=android/java

# 1. The library, a release build (SQLCipher and LibTomCrypt through the
#    NDK's clang, as in android.sh).
export ANDROID_NDK_HOME="$NDK" ANDROID_NDK_ROOT="$NDK" PATH="$TC:$PATH"
export CC_aarch64_linux_android="$TC/${TRIPLE}${MIN_SDK}-clang"
export AR_aarch64_linux_android="$TC/llvm-ar"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$TC/${TRIPLE}${MIN_SDK}-clang"
cargo build --locked --release -p buwiz-android --target "$TRIPLE"
SO="target/$TRIPLE/release/libbuwiz_android.so"

# 2. nsh's Java runtime and Buwiz's own classes.
rm -rf "$OUT" && mkdir -p "$OUT/classes"
javac --release 17 -Xlint:-options -encoding UTF-8 -classpath "$PLATFORM" \
  -d "$OUT/classes" $(find "$NSH_JAVA" "$APP_JAVA" -name '*.java')
jar cf "$OUT/classes.jar" -C "$OUT/classes" .
"$BUILD_TOOLS/d8" --release --min-api "$MIN_SDK" --lib "$PLATFORM" --output "$OUT" "$OUT/classes.jar"

# 3. The APK: android.sh's manifest without android:debuggable.
cat > "$OUT/AndroidManifest.xml" <<XML
<?xml version="1.0" encoding="utf-8"?>
<manifest xmlns:android="http://schemas.android.com/apk/res/android" package="$PACKAGE">
    <application android:label="Buwiz"
        android:extractNativeLibs="false"
        android:enableOnBackInvokedCallback="true"
        android:supportsRtl="true"
        android:theme="@android:style/Theme.Material.Light.NoActionBar">
        <meta-data android:name="com.longbridge.nsh.RUST" android:value="true" />
        <provider android:name="com.buwiz.oss.PdfFiles" android:authorities="com.buwiz.oss.pdf"
            android:exported="false" android:grantUriPermissions="true" />
        <activity android:name="com.buwiz.oss.MainActivity" android:exported="true"
            android:windowSoftInputMode="adjustResize"
            android:configChanges="orientation|screenSize|screenLayout|smallestScreenSize|keyboard|keyboardHidden|navigation|uiMode|density|fontScale|locale|layoutDirection">
            <intent-filter>
                <action android:name="android.intent.action.MAIN" />
                <category android:name="android.intent.category.LAUNCHER" />
            </intent-filter>
        </activity>
    </application>
</manifest>
XML
mkdir -p "$OUT/stage/lib/$ABI"
"$BUILD_TOOLS/aapt2" link -o "$OUT/unaligned.apk" -I "$PLATFORM" --manifest "$OUT/AndroidManifest.xml" \
  --min-sdk-version "$MIN_SDK" --target-sdk-version 35 \
  --version-code "$VERSION_CODE" --version-name "$VERSION"
cp "$OUT/classes.dex" "$OUT/stage/classes.dex"
"$TC/llvm-strip" --strip-debug -o "$OUT/stage/lib/$ABI/libnsh.so" "$SO"
(cd "$OUT/stage" && zip -q -X ../unaligned.apk classes.dex && zip -q -X -0 ../unaligned.apk "lib/$ABI/libnsh.so")
"$BUILD_TOOLS/zipalign" -P 16 -f 4 "$OUT/unaligned.apk" "$OUT/aligned.apk"
# Passwords through the environment, not the command line.
KS_PASS="$ANDROID_KEYSTORE_PASSWORD" KEY_PASS="$ANDROID_KEY_PASSWORD" \
  "$BUILD_TOOLS/apksigner" sign --ks "$ANDROID_KEYSTORE_PATH" --ks-key-alias "$ANDROID_KEY_ALIAS" \
  --ks-pass env:KS_PASS --key-pass env:KEY_PASS --out "$APK" "$OUT/aligned.apk"
"$BUILD_TOOLS/apksigner" verify --verbose "$APK" >&2
echo "$APK"
