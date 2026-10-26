# BUG-2221 positive-path slice: diagnosis and SKETCH v2 (signed 2026-10-25 23:55 MST)

Status: signed 2026-10-25 23:55 MST (P1 with O1, P2 with a condition). P2 is
implemented. P1 is held on two soundness gaps; see "Implementation status".

Budgets are unchanged: 1000 ms / 10% / 5000 ms guard, 4 s
`SHOW_LINKAGE_DEADLINE`. No relabeling or deletion of telemetry.

## Round-2 review findings: already resolved on main

Review CHANGES REQUESTED at 5f777c654a listed seven findings. The fifth
branch commit, "apply the 2026-10-25 advisor ruling", fixed every one of them,
and it is part of squash 98e6823c07 on `origin/main`:

| # | Finding | Where it is resolved on main |
|---|---|---|
| 1 | completing `(BUG-2221)` trailer | squash subject is `(Refs BUG-2221) (#2928)`; the spec stays in-progress |
| 2 | unruled beyond-sketch items | advisor ruling (spec comment 8): (a)(b)(c)(e) accepted with conditions, (d) and pure-warm ProvedEmpty rejected. All of them are applied in the 5th commit |
| 3 | serial snapshot on positive/cold shows | no snapshot on positive/cold keys; one rev-parse locates the key; BUG-1993 fixture back to 1.2 s (`bug_1993_show_deadline_tests.rs:181,216`) |
| 4 | uid / no-follow namespace | `owned_namespace_dir` checks each component with `symlink_metadata` plus `owned()`; eviction skips unowned files (`show_negative.rs:1075-1088`, `:1185-1192`) |
| 5 | LRW not LRU eviction | `touch_state` on a warm read (`show_negative.rs:372`, `:1049`) |
| 6 | file symlinks escape the fence | any symlink whose canonical target is outside the root marks the scan incomplete (`trace.rs:351-363`) |
| 7 | per-directory canonicalize | the cycle fence uses `(dev, ino)` (`trace.rs:456-458`); only symlink targets are canonicalized |

## Positive-path diagnosis (2026-10-25 ~23:15 MST, load ~12-14 on 6 cores)

These were measured in a seat clone that borrows the primary's object store
(`objects/info/alternates`). It has the same refs/commits shape as the
primary: 80,603 commits reachable from `--all` and 1,337 refs (1,177 of them
`refs/remotes/origin/*`). Wall times come from single bounded runs, so they
are indicative, not a benchmark.

| Call | Wall |
|---|---|
| `aida show BUG-2224 --format json` (shipped positive, PATH binary) | 1562 / 1586 ms |
| same with `--no-git` | 405 ms |
| original collector `log --all -F --date-order --pretty=… --grep BUG-2224` | 837 ms |
| `log --all -F --pretty=%H --grep STORY-1889` (no `--date-order`) | 740 ms |
| `log --all -F --date-order --pretty=%H --grep STORY-1889` | 1019 ms |
| `rev-list --all` (no message read at all) | 804 ms |
| `rev-list --all --since=2026-09-27` | 20,543 commits, 406 ms |
| `branch --all --contains <sha>` (one call) | 435 ms |
| `for-each-ref --contains <sha> refs/heads refs/remotes` | 320 ms |
| `merge-base --is-ancestor` | 14-44 ms |

Findings:

1. **The original `--all -F --grep` walk is the floor of every positive
   show.** It costs about 0.75-1.0 s on its own, which is most of the
   ~1.15 s default-minus-`--no-git` gap for BUG-2224. `--date-order` adds
   ~0.28 s because it forces a full topo-constrained sort before output.
   Even with no message read, `rev-list --all` costs 0.8 s. The probe and
   seed cost on positive keys is gone (finding 3 above), so what remains is
   the unchanged original query.
2. **The primary's commit-graph is stale.** `objects/info/commit-graph` is
   dated 2026-09-26. 20,543 of the 80,603 reachable commits (25%) postdate
   it, and objects are spread over 50 packs, so every `--all` walk parses
   about a quarter of the history from packs instead of the graph. This is
   an environment condition, not code, and the operator can fix it with
   `git commit-graph write --reachable` / `git maintenance run` on the
   primary. This seat's sandbox refused even a scratch-dir commit-graph
   write, so the effect of a fresh graph is **not measured** and is not
   claimed as the fix.
