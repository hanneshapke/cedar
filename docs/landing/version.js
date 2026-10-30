// The Cedar release the landing site shows until GitHub answers with the latest one.
// index.html and install.html read it; nothing else on the site should spell out the version.
// Bump it on the release/<version> branch with the Cargo.toml and iOS versions: a release
// pull request fails (release-check.yml, scripts/check-version.sh) until they all match.
window.CEDAR_VERSION = '0.3.2';
