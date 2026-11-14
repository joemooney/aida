//! `aida integration` clap types, owned by `integration_cmd.rs` and
//! re-exported from `cli.rs`.
// trace:STORY-2737 | ai:claude

use clap::Subcommand;

/// Subcommands under `aida integration`: the integration-branch flow, where
/// work lands fast on `integration` and `main` is the batched checkpoint.
// trace:STORY-2737 | ai:claude
#[derive(Subcommand, Debug)]
pub enum IntegrationCommand {
    /// Tag the open specs whose completing commits are on `origin/integration`
    /// but not yet on `origin/main` with `landed-in-integration`, and remove
    /// the tag from specs that are closed or no longer ahead of `main`.
    /// Reads the local `origin/` refs; run `git fetch` first if they may be
    /// stale. Completion itself still happens when the commit reaches `main`.
    Tags {
        /// Print the tag changes without making them.
        #[arg(long)]
        dry_run: bool,
    },
    /// Merge one open change into `integration`. Run it in the integration
    /// worktree, on `integration`, clean and equal to `origin/integration`.
    /// The change is admitted when it is approved at its head, or, with
    /// `[integration] review_after_merge = "low-risk"`, when it is low-risk
    /// enough to be reviewed after the merge; an armed merge hold refuses
    /// either way. The configured checks then run on the merge result, and a
    /// failing check puts the branch back where it was.
    // trace:STORY-2772 | ai:claude
    Merge {
        /// The change's number.
        pr: u64,
        /// Push `integration` once the checks pass.
        #[arg(long)]
        push: bool,
        /// Say whether the change would be admitted, and merge nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Check that everything on `integration` not yet promoted to `main` was
    /// reviewed: each merged change needs an approval at exactly the head
    /// that was merged. Exits 1 and names each change that lacks one, with
    /// the revert that takes it out. Reads the local `origin/` refs. A
    /// promotion is squashed into `main`, so git alone cannot always tell
    /// which commits it carried: pass `--since` to say where the last one
    /// was cut.
    // trace:STORY-2772 | ai:claude
    PromotionCheck {
        /// The commit to check; `origin/integration` when omitted.
        #[arg(long, value_name = "COMMIT")]
        tip: Option<String>,
        /// The integration commit the last promotion was cut from: only
        /// commits after it are checked. Without it the check goes back to
        /// the newest integration commit whose content is exactly a commit
        /// on `main`, which a promotion leaves only when `main` took nothing
        /// else in between.
        #[arg(long, value_name = "COMMIT")]
        since: Option<String>,
    },
}
