#!/usr/bin/env bash
# Sets the version of the platforms a release is for (scripts/lib-versions.sh says where each
# platform's version lives; the lock files follow Cargo.toml):
#
#   cedar     Cargo.toml, Cargo.lock, desktop/Cargo.toml, desktop/Cargo.lock, docs/landing/version.js
#   android   android/gradle.properties
#   ios       iOS/Cedar.xcodeproj
#
# The platforms share major.minor and count patches on their own, so a patch release goes out
# only for the platforms it changes, and a minor or major release for all of them.
#
# Usage: scripts/bump-version.sh [patch] [cedar] [android] [ios]
#        scripts/bump-version.sh minor | major
#        scripts/bump-version.sh <version> [cedar] [android] [ios]
#
#   scripts/bump-version.sh                 the next patch of every platform that changed since its
#                                           last release (iOS/ for ios, android/ for android, the
#                                           rest of the code for cedar)
#   scripts/bump-version.sh patch cedar     the next patch of the desktop app and servers only
#   scripts/bump-version.sh android ios     the next patch of both phone apps ("mobile" means both)
#   scripts/bump-version.sh minor           0.1.9 -> 0.2.0 on every platform
#   scripts/bump-version.sh major           0.1.9 -> 1.0.0 on every platform
#   scripts/bump-version.sh 0.3.0           exactly 0.3.0 on every platform
#   scripts/bump-version.sh 0.3.2 ios       exactly 0.3.2 on iOS
#
# Nothing is committed; the script prints the release branch to push it on. Merging that branch's
# pull request releases the platforms whose version it changed (scripts/release-plan.sh).
set -euo pipefail

cd "$(dirname "$0")/.."
. scripts/lib-versions.sh

kind=patch
exact=
chosen=
for arg in "$@"; do
  case "$arg" in
    patch|minor|major) kind=$arg ;;
    cedar|desktop) chosen="$chosen cedar" ;;
    android|ios) chosen="$chosen $arg" ;;
    mobile) chosen="$chosen android ios" ;;
    all) chosen=" $PLATFORMS" ;;
    -h|--help) sed -n '2,27p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) kind=exact; exact=${arg#v} ;;
  esac
done

for p in $PLATFORMS; do
  v=$(platform_version "$p")
  if ! printf '%s' "$v" | grep -Eq "$SEMVER"; then
    echo "error: can't read a major.minor.patch version for $p (got '$v')" >&2
    exit 1
  fi
done
current=$(workspace_version)
IFS=. read -r major minor _ <<< "$current"

# What changed since a platform's last release: its own directory for a phone app, and for cedar
# anything that is not a phone app, documentation, the benchmarks or the workflows.
changed_since() {
  local p=$1 tag=$2
  case "$p" in
    android) git diff --name-only "$tag" HEAD -- android ;;
    ios) git diff --name-only "$tag" HEAD -- iOS ;;
    cedar) git diff --name-only "$tag" HEAD -- . ':!android' ':!iOS' ':!docs' ':!bench' ':!examples' ':!.github' ':!*.md' ;;
  esac
}

case "$kind" in
  minor|major)
    if [ -n "$chosen" ]; then
      echo "error: a $kind release is for every platform; name platforms only for a patch" >&2
      exit 1
    fi
    chosen=" $PLATFORMS" ;;
  exact)
    [ -n "$chosen" ] || chosen=" $PLATFORMS" ;;
  patch)
    if [ -z "$chosen" ]; then
      tags=$(release_tags)
      for p in $PLATFORMS; do
        tag=$(last_release_tag "$p" "$tags")
        if [ -z "$tag" ]; then chosen="$chosen $p"; echo "$p: never released"; continue; fi
        if ! git rev-parse -q --verify "refs/tags/$tag" >/dev/null && ! git fetch -q origin "refs/tags/$tag:refs/tags/$tag"; then
          echo "error: can't fetch the tag $tag to see what changed since; name the platforms to bump" >&2
          exit 1
        fi
        n=$(changed_since "$p" "$tag" | wc -l | tr -d ' ')
        if [ "$n" -gt 0 ]; then
          chosen="$chosen $p"; echo "$p: $n files changed since $tag"
        else
          echo "$p: nothing changed since $tag, stays at $(platform_version "$p")"
        fi
      done
      if [ -z "$chosen" ]; then
        echo "error: nothing changed since the last releases; name a platform to bump it anyway" >&2
        exit 1
      fi
    fi ;;
esac

# new_version <platform>
new_version() {
  local v; v=$(platform_version "$1")
  case "$kind" in
    patch) echo "${v%.*}.$((${v##*.} + 1))" ;;
    minor) echo "$major.$((minor + 1)).0" ;;
    major) echo "$((major + 1)).0.0" ;;
    exact) echo "$exact" ;;
  esac
}

