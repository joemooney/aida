#!/usr/bin/env python3
"""Validate portability allowlist growth against a base revision."""

import argparse
from collections import Counter
import json
from pathlib import Path
import sys


def rules(path: Path) -> set[str]:
    return {item["id"] for item in json.loads(path.read_text())}


def entries(path: Path) -> set[tuple[str, str]]:
    result = set()
    for raw in path.read_text().splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        rule, separator, record = raw.partition("\t")
        if not separator or not rule.strip() or not record.strip():
            raise ValueError(f"{path}: invalid allowlist row (expected rule-id<TAB>record): {raw}")
        result.add((rule.strip(), record.strip()))
    return result


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("base_allowlist", type=Path)
    parser.add_argument("base_rules", type=Path)
    parser.add_argument("current_allowlist", type=Path)
    parser.add_argument("current_rules", type=Path)
    args = parser.parse_args()

    try:
        base_entries = entries(args.base_allowlist)
        current_entries = entries(args.current_allowlist)
        base_rules = rules(args.base_rules)
        current_rules = rules(args.current_rules)
    except (OSError, ValueError, KeyError, json.JSONDecodeError) as error:
        print(f"error: cannot validate portability allowlist growth: {error}", file=sys.stderr)
        return 1

    new_rule_ids = current_rules - base_rules
    # trace:TASK-3428 | ai:codex
    # Only retired rows can fund relocation; retained rows cannot excuse copies.
    # Keep the rule and exact source content, dropping only the path prefix.
    def fingerprint(entry):
        rule, record = entry
        return rule, record.split(":", 1)[-1]

    retired = Counter(fingerprint(entry) for entry in base_entries - current_entries)
    rejected = []
    for entry in sorted(current_entries - base_entries):
        if entry[0] in new_rule_ids:
            continue
        key = fingerprint(entry)
        if retired[key]:
            retired[key] -= 1
        else:
            rejected.append(entry)
    if not rejected:
        return 0

    if new_rule_ids:
        print(
            "error: portability allowlist additions attributed to pre-existing rules are not allowed; "
            "only findings from newly added rules may be baselined:",
            file=sys.stderr,
        )
    else:
        print(
            "error: portability rules are unchanged, so the allowlist may not grow; "
            "make the code portable or remove the new allowlist entries:",
            file=sys.stderr,
        )
    for rule, record in rejected:
        print(f"  {rule}\t{record}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
