#!/usr/bin/env python3
"""Run the targeted tests for the Rust files changed against origin/main.

This is the STANDARD tier's local test step. The full workspace suite runs
only in CI. Changed files are the merge-base(origin/main, HEAD)..HEAD diff,
plus staged, unstaged and untracked files. Each changed `.rs` file that still
exists is mapped to its package with `cargo metadata`:

- an integration test file (`<pkg>/tests/<name>.rs`, a `test` target) runs
  `cargo test -p <pkg> --test <name>`;
- a helper under `<pkg>/tests/` that is not itself a target runs every
  integration test of the package (`--test '*'`);
- a crate root (`lib.rs`, `main.rs`, a `src/bin/*.rs` target) runs the
  package's unit tests unfiltered, because no filter narrows a crate root;
- any other `<pkg>/src/**.rs` becomes a libtest name filter from its module
  path (`src/a/b.rs` -> `a::b`, `src/a/mod.rs` -> `a`). Filters are substring
  matches, so they are a heuristic: pass extra filters with `--filter`.

Everything else (docs, templates, build scripts, examples, benches) gets no
targeted test here; `make check-ci-fast` and CI cover it.

Usage: scripts/test-touched.py [--print] [--base REF] [--profile P] [--filter F]...
`make test-touched` runs it; `make test-touched ARGS=--print` only prints.

trace:TASK-1780 | ai:claude
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import PurePosixPath


@dataclass
class Package:
    name: str
    dir: str  # workspace-relative, no trailing slash ("" for the root)
    lib: bool = False
    bins: bool = False
    roots: set[str] = field(default_factory=set)  # lib/bin src paths (rel)
    tests: dict[str, str] = field(default_factory=dict)  # rel src path -> name


@dataclass
class Plan:
    commands: list[list[str]]
    notes: list[str]


LIB_KINDS = ("lib", "rlib", "proc-macro", "cdylib", "staticlib", "dylib")


def load_packages(metadata: dict) -> list[Package]:
    root = metadata["workspace_root"].rstrip("/")
    members = set(metadata.get("workspace_members", []))
    out = []
    for p in metadata["packages"]:
        if members and p["id"] not in members:
            continue
        rel = os.path.relpath(os.path.dirname(p["manifest_path"]), root)
        pkg = Package(name=p["name"], dir="" if rel == "." else rel)
        for t in p["targets"]:
            src = os.path.relpath(t["src_path"], root)
            kinds = t["kind"]
            if "test" in kinds:
                pkg.tests[src] = t["name"]
            if any(k in LIB_KINDS for k in kinds):
                pkg.lib = True
                pkg.roots.add(src)
            if "bin" in kinds:
                pkg.bins = True
                pkg.roots.add(src)
        out.append(pkg)
    return out


def owning_package(path: str, packages: list[Package]) -> Package | None:
    best = None
    for p in packages:
        prefix = p.dir + "/" if p.dir else ""
        if path.startswith(prefix) and (best is None or len(p.dir) > len(best.dir)):
            best = p
    return best


def module_filter(rel_in_src: str) -> str | None:
    parts = list(PurePosixPath(rel_in_src).with_suffix("").parts)
    if parts and parts[0] == "bin":
        return None
    if parts and parts[-1] == "mod":
        parts.pop()
    return "::".join(parts) or None


def plan(changed: list[str], packages: list[Package], profile: str, extra: list[str]) -> Plan:
    unit_all: set[str] = set()
    filters: dict[str, set[str]] = {}
    integ: dict[str, set[str]] = {}
    all_integ: set[str] = set()
    by_name = {p.name: p for p in packages}
    notes: list[str] = []
    other = 0
    for f in sorted(set(changed)):
        if not f.endswith(".rs"):
            other += 1
            continue
        pkg = owning_package(f, packages)
        if pkg is None:
            notes.append(f"{f}: not in a workspace package; no targeted test")
            continue
        if f in pkg.tests:
            integ.setdefault(pkg.name, set()).add(pkg.tests[f])
            continue
        if f in pkg.roots:
            unit_all.add(pkg.name)
            continue
        inner = f[len(pkg.dir) + 1:] if pkg.dir else f
        if inner.startswith("tests/"):
            all_integ.add(pkg.name)
            continue
        if inner.startswith("src/"):
            mod = module_filter(inner[len("src/"):])
            if mod is None:
                unit_all.add(pkg.name)
            else:
                filters.setdefault(pkg.name, set()).add(mod)
            continue
        notes.append(f"{f}: build script, example or bench; no targeted test")
    if other:
        notes.append(f"{other} non-Rust file(s) changed: no targeted test (make check-ci-fast and CI cover them)")

    base = ["cargo", "test", "--profile", profile]
    cmds: list[list[str]] = []
    for name in sorted(unit_all | set(filters)):
        pkg = by_name[name]
        sel = (["--lib"] if pkg.lib else []) + (["--bins"] if pkg.bins else [])
        if not sel:
            continue
        cmd = base + ["-p", name] + sel
        if name not in unit_all:
            cmd += ["--"] + sorted(filters[name]) + list(extra)
        elif extra:
            cmd += ["--"] + list(extra)
        cmds.append(cmd)
    for name in sorted(all_integ):
        cmds.append(base + ["-p", name, "--test", "*"])
    for name in sorted(set(integ) - all_integ):
        cmd = base + ["-p", name]
        for t in sorted(integ[name]):
            cmd += ["--test", t]
        cmds.append(cmd)
    return Plan(cmds, notes)


def git(root: str, *args: str) -> str:
    return subprocess.run(
        ["git", "-C", root, "-c", "core.quotepath=off", *args],
        check=True, capture_output=True, text=True,
    ).stdout


def changed_files(root: str, base_ref: str) -> list[str]:
    base = git(root, "merge-base", base_ref, "HEAD").strip()
    names = set()
    names.update(git(root, "diff", "--no-renames", "--name-only", base, "HEAD").split("\n"))
    names.update(git(root, "diff", "--no-renames", "--name-only", "HEAD").split("\n"))
    names.update(git(root, "ls-files", "--others", "--exclude-standard").split("\n"))
    # A deleted file has no tests left to run.
    return sorted(n for n in names if n and os.path.exists(os.path.join(root, n)))


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description="Targeted tests for the Rust files changed vs a base ref.")
    ap.add_argument("--base", default=os.environ.get("TEST_TOUCHED_BASE", "origin/main"))
    ap.add_argument("--profile", default=os.environ.get("TEST_TOUCHED_PROFILE", "agent"))
    ap.add_argument("--filter", action="append", default=[], help="extra libtest filter (repeatable)")
    ap.add_argument("--print", action="store_true", help="print the commands; run nothing")
    a = ap.parse_args(argv)
    try:
        root = git(os.getcwd(), "rev-parse", "--show-toplevel").strip()
        changed = changed_files(root, a.base)
    except subprocess.CalledProcessError as e:
        print(f"test-touched: cannot diff against {a.base}: {e.stderr.strip()}", file=sys.stderr)
        return 2
    p0 = plan(changed, [], a.profile, a.filter)
    if not any(f.endswith(".rs") for f in changed):
        for n in p0.notes:
            print(f"test-touched: {n}", file=sys.stderr)
        print(f"test-touched: no Rust changes to test vs {a.base} (the FASTTRACK tier: make check-ci-fast)")
        return 0
    meta = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        cwd=root, check=True, capture_output=True, text=True,
    ).stdout
    p = plan(changed, load_packages(json.loads(meta)), a.profile, a.filter)
    for n in p.notes:
        print(f"test-touched: {n}", file=sys.stderr)
    if not p.commands:
        print(f"test-touched: no targeted tests for the changes vs {a.base}")
        return 0
    for cmd in p.commands:
        print("+ " + " ".join(f"'{c}'" if c == "*" else c for c in cmd), flush=True)
        if a.print:
            continue
        rc = subprocess.run(cmd, cwd=root).returncode
        if rc != 0:
            return rc
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
