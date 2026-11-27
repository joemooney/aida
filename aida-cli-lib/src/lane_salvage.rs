//! BUG-2781: salvage-push a lane's committed work when the lane ends non-zero.
//!
//! A lane killed after it committed (a signal mid final gate, a crash, a
//! timeout) used to leave a finished fix unpublished. Pushing before the gate
//! (the lane job contract, AC2) is the preferred fix; this is the backstop.
//! The host, from its own trusted git and never the seat clone's config,
//! bundles the claimed branch alone, quarantines it in a host ref, checks the
//! push is a fast-forward, pushes and drops the host ref. The target is the
//! claimed branch when it already has an open PR and the PR head guard
//! (BUG-2618: no approval at the head, no train pin) lets it move; otherwise
//! it is `salvage/<lane>`, a branch no PR, review or train reads. Every
//! imported commit must carry a resolvable spec trailer, one naming the
//! lane's spec. Uncommitted edits, a non-fast-forward and any other branch
//! are never pushed; the seat keeps its work. A salvaged head carries no review: the merge train needs a fresh
//! CI run and an independent review at that exact head.
//! trace:BUG-2781 | ai:claude

use std::path::Path;

use aida_core::git_ops::RemoteCredentialsExt as _;
use anyhow::{Context, Result};

use crate::headless_background::SeatState;
use crate::standalone_implementer::Outcome;

/// How every salvage is announced.
pub(crate) const UNVERIFIED: &str = "salvaged, unverified: final gate incomplete";

/// A seat's bundler: `(main, clone, branch)` to a one-ref bundle.
pub(crate) type Bundle<'a> = &'a dyn Fn(&Path, &Path, &str) -> Result<Vec<u8>>;

/// Whether a lane's end leaves committed, unpushed work to salvage: it
/// exited non-zero, was not punted or held, and its seat `HEAD` moved past
/// the launch head with commits on no remote. Anything git cannot read is no.
pub(crate) fn wanted(
    outcome: &Outcome,
    success: bool,
    seat: &SeatState,
    launch_head: Option<&str>,
) -> bool {
    !success
        && matches!(outcome, Outcome::NoPr | Outcome::PrOpened { .. })
        && seat.head.as_deref().is_some_and(|h| launch_head != Some(h))
        && seat.pushed_since(launch_head) == Some(false)
        && seat.head_has_unpushed == Some(true)
}

/// The branch a salvage push targets: the claimed branch when it has an
/// open PR, otherwise `salvage/<lane>`.
pub(crate) fn target(branch: &str, open_pr: Option<u64>, lane: &str) -> String {
    match open_pr {
        Some(_) => branch.to_owned(),
        None => format!("salvage/{lane}"),
    }
}

/// What the launcher could read about who ended a lane (AC3): each probe
/// is named in the cause line, found or not.
// trace:BUG-2781 | ai:claude
#[derive(Debug, Default, Clone)]
pub(crate) struct EndEvidence {
    /// The agent's pid, to match a kernel OOM kill against.
    pub(crate) agent_pid: Option<u32>,
    /// Lane jobs that hit the launcher's own job deadline during the lane.
    pub(crate) job_timeouts: Vec<String>,
    /// An operator stop recorded for this lane (lease interruption or a
    /// drain stop request) during the lane.
    pub(crate) operator_stop: Option<String>,
    /// The kernel log since the lane started; `None` when it is unreadable.
    pub(crate) kernel_log: Option<String>,
}

/// The kernel log reader: the log since a time, `None` when unreadable.
pub(crate) type KernelLog<'a> = &'a dyn Fn(std::time::SystemTime) -> Option<String>;

/// Read what the launcher can know about a lane's end, from the lease's
/// job log, its lease record, the drain stop request and the kernel log.
/// Read only; a record that cannot be read contributes nothing.
// trace:BUG-2781 | ai:claude
pub(crate) fn gather(
    project_root: &Path,
    main: &Path,
    lease: &str,
    since: std::time::SystemTime,
    agent_pid: Option<u32>,
    kernel_log: KernelLog<'_>,
) -> EndEvidence {
    let since_utc: chrono::DateTime<chrono::Utc> = since.into();
    let jobs = crate::lane_job::results_dir(main, lease).join("jobs.log");
    let job_timeouts = std::fs::read_to_string(jobs)
        .map(|log| job_timeouts(&log, since_utc))
        .unwrap_or_default();
    EndEvidence {
        agent_pid,
        job_timeouts,
        operator_stop: operator_stop(project_root, lease, since_utc),
        kernel_log: kernel_log(since),
    }
}

