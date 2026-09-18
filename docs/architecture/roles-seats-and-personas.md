# Roles, seats, stakeholder personas, sessions, and users

<!-- trace:TASK-1260 | ai:codex -->

AIDA has four related identity concepts. A **role** is a persistent seat in the
build loop. A **stakeholder persona** is a temporary least-privilege gate at the
edge of that loop. A **session** is one unit of owned work, and a **user ID** is
the shell identity used to select a queue. They are deliberately not aliases.

| Concept | Storage | How you become it | Routable? | Can write? | Can hold a lease? | In `aida role list`? |
|---|---|---|---|---|---|---|
| Role (seat) | `~/.aida/roles/<name>.toml` or project `.aida/roles/` | `aida role enter <name>` or a launcher-selected role | Yes, with `aida queue add --for <role>` | Yes, within that role's authority | Yes | Yes |
| Stakeholder persona (gate) | No role file or history | Set `AIDA_SESSION_ROLE=guest` or `AIDA_SESSION_ROLE=requester` | No | `guest`: no; `requester`: Draft intake only | No | Yes, in a separate discovery-only section |
| Session | `.aida/sessions/<id>.toml` lease plus its worktree metadata | `aida session start` or `aida queue work` | No; it consumes routed work | Through its role | It is the lease-owning unit | No |
| User ID | Queue entries plus the current shell environment; not a role file | `--user`, then `AIDA_USER`, then `$USER`, then `$USERNAME` (Windows), then `default` | Queue key, not a routing target | No authority by itself | No | No |

## Role: a seat in the build loop

A role is a persistent, named workflow context. The starter seats are
`advisor`, `implementer`, `reviewer`, `integrator`, and `product`. A role can be
entered, receive queued work, own sessions with leases and isolated worktrees,
and—according to its authority—run drains, write review verdicts, or merge.
Role files preserve the context and activity needed across conversations.

A seat can therefore be a participant in the escalation cascade described in
[Autonomy and escalation](autonomy-and-escalation.md). `ADR-33` applies the
one-authoritative-driver rule to driver seats: a driver role has one
authoritative driver and may have non-authoritative companions. It does not
turn stakeholder personas into roles.

## Stakeholder persona: a gate, not a seat

The stakeholder personas are `guest` and `requester`:

- `guest` can read the supported graph and status surfaces but cannot write.
- `requester` can also file a `change-request`, `bug`, or `user` requirement;
  it is forced to Draft and tagged for advisor intake. It cannot approve,
  queue, change status, merge, or otherwise enter the build loop.

A persona is activated only through `AIDA_SESSION_ROLE=guest|requester`. It has
no role file, role history, queue route, lease, or singleton seat. The persona
check in `aida-cli-lib/src/permissions.rs` runs before role/roster resolution so
a roster or permissive fallback cannot grant it build authority.
`role_is_stakeholder_only` in `aida-cli-lib/src/queue_cmd.rs` keeps personas out
of queue routing. `print_stakeholder_personas` and the picker machinery in
`aida-cli-lib/src/role_cmd.rs` and `aida-cli-lib/src/lib.rs` make them
discoverable without creating role files.

This split exists so a PM, customer, or other external stakeholder can inspect
the graph—or file a request for later triage—without occupying a build-loop
seat. `STORY-1110` defines the least-privilege gate; `TASK-1237` makes the
personas visible without making them routable. Inspect either contract with
`aida show <id>`.

> **Anti-pattern:** `aida role add requester` creates a persistent build-loop
> role file. It does not activate the requester persona; it turns a gate into a
> seat and defeats the identity model. Use `AIDA_SESSION_ROLE=requester`.
> `BUG-1196` records the picker failure that exposed this ambiguity.

## Session: one unit of ownership

A session binds exactly one role to one owned scope through a lease and a
worktree. Roles describe *which workflow position acts*; sessions describe
*this particular unit of work*. Ending a session releases that ownership. A
stakeholder persona does not need or receive a session lease because it cannot
own build-loop work.

## User ID: the queue key

The user ID identifies whose queue a command reads or mutates. Resolution is
`--user` override, then `AIDA_USER`, then `$USER`, then `$USERNAME` on Windows,
then `default`. It is shell identity—not the active role, team-roster identity,
node ID, or email. Matching is case-insensitive while stored/displayed casing
is preserved. `BUG-89` unified queue-side resolution, and `TASK-951` made
identity comparisons case-insensitive.

For concise definitions, see the scaffolded
[machinery glossary](../../aida-core/templates/.aida/discipline/machinery-glossary.md).
