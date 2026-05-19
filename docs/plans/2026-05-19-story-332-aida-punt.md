# Plan: STORY-332 — /aida-punt mechanism + NeedsAttention lifecycle status

Date: 2026-05-19
Specs: STORY-332
Status: In Progress
Complexity: ~450 prod LOC, ~250 test LOC, 1 commit, risk medium

<!-- trace:STORY-332 | ai:claude -->

## Approach

Add an eighth lifecycle status, `NeedsAttention`, and a `/aida-punt` mechanism
(CLI subcommand + skill + command) so an autonomous agent that hits a
design-fork it cannot safely resolve can **pause the spec instead of guessing**.
The status threads through every enumeration site (compiler-forced for `match`
arms; manually audited for string-parse tables). A punt is recorded three ways:
the spec's status flips to `NeedsAttention`, a first-class typed
`attention_reason` field captures the structured why, and an append-only
`.aida/punts.jsonl` ledger record is written (the forward-compatible seed for
STORY-325's analysis layer). The triage surface composes with `aida findings`:
a NeedsAttention spec is a punt awaiting advisor triage.

### Diagram

```
   InProgress ──aida punt──► NeedsAttention ──aida edit --status──► Approved
       ▲                          │                                InProgress
       │                          └──────────────────────────────► Rejected
       └─ punt rule: into NeedsAttention ONLY from InProgress
          out of NeedsAttention ONLY to Approved / InProgress / Rejected
          all other transitions stay free-form (no regression)

   aida punt STORY-N --category design-fork --reason "..." [--lean "..."]
      │
      ├─► status        → NeedsAttention
      ├─► attention_reason (typed field on the spec)
      ├─► .aida/punts.jsonl  (append-only ledger record)
      └─► returns control — orchestrator advances (NeedsAttention not drivable)
```

## Decisions

- **Obstacle-type category vocabulary** (`design-fork`, `ambiguous-spec`,
  `missing-context`, `blocked-dependency`, `other`). **Rationale**: punt-time
  categories must be *observable facts*, not competence self-judgments. The
  escalation-reason taxonomy (`lack-of-synthesis`, `unrecorded-preference`, …)
  is STORY-325's *ledger-derived, advisor-reviewed* layer — deliberately NOT a
  punt-time label. This rationale is pinned as a `//` comment on `PuntCategory`
  so nobody later "improves" it into escalation-reason. A `blocked-dependency`
  punt is a direct signal to file a blocked-by relationship (STORY-333) — the
  punt feeds the graph that prevents the next one.
- **First-class typed `attention_reason` field** on `Requirement`, not
  `custom_fields`. **Rationale**: cleaner typing; the metadata is structural,
  not project-specific. Fields: `category: PuntCategory`; `detail: String`
  (named `detail`, not `comment`, to avoid overloading AIDA's spec comments);
  `lean: Option<String>` (the raiser's best-guess-if-forced — kept distinct
  from `detail` so the ledger separates *the fork* from *the agent's lean*;
  defining it now is the cheap moment); `raised_by`, `raised_at`. No
  `escalation_reason` field — that is STORY-325's derived layer.
- **`aida punt` is a real CLI subcommand**, not a skill that chains `aida edit`.
  **Rationale**: the status flip + metadata write + ledger append must be one
  atomic operation; the spec's Origin names "no CLI subcommand" as part of the
  gap. The `/aida-punt` skill/command just invokes it.
- **Minimal forward-compatible ledger.** **Rationale**: STORY-325 owns the full
  ledger (analysis, classification, `aida punt analyze`). STORY-332 writes one
  structured record per punt in STORY-325's field shape so 325 builds on top.
- **`attention_reason` cleared when a spec transitions out of NeedsAttention.**
  **Rationale**: it answers "why is this *currently* paused"; the ledger keeps
  the durable history. Stale metadata on a resumed spec is confusing.
- **NeedsAttention is NOT terminal.** **Rationale**: it transitions back to
  active work; `is_terminal_status` keeps its Completed/Rejected meaning.
  `queue next` gets a *separate* explicit skip so punted specs leave normal
  pickup but stay visible in `aida list` / `aida findings`.

## Files (in build-order)

### `proto/aida.proto` — protobuf enum

- `enum RequirementStatus`: add `REQUIREMENT_STATUS_NEEDS_ATTENTION = 8`.
  (`aida-server/src/generated/aida.rs` regenerates from this on build.)

### `aida-core/src/models.rs` — core types

- `enum RequirementStatus`: add `NeedsAttention` variant.
- `impl Display for RequirementStatus`: `NeedsAttention => "Needs Attention"`.
- `Requirement::set_status_from_str`: add `"needsattention"` arm.
- New `enum PuntCategory` (`DesignFork`/`AmbiguousSpec`/`MissingContext`/
  `BlockedDependency`/`Other`) + `Display` (kebab-case) + `from_str` + `all()`,
  `#[serde(rename_all = "kebab-case")]`, `TS` derive, obstacle-type `//` comment.
- New `struct AttentionReason { category, detail, lean, raised_by, raised_at }`
  with `Serialize/Deserialize/TS` derives.
- `struct Requirement`: add `attention_reason: Option<AttentionReason>`
  (`#[serde(default, skip_serializing_if = "Option::is_none")]`); update every
  constructor (`Requirement::new`, `Default`, test fixtures).
- New free fn `forbidden_attention_transition(from, to) -> Option<String>`.

### `aida-core/src/db/sqlite_backend.rs` / `postgres_backend.rs`

- `status_to_str` / `str_to_status`: add `NeedsAttention` ⇄ `"Needs Attention"`.

### `aida-core/src/import.rs`

- `KNOWN_STATUSES`: add `"Needs Attention"`; `parse_requirement_status`: arm.

### `aida-core/src/ai/prompts.rs`

- Lifecycle-description prose: mention NeedsAttention as the punt/pause state.

### `aida-cli/src/status_display.rs`

- `status_glyph`: `"needsattention" => "⚠"`; `paint_status`: bold magenta.

### `aida-cli/src/punt.rs` — NEW

- `struct PuntRecord` (ledger row) + `append_to_ledger(root, &PuntRecord)`.
- `parse_punt_category(&str) -> Result<PuntCategory, String>` (CLI parser).
- Unit tests for ledger append + category parse.

### `aida-cli/src/cli.rs`

- `enum Command`: add `Punt { id, category, reason, lean }`.
- `--status` help text: include `needs-attention`.

### `aida-cli/src/main.rs`

- `mod punt;`.
- `validate_status_input` + `parse_status`: add NeedsAttention arms.
- `RequirementStatus` `match` arms (compiler-forced): the two colored-status
  displays, `rework_smart_target` (→ InProgress), `classify_progress_bucket`,
  the status help-text match, `is_terminal_status` (NO new arm — stays
  Completed/Rejected).
- `aida edit` / `aida add` status apply: call `forbidden_attention_transition`
  before `set_status_from_str`; clear `attention_reason` on transition-out.
- `QueueCommand::Next`: filter out NeedsAttention specs (separate from terminal).
- New `fn handle_punt_command`: load spec, enforce transition, set status +
  `attention_reason`, persist, append ledger, print next-step hint.
- `handle_findings_command` `List`: add a "Punts awaiting triage" section
  (NeedsAttention specs + category/detail/lean) and fold into the count.
- `aida show`: render an `attention_reason` block when status is NeedsAttention.

### `aida-cli/src/mcp.rs`

- `parse_status`, `by_status` counts, resource help text: NeedsAttention.

### `aida-cli/src/prompts.rs`

- `status_options`: add `NeedsAttention` (and the long-missing `Done`).

### `aida-cli/src/session.rs`

- `spec_status_in_flight` `match` (compiler-forced): NeedsAttention → in-flight
  (it represents started-but-unmerged work).

### `aida-server/src/convert.rs` / `rest.rs`

- `status_to_proto` / `proto_to_status` `match` (compiler-forced); `parse_status`.

### `aida-core/templates/skills/aida-punt.md` + `commands/aida-punt.md` — NEW

- Skill: classify the fork → obstacle category → `aida punt …` → return control.
- Run `make sync-templates` to create the `.claude/` symlinks.

### Docs

- `aida-core/templates/docs/aida-discipline/lifecycle-vocabulary.md`,
  `docs/lifecycle.md`, `README.md` "Spec lifecycle": add NeedsAttention.
- `CLAUDE.md`: "31 skills" → "32 skills".

### TypeScript / React (best-effort completeness)

- `aida-web-react/src/lib/constants.ts` (`STATUS_ORDER`, `STATUS_CONFIG`),
  `StatusChart.tsx`, `CreateRequirementModal.tsx`: add NeedsAttention.

## Critical Files

- `proto/aida.proto`
- `aida-core/src/models.rs`
- `aida-core/src/db/sqlite_backend.rs`, `postgres_backend.rs`, `import.rs`
- `aida-cli/src/punt.rs` (new), `cli.rs`, `main.rs`, `status_display.rs`
- `aida-cli/src/mcp.rs`, `prompts.rs`, `session.rs`
- `aida-server/src/convert.rs`, `rest.rs`
- `aida-core/templates/skills/aida-punt.md`, `commands/aida-punt.md`
- `docs/lifecycle.md`, `aida-core/templates/docs/aida-discipline/lifecycle-vocabulary.md`

## Reusable helpers (do not reimplement)

- `validate_status_input` / `parse_status` (`aida-cli/src/main.rs`) — canonical
  status parsing; extend, don't fork.
- `status_glyph` / `paint_status` / `status_badge` (`status_display.rs`) — the
  single status-palette source of truth.
- `findings::build_findings_view` (`aida-cli/src/findings.rs`) — the existing
  triage view; the punt section composes alongside it in the handler.
- `find_project_root` (`aida-cli/src/main.rs`) — locate `.aida/` for the ledger.
- `not_found::requirement_not_found` — consistent ID-not-found errors.
- `backend.get_requirement_by_spec_id` / `update_requirement` — full-spec R/W.

## Risks + gotchas

1. **Risk**: a missed string-parse table silently accepts `needs-attention` as
   a `custom_status`. **Mitigation**: the exhaustive audit drove the file list;
   `match`-on-enum sites are compiler-forced; string tables are each in Files.
2. **Risk**: `aida-cli/src/generated/aida.rs` only regenerates under
   `--features remote`. **Mitigation**: it is unused under default features;
   hand-add the variant for `remote`-build consistency.
3. **Risk**: old YAML without `attention_reason` fails to deserialize.
   **Mitigation**: `#[serde(default)]` on the field.
4. **Risk**: punt during a headless drain stalls the orchestrator.
   **Mitigation**: NeedsAttention is not `auto_complete_head_drivable`, so the
   orchestrator skips it; `aida punt` returns 0 and control.
5. **Risk**: transition enforcement regresses existing free-form status edits.
   **Mitigation**: `forbidden_attention_transition` returns `None` for every
   edge that does not touch NeedsAttention.

## Tests (named)

- `needs_attention_status_round_trips` — Display / set_status_from_str / parse.
- `forbidden_attention_transition_into_only_from_in_progress`.
- `forbidden_attention_transition_out_only_to_approved_in_progress_rejected`.
- `forbidden_attention_transition_none_for_unrelated_edges` — no regression.
- `punt_flips_status_and_writes_attention_reason`.
- `punt_rejected_when_spec_not_in_progress`.
- `punt_appends_ledger_record` — `.aida/punts.jsonl` line shape.
- `parse_punt_category_accepts_kebab_and_rejects_garbage`.
- `status_glyph_for_needs_attention` / `paint_status_needs_attention`.
- `queue_next_skips_needs_attention_specs`.
- `findings_list_surfaces_needs_attention_punts`.

## Verification

```bash
TMP=$(mktemp -d); cd "$TMP" && git init -q && aida init >/dev/null
aida add --title "punt smoke" --type story --status approved >/dev/null
SID=$(aida list --json | python3 -c 'import sys,json;print(json.load(sys.stdin)[0]["spec_id"])')
aida edit "$SID" --status in-progress
aida punt "$SID" --category design-fork --reason "two valid auth flows" --lean "OAuth"
aida show "$SID" | grep -i "Needs Attention"          # status flipped
aida show "$SID" | grep -i "design-fork"              # reason rendered
test -s .aida/punts.jsonl                              # ledger written
aida findings list | grep -i "Punts awaiting triage"   # triage surface
aida edit "$SID" --status in-progress                  # resolve
aida show "$SID" | grep -iv "design-fork"              # metadata cleared
# negative: punt from a non-in-progress spec must fail
aida add --title "draft" --type task --status approved >/dev/null
DID=$(aida list --status approved --json | python3 -c 'import sys,json;print(json.load(sys.stdin)[-1]["spec_id"])')
aida punt "$DID" --category other --reason "x" && echo "BUG: punt should have failed"
```

## Followups

- STORY-325: full punt ledger — analysis, classification, `aida punt analyze`.
- STORY-276/STORY-306: consume `/aida-punt` from the headless implementer / advisor escalation tier.
- A `blocked-dependency` punt should auto-suggest filing a blocked-by relationship.
- TASK-338 machinery glossary: add NeedsAttention once the glossary file exists.

## Related

- Blocks: STORY-276, STORY-306
- Composes with: STORY-287, STORY-325, EPIC-28, TASK-338
- See also: `docs/lifecycle.md`, `docs/autonomous-drain.md`
