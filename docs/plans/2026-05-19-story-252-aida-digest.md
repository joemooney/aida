# Plan: STORY-252 — `/aida-digest` advisor-curated narrative work report

Date: 2026-05-19
Specs: STORY-252
Status: Approved
Complexity: ~600 prod LOC, ~250 test LOC, 4 commits, risk medium

<!--
  AIDA plan template. Save plans as docs/plans/YYYY-MM-DD-<slug>.md.
  Prefer SYMBOL refs (`fn handle_digest_command`) over LINE refs: symbol
  refs survive edits, line refs drift fast. trace:TASK-92
-->

## Approach

`aida digest` is a new CLI subcommand that assembles a **structured-from-data**
narrative report of project work in a time window — Released / Major progress /
Strategic direction / Next iteration / Process artifacts — by reading the
requirement store, git history, `docs/plans/`, and (best-effort) the memory
pack. It applies *mechanical* editorial rules (drop typo/`chore`/`style`
commits, collapse cluster-PRs to one theme line, keep only rejected specs that
carry a `supersedes`/`pivoted-from` link, strip SPEC-IDs in customer audience).
A `.aida/last-digest.toml` marker records the window end so the next bare
`aida digest` resumes from it. A thin `/aida-digest` skill + command wrap the
CLI for advisor-role use.

The work is **CLI-centric**: the STORY explicitly puts "AI-rewriting the digest
for tone/style" out of scope and states "initial digest is structured-from-data".
The skill therefore does *not* re-narrate — it picks a sensible audience/window,
runs `aida digest`, and presents/saves the output. All editorial logic is
deterministic Rust.

Built in 4 commit-sized phases, each independently demo-able. No child specs —
STORY-252 is one cohesive feature; the phases below are the build order.

### Diagram — data flow

```
                         ┌─ git tags  (v*)        ──► Released
                         ├─ git log   (window)    ──► PRs / cluster-PRs
  parse_digest_since ───► │─ RequirementsStore    ──► Completed / Strategic / Next
   (Nd|date|tag|marker)  ├─ docs/plans/*.md        ──► Notable plans
                         └─ ~/.claude/…/MEMORY.md  ──► Process artifacts
                                   │
                           editorial filters
                  (drop typo/chore · collapse cluster-PR ·
                   keep rejected-w/-supersedes · audience strip)
                                   │
                    render(audience, format)  ──► stdout | --out <file>
                                   │
                       write .aida/last-digest.toml  (unless --reset)
```

## Decisions

- **D1 — CLI-centric, structured-from-data; skill is a thin invoker.**
  **Rationale**: STORY-252 puts "AI tone-rewrite" explicitly out of scope and
  says the initial digest is structured-from-data. Editorial logic (typo
  filter, cluster-PR collapse, rejected-with-supersedes) is mechanical, so it
  belongs in deterministic Rust, not a Claude prompt. The skill's judgment is
  *which window/audience to run*, not prose rewriting.

- **D2 — New `aida-cli/src/digest.rs` module holds the data model, assembly,
  editorial filters, rendering, and the marker struct.** `main.rs` gets only
  the clap variant, dispatch arm, and a thin `handle_digest_command` that parses
  args and delegates. **Rationale**: `main.rs` is already ~51k lines;
  `usage.rs` / `orchestrator.rs` / `drain_state.rs` set the sibling-module
  precedent.

- **D3 — Load the full `RequirementsStore` in-memory; do NOT extend the cache
  SQL.** **Rationale**: a digest runs occasionally, not in a hot loop. The full
  store gives `relationships` and `implementation.completed_at` directly;
  touching `cache_schema.sql` would force a cache-version migration for no
  latency win.

- **D4 — `--audience` and `--format` are local `#[derive(ValueEnum)]` enums in
  `digest.rs`.** `DigestAudience::{Customer, Team, Slf}` and
  `DigestFormat::{Markdown, Plain, Json, Brief}`. `self` is a Rust keyword —
  the variant is `Slf` with `#[value(name = "self")]`. **Rationale**: matches
  the per-command pattern (no shared format enum exists in the codebase).

- **D5 — `--include-next` / `--include-process` are `Option<bool>` with
  `num_args(0..=1)` + `default_missing_value("true")`.** Absent → audience
  default (next: always on; process: on for team/self, off for customer); bare
  flag → true; `--include-process=false` → off. **Rationale**: honors the
  STORY's flag names while giving a real opt-out without inventing `--no-*`
  twins.

