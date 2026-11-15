# Sourced by check-version.sh, release-plan.sh and bump-version.sh: where each platform's version
# lives, and how to read it. Run from the repository's root.
#
# Cedar is released as three platforms that share major.minor and count patches on their own:
#
#   cedar     the servers, the desktop app, the images and the landing site; tag v<version>
#               Cargo.toml               [workspace.package] version
#               desktop/Cargo.toml       [package] version
#               docs/landing/version.js  window.CEDAR_VERSION
#   android   the Android app; tag android-v<version>
#               android/gradle.properties  cedar.version
#   ios       the iOS app; tag ios-v<version>
#               iOS/Cedar.xcodeproj      MARKETING_VERSION, in every build configuration
#
# So 0.11.2 on the desktop and 0.11.0 on a phone are the same Cedar 0.11: a patch release goes
# out only for the platforms it changes, and a minor or major release for all three.

PLATFORMS="cedar android ios"
SEMVER='^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$'

# Up to this version every platform was released together under the one tag v<version>; the
# android-v and ios-v tags start after it.
SHARED_TAGS_UNTIL=0.10.0

# toml_version <file> <section>: the version line inside that [section].
toml_version() {
  awk -v section="[$2]" '
    /^\[/ { on = ($0 == section) }
    on && /^version *=/ { gsub(/.*= *"|".*/, ""); print; exit }' "$1"
}

workspace_version() { toml_version Cargo.toml workspace.package; }
desktop_version() { toml_version desktop/Cargo.toml package; }
site_version() { sed -nE "s/^window\.CEDAR_VERSION = '([^']*)';.*/\1/p" docs/landing/version.js; }
android_version() { sed -nE 's/^cedar\.version *= *(.*)$/\1/p' android/gradle.properties; }
# Every distinct MARKETING_VERSION, on one line: more than one word means the configurations differ.
ios_version() {
  grep -o 'MARKETING_VERSION = [^;]*' iOS/Cedar.xcodeproj/project.pbxproj | sed 's/.* = //' | sort -u | tr '\n' ' ' | sed 's/ $//'
}

# platform_version cedar|android|ios
platform_version() {
  case "$1" in
    cedar) workspace_version ;;
    android) android_version ;;
    ios) ios_version ;;
    *) echo "unknown platform '$1'" >&2; return 1 ;;
  esac
}

# tag_prefix cedar|android|ios: what comes before the version in the platform's release tags.
tag_prefix() {
  case "$1" in
    cedar) echo v ;;
    *) echo "$1-v" ;;
  esac
}

# newer A B: whether version A is later than B.
newer() {
  [ "$1" != "$2" ] && [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | tail -1)" = "$1" ]
}

# release_tags: every tag of the repository, one per line. CEDAR_RELEASE_TAGS replaces the lookup
# (for tests, or to work offline); otherwise GitHub is asked, since a CI checkout carries no tags.
release_tags() {
  if [ -n "${CEDAR_RELEASE_TAGS+x}" ]; then
    printf '%s\n' $CEDAR_RELEASE_TAGS
  else
    git ls-remote --tags --refs origin | sed 's#.*refs/tags/##'
  fi
}

# last_released <platform> <tags>: the platform's newest released version, or nothing.
last_released() {
  local prefix; prefix=$(tag_prefix "$1")
  {
    printf '%s\n' "$2" | sed -n "s/^$prefix\([0-9][0-9.]*\)\$/\1/p"
    # The releases that covered every platform under v<version>.
    [ "$1" = cedar ] || printf '%s\n' "$2" | sed -n 's/^v\([0-9][0-9.]*\)$/\1/p' |
      while read -r v; do newer "$v" "$SHARED_TAGS_UNTIL" || echo "$v"; done
  } | sort -V | tail -1
}

# last_release_tag <platform> <tags>: the tag that release carries.
last_release_tag() {
  local v; v=$(last_released "$1" "$2")
  [ -n "$v" ] || return 0
  if printf '%s\n' "$2" | grep -qx "$(tag_prefix "$1")$v"; then echo "$(tag_prefix "$1")$v"; else echo "v$v"; fi
}
