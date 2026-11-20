//! `aida lane` clap types, owned by `lane_cmd.rs` and re-exported from
//! `cli.rs`. trace:BUG-2383 | ai:claude
//!
//! `aida lane job run` exits with the run's exit code when it finishes within
//! `--wait`, else prints the job id and exits 75; `aida lane job wait <ID>`
//! continues. `AIDA_CARGO_TIMEOUT=<secs>` in the caller's environment sets the
//! run's timeout (default 900, at most 3600).

use clap::Subcommand;
use std::path::PathBuf;

/// `aida lane` subcommands.
// trace:BUG-2383 | ai:claude
#[derive(Subcommand, Debug, Clone)]
pub enum LaneCommand {
    /// Release a host fast-gate review hold without granting publication authority.
    // trace:STORY-2640 | ai:codex
    FastGateRelease {
        /// Exact lane branch whose receipt should be settled.
        branch: String,
    },
    /// Hand cargo runs to the lane's launcher, outside the tool-call time limit.
    #[clap(subcommand)]
    Job(LaneJobCommand),
    /// Relay a brief to an implementer lane on a lane host, gated on the PR head.
    ///
    /// Posts the brief on the spec as its authoritative round brief (earlier
    /// ones are marked superseded), waits until no lane for the spec runs on
    /// the host and the host has memory headroom, then delivers the brief and
    /// starts the lane. With `--pr` on an open change it starts a rework of
    /// that change, and only while the change is still open at the head seen
    /// at invocation; a merged or closed change starts fresh work instead.
    /// Anything that stops the launch is recorded on the spec and sent to
    /// the advisor.
    // trace:TASK-3826 | ai:claude
    Relay(RelayArgs),
    /// The lane-host side of `aida lane relay`.
    // trace:TASK-3826 | ai:claude
    #[clap(subcommand, hide = true)]
    RelayHost(RelayHostCommand),
}

/// `aida lane relay` arguments.
// trace:TASK-3826 | ai:claude
#[derive(clap::Args, Debug, Clone)]
pub struct RelayArgs {
    /// The spec the lane works.
    #[clap(value_parser = crate::spec_arg::spec_id)]
    pub spec: String,
    /// The open change this brief reworks. Its head is recorded now and
    /// checked again just before the launch.
    #[clap(long, value_name = "N")]
    pub pr: Option<u64>,
    /// The brief to relay.
    #[clap(long, value_name = "FILE")]
    pub brief: PathBuf,
    /// The agent that runs the lane, by its launch name. Defaults to
    /// `[lane_relay] vendor`, then the project's default.
    #[clap(long, value_name = "VENDOR", value_parser = crate::vendors::launch_vendor_name)]
    pub vendor: Option<String>,
    /// Model for this run. Defaults to the vendor's implementer model in
    /// project config; when neither is set the lane host's own default applies.
    #[clap(long, value_name = "MODEL")]
    pub model: Option<String>,
    /// SSH alias of the lane host. Defaults to `host` in
    /// `~/.aida/lane-relay.toml`; without either the lane starts on this
    /// machine.
    #[clap(long, value_name = "HOST")]
    pub host: Option<String>,
    /// The project checkout on the lane host, relative to its home directory
    /// or absolute. Defaults to `host_dir` in `~/.aida/lane-relay.toml`, then
    /// this checkout's path relative to the local home directory.
    #[clap(long, value_name = "DIR")]
    pub host_dir: Option<String>,
    /// Round label in the brief's heading. Defaults to REWORK for an open
    /// change and FRESH otherwise.
    #[clap(long, value_name = "LABEL")]
    pub label: Option<String>,
    /// Seconds to wait for a running lane to exit and for host memory
    /// headroom before giving up.
    #[clap(long, value_name = "SECS", default_value_t = 21_600)]
    pub wait_timeout: u64,
    /// Seconds to watch the started lane for its admission result.
    #[clap(long, value_name = "SECS", default_value_t = 2_400)]
    pub admission_timeout: u64,
    /// Seconds between checks while waiting.
    #[clap(long, value_name = "SECS", default_value_t = 60, value_parser = clap::value_parser!(u64).range(1..=3600))]
    pub poll: u64,
}

/// `aida lane relay-host` subcommands: what the relay runs on the lane host,
/// from the project's primary checkout there. Each prints one JSON object.
// trace:TASK-3826 | ai:claude
#[derive(Subcommand, Debug, Clone, PartialEq, Eq)]
pub enum RelayHostCommand {
    /// Whether a lane for the spec is running, and any memory shortfall.
    Probe { spec: String },
    /// Store a brief for the vendor and retire earlier pending ones.
    PlaceBrief {
        spec: String,
        #[clap(long, value_parser = crate::vendors::launch_vendor_name)]
        vendor: String,
        #[clap(long)]
        label: String,
        /// The brief body, base64.
        #[clap(long)]
        body_b64: String,
    },
    /// Start the lane, detached.
    Launch {
        spec: String,
        #[clap(long, value_parser = crate::vendors::launch_vendor_name)]
        vendor: String,
        #[clap(long)]
        branch: Option<String>,
        #[clap(long)]
        model: Option<String>,
        #[clap(long)]
        brief: PathBuf,
    },
    /// The admission result of the lane a launch started.
    Admission {
        spec: String,
        /// RFC 3339 time the launch reported.
        #[clap(long)]
        launched_at: String,
        /// The launch log the launch reported.
        #[clap(long)]
        log: PathBuf,
    },
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
