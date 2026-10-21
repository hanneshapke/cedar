#!/usr/bin/env python3
"""dep_audit: a community cedar tool. Reads its input as JSON on stdin and prints the result.

Finds the Python and npm dependency files in the workspace (requirements*.txt, pyproject.toml,
poetry.lock, uv.lock, Pipfile.lock, package-lock.json, npm-shrinkwrap.json), checks every pinned
package against the OSV.dev vulnerability database, and looks up the latest release of the direct
dependencies on PyPI and npm. The report lists known vulnerabilities worst first, with the version
that fixes each, then the outdated packages, and ends with `[chart] {...}` lines, which the cedar
chat draws as charts (the same spec as the generate_chart tool's input).

It needs no credentials and only the standard library. cedar runs each call in a container of its
own with network access for the hosts the manifest declares; the workspace is only read, unless
the call asks for an export.

Exit non-zero with a message on stderr to report an error; the model sees it as the tool's
error and can try again differently.
"""
import json
import math
import re
import sys
import tomllib
import urllib.error
import urllib.parse
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from datetime import date
from pathlib import Path

OSV = "https://api.osv.dev/v1"
PYPI = "https://pypi.org/pypi/{}/json"
NPM = "https://registry.npmjs.org/{}/latest"
USER_AGENT = "cedar-dep_audit/1.0 (+https://github.com/hanneshapke/cedar/tree/main/registry/tools/dep_audit)"
SKIP_DIRS = {".git", "node_modules", ".venv", "venv", "env", "__pycache__", ".tox", ".nox",
             "site-packages", "dist", "build", ".mypy_cache", ".pytest_cache"}
LOCKS = {"poetry.lock", "uv.lock", "Pipfile.lock", "package-lock.json", "npm-shrinkwrap.json"}
UNSUPPORTED = {"yarn.lock", "pnpm-lock.yaml", "bun.lockb", "Pipfile", "setup.py", "setup.cfg"}
MAX_FILES, MAX_DEPTH = 20, 4
MAX_VULN_DETAILS, MAX_LATEST, WORKERS = 300, 80, 8
SEVERITIES = ["critical", "high", "moderate", "low", "unknown"]
REQUIREMENT = re.compile(r"^([A-Za-z0-9][A-Za-z0-9._-]*)\s*(\[[^\]]*\])?\s*(.*)$")
EXACT = re.compile(r"^={2,3}\s*([0-9][0-9A-Za-z.+!_-]*)$")


# --- http ------------------------------------------------------------------------------------

def http_json(url, body=None, timeout=30):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(url, data=data, headers={
        "User-Agent": USER_AGENT, "Accept": "application/json",
        **({"Content-Type": "application/json"} if data else {})})
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        return json.load(resp)


# --- versions --------------------------------------------------------------------------------

STAGES = {"dev": 0, "a": 1, "alpha": 1, "b": 2, "beta": 2, "c": 3, "rc": 3, "pre": 3,
          "preview": 3, "post": 5, "rev": 5, "r": 5}


def version_key(v):
    """A sort key for PEP 440 and semver versions, close enough to tell older from newer:
    1.0 == 1.0.0, 1.0.dev1 < 1.0a1 < 1.0b1 < 1.0rc1 < 1.0 < 1.0.post1."""
    v = v.strip().lower().lstrip("v").split("+")[0]
    v = v.split("!", 1)[-1]
    m = re.match(r"(\d+(?:\.\d+)*)(.*)", v)
    if not m:
        return ((), 0, 0)
    release = [int(x) for x in m.group(1).split(".")]
    while len(release) > 1 and release[-1] == 0:
        release.pop()
    rest = m.group(2).strip(".-_")
    if not rest:
        return (tuple(release), 4, 0)
    tag = re.match(r"([a-z]+)[._-]?(\d*)", rest)
    stage = STAGES.get(tag.group(1), 3) if tag else 3
    number = int(tag.group(2)) if tag and tag.group(2) else 0
    return (tuple(release), stage, number)


