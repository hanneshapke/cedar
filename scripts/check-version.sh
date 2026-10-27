#!/usr/bin/env bash
# Checks that every place carrying the release version agrees with the one given:
#
#   Cargo.toml                       [workspace.package] version (cedar, cedar-chat, pairing)
#   desktop/Cargo.toml               the desktop app's version
#   iOS/Cedar.xcodeproj              MARKETING_VERSION, in every build configuration
#   docs/landing/version.js          the version the landing site and install guide show
#
# Usage: scripts/check-version.sh <version>     e.g. scripts/check-version.sh 0.2.0
#
# release-check.yml runs it on every pull request from a release/<version> branch, and
# release.yml runs it again before building. Prints a GitHub Actions ::error:: line for each
# mismatch and exits non-zero if there is any.
set -euo pipefail

VERSION=${1:?usage: $0 <version>}
cd "$(dirname "$0")/.."

fail=0
mismatch() { echo "::error file=$1::$2 is '$3', the release is '$VERSION'"; fail=1; }

ws=$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.name == "cedar") | .version')
app=$(cargo metadata --no-deps --format-version 1 --manifest-path desktop/Cargo.toml | jq -r '.packages[] | select(.name == "cedar-app") | .version')
ios=$(grep -o 'MARKETING_VERSION = [^;]*' iOS/Cedar.xcodeproj/project.pbxproj | sed 's/.* = //' | sort -u | tr '\n' ' ' | sed 's/ $//')
site=$(sed -nE "s/^window\.CEDAR_VERSION = '([^']*)';.*/\1/p" docs/landing/version.js)

echo "release:              $VERSION"
echo "Cargo.toml workspace: $ws"
echo "desktop/Cargo.toml:   $app"
echo "iOS MARKETING_VERSION: $ios"
echo "landing version.js:   $site"

[ "$ws" = "$VERSION" ]   || mismatch Cargo.toml "[workspace.package] version" "$ws"
[ "$app" = "$VERSION" ]  || mismatch desktop/Cargo.toml "the desktop app's version" "$app"
[ "$ios" = "$VERSION" ]  || mismatch iOS/Cedar.xcodeproj/project.pbxproj "MARKETING_VERSION" "$ios"
[ "$site" = "$VERSION" ] || mismatch docs/landing/version.js "window.CEDAR_VERSION" "$site"

# The pages must take the version from version.js rather than spell it out again.
for page in docs/landing/index.html docs/landing/install.html; do
  if ! grep -q 'src="./version.js"' "$page"; then
    echo "::error file=$page::does not load ./version.js, so its version can drift from the release"; fail=1
  fi
  if grep -nE "ver: *'[0-9]+\.[0-9]+\.[0-9]+'" "$page"; then
    echo "::error file=$page::hard-codes a version; use window.CEDAR_VERSION from version.js"; fail=1
  fi
done

exit $fail
