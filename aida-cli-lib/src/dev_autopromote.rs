//! `aida dev build --if-main-green`: promote the live dev binary when
//! `origin/main` has moved past it and CI on that exact commit is green.
//!
//! A scheduled substrate job runs the tick side, which only decides and
//! launches: the build itself takes minutes, longer than a scheduled child
//! may run, so it goes to a detached child (`--autopromote-child <sha>`)
//! that calls the unchanged [`run_dev_build`] and so re-checks every
//! promote gate. Every doubt (a probe, fetch or CI-read error, CI that is
//! not completed and green) is a skip that leaves the checkout and the live
//! binary as they were.
// trace:STORY-1689 | ai:claude

use crate::dev_build::{
    probe_checkout, read_promotion_meta, run_dev_build, BuildSteps, CheckoutState, DevBuildOptions,
    DevLayout, RealSteps,
};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

/// The single-flight lock, beside the live pair.
pub(crate) const LOCK_FILE: &str = ".autopromote.lock";
/// The state of the last launched build, beside the live pair.
pub(crate) const STATE_FILE: &str = "autopromote.json";
/// Bound on `git fetch origin main`.
const FETCH_TIMEOUT: Duration = Duration::from_secs(40);
/// Bound on `git merge --ff-only`.
const FF_TIMEOUT: Duration = Duration::from_secs(20);
/// The whole tick's budget, kept inside the scheduler's per-job kill
/// (`SCHEDULED_COMMAND_TIMEOUT`, 120s) so it is never killed mid-ff.
pub(crate) const TICK_BUDGET: Duration = Duration::from_secs(90);
/// What must remain of the budget to start the fast-forward: its own bound
/// plus the launch (`systemd-run` is bounded at 20s).
pub(crate) const FF_RESERVE: Duration = Duration::from_secs(45);
/// How long the child waits for the tick to release the lock.
const CHILD_LOCK_WAIT: Duration = Duration::from_secs(5);
/// A `running` record with no pid (a unit whose pid was never read) that
/// the child has not claimed within this long is a build that never started.
const UNCLAIMED_GRACE_SECS: i64 = 15 * 60;
/// The longest a build unit may run before systemd stops it.
const BUILD_RUNTIME_MAX_SECS: u64 = 60 * 60;
/// Lines of the build log carried on a reported failure.
const TAIL_LINES: usize = 40;
/// Characters of that tail carried on the one routed line.
const TAIL_CHARS: usize = 3000;

/// Why a tick did not launch a build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Skip {
    /// The live pair was not built from this checkout (or does not say).
    NotLiveSource(String),
    /// Another tick or a launched build holds the lock or is still running.
    InFlight(String),
    Worktree,
    OffMain(String),
    Dirty(String),
    FetchFailed(String),
    /// The live binary is already at origin/main.
    Unchanged,
    /// origin/main is the commit whose build already failed.
    FailedSha(String),
    NonFf(String),
    CiPending(String),
    CiRed(String),
    CiUnknown(String),
    WavePinned(String),
    /// Too little of the tick's budget is left to fast-forward and launch.
    OutOfTime(String),
}

impl Skip {
    /// The stable reason token logged and printed.
    pub(crate) fn reason(&self) -> &'static str {
        match self {
            Skip::NotLiveSource(_) => "not-live-source",
            Skip::InFlight(_) => "in-flight",
            Skip::Worktree => "worktree",
            Skip::OffMain(_) => "off-main",
            Skip::Dirty(_) => "dirty",
            Skip::FetchFailed(_) => "fetch-failed",
            Skip::Unchanged => "unchanged",
            Skip::FailedSha(_) => "failed-sha",
            Skip::NonFf(_) => "non-ff",
            Skip::CiPending(_) => "ci-pending",
            Skip::CiRed(_) => "ci-red",
            Skip::CiUnknown(_) => "ci-unknown",
            Skip::WavePinned(_) => "wave-pinned",
            Skip::OutOfTime(_) => "out-of-time",
        }
    }

    fn detail(&self) -> String {
        match self {
            Skip::Worktree => "this is a linked worktree, not the primary checkout".into(),
            Skip::Unchanged => "the live binary is already at origin/main".into(),
            Skip::NotLiveSource(d)
            | Skip::InFlight(d)
            | Skip::OffMain(d)
            | Skip::Dirty(d)
            | Skip::FetchFailed(d)
            | Skip::FailedSha(d)
            | Skip::NonFf(d)
            | Skip::CiPending(d)
            | Skip::CiRed(d)
            | Skip::CiUnknown(d)
            | Skip::WavePinned(d)
            | Skip::OutOfTime(d) => d.clone(),
        }
    }
}

