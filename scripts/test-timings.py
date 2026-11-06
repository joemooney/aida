#!/usr/bin/env python3
"""Per-test timings from nextest JUnit reports, per-binary timings from a
`cargo test` log, and the slow-test budget gate.

CI's `Run tests` step runs each lane under cargo-nextest, which writes one
JUnit report per invocation (target/nextest/ci/junit-<lane>-<n>.xml), and tees
the step into a log. With `--junit`, every test in the reports is one row,
keyed `<binary id> <test name>` as nextest prints it
(`aida-cli::journey task_1417_relationship_collapse_declares_view_limit_at_threshold`,
`aida-cli-lib cli::tests::source_doc_comments_carry_no_spec_id_provenance`).
Doctests stay on `cargo test`, so their rows still come from the log.
trace:TASK-1988 | ai:claude

Without `--junit` the log is read as a `cargo test` log, one row per binary:
every test binary prints `Running <src> (target/<profile>/deps/<stem>-<hash>)`
(or `Doc-tests <crate>`) and then libtest's `test result: ... finished in <s>s`.
Binary keys are `<stem>:<src>` (`agent_error_channel:tests/agent_error_channel.rs`)
or `doctests:<crate>`. Local `cargo test` runs use this.

The budget file (scripts/test-time-budget.toml) maps a key to its allowed
seconds on CI's ubuntu-latest runner. With `--budget` the script fails when:
- a row over `default_limit` seconds has no entry (a new slow test must be
  budgeted, or made faster, in the PR that slows it down);
- a budgeted row runs over its entry.
An entry for a test whose binary ran but which did not run itself (renamed or
removed) is a note; entries for binaries that did not run (another lane's, or
the other aida-cli-lib half's) are not mentioned. Explicitly budgeted tests
at most twice their entry whose JUnit time spans overlap another eligible
over-budget test are a runner stall: a CI warning.

With `--changed-files FILE` (one repo-relative path per line: the PR's diff
against its base) the limits are hard only for tests the PR touched: a test
whose function is defined in a changed `.rs` file (an added, renamed or edited
test, or one in an edited module). A test the PR did not touch is held to its
limit times the budget's `untouched_factor` (default 1.5); between the limit
and that it is a CI warning asking for a budget entry, not a failure, because
runner load alone takes a near-limit test over (BUG-2414: 10.0 s and 11.0 s
on unrelated tests ejected train members). Doctest and per-binary rows are
always touched. Without `--changed-files` every row is touched (the old,
strict gate). trace:BUG-2414 | ai:claude

Usage:
  scripts/test-timings.py LOG [--junit XML...] [--step NAME] [--budget FILE]
      [--changed-files FILE] [--json OUT] [--summary OUT] [--top N]

`LOG` may be a raw step log or a `gh run view --log` download (the
job/step/timestamp prefixes and ANSI colors are stripped; pass `--step 'Run
tests'` so other steps' cargo runs are not counted). A `--junit` path that does
not exist (a run that failed before nextest wrote it) is skipped with a note.
`--summary` appends a Markdown table (CI passes $GITHUB_STEP_SUMMARY). Exit 0
within budget, 1 a budget violation, 2 an unreadable log, report or budget, or
nothing measured.

trace:STORY-1853 | ai:claude
"""

from __future__ import annotations

import argparse
import datetime
import json
import os
import re
import sys
import tomllib
import xml.etree.ElementTree as ET
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
    started: str = ""  # JUnit start timestamp (ISO 8601); empty for log rows


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



def parse_junit(path: str) -> list[Binary]:
    """One row per test case in a nextest JUnit report: `<binary id> <test name>`.

    nextest leaves #[ignore]d tests out of the report, so every row ran;
    a `failure` or `error` child (a failed, timed-out or crashed test) makes
    it a failed row.
    """
    rows = []
    for suite in ET.parse(path).getroot().iter("testsuite"):
        for case in suite.iter("testcase"):
            failed = case.find("failure") is not None or case.find("error") is not None
            rows.append(Binary(f"{suite.get('name')} {case.get('name')}", int(not failed), int(failed), 0,
                               float(case.get("time") or 0), case.get("timestamp") or ""))
    return rows


def binary_of(key: str) -> str:
    """The binary a row key belongs to: a per-test key's binary id, else the key."""
    return key.split(" ", 1)[0]


@dataclass
class Budget:
    default_limit: float
    entries: dict[str, float]
    # Tolerance for a test the PR did not touch (BUG-2414). trace:BUG-2414 | ai:claude
    untouched_factor: float = 1.5


