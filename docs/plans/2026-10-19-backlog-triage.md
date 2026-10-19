# Backlog triage proposal — 2026-10-19

<!-- trace:TASK-1944 | ai:codex -->

Proposal only. No spec status, archive/defer flag, queue entry, or relationship was changed. Advisor decisions must precede any disposition writes.

Snapshot: `aida list --all --format json`, captured 2026-10-19, with code evidence pinned to `origin/main` at `6d590ed914dea5135a126f7dae6abb403adec66d`. The input contained 5,348 rows; 475 had open stored statuses: Approved 208, NeedsAttention 20, Draft 235, InProgress 12. InProgress is included by the operator's follow-up instruction; there were no Planned rows. Done, Completed, Rejected, and Superseded are excluded. Input SHA-256: `e908a1e7a016cb87a920bbe9f23a22c0688cefdd52abe5f5a843be7d2fa5ac8d`.

Coverage: **400/475** open specs. Each covered spec was read through `aida show <ID> --format json --no-git --comments`; TSV statuses preserve the list snapshot rather than the show's deferred display label. Open is not the same as actionable: the full input intentionally includes archived and deferred work. Among the 475 open rows, 139 are archived and 115 deferred; these counts can overlap. KEEP preserves those shelves and does not authorize pickup. 92 rows have no description; a title-only KEEP explicitly remains uncertain.

| Disposition | Count |
|---|---:|
| DONE-ALREADY | 2 |
| DUPLICATE-OF | 34 |
| SUPERSEDED-BY | 5 |
| STALE-REJECT | 1 |
| KEEP | 358 |

KEEP priorities: P1 79, P2 53, P3 226. `target` is the priority for KEEP, the covering ID for DUPLICATE-OF, and the cited replacement or mainline commit for other dispositions.

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

Validation: pending final artifact and self-check validation. No cargo commands are permitted for this docs-only task.
