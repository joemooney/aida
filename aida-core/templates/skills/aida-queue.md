---
name: aida-queue
description: View the current personal/role queue — what's routed to you, in what order, with priority and notes. Read-only counterpart to /aida-pickup; use this when you want to inspect the queue without committing to start the next item.
allowed-tools:
  - Bash
---

# AIDA Queue Skill

## Purpose

Show what's in the current queue without acting on it. Pairs with
`/aida-pickup` (which grabs the head item and starts work) — use
`/aida-queue` to plan, `/aida-pickup` to execute.

## When to use

- The user asks "what's in my queue?" or "what's next?" and wants the
  full picture, not just the head
- At the start of a work session — orient on what's queued before
  picking the next concrete item
- After queuing several items via `aida queue add --for <role>` from the
  `dialog` seat, to confirm the routing landed correctly
- Sanity check that `aida role enter <X>` resulted in the expected
  scope filter narrowing the visible queue

## Skip if

- The user already wants to dive into the next item — call `/aida-pickup`
  instead, which is the action skill
- The user is asking about queue mechanics / setup (use `aida queue --help`
  or point them at `/aida-onboard`)

## Active role

!`echo "Role: ${AIDA_SESSION_ROLE:-(none active)}"`

## Queue contents

!`aida queue list 2>/dev/null || echo "(no items, or no AIDA project here)"`

## Workflow

### Step 1: Show the queue

The default `aida queue list` filters to items routed to the active role
(via `--for <role>` at queue-add time) AND honors any scope filter
(`scope_tags` / `scope_status`) that role has set. Surface that filtered
view first — it's almost always what the user wants.

If the user wears no role (`AIDA_SESSION_ROLE` empty), `aida queue list`
shows the full unfiltered queue.

### Step 2: Offer broader views if useful

If the filtered queue is empty, offer the broader views before
concluding "nothing to do":

- `aida queue list --no-scope` — bypass the role's tag/status scope
  filter; show everything routed to this role
- `aida queue list --all` — bypass role routing too; show every queued
  item including those routed to other roles or unrouted

Surface these options as suggestions, not auto-runs — the user may want
to know "the queue is genuinely empty for me right now" rather than dig
into other roles' lanes.

### Step 3: Suggest next actions

After showing the queue, offer the natural follow-ups:

- `/aida-pickup` — grab the head item and start work
- `aida show <id>` — read details on a specific queued item
- `aida queue move <id> --top` / `--bottom` / `--before <other-id>` —
  reorder
- `aida queue remove <id>` — drop an item without completing it
- `aida role scope show` — see whether a scope filter is hiding items

Do not pick an item up automatically. Queue inspection is read-only by
design; the action is `/aida-pickup`.

## Empty queue

If `aida queue list --all` shows nothing routed to this role:

> Your queue is empty. Either nothing's been routed to `role:<X>` yet,
> or everything you queued has been completed.
>
> To populate it, switch to the producer seat:
> `aida role enter dialog`, then `aida queue add <id> --for <role>`.

If `aida queue list` is empty but `--no-scope` shows items:

> Nothing matches your role's current scope filter (`aida role scope
> show` to see what's narrowing). Pass `--no-scope` to one command, or
> `aida role scope clear` to drop the scope persistently.

## Producer-seat reminder

If the user is in `dialog` mode and runs `/aida-queue`, the role-default
filter shows items routed to dialog — usually empty since dialog produces,
not consumes. Suggest `aida queue list --all` so they see what they've
been routing to other roles.

## Related skills / commands

- `/aida-pickup` — the action counterpart: grab queue head and start work
- `aida queue add <id> --for <role>` — route work from the producer seat
- `aida role enter <name>` / `aida role scope show` — switch hat / see
  current scope filter
- `aida statusline` — one-line queue depth + role + cache freshness
