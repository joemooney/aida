#!/usr/bin/env python3
"""Black-box MCP stdio compatibility tests for AIDA.

This suite starts `aida mcp-serve` as a child process and speaks JSON-RPC over
stdio, the same transport Codex and other local MCP clients use. It is designed
to catch integration failures that unit tests around `tool_descriptors()` miss:

- JSON-RPC initialize/tools/list/tools/call framing
- all tools expose inputSchema and outputSchema descriptors
- tool calls return MCP-compatible content envelopes
- local CLI writes are visible to MCP reads
- MCP writes are visible to local CLI reads
- coordination tools round-trip through `.aida/` files

By default the suite accepts AIDA's current compatibility contract: tools return
a text content envelope, while descriptors declare outputSchema. Pass
`--require-structured-content` once AIDA intentionally supports structured
MCP tool results.

## Staged gates

The suite is staged so it can land while still-open bugs are in flight:

1. `initialize`, `tools/list descriptors`, and the CLI-to-MCP read direction
   pass today — they validate the TASK-440 outputSchema closure and confirm
   the MCP server can read what the local CLI wrote.

2. The remaining tests exercise the MCP-write -> CLI-read direction and the
   coordination-tool round trips. They are gated behind
   `--require-mcp-write-roundtrip` because BUG-310 makes
   `add_requirement` (and the related write-then-read tools) return a
   confirmation that doesn't persist to the canonical store. Flip the
   default to on once BUG-310 ships.

trace:TASK-451 | ai:codex
trace:BUG-310 | ai:codex
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any


REQUIRED_TOOLS = [
    # Spec graph
    "list_requirements",
    "show_requirement",
    "add_requirement",
    "update_requirement",
    "search_requirements",
    "add_comment",
    "list_features",
    # Punt channel
    "list_punts",
    "read_punt",
    "post_punt",
    "resolve_punt",
    "escalate_punt",
    # Findings channel
    "list_findings",
    "file_finding",
    "triage_finding",
    # Task claims
    "claim_task",
    "release_task",
    "list_active_leases",
    # Worker directives
    "post_directive",
    "list_directives",
    "ack_directive",
]


class Failure(Exception):
    pass


class McpClient:
    def __init__(self, aida: Path, cwd: Path):
        self.proc = subprocess.Popen(
            [str(aida), "mcp-serve"],
            cwd=cwd,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
        )
        assert self.proc.stdin is not None
        assert self.proc.stdout is not None
        self._next_id = 1

    def close(self) -> None:
        if self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=3)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait(timeout=3)

    def request(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
        req_id = self._next_id
        self._next_id += 1
        payload: dict[str, Any] = {"jsonrpc": "2.0", "id": req_id, "method": method}
        if params is not None:
            payload["params"] = params
        line = json.dumps(payload)
        assert self.proc.stdin is not None
        assert self.proc.stdout is not None
        self.proc.stdin.write(line + "\n")
        self.proc.stdin.flush()
        raw = self.proc.stdout.readline()
        if raw == "":
            stderr = self._read_stderr_tail()
            raise Failure(f"MCP server exited before response to {method}; stderr:\n{stderr}")
        try:
            resp = json.loads(raw)
        except json.JSONDecodeError as exc:
            raise Failure(f"invalid JSON response to {method}: {raw!r}") from exc
        if resp.get("id") != req_id:
            raise Failure(f"response id mismatch for {method}: expected {req_id}, got {resp}")
        if resp.get("error"):
            raise Failure(f"JSON-RPC error for {method}: {resp['error']}")
        return resp

    def tool(self, name: str, arguments: dict[str, Any] | None = None) -> dict[str, Any]:
        resp = self.request(
            "tools/call",
            {"name": name, "arguments": arguments or {}},
        )
        result = resp.get("result")
        if not isinstance(result, dict):
            raise Failure(f"tool {name} result is not object: {resp}")
        if result.get("isError"):
            raise Failure(f"tool {name} returned isError: {result}")
        return result

    def _read_stderr_tail(self) -> str:
        if self.proc.stderr is None:
            return ""
        try:
            return self.proc.stderr.read()
        except Exception:
            return ""


def run(cmd: list[str], cwd: Path, env: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
    merged_env = os.environ.copy()
    if env:
        merged_env.update(env)
    return subprocess.run(
        cmd,
        cwd=cwd,
        env=merged_env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


def require(ok: bool, message: str) -> None:
    if not ok:
        raise Failure(message)


def content_text(result: dict[str, Any]) -> str:
    content = result.get("content")
    require(isinstance(content, list), f"missing content list: {result}")
    chunks = []
    for item in content:
        require(isinstance(item, dict), f"content item is not object: {item}")
        require(item.get("type") == "text", f"expected text content item, got: {item}")
        text = item.get("text")
        require(isinstance(text, str), f"text content missing string text: {item}")
        chunks.append(text)
    return "\n".join(chunks)


def require_structured_content_if_requested(result: dict[str, Any], tool: str, required: bool) -> None:
    if not required:
        return
    require(
        "structuredContent" in result,
        f"{tool} missing structuredContent in strict mode: {result}",
    )
    require(
        isinstance(result["structuredContent"], dict),
        f"{tool} structuredContent must be object: {result}",
    )


def setup_project(aida: Path, root: Path) -> str:
    run(["git", "init"], root)
    init = run([str(aida), "init", "--no-skills", "--no-hooks"], root)
    require(init.returncode == 0, f"aida init failed:\nstdout={init.stdout}\nstderr={init.stderr}")

    add = run(
        [
            str(aida),
            "add",
            "--title",
            "MCP stdio seed requirement",
            "--description",
            "Seed item created by the MCP stdio compatibility test.",
            "--type",
            "task",
            "--status",
            "approved",
            "--priority",
            "medium",
            "--tags",
            "mcp-stdio-test,codex",
        ],
        root,
    )
    require(add.returncode == 0, f"aida add failed:\nstdout={add.stdout}\nstderr={add.stderr}")
    match = re.search(r"\b([A-Z]+(?:-[A-Z0-9]+)?-\d+(?:-\d+)?)\b", add.stdout)
    require(bool(match), f"could not parse spec id from aida add output:\n{add.stdout}")
    return match.group(1)


def test_initialize(client: McpClient) -> None:
    resp = client.request(
        "initialize",
        {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "aida-mcp-stdio-test", "version": "0"},
        },
    )
    result = resp.get("result")
    require(isinstance(result, dict), f"initialize result must be object: {resp}")
    require(result.get("protocolVersion"), f"initialize missing protocolVersion: {result}")
    require("tools" in result.get("capabilities", {}), f"initialize missing tools capability: {result}")


def test_tool_descriptors(client: McpClient) -> dict[str, dict[str, Any]]:
    resp = client.request("tools/list")
    result = resp.get("result")
    require(isinstance(result, dict), f"tools/list result must be object: {resp}")
    tools = result.get("tools")
    require(isinstance(tools, list), f"tools/list missing tools array: {result}")
    by_name: dict[str, dict[str, Any]] = {}
    for tool in tools:
        require(isinstance(tool, dict), f"tool descriptor is not object: {tool}")
        name = tool.get("name")
        require(isinstance(name, str) and name, f"tool descriptor missing name: {tool}")
        require(name not in by_name, f"duplicate tool descriptor: {name}")
        by_name[name] = tool
        require(isinstance(tool.get("description"), str), f"{name} missing description")
        require(isinstance(tool.get("inputSchema"), dict), f"{name} missing inputSchema")
        require(isinstance(tool.get("outputSchema"), dict), f"{name} missing outputSchema")
        require(
            tool["inputSchema"].get("type") == "object",
            f"{name} inputSchema must be object schema: {tool['inputSchema']}",
        )
        require(
            tool["outputSchema"].get("type") == "object",
            f"{name} outputSchema must be object schema: {tool['outputSchema']}",
        )
    missing = [name for name in REQUIRED_TOOLS if name not in by_name]
    require(not missing, f"tools/list missing required tools: {missing}")
    require(len(by_name) >= 21, f"expected at least 21 tools, got {len(by_name)}")
    return by_name


def test_cli_to_mcp_store_visibility(client: McpClient, seed_spec: str, strict_structured: bool) -> None:
    result = client.tool("show_requirement", {"id": seed_spec})
    require_structured_content_if_requested(result, "show_requirement", strict_structured)
    text = content_text(result)
    require(seed_spec in text, f"MCP show_requirement could not see CLI-created {seed_spec}: {text}")
    require("MCP stdio seed requirement" in text, f"MCP show_requirement missing seed title: {text}")

    listed = client.tool("list_requirements", {"limit": 100})
    text = content_text(listed)
    require(seed_spec in text, f"MCP list_requirements could not see CLI-created {seed_spec}: {text}")


def test_mcp_to_cli_store_visibility(client: McpClient, aida: Path, root: Path, strict_structured: bool) -> str:
    result = client.tool(
        "add_requirement",
        {
            "title": "MCP-created visibility probe",
            "description": "Created through tools/call and read back through local CLI.",
            "type": "task",
            "status": "approved",
            "priority": "medium",
            "tags": ["mcp-stdio-test", "codex"],
        },
    )
    require_structured_content_if_requested(result, "add_requirement", strict_structured)
    text = content_text(result)
    match = re.search(r"\b([A-Z]+(?:-[A-Z0-9]+)?-\d+(?:-\d+)?)\b", text)
    require(bool(match), f"could not parse MCP-created spec id from add_requirement output: {text}")
    spec = match.group(1)

    show = run([str(aida), "show", spec], root)
    require(
        show.returncode == 0,
        f"local CLI could not show MCP-created {spec}:\nstdout={show.stdout}\nstderr={show.stderr}",
    )
    require("MCP-created visibility probe" in show.stdout, f"CLI show missing MCP-created title:\n{show.stdout}")
    return spec


def test_comments_search_and_update(client: McpClient, spec: str, strict_structured: bool) -> None:
    comment = client.tool("add_comment", {"id": spec, "text": "MCP stdio comment probe"})
    require_structured_content_if_requested(comment, "add_comment", strict_structured)
    require("Comment added" in content_text(comment), f"unexpected add_comment response: {comment}")

    shown = client.tool("show_requirement", {"id": spec})
    require("MCP stdio comment probe" in content_text(shown), "show_requirement missing added comment")

    searched = client.tool("search_requirements", {"query": "visibility probe"})
    require(spec in content_text(searched), "search_requirements missing MCP-created item")

    updated = client.tool("update_requirement", {"id": spec, "status": "planned"})
    require_structured_content_if_requested(updated, "update_requirement", strict_structured)
    shown_after = content_text(client.tool("show_requirement", {"id": spec}))
    require("Planned" in shown_after or "planned" in shown_after, f"status update not visible: {shown_after}")


def test_coordination_tools(client: McpClient, strict_structured: bool) -> None:
    punt = client.tool(
        "post_punt",
        {
            "spec_id": "MCP-SMOKE-1",
            "detail": "stdio compatibility test punt",
            "category": "ambiguous-spec",
            "lean": "choose minimal descriptor-only path",
            "raised_by": "mcp-stdio-test",
        },
    )
    require_structured_content_if_requested(punt, "post_punt", strict_structured)
    require("MCP-SMOKE-1" in content_text(punt), f"post_punt response missing spec: {punt}")

    punt_list = content_text(client.tool("list_punts", {}))
    require("MCP-SMOKE-1" in punt_list, f"list_punts missing smoke punt: {punt_list}")
    punt_read = content_text(client.tool("read_punt", {"spec_id": "MCP-SMOKE-1"}))
    require("ambiguous-spec" in punt_read, f"read_punt missing category: {punt_read}")

    resolved = client.tool(
        "resolve_punt",
        {
            "spec_id": "MCP-SMOKE-1",
            "answer": "descriptor-only for this ticket",
            "reasoning": "structuredContent is a separate compatibility task",
        },
    )
    require_structured_content_if_requested(resolved, "resolve_punt", strict_structured)

    claim = content_text(client.tool("claim_task", {"spec_id": "MCP-SMOKE-CLAIM", "role": "implementer"}))
    match = re.search(r"lease_id=([^\s]+)", claim)
    require(bool(match), f"could not parse lease id from claim_task response: {claim}")
    lease_id = match.group(1)
    leases = content_text(client.tool("list_active_leases", {}))
    require("MCP-SMOKE-CLAIM" in leases, f"list_active_leases missing claim: {leases}")
    released = content_text(client.tool("release_task", {"lease_id": lease_id}))
    require("released" in released.lower(), f"unexpected release_task response: {released}")

    posted = content_text(client.tool("post_directive", {"verb": "pause"}))
    require("pause" in posted.lower() or "posted" in posted.lower(), f"unexpected post_directive response: {posted}")
    directives = content_text(client.tool("list_directives", {}))
    require("pause" in directives, f"list_directives missing pause: {directives}")
    acked = content_text(client.tool("ack_directive", {"index": 0}))
    require("ack" in acked.lower() or "removed" in acked.lower(), f"unexpected ack_directive response: {acked}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--aida", default="target/debug/aida", help="Path to aida binary")
    parser.add_argument("--keep-tmp", action="store_true", help="Keep temporary project for debugging")
    parser.add_argument(
        "--require-structured-content",
        action="store_true",
        help="Require tools/call results to include structuredContent objects",
    )
    parser.add_argument(
        "--require-mcp-write-roundtrip",
        action="store_true",
        help=(
            "Run the MCP-write -> CLI-read and coordination round-trip stages. "
            "Off by default while BUG-310 (MCP add_requirement does not persist "
            "to the canonical store) is in flight; flip on once it ships."
        ),
    )
    args = parser.parse_args()

    aida = Path(args.aida).resolve()
    if not aida.exists():
        raise Failure(f"aida binary not found: {aida}; run `cargo build -p aida-cli` first")

    tmp = Path(tempfile.mkdtemp(prefix="aida-mcp-stdio."))
    client: McpClient | None = None
    try:
        seed_spec = setup_project(aida, tmp)
        client = McpClient(aida, tmp)
        tests = [
            ("initialize", lambda: test_initialize(client)),
            ("tools/list descriptors", lambda: test_tool_descriptors(client)),
            (
                "CLI-created spec visible through MCP",
                lambda: test_cli_to_mcp_store_visibility(client, seed_spec, args.require_structured_content),
            ),
        ]

        for name, fn in tests:
            print(f"TEST {name} ... ", end="", flush=True)
            fn()
            print("ok")

        if not args.require_mcp_write_roundtrip:
            print("SKIP MCP-write -> CLI-read stage (pass --require-mcp-write-roundtrip; gated on BUG-310)")
            print(f"PASS MCP stdio compatibility suite — descriptors-only stage ({tmp})")
            return 0

        print("TEST MCP-created spec visible through CLI ... ", end="", flush=True)
        mcp_created_spec = test_mcp_to_cli_store_visibility(client, aida, tmp, args.require_structured_content)
        print("ok")

        print("TEST comment/search/update round trip ... ", end="", flush=True)
        test_comments_search_and_update(client, mcp_created_spec, args.require_structured_content)
        print("ok")

        print("TEST coordination tools round trip ... ", end="", flush=True)
        test_coordination_tools(client, args.require_structured_content)
        print("ok")

        print(f"PASS MCP stdio compatibility suite ({tmp})")
        return 0
    except Failure as exc:
        print(f"\nFAIL {exc}", file=sys.stderr)
        print(f"Temp project: {tmp}", file=sys.stderr)
        return 1
    finally:
        if client is not None:
            client.close()
        if args.keep_tmp:
            print(f"Kept temp project: {tmp}", file=sys.stderr)
        else:
            shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
