//! `aida deputy shell` clap types, owned by `deputy_shell_cmd.rs`. The
//! command is the `Shell` variant of `aida deputy`'s subcommands, not a
//! top-level command of its own.
// trace:STORY-2338 | ai:claude

use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Args, Debug)]
pub(crate) struct DeputyShellArgs {
    #[command(subcommand)]
    pub command: DeputyShellCommand,
}

#[derive(Subcommand, Debug)]
pub(crate) enum DeputyShellCommand {
    /// Start the script deputy in this terminal (your terminal only).
    Run(ShellRunArgs),
    /// Queue a script for the script deputy and print its id.
    Submit(ShellSubmitArgs),
    /// List script jobs and whether the runner is live.
    List(ShellListArgs),
    /// Show a script job's state and result.
    Status(ShellIdArgs),
    /// Print a script job's transcript.
    Log(ShellLogArgs),
    /// Wait for a script job and exit with its exit code.
    Wait(ShellWaitArgs),
    /// Cancel a queued or running script job.
    Cancel(ShellIdArgs),
    /// Stop claiming new script jobs; a running job finishes.
    Pause(ShellNameArgs),
    /// Claim script jobs again after a pause (at your terminal only).
    Resume(ShellNameArgs),
    /// Acknowledge failed script jobs so `aida awaiting` stops listing them.
    Ack(ShellAckArgs),
}

#[derive(Args, Debug, Clone)]
pub(crate) struct ShellRunArgs {
    /// Name of this script deputy; each name has its own queue and audit log.
    #[arg(long, default_value = "default")]
    pub name: String,
    /// Another seat that may submit scripts (repeatable). The advisor seat
    /// always may.
    #[arg(long = "allow-seat", value_name = "SEAT")]
    pub allow_seat: Vec<String>,
    /// A seat the refreshed grant may delegate (repeatable; default advisor,
    /// implementer, reviewer and integrator).
    #[arg(long = "delegate-seat", value_name = "SEAT")]
    pub delegate_seat: Vec<String>,
    /// Re-mint the grant this often, between jobs.
    #[arg(long, default_value = "12h")]
    pub refresh: String,
    /// Keep finished jobs this many days (default: `[deputy_shell] keep_days`
    /// in `.aida/config.toml`, else 30).
    #[arg(long, value_name = "DAYS")]
    pub keep_days: Option<u32>,
    /// Keep running after the mandate active at start expires or is
    /// stopped. Without it the deputy stops claiming then.
    #[arg(long)]
    pub keep_after_mandate: bool,
}

#[derive(Args, Debug)]
pub(crate) struct ShellSubmitArgs {
    /// Which script deputy.
    #[arg(long, default_value = "default")]
    pub name: String,
    /// The bash script to run.
    #[arg(short = 'f', long = "file", value_name = "FILE")]
    pub file: PathBuf,
    /// Stop the script after this long.
    #[arg(long, default_value = crate::deputy_shell::DEFAULT_TIMEOUT)]
    pub timeout: String,
    /// The spec this script is for (recorded with the job).
    #[arg(long, value_name = "SPEC-ID", value_parser = crate::spec_arg::spec_id)]
    pub spec: Option<String>,
    /// Wait for the script and exit with its exit code.
    #[arg(long)]
    pub wait: bool,
}

#[derive(Args, Debug)]
pub(crate) struct ShellNameArgs {
    /// Which script deputy.
    #[arg(long, default_value = "default")]
    pub name: String,
}

#[derive(Args, Debug)]
pub(crate) struct ShellIdArgs {
    /// The job id `submit` printed.
    pub id: String,
    /// Which script deputy.
    #[arg(long, default_value = "default")]
    pub name: String,
}

#[derive(Args, Debug)]
pub(crate) struct ShellLogArgs {
    /// The job id `submit` printed.
    pub id: String,
    /// Which script deputy.
    #[arg(long, default_value = "default")]
    pub name: String,
    /// Keep printing the transcript until the job finishes.
    #[arg(long)]
    pub follow: bool,
}

#[derive(Args, Debug)]
pub(crate) struct ShellWaitArgs {
    /// The job id `submit` printed.
    pub id: String,
    /// Which script deputy.
    #[arg(long, default_value = "default")]
    pub name: String,
    /// Give up after this long (exit 75; the job keeps its place).
    #[arg(long)]
    pub timeout: Option<String>,
}

#[derive(Args, Debug)]
pub(crate) struct ShellListArgs {
    /// Which script deputy.
    #[arg(long, default_value = "default")]
    pub name: String,
    /// Only jobs in this state (queued, running, done, cancelled).
    #[arg(long)]
    pub state: Option<String>,
    /// At most this many jobs, newest first.
    #[arg(long, default_value_t = 20)]
    pub limit: usize,
}

#[derive(Args, Debug)]
pub(crate) struct ShellAckArgs {
    /// The failed job ids to acknowledge.
    #[arg(required_unless_present = "all")]
    pub ids: Vec<String>,
    /// Acknowledge every failed script job you submitted (every one, outside
    /// a session).
    #[arg(long, conflicts_with = "ids")]
    pub all: bool,
    /// Which script deputy.
    #[arg(long, default_value = "default")]
    pub name: String,
}
