#!/usr/bin/env python3
"""Trace/doc-header stranding gate (BUG-1550).

A new item (fn/struct/enum/trait/impl/const/static/type/mod) inserted
IMMEDIATELY BELOW an existing `///` doc comment and its `// trace:SPEC-ID`
marker steals that header: the header now documents the new item, and the
item it was written for is left with no doc and no trace. Rust compiles this
fine either way, so fmt/clippy/tests never catch it. The dangerous half of
this defect class is the TRACE marker: `aida why <file:line>` resolves code
to a decision via the NEAREST PRECEDING trace comment, so a stranded trace
silently INVERTS that resolution -- the tool confidently attributes code to
the wrong spec. See BUG-1550 for the full writeup, including three concrete
instances found the same night (aida-cli-lib/src/awaiting_you.rs, twice, and
aida-cli-lib/src/drain_state.rs) and the class-level acceptance criteria.

WHAT THIS CHECKS (and, explicitly, what it does not):

This gate detects only the TRACE-MARKER-CARRYING shape of the defect: a doc
comment sandwiched between two `// trace:` RUNS that belong to genuinely
different specs. It is a PURELY STRUCTURAL check -- no prose/semantic
comparison of doc content to identifier names is involved anywhere in this
file. That is a deliberate, measured choice, not an oversight:

  A companion word-overlap / identifier-match heuristic was prototyped to
  also catch DOC-ONLY strandings (no trace marker involved at all -- e.g. a
  doc comment quietly migrating to the wrong bare function while its own
  trace-carrying neighbour stays correct). It was rejected after empirical
  measurement, not because it caught nothing but because it was unreliable
  in a way that is worse than firing on nothing:
    - Naive first/last-sentence identifier-overlap: 177 hits tree-wide.
    - A windowed comparative scorer (only score against bare items within
      50 item-slots; flag when some other bare item scores strictly better
      than the item currently attached): 347 hits at a looser threshold,
      127 at a tighter one.
    - Spot-checking the residual showed the false-positive source is
      STRUCTURAL TO THIS CODEBASE, not a threshold nobody found yet: this
      project's descriptive-sentence test-naming convention (e.g.
      `success_rate()` vs `success_rate_is_zero_for_empty_tally()`) and its
      precise, consistent domain vocabulary between sibling functions (e.g.
      `gap()` vs `has_zero_denominator()`) are EXACTLY what a word-overlap
      scorer mistakes for misattachment. The convention that makes this
      codebase well-documented is what defeats the heuristic.
    - Worse than the false-positive rate: on the one real, independently
      confirmed doc-only stranding available to test against (see BUG-1550
      / BUG-1545, `short_sha_for_row` vs `pluralize` in awaiting_you.rs),
      every threshold correctly flagged `short_sha_for_row`'s header as
      wrong but named the WRONG bare function as the true owner
      (`compact_line` instead of `pluralize`) -- confident wrong attribution
      is worse than an honest miss, since it sends a reader to fix the
      wrong thing. And `pluralize` itself -- the actually-bare, actually
      undocumented function -- was never named by the heuristic in either
      direction, because nothing in a word-overlap scorer points at a
      function with NO header as a culprit; it only compares headers that
      exist.
  Conclusion recorded here so the investigation is not repeated: doc-only
  stranding (no trace marker stolen) is NOT reliably file-level structurally
  detectable with a lexical heuristic on this codebase. See BUG-1550 for the
  full numbers and a linked follow-up spec enumerating concrete instances
  found by manual reading instead (including a cross-file variant, where the
  true owner lives in a DIFFERENT file after a SPIKE-78-style extraction --
  outside this checker's per-file scope even in principle).

KNOWN GAP, stated rather than implied: this checker's scope is per-file. A
stranding whose true owner lives in a different file (e.g. after code was
split across files and a header did not travel with its function) is
invisible to it even when a trace marker IS involved, because there is no
bare candidate to compare against in the file being scanned.

trace:BUG-1550 | ai:claude
"""

import argparse
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

