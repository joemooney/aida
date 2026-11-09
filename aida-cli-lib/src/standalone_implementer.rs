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

#[path = "standalone_empty_rework.rs"]
mod empty_rework;

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
/// Unchanged blocked rework, queued for its one automatic retry.
pub(crate) const EMPTY_REWORK_EXIT_CODE: i32 = empty_rework::EXIT_CODE;

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

/// Native seats finish through the host after the child exits, while the
/// recorded lease still vouches for the work. This overrides the ordinary
/// pickup shipping unit without weakening the live-owner check.
// trace:TASK-2879 trace:BUG-2493 | ai:codex
pub(crate) const NATIVE_STOP_CONTRACT: &str = "\n\nStandalone native implementer launch: \
Git metadata is read-only. Finish the edits and targeted tests, leave the edits uncommitted, \
and write a final handoff listing changed files, validation results and blockers, then exit. \
Do not commit, push, open a PR or run `aida queue done` from this seat. After your process \
exits, the host launcher commits and publishes the PR; queue completion follows publication. \
Do not run `aida pr ship` or `aida review record`. For a design fork or manual gate, \
run `aida punt`; existing punt or hold signals remain publication vetoes. \
For a partial slice, use `aida ship -m \"[AI:codex] type(scope): subject (Refs SPEC-ID)\" \
--commit-body-file <file>` to declare the host publication subject and optional body; \
this records intent without publishing. \
For a completing lane, use `aida ship -m \"[AI:codex] type(scope): subject (SPEC-ID)\" \
--commit-body-file <file>` to declare completion with an optional body. \
Omit `--commit-body-file <file>` when no body trailers are needed. \
For required commit-body trailers such as `Help-Golden: unchanged`, write a UTF-8 file \
outside the checkout (for example under `/tmp`) and add `--commit-body-file <file>` \
to the declaration. Avoid body lines ending in `(ID)` groups, which can supply completion credit.";

/// BUG-2383: how a standalone implementer runs its local tests. Appended to
/// its prompt when the launcher's lane job runner is up.
// trace:BUG-2383 | ai:claude
// trace:BUG-2383 | ai:codex
pub(crate) const LANE_JOB_CONTRACT: &str = "\n\nLocal tests in this lane: a tool call is capped \
at 60 seconds and anything you detach (`setsid nohup ...`, `&`) dies with the call, so do not run \
cargo yourself and do not background it. Hand the run to the launcher instead: \
`aida lane job run -- cargo test --profile agent -p <crate> --lib -- <filter>`. It accepts \
`cargo test|check|build --profile agent ...`, runs in this seat inside a sandbox, with its own \
target, under the cargo slot limiter, and returns within 50 seconds. The runner picks the run's \
timeout: 900 seconds on a warm target, 3600 when the output says `seed: cold` (a cold build \
can take half an hour); set `AIDA_CARGO_TIMEOUT=<secs>` only to override it. When \
it finishes it prints the output tail and exits with cargo's exit code; while it still runs it \
prints the job id and exits 75, and you continue with `aida lane job wait <id>` until it \
finishes. Only a `cargo test` with an executed-test result counts as tested; compile errors, \
zero-match filters, `--no-run` and `--list` do not. If it reports that this session has no lane \
job runner, or refuses the run, say `untested=true` in your final result.";

/// Append `untested=true` to a result line when the lane ran no local test
/// to completion (BUG-2383 AC3), so whoever launched it knows the push
/// reached CI untested. A punt's line is left alone: nothing was shipped.
// trace:BUG-2383 | ai:claude
pub(crate) fn with_untested(line: String, outcome: &Outcome, untested: bool) -> String {
    if untested && !matches!(outcome, Outcome::Punted { .. }) {
        format!("{line} untested=true")
    } else {
        line
    }
}

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
    /// BUG-050: a mock forge. `Some(answer)` is what every PR lookup on the
    /// run's branch returns (`answer` itself may be `None`: no PR).
    pub(crate) static TEST_PR: std::cell::RefCell<Option<Option<(u64, Option<String>)>>> =
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
    // trace:BUG-2488 | ai:codex
    NoChange,
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
/// an open PR is better evidence than the exit code. `unshipped` is the work
/// a PR-less run left in its seat (BUG-044, BUG-050), named as the reason.
pub(crate) fn report(
    outcome: &Outcome,
    vendor_code: Option<i32>,
    unshipped: Option<&Unshipped>,
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
        Outcome::NoChange => ("implementer: no-change".into(), 0),
        Outcome::NoPr => (
            match unshipped {
                Some(Unshipped::UncommittedWork(n)) => {
                    format!("implementer: no-pr reason=uncommitted-work uncommitted={n}")
                }
                Some(Unshipped::UnpushedCommits) => {
                    "implementer: no-pr reason=unpushed-commits".to_string()
                }
                None => "implementer: no-pr".to_string(),
            },
            vendor_code.filter(|c| *c != 0).unwrap_or(NO_PR_EXIT_CODE),
        ),
    }
}

/// BUG-050: the seat as this run found it at launch: the branch's open PR,
/// if any (a rework round), and the seat's local `HEAD`.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct LaunchSeat<'a> {
    pub(crate) pr: Option<&'a (u64, Option<String>)>,
    pub(crate) head: Option<&'a str>,
}

impl LaunchSeat<'_> {
    /// Whether `outcome` is the rework round's PR with nothing of this run's
    /// on it: the seat has not pushed past its launch head. Read from the
    /// seat's git (`HEAD` against `origin` and the launch head); only when
    /// git cannot tell does it fall back to the forge's PR head, which lags
    /// a push by seconds. A PR this run opened is never stale.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
    pub(crate) fn is_stale(&self, outcome: &Outcome, seat: &SeatState) -> bool {
        let (Outcome::PrOpened { head, .. }, Some((_, pr_head_at_launch))) = (outcome, self.pr)
        else {
            return false;
        };
        match seat.pushed_since(self.head) {
            Some(pushed) => !pushed,
            None => head.is_some() && head == pr_head_at_launch,
        }
    }
}

