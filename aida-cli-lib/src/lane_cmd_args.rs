//! `aida lane` clap types, owned by `lane_cmd.rs` and re-exported from
//! `cli.rs`. trace:BUG-2383 | ai:claude
//!
//! `aida lane job run` exits with the run's exit code when it finishes within
//! `--wait`, else prints the job id and exits 75; `aida lane job wait <ID>`
//! continues. `AIDA_CARGO_TIMEOUT=<secs>` in the caller's environment sets the
//! run's timeout (default 900, at most 3600).

use clap::Subcommand;

/// `aida lane` subcommands.
// trace:BUG-2383 | ai:claude
#[derive(Subcommand, Debug, Clone)]
pub enum LaneCommand {
    /// Hand cargo runs to the lane's launcher, outside the tool-call time limit.
    #[clap(subcommand)]
    Job(LaneJobCommand),
}

/// `aida lane job` subcommands.
// trace:BUG-2383 | ai:claude
#[derive(Subcommand, Debug, Clone)]
pub enum LaneJobCommand {
    /// Hand one cargo run to the launcher and wait up to --wait seconds.
    Run {
        /// Seconds to wait before returning, at most 55.
        #[clap(long, default_value_t = 50, value_parser = clap::value_parser!(u64).range(0..=55))]
        wait: u64,
        /// Lines of output to print when the run ends.
        #[clap(long, default_value_t = 120)]
        tail: usize,
        /// The cargo command, after `--`: `cargo test|check|build --profile agent ...`.
        #[clap(last = true, required = true, value_name = "CARGO_ARGV")]
        argv: Vec<String>,
    },
    /// Keep waiting for a job `aida lane job run` started.
    Wait {
        /// The job id `aida lane job run` printed.
        id: String,
        /// Seconds to wait before returning, at most 55.
        #[clap(long, default_value_t = 50, value_parser = clap::value_parser!(u64).range(0..=55))]
        wait: u64,
        /// Lines of output to print when the run ends.
        #[clap(long, default_value_t = 120)]
        tail: usize,
    },
}