def release_parts(v):
    m = re.match(r"v?(\d+(?:\.\d+)*)", v.strip())
    parts = [int(x) for x in m.group(1).split(".")] if m else []
    return (parts + [0, 0, 0])[:3]


def how_far(current, latest):
    """'major', 'minor' or 'patch' behind, or None when current is the latest or newer."""
    if version_key(current) >= version_key(latest):
        return None
    a, b = release_parts(current), release_parts(latest)
    return "major" if a[0] != b[0] else "minor" if a[1] != b[1] else "patch"


def normalize(ecosystem, name):
    return re.sub(r"[-_.]+", "-", name).lower() if ecosystem == "PyPI" else name.lower()


# --- finding and reading dependency files ------------------------------------------------------

def inside_workspace(path):
    root = Path.cwd().resolve()
    p = (root / path).resolve()
    if p != root and root not in p.parents:
        sys.exit(f"{path} is outside the workspace")
    if not p.exists():
        sys.exit(f"{path} doesn't exist in the workspace")
    return p


def supported(name):
    return name in LOCKS or name == "pyproject.toml" or (
        name.startswith("requirements") and name.endswith((".txt", ".in")))


def find_files(start):
    """The dependency files under start, a lock file taking the place of the pyproject.toml next
    to it. Returns (files, unsupported files seen)."""
    if start.is_file():
        return [start], []
    found, other = [], []

    def walk(folder, depth):
        try:
            entries = sorted(folder.iterdir())
        except OSError:
            return
        names = {p.name for p in entries}
        for p in entries:
            if p.is_dir():
                if depth < MAX_DEPTH and p.name not in SKIP_DIRS and not p.name.startswith("."):
                    walk(p, depth + 1)
            elif p.name == "pyproject.toml" and names & {"poetry.lock", "uv.lock"}:
                continue
            elif supported(p.name) or (folder.name == "requirements" and p.suffix == ".txt"):
                found.append(p)
            elif p.name in UNSUPPORTED:
                other.append(p)

    walk(start, 0)
    return found[:MAX_FILES], other


class Deps:
    """Every package seen, keyed by (ecosystem, normalized name, version), plus the requirements
    that couldn't be checked and why."""

    def __init__(self):
        self.packages = {}
        self.skipped = []

    def add(self, ecosystem, name, version, source, direct=None, dev=False):
        version = version.strip().lstrip("=v").strip()
        if not re.match(r"\d", version):
            self.skipped.append(f"{name} {version} ({source}): not a released version")
            return
        key = (ecosystem, normalize(ecosystem, name), version)
        p = self.packages.setdefault(key, {
            "ecosystem": ecosystem, "name": name, "version": version, "files": [],
            "direct": direct, "dev": dev})
        if source not in p["files"]:
            p["files"].append(source)
        if direct:
            p["direct"] = True
        p["dev"] = p["dev"] and dev

    def requirement(self, line, source, dev=False):
        """One PEP 508 requirement. Only an exact pin (==1.2.3) names a version to check."""
        line = line.split(";", 1)[0].strip()
        if not line:
            return
        if re.match(r"^(git\+|hg\+|svn\+|bzr\+|https?:|file:|\.|/)", line) or " @ " in line:
            self.skipped.append(f"{line} ({source}): not from a package index")
            return
        m = REQUIREMENT.match(line)
        if not m:
            return
        name, spec = m.group(1), m.group(3).strip()
        pin = EXACT.match(spec)
        if pin and "*" not in pin.group(1):
            self.add("PyPI", name, pin.group(1), source, direct=True, dev=dev)
        else:
            self.skipped.append(f"{name}{(' ' + spec) if spec else ''} ({source}): no exact version")


