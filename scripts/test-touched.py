#!/usr/bin/env python3
"""Run the targeted tests for the files changed against origin/main.

This is the STANDARD tier's local test step. The full workspace suite runs
only in CI. Changed files are the merge-base(origin/main, HEAD)..HEAD diff,
plus staged, unstaged and untracked files (deleted files included). Each one
is mapped to its package with `cargo metadata` and a scan of the packages'
sources:

- workspace-root build configuration (`Cargo.toml`, `Cargo.lock`, `.cargo/`,
  `rust-toolchain*`) cannot be targeted: the script exits nonzero and says to
  run `make check-ci-compile` and rely on CI;
- a package's `Cargo.toml` or build script, a file its build script reads
  (a path literal or `rerun-if-changed` in `build.rs`, e.g.
  `aida-core/templates/**`, `proto/aida.proto`) and a non-Rust file pulled in
  with `include_str!`/`include_bytes!`/`include_dir!` run that package's unit
  tests unfiltered (an input embedded only by a `tests`/`*_tests` module runs
  that module instead);
- an integration test file (`<pkg>/tests/<name>.rs`, a `test` target) runs
  `cargo test -p <pkg> --test <name>`; a helper under `<pkg>/tests/` that is
  not itself a target runs every integration test of the package;
- a crate root (`lib.rs`, `main.rs`, a `src/bin/*.rs` target) and a file
  `include!`d into one run the package's unit tests unfiltered;
- any other `<pkg>/src/**.rs` becomes a libtest filter from its REAL module
  path: a `#[path = "..."] mod name;` declaration is followed to its declaring
  module (`src/tests/x_tests.rs` declared in `src/role.rs` is
  `role::x_tests`), and a conventional path is used only when a parent file
  declares `mod <name>`. A file whose module cannot be resolved runs the
  package's unit tests unfiltered. A `.rs` file read by a guard test with
  `include_str!` also adds that guard test's module.

Before a filtered run the script lists the package's unit tests. A derived
filter that matches 0 tests exits 3 when the derivation is suspect (the file
holds `#[test]`/`mod tests`, or was mapped through `#[path]`/`include!`), so a
wrong filter never reads as a green run; a module with no unit tests of its own
falls back to the package's unit tests unfiltered, with a note. A change to a
package's build-script inputs (aida-core's templates) also runs, in other
packages, the modules that use a symbol that build script generates (e.g.
`EMBEDDED_TEMPLATES`: `scaffold_refresh`, `story_255_discipline_pack_tests`);
crate-root consumers are left to CI. Filters are substring matches; pass extra filters
with `--filter`. Whole-tree source-scanning guard tests (ones that walk
`src/` at run time) are not reached by module filters; CI runs them.

Usage: scripts/test-touched.py [--print] [--base REF] [--profile P] [--filter F]...
`make test-touched` runs it; `make test-touched ARGS=--print` only prints.

Exit codes: 0 green or nothing to test, 2 cannot diff, 3 a suspect filter
matched 0 tests, 4 an untargetable change, otherwise cargo's exit code. Under
`make test-touched`, make itself reports any nonzero exit as `Error 3`/`Error
4` and exits 2.

trace:TASK-1780 | ai:claude
"""

from __future__ import annotations

import argparse
import json
import os
import posixpath
import re
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import PurePosixPath
from typing import Callable


@dataclass
class Package:
    name: str
    dir: str  # workspace-relative, no trailing slash ("" for the root)
    lib: bool = False
    bins: bool = False
    roots: set[str] = field(default_factory=set)  # lib/bin src paths (rel)
    tests: dict[str, str] = field(default_factory=dict)  # rel src path -> name
    build_scripts: set[str] = field(default_factory=set)  # rel build.rs paths


@dataclass
class SourceIndex:
    """What the package sources say about how files are compiled in."""
    # `#[path = "..."] mod name;` target -> (declaring file, mod name)
    path_mods: dict[str, tuple[str, str]] = field(default_factory=dict)
    # `include!("...")` target -> including file
    includes: dict[str, str] = field(default_factory=dict)
    # `include_str!`/`include_bytes!`/`include_dir!` target -> embedding files
    embeds: dict[str, set[str]] = field(default_factory=dict)
    # file or directory a build script reads -> package names
    build_inputs: dict[str, set[str]] = field(default_factory=dict)
    # file text, for `mod name` declaration checks
    texts: dict[str, str] = field(default_factory=dict)
    # package name -> files in OTHER packages that use a symbol its build
    # script generates (e.g. aida-core's EMBEDDED_TEMPLATES)
    consumers: dict[str, set[str]] = field(default_factory=dict)


