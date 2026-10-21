#!/usr/bin/env python3
"""Tally `Build (ubuntu-latest)` test failures on main over a pinned window.

BUG-1742 asks how often the required Linux build reds out on `main`, and which
tests cost the merges. Answering that by eye does not survive a second reader:
the denominator drifts, and a run that never executed the suite gets counted as
a clean pass. This script pins both.

Two things it does that a hand-rolled `gh run list` does not:

* **It paginates unfiltered and filters client-side.** The
  `/actions/workflows/<id>/runs?branch=main&event=push` server-side filter
  returned a listing whose newest entry was three weeks stale on 2026-09-30,
  while the unfiltered listing was current. `gh run list --workflow ci.yml`
  was stale the same way. Anything built on those filters silently measures
  the wrong window.
* **It separates runs that ran the suite from runs that skipped it.** A
  docs-only push short-circuits the `Run tests` step (`Detect full-CI
  changes`), so it cannot flake. Leaving those in the denominator deflates the
  rate. They are reported as an explicit exclusion rather than dropped
  silently, because BUG-1742's acceptance requires naming what was excluded.

Usage:
    scripts/ci-flake-tally.py --since 2026-09-20
    scripts/ci-flake-tally.py --last 50 --json

trace:BUG-1742 | ai:claude
"""

import argparse
import json
import re
import subprocess
import sys
from collections import Counter, defaultdict

REPO = "joemooney/aida"
WORKFLOW_ID = 210963026  # .github/workflows/ci.yml ("CI")
BUILD_JOB_PREFIX = "Build"
TEST_STEP = "Run tests"

# `test some::path ... FAILED`, as libtest prints it.
FAILED_TEST = re.compile(rb"test ([A-Za-z0-9_:]+) \.\.\. FAILED")
# `   FAIL [   0.123s] ( 3/120) aida-cli::journey some::path`, as nextest (the
# test lanes' runner since TASK-1988) prints a failed, timed-out or crashed
# test; the name is `<binary id> <test>`. trace:TASK-1988 | ai:claude
NEXTEST_FAILED = re.compile(
    rb"\s(?:FAIL|TIMEOUT|SIG[A-Z]+|ABORT) \[ *[\d.]+s\](?: \( *\d+/ *\d+\))? ([\w:.-]+ [\w:]+)")
# `test result: FAILED. 7380 passed; 1 failed; ...`
RESULT_LINE = re.compile(rb"test result: FAILED\. (\d+) passed; (\d+) failed")


def gh_json(path: str, **params: object) -> object:
    cmd = ["gh", "api", "-X", "GET", path]
    for key, value in params.items():
        cmd += ["-f", f"{key}={value}"]
    out = subprocess.run(cmd, capture_output=True, check=True).stdout
    return json.loads(out)


def gh_raw(path: str) -> bytes:
    return subprocess.run(
        ["gh", "api", path], capture_output=True, check=True
    ).stdout


def fetch_runs(pages: int) -> list[dict]:
    """Newest-first CI runs, unfiltered. See the module docstring on filters."""
    runs = []
    for page in range(1, pages + 1):
        batch = gh_json(
            f"repos/{REPO}/actions/workflows/{WORKFLOW_ID}/runs",
            per_page=100,
            page=page,
        )["workflow_runs"]
        if not batch:
            break
        runs += batch
    runs.sort(key=lambda r: r["created_at"], reverse=True)
    return runs


# trace:TASK-1949 | ai:codex
def classify(run: dict) -> tuple[str, list[int]]:
    """Classify the actual test steps across old single-job and new shard runs."""
    jobs = gh_json(f"repos/{REPO}/actions/runs/{run['id']}/jobs")["jobs"]
    builds = [j for j in jobs if j["name"].startswith((BUILD_JOB_PREFIX, "CI ("))]
    if not builds:
        return "no-build-job", []
    # The lib lane split in two (TASK-1979); older runs keep the single lane.
    # trace:TASK-1979 | ai:claude
    lib = ('lib-0', 'lib-1') if any(j['name'] == 'CI (lib-0)' for j in builds) else ('lib',)
    expected = {f"CI ({lane})" for lane in (*lib, 'cli-0', 'cli-1', 'rest')}
    sharded = any(j['name'].startswith('CI (') for j in builds)
    # Matrix gate lanes also have a skipped Run tests step. They are not test
    # shards and must not turn every complete test suite into "incomplete".
    tests = [(j, next((s["conclusion"] for s in j["steps"] if s["name"] == TEST_STEP), None))
             for j in builds if not sharded or j['name'] in expected]
    tests = [(j, result) for j, result in tests if result is not None]
    if not tests:
        return "no-test-step", []
    ran = [(j, result) for j, result in tests if result != "skipped"]
    complete = not sharded or {j['name'] for j, _ in tests} == expected
    if not ran:
        return ("docs-only" if complete else "incomplete"), []
    failed = [j["id"] for j, result in ran if result != "success"]
    if failed:
        return "fail", failed
    if not complete or len(ran) != len(tests):
        return "incomplete", []
    return "pass", []


