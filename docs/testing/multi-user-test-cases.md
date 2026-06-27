# AIDA multi-user / multi-clone test-case catalog

- **Status:** Living catalog. Phase 1 = same-host clones sharing one `aida-store` (this doc). Phase 2 = multi-host / multi-OS-user. Phase 3 = multi-repo→one-store (see SPIKE-62).
- **Purpose:** Enumerate, document, and track every multi-user interaction so we know what works, what's an accepted limitation, and what's a bug. Each case is written so it can later be lifted into an executable harness (bash or Rust integration test).
- **Grounded in code** (2026-06-16, four-pass exploration): every "current behavior" cites the controlling `file:symbol`. Verify against code before trusting — this drifts.

## How to read this

Each case: **ID · scenario · setup · steps · expected · validates · status · refs.**

Status legend:
- ✅ **works** — behavior is correct and (usually) has a test.
- 🟡 **works, untested** — behaved correctly in code-reading but no automated test pins it.
- ⚠️ **accepted limitation** — by-design today; documented so it's not mistaken for a bug.
- 🐛 **gap/bug** — real defect or missing coordination; links a spec or needs one filed.

---

## 0. Foundational model — what is per-clone vs shared

This table is the spine; almost every case is a consequence of it.

| Artifact | Where | Per-clone (local) | Shared (via `aida-store`) | Identity key |
|---|---|:---:|:---:|---|
| Node identity | `.aida/node.toml` + `registry/nodes.toml` | ✓ (local copy) | ✓ (registry) | node id (CAS-allocated) |
| Dispenser state | `.aida/dispenser.toml` | ✓ | | per-clone counter, node-namespaced output |
| Spec object YAML | `objects/TYPE/000/SPEC.yaml` | | ✓ | spec id |
| Block registry | `registry/blocks.yaml` | | ✓ | node id + type prefix |
| Agreed counters | `registry/agreed_counters.toml` | | ✓ | type prefix |
| Oplog | `oplog.yaml` | | ✓ | append-only (replay = future) |
| **Queue** | `registry/queues/<user>.yaml` | | ✓ | **OS user / `AIDA_USER`** (not node!) |
| Cache | `.aida/cache.db` | ✓ | | rebuilt per-clone from YAML |
| Briefs | `.aida/agent-briefs/<agent>/` | ✓ | | agent name + type (local only) |
| Mailbox (local) | `.aida/mailbox/*.json` | ✓ | | inbox-identity union |
| Mailbox (canonical) | `<store>/mailbox/*.json` | | ✓ | digested from local, id-keyed |
| Session leases | `.aida/sessions/*.toml` (local) + `coordination/leases/<scope>.toml` (shared, STORY-637) | ✓ | ✓ | lease id (local) + clone_path (shared) |
| Drain lock | `.aida/drain.lock` (local mirror) + `coordination/drain.lock.toml` (shared, STORY-638) | ✓ | ✓ | pid (local) + clone_path (shared) |
| Solo lock | `.aida/solo.lock` (local mirror) + `coordination/solo.lock.toml` (shared, STORY-638) | ✓ | ✓ | pid (local) + clone_path (shared) |

**The two headline consequences for multi-user:**
1. **Queue is shared and keyed by OS user**, so two same-host clones run by the same `$USER` operate on *one* queue (`current_user_id()`, `main.rs`). Different `AIDA_USER` → separate queues.
2. **Leases and locks are per-clone-local**, so they provide **no cross-clone coordination** — two clones can lease the same spec and run drains simultaneously without either noticing (MU-504/505/506). This is the biggest multi-user gap.

---

## 1. Identity & ID allocation (MU-1xx)

### MU-101 — distinct node ids for two clones
- **Setup:** Clone A `aida init` (node 1). Clone B `git clone` A's origin, `aida node acquire`.
- **Steps:** Inspect each clone's `.aida/node.toml` and the shared `registry/nodes.toml`.
- **Expected:** Distinct node ids (1, 2); both registered in `nodes.toml`.
- **Validates:** CAS allocation, append-only registry.
- **Status:** ✅ — `git_ops::register_node_full` (CAS push loop); test `test_register_node_local` (git_ops.rs).

