//! The red-main watch (STORY-2897 S3): notices when the default branch's CI
//! goes red after a merge, routes a fix forward, and recommends a revert only
//! on the fixed triggers. It is observational: no merge decision reads it,
//! and merging continues while main is red (operator direction 2026-11-18).
//!
//! An episode opens on the first red commit after the last green one. Its
//! opening is journaled and mailed (urgent, advisor and product) at once, and
//! the failed jobs are re-run once. The fix spec, the operator notice and the
//! `aida human` attention item wait until the red is confirmed: it survives
//! that re-run, or 30 minutes pass. A red that a green re-run clears first is
//! a flake: no spec, no notice.
//!
//! Durability: every transition is written to the state file first, carrying
//! a transition id, and then appended to the journal; a pass replays any
//! transition the journal lacks, so a crash at either boundary neither loses
//! nor duplicates an event. Notices wait in a persistent outbox until they
//! are delivered, and are retried every pass, also after the episode closes.
//!
//! Boundedness: every git and forge call the live watch makes has null stdin
//! and a timeout, the whole pass runs under a scoped deadline, and the watch
//! lock is only ever tried. The watch runs only as its own scheduled job
//! (`land watch`) or by hand: no landing path calls it, so nothing that
//! merges can stall on it.
//!
//! State: `.aida/land/main-watch.json`; journal: `.aida/land/main-watch.jsonl`
//! (append-only, read by `aida land metrics`).
// trace:STORY-2897 | ai:claude

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::forge::{CheckOutcome, CommitChecks};

pub(crate) const STATE: &str = ".aida/land/main-watch.json";
pub(crate) const JOURNAL: &str = ".aida/land/main-watch.jsonl";
const LOCK: &str = ".aida/land/main-watch.lock";
/// How many first-parent commits one pass reads back from the tip.
pub(crate) const SEARCH: usize = 50;
/// A red not cleared by its re-run is confirmed after this long anyway.
pub(crate) const CONFIRM_AFTER_MINUTES: i64 = 30;
/// CI steps that compile: a red failing in one of these is unbuildable.
pub(crate) const COMPILE_STEPS: &[&str] = &["Build ", "Check aida-cli"];
/// The ceiling on one git or forge call the live watch makes.
pub(crate) const CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// The ceiling on a whole live pass (the TASK-3792 scoped deadline).
pub(crate) const PASS_BUDGET: std::time::Duration = std::time::Duration::from_secs(120);
/// The tail of a job log the excerpt is taken from.
const LOG_CAP: u64 = 1024 * 1024;

/// One default-branch commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Commit {
    pub sha: String,
    pub subject: String,
}

/// What one failed CI job shows: where to look, the failed steps, and the
/// error lines of its log.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Evidence {
    pub check: String,
    pub link: String,
    pub failed_steps: Vec<String>,
    pub excerpt: String,
    /// The job log was read; `false` keeps the link and steps and retries
    /// the log on a later pass.
    #[serde(default)]
    pub log_complete: bool,
}

/// What the watch reads and does outside its own state files. [`Live`] is
/// the real one; tests use a fake.
pub(crate) trait Ports {
    fn now(&self) -> DateTime<Utc>;
    /// First-parent commits of the freshly fetched default branch, newest
    /// first, stopping before `stop` (exclusive) and after `max`.
    fn history(&mut self, stop: Option<&str>, max: usize) -> Result<Vec<Commit>>;
    /// First-parent commits in `(from, to]`, newest first.
    fn range(&mut self, from: &str, to: &str) -> Result<Vec<Commit>>;
    fn checks(&mut self, sha: &str) -> Result<CommitChecks>;
    /// The link, failed steps and log excerpt of job `job` in CI run `run_id`.
    fn evidence(&mut self, run_id: &str, job: &str) -> Result<Evidence>;
    fn rerun_failed(&mut self, run_id: &str) -> Result<()>;
    /// Urgent mail to the advisor and product seats; idempotent by `key`.
    fn mail(&mut self, key: &str, subject: &str, body: &str) -> Result<()>;
    /// The operator's optional push notice.
    fn notify_operator(&mut self, key: &str, title: &str, body: &str) -> Result<()>;
    /// Add `text` to fix spec `id` as a comment, once.
    fn comment_fix_spec(&mut self, id: &str, text: &str) -> Result<()>;
    /// Create or update the durable `aida human` item for `red_sha`.
    fn human_open(&mut self, red_sha: &str, title: &str, body: &str) -> Result<String>;
    /// Resolve the `aida human` item for `red_sha`, if there is one.
    fn human_resolve(&mut self, red_sha: &str) -> Result<()>;
    /// File the fix spec (once per red commit) and make sure it is queued at
    /// the top of the implementer queue; returns its id.
    fn file_fix_spec(&mut self, fix: &FixSpec) -> Result<String>;
    fn fix_spec_state(&mut self, id: &str) -> Result<FixState>;
}

