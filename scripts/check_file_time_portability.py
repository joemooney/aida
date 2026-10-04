#!/usr/bin/env python3
"""Flag read-only File::open handles used to mutate file timestamps.

Windows requires FILE_WRITE_ATTRIBUTES for File::set_times. File::open only
requests read access, so tests that backdate files through that handle fail on
Windows. Explicit non-Windows cfg branches are excluded.

trace:BUG-1728 | ai:codex
"""

from __future__ import annotations

import argparse
import pathlib
import re
import subprocess
import sys


FILE_OPEN = re.compile(r"\b(?:[A-Za-z_]\w*::)*File::open\s*\(")
OPEN_BINDING = re.compile(
    r"\blet\s+(?:mut\s+)?(?P<name>[A-Za-z_]\w*)\s*=\s*.*?\b(?:[A-Za-z_]\w*::)*File::open\s*\(",
    re.DOTALL,
)
TIME_MUTATION = re.compile(r"\b(?P<receiver>[A-Za-z_]\w*)\s*\.\s*(?P<method>set_times|set_modified)\s*\(")
DIRECT_TIME_MUTATION = re.compile(r"\.\s*(?:set_times|set_modified)\s*\(")
NON_WINDOWS_CFG = re.compile(
    r"#\s*\[\s*cfg\s*\(\s*not\s*\(\s*windows\s*\)\s*\)\s*\]"
)


def strip_line_comment(line: str) -> str:
    """Remove a // comment while leaving markers inside strings alone."""
    in_string = False
    escaped = False
    for index, char in enumerate(line):
        if in_string:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == '"':
                in_string = False
        elif char == '"':
            in_string = True
        elif char == "/" and line[index : index + 2] == "//":
            return line[:index]
    return line


def brace_delta(line: str) -> int:
    """Count braces outside comments and ordinary strings."""
    code = strip_line_comment(line)
    in_string = False
    escaped = False
    delta = 0
    for char in code:
        if in_string:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == '"':
                in_string = False
        elif char == '"':
            in_string = True
        elif char == "{":
            delta += 1
        elif char == "}":
            delta -= 1
    return delta


def scan_source(source: str) -> list[tuple[int, str, str]]:
    """Return (line, receiver, method) for unsafe File::open time mutations.

    The scanner follows `let` bindings through their lexical block and also
    catches direct `File::open(...).unwrap().set_times(...)` chains. Its scope
    is intentionally the Windows access-mode defect; a `cfg(not(windows))`
    binding is safe for this rule and is ignored.
    """
    lines = source.splitlines()
    depth = 0
    active: list[tuple[str, int, int]] = []  # name, binding depth, binding line
    pending_statement = ""
    pending_line = 1
    pending_depth = 0
    pending_excluded = False
    exclude_next = False
    findings: list[tuple[int, str, str]] = []

    def process(statement: str, line_no: int, scope_depth: int, excluded: bool) -> None:
        if excluded:
            return
        open_match = FILE_OPEN.search(statement)
        if open_match:
            chained = DIRECT_TIME_MUTATION.search(statement, open_match.end())
            if chained:
                method = chained.group(0).strip(". ()")
                findings.append((line_no, "<temporary>", method))

        for match in TIME_MUTATION.finditer(statement):
            if any(name == match.group("receiver") for name, _, _ in active):
                findings.append((line_no, match.group("receiver"), match.group("method")))

        binding = OPEN_BINDING.search(statement)
        if binding:
            active.append((binding.group("name"), scope_depth, line_no))

    for line_no, raw in enumerate(lines, start=1):
        code = strip_line_comment(raw)
        stripped = code.strip()

        if NON_WINDOWS_CFG.search(code):
            exclude_next = True
            depth = max(0, depth + brace_delta(raw))
            continue

        if not stripped or stripped.startswith("#"):
            depth = max(0, depth + brace_delta(raw))
            continue

        if not pending_statement:
            pending_line = line_no
            pending_depth = depth
            pending_excluded = exclude_next
            exclude_next = False

        pending_statement += code + " "
        # Rust statements end at semicolons. Splitting here preserves the
        # receiver binding while avoiding cross-statement false positives.
        parts = pending_statement.split(";")
        for part in parts[:-1]:
            process(part, pending_line, pending_depth, pending_excluded)
            pending_line = line_no
            pending_depth = depth
            pending_excluded = False
        pending_statement = parts[-1]

        depth = max(0, depth + brace_delta(raw))
        active = [binding for binding in active if depth >= binding[1]]

    if pending_statement.strip():
        process(pending_statement, pending_line, pending_depth, pending_excluded)

    # Repeated use of the same handle in a statement should only report once.
    return list(dict.fromkeys(findings))


def rust_files(root: pathlib.Path) -> list[pathlib.Path]:
    try:
        result = subprocess.run(
            ["git", "-C", str(root), "ls-files", "-z", "--", "*.rs"],
            check=True,
            capture_output=True,
        )
        relatives = result.stdout.decode(errors="replace").split("\0")
        candidates = [root / name for name in relatives if name]
    except (OSError, subprocess.CalledProcessError):
        candidates = list(root.rglob("*.rs"))
    return [
        path
        for path in sorted(candidates)
        if path.is_file() and not any(part in {".git", "target"} for part in path.parts)
    ]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=pathlib.Path)
    parser.add_argument("--print-findings", action="store_true")
    args = parser.parse_args()
    root = args.root.resolve()

    failures: list[tuple[pathlib.Path, int, str, str]] = []
    for path in rust_files(root):
        for line_no, receiver, method in scan_source(path.read_text(errors="replace")):
            failures.append((path, line_no, receiver, method))

    for path, line_no, receiver, method in failures:
        relative = path.relative_to(root).as_posix()
        message = (
            f"{relative}:{line_no}: File::open handle `{receiver}` calls `{method}`; "
            "Windows requires write-attributes access for timestamp changes"
        )
        if args.print_findings:
            print(f"readonly-file-time-mutation\t{message}")

    if failures:
        print("error: read-only File::open handle mutates file timestamps:", file=sys.stderr)
        for path, line_no, receiver, method in failures:
            relative = path.relative_to(root).as_posix()
            print(f"{relative}:{line_no}: {receiver}.{method}()", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