- **D6 — `parse_digest_since` accepts three shapes**: `Nd|Nh|Nm` (reuse
  `parse_days_arg`), an ISO date `YYYY-MM-DD` (parsed at 00:00 UTC), and any
  other token treated as a git ref/tag resolved via
  `git log -1 --format=%cI <ref>`. Bare `aida digest` (no `--since`) reads the
  marker; if no marker, defaults to `24h`. **Rationale**: covers every form the
  STORY lists with one fallthrough chain.

- **D7 — "completed in window" keys off `req.implementation.completed_at`,
  falling back to `req.modified_at` when status ∈ {Done, Completed} and
  `completed_at` is `None`.** **Rationale**: `completed_at` is set by the
  STORY-86 auto-bump but is absent on specs completed before that landed or
  bumped manually.

- **D8 — Superseding detection is lenient**: a `Custom` relationship whose
  label contains `supersed` or `pivot`, OR a tag matching
  `supersedes:<SPEC>` / `pivoted-from:<SPEC>`. **Rationale**: `RelationshipType`
  has no built-in `Supersedes` variant (only `Custom(String)`), and the project
  also expresses pivots via tags — accept both.

- **D9 — The Process-artifacts memory section is best-effort.** Locate
  `~/.claude/projects/<slug>/memory/MEMORY.md` (slug derived from project root);
  parse index lines for titles. Team/self audience only, unless an individual
  memory file carries `audience: public` frontmatter (then eligible for
  customer too). Missing dir → section silently skipped. **Rationale**: the
  memory pack lives outside the repo and current files have no `audience`
  field; the digest must degrade, not error.

- **D10 — `.aida/last-digest.toml` is runtime per-clone state.** It is covered
  by the `.aida/*` deny-by-default `.gitignore` block — **do NOT add a
  `!.aida/last-digest.toml` allow line**. **Rationale**: the marker is
  per-clone, like `cache.db`; tracking it would sync one machine's cadence to
  all clones. trace:BUG-73 convention.

## Files (in build-order)

### Phase 1 — tracer bullet: window parser + Released + Completed sections

Proves the whole pipeline end-to-end: parse window → load store → scan git tags
→ render markdown. Demo: `aida digest --since 30d` produces a real digest with
`## Released` listing v0.7.0 / v0.8.0.

#### `aida-cli/src/digest.rs` — NEW module (created this phase)

- `struct DigestOptions`: resolved `since: DateTime<Utc>`, `until: DateTime<Utc>`,
  `audience`, `format`, `include_next: bool`, `include_process: bool`,
  `out: Option<PathBuf>`.
- `fn parse_digest_since(raw: Option<&str>, project_root: &Path) -> Result<DateTime<Utc>>`:
  D6 fallthrough chain (duration → ISO date → git ref → marker → `24h`).
- `fn list_release_tags(project_root: &Path, since: DateTime<Utc>) -> Vec<ReleaseTag>`:
  `git tag -l 'v*'` + `git log -1 --format=%cI` per tag; filter to window.
  `struct ReleaseTag { name, date, commit_subjects: Vec<String> }`.
- `fn collect_completed(store: &RequirementsStore, opts: &DigestOptions) -> Vec<CompletedSpec>`:
  D7 timestamp logic.
- `fn render_markdown(report: &DigestReport, opts: &DigestOptions) -> String`:
  Released + Major progress sections only this phase.
- `fn run(opts: DigestOptions, project_root: &Path) -> Result<()>`: orchestrator
  — load store, assemble `DigestReport`, render, print.

#### `aida-cli/src/main.rs` — wire the subcommand

- Add `mod digest;` to the module list (alphabetical, near `mod docs;`).
- `fn handle_digest_command(...)`: thin — build `DigestOptions`, call
  `digest::run`. Place near `fn handle_usage_command`.
- `match &cli.command` dispatch arm: `Command::Digest { .. } => handle_digest_command(..)?`.

#### `aida-cli/src/cli.rs` — clap surface

- `Command` enum: add `Digest { since: Option<String>, audience: DigestAudience,
  format: DigestFormat, include_next: Option<bool>, include_process: Option<bool>,
  out: Option<PathBuf>, reset: bool }`.
- Re-export `DigestAudience` / `DigestFormat` from `digest.rs` for the variant
  field types (or define the `ValueEnum`s in `cli.rs` and use them in `digest.rs`).

### Phase 2 — audience modes, format modes, full section set, editorial filters

#### `aida-cli/src/digest.rs`

- `enum DigestAudience { Customer, Team, Slf }`, `enum DigestFormat { Markdown,
  Plain, Json, Brief }` (D4) — `#[derive(ValueEnum)]`.
