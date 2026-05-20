# Plan: TASK-299 — Auto-generated `CHANGELOG.md` from spec graph + git tags

Date: 2026-05-19
Specs: TASK-299
Status: Planned
Complexity: ~450 prod LOC, ~200 test LOC, ~25 lines `release.sh`, small `CLAUDE.md`; 3 commits; risk low-medium

<!--
  AIDA plan template. Save plans as docs/plans/YYYY-MM-DD-<slug>.md.
  Prefer SYMBOL refs (`fn handle_changelog_command`) over LINE refs:
  symbol refs survive edits, line refs drift fast. trace:TASK-92
-->

## Approach

`aida changelog` is a new CLI subcommand group that generates `CHANGELOG.md`
mechanically from two data sources already in the repo: **git tags** (release
boundaries) and the **spec graph** (`RequirementsStore` — titles + types +
tags). For each release tag, walk the commits between it and the previous tag,
extract `(SPEC-ID)` and `(#PR)` references from commit subjects, resolve each
spec against the store, categorize it (Features / Fixes / Documentation /
Infrastructure / Internal / Other), and render one markdown section per
release. A `[Unreleased]` section covers commits since the most recent tag.

The output is **structured-from-data and deterministic** — same git state +
store produces byte-identical output, so `aida changelog refresh` is idempotent
and safe to run from `release.sh`. The file carries a "do not edit by hand"
header; regeneration is the only supported edit path.

The work is **CLI-centric** and self-contained: no skill, no scaffolding, no
cache-schema change. It mirrors the established sibling-module pattern
(`usage.rs`, the planned `digest.rs`) — a new `aida-cli/src/changelog.rs` holds
the engine; `main.rs` gets only the clap variant, an early-dispatch arm, and a
thin handler.

Built in 3 commit-sized phases, each independently demo-able. No child specs —
TASK-299 is one cohesive feature; the phases below are the build order.

### Diagram — data flow

```
  git tag -l 'v*'  ──►  ordered [v0.1.0 … v0.8.0]  ──┐
                                                     ├─ for each (prev,cur):
  git log prev..cur --no-merges --pretty=%s  ────────┤    scan subjects
                                                     │
                  per subject:                       ▼
   extract_spec_ids_from_commit ─┐         ┌─ CommitRec { spec_ids, pr, ctype }
   extract_pr_number_…_subject  ─┼────────►│
   parse_commit_type            ─┘         └─ aggregate per spec → ChangelogEntry
                                                     │
  load_store_for_lookup ──► RequirementsStore ──► title + req_type + tags
                                                     │
                                            classify(category)
                                                     │
                              render_markdown(sections)
                                                     │
                       generate→stdout|--out · refresh→CHANGELOG.md · preview→stdout
```

## Decisions

- **D1 — New `aida-cli/src/changelog.rs` module holds the data model, git
  scanning, classification, rendering, and file writing.** `main.rs` gets only
  `mod changelog;`, the early-dispatch arm, the `unreachable!` arm in the
  legacy match, and a thin `handle_changelog_command` that parses args and
  delegates. **Rationale**: `main.rs` is ~51k lines; `usage.rs` /
  `orchestrator.rs` / `drain_state.rs` set the sibling-module precedent, and
  the STORY-252 `digest.rs` plan makes the identical call (its D2).

- **D2 — Dispatch `Command::Changelog` *before* storage init**, alongside
  `Plan` / `Ultraplan` / `Goal` (`main.rs` early-dispatch block). The handler
  self-loads the store via `load_store_for_lookup`. **Rationale**: it is
  self-contained (git + store read only), needs no shared `Storage` handle, and
  must work from any cwd inside the repo — exactly the `Plan`/`Ultraplan`
  shape.

