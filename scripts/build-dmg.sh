#!/usr/bin/env bash
# Build a Cedar DMG by hand, with the chat front end ("skin") the app opens by default, and
# optionally as a white-label app: another name for the assistant and the app, its own logo and
# app icon, and its own bundle identifier.
#
#   scripts/build-dmg.sh --skin dossier
#   scripts/build-dmg.sh --brand ~/brands/acme                  # everything from a brand folder
#   scripts/build-dmg.sh --skin v1 --assistant-name Acme --logo acme.svg --icon-background '#0b2545'
#   scripts/build-dmg.sh --brand ~/brands/acme --stage /tmp/acme  # only the rebranded sources
#   scripts/build-dmg.sh --list-skins
#
# The build never touches this checkout: the sources are copied to target/white-label/, rebranded
# there by scripts/rebrand.py, and compiled from there (sharing target/ and desktop/target/, so
# dependencies are built once).
#
# The skin is baked into cedar-chat (CEDAR_DEFAULT_UI at build time, see chat/src/config.rs): `/`
# opens it until a user picks another in Settings -> Interface, which that browser then keeps.
# CHAT_DEFAULT_UI in config.json's `chat_env` still overrides it on a given Mac.
#
# A brand folder holds any of: logo.svg (the mark on light backgrounds), logo-on-dark.svg,
# icon.png or icon.svg (the app icon's glyph, on a transparent background), and brand.conf with
# KEY=value lines for SKIN, ASSISTANT_NAME, APP_NAME, IDENTIFIER and ICON_BACKGROUND. Options
# given on the command line win over the folder.
#
# Runs on macOS with Rust, the Tauri CLI (`cargo install tauri-cli --version '^2' --locked`) and,
# for a logo or icon, Python 3 with Pillow (see scripts/rebrand.py for the SVGs it can draw).
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
       scripts/build-dmg.sh --brand <dir> [options]

  --skin <skin>             default chat front end: $(for s in $SKINS; do printf '%s (%s), ' "$s" "$(skin_label "$s")"; done | sed 's/, $//')
  --brand <dir>             a brand folder (see the top of this script); options below win over it
  --assistant-name <name>   the assistant's name in the chat, the app's menus, notifications and
                            messages, in place of "Cedar" (and "cedar" in prose)
  --name <name>             app and DMG name (default: the assistant's name, else Cedar)
  --identifier <id>         bundle identifier (default: io.github.hanneshapke.cedar); a white-label
                            app with its own identifier keeps its own settings and data folder
  --logo <svg>              the mark in the chat's top bar and the favicon, for light backgrounds
  --logo-on-dark <svg>      the same for dark backgrounds (default: --logo)
  --icon <png|svg>          the app icon's glyph, transparent background (default: from the logo)
  --icon-background <color> the app icon's tile colour (default: #ffffff)
  --target <triple>         aarch64-apple-darwin or x86_64-apple-darwin (default: this Mac's)
  --out <dir>               where the DMG goes (default: dist/)
  --notarize                also notarize and staple the DMG (needs the APPLE_API_* variables)
  --stage <dir>             only write the rebranded sources to <dir> and stop (any OS); run
                            cargo run -p cedar-chat there to look at them
  --list-skins              print the skins and exit
  -h, --help                this help
EOF
}

skin= name= identifier= target= out="$ROOT/dist" notarize=0 brand= assistant= logo= logo_dark= icon= icon_bg= stage_only=
while [ $# -gt 0 ]; do
  case "$1" in
    --skin) skin=${2:?--skin needs a value}; shift 2 ;;
    --brand) brand=${2:?--brand needs a value}; shift 2 ;;
    --assistant-name) assistant=${2:?--assistant-name needs a value}; shift 2 ;;
    --name) name=${2:?--name needs a value}; shift 2 ;;
    --identifier) identifier=${2:?--identifier needs a value}; shift 2 ;;
    --logo) logo=${2:?--logo needs a value}; shift 2 ;;
    --logo-on-dark) logo_dark=${2:?--logo-on-dark needs a value}; shift 2 ;;
    --icon) icon=${2:?--icon needs a value}; shift 2 ;;
    --icon-background) icon_bg=${2:?--icon-background needs a value}; shift 2 ;;
    --target) target=${2:?--target needs a value}; shift 2 ;;
    --out) out=${2:?--out needs a value}; shift 2 ;;
    --notarize) notarize=1; shift ;;
    --stage) stage_only=${2:?--stage needs a value}; shift 2 ;;
    --list-skins)
      for s in $SKINS; do printf '%s  %s\n' "$s" "$(skin_label "$s")"; done
      exit 0 ;;
    -h | --help) usage; exit 0 ;;
    *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done

die() { echo "error: $*" >&2; exit 1; }
abspath() { (cd "$(dirname "$1")" && printf '%s/%s\n' "$(pwd)" "$(basename "$1")"); }

# The brand folder fills in what the command line left out.
if [ -n "$brand" ]; then
  [ -d "$brand" ] || die "--brand: $brand is not a folder"
  if [ -f "$brand/brand.conf" ]; then
    while IFS= read -r line || [ -n "$line" ]; do
      case "$line" in '' | '#'*) continue ;; esac
      key=${line%%=*} value=${line#*=}
      value=${value#\"} value=${value%\"}
      case "$key" in
        SKIN) skin=${skin:-$value} ;;
        ASSISTANT_NAME) assistant=${assistant:-$value} ;;
        APP_NAME) name=${name:-$value} ;;
        IDENTIFIER) identifier=${identifier:-$value} ;;
        ICON_BACKGROUND) icon_bg=${icon_bg:-$value} ;;
        *) die "$brand/brand.conf: unknown key '$key'" ;;
      esac
    done <"$brand/brand.conf"
  fi
  [ -n "$logo" ] || [ ! -f "$brand/logo.svg" ] || logo="$brand/logo.svg"
  [ -n "$logo_dark" ] || [ ! -f "$brand/logo-on-dark.svg" ] || logo_dark="$brand/logo-on-dark.svg"
  for f in icon.png icon.svg; do
    [ -n "$icon" ] || [ ! -f "$brand/$f" ] || icon="$brand/$f"
  done