def read_requirements(path, deps, rel, seen=None):
    seen = seen if seen is not None else set()
    if path in seen:
        return
    seen.add(path)
    text = path.read_text(encoding="utf-8", errors="replace").replace("\\\n", " ")
    dev = "dev" in path.name or "test" in path.name
    for raw in text.splitlines():
        line = re.sub(r"(^|\s)#.*$", "", raw).strip()
        if not line:
            continue
        include = re.match(r"^(-r|--requirement|-c|--constraint)\s*=?\s*(\S+)", line)
        if include:
            target = (path.parent / include.group(2)).resolve()
            if target.is_file() and Path.cwd().resolve() in target.parents:
                read_requirements(target, deps, rel, seen)
            continue
        if line.startswith("-"):
            continue
        line = re.split(r"\s--hash[= ]", line)[0]
        deps.requirement(line, rel(path), dev=dev)


def read_pyproject(path, deps, rel):
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    project = data.get("project", {})
    for req in project.get("dependencies", []):
        deps.requirement(req, rel(path))
    for extra in project.get("optional-dependencies", {}).values():
        for req in extra:
            deps.requirement(req, rel(path))
    for group in data.get("dependency-groups", {}).values():
        for req in group:
            if isinstance(req, str):  # not an {include-group = ...} table
                deps.requirement(req, rel(path), dev=True)
    poetry = data.get("tool", {}).get("poetry", {})
    sections = [(poetry.get("dependencies", {}), False), (poetry.get("dev-dependencies", {}), True)]
    sections += [(g.get("dependencies", {}), True) for g in poetry.get("group", {}).values()]
    for table, dev in sections:
        for name, spec in table.items():
            if name.lower() == "python":
                continue
            version = spec if isinstance(spec, str) else spec.get("version") if isinstance(spec, dict) else None
            if isinstance(version, str) and re.match(r"^(==)?\d[\w.!+-]*$", version.strip()):
                deps.add("PyPI", name, version.strip().lstrip("="), rel(path), direct=True, dev=dev)
            else:
                deps.skipped.append(f"{name} {version or spec} ({rel(path)}): no exact version")


def read_poetry_lock(path, deps, rel):
    for p in tomllib.loads(path.read_text(encoding="utf-8")).get("package", []):
        if p.get("source", {}).get("type") in (None, "legacy"):
            dev = p.get("category") == "dev" or (p.get("groups") and "main" not in p["groups"])
            deps.add("PyPI", p["name"], p["version"], rel(path), dev=bool(dev))


def read_uv_lock(path, deps, rel):
    packages = tomllib.loads(path.read_text(encoding="utf-8")).get("package", [])
    direct, dev = set(), set()
    for p in packages:
        if "registry" not in p.get("source", {}):  # the project itself, or a path/git source
            direct |= {normalize("PyPI", d["name"]) for d in p.get("dependencies", [])}
            for group in (*p.get("dev-dependencies", {}).values(),
                          *p.get("optional-dependencies", {}).values()):
                dev |= {normalize("PyPI", d["name"]) for d in group}
    for p in packages:
        if "registry" in p.get("source", {}) and "version" in p:
            name = normalize("PyPI", p["name"])
            deps.add("PyPI", p["name"], p["version"], rel(path),
                     direct=(name in direct or name in dev) if direct or dev else None,
                     dev=name in dev and name not in direct)


def read_pipfile_lock(path, deps, rel):
    data = json.loads(path.read_text(encoding="utf-8"))
    for section, dev in (("default", False), ("develop", True)):
        for name, p in data.get(section, {}).items():
            if "version" in p:
                deps.add("PyPI", name, p["version"], rel(path), dev=dev)
            else:
                deps.skipped.append(f"{name} ({rel(path)}): not from a package index")


