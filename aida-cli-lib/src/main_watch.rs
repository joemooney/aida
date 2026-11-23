//! The red-main watch (STORY-2897 S3a): an OBSERVATIONAL watch of the
//! default branch's CI. It judges each merged commit, attributes a red main
//! to its first red commit, observes independent re-runs to tell a flake
//! from a real red, mails the advisor role, recommends a revert on the fixed
//! triggers, and records red-main episodes for `aida land metrics`. It
//! writes nothing to the requirement store: filing fix specs and `aida human`
//! items is S3b (advisor ruling 2026-11-20 17:00).
//!
//! It runs only as its own scheduled job (`land watch`) or by hand; no merge
//! or landing path calls it, so nothing that merges can stall on it, and
//! merging continues while main is red.
//!
//! Scan: commits are judged in first-parent order from a durable cursor, a
//! bounded batch per pass, and the cursor advances only past commits with a
//! final verdict. A commit whose CI is still running holds the commits after
//! it, so the first red is always attributed in order and no red between two
//! passes is skipped.
//!
//! Durability: every transition is written to the state file first, with a
//! transition id, then appended to the journal; a pass replays transitions
//! the journal lacks. Only newline-terminated journal lines are committed.
//! Mail waits in a persistent outbox until delivered. State, journal and
//! lock live under `.aida/land`, which must be a real directory; no file
//! there is followed through a link, and the state is replaced through an
//! exclusively created temporary file.
//!
//! State: `.aida/land/main-watch.json`; journal: `.aida/land/main-watch.jsonl`.
// trace:STORY-2897 | ai:claude
// trace:STORY-2897.acaf82b5 | ai:claude

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::forge::{CheckOutcome, CommitChecks};

const STATE_FILE: &str = "main-watch.json";
const JOURNAL_FILE: &str = "main-watch.jsonl";
const LOCK_FILE: &str = "main-watch.lock";
/// The watch's state, relative to the project root.
pub(crate) const STATE: &str = ".aida/land/main-watch.json";
/// The watch's journal, relative to the project root.
pub(crate) const JOURNAL: &str = ".aida/land/main-watch.jsonl";
/// How many commits one pass judges at most; the rest wait for the next.
pub(crate) const BATCH: usize = 50;
/// A red not cleared by its re-run is confirmed after this long anyway.
pub(crate) const CONFIRM_AFTER_MINUTES: i64 = 30;
/// A commit whose required CI never reported is passed over after this long.
pub(crate) const NO_CI_GRACE_HOURS: i64 = 2;
/// CI steps that compile: a red failing in one of these is unbuildable.
pub(crate) const COMPILE_STEPS: &[&str] = &["Build ", "Check aida-cli"];
/// The ceiling on one git call the live watch makes.
pub(crate) const CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// Leave the scheduler 30 seconds for checkpointing and process cleanup.
// trace:TASK-3209 | ai:codex
pub(crate) const PASS_BUDGET: std::time::Duration = std::time::Duration::from_secs(90);

/// One default-branch commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Commit {
    pub sha: String,
    pub subject: String,
    pub body: String,
    pub at: DateTime<Utc>,
}

/// What one failed CI job shows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Evidence {
    #[serde(default)]
    pub job_id: Option<u64>,
    pub workflow: Option<String>,
    pub run_id: String,
    pub attempt: u64,
    pub check: String,
    pub link: String,
    pub failed_steps: Vec<String>,
    pub excerpt: String,
    /// The job log was read; `false` keeps the link and steps and retries
    /// the log on a later pass.
    pub log_complete: bool,
}

/// What a forge job read returns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct JobReport {
    pub link: String,
    pub failed_steps: Vec<String>,
    pub excerpt: String,
    pub log_complete: bool,
}

/// What the watch reads and does outside its own state files. [`Live`] is
/// the real one; tests use a fake.
pub(crate) trait Ports {
    fn now(&self) -> DateTime<Utc>;
    /// The freshly fetched default branch's newest first-parent commits.
    fn recent(&mut self, max: usize) -> Result<Vec<Commit>>;
    /// First-parent commits after `cursor`, oldest first, at most `max`;
    /// `Ok(None)` when `cursor` is no longer on the default branch.
    fn after(&mut self, cursor: &str, max: usize) -> Result<Option<Vec<Commit>>>;
    /// First-parent commits in `(from, to]`.
    fn range(&mut self, from: &str, to: &str) -> Result<Vec<Commit>>;
    fn checks(&mut self, sha: &str) -> Result<CommitChecks>;
    fn job(&mut self, run_id: &str, job: &str, id: u64, attempt: u64) -> Result<JobReport>;
    /// Urgent mail to the advisor role; idempotent by `key`.
    fn mail(&mut self, key: &str, subject: &str, body: &str) -> Result<()>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FailedCheck {
    #[serde(default)]
    pub job_id: Option<u64>,
    pub name: String,
    pub workflow: Option<String>,
    pub run_id: Option<String>,
    pub attempt: Option<u64>,
}

impl FailedCheck {
    fn matches_evidence(&self, e: &Evidence) -> bool {
        self.workflow == e.workflow
            && self.run_id.as_ref() == Some(&e.run_id)
            && self.name == e.check
            && self.attempt == Some(e.attempt)
            && self.job_id.is_some()
            && self.job_id == e.job_id
    }