def failing_tests(job_id: int) -> tuple[list[str], list[tuple[int, int]]]:
    """Test names and per-binary (passed, failed) counts from a job's log.

    Reads the job log endpoint directly: `gh run view --log-failed` labelled
    every line `UNKNOWN STEP` on these runs and returned the checkout teardown
    instead of the test step.
    """
    log = gh_raw(f"repos/{REPO}/actions/jobs/{job_id}/logs")
    names = sorted({m.group(1).decode() for pattern in (FAILED_TEST, NEXTEST_FAILED)
                    for m in pattern.finditer(log)})
    counts = [(int(a), int(b)) for a, b in RESULT_LINE.findall(log)]
    return names, counts


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--since", help="ISO date; window starts here (UTC)")
    ap.add_argument("--last", type=int, default=50, help="minimum run count")
    ap.add_argument("--pages", type=int, default=9, help="listing pages to pull")
    ap.add_argument("--json", action="store_true", help="emit machine-readable")
    args = ap.parse_args()

    candidates = [
        r
        for r in fetch_runs(args.pages)
        if r["head_branch"] == "main"
        and r["event"] == "push"
        and r["status"] == "completed"
    ]
    # BUG-1742 pins the window as "the last N runs OR everything since <date>,
    # whichever covers more", so the next reader's number is comparable.
    window = candidates[: args.last]
    if args.since:
        by_date = [r for r in candidates if r["created_at"][:10] >= args.since]
        if len(by_date) > len(window):
            window = by_date
    if not window:
        print("no completed main/push runs in range", file=sys.stderr)
        return 1

    tally: Counter[str] = Counter()
    runs_by_test: defaultdict[str, list[str]] = defaultdict(list)
    outcomes: Counter[str] = Counter()
    failures = []
    for run in window:
        outcome, job_ids = classify(run)
        outcomes[outcome] += 1
        if outcome != "fail":
            continue
        # Count a test only once per workflow run, even across failed lanes.
        evidence = [failing_tests(job_id) for job_id in job_ids]
        names = sorted({name for names, _ in evidence for name in names})
        counts = [count for _, rows in evidence for count in rows]
        for name in names:
            tally[name] += 1
            runs_by_test[name].append(str(run["id"]))
        failures.append(
            {
                "run": str(run["id"]),
                "created_at": run["created_at"],
                "sha": run["head_sha"][:8],
                "tests": names,
                "results": counts,
            }
        )

    ran = outcomes["pass"] + outcomes["fail"]
    report = {
        "window": {
            "from": window[-1]["created_at"],
            "to": window[0]["created_at"],
            "completed_runs": len(window),
        },
        "excluded": {
            "docs-only-short-circuit": outcomes["docs-only"],
            "no-build-job": outcomes["no-build-job"],
            "no-test-step": outcomes["no-test-step"],
            "incomplete-test-lanes": outcomes["incomplete"],
        },
        "denominator": ran,
        "failures": outcomes["fail"],
        "rate": round(outcomes["fail"] / ran, 4) if ran else None,
        "per_test": {k: {"count": v, "runs": runs_by_test[k]} for k, v in tally.most_common()},
        "detail": failures,
    }
    if args.json:
        json.dump(report, sys.stdout, indent=2)
        print()
        return 0

    w = report["window"]
    print(f"window      {w['from']} .. {w['to']}")
    print(f"runs        {w['completed_runs']} completed main/push CI runs")
    for reason, n in report["excluded"].items():
        if n:
            print(f"  excluded  {n} ({reason} — the suite never ran)")
    print(f"denominator {ran} runs that executed `{TEST_STEP}`")
    pct = f"{100 * report['rate']:.1f}%" if report["rate"] is not None else "n/a"
    print(f"failures    {report['failures']}/{ran} = {pct}")
    print()
    print("per-test tally (every test, including single occurrences):")
    for name, info in report["per_test"].items():
        print(f"  {info['count']:>2}x  {name}")
        print(f"        runs: {', '.join(info['runs'])}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
