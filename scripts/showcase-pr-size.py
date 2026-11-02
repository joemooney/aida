#!/usr/bin/env python3
"""Reproduce the PR-size figures in the engineering audit (docs/showcase/engineering-audit.html).

trace:TASK-2835 | ai:claude

Usage (from the repository root):

    python3 scripts/showcase-pr-size.py            # summary statistics per window
    python3 scripts/showcase-pr-size.py --svg      # also the chart's density polylines and marker x positions

Method (the audit's methodology note states the same thing):

* PR merges are first-parent commits of the pinned revision whose subject ends in
  ``(#N)``, read from ``git log --first-parent <rev> --format='@%cs %s' --shortstat``.
* PR size = insertions + deletions from that shortstat; no files are excluded.
* Windows filter on committer date (``%cs`` >= start date).
* Median is the midpoint average; p90 is the inclusive decile
  (``statistics.quantiles(..., n=10, method="inclusive")``); mean is the arithmetic mean.

Density curve (``--svg``):

* Sample: log10(max(size, 1)) for every PR in the 2026-05-10 window (6 zero-line PRs map to 0).
* Estimator: Gaussian kernel density estimate, bandwidth from Silverman's rule of thumb
  h = 0.9 * min(sd, IQR / 1.34) * n^(-1/5), computed on the log10 sample.
* Evaluated at 46 evenly spaced points on log10 LoC in [1, log10(5012)] (10 to 5,012 LoC);
  no boundary correction, so mass below 10 and above 5,012 LoC is simply not drawn.
* Screen mapping: x = 45 + 445 * (log10(LoC) - 1) / (log10(5012) - 1); the peak density
  is scaled to y = 90 above the y = 236 baseline.

Only Python's standard library and Git are used.
"""

import argparse
import math
import re
import statistics
import subprocess

PINNED_REV = "6a3f67671a9"
WINDOWS = [
    ("all history", None, None),
    ("since 2026-05-10", "2026-05-10", None),
    ("since 2026-09-02", "2026-09-02", None),
    ("since 2026-10-02", "2026-10-02", None),
    ("June 2026", "2026-06-01", "2026-07-01"),
]
CHART_WINDOW_START = "2026-05-10"
X0, X1 = 45.0, 490.0
LOG_LO, LOG_HI = 1.0, math.log10(5012)
Y_BASE, Y_PEAK = 236.0, 90.0
GRID_POINTS = 46


def pr_sizes(rev):
    out = subprocess.run(
        ["git", "log", "--first-parent", rev, "--format=@%cs %s", "--shortstat"],
        check=True, capture_output=True, text=True,
    ).stdout
    prs, cur = [], None
    for line in out.splitlines():
        if line.startswith("@"):
            if cur:
                prs.append(cur)
            date, subject = line[1:].split(" ", 1)
            cur = [date, subject, 0]
        elif "changed" in line and cur:
            cur[2] += sum(int(n) for n in re.findall(r"(\d+) (?:insertion|deletion)", line))
    if cur:
        prs.append(cur)
    return [(d, n) for d, s, n in prs if re.search(r"\(#\d+\)\s*$", s)]


def window(prs, start, end):
    return sorted(n for d, n in prs if (start is None or d >= start) and (end is None or d < end))


def summary(sizes):
    n = len(sizes)
    return {
        "prs": n,
        "median": statistics.median(sizes),
        "p90": statistics.quantiles(sizes, n=10, method="inclusive")[-1],
        "mean": sum(sizes) / n,
        "gt400_pct": 100 * sum(s > 400 for s in sizes) / n,
        "gt1000_pct": 100 * sum(s > 1000 for s in sizes) / n,
    }


def x_of(loc):
    return X0 + (X1 - X0) * (math.log10(loc) - LOG_LO) / (LOG_HI - LOG_LO)


def kde_points(sizes):
    logs = [math.log10(max(s, 1)) for s in sizes]
    n = len(logs)
    q1, _, q3 = statistics.quantiles(logs, n=4, method="inclusive")
    h = 0.9 * min(statistics.stdev(logs), (q3 - q1) / 1.34) * n ** -0.2
    grid = [LOG_LO + (LOG_HI - LOG_LO) * i / (GRID_POINTS - 1) for i in range(GRID_POINTS)]
    dens = [sum(math.exp(-0.5 * ((g - v) / h) ** 2) for v in logs) / (n * h * math.sqrt(2 * math.pi)) for g in grid]
    peak = max(dens)
    pts = [(X0 + (X1 - X0) * i / (GRID_POINTS - 1), Y_BASE - (Y_BASE - Y_PEAK) * d / peak) for i, d in enumerate(dens)]
    return h, pts


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--rev", default=PINNED_REV)
    ap.add_argument("--svg", action="store_true", help="print the density polylines and marker positions")
    args = ap.parse_args()
    prs = pr_sizes(args.rev)
    for label, start, end in WINDOWS:
        s = summary(window(prs, start, end))
        print(f"{label}: {s['prs']} PRs, median {s['median']:g}, p90 {s['p90']:.1f}, mean {s['mean']:.1f}, "
              f"{s['gt400_pct']:.1f}% >400, {s['gt1000_pct']:.1f}% >1,000")
    if not args.svg:
        return
    sizes = window(prs, CHART_WINDOW_START, None)
    s = summary(sizes)
    h, pts = kde_points(sizes)
    print(f"KDE: Gaussian kernel, Silverman bandwidth h = {h:.4f} log10 units, n = {len(sizes)}")
    for name in ("median", "mean", "p90"):
        print(f"x({name} {s[name]:.1f}) = {x_of(s[name]):.1f}")
    print(f"x(400) = {x_of(400):.1f}")
    split = x_of(400)
    left = [(x, y) for x, y in pts if x < split]
    right = [(x, y) for x, y in pts if x >= split]
    # Linear interpolation of the curve height at the 400-LoC split.
    (xa, ya), (xb, yb) = left[-1], right[0]
    y_split = ya + (yb - ya) * (split - xa) / (xb - xa)
    fmt = lambda seq: " ".join(f"{x:.1f},{y:.1f}" for x, y in seq)
    print(f'ptsLeft = "{X0:.1f},{Y_BASE:.0f} {fmt(left)} {split:.1f},{y_split:.1f} {split:.1f},{Y_BASE:.0f}"')
    print(f'ptsRight = "{split:.1f},{Y_BASE:.0f} {split:.1f},{y_split:.1f} {fmt(right)} {X1:.1f},{Y_BASE:.0f}"')
    print(f'ptsLine = "{fmt(pts)}"')


if __name__ == "__main__":
    main()
