#!/usr/bin/env bash
# Sets the release version everywhere scripts/check-version.sh looks, and in the lock files:
#
#   Cargo.toml                 [workspace.package] version (cedar, cedar-chat, pairing)
#   Cargo.lock                 the cedar, cedar-chat and cedar-pairing entries
#   desktop/Cargo.toml         the desktop app's version
#   desktop/Cargo.lock         the cedar-app and cedar-pairing entries
#   iOS/Cedar.xcodeproj        MARKETING_VERSION, in every build configuration
#   docs/landing/version.js    the version the landing site and install guide show
#
# Usage: scripts/bump-version.sh [<version> | major | minor | patch]
#
#   scripts/bump-version.sh            0.1.9 -> 0.1.10 (patch is the default)
#   scripts/bump-version.sh minor      0.1.9 -> 0.2.0
#   scripts/bump-version.sh major      0.1.9 -> 1.0.0
#   scripts/bump-version.sh 0.3.0      exactly 0.3.0
#
# The current version is read from Cargo.toml. Nothing is committed; the script prints the
# release branch to push it on. The Android version comes from the release build (release.yml).
set -euo pipefail

cd "$(dirname "$0")/.."

current=$(awk '/^\[workspace\.package\]/{p=1; next} /^\[/{p=0} p && /^version *=/{gsub(/.*= *"|".*/, ""); print; exit}' Cargo.toml)
semver='^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$'
if ! printf '%s' "$current" | grep -Eq "$semver"; then
  echo "error: can't read a major.minor.patch version from Cargo.toml [workspace.package] (got '$current')" >&2
  exit 1
fi
IFS=. read -r major minor patch <<< "$current"

arg=${1:-patch}
case "$arg" in
  patch) new="$major.$minor.$((patch + 1))" ;;
  minor) new="$major.$((minor + 1)).0" ;;
  major) new="$((major + 1)).0.0" ;;
  -h|--help) sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
  *) new=${arg#v} ;;
esac

# The limits the release enforces: plain major.minor.patch for the Windows installer (MSI), with
# major and minor up to 255 and patch up to 65535, and minor and patch under 1000 for the Android
# versionCode (major*1000000 + minor*1000 + patch).
if ! printf '%s' "$new" | grep -Eq "$semver"; then
  echo "error: '$new' is not major.minor.patch (no prefix other than v, no -beta suffix)" >&2
  exit 1
fi
IFS=. read -r nmaj nmin npat <<< "$new"
if [ "$nmaj" -gt 255 ] || [ "$nmin" -gt 255 ] || [ "$npat" -gt 999 ]; then
  echo "error: '$new' is too large: major and minor go up to 255 (MSI), patch to 999 (Android versionCode)" >&2
  exit 1
fi
if [ "$new" = "$current" ]; then
  echo "error: the version is already $current" >&2
  exit 1
fi

# Rewrites a file through awk into a temporary file, so it works with BSD (macOS) and GNU tools.
rewrite() {
  local file=$1; shift
  local tmp; tmp=$(mktemp)
  awk "$@" "$file" > "$tmp" && cat "$tmp" > "$file"
  rm -f "$tmp"
}

# Root Cargo.toml: the version line inside [workspace.package] only.
rewrite Cargo.toml -v new="$new" '
  /^\[/ { inpkg = ($0 == "[workspace.package]") }
  inpkg && /^version *=/ { sub(/"[^"]*"/, "\"" new "\"") }
  { print }'

# desktop/Cargo.toml: the version line inside [package] only.
rewrite desktop/Cargo.toml -v new="$new" '
  /^\[/ { inpkg = ($0 == "[package]") }
  inpkg && /^version *=/ { sub(/"[^"]*"/, "\"" new "\"") }
  { print }'

# Lock files: the version line right after the name line of each of our own crates.
bump_lock() {
  local file=$1 names=$2
  rewrite "$file" -v new="$new" -v names="$names" '
    BEGIN { n = split(names, list, " "); for (i = 1; i <= n; i++) ours["name = \"" list[i] "\""] = 1 }
    /^name = / { mine = ($0 in ours); print; next }
    mine && /^version = / { $0 = "version = \"" new "\""; mine = 0 }
    { print }'
}
bump_lock Cargo.lock "cedar cedar-chat cedar-pairing"
bump_lock desktop/Cargo.lock "cedar-app cedar-pairing"

# iOS: every MARKETING_VERSION in the project.
rewrite iOS/Cedar.xcodeproj/project.pbxproj -v new="$new" '
  { gsub(/MARKETING_VERSION = [^;]*;/, "MARKETING_VERSION = " new ";"); print }'

# Landing site.
rewrite docs/landing/version.js -v new="$new" -v q="'" '
  /^window\.CEDAR_VERSION = / { $0 = "window.CEDAR_VERSION = " q new q ";" }
  { print }'

echo "Cedar $current -> $new"
git --no-pager diff --stat -- Cargo.toml Cargo.lock desktop/Cargo.toml desktop/Cargo.lock \
  iOS/Cedar.xcodeproj/project.pbxproj docs/landing/version.js 2>/dev/null || true

if command -v cargo >/dev/null && command -v jq >/dev/null; then
  scripts/check-version.sh "$new" >/dev/null || { echo "error: scripts/check-version.sh $new still fails" >&2; exit 1; }
fi

if ! grep -q "^## $new\$" CHANGELOG.md; then
  echo "note: CHANGELOG.md has no '## $new' section yet"
fi
cat <<EOF

Next:
  git checkout -b release/$new
  git commit -am "Release $new"
  git push -u origin release/$new
and open a pull request from release/$new to main.
EOF