/// The fix spec a confirmed red files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FixSpec {
    pub red_sha: String,
    pub title: String,
    pub description: String,
    /// The red commit's specs: the fix spec is linked `fixes` to each.
    pub fixes: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct FixState {
    pub completed: bool,
    pub needs_design: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct State {
    pub last_green: Option<String>,
    pub episode: Option<Episode>,
    /// Notices not yet delivered; they outlive the episode.
    #[serde(default)]
    pub outbox: Vec<Notice>,
    /// Transitions written here first and not yet confirmed in the journal.
    #[serde(default)]
    pub pending: Vec<Event>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FailedCheck {
    pub name: String,
    pub run_id: Option<String>,
    pub attempt: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Episode {
    pub red_sha: String,
    pub subject: String,
    pub pr: Option<u64>,
    pub specs: Vec<String>,
    /// The checks failing on the red commit, as last seen.
    pub failed: Vec<FailedCheck>,
    pub opened_at: DateTime<Utc>,
    /// The newest green commit before the red: the live binary stays there.
    pub held_green: Option<String>,
    /// Owning CI run id -> its attempt when the episode opened.
    pub attempts: BTreeMap<String, u64>,
    pub rerun_requested: bool,
    pub confirmed_at: Option<DateTime<Utc>>,
    pub fix_spec: Option<String>,
    /// Failure evidence per failing check.
    #[serde(default)]
    pub evidence: Vec<Evidence>,
    /// `run:attempt` of each failed job whose evidence, log included, was
    /// read; any other failed job is read again on the next pass.
    #[serde(default)]
    pub evidence_keys: BTreeSet<String>,
    /// The opening alert went out; evidence arriving later is sent after it.
    #[serde(default)]
    pub alerted: bool,
    /// `Some(why)` when a failing check failed at a compile step.
    pub unbuildable: Option<String>,
    /// The episode commit whose staged build failed the smoke test.
    pub smoke_failed: Option<String>,
    /// Every later commit that landed while the episode was open, whatever
    /// its CI said.
    pub red_merges: Vec<String>,
    /// Revert reasons already escalated (one escalation per reason).
    pub recommended: Vec<String>,
}

/// One outstanding notice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Notice {
    pub key: String,
    pub channel: Channel,
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub(crate) enum Channel {
    Mail,
    Operator,
    HumanOpen { red_sha: String },
    HumanResolve { red_sha: String },
    FixComment { spec: String },
}

/// One journal line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Event {
    /// The transition id; the journal holds each id at most once.
    #[serde(default)]
    pub id: String,
    pub at: DateTime<Utc>,
    /// `red_start`, `red_merge`, `confirmed`, `fix_filed`,
    /// `revert_recommended` or `red_end`.
    pub event: String,
    pub red_sha: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
    /// `red_end`: `flake`, `cleared`, `fixed` or `reverted`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix_spec: Option<String>,
}

/// The watch's reading of one commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Verdict {
    Green,
    Red(Vec<FailedCheck>),
    /// Pending, cancelled-only, missing or unreadable: never a transition.
    Unknown(String),
}

/// PURE: judge a default-branch commit for the watch. Red means a required
/// check (the CI-gated set, without `merge-hold-gate`, which runs on pull
/// requests only) completed as a failure that was not a cancellation. A
/// commit whose only failures are cancellations is unknown, not red.
pub(crate) fn judge(checks: Result<CommitChecks>, required: &[String]) -> Verdict {
    let checks = match checks {
        Ok(checks) => checks,
        Err(e) => return Verdict::Unknown(format!("CI could not be read: {e:#}")),
    };
    let latest = crate::forge::latest_per_name(&checks.checks);
    let required: Vec<&str> = required
        .iter()
        .map(String::as_str)
        .filter(|n| *n != crate::landing_policy::MERGE_HOLD_GATE)
        .collect();
    let mut rerunning: Vec<String> = Vec::new();
    let failed: Vec<FailedCheck> = latest
        .iter()
        .filter(|c| c.outcome == CheckOutcome::Failed && !c.cancelled)
        .copied()
        .collect::<Vec<_>>()
        .into_iter()
        .filter(|c| {
            // A failure counts for an attempt only once that attempt has
            // completed; a re-run in progress is pending.
            if c.run_in_progress && required.contains(&c.name.as_str()) {
                rerunning.push(c.name.clone());
            }
            !c.run_in_progress
        })
        .map(|c| FailedCheck {
            name: c.name.clone(),
            run_id: c.run_id.clone(),
            attempt: c.run_attempt,
        })
        .collect();
    if !rerunning.is_empty() {
        return Verdict::Unknown(format!("a re-run is in progress: {}", rerunning.join(", ")));
    }
    // A required check that failed makes the commit red; other failed checks
    // are carried as evidence beside it, but alone are not red.
    if failed.iter().any(|f| required.contains(&f.name.as_str())) {
        return Verdict::Red(failed);
    }
    match crate::forge::judge_commit_checks(&checks, &required) {
        crate::forge::CommitCi::Green => Verdict::Green,
        crate::forge::CommitCi::Failed(names) => Verdict::Unknown(format!(
            "only cancelled or optional checks failed: {}",
            names.join(", ")
        )),
        crate::forge::CommitCi::Pending(names) => {
            Verdict::Unknown(format!("CI is still running: {}", names.join(", ")))
        }
        crate::forge::CommitCi::NoChecks => Verdict::Unknown("no CI ran".into()),
        crate::forge::CommitCi::MissingRequired(name) => {
            Verdict::Unknown(format!("required check `{name}` did not report success"))
        }
    }
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(12)]
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

fn read_state(root: &Path) -> Result<State> {
    match std::fs::read(root.join(STATE)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("{STATE} is not valid; fix or remove it")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
        Err(e) => Err(e).with_context(|| format!("reading {STATE}")),
    }
}

fn write_state(root: &Path, state: &State) -> Result<()> {
    failpoint("state")?;
    let path = root.join(STATE);
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, serde_json::to_vec_pretty(state)?)?;
    std::fs::rename(&temp, &path).with_context(|| format!("writing {STATE}"))
}

fn append(root: &Path, event: &Event) -> Result<()> {
    failpoint("journal")?;
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join(JOURNAL))
        .with_context(|| format!("opening {JOURNAL}"))?;
    // Cut a torn last line (a crash mid-append) before appending.
    let len = file.metadata()?.len();
    if len > 0 {
        let mut last = [0u8; 1];
        file.seek(SeekFrom::Start(len - 1))?;
        file.read_exact(&mut last)?;
        if last[0] != b'\n' {
            let mut text = Vec::new();
            file.seek(SeekFrom::Start(0))?;
            file.read_to_end(&mut text)?;
            let keep = text.iter().rposition(|b| *b == b'\n').map_or(0, |p| p + 1);
            file.set_len(keep as u64)?;
        }
    }
    file.seek(SeekFrom::End(0))?;
    let mut line = serde_json::to_vec(event)?;
    line.push(b'\n');
    file.write_all(&line)?;
    file.sync_data()?;
    Ok(())
}