- **D3 — `ChangelogCommand { Generate, Refresh, Preview }` subcommand enum**,
  mirroring `PlanCommand`. `generate` → flexible, stdout or `--out`;
  `refresh` → always writes `CHANGELOG.md`, all releases; `preview` → stdout,
  `[Unreleased]` only. All three share one engine; the subcommands differ only
  in window + sink. **Rationale**: matches the task's proposed CLI surface and
  the repo's subcommand-group pattern (`Db`, `Cache`, `Plan`, `Drain`).

- **D4 — Categorize each spec by a documented precedence**, into six buckets
  (rendered in this order, empty buckets omitted): **Features, Fixes,
  Documentation, Infrastructure, Internal, Other**. Precedence:
  1. `req_type == Bug` → **Fixes** (unambiguous).
  2. tag ∈ {`documentation`, `docs`} → **Documentation**.
  3. tag ∈ {`release-tooling`, `ci`, `infrastructure`, `tooling`, `build`} →
     **Infrastructure**.
  4. else by the spec's commit `type` prefixes: any `feat` → Features;
     else any `fix` → Fixes; else all `docs` → Documentation; else any ∈
     {`ci`, `build`, `chore`} → Infrastructure; else ∈ {`refactor`, `perf`,
     `test`, `style`} → Internal.
  5. fallback by `req_type`: Story/Epic/Functional/User/NonFunctional/System →
     Features; Task/Spike/other → Internal.
  **Rationale**: the task says "fetch type from AIDA" but spec type alone is
  ambiguous (a `task` may be a feature, doc, or chore). The conventional commit
  `type` prefix (`[AI:tool] type(scope): desc (SPEC)`) is a reliable second
  signal already enforced by the repo's commit convention — combine both.

- **D5 — Classify the *spec*, not each commit; one entry per spec per
  release.** A spec referenced by N commits aggregates the union of their PR
  numbers and `type` prefixes into one `ChangelogEntry`. A spec spanning
  multiple PRs renders as `(#55, #57)`. **Rationale**: the CHANGELOG lists
  *what shipped*, keyed by spec; the acceptance criteria call out multi-PR
  specs explicitly.

- **D6 — `refresh --released-as <version>`** tells the generator to render the
  commits-since-last-tag under `## [<version>] — <today>` instead of
  `## [Unreleased]`. `release.sh` passes the about-to-be-created version so
  `CHANGELOG.md` commits *with* the version bump (the `v<new>` tag does not
  exist yet at generation time). Without the flag, `[Unreleased]` is always
  rendered. **Rationale**: resolves the chicken-and-egg — the acceptance
  criterion "CHANGELOG.md commits with the version bump" requires generating
  the new section before the tag exists.

- **D7 — Determinism contract.** `generate` / `refresh` *without*
  `--released-as` is byte-deterministic for a fixed git state + store: release
  dates come from git (`git log -1 --format=%cs`), entries sort by PR number
  descending then SPEC-ID, releases sort newest-first. `--released-as` injects
  *today's* date (it is the release date) — not git-derived, but it runs once
  per release, so the acceptance "idempotent — same git state → same output"
  holds for the no-flag path. **Rationale**: idempotency is what lets
  `release.sh` regenerate safely and a CI freshness gate become possible.

- **D8 — "Other" = no-SPEC-ID commits, minus noise.** A commit with no
  `(SPEC-ID)` whose type is *not* in {`chore`, `style`, `build`, `ci`,
  `revert`} and whose subject does not contain `typo`, and which is not a merge
  commit, renders under **Other** as a raw subject line. Pure-noise commits are
  dropped. **Rationale**: the acceptance says "group as Other", but a literal
  dump of every `chore:` commit would bury signal — keep Other a short
  "untracked work — consider filing a spec" list.

- **D9 — `release.sh` invokes `cargo run -q -p aida-cli -- changelog refresh
  --released-as v<new>`, not the PATH `aida`.** Failure (a pre-feature binary,
  build error) is a **non-fatal skip** with a warning — the release still
  proceeds. On success `CHANGELOG.md` is appended to `manifest_paths` so it is
  diffed in the preview, staged with the release commit, and named in the
  discard hints. **Rationale**: `cargo run` guarantees the *current branch's*
  code generates the file (the PATH `aida` may predate this feature);
  `cargo check --workspace` already ran just above, so deps are warm.