def read_package_lock(path, deps, rel):
    data = json.loads(path.read_text(encoding="utf-8"))
    packages = data.get("packages")
    if packages:  # lockfileVersion 2 and 3
        root = packages.get("", {})
        top = set()
        for key in ("dependencies", "devDependencies", "optionalDependencies", "peerDependencies"):
            top |= set(root.get(key, {}))
        for key, p in packages.items():
            if not key or p.get("link") or "node_modules/" not in key:
                continue
            name = p.get("name") or key.rsplit("node_modules/", 1)[1]
            if "version" in p:
                deps.add("npm", name, p["version"], rel(path),
                         direct=key == f"node_modules/{name}" and name in top, dev=bool(p.get("dev")))
        return

    def walk(tree):  # lockfileVersion 1
        for name, p in tree.items():
            if "version" in p:
                deps.add("npm", name, p["version"], rel(path), dev=bool(p.get("dev")))
            walk(p.get("dependencies", {}))

    walk(data.get("dependencies", {}))


def read(path, deps):
    root = Path.cwd().resolve()
    rel = lambda p: str(p.resolve().relative_to(root)) if root in p.resolve().parents else p.name  # noqa: E731
    name = path.name
    try:
        if name == "pyproject.toml":
            read_pyproject(path, deps, rel)
        elif name == "poetry.lock":
            read_poetry_lock(path, deps, rel)
        elif name == "uv.lock":
            read_uv_lock(path, deps, rel)
        elif name == "Pipfile.lock":
            read_pipfile_lock(path, deps, rel)
        elif name in ("package-lock.json", "npm-shrinkwrap.json"):
            read_package_lock(path, deps, rel)
        elif path.suffix in (".txt", ".in"):
            read_requirements(path, deps, rel)
        else:
            sys.exit(f"{rel(path)} is not a dependency file this tool reads (requirements*.txt, "
                     "pyproject.toml, poetry.lock, uv.lock, Pipfile.lock, package-lock.json)")
    except (ValueError, tomllib.TOMLDecodeError, KeyError, AttributeError) as e:
        deps.skipped.append(f"{rel(path)}: couldn't be read ({e})")


# --- severity ------------------------------------------------------------------------------

CVSS3 = {
    "AV": {"N": 0.85, "A": 0.62, "L": 0.55, "P": 0.2}, "AC": {"L": 0.77, "H": 0.44},
    "UI": {"N": 0.85, "R": 0.62}, "CIA": {"H": 0.56, "L": 0.22, "N": 0.0},
}


def cvss3_score(vector):
    """The CVSS 3.x base score of a vector such as CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H."""
    m = dict(part.split(":", 1) for part in vector.split("/")[1:] if ":" in part)
    try:
        changed = m["S"] == "C"
        pr = {"N": 0.85, "L": 0.68 if changed else 0.62, "H": 0.5 if changed else 0.27}[m["PR"]]
        iss = 1 - math.prod(1 - CVSS3["CIA"][m[k]] for k in ("C", "I", "A"))
        impact = 7.52 * (iss - 0.029) - 3.25 * (iss - 0.02) ** 15 if changed else 6.42 * iss
        exploit = 8.22 * CVSS3["AV"][m["AV"]] * CVSS3["AC"][m["AC"]] * pr * CVSS3["UI"][m["UI"]]
    except KeyError:
        return None
    if impact <= 0:
        return 0.0
    raw = min((1.08 if changed else 1) * (impact + exploit), 10)
    whole = round(raw * 100000)
    return whole / 100000 if whole % 10000 == 0 else (math.floor(whole / 10000) + 1) / 10


def severity_of(vuln):
    """critical, high, moderate, low or unknown, and the CVSS score when there is one."""
    score = None
    for s in vuln.get("severity", []):
        if s.get("type") == "CVSS_V3" and s.get("score", "").startswith("CVSS:3"):
            score = cvss3_score(s["score"])
    label = str(vuln.get("database_specific", {}).get("severity", "")).lower()
    label = {"medium": "moderate"}.get(label, label)
    if label not in SEVERITIES and score is not None:
        label = ("critical" if score >= 9 else "high" if score >= 7 else
                 "moderate" if score >= 4 else "low" if score > 0 else "unknown")
    return (label if label in SEVERITIES else "unknown"), score