    fn key(&self) -> Option<String> {
        Some(format!(
            "{}:{}:{}:{}:{}",
            self.workflow.as_deref().unwrap_or("-"),
            self.run_id.as_deref()?,
            self.attempt?,
            self.job_id?,
            self.name
        ))
    }
}

/// The watch's reading of one commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Every required check passed. Other failures are diagnostics only.
    Green,
    Red {
        /// The required checks that failed.
        required: Vec<FailedCheck>,
        /// Other failed jobs of the same runs (what the required check
        /// aggregates): read for evidence and the compile trigger.
        contributing: Vec<FailedCheck>,
        /// Failed checks of other runs: shown, never acted on.
        diagnostics: Vec<String>,
    },
    /// No required result will come (no CI ran, or only cancelled runs,
    /// past the grace period): passed over, never a transition.
    Skipped(String),
    /// Not final yet (running, re-running, unreadable): holds the scan.
    Pending(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct State {
    /// The newest commit with a final verdict; the next pass starts after it.
    pub cursor: Option<String>,
    /// Inclusive starting point until the first candidate has a final verdict.
    #[serde(default)]
    first_candidate: Option<Commit>,
    pub last_green: Option<String>,
    /// The tip seen by the last pass and its CI, reported apart from the
    /// last green commit.
    pub tip: Option<(String, String)>,
    pub episode: Option<Episode>,
    /// Mail not yet delivered; it outlives the episode.
    #[serde(default)]
    pub outbox: Vec<Notice>,
    /// Transitions written here first and not yet in the journal.
    #[serde(default)]
    pub pending: Vec<Event>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Episode {
    pub red_sha: String,
    pub subject: String,
    pub pr: Option<u64>,
    pub specs: Vec<String>,
    pub failed: Vec<FailedCheck>,
    pub contributing: Vec<FailedCheck>,
    pub diagnostics: Vec<String>,
    #[serde(default)]
    pub source_at: Option<DateTime<Utc>>,
    pub opened_at: DateTime<Utc>,
    /// The newest green commit before the red: the live binary stays there.
    pub held_green: Option<String>,
    /// Required check run id -> its attempt when the episode opened.
    pub attempts: BTreeMap<String, u64>,
    pub confirmed_at: Option<DateTime<Utc>>,
    pub evidence: Vec<Evidence>,
    /// Keys (workflow:run:attempt:job) whose evidence, log included, is read.
    pub evidence_keys: BTreeSet<String>,
    pub alerted: bool,
    /// Every later commit that landed while the episode was open.
    pub red_merges: Vec<String>,
    /// Revert reasons already mailed (one mail per reason).
    pub recommended: Vec<String>,
}

/// One mail waiting in the outbox.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Notice {
    pub key: String,
    pub title: String,
    pub body: String,
}

/// One journal line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Event {
    /// The transition id; the journal holds each id at most once.
    #[serde(default)]
    pub id: String,
    pub at: DateTime<Utc>,
    /// `red_start`, `red_merge`, `confirmed`, `revert_recommended` or `red_end`.
    pub event: String,
    pub red_sha: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
    /// `red_end`: `flake`, `cleared`, `recovered`, `reverted` or
    /// `unattributed` (the closing commits could not be read).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

fn short(sha: &str) -> &str {
    sha.get(..12).unwrap_or(sha)
}

/// PURE: the `(#N)` a squash subject ends with.
pub(crate) fn subject_pr(subject: &str) -> Option<u64> {
    let tail = subject.trim_end().strip_suffix(')')?;
    let (_, number) = tail.rsplit_once("(#")?;
    number.parse().ok()
}

/// PURE: the median, averaging the two central values of an even count.
pub(crate) fn median(sorted: &[i64]) -> Option<i64> {
    match sorted.len() {
        0 => None,
        n if n % 2 == 1 => Some(sorted[n / 2]),
        n => Some((sorted[n / 2 - 1] + sorted[n / 2]) / 2),
    }
}

/// PURE: judge a default-branch commit from its required CI. `required` is
/// the CI-gated set; `merge-hold-gate` runs on pull requests only and is
/// not required of a commit on main.
pub(crate) fn judge(
    checks: Result<CommitChecks>,
    required: &[String],
    committed_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Verdict {
    let checks = match checks {
        Ok(checks) => checks,
        Err(e) => return Verdict::Pending(format!("CI could not be read: {e:#}")),
    };
    let latest = crate::forge::latest_per_name(&checks.checks);
    let required: Vec<&str> = required
        .iter()
        .map(String::as_str)
        .filter(|n| *n != crate::landing_policy::MERGE_HOLD_GATE)
        .collect();
    let failed = |c: &crate::forge::CommitCheck| {
        c.outcome == CheckOutcome::Failed && !c.cancelled && !c.run_in_progress
    };
    let row = |c: &crate::forge::CommitCheck| FailedCheck {
        job_id: c.job_id,
        name: c.name.clone(),
        workflow: c.workflow_id.clone(),
        run_id: c.run_id.clone(),
        attempt: c.run_attempt,
    };
    let is_required = |c: &crate::forge::CommitCheck| required.contains(&c.name.as_str());
    let rerunning: Vec<&str> = latest
        .iter()
        .filter(|c| is_required(c) && c.run_in_progress)
        .map(|c| c.name.as_str())
        .collect();
    if !rerunning.is_empty() {
        return Verdict::Pending(format!(
            "a re-run of {} is in progress",
            rerunning.join(", ")
        ));
    }
    let failed_required: Vec<FailedCheck> = latest
        .iter()
        .filter(|c| is_required(c) && failed(c))
        .map(|c| row(c))
        .collect();
    if !failed_required.is_empty() {
        let runs: BTreeSet<&str> = failed_required
            .iter()
            .filter_map(|f| f.run_id.as_deref())
            .collect();
        let (contributing, other): (Vec<_>, Vec<_>) = latest
            .iter()
            .filter(|c| !is_required(c) && failed(c))
            .partition(|c| {
                c.run_id.as_deref().is_some_and(|r| runs.contains(r))
                    && failed_required.iter().any(|f| {
                        f.name == crate::forge::CI_AGGREGATOR_CHECK
                            && f.run_id == c.run_id
                            && f.workflow == c.workflow_id
                    })
                    && matches!(
                        c.name.as_str(),
                        "CI (lib-0)"
                            | "CI (lib-1)"
                            | "CI (cli-0)"
                            | "CI (cli-1)"
                            | "CI (rest)"
                            | "CI (gates)"
                            | "CI (contracts)"
                            | "CI (fast gates)"
                            | "CI (fast compile)"
                            | "ci-tier: fast"
                            | "ci-tier: full"
                    )
            });
        return Verdict::Red {
            required: failed_required,
            contributing: contributing.into_iter().map(|c| row(c)).collect(),
            diagnostics: other.into_iter().map(|c| c.name.clone()).collect(),
        };
    }
    let passed = |name: &str| {
        latest
            .iter()
            .any(|c| c.name == name && c.outcome == CheckOutcome::Passed)
    };
    if !required.is_empty() && required.iter().all(|n| passed(n)) {
        return Verdict::Green;
    }
    let pending: Vec<&str> = latest
        .iter()
        .filter(|c| is_required(c) && c.outcome == CheckOutcome::Pending)
        .map(|c| c.name.as_str())
        .collect();
    if !pending.is_empty() {
        return Verdict::Pending(format!("CI is still running: {}", pending.join(", ")));
    }
    // No required result (none reported, skipped or only cancelled): wait a
    // while for one to appear, then pass the commit over.
    let why = if required.is_empty() {
        "no CI-gated checks are configured".to_string()
    } else {
        format!("no result for {}", required.join(", "))
    };
    if now - committed_at >= Duration::hours(NO_CI_GRACE_HOURS) {
        Verdict::Skipped(why)
    } else {
        Verdict::Pending(why)
    }
}

// Fault injection at each durability boundary (tests only).
#[cfg(test)]
thread_local! {
    pub(crate) static FAIL_AT: std::cell::RefCell<Option<&'static str>> = const { std::cell::RefCell::new(None) };
}
#[cfg(test)]
pub(crate) fn failpoint(name: &str) -> Result<()> {
    FAIL_AT.with(|f| {
        if *f.borrow() == Some(name) {
            anyhow::bail!("injected failure at {name}")
        }
        Ok(())
    })
}
#[cfg(not(test))]
pub(crate) fn failpoint(_name: &str) -> Result<()> {
    Ok(())
}

struct PinnedDir {
    _handle: std::fs::File,
    path: PathBuf,
}
impl std::ops::Deref for PinnedDir {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.path
    }
}

fn pin_dir(path: &Path) -> std::io::Result<PinnedDir> {
    use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)?;
    Ok(PinnedDir {
        path: PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd())),
        _handle: file,
    })
}

