#!/usr/bin/env python3
"""Host-compiled repair regeneration program, executed only in the seat sandbox.

This file is embedded in the host binary; the seat's copy is never executed.
The closed table covers the exec-context ledger, template mirrors and CLI help
golden. It deliberately excludes local extensions.
trace:STORY-2710 | ai:codex
"""
import ctypes
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import tomllib

EXEC_CONTEXT_LEDGER = "scripts/exec-context-legacy.json"
CLI_HELP_GOLDEN = "aida-cli-lib/src/tests/golden/cli_help_snapshot.txt"
STORE_WRITERS = "scripts/store-writers.toml"
# Train-run entries; the cheap ones are rerun to prove determinism.
# trace:TASK-3963 | ai:claude
TRAIN_ENTRIES = ("showcase-search", "store-writers")

# The registry is host-owned. Mirror additions are enumerated from resolved
# staged masters before generation, never from generator-created sources.
REGISTRY = (
    ("exec-context", (EXEC_CONTEXT_LEDGER,),
     ("**/*.rs", "tests/**/*.py", "Cargo.toml", "scripts/check-exec-context.py"),
     ("python3", "scripts/check-exec-context.py", "--write-ledger")),
    ("templates", (".claude/skills/", ".claude/commands/", ".claude/hooks/",
                   ".agents/skills/aida-", ".aida/discipline/"),
     ("aida-core/templates/", "aida-core/src/", "Makefile", "scripts/check-template-hooks.py"),
     ("make", "sync-templates")),
    # trace:TASK-3963 | ai:claude
    ("showcase-search", ("docs/showcase/search-data.js",),
     ("docs/", "aida-core/templates/.aida/discipline/"),
     ("python3", "docs/showcase/build.py")),
    ("store-writers", (STORE_WRITERS,),
     ("**/*.rs", "scripts/", "aida-core/templates/", "hooks/", ".githooks/"),
     ("python3", "scripts/check-store-writers.py", "--regenerate")),
    # Reuse the default-feature renderer exercised by CI. trace:TASK-3803 | ai:codex
    ("cli-help", (CLI_HELP_GOLDEN,),
     ("aida-cli-lib/src/", "Cargo.toml", "Cargo.lock"),
     ("env", "AIDA_UPDATE_SNAPSHOTS=1", "cargo", "test", "--locked",
      "--profile", "agent", "-p", "aida-cli-lib", "--lib", "--",
      "--exact", "command_registry::story_1728_command_registry_tests::help_surface_matches_golden_snapshot")),
)
FIXED_OUTPUTS = {path for name, paths, _, _ in REGISTRY if name != "templates" for path in paths}
PACK = ".agents/skills/"
TEMPLATES = "aida-core/templates/"


def git(*args):
    return subprocess.check_output(("git", *args))


def registered(path, mode, masters=None):
    if path == CLI_HELP_GOLDEN:
        return mode == "100644"
    if path in FIXED_OUTPUTS:
        return True
    # Only tracked mirror files with a corresponding master. The ignored
    # portable pack is governed by pack_change_allowed below.
    if path.startswith(".agents/skills/aida-"):
        return False
    if path.startswith(".aida/discipline/") and path.endswith(".md"):
        return masters is None or "aida-core/templates/" + path in masters
    if mode == "120000" and path.startswith((".claude/skills/", ".claude/commands/", ".claude/hooks/")):
        if ".local.md" in path or "/local/" in path:
            return False
        if masters is None:
            return True
        rel = path.removeprefix(".claude/")
        if rel.startswith("skills/") and rel.endswith("/SKILL.md"):
            source = "aida-core/templates/" + rel.removesuffix("/SKILL.md") + ".md"
            return source in masters
        source = "aida-core/templates/" + rel
        return source in masters or any(p.startswith(source + "/") for p in masters)
    return False


