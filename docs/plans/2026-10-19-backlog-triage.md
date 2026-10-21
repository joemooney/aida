# Backlog triage proposal — 2026-10-19

<!-- trace:TASK-1944 | ai:codex -->

Proposal only. No spec status, archive/defer flag, queue entry, or relationship was changed. Advisor decisions must precede any disposition writes.

Snapshot: `aida list --all --format json`, captured 2026-10-19, with code evidence pinned to `origin/main` at `6d590ed914dea5135a126f7dae6abb403adec66d`. The input contained 5,348 rows; 475 had open list-reported statuses: Approved 208, NeedsAttention 20, Draft 235, InProgress 12. InProgress is included by the operator's follow-up instruction; there were no list-reported Planned rows. Done, Completed, Rejected, and Superseded are excluded. Input SHA-256: `e908a1e7a016cb87a920bbe9f23a22c0688cefdd52abe5f5a843be7d2fa5ac8d`.

Coverage: **475/475** open specs in the captured input. TASK-1945 (review-on-push pipeline) was filed after capture and assigned as a later item in the active batch; it is intentionally outside this frozen proposal, not an untriaged omission. Each covered contract was read through `aida show <ID> --format json --no-git --comments`; removal candidates and their targets also received full TOON comment reads, because the JSON projection does not include comment bodies. TSV statuses preserve the list-reported snapshot, which can differ from a container's stored status or the show's deferred display label. Open is not the same as actionable: the full input intentionally includes archived and deferred work. Among the 475 open rows, 139 are archived and 115 deferred; these counts can overlap. KEEP preserves those shelves and does not authorize pickup. 92 rows have no description; a title-only KEEP explicitly remains uncertain.

| Disposition | Count |
|---|---:|
| DONE-ALREADY | 2 |
| DUPLICATE-OF | 40 |
| SUPERSEDED-BY | 4 |
| STALE-REJECT | 1 |
| KEEP | 428 |

KEEP priorities: P1 116, P2 79, P3 233. `target` is the priority for KEEP, the covering ID for DUPLICATE-OF, and the cited replacement or mainline commit for other dispositions.

TSV evidence separates proof of removal from conservative retention. A merged commit or a trace alone does not close a reopened or umbrella spec; an unverified observation stays KEEP. DUPLICATE-OF means the target contract contains the source obligation, including when the target is broader; preserve the source evidence when consolidating. P1 is a dev-cycle/unblocking recommendation, not a change to stored priority or authority. P2 is substantive follow-on work. P3 includes cosmetics and conditional or shelved work. The TSV places the recommended P1 queue first, then evidence-backed removal candidates, then the remaining KEEPs by priority.

Recommended P1 KEEPs, in order (apply existing holds, dependencies, and review requirements):

1. **BUG-2190** — aida list (open lens) reports 'count: 0 of 0 open' while ~470 specs are Draft/Approved/NeedsAttention/InProgress; --status filters also return 0.
2. **BUG-2187** — Headless reviewer cannot record its verdict when AIDA_AGENT_TYPE/NAME is unset, so a finished review is lost.
3. **BUG-2173** — Attested reviewer cannot record a verdict in a non-AIDA-repo project (aida-relay).
4. **BUG-2188** — aida agent new --spec fails with an uninformative worktree error and leaves orphan leases on refusal.
5. **BUG-2168** — aida agent new --bg --cwd <worktree> without --spec dispatches into the primary checkout instead of the given cwd.
6. **BUG-2169** — Contained implementer seats cannot run git in their own worktree: write roots omit the worktree's gitdir and the common dir, so commit/fetch/merge fail with Read-only file system.
7. **BUG-2177** — Approved PRs chase main forever: a clean merge-from-main commit makes the approval stale, and the reviewer refuses to review a branch that is behind main.
8. **BUG-2179** — Mandate ship runs an unbounded 'git log --all' per traced spec over the whole store; aida pr ship sits at step 1 for 10-40 min.
9. **BUG-2184** — pr ship: the slow mandate scan uses up the --wait budget, then the CI wait times out without polling and reports 'checks have not registered'.
10. **BUG-2186** — Rework merge-holds never auto-release: release reader requires verdict 'rounds' the writer no longer records.
11. **BUG-2180** — Reviewer request-changes shelve leaves spec writes uncommitted in the store.
12. **BUG-2174** — Every PR prepends a 'recent change' paragraph to AGENTS.md/OVERVIEW.md, so each merge re-conflicts every other open PR.
13. **STORY-1853** — Cut test-suite wall time: CI Run tests ≤6 min, local full run ≤10 min on 6 cores.
14. **STORY-1854** — Callers hand cargo time budgets to the wrapper (AIDA_CARGO_TIMEOUT) instead of an outer timeout.
15. **STORY-1728** — Per-module clap subcommand registration so command changes stop touching cli.rs / lib.rs.

There are 47 proposed removal/consolidation dispositions: 31 are currently neither archived nor deferred, 11 archived, and 5 deferred. The 428 KEEPs include 209 currently unshelved rows; the ordered 15 above are the recommended next focus, not permission to drain every KEEP.

Status provenance: six containers have different list-reported/stored values: EPIC-19 Draft/InProgress, EPIC-31 Draft/Approved, EPIC-77 InProgress/Draft, EPIC-22 InProgress/Draft, EPIC-28 InProgress/Planned, and EPIC-44 InProgress/Approved. All remain open under either projection; the requested list snapshot controls the TSV.

The first priorities repair backlog visibility, reviewer verdict capture and worker launch, then unblock ship/rework and reduce repeated merge/build costs. BUG-2190 is an undisclosed persisted-focus issue per its 2026-10-19 investigation comment; no status-normalization fix is asserted here.

Validation: exact frozen coverage (475 unique IDs, including all 12 list-reported InProgress rows), six-column TSV schema, snapshot statuses/titles, nonempty evidence, target IDs/no duplicate cycles, summary counts/priorities/top 15, and DONE commit ancestry passed. Only the two requested artifacts differ from the source base; `git diff --check` passed. `check-docs-guards-scheduled.py` and the static CLI-manual hidden-flag guard passed. Ten progress checkpoints were committed at 50-spec intervals and the final 25. The author self-check loop used two fresh-context agents; round 1 found an incomplete timeout closure, a non-contained alias obligation and status-provenance wording, all corrected; round 2 found no request-changes findings. Follow-up comment checks also retained the held agent-lifecycle umbrella and the separable Linux ratchet. These checks are author checks, not independent review. KEEP incompleteness was not exhaustively verified, and uncertain rows remain conservative. No cargo/build/runtime tests or `make check-ci-fast` were run under the explicit docs-only/no-Cargo exception; its current fast plan includes cargo check and cargo fmt.
