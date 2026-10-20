# Store Writer Inventory (`TASK-1930` — `TASK-1332` Slice 1)

<!-- trace:TASK-1930 | ai:agy -->

## 1. Scope & Lock Hierarchy (`A1` Decision)

Every mutation of a live `.aida-store` checkout (working tree, index, `HEAD`, or `registry/blocks.yaml`) must serialize local readers/writers and respect a single global lock order so concurrent processes cannot interleave `git add` / `git commit` / `git pull --rebase` or deadlock across advisory lock files.

### Advisory Locks in `.aida-store`

1. **Store Write Lock (`store.lock`)**:
   - Path: `<store>/.aida/store.lock` (via `aida_core::db::with_store_write_lock`)
   - Protects: All `.aida-store` working-tree writes, git index staging (`git add`), commits (`git commit`), local rollbacks (`rollback_commit_if_head_matches`), and store rebases/merges (`pull_rebase`, `pull_rebase_auto_merge`, `merge_union_auto`, `fast_forward_store`).
   - Entry invariant: Calls `git_ops::ensure_store_write_safe(store_root)` after acquiring the flock (refusing mid-rebase, mid-merge, cherry-pick, or detached `HEAD` states).
   - Exit invariant: Network `push_exact` / `push` is **never** called while holding `store_write_lock` (`push_exact_outcome` enforces `!crate::db::any_store_write_lock_held()`).

2. **Agreed-ID Block Dispense Lock (`blocks.lock`)**:
   - Path: `<store>/registry/blocks.lock` (via `aida_core::BlockRegistry::with_dispense_lock`)
   - Protects: Read-modify-write of `<store>/registry/blocks.yaml`.

### Binding Lock Order (`A1`)

- **Always acquire `store_write_lock` BEFORE `blocks.lock` (`store.lock -> blocks.lock`).**
- **Never acquire `blocks.lock -> store.lock`.**
- **Never perform network I/O (`pull_rebase`, `push_exact`) while holding `blocks.lock`.**

---

## 2. Store Writer Classification Across `aida-core` and `aida-cli-lib`

### Category A: Already Protected (`TASK-1717` and Prior Slices)

| Crate / File | Function / Entrypoint | Lines | Protection Mechanism |
|---|---|---|---|
| `aida-core/src/db/git_backend.rs` | `GitBackend::save` / `save_locked` / `update_atomically` / `save_Streaming` | `580–880`, `1350–2510`, `3040–3060` | Acquires `with_store_write_lock(&self.root, ...)` around object writes, `oplog.yaml`, `metadata.yaml`, `registry/blocks.yaml`, and `git add` / `git commit`. |
| `aida-core/src/db/cached_git_backend.rs` | `CachedGitBackend::save` / `update_atomically` | delegates to `GitBackend` | Protected via `GitBackend::with_store_write_lock`. |
| `aida-core/src/git_ops.rs` | `pull_rebase`, `pull_rebase_auto_merge`, `merge_union_auto`, `fast_forward_store` | `1744`, `2366`, `4405` | Acquires `with_store_write_lock` (reentrant when caller already holds it), checks hypothesis evidence refusals before modifying refs. |
| `aida-core/src/git_ops.rs` | `merge_gate` / `merge_gate_locked` | `5958–6120` | Acquires `with_store_write_lock(store_path, \|\| merge_gate_locked(store_path))`. |
| `aida-cli-lib/src/db_cmd.rs` & `git_backend_cmd.rs` | `DbCommand::Sync` (`db_sync_local_window`) | `db_cmd.rs:563–650`, `git_backend_cmd.rs:1201–1306` | Runs `db_sync_local_window` under `with_store_write_lock`, captures `sha`, and runs `push_exact` outside the lock with locked `pull_rebase` retry. |
| `aida-cli-lib/src/store_push_pull.rs` | `commit_and_push_store`, `publish_mailbox_digest`, `push_store_leg`, `pull_store_leg` (`pull_store_leg_locked`) | `559–645`, `1592–1640`, `2317–2385`, `2790` | Acquires `with_store_write_lock` for local stage/commit/rebase windows and pushes outside the lock via `push_exact`. |
| `aida-cli-lib/src/store_sync.rs` | `sync_store_before_pickup`, `push_store_with_retry`, `pull_store_quiet` | `1110–1265`, `1640–1660` | Acquires `with_store_write_lock` around stage/commit and `pull_rebase`, then runs `push_exact_outcome` outside the lock. |
| `aida-cli-lib/src/mailbox_store.rs` | `write_snapshot` (`write_snapshot_locked`) | `138` | Acquires `with_store_write_lock(store_root, \|\| write_snapshot_locked(store_root, local))`. |
| `aida-cli-lib/src/timing_record.rs` | `record_timing_in_store` | `141–195` | Acquires `with_store_write_lock(store_root, ...)` around `timing/` JSONL append, `git_ops::add_all`, and `git_ops::commit_paths`. |
| `aida-cli-lib/src/regen/score.rs` | `commit_trial_record` | `817–838` | Acquires `with_store_write_lock(store, ...)` around `regen/trials/` write, `git_ops::add_all`, and `git_ops::commit_paths`. |
| `aida-cli-lib/src/child_store_commit.rs` | `commit_store_writes_since` (`commit_child_store_writes`) | `49–90` | Acquires `with_store_write_lock(store, ...)` (`BUG-2180`) around staging and committing deferred contained-session store writes. |
| `aida-cli-lib/src/remote_create.rs` | `reconcile_store_branch` | `2574–2660` | Acquires `with_store_write_lock(&store_dir, ...)` around `merge_union_auto` and publishes with `push_exact` outside the lock. |