def load_budget(path: str) -> Budget:
    with open(path, "rb") as fh:
        table = tomllib.load(fh)
    limit = float(table.get("default_limit", 10))
    entries = {str(k): float(v) for k, v in table.get("budget", {}).items()}
    factor = float(table.get("untouched_factor", 1.5))
    if factor < 1:
        raise ValueError(f"{path}: untouched_factor {factor:g} is below 1")
    return Budget(limit, entries, factor)


def test_fn(key: str) -> str | None:
    """The test function name of a per-test (JUnit) key, else None.

    `aida-cli::journey slow_one` -> `slow_one`; `aida-cli-lib cli::tests::fast`
    -> `fast`. Doctest and per-binary log keys have no space and name no test.
    """
    if " " not in key or key.startswith("doctests:"):
        return None
    return key.split(" ", 1)[1].rsplit("::", 1)[-1]


def touched_tests(rows: list[Binary], changed: list[str], root: str = ".") -> set[str]:
    """Keys of the rows the PR touched, given its changed files.

    A per-test row is touched when its function is defined (`fn <name>`) in a
    changed `.rs` file that still exists; doctest and per-binary rows, which
    name no single test, always are. Being touched is the strict side, so a
    test name shared by two files only ever makes the gate stricter.
    trace:BUG-2414 | ai:claude
    """
    sources = []
    for rel in changed:
        if rel.endswith(".rs"):
            try:
                with open(os.path.join(root, rel), encoding="utf-8", errors="replace") as fh:
                    sources.append(fh.read())
            except OSError:  # deleted or renamed away by the PR
                pass
    defined: set[str] = set()
    for text in sources:
        defined.update(re.findall(r"\bfn\s+([A-Za-z_][A-Za-z0-9_]*)", text))
    return {r.key for r in rows if (name := test_fn(r.key)) is None or name in defined}


def span(row: Binary):
    """(start, end) of a JUnit row, or None without a valid zoned timestamp."""
    try:
        start = datetime.datetime.fromisoformat(row.started)
    except (TypeError, ValueError):
        return None
    # A naive timestamp cannot establish overlap with nextest's zoned times.
    # trace:TASK-2020 | ai:codex
    if start.tzinfo is None:
        return None
    return start, start + datetime.timedelta(seconds=row.seconds)


def stalled(over: list[Binary], budget: Budget) -> set[str]:
    """Keys of eligible overruns that overlapped another eligible overrun.

    A runner stall (shared-runner I/O or CPU starvation) slows every test
    running through it at once: on PR-2785 run 39492854022 three unrelated
    lib tests that take 1-2 s went to 18 s together. A regression slows one
    test whenever it runs, so an over-budget test that overlaps no other
    over-budget test still fails the gate. trace:TASK-1988 | ai:claude
    """
    # Only bounded, explicitly budgeted overruns supply stall evidence.
    # Unbudgeted or extreme regressions must not waive themselves or peers.
    # trace:TASK-2020 | ai:codex
    over = [r for r in over if r.key in budget.entries
            and r.seconds <= 2 * budget.entries[r.key]]
    spans = {r.key: span(r) for r in over}
    keys = set()
    for a in over:
        for b in over:
            sa, sb = spans[a.key], spans[b.key]
            if a is not b and sa and sb and sa[0] < sb[1] and sb[0] < sa[1]:
                keys.add(a.key)
    return keys


def check(rows: list[Binary], budget: Budget,
          touched: set[str] | None = None) -> tuple[list[str], list[str]]:
    """(violations, notes) for the rows against the budget.

    `touched` is the keys the PR touched (None: every row). An untouched row
    over its limit but within limit x untouched_factor is a note, not a
    violation. trace:BUG-2414 | ai:claude
    """
    # Eligible budgeted rows, including tolerated untouched ones, can supply
    # overlap evidence before each row is classified. trace:TASK-2020 | ai:codex
    found: list[tuple[Binary, float, str]] = []
    notes: list[str] = []
    seen = set()
    for r in rows:
        seen.add(r.key)
        allowed = budget.entries.get(r.key)
        limit = budget.default_limit if allowed is None else allowed
        if r.seconds <= limit:
            continue
        if allowed is None:
            message = (f"{r.key}: {r.seconds:.1f}s with no budget entry (limit {budget.default_limit:g}s "
                       "for an unbudgeted test). Make it faster, or add it to scripts/test-time-budget.toml "
                       "with the measured time plus headroom and say why in the PR.")
        else:
            message = (f"{r.key}: {r.seconds:.1f}s is over its budget of {allowed:g}s. "
                       "Find what slowed it down; raise the entry only with a reason in the PR.")
        found.append((r, limit, message))
    stall = stalled([r for r, _, _ in found], budget)
    violations: list[str] = []
    for r, limit, message in found:
        if r.key in stall:
            notes.append(f"runner stall (overlaps another over-budget test, not failing): {message}")
            continue
        if touched is not None and r.key not in touched:
            tolerance = limit * budget.untouched_factor
            if r.seconds <= tolerance:
                notes.append(f"untouched test over its limit, within the {tolerance:g}s tolerance "
                             f"(not failing): {message}")
                continue
            message += f" The PR did not touch it, but it is over the {tolerance:g}s tolerance too."
        violations.append(message)
    # The aida-cli-lib unit binary runs split across lib-0 and lib-1, so the
    # other half's entries never run in a lane; only other binaries get notes.
    ran = {binary_of(key) for key in seen} - {"aida-cli-lib"}
    for key in sorted(set(budget.entries) - seen):
        if binary_of(key) in ran:
            notes.append(f"{key}: budgeted but did not run in this log")
    return violations, notes


