# Claude-Specific Surfaces — Codex Parity Inventory

Status: ground-truth inventory from 2026-06-25. Each row is sourced from code
or a scaffolded file, not from intent. Refresh when AIDA's launcher, hooks,
headless drain, or TUI host change, or when Codex's hook/non-interactive
surface changes.

This is the gap map for the migration-readiness epic: a complete catalogue of
every place AIDA currently depends on a Claude-specific surface, what the
surface does, the nearest non-Claude (Codex) equivalent, and the concrete gap
plus the spec that covers it. The companion conceptual analysis is
`docs/agents/porting-claude-code-to-codex.md`; the durable hook/defer reference
is `docs/agents/session-communication.md`. This file is deliberately concrete:
file and symbol references so a future implementer can find the exact code that
hardcodes a vendor.

The framing is vendor-neutral. "Claude" below means "the specific agent client
AIDA hardcodes today"; "Codex" stands in for "a non-Claude MCP-speaking agent
client." The migration target is not a Codex clone of Claude wiring — it is to
push every load-bearing invariant down into AIDA, git, CI, or MCP, and treat
each per-agent surface as a thin, replaceable adapter.

## How to read the tables

Each surface is classified by what it actually does:

- **observation** — logging, metrics, transcript or commit capture, notices.
- **guidance** — soft context injection; advisory text the agent may ignore.
- **enforcement** — blocks unsafe work, gates a transition, refuses an action.
- **mutation** — rewrites tool input/output or injects required context.
- **approval** — routes a human/advisor decision before work proceeds.
- **session control** — pause / abort / resume / continuation of a run.
- **UX** — status display, onboarding, shortcuts, convenience.

"Coverage" names the EPIC-0419 child spec that owns the replacement work, or
**uncovered** when no spec yet exists.

## 1. `.claude/` hooks

Hook wiring lives in `aida-core/templates/settings.json` (symlinked to
`.claude/settings.json`); the scripts live in `aida-core/templates/hooks/`
(symlinked into `.claude/hooks/` for the hooks this repo wires up). Claude
fires these on its own lifecycle events; Codex exposes a narrower event set
(`PreToolUse`, `PermissionRequest`, `PostToolUse`, `PreCompact`, `PostCompact`,
`UserPromptSubmit`, `SessionStart`, `Stop`, `SubagentStart`, `SubagentStop`)
and, per current docs, command-type handlers only — no `defer`, no documented
input/output mutation. The classification drives where each one should re-home.

| Hook (event) | What it does | Class | Codex / non-Claude equivalent | Gap + coverage |
|---|---|---|---|---|
| `aida-validate-commit.sh` (PreToolUse) | Validates the commit message shape before a `git commit` tool call | enforcement | Codex command hook on `PreToolUse`, or — better — the git `commit-msg` hook AIDA already scaffolds (`aida-commit-msg`) | The git hook is the portable boundary; the Claude hook is a redundant early warning. Move enforcement to the git hook for non-Claude agents. Coverage: TASK-0427 (decision matrix: enforcement belongs outside the agent runtime) |
| `aida-git-guardrails.sh` (PreToolUse) | Blocks dangerous git operations (force-push main, etc.) before the Bash tool runs | enforcement | Codex `PreToolUse` command hook can run the same script; Codex sandbox/approval profiles are the stronger boundary | Codex command hooks can host the same script, but the hard guarantee should be a git pre-push hook + sandbox, not an agent hook. Coverage: TASK-0423 (sandbox strategy) + TASK-0427 |
| `aida-advisor-code-guard.sh` (PreToolUse, Edit/Write matcher) | Prevents an advisor-role session from editing code (role discipline) | enforcement | No direct Codex equivalent: depends on Claude's per-matcher `PreToolUse` blocking. Substrate equivalent: role-gated MCP write tools refusing the write | Load-bearing role discipline currently enforced in the Claude hook only. Needs an AIDA-side gate (MCP/CLI refuses code writes for advisor role). Coverage: TASK-0422 (re-home defer/role hook semantics to durable AIDA state) — **partial**; a dedicated role-write-gate spec is **uncovered** |
| `aida-track-commits.sh` (PostToolUse) | After a `git commit`, records the commit→spec linkage | observation | Codex `PostToolUse` command hook, or a git `post-commit` hook | Straightforward port; observation, not load-bearing. Prefer a git hook (agent-agnostic). Coverage: TASK-0427 |
| `aida-role-context.sh` (SessionStart) | Injects the active role's context snapshot at session start | guidance | Codex `SessionStart` command hook; or `AGENTS.md` + `aida agent new --show-context` startup snapshot | Codex has `SessionStart`; port is mechanical. Already partly covered by the launcher's role-context snapshot. Coverage: TASK-0424 (template/scaffold readiness) |
| `aida-mail-notice.sh` (SessionStart + UserPromptSubmit) | Surfaces unread mailbox messages each turn | observation/guidance | Codex has `SessionStart` and `UserPromptSubmit`; same script can run there | Mechanical port; the mail state itself is already MCP-readable (`read_inbox`). Coverage: TASK-0424 |
| `aida-subagent-start.sh` (SubagentStart) | Records subagent spawn for the registry/leases | observation | Codex has `SubagentStart` | Mechanical port; depends only on Codex's subagent model existing. Coverage: TASK-0424 / uncovered for subagent-model differences |
| `aida-subagent-stop.sh` (SubagentStop) | Records subagent completion | observation | Codex has `SubagentStop` | Mechanical port. Coverage: TASK-0424 / uncovered |
| `aida-capture-prompt.sh` (Stop) | At run end, captures the session for `/aida-capture` requirement review | observation | Codex has `Stop` | Mechanical port; the capture skill itself is Claude-specific (see §3). Coverage: TASK-0422 (session-end semantics) |