- `fn scan_commits(project_root, since, until) -> Vec<CommitRec>`: `git log
  --since --until --pretty`, per commit call `extract_spec_ids_from_commit`,
  `extract_referenced_spec_ids_from_commit`, `extract_pr_number_from_commit_subject`.
- `fn collapse_cluster_prs(commits: &[CommitRec]) -> Vec<PrTheme>`: group by PR
  number; a PR with ≥2 distinct specs is a cluster → one theme line.
- `fn is_noise_commit(subject: &str) -> bool`: drop `docs:` / `style:` /
  `chore:` / `revert:` and subjects containing `typo` (the OUT list).
- `fn collect_strategic(store, opts) -> Vec<StrategicFiling>`: EPICs (and
  STORYs tagged `foundational`/`strategic`) created in window.
- `fn collect_rejected_pivots(store, opts) -> Vec<Pivot>`: rejected specs with a
  supersedes/pivot link (D8); name by the successor.
- `fn collect_next(store, project_root) -> NextSection`: Done-but-not-Completed
  specs (in-flight) from the store + queued items via `global_queue::load`,
  grouped by `batch:` tag.
- `fn collect_plans(project_root, since) -> Vec<PlanRef>`: scan `docs/plans/`,
  parse `YYYY-MM-DD` filename prefix, keep those in window.
- `fn collect_process(project_root, opts) -> Vec<MemoryEntry>`: D9 best-effort
  memory-pack read.
- Extend `render_markdown` with Strategic direction / Next iteration / Process
  artifacts; add `render_plain`, `render_json` (serde), `render_brief`
  (single-paragraph TL;DR).
- Audience gating: customer strips SPEC-IDs and cluster-PR member specs;
  team/self keep them.

### Phase 3 — cadence marker + `--reset` + `--out`

#### `aida-cli/src/digest.rs`

- `struct DigestMarker { window_end: DateTime<Utc>, written_at: DateTime<Utc> }`
  with `fn load(project_root) -> Option<DigestMarker>`,
  `fn write(&self, project_root) -> Result<()>` (serde + `toml`, atomic write
  via `aida_core::fs_atomic::write_atomic`), `fn clear(project_root) -> Result<()>`.
  Path: `project_root.join(".aida/last-digest.toml")`.
- `parse_digest_since`: wire the marker into the fallthrough (D6).
- `run`: after a successful render write the marker (skip on `--reset`, on a
  `--since` git-tag that names a *future* boundary, and arguably on
  `--format json` piping — keep it simple: always write unless `--reset`).
- `--reset`: call `DigestMarker::clear` and return early.
- `--out <path>`: write the rendered string via `write_atomic` instead of stdout.

### Phase 4 — `/aida-digest` skill + command + scaffolding wiring

#### `aida-core/templates/skills/aida-digest.md` — NEW

Frontmatter: `name: aida-digest`, `description: …`, `allowed-tools: [Bash, Read,
Write]`. Body sections: Purpose, When to use, Skip if, Workflow (run
`aida digest` with audience inferred from context, present output, offer
`--out`), CLI reference. Mention advisor/`role:dialog` home in prose (no role
metadata field — skills are role-agnostic). Include the Path/What-happens/Why
end-of-session table with the `▶`/`⇒`/`⏸` glyphs (BUG-116 regression guard).

#### `aida-core/templates/commands/aida-digest.md` — NEW

Matching slash command (mirror an existing command file's shape, e.g.
`aida-core/templates/commands/aida-status.md`).

#### `aida-core/src/scaffolding/mod.rs` — scaffold the new skill

- `struct ScaffoldConfig`: add `pub include_aida_digest_skill: bool`.
- `Default`/constructor: `include_aida_digest_skill: true`.
- Scaffold block: `if self.config.include_aida_digest_skill { … }` writing
  `.claude/skills/aida-digest.md` via `self.generate_aida_digest_skill()`.
- `fn generate_aida_digest_skill(&self) -> String`: load
  `EMBEDDED_TEMPLATES.get("skills/aida-digest.md")`.
- Command parity list (the `("aida-docs", …)` tuple array): add
  `("aida-digest", self.config.include_aida_digest_skill)`.
- `generate_commands()` command-defs array: add the
  `aida-core/templates/commands/aida-digest.md` entry.

#### `CLAUDE.md` — doc sync

- "scaffolds 32 skills" → "33 skills" (and any other count reference).

## Critical Files

