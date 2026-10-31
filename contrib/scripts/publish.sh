#!/usr/bin/env bash
# Mirror a release that `cedar-contrib release --out DIR` wrote (or that sits under
# releases/<version>/ in the bucket) to a Hugging Face dataset and a GitHub release.
#
#   contrib/scripts/publish.sh v2026.11 ./out/v2026.11 <hf-org>/<dataset> <owner>/<repo>
#
# Needs: huggingface-cli (logged in, write access to the dataset) and gh (logged in). The
# Hugging Face repo's history is squashed after each upload so a withdrawn bundle does not
# linger in an old commit; a GitHub release holds the tarball and the manifest with its
# signature, so a mirror can check it.
set -euo pipefail
version=${1:?version, like v2026.11}
dir=${2:?the release directory}
hf_repo=${3:?hf org/dataset}
gh_repo=${4:?github owner/repo}
[ -f "$dir/manifest.json" ] || { echo "no manifest.json in $dir" >&2; exit 1; }
tarball="cedar-contrib-$version.tar.gz"
tar -C "$(dirname "$dir")" -czf "$tarball" "$(basename "$dir")"
sha=$(sha256sum "$tarball" | cut -d' ' -f1)
echo "$sha  $tarball" > "$tarball.sha256"

# Hugging Face: the release's files under <version>/, then squash the history.
huggingface-cli upload --repo-type dataset "$hf_repo" "$dir" "$version" --commit-message "Release $version"
python3 - "$hf_repo" <<'PY'
import sys
from huggingface_hub import HfApi
HfApi().super_squash_history(repo_id=sys.argv[1], repo_type="dataset")
PY

# GitHub: a release with the tarball, its checksum, and the manifest and signature.
gh release create "contrib-$version" --repo "$gh_repo" --title "cedar-contrib $version" \
  --notes "Contributed conversations, release $version. See manifest.json for every entry and the exclusion list; manifest.sig for the signature." \
  "$tarball" "$tarball.sha256" "$dir/manifest.json" "$dir/manifest.sig"
echo "published $version: $tarball ($sha)"