/// Every journal event, oldest first, each transition id once.
pub(crate) fn read_journal(root: &Path) -> Result<Vec<Event>> {
    let events: Vec<Event> = match std::fs::read_to_string(root.join(JOURNAL)) {
        Ok(text) => {
            let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
            let mut events = Vec::new();
            for (i, line) in lines.iter().enumerate() {
                match serde_json::from_str(line) {
                    Ok(event) => events.push(event),
                    // A crash mid-append leaves a torn last line: skip it;
                    // the next append truncates it away.
                    Err(e) if i + 1 == lines.len() => {
                        eprintln!("red-main watch: ignoring a torn last line in {JOURNAL}: {e}");
                    }
                    Err(e) => return Err(e).with_context(|| format!("invalid line in {JOURNAL}")),
                }
            }
            events
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e.into()),
    };
    let mut seen = BTreeSet::new();
    Ok(events
        .into_iter()
        .filter(|e| e.id.is_empty() || seen.insert(e.id.clone()))
        .collect())
}

/// Append every pending transition the journal does not hold yet, then
/// clear the pending list. Safe to repeat at any point.
fn flush(root: &Path, state: &mut State) -> Result<()> {
    if state.pending.is_empty() {
        return Ok(());
    }
    let journaled: BTreeSet<String> = read_journal(root)?.into_iter().map(|e| e.id).collect();
    for event in &state.pending {
        if !journaled.contains(&event.id) {
            append(root, event)?;
        }
    }
    failpoint("flushed")?;
    state.pending.clear();
    write_state(root, state)
}

/// Commit a transition: the state (with every in-memory change the caller
/// made) and the event are written first, then the event is journaled.
fn record(root: &Path, state: &mut State, mut event: Event) -> Result<()> {
    event.id = uuid::Uuid::new_v4().to_string();
    state.pending.push(event);
    write_state(root, state)?;
    flush(root, state)
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
        fix_spec: None,
    }
}

/// Queue a notice; a newer one with the same key replaces the older.
fn notify(state: &mut State, key: String, channel: Channel, title: String, body: String) {
    state.outbox.retain(|n| n.key != key);
    state.outbox.push(Notice {
        key,
        channel,
        title,
        body,
    });
}

/// Try every outstanding notice once; delivered ones leave the outbox. The
/// `aida human` notices of one red commit are delivered in order: after one
/// fails, its later ones wait for the next pass.
fn deliver(
    root: &Path,
    state: &mut State,
    ports: &mut dyn Ports,
    notes: &mut Vec<String>,
) -> Result<()> {
    if state.outbox.is_empty() {
        return Ok(());
    }
    let mut kept = Vec::new();
    let mut blocked: BTreeSet<String> = BTreeSet::new();
    for notice in std::mem::take(&mut state.outbox) {
        let result = match &notice.channel {
            Channel::Mail => ports.mail(&notice.key, &notice.title, &notice.body),
            Channel::Operator => ports.notify_operator(&notice.key, &notice.title, &notice.body),
            Channel::HumanOpen { red_sha } | Channel::HumanResolve { red_sha }
                if blocked.contains(red_sha) =>
            {
                Err(anyhow::anyhow!(
                    "an earlier attention update is still outstanding"
                ))
            }
            Channel::HumanOpen { red_sha } => ports
                .human_open(red_sha, &notice.title, &notice.body)
                .map(|_| ()),
            Channel::HumanResolve { red_sha } => ports.human_resolve(red_sha),
            Channel::FixComment { spec } => ports.comment_fix_spec(spec, &notice.body),
        };
        if let Err(e) = result {
            if let Channel::HumanOpen { red_sha } | Channel::HumanResolve { red_sha } =
                &notice.channel
            {
                blocked.insert(red_sha.clone());
            }
            notes.push(format!(
                "red-main watch: notice `{}` not delivered yet; retried next pass: {e:#}",
                notice.key
            ));
            kept.push(notice);
        }
    }
    state.outbox = kept;
    write_state(root, state)
}

struct WatchLock(std::fs::File);
impl Drop for WatchLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
    }
}

