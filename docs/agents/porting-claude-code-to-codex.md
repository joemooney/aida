# Porting From Claude Code To Codex CLI

Status: operational comparison, last fact-checked against current Codex docs on
2026-06-24. Refresh this document before making binding architecture decisions,
because both agent CLIs are moving quickly — the 2026 deltas below show how fast
the gap moves.

This document is for advanced users and AIDA operators deciding whether Codex
CLI can replace Claude Code for local development, autonomous drains, or
cross-agent workflow. It focuses on hooks and tooling that affect workflow
correctness, not model quality.

> **What changed recently (verified 2026-06-24).** Several gaps this document
> originally described have narrowed — verify against your own Codex version
> before relying on the older framing:
>
> - **Codex now has subagents** (`.codex/agents/*.toml`, built-ins
>   `default`/`worker`/`explorer`, `max_threads`/`max_depth`, plus
>   `SubagentStart`/`SubagentStop` hook events). "Subagent/team workflows are
>   Claude-only" is no longer true.
> - **Codex hooks can now mutate tool I/O**: `PreToolUse` can rewrite a call via
>   `updatedInput`, `PostToolUse` can replace results, and several events accept
>   a `systemMessage` for context injection.
> - **A native migration skill exists** — `codex --skill migrate-to-codex`
>   imports `CLAUDE.md`, settings, skills/slash-commands, hooks, MCP servers,
>   subagents, and recent sessions. Hand-porting is no longer the only path.
> - **A narrow continuation primitive exists** on the `Stop` event
>   (`{ decision: "block", reason: … }` auto-continues). The genuine remaining
>   gap is *pending-tool-call* `defer`/resume, not continuation in general.

## Short Answer

Codex CLI can replace Claude Code for local repo work, MCP-backed
coordination, sandboxed command execution, reviews, scripted `codex exec`
automation, and — as of 2026 — parallel subagent fan-out and tool-I/O-mutating
hooks.

The genuine remaining gaps are narrower than they were. Codex CLI is still not a
drop-in replacement when a workflow depends on Claude Code's **pending-tool-call
`defer`/resume** semantics (pause a specific tool call, let an external authority
decide, resume the same call), **command-backed status lines**, or muscle memory
around `.claude/commands`. Most other 2025-era gaps (subagents, hook mutation,
import tooling) have since closed or narrowed — see "What changed" above and the
updated hook table.

For AIDA, the right migration target is:

> Codex-compatible AIDA discipline, not a Claude Code clone.

Keep correctness in AIDA, git, and MCP. Treat per-agent hooks, commands, and
skills as adapters.

## Strengths, Tradeoffs, And Migration Risk

Claude Code's current strength is that it is a rich agent runtime, not just a
model-backed terminal. Its advantage is most visible when a workflow depends on
runtime control:

- broad lifecycle hooks;
- hook decisions that can ask, deny, or **defer** a pending tool call (the
  `defer`/resume primitive is still the clearest Claude-only hook capability);
- headless pause/resume patterns;
- command-backed status lines;
- `.claude/commands` and Claude-shaped workflow shortcuts;
- a large and fast-moving Claude Code ecosystem.

Note (2026): several items that were Claude-only when this section was first
written have since landed in Codex — subagents and hook-level tool-I/O mutation
in particular (see "What changed" above and the hook table). Claude's edge has
narrowed to the `defer`/resume continuation model, command-backed status lines,
and ecosystem maturity rather than a categorical runtime-feature gap.

Codex CLI's current strength is a clean OpenAI-native execution environment
with strong local controls and automation surfaces:

- explicit sandbox and permission profiles;
- first-class `AGENTS.md` repo instructions;
- CLI/IDE shared configuration;
- strong MCP support across stdio and HTTP transports;
- `codex exec` for non-interactive runs;
- JSONL event streams and final output schemas;
- Codex skills and plugins;
- app/connectors integration;
- OpenTelemetry and policy-oriented configuration.

The danger in migrating is not that Codex cannot do useful work. It can. The
danger is losing implicit workflow guarantees that were accidentally provided by
Claude Code-specific surfaces. Common losses include:

- a hook no longer blocks the same event;
- a stopped run no longer has a resumable pending tool call (the real `defer`
  gap — Codex's `Stop`-event `{decision:"block"}` continuation is *not* the same
  thing as resuming a specific paused tool call);
- a command-backed status line disappears;
- a Claude slash command had no real script behind it;
- a Claude sandbox covered a different surface than Codex's sandbox;
- a hook-mutated prompt/tool input is assumed lost — *verify first*: Codex now
  supports `PreToolUse` `updatedInput` rewriting and `PostToolUse` result
  replacement, so many mutation hooks port directly (a `PostToolUse` rewrite
  still cannot undo already-executed side effects, so re-check those);