fn watch_directory(root: &Path, create: bool) -> std::io::Result<PinnedDir> {
    let mut dir = pin_dir(root)?;
    for name in [".aida", "land"] {
        let child = dir.join(name);
        if create {
            match std::fs::create_dir(&child) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
                Err(e) => return Err(e),
            }
        }
        dir = pin_dir(&child)?;
    }
    Ok(dir)
}

fn land_dir(root: &Path) -> Result<PinnedDir> {
    Ok(watch_directory(root, true)?)
}

const WATCH_FILE_LIMIT: u64 = 64 * 1024 * 1024;

fn read_bounded(file: &mut std::fs::File) -> Result<Vec<u8>> {
    anyhow::ensure!(
        file.metadata()?.len() <= WATCH_FILE_LIMIT,
        "watch file exceeds 64 MiB"
    );
    let mut bytes = Vec::new();
    file.take(WATCH_FILE_LIMIT + 1).read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= WATCH_FILE_LIMIT,
        "watch file exceeds 64 MiB"
    );
    Ok(bytes)
}

/// Open `path` without following a link, and require a regular file.
fn open_plain(path: &Path, options: &mut std::fs::OpenOptions) -> Result<std::fs::File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    anyhow::ensure!(
        file.metadata()?.is_file(),
        "{} is not a regular file",
        path.display()
    );
    use std::os::unix::fs::MetadataExt;
    anyhow::ensure!(
        file.metadata()?.nlink() == 1,
        "{} has multiple links",
        path.display()
    );
    Ok(file)
}

fn read_state(land: &Path) -> Result<State> {
    let path = land.join(STATE_FILE);
    if matches!(std::fs::symlink_metadata(&path), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
    {
        return Ok(State::default());
    }
    let mut file = open_plain(&path, std::fs::OpenOptions::new().read(true))?;
    let bytes = read_bounded(&mut file)?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("{STATE} is not valid; fix or remove it"))
}

fn write_state(land: &Path, state: &State) -> Result<()> {
    failpoint("state")?;
    let bytes = serde_json::to_vec_pretty(state)?;
    anyhow::ensure!(
        bytes.len() as u64 <= WATCH_FILE_LIMIT,
        "watch state exceeds 64 MiB"
    );
    let temp = land.join(format!(
        "{STATE_FILE}.{}.{}.tmp",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let mut file = open_plain(
        &temp,
        std::fs::OpenOptions::new().write(true).create_new(true),
    )?;
    file.write_all(&bytes)?;
    file.sync_data()?;
    drop(file);
    // rename replaces whatever is at the target; it never follows a link.
    std::fs::rename(&temp, land.join(STATE_FILE)).with_context(|| format!("writing {STATE}"))?;
    std::fs::File::open(land)?.sync_all()?;
    Ok(())
}

fn append(land: &Path, event: &Event) -> Result<()> {
    failpoint("journal")?;
    let mut file = open_plain(
        &land.join(JOURNAL_FILE),
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true),
    )?;
    // An unterminated tail is an append a crash cut short: cut it, and only
    // it, before appending.
    let len = file.metadata()?.len();
    anyhow::ensure!(len <= WATCH_FILE_LIMIT, "watch journal exceeds 64 MiB");
    let mut committed = len;
    if len > 0 {
        let mut last = [0u8; 1];
        file.seek(SeekFrom::Start(len - 1))?;
        file.read_exact(&mut last)?;
        if last[0] != b'\n' {
            file.seek(SeekFrom::Start(0))?;
            let bytes = read_bounded(&mut file)?;
            let keep = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |p| p + 1);
            committed = keep as u64;
        }
    }

    let mut line = serde_json::to_vec(event)?;
    line.push(b'\n');
    anyhow::ensure!(
        committed + line.len() as u64 <= WATCH_FILE_LIMIT,
        "watch journal exceeds 64 MiB"
    );
    if committed != len {
        file.set_len(committed)?;
    }
    file.seek(SeekFrom::Start(committed))?;
    file.write_all(&line)?;
    file.sync_data()?;
    Ok(())
}

/// Every committed journal event, oldest first, each transition id once.
/// Only newline-terminated lines are committed; the boundary is found in
/// bytes, so a crash inside a multibyte character never blocks a read.
pub(crate) fn read_journal(root: &Path) -> Result<Vec<Event>> {
    match watch_directory(root, false) {
        Ok(dir) => journal_in(&dir),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
}

fn journal_in(land: &Path) -> Result<Vec<Event>> {
    let path = land.join(JOURNAL_FILE);
    if matches!(std::fs::symlink_metadata(&path), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
    {
        return Ok(Vec::new());
    }
    let bytes = read_bounded(&mut open_plain(
        &path,
        std::fs::OpenOptions::new().read(true),
    )?)?;
    let committed = bytes
        .iter()
        .rposition(|b| *b == b'\n')
        .map_or(&bytes[..0], |end| &bytes[..=end]);
    let text = std::str::from_utf8(committed).with_context(|| format!("{JOURNAL} is not UTF-8"))?;
    let mut seen = BTreeSet::new();
    let mut events = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let event: Event =
            serde_json::from_str(line).with_context(|| format!("invalid line in {JOURNAL}"))?;
        if event.id.is_empty() || seen.insert(event.id.clone()) {
            events.push(event);
        }
    }
    Ok(events)
}

/// Append every pending transition the journal does not hold yet, then
/// clear the pending list. Safe to repeat at any point.
fn flush(land: &Path, state: &mut State) -> Result<()> {
    if state.pending.is_empty() {
        return Ok(());
    }
    let journaled: BTreeSet<String> = journal_in(land)?.into_iter().map(|e| e.id).collect();
    for event in &state.pending {
        if !journaled.contains(&event.id) {
            append(land, event)?;
        }
    }
    failpoint("flushed")?;
    state.pending.clear();
    write_state(land, state)
}

/// Commit a transition: the state (with every in-memory change the caller
/// made) and the event are written first, then the event is journaled.
fn record(land: &Path, state: &mut State, mut event: Event) -> Result<()> {
    event.id = uuid::Uuid::new_v4().to_string();
    state.pending.push(event);
    write_state(land, state)?;
    flush(land, state)
}

fn event(at: DateTime<Utc>, kind: &str, red_sha: &str) -> Event {
    Event {
        id: String::new(),
        at,
        event: kind.into(),
        red_sha: red_sha.into(),
        sha: None,
        outcome: None,
        reason: None,
    }
}

/// Queue a mail; a newer one with the same key replaces the older.
fn notify(state: &mut State, key: String, title: String, body: String) {
    state.outbox.retain(|n| n.key != key);
    state.outbox.push(Notice { key, title, body });
}

// trace:TASK-3209 | ai:codex
#[derive(Debug)]
struct PassDeadline;

impl std::fmt::Display for PassDeadline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("pass deadline reached")
    }
}

