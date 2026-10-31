//! STORY-1662: the standalone implementer launch — `aida queue work <SPEC>
//! --role implementer --no-human` run by an orchestrator, relay or advisor
//! session (with Bash, not the Agent tool), outside any `--auto-complete`
//! drain.
//!
//! The child is a separate headless vendor process with its own lease,
//! worktree and implementer seat (a delegated child grant, or baseline). The
//! launcher waits for it and reports one result line the caller can branch
//! on, with a distinct exit code per outcome — the implementer twin of the
//! standalone reviewer's verdict contract.
// trace:STORY-1662 | ai:claude

use crate::*;
use std::path::{Path, PathBuf};

/// The marker a standalone implementer child carries: the lease id its
/// launcher holds for it. Under it `aida pr ship` and `aida review record`
/// refuse, so the stop contract is enforced, not just prompted.
/// [`crate::session::scrub_launcher_session_env`] strips it from any
/// grandchild.
pub(crate) const MARKER_ENV: &str = "AIDA_STANDALONE_IMPLEMENTER";

/// Launcher-only variables a standalone implementer child must not inherit.
/// ONE list, read by [`scrub_standalone_child_env`] and by its test, so a new
/// launcher-only variable cannot be added to one and forgotten in the other.
///
/// The drain/orchestrator context is here because a standalone child that
/// kept it would be mistaken for a drain child (and get the advisor code
/// guard's `AIDA_AUTO_COMPLETE` exemption). These are NOT in
/// [`crate::session::scrub_launcher_session_env`], which every headless
/// child gets: a drain's own implementer child needs them.
/// The seat (`AIDA_SESSION_ROLE`) and grant ([`crate::seat_authority::GRANT_ENV`])
/// are not listed because the child sets both explicitly; the punt and
/// hold signal files are re-provisioned by this launcher after the scrub.
pub(crate) const LAUNCHER_ONLY_ENV: &[&str] = &[
    "AIDA_SESSION_PURPOSE",
    crate::orchestrator::AUTO_COMPLETE_ENV,
    crate::orchestrator::TOKEN_ENV,
    crate::orchestrator::PHASE_ENV,
    crate::orchestrator::VARIANT_ENV,
    crate::orchestrator::NO_HUMAN_MODE_ENV,
    crate::orchestrator_leases::ORCHESTRATED_LEASE_RECEIPT_ENV,
    "AIDA_REVIEW_VERDICT_FILE",
    "AIDA_GRADED_REVIEW_CONTEXT",
    crate::punt::SIGNAL_FILE_ENV,
    crate::punt::HOLD_SIGNAL_FILE_ENV,
    crate::punt::REQUEST_FILE_ENV,
    crate::punt::RESPONSE_FILE_ENV,
    crate::gate::seat_authority::GRANT_FILE_ENV,
    MARKER_ENV,
];

/// Exit code: the implementer exited without opening a PR, punting or
/// holding (and its vendor exited 0; a nonzero vendor code passes through).
pub(crate) const NO_PR_EXIT_CODE: i32 = 31;
/// Exit code: the implementer punted the spec (`aida punt`).
pub(crate) const PUNTED_EXIT_CODE: i32 = 32;
/// Exit code: the implementer pushed its branch and deliberately held the
/// PR (`aida pr hold`).
pub(crate) const HELD_EXIT_CODE: i32 = 33;
/// Exit code: the launch was refused because the standalone-implementer
/// concurrency cap is reached. Nothing was set up.
pub(crate) const CAP_EXIT_CODE: i32 = 34;

/// Default cap on concurrently running standalone implementers.
pub(crate) const DEFAULT_CAP: usize = 3;

/// Whether this `queue work` invocation is a standalone implementer launch:
/// an implementer pickup of a spec scope, headless, and not a drain's own
/// phase-1 child (which carries the orchestrator phase env or
/// `AIDA_AUTO_COMPLETE`). The caller also requires a fresh launch.
pub(crate) fn is_standalone_implementer(role: &str, scope_is_spec: bool, no_human: bool) -> bool {
    scope_is_spec
        && no_human
        && crate::canonical_role_name(role).eq_ignore_ascii_case("implementer")
        && !launched_by_drain()
}