ITEM_RE = re.compile(
    r'^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:unsafe\s+)?(?:extern\s+"[^"]*"\s+)?'
    r'(fn|struct|enum|trait|impl|const|static|type|mod)\s+([A-Za-z_][A-Za-z0-9_]*)'
)
SPEC_RE = re.compile(r'trace:([A-Za-z0-9][A-Za-z0-9-]*)')

# Files with a KNOWN, VERIFIED-BY-READING false positive: a single item
# legitimately re-documented/re-traced across two (or more) revisions, where
# the sandwiched doc segment genuinely and entirely describes the item it
# sits on. Each entry is a decision someone wrote down, not a threshold
# tuned to silence -- see scripts/glyph-lint.sh's ALLOW_LIST for the same
# convention. Keyed by "path:item_name" so a fix elsewhere in the same file
# doesn't accidentally suppress a NEW violation on a different item.
ALLOW_LIST = {
    # lib.rs: run_do_drive carries trace:TASK-1155 + trace:ADR-11 (its
    # original design), then a doc paragraph describing `aida do <spec>`,
    # then trace:STORY-776 (the shipped dispatcher). Read in full: the doc
    # covers the whole design across all three specs, not a different
    # function. No bare sibling in this file claims any part of it.
    "aida-cli-lib/src/lib.rs:run_do_drive",
    # lock_cmd.rs: handle_lock_command carries trace:TASK-661 (the
    # per-scope disposition lease design), then a doc paragraph describing
    # the `aida lock` dispatcher itself, then trace:STORY-711 (the slice
    # that shipped it). Read in full: both trace tags and the doc all
    # describe this one function's history.
    "aida-cli-lib/src/lock_cmd.rs:handle_lock_command",
}

SCAN_GLOBS = (
    "aida-cli-lib/src/**/*.rs",
    "aida-cli/src/**/*.rs",
    "aida-core/src/**/*.rs",
    "aida-tui/src/**/*.rs",
    "aida-server/src/**/*.rs",
)


def item_name(line):
    m = ITEM_RE.match(line)
    return (m.group(1), m.group(2)) if m else None


def parse_items(text):
    """Parse a source file's text into an ordered list of item dicts.

    Each dict describes either a header-carrying item (has_header=True, with
    its trace-line positions, doc-line positions, and the raw comment text
    per position -- everything downstream analysis needs) or a bare item
    (has_header=False). Non-item lines are skipped entirely.
    """
    lines = text.split("\n")
    n = len(lines)
    items = []
    i = 0
    while i < n:
        stripped = lines[i].strip()
        if stripped.startswith("///") or (stripped.startswith("//") and not stripped.startswith("//!") and "trace:" in stripped):
            j = i
            comment_lines = []
            while j < n:
                s = lines[j].strip()
                if s.startswith("///") or (s.startswith("//") and not s.startswith("//!") and "trace:" in s):
                    comment_lines.append((j, s))
                    j += 1
                else:
                    break
            k = j
            while k < n and lines[k].strip().startswith("#["):
                k += 1
            target_line = lines[k] if k < n else ""
            res = item_name(target_line)
            if res:
                trace_positions = [
                    idx for idx, (_, s) in enumerate(comment_lines) if s.startswith("// trace:")
                ]
                doc_positions = [
                    idx for idx, (_, s) in enumerate(comment_lines) if s.startswith("///")
                ]
                items.append(
                    {
                        "line": k + 1,
                        "kind": res[0],
                        "name": res[1],
                        "header_start": comment_lines[0][0] + 1,
                        "trace_positions": trace_positions,
                        "doc_positions": doc_positions,
                        "comment_texts": [s for (_, s) in comment_lines],
                        "has_header": True,
                    }
                )
                # Skip the item's own declaration line too -- otherwise the
                # plain-ITEM_RE branch below re-matches it as a spurious
                # SECOND, header-less entry at the same line.
                i = k + 1
            else:
                i = j
        else:
            m = ITEM_RE.match(lines[i])
            if m:
                items.append(
                    {
                        "line": i + 1,
                        "kind": m.group(1),
                        "name": m.group(2),
                        "header_start": None,
                        "trace_positions": [],
                        "doc_positions": [],
                        "comment_texts": [],
                        "has_header": False,
                    }
                )
            i += 1
    return items


