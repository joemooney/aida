# Chapter 5 — Planning

Most work doesn't need a plan. You file a spec, queue it, an implementer builds it, you merge. The planning commands exist for the *minority* of work where improvising is a mistake — the non-trivial change where the implementer would otherwise rediscover constraints the hard way, reimplement a helper that already exists, or drift off the spec halfway through.

The shape of the answer is always the same: a **plan file under `docs/plans/`** (`YYYY-MM-DD-<slug>.md`, the 11-section template), authored by an agent off a rich prompt, that the implementer then *rides* instead of guessing. This chapter is the commands that **assemble** that prompt (`ultraplan`), **ground** it in the real tree (`plan scan`, `plan helpers`, `deps sweep`), **lint** it (`plan verify`), **land** an externally-authored one back into convention (`import-plan`, `plan capture`), and **move plan-ahead work through the lifecycle** (`plan promote`, `plan fan-out`). Plus `goal`, the odd one out: turning a spec into a machine-checkable *done condition* for an autonomous run.

> Manual contract reminder: rationale, not flag tables. `aida <cmd> --help` is the source of truth for exact flags and defaults. We cover only the options whose *rationale* isn't obvious from their name.

---

## When to plan vs. just implement

A useful rule of thumb before reaching for any command here:

- **Just implement** — the change is small, touches one file/area, and the spec's acceptance criteria already say everything the implementer needs. Planning would be ceremony. Queueing the spec straight to `aida queue work` is correct.
- **Plan first** — the work spans several files, has a non-obvious sequencing (build order matters), reuses machinery that already exists elsewhere, or is risky enough that you want a reviewable artifact *before* code. This is where a `docs/plans/` file earns its keep: it's a brief the implementer rides and a record reviewers can check intent against.

The anti-pattern at *both* ends: planning trivial work (slop — a plan file nobody reads) and improvising non-trivial work (drift — an implementer halfway down the wrong path). The commands below are tuned to keep you out of both ditches — note how the *workable-set* discipline (drop the low-priority tail) recurs, precisely to avoid speculative plan-slop.

---

### `aida plan`

**One line** — the toolbox for the implementation-plan files under `docs/plans/`.

**Mental model.** `aida plan` is not one action — it's a *family* of plan-file operations, each a subcommand: **verify** (lint a plan), **helpers** (derive a "don't reimplement this" section), **scan** (ground the plan in the current tree), **promote** / **fan-out** (move plan-ahead specs through the lifecycle), and **capture** (reconstruct a plan file from a PR that skipped the local file). The unifying idea: a plan is a *first-class artifact* with the same care as code — linted, grounded, version-controlled, and tied to its spec.

**Reach for it when** — you're authoring, checking, or lifecycle-managing a plan file. Each subcommand has its own entry below; reach for the *subcommand*, not the bare `aida plan`.

**Don't reach for it when** — you want to *generate* the plan's prose. That's not `aida plan` — it's `aida ultraplan` (assemble the prompt) handed to the `/ultraplan` agent. `aida plan` operates *on* plan files; it doesn't write their narrative for you.

**Gotchas.** A plan file is recognized as "belonging to" a spec by its `Specs:` header line listing the SPEC-ID (that's how `promote` / `fan-out` find a spec's plan). If `promote` says a spec has no plan, check that header line first — the file existing isn't enough.

**Chains with** — `aida ultraplan` (generate) → save under `docs/plans/` → `aida plan verify` (lint) → `aida plan promote` (Approved → Planned) → `aida queue work` rides the plan.

---

### `aida plan verify`

**One line** — lint a plan file against the structured template.

**Mental model.** Plans rot the same way docs rot: a `path:line` ref drifts the moment someone edits the file it points at, a required section gets dropped, a referenced file gets renamed. `plan verify` is the **drift-guard for plan files** — it reports drifted refs (with the corrected line number), missing files, and absent required sections, and **exits non-zero on any failure** so it can run as a pre-commit hook on `docs/plans/`.

**Reach for it when** — before committing a plan, or in a pre-commit hook over `docs/plans/`. Any time a plan is more than a few hours old and you're about to rely on it — line refs go stale *fast*.

**Don't reach for it when** — you want the verifier to silently auto-correct everything: it reports by default and only rewrites refs when you ask it to. Reading the report first is usually the point (a drifted ref can mean the *code* moved, which is worth knowing).