Hooks scaffolded but **not** wired into this repo's `settings.json` —
`aida-pre-commit.sh`, `aida-session-context.sh`, `aida-stop-check.sh`,
`aida-store-pair.sh`, and the `aida-commit-msg` git hook — are git-hook /
script-class boundaries already, so they are the *correct* portable shape and
need no Claude-to-Codex migration: they run from git or the shell regardless of
agent client.

## 2. `.claude/settings.json` — config + statusline

| Surface | What it does | Class | Codex equivalent | Gap + coverage |
|---|---|---|---|---|
| `settings.json` `hooks` block | Maps Claude lifecycle events to the command hooks in §1 | enforcement/observation | `.codex/config.toml` / `.codex/hooks.json` (project) or `~/.codex/hooks.json` | A whole-file analog is needed; Codex's event set is narrower (no defer, no mutation). Coverage: TASK-0422 (defer/mutation) + TASK-0424 (scaffold the Codex equivalent) |
| `settings.json` `statusLine` (`aida statusline ...`) | Command-backed status footer showing role/queue/scope | UX | Codex built-in TUI footer fields (`[tui] status_line = [...]`) — fixed field set, no arbitrary command | Codex cannot run an arbitrary status command in its footer today. `aida statusline` survives in the shell prompt / tmux; the in-agent footer loses richness. Coverage: TASK-0427 (status/observability axis) — **gap is documented, no shim spec**; uncovered for an in-footer parity shim |
| `$CLAUDE_PROJECT_DIR` path convention in hook commands | Resolves hook scripts regardless of CWD | enforcement plumbing | Codex hook env vars differ; project trust must be granted | Hook command paths must be rewritten for Codex's env + trust model. Coverage: TASK-0424 |

## 3. `.claude/skills` and `.claude/commands`

`aida init` scaffolds 47 skills under `.claude/skills/` and matching slash
commands under `.claude/commands/` (master copies in
`aida-core/templates/{skills,commands}/`). These are the daily-driver workflow
shortcuts (`/aida-req`, `/aida-implement`, `/aida-commit`, `/aida-capture`,
`/aida-drain-queue`, etc.).

| Surface | What it does | Class | Codex equivalent | Gap + coverage |
|---|---|---|---|---|
| `.claude/skills/*` (47 skills) | Reusable agent workflows with progressive disclosure | guidance/UX | Codex skills *only where genuinely reusable*; otherwise AIDA CLI verbs + docs | Most skills wrap an AIDA CLI/MCP verb that already works agent-agnostically — the skill is convenience, not capability. A handful (`/aida-advise` headless tier, `/aida-burndown`, `/aida-solo`) encode orchestration logic. Coverage: TASK-0424 (which skills to port to Codex skills vs leave as CLI) |
| `.claude/commands/*` slash commands | Claude-native invocation of the skills | UX | **No Codex custom slash-command surface** for arbitrary project commands; map to AIDA CLI verbs, project scripts (`make`/`just`), or Codex built-ins | Slash commands have no direct Codex analog. The underlying behavior must already exist as an `aida` verb (it does, for the daily drivers). Coverage: TASK-0427 (slash-command axis) + TASK-0424 |
| `/aida-advise` skill (headless advisor tier) | Spawned by the `--no-human=both` drain on a punt to resolve/escalate a design fork | approval/session control | No Codex equivalent — it is invoked by AIDA's orchestrator via `claude -p` (see §5) | Tightly coupled to the headless `claude -p` advisor spawn; porting requires a `codex exec` advisor path. Coverage: TASK-0422 (defer/approval re-home) — **partial**; the headless advisor spawn is **uncovered** for Codex |