/// BUG-050: the outcome to report once the run is done, with the work it
/// left unshipped. A stale PR (see [`LaunchSeat::is_stale`]) with work left
/// in the seat is a stall, reported [`Outcome::NoPr`] so the caller does not
/// read the old head as a finished push; on a clean, unmoved seat it stays
/// `pr-opened` (the round had nothing to change). A shipped PR reports the
/// seat's pushed `HEAD` when `origin` is at it, ahead of the forge.
// trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
pub(crate) fn settle_outcome(
    outcome: Outcome,
    launch: &LaunchSeat<'_>,
    seat: &SeatState,
) -> (Outcome, Option<Unshipped>) {
    if let Outcome::PrOpened { pr, head } = &outcome {
        if !launch.is_stale(&outcome, seat) {
            let head = seat
                .pushed_head()
                .map(String::from)
                .or_else(|| head.clone());
            return (Outcome::PrOpened { pr: *pr, head }, None);
        }
        return match seat.unshipped(launch.head) {
            Some(unshipped) => (Outcome::NoPr, Some(unshipped)),
            None => (outcome, None),
        };
    }
    match outcome {
        Outcome::NoPr => (Outcome::NoPr, seat.unshipped(launch.head)),
        other => (other, None),
    }
}

pub(crate) use crate::headless_background::{resume_log_path, SeatState, Unshipped};

/// BUG-2436: whether a finished lane's lease is ended by its launcher: the
/// run settled as `pr-opened`, the seat's tree is clean, and `origin` is at
/// its `HEAD`. Nothing is left in the seat, and a live lease would only make
/// the review watcher refuse the PR as owned by a live author. A punt, a
/// hold, a stall, or a seat git cannot read keeps its lease.
// trace:BUG-2436 | ai:claude
pub(crate) fn finished_lease_ends(outcome: &Outcome, seat: &SeatState) -> bool {
    seat.uncommitted == Some(0)
        && (matches!(outcome, Outcome::NoChange)
            || (matches!(outcome, Outcome::PrOpened { .. }) && seat.pushed_head().is_some()))
}

/// BUG-2436: end a finished lane's lease the way the drain's phase 2 does,
/// `aida session end <lease> --yes --skip-ci` from the main checkout, which
/// releases the lease, returns the tree, and queues the PR's review. Best
/// effort: a refusal (the seat changed since it was read, say) keeps the
/// lease and is reported as a warning, and the run's result stands.
// trace:BUG-2436 | ai:claude
pub(crate) fn end_finished_lease(project_root: &Path, lease_id: &str) -> Result<()> {
    use crate::process_retry::RetryEtxtbsy;
    let main = crate::main_worktree_root_from(project_root);
    let status = std::process::Command::new(crate::kernel::aida_exe_path())
        .current_dir(&main)
        .args(["session", "end", lease_id, "--yes", "--skip-ci"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .status_retrying_etxtbsy()
        .with_context(|| format!("could not run `aida session end {lease_id}`"))?;
    if !status.success() {
        anyhow::bail!("`aida session end {lease_id} --yes --skip-ci` exited {status}");
    }
    Ok(())
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
// the host's `~/.aida/standalone-implementers/`, shared across clones. Counting and taking a
// slot happen under one file lock, so two launchers starting together cannot
// both see room for one. A slot whose process is gone is pruned, so a killed
// launcher never holds a slot.

fn legacy_slots_dir(project_root: &Path) -> PathBuf {
    crate::main_worktree_root_from(project_root)
        .join(".aida")
        .join("standalone-implementers")
}

// trace:STORY-2660 | ai:codex
fn host_home() -> Result<PathBuf> {
    let environment = aida_core::exec_context::Environment::capture();
    environment
        .get("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .context("absolute HOME unavailable; cannot resolve host lane state")
}

fn slots_dir() -> Result<PathBuf> {
    Ok(host_home()?.join(".aida/standalone-implementers"))
}

// A guard can release its reservation without the admission lock. A directory
// entry may therefore disappear before it is opened; other read failures refuse.
// trace:STORY-2660 | ai:codex
fn read_slot(path: &Path) -> Result<Option<Slot>> {
    let body = match std::fs::read(path) {
        Ok(body) => body,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading reservation {}", path.display())),
    };
    serde_json::from_slice(&body).map(Some).with_context(|| {
        format!(
            "invalid reservation {}; host capacity unknown",
            path.display()
        )
    })
}

/// Old launchers used checkout-local slots. Fail closed during the transition;
/// only the owning old launcher may release its live reservation.
fn refuse_legacy_slots(project_root: &Path) -> Result<()> {
    refuse_legacy_slots_from(
        project_root,
        aida_core::pid_observer::PidObserver::current(),
    )
}

fn refuse_legacy_slots_from(
    project_root: &Path,
    observer: aida_core::pid_observer::PidObserver,
) -> Result<()> {
    let dir = legacy_slots_dir(project_root);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).context("checking legacy lane slots"),
    };
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|v| v.to_str()) != Some("json") {
            continue;
        }
        let Some(slot) = read_slot(&path)? else {
            continue;
        };
        anyhow::ensure!(slot_liveness(&slot, observer, crate::ownership::local_slot_process_is_alive) == Some(false),
            "legacy checkout-local launcher {} still holds {}; let it finish before using host-wide slots", slot.pid, path.display());
    }
    Ok(())
}

// trace:STORY-2660 | ai:codex
#[derive(Debug, serde::Serialize)]
pub(crate) struct HostCapacity {
    pub cap: usize,
    pub cap_source: &'static str,
    pub nproc: usize,
    pub allow_ungranted: bool,
}

/// Resolve host policy without changing tracked project configuration.
fn resolve_host_capacity(
    host: Option<&Path>,
    project: Option<&toml::Value>,
    nproc: usize,
) -> Result<HostCapacity> {
    let host = match host {
        Some(path) => match std::fs::read_to_string(path) {
            Ok(body) => Some(
                body.parse::<toml::Value>()
                    .with_context(|| format!("invalid host config {}", path.display()))?,
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e).context("reading host.toml; refusing lane admission"),
        },
        None => None,
    };
    let host_cap = host
        .as_ref()
        .and_then(|v| v.get("queue"))
        .map(|queue| {
            anyhow::ensure!(queue.is_table(), "host.toml [queue] must be a table");
            queue
                .get("standalone_implementer_cap")
                .map(|v| {
                    v.as_integer()
                        .and_then(|n| usize::try_from(n).ok())
                        .filter(|n| *n > 0)
                        .context("host.toml standalone_implementer_cap must be a positive integer")
                })
                .transpose()
        })
        .transpose()?
        .flatten();
    let allow_ungranted = match host.as_ref().and_then(|v| v.get("lane_host")) {
        Some(table) => {
            anyhow::ensure!(table.is_table(), "host.toml [lane_host] must be a table");
            table
                .get("allow_ungranted")
                .map(|v| {
                    v.as_bool()
                        .context("host.toml allow_ungranted must be boolean")
                })
                .transpose()?
                .unwrap_or(false)
        }
        None => false,
    };
    let project_cap = project
        .and_then(|v| v.get("queue"))
        .and_then(|v| v.get("standalone_implementer_cap"))
        .and_then(toml::Value::as_integer)
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| *n > 0);
    let (cap, cap_source) = if let Some(cap) = host_cap {
        (cap, "host")
    } else if let Some(cap) = project_cap {
        (cap, "project")
    } else {
        ((nproc / 2).max(1), "cpu")
    };
    Ok(HostCapacity {
        cap,
        cap_source,
        nproc,
        allow_ungranted,
    })
}