### MU-102 — concurrent registration of the *same preferred* node id
- **Setup:** Two clones, both `aida node acquire --id JM` simultaneously.
- **Steps:** Race the two registrations.
- **Expected:** One wins; the loser's push is rejected, it pulls, re-reads the registry, and either errors or gets a suffixed id (`JM-2` per STORY-42 `suggest_free_node_id`).
- **Validates:** CAS push-wins, suffix fallback.
- **Status:** 🟡 — logic present (`register_node_full` retry + `test_suggest_free_node_id`), but no test races two real clones on the same preferred id.

### MU-103 — concurrent `aida add` yields non-colliding spec ids
- **Setup:** Clones A (node-aware id `*-1-*`) and B (`*-2-*`).
- **Steps:** Each `aida add` offline; push both.
- **Expected:** Ids are node-namespaced (e.g. `TASK-1-001` vs `TASK-2-001`) → no collision even before pull.
- **Validates:** Node-namespaced dispenser output.
- **Status:** ✅ — `dispenser.rs` distributed mode; `test_memory_dispenser_distributed`.

### MU-104 — offline add on both clones, both push
- **Setup:** Both clones add specs offline, then both `aida pull` + push.
- **Steps:** A pushes first; B pushes (rejected) → `pull_rebase` → push.
- **Expected:** B's specs rebase on top of A's; `ensure_no_spec_id_collisions` confirms no duplicate spec ids.
- **Validates:** Rebase reconciliation + collision guard.
- **Status:** ✅ — `ensure_no_spec_id_collisions` / `find_spec_id_collisions`; pinned two-clone by harness `case_MU-104` (EXPECT=pass): both clones add offline, A pushes first, B pull-rebases + re-pushes, both specs survive in both clones with no duplicate-id error. trace:TASK-960

### MU-105 — merge-gate assigns agreed ids across both clones' specs
- **Setup:** Both clones created node-aware specs; run `aida db merge-gate`.
- **Steps:** Merge-gate scans `objects/`, assigns `TASK-N` agreed ids.
- **Expected:** No two specs get the same agreed id; existing agreed ids skipped (idempotent).
- **Validates:** `git_ops::merge_gate`, BUG-82 collision guard.
- **Status:** ✅ — `merge_gate_skips_existing_short_id_collisions`, `merge_gate_reserves_within_run`.

### MU-106 — node hijack (laptop-died / re-clone claims old node id)
- **Setup:** Clone A registered node "JM"; A is gone. Clone B wants "JM".
- **Steps:** `aida node hijack JM` (or equivalent).
- **Expected:** If A's clone path reachable → `HIJACKED.toml` marker written there; registry entry rewritten (host/email/path/timestamp); B claims it. If unreachable → silently reattributed.
- **Validates:** STORY-43 `git_ops::hijack_node`.
- **Status:** 🟡 — logic present; CLI-integration only, no unit test for the reachable-but-unwritable fallback.

### MU-107 — block-based agreed ids don't overlap across clones
- **Setup:** Clone A `aida db block claim --type FR --size 100` (FR-1..100). Clone B claims after pulling.
- **Steps:** B's claim must start above A's range (and above the agreed-counter floor).
- **Expected:** B gets FR-101..200; dispensing from blocks never overlaps.
- **Validates:** `BlockRegistry::claim_block_with_floor`, `next_range_start_above_counter` (FR-1-073).
- **Status:** ✅ — `test_block_registry_claim_and_dispense`.

### MU-108 — concurrent dispense within one clone is unique
- **Setup:** One clone, N threads each dispensing ids.
- **Expected:** All ids unique (advisory lock serializes).
- **Status:** ✅ — `concurrent_file_dispensers_allocate_unique_ids`, `concurrent_dispense_under_lock_allocates_unique_ids`.

---

## 2. Store sync & conflict (MU-2xx)

