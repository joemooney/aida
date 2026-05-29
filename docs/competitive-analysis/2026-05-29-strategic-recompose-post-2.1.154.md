# Strategic Recompose Post Claude Code 2.1.154

**Date**: 2026-05-29
**Status**: synthesis of SPIKE-14, SPIKE-15, SPIKE-29 + four still-to-write surfaces
**Audience**: Joe + future AIDA-maintaining advisor sessions
**Verdict roll-up**: 7 axes reviewed. **0 reject. 7 compose.** Three of the seven also recommend partial **divest** (worktree creation, process supervision, single-machine dispatch verbs).

This is the integrated picture across all seven Claude Code 2.1.154 surfaces I've fetched — `/workflows`, `/agents` (agent view), `/agent-teams`, `/goal`, `/headless`, `/deep-links`, `/large-codebases`. The standalone SPIKE write-ups (some shipped, some pending) live in `2026-05-29-claude-code-2.1.154-decompose/`. This doc says what they collectively mean for AIDA's architecture.

---

## TL;DR

Anthropic is reimplementing AIDA's substrate primitives — sessions, leases, agent dispatch, multi-agent coordination, goal loops, per-folder rules, per-folder skills — **inside Claude Code's process model**. Their versions ship as a per-user supervisor process (`~/.claude/daemon/`), per-session state files (`~/.claude/jobs/<id>/state.json`), per-team configs (`~/.claude/teams/<name>/config.json`), `claude --bg`-style dispatch, structured JSON output (`claude agents --json` with `pid/cwd/kind/startedAt/sessionId/name/status`), and a workflow runtime (JS scripts, 16-concurrent/1000-total, resumable within a session).

**This collapses a lot of AIDA's surface area but does not collapse the moat.** Anthropic's versions are uniformly **single-machine + single-tool (Claude Code only) + single-session-scoped**. AIDA's git-canonical substrate stays strictly more capable on the three axes that matter:

1. **Cross-machine** — leases, briefs, queue, and lifecycle state replicate over git
2. **Cross-tool** — AIDA dispatches to Codex and Antigravity, not just Claude Code
3. **Cross-time / cross-session** — the spec graph, commit-trailer auto-bump, and trace comments persist beyond any single Claude Code supervisor lifetime

The right architectural move is to **stop reimplementing what Anthropic now ships natively, and compose with it instead**. Specifically:

- AIDA divests worktree creation, process supervision, and single-machine dispatch verbs
- AIDA exposes its substrate to Claude Code via scaffolded subagent defs, path-gated `.claude/rules/`, MCP, and workflow-compiler output
- AIDA's `aida agent new claude` wraps `claude --bg` rather than fork/launch itself
- AIDA's orchestrator becomes a **workflow.js compiler + supervisor**, not a runtime
- AIDA's spec graph is the truth Claude Code's runtime executes against

This is the "trojan horse" framing from CLAUDE.md (visible product = TUI; actual value = substrate) holding up exactly as expected — Anthropic now owns the visible runtime layer; AIDA owns the substrate beneath it. **The split is healthy.**

---

## The seven axes summarized