3. **The ~3 s non-commit stage seen under load is the in-flight branch
   search.** An unshipped spec (BUG-2221 while #2928 was open) takes the
   `else` arm in `collect_git_linkage_inner`. That arm runs one
   `git branch --all --contains` per referencing commit, newest first, at
   ~0.43 s each idle, up to `LINKAGE_BRANCH_SCAN_MAX_COMMITS` = 20 within
   `LINKAGE_BRANCH_SCAN_BUDGET` = 2 s. Add the worktree probe and the
   default-branch resolution under load, and you get ~3 s with
   `linkage.log` at only ~113 ms (a warm negative-proof read cannot be the
   cause there, because the spec had commits). Shipped specs pay only the
   ~14-44 ms ancestor check.
4. About 0.3 s of the BUG-2224 gap is still unattributed: shipped-check
   resolution, forge/shortstat lines, the filing-drift hint, and the lease
   listing. Per-phase usage events were not captured, because redirecting
   telemetry to a scratch home is refused in this seat. **That attribution
   is open.** It needs one operator-run `aida show` with phases recorded on
   the live binary.

## SKETCH v2: bounded positive path (for signoff)

Two independent mechanisms. Each is exact, so answers stay complete and
truthful, and each falls back to the unchanged original call on any
unsupported or degraded condition.

### P1. Incremental positive proof: extend the v1 CLOSED cache with hits

v1 caches only the CLOSED-negative cone and drops a key's state on its first
hit. v2 keeps the same key (exact `-F --grep` strings, parser schema
version, S digest, R roots) and the same CLOSED frontier, and adds:

- `hits`: every commit inside the CLOSED cone whose message passes the same
  parsers (`extract_spec_ids_from_commit` + referenced + `trace:` lines).
  For each hit it stores full OID, short OID as printed, subject, and
  committer timestamp. The hit count is bounded per key; past the bound
  the key is never cached, so it stays on the original query.
- Warm read: verify S/R exactly as v1 does. Walk only the commits above
  the CLOSED frontier with the same fenced `^closed` batch walk and the
  same Git prefilter and parsers. Union the new hits, then re-check S/R
  after the walk (amendment 1, applied to every answer, not only
  ProvedEmpty).
- Seeding: the same rule the advisor accepted as (a). Only a COMPLETE
  original-collector run pinned to a before-snapshot seeds a key, and here
  it seeds the hit list too. A hit in an incomplete or timed-out run seeds
  nothing.

**Ordering, the open design point.** The renderer and the shipped/branch
anchors depend on `--date-order` order (`commits.first()` is the newest
anchor). For a subset of a DAG, date order means "children before parents,
otherwise newer committer date first". Options:

- **(O1, recommended)** Store, per hit, the set of other hits it is an
  ancestor of. That set is computed incrementally during the walk: a
  commit's hit-descendants are known when its children are walked. A local
  stable topo-sort keyed by committer date then reproduces Git's
  `--date-order` on the subset. It needs no extra Git process. An
  oracle-equivalence test compares it against the original collector on
  fixtures with clock skew and a rebased branch.
- (O2) Sort by committer date only (`--no-walk=sorted` semantics). This is
  cheaper but not exact under clock skew, so it is **not proposed** unless
  the advisor rules the order non-normative.

Expected effect: a warm positive show pays the S/R snapshot plus a walk over
only the commits added since the last show. That is the same cost class as
a warm negative show (measured 560-690 ms total on the live binary), not
the 0.75-1.0 s full walk.

### P2. Name-matched branch containment before the full branch scan

For an unshipped spec, `id_matches` (BUG-553/1528/1591) only ever contains
refs whose name matches a spec id. P2 does the following:

1. One `for-each-ref refs/heads refs/remotes --format=%(refname) %(objectname)`
   (a single process). Select refs whose normalized name matches an id,
   using the same `norm_id` rule plus the main/master/HEAD/orphan-store
   exclusions.
2. For each selected ref, newest referencing commit first, run
   `merge-base --is-ancestor <commit> <tip>` (~14-44 ms each) under the
   existing branch budget. This yields the same first-seen recency order
   and local-before-remote ranking that the `--contains` union produces for
   those refs.
3. If no id-named ref contains any referencing commit, run the
   `--contains` scan, so the fallback-to-first-candidate behaviour is
   unchanged. *Amended 2026-10-26 (advisor ruling 02:20 MST):* a refused or
   failed git call is UNKNOWN, never an empty answer. The scan used to count
   a failed call as scanned and move on. It now stops at the first failed
   call. In a contained seat's clone, where host git always refuses, the
   search runs no git at all. In both cases `branch_scan_truncated` reports
   the unchecked commits, so `GitLinkage::unknown_reason` is the branch-scan
   reason and review surfaces never read the spec as never-pushed.
   `branch_scan_truncated` is therefore set when the fallback scan is cut
   short (cap, budget, timeout) **or** cannot run.

*Amended 2026-10-26:* step 1 as implemented reuses the scan's own `git branch
--all --format=%(refname:short)%00%(objectname)` listing rather than
`for-each-ref`. The id match is on a token boundary (`BUG-1` matches
`bug-1-work` but not `bug-10` or `bug-123-fix`). This applies to the scan's
selection as well, so both searches stay equal and the fast path does not
spend its 500 ms share on false candidates.

Equivalence test: on fixtures with a branch-crossing (`bug-N-work` plus
`bug-N-round2`), a stacked sibling branch (BUG-553), a remote-only newer
branch (BUG-1591), and the orphan store, the `branch` / `other_branches`
output matches the original scan.

### Not proposed

- No date cutoff, no store-ancestry subtraction, no background warming, no
  daemon.
- No change to `LINKAGE_*` budgets or the 4 s deadline.
- No automatic `git maintenance` from `aida show`. Writing the
  commit-graph is an operator action (finding 2). At most, `aida doctor`
  could report a stale graph, which would be a separate spec if wanted.

### Tests (named, hermetic, PATH-shim structural, no tight wall clock)

- `positive_warm_walks_only_new_commits`: a shim counts `--all` walks.
  The warm positive run has none.
- `positive_order_matches_date_order_oracle`: clock skew, a rebased
  branch, and a merge-heavy multi-root DAG.
- `positive_hit_added_above_frontier_is_found`
- `positive_incomplete_oracle_seeds_nothing`
- `positive_after_snapshot_moved_runs_original`
- `positive_hit_bound_exceeded_never_caches`
- `branch_name_match_equals_contains_scan` (the four fixtures above)
- `branch_no_name_match_falls_back_unchanged`
- Existing BUG-546/553/1528/1591/1594/1993/2221 tests stay green.

### Measurement plan (AC4)

Pin the live SHA and the source SHA separately. Interleave five runs each of
default and `--no-git` for a negative spec (TASK-2347), a shipped positive
spec (BUG-2224, STORY-1889), and an unshipped positive spec, and record the
load average. Report cold and warm separately. Fresh lineage-scoped guard
telemetry comes from the promoted build only, with the old 24 h window
untouched.

## Implementation status (2026-10-26, after the 23:55 MST signoff)

**P2 is implemented** in `aida-cli-lib/src/linkage_branch.rs`. The original
`--contains` loop was moved out of `trace.rs` without change, as
`contains_branch_scan`. `name_matched_branch_scan` runs in front of it. It
does one `git branch --all --format=%(refname:short)%00%(objectname)` listing
(the same listing, order and short names the scan reads). It keeps the
id-named refs, takes the first containing commit index per tip from
`merge-base --is-ancestor` over the same newest-first `n = min(len, 20)`
commits, and replays the scan's accumulation in that order. So a hit is the
scan's own answer for those commits, and commit-cap truncation is reported
the same way. A miss (no id-named ref, none containing a commit, any error or
timeout) falls through to the unchanged scan. The fast path uses at most
500 ms of the one unchanged 2 s branch deadline. Tests in
`tests/bug_2221_branch_fast_path_tests.rs` check equality with the scan on
branch-crossing, stacked-sibling, local+remote same-tip, orphan-store and
commit-cap fixtures, plus the no-name, miss and expired-deadline
fallthroughs.

**P1 is not implemented. Two soundness gaps in the signed sketch need a
ruling (SKETCH v2 amendment below).**

### Gap 1: O1 does not reproduce Git's `--date-order`

`--date-order` orders the WHOLE `--all` graph (Kahn's algorithm, newest
ready commit first, ties by insertion order). `--grep` then filters that
sequence. A non-hit commit with a skewed date reorders hits that have no
ancestry relation to each other, so a sort of the hit subset by its own
ancestry and dates is not the same order. Counterexample:

```
base(100) ── A(1000, hit) ── C(10, non-hit, tip of x)
base(100) ── B(500, hit, tip of y)
git log --all --date-order --grep : B, A   (C waits behind B; A waits for C)
O1 subset sort                    : A, B   (A and B unrelated; 1000 > 500)
```

O1 is exact only when dates never decrease along an edge and no two commits
are tied when both are ready. The fleet produces both: rebases stamp a whole
chain with one committer second, and clock skew is possible.

### Gap 2: cached hits can stop being reachable

v1 never had to ask whether a CLOSED commit is still reachable, because a
negative stays negative. A cached HIT must be answered only while some
current ref still reaches it. Merged or abandoned branches are deleted, and
`origin/*` gets pruned, all the time here. That is exactly when a spec's
branch-only commits leave `--all`. The CLOSED set keeps no internal edges, so
the warm walk cannot tell which stored hits are still reachable.

### SKETCH v2 amendment for P1 (needs a ruling)

P1' keeps P1's key, seed rules, snapshot pinning and mandatory after-S/R
re-check. It replaces O1 and adds a reachability rule:

1. **Store Git's own order.** Seeding stays as approved: a complete
   original-collector run pinned to the before-snapshot. The seed now also
   stores that run's hits in their exact output order, each with its
   committer timestamp, plus `max_root_ct` (the newest committer timestamp
   among the roots in that snapshot, from `%ct` on the existing `--no-walk`
   root listing).
2. **Reachability: answer only when no old root is lost.** Every root of the
   stored state must be a current root, or a parent of a commit this warm
   walk framed. Otherwise the original runs and reseeds. That proves the old
   cone (and so every stored hit) is still in `--all`, with no extra Git
   call.
3. **Order: answer only when the new commits provably sort first.** The warm
   walk also frames `%ct`. Answer only when every newly walked commit has
   `ct > max_root_ct`. Under that condition Git outputs every new commit
   before any old commit, and the old commits keep their stored relative
   order, except where two old cone tips tie, which is checked from the
   stored root timestamps. New hits are ordered by simulating Kahn's
   algorithm over the walked new subgraph. If the simulation ever has two
   ready commits tied on the top date, the answer is ambiguous, so the
   original runs and reseeds.
4. Every guard failure falls back to the unchanged original collector. That
   collector's complete answer reseeds the key, so no guard can produce a
   wrong order or a stale hit.

**Open risk, to measure before committing to P1':** guard 2 fails on every
show that follows a ref deletion. With this fleet's branch churn, the
warm-hit rate for positive keys may be low. If it is, P1' is a net loss,
because a reseed adds two snapshots to the original. The proposal is to
measure the guard-2 pass rate first, with a read-only probe over live
ref-churn between consecutive shows of the same specs, before any P1' code.

## Questions for the advisor

1. Approve P1 with ordering O1, or rule that the order is non-normative so
   that O2 is acceptable? *(Answered 23:55: O1. Superseded by Gap 1 above:
   O1 is not exact. Ruling needed on P1', or on measuring first.)*
2. Approve P2 as a fast path that is exact for `id_matches`, with the
   unchanged scan as the fallback?
3. Stale commit-graph: is an operator `git maintenance` run on the primary
   in scope as an environment fix before AC4 is measured, and should AC4
   numbers be reported both before and after it?
