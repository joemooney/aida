//! `aida pr_cmd` clap types, owned by `pr_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Pull-request side-effects intended to fire from /aida-pr at PR-create
/// time. Mirrors what `aida session end` would do as a backup, but moves
/// the trigger to the moment context is freshest (right after `gh pr
/// create` returns the URL).
// trace:STORY-90 | ai:claude
#[derive(Subcommand, Debug)]
pub enum PrCommand {
    /// File the reviewer story for the change (PR/MR) open on the current
    /// branch and queue it to the `reviewer` role. Idempotent: skips when a
    /// `Review PR-<n>:` / `Review MR-<n>:` story already exists for it.
    /// Detects the change through the configured Forge — GitHub via `gh`
    /// (looks up the branch's open PR), GitLab via `glab` (looks up the
    /// branch's open MR) — so that provider's CLI must be on PATH and
    /// authenticated.
    ///
    /// Intended trigger: `/aida-pr` runs this right after the change opens
    /// (`gh pr create` / `glab mr create`) — the agent already has it open,
    /// the commit range, and the spec list cached, but the auto-queue logic
    /// does its own forge detection so the call is self-contained.
    ///
    /// `aida session end` also fires this as a backup, so a forgotten
    /// /aida-pr (or a raw `gh pr create` / `glab mr create`) still ends up
    /// routed to the reviewer. The idempotency guard means both firing is
    /// fine.
    // trace:STORY-90 trace:BUG-1609 | ai:claude
    AutoQueueReview {
        /// Branch to look up the PR for. Defaults to the current
        /// branch via `git branch --show-current`.
        #[clap(long, value_name = "BRANCH")]
        branch: Option<String>,
    },

    /// List every open PR/MR with its spec ID (when one can be found),
    /// CI state and rebase status.
    #[clap(visible_alias = "mr")]
    List {
        /// Emit the list as a JSON array.
        #[clap(long)]
        json: bool,
        /// Only show PRs that map to a spec ID; hide the unmapped ones.
        // trace:BUG-1927 | ai:claude
        #[clap(long)]
        mapped_only: bool,
    },

    /// Rebase a PR onto its base in a temporary worktree, then
    /// force-push-with-lease the rebased branch. Collapses the standard
    /// 6-command "rebase a PR before review" recipe into one call.
    ///
    /// Default mode aborts cleanly on conflicts (cleans up the temp
    /// worktree, prints the manual recipe, exits non-zero). `--check`
    /// reports stale-base + overlap + conflict-prediction without
    /// modifying anything. `--interactive` drops into the temp worktree
    /// on conflict so you can resolve + continue, then AIDA finishes the
    /// push. Cross-fork PRs are refused (force-push to a fork is the
    /// contributor's job).
    ///
    /// Force-push always uses `--force-with-lease`. The smoke check
    /// (default `cargo build --release` for Rust projects) is
    /// configurable via `.aida/config.toml` `[pr-rebase] smoke_check`.
    // trace:TASK-308 | ai:claude
    Rebase {
        /// PR number to rebase.
        #[clap(value_name = "N")]
        n: u64,

        /// Report-only: don't modify anything. Prints stale-base,
        /// overlapping files, and a best-effort conflict prediction.
        #[clap(long)]
        check: bool,

        /// On conflict, leave the temp worktree in place and prompt you
        /// to resolve + `git rebase --continue`, then finish the push.
        #[clap(long, conflicts_with = "check")]
        interactive: bool,

        /// Skip the post-rebase build smoke check.
        #[clap(long)]
        no_smoke: bool,

        /// Rebase onto this explicit base ref instead of the PR's
        /// declared base. Useful for PRs against non-main branches.
        #[clap(long, value_name = "REF")]
        base: Option<String>,

        /// Stack-aware rebase: replay only the commits AFTER this SHA
        /// (the recorded fork-point of the branch this PR was stacked
        /// on), via `git rebase --onto <base> <SHA>`. Use when the
        /// stacked parent was squash-merged and its branch deleted — a
        /// plain rebase would re-apply the parent's pre-squash commits.
        /// Refused when SHA is not an ancestor of the PR head (a stale
        /// stack record).
        // trace:TASK-1080 | ai:claude
        #[clap(long, value_name = "SHA", conflicts_with = "check")]
        onto_parent: Option<String>,
    },

