# Plan: /aida-backlog-groom skill + `aida backlog` CLI (STORY-444)

Date: 2026-05-23
Specs: STORY-444
Status: Draft
Complexity: medium — new CLI subtree + heuristic engine + skill scaffolding

## Context

Operators currently triage Approved-but-not-yet-queued work by running
`aida list --status approved` and eyeballing what looks cheap and
isolated enough to throw onto the queue. For a healthy backlog this is
fine; once the Approved pile grows past ~10 items, the manual scan
misses good batch candidates (low-risk, non-overlapping specs that
could safely run in parallel) and overloads single sessions with
high-risk picks. STORY-444 makes that judgment a first-class verb:
`aida backlog list/groom/analyze` plus a `/aida-backlog-groom` skill
that wraps the verbs into an interactive triage flow.

The defensible value is the **risk + conflict heuristics**: backlog
items already have type, priority, tags, and a trace graph — a
purpose-built command can mechanically classify them faster than a
human scan, surfacing safe-to-parallelize pairs that an operator would
otherwise miss.

## Approach

Add a new top-level `Command::Backlog(BacklogCommand)` enum in
`aida-cli/src/cli.rs` with three subcommands (`list`, `groom`,
`analyze`) modeled on the existing `QueueCommand` shape. Implement
`handle_backlog_command` in a new `aida-cli/src/backlog.rs` module — the
risk/conflict logic is self-contained enough to deserve its own file
(rather than another 600 lines bolted onto the 65k-line `main.rs`),
with cross-cutting helpers (`scan_trace_graph`, `find_plan_files_for_spec`,
tag helpers) called via `pub(crate)` from `main.rs`.

`groom` defers actual queue insertion to existing helpers: it builds a
list of selected ids and a batch name, then calls the same code path
`QueueCommand::Add` already exercises (`storage.queue_add(entry)`),
followed by `aida edit --add-tag batch:NAME` on each spec via the
in-process tag mutator. `--dry-run` short-circuits before any write.

Interactive selection on the **CLI side** is not in scope for v1 —
the CLI accepts a comma-separated `--specs` list or reads ids from
stdin (`--from-stdin`), and the **skill** is the surface that drives
the interactive pick-list via Claude's `AskUserQuestion`. This split
matches the pattern used by `aida-decompose` (CLI does the filing,
skill drives the conversation).

```
┌──────────────────────────────────────────────────────────────────────┐
│           User: /aida-backlog-groom [--batch low-risk-cleanup]       │
└─────────────────────────────┬────────────────────────────────────────┘
                              │
        ┌─────────────────────▼──────────────────────┐
        │  aida backlog list --json --risk low       │
        │  (approved + non-queued + risk classified) │
        └─────────────────────┬──────────────────────┘
                              │
        ┌─────────────────────▼──────────────────────┐
        │  aida backlog analyze --specs <csv>        │
        │  (pairwise file-overlap from trace graph + │
        │   plan files → safe-parallel vs serialize) │
        └─────────────────────┬──────────────────────┘
                              │
        ┌─────────────────────▼──────────────────────┐
        │  AskUserQuestion: select items to enqueue  │
        │  (skill surface; CLI takes --specs)        │
        └─────────────────────┬──────────────────────┘
                              │
        ┌─────────────────────▼──────────────────────┐
        │  aida backlog groom --specs <csv>          │
        │    --batch NAME                            │
        │  → for each id:                            │
        │     - storage.queue_add(entry)             │
        │     - edit_tags_add(id, "batch:NAME")      │
        └────────────────────────────────────────────┘
```

## Decisions

- **New `backlog.rs` module, not more `main.rs`.** `main.rs` is at
  65k lines. The risk/conflict engine and its tests cluster well as a
  unit. **Rationale**: matches the carve-out pattern set by
  `pr_rebase.rs` / `drain_state.rs` / `digest.rs`. Cross-cutting helpers
  (`scan_trace_graph`, `find_plan_files_for_spec`, `batch_tag_of`,
  `format_tag_chip`) are made `pub(crate)` in `main.rs` and called from
  `backlog.rs`.

