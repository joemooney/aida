#!/usr/bin/env python3
"""Build the TASK-1800 CI replay fixture (TASK-2068).

The 18 audited cases of docs/testing/task-1800-semantic-audit.json pin
squash-merged PR heads that CI's clone does not have. This script writes a
git bundle holding, per case, a synthetic BASE and HEAD root commit whose
trees carry only the paths the real base..head diff changes (with the real
blobs and modes). `git diff-tree base head` on the synthetic pair therefore
yields the same paths and blobs as on the real pair, so the production
collector reproduces the recorded scoped digests and fallback sets.

Commit metadata is fixed, so every commit ID is a pure function of the real
diff: `--check` rebuilds the bundle from the pinned heads and compares its
ref -> commit-ID listing with the committed one. It needs the pinned heads
in this clone (local only, like `make task-1800-replay`).

Usage:
  scripts/task-1800-replay-fixture.py           # (re)write the bundle
  scripts/task-1800-replay-fixture.py --check   # prove the committed bundle

trace:TASK-2067 | ai:claude
"""

import json
import os
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
AUDIT = os.path.join(ROOT, "docs/testing/task-1800-semantic-audit.json")
BUNDLE = os.path.join(ROOT, "tests/fixtures/task-1800-replay/audited-cases.bundle")
STAMP = "1700000000 +0000"


def git(*args, cwd=ROOT, env=None, stdin=None):
    """Run git with no user/system config, returning stripped stdout."""
    full = dict(os.environ)
    full.update({"GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull})
    full.update(env or {})
    out = subprocess.run(
        ["git", "-c", "core.quotePath=false", *args],
        cwd=cwd,
        env=full,
        input=stdin,
        capture_output=True,
        text=True,
    )
    if out.returncode != 0:
        sys.exit(f"git {' '.join(args)}: {out.stderr.strip()}")
    return out.stdout.strip()


def changed(base, head):
    """(path, base side, head side) for every path base..head changes; a
    side is (mode, blob) or None when the path is absent on that side."""
    raw = git(
        "diff-tree", "-r", "-z", "--raw", "--no-abbrev", "--no-renames",
        "--no-ext-diff", "--no-textconv", "--ignore-submodules=none",
        base, head, "--",
    )
    fields = raw.split("\0")
    out = []
    for header, path in zip(fields[0::2], fields[1::2]):
        if not header:
            continue
        old_mode, new_mode, old_id, new_id, _ = header[1:].split()
        side = lambda mode, oid: None if mode == "000000" else (mode, oid)
        out.append((path, side(old_mode, old_id), side(new_mode, new_id)))
    return out


def commit(repo, entries, message):
    """A root commit whose tree is exactly `entries` [(mode, blob, path)]."""
    index = os.path.join(repo, "index.tmp")
    if os.path.exists(index):
        os.remove(index)
    env = {"GIT_INDEX_FILE": index}
    info = "".join(f"{mode} {blob}\t{path}\n" for mode, blob, path in entries)
    if info:
        git("update-index", "--add", "--index-info", cwd=repo, env=env, stdin=info)
    tree = git("write-tree", cwd=repo, env=env)
    stamp = {
        "GIT_AUTHOR_NAME": "aida-fixture",
        "GIT_AUTHOR_EMAIL": "fixture@aida.invalid",
        "GIT_AUTHOR_DATE": STAMP,
        "GIT_COMMITTER_NAME": "aida-fixture",
        "GIT_COMMITTER_EMAIL": "fixture@aida.invalid",
        "GIT_COMMITTER_DATE": STAMP,
    }
    return git("commit-tree", tree, "-m", message, cwd=repo, env=stamp)


def build(path):
    """Write the bundle to `path`; return its sorted ref listing."""
    audit = json.load(open(AUDIT))
    common = git("rev-parse", "--path-format=absolute", "--git-common-dir")
    with tempfile.TemporaryDirectory() as repo:
        git("init", "-q", "--bare", cwd=repo)
        with open(os.path.join(repo, "objects/info/alternates"), "w") as f:
            f.write(os.path.join(common, "objects") + "\n")
        refs = []
        for row in audit["rows"]:
            pr, base, head = row["pr"], row["base"], row["head"]
            diff = changed(base, head)
            for name, pick in (("base", 1), ("head", 2)):
                entries = [(d[pick][0], d[pick][1], d[0]) for d in diff if d[pick]]
                sha = commit(repo, entries, f"TASK-1800 replay fixture: PR {pr} {name}")
                ref = f"refs/task-1800/{pr}/{name}"
                git("update-ref", ref, sha, cwd=repo)
                refs.append(ref)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        git("bundle", "create", "-q", path, *refs, cwd=repo)
    return heads(path)


def heads(path):
    return sorted(git("bundle", "list-heads", path).splitlines())


def main():
    if sys.argv[1:] == ["--check"]:
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "check.bundle")
            rebuilt = build(path)
            same_bytes = open(path, "rb").read() == open(BUNDLE, "rb").read()
        committed = heads(BUNDLE)
        if rebuilt != committed:
            sys.exit("the committed bundle differs from a rebuild:\n"
                     + "\n".join(sorted(set(rebuilt) ^ set(committed))))
        print(f"ok: {len(committed)} refs match a rebuild from the pinned heads"
              + ("; byte-identical" if same_bytes else
                 "; bytes differ (another git version packs differently)"))
    elif sys.argv[1:] == []:
        listing = build(BUNDLE)
        print(f"wrote {BUNDLE}: {len(listing)} refs, {os.path.getsize(BUNDLE)} bytes")
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
