"""cedar-bench's command line. `python -m cedar_bench <command> --help` for each.

    serve      start the fixture server
    env        print the cedar settings that point its tools at the fixture server
    validate   check every task file
    persona    create a persona, or show one expanded
    run        run tasks against cedar and grade them
    report     summarize a results.jsonl
    bundle     write the dataset release (tasks, personas, fixtures, manifest)
"""

from __future__ import annotations

import argparse
import datetime as dt
import fnmatch
import hashlib
import json
import re
import shutil
import sys
from pathlib import Path

import yaml

from . import CANARY, __version__, fixtures, personas, report, runner, schema, timeutil

BENCH = Path(__file__).resolve().parent.parent
REPO = BENCH.parent


def _tasks(args) -> list[schema.Task]:
    dirs = [Path(d) for d in args.tasks] if args.tasks else [BENCH / "tasks"]
    tasks = schema.load_tasks(dirs, BENCH / "personas")
    if getattr(args, "filter", None):
        tasks = [t for t in tasks if any(fnmatch.fnmatchcase(t.id, f) for f in args.filter)]
    if getattr(args, "split", None):
        tasks = [t for t in tasks if t.split == args.split]
    return tasks


def cmd_serve(args):
    record = dict(r.split("=", 1) for r in args.record)
    fixtures.serve(BENCH / "fixtures", BENCH / "personas", args.host, args.port,
                   args.public_url, record)


def cmd_env(args):
    for k, v in fixtures.cedar_env(args.fixtures_url).items():
        print(f"{k}={v}" if args.dotenv else f"export {k}={v}")


def cmd_validate(args):
    try:
        tasks = _tasks(args)
    except schema.TaskError as e:
        print(e, file=sys.stderr)
        sys.exit(1)
    anchor = dt.date.today()
    for p in sorted({t.persona for t in tasks if t.persona}):
        personas.load(personas.find(BENCH / "personas", p), anchor)
    by_cat: dict[str, int] = {}
    for t in tasks:
        by_cat[t.category] = by_cat.get(t.category, 0) + 1
    print(f"{len(tasks)} tasks OK: " + ", ".join(f"{c} {n}" for c, n in sorted(by_cat.items())))


def cmd_persona(args):
    if args.action == "new":
        path = personas.write_new(BENCH / "personas", args.id, args.name, args.seed, args.timezone)
        print(f"wrote {path}")
        return
    p = personas.load(personas.find(BENCH / "personas", args.id), timeutil.parse_anchor(args.anchor))
    print(json.dumps(personas.summary(p), indent=2))
    for m in p.messages[-args.limit:]:
        print(f"  mail  {m.date:%a %Y-%m-%d %H:%M}  {m.sender:45.45}  {m.subject}  {m.labels}")
    for e in p.events[:args.limit]:
        print(f"  event {e.start}  {e.summary}")


def cmd_run(args):
    cfg = runner.load_config(Path(args.config))
    if args.repeats:
        cfg["repeats"] = args.repeats
    if args.fixtures_url:
        cfg["fixtures_url"] = args.fixtures_url
    tasks = _tasks(args)
    if not tasks:
        sys.exit("no tasks match")
    out = Path(args.out) if args.out else BENCH / "results" / dt.datetime.now().strftime("%Y%m%d-%H%M%S")
    path = runner.sweep(cfg, tasks, out, BENCH / "fixtures", BENCH / "personas", args.resume,
                        not args.no_register, args.keep_trace)
    md = report.markdown(report.load(path))
    (out / "report.md").write_text(md)
    print(f"\n{md}\nresults in {out}")


def cmd_report(args):
    md = report.markdown(report.load(Path(args.results)))
    if args.out:
        Path(args.out).write_text(md)
    print(md)