fn launched_by_drain() -> bool {
    [
        crate::orchestrator::PHASE_ENV,
        crate::orchestrator::AUTO_COMPLETE_ENV,
    ]
    .iter()
    .any(|key| std::env::var_os(key).is_some_and(|v| !v.is_empty()))
}

/// The stop contract appended to a standalone implementer's prompt.
/// [`refuse_under_marker`] enforces its two prohibitions.
pub(crate) const STOP_CONTRACT: &str =
    "\n\nStandalone implementer launch: when your PR is open, stop. \
Do not run `aida pr ship` or `aida review record` (both refuse here): the launcher reports the PR \
to whoever started it, and a separate reviewer and the merge seat take it from there. If you hit \
a design fork, run `aida punt`; to push the branch but hold the PR for a manual gate, run \
`aida pr hold`.";

/// Remove every [`LAUNCHER_ONLY_ENV`] variable from `cmd`, then set the
/// child's marker and its punt and hold signal files.
pub(crate) fn apply_standalone_child_env(
    cmd: &mut std::process::Command,
    lease_id: &str,
    punt_signal: &Path,
    hold_signal: &Path,
) {
    scrub_standalone_child_env(cmd);
    cmd.env(MARKER_ENV, lease_id)
        .env(crate::punt::SIGNAL_FILE_ENV, punt_signal)
        .env(crate::punt::HOLD_SIGNAL_FILE_ENV, hold_signal);
}

/// Strip [`LAUNCHER_ONLY_ENV`] from `cmd`.
pub(crate) fn scrub_standalone_child_env(cmd: &mut std::process::Command) {
    for key in LAUNCHER_ONLY_ENV {
        cmd.env_remove(key);
    }
}

#[cfg(test)]
thread_local! {
    /// Tests seat the marker here rather than in the process env, so a test
    /// run from inside a standalone implementer child (which carries the
    /// real marker) is not refused.
    pub(crate) static TEST_MARKER: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
}

fn marker_value() -> Option<String> {
    #[cfg(test)]
    {
        TEST_MARKER.with(|m| m.borrow().clone())
    }
    #[cfg(not(test))]
    {
        std::env::var(MARKER_ENV)
            .ok()
            .filter(|v| !v.trim().is_empty())
    }
}

/// Refuse `aida <verb>` inside a standalone implementer child.
pub(crate) fn refuse_under_marker(verb: &str) -> Result<()> {
    if marker_value().is_some() {
        anyhow::bail!(
            "`aida {verb}` is refused inside a standalone implementer launch ({MARKER_ENV} is set). \
             Open the PR and stop: the launcher reports it to whoever started you, and a separate \
             reviewer and the merge seat take it from there."
        );
    }
    Ok(())
}

/// What a standalone implementer run ended with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    PrOpened { pr: u64, head: Option<String> },
    Punted { reason: String },
    Held { branch: String },
    NoPr,
}

/// Classify a finished run the way the drain's phase 1 does: a punt signal
/// first, then a hold signal, then an open PR. `lookup_pr` runs only when
/// neither signal was written.
pub(crate) fn classify(
    punt: Option<crate::punt::PuntSignal>,
    hold: Option<crate::punt::HoldSignal>,
    lookup_pr: impl FnOnce() -> Option<(u64, Option<String>)>,
) -> Outcome {
    if let Some(signal) = punt {
        return Outcome::Punted {
            reason: signal.summary(),
        };
    }
    if let Some(signal) = hold {
        return Outcome::Held {
            branch: signal.branch,
        };
    }
    match lookup_pr() {
        Some((pr, head)) => Outcome::PrOpened { pr, head },
        None => Outcome::NoPr,
    }
}

