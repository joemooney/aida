#!/usr/bin/env bash
set -euo pipefail

# trace:TASK-1205 | ai:codex
ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
ALLOWLIST="$ROOT/scripts/portability-allowlist.txt"
RULES="$ROOT/scripts/portability-rules.json"

usage() {
  cat <<'USAGE'
usage: scripts/check-portability.sh [--print-findings] [--check-allowlist-growth BASE_REF]

Flags Linux-only assumptions in Rust test code that are not behind an
explicit Unix/Linux cfg, plus production-scope rules (e.g. control flow that
classifies on another component's message text; see STORY-1382). Baseline entries live in scripts/portability-allowlist.txt
as tab-separated `rule-id<TAB>path:trimmed source line` records.
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
  tmp_base_rules="$(mktemp)"
  trap 'rm -f "$tmp_base" "$tmp_base_rules"' EXIT

  if git show "$BASE_REF:scripts/portability-allowlist.txt" >"$tmp_base" 2>/dev/null &&
     git show "$BASE_REF:scripts/portability-rules.json" >"$tmp_base_rules" 2>/dev/null; then
    # trace:BUG-1301 | ai:codex
    python3 "$ROOT/scripts/check-portability-growth.py" \
      "$tmp_base" "$tmp_base_rules" "$ALLOWLIST" "$RULES"
  else
    echo "portability allowlist growth check: no baseline exists at $BASE_REF; skipping initial bootstrap comparison"
  fi
fi

python3 - "$ROOT" "$ALLOWLIST" "$RULES" "$PRINT_FINDINGS" <<'PY'
import json
import pathlib
import re
import sys

root = pathlib.Path(sys.argv[1])
allowlist_path = pathlib.Path(sys.argv[2])
rules_path = pathlib.Path(sys.argv[3])
print_findings = sys.argv[4] == "1"
# trace:STORY-1382 | ai:claude — a rule may declare `scope` ("test", the
# default, or "production"), and an optional `context` / `exempt` window: the
# line only counts when `context.regex` matches one of the `context.before`
# lines ABOVE it (never the matched line itself), unless `self_evident` matches
# the line; never when `exempt` matches in that window, or when `optout`
# matches the line or the line above it.
PATTERNS = []
for item in json.loads(rules_path.read_text()):
    context = item.get("context") or {}
    PATTERNS.append(
        {
            "id": item["id"],
            "label": item["label"],
            "regex": re.compile(item["regex"]),
            "scope": item.get("scope", "test"),
            "context": re.compile(context["regex"]) if context.get("regex") else None,
            "before": int(context.get("before", 0)),
            "exempt": re.compile(item["exempt"]) if item.get("exempt") else None,
            "self_evident": re.compile(item["self_evident"]) if item.get("self_evident") else None,
            "optout": re.compile(item["optout"]) if item.get("optout") else None,
        }
    )
if any(rule["scope"] not in {"test", "production"} for rule in PATTERNS):
    raise SystemExit("error: portability rule scope must be `test` or `production`")
HAS_PRODUCTION_RULES = any(rule["scope"] == "production" for rule in PATTERNS)

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


def toplevel_test_lines(lines: list[str]) -> set[int]:
    """Lines inside a column-0 `#[cfg(test)]` item, closed by a column-0 `}`.

    rustfmt puts a top-level item's closing brace in column 0, so this stays
    correct where `brace_delta` loses count (char literals, raw strings).
    trace:STORY-1382 | ai:claude
    """
    marked: set[int] = set()
    idx = 0
    while idx < len(lines):
        if re.match(r"#\[cfg\(test\)\]", lines[idx]):
            end = idx + 1
            while end < len(lines) and lines[end].startswith("#["):
                end += 1
            if end < len(lines) and lines[end].rstrip().endswith("{"):
                while end < len(lines) and not re.match(r"\}\s*$", lines[end]):
                    end += 1
            marked.update(range(idx + 1, end + 2))
            idx = end + 1
            continue
        idx += 1
    return marked


def load_allowlist() -> set[tuple[str, str]]:
    if not allowlist_path.exists():
        return set()
    allowed: set[tuple[str, str]] = set()
    for raw in allowlist_path.read_text().splitlines():
        line = raw.strip()
        if line and not line.startswith("#"):
            rule, separator, record = raw.partition("\t")
            if not separator:
                raise SystemExit(f"error: invalid portability allowlist row: {raw}")
            allowed.add((rule.strip(), record.strip()))
    return allowed


def scan() -> list[tuple[str, str, int, str, str]]:
    findings: list[tuple[str, str, int, str, str]] = []
    for path in sorted(root.rglob(f"*{RS_EXT}")):
        if any(part in {".git", "target"} for part in path.parts):
            continue
        rel = path.relative_to(root).as_posix()
        lines = path.read_text(errors="replace").splitlines()
        test_scoped = cfg_scoped_lines(lines, TEST_CFG_RE)
        # Production rules also skip column-0 test modules that brace counting
        # misses; test rules keep their original scope so their baseline holds.
        production_skip = test_scoped | toplevel_test_lines(lines)
        test_file = "/tests/" in f"/{rel}" or path.name.endswith(("_tests.rs", "_test.rs"))
        if not test_file and not test_scoped and not HAS_PRODUCTION_RULES:
            continue
        linux_lines = cfg_scoped_lines(lines, LINUX_CFG_RE)
        for idx, raw in enumerate(lines, start=1):
            code = strip_line_comment(raw)
            in_test_scope = test_file or idx in test_scoped
            stripped = raw.strip()
            for rule in PATTERNS:
                if rule["scope"] == "test":
                    if not in_test_scope or idx in linux_lines:
                        continue
                elif test_file or idx in production_skip:
                    continue
                if not rule["regex"].search(code):
                    continue
                if rule["optout"] and any(
                    rule["optout"].search(w) for w in lines[max(0, idx - 2) : idx]
                ):
                    continue
                window = lines[max(0, idx - 1 - rule["before"]) : idx - 1]
                evident = rule["self_evident"] and rule["self_evident"].search(code)
                if (
                    rule["context"]
                    and not evident
                    and not any(rule["context"].search(w) for w in window)
                ):
                    continue
                if rule["exempt"] and any(rule["exempt"].search(w) for w in lines[max(0, idx - 1 - rule["before"]) : idx]):
                    continue
                findings.append((rule["id"], rel, idx, rule["label"], stripped))
                break
    return findings


allowed = load_allowlist()
findings = scan()
new_findings = []

for rule, rel, lineno, label, stripped in findings:
    record = f"{rel}:{stripped}"
    if print_findings:
        print(f"{rule}\t{record}")
    if (rule, record) not in allowed and ("unattributed", record) not in allowed:
        new_findings.append((rule, rel, lineno, label, stripped))

if new_findings:
    print("error: new ratchet findings (non-portable test assumptions or prose classification):", file=sys.stderr)
    for rule, rel, lineno, label, stripped in new_findings:
        print(f"{rel}:{lineno}: {label}: {stripped}", file=sys.stderr)
    print(
        "\nMove the code behind #[cfg(unix)] / #[cfg(target_os = \"linux\")], "
        "make it portable, or shrink/regenerate the baseline only for existing debt. "
        "For prose-classification: branch on a typed field (enum, exit code, error "
        "variant) instead of the message text, or mark a real external-tool "
        "contract with `// external-prose-classifier: <site>`.",
        file=sys.stderr,
    )
    sys.exit(1)
PY