**Key options (rationale only).**
- `--fix` — rewrite drifted `path:line` refs in place (to the corrected line, or to symbol form where a symbol is named). The reason it's opt-in: you usually want to *see* the drift before erasing it. The deeper fix is to cite symbols, not lines, in the first place — symbol refs survive edits.
- `--quiet` — suppress the per-check OK lines, leaving only warnings/errors and the verdict. Exists specifically so the pre-commit-hook output isn't noise.

**Gotchas.** It checks structure and refs, not *correctness* — a plan can verify clean and still be a bad plan. Verify guards against drift, not against being wrong.

**Chains with** — runs after a plan is authored/imported, and as a hook before any plan commit.

---

### `aida plan helpers`

**One line** — derive a "what already exists, don't reinvent it" section from the trace graph.

**Mental model.** The most expensive implementer mistake on a multi-spec codebase is *re-implementing a helper that a sibling spec already built*. `plan helpers` walks the trace graph — sibling specs (same parent), same-feature specs, tag-sharing specs — harvests their `trace:` annotations, and reports the files and helpers they already touch. It turns the implicit "someone probably already wrote this" into an explicit, citable `## Reusable helpers` section.

**Reach for it when** — assembling a plan for work that sits near existing machinery (a new command in a family, a variant of an existing flow). The denser the local graph, the more this saves.

**Don't reach for it when** — the work is genuinely greenfield with no sibling/feature/tag neighbors — the output will be thin, and that's fine; just skip it.

**Key options (rationale only).**
- `--append <file>` — append the generated section straight into a plan file instead of printing it. The natural move when you're building the plan up section by section.

**Gotchas.** The signal quality depends on trace-comment hygiene in the neighboring specs — if siblings never left `trace:` breadcrumbs, there's nothing to harvest. It's a reflection of the graph's discipline, not a code-wide search.

**Chains with** — folded into `aida ultraplan`'s output automatically; run standalone when you want just this section, or to refresh it on an existing plan.

---

### `aida plan scan`

**One line** — a read-only, pre-plan snapshot of the code the work will actually touch.

**Mental model.** Specs are written against a mental model of the codebase that may already be stale by the time anyone plans the work. `plan scan` grounds the plan in *current reality*: the files and symbols related specs already trace to (the real, current APIs and architectural constraints) **plus** a list of likely-stale assumptions — code paths the spec text names that no longer exist in the tree. It's the "is what this spec assumes still true?" check, run *before* a single line of plan prose.

**Reach for it when** — before generating a plan for any non-trivial spec, especially an older one — and especially before importing an externally-authored artifact (Spec Kit / OpenSpec). The scan summary is grounding context you hand to whatever generates the plan.

**Don't reach for it when** — you want it to *change* anything. It's read-only by default; the lone write is opt-in (attaching the summary as provenance). If you just want to eyeball the picture, run it bare and read stdout.

**Key options (rationale only).**
- `--attach` — the *only* write the command performs: pin the scan summary to the spec as a provenance comment, so the "what the tree looked like at plan time" record travels with the work. Opt-in precisely because everything else is read-only.
- `--append <path>` — drop the summary into a plan file as a `## Pre-plan scan` section, so the grounding lives inside the plan rather than in a comment.
- `--json` — emit the scan for piping into an external plan/spec generator (the Spec-Kit / OpenSpec compose path).

**Gotchas.** The "likely-stale assumptions" list is heuristic (it matches code-path-shaped strings in the spec text against the tree) — treat it as a prompt to look, not a proof. A false positive there is cheaper than the missed drift it's hunting.

**Chains with** — `plan scan` (ground) → hand the summary to `ultraplan` / Spec Kit / OpenSpec → `--attach` or `--append` so the provenance sticks → author the plan.

---

### `aida plan promote`

**One line** — move an Approved spec to Planned once a plan file exists for it.

**Mental model.** AIDA's lifecycle has a real step between *Approved* (blessed to build) and *In Progress* (someone's building it): **Planned** — "the thinking is done, this is ready to pick up." `plan promote` formalizes that step. It bumps an Approved spec to Planned *when a plan file under `docs/plans/` lists its SPEC-ID in the `Specs:` header* — making plan-ahead work (plan now, implement later — the parallel-pipelining workflow) visible in the queue as genuinely ready.

**Reach for it when** — you (or a planning fan-out) have written plan files ahead of implementation and want the queue to reflect which specs are now plan-ready. The single-spec form for one; `--all` to sweep the backlog.

