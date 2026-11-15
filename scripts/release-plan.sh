#!/usr/bin/env bash
# Works out what a release from this commit publishes: every platform whose version has no
# release tag yet (v<version> for cedar, android-v<version>, ios-v<version>). A release pull
# request therefore releases exactly the platforms whose version it bumps; a phone app nothing
# changed in keeps its version and is not built, uploaded or submitted again.
#
# Usage: scripts/release-plan.sh [--only "<platforms>"]
#
#   --only "cedar ios"   plan these platforms whether or not they are released already (what a
#                        manual run of release.yml builds); "all" is every platform
#
# Prints the plan to stderr and, for a workflow, these lines to stdout:
#
#   cedar=true|false  android=true|false  ios=true|false     whether the platform is released
#   cedar_version=    android_version=    ios_version=       its version
#   cedar_previous=   android_previous=   ios_previous=      the tag of its last release, if any
#   any=true|false                                           whether anything is released
#
# Fails if a version to release is not later than the platform's last release. Run
# scripts/check-version.sh first: this script takes the versions to be consistent.
set -euo pipefail

cd "$(dirname "$0")/.."
. scripts/lib-versions.sh

only=
while [ $# -gt 0 ]; do
  case "$1" in
    --only) only=${2:?--only needs a list of platforms}; shift 2 ;;
    -h|--help) sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "usage: $0 [--only \"<platforms>\"]" >&2; exit 2 ;;
  esac
done
only=$(printf '%s' "$only" | tr ',' ' ')
case " $only " in
  *" all "*) only=$PLATFORMS ;;
  *" mobile "*) only="$only android ios" ;;
esac
for p in $only; do
  case "$p" in cedar|android|ios|mobile) ;; *) echo "::error::unknown platform '$p' (cedar, android, ios, mobile or all)" >&2; exit 2 ;; esac
done

tags=$(release_tags)
any=false
fail=0
for p in $PLATFORMS; do
  version=$(platform_version "$p")
  previous=$(last_released "$p" "$tags")
  previous_tag=$(last_release_tag "$p" "$tags")
  if [ -n "$only" ]; then
    case " $only " in *" $p "*) release=true ;; *) release=false ;; esac
    why="asked for"
  elif [ "$version" = "$previous" ]; then
    release=false
  elif [ -z "$previous" ] || newer "$version" "$previous"; then
    release=true
    why="last release: ${previous:-none}"
  else
    echo "::error::$p is at $version, older than its last release $previous ($previous_tag)" >&2
    release=false; fail=1
  fi
  if [ "$release" = true ]; then
    any=true
    printf '  %-8s %-9s release as %s%s (%s)\n' "$p" "$version" "$(tag_prefix "$p")" "$version" "$why" >&2
  else
    printf '  %-8s %-9s no release (%s is out)\n' "$p" "$version" "${previous_tag:-nothing}" >&2
  fi
  echo "$p=$release"
  echo "${p}_version=$version"
  echo "${p}_previous=$previous_tag"
done
echo "any=$any"
exit $fail