- **"Backlog" = Approved + not currently in any user's queue + not
  archived/terminal.** Drafts and Planned items are excluded —
  Approved is the lifecycle state that the existing `aida add
  --status approved` defaults to and what the requirement text calls
  out. **Rationale**: matches the requirement's plain reading ("Approved
  items not yet queued").

- **Risk levels are advisory chips, not gates.** Computed by a single
  `classify_risk(req) -> RiskLevel` function (Low / Medium / High /
  Unknown). The `groom` flow never refuses based on risk; it only
  surfaces the chip alongside each candidate. **Rationale**: the
  requirement explicitly says "Heuristics are HINTS, not gates.
  Operator can override during groom."

- **Risk heuristics** (matches the requirement text, implemented in
  `classify_risk`):
  - **Low**: type ∈ {task, doc} AND priority == low AND tags ∩
    {papercut, cosmetic, severity:cosmetic, lifecycle:trivial,
    docs-only, fmt} is non-empty.
  - **Medium**: type ∈ {task, bug} AND priority == medium, OR a
    `docs/plans/<date>-<slug>.md` file owns the spec
    (`find_plan_files_for_spec` returns non-empty).
  - **High**: type ∈ {story, epic, spike} OR priority == high OR the
    spec has a `BlockedBy` relationship OR has a parent relationship
    (`Child` rel to another req).
  - **Unknown** (fallthrough): anything else.

- **Conflict detection runs over candidate ids only, not the whole
  backlog.** Pairwise `O(n²)` over the file-overlap sets is fine when n
  is the groom selection (typically ≤ 30). **Rationale**: keeps the
  command snappy without needing a cross-spec inverted index.

- **File-overlap sources** (in `collect_spec_files`):
  1. `scan_trace_graph(project_root, {spec_id})` → all source files
     whose trace comments reference the spec.
  2. `find_plan_files_for_spec(project_root, spec_id)` → plan files
     that own the spec; for each plan, parse its `## Critical Files`
     section via the existing `parse_plan_critical_files` to harvest
     touched paths.
  3. **(deferred)** `git log --grep "(SPEC-ID)"` for committed code —
     not in v1; documented as a Followup. **Rationale**: the trace
     graph + plan files cover the common case (in-flight + planned
     work) and the git-log scan adds shell-out cost.

- **Overlap classification per pair**: if file sets intersect non-empty
  → `Serialize` with the overlapping path. Else → `SafeParallel`. If
  *either* spec contributed zero files (no trace comments, no plan) →
  `Unknown` (we cannot tell, and the operator should treat it as
  serialize-by-default).

- **`--batch NAME` tags via the existing `--add-tag` semantics.** Reuse
  the `Command::Edit { add_tag: Vec<String>, .. }` codepath rather
  than building a new mutator. The handler computes one queue entry +
  one add-tag op per selected id, in a single transaction-ish loop
  with a single git commit at the end (the cache write-through is the
  same one `queue add` already performs).

- **JSON output as a first-class mode.** `--json` on all three verbs
  emits a stable shape so the skill can parse it. The skill consumes
  JSON; humans get the colored table by default. **Rationale**: the
  skill→CLI handoff is the same shape as `aida ultraplan --json` from
  TASK-113.

- **Docs land in `docs/aida-discipline/`, not `docs/aida/`.** The
  requirement mentions "docs/aida/ (per Option F naming)" but no
  "Option F" decision artifact exists in the repo. `docs/aida/` is the
  auto-generated requirements projection (regenerated by `aida docs
  build`). `docs/aida-discipline/` is the existing scaffolding-pack
  channel for operator-facing workflow docs and matches the
  `propagation: scaffolding-pack` convention. **Rationale**:
  conventional location for "how to use AIDA" prose; a future
  `docs/aida/` rename can mass-move discipline docs in one pass.

## Files (in build-order)

