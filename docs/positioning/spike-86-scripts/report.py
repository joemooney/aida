"""SPIKE-86: regenerate the combined per-layer effort table (report section 2.1),
the standing LOC table (section 2.2), and the capture-coverage figures (2.5).
Run with python3 from the repository root:

    python3 docs/positioning/spike-86-scripts/report.py

Windows are absolute anchors equal to `--since=30.days` / `--since=90.days` as
evaluated at 2026-09-24 18:15 -0700 against main at 9154924fd2.
trace:SPIKE-86 | ai:claude
"""
import argparse
import os
import re
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from classify import churn, layer, numstat_commits  # noqa: E402
from deep import attribute  # noqa: E402

LAYERS = ["control", "surface", "store", "execution", "scaffold", "intent"]
TRAILER = re.compile(r"\((((SPEC|STORY|BUG|TASK|EPIC|SPIKE|CR|ADR|DOC|FR|PRIN|VIS|TERM)-[0-9-]+)[ ,]*)+\)\s*(\(#[0-9]+\))?$")


def git(*args):
    return subprocess.run(["git", *args], capture_output=True, text=True, check=True).stdout


def combined(rev, since, cap):
    c = churn(rev, since, cap)
    tests, disp, _ = attribute(rev, since, cap)
    rows = {l: (c["lines"][l], tests[l], disp[l]) for l in LAYERS}
    total = sum(sum(v) for v in rows.values())
    print(f"\n== since {since}: commits counted={c['kept']} excluded over cap={c['skipped']} attributed total={total}")
    for l, v in sorted(rows.items(), key=lambda kv: -sum(kv[1])):
        print(f"  {l:10} files={v[0]:7} tests={v[1]:6} dispatcher={v[2]:6} total={sum(v):7} {100 * sum(v) / total:5.1f}%")
    print(f"  commits touching layer: " + ", ".join(f"{l} {c['commits'][l]}" for l in LAYERS))
    print(f"  unattributed: tests={tests['unattributed']} dispatcher={disp['unattributed']}")
    surface_breakdown(rev, since, cap)


def surface_breakdown(rev, since, cap):
    kept, _ = numstat_commits(rev, since, cap)
    lines, commits = {}, {}
    for _, rows in kept:
        seen = set()
        for a, d, p in rows:
            if a == "-":
                continue
            g = ("aida-tui" if p.startswith("aida-tui/") else "aida-web-react" if p.startswith("aida-web-react/")
                 else "mcp" if re.search(r"aida-cli(-lib)?/src/mcp", p) else None)
            if g:
                lines[g] = lines.get(g, 0) + int(a) + int(d)
                seen.add(g)
        for g in seen:
            commits[g] = commits.get(g, 0) + 1
    print(f"  surface breakdown: " + ", ".join(f"{g} commits={commits[g]} lines={v}" for g, v in sorted(lines.items(), key=lambda kv: -kv[1])))


def loc(rev):
    roots = ["aida-core/src", "aida-cli-lib/src", "aida-cli/src", "aida-tui/src", "aida-server/src", "aida-web-react/src"]
    files = [f for f in git("ls-tree", "-r", "--name-only", rev, *roots).split() if f.endswith((".rs", ".ts", ".tsx"))]
    counts, nfiles = {}, {}
    for f in files:
        l = layer(f)
        nfiles[l] = nfiles.get(l, 0) + 1
        counts[l] = counts.get(l, 0) + git("show", f"{rev}:{f}").count("\n")
    total = sum(counts.values())
    print(f"\n== LOC at {rev}")
    for l, v in sorted(counts.items(), key=lambda kv: -kv[1]):
        print(f"  {l:12} files={nfiles[l]:4} loc={v:7} {100 * v / total:5.1f}%")


def coverage(rev, since):
    subjects = git("log", rev, "--since=" + since, "--no-merges", "--format=%s").splitlines()
    with_trailer = sum(1 for s in subjects if TRAILER.search(s))
    traces = git("grep", "-hoE", r"trace:[A-Z]+-[0-9][0-9-]*", rev, "--", "*.rs", "*.ts", "*.tsx", "*.py", "*.sh").count("\n")
    ac = git("grep", "-hoE", r"trace:[A-Z]+-[0-9-]+\.(ac|AC)[0-9a-f]+", rev).splitlines()
    specs = {re.sub(r"^.*trace:([A-Z]+-[0-9-]+)\..*$", r"\1", x) for x in ac}
    print(f"\n== capture coverage at {rev}")
    print(f"  commits since {since} with SPEC-ID trailer: {with_trailer}/{len(subjects)}")
    print(f"  trace comments: {traces}")
    print(f"  criteria-level traces: {len(ac)} across {len(specs)} specs")


if __name__ == "__main__":
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--rev", default="9154924fd2")
    ap.add_argument("--since30", default="2026-08-25 18:15 -0700")
    ap.add_argument("--since90", default="2026-06-26 18:15 -0700")
    ap.add_argument("--cap", type=int, default=5000)
    a = ap.parse_args()
    combined(a.rev, a.since30, a.cap)
    combined(a.rev, a.since90, a.cap)
    loc(a.rev)
    coverage(a.rev, a.since90)