/// What one tick did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TickOutcome {
    Skipped(Skip),
    /// A build of `sha` was launched (`how` names the unit or pid).
    Launched {
        sha: String,
        how: String,
    },
    /// A failed build is reported now, once; the caller exits non-zero.
    Failure {
        sha: String,
        message: String,
    },
}

/// Where the last launched build stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Phase {
    Running,
    Promoted,
    Failed,
}

/// `<live>/autopromote.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct State {
    pub sha: String,
    pub phase: Phase,
    /// The build process, once known.
    #[serde(default)]
    pub pid: Option<u32>,
    pub log: String,
    /// The failure has been reported (routed) once.
    #[serde(default)]
    pub reported: bool,
    /// RFC 3339 time of the last phase change.
    pub at: String,
    /// Why a build failed, when it did not fail inside the build log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub(crate) fn read_state(live: &Path) -> Option<State> {
    let raw = std::fs::read_to_string(live.join(STATE_FILE)).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Write the state record atomically (temp file, then rename).
pub(crate) fn write_state(live: &Path, state: &State) -> Result<()> {
    std::fs::create_dir_all(live)?;
    let tmp = live.join(format!("{STATE_FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_string_pretty(state)?)
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, live.join(STATE_FILE))
        .with_context(|| format!("writing {}", live.join(STATE_FILE).display()))
}

/// Paths the tick and the child use besides the live dir.
#[derive(Debug, Clone)]
pub(crate) struct Paths {
    /// One line per promote, launch, failure and (changed) skip.
    pub log: PathBuf,
    /// Per-build output logs go here.
    pub build_logs: PathBuf,
}

impl Paths {
    pub(crate) fn default_paths() -> Result<Self> {
        let home = crate::aida_home_dir().context("could not resolve the home directory")?;
        let dir = home.join(".aida");
        Ok(Self {
            log: dir.join("dev-autopromote.log"),
            build_logs: dir.join("dev-autopromote"),
        })
    }
}

/// The side effects of a tick. [`RealAutopromoteSteps`] runs git, the
/// forge and the spawn; tests substitute fakes.
pub(crate) trait AutopromoteSteps {
    fn probe(&mut self, repo: &Path) -> Result<CheckoutState>;
    /// `git fetch origin main`, bounded.
    fn fetch(&mut self, repo: &Path) -> Result<()>;
    /// The full SHA `rev` resolves to.
    fn rev_parse(&mut self, repo: &Path, rev: &str) -> Result<String>;
    /// `git merge-base --is-ancestor ancestor descendant`.
    fn is_ancestor(&mut self, repo: &Path, ancestor: &str, descendant: &str) -> Result<bool>;
    /// The judged CI on `sha`, with the forge's required checks.
    fn commit_ci(&mut self, repo: &Path, sha: &str) -> Result<crate::forge::CommitCi>;
    /// The live-wave guard on `binary`, never waiting.
    fn wave_guard(&mut self, repo: &Path, binary: &Path) -> Result<()>;
    /// `git merge --ff-only sha`.
    fn ff_only(&mut self, repo: &Path, sha: &str) -> Result<()>;
    /// Launch `aida dev build --autopromote-child sha` detached, output to
    /// `log`. Returns the pid when known and a description.
    fn spawn_child(&mut self, repo: &Path, sha: &str, log: &Path) -> Result<(Option<u32>, String)>;
    fn pid_alive(&mut self, pid: u32) -> bool;
}

/// The canonical form of `path` as a string, for comparing checkouts.
fn canonical(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
}

/// The not-live-source precondition: the live pair's record must name this
/// checkout. Read before anything touches the network, so another clone of
/// the repository never fetches or reads CI.
pub(crate) fn live_source_skip(layout: &DevLayout) -> Option<Skip> {
    let here = canonical(&layout.repo);
    match read_promotion_meta(&layout.live_dir) {
        None => Some(Skip::NotLiveSource(format!(
            "{} has no promotion record",
            layout.live_dir.display()
        ))),
        Some(meta) => match meta.repo {
            Some(repo) if repo == here => None,
            Some(repo) => Some(Skip::NotLiveSource(format!(
                "the live binary was built from {repo}, not {here}"
            ))),
            None => Some(Skip::NotLiveSource(
                "the live binary's record does not name its checkout; run `aida dev build` \
                 once by hand from the primary checkout to record it"
                    .into(),
            )),
        },
    }
}

/// PURE: the checkout gate. Primary checkout, on `main`, and nothing
/// modified or untracked (an untracked file is treated as dirty too).
pub(crate) fn checkout_skip(state: &CheckoutState) -> Option<Skip> {
    if state.is_linked_worktree {
        return Some(Skip::Worktree);
    }
    match state.branch.as_deref() {
        Some("main") => {}
        Some(other) => return Some(Skip::OffMain(format!("on branch `{other}`"))),
        None => return Some(Skip::OffMain("HEAD is detached".into())),
    }
    if !state.tracked_changes.is_empty() || state.untracked > 0 {
        return Some(Skip::Dirty(format!(
            "{} modified and {} untracked path(s)",
            state.tracked_changes.len(),
            state.untracked
        )));
    }
    None
}

/// PURE: the CI gate. Only a green verdict (every check completed and
/// passing, required checks present) builds.
pub(crate) fn ci_skip(sha: &str, ci: Result<crate::forge::CommitCi>) -> Option<Skip> {
    use crate::forge::CommitCi;
    let short = &sha[..sha.len().min(12)];
    match ci {
        Ok(CommitCi::Green) => None,
        Ok(CommitCi::Pending(names)) => Some(Skip::CiPending(format!(
            "CI on {short} is still running: {}",
            names.join(", ")
        ))),
        Ok(CommitCi::Failed(names)) => Some(Skip::CiRed(format!(
            "CI on {short} failed: {}",
            names.join(", ")
        ))),
        Ok(CommitCi::NoChecks) => Some(Skip::CiUnknown(format!("no CI checks on {short}"))),
        Ok(CommitCi::MissingRequired(name)) => Some(Skip::CiUnknown(format!(
            "required check `{name}` did not pass on {short}"
        ))),
        Err(e) => Some(Skip::CiUnknown(format!(
            "could not read CI on {short}: {e:#}"
        ))),
    }
}

/// PURE: the checks that must be present and pass on a commit before it is
/// promoted: the project's `[review_watch] required_checks`, the same list
/// review-watch readiness uses. On GitHub the list must name at least one
/// check (the every-lane aggregator), or the commit's CI is unknown; an
/// unreadable setting is unknown too.
pub(crate) fn required_checks(
    kind: crate::forge::ForgeKind,
    configured: Result<Vec<String>>,
) -> Result<Vec<String>> {
    let names = configured.context("could not read `[review_watch] required_checks`")?;
    if kind == crate::forge::ForgeKind::GitHub && names.is_empty() {
        bail!(
            "`[review_watch] required_checks` in .aida/config.toml names no check; on GitHub \
             the promote needs the CI aggregator check named there"
        );
    }
    Ok(names)
}

/// PURE: the CI verdict on a commit, judged with the required checks.
pub(crate) fn judged_ci(
    kind: crate::forge::ForgeKind,
    configured: Result<Vec<String>>,
    checks: Result<crate::forge::CommitChecks>,
) -> Result<crate::forge::CommitCi> {
    let required = required_checks(kind, configured)?;
    let required: Vec<&str> = required.iter().map(String::as_str).collect();
    Ok(crate::forge::judge_commit_checks(&checks?, &required))
}

/// A held lock file; released on drop.
pub(crate) struct Lock(#[allow(dead_code)] std::fs::File);

/// Try `<live>/.autopromote.lock` for up to `wait`. `None` when it is held.
pub(crate) fn try_lock(live: &Path, wait: Duration) -> Result<Option<Lock>> {
    use fs2::FileExt;
    std::fs::create_dir_all(live).with_context(|| format!("creating {}", live.display()))?;
    let path = live.join(LOCK_FILE);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    // A forked child shares the description until it execs; retry briefly
    // as `lock_live_dir` does.
    let deadline = Instant::now() + wait.max(Duration::from_millis(200));
    loop {
        if file.try_lock_exclusive().is_ok() {
            return Ok(Some(Lock(file)));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn now() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now()
}

/// The last `TAIL_LINES` of `log`, joined onto one bounded line (the
/// scheduler routes only the last stderr line of a failed job).
pub(crate) fn log_tail_line(log: &Path) -> String {
    let body = std::fs::read_to_string(log).unwrap_or_default();
    let lines: Vec<&str> = body.lines().filter(|l| !l.trim().is_empty()).collect();
    let tail = lines[lines.len().saturating_sub(TAIL_LINES)..].join(" | ");
    let chars: Vec<char> = tail.chars().collect();
    if chars.len() > TAIL_CHARS {
        let kept: String = chars[chars.len() - TAIL_CHARS..].iter().collect();
        format!("...{kept}")
    } else if tail.is_empty() {
        "(the build log is empty or unreadable)".into()
    } else {
        tail
    }
}

/// The once-only failure report for `state`, or why a `running` record is
/// still in flight. Marks a reported failure `reported`.
fn settle_previous(
    live: &Path,
    steps: &mut dyn AutopromoteSteps,
    at: chrono::DateTime<chrono::Utc>,
) -> Result<Option<TickOutcome>> {
    let Some(mut state) = read_state(live) else {
        return Ok(None);
    };
    if state.phase == Phase::Running {
        let started = chrono::DateTime::parse_from_rfc3339(&state.at)
            .map(|t| t.with_timezone(&chrono::Utc))
            .unwrap_or(at);
        let dead = match state.pid {
            Some(pid) => !steps.pid_alive(pid),
            None => (at - started).num_seconds() > UNCLAIMED_GRACE_SECS,
        };
        if !dead {
            return Ok(Some(TickOutcome::Skipped(Skip::InFlight(format!(
                "the build of {} is still running",
                state.sha
            )))));
        }
        state.phase = Phase::Failed;
        state.reported = false;
        state.error = Some(match state.pid {
            Some(pid) => format!("the build process {pid} ended without recording a result"),
            None => "the launched build never started".into(),
        });
    }
    if state.phase != Phase::Failed || state.reported {
        return Ok(None);
    }
    state.reported = true;
    state.at = at.to_rfc3339();
    write_state(live, &state)?;
    let why = state
        .error
        .clone()
        .unwrap_or_else(|| "the build or its smoke test failed".into());
    // The scheduler routes only the last stderr line, so that line carries
    // the SHA, the log path and the tail.
    let message = format!(
        "the scheduled live-binary build of {} failed ({why}); the live binary was not changed. \
         It is not retried until origin/main moves.\nbuild of {} failed; log {}; tail: {}",
        state.sha,
        state.sha,
        state.log,
        log_tail_line(Path::new(&state.log))
    );
    Ok(Some(TickOutcome::Failure {
        sha: state.sha,
        message,
    }))
}

/// One scheduled tick. Never builds: it decides, fast-forwards and
/// launches the detached child. `deadline` ends the tick's budget; the
/// fast-forward starts only with [`FF_RESERVE`] of it left.
pub(crate) fn run_tick(
    layout: &DevLayout,
    paths: &Paths,
    steps: &mut dyn AutopromoteSteps,
    deadline: Instant,
) -> Result<TickOutcome> {
    let repo = &layout.repo;
    let live = &layout.live_dir;
    if let Some(skip) = live_source_skip(layout) {
        return Ok(TickOutcome::Skipped(skip));
    }
    // The lock is held for every check, the fast-forward and the launch.
    let Some(lock) = try_lock(live, Duration::ZERO)? else {
        return Ok(TickOutcome::Skipped(Skip::InFlight(
            "another tick or build holds the autopromote lock".into(),
        )));
    };
    let at = now();
    if let Some(outcome) = settle_previous(live, steps, at)? {
        return Ok(outcome);
    }
    let skipped = |s: Skip| Ok(TickOutcome::Skipped(s));
    let checkout = match steps.probe(repo) {
        Ok(c) => c,
        Err(e) => return skipped(Skip::Dirty(format!("could not read the checkout: {e:#}"))),
    };
    if let Some(skip) = checkout_skip(&checkout) {
        return skipped(skip);
    }
    if let Err(e) = steps.fetch(repo) {
        return skipped(Skip::FetchFailed(format!("{e:#}")));
    }
    let target = match steps.rev_parse(repo, "refs/remotes/origin/main") {
        Ok(sha) => sha,
        Err(e) => return skipped(Skip::FetchFailed(format!("no origin/main: {e:#}"))),
    };
    let live_sha = read_promotion_meta(live).map(|m| m.sha).unwrap_or_default();
    if !live_sha.is_empty() && live_sha == target {
        return skipped(Skip::Unchanged);
    }
    if let Some(prev) = read_state(live) {
        if prev.phase == Phase::Failed && prev.sha == target {
            return skipped(Skip::FailedSha(format!(
                "the build of {target} already failed; waiting for origin/main to move"
            )));
        }
    }
    let head = match steps.rev_parse(repo, "HEAD") {
        Ok(sha) => sha,
        Err(e) => return skipped(Skip::NonFf(format!("could not read HEAD: {e:#}"))),
    };
    match steps.is_ancestor(repo, &head, &target) {
        Ok(true) => {}
        Ok(false) => {
            return skipped(Skip::NonFf(format!(
                "HEAD {head} is not an ancestor of origin/main {target}"
            )))
        }
        Err(e) => return skipped(Skip::NonFf(format!("{e:#}"))),
    }
    if let Some(skip) = ci_skip(&target, steps.commit_ci(repo, &target)) {
        return skipped(skip);
    }
    if let Err(e) = steps.wave_guard(repo, &layout.live("aida")) {
        return skipped(Skip::WavePinned(format!("{e:#}")));
    }
    // C1: re-probe under the same lock right before touching the checkout.
    match steps.probe(repo) {
        Ok(again) => {
            if let Some(skip) = checkout_skip(&again) {
                return skipped(skip);
            }
        }
        Err(e) => return skipped(Skip::Dirty(format!("could not read the checkout: {e:#}"))),
    }
    let left = deadline.saturating_duration_since(Instant::now());
    if left < FF_RESERVE {
        return skipped(Skip::OutOfTime(format!(
            "{}s of the tick's budget left, {}s needed to fast-forward and launch; \
             retrying next tick",
            left.as_secs(),
            FF_RESERVE.as_secs()
        )));
    }
    if let Err(e) = steps.ff_only(repo, &target) {
        return skipped(Skip::NonFf(format!("fast-forward refused: {e:#}")));
    }
    let log = paths.build_logs.join(format!(
        "{}-{}.log",
        at.format("%Y%m%dT%H%M%SZ"),
        &target[..target.len().min(12)]
    ));
    let mut state = State {
        sha: target.clone(),
        phase: Phase::Running,
        pid: None,
        log: log.display().to_string(),
        reported: false,
        at: at.to_rfc3339(),
        error: None,
    };
    write_state(live, &state)?;
    match steps.spawn_child(repo, &target, &log) {
        Ok((pid, how)) => {
            if pid.is_some() {
                // The child may already have claimed the record; only fill
                // in a pid it has not written yet.
                if read_state(live).is_some_and(|s| s.phase == Phase::Running && s.pid.is_none()) {
                    state.pid = pid;
                    write_state(live, &state)?;
                }
            }
            drop(lock);
            Ok(TickOutcome::Launched { sha: target, how })
        }
        Err(e) => {
            state.phase = Phase::Failed;
            state.error = Some(format!("the build could not be launched: {e:#}"));
            write_state(live, &state)?;
            // Report it now rather than on the next tick.
            settle_previous(live, steps, at)
                .map(|o| o.unwrap_or(TickOutcome::Skipped(Skip::FailedSha(target))))
        }
    }
}

/// The detached child: build and promote `sha` through the unchanged
/// pipeline, recording the result for the next tick.
pub(crate) fn run_child(
    layout: &DevLayout,
    sha: &str,
    pid: u32,
    build_steps: &mut dyn BuildSteps,
    out: &mut dyn Write,
) -> Result<()> {
    let live = &layout.live_dir;
    let lock = try_lock(live, CHILD_LOCK_WAIT)?
        .context("the autopromote lock is held; another build is running")?;
    let mut state = read_state(live)
        .filter(|s| s.sha == sha && s.phase == Phase::Running)
        .with_context(|| {
            format!(
                "no launched build of {sha} is recorded in {}; nothing was built",
                live.join(STATE_FILE).display()
            )
        })?;
    state.pid = Some(pid);
    write_state(live, &state)?;
    let result = (|| -> Result<()> {
        let head = build_steps.probe(&layout.repo)?.head.unwrap_or_default();
        if head != sha {
            bail!("HEAD moved to {head} before the build of {sha} started");
        }
        run_dev_build(layout, &DevBuildOptions::default(), build_steps, out)?;
        Ok(())
    })();
    state.at = now().to_rfc3339();
    match &result {
        Ok(()) => {
            state.phase = Phase::Promoted;
            state.error = None;
        }
        Err(e) => {
            state.phase = Phase::Failed;
            state.reported = false;
            state.error = Some(format!("{e:#}").lines().next().unwrap_or("").to_string());
        }
    }
    write_state(live, &state)?;
    drop(lock);
    result
}

/// Append `line` to the autopromote log. A skip is written only when its
/// reason differs from the last line's; `unchanged` never is.
pub(crate) fn log_outcome(log: &Path, outcome: &TickOutcome) -> Result<()> {
    let (token, line) = match outcome {
        TickOutcome::Skipped(Skip::Unchanged) => return Ok(()),
        TickOutcome::Skipped(skip) => (
            format!("skip {}", skip.reason()),
            format!("skip {}: {}", skip.reason(), skip.detail()),
        ),
        TickOutcome::Launched { sha, how } => ("launch".into(), format!("launch {sha}: {how}")),
        TickOutcome::Failure { sha, .. } => ("failed".into(), format!("failed {sha}")),
    };
    if matches!(outcome, TickOutcome::Skipped(_)) {
        let last = std::fs::read_to_string(log)
            .ok()
            .and_then(|b| b.lines().last().map(str::to_string))
            .unwrap_or_default();
        if last
            .split_once(' ')
            .is_some_and(|(_, rest)| rest.starts_with(&format!("{token}:")))
        {
            return Ok(());
        }
    }
    append_line(log, &line)
}

fn append_line(log: &Path, line: &str) -> Result<()> {
    if let Some(parent) = log.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .with_context(|| format!("opening {}", log.display()))?;
    writeln!(file, "{} {line}", now().to_rfc3339())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Production steps
// ---------------------------------------------------------------------------

/// The production [`AutopromoteSteps`].
pub(crate) struct RealAutopromoteSteps;

fn git(repo: &Path) -> std::process::Command {
    let mut cmd = aida_core::git_ops::host_git_command(repo);
    cmd.env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null());
    cmd
}

fn git_ok(cmd: std::process::Command, what: &str, timeout: Duration) -> Result<String> {
    let out = crate::command_output_with_timeout(cmd, timeout)
        .with_context(|| format!("`{what}` timed out or could not start"))?;
    if !out.status.success() {
        bail!(
            "`{what}` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

impl AutopromoteSteps for RealAutopromoteSteps {
    fn probe(&mut self, repo: &Path) -> Result<CheckoutState> {
        probe_checkout(repo)
    }

    fn fetch(&mut self, repo: &Path) -> Result<()> {
        let mut cmd = git(repo);
        cmd.args(["fetch", "--quiet", "origin", "main"]);
        git_ok(cmd, "git fetch origin main", FETCH_TIMEOUT).map(|_| ())
    }

    fn rev_parse(&mut self, repo: &Path, rev: &str) -> Result<String> {
        let mut cmd = git(repo);
        cmd.args(["rev-parse", "--verify", "-q", &format!("{rev}^{{commit}}")]);
        let sha = git_ok(cmd, "git rev-parse", Duration::from_secs(10))?;
        anyhow::ensure!(
            crate::git_arg_guard::is_hex_sha(&sha),
            "`{rev}` did not resolve to a commit"
        );
        Ok(sha)
    }

    fn is_ancestor(&mut self, repo: &Path, ancestor: &str, descendant: &str) -> Result<bool> {
        let mut cmd = git(repo);
        cmd.args(["merge-base", "--is-ancestor", ancestor, descendant]);
        let out = crate::command_output_with_timeout(cmd, Duration::from_secs(30))
            .context("`git merge-base --is-ancestor` timed out")?;
        match out.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => bail!(
                "`git merge-base --is-ancestor` failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        }
    }

    fn commit_ci(&mut self, repo: &Path, sha: &str) -> Result<crate::forge::CommitCi> {
        let forge = crate::forge::forge_for(repo);
        judged_ci(
            forge.kind(),
            crate::review_watch::required_checks(repo),
            forge.commit_checks(sha),
        )
    }

    fn wave_guard(&mut self, repo: &Path, binary: &Path) -> Result<()> {
        crate::dev_cmd::guard_binary_change(repo, binary, false)
    }

    fn ff_only(&mut self, repo: &Path, sha: &str) -> Result<()> {
        let mut cmd = git(repo);
        cmd.args(["merge", "--ff-only", "--quiet", sha]);
        git_ok(cmd, "git merge --ff-only", FF_TIMEOUT).map(|_| ())
    }

    fn spawn_child(&mut self, repo: &Path, sha: &str, log: &Path) -> Result<(Option<u32>, String)> {
        spawn_child_real(repo, sha, log)
    }

    fn pid_alive(&mut self, pid: u32) -> bool {
        aida_core::liveness::pid_is_alive(pid)
    }
}

/// The child argv after the executable.
pub(crate) fn child_argv(sha: &str) -> Vec<String> {
    ["dev", "build", "--autopromote-child", sha]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// Launch the child in its own transient `systemd-run --user` unit when this
/// tick runs under the systemd driver (so the tick unit's stop and timeout
/// never reach it), else detached in a new session.
fn spawn_child_real(repo: &Path, sha: &str, log: &Path) -> Result<(Option<u32>, String)> {
    use crate::schedule_driver::{
        build_wave_unit_argv, classify_systemd_run, expands_in_unit, repo_hash8, DriverHost,
        RealDriverHost, RunFailure, WaveLimits, WaveUnitSpec, WAVE_UNIT_RUN_TIMEOUT,
    };
    if let Some(parent) = log.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let exe = crate::aida_exe_path();
    let argv = child_argv(sha);
    let repo_s = repo.display().to_string();
    let exe_s = exe.display().to_string();
    let log_s = log.display().to_string();
    let path_env = std::env::var("PATH").unwrap_or_default();
    let mut host = RealDriverHost;
    let under_systemd = std::env::var("AIDA_SCHEDULE_INVOKER").ok().as_deref() == Some("systemd");
    let mut fallback = None;
    if under_systemd
        && host.systemd_supported()
        && ![&repo_s, &exe_s, &log_s, &path_env]
            .iter()
            .any(|v| expands_in_unit(v))
    {
        let unit = format!(
            "aida-dev-autopromote-{}-{}.service",
            repo_hash8(&repo_s),
            now().format("%Y%m%d-%H%M%S")
        );
        let set_env = crate::shift::wave_unit_set_env(&path_env);
        let unset_env = crate::shift::wave_unit_unset_env();
        let limits = WaveLimits::default();
        let args = build_wave_unit_argv(&WaveUnitSpec {
            unit: &unit,
            repo: &repo_s,
            exe: &exe_s,
            log: &log_s,
            description: &format!("AIDA live-binary build of {sha}"),
            runtime_max_secs: BUILD_RUNTIME_MAX_SECS,
            limits: &limits,
            set_env: &set_env,
            unset_env: &unset_env,
            argv: &argv,
        });
        match classify_systemd_run(&host.systemd_run_user(&args, WAVE_UNIT_RUN_TIMEOUT)) {
            Ok(()) => return Ok((None, format!("unit {unit}"))),
            Err(RunFailure::NotStarted(why)) => fallback = Some(why),
            // It may have started: never launch a second copy. The child
            // claims the record; an unclaimed one expires as a failure.
            Err(RunFailure::Ambiguous(why) | RunFailure::Exists(why)) => {
                return Ok((None, format!("unit {unit} (start unconfirmed: {why})")))
            }
        }
    }
    let mut cmd = crate::shift::wave_command(&exe, &argv, repo, log)?;
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    let child = cmd.spawn().context("could not launch the build")?;
    let how = match fallback {
        Some(why) => format!("detached pid {} ({why})", child.id()),
        None => format!("detached pid {}", child.id()),
    };
    Ok((Some(child.id()), how))
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `aida dev build --if-main-green`.
pub(crate) fn handle_if_main_green() -> Result<()> {
    let repo = crate::dev_build::resolve_build_repo()?;
    let layout = DevLayout::for_repo(&repo)?;
    let paths = Paths::default_paths()?;
    let deadline = Instant::now() + TICK_BUDGET;
    let outcome = run_tick(&layout, &paths, &mut RealAutopromoteSteps, deadline)?;
    if let Err(e) = log_outcome(&paths.log, &outcome) {
        eprintln!("warning: could not write {}: {e:#}", paths.log.display());
    }
    match outcome {
        TickOutcome::Skipped(Skip::Unchanged) => Ok(()),
        TickOutcome::Skipped(skip) => {
            println!("skip {}: {}", skip.reason(), skip.detail());
            Ok(())
        }
        TickOutcome::Launched { sha, how } => {
            println!("launched the build of {sha} ({how})");
            Ok(())
        }
        TickOutcome::Failure { message, .. } => bail!("{message}"),
    }
}

/// `aida dev build --autopromote-child <sha>` (hidden; launched by a tick).
pub(crate) fn handle_child(sha: &str) -> Result<()> {
    anyhow::ensure!(
        crate::git_arg_guard::is_hex_sha(sha),
        "--autopromote-child needs a commit SHA"
    );
    let repo = crate::dev_build::resolve_build_repo()?;
    let layout = DevLayout::for_repo(&repo)?;
    let paths = Paths::default_paths()?;
    let mut stdout = std::io::stdout();
    let result = run_child(
        &layout,
        sha,
        std::process::id(),
        &mut RealSteps,
        &mut stdout,
    );
    let line = match &result {
        Ok(()) => format!("promoted {sha}"),
        Err(e) => format!(
            "build-failed {sha}: {}",
            format!("{e:#}").lines().next().unwrap_or("")
        ),
    };
    if let Err(e) = append_line(&paths.log, &line) {
        eprintln!("warning: could not write {}: {e:#}", paths.log.display());
    }
    result
}

#[cfg(test)]
#[path = "tests/story_1689_dev_autopromote_tests.rs"]
mod tests;
