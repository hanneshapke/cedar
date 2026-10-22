#!/usr/bin/env bash
# Build a Cedar DMG by hand, with the chat front end ("skin") the app opens by default, and
# optionally under another name and bundle identifier for a white-label build.
#
#   scripts/build-dmg.sh --skin dossier
#   scripts/build-dmg.sh --skin v1 --name "Acme Chat" --identifier com.acme.chat
#   scripts/build-dmg.sh --list-skins
#
# The skin is baked into cedar-chat (CEDAR_DEFAULT_UI at build time, see chat/src/config.rs): `/`
# opens it until a user picks another in Settings -> Interface, which that browser then keeps.
# CHAT_DEFAULT_UI in config.json's `chat_env` still overrides it on a given Mac.
#
# Runs on macOS with Rust and the Tauri CLI (`cargo install tauri-cli --version '^2' --locked`).
# Signing and notarization follow the environment, as in .github/workflows/release.yml:
#   APPLE_SIGNING_IDENTITY (+ APPLE_CERTIFICATE, APPLE_CERTIFICATE_PASSWORD on a machine without
#   the certificate in its keychain) signs; without it the app is signed ad hoc, fine for your own
#   Mac. APPLE_API_KEY, APPLE_API_ISSUER and APPLE_API_KEY_PATH notarize the app, and with
#   --notarize the DMG too.

set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

# The front ends cedar-chat serves (chat/static/vN/), as named in chat/static/ui-pref.js.
SKINS="v1 v2 v3 v4 v5"
skin_label() {
  case "$1" in
    v1) echo "Claude style" ;;
    v2) echo "Cedar style" ;;
    v3) echo "Ledger" ;;
    v4) echo "Dossier" ;;
    v5) echo "Dataiku" ;;
  esac
}
# Accepts vN or the skin's name, in any case.
skin_id() {
  case "$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]' | tr -d ' _-')" in
    v1 | claude | claudestyle) echo v1 ;;
    v2 | cedar | cedarstyle) echo v2 ;;
    v3 | ledger) echo v3 ;;
    v4 | dossier) echo v4 ;;
    v5 | dataiku) echo v5 ;;
    *) return 1 ;;
  esac
}

usage() {
  cat <<EOF
Usage: scripts/build-dmg.sh --skin <skin> [options]

  --skin <skin>         default chat front end: $(for s in $SKINS; do printf '%s (%s), ' "$s" "$(skin_label "$s")"; done | sed 's/, $//')
  --name <name>         app and DMG name (default: Cedar)
  --identifier <id>     bundle identifier (default: io.github.hanneshapke.cedar); a white-label
                        app with its own identifier keeps its own settings and data folder
  --target <triple>     aarch64-apple-darwin or x86_64-apple-darwin (default: this Mac's)
  --out <dir>           where the DMG goes (default: dist/)
  --notarize            also notarize and staple the DMG (needs the APPLE_API_* variables)
  --list-skins          print the skins and exit
  -h, --help            this help
EOF
}

skin= name= identifier= target= out="$ROOT/dist" notarize=0
while [ $# -gt 0 ]; do
  case "$1" in
    --skin) skin=${2:?--skin needs a value}; shift 2 ;;
    --name) name=${2:?--name needs a value}; shift 2 ;;
    --identifier) identifier=${2:?--identifier needs a value}; shift 2 ;;
    --target) target=${2:?--target needs a value}; shift 2 ;;
    --out) out=${2:?--out needs a value}; shift 2 ;;
    --notarize) notarize=1; shift ;;
    --list-skins)
      for s in $SKINS; do printf '%s  %s\n' "$s" "$(skin_label "$s")"; done
      exit 0 ;;
    -h | --help) usage; exit 0 ;;
    *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done

die() { echo "error: $*" >&2; exit 1; }

[ -n "$skin" ] || { usage >&2; exit 2; }
skin_given=$skin
skin=$(skin_id "$skin_given") || die "unknown skin '$skin_given'; one of: $SKINS (or their names, see --list-skins)"
[ "$(uname -s)" = Darwin ] || die "a DMG can only be built on macOS"
command -v cargo >/dev/null || die "cargo not found; install Rust from https://rustup.rs"
cargo tauri --version >/dev/null 2>&1 || die "the Tauri CLI is missing: cargo install tauri-cli --version '^2' --locked"

