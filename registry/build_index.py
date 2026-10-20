#!/usr/bin/env python3
"""Build or check the community tool registry's index.

    python3 registry/build_index.py           # write registry/index.json
    python3 registry/build_index.py --check   # fail if a tool breaks the rules or the index is stale

Every folder in registry/tools/ is one tool: the files cedar installs (tool.json, the script, and
any other plain files) plus a listing.json that only the registry reads:

    {"summary": "One line for the list", "category": "health", "reviewed": false}

The index lists each tool's current release with the permissions its manifest asks for and the
SHA-256 of every file, so cedar refuses any file that differs from what was reviewed here. The
rules mirror what cedar itself checks (src/tools/custom.rs, src/marketplace.rs); the registry
test (tests/registry.rs) runs cedar's own checks on every tool too.
"""
import argparse
import hashlib
import json
import py_compile
import re
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent
TOOLS = ROOT / "tools"
INDEX = ROOT / "index.json"
LISTING = "listing.json"
INDEX_VERSION = 1
MAX_FILES = 50
MAX_FILE_BYTES = 2 << 20
MAX_TOTAL_BYTES = 8 << 20
MAX_LIST = 20

NAME = re.compile(r"^[a-z0-9][a-z0-9_]{0,63}$")
FILE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,99}$")
LABEL = re.compile(r"^(?!-)[a-z0-9-]{1,63}$")
SECRET = re.compile(r"^[A-Z][A-Z0-9_]{0,63}$")
REQUIREMENT = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._\-\[\],<>=!~* ]{0,199}$")
VERSION = re.compile(r"^\d+\.\d+\.\d+([-+][0-9A-Za-z.-]+)?$")
RESERVED_SECRETS = {"PATH", "HOME", "USER", "SHELL", "PWD", "TMPDIR", "LANG"}
RESERVED_PREFIXES = ("PYTHON", "PIP_", "LD_", "CEDAR_", "DOCKER_", "SSL_", "LC_")
# cedar's own tool groups and names; a tool can't take them.
RESERVED_NAMES = {
    "core", "web_search", "web_fetch", "weather", "maps", "recipes", "transit", "stocks",
    "products", "ask_user", "browser", "gmail", "calendar", "custom", "bash", "read_file",
    "write_file", "run_python", "read_document", "generate_pdf", "generate_chart", "fetch_url",
    "download_file", "weather_forecast", "show_map", "show_recipe", "plan_trip",
    "compare_stocks", "search_products",
}
MANIFEST_KEYS = {
    "name", "description", "input_schema", "entry", "timeout_secs", "version", "author",
    "homepage", "license", "network", "secrets", "pip", "health_data",
}
LISTING_KEYS = {"summary", "category", "reviewed"}
CATEGORIES = {"health", "productivity", "data", "finance", "home", "media", "developer", "other"}
# Things that look like credentials. A false alarm is better than a leaked key.
SECRET_PATTERNS = [
    re.compile(r"-----BEGIN [A-Z ]*PRIVATE KEY-----"),
    re.compile(r"\bAKIA[0-9A-Z]{16}\b"),
    re.compile(r"\bgh[pousr]_[A-Za-z0-9]{36,}\b"),
    re.compile(r"\bsk-[A-Za-z0-9_-]{20,}\b"),
    re.compile(r"\bxox[abpr]-[A-Za-z0-9-]{10,}\b"),
    re.compile(r"(?i)\b(password|passwd|api_key|secret|token)\s*=\s*['\"][^'\"\s]{8,}['\"]"),
]


class Problems(list):
    def add(self, tool, message):
        self.append(f"{tool}: {message}")


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def check_host(host):
    bare = host[2:] if host.startswith("*.") else host
    return 0 < len(bare) <= 253 and all(LABEL.match(label) for label in bare.split("."))


def check_secret(name):
    return (SECRET.match(name) and name not in RESERVED_SECRETS
            and not name.startswith(RESERVED_PREFIXES))


def release_files(folder):
    """The files cedar installs: every regular file but the listing and caches."""
    return sorted(p for p in folder.iterdir()
                  if p.is_file() and p.name != LISTING and p.suffix != ".pyc")