| # | Surface | Claude Code 2.1.154 ships | AIDA verdict | Divest? |
|---|---|---|---|---|
| 1 | **Workflows** | JS scripts, deterministic orchestration, 16-concurrent, structured output, resumable (SPIKE-14) | **COMPOSE** | Orchestrator as runtime → orchestrator as compiler |
| 2 | **Agent view** | `claude agents` TUI + `claude agents --json` + per-user supervisor + `~/.claude/jobs/<id>/state.json` + worktree isolation (SPIKE-15) | **COMPOSE + Divest** | Worktree creation, process supervision, `--bg` dispatch |
| 3 | **Agent teams** | EXPERIMENTAL — `~/.claude/teams/<name>/config.json`, lead/teammates topology, file-locked task list, mailbox communication (SPIKE-29) | **COMPOSE + Divest** | Single-tool team coordination |
| 4 | **/goal** | Prompt-based Stop-hook wrapper, Haiku-class evaluator running each turn | **COMPOSE** | `aida goal` → `claude /goal` as runtime |
| 5 | **/headless** | `claude -p` + `--output-format json` / `stream-json` + `--resume` + `--dangerously-skip-permissions` | **COMPOSE (already)** | AIDA already does this |
| 6 | **/deep-links** | `claude-cli://open?cwd=…&q=…&repo=…` URL scheme, inert until operator presses Enter | **COMPOSE** | New emit-side surface AIDA can adopt |
| 7 | **/large-codebases** | Per-directory `CLAUDE.md`, `claudeMdExcludes`, `Read` deny rules, `worktree.sparsePaths`, `additionalDirectories`, per-directory `.claude/skills/`, path-scoped `.claude/rules/`, code-intelligence plugins | **COMPOSE** | Some discipline-pack scaffolding overlap (positive — AIDA's existing pack already aligns) |

**No surface recommends rejecting AIDA functionality.** Every Claude Code addition is composable with AIDA's substrate. Three recommend divesting a piece of AIDA's runtime layer in favor of Anthropic's now-native version — and in each case the divest tightens AIDA's focus on the substrate moat.

---

## The compose architecture

```
┌────────────────────────────────────────────────────────────────┐
│  AIDA Substrate (git-canonical, orphan aida-store branch)      │
│  ─────────────────────────────────────────────────────────────  │
│  Specs · leases · briefs · lifecycle · traces · history         │
│  Cross-machine · cross-tool · cross-time                        │
└─────────────────────────┬──────────────────────────────────────┘
                          │
                          │ compiles to
                          ▼
┌────────────────────────────────────────────────────────────────┐
│  AIDA Compiler                                                  │
│  ─────────────────────────────────────────────────────────────  │
│  workflow.js (one per drain phase)                              │
│  Subagent defs (.claude/agents/<role>.md)                       │
│  Path-gated rules (.claude/rules/*.md with paths: glob)         │
│  /goal criteria (spec-graph-derived completion conditions)      │
│  Briefs (rendered from substrate state)                         │
│  CLAUDE.md scaffolding (the existing pack)                      │
└─────────────────────────┬──────────────────────────────────────┘
                          │
                          │ executes on
                          ▼
┌────────────────────────────────────────────────────────────────┐
│  Claude Code Runtime (per-user supervisor, ~/.claude/daemon/)  │
│  ─────────────────────────────────────────────────────────────  │
│  Workflows runtime · agents TUI + supervisor · agent teams      │
│  --bg dispatch · worktree isolation · /goal eval                │
│  --json structured output · deep-link URL scheme                │
│  Single-machine · single-tool (Claude Code only)                │
└─────────────────────────┬──────────────────────────────────────┘
                          │
                          │ observed by
                          ▼
┌────────────────────────────────────────────────────────────────┐
│  AIDA Supervisor + Bus                                          │
│  ─────────────────────────────────────────────────────────────  │
│  Pickability gate · commit-trailer auto-bump · status reconcile │
│  Punt → advisor → resume escalation cascade                     │
│  Findings capture · cross-tool routing (Codex, AGY briefs)      │
│  Drain orchestration · MCP server exposing spec graph           │
└────────────────────────────────────────────────────────────────┘
```

**Read top-down**: the substrate is the source of truth; the compiler renders artifacts the Claude Code runtime understands; the runtime executes; the AIDA supervisor reads the runtime's state (`claude agents --json`) and substrate state together to make lifecycle decisions.

**Read inside-out**: AIDA owns the *book-keeping* (what to do, who's doing it, where it stands). Claude Code owns the *doing* (how to run, where to isolate, how to coordinate within a single machine). Cross-tool and cross-machine work is AIDA's because there's no alternative.

---

## What AIDA divests (delegate to Claude Code)

### 1. Worktree creation

Today `aida session start` creates worktrees inline. Claude Code's `claude --bg` does this natively under `.claude/worktrees/` and tears them down on `claude rm`. AIDA should:

- Stop creating worktrees in `aida session start` when the agent target is Claude Code
- Record the worktree path AIDA finds (via `claude agents --json`) rather than allocate one
- Keep worktree creation only for non-Claude agents (Codex, AGY) until those tools ship equivalent native isolation

### 2. Process supervision

Today AIDA's `aida session prune` cleans up dead leases. Anthropic's per-user supervisor watches its own sessions: respawns on binary update, survives sleep/wake, evicts idle (~1h) but keeps state.json so attach/peek/reply revives. AIDA should:

- Stop trying to be the process supervisor for Claude Code sessions
- Reconcile AIDA leases against `claude agents --json` instead of guessing from PID liveness
- Keep process supervision only for AIDA-launched Codex/AGY sessions

### 3. Single-machine dispatch verbs

Today `aida agent new claude` does its own launching. Anthropic's `claude --bg --name "..." --agent <subagent-def>` is the cleaner verb. AIDA should:

- Re-shape `aida agent new claude` as a `claude --bg` wrapper
- Add only what AIDA needs on top: role-context snapshot file, brief-polling, lease registration
- Strip the parts that now duplicate `claude --bg`'s functionality

**These three divests reduce surface area without reducing capability** — the user still gets the same behavior, and the moat (cross-machine, cross-tool, lifecycle) is unaffected.

---

## What AIDA keeps (the moat — unchanged)

- **Substrate-grounded scope** — every session knows which SPEC-ID it owns (Claude Code doesn't have stable cross-session IDs)
- **Cross-machine substrate** — git-canonical replicates everywhere (Claude Code sessions are local)
- **Cross-tool dispatch** — AIDA briefs Codex and AGY (Claude Code only manages Claude Code)
- **Lifecycle tracking** — commit-trailer auto-bump (`Done → Completed` on merge), reconcile-status, history
- **Role-context injection** — implementer/reviewer/advisor with their respective context snapshots
- **Brief polling** — substrate-resident work routing (Claude Code's `~/.claude/teams/<name>/mailbox/` is per-tool only)
- **MCP server** — exposing the spec graph to any MCP client (Cursor, Windsurf, Continue, …)
- **Plan archival + verification** — `aida plan verify`, `aida plan helpers`
- **Findings + escalation cascade** — implementer → advisor → human handshake
- **Trace comments** — `// trace:SPEC-ID | ai:claude` (no Claude Code analog)
- **Calibration mode + substrate-learning loop** — cold-boot vs fork-from-live verdicts (no Claude Code analog)

---

## What AIDA exposes (new — pulls Claude Code into AIDA's substrate)

This is the inverse of divest — surfaces AIDA should *add* so Claude Code's runtime can read AIDA's truth natively.

### 1. Scaffolded subagent definitions

`.claude/agents/<role>.md` is Claude Code's native subagent format. AIDA scaffolds advisor/implementer/reviewer roles today; it should also scaffold them in this format so:

- `claude --bg --agent implementer` works without operator hand-authoring
- `claude agents` filters by role (`a:implementer` in the dispatch input — see SPIKE-15)
- Agent-teams configs can reference AIDA roles by name

### 2. Path-gated `.claude/rules/`

The `/large-codebases` doc confirms `.claude/rules/<name>.md` with `paths:` glob frontmatter is shipped, loading only when Claude works on a matching file. AIDA should generate these from the spec graph:

- A `.claude/rules/spec-IMPL-NNN.md` with `paths:` matching the spec's `// trace:` comment locations
- Carries the spec description + acceptance criteria + any reviewer notes
- Auto-regenerated when the spec changes — keeps Claude grounded on current scope

This is **substrate-as-bouncer in nightclub form**: Claude Code's runtime enforces it; AIDA's substrate decides what gets enforced.

### 3. /goal criteria from substrate

Claude Code's `/goal` is a Stop-hook wrapper running a Haiku-class evaluator each turn. AIDA already has `aida goal --spec SPEC-ID --copy` (TASK-242). Compose:

- `aida goal --spec SPEC-ID --invoke` already emits a bare `/goal …` line
- The `/goal` evaluator is fed substrate-grounded completion criteria (acceptance criteria, trace coverage, queue-empty, lint-clean, PR-merged) instead of operator-hand-written text
- AIDA's compose makes `/goal` substrate-aware without changing Anthropic's surface

### 4. workflow.js compiler

`aida workflow compile <SPEC>` could emit a Claude Code `meta.js` workflow that:

- Imports the spec's children as phases
- Runs implementer → reviewer → merger as `agent()` calls
- Uses Claude Code's `pipeline()` for resumability
- AIDA's orchestrator transitions from being a runtime to being a compile-target generator

This is the largest open surface — high leverage, real engineering cost. SPIKE worth filing now to scope.

### 5. Deep-link emission

Claude Code's `claude-cli://open?cwd=&q=&repo=` URL scheme is inert until Enter. AIDA already produces paste-ready prompts (briefs, `aida goal --copy`, `aida ultraplan --stdout`). Compose:

- `aida brief <agent> SPEC --as-deep-link` emits a `claude-cli://` URL
- Click → Claude Code opens in the right `cwd` with the brief prefilled
- Operator presses Enter to launch — preserves the user-in-the-loop invariant
- Eliminates one paste step per work item

---

## What AIDA composes with (uses Claude Code's runtime directly)

### Workflows (SPIKE-14 verdict reaffirmed)

AIDA writes `workflow.js` for drain phases; Claude Code's runtime executes. AIDA observes outcomes via `claude agents --json` and applies lifecycle transitions.

### `claude agents --json`

The single most load-bearing programmatic surface from this entire 2.1.154 review. AIDA's `aida status` reads it and reconciles against substrate leases, surfacing drift either way.

### Workflow resumability

`workflow.js` resumes within a session by hashing prior `agent()` calls. AIDA's orchestrator gets this for free — no need to build resumability into AIDA itself.

### Worktree isolation (`worktree.sparsePaths`, `claude --bg` worktree)

The `/large-codebases` doc confirms sparse worktrees are native and per-session. AIDA's `.aida/sessions/<id>.toml` could record sparse-paths AIDA cares about; Claude Code applies them.

---

## Concrete next SPIKEs (priority order)

| Pri | SPIKE | Effort | Leverage |
|---|---|---|---|
| 1 | **SPIKE-30**: `aida status` integrates `claude agents --json` — single command queries both substrates, surfaces drift (lease without process, process without lease) | Small | High — operator gains unified visibility |
| 2 | **SPIKE-31**: AIDA emits path-gated `.claude/rules/` from spec graph (substrate-as-bouncer) | Medium | High — makes substrate enforceable by Claude Code's runtime |
| 3 | **SPIKE-32**: `aida workflow compile <SPEC>` → `meta.js` workflow.js targeting Claude Code's runtime | Large | Very high — orchestrator becomes compiler |
| 4 | **SPIKE-33**: `aida brief … --as-deep-link` emits `claude-cli://` URLs | Small | Medium — papercut elimination |
| 5 | **SPIKE-34**: `aida agent new claude` wraps `claude --bg --agent <subagent-def>` instead of fork/launch | Medium | Medium — surface-area reduction |

File-in-order is intentional: SPIKE-30 unblocks the operator's "what's running?" question immediately and is the cheapest. SPIKE-31 and SPIKE-32 are the structural moves. SPIKE-33 and SPIKE-34 are mop-up.

---

## Surfaces NOT yet fetched (still to capture)

The user paste-bombed `/large-codebases` last; remaining high-value Claude Code surfaces to add to the picture in subsequent passes:

- `/sub-agents` — subagent definition format
- `/skills` — skill manifest format + path-gating
- `/hooks` — SessionStart/Stop/PreToolUse hooks (relevant to AIDA's auto-bump SessionEnd story)
- `/mcp` — Claude Code's MCP-server primitives (compose with AIDA's MCP)
- `/worktrees` — full worktree configuration surface
- `/plugins` — distribution mechanism for AIDA's discipline pack as a plugin
- `/permissions` — permission rule syntax (AIDA could emit project-scoped permissions)
- `/best-practices` — context-window hygiene patterns
- `/memory` — full memory loading + `@`-imports + path-specific rules

**Each adds polish, not architectural shift.** The strategic decisions can be made on the current seven axes. Subsequent surfaces refine implementation.

---

## What this means for the next two weeks

If we ship SPIKE-30 in week 1, AIDA's `aida status` already reflects the new picture. If we ship SPIKE-31 by end of week 2, every active spec auto-injects its scope into Claude Code's per-file rule loader and the substrate becomes self-enforcing during implementer work. Those two land before any architectural surface AIDA owns is at risk.

SPIKE-32 (workflow compiler) is a months-not-weeks initiative. File it now so the design space stays public; don't start until SPIKE-30 + SPIKE-31 confirm the compose direction works in production.

---

## Notes for future me

- The 7-axis sweep took ~6h of operator + advisor co-research on 2026-05-28/29. Audit `2026-05-29-claude-code-2.1.154-decompose/` for raw notes.
- Claude Code 2.1.154 was released ~2026-05-24; this analysis is from documentation dated within a week of that release. Refresh signal: when 2.1.16x or 2.2.x lands, re-fetch these same surfaces — `/workflows` and `/agent-teams` are EXPERIMENTAL today and likely to shift.
- Anthropic's framing is "Claude Code is the agent runtime" + "subagent defs are plugins" + "your team writes skills." AIDA's framing should align: "AIDA is the substrate plugin for Claude Code" might be the right positioning headline for the next docs refresh.
- The user's instinct on 2026-05-28 — "we need to decompose the changelog and farm it out to an army of agents" — was correct. The composed picture is materially clearer than any single fetch produced.

## Sources

- SPIKE-14 (workflows): `2026-05-29-claude-code-2.1.154-decompose/01-dynamic-workflows.md`
- SPIKE-15 (agent view): `2026-05-29-claude-code-2.1.154-decompose/02-agent-view.md`
- SPIKE-29 (agent teams): captured, write-up pending
- /goal: captured, SPIKE-21 write-up pending
- /headless: captured (composes with existing AIDA `--no-human` work, SPIKE-22-adjacent)
- /deep-links: captured, write-up pending
- /large-codebases: captured, write-up pending
- Master Claude Code docs index: <https://code.claude.com/docs/llms.txt>

trace:SPIKE-14 trace:SPIKE-15 trace:SPIKE-29 | ai:claude