/// Job ids the lane job log records as `timed out` at or after `since`.
fn job_timeouts(log: &str, since: chrono::DateTime<chrono::Utc>) -> Vec<String> {
    log.lines()
        .filter_map(|line| {
            let (at, rest) = line.split_once(' ')?;
            let at = chrono::DateTime::parse_from_rfc3339(at).ok()?;
            let (id, how) = rest.split_once(' ')?;
            (at >= since && how.starts_with("timed out rc=")).then(|| id.to_owned())
        })
        .collect()
}

/// An operator stop for this lane: its lease stamped interrupted, or a drain
/// stop requested, at or after `since`.
fn operator_stop(
    project_root: &Path,
    lease: &str,
    since: chrono::DateTime<chrono::Utc>,
) -> Option<String> {
    let after = |at: &str| chrono::DateTime::parse_from_rfc3339(at).is_ok_and(|at| at >= since);
    let lease_record = std::fs::read_to_string(crate::leases::lease_path(project_root, lease))
        .ok()
        .and_then(|body| toml::from_str::<toml::Value>(&body).ok());
    if let Some(at) = lease_record
        .as_ref()
        .and_then(|v| v.get("interrupted_at")?.as_str())
        .filter(|at| after(at))
    {
        let why = lease_record
            .as_ref()
            .and_then(|v| v.get("interrupted_reason")?.as_str())
            .unwrap_or("no reason recorded");
        return Some(format!("the lease was interrupted at {at} ({why})"));
    }
    let request = std::fs::read_to_string(crate::drain_cmd::drain_stop_path(project_root))
        .ok()
        .and_then(|body| serde_json::from_str::<serde_json::Value>(&body).ok())?;
    let at = request
        .get("requested_at_utc")?
        .as_str()
        .filter(|at| after(at))?;
    let mode = request.get("mode").and_then(|m| m.as_str()).unwrap_or("?");
    Some(format!("a drain stop was requested at {at} (mode {mode})"))
}

/// The host kernel log since `since`, through `journalctl -k`; `None` when
/// it cannot be read (no journalctl, a failure, a timeout). Without journal
/// access `journalctl -k` prints no kernel lines, so an empty log is also
/// `None`: unknown, never "no OOM kill".
// trace:BUG-2781 | ai:claude
pub(crate) fn read_kernel_log(since: std::time::SystemTime) -> Option<String> {
    let secs = since.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    let mut cmd = std::process::Command::new("journalctl");
    cmd.args(["-k", "--no-pager", "-q", "-o", "short-iso"])
        .arg(format!("--since=@{secs}"));
    let out = crate::command_output_with_timeout(cmd, std::time::Duration::from_secs(10))?;
    (out.status.success() && !out.stdout.is_empty())
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `(pid, task)` of each process the kernel log says the OOM killer killed.
fn oom_kills(log: &str) -> Vec<(u32, String)> {
    let mut kills: Vec<(u32, String)> = Vec::new();
    for line in log.lines() {
        let Some(rest) = line.split_once("Killed process ").map(|(_, r)| r) else {
            continue;
        };
        let (pid, rest) = rest.split_once(' ').unwrap_or((rest, ""));
        let Ok(pid) = pid.trim().parse::<u32>() else {
            continue;
        };
        let task = rest
            .strip_prefix('(')
            .and_then(|r| r.split_once(')'))
            .map(|(task, _)| task.to_owned())
            .unwrap_or_default();
        if !kills.iter().any(|(p, _)| *p == pid) {
            kills.push((pid, task));
        }
    }
    kills
}

/// What ended the lane: the signal from its exit status and every source the
/// launcher could check (AC3). An exit code with no signal names a source
/// only when one was recorded.
// trace:BUG-2781 | ai:claude
pub(crate) fn exit_cause(status: std::process::ExitStatus, evidence: &EndEvidence) -> String {
    #[cfg(unix)]
    let signal = std::os::unix::process::ExitStatusExt::signal(&status);
    #[cfg(not(unix))]
    let signal: Option<i32> = None;
    // A shell wrapper reports a signal it died of as 128 + the signal.
    let signal = signal.or(status
        .code()
        .filter(|c| (129..=192).contains(c))
        .map(|c| c - 128));
    let mut sources = Vec::new();
    if let Some(stop) = &evidence.operator_stop {
        sources.push(format!("operator stop: {stop}"));
    }
    if !evidence.job_timeouts.is_empty() {
        sources.push(format!(
            "lane-job timeout: {} hit the launcher's job deadline",
            evidence.job_timeouts.join(", ")
        ));
    }
    let Some(sig) = signal else {
        let code = format!("exited with code {}", status.code().unwrap_or(-1));
        return match sources.is_empty() {
            true => code,
            false => format!("{code}; source: {}", sources.join("; ")),
        };
    };
    match &evidence.kernel_log {
        None => sources.push("OOM unknown (kernel log unreadable)".into()),
        Some(log) => {
            let kills = oom_kills(log);
            let agent = evidence
                .agent_pid
                .filter(|pid| kills.iter().any(|(p, _)| p == pid));
            if let Some(pid) = agent {
                sources.insert(
                    0,
                    format!("the kernel OOM killer killed the agent (pid {pid})"),
                );
            } else if kills.is_empty() {
                sources.push("no OOM kill in the kernel log during the lane".into());
            } else {
                let named: Vec<String> = kills
                    .iter()
                    .map(|(pid, task)| format!("{task} (pid {pid})"))
                    .collect();
                sources.push(format!(
                    "the kernel OOM killer killed {} during the lane, not the agent's pid",
                    named.join(", ")
                ));
            }
        }
    }
    let name = match sig {
        1 => "SIGHUP",
        2 => "SIGINT",
        6 => "SIGABRT",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        15 => "SIGTERM",
        _ => "signal",
    };
    format!("ended by {name} ({sig}); source: {}", sources.join("; "))
}

/// What a salvage reads from the host, injectable for tests.
// trace:BUG-2781 | ai:claude
pub(crate) struct Host<'a> {
    /// The seat's bundler.
    pub(crate) bundle: Bundle<'a>,
    /// The BUG-2618 PR head guard, `(main, branch)`: an error refuses moving
    /// that branch's PR head (an approval at the head, a train pin, or
    /// evidence it could not read). Production is
    /// [`crate::pr_head_guard::check_branch`].
    pub(crate) pr_head: &'a dyn Fn(&Path, &str) -> Result<()>,
    /// The requirement store the trailer check resolves IDs against; `None`
    /// resolves every ID live, as `ship --from-seat` does.
    pub(crate) store: Option<&'a aida_core::RequirementsStore>,
}

