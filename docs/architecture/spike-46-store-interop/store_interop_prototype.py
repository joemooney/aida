#!/usr/bin/env python3
"""SPIKE-46 non-AIDA store interop prototype.

This is deliberately a small Python tool, not an AIDA CLI wrapper. It touches
the git-canonical store format directly to test the multi-vendor thesis:

1. READ is easy: stock YAML plus a tiny unknown-tag adapter can parse the live
   `.aida-store/objects/**.yaml` corpus and reach fields agents care about.
2. WRITE is bounded: a non-AIDA writer should not re-emit whole objects with a
   generic YAML emitter. This prototype implements one safe mutation shape
   instead: top-level status flip + modified_at update + history entry append.

Default writes are sandboxed into a temporary copy of the selected object. Pass
`--apply` to mutate the real store object in place.

trace:SPIKE-46 | ai:codex
"""

from __future__ import annotations

import argparse
import difflib
import glob
import os
import re
import shutil
import sys
import tempfile
import uuid
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

try:
    import yaml
except ImportError:
    sys.exit("pyyaml required: pip install pyyaml")


VALID_STATUSES = {
    "Draft",
    "Approved",
    "Planned",
    "InProgress",
    "Done",
    "Completed",
    "Rejected",
    "NeedsAttention",
}

STATUS_ALIASES = {
    "draft": "Draft",
    "approved": "Approved",
    "planned": "Planned",
    "in-progress": "InProgress",
    "in_progress": "InProgress",
    "inprogress": "InProgress",
    "done": "Done",
    "completed": "Completed",
    "rejected": "Rejected",
    "needs-attention": "NeedsAttention",
    "needs_attention": "NeedsAttention",
    "needsattention": "NeedsAttention",
}


class AidaLoader(yaml.SafeLoader):
    """SafeLoader variant that preserves unknown YAML tags as data."""


def unknown_tag(loader: AidaLoader, tag_suffix: str, node: yaml.Node) -> Any:
    tag = tag_suffix.lstrip("!")
    if isinstance(node, yaml.ScalarNode):
        value = loader.construct_scalar(node)
        return {tag: value} if tag else value
    if isinstance(node, yaml.SequenceNode):
        return {tag: loader.construct_sequence(node)}
    if isinstance(node, yaml.MappingNode):
        return {tag: loader.construct_mapping(node)}
    return {tag: None}


AidaLoader.add_multi_constructor("!", unknown_tag)


@dataclass(frozen=True)
class StoreObject:
    path: Path
    raw: str
    doc: dict[str, Any]


def default_store() -> Path:
    here = Path(__file__).resolve()
    return here.parents[3] / ".aida-store"


def object_paths(store: Path) -> list[Path]:
    return sorted(Path(p) for p in glob.glob(str(store / "objects" / "**" / "*.yaml"), recursive=True))


def load_object(path: Path) -> StoreObject:
    raw = path.read_text(encoding="utf-8")
    doc = yaml.load(raw, Loader=AidaLoader)
    if not isinstance(doc, dict):
        raise ValueError(f"{path}: expected mapping at document root")
    return StoreObject(path=path, raw=raw, doc=doc)


def load_all(store: Path) -> tuple[list[StoreObject], list[tuple[Path, str]]]:
    ok: list[StoreObject] = []
    failed: list[tuple[Path, str]] = []
    for path in object_paths(store):
        try:
            ok.append(load_object(path))
        except Exception as exc:  # noqa: BLE001 - prototype reports all parse classes
            failed.append((path, repr(exc)))
    return ok, failed


def normalize_status(raw: str) -> str:
    status = STATUS_ALIASES.get(raw.strip().lower(), raw.strip())
    if status not in VALID_STATUSES:
        valid = ", ".join(sorted(VALID_STATUSES))
        raise ValueError(f"invalid status {raw!r}; expected one of: {valid}")
    return status


def find_spec(store: Path, spec_id: str) -> StoreObject:
    matches = [obj for obj in load_all(store)[0] if obj.doc.get("spec_id") == spec_id]
    if len(matches) != 1:
        raise ValueError(f"expected exactly one object for {spec_id}, found {len(matches)}")
    return matches[0]


def relationship_kind(value: Any) -> str:
    if isinstance(value, dict) and value:
        key = next(iter(value))
        return f"{key}:{value[key]}"
    return str(value)


def read_report(store: Path) -> int:
    objects, failures = load_all(store)
    relationship_kinds: set[str] = set()
    field_reach = 0
    for obj in objects:
        if {"id", "spec_id", "status", "req_type"}.issubset(obj.doc):
            field_reach += 1
        for rel in obj.doc.get("relationships") or []:
            if isinstance(rel, dict):
                relationship_kinds.add(relationship_kind(rel.get("rel_type")))

    print(f"Store: {store}")
    print(f"Objects parsed: {len(objects)}/{len(objects) + len(failures)}")
    print(f"Objects with id/spec_id/status/req_type reachable: {field_reach}/{len(objects)}")
    print(f"Relationship kinds observed: {', '.join(sorted(relationship_kinds)) or '(none)'}")
    if failures:
        print("\nParse failures:")
        for path, err in failures[:20]:
            print(f"  {path}: {err}")
        return 1
    return 0