### MU-201 — A adds spec + push; B pulls and sees it
- **Setup:** Clone A `aida add FOO`, `aida push`. Clone B `aida pull`.
- **Expected:** B's `.aida-store` advances; B's cache rebuilds (stale-detected); `aida list` in B shows FOO.
- **Validates:** Store leg `pull_rebase`, cache stale-detect.
- **Status:** 🟡 — flow verified in code (`handle_pull_command` store leg → `git_ops::pull_rebase`); no two-clone test.

### MU-202 — concurrent edits to *different* specs reconcile cleanly
- **Setup:** A edits FR-1, B edits BUG-1; both push.
- **Steps:** Second pusher rebases.
- **Expected:** No conflict (separate files `objects/FR/000/FR-1.yaml` vs `objects/BUG/000/BUG-1.yaml`); both specs present after both pull.
- **Validates:** Per-file object layout (`object_store.rs`).
- **Status:** 🟡 — expected-clean by file isolation; untested end-to-end.

### MU-203 — NON-mergeable store conflict still surfaces for manual resolve
- **Setup:** Two clones make divergent edits to a store file with **no known union rule** (e.g. `metadata.yaml`); both commit on the `aida-store` branch.
- **Steps:** A pushes; B `aida pull` → store-leg rebase hits a conflict the auto-merger refuses.
- **Expected:** The conflict is **surfaced** (non-zero exit + recovery hint, "conflict in non-mergeable path …"); the auto-merger never force-resolves an unknown file. (After STORY-641 the *same-spec status edit* no longer reaches this path — see MU-204; STORY-645 further extended the spec-object merge so concurrent **comments** and **relationship/dependency edits** auto-merge too (see MU-208). MU-203 now pins the **complement**: the fallback must still surface a genuinely-unresolvable conflict in a non-spec file.)
- **Validates:** The STORY-641/645 auto-merger's safety boundary — only spec objects + the oplog are auto-resolved; everything else defers to manual.
- **Status:** ✅ — `git_ops::pull_rebase_auto_merge` bails to the manual path for any conflicted path that is neither a spec object nor `oplog.yaml`; harness `case_MU-203` is `EXPECT=pass`. trace:STORY-641

### MU-204 — concurrent same-spec edits AUTO-MERGE on pull (history union + LWW)
- **Setup:** A and B each make a status change on FR-1 (each writes the spec object YAML + an append-only oplog op), both commit.
- **Steps:** A pushes; B `aida pull` → store-leg rebase conflicts on `FR-1.yaml` **and** `oplog.yaml`; the auto-merger reconciles both.
- **Expected:** B's pull completes with **no manual conflict** (rc==0, not left mid-rebase). The spec object's append-only arrays (`history:`/`comments:`/`processing_record:`) are unioned by entry id; scalar fields resolve last-write-wins by `modified_at`; tags union. The oplog is unioned by operation id (lamport reconcile). **Neither clone's edit is dropped** — both `SetStatus` ops survive in the unioned oplog; the merged spec status is the LWW winner.
- **Validates:** Structured three-way merge of spec objects + oplog on the pull path.
- **Status:** ✅ **closed by STORY-641** — pure `conflict::merge_spec_three_way` (history/comments/processing_record union by id + scalar LWW + tag union) + `OpLog::merge` for the oplog, driven by `git_ops::pull_rebase_auto_merge` in the store-leg pull. A one-line note per auto-resolved file is printed; non-mergeable conflicts still fall back to manual (MU-203). Harness `case_MU-204` is `EXPECT=pass`. trace:STORY-641

### MU-205 — fresh-clone auto-attach
- **Setup:** Brand-new `git clone`; first `aida list`.
- **Expected:** `try_attach_store_worktree` fetches `aida-store`, creates `.aida-store/` worktree, rebuilds cache; reads work with no manual step. If the clone landed *on* `aida-store` (GitLab default-branch case), it switches off it first.
- **Validates:** TASK-621 + BUG-559 recovery.
- **Status:** ✅ — `try_attach_store_worktree`, `choose_store_attach_recovery` (BUG-559 fix verified live 2026-06-16).

