//! `aida deputy` clap types, owned by `deputy_cmd.rs`.
// trace:STORY-B6B1278890774EF993D2EBC327862470-099 | ai:claude

use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

/// Run jobs other sessions submit, under the grant held by your terminal.
///
/// Start it once in your own terminal, from your shell (`aida deputy`, through
/// the `aida()` shell function). It refuses to start without a human
/// terminal, outside the shell function, or without a seat grant. Sessions
/// then hand it work with `aida deputy submit -- <command…>`, and get the exit
/// code and output back with `--wait`, `aida deputy status <id>` or
/// `aida deputy log <id>`.
///
/// A job runs on a terminal only when its command matches a TTY entry you set
/// here (`--allow-tty`, or `aida deputy allow`). Every other job runs with no
/// terminal, so a check that needs a person at the keyboard refuses it.
#[derive(Parser, Debug)]
#[command(
    name = "deputy",
    alias = "runloop",
    args_conflicts_with_subcommands = true
)]
pub(crate) struct DeputyArgs {
    #[command(subcommand)]
    pub command: Option<DeputyCommand>,
    #[command(flatten)]
    pub start: DeputyStartArgs,
}

#[derive(Args, Debug, Clone)]
pub(crate) struct DeputyStartArgs {
    /// Name of this deputy; each name has its own queue and ledger.
    #[arg(long, default_value = "default")]
    pub name: String,
    /// How many jobs run at once.
    #[arg(long, default_value_t = 2)]
    pub workers: usize,
    /// A command prefix that may run on a terminal (repeatable), e.g.
    /// 'aida merge-hold clear'.
    #[arg(long = "allow-tty", value_name = "PREFIX")]
    pub allow_tty: Vec<String>,
    /// How long each --allow-tty entry lasts (capped at your grant's expiry).
    #[arg(long, default_value = "8h")]
    pub ttl: String,
    /// An executable other than `aida` that jobs may run (repeatable).
    #[arg(long = "allow-exec", value_name = "PATH")]
    pub allow_exec: Vec<PathBuf>,
    /// Also run shell-script jobs (`submit -f FILE` / `--script`). Script jobs
    /// never get a terminal.
    #[arg(long)]
    pub allow_scripts: bool,
    /// A terminal job silent this long shows as waiting on a prompt in
    /// `aida deputy status`. It is never stopped for it; its --timeout
    /// still applies.
    #[arg(long, default_value = "10s")]
    pub tty_idle: String,
    /// Keep running after the mandate active at start expires or is
    /// stopped. Without it the deputy stops claiming then and exits once its
    /// running jobs finish. It always does so when your grant ends.
    // trace:STORY-B6B1278890774EF993D2EBC327862470-155 | ai:claude
    #[arg(long)]
    pub keep_after_mandate: bool,
}

#[derive(Subcommand, Debug)]
pub(crate) enum DeputyCommand {
    /// Queue a job for the deputy and print its id.
    Submit(DeputySubmitArgs),
    /// Show a job's state and result.
    Status(DeputyIdArgs),
    /// Print a job's output.
    Log(DeputyLogArgs),
    /// Cancel a queued or running job.
    Cancel(DeputyIdArgs),
    /// List jobs.
    List(DeputyListArgs),
    /// Let a command prefix run on a terminal (at your terminal only).
    Allow(DeputyAllowArgs),
    /// Remove a TTY entry by id or prefix (at your terminal only).
    Deny(DeputyDenyArgs),
    /// Show the live TTY entries.
    Policy(DeputyNameArgs),
    /// Stop claiming new jobs; running jobs finish.
    Pause(DeputyNameArgs),
    /// Claim jobs again after a pause (at your terminal only).
    Resume(DeputyNameArgs),
    /// Acknowledge failed jobs so `aida awaiting` stops listing them.
    Ack(DeputyAckArgs),
}

/// trace:STORY-B6B1278890774EF993D2EBC327862470-155 | ai:claude
#[derive(Args, Debug)]
pub(crate) struct DeputyAckArgs {
    /// The failed job ids to acknowledge.
    #[arg(required_unless_present = "all")]
    pub ids: Vec<String>,
    /// Acknowledge every failed job you submitted (every one, outside a
    /// session).
    #[arg(long, conflicts_with = "ids")]
    pub all: bool,
    /// Which deputy.
    #[arg(long, default_value = "default")]
    pub name: String,
}

#[derive(Args, Debug)]
pub(crate) struct DeputyNameArgs {
    /// Which deputy.
    #[arg(long, default_value = "default")]
    pub name: String,
}

#[derive(Args, Debug)]
pub(crate) struct DeputySubmitArgs {
    /// Which deputy.
    #[arg(long, default_value = "default")]
    pub name: String,
    /// Run one job of this class at a time (e.g. `ship`).
    #[arg(long, value_name = "CLASS")]
    pub serial: Option<String>,
    /// Wait for the job and exit with its exit code.
    #[arg(long)]
    pub wait: bool,
    /// Stop the job after this long.
    #[arg(long, default_value = "30m")]
    pub timeout: String,
    /// The spec this job is for (recorded in the ledger).
    #[arg(long, value_name = "SPEC-ID", value_parser = crate::spec_arg::spec_id)]
    pub spec: Option<String>,
    /// Run this file's contents as a bash script (needs a deputy started with
    /// --allow-scripts).
    #[arg(short = 'f', long = "file", value_name = "FILE", conflicts_with_all = ["script", "argv"])]
    pub file: Option<PathBuf>,
    /// Run this text as a bash script (needs --allow-scripts on the deputy).
    #[arg(long, value_name = "BODY", conflicts_with = "argv")]
    pub script: Option<String>,
    /// The command to run, after `--`.
    #[arg(last = true, value_name = "COMMAND")]
    pub argv: Vec<String>,
}

#[derive(Args, Debug)]
pub(crate) struct DeputyIdArgs {
    /// The job id `submit` printed.
    pub id: String,
    /// Which deputy.
    #[arg(long, default_value = "default")]
    pub name: String,
}

#[derive(Args, Debug)]
pub(crate) struct DeputyLogArgs {
    /// The job id `submit` printed.
    pub id: String,
    /// Which deputy.
    #[arg(long, default_value = "default")]
    pub name: String,
    /// Keep printing output until the job finishes.
    #[arg(long)]
    pub follow: bool,
}

#[derive(Args, Debug)]
pub(crate) struct DeputyListArgs {
    /// Which deputy.
    #[arg(long, default_value = "default")]
    pub name: String,
    /// Only jobs in this state (incoming, running, done, failed).
    #[arg(long)]
    pub state: Option<String>,
    /// At most this many jobs per state, newest first.
    #[arg(long, default_value_t = 20)]
    pub limit: usize,
}

#[derive(Args, Debug)]
pub(crate) struct DeputyAllowArgs {
    /// The command prefix, e.g. 'aida merge-hold clear'.
    pub prefix: String,
    /// Which deputy.
    #[arg(long, default_value = "default")]
    pub name: String,
    /// How long the entry lasts (capped at your grant's expiry).
    #[arg(long, default_value = "8h")]
    pub ttl: String,
    /// The entry is used up by the first job that matches it.
    #[arg(long)]
    pub single_use: bool,
}

#[derive(Args, Debug)]
pub(crate) struct DeputyDenyArgs {
    /// The entry id, or its exact prefix.
    pub entry: String,
    /// Which deputy.
    #[arg(long, default_value = "default")]
    pub name: String,
}
