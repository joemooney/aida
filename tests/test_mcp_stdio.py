#!/usr/bin/env python3
"""Black-box MCP stdio compatibility tests for AIDA.

This suite starts `aida mcp-serve` as a child process and speaks JSON-RPC over
stdio, the same local transport Codex and other MCP clients use. It validates
the server at the client boundary rather than calling Rust helpers directly.

Coverage:
- JSON-RPC initialize / tools/list / tools/call framing.
- All 21 expected tools are advertised.
- Every tool advertises inputSchema and outputSchema.
- Path-A tool results use the MCP text content envelope.
- Optional future Path-B strict mode requires structuredContent.
- Per-request deadlines fail fast if `aida mcp-serve` hangs.
- CLI-created specs are visible through MCP.
- MCP-created specs are visible through local CLI.
- Core spec graph tools round-trip.
- Coordination tools round-trip through `.aida/` files.

Doc-vs-descriptor contract drift is covered by
`tests/test_mcp_doc_consistency.py`. Keep this suite focused on stdio protocol
behavior and cross-surface state roundtrips so it does not duplicate a second
agent-facing argument table.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import selectors
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any, Callable


REQUIRED_TOOLS = [
    "list_requirements",
    "show_requirement",
    "add_requirement",
    "update_requirement",
    "search_requirements",
    "add_comment",
    "list_features",
    "list_punts",
    "read_punt",
    "post_punt",
    "resolve_punt",
    "escalate_punt",
    "list_findings",
    "file_finding",
    "triage_finding",
    "claim_task",
    "release_task",
    "list_active_leases",
    "post_directive",
    "list_directives",
    "ack_directive",
]

SPEC_ID_RE = r"[A-Z]+(?:-[A-Z0-9]+)?-\d+(?:-\d+)?"


class Failure(Exception):
    pass


class McpClient:
    def __init__(self, aida: Path, cwd: Path, request_timeout: float):
        self.proc = subprocess.Popen(
            [str(aida), "mcp-serve"],
            cwd=cwd,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
        )
        if self.proc.stdin is None or self.proc.stdout is None:
            raise Failure("failed to open MCP process stdio pipes")
        self._next_id = 1
        self.request_timeout = request_timeout

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

        assert self.proc.stdin is not None
        assert self.proc.stdout is not None
        self.proc.stdin.write(json.dumps(payload) + "\n")
        self.proc.stdin.flush()
        raw = self._readline_with_deadline(method)
        if raw == "":
            raise Failure(
                f"MCP server exited before {method} response; "
                f"rc={self.proc.poll()}; stderr={self._read_stderr_tail()!r}"
            )
        try:
            resp = json.loads(raw)
        except json.JSONDecodeError as exc:
            raise Failure(f"invalid JSON response to {method}: {raw!r}") from exc
        if resp.get("id") != req_id:
            raise Failure(f"response id mismatch for {method}: expected {req_id}, got {resp}")
        if resp.get("error") is not None:
            raise Failure(f"JSON-RPC error for {method}: {resp['error']}")
        return resp

    def tool(self, name: str, arguments: dict[str, Any] | None = None) -> dict[str, Any]:
        resp = self.request("tools/call", {"name": name, "arguments": arguments or {}})
        result = resp.get("result")
        if not isinstance(result, dict):
            raise Failure(f"tool {name} returned non-object result: {resp}")
        if result.get("isError"):
            raise Failure(f"tool {name} returned isError: {result}")
        return result

    def _readline_with_deadline(self, method: str) -> str:
        assert self.proc.stdout is not None
        selector = selectors.DefaultSelector()
        selector.register(self.proc.stdout, selectors.EVENT_READ)
        try:
            if not selector.select(timeout=self.request_timeout):
                stderr = self._read_stderr_tail()
                self.close()
                raise Failure(
                    f"timed out after {self.request_timeout:g}s waiting for MCP "
                    f"response to {method}; stderr={stderr!r}"
                )
            return self.proc.stdout.readline()
        finally:
            selector.close()

    def _read_stderr_tail(self) -> str:
        if self.proc.stderr is None:
            return ""
        fd = self.proc.stderr.fileno()
        try:
            os.set_blocking(fd, False)
            chunks: list[str] = []
            while True:
                chunk = self.proc.stderr.read()
                if not chunk:
                    break
                chunks.append(chunk)
            return "".join(chunks)[-4000:]
        except (BlockingIOError, OSError, TypeError, ValueError):
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


def require(condition: bool, message: str) -> None:
    if not condition:
        raise Failure(message)


def content_text(result: dict[str, Any]) -> str:
    content = result.get("content")
    require(isinstance(content, list), f"missing MCP content array: {result}")
    chunks: list[str] = []
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
    require("structuredContent" in result, f"{tool} missing structuredContent in strict mode")
    require(isinstance(result["structuredContent"], dict), f"{tool} structuredContent must be object")


def strip_ansi(text: str) -> str:
    return re.sub(r"\x1b\[[0-9;]*m", "", text)


def parse_spec_id(text: str, source: str) -> str:
    # TASK-454: anchor on success-line shapes instead of the first loose ID.
    patterns = [
        rf"^Added:\s+({SPEC_ID_RE})\s+-\s+.+$",
        rf"^ID:\s+({SPEC_ID_RE})\s*$",
        rf"^Requirement added:\s+({SPEC_ID_RE})\s+[—-]\s+.+$",
        rf"^Finding filed:\s+({SPEC_ID_RE})\s+[—-]\s+.+$",
    ]
    anchored: list[str] = []
    for line in text.splitlines():
        cleaned = strip_ansi(line).strip()
        for pattern in patterns:
            match = re.match(pattern, cleaned)
            if match:
                anchored.append(match.group(1))
                break

    unique = sorted(set(anchored))
    if len(unique) == 1:
        return unique[0]
    if len(unique) > 1:
        raise Failure(f"{source}: multiple anchored SPEC-ID candidates {unique} in output:\n{text}")

    loose = sorted(set(re.findall(rf"\b({SPEC_ID_RE})\b", strip_ansi(text))))
    if loose:
        raise Failure(
            f"{source}: found loose SPEC-ID candidates {loose}, but none appeared "
            f"on a recognized success line:\n{text}"
        )
    raise Failure(f"{source}: could not parse SPEC-ID from output:\n{text}")


def test_parse_spec_id_fixtures() -> None:
    require(
        parse_spec_id("Hint: see META-1\nAdded: TASK-82 - created\n", "aida add") == "TASK-82",
        "aida add parser should ignore earlier loose IDs",
    )
    require(
        parse_spec_id("META-1 preface\nRequirement added: TASK-83 — created\n", "add_requirement")
        == "TASK-83",
        "MCP add_requirement parser should anchor on its success line",
    )
    require(
        parse_spec_id("BUG-1 is related\nFinding filed: TASK-84 — finding\n", "file_finding")
        == "TASK-84",
        "file_finding parser should anchor on its success line",
    )
    try:
        parse_spec_id("Hint: see META-1\nNo creation line\n", "negative fixture")
    except Failure as exc:
        require("loose SPEC-ID candidates" in str(exc), f"negative fixture had unclear failure: {exc}")
    else:
        raise Failure("negative fixture unexpectedly parsed a loose SPEC-ID")


def setup_project(aida: Path, root: Path) -> str:
    run(["git", "init"], root)
    init = run([str(aida), "init", "--no-skills", "--no-hooks"], root)
    require(init.returncode == 0, f"aida init failed:\nstdout={init.stdout}\nstderr={init.stderr}")

    added = run(
        [
            str(aida),
            "add",
            "--title",
            "MCP stdio seed requirement",
            "--description",
            "Seed item created by the MCP stdio compatibility suite.",
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
    require(added.returncode == 0, f"aida add failed:\nstdout={added.stdout}\nstderr={added.stderr}")
    return parse_spec_id(added.stdout, "aida add")


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
    require(isinstance(result, dict), f"initialize result is not object: {resp}")
    require(isinstance(result.get("protocolVersion"), str), f"missing protocolVersion: {result}")
    require("tools" in result.get("capabilities", {}), f"missing tools capability: {result}")


def test_tool_descriptors(client: McpClient) -> dict[str, dict[str, Any]]:
    resp = client.request("tools/list")
    result = resp.get("result")
    require(isinstance(result, dict), f"tools/list result is not object: {resp}")
    tools = result.get("tools")
    require(isinstance(tools, list), f"tools/list missing tools array: {result}")

    descriptors: dict[str, dict[str, Any]] = {}
    for tool in tools:
        require(isinstance(tool, dict), f"tool descriptor is not object: {tool}")
        name = tool.get("name")
        require(isinstance(name, str) and name, f"tool descriptor missing name: {tool}")
        require(name not in descriptors, f"duplicate tool descriptor: {name}")
        descriptors[name] = tool
        require(isinstance(tool.get("description"), str), f"{name} missing description")
        input_schema = tool.get("inputSchema")
        output_schema = tool.get("outputSchema")
        require(isinstance(input_schema, dict), f"{name} missing inputSchema")
        require(isinstance(output_schema, dict), f"{name} missing outputSchema")
        require(input_schema.get("type") == "object", f"{name} inputSchema must be object schema")
        require(output_schema.get("type") == "object", f"{name} outputSchema must be object schema")
        require("content" in output_schema.get("properties", {}), f"{name} outputSchema missing content")

    missing = [name for name in REQUIRED_TOOLS if name not in descriptors]
    require(not missing, f"tools/list missing required tools: {missing}")
    require(len(descriptors) >= 21, f"expected at least 21 tools, got {len(descriptors)}")
    return descriptors


def test_cli_to_mcp_visibility(client: McpClient, seed_spec: str, strict: bool) -> None:
    shown = client.tool("show_requirement", {"id": seed_spec})
    require_structured_content_if_requested(shown, "show_requirement", strict)
    text = content_text(shown)
    require(seed_spec in text, f"MCP show did not include CLI-created {seed_spec}:\n{text}")
    require("MCP stdio seed requirement" in text, f"MCP show missing seed title:\n{text}")

    listed = content_text(client.tool("list_requirements", {"limit": 100}))
    require(seed_spec in listed, f"MCP list did not include CLI-created {seed_spec}:\n{listed}")


def test_mcp_to_cli_visibility(client: McpClient, aida: Path, root: Path, strict: bool) -> str:
    added = client.tool(
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
    require_structured_content_if_requested(added, "add_requirement", strict)
    spec = parse_spec_id(content_text(added), "add_requirement")

    shown = run([str(aida), "show", spec], root)
    require(
        shown.returncode == 0,
        f"local CLI could not show MCP-created {spec}:\nstdout={shown.stdout}\nstderr={shown.stderr}",
    )
    require("MCP-created visibility probe" in shown.stdout, f"CLI show missing MCP-created title:\n{shown.stdout}")
    return spec


def test_spec_graph_round_trips(client: McpClient, spec: str, strict: bool) -> None:
    commented = client.tool("add_comment", {"id": spec, "text": "MCP stdio comment probe"})
    require_structured_content_if_requested(commented, "add_comment", strict)
    require("Comment added" in content_text(commented), f"unexpected add_comment response: {commented}")

    shown = content_text(client.tool("show_requirement", {"id": spec}))
    require("MCP stdio comment probe" in shown, f"show_requirement missing comment:\n{shown}")

    searched = content_text(client.tool("search_requirements", {"query": "visibility probe"}))
    require(spec in searched, f"search_requirements missing {spec}:\n{searched}")

    updated = client.tool("update_requirement", {"id": spec, "status": "planned"})
    require_structured_content_if_requested(updated, "update_requirement", strict)
    shown_after = content_text(client.tool("show_requirement", {"id": spec}))
    require(
        "Planned" in shown_after or "planned" in shown_after,
        f"show_requirement did not reflect status update:\n{shown_after}",
    )


def test_coordination_round_trips(client: McpClient, strict: bool) -> None:
    punted = client.tool(
        "post_punt",
        {
            "spec_id": "MCP-SMOKE-1",
            "reason": "stdio compatibility test punt",
            "category": "ambiguous-spec",
            "lean": "choose minimal descriptor-only path",
            "raised_by": "mcp-stdio-test",
        },
    )
    require_structured_content_if_requested(punted, "post_punt", strict)
    require("MCP-SMOKE-1" in content_text(punted), f"post_punt missing spec:\n{punted}")

    listed = content_text(client.tool("list_punts", {}))
    require("MCP-SMOKE-1" in listed, f"list_punts missing smoke punt:\n{listed}")

    read = content_text(client.tool("read_punt", {"punt_id": "MCP-SMOKE-1"}))
    require("ambiguous-spec" in read, f"read_punt missing category:\n{read}")

    resolved = client.tool(
        "resolve_punt",
        {
            "punt_id": "MCP-SMOKE-1",
            "answer": "descriptor-only for this ticket",
            "rationale": "structuredContent is a separate compatibility task",
        },
    )
    require_structured_content_if_requested(resolved, "resolve_punt", strict)
    require("Resolution written" in content_text(resolved), f"unexpected resolve_punt response:\n{resolved}")

    claim = content_text(client.tool("claim_task", {"spec_id": "MCP-SMOKE-CLAIM", "role": "implementer"}))
    match = re.search(r"lease_id=([^\s]+)", claim)
    require(bool(match), f"could not parse lease id from claim_task response:\n{claim}")
    lease_id = match.group(1)

    leases = content_text(client.tool("list_active_leases", {}))
    require("MCP-SMOKE-CLAIM" in leases, f"list_active_leases missing claim:\n{leases}")

    released = content_text(client.tool("release_task", {"lease_id": lease_id}))
    require("released" in released.lower(), f"unexpected release_task response:\n{released}")

    posted = content_text(client.tool("post_directive", {"verb": "pause"}))
    require("directive posted" in posted, f"unexpected post_directive response:\n{posted}")
    directives = content_text(client.tool("list_directives", {}))
    require("pause" in directives, f"list_directives missing pause:\n{directives}")
    acked = content_text(client.tool("ack_directive", {"index": 0}))
    require("acked" in acked, f"unexpected ack_directive response:\n{acked}")


def test_finding_round_trip(client: McpClient) -> None:
    filed = client.tool(
        "file_finding",
        {
            "title": "MCP stdio finding probe",
            "description": "Finding created by stdio suite.",
            "source": "review",
            "pr": 162,
            "kind": "followup-suggestion",
            "severity": "minor",
        },
    )
    spec = parse_spec_id(content_text(filed), "file_finding")
    findings = content_text(client.tool("list_findings", {"source": "review", "pr": 162}))
    require(spec in findings, f"list_findings missing filed finding {spec}:\n{findings}")

    triaged = content_text(client.tool("triage_finding", {"id": spec, "action": "promote", "reason": "stdio suite"}))
    require("promote" in triaged.lower(), f"unexpected triage_finding response:\n{triaged}")


def run_test(name: str, fn: Callable[[], Any]) -> Any:
    print(f"TEST {name} ... ", end="", flush=True)
    result = fn()
    print("ok")
    return result


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--aida", default="target/debug/aida", help="Path to aida binary")
    parser.add_argument("--keep-tmp", action="store_true", help="Keep temporary project for debugging")
    parser.add_argument(
        "--request-timeout",
        type=float,
        default=30.0,
        help="Seconds to wait for each MCP JSON-RPC response before failing fast",
    )
    parser.add_argument(
        "--require-structured-content",
        action="store_true",
        help="Require Path-B structuredContent objects in tools/call results",
    )
    parser.add_argument(
        "--skip-agent-contract",
        action="store_true",
        help=argparse.SUPPRESS,
    )
    args = parser.parse_args()
    if args.request_timeout <= 0:
        raise Failure("--request-timeout must be greater than 0")

    aida = Path(args.aida).resolve()
    if not aida.exists():
        raise Failure(f"aida binary not found: {aida}; run `cargo build -p aida-cli` first")

    tmp = Path(tempfile.mkdtemp(prefix="aida-mcp-stdio."))
    client: McpClient | None = None
    try:
        seed_spec = setup_project(aida, tmp)
        client = McpClient(aida, tmp, args.request_timeout)

        run_test("spec-ID parser fixtures", test_parse_spec_id_fixtures)
        run_test("initialize", lambda: test_initialize(client))
        run_test("tools/list descriptors", lambda: test_tool_descriptors(client))
        run_test("CLI-created spec visible through MCP", lambda: test_cli_to_mcp_visibility(client, seed_spec, args.require_structured_content))
        mcp_spec = run_test("MCP-created spec visible through CLI", lambda: test_mcp_to_cli_visibility(client, aida, tmp, args.require_structured_content))
        run_test("spec graph round trips", lambda: test_spec_graph_round_trips(client, mcp_spec, args.require_structured_content))
        run_test("coordination tools round trips", lambda: test_coordination_round_trips(client, args.require_structured_content))
        run_test("findings round trip", lambda: test_finding_round_trip(client))

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