    /// Ship a PR: create-if-needed → watch CI → squash-merge → pull →
    /// worktree-aware cleanup. The direct-publish counterpart to
    /// `aida queue work PR-N --auto-complete` (which drives the full
    /// reviewer pipeline) — use this for human-pre-approved work where
    /// no orchestrator review phase is needed (docs PRs, master-signed
    /// architecture work, recovery merges).
    ///
    /// With no `<N>`, the command targets the current branch's open PR
    /// — creating one via `gh pr create` (deriving title/body from the
    /// latest commit) if none exists. `aida pull` runs from the main
    /// worktree even when invoked from a sibling worktree, and the
    /// merge step detects branches checked out in sibling worktrees
    /// before deciding whether `--delete-branch` is safe — both
    /// papercuts surfaced by the 2026-05-22 stopgap-shell experience.
    ///
    /// Composes with the transient-retry layer for the `gh` calls
    /// (sub-second network blips no longer abort the flow).
    // trace:TASK-458 | ai:claude
    // trace:BUG-286 | ai:claude
    // trace:TASK-487 | ai:claude
    #[clap(
        after_help = "Exit codes: 0 merged (or already merged; dry-run previews); 1 other error; 20 CI red; 21 ownership/CI wait timed out; 22 needs rebase/not mergeable; 23 needs review/approval at head; 24 stale CI definition; 25 merge-hold present."
    )]
    Ship {
        /// PR number to ship. When omitted, the command resolves the
        /// PR open on the current branch (or creates one if none
        /// exists).
        #[clap(value_name = "N")]
        n: Option<u64>,

        /// Skip the post-merge `aida pull`. Useful when shipping
        /// from inside a composition that pulls separately.
        #[clap(long)]
        no_pull: bool,

        /// Skip the `aida session end` worktree-cleanup step. Useful
        /// when you want to inspect the post-merge state before the
        /// worktree disappears.
        #[clap(long)]
        no_cleanup: bool,

        /// Print the resolved sequence without executing any of it.
        #[clap(long)]
        dry_run: bool,

        /// Bound drive ownership plus CI registration/settlement wait in seconds.
        /// Bare --wait uses 300 seconds; omitted leaves CI unbounded and refuses
        /// drive-owned PRs immediately. Drive seats always refuse immediately.
        /// Exit codes: 20 CI red, 21 CI timeout, 22 not mergeable, 23 review,
        /// 24 stale CI definition, 25 merge hold (0 merged/already merged).
        // trace:TASK-1602 trace:TASK-1606 | ai:codex
        #[clap(long, value_name = "SECS", num_args = 0..=1, default_missing_value = "300")]
        wait: Option<u64>,

        /// Delete the merged branch even when branches/PRs are stacked on
        /// it. Without this, ship keeps the branch alive when it detects
        /// stacked children, so deleting it can't auto-close their PRs;
        /// pass this to orphan the children deliberately.
        // trace:BUG-434 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long)]
        force_delete_branch: bool,

        /// Implementer's self-assessed actual complexity at ship time:
        /// `low` / `med` / `high`. Captured to
        /// `.aida/complexity-calibration/<SPEC>.yaml` alongside the
        /// punt count from `.aida/punts.jsonl` — feeds the three-way
        /// calibration view (`aida autonomy calibration mismatches`).
        /// Best-effort, not graded: estimate is a substrate-knowledge
        /// signal, never an approval criterion. Absent ⇒ ship slot
        /// records only the punt count.
        // trace:STORY-439 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long, value_enum, value_name = "LEVEL")]
        complexity: Option<crate::complexity_calibration::ComplexityLevel>,

        /// Implementer's actual effort spent: 15m, 1h, 4h, 1d, or 1w.
        /// Captured to `.aida/effort-calibration/<SPEC>.yaml` as the
        /// ship/implementation touchpoint. `1d` is 8 work-hours; `1w`
        /// is 40 work-hours.
        // trace:STORY-451 | ai:codex
        #[clap(long, value_enum, value_name = "BUCKET")]
        effort: Option<crate::effort_calibration::EffortBucket>,

        /// Bypass the client-side trailer spec-ID check (Guard 1). Ship even
        /// when a commit's `(SPEC-ID)` trailer does not resolve to a live
        /// spec. Use only when you know the trailer is intentional (e.g. a
        /// cross-repo id the local store can't see).
        // trace:STORY-469 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long)]
        no_trailer_check: bool,

        /// Ship even when the PR's green check completed before a
        /// CI-definition file (a workflow, or a script it invokes directly)
        /// changed on the base branch. Without this, ship refuses so a
        /// green that no longer means what it looks like doesn't merge
        /// unnoticed. A plain test-file change on the base branch only
        /// warns and never needs this flag.
        // trace:BUG-1468 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long)]
        override_stale_check: bool,

        /// Ship even when the PR's newest recorded approval does not cover
        /// its current head — the head moved past the approved commit, the
        /// approval names no commit, or the head could not be read. Without
        /// this, ship refuses: re-review the current head instead.
        // trace:TASK-1448 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long)]
        override_stale_approval: bool,

        /// Ship this PR as a non-completing slice: write no completing
        /// `(SPEC-ID)` trailer into the squash subject from any source
        /// (title trailer or branch name), so the merge completes no spec.
        /// Use for one PR of a multi-PR spec. A PR body line reading
        /// `aida: no-complete` or `Non-completing slice` has the same
        /// effect; and the branch-name fallback is skipped on its own when
        /// the spec carries `closure:pending` or has open slice children.
        // trace:BUG-1919 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long)]
        no_complete: bool,

        /// Rebase-merge the PR (`gh pr merge --rebase --match-head-commit <SHA>`)
        /// instead of squash-merging, preserving each slice commit on the base
        /// branch. Runs the same approval, authority, CI, and merge-hold gates
        /// as squash. Rebase merges produce no squash subject and complete no
        /// spec unless `--final-slice` is also passed.
        // trace:STORY-1663 | ai:antigravity
        #[clap(long)]
        rebase: bool,

        /// Explicit merge method (`squash` or `rebase`). Equivalent to `--rebase`
        /// when set to `rebase`.
        // trace:STORY-1663 | ai:antigravity
        #[clap(long, value_enum, value_name = "METHOD", conflicts_with = "rebase")]
        merge_method: Option<crate::pr_ship::ShipMergeMethod>,

        /// Complete the PR's resolved spec(s) after a `--rebase` merge lands.
        /// Without this flag, a rebase merge treats the PR as a non-completing
        /// slice and leaves the owning spec's status unchanged.
        // trace:STORY-1663 | ai:antigravity
        #[clap(long, visible_alias = "complete-spec", conflicts_with = "no_complete")]
        final_slice: bool,
    },

    /// Ship several PRs that need a human merge, one at a time, in order.
    ///
    /// For each PR, in the order given, it refreshes the branch from main only
    /// when the PR is behind or its green predates a CI-definition change,
    /// waits for CI on that exact head, waits for an independent approval
    /// recorded at that head (it queues the PR for review but never records
    /// a verdict), then
    /// asks you to type the PR number before shipping it with every
    /// `aida pr ship` gate in force. A merge-hold on a confirmed PR is
    /// cleared only when you also type `clear`.
    ///
    /// Any failure stops the batch without shipping: red or missing CI, a
    /// conflict, no approval in time, a refused gate, or an unconfirmed
    /// prompt. Re-run the same command to continue; merged PRs are skipped.
    /// Runs only for a human at an interactive terminal, on GitHub.
    // trace:STORY-1887 | ai:claude
    ShipBatch {
        /// PR numbers, shipped in exactly this order (at most 20, no repeats).
        #[clap(required = true, value_name = "PR")]
        prs: Vec<u64>,
        /// How long to wait for an approval at each PR's head, e.g. `45m`,
        /// `2h` (a bare number is minutes; default 60m).
        #[clap(long, value_name = "DURATION", value_parser = parse_review_timeout_secs)]
        review_timeout: Option<u64>,
        /// Show each PR's state and what the batch would do; change nothing.
        /// It does not fetch: staleness is judged against your local
        /// `origin/*` refs as they are.
        #[clap(long)]
        dry_run: bool,
    },

    /// Deliberately HOLD the PR on the current session — push the branch but
    /// intentionally do not open the PR yet, pending a manual gate (a smoke
    /// test, an out-of-band review, an operator decision).
    ///
    /// Under an `--auto-complete` drain this is a clean finish outcome, NOT a
    /// failure: the orchestrator reads the hold signal this writes and reports
    /// a `Held` outcome (branch pushed, PR held) with the right recovery hint
    /// instead of mis-filing the missing PR as a phase-1 failure. Resolve the
    /// spec + branch from the active session lease; record `{spec, branch,
    /// reason}` for the orchestrator handshake.
    // trace:BUG-250 | ai:claude
    Hold {
        /// Why the PR is held — the gate you're running first. Surfaced in the
        /// drain epilogue.
        // trace:BUG-1294 | ai:claude
        #[clap(long, value_name = "REASON", allow_hyphen_values = true)]
        reason: Option<String>,
    },

    /// Sweep local review-snapshot branches (`pr-N` / `mr-N`, created by
    /// `aida session start --owns PR-N` / `aida pr rebase` when they fetch a
    /// change's head ref for headless review) whose change has reached a
    /// terminal state (merged or closed). Nothing removes these today, so a
    /// long-running project's local branch namespace accumulates one per
    /// review, forever.
    ///
    /// A candidate is deleted only when: the change is merged/closed (an
    /// open change is left alone — the review may still need it), the
    /// branch isn't checked out in any worktree, and the branch's tip still
    /// matches the change's last known head SHA (a branch that gained local
    /// commits since the fetch is left alone rather than guessed at). Every
    /// skip is reported with its reason. Scoped to the exact `pr-<digits>` /
    /// `mr-<digits>` name shape `aida session start --owns PR-N` creates —
    /// never touches an authored spec branch.
    ///
    /// Opt-in: this command is never run automatically. Use `--dry-run` to
    /// preview before deleting.
    // trace:TASK-1312 | ai:claude
    Gc {
        /// Report what would be deleted without deleting anything.
        #[clap(long)]
        dry_run: bool,
    },
}

/// `--review-timeout` for `aida pr ship-batch`, in seconds.
// trace:STORY-1887 | ai:claude
fn parse_review_timeout_secs(raw: &str) -> Result<u64, String> {
    crate::drain_caps::parse_duration(raw)
        .map(|d| d.as_secs())
        .filter(|secs| *secs > 0)
        .ok_or_else(|| format!("`{raw}` is not a duration like `45m` or `2h`"))
}