@dataclass
class Plan:
    commands: list[list[str]]
    notes: list[str]
    # package name -> filters derived from changed paths (each must match a test)
    derived: dict[str, set[str]] = field(default_factory=dict)
    # the subset whose 0-test match means a wrong mapping, not an untested module
    suspect: dict[str, set[str]] = field(default_factory=dict)
    # the subset added only as consumers of a generated symbol (dropped if untested)
    consumer: dict[str, set[str]] = field(default_factory=dict)
    untargetable: list[str] = field(default_factory=list)


LIB_KINDS = ("lib", "rlib", "proc-macro", "cdylib", "staticlib", "dylib")
ROOT_CONFIG_FILES = ("Cargo.toml", "Cargo.lock")
UNTARGETABLE_MSG = "cannot target; run make check-ci-compile and rely on CI"

PATH_MOD_RE = re.compile(
    r'^#\[path\s*=\s*"([^"]+)"\]\s*(?:#\[[^\]]*\]\s*)*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;',
    re.M,
)
# One pass over each file for include!/include_str!/include_bytes!/include_dir!
# with a literal path, or a `concat!(env!("CARGO_MANIFEST_DIR"), "...")` one.
INCLUDE_RE = re.compile(
    r'include(_str|_bytes|_dir)?!\(\s*(?:"([^"\\]+)"|concat!\(\s*env!\(\s*"CARGO_MANIFEST_DIR"\s*\)\s*,\s*"([^"\\]+)")'
)
STRING_RE = re.compile(r'"([^"\\\n{}]+)"')
RERUN_RE = re.compile(r'rerun-if-changed=([^"\\\n{}]+)')
GENERATED_SYMBOL_RE = re.compile(r'pub static (\w+)')
TEST_CODE_RE = re.compile(r'#\[test\]|\bmod\s+tests\b')


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
            if "custom-build" in kinds:
                pkg.build_scripts.add(src)
            if any(k in LIB_KINDS for k in kinds):
                pkg.lib = True
                pkg.roots.add(src)
            if "bin" in kinds:
                pkg.bins = True
                pkg.roots.add(src)
        out.append(pkg)
    return out


def _join(base_dir: str, rel: str) -> str:
    return posixpath.normpath(posixpath.join(base_dir, rel) if base_dir else rel)


def build_index(root: str, packages: list[Package]) -> SourceIndex:
    """Scan every package's `src/**.rs` and build scripts."""
    idx = SourceIndex()
    for pkg in packages:
        pdir = pkg.dir
        src_dir = _join(pdir, "src")
        files: list[str] = []
        for dirpath, _dirs, names in os.walk(os.path.join(root, src_dir)):
            for n in names:
                if n.endswith(".rs"):
                    files.append(os.path.relpath(os.path.join(dirpath, n), root))
        for f in sorted(files):
            try:
                with open(os.path.join(root, f), encoding="utf-8", errors="replace") as fh:
                    text = fh.read()
            except OSError:
                continue
            idx.texts[f] = text
            d = posixpath.dirname(f)
            for target, name in PATH_MOD_RE.findall(text):
                idx.path_mods.setdefault(_join(d, target), (f, name))
            for m in INCLUDE_RE.finditer(text):
                if m.start() and (text[m.start() - 1].isalnum() or text[m.start() - 1] in '_"\\'):
                    continue  # part of another identifier, or inside a string
                kind, lit, manifest_rel = m.groups()
                if manifest_rel is not None or kind == "_dir":
                    rel = (manifest_rel or lit).replace("$CARGO_MANIFEST_DIR", "").lstrip("/")
                    target = _join(pdir, rel)
                else:
                    target = _join(d, lit)
                if kind is None:
                    idx.includes.setdefault(target, f)
                else:
                    idx.embeds.setdefault(target, set()).add(f)
    # Build scripts last: a generated symbol's consumers may live in any package.
    for pkg in packages:
        pdir = pkg.dir
        src_dir = _join(pdir, "src")
        for bs in sorted(pkg.build_scripts):
            try:
                with open(os.path.join(root, bs), encoding="utf-8", errors="replace") as fh:
                    text = fh.read()
            except OSError:
                continue
            for sym in GENERATED_SYMBOL_RE.findall(text):
                sym_re = re.compile(rf"\b{sym}\b")
                for f, ftext in idx.texts.items():
                    if not _under(f, pdir) and sym_re.search(ftext):
                        idx.consumers.setdefault(pkg.name, set()).add(f)
            for lit in set(STRING_RE.findall(text)) | set(RERUN_RE.findall(text)):
                lit = lit.strip()
                if not lit or lit.startswith(("/", "-")) or ":" in lit or " " in lit:
                    continue
                target = _join(pdir, lit)
                if target.startswith("..") or target in (".", pdir):
                    continue
                if target == src_dir or target.startswith(src_dir + "/"):
                    continue  # sources are covered by the module mapping
                if pdir.startswith(target + "/"):
                    continue  # an ancestor of the package: not an input
                if os.path.exists(os.path.join(root, target)):
                    idx.build_inputs.setdefault(target, set()).add(pkg.name)
    return idx