### 1. `aida-cli/src/main.rs`
- Promote four helpers to `pub(crate)` so `backlog.rs` can call them
  without duplication:
  - `fn scan_trace_graph`
  - `fn find_plan_files_for_spec`
  - `fn parse_plan_critical_files`
  - `fn batch_tag_of`, `fn tag_matches_exact`,
    `fn tag_matches_prefix`, `fn format_tag_chip`,
    `fn normalize_batch_name`
  - `fn is_terminal_status`, `fn get_user`, `fn find_project_root`,
    `fn load_store_for_lookup` are already `pub(crate)` or callable.
- Add `mod backlog;` near the other `mod` declarations.
- Wire `Command::Backlog(cmd) => backlog::handle_backlog_command(cmd,
  &storage)?;` into both Command-match arms (the two dispatch sites —
  legacy SQLite branch + git-backend branch — both need the new arm).

### 2. `aida-cli/src/cli.rs`
- Add `pub enum BacklogCommand` with three subcommand variants
  (`List`, `Groom`, `Analyze`) in the same style as `QueueCommand`.
  Flag set:
  - `List`: `--risk <low|medium|high|unknown>`, `--type <ty>`,
    `--tag <name>`, `--tag-prefix <pfx>`, `--priority <pri>`,
    `--limit <N>` (default 50), `--json`.
  - `Groom`: `--specs <CSV>` (required when `--from-stdin` absent),
    `--from-stdin`, `--batch <NAME>` (optional; when set, every
    groomed id gets `batch:NAME` tag), `--dry-run`, `--note <STR>`,
    `--user <id>`.
  - `Analyze`: `--specs <CSV>` (required; ≥ 2 ids), `--json`. Optional
    `--pair SPEC SPEC` shorthand (sets `--specs` to that pair).
- Insert one variant in `pub enum Command`:
  `/// Curate Approved-not-queued work into the queue with risk +
  conflict heuristics. See `aida backlog --help`.
  Backlog(BacklogCommand),`
  Doc comment must avoid SPEC-ID leakage per TASK-268 (the trace
  marker goes on a `// ` line above the variant, not inside the `///`
  doc).

### 3. `aida-cli/src/backlog.rs` (NEW)
- Module header with one-line summary referencing STORY-444 in a
  `// trace:STORY-444` comment (plain `//`, not `///`).
- Public entry point:
  ```rust
  pub(crate) fn handle_backlog_command(
      cmd: &BacklogCommand,
      storage: &Storage,
  ) -> Result<()>
  ```
  Dispatch on the three subvariants.
- Internal:
  - `enum RiskLevel { Low, Medium, High, Unknown }` with a
    `Display`-style `chip()` that picks a colored label.
  - `fn classify_risk(req: &Requirement) -> RiskLevel` —
    implements the heuristics above.
  - `fn collect_backlog_candidates(store: &RequirementsStore,
        all_queue_entries: &[QueueEntry]) -> Vec<&Requirement>` —
    filters to status==Approved AND requirement_id is not in any
    queue entry AND req is not human-only / archived. Sorted by
    `(priority desc, created_at asc)`.
  - `fn collect_spec_files(project_root: &Path, spec_id: &str)
        -> BTreeSet<String>` — unions trace-comment files +
    plan `## Critical Files` paths.
  - `fn classify_pair_overlap(a_files: &BTreeSet<String>,
        b_files: &BTreeSet<String>) -> PairVerdict` — returns
    `Serialize { shared: Vec<String> }` / `SafeParallel` / `Unknown`.
  - `fn render_list_table(...)`, `fn render_list_json(...)`,
    `fn render_analyze_text(...)`, `fn render_analyze_json(...)`.
  - Tests at the bottom of the file (see Tests section).

