#!/usr/bin/env bash
# Builds Buwiz for Android and runs it on an arm64 emulator (or the device
# adb sees): the app's `libnsh.so` (crate `android/`), nsh's Java runtime
# compiled from the submodule, an APK assembled without Gradle (aapt2,
# zipalign, apksigner, as `nsh package` does), install + launch.
#
#   ./android.sh               build, start the emulator if none runs, launch
#   BUWIZ_FRESH=1 ./android.sh clear the app's data first: an empty database
#   BUWIZ_AVD=<name>           the emulator to start (default: the first AVD)
#
# The database and its key file (bir_key.txt: Android has no keyring
# backend) live in the app's private files directory.
set -e
cd "$(dirname "$0")"
export RUSTC_WRAPPER=

SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Library/Android/sdk}}"
NDK="${ANDROID_NDK_HOME:-$(ls -d "$SDK"/ndk/* | sort -V | tail -1)}"
BUILD_TOOLS="$(ls -d "$SDK"/build-tools/* | sort -V | tail -1)"
PLATFORM="$SDK/platforms/android-35/android.jar"
ADB="$SDK/platform-tools/adb"
TC="$NDK/toolchains/llvm/prebuilt/darwin-x86_64/bin"
MIN_SDK=29
TRIPLE=aarch64-linux-android
ABI=arm64-v8a
PACKAGE=com.buwiz.oss
OUT=target/android-apk
NSH_JAVA=../nsh/crates/nsh-platform-android/java

# 1. The library. OpenSSL (for SQLCipher) is built from source with the
#    NDK's clang, so it has to be on PATH as well as named for cargo.
export ANDROID_NDK_HOME="$NDK" ANDROID_NDK_ROOT="$NDK" PATH="$TC:$PATH"
export CC_aarch64_linux_android="$TC/${TRIPLE}${MIN_SDK}-clang"
export AR_aarch64_linux_android="$TC/llvm-ar"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$TC/${TRIPLE}${MIN_SDK}-clang"
cargo build -p buwiz-android --target "$TRIPLE"
SO="target/$TRIPLE/debug/libbuwiz_android.so"

# 2. nsh's Java runtime, rebuilt when a source changes.
mkdir -p "$OUT"
if [ ! -f "$OUT/classes.dex" ] || [ -n "$(find "$NSH_JAVA" -newer "$OUT/classes.dex" -name '*.java' | head -1)" ]; then
    rm -rf "$OUT/classes" && mkdir -p "$OUT/classes"
    javac --release 17 -Xlint:-options -encoding UTF-8 -classpath "$PLATFORM" \
        -d "$OUT/classes" $(find "$NSH_JAVA" -name '*.java')
    rm -f "$OUT/classes.jar"
    jar cf "$OUT/classes.jar" -C "$OUT/classes" .
    "$BUILD_TOOLS/d8" --min-api "$MIN_SDK" --lib "$PLATFORM" --output "$OUT" "$OUT/classes.jar"
fi

# 3. The APK. AppActivity runs the library's Rust root (meta-data
#    com.longbridge.nsh.RUST); edge to edge, the IME resizes the window.
cat > "$OUT/AndroidManifest.xml" <<XML
<?xml version="1.0" encoding="utf-8"?>
<manifest xmlns:android="http://schemas.android.com/apk/res/android" package="$PACKAGE">
    <application android:label="Buwiz"
        android:debuggable="true"
        android:extractNativeLibs="false"
        android:enableOnBackInvokedCallback="true"
        android:supportsRtl="true"
        android:theme="@android:style/Theme.Material.Light.NoActionBar">
        <meta-data android:name="com.longbridge.nsh.RUST" android:value="true" />
        <activity android:name="com.longbridge.nsh.AppActivity" android:exported="true"
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
rm -rf "$OUT/stage" && mkdir -p "$OUT/stage/lib/$ABI"
"$BUILD_TOOLS/aapt2" link -o "$OUT/base.apk" -I "$PLATFORM" --manifest "$OUT/AndroidManifest.xml" \
    --min-sdk-version "$MIN_SDK" --target-sdk-version 35 --version-code 1 --version-name 0.1.0
cp "$OUT/base.apk" "$OUT/unaligned.apk"
cp "$OUT/classes.dex" "$OUT/stage/classes.dex"
# Without debug info: a debug build's is over a gigabyte (symbols stay).
"$TC/llvm-strip" --strip-debug -o "$OUT/stage/lib/$ABI/libnsh.so" "$SO"
(cd "$OUT/stage" && zip -q -X ../unaligned.apk classes.dex && zip -q -X -0 ../unaligned.apk "lib/$ABI/libnsh.so")
"$BUILD_TOOLS/zipalign" -P 16 -f 4 "$OUT/unaligned.apk" "$OUT/aligned.apk"
"$BUILD_TOOLS/apksigner" sign --ks "$HOME/.android/debug.keystore" --ks-key-alias androiddebugkey \
    --ks-pass pass:android --key-pass pass:android --out "$OUT/Buwiz.apk" "$OUT/aligned.apk"
echo "built $OUT/Buwiz.apk"

# 4. A device: the one adb sees, else start the emulator.
if ! "$ADB" get-state >/dev/null 2>&1; then
    AVD="${BUWIZ_AVD:-$("$SDK/emulator/emulator" -list-avds | head -1)}"
    echo "starting emulator $AVD"
    nohup "$SDK/emulator/emulator" -avd "$AVD" -no-snapshot-save -no-boot-anim >/tmp/buwiz-emulator.log 2>&1 &
    "$ADB" wait-for-device
fi
until [ "$("$ADB" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" = "1" ]; do sleep 1; done

"$ADB" install -r "$OUT/Buwiz.apk" >/dev/null
if [ -n "$BUWIZ_FRESH" ]; then
    "$ADB" shell pm clear "$PACKAGE" >/dev/null
fi
"$ADB" shell am start -S -n "$PACKAGE/com.longbridge.nsh.AppActivity" >/dev/null
echo "launched $PACKAGE (logs: adb logcat -s nsh RustStdoutStderr)"
