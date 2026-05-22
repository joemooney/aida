#!/usr/bin/env python3
"""Collapse repeated `gh run watch` CI-status blocks in captured scrollback files.

Background: AIDA's `--auto-complete` phase 2 streams `gh run watch` for CI
progress. When stdout is teed to a file (the common `... | tee drain.log`
pattern), the redraw escape codes don't fire — instead, the same status block
re-prints every ~3 seconds for the full CI duration. A captured drain log ends
up with hundreds of nearly-identical blocks polluting the real signal.

This script reads a scrollback or drain log, identifies runs of consecutive
`gh run watch`-shaped blocks, and collapses each run to a single block plus
a one-line summary of how many redraws were elided. Non-CI content is
preserved verbatim.

A CI-watch block is delimited:
  - starts at a line equal to `JOBS`
  - ends at the next line starting with `Triggered via` (inclusive)
  - typically also includes one blank line in the middle (between the
    `Refreshing run status` line and the `* <branch> CI ...` line)
  - is followed by a trailing blank line before the next block

Composes with BUG-273 (the orchestrator-side fix) — same noise source, this
is the read-side cleanup; BUG-273 is the write-side prevention.

Usage:
    scripts/dedup-scrollback.py <input> [output]

If <output> is omitted, writes to <input>.dedup
"""
import re
import sys
from pathlib import Path


# Variable parts inside a CI-watch block that change between near-duplicate
# refreshes — strip these for comparison so we count "5 minutes ago" and
# "6 minutes ago" snapshots as the same logical block.
_VARIABLE_TIME = re.compile(
    r"about \d+ (?:second|minute|hour|day)s? ago"
    r"|less than a minute ago"
    r"|\d+m\s*\d+s"
    r"|\bID \d+\b"
    r"|·\s*\d+\b"  # run number after the ·
)


def normalize(block_lines: list[str]) -> str:
    """Strip variable text so near-duplicate refreshes compare equal."""
    return _VARIABLE_TIME.sub("⟨t⟩", "\n".join(block_lines))


def dedup(text: str) -> tuple[str, dict]:
    """Return (cleaned_text, stats_dict).

    State machine over input lines:
      OUT: not currently inside a JOBS block. Emit line.
      IN:  inside a JOBS block (between `JOBS` and the next `Triggered via`).
           Buffer the block lines; on `Triggered via`, decide whether to
           emit-and-reset or skip (duplicate run).
    """
    lines = text.splitlines(keepends=False)
    out: list[str] = []
    stats = {
        "in_lines": len(lines),
        "in_bytes": len(text),
        "collapsed_runs": 0,
        "blocks_elided": 0,
    }

    i = 0
    last_emitted_norm: str | None = None
    run_count = 0

    while i < len(lines):
        line = lines[i]
        if line == "JOBS":
            # Begin a JOBS block — scan to the next `Triggered via` line.
            block: list[str] = [line]
            j = i + 1
            while j < len(lines):
                block.append(lines[j])
                if lines[j].startswith("Triggered via "):
                    break
                j += 1
            # j is now at the `Triggered via` line (or end-of-file)
            norm = normalize(block)

            if norm == last_emitted_norm:
                # Duplicate run — skip this block AND its trailing blank
                run_count += 1
                stats["blocks_elided"] += 1
                # Skip the block + the trailing blank line (if any)
                i = j + 1
                if i < len(lines) and lines[i] == "":
                    i += 1
                continue

            # New block (or first of a run): emit any pending run-summary
            # for the previous run, then emit this block.
            if run_count > 0:
                out.append(
                    f"    ⟪ CI-watch block above re-printed "
                    f"{run_count + 1} times — {run_count} redraws elided ⟫"
                )
                stats["collapsed_runs"] += 1
            out.extend(block)
            last_emitted_norm = norm
            run_count = 0
            i = j + 1
            continue

        # Non-block line: close any open run-summary, emit the line.
        if run_count > 0:
            out.append(
                f"    ⟪ CI-watch block above re-printed "
                f"{run_count + 1} times — {run_count} redraws elided ⟫"
            )
            stats["collapsed_runs"] += 1
            run_count = 0
            last_emitted_norm = None
        out.append(line)
        i += 1

    # End-of-file: close any open run-summary
    if run_count > 0:
        out.append(
            f"    ⟪ CI-watch block above re-printed "
            f"{run_count + 1} times — {run_count} redraws elided ⟫"
        )
        stats["collapsed_runs"] += 1

    cleaned = "\n".join(out) + ("\n" if text.endswith("\n") else "")
    stats["out_lines"] = len(out)
    stats["out_bytes"] = len(cleaned)
    return cleaned, stats


def main() -> int:
    if len(sys.argv) < 2 or sys.argv[1] in ("-h", "--help"):
        print(__doc__)
        return 0 if len(sys.argv) > 1 else 1

    in_path = Path(sys.argv[1])
    out_path = Path(sys.argv[2]) if len(sys.argv) > 2 else in_path.with_suffix(
        in_path.suffix + ".dedup"
    )

    if not in_path.exists():
        print(f"error: input file not found: {in_path}", file=sys.stderr)
        return 2

    text = in_path.read_text()
    cleaned, stats = dedup(text)
    out_path.write_text(cleaned)

    pct_bytes = (1 - stats["out_bytes"] / max(stats["in_bytes"], 1)) * 100
    pct_lines = (1 - stats["out_lines"] / max(stats["in_lines"], 1)) * 100
    print(f"input:  {in_path}")
    print(f"output: {out_path}")
    print(
        f"  lines: {stats['in_lines']:>7,} → {stats['out_lines']:>7,}"
        f"  ({pct_lines:>5.1f}% reduction)"
    )
    print(
        f"  bytes: {stats['in_bytes']:>7,} → {stats['out_bytes']:>7,}"
        f"  ({pct_bytes:>5.1f}% reduction)"
    )
    print(f"  collapsed runs: {stats['collapsed_runs']:>3,}")
    print(f"  blocks elided:  {stats['blocks_elided']:>3,}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
