#!/usr/bin/env bash
# Builds the contribution site into a directory of static files (default contrib-web/dist):
# the page, its script and styles, and the engine compiled to WebAssembly. Any static host
# serves it; the ingest it sends to must list the site's origin in CONTRIB_CORS_ORIGINS.
#
#   contrib-web/build.sh [OUT_DIR]
#   CONTRIB_INGEST_URL=https://contrib.example.org contrib-web/build.sh
#
# Needs the wasm32-unknown-unknown target: rustup target add wasm32-unknown-unknown
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(dirname "$here")"
out="${1:-$here/dist}"

cargo build --manifest-path "$root/Cargo.toml" -p cedar-contrib-web --target wasm32-unknown-unknown --release --locked \
  --config 'profile.release.strip=true' --config 'profile.release.opt-level="s"' --config 'profile.release.lto=true'

rm -rf "$out"
mkdir -p "$out"
cp "$here/web/"*.html "$here/web/"*.js "$here/web/"*.css "$out/"
cp "$root/docs/assets/favicon.svg" "$out/favicon.svg"
cp "$root/target/wasm32-unknown-unknown/release/cedar_contrib_web.wasm" "$out/"

if [ -n "${CONTRIB_INGEST_URL:-}" ]; then
  case "$CONTRIB_INGEST_URL" in
    https://*|http://localhost*|http://127.0.0.1*) ;;
    *) echo "CONTRIB_INGEST_URL must be an https:// address (or a local http one)" >&2; exit 1 ;;
  esac
  url="${CONTRIB_INGEST_URL%/}"
  sed -i.bak -E "s#^window\.CEDAR_CONTRIB_CONFIG = .*#window.CEDAR_CONTRIB_CONFIG = { ingest: '$url' };#" "$out/config.js"
  rm -f "$out/config.js.bak"
  grep -q "ingest: '$url'" "$out/config.js"
fi
echo "built the contribution site in $out ($(du -sh "$out" | cut -f1))"