def _sha(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def cmd_bundle(args):
    """The release: tasks as JSONL plus the personas and fixtures they need, and a manifest that
    pins the cedar version they were checked against (tool changes can shift scores)."""
    tasks = _tasks(args)
    if not args.include_heldout:
        tasks = [t for t in tasks if t.split == "public"]
    out = Path(args.out)
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)
    with (out / "tasks.jsonl").open("w") as f:
        for t in tasks:
            f.write(json.dumps(t.raw, ensure_ascii=False) + "\n")
    used = sorted({t.persona for t in tasks if t.persona})
    for p in used:
        shutil.copytree(BENCH / "personas" / p, out / "personas" / p)
    shutil.copytree(BENCH / "fixtures", out / "fixtures")
    cargo = (REPO / "Cargo.toml").read_text() if (REPO / "Cargo.toml").exists() else ""
    m = re.search(r'^version\s*=\s*"([^"]+)"', cargo, re.M)
    manifest = {
        "name": "cedar-bench", "bench_version": __version__, "canary": CANARY,
        "cedar_version": m.group(1) if m else None,
        "created_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "tasks": len(tasks), "personas": used,
        "categories": sorted({t.category for t in tasks}),
        "files": {str(p.relative_to(out)): _sha(p) for p in sorted(out.rglob("*")) if p.is_file()},
    }
    (out / "manifest.json").write_text(json.dumps(manifest, indent=2))
    print(f"bundled {len(tasks)} tasks, {len(used)} personas into {out}")


def main(argv=None):
    ap = argparse.ArgumentParser(prog="cedar_bench", description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--version", action="version", version=__version__)
    sub = ap.add_subparsers(dest="cmd", required=True)

    def with_tasks(p):
        p.add_argument("--tasks", nargs="*", help="task directories (default bench/tasks)")
        p.add_argument("--filter", nargs="*", help="task id globs, e.g. 'email_calendar/*'")
        p.add_argument("--split", choices=sorted(schema.SPLITS))

    p = sub.add_parser("serve", help="start the fixture server")
    p.add_argument("--host", default="127.0.0.1")
    p.add_argument("--port", type=int, default=8765)
    p.add_argument("--public-url", help="the address cedar reaches this server at, if not host:port")
    p.add_argument("--record", nargs="*", default=[], metavar="SERVICE=URL",
                   help="forward unrecorded /replay/SERVICE requests to URL and save the answers")
    p.set_defaults(fn=cmd_serve)

    p = sub.add_parser("env", help="print cedar's settings for the fixture server")
    p.add_argument("--fixtures-url", default="http://127.0.0.1:8765")
    p.add_argument("--dotenv", action="store_true", help="KEY=value lines, for a .env file")
    p.set_defaults(fn=cmd_env)

    p = sub.add_parser("validate", help="check every task file")
    with_tasks(p)
    p.set_defaults(fn=cmd_validate)

    p = sub.add_parser("persona", help="create or show a persona")
    p.add_argument("action", choices=["new", "show"])
    p.add_argument("--id", required=True)
    p.add_argument("--name", default="Alex Doe")
    p.add_argument("--seed", type=int, default=1)
    p.add_argument("--timezone", default="America/Los_Angeles")
    p.add_argument("--anchor")
    p.add_argument("--limit", type=int, default=15)
    p.set_defaults(fn=cmd_persona)

    p = sub.add_parser("run", help="run tasks against cedar and grade them")
    p.add_argument("--config", default=str(BENCH / "models.yaml"))
    p.add_argument("--out", help="results directory (default bench/results/<timestamp>)")
    p.add_argument("--repeats", type=int)
    p.add_argument("--fixtures-url")
    p.add_argument("--resume", action="store_true", help="skip trials already in --out")
    p.add_argument("--no-register", action="store_true", help="don't PUT the models first")
    p.add_argument("--keep-trace", action="store_true", help="store every event with each trial")
    with_tasks(p)
    p.set_defaults(fn=cmd_run)

    p = sub.add_parser("report", help="summarize a results.jsonl")
    p.add_argument("results")
    p.add_argument("--out")
    p.set_defaults(fn=cmd_report)

    p = sub.add_parser("bundle", help="write the dataset release")
    p.add_argument("--out", default=str(BENCH / "dist" / f"cedar-bench-{__version__}"))
    p.add_argument("--include-heldout", action="store_true")
    with_tasks(p)
    p.set_defaults(fn=cmd_bundle)

    args = ap.parse_args(argv)
    args.fn(args)


if __name__ == "__main__":
    main()