def find_strandings(text):
    """Return a list of (header_start_line, item_line, kind, name) violations.

    A violation is a header with 2+ trace RUNS (a run = one or more
    consecutive `// trace:` lines with nothing between them -- stacking
    several trace tags on one item, e.g. co-authorship, is a normal,
    measured-common idiom: 210 instances of back-to-back trace lines exist
    in this tree with no doc between them, and none of those are
    strandings) where:
      - at least one run has ZERO doc lines in the segment immediately
        preceding it (nothing of its own explains it), AND
      - that doc-less run shares NO spec id with any other run in the
        header (a spec id repeated across runs means the same item was
        re-tagged across revisions/authors -- not a splice; two genuinely
        different specs with a naked leading run is the tracker_cmd.rs /
        mcp.rs shape this gate exists to catch).
    """
    violations = []
    items = parse_items(text)
    for it in items:
        if not it["has_header"]:
            continue
        tp = it["trace_positions"]
        dp = it["doc_positions"]
        if len(tp) < 2:
            continue

        runs = []
        cur_run = [tp[0]]
        for p in tp[1:]:
            if p == cur_run[-1] + 1:
                cur_run.append(p)
            else:
                runs.append(cur_run)
                cur_run = [p]
        runs.append(cur_run)
        if len(runs) < 2:
            continue

        prev_boundary = -1
        run_has_doc = []
        run_specs = []
        for run in runs:
            seg_doc_count = sum(1 for d in dp if prev_boundary < d < run[0])
            run_has_doc.append(seg_doc_count > 0)
            run_text = " ".join(it["comment_texts"][p] for p in run)
            run_specs.append(set(SPEC_RE.findall(run_text)))
            prev_boundary = run[-1]

        bad_runs = [idx for idx, ok in enumerate(run_has_doc) if not ok]
        if not bad_runs:
            continue
        genuinely_bad = any(
            not (run_specs[b] & set().union(*(s for k, s in enumerate(run_specs) if k != b)))
            for b in bad_runs
        )
        if genuinely_bad:
            violations.append((it["header_start"], it["line"], it["kind"], it["name"]))
    return violations


def scan_file(path: Path, root: Path):
    try:
        text = path.read_text(encoding="utf-8", errors="ignore")
    except OSError:
        return []
    try:
        rel = path.relative_to(root).as_posix()
    except ValueError:
        rel = str(path)
    out = []
    for header_start, item_line, kind, name in find_strandings(text):
        key = f"{rel}:{name}"
        if key in ALLOW_LIST:
            continue
        out.append((rel, header_start, item_line, kind, name))
    return out


def scan_tree(root: Path):
    seen = set()
    findings = []
    for pattern in SCAN_GLOBS:
        for p in sorted(root.glob(pattern)):
            if p in seen:
                continue
            seen.add(p)
            findings.extend(scan_file(p, root))
    return findings


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "paths", nargs="*", help="Specific .rs files to scan (default: the whole tree)."
    )
    args = parser.parse_args(argv)

    if args.paths:
        findings = []
        for raw in args.paths:
            p = Path(raw).resolve()
            findings.extend(scan_file(p, ROOT))
    else:
        findings = scan_tree(ROOT)

    if findings:
        print(f"trace-header stranding: {len(findings)} violation(s)\n", file=sys.stderr)
        for rel, header_start, item_line, kind, name in findings:
            print(
                f"  {rel}:{header_start}-{item_line}: {kind} `{name}` sits below a "
                "doc/trace header whose leading trace run has no doc of its own and "
                "shares no spec id with the rest of the header -- likely stranded "
                "from a different item (BUG-1550). If this is a genuine multi-"
                "revision header for THIS item, add "
                f'"{rel}:{name}" to ALLOW_LIST in scripts/check-trace-headers.py '
                "with a one-line reason.",
                file=sys.stderr,
            )
        return 1
    print("trace-header stranding: no violations", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
