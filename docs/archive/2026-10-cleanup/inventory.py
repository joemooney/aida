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

--check requires one valid audience for every tracked document, rejects duplicate
or stale rows, and checks audience placement. Historical cleanup origin/date
columns are preserved when regenerating existing rows. --working-tree includes
pending additions and excludes pending deletions for native lanes.
Read-only apart from writing inventory.tsv; run it from any directory in the
repository.
"""
import os
import csv
import subprocess
import sys

AUDIT_BASE = "738653fb3e6de71d0975047adb143c5f31a25d85"  # main when the audit began
MERGED_MAIN = "b46aec1e674baae5a329416d4119e67b59200a88"  # main merged into the cleanup branch
HERE = os.path.dirname(os.path.abspath(__file__))
TSV = os.path.join(HERE, "inventory.tsv")
HEADER = ["path", "class", "disposition", "path_before_cleanup", "last_commit_before_audit", "note", "audience"]
# Moved to the archive by the first pass, then returned because something
# still consumes them at their original path.
RESTORED = {
    "REVIEW.md": "generated review instructions read at the repository root (aida review assemble)",
    "docs/internal/cli-format-json-audit.md": "include_str! in aida-cli/tests/bug_1502_format_json_audit.rs",
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
        ("docs/internal/release-notes/", "historical-release-notes"),
        ("docs/internal/research/", "historical-artifact"),
        ("docs/internal/spikes/", "historical-artifact"),
        ("docs/spikes/", "historical-artifact"),  # trace:SPIKE-714 | ai:codex
        ("docs/internal/testing/", "historical-artifact"),
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
    if p.startswith("docs/internal/presentation/20"):
        return "historical-artifact"  # dated decks; the audience decks stay maintained
    for prefix, cls in rules:
        if p.startswith(prefix):
            return cls
    return "component-doc"


# trace:STORY-1651 | ai:codex
AUDIENCES = {"adopter", "advanced", "internal", "archive", "generated"}
ADOPTER_PAGES = set("""README.md SUMMARY.md UNDERSTANDING_SKILLS.md WHY-AIDA.md
configuration-guide.md commit-trailer-convention.md environment-variables.md
extending-skills.md first-project.md getting-started.md git-workflow.md lifecycle.md
metrics-guide.md operator-setup.md product-capabilities.md product-integrations.md
product-workflow.md requirement-types.md review-process.md selectable-autonomy.md
trust-and-evidence.md troubleshooting.md user-guide.md using-aida-with-claude-code.md
what-is-aida.md what-is-aida.html""".split())


def audience(p):
    if p.startswith("docs/archive/"):
        return "archive"
    if p.startswith(("aida-core/templates/", ".claude/", ".agents/", ".codex/", ".aida/discipline/")) or p in {
        "docs/agents/codex-brief-pickup.md", "docs/agents/antigravity-brief-pickup.md",
        "REVIEW.md", "docs/user-guide.html", "docs/user-guide-dark.html", "docs/cli/12-glossary.md",
        "docs/agents/claude-surfaces-codex-parity.md", "docs/agents/codex-mcp-roundtrip-verdict.md",
    }:
        return "generated"
    # The two-host enrollment workflow is for operators, despite its agents path.
    if p.startswith("docs/advanced/") or p == "docs/agents/second-review-node.md":
        return "advanced"
    if p.startswith(("docs/internal/", "docs/plans/", "docs/spikes/", "docs/aida/")) or p == "docs/cli/11-dev.md":
        return "internal"
    if p.startswith(("docs/cli/", "docs/agents/", "docs/showcase/", "docs/images/", "docs/tui/")) or p.removeprefix("docs/") in ADOPTER_PAGES:
        return "adopter"
    if p.startswith("docs/"):
        return None  # A new top-level topic needs an explicit audience decision.
    return "internal"


def tracked_paths():
    top = git("rev-parse", "--show-toplevel").strip()
    args = ["-C", top, "ls-files", "-z"]
    if "--working-tree" in sys.argv:
        args += ["--cached", "--others", "--exclude-standard"]
    return sorted({p for p in git(*args).split("\0") if p and
                   ("--working-tree" not in sys.argv or os.path.lexists(os.path.join(top, p)))})


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
    tracked = sorted(p for p in tracked_paths() if is_doc(p) or p in moved)
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
        rows.append([p, classify(p), disposition, before, dates.get(before) or "-", RESTORED.get(p, "-"), audience(p) or "UNCLASSIFIED"])
    return rows, moved, set(tracked)


def main():
    if "--check" in sys.argv:
        return check()
    rows, moved, tracked = build()
    if os.path.exists(TSV):
        with open(TSV, encoding="utf-8") as existing:
            history = {r["path"]: r for r in csv.DictReader(existing, delimiter="\t")}
        for row in rows:
            old = history.get(row[0])
            if old:
                row[2:6] = [old[k] for k in HEADER[2:6]]
    with open(TSV, "w", encoding="utf-8") as f:
        f.write("\t".join(HEADER) + "\n")
        for r in rows:
            f.write("\t".join(r) + "\n")
    print(f"wrote {len(rows)} rows ({len(moved)} moved)")
    return 0


# trace:STORY-1651 | ai:codex
def check():
    tracked = {p for p in tracked_paths() if is_doc(p)}
    listed = {}
    errors = []
    with open(TSV, encoding="utf-8") as source:
        reader = csv.DictReader(source, delimiter="\t")
        if reader.fieldnames != HEADER:
            print("FAIL inventory header differs; regenerate inventory.tsv")
            return 1
        for row in reader:
            p = row["path"]
            if p in listed:
                errors.append(f"duplicate inventory row: {p}")
            listed[p] = row
            if row["audience"] not in AUDIENCES:
                errors.append(f"invalid or missing audience: {p}")
            elif row["audience"] != audience(p):
                errors.append(f"wrong audience for {p}: {row['audience']} (expected {audience(p)})")
    for p in sorted(tracked):
        if audience(p) is None:
            errors.append(f"tracked doc missing audience: {p}")
    all_tracked = set(tracked_paths())
    for p in sorted(listed.keys() - all_tracked):
        errors.append(f"inventory row is not a tracked file: {p}")
    for error in errors:
        print("FAIL", error)
    print(f"checked {len(tracked)} tracked docs, {len(listed)} inventory rows: {'FAIL' if errors else 'OK'}")
    return int(bool(errors))


if __name__ == "__main__":
    sys.exit(main())
