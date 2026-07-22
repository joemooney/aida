# Plan: TASK-1176 — a terminal `superseded` state for adopted-then-replaced specs

Date: 2026-07-22
Specs: TASK-1176 (follows BUG-751, BUG-781, BUG-784)
Status: Completed
Complexity: ~350 prod LOC, ~330 test LOC, 1 commit, risk medium (new enum variant on a serialized, cached, exhaustively-matched type)

## Approach

`RequirementStatus` had no honest state for a spec that was **adopted and then
replaced**. The nearest fit was `Rejected`, which means *declined* — so
downstream (quizdom) shipped `status=rejected` + a `superseded-by:ADR-N` string
tag, and an ADR followed for months rendered identically to one turned down.

This adds `RequirementStatus::Superseded` as a **general terminal status**
(not gated to the decision class) plus a **typed relationship pair**
(`SupersededBy` / `Supersedes`) to record the successor, and a
`aida edit <ID> --superseded-by <NEW-ID>` flag that writes both halves at once.
Everything else follows from being terminal: the existing `open_statuses()` /
`closed_statuses()` split already drives the default `aida list` lens, and
`State::is_terminal` already single-sources every "is this closed?" gate, so
joining those two sets is what excludes it from open/candidate surfaces.

This is a continuation of the BUG-751 → BUG-781 → BUG-784 decision-lifecycle
work, not a parallel implementation: it reuses `status_display`'s palette,
`glyphs::Glyph`'s registry, `lifecycle::State::is_terminal`, and the
`fast_status_counts` open tally those changes established.

### Diagram

```
                      declined
   Draft ──────────────────────────► Rejected  ✗ red      terminal
   Approved ────────────────────────►
                                                          (both drop out of the
   Approved ────────────────────────► Superseded ⊡ dim-green   default open lens,
        --status superseded                                    the open tally, and
        --superseded-by <NEW-ID>            │                  candidate/ready)
        (the flag implies the status)       │
                                            └── SupersededBy ──► NEW-ID
                                                 (reciprocal Supersedes written back)
```

## Decisions

- **Decision 1 — `Superseded` is a GENERAL status, not gated to decision-class
  types.** **Rationale**: the honesty gap ("adopted then replaced" vs
  "declined") is not specific to ADRs — a TASK replaced by a broader STORY is
  the identical story. Gating would mean a *new* type-class predicate and a new
  refusal path for zero correctness gain, i.e. more special-casing, which is
  what the spec explicitly asked to avoid. The one type where the transition is
  meaningless — `Epic` — is *already* refused, for free, by the BUG-626
  read-only-rollup guard (`manual_epic_status_edit_forbidden`). Contrast with
  BUG-781's `Accepted`, which HAD to be a display-only, type-aware relabel
  because it re-uses a SHARED stored value (`Approved`); `Superseded` is its own
  stored value, so it needs no type-aware rendering at all.
- **Decision 2 — ship `superseded` only; `deprecated` is a followup.**
  **Rationale**: `deprecated` is *not* nearly free once superseded exists. It is
  a genuinely different concept ("still true, discouraged, no successor named"),
  and it is arguably NOT terminal — a deprecated ADR may still be in force. That
  is a lifecycle-semantics question worth its own spec, and folding it in would
  materially widen this change (a second variant through every exhaustive match
  plus an undecided terminal-or-not answer). Filed under **Followups**.
- **Decision 3 — `--superseded-by <ID>` IMPLIES `--status superseded`.**
  **Rationale**: naming a successor *is* the supersede move. Letting the link
  and the status be set independently is exactly how the two halves drift apart
  — the failure mode the string-tag workaround already exhibits. An explicit
  `--status` still wins, so a recovery edit (`--status approved
  --superseded-by X`) stays expressible.
- **Decision 4 — a typed relationship pair, written bidirectionally.**
  **Rationale**: `RelationshipType::Custom("superseded-by")` would be no better
  than the tag — the graph traversals do not follow `Custom` edges. A typed pair
  with a declared `inverse()` makes both "what replaced this?" and "what did
  this replace?" walkable, and `add_superseded_by_edge` is deliberately shaped
  like the existing `add_blocked_by_edge` so the two bidirectional-edge writers
  stay recognizably the same code.
