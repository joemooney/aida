#!/usr/bin/env python3
"""Host-compiled repair regeneration program, executed only in the seat sandbox.

This file is embedded in the host binary; the seat's copy is never executed.
The closed table mirrors make sync-templates, including its symlinks, portable
pack, discipline copies and hooks. It deliberately excludes local extensions.
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
import time

# Outputs are enumerated from pinned main, never from a seat-authored registry.
REGISTRY = (
    ("exec-context", ("scripts/exec-context-legacy.json",),
     ("**/*.rs", "tests/**/*.py", "Cargo.toml", "scripts/check-exec-context.py"),
     ("python3", "scripts/check-exec-context.py", "--write-ledger")),
    ("templates", (".claude/skills/", ".claude/commands/", ".claude/hooks/",
                   ".agents/skills/aida-", ".aida/discipline/"),
     ("aida-core/templates/", "aida-core/src/", "Makefile", "scripts/check-template-hooks.py"),
     ("make", "sync-templates")),
)
PACK = ".agents/skills/"
TEMPLATES = "aida-core/templates/"


def git(*args):
    return subprocess.check_output(("git", *args))


def registered(path, mode, masters=None):
    if path == "scripts/exec-context-legacy.json":
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


def run_generator(argv, timeout):
    """Reap recipe descendants before inspecting or restoring any output.

    Linux subreaping retains orphaned recipes even when make exits first.
    The process group stops the live recipe tree; adopted children are killed
    and reaped too, including children that start a separate session.
    trace:STORY-2710 | ai:codex
    """
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.prctl(36, 1, 0, 0, 0) != 0:  # PR_SET_CHILD_SUBREAPER
        raise OSError(ctypes.get_errno(), "could not enable generator subreaping")
    child = subprocess.Popen(argv, start_new_session=True)
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
    resolved_outputs = set(expected_mirrors(staged))
    # Masters authorize new mirror paths. Require mirrors already tracked in
    # either tree only while their master survives; retired paths may disappear.
    allowed_outputs = outputs | resolved_outputs
    outputs = (resolved_outputs & (set(pinned) | set(staged))) | (
        outputs & {"scripts/exec-context-legacy.json"})
    has_pack = has_pack or any(p.startswith(TEMPLATES + "skills/") for p in staged)
    pack_outputs = expected_pack_outputs(set(staged), pack_outputs)
    contents = {}
    before = snapshot(contents)
    deadline = time.monotonic() + budget
    try:
        for name, _, _, argv in REGISTRY:
            if name == "exec-context" and "scripts/exec-context-legacy.json" not in outputs:
                continue
            if name == "templates" and not has_pack and allowed_outputs <= {"scripts/exec-context-legacy.json"}:
                continue
            # Stay inside the host's bound so a slow generator is rolled back here.
            run_generator(argv, max(0.01, deadline - time.monotonic()))
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
        # Ratchet regeneration may reduce allowances, never enlarge main's ceiling.
        if "scripts/exec-context-legacy.json" in outputs:
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


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2], json.loads(sys.argv[3]) if len(sys.argv) > 3 else (),
         float(sys.argv[4]) if len(sys.argv) > 4 else 100.0)