if [ -z "$target" ]; then
  case "$(uname -m)" in
    arm64) target=aarch64-apple-darwin ;;
    x86_64) target=x86_64-apple-darwin ;;
    *) die "unknown architecture $(uname -m); pass --target" ;;
  esac
fi
case "$target" in
  aarch64-apple-darwin) arch=apple-silicon ;;
  x86_64-apple-darwin) arch=intel ;;
  *) die "unsupported target '$target'" ;;
esac
if [ -n "$identifier" ] && ! printf '%s' "$identifier" | grep -Eq '^[A-Za-z0-9-]+(\.[A-Za-z0-9-]+)+$'; then
  die "--identifier must be reverse-DNS, like com.example.chat"
fi
if [ "$notarize" = 1 ]; then
  for v in APPLE_API_KEY APPLE_API_ISSUER APPLE_API_KEY_PATH; do
    [ -n "${!v:-}" ] || die "--notarize needs $v"
  done
fi

version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/desktop/Cargo.toml" | head -n1)
[ -n "$version" ] || die "no version in desktop/Cargo.toml"
product=${name:-Cedar}

echo "==> $product $version for $target, default skin $skin ($(skin_label "$skin"))"

# 1. The servers. CEDAR_DEFAULT_UI is read by cedar-chat at compile time; cargo rebuilds it
#    whenever the value changes.
echo "==> Building cedar and cedar-chat"
rustup target add "$target" >/dev/null 2>&1 || true
(cd "$ROOT" && CEDAR_DEFAULT_UI=$skin cargo build --release --locked -p cedar -p cedar-chat --target "$target")

# 2. Where the bundler expects the sidecars.
install -m 0755 "$ROOT/target/$target/release/cedar" "$ROOT/desktop/binaries/cedar-$target"
install -m 0755 "$ROOT/target/$target/release/cedar-chat" "$ROOT/desktop/binaries/cedar-chat-$target"

# 3. The app and the DMG. A name or identifier is merged over tauri.conf.json for this build only.
tauri_args=(--target "$target" --bundles dmg)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
if [ -n "$name" ] || [ -n "$identifier" ]; then
  overrides="$work/white-label.json"
  NAME=$name IDENTIFIER=$identifier python3 - "$overrides" <<'PY'
import json, os, sys
conf = {}
if os.environ["NAME"]:
    conf["productName"] = os.environ["NAME"]
if os.environ["IDENTIFIER"]:
    conf["identifier"] = os.environ["IDENTIFIER"]
json.dump(conf, open(sys.argv[1], "w"))
PY
  tauri_args+=(--config "$overrides")
fi
bundle_dir="$ROOT/desktop/target/$target/release/bundle/dmg"
rm -f "$bundle_dir"/*.dmg
echo "==> Bundling the app"
(cd "$ROOT/desktop" && cargo tauri build "${tauri_args[@]}")

built=$(ls "$bundle_dir"/*.dmg 2>/dev/null | head -n1)
[ -n "$built" ] || die "the bundler made no DMG in $bundle_dir"
mkdir -p "$out"
dmg="$out/$(printf '%s' "$product" | tr ' ' '-')-$version-$skin-$arch.dmg"
mv -f "$built" "$dmg"

# 4. The DMG's own notarization ticket, as the release workflow does it.
if [ "$notarize" = 1 ]; then
  echo "==> Notarizing $dmg"
  xcrun notarytool submit "$dmg" --key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY" --issuer "$APPLE_API_ISSUER" --wait
  for attempt in 1 2 3 4 5; do
    if xcrun stapler staple "$dmg"; then break; fi
    [ "$attempt" -lt 5 ] || die "stapling $dmg failed 5 times"
    echo "stapling failed (attempt $attempt); trying again in $((attempt * 20))s"
    sleep $((attempt * 20))
  done
fi

echo "==> $dmg"
shasum -a 256 "$dmg"
