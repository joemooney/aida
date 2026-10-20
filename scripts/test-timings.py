#!/usr/bin/env python3
"""Per-binary test timings from a `cargo test` log, and the slow-test budget gate.

CI's `Run tests` step tees `cargo test --workspace` into a log. This script
reads it: every test binary prints `Running <src> (target/<profile>/deps/<stem>-<hash>)`
(or `Doc-tests <crate>`) and then libtest's `test result: ... finished in <s>s`.
That gives one row per binary: its key, tests passed/failed/ignored, and wall
seconds. Stable libtest has no per-test timings; per-test rows arrive with
cargo-nextest (STORY-1853 S2).

Binary keys are `<stem>:<src>` (`aida_cli_lib:src/lib.rs`,
`agent_error_channel:tests/agent_error_channel.rs`) or `doctests:<crate>`.

The budget file (scripts/test-time-budget.toml) maps a key to its allowed
seconds on CI's ubuntu-latest runner. With `--budget` the script fails when:
- a binary over `default_limit` seconds has no entry (a new slow binary must be
  budgeted, or made faster, in the PR that slows it down);
- a budgeted binary runs over its entry.
An entry whose binary did not run (a filtered or partial run) is a note.
`--lane NAME` overlays the file's `[lane_budget.NAME]` entries, for a binary
that runs split across lanes (the aida-cli-lib halves, lib-0 and lib-1).
trace:TASK-1979 | ai:claude

Usage:
  scripts/test-timings.py LOG [--step NAME] [--budget FILE [--lane NAME]] [--json OUT] [--summary OUT] [--top N]

`LOG` may be a raw `cargo test` log or a `gh run view --log` download (the
job/step/timestamp prefixes and ANSI colors are stripped; pass `--step 'Run
tests'` so other steps' cargo runs are not counted). `--summary` appends
a Markdown table (CI passes $GITHUB_STEP_SUMMARY). Exit 0 within budget, 1 a
budget violation, 2 an unreadable log or budget, or a log with no test results.

trace:STORY-1853 | ai:claude
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import tomllib
from dataclasses import asdict, dataclass

ANSI_RE = re.compile(r"\x1b\[[0-9;]*[A-Za-z]")
# `gh run view --log`: "<job>\t<step>\t<timestamp> <line>"; a raw step log: "<timestamp> <line>".
PREFIX_RE = re.compile(r"^(?:[^\t]*\t[^\t]*\t)?(?:\d{4}-\d\d-\d\dT[\d:.]+Z )?")
RUNNING_RE = re.compile(r"^\s*Running (\S+) \((?:.*/)?deps/(.+?)-[0-9a-f]+(?:\.exe)?\)\s*$")
UNITTESTS_RE = re.compile(r"^\s*Running unittests (\S+) \((?:.*/)?deps/(.+?)-[0-9a-f]+(?:\.exe)?\)\s*$")
DOCTESTS_RE = re.compile(r"^\s*Doc-tests (\S+)\s*$")
RUNNING_N_RE = re.compile(r"^running (\d+) tests?$")
RESULT_RE = re.compile(
    r"^test result: (?P<status>\w+)\. (?P<passed>\d+) passed; (?P<failed>\d+) failed; "
    r"(?P<ignored>\d+) ignored; (?P<measured>\d+) measured; \d+ filtered out; finished in (?P<secs>[\d.]+)s"
)


@dataclass
class Binary:
    key: str
    passed: int
    failed: int
    ignored: int
    seconds: float


def clean(line: str) -> str:
    return PREFIX_RE.sub("", ANSI_RE.sub("", line.rstrip("\n")), count=1)


def step_lines(lines, step: str):
    """Only the lines of one step of a `gh run view --log` download."""
    for line in lines:
        parts = line.split("\t", 2)
        if len(parts) == 3 and parts[1] == step:
            yield line


def parse_log(lines) -> list[Binary]:
    """One row per test binary, in run order.

    Two log shapes break a naive "the result after the header" reading:
    - CI merges cargo's stderr (the `Running` header) with the test binary's
      stdout, so the next binary's header can land before the previous
      binary's result (run 39186737460 credited aida_core's 1,552 unit tests
      to context_budget that way);
    - tests that re-exec their own binary as a child print a child `test
      result:` (one test run, the rest filtered out) inside the parent's output.
    So each `running N tests` line goes to the oldest header still without
    one (stdout order is preserved), and a result goes to the newest binary
    whose N equals the tests the result ran and that either has no result yet
    or is the one currently running (the newest with an N). Only the running
    binary's result may be replaced, since its own result comes after its
    children's; a child's 1-test result never overwrites an earlier, finished
    1-test binary.
    """
    rows: list[dict] = []
    for raw in lines:
        line = clean(raw).strip()
        m = UNITTESTS_RE.match(line) or RUNNING_RE.match(line)
        key = f"{m.group(2)}:{m.group(1)}" if m else None
        if key is None and (d := DOCTESTS_RE.match(line)):
            key = f"doctests:{d.group(1)}"
        if key is not None:
            rows.append({"key": key, "n": None, "result": None})
            continue
        if (n := RUNNING_N_RE.match(line)):
            row = next((r for r in rows if r["n"] is None), None)
            if row is not None:
                row["n"] = int(n.group(1))
            continue
        if (res := RESULT_RE.match(line)):
            ran = sum(int(res[k]) for k in ("passed", "failed", "ignored", "measured"))
            current = next((r for r in reversed(rows) if r["n"] is not None), None)
            row = next((r for r in reversed(rows)
                        if r["n"] == ran and (r["result"] is None or r is current)), None)
            if row is not None:
                row["result"] = res
    return [
        Binary(r["key"], int(res["passed"]), int(res["failed"]), int(res["ignored"]), float(res["secs"]))
        for r in rows if (res := r["result"]) is not None
    ]


@dataclass
class Budget:
    default_limit: float
    entries: dict[str, float]


def load_budget(path: str, lane: str | None = None) -> Budget:
    with open(path, "rb") as fh:
        table = tomllib.load(fh)
    limit = float(table.get("default_limit", 10))
    entries = {str(k): float(v) for k, v in table.get("budget", {}).items()}
    if lane:
        entries.update({str(k): float(v) for k, v in table.get("lane_budget", {}).get(lane, {}).items()})
    return Budget(limit, entries)


def check(rows: list[Binary], budget: Budget) -> tuple[list[str], list[str]]:
    """(violations, notes) for the rows against the budget."""
    violations: list[str] = []
    notes: list[str] = []
    seen = set()
    for r in rows:
        seen.add(r.key)
        allowed = budget.entries.get(r.key)
        if allowed is None:
            if r.seconds > budget.default_limit:
                violations.append(
                    f"{r.key}: {r.seconds:.1f}s with no budget entry (limit {budget.default_limit:g}s "
                    "for an unbudgeted binary). Make it faster, or add it to scripts/test-time-budget.toml "
                    "with the measured time plus headroom and say why in the PR."
                )
        elif r.seconds > allowed:
            violations.append(
                f"{r.key}: {r.seconds:.1f}s is over its budget of {allowed:g}s. "
                "Find what slowed it down; raise the entry only with a reason in the PR."
            )
    for key in sorted(set(budget.entries) - seen):
        notes.append(f"{key}: budgeted but did not run in this log")
    return violations, notes


def summary_markdown(rows: list[Binary], budget: Budget | None, top: int) -> str:
    total = sum(r.seconds for r in rows)
    counted = sum(r.passed + r.ignored for r in rows)
    lines = [
        "### Test binary timings",
        "",
        f"{len(rows)} binaries, {total:.0f}s summed (serial), {counted} tests passed+ignored.",
        "",
        "| Binary | Seconds | Tests | Budget |",
        "|---|---:|---:|---:|",
    ]
    for r in sorted(rows, key=lambda r: -r.seconds)[:top]:
        b = budget.entries.get(r.key) if budget else None
        lines.append(f"| `{r.key}` | {r.seconds:.1f} | {r.passed + r.ignored} | {'' if b is None else f'{b:g}'} |")
    return "\n".join(lines) + "\n"


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description="Per-binary cargo test timings and the slow-test budget gate.")
    ap.add_argument("log")
    ap.add_argument("--step", help="with a `gh run view --log` download: read only this step (e.g. 'Run tests')")
    ap.add_argument("--budget", help="budget TOML; fail on a violation")
    ap.add_argument("--lane", help="CI lane: apply its [lane_budget.LANE] entries over [budget]")
    ap.add_argument("--json", help="write the rows as JSON here")
    ap.add_argument("--summary", help="append a Markdown table here")
    ap.add_argument("--top", type=int, default=25, help="rows in the printed and Markdown tables")
    a = ap.parse_args(argv)
    try:
        with open(a.log, encoding="utf-8", errors="replace") as fh:
            rows = parse_log(step_lines(fh, a.step) if a.step else fh)
        budget = load_budget(a.budget, a.lane) if a.budget else None
    except (OSError, tomllib.TOMLDecodeError, ValueError) as e:
        print(f"test-timings: {e}", file=sys.stderr)
        return 2
    if not rows:
        print(f"test-timings: no `test result:` lines in {a.log}; nothing was measured", file=sys.stderr)
        return 2
    total = sum(r.seconds for r in rows)
    print(f"test-timings: {len(rows)} binaries, {total:.1f}s summed, "
          f"{sum(r.passed + r.ignored for r in rows)} tests passed+ignored")
    for r in sorted(rows, key=lambda r: -r.seconds)[:a.top]:
        print(f"  {r.seconds:8.2f}s  {r.passed + r.ignored:6d} tests  {r.key}")
    if a.json:
        with open(a.json, "w", encoding="utf-8") as fh:
            json.dump({"total_seconds": total, "binaries": [asdict(r) for r in rows]}, fh, indent=1)
    if a.summary:
        with open(a.summary, "a", encoding="utf-8") as fh:
            fh.write(summary_markdown(rows, budget, a.top))
    if budget is None:
        return 0
    violations, notes = check(rows, budget)
    for n in notes:
        print(f"test-timings: note: {n}")
    for v in violations:
        print(f"test-timings: BUDGET: {v}", file=sys.stderr)
    return 1 if violations else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