impl std::error::Error for PassDeadline {}

fn pass_budget_spent() -> bool {
    crate::forge_lookup::forge_lookup_deadline().is_some_and(|d| std::time::Instant::now() >= d)
}

/// Try outstanding mail while time remains; delivered notices leave the outbox.
fn deliver(
    land: &Path,
    state: &mut State,
    ports: &mut dyn Ports,
    notes: &mut Vec<String>,
) -> Result<()> {
    if state.outbox.is_empty() {
        return Ok(());
    }
    let mut kept = Vec::new();
    for notice in std::mem::take(&mut state.outbox) {
        if pass_budget_spent() {
            kept.push(notice);
            continue;
        }
        if let Err(e) = ports.mail(&notice.key, &notice.title, &notice.body) {
            notes.push(format!(
                "red-main watch: mail `{}` not delivered yet; retried next pass: {e:#}",
                notice.key
            ));
            kept.push(notice);
        }
    }
    state.outbox = kept;
    write_state(land, state)
}

struct WatchLock(std::fs::File);
impl Drop for WatchLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
    }
}

/// Take the watch lock without waiting: `None` only when another pass holds
/// it; any other failure is an error, never a quiet "skipped".
fn try_lock(land: &Path) -> Result<Option<WatchLock>> {
    use fs2::FileExt;
    let lock = open_plain(
        &land.join(LOCK_FILE),
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true),
    )?;
    match lock.try_lock_exclusive() {
        Ok(()) => Ok(Some(WatchLock(lock))),
        Err(e) if aida_core::file_lock::is_lock_contended(&e) => Ok(None),
        Err(e) => Err(e).context("taking the red-main watch lock"),
    }
}