**Don't reach for it when** — there's no plan file yet (promote refuses — Planned means *planned*, and the file is the evidence); or the spec isn't Approved (it only promotes from Approved).

**Key options (rationale only).**
- `--all` — sweep every Approved spec that has a matching plan file in one pass. The batch counterpart to naming one spec.
- `--dry-run` — report what *would* promote without writing. The safe pre-flight before an `--all` sweep, so you see which specs the plan-file matcher found.

**Gotchas.** The match is on the plan file's `Specs:` header line, not the filename — a plan whose header doesn't list the SPEC-ID won't promote it, however suggestive the filename. This is the same matcher `fan-out` uses.

**Chains with** — `aida ultraplan` / author plan → `aida plan promote` (Approved → Planned) → `aida queue work` picks up a Planned spec riding its plan. The fan-out version of this is `aida plan fan-out`.

---

### `aida plan fan-out`

**One line** — run the plan step across a *set* of Approved specs and promote each as its plan lands.

**Mental model.** `plan fan-out` is `plan promote` scaled to a whole batch, with the plan-*authoring* step included: pick a set of Approved specs, run the plan step for each (sequentially), and promote each Approved → Planned once its plan file exists. It's the **plan-ahead engine** for parallel pipelining — get a batch's plans written and the specs lifecycle-bumped to Planned, so a later drain has a deep ready queue to pull from. Promotion here is *contention-free pre-work* — it never merges anything.

**Reach for it when** — you're front-loading planning for a batch (`--batch NAME`), an epic's children (`--epic ID`), or an explicit list, before kicking off an implementation run. The classic "spend the evening planning so tomorrow's drain has work queued" move.

**Don't reach for it when** — you want *true parallel* plan sessions — fan-out drives them sequentially (true parallelism is the harness's job; this is the driver that hands each spec to a plan session in turn). And don't expect it to plan the low-priority tail by default — the workable-set discipline drops it on purpose (to avoid plan-slop on specs that may never be built).

**Key options (rationale only).**
- `--batch` / `--epic` / explicit SPECS — the three *mutually exclusive* selection modes; pick exactly one. They mirror the batch/epic/explicit selection the rest of AIDA uses.
- `--include-low` — opt the low-priority tail back into the set. Off by default *because* speculative plans on never-built specs are the slop this command is designed to avoid.
- `--promote-only` — skip launching plan sessions and only run the Approved → Planned promotion for specs that already have a plan file. The reconcile pass after a harness fanned the actual plan sessions out itself.
- `--dry-run` — resolve and print the set without launching or promoting anything. Always worth running first on a `--batch`/`--epic` selection to confirm the set is what you meant.

**Gotchas.** "Drops the low-priority tail" can surprise you into thinking a spec was missed — it wasn't selected. Use `--dry-run` to see the resolved set, and `--include-low` if you genuinely want the tail.

**Chains with** — selection (`--batch`/`--epic`) → `plan fan-out` (author + promote) → a later `aida queue work` / `aida burndown` drains the now-Planned set. `--promote-only` is the tail end when the harness did the authoring.

---

### `aida plan capture`

**One line** — reconstruct a `docs/plans/` file from a PR that skipped the local plan file.

**Mental model.** Some plans get authored in the web `/ultraplan` flow and land a PR *directly*, never writing a local plan file. `plan capture` reconciles those back into AIDA's plan-archival convention: it reads the PR's title, body, commit log, and changed files (`gh pr view` + `gh pr diff`), fills the 11-section template, and writes `docs/plans/<date>-<slug>-from-pr-<N>.md`. It's the "I shipped without the file, now make the file" cleanup — idempotent, and shaped to pass `aida plan verify`.

**Reach for it when** — a PR shipped from a web-authored plan and you want the plan archived under `docs/plans/` like every other plan (so the convention holds and the history is complete).

**Don't reach for it when** — you authored the plan locally already (you have the file — capture would just regenerate a thinner version from the PR); or the work was small enough to never warrant a plan (don't manufacture a plan file for a one-line fix just to satisfy the convention).

**Key options (rationale only).**
- `--stdout` — print the synthesized plan instead of writing the file. For reviewing what it reconstructed before committing it.

**Gotchas.** It needs `gh` on PATH and authenticated — the whole input is the PR via `gh`. And it synthesizes *from the PR*, so the reconstructed plan is only as rich as the PR description and commit log; a terse PR yields a terse plan.