def owning_package(path: str, packages: list[Package]) -> Package | None:
    best = None
    for p in packages:
        prefix = p.dir + "/" if p.dir else ""
        if path.startswith(prefix) and (best is None or len(p.dir) > len(best.dir)):
            best = p
    return best


def _under(path: str, target: str) -> bool:
    return path == target or path.startswith(target.rstrip("/") + "/")


def _declares(text: str | None, name: str) -> bool:
    return bool(text) and re.search(rf"\bmod\s+{re.escape(name)}\s*[;{{]", text) is not None


def module_path(f: str, pkg: Package, idx: SourceIndex, seen: frozenset = frozenset()) -> list[str] | None:
    """The module path of `f`: [] for the crate root, None if unresolvable."""
    if f in seen:
        return None
    seen = seen | {f}
    if f in pkg.roots:
        return []
    if f in idx.includes:
        return module_path(idx.includes[f], pkg, idx, seen)
    if f in idx.path_mods:
        decl, name = idx.path_mods[f]
        parent = module_path(decl, pkg, idx, seen)
        return None if parent is None else parent + [name]
    src = _join(pkg.dir, "src")
    if not f.startswith(src + "/"):
        return None
    parts = list(PurePosixPath(f[len(src) + 1:]).with_suffix("").parts)
    if parts and parts[0] == "bin":
        return None
    if parts and parts[-1] == "mod":
        parts.pop()
    if not parts:
        return None
    name = parts[-1]
    # Walk up the conventional ancestors; accept the module only when a
    # parent (or an inline `mod x { mod name; }` further up) declares it.
    for depth in range(len(parts) - 1, -1, -1):
        anc = parts[:depth]
        if anc:
            stem = posixpath.join(src, *anc)
            candidates = [stem + ".rs", stem + "/mod.rs"]
        else:
            candidates = sorted(r for r in pkg.roots if not r.startswith(src + "/bin/"))
            candidates += sorted(i for i, host in idx.includes.items() if host in pkg.roots)
        if any(_declares(idx.texts.get(c), name) for c in candidates):
            return parts
    return None


def is_test_module(mod: list[str] | None) -> bool:
    """A `tests` / `*_tests` module: what it embeds is visible only to its tests."""
    return bool(mod) and any(m == "tests" or m.endswith("_tests") for m in mod)


def is_suspect(f: str, idx: SourceIndex) -> bool:
    """A filter from `f` that matches 0 tests is a mapping bug, not an untested module."""
    if f in idx.path_mods or f in idx.includes:
        return True
    return TEST_CODE_RE.search(idx.texts.get(f, "")) is not None


def is_root_config(f: str) -> bool:
    if "/" not in f:
        return f in ROOT_CONFIG_FILES or f.startswith("rust-toolchain")
    return f.startswith(".cargo/")