/// The one stdout line and the exit code for `outcome`. `vendor_code` is the
/// vendor's own exit code; a nonzero one replaces [`NO_PR_EXIT_CODE`] (the
/// standalone reviewer's rule), and is otherwise ignored, since a signal or
/// an open PR is better evidence than the exit code. `uncommitted` is the
/// count of uncommitted paths a PR-less run left behind (BUG-044).
pub(crate) fn report(
    outcome: &Outcome,
    vendor_code: Option<i32>,
    uncommitted: Option<usize>,
) -> (String, i32) {
    let one_line = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    match outcome {
        Outcome::PrOpened { pr, head } => (
            format!(
                "implementer: pr-opened pr={pr} head={}",
                head.as_deref().unwrap_or("unknown")
            ),
            0,
        ),
        Outcome::Punted { reason } => (
            format!("implementer: punted reason={}", one_line(reason)),
            PUNTED_EXIT_CODE,
        ),
        Outcome::Held { branch } => (format!("implementer: held branch={branch}"), HELD_EXIT_CODE),
        // trace:BUG-B6B1278890774EF993D2EBC327862470-044 | ai:claude
        // BUG-050: a stall leaves work in the seat; name it as the reason.
        // trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
        Outcome::NoPr => (
            match uncommitted.filter(|n| *n > 0) {
                Some(n) => format!("implementer: no-pr reason=uncommitted-work uncommitted={n}"),
                None => "implementer: no-pr".to_string(),
            },
            vendor_code.filter(|c| *c != 0).unwrap_or(NO_PR_EXIT_CODE),
        ),
    }
}

/// BUG-050: whether `outcome` is an open PR still on `head_before`, the head
/// it had when this run launched. A rework round starts with its PR already
/// open, so finding it open proves nothing: until its head moves, this run
/// has pushed nothing and is treated as PR-less (resumed if it parked).
/// An unknown head on either side is not stale.
// trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
pub(crate) fn is_stale_pr(outcome: &Outcome, head_before: Option<&str>) -> bool {
    matches!(
        (outcome, head_before),
        (Outcome::PrOpened { head: Some(head), .. }, Some(before)) if head == before
    )
}

/// BUG-050: the outcome to report once the run is done. A stale PR (see
/// [`is_stale_pr`]) with `uncommitted` paths left in the seat is a stall,
/// reported [`Outcome::NoPr`] so the caller does not read the old head as a
/// finished push. A stale PR on a clean seat stays `pr-opened`: the round
/// had nothing to change.
// trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
pub(crate) fn settle_outcome(
    outcome: Outcome,
    head_before: Option<&str>,
    uncommitted: Option<usize>,
) -> Outcome {
    if uncommitted.is_some_and(|n| n > 0) && is_stale_pr(&outcome, head_before) {
        Outcome::NoPr
    } else {
        outcome
    }
}

/// The log of the `n`th automatic resume of the run logging to `first`:
/// a fresh file beside it, so the linger reaper's final-result watch never
/// reads the previous run's result.
// trace:BUG-B6B1278890774EF993D2EBC327862470-044 | ai:claude
pub(crate) fn resume_log_path(first: &Path, n: u32) -> PathBuf {
    let stem = first
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    first.with_file_name(format!("{stem}-resume{n}.jsonl"))
}

/// The lease's worktree, or `project_root` when the lease cannot be read.
fn lease_worktree(project_root: &Path, lease_id: &str) -> PathBuf {
    crate::list_leases(project_root)
        .into_iter()
        .find(|l| l.id == lease_id)
        .map(|l| l.worktree_path)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| project_root.to_path_buf())
}

/// A standalone implementer run that ended without an open PR, or a launch
/// refused by the concurrency cap. `exit_code_for_error` maps it to `code`.
#[derive(Debug)]
pub(crate) struct StandaloneImplementerExit {
    pub(crate) code: i32,
    pub(crate) message: String,
    /// `code` is the vendor's own exit code passed through (BUG-2187), not a
    /// named code from `EXIT_CODES`.
    pub(crate) vendor_exit: bool,
}

impl std::fmt::Display for StandaloneImplementerExit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for StandaloneImplementerExit {}

