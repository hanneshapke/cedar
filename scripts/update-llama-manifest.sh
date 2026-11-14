#!/usr/bin/env bash
# Pins the llama.cpp release Cedar installs: downloads the builds of one release tag and writes
# their sizes and SHA-256 hashes to llama/assets.txt, which cedar-llama compiles in.
#
#   scripts/update-llama-manifest.sh b12370
#
# The tag must be a release that has every file below (the newest tag is often still building).
# Tags: https://github.com/ggml-org/llama.cpp/releases
set -euo pipefail

tag=${1:?usage: update-llama-manifest.sh <tag, like b12370>}
base="https://github.com/ggml-org/llama.cpp/releases/download/$tag"
out="$(cd "$(dirname "$0")/.." && pwd)/llama/assets.txt"

# Keep in step with Build::assets in llama/src/release.rs (a test there checks it).
assets=(
  "llama-$tag-bin-macos-arm64.tar.gz"
  "llama-$tag-bin-win-cuda-12.4-x64.zip"
  "cudart-llama-bin-win-cuda-12.4-x64.zip"
  "llama-$tag-bin-win-vulkan-x64.zip"
  "llama-$tag-bin-ubuntu-cuda-12.8-x64.tar.gz"
  "cudart-llama-$tag-bin-ubuntu-cuda-12.8-x64.tar.gz"
  "llama-$tag-bin-ubuntu-vulkan-x64.tar.gz"
)

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

{
  echo "# Written by scripts/update-llama-manifest.sh; do not edit by hand."
  echo "# The llama.cpp release Cedar installs, then a line per file: name, bytes, SHA-256."
  echo "tag $tag"
} > "$tmp/assets.txt"

for name in "${assets[@]}"; do
  echo "Downloading $name" >&2
  curl -fsSL --retry 3 -o "$tmp/file" "$base/$name"
  size=$(wc -c < "$tmp/file" | tr -d ' ')
  if command -v sha256sum > /dev/null; then
    hash=$(sha256sum "$tmp/file" | cut -d' ' -f1)
  else
    hash=$(shasum -a 256 "$tmp/file" | cut -d' ' -f1)
  fi
  echo "$name $size $hash" >> "$tmp/assets.txt"
done

mv "$tmp/assets.txt" "$out"
echo "Wrote $out" >&2
