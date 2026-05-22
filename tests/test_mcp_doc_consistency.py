#!/usr/bin/env python3
"""Doc-vs-MCP consistency gate.

`docs/agents/cross-agent-onboarding.md` is the agent-facing API contract for
non-Claude-Code agents (Codex, Cursor, etc.) attaching to AIDA. When that doc
drifts from the actual MCP server's `tools/list` advertisement, every agent
following the doc fails at call time. Codex caught this on 2026-05-22 — five
fictional argument names had drifted in.

This test prevents the regression:

1. Parse the doc for every backtick-quoted ``name({args})`` and ``name()``.
2. Spin up `aida mcp-serve`, query `tools/list`.
3. Assert every documented tool exists in the advertised list.
4. Assert every documented argument exists in the corresponding tool's
   inputSchema.properties.
5. Assert every advertised tool is named in the doc (reverse direction).

Drift reports name the offending entries explicitly so the failure tells you
exactly which line(s) to fix.

trace:TASK-452 | ai:claude
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
from pathlib import Path
from typing import Any


DOC_REL_PATH = "docs/agents/cross-agent-onboarding.md"

# Match either `name({arg, arg?, ...})` or `name()` inside backticks.
# The body may contain quoted strings (with embedded commas/colons), so we
# parse comma-separation manually instead of relying on a greedy regex.
TOOL_PATTERN = re.compile(r"`([a-z_][a-z0-9_]*)\((\{[^`]*?\}|)\)`")

# Tool names that look like the pattern but are intentionally placeholders
# (e.g. an MCP-tool name reused as a CLI verb in prose). Empty for now;
# extend with a comment + reason if a real conflict arises.
NON_MCP_TOOL_NAMES: set[str] = set()


class Drift(Exception):
    pass


def parse_arglist(arglist: str) -> tuple[set[str], bool]:
    """Extract argument identifiers from a `{ ... }` body.

    Returns (args, has_ellipsis). `has_ellipsis` is True when the body contains
    a literal `...`, meaning the doc is non-exhaustive for that tool — the
    advertised inputSchema may legitimately carry additional properties not
    named in the doc. Documented args still must exist.
    """
    body = arglist.strip()
    if body.startswith("{") and body.endswith("}"):
        body = body[1:-1]
    args: set[str] = set()
    has_ellipsis = False

    # Comma-split, respecting quoted strings so `key: "a, b"` stays one part.
    parts: list[str] = []
    buf = ""
    in_quote: str | None = None
    for ch in body:
        if in_quote is not None:
            buf += ch
            if ch == in_quote:
                in_quote = None
            continue
        if ch in ('"', "'"):
            buf += ch
            in_quote = ch
            continue
        if ch == ",":
            parts.append(buf)
            buf = ""
            continue
        buf += ch
    if buf.strip():
        parts.append(buf)

    for part in parts:
        token = part.strip()
        if not token:
            continue
        if token == "..." or token.startswith("..."):
            has_ellipsis = True
            continue
        # Leading identifier — strips `?` optional marker and any `: value` tail.
        match = re.match(r"([a-zA-Z_][a-zA-Z0-9_]*)", token)
        if not match:
            continue
        args.add(match.group(1))
    return args, has_ellipsis


def parse_doc(doc_path: Path) -> dict[str, dict[str, Any]]:
    """Parse the doc into {tool_name: {"args": set, "exhaustive": bool}}.

    If the same tool is mentioned multiple times, args are unioned and
    `exhaustive` is False if any mention used `...`.
    """
    text = doc_path.read_text()
    docs: dict[str, dict[str, Any]] = {}
    for match in TOOL_PATTERN.finditer(text):
        name = match.group(1)
        body = match.group(2)
        if name in NON_MCP_TOOL_NAMES:
            continue
        args, has_ellipsis = parse_arglist(body) if body else (set(), False)
        entry = docs.setdefault(name, {"args": set(), "exhaustive": True})
        entry["args"].update(args)
        if has_ellipsis:
            entry["exhaustive"] = False
    return docs


class McpClient:
    """Minimal JSON-RPC stdio client for `aida mcp-serve`."""

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
        assert self.proc.stdin is not None and self.proc.stdout is not None
        self.proc.stdin.write(json.dumps(payload) + "\n")
        self.proc.stdin.flush()
        raw = self.proc.stdout.readline()
        if raw == "":
            stderr = self.proc.stderr.read() if self.proc.stderr else ""
            raise Drift(f"MCP server exited before responding to {method}; stderr:\n{stderr}")
        resp = json.loads(raw)
        if resp.get("error"):
            raise Drift(f"JSON-RPC error for {method}: {resp['error']}")
        return resp


def fetch_advertised_tools(client: McpClient) -> dict[str, set[str]]:
    """Return {tool_name: set_of_input_property_names} from tools/list."""
    # Initialize handshake — server doesn't strictly require this for
    # tools/list, but real MCP clients always send it.
    client.request(
        "initialize",
        {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "aida-mcp-doc-consistency-test", "version": "0"},
        },
    )
    resp = client.request("tools/list")
    tools = resp.get("result", {}).get("tools")
    if not isinstance(tools, list):
        raise Drift(f"tools/list returned unexpected shape: {resp}")
    advertised: dict[str, set[str]] = {}
    for tool in tools:
        name = tool.get("name")
        if not isinstance(name, str):
            raise Drift(f"tool descriptor missing name: {tool}")
        props = tool.get("inputSchema", {}).get("properties", {})
        if not isinstance(props, dict):
            raise Drift(f"tool {name}: inputSchema.properties is not an object")
        advertised[name] = set(props.keys())
    return advertised


def setup_project(aida: Path, root: Path) -> None:
    """Initialize a minimal AIDA project so mcp-serve can start."""
    subprocess.run(["git", "init", "-q"], cwd=root, check=True)
    init = subprocess.run(
        [str(aida), "init", "--no-skills", "--no-hooks"],
        cwd=root,
        capture_output=True,
        text=True,
    )
    if init.returncode != 0:
        raise Drift(f"aida init failed:\nstdout={init.stdout}\nstderr={init.stderr}")


def compare(
    documented: dict[str, dict[str, Any]],
    advertised: dict[str, set[str]],
) -> list[str]:
    """Compare documented vs advertised. Returns a list of drift lines."""
    drifts: list[str] = []

    doc_tools = set(documented.keys())
    adv_tools = set(advertised.keys())

    missing_from_mcp = sorted(doc_tools - adv_tools)
    if missing_from_mcp:
        drifts.append(
            "Tools documented but NOT advertised by `tools/list` "
            f"(fictional in doc): {missing_from_mcp}"
        )

    missing_from_doc = sorted(adv_tools - doc_tools)
    if missing_from_doc:
        drifts.append(
            "Tools advertised by `tools/list` but NOT documented "
            f"in {DOC_REL_PATH}: {missing_from_doc}"
        )

    for tool in sorted(doc_tools & adv_tools):
        doc_args = documented[tool]["args"]
        adv_args = advertised[tool]
        bogus = sorted(doc_args - adv_args)
        if bogus:
            drifts.append(
                f"Tool `{tool}`: documented arguments do NOT exist in "
                f"inputSchema (fictional): {bogus}. "
                f"Advertised args: {sorted(adv_args)}."
            )

    return drifts


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--aida", default="target/debug/aida", help="Path to aida binary")
    parser.add_argument(
        "--doc",
        default=DOC_REL_PATH,
        help=f"Path to the onboarding doc (default: {DOC_REL_PATH})",
    )
    parser.add_argument("--keep-tmp", action="store_true", help="Keep temp project for debugging")
    args = parser.parse_args()

    aida = Path(args.aida).resolve()
    if not aida.exists():
        print(
            f"FAIL aida binary not found: {aida}; run `cargo build -p aida-cli` first",
            file=sys.stderr,
        )
        return 1

    repo_root = Path(__file__).resolve().parent.parent
    doc_path = (repo_root / args.doc).resolve()
    if not doc_path.exists():
        print(f"FAIL onboarding doc not found: {doc_path}", file=sys.stderr)
        return 1

    print(f"TEST parse {doc_path.relative_to(repo_root)} ... ", end="", flush=True)
    documented = parse_doc(doc_path)
    if not documented:
        print("FAIL no tool mentions parsed — regex broken or doc empty", file=sys.stderr)
        return 1
    print(f"ok ({len(documented)} tools mentioned)")

    tmp = Path(tempfile.mkdtemp(prefix="aida-mcp-doc."))
    client: McpClient | None = None
    try:
        print("TEST start aida mcp-serve in scratch project ... ", end="", flush=True)
        setup_project(aida, tmp)
        client = McpClient(aida, tmp)
        print("ok")

        print("TEST tools/list ... ", end="", flush=True)
        advertised = fetch_advertised_tools(client)
        print(f"ok ({len(advertised)} tools advertised)")

        print("TEST doc-vs-MCP consistency ... ", end="", flush=True)
        drifts = compare(documented, advertised)
        if drifts:
            print("FAIL")
            print("", file=sys.stderr)
            print(
                f"DOC/MCP DRIFT — {doc_path.relative_to(repo_root)} disagrees with "
                f"`aida mcp-serve` tools/list:",
                file=sys.stderr,
            )
            for line in drifts:
                print(f"  - {line}", file=sys.stderr)
            print("", file=sys.stderr)
            print(
                "Fix the doc (or the descriptor in aida-cli/src/mcp.rs) so they "
                "agree. The doc is an agent-facing API contract — agents that "
                "follow it must succeed at call time.",
                file=sys.stderr,
            )
            return 1
        print("ok")

        print(f"PASS doc-vs-MCP consistency ({tmp})")
        return 0
    except Drift as exc:
        print("FAIL", file=sys.stderr)
        print(str(exc), file=sys.stderr)
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
