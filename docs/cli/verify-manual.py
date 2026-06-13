#!/usr/bin/env python3
"""CLI-manual drift-guard (STORY-597 core).

Two checks against the live binary:
  1. COMPLETENESS — every command in `aida help-all` has a `### `aida <cmd>`` entry
     in some docs/cli/*.md chapter (a top-level command may legitimately be covered
     under a parent header, so we match on the leading command token).
  2. FLAG ACCURACY — every `--flag` cited inside a command's section appears in that
     command's `--help` (catches invented/renamed flags — the recurring doc-drift bug).

Exit non-zero on a completeness omission (hard); print WARN for flag mismatches (soft,
since some `--flag` mentions are cross-references to other commands).

Usage: python3 docs/cli/verify-manual.py            # all chapters
       python3 docs/cli/verify-manual.py 04-git*.md # one chapter
"""
import glob
import os
import re
import subprocess
import sys

CLI_DIR = os.path.dirname(os.path.abspath(__file__))
ENTRY_RE = re.compile(r"^### `aida ([a-z][a-z0-9-]*)")  # ### `aida <cmd> ...`
FLAG_RE = re.compile(r"`(--[a-z][a-z0-9-]*)`")


def help_text(cmd):
    try:
        return subprocess.run(
            ["aida", cmd, "--help"], capture_output=True, text=True, timeout=20
        ).stdout
    except Exception:
        return ""


def help_all_commands():
    out = subprocess.run(["aida", "help-all"], capture_output=True, text=True).stdout
    cmds = []
    for line in out.splitlines():
        m = re.match(r"^  ([a-z][a-z0-9-]+)\s", line)
        if m:
            cmds.append(m.group(1))
    # meta commands the manual needn't document as their own entry
    skip = {"help", "help-all"}
    return sorted(set(cmds) - skip)


def parse_chapters(files):
    """Return {cmd: [flags cited in its section]} and the set of documented cmds."""
    documented = {}
    for f in files:
        with open(f) as fh:
            cur = None
            for line in fh:
                m = ENTRY_RE.match(line)
                if m:
                    cur = m.group(1)
                    documented.setdefault(cur, [])
                    continue
                # a non-command section (## ...) ends the current command's scope,
                # so cross-cutting prose isn't misattributed to the command above it
                if line.startswith("## ") or line.startswith("---"):
                    cur = None
                if cur:
                    for fm in FLAG_RE.finditer(line):
                        documented[cur].append(fm.group(1))
    return documented


def main():
    files = (
        [os.path.join(CLI_DIR, a) for a in sys.argv[1:]]
        if len(sys.argv) > 1
        else sorted(glob.glob(os.path.join(CLI_DIR, "[0-9][0-9]-*.md")))
    )
    documented = parse_chapters(files)
    all_cmds = help_all_commands()

    # 1. completeness (only meaningful when scanning the whole manual)
    hard_fail = False
    if len(sys.argv) == 1:
        missing = [c for c in all_cmds if c not in documented]
        if missing:
            hard_fail = True
            print(f"FAIL completeness — {len(missing)} command(s) with no manual entry:")
            for c in missing:
                print(f"    aida {c}")
        else:
            print(f"OK completeness — all {len(all_cmds)} help-all commands documented")

    # 1b. SPEC-ID leakage — the user-facing manual must carry no STORY-x/TASK-x
    # noise (same convention that keeps SPEC-IDs out of --help). Hard fail.
    specid = re.compile(r"\b(STORY|TASK|BUG|EPIC|SPIKE|ADR|FR|PRIN|VIS|CON|TERM|CR)-[0-9]+")
    leaks = []
    for f in files:
        for i, line in enumerate(open(f), 1):
            for m in specid.finditer(line):
                leaks.append(f"{os.path.basename(f)}:{i} {m.group(0)}")
    if leaks:
        hard_fail = True
        print(f"FAIL spec-id leak — {len(leaks)} SPEC-ID(s) in the user-facing manual (use <spec-id>/<epic-id> placeholders):")
        for l in leaks[:20]:
            print(f"    {l}")
    else:
        print("OK spec-ids — no SPEC-ID noise in the manual")

    # 2. flag accuracy (advisory)
    warns = 0
    for cmd, flags in sorted(documented.items()):
        if not flags:
            continue
        ht = help_text(cmd)
        if not ht:
            continue
        for fl in sorted(set(flags)):
            if fl not in ht:  # may be a legit cross-ref to another command's flag
                warns += 1
                print(f"WARN  `{fl}` cited under `aida {cmd}` not in its --help (cross-ref or drift?)")
    if warns == 0:
        print("OK flags — every cited --flag found in its command's --help")

    sys.exit(1 if hard_fail else 0)


if __name__ == "__main__":
    main()