### MU-206 — offline clone, network returns, push rejected
- **Setup:** B works offline (commits to store), A pushed meanwhile.
- **Steps:** B `aida push` → rejected (non-ff) → B `aida pull` (rebase) → push.
- **Expected:** Clean recovery if no same-spec overlap; else MU-203.
- **Status:** 🟡 — push returns false on non-ff; caller retry path exists; untested two-clone.

### MU-207 — duplicate spec id across clones is detected, not silently accepted
- **Setup:** Force two different specs to claim the same spec id (e.g. corrupted dispenser).
- **Expected:** `ensure_no_spec_id_collisions` errors with a recovery message; AIDA refuses to continue.
- **Status:** ✅ (detection) / 🟡 (repair tooling `aida db check --collisions --repair` is planned, not shipped — `main.rs` recovery message).

### MU-208 — concurrent same-spec COMMENTS (and relationship/dependency edits) AUTO-MERGE on pull
- **Setup:** A and B each `aida comment add <SPEC> "..."` a **different** comment on the SAME spec (each rewrites the spec object YAML's `comments:` array), both commit.
- **Steps:** A pushes; B `aida pull` → store-leg rebase conflicts on the spec object; the auto-merger reconciles it.
- **Expected:** B's pull completes with **no manual conflict** (rc==0, not left mid-rebase) and **BOTH comments are present** in the merged spec object — the `comments:` array is unioned by `Comment.id`. The same union applies to concurrently-added **relationships** (set union by `(rel_type, target_id)`) and **dependencies** (set union by target uuid), so neither side's edge is dropped by the LWW scalar base. Genuinely-divergent scalars still resolve LWW; truly-unresolvable non-spec files still fall back to manual (MU-203).
- **Validates:** STORY-645 completes the structural three-way merge of spec objects — finishes MU-203 for the structurally-mergeable append-only/set fields.
- **Status:** ✅ **closed by STORY-645** — `conflict::merge_spec_three_way` now also unions `comments` (by id), `relationships` (by `(rel_type, target_id)`), and `dependencies` (by uuid) on top of the STORY-641 history/processing-record/tag union + scalar LWW. Harness `case_MU-208` is `EXPECT=pass`. trace:STORY-645

---

## 3. Cache (MU-3xx)

### MU-301 — B's cache auto-rebuilds after pulling A's push
- **Expected:** After store HEAD advances, `ensure_cache_fresh` sees recorded-SHA ≠ HEAD → full rebuild on next `aida list`/`search`.
- **Status:** ✅ — `cached_git_backend::ensure_cache_fresh`.

### MU-302 — external `git pull` under a long-lived backend
- **Setup:** A long-running process holds a `CachedGitBackend`; an external `aida pull` advances HEAD.
- **Expected:** `restamp_head` detects the external move, clears the recorded SHA, forces rebuild on next read (no stale reads).
- **Status:** ✅ — TASK-712 `restamp_head`.

### MU-303 — caches are independent per clone
- **Expected:** Clone A's `.aida/cache.db` is never read by clone B; each rebuilds from YAML.
- **Status:** ✅ by design (gitignored, never pushed).

### MU-304 — `aida cache status` reports staleness across the divergence
- **Expected:** Reports cache HEAD vs orphan HEAD; mismatch flagged; `aida cache rebuild` fixes.
- **Status:** ✅ — `CacheCommand::Status`.

---

## 4. Queue identity (MU-4xx)

### MU-401 — two clones, SAME OS user → share one queue
- **Setup:** Clones A and B, both run by `$USER=joe`, no `AIDA_USER`.
- **Steps:** A `aida queue add X`; push. B `aida pull`; `aida queue list`.
- **Expected:** Both resolve `current_user_id()=joe` → both read/write `registry/queues/joe.yaml`; B sees X after pull.
- **Validates:** Shared-via-store queue keyed by OS user.
- **Status:** 🟡 — `current_user_id`, `queue_add_then_list_same_shell_is_consistent` (single-shell); no two-clone test.

### MU-402 — two clones, different `AIDA_USER` → separate queues
- **Setup:** A `AIDA_USER=alice`, B `AIDA_USER=bob`.
- **Expected:** Separate files `queues/alice.yaml`, `queues/bob.yaml`; no cross-visibility in `aida queue list` (each sees own); `aida list` glyph aggregates across all (via `all_queued_requirement_ids`).
- **Status:** 🟡 — code supports; untested two-clone.

### MU-403 — queue add in A visible in B only after sync
- **Expected:** Because the queue lives on the store branch, B sees A's add only after A pushes and B pulls. Before sync → invisible.
- **Status:** ✅ — consequence of shared-via-store; pinned two-clone by harness `case_MU-403` (EXPECT=pass): B's `queue list` does NOT show A's add before sync and DOES after push+pull. trace:TASK-960

### MU-404 — two machines both `"default"` user → collision
- **Setup:** Two clones, neither sets `$USER`/`AIDA_USER` → both `current_user_id()="default"` → both write `queues/default.yaml`.
- **Expected (today):** Warning when the default queue already carries entries from a *different* machine fingerprint (`default_queue_collision_fingerprint`); no auto-resolution — next `aida db sync --push` can conflict.
- **Status:** 🐛 **known** — BUG-89 / TASK-618 (warn-only by operator decision). On a single host two clones usually share the same real `$USER`, so this mainly bites cross-machine — but reproducible on one host by unsetting `$USER`.

### MU-405 — concurrent queue add from two same-user clones
- **Setup:** A and B (same user) both `aida queue add` different specs, both push.
- **Expected:** Second push rebases; both entries survive (YAML list append at different positions usually merges; same-position edits → conflict like MU-203).
- **Status:** 🐛 **known gap (documented, not fixed)** — a fresh `aida queue add` always appends at the SAME default `position: 1000`, so two concurrent same-user adds write the SAME list slot in `registry/queues/<user>.yaml`. The store-leg auto-merger (`conflict::merge_spec_three_way`) only unions **spec objects + the oplog** — it has **no union rule for queue files** — so B's pull-rebase hits a genuine git conflict and is left mid-rebase, one entry stranded (NOT a clean both-survive merge). This is the **queue-file analog of the MU-203 non-mergeable boundary** and the same coordination class as MU-504/505/506: a shared-store artifact the auto-merger does not yet reconcile. Harness `case_MU-405` is `EXPECT=known-gap` — it ASSERTS the current conflict-surfacing behavior and FLAGS ("GAP CLOSED — flip EXPECT to pass") if a queue-file union rule later lands. Disposition is an EPIC-46 follow-on (a `position`-keyed or append-by-id union for queue files). trace:TASK-960

---

## 5. Briefs / mailbox / leases / locks (MU-5xx) — the coordination layer

### MU-501 — brief filed in A is invisible in B
- **Setup:** A writes a brief (`aida brief <agent> <SPEC>`).
- **Expected:** It lands in A's `.aida/agent-briefs/` (local, gitignored) → clone B never sees it.
- **Validates:** Briefs are per-clone local; cross-clone handoff must use the mailbox.
- **Status:** ⚠️ **accepted limitation** — `resolve_brief_directories`, `brief_root_dir` are local. Document loudly: **do not** rely on briefs for cross-clone routing.

### MU-502 — auto mailbox sync: messages flow between users on pull/push
- **Setup:** A `aida mailbox send --to <id>` (writes local `.aida/mailbox/`).
- **Steps:** A `aida push` → B `aida pull` → B `aida mailbox inbox <id>`. **No manual `aida mailbox sync`.**
- **Expected:** The push store leg PUBLISHES the local mailbox into the canonical `<store>/mailbox/` (idempotent, id-keyed) and folds it into the store commit; B's pull rebases the canonical message down; B's read path merges canonical+local so the message surfaces. Both legs also publish on `aida pull`, so a sender that only ever pulls still propagates. Best-effort — a mailbox failure never breaks pull/push. Opt out with `AIDA_MAILBOX_AUTOSYNC=0` or `[mailbox] autosync = false`.
- **Validates:** Hybrid mailbox (STORY-493) auto-wired into sync (STORY-643). Tier-2 of EPIC-47.
- **Status:** ✅ **closed by STORY-643** — publish leg `maybe_publish_mailbox_for_sync` in the `handle_push_command` / `handle_pull_command` store legs (gated by `mailbox_autosync_enabled`); receive side already merges canonical in `aida mailbox inbox` and now also in the MCP `read_inbox`. Harness `case_MU-502` is `EXPECT=pass`. trace:STORY-643

### MU-503 — addressing by agent type reaches the mailbox
- **Expected:** `--to <type>` reaches an agent whose `AIDA_AGENT_TYPE` matches (unioned into `inbox_identities`).
- **Status:** ✅ — TASK-818; `message_to_agent_type_reaches_inbox`, `inbox_identities_unions_stable_name_and_agent_type`.

### MU-504 — two clones lease the SAME spec simultaneously
- **Setup:** A `aida session start --owns FR-1`; B `aida session start --owns FR-1`.
- **Expected:** A acquires; **B is REFUSED** with the holder's host / clone path / agent / age (pass `--force` to override). A shared lease claim lives at `coordination/leases/<scope>.toml` on the `aida-store` branch; `session start --owns` (and `aida agent new --spec`) pull-check-claim-push it. Liveness: same-host pid for process-backed claims + a universal TTL/heartbeat backstop; an `aida session end` deletes the claim. Best-effort: no remote / unreachable store WARNs and proceeds local-only.
- **Validates:** Cross-clone double-work prevention.
- **Status:** ✅ **closed by STORY-637** (slice 1) — `aida-cli/src/coordination.rs` (`decide_claim` pure decision + `acquire_claim`/`release_claim`/`list_claims`); harness `case_MU-504` is `EXPECT=pass`. Foreign claims are surfaced in `aida session leases`. trace:STORY-637

### MU-505 — two clones run drains simultaneously
- **Setup:** A `aida burndown run`; B `aida queue work --auto-complete`.
- **Expected:** A acquires; **B is REFUSED** naming the holder (host / clone path / pid / cmd / age). A shared, process-backed drain claim lives at `coordination/drain.lock.toml` on the `aida-store` branch; both drain entry points (`burndown run`, `queue work --auto-complete`, `queue integrate`) pull-check-claim-push it BEFORE the local `.aida/drain.lock`. Liveness: same-host pid (drain IS the process) + TTL/heartbeat backstop (folds in `AIDA_DRAIN_LOCK_STALE_SECS`); a background heartbeat thread keeps a long drain fresh; the `DrainGuard` deletes the claim on exit. `AIDA_DRAIN_FORCE=1` overrides. Best-effort: no remote / unreachable store WARNs and proceeds local-only.
- **Validates:** Cross-clone drain coordination (double-drive prevention).
- **Status:** ✅ **closed by STORY-638** (slice 2) — `aida-cli/src/coordination.rs` (`LockKind::Drain`, `acquire_lock_claim`/`release_lock_claim`/`heartbeat_lock_claim`) + `drain_lock::acquire_drain_lock`; harness `case_MU-505` is `EXPECT=pass`. trace:STORY-638

### MU-506 — two clones run solo loops simultaneously
- **Setup:** A `aida solo run`; B `aida solo run`.
- **Expected:** A acquires; **B is REFUSED** naming the holder. Shared process-backed solo claim at `coordination/solo.lock.toml`; `solo_lock::acquire_solo_lock` consults it before the local `.aida/solo.lock`. Same-host pid liveness + TTL/heartbeat (refreshed on each loop cycle); `SoloGuard` deletes the claim on exit. `AIDA_DRAIN_FORCE=1` overrides. Best-effort if store unreachable.
- **Validates:** Cross-clone solo coordination (same class as MU-505).
- **Status:** ✅ **closed by STORY-638** (slice 2) — `LockKind::Solo` + `solo_lock::acquire_solo_lock`; harness `case_MU-506` is `EXPECT=pass`. trace:STORY-638

### MU-507 — within one clone, a second drain is refused
- **Setup:** One clone, drain running; start a second drain.
- **Expected:** Refused with holder pid/age/command; stale (dead pid or aged-out) → reclaimed; `AIDA_DRAIN_FORCE=1` bypasses.
- **Status:** ✅ — `drain_lock::decide_lock`; 15 unit tests.

---

## 6. Completion / auto-bump across clones (MU-6xx)

### MU-601 — commit referencing a spec on A's code branch auto-completes after B pulls
- **Setup:** A merges a `(FR-1)`-trailered PR to main. B `aida pull`.
- **Expected:** B's pull scans new main commits, auto-bumps FR-1 Done→Completed.
- **Status:** 🟡 — `handle_pull_command` auto-bump (single-repo). Cross-*repo* completion is BUG-568 (warns) / SPIKE-62 (Tier-2). Within one shared-origin repo with two clones, works; untested two-clone.

### MU-602 — squashed-merge narrow-scan edge
- **Expected:** If the merge advanced local main before pull (no-op pull), auto-bump falls back to a wide scan (BUG-404).
- **Status:** ✅ — BUG-404 fix in `handle_pull_command`.

---

## Cross-cutting findings (the "so what")

1. **Coordination is the weak axis.** ID allocation, store sync, and cache are robust (CAS, rebase, stale-detect, collision guards). The gaps are all in **cross-clone coordination**: leases (MU-504), drain lock (MU-505), solo lock (MU-506) are per-clone-local and provide *zero* cross-clone safety. Two same-host clones can silently double-work the same spec. **This is the headline decision** — either put a shared lease/lock registry on the store branch, or explicitly document the single-driver assumption.
2. **Same-spec concurrent edits now AUTO-MERGE on pull** (MU-204 status/history, STORY-641; MU-208 comments/relationships, STORY-645). `git_ops::pull_rebase_auto_merge` reconciles conflicting spec objects (`conflict::merge_spec_three_way`: history/comments/processing_record union by id + relationships union by `(rel_type, target_id)` + dependencies union by uuid + scalar LWW + tag union) and the oplog (`OpLog::merge`) during the store-leg rebase, so concurrent same-spec edits no longer stop for manual resolution. Conflicts in files with no union rule still defer to manual (MU-203).
3. **Queue is shared + OS-user-keyed** (MU-401/404) — intuitive for same-user, surprising for the `"default"` fallback (BUG-89).
4. **Briefs are local-only** (MU-501) — a real cross-clone routing trap; the mailbox is the sanctioned cross-clone channel, and after STORY-643 it **auto-syncs on the normal `aida pull` / `aida push`** (MU-502) — no manual digest, so a teammate's message arrives with the next pull.

## Suggested next steps (for discussion)

- **~~Decide MU-504/505/506~~ — DONE.** Resolved in favour of shared coordination: STORY-637 (leases) + STORY-638 (drain/solo locks) put a shared registry on the `aida-store` branch; all three are now `EXPECT=pass` in the harness. They were the cross-clone analog of the per-clone spec-claim work (BUG-637 / TASK-957); their cross-clone disposition is closed.
- **~~Build a same-host harness~~ — DONE.** `scripts/multi-clone-harness.sh` exists (STORY-636) and now runs 27 MU-### cases against two real clones of a throwaway origin under an isolated `$HOME`.
- **Lift the remaining 🟡 cases into the harness** — the highest-value works-untested two-clone flows are now pinned: MU-104 (offline-add reconcile + collision guard), MU-403 (queue-add visible only after sync). Still-untested flow candidates: MU-206 (offline push-reject recovery), MU-601/602 (cross-clone Done→Completed auto-bump — note the *catalog* MU-601 id is reused by the harness for the BUG-636 cache-contention case; resolve the id clash when automating the auto-bump flow).
- **MU-405 queue-file concurrent-add gap (open follow-on)** — `case_MU-405` documents (does not fix) the gap: concurrent same-user `queue add`s land at the same default `position: 1000`, and the store-leg auto-merger has no union rule for `registry/queues/*.yaml`, so the second pull conflicts. Disposition: a `position`-keyed or append-by-id union for queue files (queue-file analog of the STORY-641/645 spec-object union). EPIC-46 follow-on.
- **Confirm MU-404/BUG-89 disposition** — the `"default"`-user cross-machine collision remains warn-only by operator decision.

<!-- trace:multi-user-test-catalog | ai:claude -->