def expected_mirrors(masters):
    """Closed mirror paths derived from resolved stage-zero template masters.
    trace:STORY-2710 | ai:codex
    """
    result = {}
    for source, mode in masters.items():
        if mode not in ("100644", "100755") or not source.startswith(TEMPLATES):
            continue
        rel = source[len(TEMPLATES):]
        if rel.startswith(".aida/discipline/") and rel.endswith(".md"):
            result[rel] = mode
        elif rel.startswith(("commands/", "hooks/")):
            path = ".claude/" + rel
            if registered(path, "120000", masters):
                result[path] = "120000"
        elif rel.startswith("skills/"):
            name, sep, _ = rel[len("skills/"):].partition("/")
            if name == "local":
                continue
            if sep:
                path = ".claude/skills/" + name
            elif name.endswith(".md"):
                path = ".claude/skills/" + name[:-3] + "/SKILL.md"
            else:
                continue
            if registered(path, "120000", masters):
                result[path] = "120000"
    return result


def with_baseline(name, argv, base, scratch):
    """store-writers refuses a conflicted inventory without explicit policy
    evidence: the working inventory when it parses, else the pinned base's.
    trace:TASK-3963 | ai:claude
    """
    if name != "store-writers":
        return argv
    try:
        data = Path(STORE_WRITERS).read_bytes()
        tomllib.loads(data.decode())
    except (OSError, ValueError):
        data = git("show", f"{base}:{STORE_WRITERS}")
    baseline = Path(scratch, "store-writers-baseline.toml")
    baseline.write_bytes(data)
    return (*argv, "--baseline", str(baseline))


def ledger_violation(old, new):
    """E2 post-condition: reviewed rows are kept and new rows start blocked.
    trace:TASK-3963 | ai:claude
    """
    def rows(data):
        return {r.get("id"): r for r in tomllib.loads(data.decode()).get("writer", [])}
    before, after = rows(old), rows(new)
    for key, row in after.items():
        if key in before and row != before[key]:
            return "regeneration changed reviewed store-writers row " + repr(key)
        if key not in before and row.get("disposition") != "blocked":
            return "regeneration added a non-blocked store-writers row " + repr(key)
    return None


def train(base, names, budget):
    """Regenerate the selected entries at a train tip and report each output.

    Only the named train entries may run, only their outputs may change, a
    rerun must change nothing, and any refusal rolls every write back.
    trace:TASK-3963 | ai:claude
    """
    entries = [e for e in REGISTRY if e[0] in names]
    if not names or len(entries) != len(set(names)) or not set(names) <= set(TRAIN_ENTRIES):
        raise RuntimeError("train regeneration refuses entries: " + repr(names))
    outputs = {p for e in entries for p in e[1]}
    contents = {}
    before = snapshot(contents)
    deadline = time.monotonic() + budget
    report = []
    try:
        with tempfile.TemporaryDirectory() as scratch:
            for name, products, _, command in entries:
                argv = with_baseline(name, command, base, scratch)
                run_generator(argv, max(0.01, deadline - time.monotonic()), quiet=True)
                first = snapshot()
                run_generator(argv, max(0.01, deadline - time.monotonic()), quiet=True)
                if snapshot() != first:
                    raise RuntimeError(f"{name}: generator output is not deterministic")
                report.append({"name": name, "argv": list(command), "status": 0, "outputs": [
                    {"path": p, "before": before.get(p, (0, None))[1], "after": first.get(p, (0, None))[1]}
                    for p in products]})
        after = snapshot()
        changed = {p for p in before.keys() | after.keys() if before.get(p) != after.get(p)}
        if changed - outputs:
            raise RuntimeError("generator wrote unregistered outputs: " + repr(sorted(changed - outputs)))
        for path in changed:
            if after.get(path, (None,))[0] != 0o644:
                raise RuntimeError("generated output must stay a regular 100644 file: " + repr(path))
        if STORE_WRITERS in changed:
            violation = ledger_violation(contents[STORE_WRITERS], Path(STORE_WRITERS).read_bytes())
            if violation:
                raise RuntimeError(violation)
    except BaseException as error:
        kept = roll_back(before, snapshot(), contents)
        raise RuntimeError(f"{error}; generator writes rolled back" + (f"; kept {kept!r}" if kept else "")) from error
    return report