- **Decision 5 — glyph `⊡` / ASCII `[=]`, dimmed closed-green.** **Rationale**:
  the box family says *adopted* (cf. BUG-781's `☑ Accepted`); the dot instead of
  a check says *no longer the live copy*. Green (not red) is the load-bearing
  choice — red is the thing that made a superseded spec read as declined.
- **Decision 6 — a superseded child is RESOLVED in the epic rollup.**
  **Rationale**: identical reasoning to BUG-764's treatment of rejected
  children. A superseded child can never transition again, so leaving it in the
  open denominator would pin a fully-delivered epic at In Progress forever.
- **Decision 7 — `NeedsAttention → Superseded` stays forbidden.**
  **Rationale**: the punt gate's contract is "triage first" (out only to
  Approved / In Progress / Rejected). Widening it here would be scope creep on
  an unrelated invariant; triage the punt, then supersede.

## Files (in build-order)

### `aida-core/src/models.rs` — the enum + the relationship pair

- `enum RequirementStatus`: add `Superseded`.
- `impl Display for RequirementStatus`, `fn from_filter_str`, `fn cache_key`,
  `fn closed_statuses` (`[_; 3]` → `[_; 4]`), `fn set_status_from_str`: add the
  arm. `open_statuses` deliberately UNCHANGED — that omission is what excludes
  it from the default lens.
- `enum RelationshipType`: add `SupersededBy` + `Supersedes`; wire
  `Display`, `fn from_str` (aliases `replaced-by` / `replaces`), `fn inverse`,
  `fn name`.

### `aida-core/src/lifecycle.rs` — the declared state machine

- `enum State`: add `Superseded`; `fn from_status_str`, `fn from_status`,
  `fn label`, `fn entry_trigger` (Cli).
- `fn State::is_terminal`: include `Superseded` — this single edit is what makes
  every downstream "is it closed?" gate agree.
- `fn LifecycleModel::declared`: add `Approved → Superseded`.
- `fn to_mermaid` terminal-edge list + `fn states_in_order`: add `Superseded`.

### `aida-core/src/graph_walk.rs`, `aida-core/src/rollup.rs`, `aida-core/src/db/cache.rs` — the epic rollup

- `struct StatusRollup`: add `superseded: usize`.
- `fn status_rollup` / `fn tally_status_str`: bucket it.
- `fn derive_epic_status_from_rollup`: subtract it from the open denominator
  alongside `rejected`.
- `fn is_terminal_epic_status`: include it.
- `fn edge_weight`: weight the supersede edges like parent/child (2).

### `aida-core/src/db/sqlite_backend.rs`, `postgres_backend.rs`, `import.rs` — legacy string maps

- The `status → &str` / `&str → status` tables: add the arm each way.

### `proto/aida.proto`, `aida-server/src/convert.rs`, `aida-server/src/rest.rs` — the gRPC/REST surface

- `enum RequirementStatus`: `REQUIREMENT_STATUS_SUPERSEDED = 9` (additive).
- `fn status_to_proto` / `fn proto_to_status`: map it. The relationship pair
  follows the STORY-333 precedent and wires as `Custom`-with-name.

### `aida-cli-lib/src/cli.rs` — the flag

- `Command::Edit`: add `superseded_by: Option<String>` (`--superseded-by`), and
  extend the `--status` help text. Plain `//` for the trace marker so the
  SPEC-ID never reaches `--help`.

### `aida-cli-lib/src/lib.rs` — parsers, predicates, the edge writer

- `const VALID_STATUS_INPUTS` (new): one canonical list both refusal messages
  echo, so `validate_status_input` and `validate_status_input_for_type` cannot
  disagree.