**Chains with** — runs after a web-authored PR merges → `plan capture <PR>` → `aida plan verify` confirms it (it's shaped to pass).

---

### `aida ultraplan`

**One line** — assemble a rich, fully-contextualized planning prompt for a spec.

**Mental model.** This is the command that *makes the plan worth writing*. A terse spec ("add retry to the pull command") becomes a bad plan because the planning agent lacks context. `ultraplan` gathers everything that context lives in — the spec's description, its `## Acceptance` criteria, related-spec context, the spec's enrichment comments, the AIDA 11-section plan-template structure, and the trace-graph reusable helpers — into **one prompt** you hand to the `/ultraplan` agent. It turns a terse ask into a fully-contextualized brief, and copies it to the clipboard by default (paste straight into the agent).

**Reach for it when** — you're about to plan any non-trivial spec. This is *step one* of planning: assemble the prompt, hand it to `/ultraplan`, save the result under `docs/plans/`.

**Don't reach for it when** — the work doesn't need a plan at all (see "When to plan vs. just implement" — don't manufacture a planning ceremony for a trivial change); or you want to operate on an *existing* plan file (that's the `aida plan` subcommands).

**Key options (rationale only).**
- `--stdout` — print the prompt instead of copying it. Also the automatic headless fallback when no clipboard tool is available — so it's the safe choice in scripts and over SSH.
- `--json` — emit the prompt plus metadata (warnings, token estimate) for scripting. The token estimate matters: a huge prompt is a signal the spec is too big and wants decomposing first.
- `--no-comments` — drop the `## Comments` section. The spec's enrichment comments are pulled in by default (they're often where the real design lives); omit them only when you want a leaner, comment-free brief.

**Gotchas.** Default behavior is *clipboard*, which silently no-ops in a headless context with no clipboard tool — but it falls back to stdout there, so you're not left empty-handed. If you're scripting, just use `--stdout` / `--json` and don't rely on the clipboard.

**Chains with** — `aida ultraplan <spec>` → paste into `/ultraplan` → save the output → `aida import-plan` (or save under `docs/plans/` directly) → `aida plan verify` → `aida plan promote`.

---

### `aida import-plan`

**One line** — land a saved plan file into AIDA conventions and pin it to its spec.

**Mental model.** `/ultraplan` (or any external generator) hands you a plan as a loose markdown file. `import-plan` is the **landing pad**: it archives the file under `docs/plans/YYYY-MM-DD-<slug>.md` and pins it to its SPEC with a comment, so the plan is discoverable from the spec and lives in the standard place. It's the round-trip partner to `ultraplan` — `ultraplan` sends the prompt out, `import-plan` brings the answer back in.

**Reach for it when** — `/ultraplan` (or a web flow) gave you a plan file and you want it archived + linked the right way without hand-placing it. This is the canonical "save plan to file" → "into AIDA" step.

**Don't reach for it when** — the plan was authored *directly into a PR* with no file (that's `aida plan capture`, which reconstructs the file from the PR instead). `import-plan` expects an actual file in hand.

**Key options (rationale only).**
- `--spec <SPEC-ID>` — name the spec this plan targets. If omitted, it tries to detect it from the filename (a `TYPE-N` pattern like `task-42`). Pass it explicitly when the filename doesn't carry the ID — guessing wrong mispins the plan.
- `--request-review` — land the plan as **awaiting master review** rather than canonical: it tags the spec `plan-review:pending` and posts a "plan landed for master review" comment, and `aida queue work <SPEC>` then *warns before pickup*. The mechanism behind sketch-first sign-off — the plan exists but isn't yet treated as the blessed brief until a human signs off.

**Gotchas.** Without `--request-review` the plan is treated as canonical immediately — pickup rides it with no gate. On a multi-agent project where architecture-class plans want master sign-off *before* implementation, reach for `--request-review` deliberately; the warning-on-pickup is the whole point.

**Chains with** — `aida ultraplan` → save file → `aida import-plan <file>` → (optionally `--request-review` → master signs off) → `aida plan promote` → `aida queue work`.

---

### `aida goal`

**One line** — derive a machine-checkable completion condition from spec metadata, ready to paste into `/goal`.

**Mental model.** An autonomous run needs a *deterministic* stop condition, not a vague "make the tests pass" that loops forever. `aida goal` reads AIDA metadata and emits a completion condition where **every clause carries an explicit verification command** a small evaluator can check deterministically. Each flag contributes one clause; multiple flags **compose with AND**. It's the bridge between "a batch of specs" and "a `/goal` (or `/schedule`) an agent can run to provable completion."