- **D10 — No `/aida-changelog` skill or scaffolding.** This is release
  machinery (`release.sh` calls it), not an agent workflow. No
  `aida-core/templates/`, no `ScaffoldConfig` field, no skill-count bump.
  **Rationale**: a skill is the surface for *judgment-bearing* agent tasks; a
  deterministic generator invoked by a script is not one. Filed as a followup
  if agent-workflow demand appears.

- **D11 — No per-release headline.** The task's proposed output shows a
  one-line headline under each `## [vX]` heading; it cannot be mechanically
  derived (annotated tags carry only a `git log` dump from `release.sh`'s
  `notes_file`). Drop it — the acceptance criteria do not require it. Followup:
  optional `.aida/changelog-headlines.toml` override file.

## Files (in build-order)

### Phase 1 — tracer: engine + `aida changelog generate`

Proves the whole pipeline end-to-end: walk every tag → scan commits → load
store → classify → render. Demo: `aida changelog generate` prints full
structured markdown for v0.1.0 … v0.8.0 + `[Unreleased]`.

#### `aida-cli/src/changelog.rs` — NEW module (created this phase)

- `struct ReleaseTag { name: String, date: String /* YYYY-MM-DD */ }`.
- `struct CommitRec { subject: String, spec_ids: Vec<String>, pr: Option<u64>,
  ctype: Option<String> }`.
- `enum Category { Features, Fixes, Documentation, Infrastructure, Internal,
  Other }` — with `fn heading(&self) -> &str` and a fixed render order.
- `struct ChangelogEntry { spec_id: String, title: String, prs: BTreeSet<u64>,
  category: Category }`.
- `struct OtherEntry { subject: String }` (no-SPEC commits).
- `struct ReleaseSection { heading: String /* "[v0.8.0] — 2026-05-15" or
  "[Unreleased]" */, lead: String /* count line */, entries: Vec<ChangelogEntry>,
  others: Vec<OtherEntry> }`.
- `fn scan_release_tags(project_root: &Path) -> Vec<ReleaseTag>`:
  `git tag -l 'v*'`, sort by semver, `git log -1 --format=%cs <tag>` per tag.
- `fn scan_commits_in_range(project_root: &Path, range: &str) -> Vec<CommitRec>`:
  `git log <range> --no-merges --pretty=format:%s`; per subject call
  `crate::extract_spec_ids_from_commit`, `crate::extract_pr_number_from_commit_subject`,
  `parse_commit_type`.
- `fn parse_commit_type(subject: &str) -> Option<String>`: strip optional
  `[AI:...]` prefix, take the token before `(` or `:`, lowercase, accept only
  the conventional set {feat,fix,docs,style,refactor,perf,test,build,ci,chore,
  revert}.
- `fn classify(req_type: Option<RequirementType>, tags: &HashSet<String>,
  ctypes: &[String]) -> Category`: the D4 precedence chain.
- `fn assemble(tags: &[ReleaseTag], store: &RequirementsStore,
  project_root: &Path, released_as: Option<&str>) -> Vec<ReleaseSection>`:
  per release range build `CommitRec`s, aggregate per spec into
  `ChangelogEntry` (union PRs + ctypes), resolve title/type/tags via
  `store.get_requirement_by_spec_id`, classify, sort.
- `fn render_markdown(sections: &[ReleaseSection]) -> String`: the
  `# Changelog` header + "do not edit" note + each section.
- `struct ChangelogOptions { window: Window, sink: Sink, released_as:
  Option<String> }` (`Window::{All, Unreleased, Range{since,until}}`,
  `Sink::{Stdout, File(PathBuf)}`).
- `fn run(opts: ChangelogOptions, project_root: &Path) -> Result<()>`:
  orchestrator — load store, assemble, render, write to the sink.

#### `aida-cli/src/cli.rs` — clap surface