def fixed_version(vuln, package):
    """The lowest version that fixes vuln and is newer than the package's."""
    current = version_key(package["version"])
    fixes = []
    for a in vuln.get("affected", []):
        pkg = a.get("package", {})
        if (pkg.get("ecosystem") != package["ecosystem"]
                or normalize(package["ecosystem"], pkg.get("name", "")) != normalize(package["ecosystem"], package["name"])):
            continue
        for r in a.get("ranges", []):
            fixes += [e["fixed"] for e in r.get("events", []) if "fixed" in e]
    newer = [f for f in fixes if version_key(f) > current]
    return min(newer, key=version_key) if newer else None


# --- checking --------------------------------------------------------------------------------

def query_osv(packages):
    """The OSV vulnerability ids of each package (in order), with one batch query per 1000."""
    ids = []
    for i in range(0, len(packages), 1000):
        chunk = packages[i:i + 1000]
        body = {"queries": [{"package": {"name": p["name"], "ecosystem": p["ecosystem"]},
                             "version": p["version"]} for p in chunk]}
        try:
            results = http_json(f"{OSV}/querybatch", body, timeout=60).get("results", [])
        except (urllib.error.URLError, TimeoutError, ValueError) as e:
            sys.exit(f"couldn't reach OSV.dev to check for vulnerabilities ({e}); try again later")
        ids += [[v["id"] for v in r.get("vulns", [])] for r in results]
    return ids


def vuln_details(ids):
    def one(vid):
        try:
            return vid, http_json(f"{OSV}/vulns/{urllib.parse.quote(vid)}")
        except (urllib.error.URLError, TimeoutError, ValueError):
            return vid, {"id": vid}
    with ThreadPoolExecutor(WORKERS) as pool:
        return dict(pool.map(one, ids))


def findings(package, ids, details):
    """The package's vulnerabilities, one per advisory: records that alias each other (a GHSA and
    the PYSEC of the same CVE) are merged, keeping the one that knows its severity."""
    groups = []
    for vid in ids:
        v = details.get(vid, {"id": vid})
        names = {vid, *v.get("aliases", [])}
        severity, score = severity_of(v)
        item = {"id": vid, "names": names, "severity": severity, "score": score,
                "summary": summary_of(v), "fixed": fixed_version(v, package)}
        for g in groups:
            if g["names"] & names:
                g["names"] |= names
                if (g["severity"] == "unknown" and severity != "unknown") or (
                        severity == g["severity"] and vid.startswith("GHSA") and not g["id"].startswith("GHSA")):
                    g.update({k: item[k] for k in ("id", "severity", "score")})
                g["summary"] = g["summary"] or item["summary"]
                g["fixed"] = g["fixed"] or item["fixed"]
                break
        else:
            groups.append(item)
    for g in groups:
        g["cves"] = sorted(n for n in g["names"] if n.startswith("CVE-"))
    return groups


def summary_of(v):
    text = (v.get("summary") or (v.get("details") or "").strip().split("\n")[0]).strip().rstrip(".")
    return text[:157] + "..." if len(text) > 160 else text


def latest_versions(packages):
    def one(p):
        name = p["name"]
        try:
            if p["ecosystem"] == "PyPI":
                return p, http_json(PYPI.format(urllib.parse.quote(name)))["info"]["version"]
            return p, http_json(NPM.format(urllib.parse.quote(name, safe="@")))["version"]
        except (urllib.error.URLError, TimeoutError, ValueError, KeyError):
            return p, None
    with ThreadPoolExecutor(WORKERS) as pool:
        return list(pool.map(one, packages))


# --- report ----------------------------------------------------------------------------------

