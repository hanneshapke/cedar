#!/usr/bin/env bash
# Checks the versions in the repository (scripts/lib-versions.sh lists where they live):
#
#   - cedar's three places agree: Cargo.toml, desktop/Cargo.toml and docs/landing/version.js
#   - the iOS project has one MARKETING_VERSION for all its build configurations
#   - each is a plain major.minor.patch within what the Windows installer and the Android
#     versionCode can hold
#   - cedar, Android and iOS share major.minor; only the patch differs between them
#
# Usage: scripts/check-version.sh
#
# release-check.yml runs it on every pull request from a release/* branch, and release.yml runs
# it again before building. Prints a GitHub Actions ::error:: line for each problem and exits
# non-zero if there is any.
set -euo pipefail

cd "$(dirname "$0")/.."
. scripts/lib-versions.sh

fail=0
error() { echo "::error file=$1::$2"; fail=1; }

ws=$(workspace_version)
app=$(desktop_version)
site=$(site_version)
android=$(android_version)
ios=$(ios_version)

echo "cedar    Cargo.toml workspace:      $ws"
echo "         desktop/Cargo.toml:        $app"
echo "         landing version.js:        $site"
echo "android  gradle.properties:         $android"
echo "ios      MARKETING_VERSION:         $ios"

[ "$app" = "$ws" ]  || error desktop/Cargo.toml "the desktop app's version is '$app', Cargo.toml's [workspace.package] version is '$ws'"
[ "$site" = "$ws" ] || error docs/landing/version.js "window.CEDAR_VERSION is '$site', Cargo.toml's [workspace.package] version is '$ws'"

# Plain major.minor.patch: Windows Installer takes nothing else (major and minor up to 255), and
# the Android versionCode is major*1000000 + minor*1000 + patch (minor and patch under 1000).
plain() {
  local file=$1 what=$2 v=$3
  if ! printf '%s' "$v" | grep -Eq "$SEMVER"; then
    error "$file" "$what is '$v', not a plain major.minor.patch (no -beta suffix)"; return
  fi
  local major minor patch
  IFS=. read -r major minor patch <<< "$v"
  if [ "$major" -gt 255 ] || [ "$minor" -gt 255 ] || [ "$patch" -gt 999 ]; then
    error "$file" "$what '$v' is too large: major and minor go up to 255 (MSI), patch to 999 (Android versionCode)"
  fi
}
plain Cargo.toml "[workspace.package] version" "$ws"
plain android/gradle.properties "cedar.version" "$android"
plain iOS/Cedar.xcodeproj/project.pbxproj "MARKETING_VERSION" "$ios"

# One Cedar: the phones may be patches behind or ahead of the desktop, never on another minor.
line=${ws%.*}
[ "${android%.*}" = "$line" ] || error android/gradle.properties "cedar.version '$android' is not a $line.x version like Cargo.toml's '$ws'"
[ "${ios%.*}" = "$line" ]     || error iOS/Cedar.xcodeproj/project.pbxproj "MARKETING_VERSION '$ios' is not a $line.x version like Cargo.toml's '$ws'"

# The pages must take the version from version.js rather than spell it out again.
for page in docs/landing/index.html docs/landing/install.html; do
  if ! grep -q 'src="./version.js"' "$page"; then
    error "$page" "does not load ./version.js, so its version can drift from the release"
  fi
  if grep -nE "ver: *'[0-9]+\.[0-9]+\.[0-9]+'" "$page"; then
    error "$page" "hard-codes a version; use window.CEDAR_VERSION from version.js"
  fi
done

exit $fail