- `Command` enum: add `#[clap(subcommand)] Changelog(ChangelogCommand)` (place
  near `Drain`).
- `enum ChangelogCommand { Generate { since: Option<String>, until:
  Option<String>, out: Option<PathBuf> }, Refresh { released_as:
  Option<String>, out: Option<PathBuf> }, Preview }` — mirror `PlanCommand`'s
  shape and doc-comment density.

#### `aida-cli/src/main.rs` — wire the subcommand

- Add `mod changelog;` to the module list (alphabetical, near `mod auto_complete;`).
- Early-dispatch arm in the pre-storage block (after the `Plan` arm):
  `if let Command::Changelog(cmd) = &cli.command { return
  handle_changelog_command(cmd); }`.
- `fn handle_changelog_command(cmd: &ChangelogCommand) -> Result<()>`: thin —
  `find_project_root`, build `ChangelogOptions` from the subcommand, call
  `changelog::run`. Place near `fn handle_ultraplan_command`.
- Legacy `match &cli.command`: add
  `Command::Changelog(_) => unreachable!("changelog is dispatched before storage init")`.

### Phase 2 — `preview`, `refresh`, `--out`, `--released-as`, `--since`/`--until`

#### `aida-cli/src/changelog.rs`

- `Window::Unreleased` → `assemble` restricted to the single
  `<last-tag>..HEAD` range; `preview` uses it.
- `Window::Range { since, until }` → filter the ordered `ReleaseTag` list to
  the named-tag span (inclusive); `generate --since/--until` uses it.
- `released_as`: when `Some`, the `<last-tag>..HEAD` section heading becomes
  `[<version>] — <today>` (`chrono::Utc::now()` date) and no separate
  `[Unreleased]` section renders.
- `Sink::File`: write the rendered string via
  `aida_core::fs_atomic::write_atomic`; `refresh` hardcodes
  `Sink::File(project_root.join("CHANGELOG.md"))`.
- Empty-`[Unreleased]` rendering: `_No changes since <last-tag>._`.

#### `aida-cli/src/main.rs`

- `handle_changelog_command`: map each subcommand to a `ChangelogOptions`
  (`Preview` → `Window::Unreleased`/`Sink::Stdout`; `Refresh` →
  `Window::All`/`Sink::File(CHANGELOG.md)`/`released_as`; `Generate` →
  `Window` from `--since`/`--until`, `Sink` from `--out`).

### Phase 3 — `release.sh` integration + doc sync

#### `scripts/release.sh` — regenerate CHANGELOG.md before tagging

Insert after the `cargo check --workspace` block, before `prev_tag=$(git
describe …)`:

```sh
echo
echo "─── Regenerating CHANGELOG.md ───"
if cargo run -q -p aida-cli -- changelog refresh --released-as "v$new"; then
    manifest_paths+=("CHANGELOG.md")
    echo "  ok — CHANGELOG.md regenerated for v$new"
else
    echo "  warning: 'aida changelog' unavailable (pre-feature binary) — skipping" >&2
fi
```

`CHANGELOG.md` joins `manifest_paths` only on success — it is then diffed in
the "Version bump diff" preview, staged by `git add "${manifest_paths[@]}"`,
and named in the cancel/restore hints, with no other change to that machinery.

#### `CLAUDE.md` — doc sync

- Add to the "Daily-use commands" block:
  `aida changelog generate`, `aida changelog refresh`, `aida changelog preview`.
- In the `release.sh` paragraph (AIDA-developer workflow): note it now
  regenerates `CHANGELOG.md` before tagging.

## Critical Files

- `aida-cli/src/changelog.rs` — NEW: data model, git scan, classification,
  render, file write, `run`.
- `aida-cli/src/cli.rs` — `Command::Changelog` variant + `enum ChangelogCommand`.
- `aida-cli/src/main.rs` — `mod changelog;`, early-dispatch arm,
  `handle_changelog_command`, `unreachable!` arm.