pub(crate) fn host_capacity(project_root: &Path) -> Result<HostCapacity> {
    let home = host_home()?;
    let root = crate::main_worktree_root_from(project_root);
    let project = crate::read_project_config_value(&root);
    let nproc = std::thread::available_parallelism()
        .context("reading effective host CPU count")?
        .get();
    resolve_host_capacity(Some(&home.join(".aida/host.toml")), project.as_ref(), nproc)
}

pub(crate) fn configured_cap(project_root: &Path) -> Result<usize> {
    Ok(host_capacity(project_root)?.cap)
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Slot {
    pid: u32,
    #[serde(default)]
    pid_namespace: Option<u64>,
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

// Unknown PID visibility must retain capacity, even if a final log exists.
// trace:STORY-2660 | ai:codex
fn slot_liveness(
    slot: &Slot,
    observer: aida_core::pid_observer::PidObserver,
    is_alive: impl Fn(u32, Option<&str>) -> bool,
) -> Option<bool> {
    observer
        .can_observe(slot.pid_namespace)
        .then(|| is_alive(slot.pid, slot.start.as_deref()))
}

// trace:STORY-2660 | ai:codex
#[derive(serde::Serialize)]
pub(crate) struct HostReservation {
    pid: u32,
    spec: Option<String>,
    state: &'static str,
    counted: bool,
}

#[derive(serde::Serialize)]
pub(crate) struct HostStatus {
    schema_version: u32,
    #[serde(flatten)]
    capacity: HostCapacity,
    slot_scope: PathBuf,
    pub(crate) occupied: usize,
    free: usize,
    reservations: Vec<HostReservation>,
    warnings: Vec<String>,
}

/// Observes existing slot state only: no directories, locks or receipts created,
/// and no dead reservations removed. A busy writer is an unknown observation.
pub(crate) fn host_status(project_root: &Path) -> Result<HostStatus> {
    refuse_legacy_slots(project_root)?;
    observe_host_slots(slots_dir()?, host_capacity(project_root)?, |pid, start| {
        crate::ownership::local_slot_process_is_alive(pid, start)
    })
}

fn observe_host_slots(
    dir: PathBuf,
    capacity: HostCapacity,
    is_alive: impl Fn(u32, Option<&str>) -> bool,
) -> Result<HostStatus> {
    observe_host_slots_from(
        dir,
        capacity,
        is_alive,
        aida_core::pid_observer::PidObserver::current(),
    )
}

fn observe_host_slots_from(
    dir: PathBuf,
    capacity: HostCapacity,
    is_alive: impl Fn(u32, Option<&str>) -> bool,
    observer: aida_core::pid_observer::PidObserver,
) -> Result<HostStatus> {
    use fs2::FileExt;
    let mut reservations = Vec::new();
    let mut warnings = Vec::new();
    if capacity.allow_ungranted {
        warnings.push("Legacy ungranted pool placement opted in; no remote grant issuer (placement is not enabled in this slice)".into());
    }
    let mut lock = None;
    match std::fs::read_dir(&dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).context("reading host reservations"),
        Ok(entries) => {
            let file = std::fs::File::open(dir.join(".lock"))
                .context("opening existing slot observation lock")?;
            FileExt::try_lock_shared(&file)
                .context("slot writer busy; host status unknown, retry")?;
            lock = Some(file);
            for entry in entries {
                let path = entry?.path();
                if path.extension().and_then(|v| v.to_str()) != Some("json") {
                    continue;
                }
                let Some(slot) = read_slot(&path)? else {
                    continue;
                };
                let live = slot_liveness(&slot, observer, &is_alive);
                let finishing = !slot.waiting
                    && slot
                        .log
                        .as_deref()
                        .is_some_and(crate::headless_linger::log_has_final_result);
                reservations.push(HostReservation {
                    pid: slot.pid,
                    spec: Some(slot.spec),
                    state: if live.is_none() {
                        "unknown"
                    } else if live == Some(false) {
                        "dead"
                    } else if finishing {
                        "finishing"
                    } else {
                        "running"
                    },
                    counted: live.is_none() || (live == Some(true) && !finishing),
                });
            }
        }
    }
    reservations.sort_by_key(|r| r.pid);
    let occupied = reservations.iter().filter(|r| r.counted).count();
    let free = capacity.cap.saturating_sub(occupied);
    drop(lock);
    Ok(HostStatus {
        schema_version: 1,
        capacity,
        slot_scope: dir,
        occupied,
        free,
        reservations,
        warnings,
    })
}

pub(crate) fn doctor_host_status(project_root: &Path, json: bool) -> Result<()> {
    let status = host_status(project_root)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&status)?);
    } else {
        println!(
            "Lane host capacity: {} (source: {}, nproc: {}); occupied: {}, free: {}",
            status.capacity.cap,
            status.capacity.cap_source,
            status.capacity.nproc,
            status.occupied,
            status.free
        );
        println!("Slot scope: {}", status.slot_scope.display());
        for r in &status.reservations {
            println!(
                "  pid {} {} {}",
                r.pid,
                r.spec.as_deref().unwrap_or("unknown"),
                r.state
            );
        }
        for warning in &status.warnings {
            println!("WARNING: {warning}");
        }
        println!("Read-only local capacity observation; not a lane-host readiness or remote placement admission.");
    }
    Ok(())
}