def check_tool(folder, problems):
    """Check one tool folder and return its index entry (None if it can't have one)."""
    name = folder.name
    if not NAME.match(name) or name in RESERVED_NAMES or name.startswith(("gmail_", "calendar_")):
        problems.add(name, "folder names are lowercase letters, digits and _, and not one of cedar's own")
    for p in folder.iterdir():
        if p.is_dir():
            problems.add(name, f"{p.name}/: a tool is a flat folder of files")
    try:
        manifest = json.loads((folder / "tool.json").read_text(encoding="utf-8"))
    except (OSError, ValueError) as e:
        problems.add(name, f"tool.json: {e}")
        return None
    try:
        listing = json.loads((folder / LISTING).read_text(encoding="utf-8"))
    except (OSError, ValueError) as e:
        problems.add(name, f"{LISTING}: {e}")
        return None

    for key in set(manifest) - MANIFEST_KEYS:
        problems.add(name, f"tool.json has an unknown field {key!r}")
    for key in set(listing) - LISTING_KEYS:
        problems.add(name, f"{LISTING} has an unknown field {key!r}")
    if manifest.get("name", name) != name:
        problems.add(name, "tool.json names another tool than its folder")
    if not str(manifest.get("description", "")).strip():
        problems.add(name, "tool.json needs a description")
    version = manifest.get("version")
    if not isinstance(version, str) or not VERSION.match(version):
        problems.add(name, "tool.json needs a version like 1.2.3")
    for key in ("author", "license"):
        if not str(manifest.get(key, "")).strip():
            problems.add(name, f"tool.json needs the {key} field")
    summary = listing.get("summary")
    if not isinstance(summary, str) or not summary.strip() or len(summary) > 120 or "\n" in summary:
        problems.add(name, f"{LISTING} needs a one-line summary (at most 120 characters)")
    if listing.get("category", "other") not in CATEGORIES:
        problems.add(name, f"category must be one of {', '.join(sorted(CATEGORIES))}")

    network = manifest.get("network", [])
    secrets = [s if isinstance(s, str) else (s or {}).get("name", "")
               for s in manifest.get("secrets", [])]
    pip = manifest.get("pip", [])
    for what, values in (("network", network), ("secrets", secrets), ("pip", pip)):
        if not isinstance(values, list) or len(values) > MAX_LIST:
            problems.add(name, f"{what} is a list of at most {MAX_LIST} entries")
    for host in network:
        if not isinstance(host, str) or not check_host(host):
            problems.add(name, f"network: {host!r} is not a lowercase host name (or *.domain)")
    for secret in secrets:
        if not isinstance(secret, str) or not check_secret(secret):
            problems.add(name, f"secrets: {secret!r} is not an allowed variable name")
    if len(set(secrets)) != len(secrets):
        problems.add(name, "a secret is declared twice")
    for req in pip:
        if not isinstance(req, str) or not REQUIREMENT.match(req):
            problems.add(name, f"pip: {req!r} is not a plain requirement (no options, URLs or paths)")
    health = manifest.get("health_data", False) is True
    if health and listing.get("reviewed") is not True:
        problems.add(name, "a tool that handles health data needs a maintainer's review "
                           "(\"reviewed\": true in listing.json, set by a maintainer)")

    files, total = {}, 0
    entry = manifest.get("entry", "tool.py")
    shipped = release_files(folder)
    if len(shipped) > MAX_FILES:
        problems.add(name, f"a release has at most {MAX_FILES} files")
    for p in shipped:
        if p.name == ".env" or (p.name.startswith(".") and p.name != ".env.example"):
            problems.add(name, f"{p.name}: hidden files (above all .env) are never published")
            continue
        if not FILE.match(p.name) and p.name != ".env.example":
            problems.add(name, f"{p.name}: file names are letters, digits, '.', '_' and '-'")
        data = p.read_bytes()
        total += len(data)
        if len(data) > MAX_FILE_BYTES:
            problems.add(name, f"{p.name} is larger than {MAX_FILE_BYTES} bytes")
        text = data.decode("utf-8", errors="replace")
        for pattern in SECRET_PATTERNS:
            if pattern.search(text):
                problems.add(name, f"{p.name} looks like it contains a credential ({pattern.pattern})")
        if p.suffix == ".py":
            with tempfile.TemporaryDirectory() as tmp:
                try:
                    py_compile.compile(str(p), cfile=str(Path(tmp) / "x.pyc"), doraise=True)
                except py_compile.PyCompileError as e:
                    problems.add(name, f"{p.name} doesn't compile: {e.msg}")
        files[p.name] = sha256(data)
    if total > MAX_TOTAL_BYTES:
        problems.add(name, f"a release is at most {MAX_TOTAL_BYTES} bytes")
    if entry not in files:
        problems.add(name, f"the entry script {entry} is missing")

    return {
        "name": name,
        "version": version,
        "summary": summary,
        "category": listing.get("category", "other"),
        "author": manifest.get("author"),
        "homepage": manifest.get("homepage"),
        "license": manifest.get("license"),
        "permissions": {"network": network, "secrets": secrets, "pip": pip, "health_data": health},
        "base_url": f"tools/{name}/",
        "files": files,
        "reviewed": listing.get("reviewed") is True,
    }


def build(problems):
    tools = []
    for folder in sorted(p for p in TOOLS.iterdir() if p.is_dir()):
        if folder.name.startswith(".") or folder.name == "__pycache__":
            continue
        entry = check_tool(folder, problems)
        if entry:
            tools.append(entry)
    return {"version": INDEX_VERSION, "tools": tools}


def render(index):
    return json.dumps(index, indent=2, sort_keys=False) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--check", action="store_true",
                        help="check the tools and that index.json is up to date; write nothing")
    args = parser.parse_args()
    problems = Problems()
    index = build(problems)
    if problems:
        print("The registry has problems:", *problems, sep="\n  ", file=sys.stderr)
        return 1
    text = render(index)
    if args.check:
        current = INDEX.read_text(encoding="utf-8") if INDEX.is_file() else ""
        if current != text:
            print("registry/index.json is out of date: run python3 registry/build_index.py",
                  file=sys.stderr)
            return 1
        print(f"registry ok: {len(index['tools'])} tools")
        return 0
    INDEX.write_text(text, encoding="utf-8")
    print(f"wrote {INDEX.relative_to(ROOT.parent)} with {len(index['tools'])} tools")
    return 0


if __name__ == "__main__":
    sys.exit(main())
