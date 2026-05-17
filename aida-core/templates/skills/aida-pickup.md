---
name: aida-pickup
description: Producer/consumer queue loop — peek at the next item routed to your active role, work it, mark it done, repeat. Use this between work items to pick up the next thing without re-entering the conversation.
allowed-tools:
  - Bash
  - Read
  - Grep
  - Edit
  - Write
---

# AIDA Pickup Skill

## Purpose

Drive the implementer / reviewer / triage / architect loop where the active
role pulls the next item from the queue, works on it, marks it complete,
and pulls the next. Pairs with the `dialog` role on the producer side
(see `aida role enter dialog` and `aida queue add --for <role>`).

## When to use

- The user is in a doer role (implementer, reviewer, etc.) and asks
  "what's next?" or "pick up the next task"
- After completing a piece of work — proactively offer to grab the next
  item from the queue
- At the start of a focused work session — show what's queued before
  the user dives in

## Skip if

- No role is active (`AIDA_SESSION_ROLE` empty) — suggest
  `eval "$(aida role enter <name>)"` first so the queue filter has a target
  (or the `aida-role <name>` shell helper if `aida dev shell-init --install`
  has been run)
- The user is in `dialog` mode — that's the producer seat, not the consumer

## Active role

!`echo "Role: ${AIDA_SESSION_ROLE:-(none active)}"`

## Current queue head

!`aida queue next 2>/dev/null || echo "(no items)"`

## Plan brief

!`aida session show --plan 2>/dev/null | sed -n '/Plan brief:/,$p' || true`

## Pending findings

!`c=$(aida findings list --count 2>/dev/null || echo 0); [ "${c:-0}" -gt 0 ] && echo "$c findings from recent merges awaiting triage — run: aida findings list" || true`

## Workflow

### Step 1: Check the queue

Run `aida queue next` to see the top item routed to the current role.
The output includes:
- spec_id, title, status, priority, owner
- The note from whoever queued it (often the dialog seat)
- First 10 lines of the description
- Suggested follow-up commands

If the queue is empty, surface that to the user and stop. Don't fabricate
work — empty queue is a real signal.

