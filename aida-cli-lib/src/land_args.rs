//! `aida land` clap types, owned by `land.rs`.
// trace:STORY-2768 | ai:claude

use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub enum LandCommand {
    /// Print every open change's landing state and the one action it calls
    /// for. Records readiness observations; nothing is dispatched, cleared or merged.
    Status,
    /// Report landing throughput, READY waits and review rounds.
    // trace:TASK-3813 | ai:codex
    Metrics {
        /// Inclusive start timestamp or lookback from the window end.
        #[clap(long, default_value = "7d")]
        since: String,
        /// Exclusive end timestamp (defaults to now).
        // trace:TASK-3817 | ai:codex
        #[clap(long)]
        until: Option<String>,
        /// Machine-readable JSON report.
        #[clap(long)]
        json: bool,
    },
    /// Run the pass and carry out each change's action: launch reviewers
    /// and rework lanes as child processes, release holds whose condition
    /// is met, and send every ready change to one train. Run it in a
    /// terminal that holds a seat grant, with autoland and the review
    /// watcher stopped.
    Run(RunArgs),
    /// Print which open changes are stuck, why, and which seat can move
    /// them. Also lists recent landings and lanes per host. Nothing is
    /// routed.
    // trace:STORY-2906 | ai:claude
    Stalls {
        /// Machine-readable JSON report.
        #[clap(long)]
        json: bool,
    },
    /// Show or change the landing mode, keeping the forge's required checks
    /// in step with it.
    // trace:STORY-2897 | ai:claude
    Mode {
        #[clap(subcommand)]
        action: ModeCommand,
    },
    /// Check the default branch's CI once and report: name the first red
    /// commit, mail the advisor, and recommend a revert only when main does
    /// not build or stays red past the fix window. It writes nothing to the
    /// requirement store, and runs as its own scheduled job, never inside
    /// the land loop or a train.
    // trace:STORY-2897 | ai:claude
    Watch,
}

/// `aida land mode` verbs.
// trace:STORY-2897 | ai:claude
#[derive(Subcommand, Debug)]
pub enum ModeCommand {
    /// Print the landing mode, the checks it requires, what the forge
    /// requires, any drift between them, and the recent changes.
    Status,
    /// Set the landing mode: replace the forge's required checks with the
    /// mode's list, read them back, and record the mode in
    /// `.aida/config.toml`. Needs the operator at a terminal, or the advisor
    /// seat under a mandate that includes protected paths.
    Set {
        /// `ci-gated` or `review-gated`.
        mode: String,
    },
    /// Re-apply the current mode's required checks to the forge.
    Reconcile,
}

#[derive(clap::Args, Debug)]
pub struct RunArgs {
    /// Run one pass, wait for the children it started, and exit.
    #[clap(long)]
    pub once: bool,
    /// Seconds between passes.
    #[clap(long, default_value_t = 120, value_parser = clap::value_parser!(u64).range(30..=3600))]
    pub interval: u64,
    /// Print what a pass would do and do none of it.
    #[clap(long)]
    pub dry_run: bool,
    /// The vendor reviewers run on; by default the usual selection applies.
    /// Rework lanes always keep the vendor their change already has.
    #[clap(long, value_name = "VENDOR")]
    pub vendor: Option<String>,
}
