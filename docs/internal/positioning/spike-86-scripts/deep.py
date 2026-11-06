"""SPIKE-86: attribute test-file churn and dispatcher (main.rs / lib.rs) hunk churn
to architectural layers. Run with python3 from the repository root.
trace:SPIKE-86 | ai:claude

Tests: a test file's stem (spec-ID prefix such as `bug_775_` and `_tests`
suffix stripped) is looked up as a module at --rev; if no such module exists, a
keyword fallback picks the layer, else "unattributed".
Dispatcher: each -U0 hunk is attributed by the `fn` named in its hunk header.
"""
import argparse
import collections
import os
import re
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from classify import layer, numstat_commits  # noqa: E402

DISPATCHER = ["aida-cli/src/main.rs", "aida-cli-lib/src/lib.rs"]


def files_at(rev):
    out = subprocess.run(["git", "ls-tree", "-r", "--name-only", rev],
                         capture_output=True, text=True, check=True).stdout
    return set(out.split())


def test_layer(path, tree):
    stem = os.path.basename(path).rsplit(".", 1)[0]
    stem = re.sub(r"^(test_)?((bug|story|task|epic|spike|cr|adr)_?\d+_?)+", "", stem.lower())
    stem = re.sub(r"(_tests?|_it)$", "", stem)
    guess = None
    for base in ("aida-cli-lib/src/", "aida-core/src/"):
        if stem and (base + stem + ".rs") in tree:
            guess = layer(base + stem + ".rs")
            break
    if guess in (None, "other", "tests_dir"):
        k = stem
        guess = ("control" if re.search(r"queue|drain|orchestr|review|verdict|merge|lease|gate|auto_bump|autocomplete|auto_complete|integrat|pr_|ship|advis|seat|phase|sched|mailbox|presence|human|decide|rework|ps$|ps_|completion_seam|commits_ahead|protocol|punt|claim|stale", k) else
                 "execution" if re.search(r"worktree|session|agent|headless|forge|vendor|sandbox|launch|gitlab|mirror", k) else
                 "surface" if re.search(r"mcp|tui|status|cli|help|glyph|output|json|toon|render|format|presentation|journey|exposition|wiki", k) else
                 "intent" if re.search(r"criteria|trace|reconstitut|intent|contradict|interview|harvest|digest|carve|prose|classif", k) else
                 "scaffold" if re.search(r"init|scaffold|template|skill|upgrade", k) else
                 "store" if re.search(r"store|cache|db|graph|rel|edge|manifest|id_|import|export|yaml|lifecycle|history|search|edit|comment|archive", k) else
                 "unattributed")
    return guess


def fn_layer(f):
    k = f.lower()
    if re.search(r"queue|drain|orchestr|review|verdict|merge|lease|gate|auto_?complete|integrat|pr_|ship|advis|seat|phase|sched|mailbox|presence|human|decide|punt|requeue|lock|autopilot|supervis|event|watch|health|doctor|metric|usage|claim|release|stranded|reap", k):
        return "control"
    if re.search(r"worktree|session|agent|headless|forge|vendor|sandbox|spawn|launch|process", k):
        return "execution"
    if re.search(r"criteria|trace|reconstitut|intent|contradict|interview|harvest|digest|evaluat|clarif|capture|research|plan", k):
        return "intent"
    if re.search(r"mcp|tui|status|print|render|display|help|glyph|json|toon|format|output|cli|completion|statusline|main_entry|dispatch|run_command|main", k):
        return "surface"
    if re.search(r"init|scaffold|template|skill|upgrade|rules", k):
        return "scaffold"
    if re.search(r"store|cache|db|graph|relat|edit|add|show|list|search|comment|history|import|export|archive|git_backend|config|load|id|spec|type|feature", k):
        return "store"
    return "unattributed"


def attribute(rev, since, cap):
    kept, skipped = numstat_commits(rev, since, cap)
    skip = {sha for sha, _ in skipped}
    tree = files_at(rev)
    tests = collections.Counter()
    for _, rows in kept:
        for a, d, p in rows:
            if a == "-" or layer(p) != "tests_dir":
                continue
            tests[test_layer(p, tree)] += int(a) + int(d)
    patch = subprocess.run(["git", "log", rev, "--since=" + since, "--no-merges", "-U0",
                            "--format=@@C %H", "--"] + DISPATCHER,
                           capture_output=True, text=True, check=True).stdout
    fnl = collections.Counter()
    cur, fn = None, ""
    for ln in patch.splitlines():
        if ln.startswith("@@C "):
            cur = ln[4:].strip()
            continue
        if cur in skip:
            continue
        if ln.startswith("@@ "):
            m = re.search(r"@@[^@]*@@\s*(.*)", ln)
            ctx = m.group(1) if m else ""
            mm = re.search(r"fn\s+(\w+)", ctx)
            fn = mm.group(1) if mm else ("<mod>" if ctx else "<top>")
            continue
        if (ln.startswith("+") and not ln.startswith("+++")) or (ln.startswith("-") and not ln.startswith("---")):
            fnl[fn] += 1
    disp = collections.Counter()
    for f, v in fnl.items():
        disp[fn_layer(f)] += v
    return tests, disp, fnl


if __name__ == "__main__":
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--rev", default="9154924fd2")
    ap.add_argument("--since", required=True)
    ap.add_argument("--cap", type=int, default=5000)
    a = ap.parse_args()
    tests, disp, fnl = attribute(a.rev, a.since, a.cap)
    print(f"[{a.since}] test lines by layer:", dict(tests.most_common()))
    print(f"[{a.since}] dispatcher lines by enclosing-fn layer:", dict(disp.most_common()))
    print(f"[{a.since}] top dispatcher fns:", fnl.most_common(10))