- `scripts/release.sh` — `changelog refresh` regeneration block + `manifest_paths`.
- `CLAUDE.md` — daily-use commands + release-process note.

## Reusable helpers (do not reimplement)

- `extract_spec_ids_from_commit` (`aida-cli/src/main.rs`) — pulls `(SPEC-ID)`
  trailers from a commit *subject* (handles `[AI:tool]` prefix, `(#N)` suffix,
  comma-separated cluster IDs). Works on a single-line subject too. Core of
  per-commit spec extraction.
- `extract_pr_number_from_commit_subject` (`aida-cli/src/main.rs`) — pulls the
  trailing `(#N)` PR number `gh` writes on squash-merge.
- **Visibility note**: both are private `fn`s in the `main.rs` crate root. A
  `mod changelog;` child module *can* call them as `crate::extract_…` —
  descendant modules see crate-root private items. No `pub(crate)` bump is
  needed; if the compiler disagrees on the exact edition rules, bump both to
  `pub(crate)` (pure functions, no behavior change). The STORY-252 `digest.rs`
  plan touches the same two functions — see Risk 3.
- `find_project_root` (`aida-cli/src/main.rs`, already `pub(crate)`) — walk cwd
  up to `.git/`.
- `load_store_for_lookup` (`aida-cli/src/main.rs`, already `pub(crate)`) —
  returns the full `RequirementsStore` (git-canonical, legacy fallback). Same
  helper `handle_ultraplan_command` / `handle_plan_command` use.
- `RequirementsStore::get_requirement_by_spec_id` (`aida-core/src/models.rs`) —
  SPEC-ID → `Option<&Requirement>`. Returns `None` for an unfiled/deleted ID
  (Risk 5).
- `Requirement::{title, req_type, tags}` (`aida-core/src/models.rs`) —
  `tags: HashSet<String>`; `req_type: RequirementType`.
- `RequirementType` (`aida-core/src/models.rs`) — the `enum` matched in
  `classify`.
- `aida_core::fs_atomic::write_atomic` — atomic write for `--out` / `refresh`.
- `usage` module (`aida-cli/src/usage.rs`) — structural precedent for a sibling
  CLI module; `handle_usage_command` for the thin-handler shape.
- The planned `aida-cli/src/digest.rs` (STORY-252) — shares the git-tag +
  spec-graph data sources (`list_release_tags`, `scan_commits`). If it lands
  first, reuse its tag/commit scanners rather than duplicating; see Risk 3.

## Risks + gotchas