- `Groom` impl:
  - Resolve `--specs` (parse CSV) or read newline-separated ids from
    stdin (`--from-stdin`).
  - Validate each: must exist, must be Approved, must not already be
    queued. Bail with the offending id when invalid.
  - When `--dry-run`: print a preview table (id, title, risk chip,
    "would be queued" / "would be tagged batch:NAME") and return.
  - Else: for each id, build a `QueueEntry` (the same shape
    `QueueCommand::Add` builds, with i64::MAX position sentinel so
    the backend appends), call `storage.queue_add(entry)`, then
    apply `batch:NAME` tag via the same mutation `Command::Edit
    { add_tag }` uses (the `add_tag_to_requirement` helper, factored
    out of `main.rs` if currently inline — see step below).
  - Print a one-line summary per id and a tail summary
    (`✓ Groomed N items into the queue` + optional
    `[batch:NAME — drain with \`aida queue work --batch NAME\`]`).

### 4. `aida-cli/src/main.rs` (helper factoring)
- If the `--add-tag` logic in `Command::Edit` is inline, extract a
  small `pub(crate) fn add_tags_to_requirement(storage: &Storage,
  req_id: &Uuid, tags: &[String]) -> Result<()>` helper. If it is
  already a callable mutator, just import it. (Verify by reading
  the `Edit { ... add_tag, ... }` arm before deciding.)

### 5. `aida-core/templates/skills/aida-backlog-groom.md` (NEW)
- Frontmatter: `name: aida-backlog-groom`,
  `description: Curate Approved-but-not-queued work into the queue,
  surfacing risk chips and pair-overlap so low-risk non-conflicting
  items can drain in parallel.`,
  `allowed-tools: [Bash, Read, Grep, Glob]`.
- Body sections: Purpose, When to use, Workflow:
  1. Run `aida backlog list --json` (filter knobs as args).
  2. For each candidate, summarize: id, type/priority, risk chip,
     tag list.
  3. Run `aida backlog analyze --specs <ids> --json` over the top
     ~10 low/medium-risk candidates.
  4. Render an AskUserQuestion multi-select listing each candidate
     with its risk + conflict tag (`safe-parallel`, `serialize: shares
     <file>`, `unknown`).
  5. Once selected, call `aida backlog groom --specs <csv> [--batch
     NAME]` and print the resulting queue snapshot
     (`aida queue list --batch NAME`).
