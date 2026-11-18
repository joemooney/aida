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
        /// Window duration, in hours (24h) or days (7d).
        #[clap(long, default_value = "7d")]
        since: String,
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