fn verdict_text(verdict: &Verdict) -> String {
    match verdict {
        Verdict::Green => "green".into(),
        Verdict::Red { required, .. } => format!(
            "red ({})",
            required
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Verdict::Skipped(why) => format!("no CI result ({why})"),
        Verdict::Pending(why) => format!("pending ({why})"),
    }
}

/// One watch pass. `required` is the CI-gated check set, `window` the fix
/// window (from confirmation). Returns what it observed and did.
pub(crate) fn pass(
    root: &Path,
    required: &[String],
    window: std::time::Duration,
    ports: &mut dyn Ports,
) -> Result<Vec<String>> {
    let land = land_dir(root)?;
    let Some(_lock) = try_lock(&land)? else {
        return Ok(vec![
            "red-main watch: another pass is running; skipped".into()
        ]);
    };
    let mut state = read_state(&land)?;
    let mut notes = Vec::new();
    flush(&land, &mut state)?;
    deliver(&land, &mut state, ports, &mut notes)?;
    let scan = scan_history(&land, &mut state, required, window, ports, &mut notes);
    let deadline = scan.as_ref().is_err_and(|error| error.is::<PassDeadline>());
    if deadline {
        write_state(&land, &state)?;
    } else {
        scan?;
    }
    if deadline || pass_budget_spent() {
        notes.push("red-main watch: pass deadline reached; resuming next pass".into());
    }
    deliver(&land, &mut state, ports, &mut notes)?;
    Ok(notes)
}

fn scan_history(
    land: &Path,
    state: &mut State,
    required: &[String],
    window: std::time::Duration,
    ports: &mut dyn Ports,
    notes: &mut Vec<String>,
) -> Result<()> {
    let now = ports.now();
    let recent = ports.recent(BATCH)?;
    let mut verdicts: BTreeMap<String, Verdict> = BTreeMap::new();
    let mut judged = |ports: &mut dyn Ports, commit: &Commit| -> Verdict {
        verdicts
            .entry(commit.sha.clone())
            .or_insert_with(|| {
                if pass_budget_spent() {
                    return Verdict::Pending("pass deadline reached".into());
                }
                judge(ports.checks(&commit.sha), required, commit.at, now)
            })
            .clone()
    };

    // First pass: anchor at the newest green commit read, or the oldest.
    if state.cursor.is_none() && state.first_candidate.is_none() {
        let green = recent
            .iter()
            .find(|c| judged(ports, c) == Verdict::Green)
            .cloned();
        if let Some(anchor) = green {
            state.last_green = Some(anchor.sha.clone());
            state.cursor = Some(anchor.sha.clone());
        } else {
            state.first_candidate = recent.last().cloned();
        }
        if !recent.is_empty() {
            write_state(land, state)?;
        }
    }

    let candidates = match &state.cursor {
        Some(cursor) => ports.after(cursor, BATCH)?,
        None => match &state.first_candidate {
            Some(first) => ports.after(&first.sha, BATCH - 1)?.map(|mut later| {
                later.insert(0, first.clone());
                later
            }),
            None => Some(Vec::new()),
        },
    };
    let Some(mut batch) = candidates else {
        notes.push(format!(
            "red-main watch: {} is no longer on the default branch; re-anchoring next pass",
            short(state.cursor.as_deref().unwrap_or_default())
        ));
        state.first_candidate = None;
        state.cursor = None;
        state.last_green = None;
        state.tip = None;
        if let Some(episode) = state.episode.take() {
            let mut end = event(now, "red_end", &episode.red_sha);
            end.outcome = Some("unattributed".into());
            record(land, state, end)?;
        } else {
            write_state(land, state)?;
        }
        return Ok(());
    };
    // The open episode's red commit first: a green re-run closes it as a
    // flake, and the scan resumes from it so the commits after it are
    // judged again on their own.
    if let Some(episode) = state.episode.clone() {
        let red = Commit {
            sha: episode.red_sha.clone(),
            subject: episode.subject.clone(),
            body: String::new(),
            at: episode.source_at.unwrap_or(episode.opened_at),
        };
        match judged(ports, &red) {
            Verdict::Green => {
                state.cursor = Some(red.sha.clone());
                state.last_green = Some(red.sha.clone());
                close(land, state, &red, true, now, ports, notes)?;
                batch = ports
                    .after(&red.sha, BATCH)?
                    .context("verified red left the pinned first-parent history")?;
            }
            Verdict::Red {
                required: failed,
                contributing,
                diagnostics,
            } => {
                let mut episode = episode;
                if (&episode.failed, &episode.contributing, &episode.diagnostics)
                    != (&failed, &contributing, &diagnostics)
                {
                    episode.failed = failed;
                    episode.contributing = contributing;
                    episode.diagnostics = diagnostics;
                    state.episode = Some(episode);
                    write_state(land, state)?;
                }
            }
            _ => {}
        }
    }

    if batch.is_empty() && state.episode.is_none() {
        if let Some(tip) = recent.first() {
            if state.cursor.as_ref() == Some(&tip.sha)
                && matches!(judged(ports, tip), Verdict::Red { .. })
            {
                if state.last_green.as_ref() == Some(&tip.sha) {
                    state.last_green = recent
                        .iter()
                        .skip(1)
                        .find(|c| judged(ports, c) == Verdict::Green)
                        .map(|c| c.sha.clone());
                }
                batch.push(tip.clone());
            }
        }
    }

    for commit in batch {
        let verdict = judged(ports, &commit);
        if !matches!(verdict, Verdict::Pending(_) | Verdict::Green) {
            state.first_candidate = None;
            state.cursor = Some(commit.sha.clone());
        }
        match &verdict {
            Verdict::Pending(why) => {
                notes.push(format!(
                    "red-main watch: waiting on {} ({why}); later commits wait for it",
                    short(&commit.sha)
                ));
                break;
            }
            Verdict::Green => {
                if state.episode.is_some() {
                    close(land, state, &commit, false, now, ports, notes)?;
                } else {
                    state.last_green = Some(commit.sha.clone());
                }
                state.cursor = Some(commit.sha.clone());
                state.first_candidate = None;
            }
            Verdict::Red {
                required: failed,
                contributing,
                diagnostics,
            } => {
                if state.episode.is_none() {
                    open(
                        land,
                        state,
                        &commit,
                        (failed, contributing, diagnostics),
                        now,
                        ports,
                        notes,
                    )?;
                } else {
                    joined(land, state, &commit)?;
                }
            }
            Verdict::Skipped(_) => {
                if state.episode.is_some() {
                    joined(land, state, &commit)?;
                }
            }
        }
        write_state(land, state)?;
    }
    // The tip, reported apart from the last green commit.
    if let Some(tip) = recent.first().filter(|_| !pass_budget_spent()) {
        let text = verdict_text(&judged(ports, tip));
        if state.tip.as_ref() != Some(&(tip.sha.clone(), text.clone())) {
            state.tip = Some((tip.sha.clone(), text));
            write_state(land, state)?;
        }
    }

    if state
        .episode
        .as_ref()
        .is_some_and(|ep| matches!(verdicts.get(&ep.red_sha), Some(Verdict::Red { .. })))
    {
        advance(land, state, window, now, ports, notes)?;
    }
    Ok(())
}

/// A commit that landed while the episode was open.
fn joined(land: &Path, state: &mut State, commit: &Commit) -> Result<()> {
    let Some(episode) = state.episode.as_mut() else {
        return Ok(());
    };
    if episode.red_merges.contains(&commit.sha) {
        return Ok(());
    }
    episode.red_merges.push(commit.sha.clone());
    let mut e = event(commit.at, "red_merge", &episode.red_sha);
    e.sha = Some(commit.sha.clone());
    record(land, state, e)
}

/// Read the evidence of each failed required or contributing job whose
/// workflow:run:attempt:job has not been read in full. Returns whether
/// anything was read, and the evidence that completed only after the
/// opening mail (sent as a follow-up).
fn probe(
    episode: &mut Episode,
    ports: &mut dyn Ports,
    notes: &mut Vec<String>,
) -> (bool, Vec<Evidence>) {
    let mut changed = false;
    let mut late = Vec::new();
    let jobs: Vec<FailedCheck> = episode
        .failed
        .iter()
        .chain(&episode.contributing)
        .cloned()
        .collect();
    for f in jobs {
        if pass_budget_spent() {
            break;
        }
        let (Some(key), Some(run), Some(id), Some(attempt)) =
            (f.key(), f.run_id.clone(), f.job_id, f.attempt)
        else {
            continue;
        };
        if episode.evidence_keys.contains(&key) {
            continue;
        }
        match ports.job(&run, &f.name, id, attempt) {
            Ok(report) => {
                changed = true;
                let found = Evidence {
                    job_id: Some(id),
                    workflow: f.workflow.clone(),
                    run_id: run.clone(),
                    attempt,
                    check: f.name.clone(),
                    link: report.link,
                    failed_steps: report.failed_steps,
                    excerpt: report.excerpt,
                    log_complete: report.log_complete,
                };
                if found.log_complete {
                    episode.evidence_keys.insert(key);
                    if episode.alerted {
                        late.push(found.clone());
                    }
                } else {
                    notes.push(format!(
                        "red-main watch: the log of `{}` could not be read; retried next pass",
                        f.name
                    ));
                }
                episode.evidence.retain(|e| !f.matches_evidence(e));
                episode.evidence.push(found);
            }
            Err(e) => notes.push(format!(
                "red-main watch: the evidence of `{}` could not be read; retried next pass: {e:#}",
                f.name
            )),
        }
    }
    (changed, late)
}

/// The evidence of the jobs failing now (their current attempts).
fn current_evidence(episode: &Episode) -> Vec<&Evidence> {
    episode
        .failed
        .iter()
        .chain(&episode.contributing)
        .filter_map(|f| episode.evidence.iter().find(|e| f.matches_evidence(e)))
        .collect()
}

/// PURE: why main does not build, from the current attempts' evidence only:
/// a compile failure fixed by a re-run no longer counts.
fn unbuildable(episode: &Episode) -> Option<String> {
    current_evidence(episode).into_iter().find_map(|e| {
        e.failed_steps
            .iter()
            .find(|s| COMPILE_STEPS.iter().any(|p| s.starts_with(p)))
            .map(|step| format!("`{}` failed at step `{step}`", e.check))
    })
}

fn names(list: &[FailedCheck]) -> String {
    list.iter()
        .map(|f| f.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn evidence_text(episode: &Episode) -> String {
    let mut out = String::new();
    for f in episode.failed.iter().chain(&episode.contributing) {
        let found = episode.evidence.iter().find(|e| f.matches_evidence(e));
        match found {
            Some(e) => {
                out.push_str(&format!("- {} ({})\n", f.name, e.link));
                if !e.failed_steps.is_empty() {
                    out.push_str(&format!("  failed steps: {}\n", e.failed_steps.join(", ")));
                }
                for line in e.excerpt.lines() {
                    out.push_str(&format!("  | {line}\n"));
                }
            }
            None => out.push_str(&format!(
                "- {} (evidence not read yet{})\n",
                f.name,
                f.run_id
                    .as_deref()
                    .map(|r| format!("; CI run {r}"))
                    .unwrap_or_default()
            )),
        }
    }
    if !episode.diagnostics.is_empty() {
        out.push_str(&format!(
            "Also failing in other runs (not acted on): {}\n",
            episode.diagnostics.join(", ")
        ));
    }
    out
}

fn held_text(episode: &Episode) -> String {
    match &episode.held_green {
        Some(sha) => format!(
            "The live binary stays on the last green commit, {}.",
            short(sha)
        ),
        None => "No green commit is known before it; the live binary stays where it is.".into(),
    }
}

#[allow(clippy::too_many_arguments)]
fn open(
    land: &Path,
    state: &mut State,
    commit: &Commit,
    (failed, contributing, diagnostics): (&Vec<FailedCheck>, &Vec<FailedCheck>, &Vec<String>),
    now: DateTime<Utc>,
    ports: &mut dyn Ports,
    notes: &mut Vec<String>,
) -> Result<()> {
    let mut episode = Episode {
        red_sha: commit.sha.clone(),
        subject: commit.subject.clone(),
        pr: subject_pr(&commit.subject),
        specs: crate::pr_ship::extract_authorship_spec_ids_from_subject(&commit.subject),
        failed: failed.clone(),
        contributing: contributing.clone(),
        diagnostics: diagnostics.clone(),
        source_at: Some(commit.at),
        opened_at: now,
        held_green: state.last_green.clone(),
        attempts: failed
            .iter()
            .filter_map(|f| Some((f.run_id.clone()?, f.attempt?)))
            .collect(),
        confirmed_at: None,
        evidence: Vec::new(),
        evidence_keys: BTreeSet::new(),
        alerted: false,
        red_merges: Vec::new(),
        recommended: Vec::new(),
    };
    probe(&mut episode, ports, notes);
    episode.alerted = true;
    let body = format!(
        "Main is red at {} ({}).\nPR: {}\nSpecs: {}\nFailing:\n{}{}\n\nIf the red survives an independent re-run or 30 minutes it is confirmed. Merging continues \
         meanwhile.",
        episode.red_sha,
        episode.subject,
        episode.pr.map_or("unknown".into(), |n| format!("#{n}")),
        if episode.specs.is_empty() {
            "none named".into()
        } else {
            episode.specs.join(", ")
        },
        evidence_text(&episode),
        held_text(&episode),
    );
    notify(
        state,
        format!("main-red-{}", episode.red_sha),
        format!(
            "main is red at {}: {}",
            short(&episode.red_sha),
            names(&episode.failed)
        ),
        body,
    );
    notes.push(format!(
        "red-main watch: main went red at {} ({})",
        short(&episode.red_sha),
        names(&episode.failed)
    ));
    let start = event(commit.at, "red_start", &episode.red_sha);
    state.episode = Some(episode);
    record(land, state, start)
}

/// Move an open episode on: the confirming re-run, the evidence, the
/// confirmation and the revert recommendations (mail only).
fn advance(
    land: &Path,
    state: &mut State,
    window: std::time::Duration,
    now: DateTime<Utc>,
    ports: &mut dyn Ports,
    notes: &mut Vec<String>,
) -> Result<()> {
    let Some(mut episode) = state.episode.clone() else {
        return Ok(());
    };
    let (read, late) = if episode.opened_at == now || pass_budget_spent() {
        (false, Vec::new())
    } else {
        probe(&mut episode, ports, notes)
    };
    for found in &late {
        notify(
            state,
            format!(
                "main-red-{}-evidence-{}-{:x}",
                episode.red_sha,
                found.run_id,
                <sha2::Sha256 as sha2::Digest>::digest(serde_json::to_vec(&(
                    &found.workflow,
                    &found.check,
                    found.attempt,
                    found.job_id
                ))?)
            ),
            format!(
                "main red at {}: log of {}",
                short(&episode.red_sha),
                found.check
            ),
            format!(
                "The log of `{}` for main's red at {} is now read:\n{}\n{}",
                found.check,
                short(&episode.red_sha),
                found.link,
                found.excerpt
            ),
        );
    }
    if read || !late.is_empty() {
        state.episode = Some(episode.clone());
        write_state(land, state)?;
    }
    let now = ports.now();
    if episode.confirmed_at.is_none() {
        // Confirmed by a required check failing again at a newer, finished
        // attempt of its run, or by time.
        let rerun_failed_again = episode.failed.iter().any(|f| {
            match (
                f.run_id.as_ref().and_then(|r| episode.attempts.get(r)),
                f.attempt,
            ) {
                (Some(before), Some(now)) => now > *before,
                _ => false,
            }
        });
        let aged = now - episode.opened_at >= Duration::minutes(CONFIRM_AFTER_MINUTES);
        if !(rerun_failed_again || aged) {
            return Ok(());
        }
        episode.confirmed_at = Some(now);
        notify(
            state,
            format!("main-red-{}-confirmed", episode.red_sha),
            format!("main red at {} is confirmed", short(&episode.red_sha)),
            format!(
                "The red at {} ({}) survived its re-run or 30 minutes.\nFailing:\n{}{}",
                episode.red_sha,
                episode.subject,
                evidence_text(&episode),
                held_text(&episode)
            ),
        );
        state.episode = Some(episode.clone());
        let mut e = event(now, "confirmed", &episode.red_sha);
        e.reason = Some(if rerun_failed_again {
            "re-run failed".into()
        } else {
            "30 minutes".into()
        });
        record(land, state, e)?;
        notes.push(format!(
            "red-main watch: the red at {} is confirmed",
            short(&episode.red_sha)
        ));
    }
    // Revert recommendations, each mailed once. The fix window runs from
    // confirmation, as documented.
    let confirmed = episode.confirmed_at.unwrap_or(now);
    let window = Duration::from_std(window).unwrap_or(Duration::hours(2));
    let mut reasons = Vec::new();
    if let Some(why) = unbuildable(&episode) {
        reasons.push(("unbuildable", format!("main does not build: {why}")));
    }
    if now - confirmed >= window {
        reasons.push((
            "fix-window-elapsed",
            format!(
                "main is still red {} min after it was confirmed",
                window.num_minutes()
            ),
        ));
    }
    for (key, why) in reasons {
        if episode.recommended.iter().any(|r| r == key) {
            continue;
        }
        episode.recommended.push(key.into());
        notify(
            state,
            format!("main-red-{}-revert-{key}", episode.red_sha),
            format!("revert recommended: {why}"),
            format!(
                "Revert recommended for main's red at {} ({}): {why}.\nRevert with an ordinary \
                 reviewed PR: git revert --no-edit {}\n\nFailing:\n{}",
                episode.red_sha,
                episode.subject,
                episode.red_sha,
                evidence_text(&episode)
            ),
        );
        state.episode = Some(episode.clone());
        let mut e = event(now, "revert_recommended", &episode.red_sha);
        e.reason = Some(key.into());
        record(land, state, e)?;
        notes.push(format!("red-main watch: revert recommended ({key})"));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn close(
    land: &Path,
    state: &mut State,
    green: &Commit,
    rerun_green: bool,
    now: DateTime<Utc>,
    ports: &mut dyn Ports,
    notes: &mut Vec<String>,
) -> Result<()> {
    let Some(episode) = state.episode.clone() else {
        return Ok(());
    };
    let confirmed = episode.confirmed_at.is_some();
    // A revert is looked for in exactly the commits that closed the episode;
    // when they cannot be read the outcome is unattributed.
    let outcome = if rerun_green {
        if confirmed {
            "cleared"
        } else {
            "flake"
        }
    } else {
        match ports.range(&episode.red_sha, &green.sha) {
            Ok(closing) => {
                let revert = format!("This reverts commit {}.", episode.red_sha);
                if closing
                    .iter()
                    .any(|c| c.body.lines().any(|line| line == revert))
                {
                    "reverted"
                } else if confirmed {
                    "recovered"
                } else {
                    "cleared"
                }
            }
            Err(e) if e.is::<PassDeadline>() => return Err(e),
            Err(e) => {
                notes.push(format!(
                    "red-main watch: the closing commits could not be read: {e:#}"
                ));
                "unattributed"
            }
        }
    };
    let body = format!(
        "Main is green again at {} after {} min red (first red: {}). Outcome: {outcome}.",
        short(&green.sha),
        (now - episode.opened_at).num_minutes(),
        short(&episode.red_sha)
    );
    notify(
        state,
        format!("main-red-{}-green", episode.red_sha),
        format!("main is green again at {}", short(&green.sha)),
        body.clone(),
    );
    state.episode = None;
    state.cursor = Some(green.sha.clone());
    state.first_candidate = None;
    state.last_green = Some(green.sha.clone());
    let mut e = event(
        if rerun_green { now } else { green.at },
        "red_end",
        &episode.red_sha,
    );
    e.sha = Some(green.sha.clone());
    e.outcome = Some(outcome.into());
    record(land, state, e)?;
    notes.push(format!("red-main watch: {body}"));
    Ok(())
}

/// The watch's state in one line, read from its state file: the tip's CI
/// and the last green commit apart, and the open episode if any. It never
/// says green while an episode is open.
pub(crate) fn status_line(root: &Path) -> Result<String> {
    let state = match watch_directory(root, false) {
        Ok(dir) => read_state(&dir)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::default(),
        Err(e) => return Err(e.into()),
    };
    let tip = match &state.tip {
        Some((sha, text)) => format!("tip {}: {text}", short(sha)),
        None => "tip: not read yet".to_string(),
    };
    let green = state
        .last_green
        .as_deref()
        .map_or("no green commit known".to_string(), |g| {
            format!("last green {}", short(g))
        });
    Ok(match &state.episode {
        Some(episode) => format!(
            "red-main watch: main is red at {} since {} ({}); {tip}; {green}",
            short(&episode.red_sha),
            episode.opened_at.format("%Y-%m-%d %H:%M UTC"),
            if episode.confirmed_at.is_some() {
                "confirmed"
            } else {
                "waiting for its re-run or 30 minutes"
            }
        ),
        None => format!("red-main watch: no red episode; {tip}; {green}"),
    })
}

/// The red-main block of `aida land metrics` for `[since, until)`. Only what
/// happened inside the window counts; an episode that ends at or after
/// `until` is still open at the cutoff.
pub(crate) fn metrics(
    events: &[Event],
    since: DateTime<Utc>,
    until: DateTime<Utc>,
) -> serde_json::Value {
    #[derive(Default)]
    struct Ep {
        start: Option<DateTime<Utc>>,
        end: Option<DateTime<Utc>>,
        outcome: Option<String>,
        merges: Vec<DateTime<Utc>>,
    }
    let inside = |at: DateTime<Utc>| at >= since && at < until;
    let mut episodes: Vec<(String, Ep)> = Vec::new();
    for e in events {
        let slot = match episodes
            .iter()
            .rposition(|(sha, ep)| *sha == e.red_sha && ep.end.is_none())
        {
            Some(i) => &mut episodes[i].1,
            None if e.event == "red_start" => {
                episodes.push((e.red_sha.clone(), Ep::default()));
                &mut episodes.last_mut().unwrap().1
            }
            None => continue,
        };
        match e.event.as_str() {
            "red_start" => slot.start = Some(e.at),
            "red_merge" => slot.merges.push(e.at),
            "red_end" => {
                slot.end = Some(e.at);
                slot.outcome = e.outcome.clone();
            }
            _ => {}
        }
    }
    let mut red_seconds = 0i64;
    let mut count = 0usize;
    let mut flakes = 0usize;
    let mut red_merges = 0usize;
    let mut latencies: Vec<i64> = Vec::new();
    let mut reverted = 0usize;
    let mut resolved = 0usize;
    for (_, ep) in &episodes {
        let Some(start) = ep.start else { continue };
        let ended = ep.end.filter(|end| *end < until);
        let end = ended.unwrap_or(until);
        if end <= since || start >= until {
            continue;
        }
        count += 1;
        red_seconds += (end - start.max(since)).num_seconds().max(0);
        if ended.is_some() && ep.outcome.as_deref() == Some("flake") {
            flakes += 1;
            continue;
        }
        red_merges +=
            usize::from(inside(start)) + ep.merges.iter().filter(|at| inside(**at)).count();
        if let (Some(end), Some(outcome)) = (ended, ep.outcome.as_deref()) {
            latencies.push((end - start).num_seconds());
            resolved += 1;
            if outcome == "reverted" {
                reverted += 1;
            }
        }
    }
    latencies.sort_unstable();
    serde_json::json!({
        "source": "red-main watch journal",
        "episodes": count,
        "flakes": flakes,
        "red_minutes": red_seconds / 60,
        "red_merges": red_merges,
        "fix_latency_seconds_median": median(&latencies),
        "fix_latency_seconds_max": latencies.last(),
        "reverted": reverted,
        "resolved": resolved,
    })
}

/// PURE: the error lines of a job log: the step's own error line, then the
/// last lines naming an error, a failed test or a panic.
pub(crate) fn excerpt(log: &str, step: Option<&str>) -> String {
    let mut lines: Vec<String> = Vec::new();
    if let Some(step) = step {
        let line = crate::landing_health::error_line(log, step);
        if !line.starts_with("unknown") {
            lines.push(line);
        }
    }
    let marked: Vec<String> = log
        .lines()
        .filter(|l| {
            l.to_ascii_lowercase().contains("error")
                || l.contains("FAILED")
                || l.contains("panicked")
        })
        .map(|l| l.chars().take(300).collect())
        .collect();
    let tail = marked.len().saturating_sub(12);
    lines.extend(marked.into_iter().skip(tail));
    if lines.is_empty() {
        "(no error lines in the job log)".into()
    } else {
        lines.join("\n")
    }
}

/// The production ports.
// trace:STORY-2897 | ai:codex
pub(crate) struct Live {
    pub root: PathBuf,
    pub forge: Box<dyn crate::forge::Forge>,
    pinned_tip: Option<String>,
    /// The ceiling on each git call.
    pub call_timeout: std::time::Duration,
}

// trace:TASK-3209 | ai:codex
fn git_timeout_error(
    args: &[&str],
    timeout: std::time::Duration,
    deadline: Option<std::time::Instant>,
    now: std::time::Instant,
) -> anyhow::Error {
    // The runner can shorten the timeout after Git's initial budget sample.
    if deadline.is_some_and(|end| now >= end) {
        PassDeadline.into()
    } else {
        anyhow::anyhow!("git {} timed out after {:?}", args.join(" "), timeout)
    }
}

impl Live {
    fn git(&self, args: &[&str]) -> Result<String> {
        let mut cmd = std::process::Command::new("git");
        crate::review_watch::forge::clear_routing(&mut cmd);
        cmd.args(args)
            .current_dir(&self.root)
            .stdin(std::process::Stdio::null())
            .env("GIT_TERMINAL_PROMPT", "0");
        let timeout = crate::forge_lookup::forge_lookup_deadline().map_or(self.call_timeout, |d| {
            self.call_timeout
                .min(d.saturating_duration_since(std::time::Instant::now()))
        });
        if timeout.is_zero() && pass_budget_spent() {
            return Err(PassDeadline.into());
        }
        match crate::command_output_with_timeout_capped(cmd, timeout, 16 * 1024 * 1024) {
            crate::BoundedCommandOutput::Completed(out) if out.status.success() => {
                anyhow::ensure!(
                    out.stdout.len() <= 16 * 1024 * 1024,
                    "git output was truncated"
                );
                Ok(String::from_utf8_lossy(&out.stdout).into_owned())
            }
            crate::BoundedCommandOutput::Completed(out) => anyhow::bail!(
                "git {} failed ({}): {}",
                args.join(" "),
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ),
            crate::BoundedCommandOutput::TimedOut => Err(git_timeout_error(
                args,
                timeout,
                crate::forge_lookup::forge_lookup_deadline(),
                std::time::Instant::now(),
            )),
            crate::BoundedCommandOutput::SpawnFailed => anyhow::bail!("git could not start"),
        }
    }

    fn log(&self, args: &[&str]) -> Result<Vec<Commit>> {
        let mut full = vec![
            "log",
            "-z",
            "--first-parent",
            "--format=%H%x00%cI%x00%s%x00%b",
        ];
        full.extend_from_slice(args);
        let raw = self.git(&full)?;
        let mut fields = raw.split_terminator('\0');
        let mut commits = Vec::new();
        while let Some(sha) = fields.next() {
            let at = DateTime::parse_from_rfc3339(fields.next().context("missing commit date")?)?;
            commits.push(Commit {
                sha: sha.into(),
                at: at.with_timezone(&Utc),
                subject: fields.next().context("missing commit subject")?.into(),
                body: fields.next().context("missing commit body")?.into(),
            });
        }
        Ok(commits)
    }
}

impl Ports for Live {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }

    fn recent(&mut self, max: usize) -> Result<Vec<Commit>> {
        let remote = self.git(&["ls-remote", "--symref", "origin", "HEAD"])?;
        let tip = remote
            .lines()
            .find_map(|l| {
                l.strip_suffix("\tHEAD").filter(|sha| {
                    matches!(sha.len(), 40 | 64) && sha.bytes().all(|b| b.is_ascii_hexdigit())
                })
            })
            .context("origin did not advertise a default tip")?
            .to_owned();
        self.git(&["fetch", "--quiet", "--no-write-fetch-head", "origin", &tip])?;
        self.pinned_tip = Some(tip.clone());
        self.log(&[&format!("--max-count={max}"), &tip])
    }

    fn after(&mut self, cursor: &str, max: usize) -> Result<Option<Vec<Commit>>> {
        let tip = self.pinned_tip.as_ref().context("no fetched tip")?;
        anyhow::ensure!(
            cursor.len() == tip.len() && cursor.bytes().all(|c| c.is_ascii_hexdigit()),
            "invalid cursor"
        );
        let distance: usize = self
            .git(&[
                "rev-list",
                "--first-parent",
                "--count",
                &format!("{cursor}..{tip}"),
            ])?
            .trim()
            .parse()?;
        let ancestor = self.git(&[
            "rev-list",
            "--first-parent",
            "--max-count=1",
            &format!("--skip={distance}"),
            tip,
        ])?;
        if ancestor.trim() != cursor {
            return Ok(None);
        }
        let end = format!("{tip}~{}", distance.saturating_sub(max));
        let commits = self.log(&["--reverse", &format!("{cursor}..{end}")])?;
        Ok(Some(commits))
    }

    fn range(&mut self, from: &str, to: &str) -> Result<Vec<Commit>> {
        self.log(&[&format!("{from}..{to}")])
    }

    fn checks(&mut self, sha: &str) -> Result<CommitChecks> {
        self.forge.watch_commit_checks(sha)
    }

    fn job(&mut self, run_id: &str, job: &str, id: u64, attempt: u64) -> Result<JobReport> {
        let report = self.forge.run_job_report(run_id, job, id, attempt)?;
        let (excerpt, log_complete) = match &report.log {
            Ok(log) => (
                excerpt(log, report.failed_steps.first().map(String::as_str)),
                true,
            ),
            Err(e) => (format!("(the job log could not be read yet: {e})"), false),
        };
        Ok(JobReport {
            link: report.link,
            failed_steps: report.failed_steps,
            excerpt,
            log_complete,
        })
    }

    fn mail(&mut self, key: &str, subject: &str, body: &str) -> Result<()> {
        use aida_core::mailbox::{Intent, Message, Recipient, SenderSource};
        let id = format!("{key}-advisor");
        if aida_core::mailbox::read_local_messages(&self.root)?
            .iter()
            .any(|m| m.id == id)
        {
            return Ok(());
        }
        let message = Message {
            id: id.clone(),
            thread_id: key.to_string(),
            from: "red-main-watch".into(),
            to: Recipient::Agent("advisor".into()),
            timestamp: Utc::now().timestamp_millis(),
            in_reply_to: None,
            subject: Some(subject.to_string()),
            body: body.to_string(),
            urgent: true,
            intent: Intent::Request,
            retracted: false,
            deleted: false,
            archived: false,
            from_source: SenderSource::Explicit,
            from_role: None,
            from_seat: None,
            relayed_from: None,
        };
        match aida_core::mailbox::write_local_message(&self.root, &message) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

/// The one watch root: the project's main checkout, the same root `aida land
/// metrics` reads, whichever checkout the command runs in.
pub(crate) fn watch_root(start: &Path) -> Result<PathBuf> {
    crate::review_watch::forge::main_root(start)
}

/// One pass with the production ports, for `aida land watch` (run by hand or
/// by its own scheduled job; no landing path calls it), bounded by
/// [`PASS_BUDGET`].
pub(crate) fn live_pass(root: &Path) -> Result<Vec<String>> {
    crate::forge_lookup::with_forge_lookup_deadline(PASS_BUDGET, || {
        let policy = crate::landing_policy::load(root);
        let mut ports = Live {
            root: root.to_path_buf(),
            forge: crate::forge::forge_for_kind(root, crate::forge::peek_forge_kind(root)),
            pinned_tip: None,
            call_timeout: CALL_TIMEOUT,
        };
        pass(
            root,
            &policy.ci_gated_checks,
            policy.red_main_fix_window,
            &mut ports,
        )
    })
}

#[cfg(test)]
#[path = "tests/story_2897_main_watch_tests.rs"]
mod tests;