- Two traps (mirror aida-drain-queue's style): (a) every command in
  the skill must be a real shell line; (b) risk chips are advisory —
  the user picks, not the heuristic.

### 6. `aida-core/templates/commands/aida-backlog-groom.md` (NEW)
- Slash-command wrapper following the existing 3-paragraph pattern
  (one-line purpose, instructions list pointing at the skill, optional
  `ARGUMENTS: $ARGUMENTS` tail). Mirror `aida-decompose.md`'s
  structure.

### 7. `aida-core/src/scaffolding/mod.rs`
- Add `pub include_aida_backlog_groom_skill: bool` to `ScaffoldConfig`
  (next to `include_aida_digest_skill`); default `true` in the
  `Default` impl.
- Add the per-skill emit block (modeled on the `aida-digest` block)
  under the Claude skills section.
- Add the entry in `codex_skill_defs` and in the `command_defs` array.
- Add `fn generate_aida_backlog_groom_skill(&self) -> String`
  (modeled on `generate_aida_digest_skill`).

### 8. `Makefile`
- No edit needed — `make sync-templates` iterates
  `aida-core/templates/skills/*.md` and
  `aida-core/templates/commands/*.md` automatically; the new files are
  picked up on the next run. Document in plan that the user must run
  `make sync-templates` after pulling.

### 9. `.claude/skills/aida-backlog-groom.md` and
       `.claude/commands/aida-backlog-groom.md`
- Add by running `make sync-templates` (creates the per-file symlinks
  into `aida-core/templates/`). Don't hand-author either side — the
  templates are the source.

### 10. `docs/aida-discipline/backlog-grooming.md` (NEW)
- Add `propagation: scaffolding-pack` frontmatter so it joins the
  starter memory pack on the next build (per CLAUDE.md's "Starter
  discipline pack" section).
- Sections: what "backlog" means in AIDA (Approved-not-queued); how to
  run `/aida-backlog-groom`; how the risk + overlap heuristics work +
  what they explicitly do NOT promise; how `batch:NAME` composes with
  `aida queue work --batch NAME`; out-of-scope items (no
  auto-grooming, no cross-machine).

## Critical Files

- `aida-cli/src/cli.rs`
- `aida-cli/src/main.rs`
- `aida-cli/src/backlog.rs` (new)
- `aida-core/src/scaffolding/mod.rs`
- `aida-core/templates/skills/aida-backlog-groom.md` (new)
- `aida-core/templates/commands/aida-backlog-groom.md` (new)
- `docs/aida-discipline/backlog-grooming.md` (new)
- `.claude/skills/aida-backlog-groom.md` (symlink, created via `make sync-templates`)
- `.claude/commands/aida-backlog-groom.md` (symlink, created via `make sync-templates`)

## Reusable helpers (do not reimplement)

- `scan_trace_graph` (`aida-cli/src/main.rs`) — code → spec trace
  references. Already returns `HashMap<String, Vec<TraceHit>>` keyed
  by spec id, exactly the shape needed for file-overlap.
- `find_plan_files_for_spec` (`aida-cli/src/main.rs`) — `docs/plans/`
  files that own a spec id. Use for the plan-side of the overlap
  scan.
- `parse_plan_critical_files` (`aida-cli/src/main.rs`) — extracts the
  backtick-quoted paths from a plan's `## Critical Files` section.
  Use to harvest the paths each plan owns.
- `batch_tag_of`, `tag_matches_exact`, `tag_matches_prefix`,
  `format_tag_chip`, `normalize_batch_name`
  (`aida-cli/src/main.rs`) — TASK-238/TASK-270 tag helpers. Reuse for
  the `--tag` / `--tag-prefix` / `--batch` flags so semantics match
  `aida queue list`.
- `is_terminal_status` (`aida-cli/src/main.rs`) — keeps the backlog
  filter consistent with everywhere else (excludes Completed/Rejected).
- `derive_parent_epic_label` (`aida-cli/src/main.rs`) — used for the
  `analyze` text output to group pairs by their parent EPIC.
- `storage.queue_add(entry)` (the `Storage` trait method in
  `aida-core/src/db/git_backend.rs`) — the actual queue-write.
  Reuse the same `QueueEntry` construction `QueueCommand::Add` uses
  in `main.rs`.
- `get_user(user_arg)` (`aida-cli/src/main.rs`) — resolves the
  shell-user identity per the BUG-89 convention. Use for the
  groom's queue inserts.
- `Command::Edit { add_tag, .. }` codepath
  (`aida-cli/src/main.rs`) — extract or call its tag-add helper for
  applying the `batch:NAME` tag in one place; do **not** hand-edit
  the requirement's `tags` field.
- Embedded-templates pattern (`aida-core/build.rs`,
  `aida-core/src/scaffolding/mod.rs`) — new skill/command files are
  picked up automatically by `embed_directory("templates/skills",
  ...)`. No build-script edit needed.

## Risks + gotchas

1. **Helper visibility drift.** Promoting four `fn`s to `pub(crate)`
   touches a heavily-rebuilt file (main.rs). **Mitigation**: keep
   each promotion a single-line `pub(crate)` change; do not move
   the function bodies. Run `cargo check -p aida-cli` after step 1
   before adding the module.

2. **`Command::Backlog` requires updating BOTH dispatch sites in
   main.rs.** There are two `Command::Queue(queue_cmd) =>` arms (one
   for legacy SQLite, one for the git-backend path). Forgetting the
   second causes "unimplemented" at runtime in any project on the
   modern backend. **Mitigation**: add both arms in the same commit;
   `cargo check` will not catch a missing match arm because `Command`
   is non_exhaustive-ish via `Subcommand`, so the dispatcher just
   falls through. Verification step below runs the command against
   both paths.

3. **JSON output stability.** The skill consumes the JSON shape, so a
   later field rename would break it silently. **Mitigation**:
   define one `BacklogListRow` / `AnalyzeReport` `#[derive(Serialize)]`
   struct in `backlog.rs` and unit-test the JSON shape with
   `serde_json::to_value` + an assertion on the field names.

4. **Pair-overlap false negatives** when neither spec has trace
   comments or a plan file. The classifier returns `Unknown` for these
   — but a reader might read "Unknown" as "safe". **Mitigation**:
   render `Unknown` with a `?` glyph and a footer line explaining
   that no signals were found and the pair should be treated as
   serialize.

5. **`batch:NAME` tag races with concurrent edits.** Two `aida
   backlog groom` runs against overlapping ids could race-write the
   tag set. **Mitigation**: the existing `add_tag` semantics already
   re-read + merge; the upstream pattern is fine. Document that
   groom is not concurrent-safe (no lock yet).

6. **Risk heuristic's `BlockedBy` check requires the relationship
   graph.** Cheap to scan but the iteration order on `relationships`
   matters for performance on large stores. **Mitigation**: do one
   pass per req with `req.relationships.iter().any(...)` — O(n*k)
   where k is small.

7. **`make sync-templates` is a manual step.** The new skill files
   live in `aida-core/templates/` but the project's `.claude/` copies
   are symlinks the makefile creates. **Mitigation**: call out the
   `make sync-templates` step explicitly in the commit message and
   the verification.

8. **CLAUDE.md "discipline" doc-link in scaffolding** would need a
   one-line addition if we want `backlog-grooming.md` linked from
   the auto-appended Discipline section. **Mitigation**: in v1, the
   pack discovery is automatic (every `propagation:
   scaffolding-pack` file ships); explicit linking from CLAUDE.md
   can be a Followup.

## Tests

(all in `aida-cli/src/backlog.rs` `#[cfg(test)] mod tests`)

- `classify_risk_low_for_papercut_task` — task + low priority +
  `papercut` tag → `RiskLevel::Low`.
- `classify_risk_high_for_blocked_story` — story type, even with low
  priority, → `RiskLevel::High`.
- `classify_risk_medium_when_plan_owns_spec` — plan file present →
  `Medium` for a task without a low-risk tag.
- `classify_risk_high_when_blocked_by_present` — req with
  `RelationshipType::BlockedBy` → `RiskLevel::High`.
- `collect_backlog_candidates_excludes_queued_and_terminal` — given
  a fixture store with one Approved-queued, one Approved-unqueued,
  one Completed, one Draft → returns only the Approved-unqueued.
- `collect_spec_files_unions_traces_and_plan_critical_files` — temp
  project root with a `// trace:STORY-X` source file and a
  `docs/plans/2026-01-01-x.md` listing `aida-cli/src/cli.rs` in its
  `## Critical Files` → returns both paths.
- `classify_pair_overlap_safe_when_no_shared_files` —
  `BTreeSet{"a.rs"}` vs `BTreeSet{"b.rs"}` → `SafeParallel`.
- `classify_pair_overlap_serialize_when_shared_file` —
  `{"a.rs","b.rs"}` vs `{"b.rs"}` → `Serialize{shared:["b.rs"]}`.
- `classify_pair_overlap_unknown_when_either_empty` — empty set on
  one side → `Unknown`.
- `groom_dry_run_makes_no_writes` — fixture storage, run groom in
  dry-run, assert queue is unchanged AND tag set is unchanged.
- `groom_applies_batch_tag_to_each_spec` — non-dry-run with
  `--batch low-risk` → asserts each spec carries `batch:low-risk`
  and each is in the queue.
- `analyze_report_json_shape_is_stable` — `serde_json::to_value` on
  an `AnalyzeReport` for a 3-spec case, assert `pairs[0]` has
  `a`, `b`, `verdict`, `shared_files` keys.
- `backlog_list_json_shape_is_stable` — same idea for the list row.

## Verification

```bash
set -euo pipefail
REPO=/home/joe/ai/aida
cd "$REPO"

# 1. Builds clean on PR-CI's Linux configuration.
cargo build -p aida-cli
cargo test -p aida-cli backlog::

# 2. Sync the template symlinks so .claude/ sees the new files.
make sync-templates
test -L .claude/skills/aida-backlog-groom.md
test -L .claude/commands/aida-backlog-groom.md

# 3. End-to-end against a temp project (positive path).
TMP=$(mktemp -d)
cd "$TMP"
git init -q
AIDA="$REPO/target/debug/aida"
$AIDA init --no-skills --no-hooks
# Seed three Approved candidates with telling tags + types.
$AIDA add --type task --status approved --priority low \
    --tags papercut --title "fix typo in README"
$AIDA add --type task --status approved --priority low \
    --tags lifecycle:trivial --title "remove unused import"
$AIDA add --type story --status approved --priority high \
    --title "redesign auth flow"

# list filter
$AIDA backlog list --risk low | tee /tmp/list.txt
grep -E "papercut|lifecycle:trivial" /tmp/list.txt
! grep "redesign auth flow" /tmp/list.txt   # high risk hidden

# analyze (no overlap → safe-parallel)
LOW_IDS=$($AIDA backlog list --risk low --json | jq -r '.rows[].spec_id' | paste -sd,)
$AIDA backlog analyze --specs "$LOW_IDS" --json | jq '.pairs[0].verdict' \
    | grep -E "SafeParallel|Unknown"

# groom (dry-run → no queue change)
$AIDA queue list > /tmp/before.txt
$AIDA backlog groom --specs "$LOW_IDS" --batch cleanup --dry-run
$AIDA queue list > /tmp/after_dry.txt
diff /tmp/before.txt /tmp/after_dry.txt   # identical

# groom (real → both ids queued and tagged)
$AIDA backlog groom --specs "$LOW_IDS" --batch cleanup
$AIDA queue list --batch cleanup | grep -E "papercut|lifecycle:trivial"

# 4. Negative path: refuse a non-Approved id.
DRAFT_ID=$($AIDA add --type task --status draft --title "draft thing" \
    | grep -oE '[A-Z]+-[0-9]+')
! $AIDA backlog groom --specs "$DRAFT_ID" --batch cleanup 2>/tmp/err.txt
grep -q "not Approved" /tmp/err.txt || grep -q "Approved" /tmp/err.txt

# 5. cargo fmt + clippy (project convention).
cd "$REPO"
cargo fmt --all -- --check
cargo clippy -p aida-cli -- -D warnings
```

## Followups

- TASK: `aida backlog groom`: file-overlap source from `git log --grep "(SPEC-ID)"` so already-committed code joins the conflict set (currently trace+plan only).
- TASK: `aida backlog list --by-risk` (group view, mirrors `aida queue list --by-batch`).
- TASK: scaffolding-pack: link `backlog-grooming.md` from the auto-appended Discipline section in CLAUDE.md.
- TASK: `aida backlog groom --interactive` (drive a TTY prompt instead of requiring `--specs` from the CLI side) — once we have a justified pattern for CLI-side interactive prompts.
- TASK: cross-machine groom — emit a JSON plan another node can consume via `aida queue add`.
- TASK: feed the risk heuristic from STORY-439's calibration data once that lands.

## Related

- builds-on: TASK-229 (`batch:NAME` tag convention), TASK-238 (`--tag` / `--tag-prefix` flags), TASK-270 (`--batch` normalization).
- builds-on: TASK-94 (`build_reusable_helpers_section` and `scan_trace_graph` — the trace-graph engine the analyzer reuses).
- composes-with: STORY-248 (stacked-branch awareness for parallel pipelining — the conflict pairs are the input).
- composes-with: STORY-441 (archive concept — archived specs excluded from grooming by default).
- see-also: `docs/autonomous-drain.md` (the consumer of the `batch:NAME` tag this command produces).
- see-also: `aida-decompose` skill — same split (skill drives the AskUserQuestion conversation, CLI does the filing).