## 4. `CLAUDE.md` vs `AGENTS.md`

| Surface | What it does | Class | Codex equivalent | Gap + coverage |
|---|---|---|---|---|
| `CLAUDE.md` (repo root) | Primary durable instruction surface for Claude sessions | guidance | `AGENTS.md` (Codex-native) + `docs/aida/discipline/` | `aida init` already scaffolds both `CLAUDE.md` and `AGENTS.md`. The risk is drift: agent-neutral discipline that lives only in `CLAUDE.md`. Coverage: TASK-0424 (template completeness) + TASK-0426 (positioning/cold-reader clarity) |
| `CLAUDE.md` `@`-imports (discipline README) | Transitively loads `docs/aida/discipline/README.md` into context | guidance | `AGENTS.md` references; Codex has no identical `@`-import transitive-load semantics | The discipline pack is plain markdown, so it ports as references. The auto-load mechanic is Claude-specific but non-load-bearing. Coverage: TASK-0424 |

## 5. The headless path — `claude -p` vs `codex exec`

This is the **highest-risk gap**. AIDA's autonomous drain and headless advisor
tier hardcode `claude -p`. The orchestrator phases (implementer, CI wait,
reviewer, advisor) are driven by Claude-specific spawn helpers in
`aida-cli/src/session.rs`.

| Surface (code) | What it does | Class | Codex equivalent | Gap + coverage |
|---|---|---|---|---|
| `session::spawn_claude_headless` + `claude_headless_args` (`session.rs`) | Spawns the headless drain worker: `claude -p --permission-mode bypassPermissions --output-format stream-json --verbose --disallowed-tools AskUserQuestion --session-id <uuid> <prompt>` | session control / enforcement | `codex exec [--json] [--output-schema] <prompt>` | Hardcoded Claude binary + Claude-only flags. No `codex exec` drain path exists. The `--disallowed-tools AskUserQuestion` structural gate has no Codex flag analog. Coverage: TASK-0422 (session-comm replacement) + TASK-0425 (validation runbook) — **the orchestrator drain itself is uncovered for Codex**; needs an implementation child spec |
| `session::spawn_claude_resume` / `advisor_watch::spawn_claude_headless_resume` | Resumes a deferred/parked headless run with `claude --resume <id>` | session control | `codex exec` has no documented resumable-pending-tool-call equivalent | Resume-after-defer is Claude-specific (see §6). For Codex the pattern is "record durable state, start a fresh run." Coverage: TASK-0422 |
| `auto_complete.rs` orchestrator phases | Drives implementer → CI → reviewer → advisor through headless `claude -p` calls | session control / approval | Would need per-phase `codex exec` invocations | All phase spawns assume Claude. Coverage: TASK-0425 (validation) — **uncovered** for an actual Codex drain implementation |
| `compete.rs` `vendor_adapter("codex")` → `codex exec --dangerously-bypass-approvals-and-sandbox` | The N-way "compete" feature *already* has a Codex headless adapter | session control | This IS the Codex equivalent, but only for the experimental compete feature | A working `codex exec` adapter already exists in-tree for `aida compete`, but the **main orchestrator drain does not reuse it** (`compete.rs:50` notes the claude argv intentionally does not reuse `session::claude_headless_args`). Reusable prior art for the drain port. Coverage: TASK-0425 / uncovered for drain reuse |

## 6. `defer` / resume + session communication

| Surface | What it does | Class | Codex equivalent | Gap + coverage |
|---|---|---|---|---|
| Claude hook `permissionDecision: "defer"` | Headless run exits `stop_reason: "tool_deferred"`, preserves the pending tool call, resumable via `--resume` after an external decision | session control / approval | **No documented Codex hook-level `defer` primitive** | The single most important migration gap (per `porting-claude-code-to-codex.md`). The portable replacement is durable AIDA state: post a punt/finding/directive, park `needs-attention`, stop, and start a *fresh* run after approval. Coverage: TASK-0422 (designs the substrate replacement) |
| `continue: false` terminal stop | Hook halts the Claude run, with a notification side effect | session control | Codex `Stop`-event command hook can notify, but stop semantics differ | Notification must be emitted by the component that stops the run, not a later post-hook. Coverage: TASK-0422 |
| "block in PreToolUse, ask in PostToolUse" anti-pattern | Documented as impossible in Claude; relevant when porting | guidance | Same ordering constraint applies; design checkpoints in the orchestrator instead | Documentation-level; covered by `session-communication.md`. Coverage: TASK-0422 |

