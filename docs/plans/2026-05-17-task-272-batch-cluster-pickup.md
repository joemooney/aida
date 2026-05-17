# Plan: TASK-272 — /aida-pickup batch-context detection + cluster-mode continuation

Date: 2026-05-17
Specs: TASK-272
Status: Completed
Complexity: ~40 prod LOC, ~50 test LOC, 1 commit, risk low

<!--
  Skill-markdown changes (aida-pickup.md, aida-pr.md) are the bulk of the
  user-visible behavior; the Rust change is a single persisted manifest field.
-->

## Approach

`aida queue work --batch NAME` resolves the batch head and launches an ordinary
item-mode session — the batch name is dropped before the session manifest is
written, so `/aida-pickup` has no way to know it is mid-batch. This plan adds a
persisted `batch_name` field to the session manifest, threaded from the
`--batch` invocation through `handle_queue_work` into `write_queue_work_manifest`.
`aida session show --plan` prints the batch marker so the `/aida-pickup` and
`/aida-pr` skills can detect batch context with a single grep. The skill
markdown then gains: a `/aida-pickup --batch <NAME>` continuation form (next
batch member as commits on the *same* branch, no new worktree), a batch-aware
end-of-session next-steps template (cluster-continue is the primary option,
cluster-pr is option 2), and `/aida-pr` batch framing (title + summary name the
batch). When the batch is exhausted the menu reverts to the single-spec form.

### Diagram

```
  aida queue work --batch NAME
        │  resolves batch head, drops NAME today
        ▼
  handle_queue_work(batch_name = Some(NAME))   ← NEW: NAME threaded through
        │
        ▼
  write_queue_work_manifest → SessionManifest { batch_name: Some(NAME), .. }
        │
        ▼
  aida session show --plan  prints  "batch: NAME"
        │
        ├──► /aida-pickup  detects batch → batch-aware next-steps menu
        └──► /aida-pr      detects batch → batch-framed PR title/body
```

## Decisions

- **Persist the batch name in the session manifest, not the lease.** Rationale:
  the manifest already carries "what this session intends to do" (plan brief,
  planned items, claude_session_id); the lease is identity-only. `batch_name`
  is intent, so it belongs with the manifest — and the manifest is already the
  surface `/aida-pickup` reads via `aida session show --plan`.
- **Detection surface is `aida session show --plan` text, not a new command.**
  Rationale: the skill already runs `aida session show --plan` for the plan
  brief; adding a `batch:` line there means zero new CLI surface and one grep.
- **`/aida-pickup --batch <NAME>` continues on the same branch — no new
  worktree.** Rationale: the whole point of cluster mode is one branch / one PR;
  spawning `aida queue work` would create a sibling worktree and defeat it. The
  skill picks the next queued batch member directly (`aida queue list --batch`),
  marks it in-progress, and commits onto the current branch.
- **`aida session manifest write` preserves an existing `batch_name`.**
  Rationale: `/aida-pickup` Step 3a may rewrite the manifest's planned items
  mid-batch; that rewrite must not silently drop the batch marker.
- **Batch exhaustion is read from `aida queue list --batch NAME`, not the
  manifest.** Rationale: the queue is the live source of truth for "what's
  left"; the manifest records the *name* once and never needs item-level churn.

## Files (in build-order)

### `aida-cli/src/session_manifest.rs` — persisted field

- `struct SessionManifest`: add `batch_name: Option<String>` after
  `claude_session_id`, before `plan` (scalar must serialize before the
  `[plan]` table). `#[serde(default, skip_serializing_if = "Option::is_none")]`.
- Test helper `manifest()` + the three inline test constructors: add
  `batch_name: None`.
- New test `batch_name_survives_roundtrip`.

### `aida-cli/src/main.rs` — thread the name through

- `fn write_queue_work_manifest`: add `batch_name: Option<&str>` param; set it
  on the constructed `SessionManifest`.
- `fn handle_queue_work`: add `batch_name: Option<&str>` param; pass to
  `write_queue_work_manifest`; add `|| batch_name.is_some()` to the
  manifest-write guard so a `--batch` pickup always records the marker.
- `QueueCommand::Work` dispatch: pass `effective_batch` to `handle_queue_work`.
- `QueueCommand::Rework` `--work` chain: pass `None`.
- `fn session_manifest_write`: load any existing manifest first and carry its
  `batch_name` forward into the rewritten manifest.
