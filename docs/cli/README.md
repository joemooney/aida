# AIDA CLI Reference Manual

> **What this is.** `aida <command> --help` tells you *what* a command does and *which* flags exist. This manual tells you **when and why** — the mental model behind each command, when to reach for it, when *not* to (and what to use instead), and how the commands chain into one coherent lifecycle. It's the difference between a parts list and an owner's manual.
>
> **Who it's for.** A new AIDA user who wants to understand the whole shape before diving in; an experienced user who hit a command and thought "wait, when would I actually use *this*?"; a reviewer who wants to see that the surface is coherent, not accreted.

## How this manual is organized

The reference body mirrors `aida help-all` — the same 11 thematic chapters the binary itself maintains, so the manual and the tool never disagree on *which* commands exist (a drift-guard, [docs/cli/_completeness](#drift-guard) below). Read the **journey** first for the story; dip into the **chapters** for any single command.

Two ways in:

1. **[The journey](#the-journey--from-empty-repo-to-shipped-feature)** *(read this first)* — the narrative spine. One feature's life, start to finish, naming every command you touch and why. This is the "what to expect" story.
2. **The chapters** *(reference)* — every command, grouped as `help-all` groups them, each with the full *when/why/when-not* treatment.

Each chapter covers exactly the commands `aida help-all` lists under the named group — we deliberately **don't** hand-list them here (that table would drift from the binary; the drift-guard enforces coverage instead). Run `aida help-all` for the live membership.

| # | Chapter | `help-all` group | Status |
|---|---------|------------------|--------|
| 1 | [Getting started & the daily drivers](01-getting-started.md) | *Getting started* (+ `edit`) | ✅ |
| 2 | [Specs — shaping the graph](02-specs.md) | *Specs* | ✅ |
| 3 | [Work & autonomy](03-work-autonomy.md) | *Work & autonomy* | 🚧 in progress |
| 4 | [Git & lifecycle](04-git-lifecycle.md) | *Git & lifecycle* | ✅ |
| 5 | [Planning](05-planning.md) | *Planning* | ✅ |
| 6 | [Roles & sessions](06-roles-sessions.md) | *Roles & sessions* | ✅ |
| 7 | [Project setup](07-project-setup.md) | *Project setup* | ✅ |
| 8 | [Reporting & lenses](08-reporting.md) | *Reporting* | ✅ |
| 9 | [Integrations & servers](09-integrations.md) | *Integrations & servers* | ✅ |
| 10 | [Storage & data](10-storage.md) | *Storage & data* | ✅ |
| 11 | [Working on AIDA itself](11-dev.md) | *Working on aida itself* | ✅ |
| — | [Glossary](12-glossary.md) | *(generated)* | ✅ |

> **The Glossary chapter is generated, not hand-written.** [`12-glossary.md`](12-glossary.md) is produced by `bash docs/cli/generate-glossary.sh` (or `make book-glossary`) from `aida docs glossary` — the binary's embedded machinery + lifecycle vocabulary. Edit a term in `aida-core/templates/docs/aida/discipline/{machinery-glossary,lifecycle-vocabulary}.md`, rebuild the binary, re-run the generator, and the page follows. Per ADR-5, there is no hand-maintained term list to drift.

> **Structured for downstream consumers.** Every command entry uses the same parseable shape — an ``### `aida <cmd>` `` header followed by fixed labeled fields — so a tool like `aida-tutor` (or a future `aida manual <cmd>`) can split any chapter into `{command → {field → text}}` without heuristics. And the manual carries **no SPEC-IDs** (`STORY-x`/`TASK-x`): it's user-facing, and those are kept out of user-facing surfaces by the same convention that keeps them out of `--help`. The drift-guard fails on either violation.

## How to read a command entry

Every command in the chapters follows the same shape, so you can scan for the part you need:

> ### `aida <command>`
> **One line** — what it is.
> **Mental model** — the concept you need to hold to use it correctly.
> **Reach for it when** — the situations it's the right tool for.
> **Don't reach for it when** — and what to use instead (the anti-pattern guard).
> **Key options** — only the flags with non-obvious *rationale* (the obvious ones live in `--help`; we don't re-type them here — that's how manuals rot).
> **Gotchas** — the thing that bites people.
> **Chains with** — what usually comes before/after in the lifecycle.

We deliberately **do not** reproduce the full flag list or exact defaults — `aida <command> --help` is the source of truth for that, and copying it here guarantees drift. This manual owns *rationale*; `--help` owns *facts*.

## Drift-guard

A completeness check (filed as a slice of this effort) asserts that **every command in `aida help-all` has an entry in some chapter**, and flags any command whose `--help` changed since its entry was last touched. The manual can fall behind on *prose*, but it cannot silently *omit* a command — the gate fails CI first.

---

## The journey — from empty repo to shipped feature

*(This is the narrative spine. Each command links to its full chapter entry.)*

AIDA's commands feel like a pile until you see the one path they're all arranged around: **an idea becomes an approved spec, an approved spec becomes queued work, queued work becomes a branch, a branch becomes a merged PR, a merge becomes a completed spec, and a completed spec — eventually — becomes a released version.** Every command is a verb somewhere on that path, or a lens onto it. Here is that path, once, end to end.

**0 · Set up** — `aida init` turns a git repo into an AIDA project: it creates the store (an orphan `aida-store` branch holding one YAML per spec), a rebuildable cache, the MCP server registration, and the scaffolding (skills, hooks, templates). You do this once. *(→ Ch.1, Ch.7)*

**1 · Capture the idea** — a thought becomes a spec with `aida add`. At this moment it's a **Draft** — captured, not blessed. Capturing is cheap and you should do it freely; the gate that matters comes later. *(→ Ch.1)*

**2 · Dispose it (the front gate)** — someone with authority decides the draft's fate: approve it, reject it, or leave it parked. This is the **approval gate** — the deliberate human "yes, build this," distinct from merely having written it down. Approval moves Draft → **Approved**. *(→ Ch.2, Ch.8 `questions`)*

**3 · Queue it (the sign-off)** — an approved spec isn't work-in-progress until it's *queued*. `aida queue add` (or grooming via `aida backlog`) is the act of teeing a spec up for an agent to pick up — and because queueing is authority-gated, the queue *is* the record of what's been blessed for work. *(→ Ch.3)*

**4 · Pick it up** — `aida queue work` (or the pickup skill) leases the spec, spins a worktree, and starts an implementer session. The spec is now **In Progress**, and a *lease* marks who holds it so two agents never collide. *(→ Ch.3, Ch.6)*

**5 · Plan it (optional, for non-trivial work)** — `aida plan` / `aida ultraplan` turns a spec into a structured implementation plan under `docs/plans/`, so the implementer rides a brief instead of improvising. *(→ Ch.5)*

**6 · Build & trace** — the implementer writes code, leaving `// trace:SPEC-ID` breadcrumbs that bind code back to the spec it satisfies. This binding is the anti-drift loop: later, anyone can ask "what code serves this spec, and what spec does this code serve?" *(→ Ch.2 `trace`)*

**7 · Finish on a branch** — `aida queue done` (or `aida pr`) marks the work **Done** — "finished on a branch," a PR is open. Note the precise vocabulary: *done* ≠ *completed*. Done means the branch exists; nothing has merged yet. *(→ Ch.4)*

**8 · Review** — `aida review` runs the reviewer phase against the actual diff. A reviewer reads *code*; it can send the spec back to **Rework** or pass it toward merge. *(→ Ch.4)*

**9 · Merge & auto-bump** — when the PR merges to the default branch, `aida pull` notices a commit referencing the spec and promotes it Done → **Completed** automatically. You rarely set *completed* by hand; the merge earns it. *(→ Ch.4)*

**10 · Release** — periodically, `aida release` (via `scripts/release.sh`) tags a version; the specs that merged since the last tag become **Released**. *(→ Ch.4, Ch.11)*

**11 · Archive** — long after a spec is completed, `aida archive` hides it from default views without deleting it — the YAML, history, and graph survive. Archive is a *view* flag, orthogonal to status. *(→ Ch.2)*

Around that spine sit the **lenses** (no state change, just sight): `aida list`/`show`/`graph`/`history`/`metrics`/`digest`/`findings`/`questions` — and the **autonomy ladder** (`aida burndown`/`drain`/`away`/`home`) that lets agents walk the spine for you while you watch. The chapters cover each in depth.

> The single most important vocabulary to internalize, because it trips up everyone who knows git but not AIDA: **committed ≠ pushed ≠ PR-opened ≠ merged ≠ completed ≠ released.** AIDA gives each its own verb and its own state. The full state machine is in [`docs/lifecycle.md`](../lifecycle.md); this manual shows you the *commands* that drive each transition.