The durable AIDA substitute already exists and is agent-agnostic: punts
(`post_punt`), findings (`file_finding`), directives (`post_directive`),
briefs, queue state, and `needs-attention` status — all reachable via MCP and
CLI. The gap is not the substrate; it is that the headless orchestrator
*invokes* the substrate through a Claude-only spawn/resume loop (§5).

## 7. MCP client config — `.mcp.json` vs `.codex/config.toml`

| Surface | What it does | Class | Codex equivalent | Gap + coverage |
|---|---|---|---|---|
| `.mcp.json` (repo root) | Registers `aida mcp-serve` as a Claude Code MCP server | enforcement plumbing | `.codex/config.toml` `[mcp_servers.aida]` block, or `codex mcp add aida -- aida mcp-serve` | **Already covered and shipped.** `aida init` scaffolds `.codex/config.toml` (TASK-0424, merged, `aida-core/src/scaffolding/codex_md.rs::generate_codex_config`) alongside `.mcp.json`. Init also auto-runs `codex mcp add` when the `codex` binary is present (`main.rs`). Coverage: TASK-0424 (DONE) |
| MCP tool surface (~58–60 tools) | The spec graph + coordination tools both agents call | n/a (agent-agnostic) | Identical — MCP is the vendor-neutral layer | No gap. This is the layer the whole migration leans on. Setup details: `docs/agents/codex-mcp-setup.md` |

## 8. Launcher + background supervisor

| Surface (code) | What it does | Class | Codex equivalent | Gap + coverage |
|---|---|---|---|---|
| `aida agent new <claude\|codex\|antigravity>` interactive launcher (`main.rs`) | Resolves the agent binary by type, registers it, snapshots role context, spawns it interactively | session control | **Already vendor-aware** — accepts `codex` and resolves the codex binary | The dispatch (`main.rs`, the `config.agent_type == "claude"` branch) already supports non-Claude types for the bare spawn. Coverage: works today |
| `os_wrap` (bwrap) confinement around the launch | Optional OS sandbox boundary around the spawned process | enforcement | Codex-native sandbox / permission profiles | **Claude-only:** the `os_wrap` wrapping branch fires only when `config.agent_type == "claude"`; non-Claude agents take the bare path (`main.rs`, `else` branch — "Non-claude agents keep the bare path"). Same for headless (`session::spawn_claude_headless`). Coverage: TASK-0423 (decide whether Codex needs an AIDA outer wrapper or whether Codex-native sandbox suffices) |
| Background dispatch / supervisor (`aida agent new` `--bg` path, AIDA_USER env) | Launches an agent detached with its own mailbox/queue identity | session control | A `codex exec` background run + the same registry/identity env | The registry/identity plumbing is agent-agnostic; the detached *worker* still routes to the Claude headless spawn for drains. Coverage: TASK-0425 / uncovered for a Codex background worker |
| Pending-brief banner on agent exit | Warns claude/codex/antigravity sessions of unacked briefs before exit | observation | Already vendor-aware (`pending_brief_banner_lines` matches all three) | No gap |

## 9. The TUI PTY host

| Surface (code) | What it does | Class | Codex equivalent | Gap + coverage |
|---|---|---|---|---|
| `aida-tui` `PtyHost::spawn(argv, ...)` (`aida-tui/src/pty.rs`) | Opens a PTY and spawns an arbitrary argv inside it | n/a (argv-agnostic) | Could host any agent — the host itself is not Claude-specific | The PTY host is fully generic; it spawns whatever argv it is handed |
| TUI tab argv (`app.rs::spawn_tab`) | Builds `aida queue work <scope> --session-id/--resume <id>` as the hosted argv | session control | Would need a Codex-flavored hosted session | The TUI hosts `aida queue work`, which in turn calls `session::spawn_claude_session` (claude-only) and threads Claude `--session-id`/`--resume` flags. So the **host is generic but the hosted session is Claude-bound**. Coverage: **uncovered** — no spec yet covers a Codex-hosting TUI session; closest is TASK-0425 (validation) which would surface it |
| TUI crash-recovery re-attach (`--resume <claude_session_id>`) | Re-attaches recorded Claude session ids after a crash | session control | Codex session-resume semantics differ | Recorded ids are Claude session ids; resume is Claude-specific. Coverage: uncovered |

