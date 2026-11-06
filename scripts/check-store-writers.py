#!/usr/bin/env python3
"""Conservative source-review ratchet for STORY-2561; never authorizes a writer.

Inventory updates are deliberate review work, not a --bless-baseline operation.
See docs/internal/security/store-mutation-writers.md for audit scope and limits.
"""
# trace:TASK-3222 | ai:codex
from __future__ import annotations

import argparse
import bisect
import hashlib
import json
from pathlib import Path
import re
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]
INVENTORY = "scripts/store-writers.toml"
# Include adjacent REST/TUI consumers, not just the two primary crates.
RUST_ROOTS = ("aida-core/src", "aida-cli-lib/src", "aida-cli/src", "aida-server/src", "aida-tui/src", "aida-desktop/src", "aida-crate/src", "aida-generate-types/src")
SCRIPT_ROOTS = ("scripts", "hooks", ".githooks")
# Deliberately over-inclusive: a filesystem destination need not contain 'store'.
APIS = (
    r"(?:fs::|File::|OpenOptions::|tokio::fs::)(?:write|rename|remove_file|remove_dir_all|create_dir_all|create|copy|hard_link|symlink|new)",
    r"\b(?:write_atomic|atomic_write|write_object(?:_if_changed)?|delete_object|with_store_write_lock|with_sync_lock|with_dispense_lock)\b",
    r"\b(?:save(?:_reporting(?:_with_origin)?|_imported|_unstamped|_with_origin|_with_commit_subject|_with_conflict_detection)?|update_atomically(?:_tracked)?|update_spec_atomically(?:_with_subject)?|bulk_update(?:_atomically)?|add_requirement(?:_atomic|_with_id)?|update_requirement(?:_versioned)?|delete_requirement|queue_(?:add|remove\w*|reorder|clear)|lock_store|lock_queue_write)\s*(?=\()",
    r"\b(?:execute|execute_batch|transaction|transaction_with_behavior|unchecked_transaction|write_all|write_fmt|set_len|persist|persist_noclobber|truncate|create_new)\s*(?=\()",
    r"\b(?:write_cas\w*|write_batch_cas\w*|write_local|write_batch_local|delete_ledger|set_role_cas|set_allowed_seats_cas|allow_seat_cas|disallow_seat_cas|update_seats_cas|unset_role_cas|link_cas|register_node\w*|set_node_identity_field|hijack_node|unregister_node|acquire_claim|release_claim|release_stale_foreign_claim|acquire_lock_claim|heartbeat_lock_claim|release_lock_claim|claim_first_block|auto_allocate_block_inner|rollback_unpushed_claim|digest_local_to_canonical|digest_snapshot_to_canonical|write_snapshot_locked|publish_to_store|commit_record|commit_child_store_writes|commit_store_writes_since)\s*(?=\()",
    r"\b(?:pull_rebase\w*|merge_union_auto|fast_forward_store|merge_gate(?:_locked)?|sync_objects|commit_pending_store_changes_on_host|rollback_commit_if_head_matches|squash_orphan_to_snapshot|opportunistic_store_gc|gc_aggressive|push_exact(?:_outcome)?)\s*(?=\()",
    r"\bgit_ops::(?:add|add_all|commit|commit_paths|push|init|configure_user)\s*(?=\()",
)
SEED = re.compile("|".join(APIS))
FUNCTION = re.compile(r"\bfn\s+(\w+)\s*(?:<[^;{}]*>)?\s*\(")
# Strings are retained separately for shell/SQL dispatch evidence. Source bodies
# are hashed too: replacing a destination or argument is a reviewable change.
SCRIPT_WRITE = re.compile(r"\b(?:write_text|write_bytes|write|rename|replace|unlink|mkdir|makedirs|rmtree|copyfile|copy2|execute|executemany|commit|open|dump|safe_dump)\s*\(|\b(?:aida\s+(?:add|edit|comment|queue|claim|mailbox|db|sync|finding)|git\s+(?:add|commit|push|update-ref|merge|rebase)|(?:mv|cp|rm|mkdir|tee|install)\s)|(?<![>=])>>(?![>=])|(?<![>=])>(?![>=])")
DISPOSITIONS = {"s2_typed", "host_only", "lane_private", "read_only", "blocked"}
ARTIFACTS = {"canonical_objects", "store_metadata", "authority", "cache", "local_coordination", "store_git", "external_output", "unresolved"}


def blank(text: str) -> str:
    return re.sub(r"[^\n]", " ", text)


