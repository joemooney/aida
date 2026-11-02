//! `aida lane job run|wait`: the seat side of the lane job runner
//! ([`crate::lane_job`]). A headless implementer hands a cargo run to its
//! launcher and polls it in steps that fit one tool call.
// trace:BUG-2383 | ai:claude

use anyhow::Result;
use std::time::Duration;

#[path = "lane_cmd_args.rs"]
pub(crate) mod lane_cmd_args;

use crate::cli::{LaneCommand, LaneJobCommand};
use crate::lane_job::{self, JobRequest, Waited};

/// Run cargo builds and tests through the launcher of a headless lane.
// trace:BUG-2383 | ai:claude
#[derive(clap::Parser, Debug)]
#[clap(name = "lane")]
pub(crate) struct LaneArgs {
    #[clap(subcommand)]
    pub command: LaneCommand,
}

/// How often a wait prints that it is still waiting, so a stream-json log
/// keeps moving.
const PROGRESS_EVERY: Duration = Duration::from_secs(15);

fn run_lane(matches: &clap::ArgMatches, _ctx: &crate::command_registry::DispatchCtx) -> Result<()> {
    let args = <LaneArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    let LaneCommand::Job(job) = args.command;
    let code = match job {
        LaneJobCommand::Run { wait, tail, argv } => run_job(argv, wait, tail)?,
        LaneJobCommand::Wait { id, wait, tail } => wait_job(&id, wait, tail)?,
    };
    if code != 0 {
        use std::io::Write;
        let _ = std::io::stdout().flush();
        std::process::exit(code);
    }
    Ok(())
}

/// The job for `argv`, carrying the allow-listed variables set in this
/// process's environment.
pub(crate) fn job_from(argv: Vec<String>, env: impl Fn(&str) -> Option<String>) -> JobRequest {
    let env = lane_job::ALLOWED_ENV
        .iter()
        .filter_map(|k| env(k).map(|v| ((*k).to_string(), v)))
        .collect();
    JobRequest { argv, env }
}

fn run_job(argv: Vec<String>, wait: u64, tail: usize) -> Result<i32> {
    let job = job_from(argv, |k| std::env::var(k).ok());
    if let Err(why) = lane_job::validate(&job) {
        anyhow::bail!("refused: {why}");
    }
    let (inbox, results) = lane_job::seat_dirs()?;
    let id = lane_job::new_id();
    lane_job::submit(&inbox, &id, &job)?;
    // The runner knows whether the seat target is seeded; the seat does not.
    // trace:BUG-2383 | ai:claude
    let timeout = if job.env.contains_key("AIDA_CARGO_TIMEOUT") {
        format!("timeout {}s", lane_job::timeout_for(&job, false).as_secs())
    } else {
        format!(
            "timeout {}s on a seeded target, {}s on a cold one",
            lane_job::DEFAULT_TIMEOUT_SECS,
            lane_job::MAX_TIMEOUT_SECS
        )
    };
    println!(
        "lane job {id} submitted: {} ({timeout})",
        job.argv.join(" ")
    );
    finish_wait(&inbox, &results, &id, wait, tail)
}

fn wait_job(id: &str, wait: u64, tail: usize) -> Result<i32> {
    if !lane_job::valid_id(id) {
        anyhow::bail!("`{id}` is not a lane job id");
    }
    let (inbox, results) = lane_job::seat_dirs()?;
    finish_wait(&inbox, &results, id, wait, tail)
}

fn finish_wait(
    inbox: &std::path::Path,
    results: &std::path::Path,
    id: &str,
    wait: u64,
    tail: usize,
) -> Result<i32> {
    let waited = lane_job::wait_for(
        inbox,
        results,
        id,
        Duration::from_secs(wait),
        PROGRESS_EVERY,
        |elapsed, taken| {
            println!(
                "lane job {id}: {} ({}s)",
                if taken { "running" } else { "queued" },
                elapsed.as_secs()
            );
        },
    );
    let out = lane_job::out_path(results, id);
    match waited {
        Waited::Done(rc) => {
            let text = std::fs::read_to_string(&out).unwrap_or_default();
            println!("{}", lane_job::tail(&text, tail));
            println!(
                "lane job {id} finished: exit {rc} (full output: {})",
                out.display()
            );
            Ok(rc)
        }
        Waited::Running { taken } => {
            println!(
                "lane job {id} is still {}; continue with `aida lane job wait {id}` \
                 (output so far: {})",
                if taken {
                    "running"
                } else {
                    "queued (the launcher has not started it yet)"
                },
                out.display()
            );
            Ok(lane_job::STILL_RUNNING_EXIT)
        }
    }
}

// trace:BUG-2383 | ai:claude
pub(crate) static LANE: crate::command_registry::CommandReg = crate::command_registry::CommandReg {
    name: "lane",
    help_after: "lint",
    command: <LaneArgs as clap::CommandFactory>::command,
    policy: |_| crate::command_registry::CmdPolicy {
        stakeholder: crate::stakeholder_role::StakeholderAction::BuildLoop,
        per_write_auto_push: false,
        skip_role_restore_prompt: true,
        team_identity_write: false,
    },
    run: crate::command_registry::Handler::Early(run_lane),
};
