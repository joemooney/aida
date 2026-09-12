#!/usr/bin/env bash
set -euo pipefail

# trace:TASK-1205 | ai:codex
ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
ALLOWLIST="$ROOT/scripts/portability-allowlist.txt"

usage() {
  cat <<'USAGE'
usage: scripts/check-portability.sh [--print-findings] [--check-allowlist-growth BASE_REF]

Flags Linux-only assumptions in Rust test code that are not behind an
explicit Unix/Linux cfg. Baseline entries live in scripts/portability-allowlist.txt
as exact `path:trimmed source line` records.
USAGE
}

PRINT_FINDINGS=0
BASE_REF=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --print-findings)
      PRINT_FINDINGS=1
      shift
      ;;
    --check-allowlist-growth)
      BASE_REF="${2:-}"
      if [[ -z "$BASE_REF" ]]; then
        echo "error: --check-allowlist-growth requires a base ref" >&2
        exit 2
      fi
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "error: unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ -n "$BASE_REF" ]]; then
  tmp_base="$(mktemp)"
  tmp_base_sorted="$(mktemp)"
  tmp_new="$(mktemp)"
  trap 'rm -f "$tmp_base" "$tmp_base_sorted" "$tmp_new"' EXIT

  if git show "$BASE_REF:scripts/portability-allowlist.txt" >"$tmp_base" 2>/dev/null; then
    grep -v '^[[:space:]]*$' "$ALLOWLIST" | grep -v '^[[:space:]]*#' | sort -u >"$tmp_new" || true
    grep -v '^[[:space:]]*$' "$tmp_base" | grep -v '^[[:space:]]*#' | sort -u >"$tmp_base_sorted" || true
    if comm -13 "$tmp_base_sorted" "$tmp_new" | grep -q .; then
      echo "error: scripts/portability-allowlist.txt grew relative to $BASE_REF" >&2
      comm -13 "$tmp_base_sorted" "$tmp_new" >&2
      exit 1
    fi
  else
    echo "portability allowlist growth check: no baseline exists at $BASE_REF; skipping initial bootstrap comparison"
  fi
fi

python3 - "$ROOT" "$ALLOWLIST" "$PRINT_FINDINGS" <<'PY'
import pathlib
import re
import sys

root = pathlib.Path(sys.argv[1])
allowlist_path = pathlib.Path(sys.argv[2])
print_findings = sys.argv[3] == "1"

PATTERNS = [
    ("proc literal", re.compile(r'"[^"\n]*/proc/[^"\n]*"')),
    ("tmp literal", re.compile(r'"[^"\n]*/tmp[^"\n]*"')),
    ("PATH separator join", re.compile(r"\.join\(\s*\":\"\s*\)")),
    ("PATH separator split", re.compile(r"\.split\(\s*(?:':'|\":\")\s*\)")),
    ("shell command", re.compile(r'Command::new\(\s*"(?:sh|bash|/bin/sh)"\s*\)')),
]

LINUX_CFG_RE = re.compile(
    r"#\s*\[\s*cfg\s*\(\s*(?:unix|target_os\s*=\s*\"linux\")\s*\)\s*\]"
)
TEST_CFG_RE = re.compile(r"#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]")
RS_EXT = ".rs"


def strip_line_comment(line: str) -> str:
    in_string = False
    escaped = False
    for i, ch in enumerate(line):
        if in_string:
            if escaped:
                escaped = False
            elif ch == "\\":
                escaped = True
            elif ch == '"':
                in_string = False
        elif ch == '"':
            in_string = True
        elif ch == "/" and i + 1 < len(line) and line[i + 1] == "/":
            return line[:i]
    return line


def brace_delta(line: str) -> int:
    code = strip_line_comment(line)
    in_string = False
    escaped = False
    delta = 0
    for ch in code:
        if in_string:
            if escaped:
                escaped = False
            elif ch == "\\":
                escaped = True
            elif ch == '"':
                in_string = False
        elif ch == '"':
            in_string = True
        elif ch == "{":
            delta += 1
        elif ch == "}":
            delta -= 1
    return delta


def cfg_scoped_lines(lines: list[str], attr_re: re.Pattern[str]) -> set[int]:
    active_until_depths: list[int] = []
    pending_depths: list[int] = []
    cfg_lines: set[int] = set()
    depth = 0
    for lineno, line in enumerate(lines, start=1):
        while active_until_depths and depth < active_until_depths[-1]:
            active_until_depths.pop()
        if active_until_depths:
            cfg_lines.add(lineno)

        if attr_re.search(line):
            pending_depths.append(depth)
            cfg_lines.add(lineno)

        delta = brace_delta(line)
        opens_block = "{" in strip_line_comment(line)
        if pending_depths and opens_block:
            while pending_depths:
                pending = pending_depths.pop()
                active_until_depths.append(max(pending + 1, depth + 1))
        depth = max(0, depth + delta)
    return cfg_lines


def load_allowlist() -> set[str]:
    if not allowlist_path.exists():
        return set()
    allowed: set[str] = set()
    for raw in allowlist_path.read_text().splitlines():
        line = raw.strip()
        if line and not line.startswith("#"):
            allowed.add(line)
    return allowed


def scan() -> list[tuple[str, int, str, str]]:
    findings: list[tuple[str, int, str, str]] = []
    for path in sorted(root.rglob(f"*{RS_EXT}")):
        if any(part in {".git", "target"} for part in path.parts):
            continue
        rel = path.relative_to(root).as_posix()
        lines = path.read_text(errors="replace").splitlines()
        test_scoped = cfg_scoped_lines(lines, TEST_CFG_RE)
        if "/tests/" not in f"/{rel}" and not test_scoped:
            continue
        linux_lines = cfg_scoped_lines(lines, LINUX_CFG_RE)
        depth = 0
        for idx, raw in enumerate(lines, start=1):
            code = strip_line_comment(raw)
            in_test_scope = "/tests/" in f"/{rel}" or idx in test_scoped
            if in_test_scope and idx not in linux_lines:
                stripped = raw.strip()
                for label, pattern in PATTERNS:
                    if pattern.search(code):
                        findings.append((rel, idx, label, stripped))
                        break
    return findings


allowed = load_allowlist()
findings = scan()
new_findings = []

for rel, lineno, label, stripped in findings:
    record = f"{rel}:{stripped}"
    if print_findings:
        print(record)
    if record not in allowed:
        new_findings.append((rel, lineno, label, stripped))

if new_findings:
    print("error: new non-portable Rust test assumptions found:", file=sys.stderr)
    for rel, lineno, label, stripped in new_findings:
        print(f"{rel}:{lineno}: {label}: {stripped}", file=sys.stderr)
    print(
        "\nMove the code behind #[cfg(unix)] / #[cfg(target_os = \"linux\")], "
        "make it portable, or shrink/regenerate the baseline only for existing debt.",
        file=sys.stderr,
    )
    sys.exit(1)
PY
