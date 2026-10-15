# SPIKE-9: MCP server as the inter-agent communication bus — evaluate vs file-handshakes

*2026-05-20 — analytic investigation completed*

**Verdict: hybrid.** Keep file-handshakes as the canonical substrate for orchestrator ↔ skill (local, simple, fast, debuggable). Extend the MCP server with thin coordination tools that *read from and write to the same file substrate*, opening cross-agent and cross-machine participation without re-platforming the orchestrator. The MCP server becomes a transport layer over the filesystem state, not a replacement for it.

## Question

AIDA today coordinates the orchestrator (Rust process) with skills (Claude Code sessions executing markdown instructions) via filesystem handshakes:
- Reviewer verdicts → `.aida/review-verdicts/PR-N.json` (STORY-263)
- Skill findings → `.aida/findings/` (STORY-285)
- Exit signals → `$AIDA_EXIT_SENTINEL` touch (TASK-329)
- Punt signals → `$AIDA_PUNT_SIGNAL_FILE` (STORY-306)
- Session leases → `.aida/sessions/*.toml`

Claude Code's experimental Agent Teams uses a shared task list — the same model AIDA's queue+roles+store already is. But Agent Teams is Claude-Code-locked. AIDA's substrate is agent-agnostic (git + YAML + MCP). The open question: should AIDA's inter-agent messaging transport move from filesystem handshakes to MCP-as-bus?

## Methodology

Analytic. The SPIKE evaluates trade-offs by:
1. Surveying the existing file-handshake patterns in production
2. Surveying AIDA's current MCP server surface
3. Mapping each coordination primitive to candidate transports
4. Evaluating latency, durability, debuggability, multi-agent reach, and operational complexity

## Findings

### 1. Existing MCP server surface (today)

`aida-cli/src/mcp.rs` exposes 7 tools, all read/write on the spec graph:
- `list_requirements`, `show_requirement`, `add_requirement`, `update_requirement`, `search_requirements`, `add_comment`, `list_features`

No coordination tools. No punt-channel tools. No directive-channel tools. The MCP server is a read/write surface on the spec graph, not a bus.

### 2. File-handshake patterns currently in production

| Channel | Path | Producer | Consumer | Purpose |
|---|---|---|---|---|
| Reviewer verdict | `.aida/review-verdicts/PR-N.json` | `/aida-review` skill | Orchestrator | Phase 3 verdict + merge decision |
| Findings | `.aida/findings/` | Skills (reviewer, implementer) | Advisor + queue | Triage-able observations |
| Exit sentinel | `$AIDA_EXIT_SENTINEL` touch | Skill, before exit | Orchestrator | "Phase complete, advance" |
| Punt signal | `$AIDA_PUNT_SIGNAL_FILE` write | Implementer (`/aida-punt`) | Orchestrator | "Design fork, escalate" |
| Session lease | `.aida/sessions/<lease>.toml` | `aida session start` | Orchestrator + CLI | "Who owns this scope" |
| Cache state | `.aida/cache.db` | All AIDA writes | All AIDA reads | Cache projection of the orphan-store |

Properties of file-handshakes in practice:
- **Microsecond-latency** on local filesystem
- **Debuggable**: `ls .aida/`, `cat .aida/<file>`, easy diagnosis
- **Crash-safe**: filesystem persists across process restarts
- **No service to keep alive**: no daemon, no port, no version skew
- **Visible diff**: file contents are inspectable by humans during incidents

Limitations:
- **Single-machine scope**: filesystem doesn't span hosts (without NFS or equivalent, which adds operational complexity)
- **Polling-only**: file-watch via `inotify` works but isn't standard cross-language; producers typically `touch` and consumers poll mtime
- **Race conditions** under concurrent writers: requires careful locking (AIDA does this for HLC + dispenser)
- **Schema discovery**: each file format is hand-rolled; no `tools/list` equivalent

### 3. What MCP-as-bus would look like

Concrete sketch — add coordination tools to `aida-cli/src/mcp.rs`:

```
# Punt channel
post_punt({spec_id, reason, category}) → punt_id
list_punts({status: "awaiting" | "resolved"}) → [punt_id, ...]
read_punt(punt_id) → {reason, category, context, status}
resolve_punt({punt_id, verdict, rationale}) → ok
escalate_punt({punt_id, reason}) → ok

# Directive channel (TASK-294 territory)
post_directive({recipient_role, body, ttl}) → directive_id
list_directives({recipient_role}) → [directive_id, ...]
ack_directive(directive_id) → ok

# Task claim channel
claim_task({spec_id, role}) → lease_id | already_claimed
release_task({lease_id}) → ok
list_active_leases() → [{lease_id, scope, role, mtime}, ...]

# Findings channel
file_finding({source, level, summary, body}) → finding_id
list_findings({status: "awaiting_triage"}) → [finding_id, ...]
triage_finding({finding_id, decision, reason}) → ok
```

