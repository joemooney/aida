# Cross-clone coordination: shared leases + drain/solo locks

- **Date:** 2026-06-16
- **Specs:** EPIC-46 (multi-user test coverage). Closes catalog gaps MU-504 (cross-clone lease), MU-505 (cross-clone drain lock), MU-506 (cross-clone solo lock).
- **Status:** Design — needs operator nod on the three forks (§4) before slice 1.
- **Complexity:** Architecture-class. Introduces load-bearing SHARED state on the `aida-store` branch and adds a pull+claim+push to the session-start / drain-start hot path. Hard-to-reverse once clones depend on the registry.
- **Gate:** the harness (`scripts/multi-clone-harness.sh`) is the red→green driver — MU-504/505 are GAP today; each slice flips one to PASS.

## 1. Problem

Per the EPIC-46 catalog (code-grounded): leases (`.aida/sessions/*.toml`), `drain.lock`, and `solo.lock` are **per-clone-local**. Two clones sharing one store therefore have **zero cross-clone coordination** — both can `session start --owns FR-1`, both can run drains over the same shared queue → duplicate PRs, merge races, double-work. Intra-clone coordination already works (`find_scope_lease_conflict`, `drain_lock::decide_lock`); the gap is purely cross-clone visibility.

## 2. Approach — a shared coordination registry on the store branch

Put claim records on the `aida-store` orphan branch (the existing shared substrate) under a new `coordination/` tree:

```
coordination/
  leases/<scope>.toml         # one file per leased scope (spec id or worktree scope)
  drain.lock.toml             # per-repo drain claim (one active drain across all clones)
  solo.lock.toml              # per-repo solo-loop claim
```

A claim record carries enough to (a) identify the holder and (b) decide liveness:

```toml
scope       = "FR-1"          # leases only
node_id     = "2"             # which node/clone holds it
clone_path  = "/home/joe/ai/aida-b"
host        = "imac"
pid         = 48213
agent       = "codex-implementer-1"
started_at  = "2026-06-16T22:10:00Z"
heartbeat_at= "2026-06-16T22:14:30Z"
ttl_secs    = 1800
review_verb  = false
```

**Claim protocol (CAS push-wins, mirrors `register_node_full`):**
1. `git_ops::pull_rebase` the store (cheap; coarse event).
2. Read the relevant `coordination/` file. If a **live** claim by another clone exists → refuse (or `--force`).
3. Else write our claim, commit, push. On non-ff rejection → pull, re-check, retry (bounded).
4. Release on `session end` / drain exit (RAII guard) → delete the file, commit, push (best-effort; staleness covers a crash).

