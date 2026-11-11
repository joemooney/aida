#!/usr/bin/env python3
"""Host-compiled repair regeneration program, executed only in the seat sandbox.

This file is embedded in the host binary; the seat's copy is never executed.
The closed table mirrors make sync-templates, including its symlinks, portable
pack, discipline copies and hooks. It deliberately excludes local extensions.
trace:STORY-2710 | ai:codex
"""
import hashlib
import os
from pathlib import Path
import subprocess
import sys

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


def git(*args):
    return subprocess.check_output(("git", *args))


def registered(path, mode, masters=None):
    if path == "scripts/exec-context-legacy.json":
        return True
    # Only tracked mirror files with a corresponding master, or portable pack
    # files whose authoritative renderer is covered by the template dependency.
    if path.startswith(".agents/skills/aida-"):
        return True
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


def snapshot():
    result = {}
    for root, dirs, files in os.walk(".", followlinks=False):
        dirs[:] = [d for d in dirs if not (root == "." and d in (".git", "target"))]
        for name in files + [d for d in dirs if Path(root, d).is_symlink()]:
            p = Path(root, name)
            key = p.as_posix()
            if p.is_symlink():
                result[key] = ("link", os.readlink(p))
            elif p.is_file():
                result[key] = (p.stat().st_mode & 0o777, hashlib.sha256(p.read_bytes()).hexdigest())
            else:
                result[key] = ("special", p.lstat().st_mode, p.lstat().st_rdev)
        dirs[:] = [d for d in dirs if not Path(root, d).is_symlink()]
    return result


def main(base, phase):
    pinned = {}
    for row in git("ls-tree", "-rz", base).split(b"\0"):
        if not row:
            continue
        meta, raw = row.split(b"\t", 1)
        path = os.fsdecode(raw)
        pinned[path] = meta.split()[0].decode()
    outputs = {path for path, mode in pinned.items() if registered(path, mode, pinned)}
    if not outputs:
        return
    unmerged = {os.fsdecode(p) for p in git("diff", "--name-only", "--diff-filter=U", "-z").split(b"\0") if p}
    # Both generators inspect Rust sources. Do not run against partial sources.
    blockers = sorted(p for p in unmerged - outputs if p.endswith((".rs", ".py")) or p.startswith("aida-core/templates/") or p.endswith("Cargo.toml") or p == "Makefile")
    if blockers:
        raise RuntimeError("regeneration requires resolved authoritative sources: " + repr(blockers))
    before = snapshot()
    if phase == "restore":
        subprocess.run(("git", "--literal-pathspecs", "restore", "--source", base,
                        "--staged", "--worktree", "--", *sorted(outputs)), check=True)
        return
    for name, _, _, argv in REGISTRY:
        if name == "exec-context" and "scripts/exec-context-legacy.json" not in outputs:
            continue
        if name == "templates" and outputs == {"scripts/exec-context-legacy.json"}:
            continue
        subprocess.run(argv, check=True)
    after = snapshot()
    unexpected = sorted(p for p in before.keys() | after.keys() if before.get(p) != after.get(p) and p not in outputs)
    if unexpected:
        raise RuntimeError("generator wrote unregistered outputs: " + repr(unexpected))
    for path in outputs:
        if path not in after:
            raise RuntimeError("generator removed declared output: " + repr(path))
    # Ratchet regeneration may reduce allowances, never enlarge main's ceiling.
    if "scripts/exec-context-legacy.json" in outputs:
        subprocess.run(("python3", "scripts/check-exec-context.py", "--base", base), check=True)


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
