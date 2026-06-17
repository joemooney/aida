# Team-of-users support: roadmap

- **Date:** 2026-06-16
- **Specs:** EPIC-47 (team support). Builds on EPIC-46 (multi-user test coverage + cross-clone coordination).
- **Status:** Roadmap. Tier 1 = build now. Tier 2 = medium-term (design-first). Tier 3 = strategic (flag).
- **Gate:** `scripts/multi-clone-harness.sh` is the regression guard; each capability adds cases.

## 0. Where we are

EPIC-46 closed the *correctness* floor for multiple clones sharing one store: distinct node ids + non-colliding spec ids, store sync with conflict surfacing, and **cross-clone coordination** (leases + drain/solo locks on the store, so two clones can't double-work). The harness proves all 12 same-host cases green.

"A team of users" adds the *workflow* layer on top: distinct people need to **divide work, see each other, communicate, and resolve the conflicts that come from real concurrency** — across machines, not just clones on one host.

## 1. Capability map (what a team needs)

### Identity (who is each person)
- **Have:** node-id per clone; `current_user_id()` = `$USER`/`AIDA_USER`; per-user queue; history `author`.
- **Gap:** BUG-89 — two members with no `AIDA_USER` both resolve to `"default"` and collide. No guard that team members have *distinct* identities. No roster of who's on the team.
- **Need (Tier 1):** in a team context (a roster exists / >1 node registered), refuse or loudly warn the `"default"` identity and guide the member to set a stable `AIDA_USER`; a `aida team` roster.

### Work division (who does what)
- **Have:** per-user queues; an `owner` field on Requirement; advisor routes via `aida queue add --for <role>`.
- **Gap:** no per-*user* assignment verb; no "my work" / "assigned to X" view; queue is the *now-doing* list, not durable assignment.
- **Need (Tier 1):** `aida assign <spec> --to <user>` (sets the assignee + routes to their queue); `aida list --mine` / `--assigned <user>`; assignee shown in `aida show`. This is the core team workflow — TASK-749's "only meaningful with >1 human" gate is now met for *basic* assignment (full RBAC stays Tier 3).

### Visibility (situational awareness)
- **Have:** `aida status`, `aida session leases` (cross-clone after EPIC-46), `aida list inflight`.
- **Gap:** no team-wide view — who's on the team, who's active right now, who holds which lease/drain across the team.
- **Need (Tier 1):** `aida team` (roster + last-seen) and a cross-clone **coordination view** in `aida status` (the deferred coordination slice 3): the active `coordination/` claims with holder host/clone/agent/age.

### Conflict resolution (real concurrency)
- **Have:** rebase surfaces conflicts; `conflict.rs` (field-level detect + LWW) and `oplog.rs` exist but are **not wired into the live pull path**.
- **Gap:** same-spec concurrent edits → manual rebase (MU-203); append-only `history:` arrays conflict instead of union-merging (MU-204).
- **Need (Tier 1):** MU-204 — a custom merge for the append-only `history:` array (union by entry id) so concurrent status/field changes on one spec stop producing spurious conflicts. **(Tier 2):** MU-203 — wire `conflict.rs` field-level resolution (or a git merge driver) so safe same-spec edits auto-reconcile.

### Communication
- **Have:** mailbox (hybrid local + canonical-on-store), briefs (local-only).
- **Gap:** cross-user mailbox visibility requires a manual digest→push→pull; no "you were assigned / mentioned" surfacing.
- **Need (Tier 2):** auto-digest mailbox on `aida pull`/`push` so messages flow between users without a manual step; surface assignment + mentions as mail/notices.

### Multi-host (the actual team topology)
- **Have:** TTL/heartbeat liveness in the coordination layer (designed for cross-host); store sync over a remote.
- **Gap:** untested cross-host — the harness is same-host (pid liveness). Cross-host reclaim relies on TTL.
- **Need (Tier 1):** harness cases that simulate distinct hosts (override the host fingerprint) to prove TTL-based reclaim + that a *foreign-host* live claim is honored.

### Permissions / RBAC
- **Have:** advisor/implementer role gating (status transitions are advisor-only).
- **Gap:** no per-user permissions / who-can-do-what.
- **Need (Tier 3 — strategic, flag):** TASK-749. Only build on real demand; role gating covers the important case (who can approve) today.

### Onboarding
- **Have:** `aida init` / `aida node acquire` / fresh-clone auto-attach.
- **Gap:** no guided "join an existing team" flow; `AIDA_USER` isn't prompted, so members drift into the `"default"` collision.
- **Need (Tier 1):** fold into identity hygiene — when a clone joins an existing store with a roster, guide setting a distinct `AIDA_USER` + acquiring a node id.

## 2. Build plan

### Tier 1 — build now (this campaign)
1. **Assignment + my-work** (`aida assign --to`, `--mine`/`--assigned`, assignee in `show`, queue routing). *Biggest workflow win.*
2. **Team identity & awareness** (`aida team` roster + `aida status` cross-clone coordination view + BUG-89 distinct-identity guard + join-the-team onboarding guidance). *Covers identity + visibility + onboarding together — they share the roster.*
3. **History union-merge** (MU-204) — custom merge for the `history:` array so concurrent edits stop conflicting.
4. **Multi-host harness cases** — simulate distinct hosts; prove TTL reclaim + foreign-host claim honored. Extends the EPIC-46 harness to Phase 2.

### Tier 2 — medium-term (design-first)
5. **Field-level conflict auto-merge** (MU-203) — wire `conflict.rs`/oplog or a git merge driver into pull so safe same-spec edits auto-reconcile. Riskier; design before building.
6. **Auto mailbox sync + assignment/mention notifications** — digest on pull/push; surface "assigned to you".

### Tier 3 — strategic (flag, don't build yet)
7. **RBAC / permissions** (TASK-749) — per-user who-can-do-what. Build on real demand; role gating suffices today.
8. **Team server / web dashboard** — `aida-server` already exists; a team view is a bigger product surface.

## 3. Principles carried from EPIC-46
- **Substrate as bouncer** — coordination/identity are enforced by the store + programmatic gates, not by convention.
- **Best-effort on the network** — team features degrade to a warning + local behavior when the store is unreachable; never block work.
- **Harness-gated** — every capability adds a `MU-###` case; the suite is the regression guard.
- **Backward-compatible** — absent roster / assignee / coordination = current single-user behavior; old binaries don't break.

<!-- trace:EPIC-47 | ai:claude -->