- a subagent/team workflow is assumed to collapse to single-agent — *also verify
  first*: Codex subagents (`.codex/agents/*.toml`) cover the fan-out/delegation
  case; what does *not* port one-to-one is a persistent named-team UX.

There is also a future-capability risk. Claude Code and Codex CLI are both
shipping quickly, but not in identical directions. Betting exclusively on
Claude means gaining deeper Claude-native runtime features while increasing
vendor-specific coupling. Betting exclusively on Codex means gaining OpenAI
platform integration and Codex-native automation while giving up some mature
Claude runtime affordances today. A reversible migration keeps project
invariants outside either agent: scripts, MCP servers, CI, git hooks,
orchestrators, and durable docs.

The strategic rule is:

> Migrate workflows, not vibes. If a Claude feature was enforcing behavior,
> replace that enforcement with an equally concrete Codex, OS, CI, MCP, or
> project-level mechanism before declaring the migration complete.

## What AIDA Offers During This Migration

AIDA is a small, git-backed coordination layer for AI-assisted software work.
It keeps a durable graph of requirements, bugs, tasks, decisions, and their
relationships; links code changes back to those records with trace comments;
and exposes the graph to agents through a CLI and MCP server.

AIDA is also a work in progress. Its direction is more mature than its polish:
the core idea is a vendor-neutral substrate for project intent and
multi-agent coordination, but the product surface, onboarding path, and some
cross-agent workflows are still being hardened. Treat it as an emerging
infrastructure layer, not a finished replacement for an issue tracker, CI
system, or agent runtime.

AIDA becomes relevant when the migration exposes a deeper problem: too much of
the team's workflow lived inside one vendor's agent runtime. If Claude hooks,
slash commands, status lines, subagents, or memories were carrying project
state and safety rules, then switching agents is not just a CLI migration. It is
a workflow-portability problem.

AIDA's value in that situation is a vendor-neutral substrate:

- stable requirement and task IDs that survive agent changes;
- a git-canonical spec graph instead of private agent memory;
- code-to-intent trace comments;
- MCP tools that multiple agents can read and write;
- queues, leases, briefs, punts, findings, and directives for coordination;
- lifecycle conventions around planning, implementation, review, and shipping;
- commit trailers and traceability that do not depend on Claude or Codex.

That means AIDA can help a team move load-bearing workflow out of Claude Code
and into project-owned state. Codex then becomes one worker against the same
substrate, not the new place where all hidden process rules are re-created.
The tradeoff is that adopting AIDA means accepting some immature edges in return
for a deeper portability model than plain docs and scripts can usually provide.

Even if AIDA is not the right immediate dependency for a migration, it may be
interesting to people studying the AI coding-agent landscape and looking for a
project to collaborate on. The problems AIDA is trying to address are
infrastructure problems: durable project intent, cross-agent coordination,
traceability, lifecycle state, and vendor portability. If those are the gaps a
team sees while moving between Claude Code, Codex CLI, and other agents, AIDA
may be a vehicle they can help own, shape, and steer.

### When to consider AIDA

Consider AIDA during a Claude-to-Codex migration if several of these are true:

- multiple agents or humans work in the same repo and step on each other;
- work often loses context between sessions;
- Claude-specific commands or hooks encode project lifecycle rules;
- reviews need to know which requirement a code change implements;
- the team wants traceability from task to code to PR;
- you need an MCP-accessible coordination layer shared by Claude, Codex, and
  other tools;
- a future vendor switch is plausible and you want less agent lock-in.

In that setting, AIDA is not primarily a Codex migration tool. It is a way to
make the next migration less disruptive by putting durable project intent in
git and MCP instead of in one agent's local conventions.

### When not to add AIDA

Do not add AIDA just because you are trying Codex. It is probably too much
machinery if:

- one person works in one repo with simple issue tracking;
- Claude Code was used mostly as an interactive coding assistant;
- there are no durable specs, lifecycle states, or cross-agent handoffs to
  preserve;
- existing tools such as GitHub Issues, Linear, Jira, or plain scripts already
  carry the process clearly enough;
- the migration goal is only "use Codex instead of Claude for ad hoc edits."

For those users, the better path is usually lighter: write a good `AGENTS.md`,
move important slash-command behavior into scripts, configure Codex permissions
carefully, register any needed MCP servers, and verify the old workflow with a
real task.

### The practical adoption path

For a non-AIDA team that sees the portability problem, the pragmatic path is not
"install all of AIDA and redesign the process." Start with the smallest piece
that solves the migration pain:

1. Capture the existing Claude-dependent workflow in plain docs and scripts.
2. Add `AGENTS.md` so Codex and other agents get the durable repo rules.
3. Identify what needs structured tracking: requirements, bugs, review
   findings, handoffs, or blocked decisions.
