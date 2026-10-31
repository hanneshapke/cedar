"""Where the benchmark's data lives: the folder with tasks/, personas/ and fixtures/ (bench/ in the
cedar repo). The package itself holds only code, so an installed cedar-bench finds the data:

1. in `CEDAR_BENCH_DIR`, if set;
2. next to the package, when it runs from the checkout (`uv run`, or an editable install);
3. in the current directory or one of its parents (`.` or `./bench`), for an installed package
   run from inside the cedar checkout.
"""

from __future__ import annotations

import os
from functools import cache
from pathlib import Path

ENV = "CEDAR_BENCH_DIR"


def is_bench(path: Path) -> bool:
    return (path / "tasks").is_dir() and (path / "personas").is_dir() and (path / "fixtures").is_dir()


@cache
def bench_dir() -> Path:
    if os.environ.get(ENV):
        path = Path(os.environ[ENV]).expanduser().resolve()
        if not is_bench(path):
            raise SystemExit(f"${ENV} is {path}, which has no tasks/, personas/ and fixtures/")
        return path
    beside = Path(__file__).resolve().parent.parent
    if is_bench(beside):
        return beside
    here = Path.cwd().resolve()
    for d in (here, *here.parents):
        for candidate in (d, d / "bench"):
            if is_bench(candidate):
                return candidate
    raise SystemExit(
        "can't find the benchmark data (tasks/, personas/ and fixtures/). Run cedar-bench from "
        f"inside the cedar checkout, or set {ENV} to its bench/ folder.")


def repo_dir() -> Path:
    """The cedar checkout around the data, for its Cargo.toml (the cedar version)."""
    return bench_dir().parent