// --- Concurrency cap (signoff C2) ------------------------------------------
//
// Each running standalone launcher holds a slot file named for its pid under
// the main checkout's `.aida/standalone-implementers/`. Counting and taking a
// slot happen under one file lock, so two launchers starting together cannot
// both see room for one. A slot whose process is gone is pruned, so a killed
// launcher never holds a slot.

fn slots_dir(project_root: &Path) -> PathBuf {
    crate::main_worktree_root_from(project_root)
        .join(".aida")
        .join("standalone-implementers")
}

/// `[queue] standalone_implementer_cap` from `.aida/config.toml`, or
/// [`DEFAULT_CAP`]. A cap of 0 is read as the default.
pub(crate) fn configured_cap(project_root: &Path) -> usize {
    let root = crate::main_worktree_root_from(project_root);
    crate::read_project_config_value(&root)
        .and_then(|v| {
            v.get("queue")
                .and_then(|d| d.get("standalone_implementer_cap"))
                .and_then(|n| n.as_integer())
        })
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_CAP)
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Slot {
    pid: u32,
    #[serde(default)]
    start: Option<String>,
    spec: String,
    /// The child's stream-json log, once launched. A slot whose log holds
    /// the final result no longer counts toward the cap.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-027 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    log: Option<PathBuf>,
    /// The launcher is holding off the reaper for, or waiting on, the
    /// child's own background run, to resume it: the lane is still working
    /// though its log ends in a final result, so the slot counts.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-044 | ai:claude
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    waiting: bool,
}

/// Rewrite this process's slot with `edit`. Best effort.
fn edit_own_slot(project_root: &Path, edit: impl FnOnce(&mut Slot)) {
    let path = slots_dir(project_root).join(format!("{}.json", std::process::id()));
    let Some(mut slot) = std::fs::read_to_string(&path)
        .ok()
        .and_then(|body| serde_json::from_str::<Slot>(&body).ok())
    else {
        return;
    };
    edit(&mut slot);
    if let Ok(body) = serde_json::to_string(&slot) {
        let _ = aida_core::write_atomic(&path, body);
    }
}

/// Record the child's log on this process's slot, so the cap stops counting
/// the slot once the child logs its final result. A new log clears the
/// waiting mark: the resumed run is counted by its own log.
// trace:BUG-B6B1278890774EF993D2EBC327862470-027 | ai:claude
pub(crate) fn record_slot_log(project_root: &Path, log_path: &Path) {
    edit_own_slot(project_root, |slot| {
        slot.log = Some(log_path.to_path_buf());
        slot.waiting = false;
    });
}

/// Mark this process's slot as waiting on its child's background run, so
/// the cap keeps counting it while the logged final result says it ended.
// trace:BUG-B6B1278890774EF993D2EBC327862470-044 | ai:claude
pub(crate) fn mark_slot_waiting(project_root: &Path) {
    edit_own_slot(project_root, |slot| slot.waiting = true);
}

/// A held concurrency slot; released when dropped.
pub(crate) struct SlotGuard {
    path: PathBuf,
}

impl Drop for SlotGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Take a slot for this process, or refuse with [`CAP_EXIT_CODE`] when
/// `cap` live launchers already hold one.
pub(crate) fn acquire_slot(project_root: &Path, spec: &str, cap: usize) -> Result<SlotGuard> {
    acquire_slot_with(project_root, spec, cap, std::process::id(), |pid, start| {
        aida_core::liveness::process_identity_is_alive(pid, start)
    })
}