/// A salvage push that happened.
#[derive(Debug)]
pub(crate) struct Pushed {
    /// The pushed tip.
    pub(crate) tip: String,
    /// The branch it went to.
    pub(crate) target: String,
    /// Why the open PR's branch was not the target, when it was refused.
    pub(crate) pr_refused: Option<String>,
}

/// Push the seat's committed `branch` as a fast-forward, from host git only:
/// to the open PR's branch when there is one and the PR head guard allows
/// moving it, otherwise to `salvage/<lane>`. Refuses a seat this checkout did
/// not record, a branch its lease does not own, a punt or hold, an imported
/// commit without a resolvable spec trailer, and a push that is not a
/// fast-forward. The host import ref is always dropped.
pub(crate) fn push(lane: &Lane<'_>, host: &Host<'_>) -> Result<Pushed> {
    let seat = crate::ship_from_seat::recorded_seat(lane.main, lane.clone)?;
    let main = seat.main.clone();
    let branch = lane.branch;
    crate::ship_from_seat::require_publication_branch(&main, &seat.root, branch)?;
    crate::ship_from_seat::refuse_publication_veto(&main, &seat.root, lane.spec)?;
    let bytes = (host.bundle)(&main, &seat.root, branch)?;
    let staged = crate::ship_from_seat::stage_bundle(&main, branch, &bytes, false)?;
    let pushed = guarded_push(&main, &seat.root, lane, host, &staged);
    let dropped = crate::ship_from_seat::git_text(
        &main,
        &["update-ref", "-d", &staged.reference, &staged.tip],
    );
    let pushed = pushed?;
    if let Err(e) = dropped {
        eprintln!("implementer: could not drop {}: {e:#}", staged.reference);
    }
    Ok(pushed)
}

