#!/usr/bin/env python3
"""Regression check for TASK-1447: docs-drift guards must schedule on docs-only PRs.

BUG-1578 found that gating a documentation-drift guard on
`steps.changes.outputs.full_ci == 'true'` silently skips it on a docs-only
PR -- exactly the PR shape most likely to introduce the drift the guard
exists to catch. TASK-1447 fixed the four guards still gated that way
(the External-prose classifier guard was already fixed by BUG-1578 itself).

This script is the guard against regression: for each known docs-drift
guard step in .github/workflows/ci.yml, it asserts one of:
  1. The step's `if:` condition does not reference `full_ci` at all (so it
     always schedules, on docs-only PRs included), or
  2. The step IS gated on `full_ci`, but carries a `DOCS-GUARD-EXEMPT:`
     marker in its preceding comment block explaining why it cannot run on
     a docs-only PR (e.g. it needs a build artifact docs-only CI never
     produces, and building one was judged not worth the cost).

Anything else -- a full_ci-gated step with no exemption marker, or a guard
that has silently disappeared/been renamed -- fails the check.

USAGE
-----
    python3 scripts/check-docs-guards-scheduled.py

Exit 0 = every known docs guard schedules on docs-only PRs, or is
         documented as unable to.
Exit 1 = a docs guard regressed to a silent full_ci gate, or is missing.

trace:TASK-1447 | ai:claude
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CI_YAML = ROOT / ".github" / "workflows" / "ci.yml"

# The docs-drift guards this check protects. Each is a step `name:` in the
# `build` job of ci.yml. Add a new entry here whenever a new guard is added
# that exists to catch documentation drift -- that is the mechanism that
# keeps this check from going stale the way the full_ci gates themselves did.
DOCS_GUARDS = [
    "External-prose classifier inventory",
    "Doc-intent gate (surface changes mark doc-impact)",
    "CLI-manual drift-guard (every command documented)",
    "Monitor contract drift guard",
    "Trace/doc-header stranding gate",
]

EXEMPT_MARKER = "DOCS-GUARD-EXEMPT"

NAME_RE = re.compile(r'^(\s*)-\s+name:\s*(.+?)\s*$')
STEP_KEY_RE = re.compile(r'^(\s*)(if|run|uses|shell|with|env)\s*:')
COMMENT_RE = re.compile(r'^\s*#')


def find_step_blocks(lines):
    """Return {step_name: (name_line_idx, indent)} for every top-level step."""
    blocks = {}
    for i, line in enumerate(lines):
        m = NAME_RE.match(line)
        if m:
            indent, name = m.groups()
            # Later duplicate names overwrite earlier ones; ci.yml step names
            # are unique in practice, and we want the block that actually
            # exists in the file today.
            blocks[name] = (i, len(indent))
    return blocks


def step_if_condition(lines, name_idx, indent):
    """Return the `if:` value for the step starting at name_idx, or None."""
    i = name_idx + 1
    while i < len(lines):
        line = lines[i]
        if line.strip() == "":
            i += 1
            continue
        stripped_indent = len(line) - len(line.lstrip(" "))
        if stripped_indent <= indent and line.strip().startswith("- "):
            break  # next step
        if stripped_indent <= indent and not line.strip().startswith("#"):
            break  # dedented out of this step entirely
        m = STEP_KEY_RE.match(line)
        if m and m.group(2) == "if":
            return line.split(":", 1)[1].strip()
        if m and m.group(2) in ("run", "uses"):
            # `if:` (when present) always precedes run/uses in this file's
            # step ordering convention; stop scanning once we hit either.
            return None
        i += 1
    return None


def preceding_comment_block(lines, name_idx):
    """Return the contiguous comment lines directly above the step, joined."""
    i = name_idx - 1
    collected = []
    while i >= 0 and (COMMENT_RE.match(lines[i]) or lines[i].strip() == ""):
        if lines[i].strip() != "":
            collected.append(lines[i])
        elif collected:
            # blank line inside the comment run is fine; a blank line before
            # any comment text means we've left the comment block
            pass
        i -= 1
        if lines[i].strip() != "" and not COMMENT_RE.match(lines[i]):
            break
    return "\n".join(reversed(collected))


def main():
    text = CI_YAML.read_text()
    lines = text.splitlines()
    blocks = find_step_blocks(lines)

    failures = []
    for guard in DOCS_GUARDS:
        if guard not in blocks:
            failures.append(f"guard step not found in {CI_YAML}: {guard!r}")
            continue
        name_idx, indent = blocks[guard]
        condition = step_if_condition(lines, name_idx, indent)
        if condition and "full_ci" in condition:
            comment = preceding_comment_block(lines, name_idx)
            if EXEMPT_MARKER not in comment:
                failures.append(
                    f"guard {guard!r} is gated on full_ci (`if: {condition}`) "
                    f"and skips docs-only PRs, with no {EXEMPT_MARKER} marker "
                    "documenting why it can't run there"
                )

    if failures:
        print("check-docs-guards-scheduled: FAIL", file=sys.stderr)
        for f in failures:
            print(f"  - {f}", file=sys.stderr)
        return 1

    print(f"check-docs-guards-scheduled: OK ({len(DOCS_GUARDS)} guards schedule on docs-only PRs)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