pub(crate) fn acquire_slot_with(
    project_root: &Path,
    spec: &str,
    cap: usize,
    pid: u32,
    is_alive: impl Fn(u32, Option<&str>) -> bool,
) -> Result<SlotGuard> {
    use fs2::FileExt;
    let dir = slots_dir(project_root);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(".lock"))
        .with_context(|| format!("opening the slot lock in {}", dir.display()))?;
    lock.lock_exclusive()
        .with_context(|| format!("locking {}", dir.display()))?;
    let mut running = Vec::new();
    for entry in std::fs::read_dir(&dir)?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let slot = std::fs::read_to_string(&path)
            .ok()
            .and_then(|body| serde_json::from_str::<Slot>(&body).ok());
        match slot {
            // A launcher whose child already logged its final result is
            // finishing, not running: its slot stays but is not counted.
            // trace:BUG-B6B1278890774EF993D2EBC327862470-027 | ai:claude
            Some(slot) if slot.pid != pid && is_alive(slot.pid, slot.start.as_deref()) => {
                // trace:BUG-B6B1278890774EF993D2EBC327862470-044 | ai:claude
                let finishing = !slot.waiting
                    && slot
                        .log
                        .as_deref()
                        .is_some_and(crate::headless_linger::log_has_final_result);
                if !finishing {
                    running.push(slot.spec)
                }
            }
            _ => {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    if running.len() >= cap {
        let _ = FileExt::unlock(&lock);
        running.sort();
        return Err(anyhow::Error::new(StandaloneImplementerExit {
            vendor_exit: false,
            code: CAP_EXIT_CODE,
            message: format!(
                "{} standalone implementers are already running ({}); the cap is {cap} \
                 (`[queue] standalone_implementer_cap` in .aida/config.toml). Wait for one to \
                 finish, then launch again. Nothing was set up for {spec}.",
                running.len(),
                running.join(", ")
            ),
        }));
    }
    let path = dir.join(format!("{pid}.json"));
    let slot = Slot {
        pid,
        start: aida_core::liveness::process_start_identity(pid),
        spec: spec.to_string(),
        log: None,
        waiting: false,
    };
    aida_core::write_atomic(&path, serde_json::to_string(&slot)?)
        .with_context(|| format!("writing {}", path.display()))?;
    let _ = FileExt::unlock(&lock);
    Ok(SlotGuard { path })
}

// --- The launch ---------------------------------------------------------------

/// Everything `handle_queue_work` has set up for a standalone implementer
/// once its lease and worktree exist.
pub(crate) struct Launch<'a> {
    pub(crate) project_root: &'a Path,
    pub(crate) vendor: crate::session::HeadlessVendor,
    pub(crate) prompt: &'a str,
    pub(crate) session_id: &'a str,
    pub(crate) spec: &'a str,
    pub(crate) lease_id: &'a str,
    pub(crate) branch: &'a str,
    pub(crate) contained: bool,
    pub(crate) child_grant: &'a crate::gate::seat_authority::HeadlessChildGrant,
}

/// The audit reason for a baseline child whose launcher presented no grant at
/// all, as opposed to [`BASELINE_NOT_DELEGATED`].
pub(crate) const BASELINE_NO_GRANT: &str = "launcher-has-no-grant";
/// The audit reason for a baseline child whose launcher holds a grant that
/// does not delegate `implementer` (signoff Q1).
pub(crate) const BASELINE_NOT_DELEGATED: &str = "launcher-did-not-delegate";

/// Why a baseline child is ungranted: `launcher_grant` is the launcher's
/// session grant ([`crate::seat_authority::GRANT_ENV`]) value. Unset or blank
/// means the launcher had no grant to delegate from; anything else is a grant
/// that did not delegate the seat.
// trace:STORY-1662 | ai:claude
pub(crate) fn baseline_reason(launcher_grant: Option<&str>) -> &'static str {
    match launcher_grant.map(str::trim) {
        None | Some("") => BASELINE_NO_GRANT,
        Some(_) => BASELINE_NOT_DELEGATED,
    }
}

/// Record on the authorship ledger the authority the child runs with (the
/// Q1 audit note: a baseline child names why it is ungranted).
/// `launcher_grant` is the launcher's own session grant (`GRANT_ENV`), which
/// tells "no grant" apart from "a grant that does not delegate".
pub(crate) fn record_child_authority(
    project_root: &Path,
    lease_id: &str,
    grant: &crate::gate::seat_authority::HeadlessChildGrant,
    launcher_grant: Option<&str>,
) {
    use crate::gate::seat_authority::HeadlessChildGrant;
    let launch = match grant {
        HeadlessChildGrant::Issued(issued) => crate::gate::review_authority::LaunchAuthority {
            seat: "implementer".to_string(),
            grant: issued.id.clone(),
            reason: "delegated".to_string(),
            detail: None,
        },
        HeadlessChildGrant::Baseline { cause } => crate::gate::review_authority::LaunchAuthority {
            seat: "implementer".to_string(),
            grant: "baseline".to_string(),
            reason: baseline_reason(launcher_grant).to_string(),
            detail: Some(cause.clone()),
        },
    };
    crate::gate::review_authority::record_launch_authority(project_root, lease_id, launch);
}