def report(files, other, packages, vulnerable, outdated, deps, latest_checked, capped):
    out = []
    counts = {}
    for p in packages:
        counts[p["ecosystem"]] = counts.get(p["ecosystem"], 0) + 1
    by_eco = ", ".join(f"{n} {e}" for e, n in sorted(counts.items()))
    out.append(f"Dependency audit of {len(files)} file{'s' * (len(files) != 1)} "
               f"({', '.join(files)}): {len(packages)} pinned packages ({by_eco or 'none'}), "
               f"checked against OSV.dev on {date.today().isoformat()}.")

    total = sum(len(v) for _, v in vulnerable)
    out.append("")
    if not total:
        out.append("No known vulnerabilities in the pinned packages.")
    else:
        sev = {s: 0 for s in SEVERITIES}
        for _, vulns in vulnerable:
            for v in vulns:
                sev[v["severity"]] += 1
        parts = ", ".join(f"{n} {s}" for s, n in sev.items() if n)
        out.append(f"{total} known vulnerabilit{'ies' if total != 1 else 'y'} in "
                   f"{len(vulnerable)} package{'s' * (len(vulnerable) != 1)} ({parts}), worst first:")
        rows = [(SEVERITIES.index(v["severity"]), -(v["score"] or 0), p, v)
                for p, vulns in vulnerable for v in vulns]
        for _, _, p, v in sorted(rows, key=lambda r: r[:2]):
            ident = v["id"] + (f" ({', '.join(v['cves'])})" if v["cves"] and v["id"] not in v["cves"] else "")
            score = f", CVSS {v['score']}" if v["score"] is not None else ""
            fix = f"Fixed in {v['fixed']}." if v["fixed"] else "No fixed version yet."
            where = ", ".join(p["files"])
            dev = ", dev only" if p["dev"] else ""
            out.append(f"- {v['severity'].upper()}{score}: {p['name']} {p['version']} "
                       f"({p['ecosystem']}, {where}{dev}) {ident}: {v['summary'] or 'no summary'}. "
                       f"{fix} https://osv.dev/vulnerability/{v['id']}")
        upgrades = []
        for p, vulns in vulnerable:
            fixes = [v["fixed"] for v in vulns if v["fixed"]]
            if fixes:
                target = max(fixes, key=version_key)
                note = "" if len(fixes) == len(vulns) else " (some issues have no fix yet)"
                upgrades.append(f"{p['name']} >= {target}{note}")
        if upgrades:
            out.append("Upgrades that clear the fixable ones: " + "; ".join(upgrades) + ".")

    if latest_checked:
        out.append("")
        if outdated:
            out.append(f"Outdated, of the {latest_checked} dependencies checked for newer releases "
                       "(major updates may break things; read their changelogs):")
            order = {"major": 0, "minor": 1, "patch": 2}
            for p, latest, far in sorted(outdated, key=lambda r: (order[r[2]], r[0]["name"].lower())):
                out.append(f"- {p['name']} {p['version']} -> {latest} ({far}, {p['ecosystem']})")
        else:
            out.append(f"All {latest_checked} dependencies checked are on their latest release.")
        if capped:
            out.append(f"(Only the first {MAX_LATEST} were checked for newer releases.)")
    if deps.skipped:
        out.append("")
        shown = deps.skipped[:15]
        more = len(deps.skipped) - len(shown)
        out.append("Not checked (pin exact versions, or audit the lock file, to cover them):")
        out += [f"- {s}" for s in shown]
        if more > 0:
            out.append(f"- and {more} more")
    if other:
        out.append("")
        out.append("Also found, but not read by this tool: " + ", ".join(other) + ".")
    return "\n".join(out)