/// Rewrite this process's slot with `edit`. Best effort.
fn edit_own_slot(edit: impl FnOnce(&mut Slot)) {
    let Ok(dir) = slots_dir() else {
        return;
    };
    let path = dir.join(format!("{}.json", std::process::id()));
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
pub(crate) fn record_slot_log(log_path: &Path) {
    edit_own_slot(|slot| {
        slot.log = Some(log_path.to_path_buf());
        slot.waiting = false;
    });
}

/// Mark this process's slot as waiting on its child's background run, so
/// the cap keeps counting it while the logged final result says it ended.
// trace:BUG-B6B1278890774EF993D2EBC327862470-044 | ai:claude
pub(crate) fn mark_slot_waiting() {
    edit_own_slot(|slot| slot.waiting = true);
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
    refuse_legacy_slots(project_root)?;
    acquire_slot_in(slots_dir()?, spec, cap, std::process::id(), |pid, start| {
        crate::ownership::local_slot_process_is_alive(pid, start)
    })
}

#[cfg(test)]
pub(crate) fn acquire_slot_with(
    project_root: &Path,
    spec: &str,
    cap: usize,
    pid: u32,
    is_alive: impl Fn(u32, Option<&str>) -> bool,
) -> Result<SlotGuard> {
    acquire_slot_in(legacy_slots_dir(project_root), spec, cap, pid, is_alive)
}

fn acquire_slot_in(
    dir: PathBuf,
    spec: &str,
    cap: usize,
    pid: u32,
    is_alive: impl Fn(u32, Option<&str>) -> bool,
) -> Result<SlotGuard> {
    acquire_slot_in_from(
        dir,
        spec,
        cap,
        pid,
        is_alive,
        aida_core::pid_observer::PidObserver::current(),
    )
}

fn acquire_slot_in_from(
    dir: PathBuf,
    spec: &str,
    cap: usize,
    pid: u32,
    is_alive: impl Fn(u32, Option<&str>) -> bool,
    observer: aida_core::pid_observer::PidObserver,
) -> Result<SlotGuard> {
    use fs2::FileExt;
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
    for entry in std::fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(slot) = read_slot(&path)? else {
            continue;
        };
        match slot {
            slot if slot_liveness(&slot, observer, &is_alive).is_none() => {
                running.push(slot.spec);
            }
            // A launcher whose child already logged its final result is
            // finishing, not running: its slot stays but is not counted.
            // trace:BUG-B6B1278890774EF993D2EBC327862470-027 | ai:claude
            slot if slot.pid != pid && is_alive(slot.pid, slot.start.as_deref()) => {
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
                 (`[queue] standalone_implementer_cap` in ~/.aida/host.toml or .aida/config.toml). Wait for one to \
                 finish, then launch again. Nothing was set up for {spec}.",
                running.len(),
                running.join(", ")
            ),
        }));
    }
    let path = dir.join(format!("{pid}.json"));
    anyhow::ensure!(
        !path.exists(),
        "reservation PID collision with unobservable owner; retry on the host"
    );
    let slot = Slot {
        pid,
        pid_namespace: aida_core::pid_observer::current_pid_namespace(),
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
    /// TASK-2032: the host main `handle_queue_work` resolved before its chdir
    /// into the seat; the child's boundary is built from it.
    // trace:TASK-2032 | ai:claude
    pub(crate) host_main: Option<&'a Path>,
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

// Revoke on setup, vendor and gate errors as well as successful completion.
// trace:STORY-2640 trace:BUG-2139 | ai:codex
pub(crate) struct ChildGrantGuard<'a>(
    pub(crate) &'a crate::gate::seat_authority::HeadlessChildGrant,
);

impl Drop for ChildGrantGuard<'_> {
    fn drop(&mut self) {
        crate::session::revoke_standalone_implementer_grant(self.0);
    }
}

/// Pin every handshake parent before consuming any prior-attempt marker.
// trace:BUG-2488 | ai:codex
fn prepare_signal_paths(worktree: &Path, spec: &str) -> Result<[(PathBuf, std::fs::File); 2]> {
    let punt = crate::punt::signal_path(worktree, spec);
    let hold = crate::punt::hold_signal_path(worktree, spec);
    let punt_parent = crate::punt::seat_handshake_parent(worktree, &punt, true)?
        .context("missing punt directory")?;
    let hold_parent = crate::punt::seat_handshake_parent(worktree, &hold, true)?
        .context("missing hold directory")?;
    Ok([(punt, punt_parent), (hold, hold_parent)])
}