class GeneratorLaunchError(OSError):
    pass


def run_generator(argv, timeout, quiet=False):
    """Reap recipe descendants before inspecting or restoring any output.

    Linux subreaping retains orphaned recipes even when make exits first.
    The process group stops the live recipe tree; adopted children are killed
    and reaped too, including children that start a separate session.
    trace:STORY-2710 | ai:codex
    """
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.prctl(36, 1, 0, 0, 0) != 0:  # PR_SET_CHILD_SUBREAPER
        raise OSError(ctypes.get_errno(), "could not enable generator subreaping")
    output = subprocess.DEVNULL if quiet else None
    try:
        child = subprocess.Popen(argv, start_new_session=True, stdout=output, stderr=output)
    except OSError as error:
        raise GeneratorLaunchError(str(error)) from error
    try:
        child.wait(timeout=timeout)
        if child.returncode:
            raise subprocess.CalledProcessError(child.returncode, argv)
    finally:
        try:
            os.killpg(child.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        child.wait()
        # Orphans reparent here as their parents die. Never roll back while
        # any such process can still write, even after a successful make exit.
        while True:
            children = Path(f"/proc/self/task/{os.getpid()}/children").read_text().split()
            if not children:
                break
            for pid in children:
                try:
                    os.kill(int(pid), signal.SIGKILL)
                except ProcessLookupError:
                    pass
            try:
                os.waitpid(-1, 0)
            except ChildProcessError:
                break


def expected_pack_outputs(staged, host_outputs):
    """Freeze the closed pack inventory before running any checkout program.

    Host-rendered paths cover the installed generator's inventory. Staged
    template masters also cover skills added by the resolved source tree.
    Generator-created masters cannot authorize additional output writes.
    trace:STORY-2710 | ai:codex
    """
    outputs = set(host_outputs)
    masters = {}
    for source in staged:
        if not source.startswith(TEMPLATES) or not source.endswith(".md"):
            continue
        path = Path(source)
        # A regular leaf through a replaced template directory is not a
        # staged master. Do not let a symlinked ancestor expand this inventory.
        if any(parent.is_symlink() for parent in (path, *path.parents)) or not path.is_file():
            continue
        rel = source[len(TEMPLATES):]
        kind, _, skill = rel.partition("/")
        if kind not in ("skills", "skills-portable"):
            continue
        name, sep, helper = skill.partition("/")
        if not sep:
            name, helper = name.removesuffix(".md"), "SKILL.md"
        elif kind == "skills-portable":
            continue
        if name.startswith("aida-") and ".." not in helper.split("/"):
            masters.setdefault(name, {}).setdefault(kind, set()).add(helper)
    for name, kinds in masters.items():
        # The real renderer requires an ordinary skill prompt. Portable-only
        # overrides and helper-only directories do not create a skill.
        ordinary = kinds.get("skills", set())
        if "SKILL.md" in ordinary:
            outputs.update(PACK + name + "/" + helper for helper in ordinary)
    return outputs


def pack_change_allowed(path, before, after, pack_outputs):
    """Only frozen expected regular files and renderer orphan removals.
    trace:STORY-2710 | ai:codex
    """
    if not path.startswith(PACK + "aida-"):
        return False
    if after is None:
        return before is not None and before[0] != "special"
    return path in pack_outputs and isinstance(after[0], int)


def snapshot(contents=None):
    result = {}
    for root, dirs, files in os.walk(".", followlinks=False):
        dirs[:] = [d for d in dirs if not (root == "." and d in (".git", "target"))]
        for name in files + [d for d in dirs if Path(root, d).is_symlink()]:
            p = Path(root, name)
            key = p.as_posix()
            if p.is_symlink():
                result[key] = ("link", os.readlink(p))
            elif p.is_file():
                data = p.read_bytes()
                result[key] = (p.stat().st_mode & 0o777, hashlib.sha256(data).hexdigest())
                if contents is not None:
                    contents[key] = data
            else:
                result[key] = ("special", p.lstat().st_mode, p.lstat().st_rdev)
        dirs[:] = [d for d in dirs if not Path(root, d).is_symlink()]
    return result


def index_entries():
    entries = {}
    for row in git("ls-files", "-s", "-z").split(b"\0"):
        if not row:
            continue
        meta, raw = row.split(b"\t", 1)
        mode, oid, stage = meta.decode().split()
        entries.setdefault(os.fsdecode(raw), []).append((mode, oid, stage))
    return entries


def roll_back(before, after, contents):
    """Restore pre-run bytes, including unstaged edits and untracked files.
    trace:STORY-2710 | ai:codex
    """
    kept = []
    changed = {p for p in before.keys() | after.keys() if before.get(p) != after.get(p)}
    # Remove shallow replacements first so cleanup never traverses a new
    # symlink ancestor into a different pre-existing user directory.
    for key in sorted(changed, key=lambda p: (len(Path(p).parts), p)):
        path = Path(key)
        if any(parent.is_symlink() for parent in path.parents):
            kept.append(key)
            continue
        if before.get(key, (None,))[0] == "special":
            kept.append(key)
            continue
        if path.is_dir() and not path.is_symlink():
            shutil.rmtree(path)
        elif path.is_symlink() or path.exists():
            path.unlink()
    for key in sorted(changed, key=lambda p: (len(Path(p).parts), p)):
        if key not in before or key in kept:
            continue
        path = Path(key)
        path.parent.mkdir(parents=True, exist_ok=True)
        mode, value = before[key]
        if mode == "link":
            os.symlink(value, path)
        else:
            path.write_bytes(contents[key])
            path.chmod(mode)
    return kept


def main(base, phase, pack_outputs=(), budget=100.0):
    pinned = {}
    for row in git("ls-tree", "-rz", base).split(b"\0"):
        if not row:
            continue
        meta, raw = row.split(b"\t", 1)
        path = os.fsdecode(raw)
        pinned[path] = meta.split()[0].decode()
    # Preparation uses pinned products; generation derives required mirrors
    # from resolved staged masters below. Portable outputs are never required.
    outputs = {path for path, mode in pinned.items() if registered(path, mode, pinned)}
    has_pack = any(p.startswith(TEMPLATES + "skills/") for p in pinned)
    if phase == "restore" and not outputs and not has_pack:
        return
    if phase == "restore":
        # Preparation must not run source-dependent generators. Restore only
        # conflicted products; preserve already merged generated changes.
        unmerged = {os.fsdecode(p) for p in git("diff", "--name-only", "--diff-filter=U", "-z").split(b"\0") if p}
        restore = outputs & unmerged
        if restore:
            subprocess.run(("git", "--literal-pathspecs", "restore", "--source", base,
                            "--staged", "--worktree", "--", *sorted(restore)), check=True)
        return
    # Publication stages resolved authoritative sources before generation.
    # Unmerged source entries therefore indicate incomplete host preparation.
    unmerged = {os.fsdecode(p) for p in git("diff", "--name-only", "--diff-filter=U", "-z").split(b"\0") if p}
    blockers = sorted(unmerged - outputs)
    if blockers:
        raise RuntimeError("regeneration requires resolved authoritative sources: " + repr(blockers))
    staged = {p: entries[0][0] for p, entries in index_entries().items()
              if len(entries) == 1 and entries[0][2] == "0"}
    resolved_outputs = frozenset(expected_mirrors(staged))
    # Masters authorize new mirror paths. Require mirrors already tracked in
    # either tree only while their master survives; retired paths may disappear.
    allowed_outputs = outputs | resolved_outputs
    fixed_outputs = outputs & FIXED_OUTPUTS
    outputs = (resolved_outputs & (set(pinned) | set(staged))) | fixed_outputs
    has_pack = has_pack or any(p.startswith(TEMPLATES + "skills/") for p in staged)
    pack_outputs = expected_pack_outputs(set(staged), pack_outputs)
    contents = {}
    before = snapshot(contents)
    deadline = time.monotonic() + budget
    warning = None
    try:
        for name, products, _, argv in REGISTRY:
            if name == "templates":
                if not has_pack and allowed_outputs <= FIXED_OUTPUTS:
                    continue
            elif fixed_outputs.isdisjoint(products):
                continue
            if name == "cli-help":
                golden = Path(CLI_HELP_GOLDEN)
                if any(p.is_symlink() for p in (golden, *golden.parents)) or (
                    golden.exists() and not golden.is_file()
                ):
                    raise RuntimeError("CLI help golden must be a regular file: " + str(golden))
            # Stay inside the host's bound so a slow generator is rolled back here.
            # trace:BUG-2756 | ai:codex
            if phase == "publish" and name == "cli-help" and golden.is_file():
                previous = snapshot()
                original = golden.read_bytes()
                try:
                    # Leave time for the final exec-context ledger check.
                    reserve = min(30.0, budget / 4) if EXEC_CONTEXT_LEDGER in outputs else 0
                    run_generator(argv, max(0.01, deadline - time.monotonic() - reserve), quiet=True)
                except (subprocess.CalledProcessError, subprocess.TimeoutExpired, GeneratorLaunchError) as error:
                    current = snapshot()
                    changed = {p for p in current.keys() | previous.keys()
                               if current.get(p) != previous.get(p)}
                    if changed - {CLI_HELP_GOLDEN}:
                        raise RuntimeError("failed snapshot command changed other files") from error
                    if roll_back(previous, current, {CLI_HELP_GOLDEN: original}):
                        raise RuntimeError("could not restore CLI help golden") from error
                    warning = f"snapshot regeneration failed: {error}; CI will verify"
            else:
                with tempfile.TemporaryDirectory() as scratch:
                    run_generator(with_baseline(name, argv, base, scratch),
                                  max(0.01, deadline - time.monotonic()))
        after = snapshot()
        unexpected = sorted(
            p for p in before.keys() | after.keys()
            if before.get(p) != after.get(p) and p not in allowed_outputs
            and not pack_change_allowed(p, before.get(p), after.get(p), pack_outputs))
        if unexpected:
            raise RuntimeError("generator wrote unregistered outputs: " + repr(unexpected))
        for path in outputs:
            if path not in after:
                raise RuntimeError("generator removed declared output: " + repr(path))
        if CLI_HELP_GOLDEN in outputs and not isinstance(after[CLI_HELP_GOLDEN][0], int):
            raise RuntimeError("generator produced an invalid CLI help golden")
        # Ratchet regeneration may reduce allowances, never enlarge main's ceiling.
        if EXEC_CONTEXT_LEDGER in outputs:
            run_generator(("python3", "scripts/check-exec-context.py", "--base", base),
                          max(0.01, deadline - time.monotonic()))
    except BaseException as error:
        # A refused or failed run never leaves partial products for a later
        # attempt to stage. trace:STORY-2710 | ai:claude
        kept = roll_back(before, snapshot(), contents)
        note = "; generator writes rolled back to the pre-run working tree"
        if kept:
            note += "; not restorable from the snapshot: " + repr(kept)
        raise RuntimeError(f"{error}{note}") from error
    return warning


if __name__ == "__main__" and sys.argv[2] == "train":
    print(json.dumps(train(sys.argv[1], json.loads(sys.argv[3]),
                           float(sys.argv[4]) if len(sys.argv) > 4 else 300.0)))
elif __name__ == "__main__":
    warning = main(sys.argv[1], sys.argv[2], json.loads(sys.argv[3]) if len(sys.argv) > 3 else (),
         float(sys.argv[4]) if len(sys.argv) > 4 else 100.0)
    if warning:
        print("\n" + warning, file=sys.stderr)
        sys.exit(75)
