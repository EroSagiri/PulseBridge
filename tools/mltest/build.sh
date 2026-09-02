#!/usr/bin/env bash
# Builds the Multi-Link probe APK without Gradle: aapt2 -> javac -> d8 -> apksigner.
set -euo pipefail
cd "$(dirname "$0")"

SDK="${ANDROID_HOME:-$HOME/AppData/Local/Android/Sdk}"
BT="$SDK/build-tools/36.0.0"
JAR="$SDK/platforms/android-36/android.jar"

rm -rf build && mkdir -p build/classes

"$BT/aapt2.exe" link \
    --manifest AndroidManifest.xml \
    -I "$JAR" \
    --min-sdk-version 21 \
    --target-sdk-version 30 \
    -o build/base.apk

javac -nowarn -source 11 -target 11 -classpath "$JAR" \
    -d build/classes $(find src -name '*.java')

"$BT/d8.bat" --lib "$JAR" --min-api 21 --output build \
    $(find build/classes -name '*.class')

python - <<'PY'
import zipfile, shutil
shutil.copy("build/base.apk", "build/unsigned.apk")
with zipfile.ZipFile("build/unsigned.apk", "a", zipfile.ZIP_DEFLATED) as z:
    z.write("build/classes.dex", "classes.dex")
PY

if [ ! -f debug.keystore ]; then
    keytool -genkeypair -keystore debug.keystore -storepass android -keypass android \
        -alias mltest -keyalg RSA -keysize 2048 -validity 10000 \
        -dname "CN=mltest, OU=dev, O=pulsebridge, L=x, S=x, C=US"
fi

"$BT/zipalign.exe" -f 4 build/unsigned.apk build/mltest.apk
"$BT/apksigner.bat" sign --ks debug.keystore --ks-pass pass:android \
    --key-pass pass:android build/mltest.apk

echo "built: build/mltest.apk"