/// Run the child, wait, classify, print the result line, and return the
/// outcome's exit code (`Ok` only for an open PR).
pub(crate) fn run(launch: Launch<'_>) -> Result<()> {
    let main_root = crate::main_worktree_root_from(launch.project_root);
    claim_spec(launch.project_root, launch.spec);
    // Provision the punt and hold handshakes exactly as the drain's phase 1
    // does: a fresh path under the main checkout, any stale file removed.
    let punt_signal = crate::punt::signal_path(&main_root, launch.spec);
    let hold_signal = crate::punt::hold_signal_path(&main_root, launch.spec);
    for path in [&punt_signal, &hold_signal] {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::remove_file(path);
    }
    let prompt = format!("{}{STOP_CONTRACT}", launch.prompt);
    let log_path = launch
        .project_root
        .join(".aida")
        .join("headless-logs")
        .join(format!("{}-{}.jsonl", launch.branch, launch.session_id));
    // trace:BUG-B6B1278890774EF993D2EBC327862470-027 | ai:claude
    record_slot_log(launch.project_root, &log_path);
    eprintln!(
        "{} {}",
        crate::glyph(crate::glyphs::Glyph::FlowActive)
            .green()
            .bold(),
        format!(
            "launching {} standalone implementer for {} (waits for the result)",
            launch.vendor.as_str(),
            launch.spec
        )
        .cyan()
    );
    eprintln!(
        "  {} headless output → {}",
        crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
        log_path.display().to_string().dimmed()
    );
    let tee_opts =
        crate::headless_tee::TeeOptions::from_env_and_flag(false).with_label(launch.branch);
    let store_commit =
        crate::child_store_commit::ChildStoreCommit::arm(launch.project_root, "implementer");
    let customize = |cmd: &mut std::process::Command| {
        apply_standalone_child_env(cmd, launch.lease_id, &punt_signal, &hold_signal)
    };
    // The child is spawned, not exec'd: stamp it on the lease for every
    // vendor, so `aida ps` reads the lane as live (signoff C6), and record
    // it as the lease's worker (BUG-1918).
    let on_spawn = |pid: u32| {
        if let Err(e) =
            crate::session::stamp_lease_active_pid_at(launch.project_root, launch.lease_id, pid)
        {
            eprintln!(
                "{} could not stamp the implementer's pid on lease {}: {e:#}",
                "Warning:".yellow().bold(),
                launch.lease_id
            );
            crate::gate::review_authority::record_launched_worker(
                launch.project_root,
                launch.lease_id,
                pid,
            );
        }
    };
    // BUG-044: a run that parks on its own background build is resumed, not
    // reported. trace:BUG-B6B1278890774EF993D2EBC327862470-044 | ai:claude
    use crate::headless_background as bg;
    let can_resume = launch.vendor == crate::session::HeadlessVendor::Claude;
    let ceiling = std::time::Duration::from_millis(
        crate::burndown_run::resolved_bg_wait_ceiling_ms(Some(launch.project_root)),
    );
    let live_work = || bg::live_background_work(launch.lease_id);
    // BUG-050: the head of the branch's PR before this run, if one is open
    // (a rework round), so finding that same PR afterwards is not a push.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
    let head_before = lookup_pr_with(launch.project_root, launch.branch, 1).and_then(|(_, h)| h);
    let mut resumes = 0;
    let mut run_log = log_path.clone();
    let mut run_prompt = prompt;
    let finished = loop {
        let reap_hold = bg::ReapHold::new(ceiling);
        let hold = || {
            let was_held = reap_hold.held();
            let on = reap_hold.check(!live_work().is_empty());
            if on && !was_held {
                // The lane is still working: keep its cap slot counted.
                mark_slot_waiting(launch.project_root);
            }
            on
        };
        let status = crate::session::run_standalone_implementer_child(
            launch.vendor,
            &run_prompt,
            launch.session_id,
            &run_log,
            &tee_opts,
            launch.contained,
            launch.lease_id,
            launch.child_grant,
            &crate::session::HeadlessChildHooks {
                customize: Some(&customize),
                on_spawn: Some(&on_spawn),
                resume: resumes > 0,
                hold_reap: Some(&hold),
            },
        );
        let status = match status {
            Ok(status) => status,
            Err(e) => break Err(e),
        };
        let outcome = classify(
            crate::punt::read_signal(&punt_signal),
            crate::punt::read_hold_signal(&hold_signal),
            || lookup_pr(launch.project_root, launch.branch),
        );
        // trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
        if outcome != Outcome::NoPr && !is_stale_pr(&outcome, head_before.as_deref()) {
            break Ok((status, outcome));
        }
        let live = live_work();
        let Some(mut reason) = bg::resume_reason(
            resumes,
            can_resume,
            crate::headless_linger::final_result_text(&run_log).as_deref(),
            !live.is_empty(),
            reap_hold.held(),
            reap_hold.expired(),
        ) else {
            break Ok((status, outcome));
        };
        // A resume is pending: the lane keeps its cap slot while it waits.
        mark_slot_waiting(launch.project_root);
        if !live.is_empty() && !reap_hold.expired() {
            eprintln!(
                "{} the implementer ended its turn with its background run still going; waiting for it (up to {}m)",
                crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                ceiling.as_secs() / 60
            );
        }
        let settled = bg::settle_background_work(ceiling, reap_hold.expired(), live_work, || {
            bg::terminate_background_work(live_work)
        });
        if settled == bg::Settled::Terminated {
            eprintln!(
                "{} the background run was still going after {}m; the launcher stopped it",
                "Note:".cyan().bold(),
                ceiling.as_secs() / 60
            );
            reason = bg::ResumeReason::Terminated;
        }
        resumes += 1;
        run_log = resume_log_path(&log_path, resumes);
        record_slot_log(launch.project_root, &run_log);
        eprintln!(
            "{} resuming the implementer session ({resumes}/{}; {}) → {}",
            crate::glyph(crate::glyphs::Glyph::FlowActive)
                .green()
                .bold(),
            bg::MAX_AUTO_RESUMES,
            match reason {
                bg::ResumeReason::Terminated => "its background run was stopped at the ceiling",
                bg::ResumeReason::LiveWork | bg::ResumeReason::HeldForWork =>
                    "its background run finished",
                bg::ResumeReason::SaidWaiting => "it ended its turn waiting on a background run",
            },
            run_log.display().to_string().dimmed()
        );
        run_prompt = bg::resume_prompt(&reason).to_string();
    };
    crate::session::revoke_standalone_implementer_grant(launch.child_grant);
    let (status, outcome) = finished?;
    store_commit.commit();

    // trace:BUG-B6B1278890774EF993D2EBC327862470-044 | ai:claude
    // BUG-050: a stale PR is as unshipped as no PR.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
    let unshipped =
        matches!(outcome, Outcome::NoPr) || is_stale_pr(&outcome, head_before.as_deref());
    let uncommitted = unshipped
        .then(|| bg::uncommitted_paths(&lease_worktree(launch.project_root, launch.lease_id)))
        .flatten()
        .filter(|n| *n > 0);
    let outcome = settle_outcome(outcome, head_before.as_deref(), uncommitted);
    let (line, code) = report(&outcome, status.code(), uncommitted);
    println!("{line}");
    if code == 0 {
        return Ok(());
    }
    let message = match &outcome {
        Outcome::Punted { .. } => format!(
            "the implementer punted {}; see `aida show {} --comments`",
            launch.spec, launch.spec
        ),
        Outcome::Held { branch } => {
            format!("the implementer pushed `{branch}` and is holding the PR for a manual gate")
        }
        _ => format!(
            "the implementer exited ({}) without opening a PR for {}{}{}; its log is {}",
            status
                .code()
                .map(|c| format!("code {c}"))
                .unwrap_or_else(|| "on a signal".to_string()),
            launch.spec,
            if resumes > 0 {
                format!(" after {resumes} automatic resume(s)")
            } else {
                String::new()
            },
            uncommitted
                .map(|n| format!(", leaving {n} uncommitted path(s) in its worktree"))
                .unwrap_or_default(),
            run_log.display()
        ),
    };
    Err(anyhow::Error::new(StandaloneImplementerExit {
        vendor_exit: matches!(outcome, Outcome::NoPr) && code != NO_PR_EXIT_CODE,
        code,
        message,
    }))
}

