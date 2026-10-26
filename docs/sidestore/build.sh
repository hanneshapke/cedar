#!/usr/bin/env bash
# Writes the SideStore/AltStore source to stdout: docs/sidestore/source.json with the site's
# address in front of its relative URLs and each release's IPA as one of the app's versions.
#
#   docs/sidestore/build.sh <site url> <versions dir>
#
# The versions dir holds one JSON file per release: the Cedar-<version>-ios.source.json ipa.yml
# attaches to it (bundle id, version, build number, minimum iOS, size, privacy strings), plus the
# release's downloadURL, date and localizedDescription. pages.yml fetches them. Versions are listed
# newest first; the newest one's privacy strings are the app's, and its fields also fill the
# app-level version, versionDate, downloadURL and size older SideStore and AltStore read. An app
# without any version is left out, since SideStore can't install it.
set -euo pipefail

site=${1:?usage: build.sh <site url> <versions dir>}
dir=${2:?usage: build.sh <site url> <versions dir>}
here=$(cd "$(dirname "$0")" && pwd)

shopt -s nullglob
files=("$dir"/*.json)
if [ ${#files[@]} -gt 0 ]; then
  versions=$(jq -s . "${files[@]}")
else
  versions='[]'
fi

jq --arg site "${site%/}/" --argjson versions "$versions" '
  def url: if test("^https?://") then . else $site + . end;
  def newest_first: sort_by(
      (.version | split(".") | map(tonumber? // 0)),
      (.buildVersion | split(".") | map(tonumber? // 0))
    ) | reverse;
  .website = (if .website == "" then $site else .website end)
  | .iconURL |= url
  | .apps |= map(
      . as $app
      | ([$versions[] | select(.bundleIdentifier == $app.bundleIdentifier)] | newest_first) as $mine
      | select($mine | length > 0)
      | .iconURL |= url
      | .screenshots |= map(url)
      | .appPermissions = {entitlements: [], privacy: $mine[0].privacy}
      | .versions = [$mine[] | {version, buildVersion, date, localizedDescription, downloadURL, size, minOSVersion}]
      | .version = $mine[0].version
      | .versionDate = $mine[0].date
      | .versionDescription = $mine[0].localizedDescription
      | .downloadURL = $mine[0].downloadURL
      | .size = $mine[0].size
    )
  | .featuredApps |= map(select(. as $id | any($versions[]; .bundleIdentifier == $id)))
' "$here/source.json"
