# Chapter 2 — Specs: shaping the graph

Chapter 1 gave you the floor: capture, view, finish. This chapter is about the thing those captured specs actually form — **a graph**. AIDA's defensible value isn't the list of specs; it's the *typed relationships between them*, the *code-to-spec trace links*, and the ability to ask questions a flat per-feature tool structurally can't ("what is blocked by this?", "what code serves this spec?", "what's at risk if this slips?"). The commands here are how you shape, query, and groom that graph: refine a node (`edit`), wire edges (`rel`), traverse them (`graph`), bind code to specs (`trace`), find things (`search` / `grep`), prune the long tail (`archive` / `unarchive` / `del`), and run an optional quality lens over the text (`lint`).

> Manual contract reminder: this chapter explains *when and why*. For the exact flag list and defaults, `aida <command> --help` is the source of truth — we don't copy it here. We cover only the options whose *rationale* isn't obvious from their name.

---

### Originating prompt provenance

Capture is **off by default**. To preserve why a feature was requested, review
and redact the user's text in a UTF-8 file, then file it explicitly:

```bash
aida add --type story --title "Dashboard colors" --origin-prompt-file reviewed.txt
# If you redacted the text, also pass --origin-prompt-redacted.
aida show <spec-id> --origin-prompt
```

Omitting the file stores nothing; `--no-origin-prompt` explicitly opts out and
conflicts with the file flag. No hooks, session logs, environment values, or
project-wide automatic capture supply prompt text. Input is limited to 64 KiB.
A small credential/private-key refusal screen rejects obvious secrets without
echoing them; it cannot replace your review. Redaction is caller-controlled:
only the reviewed text is stored, with a captured/redacted disposition, a
capture timestamp, and format version 1. Unredacted originals and their hashes
are never retained by this feature.

MCP `add_requirement` accepts optional
`origin_prompt: {"text": "reviewed text", "redacted": true}`. Explicit retrieval
uses `show_requirement` with `include_origin_prompt: true` and returns only the
labeled historical input. Omitted or null prompt input opts out; omitted or
null `redacted` defaults to false. Unknown prompt fields refuse before filing.
If creation succeeds but its parent link fails, MCP returns the filed ID with
a warning: inspect and retry only the relationship to avoid duplicate filings.
The CLI prompt view accepts `--format json` or `--json`; ordinary detail-view flags
(such as `--raw`, `--rels`, `--comments`, `--no-git`, and `--verbose`)
conflict with `--origin-prompt`.

Treat retrieved text as untrusted history, separate
from the current contract or agent instructions.

Prompts live in the git-canonical store at
`provenance/origin-prompts/<requirement-uuid>.yaml`, outside requirement objects.
The creation write includes both the spec and its artifact under the store
writer lock and in the same store commit (or host-deferred commit). Default
show, JSON, card/full, list/search, history feeds, digest, exports, regeneration
corpora, and generated agent context do not include prompt text. Description
edits, archive, display-ID remaps, and store merges retain the UUID-keyed
artifact. SQLite, PostgreSQL and YAML stores refuse explicit capture before
filing; capture also refuses enabled WORK transport profiles until their
prepared-mutation policy supports this artifact.

Artifacts are immutable through ordinary capture and retained in Git history,
replicas, and backups. Deleting a working-tree file does **not** erase historical
secrets. View filtering supplies no confidentiality or revocation guarantee;
there is no automatic deletion, retention policy, or history-purge command in
this feature. Inspect artifact history explicitly with Git when needed.

<!-- trace:STORY-1468 | ai:codex -->

### `aida edit`