Each of these tools, in the recommended design, **maps directly to the existing file substrate.** `post_punt` creates `.aida/punts/<punt_id>.toml`, posts a comment on the spec, flips status to NeedsAttention — same as today's `/aida-punt` skill does via bash. `file_finding` writes to `.aida/findings/<id>.toml`. `claim_task` writes a `.aida/sessions/<lease>.toml`. The MCP server is a *thin layer over the filesystem state*, not a replacement for it.

### 4. The orchestrator-as-participant question

A central design constraint: AIDA's orchestrator is a Rust process. If MCP became the canonical transport, the orchestrator would need to be an MCP **client** (not just a server). That's new infrastructure — today AIDA's MCP code is server-side only, and adding client-side complexity (connection management, request/response handling, server lifecycle awareness) is a substantial undertaking.

**Crucial design insight:** the orchestrator doesn't need to be an MCP client *if MCP is a transport layer over the filesystem*. The orchestrator reads/writes files directly (fast, no client-server complexity). MCP-speaking agents read/write the *same files* through the MCP server. Both surfaces are valid; both see the same authoritative state.

This is the file-system-as-substrate, MCP-as-transport pattern. The substrate is canonical; MCP is one access surface. Multiple surfaces (CLI, MCP, gRPC, raw filesystem) can coexist over the same substrate.

### 5. Trade-off matrix

| Property | File-handshakes | MCP-as-bus | Hybrid (recommended) |
|---|---|---|---|
| Orchestrator integration | Native (reads files directly) | Requires new client infrastructure | Native for orchestrator |
| Single-machine latency | µs | µs (local socket) + JSON-RPC overhead | µs |
| Cross-machine | ✗ (without NFS) | ✓ | ✓ via MCP, ✗ via files |
| Cross-agent (Codex / Cursor) | ✗ (requires AIDA-specific file conventions) | ✓ | ✓ |
| Debuggability | `ls / cat` | `tools/list / tools/call` | Both |
| Service lifecycle | None | Required (server up/down/restart) | Required *only when MCP clients are participating* |
| Schema discovery | Hand-rolled per file | Standardized via JSON Schema | MCP tools self-describe; files documented |
| Operational complexity | Low | Medium-high | Low for orchestrator, medium when MCP clients exist |
| Concurrent writers | Hand-rolled locking | Server serializes | Hand-rolled locking still required at the file layer |
| Crash recovery | Files persist | MCP server restart needed; files persist | Resilient on both surfaces |

The hybrid wins on every cell that matters strategically — orchestrator simplicity stays, cross-agent reach is added.

### 6. The Codex / Cursor case — load-bearing

The strategic case for adding MCP coordination tools is **agent-agnostic participation.** Today, only Claude Code can participate in AIDA drains, because the file-handshake patterns are encoded in `/aida-pickup`, `/aida-review`, etc. — markdown skills Claude Code reads. Codex doesn't run those skills.

If AIDA wants Codex (or any future MCP-speaking agent) to be able to:
- Pick up queued tasks
- File findings
- Post punts
- Respond to directives

…then the MCP tool surface is the right way for Codex to do it. Codex doesn't need AIDA-specific file conventions; it just calls MCP tools. The orchestrator still drives the loop; agents just have a portable way to participate.

This is the **AIDA-as-coordination-substrate-for-any-agent** positioning. It's the layer beneath the IDE-agent war.

## Recommended design

### Principle

**Filesystem is canonical. MCP is a transport.** Any coordination data has exactly one source of truth (a file or set of files in `.aida/`); MCP tools read from and write to those files via the same paths AIDA already uses internally. Both the orchestrator (file-direct) and MCP clients (tool-mediated) see consistent state.

### Smallest-valuable-slice scope

1. **Extend `aida-cli/src/mcp.rs` with coordination tools** that wrap existing file operations:
   - `list_punts`, `read_punt`, `resolve_punt`, `escalate_punt`
   - `list_findings`, `file_finding`, `triage_finding`
   - `claim_task`, `release_task`, `list_active_leases`
   - `post_directive`, `list_directives`, `ack_directive` (TASK-294 composes here)