def first_top_level_index(lines: list[str], key: str) -> int | None:
    pattern = re.compile(rf"^{re.escape(key)}:\s*")
    for idx, line in enumerate(lines):
        if pattern.match(line):
            return idx
    return None


def append_history(raw: str, *, old_status: str, new_status: str, author: str, timestamp: str) -> str:
    lines = raw.splitlines()
    history_idx = first_top_level_index(lines, "history")
    entry = [
        f"- id: {uuid.uuid4()}",
        f"  author: {author}",
        f"  timestamp: {timestamp}",
        "  changes:",
        "  - field_name: status",
        f"    old_value: {old_status}",
        f"    new_value: {new_status}",
    ]
    if history_idx is None:
        if lines and lines[-1].strip():
            lines.append("history:")
        else:
            lines[-1:] = ["history:"]
    lines.extend(entry)
    return "\n".join(lines) + "\n"


def patch_status(raw: str, *, new_status: str, author: str) -> tuple[str, str]:
    doc = yaml.load(raw, Loader=AidaLoader)
    if not isinstance(doc, dict):
        raise ValueError("expected mapping at document root")
    old_status = str(doc.get("status", ""))
    if not old_status:
        raise ValueError("object has no top-level status")
    if old_status == new_status:
        raise ValueError(f"status is already {new_status}")

    timestamp = datetime.now(timezone.utc).isoformat(timespec="microseconds").replace("+00:00", "Z")
    lines = raw.splitlines()

    status_idx = first_top_level_index(lines, "status")
    if status_idx is None:
        raise ValueError("could not locate top-level status line")
    lines[status_idx] = f"status: {new_status}"

    modified_idx = first_top_level_index(lines, "modified_at")
    if modified_idx is None:
        # Insert after created_at when possible, otherwise after status.
        created_idx = first_top_level_index(lines, "created_at")
        insert_at = (created_idx + 1) if created_idx is not None else (status_idx + 1)
        lines.insert(insert_at, f"modified_at: {timestamp}")
    else:
        lines[modified_idx] = f"modified_at: {timestamp}"

    patched = "\n".join(lines) + "\n"
    return append_history(
        patched,
        old_status=old_status,
        new_status=new_status,
        author=author,
        timestamp=timestamp,
    ), old_status


def diff_text(before: str, after: str, fromfile: str, tofile: str) -> str:
    return "".join(
        difflib.unified_diff(
            before.splitlines(keepends=True),
            after.splitlines(keepends=True),
            fromfile=fromfile,
            tofile=tofile,
        )
    )


def write_status(store: Path, spec_id: str, status: str, author: str, apply: bool) -> int:
    new_status = normalize_status(status)
    source = find_spec(store, spec_id)

    target_path = source.path
    cleanup: tempfile.TemporaryDirectory[str] | None = None
    if not apply:
        cleanup = tempfile.TemporaryDirectory(prefix="aida-store-interop-")
        sandbox = Path(cleanup.name)
        rel = source.path.relative_to(store)
        target_path = sandbox / rel
        target_path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source.path, target_path)
        print(f"Sandbox write target: {target_path}")
        print("Pass --apply to mutate the real store object.")

    target = load_object(target_path)
    patched, old_status = patch_status(target.raw, new_status=new_status, author=author)
    target_path.write_text(patched, encoding="utf-8")

    reparsed = load_object(target_path)
    if reparsed.doc.get("status") != new_status:
        raise RuntimeError("patched object did not reparse with requested status")
    history = reparsed.doc.get("history") or []
    if not isinstance(history, list) or not history:
        raise RuntimeError("patched object did not reparse with a history entry")

    print(f"Status flip: {spec_id} {old_status} -> {new_status}")
    print(f"Write mode: {'REAL STORE' if apply else 'sandbox copy'}")
    print()
    print(diff_text(target.raw, patched, str(source.path), str(target_path)))

    if cleanup is not None:
        cleanup.cleanup()
    return 0


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--store", type=Path, default=default_store(), help="Path to .aida-store")
    sub = parser.add_subparsers(dest="command", required=True)

    sub.add_parser("read-report", help="Parse every object and report field reachability")

    write = sub.add_parser("write-status", help="Bounded direct-YAML status flip prototype")
    write.add_argument("spec_id", help="Spec ID to patch, e.g. SPIKE-46")
    write.add_argument("status", help="New status, e.g. InProgress or approved")
    write.add_argument("--author", default="codex-store-interop", help="History author")
    write.add_argument("--apply", action="store_true", help="Mutate the real store object")

    args = parser.parse_args(argv)
    store = args.store.resolve()
    if args.command == "read-report":
        return read_report(store)
    if args.command == "write-status":
        return write_status(store, args.spec_id, args.status, args.author, args.apply)
    raise AssertionError(args.command)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
