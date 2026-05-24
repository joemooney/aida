# How agents should poll AIDA briefs

**Last updated**: 2026-05-24
**Principle Trace**: `BUG-378` | `feedback_substrate_as_bouncer_not_rules`

Coding agents (Claude Code, Codex, Antigravity, Gemini) maintain their own
internal session state — a private scratchpad file like `task.md`,
`walkthrough.md`, or `~/.gemini/antigravity-cli/brain/<session-id>/task.md`.
That scratchpad is a working draft. **It is NOT AIDA's substrate, and it
does not auto-sync with the brief surface, the queue, or the spec store.**

When an agent treats its scratchpad as ground truth, the **scratchpad-drift
failure mode** appears: the agent finishes the work named in its scratchpad,
loops on re-reading the same scratchpad, and keeps re-rendering "all done"
while a new brief sits unread in `.aida/agent-briefs/<agent-type>/`. The
operator has to manually paste an override directive to break the loop.

This document is the discipline that prevents scratchpad-drift, plus the
substrate-as-bouncer gate that catches it when the discipline slips.

---

## The canonical state surfaces

| Surface | Source | When to read |
|---------|--------|--------------|
| `aida queue next` / `aida queue list --for <role>` | Role-routed work queue | At the start of every pickup |
| `aida brief list --for-agent <type>` | `.aida/agent-briefs/<type>/*.md` | At every work-cycle boundary |
| `aida show <SPEC-ID>` | The spec's current contract | Before declaring "done" on a spec |
| `aida findings list` | Triage queue (advisor seat) | Before opening a follow-up TASK |

None of these surfaces auto-replicate into your agent's internal scratchpad.
You must actively read them.

---

## The polling discipline

At each work-cycle boundary — *start of a pickup, after a `queue done`,
before declaring all work shipped* — run **one** of:

```bash
aida brief list --for-agent <your-type>        # claude | codex | antigravity
aida queue next                                # role-scoped, head item only
aida queue list --for <your-role>              # full role queue
```

The bare minimum is the brief list — briefs are the explicit, named
pickup-requests filed for your agent type. The queue commands are the
broader "what's routed to my role" view.

When you finish a spec and your scratchpad says you are done with the
session's full mandate, **the discipline is to poll the brief surface one
more time before exiting**. The substrate cannot know what is *next*; only
the operator and the advisor do. Polling is your check that nothing landed
while you were heads-down on the previous spec.

---

## The substrate-as-bouncer backstop (BUG-378)

When an agent ignores the polling discipline, `aida queue done` and
`aida edit --status done|completed` close the loop themselves:

1. Detect the running agent's type via `$AIDA_AGENT_TYPE` (set by
   `aida agent new`) or environment fingerprint (`CODEX_*` →  `codex`,
   `ANTIGRAVITY_*` / `GEMINI_*` → `antigravity`, `CLAUDE*` →  `claude`).
2. Scan `.aida/agent-briefs/<detected-type>/` for unacked briefs.
3. If any are present, print a loud red banner to **stderr** before the
   command returns:

   ```
   ⚠ NEW BRIEF(S) PENDING for agent `antigravity` — read before exiting:
       .aida/agent-briefs/antigravity/TASK-542-2026-05-24T180754Z.md
       .aida/agent-briefs/antigravity/STORY-462-2026-05-24T191029Z.md
     Run: aida brief list --for-agent antigravity
     Your internal task.md / scratchpad is NOT ground truth — poll the
     brief surface before declaring work complete.
   ```

The banner is intentionally narrow:

- It fires **only** for `claude`, `codex`, and `antigravity` agent types.
  Raw shell invocations by a human (no detectable agent type) get nothing
  — the banner is for agents, not interactive users.
- It lists **only** briefs whose directory matches the running agent's
  type. A Codex session never sees an Antigravity brief banner; that
  would be noise.
- It writes to **stderr** so it survives stdout piping (`aida queue done
  | jq …`) and stays visible in scrollback even when the rest of the
  output is captured.

The banner is a backstop, not a substitute for discipline. Polling first
is cheaper than reading a startled banner mid-exit and going back to
re-pick up the work you almost dropped.

---

## What the banner does NOT do

- It does not block the exit. The spec still flips to Done/Completed; the
  queue entry is still removed. The banner is informational — the agent
  decides whether to pick up the brief next or close out the session and
  let another agent / session take it.
- It does not fire when no briefs are queued. The common case (clean
  exit, no pending work) prints nothing, so the banner stays loud when
  it does fire.
- It does not fire for `"other"` agent types (raw shell, unknown
  fingerprint). A human running `aida queue done` from their terminal
  gets the normal output, not an agent-targeted scolding.

---

## Composes with

- `aida-pickup.md` skill — the "your local scratchpad is NOT AIDA
  substrate" directive that runs at the *start* of every pickup, paired
  with the banner at the *end*.
- `aida-implement.md` skill — Step 7 ("Exit after `aida pr ship`") names
  the banner as the substrate signal to re-poll before exiting.
- `substrate-as-bouncer.md` — the broader principle this gate
  instantiates.
- `feedback_substrate_as_bouncer_not_rules` (memory) — the originating
  principle: when an invariant matters and an LLM is the bouncer, ship
  a programmatic gate; rules in CLAUDE.md / memory / skill template are
  necessary but not sufficient.