def charts(vulnerable, outdated, latest_checked):
    out = []
    sev = {s: 0 for s in SEVERITIES}
    for _, vulns in vulnerable:
        for v in vulns:
            sev[v["severity"]] += 1
    if any(sev.values()):
        labels = [s for s in SEVERITIES if s != "unknown" or sev[s]]
        out.append({"kind": "bar", "title": "Known vulnerabilities by severity", "labels": labels,
                    "series": [{"name": "vulnerabilities", "values": [sev[s] for s in labels]}],
                    "y_label": "advisories"})
    if latest_checked and outdated:
        far = {"major": 0, "minor": 0, "patch": 0}
        for _, _, f in outdated:
            far[f] += 1
        labels = ["up to date", "patch", "minor", "major"]
        values = [latest_checked - len(outdated), far["patch"], far["minor"], far["major"]]
        out.append({"kind": "bar", "title": "Dependencies by how far behind", "labels": labels,
                    "series": [{"name": "packages", "values": values}], "y_label": "packages"})
    return ["[chart] " + json.dumps(c, separators=(",", ":"), ensure_ascii=False) for c in out]


def export(packages, vulnerable, outdated):
    out = Path("dep_audit")
    out.mkdir(exist_ok=True)
    data = {
        "date": date.today().isoformat(),
        "packages": packages,
        "vulnerabilities": [{**{k: p[k] for k in ("ecosystem", "name", "version", "files")},
                             **{k: v[k] for k in ("id", "cves", "severity", "score", "summary", "fixed")}}
                            for p, vulns in vulnerable for v in vulns],
        "outdated": [{"ecosystem": p["ecosystem"], "name": p["name"], "version": p["version"],
                      "latest": latest, "behind": far} for p, latest, far in outdated],
    }
    (out / "findings.json").write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")


def main():
    args = json.load(sys.stdin) or {}
    start = inside_workspace(str(args.get("path") or "."))
    include_dev = args.get("include_dev", True) is not False
    check_latest = args.get("check_latest", True) is not False

    files, other = find_files(start)
    root = Path.cwd().resolve()
    rel = lambda p: str(p.resolve().relative_to(root)) if root in p.resolve().parents else p.name  # noqa: E731
    if not files:
        seen = f" (found {', '.join(rel(p) for p in other)}, which it doesn't read)" if other else ""
        sys.exit(f"no dependency files under {rel(start) if start != root else 'the workspace'}{seen}: "
                 "it reads requirements*.txt, pyproject.toml, poetry.lock, uv.lock, Pipfile.lock "
                 "and package-lock.json")
    deps = Deps()
    for f in files:
        read(f, deps)
    packages = [p for p in deps.packages.values() if include_dev or not p["dev"]]
    packages.sort(key=lambda p: (p["ecosystem"], p["name"].lower(), version_key(p["version"])))
    if not packages:
        print(report([rel(f) for f in files], [rel(p) for p in other], [], [], [], deps, 0, False))
        return

    ids = query_osv(packages)
    unique = list(dict.fromkeys(v for vs in ids for v in vs))[:MAX_VULN_DETAILS]
    details = vuln_details(unique) if unique else {}
    vulnerable = [(p, findings(p, vs, details)) for p, vs in zip(packages, ids) if vs]

    outdated, latest_checked, capped = [], 0, False
    if check_latest:
        # Direct dependencies first, then those whose file doesn't say (a poetry.lock).
        candidates = [p for p in packages if p["direct"]] + [p for p in packages if p["direct"] is None]
        capped = len(candidates) > MAX_LATEST
        for p, latest in latest_versions(candidates[:MAX_LATEST]):
            if latest:
                latest_checked += 1
                far = how_far(p["version"], latest)
                if far:
                    outdated.append((p, latest, far))

    print(report([rel(f) for f in files], [rel(p) for p in other], packages, vulnerable,
                 outdated, deps, latest_checked, capped))
    lines = charts(vulnerable, outdated, latest_checked)
    if lines:
        print("\n" + "\n".join(lines))
    if args.get("export"):
        export(packages, vulnerable, outdated)


if __name__ == "__main__":
    main()