2. **No orchestrator changes required.** The orchestrator continues to read files directly. MCP clients get a coordination surface; the orchestrator gets free interoperability.
3. **Schema-document each tool**: input + output JSON schemas, with a brief description of which file(s) on disk it touches. Discoverable via `tools/list`.
4. **Add an `aida mcp register-agent` CLI for first-time agent setup**: writes the agent's MCP config (server URL, auth if needed) into the agent's own config file. For local-only single-machine use, this is a no-op; for remote, it produces a connect-string.
5. **Tests covering**: each new tool's happy-path + concurrent-write contention + crash-mid-write recovery (verify file state is consistent after partial writes).

### What we explicitly do NOT do

- Re-platform the orchestrator to be an MCP client. (Adds complexity, no benefit since the orchestrator owns the file substrate.)
- Move authoritative state out of the filesystem. (Files stay canonical; MCP is a surface, not a sink.)
- Promise streaming subscriptions in v1. (Polling works; subscriptions can come later if a real need emerges.)
- Migrate existing skills off file-handshakes. (Skills under Claude Code continue to use `touch $AIDA_EXIT_SENTINEL` and the punt-signal file. MCP-mediated coordination is for agents *outside* the Claude Code path.)

## Risks + gotchas

- **Concurrent writer contention** — if both an MCP client and a file-direct writer modify the same `.aida/` file simultaneously, lockless writes corrupt. Today AIDA uses file locks at the orphan-store layer; MCP tools must respect them. Mitigation: each MCP tool acquires the same locks the file-direct path uses.
- **Server lifecycle in a single-machine no-cross-agent context** — the MCP server doesn't *need* to be running for the orchestrator to function. Mitigation: keep MCP server fully optional; `aida init` doesn't start it; users opt in by running `aida mcp-serve` when they want it. Document this clearly.
- **Schema drift between MCP tool contracts and the underlying file formats** — file formats evolve organically; tool schemas must track. Mitigation: each MCP tool's implementation calls the same internal Rust functions the CLI uses, so schema drift is detected at compile time.
- **Authentication for cross-machine MCP** — out of scope for v1 (single-machine local-socket assumed). Cross-machine deployment is its own follow-up SPIKE.
- **The orchestrator-as-MCP-client temptation** — future maintainers may argue "let's just make the orchestrator an MCP client for symmetry." Resist. The orchestrator owns the substrate; making it a remote client adds latency, complexity, and failure modes for no benefit in the single-machine local case.

## Verdict

**Hybrid, with file-handshakes as canonical and MCP as a coordination-surface layer.**

For the orchestrator ↔ skill case (single-machine, Claude-Code-only, today's deployment): file-handshakes stay. They're simple, fast, debuggable, and have no service-lifecycle concerns.

For cross-agent and cross-machine cases (Codex / Cursor / future agents, multi-machine drains): MCP-as-bus is the right transport. Extending `aida-cli/src/mcp.rs` with coordination tools is contained work — ~15 new tools mapping to ~15 existing file conventions, with the orchestrator unchanged.

This positions AIDA's MCP server as **the agent-agnostic coordination surface beneath the IDE-agent war.** Each agent's IDE integration is its own product; the substrate they coordinate against is shared. That's the moat positioning called out in CLAUDE.md ("the MCP server is the highest-leverage surface for the agent-context vision") made operational.

## Smallest-valuable-slice — implementation story to file

After this SPIKE merges, file:

**STORY-N: Extend AIDA MCP server with coordination tools (SPIKE-9 outcome)**

- Add 12-15 new tools to `aida-cli/src/mcp.rs` mapping to existing file conventions
- No orchestrator changes
- Schema documentation for each new tool
- Tests for happy-path + concurrent-write contention + crash recovery
- Update `docs/architecture/` with the "filesystem-canonical, MCP-transport" pattern
- README mention: "MCP coordination surface — any MCP-speaking agent can participate in AIDA drains"

Estimated complexity: medium. Touches one file substantially; doesn't change architecture. ~3-4 sessions of focused implementation work.

## Related

- **SPIKE-11** — fork-from-live advisor; orthogonal direction (advisor-context architecture, not transport)
- **STORY-306** — headless advisor escalation; uses file-handshakes today, would gain MCP equivalents via this story's implementation
- **STORY-285** — findings filing; same
- **TASK-294** — directive channel; will compose with `post_directive` / `list_directives` if/when both ship
- **TASK-337** — `docs/positioning/vs-claude-code-subagents.md`; positioning of AIDA's MCP coordination vs Claude Code Agent Teams
- **Claude Code Agent Teams** (`CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1`) — Claude-Code-locked equivalent that AIDA's MCP-as-coordination-surface supersedes for cross-agent use