- `fn render_session_manifest`: print a `batch:` line in the Plan header block
  when `manifest.batch_name` is set.

### `aida-core/templates/skills/aida-pickup.md` — batch-aware skill

- New "## Arguments" note: `--batch <NAME>` form alongside the SPEC-ID form.
- Step 1: detect batch context from `aida session show --plan`.
- New Step "Batch continuation": `/aida-pickup --batch NAME` picks the next
  queued batch member onto the same branch, no `aida queue work`.
- Step 6: new "Batch mode" next-steps template; revert to single-spec form when
  the batch is exhausted.

### `aida-core/templates/skills/aida-pr.md` — batch-framed PR

- Step 2 + Step 8: detect `batch:` from `aida session show --plan`; when set,
  the PR title/summary name the batch and the body bundles all batch members
  on the branch.

## Critical Files

- `aida-cli/src/session_manifest.rs`
- `aida-cli/src/main.rs`
- `aida-core/templates/skills/aida-pickup.md`
- `aida-core/templates/skills/aida-pr.md`

## Reusable helpers (do not reimplement)

- `session_manifest::load` / `save` (`aida-cli/src/session_manifest.rs`) — manifest I/O.
- `session_manifest::manifest_path` — resolves `.aida/sessions/<id>.manifest.toml`.
- `resolve_queue_work_batch` / `normalize_batch_name` (`aida-cli/src/main.rs`) — `--batch` parsing.
- `resolve_batch_members` (`aida-cli/src/main.rs`) — queued items tagged `batch:NAME`.

## Risks + gotchas

1. **Risk**: TOML serialization order — a scalar declared after the `[plan]`
   table or `[[items]]` array fails to round-trip. **Mitigation**: place
   `batch_name` immediately after `claude_session_id`, before `plan` (the
   existing comment on `claude_session_id` documents exactly this constraint).
2. **Risk**: `aida session manifest write` clobbers the batch marker when the
   skill rewrites planned items mid-batch. **Mitigation**: `session_manifest_write`
   loads the existing manifest and carries `batch_name` forward.
3. **Risk**: a `--batch` pickup with `--no-launch` and no plan context would
   skip the manifest write entirely. **Mitigation**: `|| batch_name.is_some()`
   in the write guard forces the manifest so the marker is always recorded.
4. **Risk**: skill behavior (menu rendering, cluster PR) is Claude-executed
   markdown — not unit-testable. **Mitigation**: Rust tests cover the
   detectable core (manifest persistence); the skill changes are verified by
   the executable smoke below.

## Tests (named, not "add tests")

- `batch_name_survives_roundtrip` — save a manifest with `batch_name`, load it,
  assert the value persists (covers acceptance "batch detection").
- `save_load_roundtrip` / `manifest()` helper updated so the new field compiles
  across the existing suite.

## Verification

```bash
cargo test -p aida-cli session_manifest
cargo build -p aida-cli

# Positive: a --batch session records the marker
TMP=$(mktemp -d); cd "$TMP" && git init -q && aida init --no-skills --no-hooks >/dev/null
aida add --title "batch member one" --type task --status approved >/dev/null
aida add --title "batch member two" --type task --status approved >/dev/null
# tag both into a batch, queue them, then `aida queue work --batch demo --no-launch`
# → .aida/sessions/<id>.manifest.toml contains  batch_name = "demo"
# → `aida session show --plan` prints a `batch:` line

# Negative: a non-batch pickup writes no batch_name
aida queue work TASK-1 --no-launch   # manifest has no batch_name key
```

## Followups

- Grow the manifest `items` list as `/aida-pickup --batch` picks up each member
  so `aida session show --plan` shows full cluster progress (today it shows the
  batch head + the `batch:` marker; remaining members are read live from the
  queue). File as a TASK if the `--plan` table proves insufficient in dogfood.

## Related

- TASK-260 — Path/Action/Why next-steps tables (this TASK extends the menu shape).
- TASK-229 — `batch:NAME` tag convention + `aida queue work --batch`.
- TASK-285 — `--batch --auto-complete` orchestrator drain (autonomous sibling).
- STORY-98 — session manifest / planned-cluster bookkeeping.
- TASK-95 — plan brief rides into the session via the manifest.