## Coverage summary

**Already shipped / no gap:**

- MCP client config for Codex (`.codex/config.toml`) — TASK-0424, merged.
- The MCP tool surface itself — vendor-neutral by construction.
- `aida agent new codex` bare interactive spawn — launcher is vendor-aware.
- The durable coordination substrate (punts, findings, directives, briefs,
  queue, needs-attention) — agent-agnostic; reachable from any MCP client.
- The TUI PTY host mechanism — argv-agnostic.
- The non-wired git-hook / script-class boundaries — already portable.

**Covered by an existing EPIC-0419 child spec (design/decision work pending):**

- Hook re-homing + classification — TASK-0427 (decision matrix), TASK-0424
  (Codex hook scaffold).
- `defer`/resume + session communication → durable AIDA state — TASK-0422.
- Outer OS sandbox (`os_wrap`) for Codex launches — TASK-0423.
- `SessionStart` / mail-notice / role-context hook ports — TASK-0424.
- Skill/command mapping to Codex skills vs AIDA CLI — TASK-0424, TASK-0427.
- End-to-end Codex validation through MCP/queue/brief — TASK-0425.
- Cold-reader positioning of `CLAUDE.md`→`AGENTS.md` split — TASK-0426.

**Uncovered gaps found (need new child specs):**

1. **Headless orchestrator drain on Codex.** `session::spawn_claude_headless`,
   `auto_complete.rs` phase spawns, and `advisor_watch` resume are Claude-only.
   The `codex exec` adapter in `compete.rs` is reusable prior art but the main
   drain does not use it. TASK-0425 only *validates*; no spec implements a
   `codex exec` drain worker. **High-risk: this is the autonomy keystone.**
2. **Headless advisor tier (`/aida-advise`) on Codex.** Spawned via `claude -p`
   on a punt; no Codex spawn path. TASK-0422 designs the substrate, not the
   Codex advisor invocation.
3. **Advisor-role code-write gate as a substrate gate.** Today enforced only by
   the `aida-advisor-code-guard.sh` Claude hook (`PreToolUse` Edit/Write
   matcher). For a non-Claude agent the invariant evaporates unless an MCP/CLI
   write gate refuses code writes for the advisor role. (Substrate-as-bouncer.)
4. **TUI hosting a Codex session.** `app.rs::spawn_tab` hosts
   `aida queue work`, which calls the Claude-only `spawn_claude_session` and
   threads Claude `--session-id`/`--resume` flags. No spec covers a Codex-hosted
   TUI tab or Codex crash-recovery resume.
5. **In-agent status footer parity.** Codex's footer is a fixed field set; the
   command-backed `aida statusline` richness is lost inside the agent (it
   survives in the shell prompt / tmux). TASK-0427 names the axis but no shim
   spec exists. (Low severity — UX, not load-bearing.)

The strategic read matches the porting doc's thesis: the load-bearing
invariants (graph, IDs, traces, MCP, queue, leases, briefs, punts, findings,
commit trailers, git hooks, CI) are already agent-agnostic and need no port.
The real Claude coupling is concentrated in the **headless orchestrator drain
and its advisor tier** (§5, gaps 1–2), the **outer sandbox wiring** (§8,
TASK-0423), the **role-write enforcement hook** (gap 3), and the **TUI-hosted
session** (gap 4). Those four are where a "no Claude" mandate would actually
bite — everything else is convenience or already covered.

## References

- `docs/agents/porting-claude-code-to-codex.md` — conceptual migration analysis
- `docs/agents/session-communication.md` — durable hook/defer/resume reference
- `docs/agents/cross-agent-onboarding.md` — MCP onboarding for non-Claude agents
- `docs/agents/codex-mcp-setup.md` — working Codex MCP registration
- `docs/agents/per-agent-config.md` — per-agent permission posture + `os_wrap`
- `docs/agents/claude-bubblewrap-sandbox.md` — `os_wrap` outer OS boundary
- `aida-core/src/scaffolding/codex_md.rs` — `.codex/config.toml` + `AGENTS.md` scaffolding
- `aida-cli/src/session.rs` — `spawn_claude_headless`, `claude_headless_args`, `os_wrap` helpers
- `aida-cli/src/auto_complete.rs` — orchestrator drain phases
- `aida-cli/src/compete.rs` — existing `codex exec` vendor adapter (reusable prior art)
- `aida-tui/src/pty.rs`, `aida-tui/src/app.rs` — TUI PTY host + hosted argv
