# MCP coordination surface — filesystem-canonical, MCP-transport

*STORY-361 — implemented 2026-05-21*

## TL;DR

AIDA's orchestrator and skills coordinate through files under `.aida/`. The
**filesystem stays canonical**: it is local, durable, debuggable with
`ls`/`cat`, and survives process crashes without a daemon. The AIDA MCP server
extends that substrate with **thin tools that read from and write to the same
files**, so any MCP-speaking agent (Codex, Cursor, the Anthropic MCP
inspector, …) can participate in an AIDA drain without re-platforming the
orchestrator.

The MCP server is a **transport layer over filesystem state**, not a
replacement for it.

```
                                    ┌───────────────────────┐
        Claude Code  ──────────────▶│                       │
                                    │   .aida/  (canonical) │
        Codex / Cursor / etc.       │   ├─ punts.jsonl     │
              │                     │   ├─ punts/*.json    │
              ▼                     │   ├─ sessions/*.toml │
   ┌──────────────────────┐ ─writes▶│   ├─ worker.cmd      │
   │ aida MCP server      │         │   └─ findings (specs)│
   │ (aida mcp-serve)     │ reads─▶ │                       │
   └──────────────────────┘         └───────────────────────┘
              ▲                                  ▲
              │                                  │
              └── stdio JSON-RPC ────────┐       │
                                         │       └── orchestrator
                                         │           (Rust)
                                         └── skills (`/aida-…`)
```

## Why filesystem-canonical

The SPIKE-9 evaluation ([`docs/spikes/2026-05-20-spike-9-mcp-as-bus.md`])
weighed MCP-as-bus against the existing file-handshakes and landed on
**hybrid**:

| Property | File handshakes | MCP-as-bus |
|---|---|---|
| Latency (local) | microseconds | ~ms RPC roundtrip |
| Crash safety | filesystem persists | depends on server uptime |
| Debuggability | `ls .aida/` | requires a tools client |
| Single-machine ops | no daemon | one process to keep alive |
| Cross-agent reach | Claude Code only (today) | any MCP-speaking agent |
| Cross-machine reach | needs NFS / equivalent | trivial (HTTP transport) |

The orchestrator pays the operational complexity of "one more daemon" only
when it gains something file-handshakes can't deliver. Cross-agent
participation is exactly that something — file-handshakes are agent-agnostic
in *principle* but they assume the other agent can read AIDA's file
conventions natively. MCP, by contrast, is a published protocol with broad
client support.

## What the MCP server exposes

`aida mcp-serve` runs a JSON-RPC 2.0 server over stdio that exposes two
surfaces:

### Surface 1 — Spec graph (original 7 tools)

`list_requirements`, `show_requirement`, `add_requirement`,
`update_requirement`, `search_requirements`, `add_comment`, `list_features`.
These read/write the AIDA requirement store — the same data `aida list` /
`aida show` / `aida edit` operate on.

### Surface 2 — Coordination (STORY-361, 14 tools)

Each tool wraps a file convention. The file is the source of truth; the tool
is a transport.

| Tool | File | Notes |
|---|---|---|
| `list_punts` | `.aida/punts.jsonl` | Read the punt ledger |
| `read_punt` | `.aida/punts.jsonl` | Most-recent record for a spec |
| `post_punt` | `.aida/punts.jsonl` | Append a punt record |
| `resolve_punt` | `.aida/punts/<spec>.response.json` | Advisor-tier write (STORY-306) |
| `escalate_punt` | `.aida/punts/<spec>.response.json` | Advisor-tier escalation |
| `list_findings` | spec graph (tagged drafts) | Wraps `aida findings list` |
| `file_finding` | spec graph | Draft TASK with `from-*` tags |
| `triage_finding` | spec graph | Promote / dismiss |
| `claim_task` | `.aida/sessions/mcp-claim.<spec>.toml` | Lightweight lease (mcp_claim=true); spec-keyed filename gives `O_EXCL` single-winner semantics |
| `release_task` | `.aida/sessions/mcp-claim.<spec>.toml` | Delete an MCP lease (looked up by embedded lease_id) |
| `list_active_leases` | `.aida/sessions/*.toml` | Read every lease (real + MCP) |
| `post_directive` | `.aida/worker.cmd` | Append a worker directive |
| `list_directives` | `.aida/worker.cmd` | Wraps `aida worker directives` |
| `ack_directive` | `.aida/worker.cmd` | Pop a directive by index |

## Properties of the design

- **No schema drift**. Each tool calls the same Rust function the file-direct
  AIDA CLI uses (`punt::append_to_ledger`, `worker::parse_directives`, …).
  If the file format changes, the tool changes at the same point in the type
  system — they cannot diverge.
- **Crash safety is the filesystem's**. `append_to_ledger` uses
  `O_APPEND` so concurrent writers on POSIX never lose a record. Lease,
  directive, and punt-response writes go through `write_atomic` (write to
  `.tmp-<uuid>`, then `rename`) so a crash mid-write leaves the previous
  file intact. The `concurrent_*` and `*_recovery` tests in
  `aida-cli/src/mcp.rs` cover both contention and torn-write cases.
- **No new lock primitives**. The MCP tools acquire the same file-level
  semantics the file-direct path already uses. There is no global MCP lock,
  no in-memory queue, no daemon-internal state that could diverge from the
  files on disk.
- **No orchestrator changes**. The orchestrator continues to read and write
  `.aida/` files directly. The MCP server is a *parallel* read/write
  surface — not in the orchestrator's hot path.
- **Lightweight claims are explicit**. `claim_task` writes a lease with
  `mcp_claim = true`. The full `aida session start` flow (which creates a
  worktree, branch, env scaffolding) is *not* what `claim_task` does — that
  shape requires a shell. The lightweight claim is enough to coordinate "I'm
  working on this; others stay out."

## Registering a non-Claude-Code agent

```bash
aida mcp register-agent --print               # see the rendered config
aida mcp register-agent                       # write to .mcp.json
aida mcp register-agent --name codex --force  # custom name, overwrite
```

For local single-machine use the server is launched over stdio
(`aida mcp-serve`). The printed `serverUrl` is `stdio://aida`; cross-machine
transport (HTTP / SSE) is deferred to a follow-up SPIKE.

## What is deliberately not in scope (STORY-361)

- **Re-platforming the orchestrator** to be an MCP client. Explicitly
  rejected — the orchestrator owns the substrate.
- **Streaming subscriptions** (server-sent updates). Polling works; add later
  if the use case demands it.
- **Cross-machine authentication**. Stdio transport assumes process-local
  trust. Cross-machine reaches a follow-up SPIKE.
- **Migrating existing Claude Code skills off file-handshakes**. Skills stay
  file-direct; MCP is the *additional* surface for non-Claude-Code agents.

## Related

- SPIKE-9 — the analysis behind hybrid: `docs/spikes/2026-05-20-spike-9-mcp-as-bus.md`
- STORY-285 — implementer findings as draft TASKs
- STORY-263 — reviewer verdict files
- STORY-306 — advisor-tier punt resolution / escalation
- TASK-294 — worker directive channel
- `docs/positioning/vs-claude-code-subagents.md` — the positioning piece this
  story enables