**Reach for it when** — you're about to kick off an autonomous `/goal` or scheduled run and need its done-condition expressed as something checkable: a batch reaching terminal status, an epic's children all closed, a specific spec Completed, a PR merged, a role's queue empty.

**Don't reach for it when** — you want to *run* the work (that's `aida queue work` / `aida burndown`); `goal` only produces the *condition*. And it lives slightly apart from the plan-file family — it's planning in the sense of "defining the target," not "writing the implementation brief."

**Key options (rationale only).**
- `--batch` / `--epic` / `--spec` / `--pr` / `--queue-empty` — the five clause sources; each adds one AND-composed condition. Compose them to express "all of this batch terminal *and* the reviewer queue empty." Note `--queue-empty` takes a *role* — pick a mechanism whose clause routes handoffs as you intend (e.g. an autonomous-merge path may skip the reviewer queue, so don't gate on a queue that never fills).
- `--copy` — copy the assembled `/goal` line to the clipboard, the common interactive path.
- `--invoke` — print only the bare `/goal <condition>` line with no framing, for command-substitution / scripting.
- `--as-deep-link` — emit a `claude-cli://open` deep link with the `/goal` line pre-filled; clicking opens Claude Code in the cwd with the prompt staged (inert until Enter). Requires a recent Claude Code.

**Gotchas.** The clauses are only as honest as the routing they describe — gating on a queue or PR that a chosen drain mode bypasses produces a condition that's instantly (and misleadingly) "met." Pick clause mechanisms that match how the work will actually be driven.

**Chains with** — `aida goal --batch X --copy` → paste into `/goal` (or `/schedule`) → an autonomous run drives the batch to the derived condition. Pairs naturally with `aida burndown` (Ch.3) for the actual draining.

---

### `aida deps sweep`

**One line** — list *likely* dependencies inferred read-only from the trace graph.

**Mental model.** Before an overnight drain, the failure you most want to catch is a *missed dependency* — two specs that touch the same files, where building B before A means rework. `deps sweep` infers likely edges from the trace graph (specs sharing trace-link files, ranked: ≥2 shared files = high, 1 = medium; same-parent siblings, weaker) and **lists them without writing anything**. It's a "did I miss a dependency?" check, not an edge-writer — confirmed edges you add by hand.

**Reach for it when** — before a `--batch` / `--epic` autonomous drain, to sanity-check that the specs you're about to fan out don't have undeclared ordering between them. The pre-drain dependency audit.

**Don't reach for it when** — you want the dependency *written*: sweep is strictly read-only by design (it never writes edges — inferred-from-traces guesses shouldn't auto-become hard `BlockedBy` edges). Confirm a real one with `aida edit <id> --blocked-by <dep>`.

**Key options (rationale only).**
- `--for-spec <spec>` — limit the sweep to one source spec instead of the whole store. The targeted "what might *this* spec depend on?" before queueing just it.
- `--json` — emit for agents/scripts (e.g. a pre-drain gate that flags high-confidence shared-file pairs).

**Gotchas.** It's *heuristic and read-only* on purpose — high confidence (≥2 shared files) is a strong hint, not a fact, and same-parent is weak signal. Don't treat its output as a dependency graph; treat it as a list to confirm or dismiss by hand.

**Chains with** — `aida deps sweep` → eyeball → `aida edit <id> --blocked-by <dep>` for the real ones → then a clean `aida burndown` / `aida plan fan-out` over a batch with its ordering declared.

---

## Where to go next

You now have the planning layer: assemble a context-rich prompt, ground it in the real tree, lint the result, land it into convention, and move plan-ahead specs through the lifecycle — plus the `goal`/`deps` tooling that frames and de-risks an autonomous run. The threads back into the spine:

- **[Chapter 3 — Work & autonomy](03-work-autonomy.md)**: where a Planned spec gets picked up — `queue work` rides the plan, and `burndown` drains the set `plan fan-out` + `goal` prepared.
- **[Chapter 2 — Specs](02-specs.md)**: the `trace` and `graph` machinery that `plan helpers` / `plan scan` / `deps sweep` all read from — the better your trace hygiene, the better they perform.
- **[Chapter 4 — Git & lifecycle](04-git-lifecycle.md)**: the Approved → Planned step `plan promote` formalizes, in the context of the full lifecycle.
- The plan-file template itself lives at `docs/plans/_TEMPLATE.md`; the full lifecycle is [`docs/lifecycle.md`](../lifecycle.md).