def summary_markdown(rows: list[Binary], budget: Budget | None, top: int) -> str:
    total = sum(r.seconds for r in rows)
    counted = sum(r.passed + r.ignored for r in rows)
    lines = [
        "### Test timings",
        "",
        f"{len(rows)} rows (tests, or binaries from a cargo test log), {total:.0f}s summed, "
        f"{counted} tests passed+ignored.",
        "",
        "| Test or binary | Seconds | Tests | Budget |",
        "|---|---:|---:|---:|",
    ]
    for r in sorted(rows, key=lambda r: -r.seconds)[:top]:
        b = budget.entries.get(r.key) if budget else None
        lines.append(f"| `{r.key}` | {r.seconds:.1f} | {r.passed + r.ignored} | {'' if b is None else f'{b:g}'} |")
    return "\n".join(lines) + "\n"


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description="Per-test (JUnit) or per-binary (cargo test log) timings and the slow-test budget gate.")
    ap.add_argument("log")
    ap.add_argument("--junit", nargs="+", default=[], metavar="XML",
                    help="nextest JUnit reports: one row per test; the log then adds only its doctest rows")
    ap.add_argument("--step", help="with a `gh run view --log` download: read only this step (e.g. 'Run tests')")
    ap.add_argument("--budget", help="budget TOML; fail on a violation")
    ap.add_argument("--changed-files", metavar="FILE",
                    help="the PR's changed paths, one per line: limits are hard only for tests it touched; "
                         "others get the budget's untouched_factor tolerance")
    ap.add_argument("--json", help="write the rows as JSON here")
    ap.add_argument("--summary", help="append a Markdown table here")
    ap.add_argument("--top", type=int, default=25, help="rows in the printed and Markdown tables")
    a = ap.parse_args(argv)
    try:
        with open(a.log, encoding="utf-8", errors="replace") as fh:
            rows = parse_log(step_lines(fh, a.step) if a.step else fh)
        if a.junit:
            rows = [r for r in rows if r.key.startswith("doctests:")]
            for path in a.junit:
                try:
                    rows += parse_junit(path)
                except FileNotFoundError:
                    print(f"test-timings: note: {path}: no such report", file=sys.stderr)
        budget = load_budget(a.budget) if a.budget else None
        changed = None
        if a.changed_files:
            with open(a.changed_files, encoding="utf-8") as fh:
                changed = [line.strip() for line in fh if line.strip()]
    except (OSError, tomllib.TOMLDecodeError, ET.ParseError, ValueError) as e:
        print(f"test-timings: {e}", file=sys.stderr)
        return 2
    if not rows:
        print(f"test-timings: no test results in {a.log} {' '.join(a.junit)}; nothing was measured", file=sys.stderr)
        return 2
    total = sum(r.seconds for r in rows)
    print(f"test-timings: {len(rows)} rows, {total:.1f}s summed, "
          f"{sum(r.passed + r.ignored for r in rows)} tests passed+ignored")
    for r in sorted(rows, key=lambda r: -r.seconds)[:a.top]:
        print(f"  {r.seconds:8.2f}s  {r.passed + r.ignored:6d} tests  {r.key}")
    if a.json:
        with open(a.json, "w", encoding="utf-8") as fh:
            json.dump({"total_seconds": total, "rows": [asdict(r) for r in rows]}, fh, indent=1)
    if a.summary:
        with open(a.summary, "a", encoding="utf-8") as fh:
            fh.write(summary_markdown(rows, budget, a.top))
    if budget is None:
        return 0
    touched = None if changed is None else touched_tests(rows, changed)
    violations, notes = check(rows, budget, touched)
    for n in notes:
        # A stall or a tolerated untouched test shows as a CI warning
        # annotation, so it is seen, not buried.
        warn = n.startswith(("runner stall", "untouched test"))
        print(f"{'::warning::' if warn else ''}test-timings: note: {n}")
    for v in violations:
        print(f"test-timings: BUDGET: {v}", file=sys.stderr)
    return 1 if violations else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
