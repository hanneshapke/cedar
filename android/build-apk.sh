#!/usr/bin/env bash
# Builds the Cedar Android app as an APK on your own computer.
#
#   ./build-apk.sh             a debug APK, signed with the SDK's debug key
#   ./build-apk.sh release     a release APK, signed with your keystore (see below), else unsigned
#   ./build-apk.sh test        only the unit tests of the core module
#   ./build-apk.sh install     build the debug APK and install it on the phone adb sees
#
# Needs JDK 17 or newer and the Android SDK (Android Studio installs one; or the command-line
# tools from https://developer.android.com/studio#command-line-tools-only). The SDK is found
# through ANDROID_HOME, ANDROID_SDK_ROOT, android/local.properties or the usual install folders.
#
# FCM: put your Firebase project's google-services.json in android/app/ to build with Google push.
# Without it the app builds all the same and uses ntfy (UnifiedPush) for notifications.
#
# Release signing: set CEDAR_KEYSTORE (path to a .jks), CEDAR_KEYSTORE_PASSWORD, CEDAR_KEY_ALIAS
# and CEDAR_KEY_PASSWORD, or write them as storeFile/storePassword/keyAlias/keyPassword into
# android/keystore.properties. Make a keystore once with:
#   keytool -genkeypair -v -keystore cedar.jks -alias cedar -keyalg RSA -keysize 4096 -validity 10000
# and keep it: an installed app only updates from an APK signed with the same key.
set -euo pipefail

cd "$(dirname "$0")"
mode="${1:-debug}"

die() { echo "error: $*" >&2; exit 1; }

# JDK 17+
command -v java >/dev/null || die "no java on PATH; install JDK 17 or newer (e.g. Temurin) and try again"
java_major=$(java -version 2>&1 | awk -F'"' '/version/ {split($2, v, "."); print (v[1] == "1" ? v[2] : v[1]); exit}')
[ "${java_major:-0}" -ge 17 ] || die "JDK 17 or newer is needed, found ${java_major:-none}"

# The Android SDK
if ! grep -qs '^sdk.dir=' local.properties; then
  sdk="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
  if [ -z "$sdk" ]; then
    for candidate in "$HOME/Library/Android/sdk" "$HOME/Android/Sdk" "$HOME/AppData/Local/Android/Sdk" /opt/android-sdk /usr/lib/android-sdk; do
      [ -d "$candidate" ] && { sdk="$candidate"; break; }
    done
  fi
  [ -n "$sdk" ] && [ -d "$sdk" ] || die "Android SDK not found; set ANDROID_HOME, or install Android Studio"
  echo "sdk.dir=$sdk" > local.properties
  echo "Using the Android SDK in $sdk"
fi

if [ -f app/google-services.json ]; then
  echo "Building with FCM (app/google-services.json)"
else
  echo "Building without FCM: notifications go through ntfy / UnifiedPush"
fi

case "$mode" in
  test)
    ./gradlew :core:test
    ;;
  debug)
    ./gradlew :core:test :app:assembleDebug
    apk=$(ls app/build/outputs/apk/debug/*.apk | head -1)
    echo; echo "APK: android/$apk"
    ;;
  install)
    command -v adb >/dev/null || die "no adb on PATH (it is in the SDK's platform-tools)"
    ./gradlew :app:installDebug
    ;;
  release)
    ./gradlew :core:test :app:assembleRelease
    if ls app/build/outputs/apk/release/*-unsigned.apk >/dev/null 2>&1; then
      apk=$(ls app/build/outputs/apk/release/*-unsigned.apk | head -1)
      echo; echo "APK (unsigned, no keystore configured): android/$apk"
      echo "Sign it with apksigner, or set CEDAR_KEYSTORE & co. and build again."
    else
      apk=$(ls app/build/outputs/apk/release/*.apk | head -1)
      echo; echo "APK (signed): android/$apk"
    fi
    ;;
  *)
    die "unknown mode '$mode'; use debug, release, test or install"
    ;;
esac
