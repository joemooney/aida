# SPIKE-15: Claude Code Agent View

**Date**: 2026-05-29
**Source-verified**: Yes — [code.claude.com/docs/en/agent-view](https://code.claude.com/docs/en/agent-view), live operator-keyboard validation pending (`claude agents --json` would be the highest-value follow-up paste).
**Verdict**: **COMPOSE + Divest**. AIDA's session/lease/multi-agent registry layer overlaps materially with what `claude agents` ships. Persistent supervisor + per-session state.json + roster + worktree isolation are all native. AIDA's git-canonical substrate is strictly more capable (cross-machine, cross-tool), but AIDA's IN-Claude-Code single-machine session management should compose with `claude agents` rather than fight it.

---

## What agent view is

> *"Agent view, opened with `claude agents`, is one screen for all your background sessions: what's running, what needs your input, and what's done."*

Two things in one:

1. **A TUI dashboard** — `claude agents` opens a full-terminal grouped table of every background Claude Code session, with dispatch input at bottom.
2. **A supervisor-process model** — sessions run as detached processes managed by a per-user supervisor (`~/.claude/daemon.log`, `~/.claude/daemon/roster.json`, `~/.claude/jobs/<id>/state.json`). They keep running with no terminal attached, survive auto-update of the Claude Code binary (the supervisor watches the binary file on disk and respawns sessions into the new version), and resume across machine sleep/wake.

Research preview, requires v2.1.139+.

## The programmatic interface AIDA needs

The single most load-bearing finding:

```bash
claude agents --json
```

Schema per the docs:
> *"Print live sessions as a JSON array and exit. Each entry has `pid`, `cwd`, `kind`, and `startedAt`, plus `sessionId`, `name`, and `status` when set. Combine with `--cwd <path>` to filter"*

That's the integration surface AIDA needs. AIDA can:
- Query `claude agents --json` to discover Claude Code sessions
- Cross-reference against AIDA's lease registry (`.aida/sessions/`)
- Surface unified state in `aida status`
- Detect drift: an AIDA lease without a corresponding `claude agents` row, or a `claude agents` session without an AIDA lease

Other shell-level CLI surfaces:

| Verb | What it does |
|---|---|
| `claude --bg "prompt"` | Dispatch a session to background |
| `claude --bg --exec 'shell-cmd'` | PTY-backed shell job (no model, runs in agent view rows) |
| `claude --bg --name "..."` | Named session |
| `claude --bg --agent <name>` | Run a specific subagent as the main agent |
| `claude attach <id>` | Open session in this terminal |
| `claude logs <id>` | Recent output |
| `claude stop <id>` (alias `claude kill`) | Stop session |
| `claude respawn <id>` | Restart with conversation intact (for binary update pickup) |
| `claude respawn --all` | Restart every running session |
| `claude rm <id>` | Remove session (keeps worktree if uncommitted) |
| `claude daemon status` | Supervisor status |

## State model (the substrate-of-truth in agent view's world)

| Path | Contents |
|---|---|
| `~/.claude/daemon.log` | Supervisor log |
| `~/.claude/daemon/roster.json` | Live session roster (reconnect after restart) |
| `~/.claude/jobs/<id>/state.json` | Per-session state |
| `~/.claude/teams/<name>/config.json` | Agent teams config (see SPIKE-29) |

### Session states

| State | What it means |
|---|---|
| Working | Actively running tools or generating |
| Needs input | Waiting on a question or permission decision |
| Idle | Done, ready for next prompt |
| Completed | Finished successfully |
| Failed | Ended with error |
| Stopped | `Ctrl+X` or `claude stop` |

### Process shapes

| Glyph | Meaning |
|---|---|
| `✻` / animated `✽` | Process alive, replies immediately |
| `∙` | Process exited (after ~1h idle), but resumable — supervisor starts fresh process on attach/peek/reply |
| `✢` | `/loop` session sleeping between iterations, with countdown |

### Pull request label colors

Inline PR status per row (yellow=waiting, green=passed, purple=merged, grey=draft/closed). Hyperlinked in supporting terminals.

## Worktree isolation (built-in)

> *"Every background session, whether started from agent view, /bg, or claude --bg, starts in your working directory. Before editing files, Claude moves the session into an isolated git worktree under `.claude/worktrees/`, so parallel sessions can read the same checkout but each writes to its own."*

Configurable via `worktree.bgIsolation: "none"` (v2.1.143+) for repos where worktrees are impractical.

## Filter syntax (in the dispatch input)

| Filter | Shows |
|---|---|
| `a:<name>` | Sessions running named agent |
| `s:<state>` | Sessions in state (e.g. `s:blocked` covers everything waiting on you) |
| `#<number>` or PR URL | Session working on that PR |

## Direct overlap with AIDA primitives

This is the substrate-comparison table:

| AIDA primitive | Claude Code agent view equivalent | Verdict |
|---|---|---|
| `aida session start --owns SPEC` → lease + worktree | `claude --bg "prompt"` → session + auto worktree under `.claude/worktrees/` | **Native overlap.** Claude Code does the worktree isolation natively now. AIDA's `aida session start` doesn't need to do it; could delegate. |
| `.aida/sessions/<id>.toml` lease file | `~/.claude/jobs/<id>/state.json` | Native overlap. AIDA's lease file format is richer (scope, role, branch, spec id) but the storage pattern is identical. |
| `aida session leases` | `claude agents --json` | Native overlap. `--json` schema is shallower than AIDA's, but Claude Code is doing the cross-session aggregation. |
| `aida session end` | `claude stop <id>` + `claude rm <id>` | Native overlap (with the worktree-cleanup gotcha being similar). |
| `aida agent new claude` (STORY-432) | `claude --bg --name "..."` | Direct competition — but Claude Code's verb is cleaner. AIDA's `aida agent new` adds role-context injection + brief polling. |
| `aida status` cross-agent view | `claude agents` TUI | Different shapes — AIDA shows substrate state, Claude Code shows process state. Could compose: AIDA's `aida status` includes "Claude Code sessions: 3 working, 1 awaiting input" pulled from `claude agents --json`. |
| Cross-machine lease handoff | None — `"Sessions are local"` | **AIDA wins.** Claude Code sessions don't migrate; AIDA's substrate is git-distributable. |
| Cross-tool agent dispatch (Codex, AGY) | None — only Claude Code sessions | **AIDA wins.** Claude Code only manages Claude Code. AIDA briefs Codex and AGY. |
| Lifecycle-aware orchestration (auto-bump on commit trailer) | None | **AIDA wins.** No spec/PR/commit relationship tracking. |

## What divests, what stays

### Divest

- **Worktree creation** in `aida session start`. Claude Code does it natively. AIDA delegates and records the worktree path it finds.
- **Process supervision** of Claude Code instances. Anthropic's supervisor handles binary updates, sleep/wake, idle eviction. AIDA's own session-end + pruning is overlap.
- **Single-machine dispatch verbs**. Operators should reach for `claude --bg` or the agent view TUI; AIDA's `aida agent new claude` should *use* `claude --bg` underneath rather than implement equivalent fork/launch.

### Stay (the moat)

- **Substrate-grounded scope** (the SPEC-ID the session owns)
- **Cross-machine substrate** (git-canonical store)
- **Cross-tool dispatch** (Codex, AGY briefs)
- **Lifecycle tracking** (Approved → InProgress → Done → Completed via commit trailers)
- **Role-context injection** (master/implementer/reviewer/advisor with their respective contexts)
- **Brief polling** (the work-arrival mechanism)
- **MCP server exposing the spec graph**

## Specific scoped follow-ups to file

1. **SPIKE**: AIDA status integration with `claude agents --json` — single command queries both substrates, surfaces drift (lease without process, process without lease)
2. **SPIKE**: refactor `aida session start` to delegate worktree creation to `claude --bg` when target agent is Claude Code (vs handling it inline only for non-Claude agents)
3. **SPIKE**: `aida agent new claude` re-shape — should it wrap `claude --bg --agent <subagent-def>` under the hood and just add the role-context-snapshot + brief-polling layer?
4. **DOC**: update `vs-claude-code-subagents.md` positioning doc — agent view + agent teams + workflows change the comparison
5. **STORY**: AIDA scaffolds subagent definitions under `.claude/agents/<role>.md` that Claude Code's agent-teams runtime can use directly — eliminating duplication between AIDA's role definitions and Claude Code's subagent format

## Sources

- [code.claude.com/docs/en/agent-view](https://code.claude.com/docs/en/agent-view) — official documentation
- [code.claude.com/docs/en/agent-teams](https://code.claude.com/docs/en/agent-teams) — sibling surface, see SPIKE-29
- [code.claude.com/docs/en/sub-agents](https://code.claude.com/docs/en/sub-agents) — subagent primitive (referenced)
- [code.claude.com/docs/en/worktrees](https://code.claude.com/docs/en/worktrees) — worktree mechanism (referenced)

## Operator follow-up

Highest-value paste for sharpening this analysis:

```bash
claude agents --json | jq .
claude daemon status
ls -la ~/.claude/daemon/ ~/.claude/jobs/
```

The `--json` schema and a sample `jobs/<id>/state.json` would let AIDA's status integration design start immediately rather than guess.