fi

[ -n "$skin" ] || { usage >&2; exit 2; }
skin_given=$skin
skin=$(skin_id "$skin_given") || die "unknown skin '$skin_given'; one of: $SKINS (or their names, see --list-skins)"
for f in "$logo" "$logo_dark" "$icon"; do
  [ -z "$f" ] || [ -f "$f" ] || die "$f: no such file"
done
[ -z "$logo_dark" ] || [ -n "$logo" ] || die "--logo-on-dark needs --logo too"
if [ -n "$identifier" ] && ! printf '%s' "$identifier" | grep -Eq '^[A-Za-z0-9-]+(\.[A-Za-z0-9-]+)+$'; then
  die "--identifier must be reverse-DNS, like com.example.chat"
fi
name=${name:-$assistant}
product=${name:-Cedar}

# 1. A rebranded copy of the sources: what git tracks, plus files not yet committed.
stage=${stage_only:-$ROOT/target/white-label}
mkdir -p "$(dirname "$stage")"
case "$(abspath "$stage")/" in "$ROOT"/target/*) ;; "$ROOT"/*) die "--stage must be outside the checkout (or under target/)" ;; esac
echo "==> Staging the sources in $stage"
# Only ever clear a folder an earlier run staged into.
if [ -d "$stage" ] && [ -n "$(ls -A "$stage")" ] && [ ! -f "$stage/.white-label-stage" ]; then
  die "$stage is not empty and was not staged by this script; pick another --stage"
fi
rm -rf "$stage"
mkdir -p "$stage"
touch "$stage/.white-label-stage"
(cd "$ROOT" && git ls-files -co --exclude-standard -z | tar --null -T - -cf -) | tar -xf - -C "$stage"
rebrand=()
[ -z "$assistant" ] || rebrand+=(--assistant-name "$assistant")
[ -z "$name" ] || rebrand+=(--app-name "$name")
[ -z "$identifier" ] || rebrand+=(--identifier "$identifier")
[ -z "$logo" ] || rebrand+=(--logo "$(abspath "$logo")")
[ -z "$logo_dark" ] || rebrand+=(--logo-on-dark "$(abspath "$logo_dark")")
[ -z "$icon" ] || rebrand+=(--icon "$(abspath "$icon")")
[ -z "$icon_bg" ] || rebrand+=(--icon-background "$icon_bg")
if [ ${#rebrand[@]} -gt 0 ]; then
  echo "==> Rebranding"
  python3 "$ROOT/scripts/rebrand.py" "$stage" "${rebrand[@]}"
fi
if [ -n "$stage_only" ]; then
  echo "==> Rebranded sources in $stage; preview with:"
  echo "    cd $stage && CEDAR_DEFAULT_UI=$skin CEDAR_API_KEY=… cargo run -p cedar-chat"
  exit 0
fi

[ "$(uname -s)" = Darwin ] || die "a DMG can only be built on macOS (--stage works anywhere)"
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
if [ "$notarize" = 1 ]; then
  for v in APPLE_API_KEY APPLE_API_ISSUER APPLE_API_KEY_PATH; do
    [ -n "${!v:-}" ] || die "--notarize needs $v"
  done
fi
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/desktop/Cargo.toml" | head -n1)
[ -n "$version" ] || die "no version in desktop/Cargo.toml"

echo "==> $product $version for $target, default skin $skin ($(skin_label "$skin"))"

# 2. The servers. CEDAR_DEFAULT_UI is read by cedar-chat at compile time; cargo rebuilds it
#    whenever the value changes.
echo "==> Building cedar and cedar-chat"
rustup target add "$target" >/dev/null 2>&1 || true
(cd "$stage" && CARGO_TARGET_DIR="$ROOT/target" CEDAR_DEFAULT_UI=$skin \
  cargo build --release --locked -p cedar -p cedar-chat --target "$target")

# 3. Where the bundler expects the sidecars.
mkdir -p "$stage/desktop/binaries"
install -m 0755 "$ROOT/target/$target/release/cedar" "$stage/desktop/binaries/cedar-$target"
install -m 0755 "$ROOT/target/$target/release/cedar-chat" "$stage/desktop/binaries/cedar-chat-$target"

# 4. The app and the DMG, with the name, identifier and icons rebrand.py wrote into the copy.
bundle_dir="$ROOT/desktop/target/$target/release/bundle/dmg"
rm -f "$bundle_dir"/*.dmg
echo "==> Bundling the app"
(cd "$stage/desktop" && CARGO_TARGET_DIR="$ROOT/desktop/target" cargo tauri build --target "$target" --bundles dmg)

built=$(ls "$bundle_dir"/*.dmg 2>/dev/null | head -n1)
[ -n "$built" ] || die "the bundler made no DMG in $bundle_dir"
mkdir -p "$out"
dmg="$out/$(printf '%s' "$product" | tr ' ' '-')-$version-$skin-$arch.dmg"
mv -f "$built" "$dmg"

# 5. The DMG's own notarization ticket, as the release workflow does it.
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
