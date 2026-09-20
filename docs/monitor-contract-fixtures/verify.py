#!/usr/bin/env python3
"""Guard the consumed monitor field subset and event-kind registry."""
# trace:STORY-1352 | ai:codex

import json
import os
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
FIXTURES = pathlib.Path(__file__).resolve().parent


def semver(value):
    match = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)", value)
    if not match:
        raise SystemExit(f"invalid monitor contract semver: {value}")
    return tuple(map(int, match.groups()))


def load_fixtures(directory):
    result = {}
    for path in sorted(directory.glob("*.json")):
        value = json.loads(path.read_text())
        result[value["name"]] = value
    return result


def compatibility_errors(old, current, old_version, new_version):
    breaking = []
    additive = []
    for name, fixture in old.items():
        now = current.get(name)
        if now is None:
            breaking.append(f"surface {name}")
            continue
        for field, kind in fixture["fields"].items():
            if field not in now["fields"] or now["fields"][field] != kind:
                breaking.append(f"{name}.{field}")
        additive.extend(f"{name}.{f}" for f in now["fields"] if f not in fixture["fields"])
    errors = []
    if breaking and new_version[0] <= old_version[0]:
        errors.append("major version bump required for: " + ", ".join(breaking))
    if additive and new_version[:2] <= old_version[:2]:
        errors.append("minor version bump required for: " + ", ".join(additive))
    return errors


def main():
    exe = pathlib.Path(os.environ.get("AIDA_BIN", ROOT / "target" / "debug" / "aida"))
    live = json.loads(subprocess.check_output([exe, "contract", "--json"], text=True))
    current = load_fixtures(FIXTURES)
    advertised = {item["name"]: item for item in live["surfaces"]}
    errors = []
    for name, fixture in current.items():
        if name not in advertised:
            errors.append(f"surface removed: {name}")
            continue
        actual = {field["path"]: field["type"] for field in advertised[name]["fields"]}
        for field, kind in fixture["fields"].items():
            if field not in actual:
                errors.append(f"{name}: covered field removed: {field}")
            elif actual[field] != kind:
                errors.append(f"{name}: covered field type changed: {field} ({kind} -> {actual[field]})")

    source = (ROOT / "aida-cli-lib/src/events.rs").read_text()
    block = source.split("pub fn known_names()", 1)[1].split("{", 1)[1].split("]", 1)[0]
    known = re.findall(r'"([A-Z][A-Za-z0-9]+)"', block)
    expected = current["events-follow"]["event_kinds"]
    if known != expected:
        errors.append(f"events-follow: EventKind::known_names drift: expected {expected}, got {known}")

    if len(sys.argv) == 2:
        base = sys.argv[1]
        exists = subprocess.run(
            ["git", "cat-file", "-e", f"{base}:aida-cli-lib/src/monitor_contract.rs"],
            cwd=ROOT,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        ).returncode == 0
        if not exists:
            print(f"monitor contract is new relative to {base}; no prior version to compare")
            base = None
    if len(sys.argv) == 2 and base is not None:
        names = subprocess.check_output(
            ["git", "ls-tree", "-r", "--name-only", base, "docs/monitor-contract-fixtures"],
            cwd=ROOT,
            text=True,
        ).splitlines()
        old = {}
        for name in names:
            if not name.endswith(".json"):
                continue
            value = json.loads(subprocess.check_output(["git", "show", f"{base}:{name}"], cwd=ROOT, text=True))
            old[value["name"]] = value
        old_source = subprocess.check_output(
            ["git", "show", f"{base}:aida-cli-lib/src/monitor_contract.rs"], cwd=ROOT, text=True
        )
        version_match = re.search(r'pub const VERSION: &str = "([^"]+)"', old_source)
        if not version_match:
            raise SystemExit("base monitor contract version not found")
        old_version = semver(version_match.group(1))
        new_version = semver(live["version"])
        errors.extend(compatibility_errors(old, current, old_version, new_version))

    if errors:
        print("monitor contract drift:\n- " + "\n- ".join(errors), file=sys.stderr)
        return 1
    print(f"monitor contract {live['version']}: {len(current)} surfaces verified")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