1. **Risk**: the `aida` binary on `PATH` at release time may predate this
   feature → `changelog refresh` is an unknown subcommand. **Mitigation**: D9 —
   `release.sh` uses `cargo run -q -p aida-cli` (current branch's code), and a
   failed run is a non-fatal skip with a warning; the release proceeds.
2. **Risk**: `cargo run` in `release.sh` adds a build step. **Mitigation**:
   acceptable for a once-per-release script; `cargo check --workspace` runs
   immediately above, so the dependency graph is already compiled and only
   `aida`'s final codegen/link is incremental.
3. **Risk**: STORY-252 (`/aida-digest`) plan also scans git tags + the spec
   graph and also plans to touch `extract_spec_ids_from_commit` /
   `extract_pr_number_from_commit_subject`. **Mitigation**: the helper changes
   are idempotent (worst case both bump to `pub(crate)` — same edit). Whichever
   feature lands second reuses the first's tag/commit scanners instead of
   duplicating. No merge conflict expected (separate new modules); coordinate
   if both land in the same window. A shared extraction module is a followup.
4. **Risk**: shallow clone, missing tags, or not a git repo → git calls fail.
   **Mitigation**: every `git` invocation goes through a helper that treats a
   non-zero exit / missing binary as an empty result; the changelog renders the
   sections it can and the missing ones are simply absent.
5. **Risk**: a SPEC-ID appears in a commit but is not in the store (deleted, or
   never filed). **Mitigation**: `get_requirement_by_spec_id` returns `None` —
   classify by commit `type` alone (D4 step 4), use the SPEC-ID itself as the
   title (`- **TASK-999** — _(spec not in store)_`). Never drop, never panic.
6. **Risk**: non-determinism — `Requirement::tags` is a `HashSet` (unordered
   iteration). **Mitigation**: `classify` only ever calls `.contains()` on
   tags; tags are never iterated into output. Entry ordering is an explicit
   sort (PR desc, then SPEC-ID). Output is deterministic — verified by a
   render-twice test.
7. **Risk**: a cluster commit carries multiple SPEC-IDs
   (`(STORY-1, TASK-2)`). **Mitigation**: `extract_spec_ids_from_commit`
   returns all of them; each gets a `ChangelogEntry` sharing that commit's PR +
   type. Expected behavior — both shipped.
8. **Risk**: `--released-as` injects today's date, so its section is not
   git-derived. **Mitigation**: D7 — it *is* the release date and runs exactly
   once per release; the deterministic-idempotency contract is scoped to the
   no-flag path, which is what a CI freshness gate would check.
9. **Risk**: the first-ever `release.sh` run generates a brand-new
   `CHANGELOG.md` (untracked). **Mitigation**: it lives at the repo root, *not*
   under `.aida/`, so the deny-by-default `.gitignore` block does not apply and
   there is no session-end untracked-path papercut. The release commit stages
   it via `manifest_paths`; from the next release on it is tracked.
10. **Risk**: `[Unreleased]` with zero commits since the last tag renders an
    empty/odd section. **Mitigation**: emit `_No changes since <last-tag>._` so
    the file shape stays stable.

## Tests (named, not "add tests")

In `aida-cli/src/changelog.rs` `#[cfg(test)]`:

- `parse_commit_type_strips_ai_prefix` — `[AI:claude] feat(x): y (Z-1)` →
  `Some("feat")`; `chore(deps): bump` → `Some("chore")`; `random text` →
  `None`; `Feat: X` (wrong case) normalizes to `feat`.
- `classify_bug_type_is_fixes` — `req_type = Bug` → `Category::Fixes`
  regardless of tags/ctypes.
- `classify_docs_tag_is_documentation` — a `documentation` tag wins over a
  `feat` commit type.
- `classify_release_tooling_tag_is_infrastructure` — `release-tooling` tag →
  `Infrastructure`.
- `classify_feat_commit_overrides_task_type` — `req_type = Task`, no
  discriminating tag, a `feat` commit → `Features` (not the Task→Internal
  fallback).
- `classify_falls_back_to_story_as_features` — `req_type = Story`, no tags, no
  parseable ctype → `Features`.
- `aggregate_spec_across_commits_unions_prs` — one SPEC-ID in two commits with
  `(#5)` and `(#7)` → one `ChangelogEntry`, `prs = {5, 7}`.
- `no_spec_commit_goes_to_other` — `feat: untracked thing` (no SPEC-ID) →
  one `OtherEntry`.
- `noise_commit_excluded_from_other` — `chore(lease): x`, `style: fmt`,
  `fix typo` (no SPEC-ID) produce no `OtherEntry`.
- `unknown_spec_id_uses_id_as_title` — a SPEC-ID absent from the store renders
  with the ID as its own title and classifies by commit type.
- `render_orders_categories_and_releases` — categories appear
  Features→Fixes→Documentation→Infrastructure→Internal→Other; releases
  newest-first; empty categories omitted.
- `render_is_deterministic` — `render_markdown` of the same sections twice →
  byte-identical.
- `released_as_replaces_unreleased_heading` — with `released_as = Some("v0.9.0")`
  the section heading is `[v0.9.0] — <date>` and no `[Unreleased]` heading
  appears.
- `scan_release_tags_in_temp_repo` — temp git repo with two tagged commits →
  two `ReleaseTag`s, semver-ordered, dates populated (mirrors the digest plan's
  `parse_digest_since_handles_git_tag` temp-repo pattern).

## Verification

```bash
# Run from a clean checkout of this repo with aida built.
set -euo pipefail

# 1. Tracer: generate the full changelog — must list real releases.
aida changelog generate | tee /tmp/cl.md
grep -q '^# Changelog' /tmp/cl.md
grep -q '## \[Unreleased\]' /tmp/cl.md
grep -q '## \[v0.8.0\]' /tmp/cl.md
grep -q '### Features' /tmp/cl.md

# 2. Preview is Unreleased-only.
aida changelog preview | tee /tmp/clp.md
grep -q '## \[Unreleased\]' /tmp/clp.md
grep -q '## \[v0.8.0\]' /tmp/clp.md && \
  { echo 'FAIL: preview leaked a tagged release'; exit 1; } || echo 'OK: preview scoped'

# 3. refresh writes CHANGELOG.md and is idempotent.
aida changelog refresh
test -s CHANGELOG.md
cp CHANGELOG.md /tmp/cl-run1.md
aida changelog refresh
diff -q /tmp/cl-run1.md CHANGELOG.md   # expect: identical (idempotent)

# 4. --released-as replaces [Unreleased] with a versioned heading.
aida changelog refresh --released-as v9.9.9 --out /tmp/cl-ra.md
grep -q '## \[v9.9.9\]' /tmp/cl-ra.md
grep -q '## \[Unreleased\]' /tmp/cl-ra.md && \
  { echo 'FAIL: [Unreleased] present under --released-as'; exit 1; } || echo 'OK: released-as'

# 5. --since/--until bound the tag span.
aida changelog generate --since v0.7.0 --until v0.8.0 | grep -q '## \[v0.8.0\]'

# 6. Bad PR/spec edges do not panic.
aida changelog generate >/dev/null && echo 'OK: no panic on full history'

# 7. Tests.
cargo test -p aida-cli changelog

# 8. release.sh wiring is syntactically valid.
bash -n scripts/release.sh && echo 'OK: release.sh parses'

# Reset the spot-check artifact (refresh leaves a real CHANGELOG.md — keep it).
rm -f /tmp/cl.md /tmp/clp.md /tmp/cl-run1.md /tmp/cl-ra.md
echo 'ALL CHANGELOG CHECKS PASSED'
```

## Followups

Out of scope now — file as child TASKs at `aida queue done` time:

- CI freshness gate: a workflow step running `aida changelog refresh && git
  diff --exit-code CHANGELOG.md` to fail PRs that left the changelog stale.
- `/aida-changelog` skill + slash command, if agent-workflow demand appears
  (D10 — currently release machinery only).
- Optional `.aida/changelog-headlines.toml` per-release headline override
  (D11 — restores the proposed-output headline as a manual opt-in).
- Shared git-tag + spec-graph extraction module factored out of `changelog.rs`
  and the STORY-252 `digest.rs` (Risk 3).
- `release.sh` consumes `aida changelog generate --since <prev-tag>` for the
  GitHub Release body, replacing the raw `git log` `notes_file`.
- `aida show <SPEC>` surfaces "shipped in v0.X.Y" by reverse-indexing the
  changelog (the task lists this as a sibling capability, explicitly not in
  scope here).

## Related

- See also: STORY-252 (`/aida-digest` — overlapping git-tag + spec-graph data
  sources; digest is narrative customer-facing, CHANGELOG is structured
  developer-facing; `docs/plans/2026-05-19-story-252-aida-digest.md`).
- Composes with: `scripts/release.sh` (regenerates `CHANGELOG.md` before
  tagging — D9), the `[AI:tool] type(scope): description (SPEC-ID)` commit
  convention in `CLAUDE.md` (the structured input the generator parses).
- Builds on: EPIC-24 (Living documentation) — a CHANGELOG is a generated
  doc projected from the spec graph.
- Precedent: `aida usage` (`aida-cli/src/usage.rs`) — sibling read-only
  telemetry/reporting CLI surface.