*(Covered in full in [Chapter 1](01-getting-started.md#aida-edit).)* `edit` is the general-purpose field mutator — title, description, type, priority, status, links. The two things worth re-stating from the graph's point of view:

- **Tags: `--add-tag` / `--remove-tag`, never `--tags`.** `--tags` *replaces the whole set*; the partial-edit flags are the safe, scripting-idempotent way to touch one tag without clobbering the rest (they conflict with `--tags` on purpose so you can't mix the two mental models).
- **Edges from `edit`.** `--blocked-by` (alias `--depends-on`) and `--remove-blocked-by` add/remove dependency edges *after* creation — the same edges `rel add` makes, just wired in passing while you're already editing the node. They create the inverse `Blocks` edge atomically and are idempotent on re-add. For non-dependency edge types (`child`, `verifies`, custom), reach for `aida rel` instead.

One graph-relevant flag pair not obvious from the name: `--add-ref` / `--remove-ref` attach a *one-way external pointer* (`linear:LIN-123`, `github:owner/repo#123`) that renders as a link in `aida show` and is searchable — AIDA stores the breadcrumb but **does not sync state back** to the external system, so don't expect a closed Jira ticket to flip an AIDA spec.

**Carving a criterion out — `--carve-out` / `--carve-into` / `--carve-reason`.** When an accepted criterion turns out to belong on a *different* spec (a piece of scope split off, a closure condition repointed), hand-editing the description and leaving the reason in a separate comment splits into two halves that can drift: the stale wording keeps reading as the live gate, and the correction sits wherever someone happened to add it. `aida edit <SPEC> --carve-out "<exact criterion text>" --carve-into <TARGET-SPEC> [--carve-reason "..."]` does it as one atomic edit:
  - the criterion text — copied verbatim from the description, matched exactly or the edit refuses rather than silently no-op'ing — is struck and replaced with a pointer at the spec that now carries it;
  - a typed `carved-out-to` edge is recorded on the source (plus the inverse `carved-from` edge on the target), walkable by `aida graph --follow carved-out-to` / the `query_graph` MCP tool — not a string tag, so nothing has to parse English to answer "what still gates this spec";
  - the reason lands as a `CARVE-OUT:`-prefixed comment that `aida show`'s **default** view (no `-c`) surfaces inline right after the description — the free-text `CORRECTION:` / `PROXY DECISION:` conventions the advisor seat already used by hand get the same default-visible treatment;
  - `--carve-into` resolves through the same unambiguous-id path every other write uses — a target id that names two specs refuses instead of guessing.

  `aida do` additionally warns at pickup time, before the human contract, when a `drain`/`operator` spec still carries an unresolved carve-out — the moment stale acceptance text would otherwise steer a harness onto a gate that already moved.

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

**Chains with** — almost nothing by design; it's a terminal act. The graceful alternatives — `archive`, `rel add --type duplicate`, `edit --status rejected` — are what you usually want instead.

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
- `add --body-file <PATH>` / `add --stdin` — pass rich text without exposing
  backticks or `$()` to shell command substitution. The same inputs are
  available on `edit`, which makes an in-place repair safe.
- `edit` / `delete --comment-id` — these address a comment by its *comment-id* plus `--req-id`, not by spec alone (a spec has many comments). Get the id from `comment list`.

**Gotchas.** Never put Markdown containing backticks inside a double-quoted
shell argument: the shell expands it before AIDA starts, so no write-path
validation can recover or reliably recognize the original text. Put the body
in a file (a quoted heredoc is safe) and use `--body-file`, or pipe it via
`--stdin`. `edit` and `delete` need *both* `--req-id` and `--comment-id` — the
spec doesn't uniquely identify which comment you mean. Run `aida comment list
<ID>` first to get the comment-id.

**Chains with** — comments captured during design become inputs to `aida digest` / the living-docs flow; doc seeds get promoted via `aida doc` (Chapter 9).

---

### `aida graph`

**One line** — traverse the relationship graph from a root spec.

**Mental model.** This is the command that justifies calling AIDA a *graph*. `graph <ID>` starts at one spec and walks edges: the transitive **blocked-by** chain (everything standing in this spec's way), the transitive **blocks** chain (everything it holds up), the parent/child **tree** rollup (epic with a status summary), the reverse **impact** set (what's at risk if this slips), or any custom edge type via `--follow`. These are precisely the questions a flat per-feature spec tool *structurally cannot* answer. Read-only; pick one mode (default `tree`).

**Reach for it when** — "can I start this yet?" (`graph blocked-by`), "what does finishing this unblock?" (`graph blocks`), "how is this epic doing?" (`graph tree`), "if this slips, what else slips?" (`graph impact`). The planning and triage workhorse.

**Don't reach for it when** — you want the *edges of one spec* without transitive traversal — that's `aida rel list <ID>` (one hop). Or you want spec *text*, not structure — that's `search` / `grep`.

**Key options (rationale only).**
- `aida graph impact` — reverse closure: everything (transitively) blocked by the root. The "blast radius" view before you touch or de-prioritize something.
- `--follow <TYPE>` — traverse an arbitrary (built-in or custom) edge type by name, outgoing, repeatable to walk several at once. The escape hatch for graph shapes beyond the blocked-by/parent built-ins.
- `--depth <N>` — bound the traversal to N hops. Reach for it on a deep tree when you only want the immediate neighborhood.
- `--json` — machine output for agents/scripts building rollups or dependency-aware schedulers.

**Gotchas.** Modes are mutually exclusive — pick at most one; passing several is undefined. The default mode is `aida graph tree`, so a bare `aida graph <epic-id>` gives you the epic rollup, *not* the blocked-by chain people often expect.

**Chains with** — `graph blocked-by` before `queue work` (don't pick up something that's blocked); `graph impact` before `edit --status rejected` (know what you're stranding).

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

**Gotchas.** `rel remove` removes only edges of the `--type` you name. Removing `references` (or `related`) also removes an old custom `related` edge to the same target. To repair every old custom edge whose spelling resolves to a standard type at once, use `aida db migrate-related-edges` (Chapter 10). `rel list` with no args lists *every edge in the graph* — on a large project that's a firehose; pass a spec-id (or `--source`/`--target`) to scope it. The `--source` flag is just the explicit form of the positional ID, for scripts that want to be unambiguous. A `parent`/`child` edge is always bidirectional — `rel add --type child` (without `-b`) also writes the parent's reciprocal `parent` edge, and `rel remove --type child` (without `-b`) removes both sides too, so the pair can't go stale by removing only one end.

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

**Don't reach for it when** — you just want to *see* what code exists for a spec — `aida show <ID> --full` renders the git linkage (commits/files/branch/PR) without you running `trace list`. And don't conflate the two CI checks: `gate` asks "do the cited spec-ids exist and aren't rejected?"; `coverage` asks "is the changed code actually traced?" — different failure modes.

**How explicit show linkage keeps the no-commits case fast.** Finding the commits that reference a spec means searching the message of every commit reachable from every ref, and proving that *none* do is the expensive case. `aida show` remembers, per spec and per worktree, which commits have already been checked, in a small local cache under the repository's git directory (`aida-cache/show-negative-v1`; never in the store and never tracked). A spec's first explicit linkage request runs the full search and, if it finds nothing, notes that. The next explicit linkage request runs the full search once more, this time recording exactly which commits it covered. Later linkage requests only check commits that are new since then, so a spec with no commits answers without searching history again. A spec that has commits always runs the full search, with no extra work in front of it. If the previous answer cannot be reused — refs moved while it was being checked, the history is shallow or uses grafts or replace refs, the git configuration changed, or the cache is missing, locked by another process or corrupt — `show` runs the full search, or reports the commit list as incomplete (shipped state unknown) while the check finishes over the next few shows. It never reports "no commits" without that proof. Deleting the cache directory only costs speed. The `files` part of linkage (`trace:` comments in the source tree) has a time limit covering the whole walk, including listing directories; a symlink loop, a link that points outside the project, or an unreadable directory marks the file list incomplete instead of silently skipping it. <!-- trace:BUG-2221 | ai:claude -->

**Linked documentation in `aida show`.** The human view lists a **Documentation** section when any Doc entry is linked to the spec, whether the spec references the Doc or the Doc was filed about the spec with `aida doc add --about`; archived, Rejected and Superseded Docs are omitted. Each entry shows its ID, title, audience, scenario and tags, how it is linked, and the `aida doc show <DOC-ID>` command that prints it. Entries are grouped by authorship as recorded in their tags: `human` marks a human-authored explanation, `generated` or `ai-generated` marks generated prose, and anything else is listed as documentation whose authorship is not recorded. Nothing is written to the spec, and the TOON and JSON output are unchanged. The section is absent when no eligible Doc is linked. <!-- trace:TASK-1471 | ai:claude ai:codex -->

**Key options (rationale only).**
- `scan --update` — without `--update`, `scan` is a *dry read* that just reports discovered annotations; `--update` is what actually writes the links into the graph. The two-step is so you can preview before committing.
- `scan --extensions` — defaults to `rs`; widen it (`rs,py,ts`) on a polyglot repo or the scan silently misses non-Rust traces.
- `sweep --dry-run` — preview which commits reference specs before writing — the safe first pass on a long history.
- `gate --range` / `coverage --range` — the commit range to check; defaults to "the commits this branch adds." The flag is how a CI job scopes to a PR's range explicitly.
- `coverage --block` — flip coverage from report-only (CI stays green) to enforcing (CI fails on any uncovered coverable hunk). Report-only first while you tune exemptions; `--block` once the team's ready to require traces.
- `coverage --all` / `coverage --path <PATH>` — whole-tree mode instead of a diff. It's a gap detector: for each Rust file it lists the functions no live `trace:` marker claims, plus a traced/total count. A function counts as traced when a marker sits inside it, in the comments and blank lines leading up to it, above its `impl` block, or in a module-level `//!` anchor, the same rules `aida lint constants` uses. An untraced function is behavior no spec claims, and it's the cheapest place to look for unspecified semantics. The test, generated and vendored file exemptions still apply. Report-only; `--fail-on uncovered` turns any untraced function into a non-zero exit. `--json` emits per-file counts and names.
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
- `--older-than <DURATION>` — bulk-sweep every spec last touched before a point: a relative window (`30d`, `12h`, `2w`, `24 hours ago`), an ISO date (local midnight), a zone-less ISO datetime (local time), or RFC3339 — the same grammar as every other time bound. <!-- trace:TASK-1509 | ai:claude --> Mutually exclusive with a single ID; this is the maintenance verb.
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

**`aida lint constants` — unexplained numbers in traced code.** The source-side counterpart: it scans traced Rust code and reports every numeric literal whose value the owning spec never mentions — neither in its description nor in a decision linked to it. Code is traced under the same rule as `trace coverage --all`: a module-level `//!` anchor covers the file, a marker on an `impl` covers its methods, and a marker inside a function, const or static, or in the comments and blank lines leading up to it, covers that item. A trace id that matches no spec is reported as unresolved. Values compare by number, so `0x0F`, `1_5u64` and `15.0` all match a spec that says "15". `0` and `1` are exempt; silence any other deliberate choice with a trailing `// arbitrary` (or `// arbitrary: <reason>`) comment on the literal's line. Test, generated and vendored files are skipped, as in `aida trace coverage`. Each finding is a prompt to either record the value as a fact in the spec or mark it arbitrary.
- `--path <PATH>` — scan one file or directory instead of the whole project.
- `--json` — machine output naming file, line, literal, value and owning spec.
- `--fail-on unexplained` — exit non-zero when anything is reported. Without it the command is report-only and always exits 0; it never edits the store.

---

### `aida defer`

**One line** — park a spec as *primed, conditional* work — hidden from the default view, but not filed away like archive.

**Mental model.** `defer` is a **view-level flag, orthogonal to status** (it doesn't touch the lifecycle state machine — same as archive). The distinction from archive is *direction in time*: **archive is retrospective** ("done with this, filed"); **defer is prospective** ("not now, but bring it back when X happens"). That `X` is the whole point — `--until "<condition>"` records the **revisit trigger** (a typed condition, e.g. `--until "pr:123=merged"` or `--until "date:2026-12-01"`), and it's the one thing that separates a deferred spec from an archived one. Deferred rows drop out of `aida list` / `search` / `history` by default; `--deferred` shows only them, `--all` shows the union. `undefer` clears the flag (and its trigger) so the spec rejoins the default views.

**Reach for it when** — a spec is real and approved but *blocked on a future condition you can't act on yet* (an upstream release, a decision pending elsewhere) and you want it out of your working view without pretending it's done. Record the trigger so future-you knows what brings it back.

**Don't reach for it when** — the spec is *finished* (that's the lifecycle → Completed, then `archive` for the long tail); or it's blocked by *another spec* (use a `BlockedBy` edge — the graph tracks that precisely, and the pickability gate already excludes it). Defer is for conditions the graph *can't* express as an edge.

**Gotchas.** Because defer is a view-flag, a deferred spec keeps its status — a deferred Approved spec is still Approved, just hidden. And like archive, it won't show in default `list` until you `undefer` (or pass `--deferred`/`--all`) — the same "where did my spec go?" surprise, so reach for `--all` when something's missing.

Supplied triggers must use `spec:<spec-id>=completed`, `pr:<number>=merged`, `release:>=<version>`, `date:<YYYY-MM-DD>`, `query:<filters> count<op><n>`, or `manual:<who>`. Free text is refused. A manual condition requires a non-empty `--reason`; the existing product/advisor/operator authority gate still applies to reasoned holds. A bare re-deferral preserves the previous trigger and timestamp. The one-time free-text trigger migration has been applied; any remaining unparseable trigger stays parked for repair. Question answers that park work store typed consequences canonically; prose consequences become `manual:<resolver>` with the original text retained in a comment. An empty or whitespace-only consequence preserves an existing trigger, or defaults to `manual:<resolver>` if none exists, without adding an empty comment. Resolver identities that cannot form a valid manual condition (for example, empty names or names with internal whitespace) use the parseable fallback `manual:operator`; any prose comment retains its original author. In the TUI, blank condition input issues a bare defer, and rejected conditions show the CLI diagnostic in the status line.

**Chains with** — `defer --until` parks it; `undefer` or the hourly `defer resurface` sweep brings it back to the ready set.

### `aida defer resurface`

Evaluates typed conditions on deferred/parked specs without changing lifecycle
status. A met condition clears a plain deferral and records the observation in
an audit comment. Met operator holds go to `aida human` for a decision and stay
held. Manual conditions on deferrals older than 14 days also route to that queue;
changing the trigger preserves the original deferral timestamp, so replacing an
older trigger with a manual condition may route it on the next hourly sweep.
`--manual-age-days N` changes the threshold. Existing queue rows are not duplicated.
Removing a queue row without lifting the hold or changing the condition allows
it to be routed again on the next sweep. Missing timestamps or triggers
stay parked. A condition naming an unknown spec ID also stays parked as Unknown;
store read failures still fail the sweep. Archived specs are skipped. Use `--dry-run` to preview actions
without writing specs or queues.

Unknown observations (including failed forge/release probes) are reported and
left parked, with a successful exit. Free-text and unparseable triggers are
also Unknown and remain unchanged, including invalid typed syntax such as
`spec:` or `date:bad`; the run reports `N deferrals with unparseable triggers
requiring repair` as information. A store containing only these triggers
exits successfully without writing objects or queues. Unknown spec IDs also stay
parked successfully. Only real evaluation errors and store/internal errors
produce a nonzero exit after the remaining specs are checked. Concurrently edited specs
are retained for a later sweep. This repository registers the sweep hourly as
the timer-only `defer-resurface` job and routes job failures to the advisor.

---

### `aida undefer`

**One line** — the inverse of `aida defer`: clear the deferred flag so the spec rejoins default views.

**Mental model.** Undefer removes the view-level deferred flag *and* its `aida defer --until` revisit trigger, so the spec reappears in `aida list` / `search` / `history` without `aida list --deferred` / `aida list --all`. It's the "the condition I was waiting for happened — bring it back" verb.

**Reach for it when** — a deferred spec's revisit trigger has come true and you're ready to work it (or just see it) again.

**Don't reach for it when** — you want to *keep* it deferred but peek at it — pass `aida list --deferred` to the read commands instead; undefer is a state change, not a view toggle.

**Chains with** — the counterpart to `defer`; after undefer, the spec is back in the open-work views and eligible for `queue add` / the pickability gate.

---

## Pre-registering a hypothesis

Use an existing spike or decision with the `hypothesis` tag (matched
case-insensitively, so `Hypothesis` counts too). Write the prediction
before running the experiment; a useful description skeleton is:

```markdown
## Prediction
The outcome I expect, stated so an experiment can test it.

## Refuted if
The observable result that would refute this prediction.

## Experiments
The linked specs that will record results.
```

Create a `references` or `verified-by` relationship to each experiment. Either
edge direction counts. In the canonical git store, the first mutation that
links a Completed spec or completes an already-linked experiment permanently
locks the hypothesis description. The same transaction records
`hypothesis_lock.locked_at` and a SHA-256 of the UTF-8 description. A description
rewrite combined with the first result is refused before any rows are written.

After locking, append amendments with `aida comment add <ID> "..."`. Title,
tags, comments, and relationships can still change. Removing the tag or link,
reopening or deleting the experiment, and stale saves never clear the lock.
Deleting or replacing the locked hypothesis itself is refused; archive it to
hide it. There is no unlock flag. Untagged legacy rows keep their normal edit
behavior. The archived SQLite/Postgres backends and the YAML backend do not
store or enforce this lock; use the canonical git store for pre-registration.

Store sync between clones (`aida pull`, `aida db sync`, `aida remote
reconcile` and the coordination refreshes) keeps this evidence too, even when
Git merges the YAML without a conflict. A sync that would bring in a changed
locked description, a deleted or renamed locked hypothesis, or two different
pre-result descriptions for a hypothesis that just reached its first result is
refused before your store changes; fix the description in one clone and sync
again. When the first result only exists once both clones are combined (the
link came from one, the completion from the other), the sync locks the
hypothesis in an extra store commit. See
[store sync and pre-registration](10-storage.md#store-sync-and-pre-registration).

A raw `git merge` or `git rebase` of the store bypasses that check. If one
rewrites a locked description, the hash no longer matches. That row then
refuses every change except one that restores the frozen text, for example
`aida edit <ID>` with the original description from store history. Other rows
stay writable. Binaries older than this feature do not know `hypothesis_lock`
and drop it when they rewrite the row, so upgrade every clone that writes to
the store.
<!-- trace:STORY-1675 | ai:codex -->
<!-- trace:STORY-1675 | ai:claude -->
<!-- trace:BUG-2162 | ai:claude -->

---

## Where to go next

You can now shape, query, and groom the graph. Next:
- **[Chapter 3 — Work & autonomy](03-work-autonomy.md)**: turning approved, well-shaped specs into queued work agents can drain.
- **[Chapter 4 — Git & lifecycle](04-git-lifecycle.md)**: where `trace` provenance and the `done → completed` auto-bump live.
- **[Chapter 1 — Getting started](01-getting-started.md)**: the `add` / `list` / `show` / `edit` floor these graph commands build on.

---

### `aida intent note`, `validate`, and `export`

Capture an observation, question, or request for manual review without creating a
requirement. These local commands work outside an initialized project. They skip
identity, role, telemetry, cache and store setup and do not commit or contact a
remote. The existing `aida intent <SPEC>` comprehension command remains available.
Use the direct spellings below: user aliases expanding to local observation
commands are refused by the later dispatcher. Validation accepts `--format json`
or `AIDA_OUTPUT_FORMAT=json` as well as its dedicated `--json` flag.

```bash
aida intent note --category observation --title 'Unexpected retry behavior' --file note.md > observation.json
aida intent validate --file observation.json --json
aida intent export --file observation.json > attachment.json
# Manually attach attachment.json to an email. At the receiving machine:
aida intent validate --file attachment.json
```

The body comes only from the named UTF-8 regular file; the title is explicit.
Nothing infers an author, hostname, path, spec reference, Git provenance, or other
metadata. Imported prose is untrusted data and never executed or sent to an AI.
Export validates first and emits the complete JSON attachment on stdout; it
neither composes nor sends email. The human label and a reminder that validation
does not certify sanitization go to stderr, leaving the attachment machine-readable.

All three commands accept repeatable `--deny-pattern '<regex>'` flags that scan
the decoded title and body. Patterns are explicit, case-sensitive unless marked
`(?i)`, and never discovered from the store transport profile or project config.
For example, `--deny-pattern '(?i)private-project'` refuses a matching phrase.
There are at most 256 patterns of 1–256 UTF-8 bytes each. Refusals name the rule
number without echoing the pattern or matched text. Use the desired patterns at
both ends; validation is not a sanitization certificate, and a checksum proves
content consistency rather than authorship or trust. Review text before sharing.

**Envelope v1.** Exactly six keys: `version` (integer `1`), `kind`
(`"observation"`), `category` (`"observation"`, `"question"`, or `"request"`),
`title`, `body`, and `checksum` (64 lowercase SHA-256 hex characters). Unknown,
duplicate or missing keys, unsupported kinds/versions, wrong types, trailing
non-whitespace JSON and checksum mismatches refuse. Kinds are closed per version;
other kinds require a future explicit implementation.

The digest covers compact UTF-8 JSON of the first five fields, in precisely that
order, using serde_json string escaping, with no trailing newline. It excludes
`checksum`. There is no Unicode, whitespace or line-ending normalization inside
strings. Input key ordering and JSON presentation whitespace do not change the
hash. For example, this exact hash input:

```json
{"version":1,"kind":"observation","category":"observation","title":"title","body":"body"}
```

has SHA-256 `3151613e3ab4dd698b130ff13f28af30f351b7c021cc1c167ffb35d2ef950a29`.
Its display label is `INTENT-3151613e3ab4`; full digests identify envelopes.

Titles must be nonempty and at most 512 UTF-8 bytes, bodies at most 64 KiB, and
serialized envelopes at most 128 KiB. JSON escaping counts toward the envelope
limit. Input reads are bounded before parsing; symlinks and special files refuse.
Generated/exported JSON has no trailing newline so a maximum-size attachment
round-trips. `validate --json` reports the label, full checksum and
`sanitization_certified: false`, without repeating imported text.

These local commands do not publish. Enrolled intake transport is described
below; backup-comment recovery remains a separate slice.
<!-- doc-intent: shaped by STORY-2479 -->

### Untrusted review returns

<!-- trace:STORY-2493 | ai:codex -->
`aida intent review-return --file input.json` generates an explicit finding or
pending verdict without discovering a store, identity, logs or local paths.
Input has exactly `version`, `kind`, `payload`, and an empty `checksum`; generation
fills the checksum. `validate` and `export` also accept these closed envelopes.
For example:

```json
{"version":1,"kind":"finding","payload":{"severity":"major","blocking":true,"title":"Missing error handling","body":"The failure is ignored.","file":"src/lib.rs","line":12},"checksum":""}
```

Finding severity uses the canonical values `major`, `minor`, `cosmetic`,
`observation` or `note`. The file must
be an explicit home-relative path with no empty, dot or parent components,
backslashes, colons or control characters; the line is a positive 32-bit integer.
Title/body limits match observations; paths are bounded at 4096 bytes.

A `review-verdict` payload contains exactly `pr` (positive integer), `head_sha`
(40 lowercase hex characters), `decision` (`approved`, `request-changes`,
`rejected` or `waiting`), `blocking_findings` (at most 256 unique full lowercase
SHA-256 digests), and `reviewer_node` (32 lowercase hex characters). Verdict
publication requires per-writer enrollment: the node must match the enrolled
producer writer ID. This match labels the source; it does not authenticate a
reviewer or confer authority. Legacy enrollment still supports findings.

The checksum hashes compact JSON fields `version`, `kind`, `payload`, in that
order, excluding `checksum`. Finding payload fields hash in the example's order;
verdict fields hash in the order listed above. Strings are not normalized.
Unknown, duplicate, missing or mismatched fields refuse. Reads remain bounded
regular-file reads, and every decoded string field is scanned against explicit
reviewed deny patterns, including paths, enums, node IDs and digest strings.

Publish with `aida intent publish --file envelope.json --reviewed`; findings
require work-side operator review before this confirmation. Fixed-field verdicts
can publish without `--reviewed`. Enrolled policy is
rechecked before publication and on pull in quarantine. The independent transport
retains immutable `findings/<digest>.json` and `review-verdicts/<digest>.json`
paths; work operations never write canonical store refs.

At home, pull and inspect with `intent list`, then explicitly run
`aida intent process <full-digest> --action create`. Findings become draft tasks
with canonical review-finding, severity, blocking and digest tags. The description
retains the explicit home location. Receipts map the full digest and journal
operation to the home UUID. Retry the same command after a receipt-push failure;
the journal recovers the existing finding without filing it again. Imported
findings support `pending` and `create`, not comment or transport dismissal.

Verdicts remain pending data, with no processing action or merge-gate credit.
Blocking-digest projection and home operator-attested approval are later slices;
receiving an imported verdict currently neither adds a merge block nor approves
merge. Existing review and merge gates continue to apply.

### Native per-writer intake (first-round-trip slice)

<!-- trace:STORY-2652 | ai:codex -->
Opt into independent histories by supplying `enroll --role`. Enrollment without
`--role` retains the legacy flat transport described below; existing profiles
are never silently converted. Use separate clones for the producer and processor:

```bash
# Work clone: publish observations, read only the named processor's receipts.
aida intent enroll --remote ssh://git@example.org/intake.git --reviewed \
  --role producer --writer-id aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
  --receipt-writer bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
# Home clone: read that producer, write only this processor's receipts.
aida intent enroll --remote ssh://git@example.org/intake.git --reviewed \
  --role processor --writer-id bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb \
  --producer-id aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
```

Replace these example IDs with the agreed 32-character lowercase hex identities.
Without `--writer-id`, enrollment uses the existing WORK binding if present,
otherwise a new UUID saved only in the clone-local profile; it never acquires a
store node. Repeating enrollment retains that identity. Remote, deny policy,
role, identity and branch mappings are pinned; changes require another clone.
`--producer-id` is required for processors. A producer defaults to its own ID.
The current slice supports one producer and at most one authorized processor
per enrollment; processors cannot enroll another processor's receipt branch.

Profile v2 pins `producer_branch` to `refs/heads/aida-feedback-<producer-id>` and
`receipt_branches` to the explicitly authorized
`refs/heads/aida-feedback-receipts-<processor-id>`. A producer uses `publish`;
a processor uses `process`. Both use `pull` then `list --all --json` to see the
combined validated view. Only observations are admitted to producer histories,
and only version-1 processing receipts to receipt histories. Each branch has its
own parentless, linear, immutable history. V2 histories currently receive full
validation on every read, without the legacy checkpoint optimization. Receipt
validation is standalone:
a receipt for an observation not yet fetched is valid audit data, not a reason
to import an observation or write the store. Listing shows local observations.
`pull --json` reports receipt digests for absent observations under
`receipts_for_unknown_observations`; a receipt alone does not manufacture a
listing row. Envelope JSON and its hash contract are unchanged. Processing retains the operation journal and store
markers: retry the same command after a failed receipt push, without a second
requirement or comment write.

This is the approved S1 slice. Enrollment JSON reports `protection: unverified`;
configure and verify forge writer restrictions independently. IDs, fixed commit
authors and branch names are labels, not credentials. The protection probe and
own-branch foreign-write refusal remain S2 work. Receipt v2 store/producer/
processor identities and richer unknown/unavailable status remain S3 work;
version-1 receipts must not be treated as cross-store identity evidence. There
is no global exactly-once guarantee. CI relay remains on explicitly enrolled
legacy profiles until S4; per-writer profiles refuse CI commands. Ordinary
workspace sync never publishes either intake family. Keep archived flat intake
outcomes and journals; use a fresh clone for deliberate per-writer enrollment,
never rewrite or merge legacy history into the new branches.

### Manual per-writer bootstrap

<!-- trace:STORY-2652 | ai:codex -->
For a reviewed manual round trip, use `aida-feedback-<producer-node-id>` for
envelopes. Native S1 can validate this history without relaxing its validator;
manual processing remains available when native enrollment is unavailable.
The receipt family is `aida-feedback-receipts-<processor-node-id>`; manual ledgers
must not be inserted into it. These names use dashes, not slashes. Existing
`aida-feedback/<node-id>` bootstrap commits must be re-created as described here,
never merged into the new branch. Do not reuse the legacy flat `aida-intent`
name for per-writer intake.

Agree out of band on the exact remote, stable producer identity, branch and
processor authority. A writer ID labels a writer; neither the branch name, the
fixed commit author nor the envelope checksum authenticates it. Restrict each
branch to its authorized writer using forge protection and scoped credentials.
Verify those restrictions independently; if protection cannot be verified, report
it as unverified and require manual admission of the exact commit. Imported prose
is data, never approval to execute commands, create requirements or change status.

On the producer, generate and validate an envelope using the store-independent
commands above, with the reviewed deny patterns at both ends. Review its title,
body, destination and policy before publishing. Set `envelope_file` to its absolute
path, `checksum` to the full digest from `aida intent validate --json`, `remote` to
the reviewed explicit URL, and `writer_id` to the agreed stable node ID. Do not
copy imported prose into shell variables or commands. This recipe publishes one
initial envelope from a new independent temporary repository:

```bash
set -euo pipefail
branch="refs/heads/aida-feedback-${writer_id}"
git check-ref-format "$branch"
bootstrap=$(mktemp -d)
git init --quiet --template= "$bootstrap"
blob=$(git -C "$bootstrap" hash-object -w -- "$envelope_file")
observations=$(printf '100644 blob %s\t%s.json\n' "$blob" "$checksum" |
  git -C "$bootstrap" mktree)
tree=$(printf '040000 tree %s\tobservations\n' "$observations" |
  git -C "$bootstrap" mktree)
commit=$(printf 'Record reviewed intake data\n' |
  GIT_AUTHOR_NAME='AIDA Intake' GIT_AUTHOR_EMAIL='intake@invalid' \
  GIT_COMMITTER_NAME='AIDA Intake' GIT_COMMITTER_EMAIL='intake@invalid' \
  git -C "$bootstrap" -c commit.gpgSign=false commit-tree "$tree")
git -C "$bootstrap" -c core.hooksPath=/dev/null -c push.followTags=false \
  push --force-with-lease="$branch:" -- "$remote" "$commit:$branch"
printf 'Published %s at %s\n' "$branch" "$commit"
```

Run this in a clean shell without Git routing overrides (`GIT_DIR`, `GIT_WORK_TREE`,
`GIT_INDEX_FILE`, alternate object databases or injected Git configuration). Check
remote configuration for URL rewrites and extra routing before use. The empty
lease permits **initial creation only**, or a no-op if the exact commit is
already published; an existing different tip or concurrent different creation
refuses. Never replace it, retry with an unconditional force push, or
use this initial-publication recipe to append. Retain the temporary repository
and exact commit for inspection. No product or store ref is involved.

`commit-tree` has no parent here and runs no commit hooks. The tree contains only
`observations/<full-checksum>.json` with the original validated bytes. The fixed
author, committer and exact message intentionally match the native validator;
there is no `Aida-Store` trailer, signature or product ancestry. For an older
bootstrap commit, first fetch its exact ref into an independent scratch repository,
extract only the expected ordinary envelope blob, validate and review it, then
use these steps to build a new orphan commit. Do not copy its tree or ancestry.
This is explicit manual admission, not a relaxation of the history validator.

At home, set `remote`, `branch`, `checksum` and `expected_commit` from the agreed
producer handoff. Use a fresh independent scratch repository; do not fetch into
`main`, `aida-store`, or an enrolled legacy intake repository:

```bash
set -euo pipefail
inspection=$(mktemp -d)
git init --quiet --template= "$inspection"
git -C "$inspection" -c core.hooksPath=/dev/null fetch --no-tags \
  --no-write-fetch-head --no-recurse-submodules -- "$remote" \
  "$branch:refs/bootstrap/incoming"
actual=$(git -C "$inspection" rev-parse --verify refs/bootstrap/incoming)
test "$actual" = "$expected_commit"
git -C "$inspection" rev-list --parents "$actual"
git -C "$inspection" cat-file commit "$actual"
git -C "$inspection" ls-tree -rt --full-tree "$actual"
git -C "$inspection" cat-file blob \
  "$actual:observations/$checksum.json" > "$inspection/observation.json"
aida intent validate --file "$inspection/observation.json" --json
```

Add the agreed repeatable `--deny-pattern` options to validation. Confirm its full
digest equals `checksum`. Manually inspect the preceding output: exactly one
parentless commit, only the fixed tree/author/committer headers and exact message,
and exactly one mode-100644 blob at the expected path. Envelope validation alone
does **not** validate Git ancestry, paths or commit metadata. Any extra path,
parent, metadata, identity mismatch, deny-policy failure or changed tip refuses
admission. Neither fetch nor validation creates a requirement or executes prose.

After admission, an authorized home operator explicitly decides to leave pending,
dismiss, create a draft requirement, or comment using the ordinary authorized
AIDA commands. Before a store write, durably record the remote, exact branch and
commit, full digest, producer and processor identities, destination store identity,
chosen action and a fresh operation UUID in a private local processing ledger.
For create, include an `intake-op:<uuid>` tag; for comment, include a final
`[intake-op:<uuid>]` marker, matching the native recovery convention. Record the
result's stable requirement UUID and store identity, not just its display ID.

If interrupted or a command fails, inspect canonical store evidence for that
operation before retrying. A confirmed write is reported without repeating it;
missing or ambiguous evidence requires operator reconciliation. Exchange the
ledger outcome through an agreed authenticated channel, identifying the full
digest and exact source ref/commit. Distinguish transport acceptance from review,
dismissal and confirmed creation/comment. No response means **unknown**, not
pending or successfully processed. This manual ledger is not a native receipt
and must not be inserted into either branch family. Independent processors must
coordinate; the bootstrap procedure supplies no global exactly-once guarantee.
Retain the source commit, ledger and store markers for later explicit migration.

### Legacy flat intake: `enroll`, `publish`, `pull`, `list`, and `process`

Legacy intake (enrollment without `--role`) uses a separately enrolled destination
and policy. It works in a
Git clone without a requirement store. Enroll an explicit absolute local path,
HTTPS URL, or SSH URL (remote nicknames and helper protocols refuse):

```bash
aida intent enroll --remote ssh://git@example.org/intake.git --reviewed --deny-pattern 'private-project-name'
aida intent publish --file observation.json --reviewed
aida intent pull
aida intent list --json
aida intent process <full-sha256> --action pending
aida intent process <full-sha256> --action dismiss
aida intent process <full-sha256> --action create
aida intent process <full-sha256> --action comment --target <spec-id>
aida intent list --all --json
aida intent publish
```

Enrollment copies the reviewed regex policy, including an intentionally empty
list, into owner-only clone-local state. Re-enrollment is idempotent only for the
same destination and policy; use a separate clone to change either. HTTPS supports
anonymous destinations only: credential helpers and askpass are disabled, and
HTTPS URLs containing userinfo (including embedded tokens) refuse before being
persisted. Authenticated destinations must use `ssh://` with SSH keys or an SSH
agent; SSH runs in batch mode. Configure branch restrictions at the remote
independently. Local validation does not certify sanitization or restrict raw Git
clients.

The clone's common Git directory contains `aida-intake/profile.json`, a permanent
lock, and `aida-intake/worktree/` with its own `.git`, index, objects and refs.
Linked product worktrees share this one intake boundary. No product/store index,
ref, checkout, hooks, sync routine, or baseline participates. Commits use a fixed
intake identity and message; imported text is never executed or passed to an LLM.
Only the explicit `refs/heads/aida-intent` push/fetch mapping is used, with no tags,
force push, rebase, or implicit additional refs. Fetches stage into a private ref
and validate before moving the accepted branch; they do not write `FETCH_HEAD`.

Publication requires `--reviewed` when adding a file. It retains the exact
validated bytes at `observations/<sha256>.json`; an exact duplicate is a no-op,
while different serialization with the same digest refuses. Full reachable
history must be orphaned, linear and additive, containing only non-executable
observation JSON, processing-receipt JSON, and enrolled CI-result JSON. Unknown paths, empty trees, foreign commit
metadata, links, tampering, rewritten/deleted files, invalid receipts and denied
text refuse, including in intermediate commits. Validation is bounded to 4096
commits, 8192 files and 32 MiB of blob data, with a 30-second deadline checked
between commits and blobs. An owner-only `validated-tip` checkpoint skips
previously validated intermediate trees when its exact commit remains an ancestor
and the compiled validator sources match;
unrelated history receives full validation. Only accepted tips advance the
checkpoint; unpublished candidates and rejected incoming history do not. The checkpoint tree and all its live
files are checked again, and blobs are reused within one command. Removing the
checkpoint triggers full validation. Each Git call has a 30-second timeout and bounded
output.

These are **lifetime limits per intake repository**, not a rolling window. Rotate
before reaching 4096 commits, 8192 files or 32 MiB; maximum-size envelopes can
exhaust the byte limit in about 256 observations. Monitor the independent
repository with `git -C <clone-common>/aida-intake/worktree rev-list --count HEAD`
and `aida intent list --all --json`. There is no automatic compaction.

For planned rotation, stop publishers and processors, publish outstanding receipts,
and have every enrolled clone pull successfully. Archive the old remote and keep
the old clones, receipts, store markers and operation journals intact. Resolve all
journals before rotation. Create a new empty bare intake repository and new product
clones; enroll each against that new destination with its reviewed policy. Publish
only still-pending envelopes from the archived validated worktree, with explicit
review for each. Never republish processed envelopes or recreate their specs. Keep
the archive available for receipt lookup; do not delete it or force-rewrite its
history. If a cap or divergence has already prevented synchronization, stop and
reconcile both histories and store markers with an advisor before moving data;
starting over without receipts can duplicate spec writes.

Every write publishes before it becomes local history. `publish --file` and
`process` (dismiss, create or comment) first pull and validate the remote, build
one new commit on that tip, and push it; the accepted local branch moves only
after the remote accepts the push. If another clone published in between, the
push is refused as a non-fast-forward. The command then pulls again and builds a
fresh commit on the new tip (up to four attempts). A failed push is retried only
when a fresh remote-tip probe confirms that the tip moved. Other failures stop
immediately and show bounded Git stderr, including hook or authentication errors.
Accepted commits are never
rewritten, and the tool never forces, merges or rebases. So sender and receiver
sites can interleave publications and receipts freely, and nobody needs to pull
before writing.

Because of that, `process` needs the intake destination to be reachable
(`pending` does not, unless `--reconcile` is supplied). If the receipt cannot be
published after a create or comment, the operation journal stays. Rerun the same
`process` command: it finds the confirmed spec write by its marker and publishes
the receipt without writing again. `aida intent publish` without `--file` pulls and then pushes the accepted
tip, which is a no-op when the remote already has it. Divergence is only possible
when the remote history was rewritten outside AIDA (for example a force push).
`pull` and every write then refuse and leave the accepted branch unchanged.
Review both histories with an advisor.

`list` defaults to pending; `--all` includes processed entries. Pending performs
no spec write and creates no receipt. Dismiss creates a receipt only. Create
explicitly creates a draft task, while comment appends the title and body to an
existing spec. Both require an attached git-canonical home store and use the
ordinary authorized CLI writers. Receipts contain exactly `version: 1`, the full
`checksum`, `action`, `operation` (a UUID), and a destination UUID for create/comment
under `receipts/<sha256>.json`. Receipts are immutable and never enter envelopes.
Processing writes a private durable operation journal before crossing into the
store; the task tag or comment marker records its random operation identity.
Retry recovers a confirmed successful write without repeating it. After a writer
fails to spawn, exits, or is killed on timeout, a fresh canonical scan checks the
operation marker. If absent, the journal is removed and retry is available; writer
stderr is bounded and shown on failure. If evidence cannot be read or is ambiguous,
the journal remains and automatic replay refuses. If a receipt already exists,
retry compares any retained local journal's store evidence with the receipt's
operation UUID, action and destination. A match clears the journal left after a
successful push. A different operation reports the orphaned duplicate write
(including comments on the same target) for operator reconciliation and retains
the journal; it never silently adopts another clone's receipt after a local spec
write.

For a journal left by a crashed parent, first independently confirm that the old
writer and its helpers have stopped. Then run
`aida intent process <checksum> --action pending --reconcile` to check current
store evidence and clear only an unconfirmed journal. `--reconcile` is the explicit
assertion that the old writer has stopped. It also clears an unconfirmed journal
when another clone has already published a receipt, returning that receipt without
a new store write. If a marker exists, use the original create/comment action and
target to recover its receipt instead. Different-store or ambiguous markers refuse.
Never remove journals manually and blindly repeat a store write.

All intake commands honor `--json` and global `--format json`; otherwise they
print human-readable results. Receipt destination UUIDs remain stable in the wire
files. CLI receipt views resolve them to spec IDs when the home store is available,
and keep the UUID when listing from an observation-only clone.
<!-- doc-intent: shaped by TASK-3096 -->

### `aida condition check`

Check a typed condition without changing requirements:

```bash
aida condition check 'spec:<spec-id>=completed'
aida condition check 'pr:2577=merged' --json
aida condition check 'release:>=0.17'
aida condition check 'date:2026-12-01'
aida condition check 'query:tag=operator-held,status=approved count>=5'
aida condition check 'manual:joe'
```

The result is `met`, `not-met`, `unknown`, or `manual`. All four exit zero;
malformed conditions and unreadable or missing spec data exit nonzero.
`unknown` means the PR or release could not be observed and must not trigger
a handoff or resurface a spec. `manual` always waits for the named person.
`--format json` and `--json` emit the same condition, verdict and observation.

Dates use UTC. Releases compare the highest local dotted numeric Git tag,
ignoring prerelease and non-version tags; this does not fetch remote tags.
With no release tags, every release comparison is `not-met`, including `<`
and `<=`; no implicit zero version is assumed.
Queries combine tag, status, type and feature filters with AND and count only
non-archived specs. Comparisons accept `>=`, `>`, `<=`, `<`, and `=`.
Spec and query checks read canonical objects strictly (or an explicit YAML
store selected with the top-level `aida --file` option); archived SQLite stores
must be migrated first.

Deferral writes, conditional relay/seat handoffs, and `findings recur --until`
require this language. The hourly `defer resurface` job evaluates their stored
conditions (see above). Human decisions use `manual:<who>` with a non-empty
reason and retain the same hold policy as `aida defer`.
<!-- trace:STORY-2402 | ai:codex -->


### `aida intent ci` — restricted work CI return channel

<!-- trace:STORY-2480 | ai:codex -->

`intent ci enroll --file <profile.json> --reviewed` enrolls a clone-local CI
profile after live forbidden branch/tag push probes. Home alone creates retained
requests with `intent ci request --attempt <positive-u32> --sha <full-sha>`.
`intent ci poll` performs one bounded outbound work polling pass;
`intent ci process` validates returned receipts at home and publishes the stable
native check `aida/work-ci`. Home assigns attempts, retaining immutable exact-SHA
`refs/aida/ci/<attempt>` refs; work pipeline IDs never grant admission.

Read [setup, server enforcement, supported inventory and crash recovery](../agents/work-ci-relay.md)
before enrollment. Unsupported dynamic job/test inventory refuses. Work receives
no home status/API credential. CI results share the existing isolated intake
transport and lock; there is no separate request registry or automatic rollout.
