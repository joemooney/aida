# STORY-733 — Compact the orphan aida-store branch (substrate tax relief)

- **Date**: 2026-06-30
- **Specs**: STORY-733
- **Status**: Implemented (safe path shippable; destructive `--squash` opt-in, supervised)
- **Complexity**: Medium

## Approach

The orphan `aida-store` branch has ~15.7k commits, never compacted — so every
gc-repack, push-reject rebase, and fetch pays for the full history. Two levers,
deliberately split by safety:

- **PRIMARY (safe, non-destructive):** a deep `git gc --aggressive` consolidates
  the full history + accumulated packs into efficient packs. Delivers most of
  the perf benefit WITHOUT rewriting history, breaking `aida history`, or a
  force-push. Wired two ways: an on-demand `aida store compact` (alias `aida
  store gc`) command, AND an opportunistic aggressive repack at the existing
  sync chokepoint (`opportunistic_store_gc`) when the pack count crosses a high
  threshold. Self-throttling: aggressive collapses many packs into one, so it
  won't re-fire until packs climb again.
- **SECONDARY (destructive, opt-in only):** `aida store compact --squash`
  collapses the orphan history to a single snapshot commit. Gated behind `--yes`
  (prints the plan otherwise), records a backup branch BEFORE any rewrite, and
  prints — never runs — the coordinated force-push. Never automatic.

```
opportunistic_store_gc (sync chokepoint)
  ├─ apply gc.auto config (TASK-1033)
  ├─ git gc --auto                          (cheap, every sync)
  └─ if packs >= AIDA_STORE_GC_AGGRESSIVE_PACKS: git gc --aggressive   (occasional)

aida store compact            → gc_aggressive(store)          [safe]
aida store compact --squash   → plan only                     [no --yes]
aida store compact --squash --yes
  ├─ create+verify backup branch  (BEFORE rewrite)
  ├─ commit-tree HEAD^{tree} → root snapshot
  ├─ reset --soft to snapshot
  ├─ gc --aggressive (reclaim)
  └─ print force-push command     (NEVER pushes)
```

## Decisions

- **`git count-objects -v`** parses pack/loose/in-pack counts — works regardless
  of the linked-worktree layout (the store worktree shares the common object dir).
- **Aggressive is pack-count-gated, not time-gated.** A high pack count is the
  direct symptom of an un-compacted store, and aggressive self-resets it.
- **`aida history --events` after a squash:** the pre-squash horizon is preserved
  on the backup branch (`aida-store-pre-squash-<ts>`). `aida history` reads the
  `aida-store` branch, so after a squash it sees only the snapshot; the full
  timeline is recoverable by pointing history tooling at the backup ref. This is
  a documented, honest truncation with a preserved horizon — not a silent loss.
- **Backup before rewrite is a hard invariant.** `squash_orphan_to_snapshot`
  creates AND verifies the backup ref points at HEAD before touching history; a
  backup-create failure bails with HEAD untouched.

## Files (build order)

- `aida-core/src/git_ops.rs` — `STORE_GC_AGGRESSIVE_PACKS_DEFAULT`,
  `resolve_store_gc_aggressive_packs`, `StoreObjectCounts` + `count_objects`,
  `gc_aggressive`, `SquashOutcome` + `squash_orphan_to_snapshot`; extend
  `opportunistic_store_gc` with the occasional aggressive leg.
- `aida-cli/src/cli.rs` — `StoreCommand::Compact { squash, yes }` (`gc` alias).
- `aida-cli/src/main.rs` — `store_compact` + `store_compact_squash` handlers.
- `docs/environment-variables.md` — `AIDA_STORE_GC_AGGRESSIVE_PACKS` row.

## Critical files

- `aida-core/src/git_ops.rs::squash_orphan_to_snapshot` — the destructive core;
  the backup-before-rewrite ordering is the safety invariant.
- `aida-core/src/git_ops.rs::opportunistic_store_gc` — the auto chokepoint; the
  aggressive leg must stay best-effort (never break a sync).

## Tests

- `resolve_store_gc_aggressive_packs_maps_overrides` — pure resolver + off/0 disable.
- `gc_aggressive_reduces_pack_count` — fixture with many packs, aggressive drops count.
- `count_objects_tracks_loose_and_packed` — loose then packed transition.
- `squash_creates_backup_then_collapses_to_root_snapshot` — backup records old
  head; HEAD becomes a single root commit; tree byte-identical.
- `squash_bails_without_rewrite_when_backup_ref_exists` — backup-create failure
  leaves HEAD untouched (the guard).

## Verification

```bash
cargo build -p aida-core -p aida-cli
env -u AIDA_SESSION_ROLE cargo test -p aida-core git_ops::tests
cargo fmt --all -- --check
cargo clippy --workspace -- -D clippy::correctness
bash scripts/glyph-lint.sh --block
aida store compact --help          # surface + gc alias
aida store compact --squash        # plan-only, zero changes
```

## Followups

- Consider an `aida history --events --pre-squash` flag that auto-resolves the
  newest `aida-store-pre-squash-*` backup ref as the walk root, so the timeline
  stays inspectable after a squash without manual ref juggling.
- A post-squash `aida store compact --drop-backup <ref>` once the truncation is
  accepted across all clones (reclaims the retained pre-squash objects).

## Related

- TASK-1033 / BUG-663 — store gc self-maintenance (gc.auto + autoDetach) this builds on.
