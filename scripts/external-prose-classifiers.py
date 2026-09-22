#!/usr/bin/env python3
"""Enumerate declared production classifiers over external-tool prose.

Predicate: a classifier is a production Rust decision site carrying a line
comment of the exact form `// external-prose-classifier: <site>`.  Generated
files, tests, fixtures, and prose without that marker deliberately do not
count.  This marker-based predicate is intentionally syntactic: inferring
whether an arbitrary string comparison consumes external output is not
decidable from Rust syntax alone.  Review must require the marker when adding
such a decision; this checker then makes additions and documentation drift
mechanically visible.

trace:TASK-1300 | ai:codex
"""

from __future__ import annotations

import argparse
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
DOC = ROOT / "docs/architecture/external-tool-output-classifiers.md"
MARKER = re.compile(r"^\s*// external-prose-classifier: ([A-Za-z0-9_:{}-]+)\s*$")


def marker_anchors(source: str) -> dict[int, str]:
    """Map marker lines to their lexically containing Rust function."""
    anchors: dict[int, str] = {}
    scopes: list[str | None] = []
    pending_fn = False
    pending_name: str | None = None
    block_depth = 0
    quote: str | None = None
    escaped = False
    raw_hashes: int | None = None

    for line_no, line in enumerate(source.splitlines(), 1):
        if MARKER.match(line):
            containing = next((scope for scope in reversed(scopes) if scope), None)
            if containing:
                anchors[line_no] = containing

        index = 0
        while index < len(line):
            char = line[index]
            following = line[index + 1] if index + 1 < len(line) else ""
            if raw_hashes is not None:
                terminator = '"' + "#" * raw_hashes
                end = line.find(terminator, index)
                if end < 0:
                    break
                raw_hashes = None
                index = end + len(terminator)
                continue
            if block_depth:
                if char == "/" and following == "*":
                    block_depth += 1
                    index += 2
                elif char == "*" and following == "/":
                    block_depth -= 1
                    index += 2
                else:
                    index += 1
                continue
            if quote:
                if escaped:
                    escaped = False
                elif char == "\\":
                    escaped = True
                elif char == quote:
                    quote = None
                index += 1
                continue
            if char == "/" and following == "/":
                break
            if char == "/" and following == "*":
                block_depth = 1
                index += 2
                continue
            raw = re.match(r"(?:b)?r(#+)?\"", line[index:])
            if raw:
                raw_hashes = len(raw.group(1) or "")
                index += len(raw.group(0))
                continue
            if char == "'":
                # A Rust lifetime (`'a`) is not a character literal. Only enter
                # quote state when this line has a syntactic closing apostrophe.
                literal = re.match(r"'(?:\\.|[^\\'])'", line[index:])
                if not literal:
                    index += 1
                    continue
            if char in {'"', "'"}:
                quote = char
                escaped = False
                index += 1
                continue
            if char.isalpha() or char == "_":
                end = index + 1
                while end < len(line) and (line[end].isalnum() or line[end] == "_"):
                    end += 1
                token = line[index:end]
                if pending_fn and pending_name is None:
                    pending_name = token
                elif token == "fn":
                    pending_fn = True
                    pending_name = None
                index = end
                continue
            if char == "{":
                scopes.append(pending_name if pending_fn else None)
                pending_fn = False
                pending_name = None
            elif char == "}":
                if scopes:
                    scopes.pop()
                pending_fn = False
                pending_name = None
            elif char == ";" and pending_fn:
                pending_fn = False
                pending_name = None
            index += 1
    return anchors