4. Introduce AIDA only if that structured tracking needs stable IDs, graph
   relationships, MCP access, or multi-agent coordination.
5. Keep CI, git hooks, and project scripts as the hard enforcement layer.

The decision rule is simple: if the problem is "Codex needs instructions," use
`AGENTS.md`. If the problem is "our agent workflow needs durable shared state
that survives vendors," consider AIDA.

## Hook Capability Comparison

| Capability | Claude Code | Codex CLI |
|---|---|---|
| Hook config location | `~/.claude/settings.json`, `.claude/settings.json`, `.claude/settings.local.json`, managed policy, plugins, skills/agents | `~/.codex/hooks.json`, `~/.codex/config.toml`, project `.codex/hooks.json`, project `.codex/config.toml`, plugin-bundled `hooks/hooks.json`, and enterprise-managed `requirements.toml` `[hooks]` (managed hooks bypass `/hooks` trust) |
| Handler types | command, HTTP, MCP tool, prompt, agent | documented current runtime: command only; `prompt` and `agent` are parsed but skipped |
| Event coverage | broad lifecycle surface including `PreToolUse`, `PostToolUse`, tool failure/denial, notification, session, file/config/cwd changes, compaction, worktree, and message display events | narrower: `PreToolUse`, `PermissionRequest`, `PostToolUse`, `PreCompact`, `PostCompact`, `UserPromptSubmit`, `SessionStart`, `Stop`, `SubagentStart`, `SubagentStop` |
| Blocking mechanics | hook exit code and JSON can block or steer selected events | hook events exist, but current public Codex docs emphasize command execution/trust and do not expose the same broad Claude hook-control matrix |
| Tool permission decisions | `allow`, `deny`, `ask`, `defer` in Claude's hook model | `allow`/`deny` documented; **no** pending-tool-call `defer` (can't pause one tool call and resume it). A narrow continuation exists on `Stop`: returning `{decision:"block", reason:…}` auto-creates a continuation prompt — a turn-level loop, not a tool-call resume |
| Tool input/output rewriting | Claude SDK hook docs describe input/output/context mutation paths | **now supported** (changed in 2026): `PreToolUse` returns `permissionDecision:"allow"` + `updatedInput` to rewrite a call; `PostToolUse` can replace results; multiple events accept `systemMessage` for context injection. Caveat: a `PostToolUse` rewrite can't undo already-executed side effects |
| Hook trust | managed policy hooks plus project/user settings | non-managed command hooks require review/trust by hash; `/hooks` can inspect, trust, or disable them |
| Async hooks | supported in Claude SDK-style hooks | `async` is parsed but skipped for Codex command hooks today |

The most important migration gap is the **pending-tool-call `defer`**. In Claude
Code headless flows, a hook can defer a *specific pending tool call*, let an
external authority decide, and then resume the Claude session at that call. Codex
does have a *turn-level* continuation (the `Stop`-event `{decision:"block"}`
auto-continuation), but that is not the same primitive — it does not resume a
paused tool call. So the gap is narrower than "no continuation at all," but real:
for the deferred-approval pattern on Codex, use external orchestration — record
the pending decision in AIDA, stop or fail closed, and start/resume a new
Codex/AIDA run after approval.

See `docs/agents/session-communication.md` for the durable AIDA reference on
Claude `ask`, `continue: false`, and `defer` semantics.

## The `defer`/Resume Primitive, And What AIDA Actually Depends On

This section makes the `defer` gap concrete and — importantly — checks it
against what AIDA *actually* relies on, rather than what it could in principle
use. The conclusion matters for migration planning: AIDA loses far less than the
headline gap suggests, because it deliberately kept its invariants in the
substrate rather than in Claude's hook primitives.

### What `defer` is, concretely

In headless `claude -p` mode, a `PreToolUse` hook can return
`{"permissionDecision": "defer"}`. Claude exits with `stop_reason:
"tool_deferred"` and **preserves the pending tool call in session state**. An
external caller then:

1. reads the deferred tool-use payload;
2. asks an external authority (web UI, approval service, advisor, or human);
3. resumes the *same session at the same pending call* via
   `claude -p --resume <session-id>`;
4. ensures the hook now returns `allow` or `deny` per that decision.

The defining constraint: it only works for a **single** pending tool call — if
the model emitted several parallel calls, defer is ignored, because resume can
replay exactly one. Concretely, the capability lets you build a headless gate
that pauses *one specific action* — "block this `git push` until an advisor
approves, then continue the very same run from the very same push" — with no
re-run.

### What losing it would mean

If AIDA had built its approval gates on `defer`, the Codex migration would cost:

- a paused-and-resumed run becomes a stopped run plus a *fresh* run — the
  original turn's in-context reasoning is gone, replaced by a cold-boot resume
  (the same substrate-bounded-autonomy cost documented elsewhere in AIDA);