**Liveness (decides "live" vs "reclaimable"):**
- **Same host** (`host == ours`): check `pid` with the existing process-probe → dead pid = reclaim immediately (fast path; Phase 1's primary case).
- **Any host:** `now - heartbeat_at > ttl_secs` = stale = reclaimable (portable backstop). Long-running holders refresh `heartbeat_at` periodically (drain loop tick / session keepalive).
- `--force` / `AIDA_*_FORCE=1` bypasses (matches `drain_lock` today).

This reuses three patterns already in the codebase: CAS push-wins (`register_node_full`), pid-liveness + TTL backstop (`drain_lock::decide_lock`), and RAII release guards (`DrainGuard`/`SoloGuard`).

## 3. Hot-path cost

Session-start and drain-start gain one `pull_rebase` + (on claim) one `push`. These are **coarse events** (not per-spec-read), so the added latency is acceptable. Reads (`aida list`, etc.) are untouched. The heartbeat refresh is a small periodic commit during a drain loop — already a long operation. No change to the read hot path or the cache.

## 4. Forks needing an operator nod

- **F1 — storage shape:** one file per scope (`leases/<scope>.toml`) vs a single `leases.yaml`. **Recommend per-file** — different scopes never git-conflict; CAS contention only on a genuine same-scope race. (Same reasoning as one-YAML-per-spec in `objects/`.)
- **F2 — liveness model:** **Recommend both** — same-host pid (fast, exact, covers Phase 1) + universal TTL/heartbeat (portable, covers cross-host Phase 2). Ship both in slice 1 so cross-host works the day we test it.
- **F3 — enforcement:** hard-refuse (+`--force` + stale auto-reclaim) vs advisory warn-and-allow. **Recommend hard-refuse** — matches intra-clone `drain_lock` semantics; the whole point is to *prevent* double-work, not just announce it. `--force` is the escape hatch.

## 5. Slices (each gated on the harness)

- **Slice 1 — cross-clone LEASES (closes MU-504). ✅ SHIPPED (STORY-637).** `coordination/leases/<scope>.toml` claim+release on the store (`aida-cli/src/coordination.rs`); `session start --owns` and `aida agent new --spec` pull+check+claim+push; refuse on a live cross-clone lease; same-host pid (process-backed claims only) + universal TTL reclaim; `--force` / orchestrator-corroborated short-circuit. **Clone-identity keys off `clone_path`, not `node_id`** — two clones sharing one store inherit the same store-branch `node.toml` node id, so node_id can't tell clones apart. Session leases are NOT process-backed (the `session start` pid is ephemeral), so TTL — not pid — governs their reclaim. Harness MU-504 → PASS. Foreign leases surfaced in `aida session leases`.
- **Slice 2 — cross-clone DRAIN + SOLO locks (closes MU-505/506). ✅ SHIPPED (STORY-638).** Promoted the drain/solo locks to per-repo claims on the store at `coordination/drain.lock.toml` / `coordination/solo.lock.toml` (`aida-cli/src/coordination.rs`: `LockKind`, `acquire_lock_claim`/`release_lock_claim`/`heartbeat_lock_claim`, reusing slice-1's `Claim` + `decide_claim`). `acquire_drain_lock`/`acquire_solo_lock` now pull-check-claim-push the shared claim BEFORE taking the local lock (the local `.aida/{drain,solo}.lock` stays as the fast intra-clone mirror/probe — MU-507 unchanged). Drain/solo claims are **process-backed** (`process_backed = true`), so same-host pid liveness is the authoritative reclaim signal, with the TTL/heartbeat (folding in `AIDA_DRAIN_LOCK_STALE_SECS`) as the cross-host / pid-recycle backstop. `AIDA_DRAIN_FORCE=1` (and the drain's existing force) is the shared override. The `DrainGuard` runs a background heartbeat thread (every 300s) and the solo loop refreshes on each cycle tick, so a long drain/solo never ages out; both guards release the shared claim on drop (best-effort, staleness covers a crash). Best-effort throughout: no origin / unreachable store WARNs ("cross-clone coordination unavailable: proceeding") and falls back to local-only. Harness MU-505 flipped GAP→PASS and a new MU-506 (solo) case added; both PASS, MU-504/507 still PASS, suite exits 0. trace:STORY-638
- **Slice 3 (optional) — observability + cross-host hardening.** `aida status` cross-clone coordination view (who holds what, where), heartbeat tuning, a `coordination doctor` to reap orphaned claims.

## 6. Backward-compat / safety

- New `coordination/` tree; absent = no claims = current behavior. Old binaries ignore it (graceful — they just keep their per-clone-local behavior, i.e. the current gap, no crash).
- Release is best-effort; staleness (pid/TTL) guarantees a crashed holder's claim is always eventually reclaimable — no permanent deadlock.
- `.gitignore` unaffected (this is *inside* the store worktree, which is fully tracked).

## 7. Tests

- The harness (MU-504/505, + a new MU-506) is the integration gate.
- Unit tests for the pure decision (`decide_claim(existing, now, ours) -> Acquire|Refuse|Reclaim`) mirroring `drain_lock`'s 15-test pattern: no claim → acquire; live same-host pid → refuse; dead pid → reclaim; heartbeat within ttl → refuse; stale → reclaim; `--force` → acquire.
- Do NOT run a real auto-complete drain in tests (per the project rule) — test claim mechanics structurally.

## Critical files
- `aida-cli/src/worktree_lease.rs`, `main.rs` (`list_leases`, `session start`/`agent new`, `find_scope_lease_conflict`) — lease claim points
- `aida-cli/src/drain_lock.rs`, `solo_lock.rs` — promote to shared claim
- `aida-core/src/git_ops.rs` (`register_node_full` as the CAS template; `pull_rebase`)
- `aida-core/src/node.rs` (registry CAS pattern), `process_probe` (pid liveness)
- `scripts/multi-clone-harness.sh` (the gate)

<!-- trace:EPIC-46 | ai:claude -->
