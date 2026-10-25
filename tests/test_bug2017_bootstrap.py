#!/usr/bin/env python3
"""Ordinary WORK bootstrap, restart, MCP, and synthetic HOME promotion contract.
trace:BUG-2017 | ai:codex
"""
import concurrent.futures
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import tomllib
import yaml
from unittest.mock import patch
from test_mcp_stdio import McpClient, content_text, mint_seat_grant


def main():
    binary = Path(sys.argv[1]).resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="bug2017-bootstrap-") as tmp:
        base = Path(tmp)
        home = base / "private-home"
        home.mkdir()
        env = {k: v for k, v in os.environ.items() if not k.startswith(("AIDA_", "GIT_", "XDG_", "CLAUDE", "CODEX", "AGY_"))}
        env.update(HOME=str(home), AIDA_HOME=str(home), XDG_CONFIG_HOME=str(home / ".config"), XDG_CACHE_HOME=str(home / ".cache"), USER="bootstrap-fixture", NO_COLOR="1", AIDA_BG_FETCH="false", AIDA_AUTO_PUSH="false")
        root = base / "work"
        root.mkdir()

        def run(*args, cwd=root, ok=True):
            result = subprocess.run([str(binary), *args], cwd=cwd, env=env, text=True, capture_output=True, timeout=90)
            assert (result.returncode == 0) == ok, (args, result.returncode, result.stdout, result.stderr)
            return result

        def git(*args, cwd=root):
            return subprocess.run(["git", "-c", "maintenance.auto=false", "-c", "gc.auto=0", *map(str, args)], cwd=cwd, env=env, text=True, capture_output=True, check=True).stdout.strip()

        git("init", "-q", "-b", "main")
        git("config", "user.name", "Synthetic Test")
        git("config", "user.email", "test@example.invalid")
        git("commit", "-q", "--allow-empty", "-m", "init")
        run("init", "--no-skills", "--no-hooks")
        common = root / ".git"
        binding = tomllib.loads((common / "aida-work-node.toml").read_text())
        assert set(binding) == {"version", "node_id"} and binding["version"] == 1
        node = binding["node_id"]
        assert re.fullmatch("[a-f0-9]{32}", node)
        assert not (common / "aida-allocation.toml").exists()
        store = root / ".aida-store"
        assert not (store / "registry/nodes.toml").exists()
        assert not (store / ".aida/node.toml").exists()

        def added(text, prefix):
            matches = re.findall(rf"{prefix}-{node}-[0-9]+", text, flags=re.I)
            assert matches, text
            return matches[-1]

        first = added(run("add", "--title", "generic", "--type", "functional", "--status", "draft").stdout, "FR")
        bug = added(run("add", "--title", "defect", "--type", "bug", "--status", "draft").stdout, "BUG")
        assert first.endswith("001") and bug.endswith("001"), (first, bug)
        def rows():
            return {str(row["id"]): row for path in (store / "objects").rglob("*.yaml")
                    if (row := yaml.safe_load(path.read_text()))}

        before_mcp = rows()
        assert any(row.get("spec_id", "").startswith("META-") for row in before_mcp.values())
        first_uuid = next(key for key, row in before_mcp.items() if row.get("spec_id") == first)
        bug_uuid = next(key for key, row in before_mcp.items() if row.get("spec_id") == bug)
        with patch.dict(os.environ, env, clear=True):
            client = McpClient(binary, root, 60)
            try:
                client.request("initialize", {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "bootstrap-contract", "version": "1"}})
                descriptors = client.request("tools/list")["result"]["tools"]
                id_patterns = [field["pattern"] for tool in descriptors
                               for field in tool.get("inputSchema", {}).get("properties", {}).values()
                               if field.get("pattern", "").startswith("^[A-Z]+-")]
                assert len(id_patterns) >= 10
                for pattern in id_patterns:
                    assert re.fullmatch(pattern, f"BUG-{node}-001"), pattern
                    for valid in ["BUG-001", "BUG-1-2-003", "BUG-team_a--west-003"]:
                        assert re.fullmatch(pattern, valid), (pattern, valid)
                    for invalid in ["other#BUG-001", "BUG-../001", "BUG-_bad-001", "BUG-001/other"]:
                        assert not re.fullmatch(pattern, invalid), (pattern, invalid)
                response = client.tool("add_requirement", {"title": "MCP defect", "description": "native WORK MCP allocation", "type": "bug", "status": "draft"})
                assert added(content_text(response), "BUG").endswith("002")
                assert set(before_mcp) <= set(rows()), "seeded/CLI UUID lost after MCP BUG save"
                generic = client.tool("add_requirement", {"title": "MCP generic", "description": "native WORK generic allocation", "type": "functional", "status": "draft"})
                assert added(content_text(generic), "FR").endswith("002")
                assert set(before_mcp) <= set(rows()), "seeded/CLI UUID lost after MCP FR save"
                client.tool("add_relationship", {"spec_id": bug, "relationship_type": "references", "target_spec_id": first})
                assert set(before_mcp) <= set(rows()), "seeded/CLI UUID lost after MCP relationship save"
            finally:
                client.close()
        assert added(run("add", "--title", "restart defect", "--type", "bug", "--status", "draft").stdout, "BUG").endswith("003")
        reloaded = rows()
        assert set(before_mcp) <= set(reloaded), "seeded META/CLI UUID lost during MCP writes"
        assert reloaded[first_uuid]["spec_id"] == first
        assert reloaded[bug_uuid]["spec_id"] == bug
        assert any(str(edge["target_id"]) == first_uuid and edge["rel_type"].lower() == "references"
                   for edge in reloaded[bug_uuid].get("relationships", [])), reloaded[bug_uuid]
        run("node", "acquire")
        assert tomllib.loads((common / "aida-work-node.toml").read_text()) == binding
        # Concurrent fresh CLI processes use the same persistent allocator.
        with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
            ids = list(pool.map(lambda n: added(run("add", "--title", f"parallel {n}", "--type", "bug", "--status", "draft").stdout, "BUG"), range(8)))
        assert len(set(ids)) == 8, ids
        assert sorted(int(i.rsplit("-", 1)[1]) for i in ids) == list(range(4, 12))
        # Copied tracked claims cannot change effective clone-local authority.
        (root / ".aida/allocation.toml").write_text("version=1\nauthoritative=true\n")
        (store / "registry").mkdir(exist_ok=True)
        (store / "registry/allocation.toml").write_text("version=1\nauthoritative=true\n")

        def snapshot():
            paths = list((store / "objects").rglob("*.yaml")) + [store / "metadata.yaml", common / "aida-work-dispenser.toml"]
            paths += [p for p in [store / "oplog.yaml", store / "registry/blocks.yaml"] if p.exists()]
            return {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in paths}, git("show-ref")

        before = snapshot()
        run("db", "block", "claim", "--type", "BUG", "--size", "10", ok=False)
        run("db", "merge-gate", ok=False)
        run("config", "allocation", "enroll-home", ok=False)
        assert snapshot() == before
        assert run("config", "allocation", "show").stdout.strip() == "WORK (node-local)"
        run("comment", "add", bug, "preserved comment trace:BUG-2017")
        # WORK -> HUB is ordinary Git ref transport; it allocates no IDs.
        hub = base / "hub.git"
        git("init", "--bare", "-q", hub)
        git("push", str(hub), "aida-store:refs/heads/aida-store")
        assert not (hub / "aida-allocation.toml").exists()
        original = {p.relative_to(store / "objects"): p.read_text() for p in (store / "objects").rglob("*.yaml")}
        # Enrollment transition on the SAME previously WORK clone must also work.
        # Keep its opaque binding/counter and old native objects intact.
        (common / "aida-allocation.toml").write_text("version=1\nauthoritative=true\n")
        run("add", "--title", "after explicit synthetic HOME transition", "--type", "task", "--status", "draft")
        assert tomllib.loads((common / "aida-work-node.toml").read_text()) == binding
        run("show", bug)
        home_clone = base / "synthetic-home"
        git("clone", "-q", "--branch", "aida-store", str(hub), home_clone)
        git("config", "user.name", "Synthetic HOME", cwd=home_clone)
        git("config", "user.email", "home@example.invalid", cwd=home_clone)
        # Exact owned temporary clone only: never production enrollment.
        (home_clone / ".git/aida-allocation.toml").write_text("version=1\nauthoritative=true\n")
        (home_clone / ".aida").mkdir(exist_ok=True)
        (home_clone / ".aida/config.toml").write_text('store_path = "."\n')
        with patch.dict(os.environ, env, clear=True):
            env.update(mint_seat_grant(home_clone, "advisor"))
        run("--file", str(home_clone), "db", "merge-gate", cwd=home_clone)
        for path, text in original.items():
            promoted = (home_clone / "objects" / path).read_text()
            # UUID, native ID, comments, relationship and trace content retained.
            assert "agreed_id:" in promoted, path
            before_row, after_row = yaml.safe_load(text), yaml.safe_load(promoted)
            # Promotion records write provenance; every semantic object field survives.
            for key in set(before_row) | set(after_row):
                if key not in {"agreed_id", "filing_provenance", "history", "modified_at"}:
                    assert before_row.get(key) == after_row.get(key), (path, key)
            old_history = before_row.get("history", [])
            new_history = after_row.get("history", [])
            assert new_history[:len(old_history)] == old_history, path
            assert all(not entry.get("changes") for entry in new_history[len(old_history):]), path
            assert after_row["modified_at"] == before_row["modified_at"], path
        for native in [first, bug]:
            run("--file", str(home_clone), "show", native, cwd=home_clone)
        before_retire = {p: p.read_bytes() for p in (home_clone / "objects").rglob("*.yaml")}
        run("--file", str(home_clone), "db", "retire-legacy-ids", cwd=home_clone, ok=False)
        assert all(p.read_bytes() == value for p, value in before_retire.items())
        run("--file", str(home_clone), "show", bug, cwd=home_clone)
        promoted = {p: p.read_bytes() for p in (home_clone / "objects").rglob("*.yaml")}
        promoted_ref = git("rev-parse", "HEAD", cwd=home_clone)
        counter_bytes = (home_clone / "registry/agreed_counters.toml").read_bytes()
        run("--file", str(home_clone), "db", "merge-gate", cwd=home_clone)
        assert all(p.read_bytes() == value for p, value in promoted.items())
        assert (home_clone / "registry/agreed_counters.toml").read_bytes() == counter_bytes
        assert git("rev-parse", "HEAD", cwd=home_clone) == promoted_ref
        legacy = base / "legacy"
        legacy.mkdir()
        git("init", "-q", "-b", "main", cwd=legacy)
        git("config", "user.name", "Legacy fixture", cwd=legacy)
        git("config", "user.email", "legacy@example.invalid", cwd=legacy)
        git("commit", "-q", "--allow-empty", "-m", "init", cwd=legacy)
        run("init", "--no-skills", "--no-hooks", cwd=legacy)
        legacy_store = legacy / ".aida-store"
        for filename in ["aida-work-node.toml", "aida-work-dispenser.toml"]:
            (legacy / ".git" / filename).unlink()
        (legacy_store / ".aida/node.toml").write_text("node_id='7'\nuser_id=0\nhostname=''\nregistered_at='2026-01-01T00:00:00Z'\n")
        legacy_counter = legacy_store / ".aida/dispenser.toml"
        old_bytes = b"[mode.Distributed]\nnode_id='7'\n[sequences]\nBUG=41\n"
        legacy_counter.write_bytes(old_bytes)
        node_file = legacy_store / ".aida/node.toml"
        valid_node = node_file.read_bytes()

        def legacy_snapshot():
            paths = list((legacy_store / "objects").rglob("*.yaml"))
            paths += [legacy_store / name for name in ["metadata.yaml", "oplog.yaml", ".aida/node.toml", ".aida/dispenser.toml"]]
            paths += [legacy / ".git" / name for name in ["aida-work-node.toml", "aida-work-dispenser.toml"]]
            return {str(p): p.read_bytes() if p.exists() else None for p in paths}, git("show-ref", cwd=legacy)

        for bad_node, bad_counter in [
            (valid_node, old_bytes.replace(b"node_id='7'", b"node_id='8'")),
            (valid_node, b"mode='Centralized'\n[sequences]\nBUG=41\n"),
            (b"node_id='7'\nhostname=''\n", old_bytes),
            (b"invalid toml !", old_bytes),
            (valid_node, None),
            (None, old_bytes),
        ]:
            for path, value in [(node_file, bad_node), (legacy_counter, bad_counter)]:
                if value is None:
                    path.unlink(missing_ok=True)
                else:
                    path.write_bytes(value)
            before_invalid = legacy_snapshot()
            run("node", "acquire", cwd=legacy, ok=False)
            assert legacy_snapshot() == before_invalid
            run("add", "--title", "invalid legacy migration", "--type", "bug", cwd=legacy, ok=False)
            assert legacy_snapshot() == before_invalid
        node_file.write_bytes(valid_node)
        legacy_counter.write_bytes(old_bytes)
        before_objects = {p: p.read_bytes() for p in (legacy_store / "objects").rglob("*.yaml")}
        before_ref = git("show-ref", cwd=legacy)
        run("db", "block", "claim", "--type", "BUG", "--size", "10", cwd=legacy, ok=False)
        assert legacy_counter.read_bytes() == old_bytes
        assert not (legacy / ".git/aida-work-node.toml").exists()
        assert not (legacy / ".git/aida-work-dispenser.toml").exists()
        assert all(p.read_bytes() == value for p, value in before_objects.items())
        assert git("show-ref", cwd=legacy) == before_ref
        migrated = run("add", "--title", "legacy local admission", "--type", "bug", "--status", "draft", cwd=legacy)
        migrated_node = tomllib.loads((legacy / ".git/aida-work-node.toml").read_text())["node_id"]
        assert f"BUG-{migrated_node}-042" in migrated.stdout
        # Explicit non-Git directories support read-only protocol startup, but
        # never acquire a global/internal fallback allocator.
        raw_store = base / "raw-read-only-store"
        raw_store.mkdir()
        requests = [
            {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "raw-contract", "version": "1"}}},
            {"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "add_requirement", "arguments": {"title": "must refuse", "description": "no Git identity", "type": "bug", "status": "draft"}}},
        ]
        raw_env = {k: v for k, v in env.items() if k != "AIDA_SESSION_GRANT"}
        raw = subprocess.run([str(binary), "--file", str(raw_store), "mcp-serve"], cwd=base, env=raw_env,
                             input="".join(json.dumps(r) + "\n" for r in requests), text=True, capture_output=True, timeout=30)
        assert raw.returncode == 0, (raw.stdout, raw.stderr)
        responses = [json.loads(line) for line in raw.stdout.splitlines() if line.strip()]
        assert responses[0]["result"]["serverInfo"]
        assert responses[1]["result"]["isError"]
        assert "requirement ID allocation failed" in content_text(responses[1]["result"])
        assert responses[1]["result"]["structuredError"]["code"] == "internal"
        assert not list(raw_store.rglob("*.yaml"))
        assert not list(raw_store.rglob("*dispenser*"))
        print("PASS ordinary WORK init/CLI/MCP/restart/concurrency/ref-only HUB/HOME aliases/idempotency/legacy refusal/raw MCP read-only")


if __name__ == "__main__":
    main()
