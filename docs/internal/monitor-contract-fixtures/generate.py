#!/usr/bin/env python3
"""Refresh event kinds without broadening the reviewed monitor field subset."""
# trace:TASK-3119 | ai:codex

import json
import pathlib
import re


def main():
    fixtures = pathlib.Path(__file__).resolve().parent
    root = fixtures.parents[2]
    source = (root / "aida-cli-lib/src/events.rs").read_text()
    block = source.split("pub fn known_names()", 1)[1].split("{", 1)[1].split("]", 1)[0]
    kinds = re.findall(r'"([A-Z][A-Za-z0-9]+)"', block)
    if not kinds or len(kinds) != len(set(kinds)):
        raise SystemExit("refusing empty or duplicate event-kind registry")
    path = fixtures / "events-follow.json"
    fixture = json.loads(path.read_text())
    fixture["event_kinds"] = kinds
    path.write_text(json.dumps(fixture, separators=(",", ":")) + "\n")


if __name__ == "__main__":
    main()