def plan(changed: list[str], packages: list[Package], profile: str, extra: list[str],
         idx: SourceIndex | None = None) -> Plan:
    idx = idx or SourceIndex()
    unit_all: set[str] = set()
    filters: dict[str, set[str]] = {}
    integ: dict[str, set[str]] = {}
    all_integ: set[str] = set()
    by_name = {p.name: p for p in packages}
    notes: list[str] = []
    untargetable: list[str] = []
    other = 0

    def unfiltered(pkg_name: str, why: str) -> None:
        unit_all.add(pkg_name)
        notes.append(why)

    # (pkg, filter) -> [suspect, consumer-only]
    origin: dict[tuple[str, str], list[bool]] = {}

    def add_module(f: str, pkg: Package, consumer: bool = False) -> None:
        mod = module_path(f, pkg, idx)
        if consumer and not mod:
            notes.append(f"{f}: consumer at the crate root or unresolvable; not targeted (CI covers it)")
        elif mod is None:
            unfiltered(pkg.name, f"{f}: module path not resolvable; {pkg.name} unit tests run unfiltered")
        elif not mod:
            unit_all.add(pkg.name)
        else:
            flt = "::".join(mod)
            filters.setdefault(pkg.name, set()).add(flt)
            o = origin.setdefault((pkg.name, flt), [False, True])
            o[0] = o[0] or is_suspect(f, idx)
            o[1] = o[1] and consumer

    def add_consumers(producer: str) -> None:
        """Modules in other packages that use a symbol `producer`'s build script generates."""
        for c in sorted(idx.consumers.get(producer, ())):
            cpkg = owning_package(c, packages)
            if cpkg is not None:
                add_module(c, cpkg, consumer=True)

    for f in sorted(set(changed)):
        if is_root_config(f):
            untargetable.append(f)
            continue
        embedded = False
        for target, names in sorted(idx.build_inputs.items()):
            if _under(f, target):
                embedded = True
                for n in sorted(names):
                    unfiltered(n, f"{f}: read by {n}'s build script; {n} unit tests run unfiltered")
                    add_consumers(n)
        for target, embedders in sorted(idx.embeds.items()):
            if not _under(f, target):
                continue
            embedded = True
            for e in sorted(embedders):
                epkg = owning_package(e, packages)
                if epkg is None:
                    continue
                emod = module_path(e, epkg, idx)
                if f.endswith(".rs") and f == target or is_test_module(emod):
                    add_module(e, epkg)  # only that test module sees the bytes
                else:
                    unfiltered(epkg.name, f"{f}: embedded by {e}; {epkg.name} unit tests run unfiltered")
        pkg = owning_package(f, packages)
        if pkg is None:
            if f.endswith(".rs"):
                notes.append(f"{f}: not in a workspace package; no targeted test")
            elif not embedded:
                other += 1
            continue
        inner = f[len(pkg.dir) + 1:] if pkg.dir else f
        if inner == "Cargo.toml" or f in pkg.build_scripts:
            unfiltered(pkg.name, f"{f}: package build input; {pkg.name} unit tests run unfiltered")
            if f in pkg.build_scripts:
                add_consumers(pkg.name)
            continue
        if not f.endswith(".rs"):
            if not embedded:
                other += 1
            continue
        if f in pkg.tests:
            integ.setdefault(pkg.name, set()).add(pkg.tests[f])
            continue
        if f in pkg.roots:
            unit_all.add(pkg.name)
            continue
        if inner.startswith("tests/"):
            all_integ.add(pkg.name)
            continue
        if inner.startswith("src/"):
            add_module(f, pkg)
            continue
        notes.append(f"{f}: example or bench; no targeted test")
    notes = list(dict.fromkeys(notes))
    if other:
        notes.append(f"{other} non-Rust file(s) changed: no targeted test (make check-ci-fast and CI cover them)")

    base = ["cargo", "test", "--profile", profile]
    cmds: list[list[str]] = []
    derived: dict[str, set[str]] = {}
    suspect: dict[str, set[str]] = {}
    consumer: dict[str, set[str]] = {}
    for name in sorted(unit_all | set(filters)):
        pkg = by_name[name]
        sel = (["--lib"] if pkg.lib else []) + (["--bins"] if pkg.bins else [])
        if not sel:
            notes.append(f"{name}: no lib or bin target, so no unit tests to run; skipped (CI covers it)")
            continue
        cmd = base + ["-p", name] + sel
        if name not in unit_all:
            derived[name] = set(filters[name])
            sus = {fl for fl in filters[name] if origin.get((name, fl), [False])[0]}
            con = {fl for fl in filters[name] if origin.get((name, fl), [False, False])[1]}
            if sus:
                suspect[name] = sus
            if con:
                consumer[name] = con
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
    return Plan(cmds, notes, derived, suspect, consumer, untargetable)


def unmatched_filters(filters: set[str] | list[str], test_names: list[str]) -> list[str]:
    """Filters that select no test (libtest filters are substring matches)."""
    return sorted(f for f in filters if not any(f in n for n in test_names))