- `aida-cli/src/digest.rs` — NEW: model, assembly, filters, render, marker.
- `aida-cli/src/main.rs` — `mod digest;`, dispatch arm, `handle_digest_command`.
- `aida-cli/src/cli.rs` — `Digest` variant, `DigestAudience`/`DigestFormat`.
- `aida-core/templates/skills/aida-digest.md` — NEW skill template.
- `aida-core/templates/commands/aida-digest.md` — NEW command template.
- `aida-core/src/scaffolding/mod.rs` — `ScaffoldConfig` field + default +
  scaffold block + parity tuple + command-defs entry + generate method.
- `CLAUDE.md` — skill count.

## Reusable helpers (do not reimplement)

- `parse_days_arg` (`aida-cli/src/main.rs`) — `Nd|Nh|Nm` → `chrono::Duration`.
  `parse_digest_since` wraps this; do not re-parse durations.
- `extract_spec_ids_from_commit`, `extract_referenced_spec_ids_from_commit`,
  `extract_pr_number_from_commit_subject` (`aida-cli/src/main.rs`) — pull
  SPEC-IDs (subject + body) and the `(#N)` PR number from a commit. Core of
  cluster-PR detection. May need to be made `pub(crate)`.
- `split_md_frontmatter`, `frontmatter_field`, `normalize_line_endings`
  (`aida-cli/src/main.rs`) — parse memory-file YAML frontmatter (the
  `audience: public` check in D9).
- `find_project_root` (`aida-cli/src/main.rs`) — walk CWD up to `.git/`.
- `aida_core::fs_atomic::write_atomic` — atomic write for the marker and
  `--out`.
- `node::NodeConfig::{load, save}` (`aida-core/src/node.rs`) — copy this
  `toml::from_str` / `toml::to_string_pretty` pattern for `DigestMarker`.
- `drain_state` module (`aida-cli/src/drain_state.rs`) — closest precedent for
  a `.aida/` state file: `path()` / `read()` / `write()` / `clear()` trio.
  Mirror its shape for `DigestMarker`.
- `global_queue::load(role)`, `global_queue::project_name_for(root)`
  (`aida-cli/src/global_queue.rs`) — queued items for the Next section, and the
  project slug for locating the memory dir.
- `RequirementsStore` via `storage.load()` — full store. Read
  `req.implementation.completed_at` / `completion_sha`, `req.relationships`,
  `req.status`, `req.created_at`, `req.modified_at`, `req.tags`,
  `req.requirement_type`.
- `copy_to_clipboard` (`aida-cli/src/main.rs`) — only if a `--copy` flag is
  added later (not in scope; see Followups).
- `usage` module (`aida-cli/src/usage.rs`) — structural precedent for a sibling
  CLI module; `handle_usage_command` for the handler shape.
- `test_skill_command_parity` (`aida-core/src/templates.rs`) — already asserts
  every skill has a command; auto-covers the new pair, no new test needed there.

## Risks + gotchas

1. **Risk**: git unavailable, shallow clone, or not a git repo — Released /
   commit-scan sections fail. **Mitigation**: every `git` call goes through a
   helper that treats non-zero exit / missing binary as an empty result; the
   digest renders the remaining sections.
2. **Risk**: memory pack lives *outside* the repo
   (`~/.claude/projects/<slug>/memory/`) and current files have no `audience`
   field. **Mitigation**: D9 — best-effort, team/self only by default, silent
   skip when the dir is absent. Never error on it.
3. **Risk**: `self` is a Rust keyword — `--audience self` cannot map to a
   `Self` variant. **Mitigation**: D4 — variant `Slf` with
   `#[value(name = "self")]`.
4. **Risk**: cluster-PR collapse over-collapses a normal single-spec PR.
   **Mitigation**: only collapse when ≥2 *distinct* SPEC-IDs share one PR
   number; a 1-spec PR renders as an ordinary completed-spec line.
5. **Risk**: adding `.aida/last-digest.toml` triggers the session-end
   untracked-path papercut. **Mitigation**: D10 — it is covered by `.aida/*`
   deny-by-default; do not add an allow line. Verify `git status` stays clean
   after a run.
6. **Risk**: `extract_spec_ids_from_commit` & friends may be private to
   `main.rs`. **Mitigation**: bump them to `pub(crate)` (Phase 2) — pure
   functions, no behavior change.
7. **Risk**: ISO-date vs git-tag ambiguity in `parse_digest_since` (a tag could
   look date-ish). **Mitigation**: try strict `YYYY-MM-DD` regex *first*, git
   ref only as the final fallthrough; bail with a clear message if the ref does
   not resolve.
8. **Risk**: `aida init --no-skills` must still skip the digest skill.
   **Mitigation**: gate the scaffold block on `include_aida_digest_skill`
   exactly like every sibling skill — `--no-skills` already flips all such
   flags off; follow the pattern, add no special case.

## Tests (named, not "add tests")

