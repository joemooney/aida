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

fn run_lane(matches: &clap::ArgMatches, _ctx: &crate::cmdkit::DispatchCtx) -> Result<()> {
    let args = <LaneArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    let job = match args.command {
        LaneCommand::Job(job) => job,
        LaneCommand::FastGateRelease { branch } => {
            let cwd = std::env::current_dir()?;
            let main = crate::main_worktree_root_from(&cwd);
            let inputs = aida_core::exec_context::Environment::capture();
            let lane_context = [
                "AIDA_SESSION_ID",
                "AIDA_HEADLESS",
                "AIDA_CONTAINED",
                crate::gate::seat_authority::GRANT_ENV,
                crate::gate::seat_authority::GRANT_FILE_ENV,
                crate::headless_background::LANE_MARKER_ENV,
            ]
            .iter()
            .any(|key| inputs.get(key).is_some())
                || crate::seat_clone::seat_clone_root(&cwd).is_some()
                || crate::list_leases(&main)
                    .iter()
                    .any(|l| cwd.starts_with(&l.worktree_path));
            crate::lane_fast_gate::operator_release(&main, &branch, lane_context)?;
            println!(
                "{}",
                crate::lane_fast_gate::result_line(&main, &branch, "review hold released:")
            );
            return Ok(());
        }
    };
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
    submit_and_wait(
        &inbox,
        &results,
        &job,
        wait,
        tail,
        &mut std::io::stdout().lock(),
    )
}

/// `aida lane job run` once the seat's runner directories are known: submit
/// `job`, say so, and wait for it as [`wait_in`] does. Split out so a test
/// drives the exact CLI path against a fixture lane.
// trace:BUG-2383 | ai:claude
pub(crate) fn submit_and_wait(
    inbox: &std::path::Path,
    results: &std::path::Path,
    job: &JobRequest,
    wait: u64,
    tail: usize,
    out: &mut dyn std::io::Write,
) -> Result<i32> {
    let id = lane_job::new_id();
    lane_job::submit(inbox, &id, job)?;
    // The runner knows whether the seat target is seeded; the seat does not.
    // trace:BUG-2383 | ai:claude
    let timeout = if job.env.contains_key("AIDA_CARGO_TIMEOUT") {
        format!("timeout {}s", lane_job::timeout_for(job, false).as_secs())
    } else {
        format!(
            "timeout {}s on a seeded target, {}s on a cold one",
            lane_job::DEFAULT_TIMEOUT_SECS,
            lane_job::MAX_TIMEOUT_SECS
        )
    };
    writeln!(
        out,
        "lane job {id} submitted: {} ({timeout})",
        job.argv.join(" ")
    )?;
    wait_in(inbox, results, &id, wait, tail, out)
}

fn wait_job(id: &str, wait: u64, tail: usize) -> Result<i32> {
    if !lane_job::valid_id(id) {
        anyhow::bail!("`{id}` is not a lane job id");
    }
    let (inbox, results) = lane_job::seat_dirs()?;
    wait_in(
        &inbox,
        &results,
        id,
        wait,
        tail,
        &mut std::io::stdout().lock(),
    )
}

/// `aida lane job wait <id>` once the seat's runner directories are known:
/// wait up to `wait` seconds, then print the output tail and return cargo's
/// exit code, or say how to continue and return
/// [`lane_job::STILL_RUNNING_EXIT`].
// trace:BUG-2383 | ai:claude
pub(crate) fn wait_in(
    inbox: &std::path::Path,
    results: &std::path::Path,
    id: &str,
    wait: u64,
    tail: usize,
    out: &mut dyn std::io::Write,
) -> Result<i32> {
    let mut progress_err = None;
    let waited = lane_job::wait_for(
        inbox,
        results,
        id,
        Duration::from_secs(wait),
        PROGRESS_EVERY,
        |elapsed, taken| {
            if let Err(e) = writeln!(
                out,
                "lane job {id}: {} ({}s)",
                if taken { "running" } else { "queued" },
                elapsed.as_secs()
            ) {
                progress_err.get_or_insert(e);
            }
        },
    );
    if let Some(e) = progress_err {
        return Err(e.into());
    }
    let path = lane_job::out_path(results, id);
    match waited {
        Waited::Done(rc) => {
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            writeln!(out, "{}", lane_job::tail(&text, tail))?;
            writeln!(
                out,
                "lane job {id} finished: exit {rc} (full output: {})",
                path.display()
            )?;
            Ok(rc)
        }
        Waited::Running { taken } => {
            writeln!(
                out,
                "lane job {id} is still {}; continue with `aida lane job wait {id}` \
                 (output so far: {})",
                if taken {
                    "running"
                } else {
                    "queued (the launcher has not started it yet)"
                },
                path.display()
            )?;
            Ok(lane_job::STILL_RUNNING_EXIT)
        }
    }
}

// trace:BUG-2383 | ai:claude
pub(crate) static LANE: crate::cmdkit::CommandReg = crate::cmdkit::CommandReg {
    name: "lane",
    help_after: "lint",
    command: <LaneArgs as clap::CommandFactory>::command,
    policy: |_| crate::cmdkit::CmdPolicy {
        stakeholder: crate::stakeholder_role::StakeholderAction::BuildLoop,
        per_write_auto_push: false,
        skip_role_restore_prompt: true,
        team_identity_write: false,
    },
    run: crate::cmdkit::Handler::Early(run_lane),
};