/// [`push`]'s checks on the staged import, then the push itself.
// trace:BUG-2781 | ai:claude
fn guarded_push(
    main: &Path,
    tree: &Path,
    lane: &Lane<'_>,
    host: &Host<'_>,
    staged: &crate::ship_from_seat::Staged,
) -> Result<Pushed> {
    imported_trailers(main, staged, lane.spec, host.store)?;
    let mut pr_refused = None;
    let target = match lane.open_pr {
        Some(pr) => match (host.pr_head)(main, lane.branch) {
            Ok(()) => lane.branch.to_owned(),
            Err(e) => {
                let why = format!("PR #{pr} head not moved: {e:#}");
                eprintln!("implementer: {why}; salvaging to the salvage ref instead");
                pr_refused = Some(why);
                target(lane.branch, None, lane.lane)
            }
        },
        None => target(lane.branch, None, lane.lane),
    };
    // Re-read ownership after preparation, before the remote mutation, as
    // seat publication does. trace:BUG-2633 | ai:claude
    crate::ship_from_seat::require_publication_branch(main, tree, lane.branch)?;
    push_fast_forward(main, &staged.reference, &staged.tip, &target)?;
    Ok(Pushed {
        tip: staged.tip.clone(),
        target,
        pr_refused,
    })
}

/// Every commit the salvage imports (past the default branch) names a spec in
/// a `(SPEC-ID)` or `(Refs SPEC-ID)` trailer, every named ID resolves to a live
/// spec, and the lane's spec is among them; otherwise the salvage is refused.
/// Stricter than `ship --from-seat`, which needs only one trailered commit: a
/// salvaged head skips the lane's own final checks.
// trace:BUG-2781 | ai:claude
fn imported_trailers(
    main: &Path,
    staged: &crate::ship_from_seat::Staged,
    spec: &str,
    store: Option<&aida_core::RequirementsStore>,
) -> Result<()> {
    let base = crate::detect_default_branch_ref(main).unwrap_or_else(|| "origin/main".into());
    let base_oid = crate::ship_from_seat::git_text(
        main,
        &["rev-parse", "--verify", &format!("{base}^{{commit}}")],
    )?;
    let range = format!("{base_oid}..{}", staged.reference);
    let commits = crate::trace::read_commits_in_range_unadjusted(&staged.repo, &range)?;
    if let Some((sha, subject)) = commits.iter().find(|(_, subject)| {
        crate::extract_spec_ids_from_commit(subject).is_empty()
            && crate::pr_ship::extract_authorship_spec_ids_from_subject(subject).is_empty()
    }) {
        anyhow::bail!(
            "imported commit {sha} (`{subject}`) carries no `(SPEC-ID)` or `(Refs SPEC-ID)` \
             trailer; nothing pushed"
        );
    }
    let ids = crate::ship_from_seat::seat_trailer_check(&staged.repo, &range, |id| {
        crate::trace::resolve_spec_in_store(store, id)
    })?;
    crate::ship_from_seat::explicit_spec_check(store, spec, &ids)
}

fn push_fast_forward(main: &Path, import: &str, tip: &str, target: &str) -> Result<()> {
    if let Some(remote) = crate::ship_from_seat::pin_remote_branch(main, target)? {
        let ancestry = aida_core::git_ops::host_git_command(main)
            .args(["merge-base", "--is-ancestor", &remote.tip, tip])
            .status()
            .context("could not check the fast-forward; nothing pushed")?;
        match ancestry.code() {
            Some(0) => {}
            Some(1) => anyhow::bail!(
                "{tip} does not fast-forward origin/{target} ({}); nothing pushed",
                remote.tip
            ),
            _ => anyhow::bail!("could not check the fast-forward; nothing pushed"),
        }
    }
    aida_core::git_ops::clear_sandbox_config_lock(main);
    // No `+`: origin refuses a non-fast-forward that raced the check above.
    let push = aida_core::git_ops::host_git_command(main)
        .args(["push", "origin", &format!("{import}:refs/heads/{target}")])
        .remote_credentials(main, "origin")
        .status()
        .context("could not invoke `git push`")?;
    anyhow::ensure!(
        push.success(),
        "`git push origin {import}:refs/heads/{target}` failed; nothing salvaged"
    );
    Ok(())
}

/// The announcement for a salvaged head, for the spec, the PR and the log.
pub(crate) fn announcement(tip: &str, target: &str, cause: &str) -> String {
    format!(
        "{UNVERIFIED} ({cause}). The host pushed the lane's committed head {tip} to \
         `{target}`. It is not merge-train eligible until a fresh CI run and an independent \
         review at that head."
    )
}

/// The lane a failed exit may salvage.
pub(crate) struct Lane<'a> {
    pub(crate) main: &'a Path,
    pub(crate) clone: &'a Path,
    pub(crate) spec: &'a str,
    pub(crate) branch: &'a str,
    pub(crate) lane: &'a str,
    pub(crate) launch_head: Option<&'a str>,
    pub(crate) open_pr: Option<u64>,
}