/// Claim the spec for the lane (signoff C6): an empty owner becomes the
/// launching principal, and an Approved spec moves to In Progress (a no-op
/// when the pickup's own status bump already did). Best effort: a store
/// that cannot be written warns and the launch goes on.
pub(crate) fn claim_spec(project_root: &Path, spec: &str) {
    let store_path = crate::detect_distributed_store_from(project_root)
        .unwrap_or_else(|| project_root.join(".aida-store"));
    let storage = Storage::new(store_path);
    let claimed = storage.load().and_then(|mut store| {
        let Some(req) = store.get_requirement_unambiguous_mut(spec)? else {
            return Ok(false);
        };
        let mut changed = crate::leases::approved_to_in_progress_bump(
            req,
            aida_core::conflict::SESSION_START_AUTHOR,
        );
        if req.owner.trim().is_empty() {
            req.owner = crate::get_default_author();
            req.modified_at = chrono::Utc::now();
            changed = true;
        }
        if changed {
            storage.save(&store)?;
        }
        Ok::<bool, anyhow::Error>(changed)
    });
    if let Err(e) = claimed {
        eprintln!(
            "{} could not claim {spec} for the implementer (owner / In Progress): {e:#}",
            "Warning:".yellow().bold()
        );
    }
}

/// A forge-less (pure-git) project reports the branch itself as change 0.
/// That is not a PR the caller can review.
pub(crate) fn is_real_pr(pr: &crate::OpenPrInfo) -> bool {
    pr.number > 0
}