// trace:BUG-2488 | ai:codex
fn clear_signal_paths(paths: &[(PathBuf, std::fs::File)]) -> Result<()> {
    use std::os::fd::AsRawFd;
    for (path, parent) in paths {
        let pinned = PathBuf::from(format!("/proc/self/fd/{}", parent.as_raw_fd()));
        let name = path.file_name().context("invalid signal filename")?;
        // Clear every spelling through the pinned parent, including entries
        // hidden by the exact-name preference in signal_path.
        // trace:BUG-2488 | ai:codex
        for entry in std::fs::read_dir(&pinned)? {
            let entry = entry?;
            if !entry
                .file_name()
                .as_encoded_bytes()
                .eq_ignore_ascii_case(name.as_encoded_bytes())
            {
                continue;
            }
            match std::fs::remove_file(pinned.join(entry.file_name())) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(())
}

/// Put the handshakes inside the leased seat's writable runtime directory.
// trace:BUG-2488 | ai:codex
#[cfg(test)]
pub(crate) fn provision_signals(worktree: &Path, spec: &str) -> Result<(PathBuf, PathBuf)> {
    let paths = prepare_signal_paths(worktree, spec)?;
    clear_signal_paths(&paths)?;
    Ok((paths[0].0.clone(), paths[1].0.clone()))
}

/// Start a new attempt after held-resume policy and command preflight succeed.
// trace:BUG-2488 | ai:codex
pub(crate) fn provision_attempt_signals(
    main: &Path,
    tree: &Path,
    spec: &str,
) -> Result<(PathBuf, PathBuf)> {
    // Retain the same pinned parents through cleanup: re-opening the seat
    // after consuming host markers could introduce a second refusal/race.
    let seat = prepare_signal_paths(tree, spec)?;
    let host = prepare_signal_paths(main, spec)?;
    clear_signal_paths(&seat)?;
    clear_signal_paths(&host)?;
    Ok((seat[0].0.clone(), seat[1].0.clone()))
}

/// Consume both copies of a handled punt so an advisor resume cannot replay it.
/// Hold markers remain available for held-resume admission.
// trace:BUG-2488 | ai:codex
pub(crate) fn consume_punt_signal(main: &Path, tree: &Path, spec: &str) -> Result<()> {
    // Recovery receipts may refer to a workspace the child already removed.
    // Consumption only unlinks existing punts; it must neither recreate that
    // workspace nor require unrelated hold directories to be writable.
    // trace:BUG-2488 | ai:codex
    let mut paths = Vec::new();
    for root in [tree, main] {
        let path = crate::punt::signal_path(root, spec);
        if let Some(parent) = crate::punt::seat_handshake_parent(root, &path, false)? {
            paths.push((path, parent));
        }
    }
    clear_signal_paths(&paths)
}

/// Install explicit seat paths rather than inheriting a host-only handshake.
// trace:BUG-2488 | ai:codex
pub(crate) fn apply_seat_signals(cmd: &mut std::process::Command, tree: &Path, spec: &str) {
    cmd.env(
        crate::punt::SIGNAL_FILE_ENV,
        crate::punt::signal_path(tree, spec),
    )
    .env(
        crate::punt::HOLD_SIGNAL_FILE_ENV,
        crate::punt::hold_signal_path(tree, spec),
    );
}

/// Reconcile seat-local runtime evidence before publication or lease cleanup.
// trace:BUG-2488 | ai:codex
pub(crate) fn reconcile_signals(main: &Path, tree: &Path, spec: &str) -> Result<()> {
    // A retired/missing seat has no local evidence to mirror. Host-side signals
    // still apply, including phase recovery after a child lost its workspace.
    if main == tree
        || matches!((main.canonicalize(), tree.canonicalize()), (Ok(a), Ok(b)) if a == b)
    {
        return Ok(());
    }
    if let Some(signal) = crate::punt::read_seat_signal(tree, spec)? {
        crate::punt::write_signal(&crate::punt::signal_path(main, spec), &signal)?;
    }
    if let Some(signal) = crate::punt::read_seat_hold_signal(tree, spec)? {
        crate::punt::write_hold_signal(&crate::punt::hold_signal_path(main, spec), &signal)?;
    }
    let mut existing = crate::punt::read_ledger(main);
    for record in crate::punt::read_seat_ledger(tree)?
        .into_iter()
        .filter(|r| r.spec.eq_ignore_ascii_case(spec))
    {
        // Resolution rewrites the host record; the seat retains its open copy.
        // Compare immutable identity so replay cannot reopen a resolved punt.
        if !existing.iter().any(|r| {
            r.spec.eq_ignore_ascii_case(&record.spec)
                && r.timestamp == record.timestamp
                && r.category == record.category
        }) {
            crate::punt::append_to_ledger(main, &record)?;
            existing.push(record);
        }
    }
    Ok(())
}

/// Classify only successful, unpublished native runs as unchanged.
// trace:BUG-2488 | ai:codex
pub(crate) fn classify_no_change(
    outcome: Outcome,
    success: bool,
    has_commits: impl FnOnce() -> Result<bool>,
) -> Result<Outcome> {
    if matches!(outcome, Outcome::NoPr) && success && !has_commits()? {
        Ok(Outcome::NoChange)
    } else {
        Ok(outcome)
    }
}

/// Worktree fallback lanes retain their ordinary unpublished-work outcome.
// trace:BUG-2488 | ai:codex
pub(crate) fn classify_seat_no_change(
    outcome: Outcome,
    success: bool,
    main: &Path,
    tree: &Path,
) -> Result<Outcome> {
    if crate::seat_clone::host_recorded_clone(main, tree).is_none() {
        return Ok(outcome);
    }
    classify_no_change(outcome, success, || {
        crate::seat_clone::has_commits_beyond_base(main, tree)
    })
}

/// Run the child, wait, classify, print the result line, and return the
/// outcome's exit code (`Ok` only for an open PR).
pub(crate) fn run(launch: Launch<'_>) -> Result<()> {
    let child_grant_guard = ChildGrantGuard(launch.child_grant);
    let main_root = crate::main_worktree_root_from(launch.project_root);
    claim_spec(launch.project_root, launch.spec);
    let worktree = lease_worktree(launch.project_root, launch.lease_id);
    // Resolve transport now, but retain the prior veto until command policy
    // and preparation have succeeded at the final spawn boundary.
    // trace:BUG-2488 | ai:codex
    let punt_signal = crate::punt::signal_path(&worktree, launch.spec);
    let hold_signal = crate::punt::hold_signal_path(&worktree, launch.spec);
    // BUG-2383: the lane job runner, so the seat can run its tests here,
    // outside its 60-second tool-call cap. trace:BUG-2383 | ai:claude
    // trace:TASK-2902 | ai:codex
    if launch.vendor == crate::session::HeadlessVendor::Codex {
        crate::native_publication::prepare(&main_root, &worktree)?;
    }
    // A job runs seat-written code, so it runs only inside bubblewrap: with
    // os_wrap off the runner would refuse every job, so it is not offered and
    // the lane reports untested=true. An unset os_wrap defaults on for this
    // lane's runner alone when bubblewrap works (BUG-2434).
    // trace:BUG-2434 | ai:claude
    let mut _fast_gate = crate::lane_fast_gate::reserve(&main_root, launch.branch)?;
    let lane_jobs = crate::lane_job::offer(&main_root, &worktree, launch.lease_id);
    let prompt = format!(
        "{}{}{}",
        launch.prompt,
        if launch.vendor == crate::session::HeadlessVendor::Codex
            && crate::session::containment_posture(launch.vendor, launch.contained)?
                == crate::session::ContainmentPosture::Native
        {
            NATIVE_STOP_CONTRACT
        } else {
            STOP_CONTRACT
        },
        if lane_jobs.is_some() {
            LANE_JOB_CONTRACT
        } else {
            ""
        }
    );
    let log_path = launch
        .project_root
        .join(".aida")
        .join("headless-logs")
        .join(format!("{}-{}.jsonl", launch.branch, launch.session_id));
    // trace:BUG-B6B1278890774EF993D2EBC327862470-027 | ai:claude
    record_slot_log(&log_path);
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
        apply_standalone_child_env(cmd, launch.lease_id, &punt_signal, &hold_signal);
        if let Some(lane_jobs) = &lane_jobs {
            lane_jobs.apply_selector(cmd);
        }
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
    // BUG-2386: a Codex lane resumes too, by the thread its first run bound
    // to the lease. trace:BUG-2386 | ai:claude
    let can_resume = crate::session::vendor_can_resume(launch.vendor);
    let ceiling = std::time::Duration::from_millis(
        crate::burndown_run::resolved_bg_wait_ceiling_ms(Some(launch.project_root)),
    );
    // BUG-050: what the seat looked like at launch. A rework round starts
    // with its PR open (`launch_pr`) and its seat on that PR's head
    // (`launch_head`), so finding the PR open afterwards proves nothing:
    // only the seat's git state says whether this run pushed.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
    let launch_head = bg::seat_head(&worktree);
    let launch_pr = lookup_pr_with(launch.project_root, launch.branch, 1);
    let launch_seat = LaunchSeat {
        pr: launch_pr.as_ref(),
        head: launch_head.as_deref(),
    };
    let lane = bg::ResumeLoop {
        can_resume,
        ceiling,
        lease_id: launch.lease_id,
        log_path: &log_path,
        // The lane is still working: keep its cap slot counted.
        on_waiting: &|| mark_slot_waiting(),
        on_log: &|log| record_slot_log(log),
    };
    // Published on the lease, so every vendor's tool shell finds it through
    // its public selector. trace:BUG-2383 | ai:claude
    let runner = lane_jobs
        .clone()
        .map(|lane| crate::lane_job::Runner::start_published(lane, &main_root));
    let run_child = |prompt: &str, log: &Path, resume: bool, hold: &dyn Fn() -> bool| {
        crate::session::run_standalone_implementer_child(
            // Bind the stamp to this launch's lease root, even uncontained.
            // trace:STORY-2537 trace:BUG-2458 | ai:codex
            launch.project_root,
            launch.host_main,
            launch.vendor,
            prompt,
            launch.session_id,
            log,
            &tee_opts,
            launch.contained,
            launch.lease_id,
            launch.child_grant,
            &crate::session::HeadlessChildHooks {
                customize: Some(&customize),
                on_spawn: Some(&on_spawn),
                resume,
                hold_reap: Some(hold),
                before_spawn: Some(&|| {
                    if !resume {
                        provision_attempt_signals(&main_root, &worktree, launch.spec)?;
                    }
                    Ok(())
                }),
            },
        )
    };
    let finished = lane.run(prompt, &run_child, || {
        let (Ok(punt), Ok(hold)) = (
            crate::punt::read_seat_signal(&worktree, launch.spec),
            crate::punt::read_seat_hold_signal(&worktree, launch.spec),
        ) else {
            // Stop resuming on unsafe evidence. The mandatory reconciliation
            // below returns its error before any publication or lease cleanup.
            return (Outcome::NoPr, true);
        };
        let outcome = classify(punt, hold, || lookup_pr(launch.project_root, launch.branch));
        let done = match &outcome {
            Outcome::NoPr => false,
            Outcome::NoChange => true,
            Outcome::PrOpened { .. } => {
                !launch_seat.is_stale(&outcome, &bg::SeatState::read(&worktree, launch.branch))
            }
            Outcome::Punted { .. } | Outcome::Held { .. } => true,
        };
        (outcome, done)
    });
    // trace:STORY-2640 | ai:codex — keep the grant and runner through repair.
    let mut finished = finished?;
    reconcile_signals(&main_root, &worktree, launch.spec)?;
    let host_published = launch.vendor == crate::session::HeadlessVendor::Codex && launch.contained;
    let mut publish_native = host_published;
    if host_published
        && !matches!(
            finished.verdict,
            Outcome::Punted { .. } | Outcome::Held { .. }
        )
    {
        publish_native = crate::lane_fast_gate::native(
            &main_root,
            &worktree,
            launch.spec,
            launch.lease_id,
            launch.branch,
            finished.status.success(),
            |prompt| {
                let log = log_path.with_extension("fast-fix.jsonl");
                let result = run_child(prompt, &log, true, &|| false)?;
                reconcile_signals(&main_root, &worktree, launch.spec)?;
                finished.status = result;
                Ok(result.success())
            },
        )?;
        finished.verdict = classify(
            crate::punt::read_seat_signal(&worktree, launch.spec)?,
            crate::punt::read_seat_hold_signal(&worktree, launch.spec)?,
            || lookup_pr(launch.project_root, launch.branch),
        );
    }
    // trace:STORY-2640 | ai:codex
    let actual_branch = if !host_published
        && !matches!(
            finished.verdict,
            Outcome::Punted { .. } | Outcome::Held { .. }
        ) {
        _fast_gate.exit_branch(&worktree)
    } else {
        launch.branch.to_owned()
    };
    let launch = Launch {
        branch: &actual_branch,
        ..launch
    };
    if !host_published
        && !matches!(
            finished.verdict,
            Outcome::Punted { .. } | Outcome::Held { .. }
        )
    {
        finished.verdict = classify(
            crate::punt::read_seat_signal(&worktree, launch.spec)?,
            crate::punt::read_seat_hold_signal(&worktree, launch.spec)?,
            || lookup_pr(launch.project_root, launch.branch),
        );
    }
    // trace:BUG-2383 | ai:claude
    let untested = runner.map(|r| r.stop().untested()).unwrap_or(true);
    drop(child_grant_guard);
    let bg::LoopEnd {
        status,
        verdict: outcome,
        resumes,
        log: run_log,
    } = finished;
    reconcile_signals(&main_root, &worktree, launch.spec)?;
    store_commit.commit();

    // Publish before reading the final seat state or ending its lease. A punt
    // or manual hold is an explicit instruction to leave publication alone.
    // trace:STORY-2475 | ai:codex
    let mut outcome = outcome;
    if publish_native && !matches!(outcome, Outcome::Punted { .. } | Outcome::Held { .. }) {
        crate::lane_fast_gate::publication_result(
            &main_root,
            launch.branch,
            crate::ship_from_seat::publish_queue_exit(
                &main_root,
                &worktree,
                launch.spec,
                launch.lease_id,
                status.success(),
            ),
        )?;
        outcome = classify(None, None, || lookup_pr(launch.project_root, launch.branch));
        // trace:BUG-2488 | ai:codex
        outcome = classify_seat_no_change(outcome, status.success(), &main_root, &worktree)?;
    }

    // trace:STORY-2640 | ai:codex
    let mut fast_rework = false;
    if let Outcome::PrOpened { pr, .. } = &outcome {
        if !host_published {
            fast_rework = crate::lane_fast_gate::pushed(
                &main_root,
                &worktree,
                launch.spec,
                launch.lease_id,
                launch.branch,
                *pr,
                launch.vendor.as_str(),
            )?;
        }
    }
    // trace:BUG-B6B1278890774EF993D2EBC327862470-044 | ai:claude
    // trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
    let seat = bg::SeatState::read(&worktree, launch.branch);
    // trace:BUG-2452 | ai:codex
    // Check before settlement turns a stale PR with leftover work into NoPr.
    if let Some((pr, head, reason)) = if fast_rework {
        None
    } else {
        empty_rework::inspect(&main_root, launch.spec, &launch_seat, &outcome, &seat)?
    } {
        let escalated = empty_rework::route(
            &main_root,
            launch.spec,
            launch.vendor.as_str(),
            pr,
            &head,
            &reason,
        )?;
        let action = if escalated { "punted" } else { "requeued" };
        println!(
            "implementer: empty-rework pr={pr} head={head} action={action} fast_gate={}",
            crate::lane_fast_gate::marker(&main_root, launch.branch)
        );
        return Err(anyhow::Error::new(StandaloneImplementerExit {
            code: if escalated {
                PUNTED_EXIT_CODE
            } else {
                EMPTY_REWORK_EXIT_CODE
            },
            message: format!("empty rework for {}: {action}; {reason}", launch.spec),
            vendor_exit: false,
        }));
    }
    let (outcome, unshipped) = settle_outcome(outcome, &launch_seat, &seat);
    let (line, mut code) = report(&outcome, status.code(), unshipped.as_ref());
    if fast_rework {
        code = EMPTY_REWORK_EXIT_CODE;
    }
    let line = crate::lane_fast_gate::result_line(&main_root, launch.branch, &line);
    // BUG-2436: end the finished lane's lease before the result line goes
    // out, so whoever reads it can launch the review at once.
    // trace:BUG-2436 | ai:claude
    if finished_lease_ends(&outcome, &seat) {
        if let Err(e) = end_finished_lease(launch.project_root, launch.lease_id) {
            eprintln!(
                "{} the lane finished but lease {} was kept: {e:#}. End it with \
                 `aida session end {} --yes --skip-ci` so its review can start.",
                "Warning:".yellow().bold(),
                launch.lease_id,
                launch.lease_id
            );
        }
    }
    // BUG-2434: an untested lane that ran without os_wrap (no job runner)
    // says so on the line its launcher reads. trace:BUG-2434 | ai:claude
    let os_wrap_off = untested
        && !matches!(outcome, Outcome::Punted { .. })
        && crate::lane_job::lane_ran_without_os_wrap(&main_root, launch.lease_id);
    println!(
        "{}",
        crate::lane_job::with_os_wrap_off(with_untested(line, &outcome, untested), os_wrap_off)
    );
    if code == 0 {
        return Ok(());
    }
    let message = if fast_rework {
        format!(
            "fast gate failed for {}; one repair round queued before review",
            launch.spec
        )
    } else {
        match &outcome {
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
                match unshipped {
                    Some(bg::Unshipped::UncommittedWork(n)) =>
                        format!(", leaving {n} uncommitted path(s) in its worktree"),
                    Some(bg::Unshipped::UnpushedCommits) =>
                        ", leaving commits it did not push in its worktree".to_string(),
                    None => String::new(),
                },
                run_log.display()
            ),
        }
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
pub(crate) fn lookup_pr(project_root: &Path, branch: &str) -> Option<(u64, Option<String>)> {
    lookup_pr_with(project_root, branch, 3)
}

/// [`lookup_pr`] with `attempts` looks (one look never sleeps).
fn lookup_pr_with(
    project_root: &Path,
    branch: &str,
    attempts: u64,
) -> Option<(u64, Option<String>)> {
    #[cfg(test)]
    if let Some(pr) = TEST_PR.with(|p| p.borrow().clone()) {
        return pr;
    }
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

#[cfg(test)]
#[path = "tests/bug_2436_finished_lane_lease_tests.rs"]
mod bug_2436_finished_lane_lease_tests;

// trace:STORY-2660 | ai:codex
#[cfg(test)]
mod host_capacity_tests {
    // trace:STORY-2640 trace:BUG-2139 | ai:codex
    #[test]
    fn fast_gate_standalone_errors_revoke_issued_child_grant() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let _env = crate::test_env::AmbientGuard::hermetic_with_seat(
            tmp.path(),
            "advisor",
            &["implementer"],
        );
        for failure in [
            "vendor loop",
            "native commit",
            "uninspectable seat",
            "fast gate reservation",
        ] {
            let child = crate::gate::seat_authority::issue_child(
                tmp.path(),
                "implementer",
                crate::test_env::HERMETIC_TEST_USER,
            )
            .unwrap();
            let id = child.id.clone();
            let grant = crate::gate::seat_authority::HeadlessChildGrant::Issued(child);
            let result: Result<()> = (|| {
                let _guard = super::ChildGrantGuard(&grant);
                if failure == "fast gate reservation" {
                    std::fs::write(tmp.path().join(".aida/fast-gates"), "blocked").unwrap();
                    crate::lane_fast_gate::reserve(tmp.path(), "lane")?;
                    panic!("reservation unexpectedly succeeded");
                }
                Err(anyhow::anyhow!("{failure}"))?;
                Ok(())
            })();
            assert!(result.is_err());
            assert!(
                !crate::gate::seat_authority::revoke_by_id(&id).unwrap(),
                "grant still live after {failure}"
            );
        }
    }

    use super::*;

    #[test]
    fn host_capacity_isolated_observer_retains_unknown_reservations() {
        use aida_core::pid_observer::PidObserver;
        let tmp = tempfile::tempdir().unwrap();
        let dir = legacy_slots_dir(tmp.path());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".lock"), "").unwrap();
        let observer = PidObserver {
            namespace: Some(22),
            isolated: true,
        };
        let log = tmp.path().join("finished.jsonl");
        std::fs::write(&log, "{\"type\":\"result\",\"is_error\":false}\n").unwrap();
        for namespace in [Some(11), None] {
            let slot = Slot {
                pid: 500,
                pid_namespace: namespace,
                start: None,
                spec: "STORY-2660".into(),
                log: Some(log.clone()),
                waiting: false,
            };
            let path = dir.join("500.json");
            let body = serde_json::to_string(&slot).unwrap();
            std::fs::write(&path, &body).unwrap();
            let probe = |_, _: Option<&str>| panic!("unobservable PID must not be probed");
            let status = observe_host_slots_from(
                dir.clone(),
                resolve_host_capacity(None, None, 2).unwrap(),
                probe,
                observer,
            )
            .unwrap();
            assert_eq!((status.occupied, status.free), (1, 0));
            assert_eq!(status.reservations[0].state, "unknown");
            let error = acquire_slot_in_from(dir.clone(), "STORY-OTHER", 1, 500, probe, observer)
                .err()
                .unwrap();
            assert_eq!(
                error
                    .downcast_ref::<StandaloneImplementerExit>()
                    .unwrap()
                    .code,
                CAP_EXIT_CODE
            );
            assert!(
                acquire_slot_in_from(dir.clone(), "STORY-OTHER", 2, 500, probe, observer).is_err()
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), body);
            assert!(refuse_legacy_slots_from(tmp.path(), observer).is_err());
        }
        let visible = Slot {
            pid: 500,
            pid_namespace: Some(22),
            start: None,
            spec: "STORY-2660".into(),
            log: None,
            waiting: false,
        };
        assert_eq!(slot_liveness(&visible, observer, |_, _| false), Some(false));
        assert_eq!(slot_liveness(&visible, observer, |_, _| true), Some(true));
    }

    #[test]
    fn host_capacity_released_directory_entry_is_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let guard = acquire_slot_in(tmp.path().into(), "STORY-2660", 1, 500, |_, _| true).unwrap();
        let entry = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.extension().is_some_and(|e| e == "json"))
            .unwrap();
        drop(guard);
        assert!(read_slot(&entry).unwrap().is_none());
        assert_eq!(
            observe_host_slots(
                tmp.path().into(),
                resolve_host_capacity(None, None, 2).unwrap(),
                |_, _| true
            )
            .unwrap()
            .free,
            1
        );
        assert!(acquire_slot_in(tmp.path().into(), "STORY-2660", 1, 501, |_, _| true).is_ok());
        std::fs::create_dir(&entry).unwrap();
        assert!(read_slot(&entry).is_err(), "other read errors must refuse");
    }

    #[test]
    fn host_capacity_precedence_and_cpu_default() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("host.toml");
        let project: toml::Value = "[queue]\nstandalone_implementer_cap = 4".parse().unwrap();
        let cpu = resolve_host_capacity(Some(&path), None, 20).unwrap();
        assert_eq!((cpu.cap, cpu.cap_source, cpu.nproc), (10, "cpu", 20));
        assert_eq!(resolve_host_capacity(None, None, 1).unwrap().cap, 1);
        let configured = resolve_host_capacity(Some(&path), Some(&project), 20).unwrap();
        assert_eq!((configured.cap, configured.cap_source), (4, "project"));
        std::fs::write(
            &path,
            "[queue]\nstandalone_implementer_cap = 8\n[lane_host]\nallow_ungranted = true",
        )
        .unwrap();
        let host = resolve_host_capacity(Some(&path), Some(&project), 20).unwrap();
        assert_eq!(
            (host.cap, host.cap_source, host.allow_ungranted),
            (8, "host", true)
        );
        let zero: toml::Value = "[queue]\nstandalone_implementer_cap = 0".parse().unwrap();
        assert_eq!(resolve_host_capacity(None, Some(&zero), 6).unwrap().cap, 3);
    }

    #[test]
    fn host_capacity_status_is_read_only_and_matches_admission() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = legacy_slots_dir(tmp.path());
        let capacity = || resolve_host_capacity(None, None, 6).unwrap();
        let empty = observe_host_slots(dir.clone(), capacity(), |_, _| true).unwrap();
        assert_eq!((empty.occupied, empty.free), (0, 3));
        assert!(
            !dir.exists(),
            "observation must not initialize runtime state"
        );
        let alive = |pid, _: Option<&str>| pid == 500;
        let guard = acquire_slot_with(tmp.path(), "STORY-2660", 3, 500, alive).unwrap();
        std::fs::write(
            dir.join("99.json"),
            &format!(
                r#"{{"pid":99,"spec":"STORY-DEAD","pid_namespace":{}}}"#,
                aida_core::pid_observer::current_pid_namespace().unwrap()
            ),
        )
        .unwrap();
        let status = observe_host_slots(dir.clone(), capacity(), alive).unwrap();
        assert_eq!(
            (status.occupied, status.free, status.reservations.len()),
            (1, 2, 2)
        );
        assert!(dir.join("99.json").exists(), "read-only status never reaps");
        let reduced = observe_host_slots(
            dir.clone(),
            resolve_host_capacity(None, None, 1).unwrap(),
            alive,
        )
        .unwrap();
        assert_eq!(reduced.free, 0);
        let held = std::fs::File::open(dir.join(".lock")).unwrap();
        fs2::FileExt::lock_exclusive(&held).unwrap();
        assert!(observe_host_slots(dir.clone(), capacity(), alive).is_err());
        fs2::FileExt::unlock(&held).unwrap();
        std::fs::write(dir.join("broken.json"), "invalid").unwrap();
        assert!(observe_host_slots(dir, capacity(), alive).is_err());
        drop(guard);
    }

    #[test]
    fn host_capacity_shared_slot_lock_enforces_cap_across_callers() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("host-slots");
        let alive = |_: u32, _: Option<&str>| true;
        let first = acquire_slot_in(dir.clone(), "STORY-1", 1, 501, alive).unwrap();
        assert!(acquire_slot_in(dir.clone(), "STORY-2", 1, 502, alive).is_err());
        drop(first);
        let second = acquire_slot_in(dir.clone(), "STORY-2", 1, 502, alive).unwrap();
        std::fs::write(dir.join("unknown.json"), "bad").unwrap();
        assert!(acquire_slot_in(dir, "STORY-3", 8, 503, alive).is_err());
        drop(second);
    }

    #[test]
    fn host_capacity_concurrent_callers_share_admission_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(6));
        let handles: Vec<_> = (0..6)
            .map(|n| {
                let dir = tmp.path().join("shared-host-slots");
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    acquire_slot_in(dir, "STORY-2660", 2, 1000 + n, |_, _| true).ok()
                })
            })
            .collect();
        let guards: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(guards.iter().filter(|g| g.is_some()).count(), 2);
    }

    #[test]
    fn host_capacity_invalid_explicit_config_refuses() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("host.toml");
        for body in [
            "[",
            "queue = 3",
            "[queue]\nstandalone_implementer_cap = 0",
            "[queue]\nstandalone_implementer_cap = -1",
            "[queue]\nstandalone_implementer_cap = '8'",
            "lane_host = false",
            "[lane_host]\nallow_ungranted = 'true'",
        ] {
            std::fs::write(&path, body).unwrap();
            assert!(
                resolve_host_capacity(Some(&path), None, 20).is_err(),
                "{body}"
            );
        }
        assert!(resolve_host_capacity(Some(tmp.path()), None, 20).is_err());
    }
}