/// A failed lane exit's salvage: push its committed work when [`wanted`],
/// announce it through `announce(text, open_pr)`, and return the note for
/// the lane's log and result, or `None` when there was nothing to salvage.
/// A refusal keeps the commits in the seat and says why.
pub(crate) fn on_failed_exit(
    lane: &Lane<'_>,
    outcome: &Outcome,
    seat: &SeatState,
    cause: &str,
    host: &Host<'_>,
    announce: &dyn Fn(&str, Option<u64>),
) -> Option<String> {
    if !wanted(outcome, false, seat, lane.launch_head) {
        return None;
    }
    match push(lane, host) {
        Ok(Pushed {
            tip,
            target,
            pr_refused,
        }) => {
            let mut text = announcement(&tip, &target, cause);
            if let Some(why) = &pr_refused {
                text = format!("{text} {why}.");
            }
            eprintln!("implementer: {text}");
            announce(&text, lane.open_pr);
            let mut note = format!("{UNVERIFIED}: pushed {tip} to {target}");
            if let Some(why) = pr_refused {
                note = format!("{note} ({why})");
            }
            Some(note)
        }
        Err(e) => {
            eprintln!("implementer: salvage-push refused: {e:#}");
            Some(format!(
                "salvage-push refused, commits retained in the seat: {e:#}"
            ))
        }
    }
}

/// The lane a failed exit ended, as `run()` knows it after publication.
// trace:BUG-2781 | ai:claude
pub(crate) struct FailedExit<'a> {
    pub(crate) main: &'a Path,
    pub(crate) clone: &'a Path,
    pub(crate) spec: &'a str,
    pub(crate) branch: &'a str,
    pub(crate) lane: &'a str,
    /// Whether the seat is a host-recorded clone that publishes itself.
    pub(crate) seat_clone: bool,
    pub(crate) launch_seat: &'a crate::standalone_implementer::LaunchSeat<'a>,
    /// Salvage to `salvage/<lane>` only, never a PR branch: a usage-limit
    /// stop's partial work. trace:STORY-3134 | ai:claude
    pub(crate) side_branch_only: bool,
}

/// `run()`'s failed-exit step: name what ended the lane (`cause`) and, for a
/// self-publishing seat clone, salvage its committed work toward the PR the
/// lane opened, else the PR it launched on, else `salvage/<lane>`. Folds the
/// note into the attempt's `leftover` note, which the attempt then records.
// trace:BUG-2781 | ai:claude
pub(crate) fn failed_exit_note(
    exit: &FailedExit<'_>,
    cause: &str,
    outcome: &Outcome,
    read_seat: &dyn Fn() -> SeatState,
    host: &Host<'_>,
    announce: &dyn Fn(&str, Option<u64>),
    leftover: Option<String>,
) -> Option<String> {
    let mut note = format!("lane {cause}");
    if exit.seat_clone {
        let open_pr = match outcome {
            _ if exit.side_branch_only => None,
            Outcome::PrOpened { pr, .. } => Some(*pr),
            _ => exit.launch_seat.pr.map(|(pr, _)| *pr),
        };
        let lane = Lane {
            main: exit.main,
            clone: exit.clone,
            spec: exit.spec,
            branch: exit.branch,
            lane: exit.lane,
            launch_head: exit.launch_seat.head,
            open_pr,
        };
        if let Some(salvage) = on_failed_exit(&lane, outcome, &read_seat(), cause, host, announce) {
            note = format!("{note}; {salvage}");
        }
    }
    Some(match leftover {
        Some(left) => format!("{left}; {note}"),
        None => note,
    })
}

/// Post `text` on the spec and, when there is one, on its open PR. Best
/// effort: the push already happened; a failed note warns.
pub(crate) fn announce(main: &Path, spec: &str, branch: &str, text: &str, pr: Option<u64>) {
    let store =
        crate::detect_distributed_store_from(main).unwrap_or_else(|| main.join(".aida-store"));
    let storage = crate::Storage::new(store);
    if let Err(e) =
        crate::comment_cmd::add_comment_cli(&storage, spec, text, Some("aida-host"), None)
    {
        eprintln!("implementer: could not note the salvage on {spec}: {e:#}");
    }
    if let Some(id) = pr {
        let change = crate::forge::ChangeRef {
            id,
            url: String::new(),
            branch: branch.to_owned(),
            base: String::new(),
            title: None,
        };
        if let Err(e) = crate::forge::forge_for_open_change(main).comment(&change, text) {
            eprintln!("implementer: could not note the salvage on PR #{id}: {e:#}");
        }
    }
}

#[cfg(test)]
#[path = "tests/bug_2781_lane_salvage_unit_tests.rs"]
mod unit_tests;