**If the Pending findings block above emitted a line** (STORY-278) — the
headless reviewer filed review follow-ups that the advisor hasn't triaged
yet — surface it verbatim to the user as a one-line nudge. Don't act on it
here (triage is the `dialog` role's job); just make sure it isn't missed.
Stay silent when the block is empty.

**If a Plan brief is shown above** (TASK-95) — `aida queue work`
pre-populated it from a matching `docs/plans/` file — lead your first
message with it: name the plan file, the Critical Files (the blast
radius), and the Verification script (the definition of done). The
implementer should not have to grep for the plan. The Followups list is
informational here; the `aida queue done` handler offers to file those as
TASKs at completion time (TASK-96).

### Step 2: Confirm pickup

Show the user the item and ask whether to start. Examples:

> Next up: **FR-1-042 — Add OAuth provider** (Approved · High · joe)
>
> Note from dialog: "high priority, customer ask"
>
> Want me to start on this? I'll mark it in-progress before diving in.

If the user says no (wants to skip, prioritize differently, etc.), stop
here. Don't auto-skip to the next item — the queue order encodes priority.

**Skip the confirm when invoked with `--auto-first`** (TASK-86). When
`aida queue work` launches the skill in cluster mode (drain a parent
scope) or head mode (no-arg, top of queue), it passes `--auto-first` to
signal that the user has already authorized draining via the queue-work
pre-flight summary. In that case, skip the "want me to start?" prompt
and proceed straight to Step 3a/3b — re-asking inside the launched
session is friction-without-value.

Keep the confirm for:
- Direct `/aida-pickup` invocation (no upstream consent)
- `aida queue work <ITEM-ID>` (item mode — user named one item, may
  want to verify it's the right pickup)

After the first item, you can also skip the per-item confirm when
walking a planned cluster — the manifest IS the consent record. Surface
each item briefly (one line) and move to mark-in-progress.

### Step 3a: Record the planned cluster (STORY-98)

If the user's confirmation covers MORE than one item — i.e. they want
you to work a multi-item batch ("do all of TASK-67 through TASK-74",
"work STORY-98 + STORY-90 + BUG-74", etc.) — write the planned list to
the session manifest before starting:

```bash
aida session manifest write --items SPEC-ID-1,SPEC-ID-2,SPEC-ID-3 \
  --source "user prompt"
```

This:
- Records each spec's position in the cluster + its status at plan time
- Renders a `[planned:by-<session>]` chip on those specs in other
  sessions' `aida queue list` output, so a concurrent reviewer/agent
  doesn't grab work you've claimed
- Powers `aida session show --plan` (✓ Done / ◐ In progress / ○ Pending
  status table) so you and the user can see cluster progress at a glance

Skip this step for single-item pickups (one spec, no batch intent) — the
manifest only earns its keep when there's a planned-cluster shape to
track.

### Step 3b: Mark the current item in-progress

Once the user confirms:

```bash
aida edit <spec_id> --status in-progress
```

This makes it visible to other sessions / dashboards that someone's on
it. If a session manifest exists (step 3a), `aida edit --status` also
stamps the manifest row's `started_at`, so the cluster's `◐ In progress`
column flips automatically.

### Step 4: Do the work

Drive the actual implementation. Read the requirement (`aida show <spec_id>`),
follow related links, write the code, add trace comments
(`// trace:<spec_id> | ai:claude`), commit.

### Step 5: Mark done atomically

When the work lands:

```bash
aida queue done <spec_id>
```

This is one atomic step that:
- Sets status to **Done** (STORY-86: work finished on a branch, not yet
  merged to main). `aida pull` / `aida db sync --pull` automatically
  bumps `Done → Completed` once a commit referencing the spec lands on
  the default branch, so you don't need a second command after the PR
  merges.
- Removes the item from the queue
- Stamps the manifest row's `completed_at` (when a session manifest
  covers the current session) so `aida session show --plan` flips
  ✓ Done

Equivalent to: `aida edit <spec_id> --status done && aida queue remove <spec_id>`

### Step 6: Next steps (state-aware) — trace:TASK-87 trace:TASK-260

After step 5 succeeds, surface a structured next-steps table so the
workflow self-guides instead of relying on improvised "want me to push?"
prompts. Don't auto-execute — the user picks.

**Detect state first.** Three signals decide which template to render:

```bash
aida session show --plan 2>/dev/null   # manifest rows + ✓/◐/○ status
aida queue next 2>/dev/null            # is there another item routed to this role?
aida session show 2>/dev/null | awk '/^Session /{print $2; exit}'   # session-id prefix
```

- **Manifest exists, all rows ✓ Done** → cluster drained
- **Manifest exists, some ◐ / ○** → cluster partial (mid-drain)
- **No manifest** (single-item pickup) → simple mode

**Glyph convention** (consistent across `/aida-pickup`, `/aida-pr`,
`/aida-review`): `▶` = primary recommended action, `⏵` = alternative path,
`🚪` = stop/exit. Recommendations must be CONCRETE — name the command, name
the IDs. "You might want to consider…" is not a Next step.

**Render multi-option prompts as a table.** When presenting 2+ paths
forward, render as a markdown table with columns Path / What happens / Why.
Use ▶ ⏵ 🚪 glyphs in the Path cell for the primary / alternate / exit
semantics. Emit it as a real GFM markdown table — *not* wrapped in a code
fence — so Claude Code's terminal draws the box-rule grid instead of raw
pipes. The **Why** column is load-bearing: it explains the role / lease /
worktree implication of each path, never just restates the action. A
single linear next-step stays a compact one-liner — the table is for 2+
options only. Full convention: `docs/skills-convention.md`.

**Templates** (substitute `<session-id>`, `<cluster-id>`, etc. from
detection above). Each shows a prose lead-in line followed by the
next-steps table — print the lead-in as a normal sentence, then the table
as a real GFM markdown table (no surrounding code fence):

*Cluster drained:*

Drained <N> items from <cluster-id>.

| Path | What happens | Why |
|------|--------------|-----|
| ▶ Open PR for this batch | `/aida-pr` | Same session, same lease — the batch is done; ship it before the context goes cold |
| ⏵ Pick up a different cluster | `aida queue work <EPIC-M>` | New scope → new lease + worktree; end this session first or the leases conflict |
| 🚪 Stop here | Ctrl+D, then `aida session end <session-id>` from the parent shell | Releases the cluster lease — the drained work is safe, the PR can wait |

*Cluster partial:*

<N>/<total> done on <cluster-id> (<remaining> remaining).

| Path | What happens | Why |
|------|--------------|-----|
| ▶ Keep draining this cluster | `aida queue work` (no-arg = next planned item) | Same role + lease + worktree — the manifest is the consent record, no re-confirm |
| ⏵ Pause + check on something else | `aida queue list --all` | Read-only peek; doesn't drop the lease, you can return to the drain |
| 🚪 Stop here | Ctrl+D, then `aida session end <session-id>` from the parent shell | Releases the lease mid-cluster; `[planned:by-<session>]` chips keep the rest claimed for next time |

*Simple mode, queue has more items routed to this role:*

✓ <SPEC-ID> done. <N> more items queued for <role>.

| Path | What happens | Why |
|------|--------------|-----|
| ▶ Grab next item | `/aida-pickup` | Same role + lease + worktree — reuse this session, no re-entry cost |
| ⏵ Wrap up what's shipped as a PR | `/aida-pr` | Still this session; ships the current branch before picking up more |
| 🚪 Stop here | Ctrl+D, then `aida session end <session-id>` from the parent shell | Releases the lease; the queued items stay routed to <role> for later |

*Simple mode, queue empty:*

✓ <SPEC-ID> done. Queue empty for <role>.

| Path | What happens | Why |
|------|--------------|-----|
| ▶ Open PR for what shipped | `/aida-pr` | Same session — nothing left queued, so ship the branch now |
| ⏵ Switch hats and queue more | `eval "$(aida role enter dialog)"` then `aida queue add <id> --for <role>` | Changes the active role on this shell; dialog is the producer seat that refills the queue |
| 🚪 Stop here | Ctrl+D, then `aida session end <session-id>` from the parent shell | Releases the lease; nothing is queued, so it's a clean stopping point |

Print exactly one block — don't dump all four templates. Don't auto-loop
without confirmation: the user may want to break, review, switch roles, or
call it for the day.

## Producer side reminder

If the user complains the queue is always empty, gently remind them about
the dialog/captain seat:

> The queue is filled by whoever wears the `dialog` role
> (`eval "$(aida role enter dialog)"`, then
> `aida queue add <id> --for implementer`).
> Want to switch hats and queue some work?

## Related skills / commands

- `aida role enter <name>` / `aida role list` — switch personas
- `aida queue list --all` — see the full queue including other-role items
- `aida queue add <id> --for <role> --note "..."` — route work
- `aida statusline` — confirm the active role + queue depth
- `/aida-rebase` — at session pickup, run `aida rebase --dry-run --json`
  to verify the branch isn't stale before diving in; the playbook in the
  `/aida-rebase` skill treats session pickup as an invocation trigger.
  trace:TASK-105

## Shell helper (for developers)

`aida role enter <name>` prints shell code; you must `eval` it for the role to
attach to the current shell. `aida dev shell-init --install` adds two helpers
(`aida-role` and `aida-off`) that wrap the eval, so you can type
`aida-role implementer` instead of `eval "$(aida role enter implementer)"`.
The helpers are convenience only — recommend the canonical `aida role enter`
form in user-facing instructions, since it works in every shell regardless of
whether the helpers are installed.
