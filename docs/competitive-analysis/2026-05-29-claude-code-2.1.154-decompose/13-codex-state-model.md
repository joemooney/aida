# Codex task and state model: observed persistence and AIDA's wrapper role

Date: 2026-06-04

Spec: SPIKE-26

## Sources

- Local observation: `codex --version` reported `codex-cli 0.135.0`.
- Local observation: `codex --help`, `codex exec --help`,
  `codex resume --help`, `codex fork --help`, `codex exec resume --help`.
- Empirical probe run in `/home/joe/ai/aida-spike-24` on 2026-06-04:
  start session `019e934d-9808-76b0-a4ca-7a52da104a87`, then
  `codex exec resume 019e934d-9808-76b0-a4ca-7a52da104a87`.
- Observed local Codex files under `~/.codex/`: `sessions/`,
  `history.jsonl`, `state_5.sqlite`, `logs_2.sqlite`, `goals_1.sqlite`,
  `memories_1.sqlite`, `config.toml`, and `auth.json`.
- OpenAI Codex CLI docs: <https://developers.openai.com/codex/cli>
- OpenAI AGENTS.md guide: <https://developers.openai.com/codex/guides/agents-md>
- OpenAI Codex subagents docs: <https://developers.openai.com/codex/subagents>
- AIDA local docs/specs: `AGENTS.md`, `docs/agents/codex-mcp-setup.md`,
  STORY-431, STORY-432, STORY-433, TASK-485.

## Executive verdict

Codex has native **conversation state**, **configuration state**, and **local
history state**. It does not have a native durable **project work-state model**
equivalent to AIDA's spec graph, leases, brief queue, or status lifecycle.

The practical split is:

- Codex persists "what happened in this Codex conversation."
- AIDA persists "what work exists, who owns it, where it is being worked, and
  what state the project believes it is in."

AIDA's cross-Codex wrapper is justified. It should not be simplified away into
Codex resume/fork, because resume/fork does not answer the project-level
coordination questions AIDA is built to answer.

## Native Codex state primitives

Local help and files show these primitives:

| Primitive | Evidence | Meaning |
| --- | --- | --- |
| Interactive session | Bare `codex [PROMPT]`. | Starts a TUI session that can load project instructions and use tools. |
| Non-interactive session | `codex exec [PROMPT]`. | Runs an agent turn from the shell and persists by default unless `--ephemeral` is used. |
| Resume | `codex resume` and `codex exec resume`. | Continues a previous session by UUID/thread name or `--last`. |
| Fork | `codex fork`. | Creates a branch of a previous interactive session. |
| Session artifact | `~/.codex/sessions/YYYY/MM/DD/rollout-...<session-id>.jsonl`. | JSONL record for at least some sessions. |
| Local history/config | `~/.codex/history.jsonl`, `~/.codex/config.toml`, SQLite files. | User-local Codex state, not project-canonical state. |
| Ephemeral opt-out | `codex exec --ephemeral`. | Explicitly disables persisted session files for an exec run. |
| Subagents | Codex docs and CLI/app surface. | Parallel child-agent execution inside Codex's own session model. |

## Empirical start -> persist -> resume probe

Command 1:

```bash
codex exec --sandbox read-only \
  --output-last-message /tmp/aida-spike-26-start.txt \
  "For an AIDA SPIKE-26 state persistence probe, do not inspect files or run tools. Reply exactly: AIDA_SPIKE_26_PROBE_START"
```

Observed:

- Codex printed session id `019e934d-9808-76b0-a4ca-7a52da104a87`.
- The final message file contained `AIDA_SPIKE_26_PROBE_START`.
- A persisted JSONL session file appeared at
  `~/.codex/sessions/2026/06/04/rollout-2026-06-04T08-43-16-019e934d-9808-76b0-a4ca-7a52da104a87.jsonl`.

Command 2:

```bash
codex exec resume 019e934d-9808-76b0-a4ca-7a52da104a87 \
  --output-last-message /tmp/aida-spike-26-resume.txt \
  "For the same AIDA SPIKE-26 state persistence probe, do not inspect files or run tools. Reply exactly: AIDA_SPIKE_26_PROBE_RESUME"
```

Observed:

- Codex resumed the same session id.
- The final message file contained `AIDA_SPIKE_26_PROBE_RESUME`.

Conclusion: Codex can persist and resume conversation state across invocations
by session id. This is real and useful, including in non-interactive `exec`
mode.

## What Codex state does not prove

The probe does not show that Codex tracks AIDA work state. It proves
conversation continuity, not durable project coordination.

Codex native state did not record, in a project-canonical way:

- which AIDA spec was owned,
- whether the spec was Approved/In Progress/Completed,
- which sibling worktree held the implementation,
- whether a lease existed,
- whether the branch had an open PR,
- whether the brief was acknowledged,
- whether another agent should avoid the same work,
- whether a punt/finding/comment should route to the advisor.

AIDA provided those separately through the SPIKE-26 lease, branch/worktree, spec
status, brief queue, and PR discipline.

## Mapping to AIDA

| Need | Codex native support | AIDA support | Integration stance |
| --- | --- | --- | --- |
| Continue a conversation | Strong: `resume`, `fork`, JSONL session artifacts. | Not AIDA's job. | Let Codex own conversation continuity. |
| Remember repo instructions | Strong: AGENTS.md hierarchy. | AIDA scaffolds `AGENTS.md` and docs. | AIDA should keep instructions current and concise. |
| Track project work item | Weak/native absent. | Strong: spec graph and statuses. | AIDA remains source of truth. |
| Claim ownership | Native absent for project graph. | Leases and agent registry. | Use `aida session start` / MCP `claim_task`. |
| Communicate handoff | Conversation-local only. | Briefs, comments, punts, findings, mailbox. | Use AIDA for durable handoffs. |
| Launch supervised agent | Codex can start itself. | `aida agent new codex` adds worktree/lease/context. | Use AIDA launcher for project work. |
| Inspect active agents | Codex has session/thread visibility. | `aida status` agent registry. | AIDA provides cross-tool visibility. |

## Simplification opportunities

AIDA does not need to duplicate these Codex capabilities:

1. Conversation transcript persistence.
2. Session resume/fork.
3. Codex config/profile layering.
4. Codex sandbox/approval UI.
5. Codex subagent thread orchestration inside one Codex session.

AIDA should instead record pointers where useful:

- Codex session id in an AIDA agent registry entry when launch output exposes it.
- Codex version and working directory at launch.
- AIDA spec id and lease id in the Codex launch context file.

That creates a bridge without trying to parse or own Codex's private state
databases.

## Risks

1. **False confidence from resume**: resuming a Codex session may restore
   conversation context while the AIDA branch/spec/lease has moved on. AIDA
   launch context should be treated as point-in-time, and agents should poll
   AIDA before acting.
2. **User-local state is not team state**: `~/.codex` is per-user/per-machine.
   It is not a coordination ledger.
3. **Ephemeral runs are possible**: `codex exec --ephemeral` intentionally avoids
   persisted session files. AIDA must not assume every Codex invocation leaves a
   resumable transcript.
4. **Subagents are not AIDA agents by default**: Codex child threads can do real
   work without appearing in AIDA's agent registry unless AIDA explicitly
   launches/registers them.

## Recommended AIDA posture

Keep AIDA's Codex wrapper. It is not redundant with Codex native state.

Concretely:

- Keep `aida agent new codex` as the standard project-work launcher.
- Keep `AGENTS.md` and Codex setup docs scaffolded into projects.
- Use AIDA leases/spec statuses/briefs for all durable work coordination.
- Avoid reading or depending on private `~/.codex/*.sqlite` schemas.
- Optionally capture Codex session ids as metadata, but never make them the
  primary project state key.
- Document the difference sharply: "Codex resume resumes a conversation; AIDA
  resumes project coordination."
