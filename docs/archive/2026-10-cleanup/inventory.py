#!/usr/bin/env python3
"""Generate or check the 2026-10 documentation cleanup inventory.

trace:TASK-1611 | ai:claude

The inventory lists every tracked Markdown/HTML file at HEAD, plus any other
file the cleanup moved, with its
classification, its disposition in this cleanup, its path before the cleanup,
and the date of its last commit before the audit base. The original paths
come from git: renames between the merged main commit and HEAD.

  python3 docs/archive/2026-10-cleanup/inventory.py            # rewrite inventory.tsv
  python3 docs/archive/2026-10-cleanup/inventory.py --check    # coverage check, no writes

--check fails if a file moved by this cleanup is missing from the inventory
or listed with the wrong destination, or if an inventory row's path is not
tracked. Docs added on main after the cleanup are not required to appear.
Read-only apart from writing inventory.tsv; run it from any directory in the
repository.
"""
import os
import subprocess
import sys

AUDIT_BASE = "738653fb3e6de71d0975047adb143c5f31a25d85"  # main when the audit began
MERGED_MAIN = "b46aec1e674baae5a329416d4119e67b59200a88"  # main merged into the cleanup branch
HERE = os.path.dirname(os.path.abspath(__file__))
TSV = os.path.join(HERE, "inventory.tsv")
HEADER = ["path", "class", "disposition", "path_before_cleanup", "last_commit_before_audit", "note"]
# Moved to the archive by the first pass, then returned because something
# still consumes them at their original path.
RESTORED = {
    "REVIEW.md": "generated review instructions read at the repository root (aida review assemble)",
    "docs/cli-format-json-audit.md": "include_str! in aida-cli/tests/bug_1502_format_json_audit.rs",
    "docs/agents/codex-mcp-roundtrip-verdict.md": "scaffolded by aida init from its template master",
    "docs/agents/claude-surfaces-codex-parity.md": "in-repo copy of a template master",
    "docs/user-guide.html": "opened by aida user-guide; written by helper/generate-docs.sh",
    "docs/user-guide-dark.html": "opened by aida user-guide --dark; written by helper/generate-docs.sh",
}


def git(*args):
    return subprocess.run(["git", *args], capture_output=True, text=True, check=True, cwd=HERE).stdout


def is_doc(p):
    return p.endswith((".md", ".html"))


def classify(p):
    rules = [
        ("docs/archive/2026-10-cleanup/", "cleanup-record"),
        ("docs/archive/", "historical-archive"),
        ("aida-core/templates/", "template-master"),
        (".claude/", "generated-mirror"),
        (".aida/discipline/", "generated-mirror"),
        ("docs/aida/05-decisions/", "normative-adr"),
        ("docs/plans/", "historical-plan"),
        ("docs/release-notes/", "historical-release-notes"),
        ("docs/research/", "historical-artifact"),
        ("docs/spikes/", "historical-artifact"),
        ("docs/testing/", "historical-artifact"),
        ("docs/cli/", "maintained-manual"),
        ("docs/agents/", "maintained-agent-doc"),
        ("docs/", "maintained-doc"),
    ]
    if p == "CHANGELOG.md":
        return "historical-release-notes"
    if p == "REVIEW.md":
        return "generated-live-instruction"
    if "/" not in p:
        return "live-entry-point"
    if p.startswith("docs/presentation/20"):
        return "historical-artifact"  # dated decks; the audience decks stay maintained
    for prefix, cls in rules:
        if p.startswith(prefix):
            return cls
    return "component-doc"


def renames():
    out = {}
    # The index, so the inventory can be generated before it is committed;
    # after the commit the index equals HEAD.
    for line in git("diff", "--cached", "-M", "--diff-filter=R", "--name-status", MERGED_MAIN).splitlines():
        _, old, new = line.split("\t")
        out[new] = old  # every moved file, whatever its type
    return out


def last_dates():
    dates, current = {}, None
    log = git("log", "--format=@%ad", "--date=short", "--name-only", AUDIT_BASE)
    for line in log.splitlines():
        if line.startswith("@"):
            current = line[1:]
        elif line and line not in dates:
            dates[line] = current
    return dates


def build():
    top = git("rev-parse", "--show-toplevel").strip()
    moved = renames()
    tracked = sorted(p for p in git("-C", top, "ls-files").splitlines() if is_doc(p) or p in moved)
    dates = last_dates()
    rows = []
    for p in tracked:
        before = moved.get(p, p)
        if p in moved:
            disposition = "renamed" if not p.startswith("docs/archive/") else "archived"
        elif p in RESTORED:
            disposition = "restored"
        elif p.startswith("docs/archive/2026-10-cleanup/"):
            disposition = "added"
        else:
            disposition = "kept"
        # "-" rather than an empty field, so rows never end in whitespace.
        rows.append([p, classify(p), disposition, before, dates.get(before) or "-", RESTORED.get(p, "-")])
    return rows, moved, set(tracked)


def main():
    rows, moved, tracked = build()
    if "--check" not in sys.argv:
        with open(TSV, "w", encoding="utf-8") as f:
            f.write("\t".join(HEADER) + "\n")
            for r in rows:
                f.write("\t".join(r) + "\n")
        print(f"wrote {len(rows)} rows ({len(moved)} moved)")
        return 0
    listed = {}
    with open(TSV, encoding="utf-8") as f:
        next(f)
        for line in f:
            r = line.rstrip("\n").split("\t")
            listed[r[0]] = r
    errors = []
    for new, old in sorted(moved.items()):
        r = listed.get(new)
        if r is None:
            errors.append(f"moved file missing from inventory: {old} -> {new}")
        elif r[3] != old:
            errors.append(f"wrong origin for {new}: inventory says {r[3]}, git says {old}")
    for p in listed:
        if p not in tracked:
            errors.append(f"inventory row is not a tracked file: {p}")
    for e in errors:
        print("FAIL", e)
    print(f"checked {len(moved)} moves against {len(listed)} inventory rows: {'FAIL' if errors else 'OK'}")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