def mask_rust(source: str) -> str:
    """Offset-preserving comment/string mask, including nested block comments."""
    token = re.compile(r'//|/\*|(?<!\w)(?:b|c)?r(#*)"|"|\'(?:\\.|[^\'\\\n])\'')
    out, pos = [], 0
    while match := token.search(source, pos):
        out.append(source[pos:match.start()])
        start, kind = match.start(), match.group()
        if kind == "//":
            end = source.find("\n", match.end())
            end = len(source) if end < 0 else end
        elif kind == "/*":
            end, depth = match.end(), 1
            while depth:
                sub = re.search(r"/\*|\*/", source[end:])
                if not sub:
                    raise ValueError("unterminated Rust comment")
                depth += 1 if sub.group() == "/*" else -1
                end += sub.end()
        elif match.group(1) is not None:
            close = '"' + match.group(1)
            end = source.find(close, match.end())
            if end < 0:
                raise ValueError("unterminated Rust raw string")
            end += len(close)
        elif kind == '"':
            end = match.end()
            while end < len(source) and source[end] != '"':
                end += 2 if source[end] == "\\" else 1
            end += 1
        else:
            end = match.end()
        out.append(blank(source[start:end]))
        pos = end
    return "".join(out) + source[pos:]


def block_end(masked: str, start: int) -> int:
    depth = 0
    for at in range(start, len(masked)):
        if masked[at] == "{":
            depth += 1
        elif masked[at] == "}":
            depth -= 1
            if depth == 0:
                return at + 1
    raise ValueError("unclosed Rust item; audit syntax before inventorying")


def production(path: str, source: str) -> tuple[str, str]:
    """Remove only test items, never the rest of a mixed production/test file."""
    if path.endswith(".rs"):
        masked = mask_rust(source)
        spans = []
        for match in re.finditer(r"#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]", masked):
            tail = re.match(r"(?:\s*#\[[^\]]*\])*\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:mod|fn)\b", masked[match.end():])
            if not tail:
                continue  # cfg(test) fields/arms retained conservatively
            start = masked.find("{", match.end())
            semi = masked.find(";", match.end())
            end = semi + 1 if 0 <= semi < start or start < 0 else block_end(masked, start)
            spans.append((match.start(), end))
        for start, end in reversed(spans):
            source = source[:start] + blank(source[start:end]) + source[end:]
            masked = masked[:start] + blank(masked[start:end]) + masked[end:]
        return source, masked
    return source, source


def sources(root: Path) -> dict[str, tuple[str, str]]:
    paths = set()
    # Discover new workspace-style crate roots too; a new crate cannot silently
    # escape the ratchet just because it was not in the initial audit.
    rust_roots = set(RUST_ROOTS) | {p.relative_to(root).as_posix() for p in root.glob("aida-*/src")}
    for directory in sorted(rust_roots):
        for suffix in ("*.rs", "*.py", "*.sh"):
            paths.update((root / directory).rglob(suffix))
    for directory in SCRIPT_ROOTS:
        for suffix in ("*.py", "*.sh"):
            paths.update((root / directory).rglob(suffix))
    result = {}
    for path in sorted(paths):
        rel = path.relative_to(root).as_posix()
        if any(p in {"tests", "test", "fixtures", "__pycache__"} for p in path.relative_to(root).parts) or path.name == "tests.rs" or path.name.startswith("test_") or path.name.endswith("_tests.rs"):
            continue
        if rel == "scripts/check-store-writers.py":
            continue  # read-only guard; covered by its independent unit fixtures
        if path.is_symlink():
            raise ValueError(f"source symlink needs explicit audit: {rel}")
        result[rel] = production(rel, path.read_text())
    return result


def functions(masked: str) -> list[tuple[int, int, str]]:
    result = []
    for match in FUNCTION.finditer(masked):
        start = masked.find("{", match.end())
        semi = masked.find(";", match.end())
        if start < 0 or 0 <= semi < start:
            continue  # trait declaration, not a body
        result.append((match.start(), block_end(masked, start), match.group(1)))
    return result