/// Whether `pr` is THIS run's PR: a real PR whose head is the lease branch.
/// A PR on any other branch (a sibling slice of the same spec, say) is not
/// this implementer's outcome.
pub(crate) fn is_run_pr(pr: &crate::OpenPrInfo, branch: &str) -> bool {
    is_real_pr(pr) && pr.head_branch.as_deref() == Some(branch)
}

/// The open PR whose head is the run's branch, with its head commit. A
/// just-created PR may lag the forge's list queries by seconds, so a branch
/// that reached origin gets two more looks before the run is called PR-less.
fn lookup_pr(project_root: &Path, branch: &str) -> Option<(u64, Option<String>)> {
    lookup_pr_with(project_root, branch, 3)
}

/// [`lookup_pr`] with `attempts` looks (one look never sleeps).
fn lookup_pr_with(
    project_root: &Path,
    branch: &str,
    attempts: u64,
) -> Option<(u64, Option<String>)> {
    for attempt in 0..attempts {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_secs(3 * attempt));
        }
        if let PrLookup::Found(pr) = crate::detect_open_pr_for_branch(project_root, branch) {
            if is_run_pr(&pr, branch) {
                return Some((
                    pr.number,
                    crate::queue_cmd::standalone_review_pr_head(project_root, pr.number),
                ));
            }
        }
        if !matches!(
            crate::probe_branch_on_origin(project_root, branch),
            BranchOriginProbe::Present
        ) {
            return None;
        }
    }
    None
}

#[cfg(test)]
#[path = "tests/story_1662_standalone_implementer_tests.rs"]
mod story_1662_standalone_implementer_tests;