/// Try the watch lock for up to `wait`; `None` when another pass holds it.
fn try_lock(root: &Path, wait: std::time::Duration) -> Result<Option<WatchLock>> {
    use fs2::FileExt;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(root.join(LOCK))?;
    let until = std::time::Instant::now() + wait;
    loop {
        if lock.try_lock_exclusive().is_ok() {
            return Ok(Some(WatchLock(lock)));
        }
        if std::time::Instant::now() >= until {
            return Ok(None);
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// One watch pass. `required` is the CI-gated check set, `window` the fix
/// window. Returns what it observed and did, one line each.
pub(crate) fn pass(
    root: &Path,
    required: &[String],
    window: std::time::Duration,
    ports: &mut dyn Ports,
) -> Result<Vec<String>> {
    std::fs::create_dir_all(root.join(".aida/land"))?;
    let Some(_lock) = try_lock(root, std::time::Duration::ZERO)? else {
        return Ok(vec![
            "red-main watch: another pass is running; skipped".into()
        ]);
    };
    let mut state = read_state(root)?;
    let mut notes = Vec::new();
    // Replay transitions a crash left unjournaled, and retry notices.
    flush(root, &mut state)?;
    deliver(root, &mut state, ports, &mut notes)?;
    let now = ports.now();

    // Newest first. With no open episode, history older than the newest
    // green commit decides nothing; with one open, every commit since the
    // last green one is read again, so a green re-run or an intervening
    // green commit is seen.
    let commits = ports.history(state.last_green.as_deref(), SEARCH)?;
    let mut seen: Vec<(Commit, Verdict)> = Vec::new();
    for commit in commits {
        let verdict = judge(ports.checks(&commit.sha), required);
        let stop = state.episode.is_none() && verdict == Verdict::Green;
        seen.push((commit, verdict));
        if stop {
            break;
        }
    }
    if seen.iter().all(|(_, v)| matches!(v, Verdict::Unknown(_))) {
        let why = seen
            .first()
            .map(|(c, v)| match v {
                Verdict::Unknown(why) => format!("{}: {why}", short(&c.sha)),
                _ => String::new(),
            })
            .unwrap_or_else(|| "no new commits".into());
        notes.push(format!("red-main watch: main CI unknown ({why})"));
    }

    // Oldest first: open at the first red, close at a green, and record each
    // commit that lands while an episode is open. Commits older than the
    // episode's red commit (read again because the scan starts at the last
    // green one) neither close it nor join it; when the red commit is past
    // the scan's reach, every commit read is newer.
    let mut past_red = state
        .episode
        .as_ref()
        .is_none_or(|e| !seen.iter().any(|(c, _)| c.sha == e.red_sha));
    for (commit, verdict) in seen.iter().rev() {
        let Some(episode) = state.episode.clone() else {
            match verdict {
                Verdict::Green => {
                    if state.last_green.as_deref() != Some(commit.sha.as_str()) {
                        state.last_green = Some(commit.sha.clone());
                        write_state(root, &state)?;
                    }
                }
                Verdict::Red(failed) => {
                    open(root, &mut state, commit, failed, now, ports, &mut notes)?;
                    past_red = true;
                }
                Verdict::Unknown(_) => {}
            }
            continue;
        };
        if commit.sha == episode.red_sha {
            past_red = true;
            match verdict {
                Verdict::Green => close(root, &mut state, commit, &seen, now, ports, &mut notes)?,
                Verdict::Red(failed) if *failed != episode.failed => {
                    let mut episode = episode;
                    episode.failed = failed.clone();
                    state.episode = Some(episode);
                    write_state(root, &state)?;
                }
                _ => {}
            }
        } else if !past_red {
            continue;
        } else if *verdict == Verdict::Green {
            close(root, &mut state, commit, &seen, now, ports, &mut notes)?;
        } else if !episode.red_merges.contains(&commit.sha) {
            let red_sha = episode.red_sha.clone();
            let mut episode = episode;
            episode.red_merges.push(commit.sha.clone());
            state.episode = Some(episode);
            let mut e = event(now, "red_merge", &red_sha);
            e.sha = Some(commit.sha.clone());
            record(root, &mut state, e)?;
        }
    }

    if state.episode.is_some() {
        advance(root, &mut state, window, now, ports, &mut notes)?;
    }
    deliver(root, &mut state, ports, &mut notes)?;
    Ok(notes)
}

/// Read the failure evidence of each failed job whose `run:attempt` has not
/// been read in full yet. Returns whether anything was read, and the
/// evidence that completed only after the opening alert went out (it is sent
/// as a follow-up). A failed read is retried next pass.
fn probe(
    episode: &mut Episode,
    ports: &mut dyn Ports,
    notes: &mut Vec<String>,
) -> (bool, Vec<Evidence>) {
    let mut changed = false;
    let mut late = Vec::new();
    for f in episode.failed.clone() {
        let Some(run) = f.run_id.as_deref() else {
            continue;
        };
        // One key per failed job of a run attempt: sibling jobs that fail in
        // the same run are read independently.
        let key = format!("{run}:{}:{}", f.attempt.unwrap_or(0), f.name);
        if episode.evidence_keys.contains(&key) {
            continue;
        }
        match ports.evidence(run, &f.name) {
            Ok(mut found) => {
                changed = true;
                found.check = f.name.clone();
                if let Some(step) = found
                    .failed_steps
                    .iter()
                    .find(|s| COMPILE_STEPS.iter().any(|p| s.starts_with(p)))
                {
                    episode.unbuildable = Some(format!("`{}` failed at step `{step}`", f.name));
                }
                let had_log = episode
                    .evidence
                    .iter()
                    .any(|e| e.check == found.check && e.log_complete);
                if found.log_complete {
                    episode.evidence_keys.insert(key);
                    if episode.alerted && !had_log {
                        late.push(found.clone());
                    }
                } else {
                    notes.push(format!(
                        "red-main watch: the log of `{}` could not be read; retried next pass",
                        f.name
                    ));
                }
                episode.evidence.retain(|e| e.check != found.check);
                episode.evidence.push(found);
            }
            Err(e) => notes.push(format!(
                "red-main watch: the failure evidence of `{}` could not be read; retried next pass: {e:#}",
                f.name
            )),
        }
    }
    (changed, late)
}

/// A check name made safe for a notice key.
fn slug(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn evidence_text(episode: &Episode) -> String {
    let mut out = String::new();
    for f in &episode.failed {
        match episode.evidence.iter().find(|e| e.check == f.name) {
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

fn names(episode: &Episode) -> String {
    episode
        .failed
        .iter()
        .map(|f| f.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The `aida human` item's current text.
fn attention_body(episode: &Episode) -> String {
    let mut body = format!(
        "Main is red at {} ({}).\nFailing:\n{}{}\n",
        episode.red_sha,
        episode.subject,
        evidence_text(episode),
        held_text(episode)
    );
    match &episode.fix_spec {
        Some(id) => body.push_str(&format!(
            "Fix forward: {id}, top of the implementer queue.\n"
        )),
        None => body.push_str("The fix spec is being filed.\n"),
    }
    if !episode.recommended.is_empty() {
        body.push_str(&format!(
            "Revert recommended ({}): git revert --no-edit {}, as an ordinary reviewed PR.\n",
            episode.recommended.join(", "),
            episode.red_sha
        ));
    }
    body
}

fn attention(state: &mut State, episode: &Episode) {
    notify(
        state,
        format!("human-{}", episode.red_sha),
        Channel::HumanOpen {
            red_sha: episode.red_sha.clone(),
        },
        format!("Main is red at {}", short(&episode.red_sha)),
        attention_body(episode),
    );
}

#[allow(clippy::too_many_arguments)]
fn open(
    root: &Path,
    state: &mut State,
    commit: &Commit,
    failed: &[FailedCheck],
    now: DateTime<Utc>,
    ports: &mut dyn Ports,
    notes: &mut Vec<String>,
) -> Result<()> {
    let mut episode = Episode {
        red_sha: commit.sha.clone(),
        subject: commit.subject.clone(),
        pr: subject_pr(&commit.subject),
        specs: crate::pr_ship::extract_authorship_spec_ids_from_subject(&commit.subject),
        failed: failed.to_vec(),
        opened_at: now,
        held_green: state.last_green.clone(),
        attempts: failed
            .iter()
            .filter_map(|f| Some((f.run_id.clone()?, f.attempt?)))
            .collect(),
        rerun_requested: false,
        confirmed_at: None,
        fix_spec: None,
        evidence: Vec::new(),
        evidence_keys: BTreeSet::new(),
        alerted: false,
        unbuildable: None,
        smoke_failed: None,
        red_merges: Vec::new(),
        recommended: Vec::new(),
    };
    probe(&mut episode, ports, notes);
    episode.alerted = true;
    let body = format!(
        "Main is red at {} ({}).\nPR: {}\nSpecs: {}\nFailing:\n{}{}\n\nThe failed jobs are re-run \
         once. If the red survives that re-run or 30 minutes, a fix spec is filed at the top of \
         the implementer queue and the operator is notified. Merging continues meanwhile.",
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
        Channel::Mail,
        format!(
            "main is red at {}: {}",
            short(&episode.red_sha),
            names(&episode)
        ),
        body,
    );
    notes.push(format!(
        "red-main watch: main went red at {} ({})",
        short(&episode.red_sha),
        names(&episode)
    ));
    let start = event(now, "red_start", &episode.red_sha);
    state.episode = Some(episode);
    record(root, state, start)
}

/// Move an open episode on: the confirming re-run, the evidence, the
/// confirmation, the fix spec and the revert triggers.
fn advance(
    root: &Path,
    state: &mut State,
    window: std::time::Duration,
    now: DateTime<Utc>,
    ports: &mut dyn Ports,
    notes: &mut Vec<String>,
) -> Result<()> {
    let Some(mut episode) = state.episode.clone() else {
        return Ok(());
    };
    // One confirming re-run of the failed jobs. A re-run asked twice after a
    // crash is harmless.
    if !episode.rerun_requested {
        for run in episode.attempts.keys() {
            if let Err(e) = ports.rerun_failed(run) {
                notes.push(format!(
                    "red-main watch: CI run {run} was not re-run: {e:#}"
                ));
            }
        }
        episode.rerun_requested = true;
        state.episode = Some(episode.clone());
        write_state(root, state)?;
    }
    // Evidence (and the unbuildable trigger) is read again until it is
    // complete, and again for each new run attempt.
    // The pass that opened the episode already read the evidence.
    let (read, late) = if episode.opened_at == now {
        (false, Vec::new())
    } else {
        probe(&mut episode, ports, notes)
    };
    for found in &late {
        let body = format!(
            "The log of `{}` for main's red at {} is now read:\n{}\n{}",
            found.check,
            short(&episode.red_sha),
            found.link,
            found.excerpt
        );
        let key = format!(
            "main-red-{}-evidence-{}",
            episode.red_sha,
            slug(&found.check)
        );
        notify(
            state,
            key.clone(),
            Channel::Mail,
            format!(
                "main red at {}: log of {}",
                short(&episode.red_sha),
                found.check
            ),
            body.clone(),
        );
        if let Some(id) = &episode.fix_spec {
            notify(
                state,
                format!("{key}-comment"),
                Channel::FixComment { spec: id.clone() },
                String::new(),
                body,
            );
        }
    }
    if read {
        state.episode = Some(episode.clone());
        write_state(root, state)?;
    }
    if episode.confirmed_at.is_none() {
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
            Channel::Operator,
            "aida: main is red".into(),
            format!(
                "Main is red at {} ({}); the red survived its re-run or 30 minutes. See `aida human`.",
                short(&episode.red_sha),
                names(&episode)
            ),
        );
        attention(state, &episode);
        state.episode = Some(episode.clone());
        let mut e = event(now, "confirmed", &episode.red_sha);
        e.reason = Some(if rerun_failed_again {
            "re-run failed".into()
        } else {
            "30 minutes".into()
        });
        record(root, state, e)?;
        notes.push(format!(
            "red-main watch: the red at {} is confirmed",
            short(&episode.red_sha)
        ));
    }
    if episode.fix_spec.is_none() {
        let fix = FixSpec {
            red_sha: episode.red_sha.clone(),
            title: format!(
                "main red at {}: {}",
                short(&episode.red_sha),
                names(&episode)
            ),
            description: format!(
                "The red-main watch confirmed that main's CI is red at {} ({}).\n\nFailing:\n{}\n{}\n\n\
                 Scope: make the failing checks green on main; no unrelated change. Revert \
                 instead only if the operator or advisor recommends it.",
                episode.red_sha,
                episode.subject,
                evidence_text(&episode),
                held_text(&episode)
            ),
            fixes: episode.specs.clone(),
        };
        // Filing is idempotent per red commit and reconciles the queue entry,
        // so a failure here is retried on the next pass.
        match ports.file_fix_spec(&fix) {
            Ok(id) => {
                notes.push(format!("red-main watch: filed {id} to fix main"));
                episode.fix_spec = Some(id.clone());
                attention(state, &episode);
                state.episode = Some(episode.clone());
                let mut e = event(now, "fix_filed", &episode.red_sha);
                e.fix_spec = Some(id);
                record(root, state, e)?;
            }
            Err(e) => notes.push(format!(
                "red-main watch: the fix spec was not filed; retried next pass: {e:#}"
            )),
        }
    }
    // Revert triggers, each escalated once.
    let fix_state = match episode.fix_spec.as_deref() {
        Some(id) => ports.fix_spec_state(id).unwrap_or_else(|e| {
            notes.push(format!("red-main watch: {id} could not be read: {e:#}"));
            FixState::default()
        }),
        None => FixState::default(),
    };
    let window = Duration::from_std(window).unwrap_or(Duration::hours(2));
    let mut reasons = Vec::new();
    if let Some(why) = &episode.unbuildable {
        reasons.push(("unbuildable", format!("main does not build: {why}")));
    }
    if let Some(sha) = &episode.smoke_failed {
        reasons.push((
            "smoke-fail",
            format!("the build of {} failed the smoke test", short(sha)),
        ));
    }
    if now - episode.opened_at >= window {
        reasons.push((
            "fix-window-elapsed",
            format!(
                "no fix landed within the fix window ({} min)",
                window.num_minutes()
            ),
        ));
    }
    if fix_state.needs_design {
        reasons.push((
            "needs-design",
            "the fix needs a design decision".to_string(),
        ));
    }
    for (key, why) in reasons {
        if episode.recommended.iter().any(|r| r == key) {
            continue;
        }
        episode.recommended.push(key.into());
        let body = format!(
            "Revert recommended for main's red at {} ({}): {why}.\nRevert with an ordinary \
             reviewed PR: git revert --no-edit {}\n\nFailing:\n{}",
            episode.red_sha,
            episode.subject,
            episode.red_sha,
            evidence_text(&episode)
        );
        let notice = format!("main-red-{}-revert-{key}", episode.red_sha);
        notify(
            state,
            notice.clone(),
            Channel::Mail,
            format!("revert recommended: {why}"),
            body.clone(),
        );
        notify(
            state,
            format!("{notice}-operator"),
            Channel::Operator,
            "aida: revert recommended on main".into(),
            body,
        );
        attention(state, &episode);
        state.episode = Some(episode.clone());
        let mut e = event(now, "revert_recommended", &episode.red_sha);
        e.reason = Some(key.into());
        record(root, state, e)?;
        notes.push(format!("red-main watch: revert recommended ({key})"));
    }
    Ok(())
}

fn close(
    root: &Path,
    state: &mut State,
    green: &Commit,
    seen: &[(Commit, Verdict)],
    now: DateTime<Utc>,
    ports: &mut dyn Ports,
    notes: &mut Vec<String>,
) -> Result<()> {
    let Some(episode) = state.episode.clone() else {
        return Ok(());
    };
    let rerun_green = green.sha == episode.red_sha;
    // A revert is found in the commits that closed the episode, read on
    // their own; the CI scan may have stopped short of them.
    let revert_subject = format!("Revert \"{}", episode.subject);
    let closing = if rerun_green {
        Vec::new()
    } else {
        match ports.range(&episode.red_sha, &green.sha) {
            Ok(commits) => commits,
            Err(e) => {
                notes.push(format!(
                    "red-main watch: the closing commits could not be read; using the scanned ones: {e:#}"
                ));
                seen.iter().map(|(c, _)| c.clone()).collect()
            }
        }
    };
    let reverted = closing
        .iter()
        .any(|c| c.subject.starts_with(&revert_subject));
    let confirmed = episode.confirmed_at.is_some();
    let outcome = match (confirmed, rerun_green) {
        (true, _) if reverted => "reverted",
        (false, true) => "flake",
        (false, false) | (true, true) => "cleared",
        (true, false) => "fixed",
    };
    let minutes = (now - episode.opened_at).num_minutes();
    let mut body = format!(
        "Main is green again at {} after {minutes} min red (first red: {}). Outcome: {outcome}.",
        short(&green.sha),
        short(&episode.red_sha)
    );
    if outcome == "reverted" {
        if let Some(id) = &episode.fix_spec {
            body.push_str(&format!(
                " {id} stays open: re-scope it against the revert."
            ));
        }
    }
    notify(
        state,
        format!("main-red-{}-green", episode.red_sha),
        Channel::Mail,
        format!("main is green again at {}", short(&green.sha)),
        body.clone(),
    );
    if confirmed {
        notify(
            state,
            format!("main-red-{}-green-operator", episode.red_sha),
            Channel::Operator,
            "aida: main is green again".into(),
            body.clone(),
        );
        notify(
            state,
            format!("human-{}-resolve", episode.red_sha),
            Channel::HumanResolve {
                red_sha: episode.red_sha.clone(),
            },
            String::new(),
            body.clone(),
        );
    }
    state.episode = None;
    state.last_green = Some(green.sha.clone());
    let mut e = event(now, "red_end", &episode.red_sha);
    e.sha = Some(green.sha.clone());
    e.outcome = Some(outcome.into());
    e.fix_spec = episode.fix_spec.clone();
    record(root, state, e)?;
    notes.push(format!("red-main watch: {body}"));
    Ok(())
}

/// `aida dev build` saw the build of `sha` fail its smoke test: when `sha`
/// belongs to the open episode, record it as a revert trigger. Never waits
/// on a running pass for long: a busy watch is reported, not awaited.
pub(crate) fn note_smoke_failure(root: &Path, sha: &str) -> Result<bool> {
    if !root.join(STATE).exists() {
        return Ok(false);
    }
    let Some(_lock) = try_lock(root, std::time::Duration::ZERO)? else {
        anyhow::bail!("the red-main watch is busy; the smoke failure of {sha} was not recorded");
    };
    let mut state = read_state(root)?;
    let Some(episode) = state.episode.as_mut() else {
        return Ok(false);
    };
    if episode.red_sha != sha && !episode.red_merges.iter().any(|s| s == sha) {
        return Ok(false);
    }
    if episode.smoke_failed.is_none() {
        episode.smoke_failed = Some(sha.to_string());
        write_state(root, &state)?;
    }
    Ok(true)
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
        // Ended inside the window, or still open at its cutoff.
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

/// The production ports.
pub(crate) struct Live {
    pub root: PathBuf,
    pub forge: Box<dyn crate::forge::Forge>,
    /// The ceiling on each git or gh call.
    pub call_timeout: std::time::Duration,
}

impl Live {
    /// Run a bounded subprocess with null stdin and no terminal prompt.
    fn run(&self, mut cmd: std::process::Command, cap: u64) -> Result<Vec<u8>> {
        cmd.current_dir(&self.root)
            .stdin(std::process::Stdio::null())
            .env("GIT_TERMINAL_PROMPT", "0");
        let shown = format!("{:?}", cmd.get_program());
        match crate::command_output_with_timeout_capped(cmd, self.call_timeout, cap) {
            crate::BoundedCommandOutput::Completed(out) if out.status.success() => Ok(out.stdout),
            crate::BoundedCommandOutput::Completed(out) => anyhow::bail!(
                "{shown} failed ({}): {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ),
            crate::BoundedCommandOutput::TimedOut => {
                anyhow::bail!("{shown} timed out after {:?}", self.call_timeout)
            }
            crate::BoundedCommandOutput::SpawnFailed => anyhow::bail!("{shown} could not start"),
        }
    }

    fn git(&self, args: &[&str]) -> Result<String> {
        let mut cmd = std::process::Command::new("git");
        cmd.args(args);
        Ok(String::from_utf8_lossy(&self.run(cmd, 16 * 1024 * 1024)?).into_owned())
    }

    fn log(&self, args: &[&str]) -> Result<Vec<Commit>> {
        let mut full = vec!["log", "--first-parent", "--format=%H%x09%s"];
        full.extend_from_slice(args);
        Ok(self
            .git(&full)?
            .lines()
            .filter_map(|l| l.split_once('\t'))
            .map(|(sha, subject)| Commit {
                sha: sha.to_string(),
                subject: subject.to_string(),
            })
            .collect())
    }

    fn store(&self) -> aida_core::Storage {
        aida_core::Storage::new(
            crate::detect_distributed_store_from(&self.root)
                .unwrap_or_else(|| self.root.join(".aida-store")),
        )
    }
}

impl Ports for Live {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }

    fn history(&mut self, stop: Option<&str>, max: usize) -> Result<Vec<Commit>> {
        let branch = aida_core::git_ops::default_branch_name(&self.root);
        self.git(&["fetch", "--quiet", "origin", &branch])?;
        let tip = format!("origin/{branch}");
        let max = format!("--max-count={max}");
        let mut args = vec![max.as_str(), tip.as_str()];
        // A recorded green commit no longer in history (rewritten) is ignored.
        let exclude;
        if let Some(stop) = stop {
            if self
                .git(&["merge-base", "--is-ancestor", stop, &tip])
                .is_ok()
            {
                exclude = format!("^{stop}");
                args.push(&exclude);
            }
        }
        self.log(&args)
    }

    fn range(&mut self, from: &str, to: &str) -> Result<Vec<Commit>> {
        self.log(&[&format!("{from}..{to}")])
    }

    fn checks(&mut self, sha: &str) -> Result<CommitChecks> {
        self.forge.commit_checks(sha)
    }

    fn evidence(&mut self, run_id: &str, job: &str) -> Result<Evidence> {
        let report = self.forge.run_job_report(run_id, job)?;
        let excerpt = match &report.log {
            Ok(log) => {
                let cut = log.len().min(LOG_CAP as usize);
                let start = log.len() - cut;
                let start = (start..log.len())
                    .find(|i| log.is_char_boundary(*i))
                    .unwrap_or(log.len());
                excerpt(
                    &log[start..],
                    report.failed_steps.first().map(String::as_str),
                )
            }
            Err(e) => format!("(the job log could not be read yet: {e})"),
        };
        Ok(Evidence {
            check: job.to_string(),
            link: report.link,
            failed_steps: report.failed_steps,
            excerpt,
            log_complete: report.log.is_ok(),
        })
    }

    fn rerun_failed(&mut self, run_id: &str) -> Result<()> {
        self.forge.rerun_failed_jobs(run_id)
    }

    fn mail(&mut self, key: &str, subject: &str, body: &str) -> Result<()> {
        use aida_core::mailbox::{Intent, Message, Recipient, SenderSource};
        let existing = aida_core::mailbox::read_local_messages(&self.root)?;
        for to in ["advisor", "product"] {
            let id = format!("{key}-{to}");
            if existing.iter().any(|m| m.id == id) {
                continue;
            }
            let message = Message {
                id: id.clone(),
                thread_id: key.to_string(),
                from: "red-main-watch".into(),
                to: Recipient::Agent(to.into()),
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
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    fn notify_operator(&mut self, key: &str, title: &str, body: &str) -> Result<()> {
        crate::notify::send_direct_bounded(&self.root, key, title, body, Some(self.call_timeout))
            .map(|_| ())
    }

    fn comment_fix_spec(&mut self, id: &str, text: &str) -> Result<()> {
        comment_fix_spec(&self.store(), id, text)
    }

    fn human_open(&mut self, red_sha: &str, title: &str, body: &str) -> Result<String> {
        human_open(&self.store(), red_sha, title, body)
    }

    fn human_resolve(&mut self, red_sha: &str) -> Result<()> {
        human_resolve(&self.store(), red_sha)
    }

    fn file_fix_spec(&mut self, fix: &FixSpec) -> Result<String> {
        file_fix_spec(&self.store(), fix)
    }

    fn fix_spec_state(&mut self, id: &str) -> Result<FixState> {
        let store = self.store().load()?;
        let req = store
            .get_requirement_unambiguous(id)?
            .with_context(|| format!("{id} is not in the store"))?;
        Ok(fix_state_of(req))
    }
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

/// PURE: what a fix spec says about the episode: whether it is completed,
/// and whether its fix needs a design decision (a `needs-design` tag or
/// comment, or a lane's recorded design-fork punt).
pub(crate) fn fix_state_of(req: &aida_core::Requirement) -> FixState {
    let design_punt = req
        .attention_reason
        .as_ref()
        .is_some_and(|r| r.category == aida_core::PuntCategory::DesignFork);
    FixState {
        completed: req.status == aida_core::RequirementStatus::Completed,
        needs_design: design_punt
            || req.tags.iter().any(|t| t == "needs-design")
            || req
                .comments
                .iter()
                .any(|c| c.content.contains("#needs-design")),
    }
}

/// The tag that keys a fix spec to its red commit (the filing dedup).
pub(crate) fn fix_tag(red_sha: &str) -> String {
    format!("main-red:{}", short(red_sha))
}

/// The tag that keys the `aida human` attention item to its red commit.
pub(crate) fn human_tag(red_sha: &str) -> String {
    format!("main-red-human:{}", short(red_sha))
}

fn display(r: &aida_core::Requirement) -> String {
    r.agreed_id
        .clone()
        .or_else(|| r.spec_id.clone())
        .unwrap_or_else(|| r.id.to_string())
}

fn tagged(store: &aida_core::RequirementsStore, tag: &str) -> Option<(uuid::Uuid, String)> {
    store
        .requirements
        .iter()
        .find(|r| r.tags.iter().any(|t| t == tag))
        .map(|r| (r.id, display(r)))
}

/// Run a store effect under the store write lock without ever waiting for
/// it: another writer holding the lock fails the effect at once, and the
/// watch retries it on its next pass. The land loop and the auto-train
/// worker never wait on the store through the watch.
fn locked<T>(storage: &aida_core::Storage, f: impl FnOnce() -> Result<T>) -> Result<T> {
    aida_core::db::with_store_write_lock_try(storage.path(), f)
        .context("the requirement store is busy or not writable; retried next pass")
}

/// Add `text` to fix spec `id` as a comment, unless it already has it.
pub(crate) fn comment_fix_spec(storage: &aida_core::Storage, id: &str, text: &str) -> Result<()> {
    locked(storage, || {
        let mut store = storage.load()?;
        let author = aida_core::provenance::default_author_in(Some(storage.path()));
        let req = store
            .get_requirement_unambiguous_mut(id)?
            .with_context(|| format!("{id} is not in the store"))?;
        if req.comments.iter().any(|c| c.content == text) {
            return Ok(());
        }
        req.add_comment(aida_core::Comment::new(author, text.to_string()));
        storage.save(&store)
    })
}

/// Queue `id` at the top for `role`, unless the user already has it queued.
fn ensure_queued(
    storage: &aida_core::Storage,
    id: uuid::Uuid,
    role: &str,
    note: &str,
) -> Result<()> {
    let user = crate::current_user_id(None);
    let entries = storage.queue_list(&user, true)?;
    if entries.iter().any(|e| e.requirement_id == id) {
        return Ok(());
    }
    failpoint("queue_add")?;
    storage.queue_add(aida_core::models::QueueEntry {
        user_id: user.clone(),
        requirement_id: id,
        position: entries.first().map(|e| e.position - 1000).unwrap_or(1000),
        added_by: user,
        note: Some(note.into()),
        added_at: Utc::now(),
        for_role: Some(role.into()),
        for_scope: None,
        for_session: None,
        added_by_machine: None,
    })
}

/// File the fix spec: Approved, high priority, tagged with its red commit,
/// linked `fixes` to the red commit's specs, at the top of the implementer
/// queue. An existing spec with the same tag is reused, and its queue entry
/// is restored if a crash or a failed queue write left it out.
pub(crate) fn file_fix_spec(storage: &aida_core::Storage, fix: &FixSpec) -> Result<String> {
    locked(storage, || file_fix_spec_locked(storage, fix))
}

fn file_fix_spec_locked(storage: &aida_core::Storage, fix: &FixSpec) -> Result<String> {
    use aida_core::{Requirement, RequirementStatus, RequirementType};
    let tag = fix_tag(&fix.red_sha);
    let mut store = storage.load()?;
    let (id, shown) = match tagged(&store, &tag) {
        Some(found) => found,
        None => {
            let mut req = Requirement::new(fix.title.clone(), fix.description.clone());
            req.req_type = RequirementType::Bug;
            req.status = RequirementStatus::Approved;
            req.priority = aida_core::RequirementPriority::High;
            req.owner = aida_core::provenance::default_author_in(Some(storage.path()));
            req.tags.extend(["main-red".to_string(), tag.clone()]);
            aida_core::provenance::stamp_if_absent(&mut req);
            let prefix = store.get_type_prefix(&req.req_type);
            store.add_requirement_with_id(req, None, prefix.as_deref());
            let (id, shown) = tagged(&store, &tag).context("the store did not add the fix spec")?;
            for spec in &fix.fixes {
                if let Ok(Some(target)) = store
                    .get_requirement_unambiguous(spec)
                    .map(|r| r.map(|r| r.id))
                {
                    let _ = store.add_relationship(
                        &id,
                        aida_core::RelationshipType::Custom("fixes".into()),
                        &target,
                        false,
                    );
                }
            }
            storage.save(&store)?;
            (id, shown)
        }
    };
    ensure_queued(storage, id, "implementer", "main is red: fix forward first")?;
    Ok(shown)
}

/// Create or update the human-only attention item for `red_sha`, routed to
/// the human queue.
pub(crate) fn human_open(
    storage: &aida_core::Storage,
    red_sha: &str,
    title: &str,
    body: &str,
) -> Result<String> {
    locked(storage, || human_open_locked(storage, red_sha, title, body))
}

fn human_open_locked(
    storage: &aida_core::Storage,
    red_sha: &str,
    title: &str,
    body: &str,
) -> Result<String> {
    use aida_core::{Requirement, RequirementStatus, RequirementType};
    let tag = human_tag(red_sha);
    let mut store = storage.load()?;
    match store
        .requirements
        .iter_mut()
        .find(|r| r.tags.contains(&tag))
    {
        Some(existing) => {
            if existing.description != body {
                existing.description = body.to_string();
                existing.modified_at = Utc::now();
                storage.save(&store)?;
            }
        }
        None => {
            let mut req = Requirement::new(title.to_string(), body.to_string());
            req.req_type = RequirementType::Task;
            req.status = RequirementStatus::Approved;
            req.priority = aida_core::RequirementPriority::High;
            req.human_only = true;
            req.owner = aida_core::provenance::default_author_in(Some(storage.path()));
            req.tags.extend(["main-red".to_string(), tag.clone()]);
            aida_core::provenance::stamp_if_absent(&mut req);
            let prefix = store.get_type_prefix(&req.req_type);
            store.add_requirement_with_id(req, None, prefix.as_deref());
            storage.save(&store)?;
        }
    }
    let (id, shown) = tagged(&store, &tag).context("the attention item is not in the store")?;
    ensure_queued(storage, id, "human", "main is red")?;
    Ok(shown)
}

/// Complete the attention item for `red_sha` and take it off the queue.
pub(crate) fn human_resolve(storage: &aida_core::Storage, red_sha: &str) -> Result<()> {
    locked(storage, || human_resolve_locked(storage, red_sha))
}

fn human_resolve_locked(storage: &aida_core::Storage, red_sha: &str) -> Result<()> {
    let tag = human_tag(red_sha);
    let mut store = storage.load()?;
    let Some(item) = store
        .requirements
        .iter_mut()
        .find(|r| r.tags.contains(&tag))
    else {
        return Ok(());
    };
    let id = item.id;
    if item.status != aida_core::RequirementStatus::Completed {
        item.status = aida_core::RequirementStatus::Completed;
        item.modified_at = Utc::now();
        storage.save(&store)?;
    }
    let user = crate::current_user_id(None);
    if storage
        .queue_list(&user, true)?
        .iter()
        .any(|e| e.requirement_id == id)
    {
        storage.queue_remove(&user, &id)?;
    }
    Ok(())
}

/// One pass with the production ports, for `aida land watch` (run by hand or
/// by its own scheduled job; no landing path calls it), bounded by
/// [`PASS_BUDGET`].
pub(crate) fn live_pass(root: &Path) -> Result<Vec<String>> {
    crate::forge_lookup::with_forge_lookup_deadline(PASS_BUDGET, || {
        let policy = crate::landing_policy::load(root);
        let mut ports = Live {
            root: root.to_path_buf(),
            forge: crate::forge::forge_for(root),
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