- "approve this exact tool call and continue" degrades to "park the work, decide
  out of band, start a new run with the decision in context";
- the approval can no longer be a momentary pause *inside* one agent turn.

### Why AIDA barely feels this (verified against the shipped hooks)

AIDA does **not** depend on `defer`. None of AIDA's shipped hooks emit
`permissionDecision`, `defer`, or `continue: false` — the `defer` pattern lives
in `session-communication.md` as a *reference for what Claude can do*, not as a
load-bearing AIDA path. AIDA's real escalation is substrate-based and already
portable: a headless implementer that hits a decision it cannot safely make
**punts** (parks the spec `NeedsAttention`, files a finding/punt), the
orchestrator routes that to a headless advisor tier or a human, and work resumes
as a *fresh* run carrying the recorded decision — exactly the "park, decide out
of band, resume with context" shape above. That is the agent-agnostic version of
`defer`, and it behaves identically under Claude, Codex, Antigravity, CI, or a
human at a keyboard.

So the honest verdict on `defer` is: **AIDA loses a capability it deliberately
never used.** A project that *did* couple its approval gates to `defer` would
have real migration work here; AIDA does not — which is the opening principle of
this document (keep the invariant in the substrate; treat the agent primitive as
an optional accelerator) paying off at exactly the moment a vendor switch tests
it.

### AIDA's actual Claude-Code dependency surface

Mapping what AIDA touches against verified Codex coverage (2026-06-24):