def parse_test_list(output: str) -> list[str]:
    return [line[: -len(": test")] for line in output.splitlines() if line.endswith(": test")]


def execute(p: Plan, extra: list[str], dry: bool,
            run: Callable[[list[str]], int],
            list_tests: Callable[[list[str]], tuple[int, str]]) -> int:
    """Run the plan; refuse untargetable changes and filters matching 0 tests."""
    if p.untargetable:
        print(f"test-touched: {', '.join(p.untargetable)} changed: {UNTARGETABLE_MSG}", file=sys.stderr)
        return 4
    for cmd in p.commands:
        print("+ " + " ".join(f"'{c}'" if c == "*" else c for c in cmd), flush=True)
        if dry:
            continue
        if "--" in cmd:
            head = cmd[: cmd.index("--")]
            pkg = head[head.index("-p") + 1]
            rc, out = list_tests(head + ["--", "--list"])
            if rc != 0:
                return rc
            names = parse_test_list(out)
            derived = p.derived.get(pkg, set())
            dead = unmatched_filters(derived, names)
            suspect = [f for f in dead if f in p.suspect.get(pkg, set())]
            if not derived and len(unmatched_filters(extra, names)) == len(extra):
                suspect = list(extra)  # the caller's own filters select nothing
            if suspect:
                print(
                    f"test-touched: filter(s) {', '.join(suspect)} match 0 of {len(names)} unit tests in {pkg}, "
                    "but the source holds tests or was mapped through #[path]/include!, so the mapping is "
                    "wrong; a filtered run would pass vacuously. This is a failure, not a green run; "
                    f"run the package's unit tests unfiltered ({' '.join(head)}) and fix the mapping",
                    file=sys.stderr,
                )
                return 3
            consumers = [f for f in dead if f in p.consumer.get(pkg, set())]
            untested = [f for f in dead if f not in consumers]
            if untested:
                print(
                    f"test-touched: {', '.join(untested)} has no unit tests of its own; "
                    f"running {pkg}'s unit tests unfiltered instead",
                    file=sys.stderr,
                )
                cmd = head + (["--"] + list(extra) if extra else [])
                print("+ " + " ".join(cmd), flush=True)
            elif consumers:
                print(f"test-touched: consumer module(s) {', '.join(consumers)} have no unit tests; dropped",
                      file=sys.stderr)
                live = [f for f in cmd[cmd.index("--") + 1:] if f not in consumers]
                if not live:
                    continue
                cmd = head + ["--"] + live
                print("+ " + " ".join(cmd), flush=True)
        rc = run(cmd)
        if rc != 0:
            return rc
    return 0


def git(root: str, *args: str) -> str:
    return subprocess.run(
        ["git", "-C", root, "-c", "core.quotepath=off", *args],
        check=True, capture_output=True, text=True,
    ).stdout


def changed_files(root: str, base_ref: str) -> list[str]:
    """Changed paths, deletions included: deleting an input changes the build."""
    base = git(root, "merge-base", base_ref, "HEAD").strip()
    names = set()
    names.update(git(root, "diff", "--no-renames", "--name-only", base, "HEAD").split("\n"))
    names.update(git(root, "diff", "--no-renames", "--name-only", "HEAD").split("\n"))
    names.update(git(root, "ls-files", "--others", "--exclude-standard").split("\n"))
    return sorted(n for n in names if n)


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description="Targeted tests for the files changed vs a base ref.")
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
    if not changed:
        print(f"test-touched: no changes vs {a.base}")
        return 0
    meta = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        cwd=root, check=True, capture_output=True, text=True,
    ).stdout
    packages = load_packages(json.loads(meta))
    p = plan(changed, packages, a.profile, a.filter, build_index(root, packages))
    for n in p.notes:
        print(f"test-touched: {n}", file=sys.stderr)
    if not p.commands and not p.untargetable:
        print(f"test-touched: no Rust compile inputs changed vs {a.base} (the FASTTRACK tier: make check-ci-fast)")
        return 0

    def run(cmd: list[str]) -> int:
        return subprocess.run(cmd, cwd=root).returncode

    def list_tests(cmd: list[str]) -> tuple[int, str]:
        r = subprocess.run(cmd, cwd=root, stdout=subprocess.PIPE, text=True)
        return r.returncode, r.stdout

    return execute(p, a.filter, a.print, run, list_tests)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