def enumerate_sites(root: pathlib.Path) -> list[tuple[str, str, int]]:
    found: list[tuple[str, str, int]] = []
    for path in sorted(root.glob("aida-*/src/**/*.rs")):
        source = path.read_text(encoding="utf-8")
        lines = source.splitlines()
        anchors = marker_anchors(source)
        for line_no, line in enumerate(lines, 1):
            match = MARKER.match(line)
            if match:
                site = match.group(1)
                # Resolve structural containment rather than the nearest prior
                # declaration: a marker after a closed function has no anchor.
                # trace:TASK-1309 | ai:codex
                function = anchors.get(line_no)
                if function is None:
                    raise ValueError(
                        f"marker {site} at {path.relative_to(root)}:{line_no} "
                        "has no function anchor"
                    )
                if site.rsplit("::", 1)[-1] != function:
                    raise ValueError(
                        f"marker {site} at {path.relative_to(root)}:{line_no} "
                        f"is anchored to function {function}"
                    )
                found.append((site, path.relative_to(root).as_posix(), line_no))
    names = [row[0] for row in found]
    duplicates = sorted({name for name in names if names.count(name) > 1})
    if duplicates:
        raise ValueError(f"duplicate classifier marker(s): {', '.join(duplicates)}")
    return sorted(found)


def render(rows: list[tuple[str, str, int]]) -> str:
    body = [
        "# External-tool output classifiers",
        "",
        "<!-- Generated by scripts/external-prose-classifiers.py; do not edit by hand. -->",
        "",
        "A production classifier is enumerated when its Rust decision site carries",
        "`// external-prose-classifier: <site>`. Tests, fixtures, generated files, and",
        "unmarked string comparisons deliberately do not count. The marker is the precise",
        "boundary because Rust syntax cannot reveal whether an arbitrary string came from",
        "an external process; code review must require it for every new such decision.",
        "The module-qualified symbol is the stable inventory key. Source paths and line",
        "numbers are emitted by the generator only as non-authoritative navigation aids.",
        "",
        "| Stable classifier key |",
        "|---|",
    ]
    # TASK-1309: only the semantic module-qualified symbol is authoritative.
    # Source paths and line numbers remain available in stdout for navigation;
    # neither belongs in the generated comparison because both can move while
    # the classifier set remains unchanged.
    # trace:TASK-1309 | ai:codex
    body.extend(f"| `{name}` |" for name, _path, _line in rows)
    body.extend(
        [
            "",
            "<!-- trace:TASK-1300 | ai:codex -->",
            "<!-- trace:TASK-1309 | ai:codex -->",
            "",
        ]
    )
    return "\n".join(body)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true", help="fail if the generated doc is stale")
    parser.add_argument("--root", type=pathlib.Path, default=ROOT, help=argparse.SUPPRESS)
    args = parser.parse_args()
    root = args.root.resolve()
    try:
        rows = enumerate_sites(root)
    except ValueError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    if not rows:
        print("error: no external-prose classifier markers found", file=sys.stderr)
        return 1
    rendered = render(rows)
    doc = root / DOC.relative_to(ROOT)
    if args.check:
        current = doc.read_text(encoding="utf-8") if doc.exists() else ""
        if current != rendered:
            # BUG-1526 criterion 3: name what actually changed. The old message
            # said only "is stale", which reads as a real inventory change and
            # sent reviewers to re-derive a diff the checker already knows.
            # Since criterion 1 removed line numbers from the comparison, a
            # difference here IS a set change — drift alone can no longer
            # produce one.
            # trace:BUG-1526 | ai:claude
            def sites(text: str) -> set[str]:
                rows = set()
                for line in text.splitlines():
                    if not line.startswith("| `"):
                        continue
                    cells = [c.strip().strip("`") for c in line.split("|")]
                    if len(cells) >= 2:
                        rows.add(cells[1])
                return rows

            added = sorted(sites(rendered) - sites(current))
            removed = sorted(sites(current) - sites(rendered))
            print(f"error: {doc.relative_to(root)} does not match the markers in the tree", file=sys.stderr)
            if added:
                print(f"  added:   {', '.join(added)}", file=sys.stderr)
            if removed:
                print(f"  removed: {', '.join(removed)}", file=sys.stderr)
            if not added and not removed:
                print("  the classifier set is unchanged; only surrounding text differs", file=sys.stderr)
            print(f"  regenerate with a bare run: python3 {pathlib.Path(__file__).name}", file=sys.stderr)
            return 1
    else:
        doc.write_text(rendered, encoding="utf-8")
    for name, path, line in rows:
        print(f"{name}\t{path}:{line}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
