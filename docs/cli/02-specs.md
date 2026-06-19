# Chapter 2 — Specs: shaping the graph

Chapter 1 gave you the floor: capture, view, finish. This chapter is about the thing those captured specs actually form — **a graph**. AIDA's defensible value isn't the list of specs; it's the *typed relationships between them*, the *code-to-spec trace links*, and the ability to ask questions a flat per-feature tool structurally can't ("what is blocked by this?", "what code serves this spec?", "what's at risk if this slips?"). The commands here are how you shape, query, and groom that graph: refine a node (`edit`), wire edges (`rel`), traverse them (`graph`), bind code to specs (`trace`), find things (`search` / `grep`), prune the long tail (`archive` / `unarchive` / `del`), and run an optional quality lens over the text (`lint`).

> Manual contract reminder: this chapter explains *when and why*. For the exact flag list and defaults, `aida <command> --help` is the source of truth — we don't copy it here. We cover only the options whose *rationale* isn't obvious from their name.

---

### `aida edit`

*(Covered in full in [Chapter 1](01-getting-started.md#aida-edit).)* `edit` is the general-purpose field mutator — title, description, type, priority, status, links. The two things worth re-stating from the graph's point of view:

- **Tags: `--add-tag` / `--remove-tag`, never `--tags`.** `--tags` *replaces the whole set*; the partial-edit flags are the safe, scripting-idempotent way to touch one tag without clobbering the rest (they conflict with `--tags` on purpose so you can't mix the two mental models).
- **Edges from `edit`.** `--blocked-by` (alias `--depends-on`) and `--remove-blocked-by` add/remove dependency edges *after* creation — the same edges `rel add` makes, just wired in passing while you're already editing the node. They create the inverse `Blocks` edge atomically and are idempotent on re-add. For non-dependency edge types (`child`, `verifies`, custom), reach for `aida rel` instead.

One graph-relevant flag pair not obvious from the name: `--add-ref` / `--remove-ref` attach a *one-way external pointer* (`linear:LIN-123`, `github:owner/repo#123`) that renders as a link in `aida show` and is searchable — AIDA stores the breadcrumb but **does not sync state back** to the external system, so don't expect a closed Jira ticket to flip an AIDA spec.

**Chains with** — the disposing seat's `edit --status approved` is step 2 of the [journey](README.md#the-journey--from-empty-repo-to-shipped-feature); everyone's everyday refinement tool.

---

### `aida del`

**One line** — permanently remove a spec from the graph.

**Mental model.** `del` is *destruction*, not hiding. It tears the node out — and with it, the audit trail, history, and the edges that pointed at it (leaving dangling-target tombstones for `rel list --dangling` to surface). On a substrate whose whole value is *stable IDs and a durable graph*, deletion is the rare, deliberate exception, not a cleanup habit.

**Reach for it when** — a spec was created in genuine error: a duplicate filed seconds ago, a typo'd test row, a spec that should never have existed. The bar is "this never should have been a node," not "this work is done."

**Don't reach for it when** — the work is *finished* or *abandoned-but-real*. Completed/rejected specs are the historical record; hide them with [`aida archive`](#aida-archive), don't delete them. If a spec turned out to be a duplicate of a real one, prefer `aida rel add --type duplicate` (keeps the breadcrumb) over deleting. Deleting a spec other specs depend on leaves dangling edges — a graph wound, not a clean cut.

**Key options (rationale only).**
- `-y/--yes` — skip the confirmation prompt. The confirmation exists precisely because this is irreversible; only suppress it in scripts where you've already decided.

**Gotchas.** There is no undo. If you deleted something with inbound edges, run `aida rel list --dangling` afterward (and `aida doctor verify-relationships --repair`) to clean up the tombstones it left behind.

**Chains with** — almost nothing by design; it's a terminal act. The graceful alternatives — `archive`, `rel --type duplicate`, `edit --status rejected` — are what you usually want instead.

---

### `aida search`

**One line** — the everyday case-insensitive find across spec text.

**Mental model.** `search` is the *simple* lens over the cache's full-text index — it matches a substring across title, description, and comments and ranks results, with no regex to think about. It's the "I remember a word from that spec" tool. Like `list`, it defaults to the *live, non-archived, non-meta* set, so it answers about current work unless you widen it.

**Reach for it when** — you half-remember a spec by a word in its title or body and want to find its ID. The fast, forgiving lookup.

**Don't reach for it when** — you need a *pattern* (anchors, alternation, field-scoping, context lines) — that's [`aida grep`](#aida-grep). Or you want to traverse relationships rather than text-match — that's [`aida graph`](#aida-graph). `search` reads text; it doesn't walk edges.

**Key options (rationale only).**
- `--all` — the everything-escape-hatch: includes *archived AND deferred* specs. Reach for it when a default search comes up empty and you suspect the spec you want is shelved.
- `--archived` / `--deferred` — narrow to *only* that shelf, for auditing the archive or the conditional/primed backlog specifically (different question from `--all`).
- `--sync` — pull the store from origin before searching, same opt-in rationale as `aida list --sync`: the fast local path is the common case; reach for `--sync` when a collaborator or another session may have written.
- `--include-meta` — surface the seeded META AI-prompt specs, hidden by default for parity with `list`.

**Gotchas.** A default `search` that returns nothing isn't proof the spec doesn't exist — it may be archived or deferred. Re-run with `--all` before concluding it was never filed.

**Chains with** — `search` to find the ID → `show` it → act on it.

---

### `aida grep`

**One line** — regex/pattern search across spec fields, with grep's muscle memory.

**Mental model.** Where `search` is fuzzy-and-simple, `grep` is *precise-and-powerful*: extended regex, field-scoping, context lines, invert-match, count, files-with-matches — the familiar `grep` verbs, applied to the requirement graph instead of files. Reach for it when you need to express *exactly* what matches.

**Reach for it when** — you need a real pattern (an anchored ID prefix, an alternation, a regex over just the `tags` field), or you want grep-shaped output: `-l` for just the matching SPEC-IDs (pipe into a loop), `-c` for per-spec match counts.

**Don't reach for it when** — a plain substring would do — `search` is friendlier and ranks results. And `grep` is text, not graph: for "what blocks this" use `aida graph`.

**Key options (rationale only).**
- `-E/--extended-regex` — opt into ERE. Without it the pattern is treated literally, so reach for `-E` the moment you want alternation/grouping.
- `-f/--field` — confine the match to specific field(s) (`title`, `description`, `comments`, `tags`, `owner`, `feature`). The way to avoid a body-text false-positive when you mean "tagged X."
- `-l/--files-with-matches` / `-c/--count` — the scripting outputs: `-l` gives you a clean ID list to feed downstream; `-c` quantifies without dumping bodies.
- `-v/--invert-match` — find specs that *don't* match — useful for "everything without this tag/word."

**Gotchas.** `-i/--ignore-case` here is opt-in, the inverse of `aida search` (which is case-insensitive *by default*). If a `grep` misses what `search` found, you probably want `-i`.

**Chains with** — `grep -l <pattern>` → pipe the IDs into `aida edit` / `aida queue add` for a bulk operation.

---

### `aida comment`

**One line** — the discussion and doc-seed log attached to a spec.

**Mental model.** Comments are the *narrative margin* of a spec — design discussion, decisions-in-flight, doc seeds for later living-docs, observations not yet confirmed as bugs. They form an append-mostly thread (`add` / `list` / `edit` / `delete`, with `--parent` for replies). The one rule that matters: **a comment is commentary, not contract.**

**Reach for it when** — you want to record *why* something is the way it is, capture a doc seed during a design conversation, or thread a reply onto an existing discussion. Capture freely; a thought you didn't write down is lost.

**Don't reach for it when** — the thing you're writing is a **binding refinement** to what should be built. Implementers follow the description and acceptance criteria, **not** comments — so a design decision that must be honored belongs in `aida edit --description` / acceptance criteria, not a comment. Leaving a load-bearing constraint in a comment and assuming it'll be followed is a recurring, real failure.

**Key options (rationale only).**
- `add --parent <COMMENT_ID>` — thread a reply under an existing comment rather than starting a new top-level note. Keeps a back-and-forth readable.
- `add --author` — override the recorded author (defaults to `AIDA_AUTHOR` env or system user). For when one shell is posting on behalf of a named agent/role.
- `edit` / `delete --comment-id` — these address a comment by its *comment-id* plus `--req-id`, not by spec alone (a spec has many comments). Get the id from `comment list`.

**Gotchas.** `edit` and `delete` need *both* `--req-id` and `--comment-id` — the spec doesn't uniquely identify which comment you mean. Run `aida comment list <ID>` first to get the comment-id.

**Chains with** — comments captured during design become inputs to `aida digest` / the living-docs flow; doc seeds get promoted via `aida doc` (Chapter 9).

---

### `aida graph`

**One line** — traverse the relationship graph from a root spec.

**Mental model.** This is the command that justifies calling AIDA a *graph*. `graph <ID>` starts at one spec and walks edges: the transitive **blocked-by** chain (everything standing in this spec's way), the transitive **blocks** chain (everything it holds up), the parent/child **tree** rollup (epic with a status summary), the reverse **impact** set (what's at risk if this slips), or any custom edge type via `--follow`. These are precisely the questions a flat per-feature spec tool *structurally cannot* answer. Read-only; pick one mode (default `--tree`).

**Reach for it when** — "can I start this yet?" (`--blocked-by`), "what does finishing this unblock?" (`--blocks`), "how is this epic doing?" (`--tree`), "if this slips, what else slips?" (`--impact`). The planning and triage workhorse.

**Don't reach for it when** — you want the *edges of one spec* without transitive traversal — that's `aida rel list <ID>` (one hop). Or you want spec *text*, not structure — that's `search` / `grep`.

**Key options (rationale only).**
- `--impact` — reverse closure: everything (transitively) blocked by the root. The "blast radius" view before you touch or de-prioritize something.
- `--follow <TYPE>` — traverse an arbitrary (built-in or custom) edge type by name, outgoing, repeatable to walk several at once. The escape hatch for graph shapes beyond the blocked-by/parent built-ins.
- `--depth <N>` — bound the traversal to N hops. Reach for it on a deep tree when you only want the immediate neighborhood.
- `--json` — machine output for agents/scripts building rollups or dependency-aware schedulers.

**Gotchas.** Modes are mutually exclusive — pick at most one; passing several is undefined. The default (no mode flag) is `--tree`, so a bare `aida graph <epic-id>` gives you the epic rollup, *not* the blocked-by chain people often expect.

**Chains with** — `graph --blocked-by` before `queue work` (don't pick up something that's blocked); `graph --impact` before `edit --status rejected` (know what you're stranding).

---

### `aida rel`

**One line** — manage individual relationship edges (the typed wiring `graph` later traverses).

**Mental model.** If `graph` is *reading* the graph, `rel` is *writing* it one edge at a time: `add` / `remove` an edge of a given `--type` (`parent`, `child`, `duplicate`, `verifies`, `verified-by`, `references`, or a custom name), and `list` to inspect edges. Typed edges are what make the graph queryable — a `child` edge feeds the epic rollup, a `duplicate` edge records "same thing," `verifies` binds a test spec to what it covers.

**Reach for it when** — wiring structural relationships that aren't plain dependencies: parent/child hierarchy, marking duplicates, verification links, or any custom edge type your project uses. For the *dependency* edge specifically (`BlockedBy`), `aida edit --blocked-by` is the in-passing shortcut; `rel` is the general tool for every other type.

**Don't reach for it when** — you only need a dependency edge while already editing the spec — `edit --blocked-by` is fewer keystrokes. Or you want to *traverse* rather than wire — that's `graph`.

**Key options (rationale only).**
- `add -b/--bidirectional` — also create the inverse edge automatically. The right default for genuinely symmetric relationships (`duplicate`) so you don't hand-wire both directions.
- `add --force-parent` — override the guard that refuses a `child` edge onto a *terminal* (Completed/Rejected) parent. The deliberate escape for backfilling a forgotten child onto a closed epic.
- `list --target <ID>` — invert the query to "what edges point AT this?" — the "who depends on this epic?" lookup, which the positional/`--source` form can't express.
- `list --dangling` — surface edges whose target no longer resolves (tombstones from deleted specs). Pairs with `aida doctor verify-relationships --repair` to clean them.
- `list --all` — include edges between terminal-status specs, hidden by default so the global view stays focused on actionable work.

**Gotchas.** `rel list` with no args lists *every edge in the graph* — on a large project that's a firehose; pass a spec-id (or `--source`/`--target`) to scope it. The `--source` flag is just the explicit form of the positional ID, for scripts that want to be unambiguous.

**Chains with** — `rel add` wires the edge → `graph` traverses it. `rel list --dangling` → `aida doctor` to repair.

---

### `aida trace`

**One line** — bind code to the spec it satisfies (and verify that binding in CI).

**Mental model.** `trace` is the **anti-drift loop** — the thing that makes AIDA more than a spec database. A `// trace:SPEC-ID` comment in code (or a `(SPEC-ID)` commit trailer) is a *bidirectional breadcrumb*: from a spec you can find the code that serves it; from a line of code you can find the spec that justifies it. The subcommands split into two jobs: **recording** links (`add` by hand, `scan` from inline comments, `sweep` from commit trailers, `list` to read them back) and **enforcing** them in CI (`gate` validates the trailer spec-ids resolve; `coverage` checks the changed *code* is traced; `check` flags inline trace markers whose target has rotted).

**Reach for it when**
- you've written code for a spec — leave the inline `// trace:SPEC-ID | ai:<tool>` comment (the everyday path), then `trace scan --update` picks it up.
- you want to backfill provenance from history — `trace sweep` walks commits for `(SPEC-ID)` references.
- you're wiring CI provenance gates — `trace gate`, `trace coverage`, and `trace check` are the three checks.
- you suspect *existing* trace comments have gone stale (a spec was deleted, renumbered by the merge-gate, or rejected) — `trace check` scans the inline `// trace:SPEC-ID` markers already in the tree, resolves each against the live graph, and flags the dead links so a rotted trace goes red like a failing type.

**Don't reach for it when** — you just want to *see* what code exists for a spec — `aida show <ID>` already renders the git linkage (commits/files/branch/PR) without you running `trace list`. And don't conflate the two CI checks: `gate` asks "do the cited spec-ids exist and aren't rejected?"; `coverage` asks "is the changed code actually traced?" — different failure modes.

**Key options (rationale only).**
- `scan --update` — without `--update`, `scan` is a *dry read* that just reports discovered annotations; `--update` is what actually writes the links into the graph. The two-step is so you can preview before committing.
- `scan --extensions` — defaults to `rs`; widen it (`rs,py,ts`) on a polyglot repo or the scan silently misses non-Rust traces.
- `sweep --dry-run` — preview which commits reference specs before writing — the safe first pass on a long history.
- `gate --range` / `coverage --range` — the commit range to check; defaults to "the commits this branch adds." The flag is how a CI job scopes to a PR's range explicitly.
- `coverage --block` — flip coverage from report-only (CI stays green) to enforcing (CI fails on any uncovered coverable hunk). Report-only first while you tune exemptions; `--block` once the team's ready to require traces.
- `check --block` — flip the rot check from report-only (exit 0) to enforcing (exit non-zero on any *dead* trace link). `check` treats a deleted/renumbered target (`unknown`) or a rejected target as hard rot that `--block` fails on; a marker pointing at an *archived* spec still resolves, so it's reported as a soft signal but never blocks.
- `check --json` — machine-readable rot report (`total_traces`, `resolved`, `dangling`, `dangling_unknown`/`dangling_rejected`, `archived`, `rot_rate_pct`) for dashboards or a CI annotation step.

**Gotchas.** Keep the `trace:` marker a plain `//` comment, *not* a `///` doc comment on a `clap` field — a doc comment is both code *and* `--help` output, and SPEC-IDs must never leak into user-facing text. `coverage` exempts tests/generated/docs/config/vendored/pure-deletion/fmt-only/trivial hunks by design, so a green coverage report doesn't mean *every* line is traced — only every *coverable* one.

**Chains with** — step 6 of the [journey](README.md#the-journey--from-empty-repo-to-shipped-feature): build → `trace` → the binding survives into review and lets anyone later ask "what serves this spec?" `gate`/`coverage` run in CI alongside the merge.

---

### `aida archive`

**One line** — hide a completed/rejected spec from default views without destroying it.

**Mental model.** Archive is a **view-level flag, orthogonal to status** — *not* a lifecycle state. A freshly-Completed spec stays fully visible until you archive it; archiving just removes it from the default `list` / `history` / `search` views while preserving the YAML, the history, the audit trail, and its edges in graph traversals. It's how you keep the long tail of finished work from drowning the live view, without the data loss of `del`.

**Reach for it when** — a spec is genuinely *done with* (completed or rejected) and cluttering your daily views, or you want a bulk sweep of the closed long tail (`--older-than 30d`). The everyday "tidy the closed pile" tool.

**Don't reach for it when** — the spec is still *live* work — archiving non-terminal specs is guarded for a reason (the closed long-tail is the target). And don't reach for `archive` when you mean *delete* (`del`) — archive preserves; or when you mean *reject* (`edit --status rejected`) — archive is orthogonal to status, it doesn't say "we decided no."

**Key options (rationale only).**
- `--older-than <DURATION>` — bulk-sweep every spec last touched before a window (`30d`, `12h`, RFC3339). Mutually exclusive with a single ID; this is the maintenance verb.
- `--status <CSV>` — restrict the `--older-than` sweep to specific statuses; defaults to `completed,rejected` so a bulk sweep can't accidentally archive live work.
- `--dry-run` — print the sweep plan without writing. **Always** dry-run a bulk `--older-than` first.
- `--force` — opt past the safety rails: archive a *non-terminal* or *queued* spec, or let the sweep include non-terminal statuses. The deliberate override when you really do mean to shelve live work.
- `--verbose` — list each archived id during a sweep instead of just the throttled progress tick.

**Gotchas.** Because archive is orthogonal to status, an archived spec is **still in the queue** if it was queued — `queue list` ignores the archive flag, so a spec can vanish from `aida list` yet still appear in `queue list`. That split surprises people; if a queued spec disappeared from `list`, check whether it got archived.

**Chains with** — step 11 of the [journey](README.md#the-journey--from-empty-repo-to-shipped-feature), long after Completed. The opt-in auto-sweep on `aida pull` (gated on `[archive] auto_after_days`) automates it; `aida unarchive` reverses it.

---

### `aida unarchive`

**One line** — clear the archive flag so a spec reappears in default views.

**Mental model.** The exact inverse of `archive` — it flips one bit back, restoring the spec to default `list` / `history` / `search` views. Nothing about its *status* changes (it was never a status), so unarchiving a Completed spec leaves it Completed, just visible again.

**Reach for it when** — you archived something prematurely, or a long-closed spec became relevant again (a regression reopened the question, you need it in a report) and you want it back in the default view.

**Don't reach for it when** — you actually want to *reopen the work* — that's a status change (`aida rework` / `aida edit --status in-progress --force`), not an unarchive. Unarchiving only changes visibility.

**Gotchas.** Since archive isn't a status, `unarchive` won't make a Completed spec actionable — it just shows it again. If you need it back in the pipeline, change the status separately.

**Chains with** — the reverse of `archive`; pair with a status change if the work genuinely needs reopening.

---

### `aida lint`

**One line** — an opt-in EARS-style clarity lens over a spec's text.

**Mental model.** `lint` is a *deliberately optional* quality lens, never a required schema — AIDA stays graph-first (stable IDs, typed edges, traces), and clarity scoring is a bolt-on you reach for, not a gate you pass. It reads a spec's description + acceptance criteria and flags an empty/too-thin body, vague triggers, missing expected-behavior, conflicting constraints, and low-testability wording, printing suggested rewrites **as drafts only**. It is read-only and deterministic (no LLM call) — it *never* edits the spec.

**Reach for it when** — you're sharpening a spec before it goes to an implementer and want a mechanical second opinion on whether the acceptance criteria are testable, or you want to sweep a whole kind of spec for clarity debt (`--scope story`).

**Don't reach for it when** — you expect it to *fix* anything (it only suggests — you apply via `edit`), or you think it's mandatory (it isn't; a spec can be perfectly usable and lint-noisy). Don't let lint output block work; it's advisory.

**Key options (rationale only).**
- `--scope <KIND>` — sweep every spec of a kind (`feature` / `task` / `story`) instead of one ID; `feature` covers the requirement types (functional / non-functional / system / user). The "audit our backlog's clarity" pass. Omit it and pass a SPEC-ID to lint just one.
- `--json` — machine output, for wiring lint into a grooming script or a findings feed.

**Gotchas.** It scores *text*, not *correctness* — a perfectly-worded spec can be wrong, and a clumsy one can be right. Treat the suggestions as drafts to consider, never as edits to apply blindly. Because it's deterministic and read-only, running it costs nothing — but acting on it always goes through `aida edit`.

**Chains with** — `lint <ID>` → `edit --description` to apply any suggestion worth taking → ready for `queue add`.

---

### `aida defer`

**One line** — park a spec as *primed, conditional* work — hidden from the default view, but not filed away like archive.

**Mental model.** `defer` is a **view-level flag, orthogonal to status** (it doesn't touch the lifecycle state machine — same as archive). The distinction from archive is *direction in time*: **archive is retrospective** ("done with this, filed"); **defer is prospective** ("not now, but bring it back when X happens"). That `X` is the whole point — `--until "<condition>"` records the **revisit trigger** (free text, e.g. `--until "when the slice verb ships"`), and it's the one thing that separates a deferred spec from an archived one. Deferred rows drop out of `aida list` / `search` / `history` by default; `--deferred` shows only them, `--all` shows the union. `undefer` clears the flag (and its trigger) so the spec rejoins the default views.

**Reach for it when** — a spec is real and approved but *blocked on a future condition you can't act on yet* (an upstream release, a decision pending elsewhere) and you want it out of your working view without pretending it's done. Record the trigger so future-you knows what brings it back.

**Don't reach for it when** — the spec is *finished* (that's the lifecycle → Completed, then `archive` for the long tail); or it's blocked by *another spec* (use a `BlockedBy` edge — the graph tracks that precisely, and the pickability gate already excludes it). Defer is for conditions the graph *can't* express as an edge.

**Gotchas.** Because defer is a view-flag, a deferred spec keeps its status — a deferred Approved spec is still Approved, just hidden. And like archive, it won't show in default `list` until you `undefer` (or pass `--deferred`/`--all`) — the same "where did my spec go?" surprise, so reach for `--all` when something's missing.

**Chains with** — `defer --until` parks it; the trigger condition is your reminder; `undefer` (or a future trigger-aware sweep) brings it back to the ready set.

---

### `aida undefer`

**One line** — the inverse of `aida defer`: clear the deferred flag so the spec rejoins default views.

**Mental model.** Undefer removes the view-level deferred flag *and* its `--until` revisit trigger, so the spec reappears in `aida list` / `search` / `history` without `--deferred`/`--all`. It's the "the condition I was waiting for happened — bring it back" verb.

**Reach for it when** — a deferred spec's revisit trigger has come true and you're ready to work it (or just see it) again.

**Don't reach for it when** — you want to *keep* it deferred but peek at it — pass `--deferred` to the read commands instead; undefer is a state change, not a view toggle.

**Chains with** — the counterpart to `defer`; after undefer, the spec is back in the open-work views and eligible for `queue add` / the pickability gate.

---

## Where to go next

You can now shape, query, and groom the graph. Next:
- **[Chapter 3 — Work & autonomy](03-work-autonomy.md)**: turning approved, well-shaped specs into queued work agents can drain.
- **[Chapter 4 — Git & lifecycle](04-git-lifecycle.md)**: where `trace` provenance and the `done → completed` auto-bump live.
- **[Chapter 1 — Getting started](01-getting-started.md)**: the `add` / `list` / `show` / `edit` floor these graph commands build on.