- `fn parse_status`, `fn validate_status_input`: add the arm.
- `fn is_terminal_status_str`: add `superseded`.
- `fn fast_status_counts`: treat it as terminal (the `aida status` open tally).
- `fn add_superseded_by_edge` (new): bidirectional, idempotent, self-edge
  rejected — modelled on `fn add_blocked_by_edge`.
- `fn relationship_phrase`, `fn rel_type_label`, `const STANDARD_REL_TYPES`.

### `aida-cli-lib/src/git_backend_cmd.rs` — the edit handler + `rel add`

- `Command::Edit` arm: destructure `superseded_by`; shadow `status` with the
  implied `"superseded"` when the flag is present and `--status` is not; extend
  `no_field_flags` and the "No changes specified" guard; call
  `add_superseded_by_edge` AFTER the scalar save (same ordering rule the
  blocked-by block documents).
- `Command::Rel Add` type map: accept `superseded-by` / `supersedes`.

### `aida-cli-lib/src/glyphs.rs`, `status_display.rs` — rendering

- `enum Glyph`: add `Superseded` (`⊡` / `[=]`), name `superseded`, `ALL` 27→28.
- `fn status_glyph_for_profile`, `fn status_glyph_literal`, `fn paint_status`:
  add the `"superseded"` arm (dimmed green).

### `aida-cli-lib/src/mcp.rs`, `schema.rs` — the mirrored agent surfaces

- `fn parse_status` + the four status `enum` arrays in the tool schemas.
- `fn parse_rel_type` + the `add_relationship` description.
- `schema.rs` FieldDoc prose + the two reflection drift-guard expectations.

### Remaining exhaustive matches (compiler-enumerated, each handled explicitly)

- `queue_cmd.rs`: `fn rework_smart_target` (→ `None`; the successor is the
  rework target, not this record), the unqueued-spec explainer (points at the
  successor rather than offering Rejected's re-open).
- `zen_drive.rs` + its caller: `ZenEligibility::Superseded` with its own
  refusal text ("a later spec replaced it"), NOT folded into `Rejected`.
- `lib.rs`: `fn preflight_spec_status`, the two legacy status colour tables.
- `help_next.rs`: `fn state_token`, `fn rank`, `fn transition_command` (the
  nudge carries `--superseded-by <NEW-ID>`).
- `aida-tui/src/redesign/store.rs`: `fn graph_from_relationships` surfaces the
  lineage in the preview modal's related group.

### Docs

- `docs/lifecycle.md` (regenerated mermaid pin via `aida lifecycle --diagram
  --check --write`, plus a new "two terminal off-ramps" section and a glossary
  row), `README.md` "Spec lifecycle", `CLAUDE.md` status list + ADR bullet,
  `aida-core/templates/docs/aida/discipline/lifecycle-vocabulary.md`.

## Critical Files

- `aida-core/src/models.rs`
- `aida-core/src/lifecycle.rs`
- `aida-core/src/rollup.rs`
- `aida-core/src/graph_walk.rs`
- `aida-core/src/db/cache.rs`
- `aida-core/src/db/sqlite_backend.rs`
- `aida-core/src/db/postgres_backend.rs`
- `aida-core/src/import.rs`
- `proto/aida.proto`
- `aida-server/src/convert.rs`
- `aida-server/src/rest.rs`
- `aida-cli-lib/src/cli.rs`
- `aida-cli-lib/src/lib.rs`
- `aida-cli-lib/src/git_backend_cmd.rs`
- `aida-cli-lib/src/glyphs.rs`
- `aida-cli-lib/src/status_display.rs`
- `aida-cli-lib/src/queue_cmd.rs`
- `aida-cli-lib/src/zen_drive.rs`
- `aida-cli-lib/src/help_next.rs`
- `aida-cli-lib/src/relationship_cmd.rs`
- `aida-cli-lib/src/mcp.rs`
- `aida-cli-lib/src/schema.rs`
- `aida-cli-lib/src/tests/task_1176_superseded_state_tests.rs` (new)
- `aida-tui/src/redesign/store.rs`
- `docs/lifecycle.md`
- `README.md`
- `CLAUDE.md`
- `aida-core/templates/docs/aida/discipline/lifecycle-vocabulary.md`
- `docs/plans/2026-07-22-task-1176-superseded-decision-state.md` (new)

## Reusable helpers (do not reimplement)

- `lifecycle::State::is_terminal` (`aida-core/src/lifecycle.rs`) — THE single
  source for "is this closed?". `is_terminal_status` in the CLI delegates to it;
  editing it once propagates to the archive invariant, the BUG-64 parent guard,
  and the diagram.
- `RequirementStatus::open_statuses` / `closed_statuses` /
  `expand_filter_token` (`aida-core/src/models.rs`) — the default `aida list`
  lens is a POSITIVE `open_statuses()` filter, so exclusion is achieved by NOT
  adding the variant there. No new lens code.
- `lifecycle::is_work_item_type` / `is_work_item_type_str` — BUG-784's
  candidate/ready type filter; the burndown + backlog selectors additionally
  filter on approved+pickable status, so a superseded spec is already excluded.
  No change needed there.
- `add_blocked_by_edge` (`aida-cli-lib/src/lib.rs`) — the bidirectional,
  idempotent edge-writer shape `add_superseded_by_edge` mirrors.
- `status_display::{status_glyph_for_profile, paint_status, status_badge}` +
  `glyphs::Glyph` — the shared palette/registry; add an arm, don't add a
  renderer.
- `rollup::derive_epic_status_from_rollup` — the BUG-626/BUG-764 epic rollup;
  extend the resolved-children subtraction, don't add a second derivation.
- `validate_status_input_for_type` — BUG-751's type-aware wrapper (the
  `accepted` → `Approved` alias); it delegates to `validate_status_input`, so
  one new arm serves both.

