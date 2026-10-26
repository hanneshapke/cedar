#!/usr/bin/env bash
# Install a Cedar release on a Linux server set up as DEPLOYMENT.md describes: download the
# release's tarball for this CPU and its checksums.txt from GitHub, check the tarball against it,
# install `cedar` and `cedar-chat` into /usr/local/bin (keeping the old ones as *.prev), restart
# both services, and put the old binaries back if cedar doesn't come up healthy.
#
# Installed as /usr/local/sbin/cedar-update and run as root, either by hand or by the
# cedar-update.service unit, which cedar-update.path starts when the chat's Update button (an
# admin's) writes $CEDAR_UPDATE_DIR/request. Progress goes to $CEDAR_UPDATE_DIR/status.json,
# which the chat's banner shows. Only the version is read from the request, and it must be
# major.minor.patch; the download address is always built here. That directory belongs to the
# `cedar` user, so nothing in it is opened for writing or followed as a link: the status is a
# fresh file renamed into place, and the lock lives in /run.
#
#   sudo cedar-update              the version the chat asked for, or else the latest release
#   sudo cedar-update 0.1.9        that version
#   sudo cedar-update --check      print the installed and the latest version, change nothing
#   sudo cedar-update --force V    also when V is not newer than what is installed
#
# Settings, from the environment or /etc/cedar/update.env:
#   CEDAR_UPDATE_DIR         the request and status directory (default /var/lib/cedar/update)
#   CEDAR_BIN_DIR            where the binaries go (default /usr/local/bin)
#   CEDAR_UPDATE_PULL_IMAGE  1: also pull ghcr.io/hanneshapke/cedar-sandbox:<version> and tag it
#                            as CEDAR_SANDBOX_IMAGE (default cedar-sandbox), for a server that
#                            doesn't build its own image (off by default; DEPLOYMENT.md builds it)
#   CEDAR_HEALTH_URL         checked after the restart (default http://127.0.0.1:8080/healthz)
set -euo pipefail

# shellcheck source=/dev/null
[[ -f /etc/cedar/update.env ]] && . /etc/cedar/update.env

