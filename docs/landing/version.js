// The Cedar release the landing site shows until GitHub answers with the latest one.
// index.html and install.html read it; nothing else on the site should spell out the version.
// scripts/bump-version.sh sets it with the Cargo.toml versions: a release pull request fails
// (release-check.yml, scripts/check-version.sh) until they match. The phone apps count their own
// patch versions and are released on their own, so the published site also gets
// window.CEDAR_DOWNLOADS = { apk, ipa } here, the newest of each (pages.yml).
window.CEDAR_VERSION = '0.11.0';