---

### Category B: Newly Wrapped in `TASK-1930`

| Crate / File | Function / Entrypoint | Lines | `TASK-1930` Protection & Error Semantics |
|---|---|---|---|
| `aida-cli-lib/src/coordination.rs` | `acquire_claim`, `release_claim`, `mark_claim_merged`, `acquire_lock_claim`, `release_lock_claim`, `reap_stale_lock_claims` | `737–855`, `921–958`, `990–1025`, `1204–1325`, `1419–1456`, `1476–1508` | Each retry attempt wraps `refresh_store` + read/decide + file write/remove + `git add` + `git commit` + `head_sha` in `with_store_write_lock`, then calls `push_exact` outside the lock. On `Ok(false)` rejection, rolls back via `rollback_commit_if_head_matches` (which acquires `with_store_write_lock`) before retrying. **Fail-closed**: lock timeout, unsafe store (`ensure_store_write_safe`), file write error, `git add` error, and `git commit` error propagate as `Err`; only network `refresh_store` and network `push_exact` transport errors map to `Unavailable`. |
| `aida-cli-lib/src/schedule_ledger.rs` | `write_cas_opts`, `write_batch_cas_opts` | `371–440`, `542–606` | Each retry attempt wraps `pull_rebase_auto_merge` (when `attempt > 0`), fresh ledger load/merge (`reapply_delta`), `save`, `git add`, `git commit`, and `head_sha` in `with_store_write_lock`, then calls `push_exact` outside the lock with `rollback_commit_if_head_matches` on push rejection. |
| `aida-core/src/team.rs` | `register_member_cas`, `set_allowed_seats_cas`, `retire_seat_cas` | `240–285`, `320–378`, `400–445` | Each retry attempt wraps `pull_rebase` (when `attempt > 0`), `TeamRegistry::load`, mutation, `save`, `git add`, `git commit`, and `head_sha` in `with_store_write_lock`, then calls `push_exact` outside the lock with `rollback_commit_if_head_matches` on rejection. |
| `aida-core/src/alias.rs` | `link_cas` | `233–278` | Each retry attempt wraps `pull_rebase` (when `attempt > 0`), `AliasRegistry::load`, `link`, `save`, `git add`, `git commit`, and `head_sha` in `with_store_write_lock`, then calls `push_exact` outside the lock with `rollback_commit_if_head_matches` on rejection. |
| `aida-core/src/git_ops.rs` | `register_node_full`, `register_node_with_explicit_id`, `register_node_auto_next`, `ensure_node_identity`, `rename_node`, `retire_legacy_numeric_node`, `commit_pending_store_changes_on_host`, `squash_orphan_to_snapshot` | `4144–4200`, `4505–5140`, `6165–6205` | Wraps `registry/nodes.toml` load/modify/save + `git add` + `git commit` + `head_sha` (and `squash_orphan_to_snapshot` history squash) in `with_store_write_lock` and publishes outside the lock via `push_exact` with `rollback_commit_if_head_matches` on rejection. |
| `aida-cli-lib/src/init_cmd.rs` | `create_or_clone_distributed_store` (existing-remote push-race retry) | `5463–5475` | Wraps the retry `git_ops::pull_rebase(&store_dir, "origin", &branch)` in `with_store_write_lock` before re-pushing. |
| `aida-cli-lib/src/upgrade_release.rs` | `sync_store_during_upgrade` | `1763–1766`, `1876–1908` | Wraps `pull_rebase` and `git add` / `git commit` in `with_store_write_lock`, keeping `git push` outside the lock. |
| `aida-cli-lib/src/db_block.rs` | `handle_block_command` (`BlockCommand::Claim`), `auto_allocate_block_inner`, `claim_first_block`, `rollback_unpushed_claim` | `245–306`, `888–968`, `1248–1375` | Enforces `A1` lock order (`with_store_write_lock` -> `BlockRegistry::with_dispense_lock`), runs `pull_unless_refused` under `store_write_lock` **before** taking `blocks.lock`, and runs `push_exact` outside both locks. |
| `aida-cli-lib/src/requirements_cli.rs` | `run_add` (agreed-ID dispense from `registry/blocks.yaml`) | `3085–3245` | Enforces `A1` lock order (`with_store_write_lock` -> `BlockRegistry::with_dispense_lock`) around `BlockRegistry::load`, `dispense`, `save`, and `git_ops::add(store_path, &["registry/blocks.yaml"])`. |
| `aida-cli-lib/src/doctor_cmd.rs` | `doctor_verify_relationships`, `doctor_repair_stale_blocks`, `doctor_migrate_counter_scope` | `12835–12885`, `13290–13325`, `14010–14065` | Keeps read-only diagnosis and interactive `[y/N]` prompt outside the lock, then acquires `with_store_write_lock` (and `with_dispense_lock` inside it for the two block writers), **re-loads and re-validates** canonical objects / `blocks.yaml` / `nodes.toml` from disk inside the lock, writes, stages, and commits. |