REPO=hanneshapke/cedar
DIR=${CEDAR_UPDATE_DIR:-/var/lib/cedar/update}
BIN=${CEDAR_BIN_DIR:-/usr/local/bin}
DOWNLOADS=${CEDAR_UPDATE_BASE:-https://github.com/$REPO/releases/download}
LATEST_API=${CEDAR_UPDATE_API:-https://api.github.com/repos/$REPO/releases/latest}
PROTO=${CEDAR_UPDATE_PROTO:-=https}
HEALTH=${CEDAR_HEALTH_URL:-http://127.0.0.1:8080/healthz}
HEALTH_WAIT=${CEDAR_HEALTH_WAIT:-60}
LOCK=${CEDAR_UPDATE_LOCK:-/run/cedar-update.lock}
SYSTEMCTL=${SYSTEMCTL:-systemctl}
IMAGE_REPO=ghcr.io/hanneshapke

version=""
target=""
check_only=false
force=false

say() { echo "cedar-update: $*"; }

# status STATE [DETAIL]: what the chat's banner shows. Written whole and renamed.
status() {
    local detail=${2:-}
    detail=${detail//\\/\\\\}
    detail=${detail//\"/\\\"}
    local tmp
    [[ -d $DIR && ! -L $DIR ]] || return 0
    # mktemp creates a new file (never an existing one or a link's target); the rename replaces
    # whatever status.json was, a link included, not what it points to.
    tmp=$(mktemp "$DIR/.status.XXXXXX") || return 0
    printf '{"state":"%s","version":"%s","detail":"%s","at":"%s"}\n' \
        "$1" "$target" "$detail" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >"$tmp"
    chmod 0644 "$tmp"
    mv -fT "$tmp" "$DIR/status.json"
}

fail() {
    say "error: $*" >&2
    [[ $check_only == true ]] || status failed "$*"
    exit 1
}

valid() { [[ $1 =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]; }

# newer A B: whether version A is later than B.
newer() {
    local a b i
    IFS=. read -r -a a <<<"$1"
    IFS=. read -r -a b <<<"$2"
    for i in 0 1 2; do
        ((10#${a[i]} > 10#${b[i]})) && return 0
        ((10#${a[i]} < 10#${b[i]})) && return 1
    done
    return 1
}

fetch() { curl -fsSL --proto "$PROTO" --retry 3 --connect-timeout 15 "$@"; }

# The installed version, or 0.0.0 when it can't be told. Releases before 0.1.8 have no
# --version: without its environment such a cedar-chat stops at once (CEDAR_API_KEY must be
# set) instead of starting a server, so only cedar-chat is asked.
installed() {
    local out
    out=$(env -i timeout 5 "$BIN/cedar-chat" --version 2>/dev/null || true)
    if [[ $out =~ ^cedar-chat\ ([0-9]+\.[0-9]+\.[0-9]+)$ ]]; then
        echo "${BASH_REMATCH[1]}"
    else
        echo 0.0.0
    fi
}

latest() {
    fetch -H "Accept: application/vnd.github+json" -H "User-Agent: cedar-update" "$LATEST_API" |
        sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"v\{0,1\}\([^"]*\)".*/\1/p' | head -n 1
}

arch() {
    case $(uname -m) in
        x86_64 | amd64) echo x64 ;;
        aarch64 | arm64) echo arm64 ;;
        *) fail "releases are built for x86_64 and aarch64, not $(uname -m)" ;;
    esac
}

while (($#)); do
    case $1 in
        --check) check_only=true ;;
        --force) force=true ;;
        -h | --help) sed -n '2,25p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        -*) fail "unknown option $1" ;;
        *) version=$1 ;;
    esac
    shift
done

current=$(installed)

if [[ $check_only == true ]]; then
    newest=$(latest) || fail "could not look up the latest release"
    echo "installed: $current"
    echo "latest:    ${newest:-unknown}"
    exit 0
fi

[[ $(id -u) -eq 0 || -n ${CEDAR_UPDATE_NOT_ROOT:-} ]] || fail "run it as root (sudo cedar-update)"

exec 9>"$LOCK"
flock -n 9 || { say "another update is running"; exit 0; }

# The chat's request: only its version is used, from a small, plain file.
if [[ -z $version && ( -e $DIR/request || -L $DIR/request ) ]]; then
    if [[ -f $DIR/request && ! -L $DIR/request ]]; then
        version=$(head -c 1024 "$DIR/request" | sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([0-9.]*\)".*/\1/p' | head -n 1)
    fi
    rm -f "$DIR/request"
    [[ -n $version ]] || fail "the update request names no version"
fi
if [[ -z $version ]]; then
    version=$(latest) || fail "could not look up the latest release; check the internet connection"
fi
version=${version#v}
valid "$version" || fail "'$version' is not a release version (major.minor.patch)"
target=$version

if [[ $force != true ]] && ! newer "$version" "$current"; then
    say "Cedar $current is installed; $version is not newer, nothing to do (--force installs it anyway)"
    status "done" "Cedar $current is already installed"
    exit 0
fi

name="Cedar-$version-linux-$(arch)"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

say "downloading $name.tar.gz"
status downloading "Downloading $name.tar.gz"
fetch -o "$work/$name.tar.gz" "$DOWNLOADS/v$version/$name.tar.gz" ||
    fail "could not download $name.tar.gz from $DOWNLOADS/v$version"
fetch -o "$work/checksums.txt" "$DOWNLOADS/v$version/checksums.txt" ||
    fail "could not download checksums.txt for $version"

say "checking it"
status verifying "Checking the download"
grep -E "^[0-9a-f]{64}  $name\.tar\.gz\$" "$work/checksums.txt" >"$work/expected" ||
    fail "checksums.txt does not list $name.tar.gz"
(cd "$work" && sha256sum -c --status expected) ||
    fail "$name.tar.gz does not match its checksum; nothing was installed"
mkdir "$work/x"
tar -xzf "$work/$name.tar.gz" -C "$work/x" --no-same-owner "$name/cedar" "$name/cedar-chat" ||
    fail "$name.tar.gz does not hold cedar and cedar-chat"
for exe in cedar cedar-chat; do
    got=$(env -i timeout 5 "$work/x/$name/$exe" --version 2>/dev/null || true)
    [[ $got == "$exe $version" ]] || fail "the downloaded $exe says '${got:-nothing}', not '$exe $version'"
done

if [[ ${CEDAR_UPDATE_PULL_IMAGE:-0} == 1 ]]; then
    image=${CEDAR_SANDBOX_IMAGE:-cedar-sandbox}
    say "pulling $IMAGE_REPO/cedar-sandbox:$version as $image"
    status installing "Pulling the sandbox image"
    docker pull "$IMAGE_REPO/cedar-sandbox:$version" >/dev/null || fail "could not pull $IMAGE_REPO/cedar-sandbox:$version"
    docker tag "$IMAGE_REPO/cedar-sandbox:$version" "$image"
fi

say "installing into $BIN"
status installing "Installing Cedar $version"
for exe in cedar cedar-chat; do
    if [[ -f $BIN/$exe ]]; then
        cp -p "$BIN/$exe" "$BIN/$exe.prev"
    fi
    # A new file renamed over the old one: a running server keeps its binary until it restarts.
    install -m 0755 "$work/x/$name/$exe" "$BIN/.$exe.new"
    mv -f "$BIN/.$exe.new" "$BIN/$exe"
done

healthy() {
    local i
    for ((i = 0; i < HEALTH_WAIT; i++)); do
        if curl -fs -o /dev/null --max-time 3 "$HEALTH" && $SYSTEMCTL is-active --quiet cedar-chat; then
            return 0
        fi
        sleep 1
    done
    return 1
}

say "restarting cedar and cedar-chat"
status restarting "Restarting Cedar"
$SYSTEMCTL restart cedar cedar-chat || true
if healthy; then
    say "Cedar $version is installed (the old binaries are $BIN/cedar.prev and $BIN/cedar-chat.prev)"
    status "done" "Cedar $version is installed"
    exit 0
fi

say "Cedar $version did not come up; putting $current back" >&2
for exe in cedar cedar-chat; do
    [[ -f $BIN/$exe.prev ]] && mv -f "$BIN/$exe.prev" "$BIN/$exe"
done
$SYSTEMCTL restart cedar cedar-chat || true
fail "Cedar $version did not start (see journalctl -u cedar -u cedar-chat); $current was put back"