# Rewrites a file through awk into a temporary file, so it works with BSD (macOS) and GNU tools.
rewrite() {
  local file=$1; shift
  local tmp; tmp=$(mktemp)
  awk "$@" "$file" > "$tmp" && cat "$tmp" > "$file"
  rm -f "$tmp"
}

# Lock files: the version line right after the name line of each of our own crates.
bump_lock() {
  local file=$1 names=$2 new=$3
  rewrite "$file" -v new="$new" -v names="$names" '
    BEGIN { n = split(names, list, " "); for (i = 1; i <= n; i++) ours["name = \"" list[i] "\""] = 1 }
    /^name = / { mine = ($0 in ours); print; next }
    mine && /^version = / { $0 = "version = \"" new "\""; mine = 0 }
    { print }'
}

set_cedar() {
  local new=$1
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
  bump_lock Cargo.lock "cedar cedar-chat cedar-pairing cedar-llama cedar-contrib-schema cedar-contrib cedar-contrib-web" "$new"
  bump_lock desktop/Cargo.lock "cedar-app cedar-pairing cedar-llama" "$new"
  # Landing site.
  rewrite docs/landing/version.js -v new="$new" -v q="'" '
    /^window\.CEDAR_VERSION = / { $0 = "window.CEDAR_VERSION = " q new q ";" }
    { print }'
}

set_android() {
  rewrite android/gradle.properties -v new="$1" '
    /^cedar\.version *=/ { $0 = "cedar.version=" new }
    { print }'
}

# iOS: every MARKETING_VERSION in the project.
set_ios() {
  rewrite iOS/Cedar.xcodeproj/project.pbxproj -v new="$1" '
    { gsub(/MARKETING_VERSION = [^;]*;/, "MARKETING_VERSION = " new ";"); print }'
}

# Work out every new version before writing any, so a refused one leaves the files alone.
plan=
for p in $PLATFORMS; do
  case " $chosen " in *" $p "*) ;; *) continue ;; esac
  old=$(platform_version "$p")
  new=$(new_version "$p")
  # The limits the release enforces: plain major.minor.patch for the Windows installer (MSI), with
  # major and minor up to 255, and minor and patch under 1000 for the Android versionCode
  # (major*1000000 + minor*1000 + patch).
  if ! printf '%s' "$new" | grep -Eq "$SEMVER"; then
    echo "error: '$new' is not major.minor.patch (no prefix other than v, no -beta suffix)" >&2
    exit 1
  fi
  IFS=. read -r nmaj nmin npat <<< "$new"
  if [ "$nmaj" -gt 255 ] || [ "$nmin" -gt 255 ] || [ "$npat" -gt 999 ]; then
    echo "error: '$new' is too large: major and minor go up to 255 (MSI), patch to 999 (Android versionCode)" >&2
    exit 1
  fi
  if [ "$new" = "$old" ]; then
    echo "error: $p is already at $old" >&2
    exit 1
  fi
  plan="$plan $p:$old:$new"
done

# One Cedar: after the bump every platform must still be on the same major.minor.
lines=
for p in $PLATFORMS; do
  v=$(platform_version "$p")
  for entry in $plan; do
    IFS=: read -r q _ new <<< "$entry"
    [ "$q" != "$p" ] || v=$new
  done
  lines="$lines ${v%.*}"
done
if [ "$(printf '%s\n' $lines | sort -u | wc -l | tr -d ' ')" != 1 ]; then
  echo "error: that would leave the platforms on different major.minor versions ($(printf '%s.x ' $lines | sed 's/ $//'));" >&2
  echo "       a minor or major release is for every platform" >&2
  exit 1
fi

label=
for entry in $plan; do
  IFS=: read -r p old new <<< "$entry"
  "set_$p" "$new"
  printf '%-8s %s -> %s\n' "$p" "$old" "$new"
  label="$label $p $new"
done
git --no-pager diff --stat -- Cargo.toml Cargo.lock desktop/Cargo.toml desktop/Cargo.lock \
  docs/landing/version.js android/gradle.properties iOS/Cedar.xcodeproj/project.pbxproj 2>/dev/null || true

scripts/check-version.sh >/dev/null || { scripts/check-version.sh | grep '^::error' >&2; exit 1; }

# The branch and commit: the version alone when cedar is released, else the phone apps by name.
label=${label# }
case " $chosen " in
  *" cedar "*) branch=$(workspace_version); title="Release $branch"
    [ "$label" = "cedar $branch" ] || title="$title (${label#cedar $branch })" ;;
  *) branch=$(printf '%s' "$label" | tr ' ' '-'); title="Release $label" ;;
esac
case " $chosen " in
  *" cedar "*) grep -q "^## $branch\$" CHANGELOG.md || echo "note: CHANGELOG.md has no '## $branch' section yet" ;;
esac
cat <<EOF2

Next:
  git checkout -b release/$branch
  git commit -am "$title"
  git push -u origin release/$branch
and open a pull request from release/$branch to main. Merging it releases:$(printf ' %s' $(for e in $plan; do echo "${e%%:*}"; done))
EOF2