| Claude-Code capability AIDA uses | Depends on it for correctness? | Codex port status |
|---|---|---|
| Hook events: `PreToolUse`, `PostToolUse`, `SessionStart`, `Stop`, `UserPromptSubmit`, `SubagentStart`, `SubagentStop` | yes (observation/guidance hooks) | **all seven exist in Codex** (incl. SubagentStart/Stop) — port directly |
| Command-type (`.sh`) hook handlers | yes | Codex supports command hooks — port |
| `permissionDecision` / `defer` / `continue:false` | **no** (not emitted by any shipped hook) | n/a — nothing to port |
| `.claude/skills` + `.claude/commands` | no (convenience; real workflows live in AIDA CLI/MCP) | Codex skills, or AIDA CLI verbs |
| Command-backed status line (`statusLine.command = aida statusline`) | no (observability, not correctness) | **no Codex equivalent yet** (open FR #20043/#20244) — the one genuine gap |
| Starter memory pack under `~/.claude/projects/<slug>/memory/` | no (guidance) | lives Claude-side; mirror key rules into `AGENTS.md` |
| `os_wrap` around `claude -p` | only when outer OS confinement is required | Claude-launch-specific — needs a deliberate Codex wrapper (see the bubblewrap section) |

The pattern is consistent: **the only hard Claude-Code dependency AIDA carries is
the command-backed status line, and that is observability, not correctness.**
Everything load-bearing — the gates, the escalation, the lifecycle, the trace
graph — already lives in AIDA, git, and MCP. Until Codex ships a command-backed
status line, rebuild the at-a-glance surface with the AIDA TUI, a shell-prompt or
tmux integration, or `aida status` polling instead of the Claude footer.

## Tooling Comparison

Claude Code is stronger today for a deeply instrumented agent runtime:

- broader lifecycle hooks;
- richer hook handler types;
- Claude-native slash commands and `.claude/commands`;
- Claude skill and plugin integration;
- SDK-level hook control and the pending-tool-call `defer`/resume primitive;
- command-backed status line support;
- persistent named-team UX (Codex now has subagents for the fan-out/delegation
  case — see "What changed" — so this is a UX-maturity edge, not a capability gap).

Codex CLI is strong for OpenAI-native local execution:

- explicit sandbox and approval profiles;
- shared CLI/IDE `config.toml`;
- project-scoped `.codex/config.toml` in trusted repos;
- stdio and streamable HTTP MCP with OAuth or bearer auth;
- `codex exec` for non-interactive automation;
- JSONL event output and optional final output schemas;
- skills and plugins across Codex surfaces;
- app/connectors integration;
- OpenTelemetry export;
- `AGENTS.md` as the native repo instruction surface.

In AIDA terms, both can be productive agents if they use the same substrate:
`aida mcp-serve`, stable specs, leases, briefs, punts, findings, queues,
trace comments, and commit trailers.

## General Implications For Advanced Users

For a general advanced user, moving from Claude Code to Codex CLI is less about
renaming config files and more about changing where workflow guarantees live.
Claude Code exposes many workflow-control affordances directly in the agent
runtime. Codex CLI exposes strong local execution, sandboxing, MCP, skills,
plugins, and non-interactive automation, but some Claude runtime controls need
to move outward into shell scripts, CI, git hooks, task runners, or a project
orchestrator.

### Treat hooks as similar shape, different contract

Both tools have hook concepts, but they are not semantically identical. Before
porting, inventory each Claude hook by purpose:

- Observation: logging, metrics, transcript capture, notifications.
- Soft guidance: adding context, warning the user, reminding the agent.
- Enforcement: blocking unsafe commands, preventing writes, requiring approval.
- Mutation: rewriting tool input/output or injecting additional context.
- Continuation control: pausing and later resuming a pending action.

Observation and soft guidance are usually straightforward to port to Codex
command hooks or external wrappers. Enforcement should be moved to the most
reliable boundary available: sandbox/permission profiles, git hooks, CI,
pre-commit, a task runner, or an MCP server that refuses unsafe operations.
Mutation and continuation-control hooks require special attention because Codex
does not currently document Claude-equivalent hook mutation or `defer`
semantics.

### Move approval policy into Codex permissions

Claude hook-based approval workflows often encode local policy in scripts.
Codex has first-class approval and sandbox configuration, so prefer those for
baseline safety. Note these are **two orthogonal knobs**, not one — set them
independently:

- **Sandbox mode** (what the filesystem/network boundary is): `read-only`,
  `workspace-write`, or `danger-full-access`.
- **Approval mode** (`--ask-for-approval`: when Codex pauses to ask): `untrusted`,
  `on-request`, or `never` (`on-failure` is deprecated).

Typical combinations:

- `read-only` for analysis;
- `workspace-write` + `on-request` approvals for normal repo edits;
- `danger-full-access` only inside an externally isolated environment;
- permission profiles for durable filesystem and network boundaries;
- MCP per-server and per-tool approval settings for connected tools.

The practical shift is that Codex policy should start with sandbox and approval
configuration, then add hooks for project-specific checks. Avoid using hooks as
the only guardrail for file, network, or destructive tool access.

### Separate agent-native sandboxing from outer OS confinement

Claude Code, Codex CLI, project launchers, and host containers can all impose
different sandbox layers. Do not treat them as interchangeable:

- Agent-native sandboxing is part of the agent client's permission model. It
  controls what that client will ask, deny, or allow.
- Outer OS confinement wraps the whole agent process. It can limit damage even
  when the agent, a tool, or a plugin behaves unexpectedly.
- CI, VMs, dev containers, and managed workstations are environment boundaries.
  They can make broader agent permissions acceptable because the host is
  already isolated.

When moving to Codex, prefer Codex's native permission profiles as the first
line of defense, then add an outer wrapper only when you need a hard process
boundary. If you relied on a Claude-specific sandbox posture, re-audit the exact
coverage: some sandboxes govern Bash/tool behavior, not every process-level
read/write the launched agent can perform.

For Linux users who build their own wrapper, bubblewrap is a reasonable
write-confinement tool, but it has host prerequisites. Recent Ubuntu systems
may block unprivileged user namespaces unless an AppArmor profile or sysctl
allows them. A fail-closed wrapper is the defensible shape: if confinement
cannot be established, the agent should not launch unconfined under the
pretense of being sandboxed.

Universal sandbox migration checklist:

1. Identify the current boundary: agent-native sandbox, hook policy, OS wrapper,
   container, VM, CI runner, or some combination.
2. State what the boundary actually protects: writes, reads, network egress,
   subprocesses, MCP/app tools, credentials, or only shell commands.
3. Recreate the boundary in Codex with permission profiles first.
4. Add outer OS confinement only for gaps that Codex's native sandbox does not
   cover in your threat model.
5. Test failure modes: missing wrapper binary, blocked kernel feature, denied
   network, denied write, and a command that tries to escape the workspace.

### Replace Claude slash commands with durable commands

Claude users often build muscle memory around custom slash commands. In Codex,
map those to one of three surfaces:

- built-in Codex slash commands, when there is a direct equivalent;
- project scripts such as `make`, `just`, `npm run`, `cargo xtask`, or shell
  wrappers;
- Codex skills/plugins, when the command is really a reusable agent workflow.

If a command changes project state, prefer a real CLI or script that humans and
agents can both run. Agent slash commands are good UI; they should not be the
only implementation of the workflow.

### Re-home long-lived instructions

Claude-centric repositories often spread guidance across `CLAUDE.md`,
`.claude/commands`, `.claude/skills`, and hook prompts. Codex's native durable
instruction path is `AGENTS.md`, plus Codex skills for reusable workflows.

A clean migration usually creates this split:

- `AGENTS.md`: repo conventions, build/test commands, safety rules, review
  expectations, and agent-neutral workflow instructions.
- Codex skills: specialized workflows that need progressive disclosure,
  references, or helper scripts.
- Project scripts: deterministic actions that should not depend on model
  interpretation.
- Hooks: lifecycle checks around tool use or turn boundaries.

The best portable instruction is one that a human can also understand and a CI
job can partly enforce.

### Revisit MCP setup and trust boundaries

Both Claude Code and Codex CLI support MCP, but config locations, approval
defaults, OAuth flows, and project trust behavior differ. When moving to Codex:

- register MCP servers in `~/.codex/config.toml`, project `.codex/config.toml`,
  or with `codex mcp add`;
- mark required MCP servers as required only when startup should fail without
  them;
- configure `enabled_tools`, `disabled_tools`, and approval modes explicitly
  for broad or destructive servers;
- account for project trust, because project-local Codex config and hooks load
  only in trusted projects.

Do not assume a Claude MCP config file is consumed by Codex. Treat MCP
registration as a fresh client integration.

### Rebuild status and observability

Claude command-backed status lines and hook notifications may not map directly
to Codex. For Codex, combine:

- the built-in TUI footer/status-line fields;
- shell prompt or terminal multiplexer status commands;
- `codex exec --json` for automation event streams;
- OpenTelemetry when a team needs centralized logs;
- project-native logs or CI artifacts for durable evidence.

For advanced automation, `codex exec --json` is often more useful than trying
to infer state from terminal text. It gives a structured stream of thread,
turn, tool, and error events.

Two notes on the Codex `[tui] status_line`: its **default** is
`["spinner", "project"]` (set to `null` to disable), and the field set is richer
than the four-item example shown later in this document — e.g. `five-hour-limit`,
`weekly-limit`, `context-window-size`, and token counters. What Codex does *not*
yet have is Claude's **command-backed** status line (a script that emits the
line); that is an open feature request (codex issues #20043, #20244) and a
near-term watch item — so for now `aida statusline` cannot drive the Codex
footer the way it drives Claude's `statusLine.command`.

### Plan for non-interactive differences

Claude headless workflows that rely on hook-mediated `ask` or `defer` need a
new design. In Codex automation:

- choose sandbox and approval settings up front;
- use `codex exec` for scripted runs;
- use `--json` when an orchestrator needs events;
- use `--output-schema` for the final answer shape;
- stop and externalize state when human approval is required.

The important rule: do not assume a Codex hook stop is a resumable pending tool
call. If the workflow needs human approval mid-run, design an explicit
checkpoint in your orchestrator.

### Rebuild session communication as explicit state

Claude Code exposes specific pause/abort/resume semantics through hooks and
headless session resume. Codex sessions should be treated differently unless
and until Codex documents equivalent primitives.

For portable advanced workflows:

- make "needs human decision" a durable external state, not an implicit paused
  agent turn;
- record who must decide, what tool/action was blocked, and what inputs are
  needed to continue;
- resume by starting a new agent turn with that decision in context, not by
  assuming the original pending tool call still exists;
- send notifications as side effects of the component that stops the run;
- avoid designing "block now, ask in a later post-hook" flows, because a
  blocked tool call usually has no successful post-tool event to hang that on.

This pattern is more verbose than Claude's `defer` path, but it is portable
across Codex, Claude, Antigravity, CI, and human-operated scripts.

### Expect import to help, not finish the migration

Codex has a native migration skill for Claude Code setup —
`codex --skill migrate-to-codex` (useful flags: `--scan-only`, `--plan`,
`--doctor`, `--dry-run`). It imports `CLAUDE.md` → `AGENTS.md`,
`settings.json` → `config.toml`, skills/slash-commands → Codex skills, MCP
servers, hooks → `.codex/hooks/`, subagents → `.codex/agents/*.toml`, and recent
(~30-day) sessions → Codex threads. Advanced users should still treat it as a
**bootstrap**, not the finish line. Run `--scan-only`/`--plan` first to preview,
then after importing, audit:

- which instructions landed in Codex-readable surfaces;
- which hooks were imported, trusted, skipped, or need replacement;
- which slash-command workflows need project scripts or skills;
- which MCP servers need Codex-specific approval settings;
- which subagents imported, and whether their sandbox/model settings are right;
- which Claude-only semantics (pending-tool-call `defer`, command-backed status
  line) still have no Codex equivalent.

The final migration step is not "the files imported"; it is "the old workflow
can be run, observed, interrupted, and recovered under Codex semantics."

### Practical universal migration checklist

1. List every Claude hook, command, skill, MCP server, status-line command, and
   headless script currently in use.
2. Mark each item as observation, guidance, enforcement, mutation, approval, or
   UX.
3. Inventory sandbox boundaries and state exactly what each one protects.
4. Move enforcement into sandbox/permissions, CI, git hooks, task runners, or
   MCP server-side checks.
5. Move reusable workflows into Codex skills/plugins or durable project
   scripts.
6. Move repo instructions into `AGENTS.md`.
7. Re-register MCP servers in Codex and review tool approval settings.
8. Replace Claude headless `defer` flows with explicit orchestrator
   checkpoints.
9. Verify with one real task: inspect, edit, test, review diff, recover from a
   blocked action, and resume work.

## AIDA Migration Rules

Keep these vendor-neutral and load-bearing:

- `aida mcp-serve`;
- requirement graph operations;
- leases and session registry;
- briefs, punts, findings, directives, and queues;
- worktree discipline;
- trace comments;
- commit trailers;
- pre-commit/reviewer gates;
- `aida pr ship`.

Translate Claude-specific surfaces selectively:

| Claude Code surface | Codex/AIDA replacement |
|---|---|
| `CLAUDE.md` | `AGENTS.md` plus `docs/aida/discipline/` |
| `.mcp.json` | `.codex/config.toml` or `codex mcp add aida -- aida mcp-serve` |
| `.claude/skills` | Codex skills only where reusable; otherwise AIDA docs and CLI verbs |
| `.claude/commands` | AIDA CLI verbs / MCP tools for state-changing work; a Codex **skill** for a reusable prompt/workflow (Codex has no custom-slash-command files — `migrate-to-codex` converts Claude slash-commands into skills); a **built-in** Codex slash command only where a direct equivalent exists |
| `.claude/hooks` | Codex command hooks only for simple checks; otherwise AIDA gates |
| Claude command-backed status line | shell/tmux `aida statusline` plus Codex built-in footer fields |
| Claude `defer` approval loop | AIDA punt/finding/brief plus external orchestration and a later Codex run |
| AIDA `os_wrap` around headless `claude -p` | not yet a Codex replacement; use Codex native sandbox/permissions, or add a new AIDA wrapper path deliberately |

Do not port by copying every Claude hook into Codex. Start from the invariant:
if it must hold for correctness, enforce it in AIDA, git, CI, or the
orchestrator. Use Codex hooks as convenience checks around that core.

### Bubblewrap and AIDA containment

`docs/agents/claude-bubblewrap-sandbox.md` documents AIDA's `os_wrap`
mechanism. Despite the filename, the architectural point is not "Claude"; it is
"outer OS boundary around an unattended agent process."

Current AIDA caveats:

- `os_wrap` is opt-in under `[contained] os_wrap`.
- It is wired around headless Claude drain paths today, not interactive
  `aida agent new` launches and not generalized Codex launches.
- It is write-confinement by default: broad host reads remain possible unless
  strict read confinement is configured.
- Network control is separate from bubblewrap; egress is governed by
  `[contained] allowed_hosts` and `managed_domains_only`.
- It fails closed when `bwrap` is missing or unprivileged user namespaces are
  unavailable.

For a Codex migration, do not assume `os_wrap = true` automatically confines
Codex sessions. The immediate Codex posture should be Codex-native sandbox and
approval settings. If AIDA needs the same outer boundary for Codex, that should
be implemented as a tracked launcher/orchestrator change with the same
fail-closed semantics, host self-test, read/write/egress documentation, and
doctor output that the Claude drain wrapper has.

The portable lesson from `os_wrap` is:

- be explicit about whether the boundary is read, write, or network
  confinement;
- fail closed when the boundary cannot be established;
- prefer scoped AppArmor/userns exceptions over host-wide hardening changes on
  managed machines;
- verify with an actual escape/write/network test, not just a config flag.

## If There Is A Mandate To Stop Using Claude

A hard "no Claude Code" mandate is manageable only if AIDA treats Claude as one
adapter, not as the source of truth. The response should be deliberate:

1. Freeze new Claude-specific surface area.
   - Do not add new `.claude/commands`, `.claude/hooks`, or Claude-only skills
     unless they are needed only for a temporary transition.
   - Any new discipline must first land in AIDA docs, CLI/MCP, git hooks, CI,
     or reviewer gates.

2. Classify existing Claude dependencies.
   - Load-bearing: anything that prevents unsafe work, enforces lifecycle, gates
     merges, or routes human approval.
   - Convenience: shortcuts, prompts, status display, onboarding, command
     aliases, and UX sugar.
   - Retire convenience items after equivalents exist; reimplement load-bearing
     items in AIDA itself.

3. Replace launch and session flow.
   - Use `aida agent new codex --role <role>` for normal Codex sessions.
   - Use `aida agent new codex --spec <SPEC> --role implementer` for direct
     assigned work.
   - Keep sibling worktrees, leases, trace comments, and commit trailers
     unchanged.

4. Replace MCP setup.
   - Register AIDA with Codex:

     ```bash
     codex mcp add aida -- aida mcp-serve
     ```

   - Verify from a Codex session with `/mcp`.
   - Keep `tools/list` as canonical for tool names and schemas.

5. Replace Claude hook gates with substrate gates.
   - Pre-tool warnings become Codex command hooks only when best-effort is
     acceptable.
   - Required stop/approval behavior becomes an AIDA CLI/orchestrator gate,
     a git hook, CI, or a reviewer check.
   - Claude `defer` flows become durable AIDA state: punt, finding, directive,
     brief, queue state, or needs-attention status.

6. Replace Claude sandbox assumptions.
   - Use Codex-native sandbox/approval settings for interactive and
     non-interactive Codex sessions.
   - Do not rely on `[contained] os_wrap` for Codex until AIDA explicitly wraps
     Codex launches.
   - If outer OS confinement is mandatory, block the migration path or add a
     Codex-capable wrapper with fail-closed behavior before permitting
     unattended Codex work.

7. Replace session communication.
   - Convert Claude `ask`/`defer`/resume workflows into durable AIDA state and
     explicit follow-up runs.
   - Notifications should be emitted by the component that stops the run.
   - Do not rely on a later post-tool hook after a blocked pre-tool decision.

8. Replace operator visibility.
   - Use `aida status`, `aida statusline`, `aida session leases`, and the AIDA
     TUI/status overlay where available.
   - Configure Codex's built-in footer for complementary local state:

     ```toml
     [tui]
     status_line = ["model-with-reasoning", "context-remaining", "git-branch", "current-dir"]
     ```

9. Replace headless automation.
   - Use `codex exec` for non-interactive Codex runs.
   - Use `--json` when an orchestrator needs event streams.
   - Use output schemas only for final structured results, not for live tool
     approval semantics.
   - Do not assume a stopped Codex hook leaves a resumable pending tool call in
     the Claude `defer` sense.

10. Update docs and templates.
   - Make `AGENTS.md` the primary non-Claude instruction path.
   - Keep `CLAUDE.md` only as legacy or optional Claude support if policy
     allows checked-in Claude docs.
   - Ensure `docs/agents/codex-mcp-setup.md`,
     `docs/agents/codex-brief-pickup.md`, and this document are linked from
     cross-agent onboarding.

11. Run a transition verification pass.
   - Start a fresh Codex session through AIDA.
   - Read a spec through MCP.
   - Claim it through MCP.
   - Make a small traced edit.
   - Run targeted tests.
   - Mark the queue item done or punt through MCP.
   - Verify the same state through the CLI.

12. Keep an explicit exception path.
    - If a workflow still requires Claude-only `defer`, hook mutation, or
      status-line behavior, either keep that workflow blocked until AIDA owns
      the invariant or file a task to reimplement it outside Claude.
    - Do not silently emulate Claude-only semantics with prompt instructions.

The strategic version: a stop-Claude mandate should accelerate AIDA's
agent-agnostic architecture, not trigger a rushed reimplementation of Claude
inside Codex. The more invariants move into AIDA's substrate, the less any
future vendor mandate matters.

## Current Practical Recommendation

Use Codex CLI for AIDA implementer and reviewer sessions where the path is:

1. read the spec through MCP;
2. edit in a sibling worktree;
3. run tests;
4. coordinate through AIDA MCP/CLI;
5. ship through AIDA.

Keep Claude Code only for workflows that still depend on Claude-specific
hook-mediated pause/resume, rich lifecycle automation, or Claude-hosted TUI
behavior. If Claude must be removed, treat those workflows as migration tasks,
not as already-covered Codex behavior.

## References

- `docs/agents/session-communication.md`
- `docs/agents/claude-bubblewrap-sandbox.md`
- `docs/agents/codex-mcp-setup.md`
- `docs/agents/codex-brief-pickup.md`
- `docs/agents/per-agent-config.md`
- `docs/aida/discipline/agent-agnostic-vs-claude-specific.md`
- Claude Code hooks: `https://code.claude.com/docs/en/hooks`
- Claude Agent SDK hooks: `https://code.claude.com/docs/en/agent-sdk/hooks`
- Claude Code MCP: `https://code.claude.com/docs/en/mcp`
- Claude Code subagents: `https://code.claude.com/docs/en/sub-agents`
- Codex hooks: `https://developers.openai.com/codex/hooks`
- Codex MCP: `https://developers.openai.com/codex/mcp`
- Codex non-interactive mode: `https://developers.openai.com/codex/noninteractive`
- Codex slash commands: `https://developers.openai.com/codex/cli/slash-commands`
- Codex subagents: `https://developers.openai.com/codex/subagents`
- Codex config reference (sandbox, approval, `tui.status_line`): `https://developers.openai.com/codex/config-reference`
- Codex migration skill: `codex --skill migrate-to-codex` (see `https://developers.openai.com/codex/cli`)
- `AGENTS.md` guide: `https://developers.openai.com/codex/guides/agents-md`