## Risks + gotchas

1. **Risk**: a new enum variant on a type that is serialized to YAML, projected
   into SQLite, and exhaustively matched in ~30 places — a missed site
   mis-buckets the state silently. **Mitigation**: no catch-all `_ =>` arm was
   added anywhere; every site the compiler flagged was handled explicitly, and
   each got a `trace:TASK-1176` marker so the set is greppable.
2. **Risk**: forward compatibility — an OLDER binary reading a `Superseded`
   YAML. **Mitigation**: `RequirementStatus` has no forgiving deserializer, so
   an old binary errors on that ONE spec rather than mis-reading it; that is the
   pre-existing contract for every status added since (Done, NeedsAttention).
   `RelationshipType` *does* have the BUG-251 forgiving deserializer, so an old
   binary degrades a supersede edge to `Custom("SupersededBy")` and preserves it
   rather than failing the parse.
3. **Risk**: the proto enum is a wire contract. **Mitigation**: added as a NEW
   tag (`= 9`), never renumbering an existing one — additive and
   backward-compatible for both directions.
4. **Risk**: `--superseded-by` implying a status change is a surprising side
   effect. **Mitigation**: documented in `--help`, in `docs/lifecycle.md`, and in
   CLAUDE.md; an explicit `--status` always wins.
5. **Risk**: the epic rollup denominator (`total - rejected - superseded`)
   under-flowing or closing an epic that still has open work. **Mitigation**:
   `superseded_child_does_not_mask_open_work` pins the negative cases, including
   the all-superseded case returning `None` (don't auto-close).
6. **Gotcha**: `docs/lifecycle.md`'s committed mermaid block was ALREADY drifted
   from the declared model before this change (it predated the NeedsAttention
   and off-mainline edges). `aida lifecycle --diagram --check --write`
   regenerates the whole block, so the diff is larger than this spec alone —
   that is the pin doing its job, not scope creep.

## Tests (named, not "add tests")

- `db::cache::tests::superseded_round_trips_through_yaml_and_cache` — the
  persistence round-trip: serde YAML (writer of record) → typed variant →
  SQLite projection → `--status superseded` filter, WITHOUT sweeping up the
  Rejected sibling.
- `models::tests::relationship_type_superseded_by_is_typed_and_has_an_inverse`
  — the link is first-class: every CLI spelling lands on the typed variant (not
  `Custom`), the inverse pair round-trips, and it survives serde.
- `models::tests::expand_filter_token_handles_aliases` (extended) — `closed`
  expands to include `Superseded`.
- `rollup::tests::superseded_child_is_resolved_in_the_epic_rollup` — a
  superseded child does not corrupt the BUG-626 epic rollup.
- `rollup::tests::superseded_child_does_not_mask_open_work` — the guard: it
  must not close an epic that still has open work.
- `task_1176_superseded_state_tests::superseded_parses_validates_and_is_advertised`
- `task_1176_superseded_state_tests::superseded_is_terminal_like_completed_and_rejected`
  — the open-lens exclusion, via the open/closed set split that drives it.
- `task_1176_superseded_state_tests::superseded_is_not_open_work_in_status_counts`
  — the `aida status` open tally.
- `task_1176_superseded_state_tests::superseded_renders_distinctly_from_rejected`
  — THE acceptance test: different glyph, different colour, honest label.
- `task_1176_superseded_state_tests::superseded_glyph_is_profile_aware_and_distinct_in_ascii`
- `task_1176_superseded_state_tests::superseded_by_is_a_typed_edge_not_a_custom_string`
- `task_1176_superseded_state_tests::superseded_is_a_declared_transition_with_a_successor_carrying_nudge`
- `lifecycle::tests::mermaid_is_a_state_diagram` (extended) — the state appears
  in the generated diagram with a terminal edge.
- `schema::tests::schema_enums_match_reflection` (extended) — the reflection
  drift-guard for both the status and relationship token lists.

## Verification

```bash
AIDA_BIN="$(git rev-parse --show-toplevel)/target/debug/aida"
TMP=$(mktemp -d); cd "$TMP" && git init -q && git commit -q --allow-empty -m init
"$AIDA_BIN" init --no-skills --no-hooks --no-agent-config >/dev/null

export AIDA_SESSION_ROLE=advisor
"$AIDA_BIN" add --title "use widget A" --type decision --status approved
"$AIDA_BIN" add --title "use widget B instead" --type decision --status approved

# The whole move: one flag records BOTH the status and the successor.
"$AIDA_BIN" edit ADR-1 --superseded-by ADR-2        # expect: "Superseded by: ADR-2"

# 1. terminal-but-adopted rendering, distinct from Rejected
"$AIDA_BIN" show ADR-1 | grep -i status             # expect: ⊡ Superseded (NOT Rejected)

# 2. excluded from the default open lens; reachable when asked for
"$AIDA_BIN" list | grep -c ADR-1                    # expect: 0
"$AIDA_BIN" list --status superseded | grep -c ADR-1  # expect: 1
"$AIDA_BIN" list --all | grep -c ADR-1              # expect: 1

# 3. the link is a first-class typed edge, walkable in BOTH directions
"$AIDA_BIN" show ADR-1 | grep "is superseded by"    # expect: is superseded by ADR-2
"$AIDA_BIN" show ADR-2 | grep "supersedes"          # expect: supersedes ADR-1

# 4. the state machine is documented and pinned
"$AIDA_BIN" schema requirement | grep superseded    # expect: in BOTH the status and relationship token lists
cd - >/dev/null && "$AIDA_BIN" lifecycle --diagram --check   # expect: pin OK
```

## Followups

- Add a `deprecated` terminal-or-not state for specs discouraged without a named successor.
- Teach `aida archive --older-than` to include superseded in its default status csv.
- Add a dedicated `aida graph <ID> --superseded-by` traversal mode.
- Let `aida why` on a superseded spec name its successor as the next action.

## Related

- Builds on: BUG-751 (approved = accepted for decisions), BUG-781 (accepted decisions render terminal and leave the open lens), BUG-784 (knowledge-class types omitted from candidate/ready), BUG-764 (resolved children excluded from the epic rollup denominator)
- See also: `docs/lifecycle.md`, `aida-core/templates/docs/aida/discipline/lifecycle-vocabulary.md`
