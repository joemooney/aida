# STORY-1674: write provenance and session-ledger view

<!-- trace:STORY-1674 | ai:codex -->

Advisor signed the B+C sketch on 2026-10-15 at 08:35 MST. The authoritative
ruling remains the STORY-1674 comment in AIDA. This plan records the inspected
implementation seams; it grants no review, merge, or launcher authority.

## Contract

Extend `aida usage show <spec> --provenance` with human, JSON and TOON output.
Each write event reports author, timestamp, exact session identity, explicitly
reported harness version, and available session token/model/effort evidence.
Only `AIDA_HARNESS_VERSION` may supply the harness version. Absence is unknown.
Join ledger rows by exact session identity and timestamps at or before the
write. Never infer model/effort from a vendor or borrow another session's data.
Do not create another ledger or stamp cumulative token counts into spec rows.

## Inspected seams

- `aida-core/src/models.rs`: `FilingProvenance.session` exists;
  `Comment.session_id` exists. Contrary to the sketch's assumption,
  `HistoryEntry` currently has NO session field. Add optional serde-default
  session/harness fields without retro-attributing old rows. Update struct
  literals and frontend bindings where necessary.
- `aida-core/src/provenance.rs`: existing `capture_in`, `capture`,
  `stamp_with`, `preserve_from_disk` implement write-once filing provenance.
  Add harness version there; session-only edit context must avoid Git probes.
- `aida-core/src/db/git_backend.rs`: creations enter `add_requirement`,
  full-save creation, atomic-store creation, and transactions. Updates converge
  on `stage_requirement_update`, except transaction writes. Inspect and cover
  all of these; stamping only CLI edit/comment handlers is insufficient.
- Legacy Storage/YAML/SQLite paths require the same missing-field behavior.
  Preserve historical metadata on retries/full saves; never stamp existing old
  events with the current process's identity. Direct writes without an added
  history event need a current write event rather than rewriting old history.
- `aida-cli-lib/src/cli.rs`: `UsageCommand::Show`; dispatch occurs in BOTH
  `lib_part1.rs` and `git_backend_cmd.rs`. Keep ordinary usage/cost unchanged.
- `aida-cli-lib/src/token_ledger.rs`: schema v1 `usage_record` has
  `vendor_session_id`, `occurred_at`, `raw_model`, measured token classes and
  raw vendor usage JSON. `attribution` has `aida_session_id`. There is NO
  dedicated effort column. Report effort unknown unless an explicit,
  documented ledger field supplies it; never inspect transcripts here.

## Join and presentation

Read the existing derived ledger without rebuilding or mutating it. A missing
ledger still permits displaying write metadata with unknown token/model/effort
fields. Read-only SQLite open avoids accidentally creating an empty database.
Match the event's exact identity to recorded identity fields (do not use prefix
matches). Parse timestamps as instants rather than comparing RFC3339 text,
which can differ by offset. Exclude absent/invalid or future timestamps. Keep
unknown token classes distinct from zero. If multiple models occurred in the
session before a write, show the actual set rather than choosing arbitrarily.
Clearly label cumulative session-at-write usage as overlapping between events;
it must never be summed as per-event or per-spec incremental cost.

## Verification

1. Legacy model YAML loads with unknown context; current fields roundtrip.
2. Explicit/absent/blank harness versions; no vendor-derived version.
3. Canonical create, edit, comment, reply, atomic update, full-save and
   transaction paths persist current identity, preserve old metadata, and do
   not create duplicate events for no-op writes.
4. CLI and MCP writes reach the shared seam; include actual persisted evidence.
5. In-memory ledger fixture: exact session, other-session decoy, future row,
   timezone offset, missing timestamp, missing model/effort/token classes.
6. Human/JSON/TOON query surfaces and CLI parsing; existing --cost behavior.
7. Required targeted tests, fmt, fast gate, compile gate for changed model
   literals/bindings; author self-check followed by a STRICT separate reviewer.

## State

Exploration only in Session3. No production implementation yet. Base:
`origin/main` at `3d85bbde84`; worktree `/home/joe/ai/aida-story-1674`,
branch `codex/story-1674`, lease `01a1403c9371`. Prior regen validation and
TASK-1734 rework take priority. Do not describe this plan as shipped acceptance.