def discover(root: Path) -> dict[str, dict]:
    files = sources(root)
    parsed = {p: functions(m) for p, (_, m) in files.items() if p.endswith(".rs")}
    # Fixed-point caller reconciliation: learn wrappers from current source,
    # including wrappers of wrappers, never from the checked-in allow-list.
    caller = re.compile(r"\b([A-Za-z_]\w*)\s*(?=\()")
    common = {"new", "default", "from", "run", "main", "open", "read", "get", "init",
              "resolve", "start", "load", "drop", "add", "apply", "allow", "check",
              "clear", "collect", "commit", "contains", "insert", "push", "remove",
              "set", "create", "build", "update", "finish", "stop", "send", "write",
              "delete", "lock"}
    graph, wrappers = {}, set()
    for path, items in parsed.items():
        masked = files[path][1]
        for start, end, name in items:
            if name in common:
                continue
            body = masked[start:end]
            graph.setdefault(name, set()).update(h.group(1) for h in caller.finditer(body))
            if SEED.search(body):
                wrappers.add(name)
    while True:
        expanded = wrappers | {name for name, calls in graph.items() if calls & wrappers}
        if expanded == wrappers:
            break
        wrappers = expanded
    result = {}
    for path, (source, masked) in files.items():
        if path.endswith(".rs"):
            hits = list(SEED.finditer(masked))
            hits += [h for h in caller.finditer(masked) if h.group(1) in wrappers]
        else:
            hits = list(SCRIPT_WRITE.finditer(source))
        dispatch = path in {"aida-cli-lib/src/command_registry.rs", "aida-cli-lib/src/cli.rs", "aida-cli-lib/src/lib.rs", "aida-cli/src/main.rs"}
        if not hits and not dispatch:
            continue
        offsets = sorted(h.start() for h in hits)
        names = sorted({name for start, end, name in parsed.get(path, []) if (at := bisect.bisect_left(offsets, start)) < len(offsets) and offsets[at] < end})
        result[path] = {
            "entrypoints": names or ["<module/script>"],
            "sink": sorted({re.sub(r"\s+", "", h.group()) for h in hits}) or ["<dispatch: follow registered/legacy surfaces>"],
            # Hash all production text, including destination strings and dispatch
            # selectors. New calls in an already inventoried function must fail.
            "source_sha256": hashlib.sha256(source.encode()).hexdigest(),
        }
    return result


def check(root: Path, inventory: Path | None = None) -> list[str]:
    data = tomllib.loads((inventory or root / INVENTORY).read_text())
    if data.get("version") != 1 or not re.fullmatch(r"[0-9a-f]{40}", data.get("source_revision", "")):
        raise ValueError("inventory needs version=1 and an audited 40-digit source_revision")
    rows = data.get("writer", [])
    if not isinstance(rows, list) or not rows:
        raise ValueError("inventory must contain nonempty writer records")
    paths, ids, errors = {}, set(), []
    required = ("id", "path", "surface", "selector", "entrypoints", "sink", "lock", "gate", "artifacts", "disposition", "refusal", "cutover_owner", "evidence", "source_sha256")
    for row in rows:
        if not isinstance(row, dict):
            raise ValueError("writer record must be a table")
        if any(not row.get(key) for key in required):
            errors.append(f"incomplete writer row: {row.get('path', '<missing>')}")
            continue
        path = row["path"]
        if path in paths or row["id"] in ids:
            errors.append(f"duplicate writer id/path: {path}")
        paths[path] = row
        ids.add(row["id"])
        if Path(path).is_absolute() or ".." in Path(path).parts:
            errors.append(f"unsafe path: {path}")
        if row["disposition"] not in DISPOSITIONS or not set(row["artifacts"]) <= ARTIFACTS:
            errors.append(f"invalid classification: {path}")
        if row["disposition"] == "blocked" and not row.get("blocker"):
            errors.append(f"blocked row needs a cutover blocker: {path}")
    current = discover(root)
    for path in sorted(current.keys() | paths.keys()):
        if path not in paths:
            errors.append(f"unclassified writer/caller: {path}")
        elif path not in current:
            errors.append(f"stale writer row: {path}")
        else:
            for field, value in current[path].items():
                if paths[path].get(field) != value:
                    errors.append(f"writer evidence changed ({field}): {path}")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument("--candidates", action="store_true", help="print discovery evidence for manual audit; never rewrites inventory")
    args = parser.parse_args()
    try:
        if args.candidates:
            print(json.dumps(discover(args.root), indent=2, sort_keys=True))
            return 0
        errors = check(args.root)
        if errors:
            print("store-writer inventory refused:\n" + "\n".join(errors), file=sys.stderr)
            return 1
        data = tomllib.loads((args.root / INVENTORY).read_text())
        blocked = sum(row["disposition"] == "blocked" for row in data["writer"])
        print(f"store-writers: {len(data['writer'])} source records reconciled; {blocked} explicit S2 cutover blockers; no runtime authority granted")
        return 0
    except (OSError, ValueError, TypeError, KeyError) as error:
        print(f"store-writer inventory refused: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