---

### Category C: Private-Init / Scratch-Only (Unlocked by Design — Unpublished Directory)

These paths initialize a brand-new, private directory that has not yet been exposed to concurrent sessions or published to a shared store path:

| Crate / File | Function / Entrypoint | Lines | Notes |
|---|---|---|---|
| `aida-cli-lib/src/init_cmd.rs` | `init_distributed_store`, `create_or_clone_distributed_store` (initial seed) | `4400–4444`, `5422–5456` | Seeds `metadata.yaml`, `objects/.gitkeep`, `.gitignore`, and initial commit in a brand-new store directory. Once published/cloned, the push-race retry at `:5470` uses `with_store_write_lock`. |
| `aida-core/src/workspace.rs` | `init_workspace` | `210–231` | Initializes a fresh workspace store directory. |
| `aida-cli-lib/src/sandbox_cmd.rs` | `init_sandbox` | `263–270` | Initializes an isolated scratch sandbox store. |
| `aida-cli-lib/src/db_cmd.rs` | `DbCommand::ExportGit` | `852–880` | Exports into a caller-specified target output directory. |

---

### Category D: Read-Only / Fetch-Only (`FETCH_HEAD` or Private Ref Only)

These paths inspect the store or fetch remote refs into an invocation-private ref without mutating the store working tree, index, or `HEAD`:

| Crate / File | Function / Entrypoint | Lines | Notes |
|---|---|---|---|
| `aida-core/src/git_ops.rs` | `fetch_branch_to_private_ref`, `is_remote_reachable`, `resolve_store_hooks_dir` | `384–435`, `1660–1730` | Read-only git queries or private-ref fetches (`refs/aida-fetch/...`). |
| `aida-cli-lib/src/coordination.rs` | `list_claims`, `live_foreign_lock_claim` | `865–905`, `1340–1390` | Best-effort `fetch_branch_to_private_ref` / `refresh_store` read path; does not write or commit to the store. |
| `aida-cli-lib/src/doctor_cmd.rs` | `doctor_scrub_collisions`, `doctor_fsck`, `doctor_contradictions`, `doctor_check_store_free_light` | `1093`, `13326`, `13491`, `14123` | Read-only store audits. |
| `aida-cli-lib/src/db_block.rs` | `handle_db_check_collisions`, `BlockCommand::List`, `BlockCommand::Status` | `20–118`, `325–530` | Read-only audits and listings of `objects/` and `registry/blocks.yaml`. |

---

### Category E: Named Follow-Ups Remaining on Parent `TASK-1332` (Items 2–5)

These paths were identified during the inventory and remain on parent `TASK-1332`:

| Crate / File | Function / Entrypoint | Lines | Finding & Follow-Up Scope on `TASK-1332` |
|---|---|---|---|
| `aida-cli-lib/src/doc_cmd.rs` | `handle_doc_command` (`DocCommand::Add`) | `111–120` | Calls `backend.update_atomically(...)` (which writes the object under `with_store_write_lock`) and then redundantly calls `object_store::write_object(&store_path.join("objects"), &written)` outside the lock at line `119`. Remove redundant unlocked `write_object` or fold into the atomic update closure. |
| `aida-cli-lib/src/findings_gate.rs` | `file_finding`, `promote_finding`, `recur_finding` | `1314–1324`, `1665–1677`, `1848–1859`, `1903–1914` | Same pattern as `doc_cmd.rs`: calls `backend.update_atomically(...)` followed by an unlocked `object_store::write_object(&store_path.join("objects"), &written)`. |
| `aida-cli-lib/src/burndown_run.rs` | `file_integration_wait_parked_finding` | `1389–1399` | Same pattern as `doc_cmd.rs`: calls `backend.update_atomically(...)` followed by an unlocked `object_store::write_object(&store_path.join("objects"), &written)`. |
| `aida-cli-lib/src/lib_part2.rs` | `handle_retire_legacy_ids` (`aida db retire-legacy-ids`) | `15–90` | Calls `let store = backend.load()?;` outside the lock (`:22`), mutates in memory, and calls `backend.save(&mutable)?` (`:80`). Should re-read inside `with_store_write_lock` / `update_atomically` so concurrent spec edits between preview and save are not clobbered. |
| `aida-cli-lib/src/doctor_cmd.rs` | `doctor_heal` spec/queue mutation paths | `7389` | Uses `storage.load()` + `storage.save(&store)?`; follow up in `TASK-1332` to ensure heal re-reads under `with_store_write_lock` before writing. |
| `aida-core/src/git_ops.rs` & `aida-cli-lib/src/store_cmd.rs` | `opportunistic_store_gc`, `gc_aggressive` (`store_compact`) | `git_ops.rs:4019–4113`, `store_cmd.rs:85–134` | Runs `git gc --auto` / `git gc --aggressive` on the store object database outside `with_store_write_lock`; tracked under `TASK-1332` item 4 (competing GC/cache residuals). |

---

## 3. Reconciliation Against Advisor's 2026-10-12 Ruling (`TASK-1332` / `TASK-1-225`)

| 2026-10-12 Ruling Item | Sub-Item | Disposition in `TASK-1930` vs Parent `TASK-1332` |
|---|---|---|
| **(1) Unlocked shared-store writers** | `coordination.rs` `pull_rebase` (claim/lock-claim acquire, release, heartbeat/reap) | **Resolved in `TASK-1930`** (`coordination.rs:737–1508`), with fail-closed error propagation for local/lock errors. |
| **(1) Unlocked shared-store writers** | `schedule_ledger.rs` `pull_rebase_auto_merge` (`write_cas_opts`, `write_batch_cas_opts`) | **Resolved in `TASK-1930`** (`schedule_ledger.rs:371–606`), preserving original errors on unsafe store state. |
| **(1) Unlocked shared-store writers** | `aida-core` `team.rs` roster CAS rebases (`register_member_cas`, `set_allowed_seats_cas`, `retire_seat_cas`) | **Resolved in `TASK-1930`** (`aida-core/src/team.rs:240–445`). |
| **(1) Unlocked shared-store writers** | `aida-core` `alias.rs` `link_cas` | **Resolved in `TASK-1930`** (`aida-core/src/alias.rs:233–278`). |
| **(1) Unlocked shared-store writers** | `init_cmd.rs` existing-store retry (`create_or_clone_distributed_store`) | **Resolved in `TASK-1930`** (`aida-cli-lib/src/init_cmd.rs:5470`). |
| **(1) Unlocked shared-store writers** | `git_ops.rs` node registration / identity / hijack / rename / retire | **Resolved in `TASK-1930`** (`aida-core/src/git_ops.rs:4505–5140`). |
| **(1) Unlocked shared-store writers** | Doctor & compaction maintenance writers | **Direct `.aida-store` writers (`doctor_verify_relationships`, `doctor_repair_stale_blocks`, `doctor_migrate_counter_scope`, `squash_orphan_to_snapshot`) wrapped and re-validated inside lock in `TASK-1930`**; remaining `storage.save`-based maintenance paths (`handle_retire_legacy_ids`, `doctor_heal`, redundant `write_object` after `update_atomically` in `doc_cmd.rs`/`findings_gate.rs`/`burndown_run.rs`) inventoried in Category E above for `TASK-1332`. |
| **(2) A1 dispenser ordering** | `store_write_lock` before `blocks.lock` + barrier/order tests | **Enforced in `TASK-1930`** across `db_block.rs`, `requirements_cli.rs`, `doctor_cmd.rs`, and runtime guard in `aida-core/src/db/store_lock.rs` (`dispense_lock_held()`). Full parent `TASK-1332` closure remains open per operator ruling. |
| **(3) Conditional-push writers & A2 owner-join** | `BUG-1808` A2 common owner join | **Stays on `TASK-1332` / `BUG-1808`** per operator ruling (option b). |
| **(4) A4 residuals** | Private-ref `FETCH_HEAD`, rebase-abort/competing cache/GC review | **Stays on `TASK-1332`** per operator ruling (option b). |
| **(5) `TASK-1333` dependency** | Contained/hook profile dependency | **Stays on `TASK-1332` / `TASK-1333`** per operator ruling (option b). |