In `aida-cli/src/digest.rs` `#[cfg(test)]`:

- `parse_digest_since_handles_duration` — `7d` → `now − 7 days`.
- `parse_digest_since_handles_iso_date` — `2026-05-15` → that date 00:00 UTC.
- `parse_digest_since_handles_git_tag` — `v0.7.0` resolves to the tag's commit
  date (run in a temp repo with a tagged commit).
- `parse_digest_since_rejects_garbage` — `not-a-window` → `Err`.
- `parse_digest_since_falls_back_to_marker` — no `--since`, marker present →
  marker's `window_end`; no marker → `24h`.
- `digest_marker_round_trip` — `write` then `load` `.aida/last-digest.toml`
  yields the same `window_end`; `clear` removes the file.
- `collapse_cluster_pr_groups_multi_spec` — two specs sharing `(#42)` collapse
  to one `PrTheme`; a lone-spec PR does not.
- `is_noise_commit_drops_typo_and_chore` — `docs:`, `chore(lease):`,
  `style:`, "fix typo" → true; `feat(...)` → false.
- `collect_rejected_pivots_keeps_only_superseded` — a rejected spec with a
  `pivoted-from` link is included; a rejected spec with no link is excluded.
- `customer_audience_strips_spec_ids` — `render(... Customer ...)` output
  contains no `STORY-`/`TASK-`/`EPIC-` tokens; team audience does.
- `render_brief_is_single_paragraph` — `Brief` output has no blank lines / no
  `##` headings.

`test_skill_command_parity` (`aida-core/src/templates.rs`) already covers the
new skill↔command pair once both template files exist — no new test there.

## Verification

```bash
# Run from a clean checkout of this repo with aida built.
set -euo pipefail

# 1. Tracer: digest the last 30d — must list the recent releases.
aida digest --since 30d | tee /tmp/d.md
grep -q '## Released' /tmp/d.md
grep -q 'v0.8.0' /tmp/d.md            # expect: recent release present

# 2. Audience strip — customer mode emits no SPEC-IDs.
aida digest --since 30d --audience customer | grep -Eq 'STORY-[0-9]' && \
  { echo 'FAIL: SPEC-ID leaked into customer audience'; exit 1; } || echo 'OK: customer strip'
aida digest --since 30d --audience team | grep -Eq 'STORY-[0-9]'     # expect: present

# 3. Formats parse.
aida digest --since 7d --format json | python3 -c 'import json,sys; json.load(sys.stdin)'
aida digest --since 7d --format brief | wc -l                        # expect: small

# 4. Cadence marker round-trip.
aida digest --since 7d >/dev/null
test -f .aida/last-digest.toml                                       # expect: written
aida digest >/dev/null                                               # picks up from marker
aida digest --reset >/dev/null
test ! -f .aida/last-digest.toml                                     # expect: cleared

# 5. Marker did not dirty the tree.
git status --porcelain | grep -q 'last-digest.toml' && \
  { echo 'FAIL: marker is tracked'; exit 1; } || echo 'OK: marker gitignored'

# 6. --out writes a file.
aida digest --since 7d --out /tmp/digest-out.md && test -s /tmp/digest-out.md

# 7. Scaffolding parity (the embedded-template test).
cargo test -p aida-core test_skill_command_parity

echo 'ALL DIGEST CHECKS PASSED'
```

## Followups

Out of scope now — file as child TASKs at `aida queue done` time:

- `/aida-daily-digest` cron sibling skill that runs `aida digest` on a cadence.
- `aida release` integration — emit `aida digest --since <prev-tag>` as release notes.
- AI tone-rewrite pass over the structured digest (explicitly STORY-252 OUT).
- Slack/email posting of digest output.
- Cross-project (multi-repo) digest aggregation.
- `audience: public` memory-frontmatter convention — document it and tag the
  public-worthy starter-pack memories.
- `aida digest --copy` clipboard flag (helper already exists).
- Reconcile with TASK-299 (auto-generated `CHANGELOG.md`) — shared git-tag /
  spec-graph data sources; consider a common extraction layer.

## Related

- Builds on: EPIC-24 (Living documentation) — parent; a digest is a
  time-window-bounded narrative doc.
- Composes with: `aida usage` (sibling telemetry surface), `role:dialog` /
  advisor responsibilities (editorial home).
- See also: TASK-299 (CHANGELOG from spec graph + tag boundaries — overlapping
  data sources), STORY-262 (advisor scheduled tasks — a natural scheduler for a
  recurring digest), `feedback_precise_lifecycle_vocabulary.md` memory (digest
  uses the precise verbs: merged / released / completed).
