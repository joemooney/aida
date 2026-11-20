//! The red-main watch (STORY-2897 S3): notices when the default branch's CI
//! goes red after a merge, routes a fix forward, and recommends a revert only
//! on the fixed triggers. It is observational: no merge decision reads it,
//! and merging continues while main is red (operator direction 2026-11-18).
//!
//! An episode opens on the first red commit after the last green one. Its
//! opening is journaled and mailed (urgent, advisor and product) at once, and
//! the failed jobs are re-run once. The fix spec and the operator notice wait
//! until the red is confirmed: it survives that re-run, or 30 minutes pass.
//! A red that a green re-run clears first is a flake: no spec, no notice.
//!
//! State: `.aida/land/main-watch.json` (one episode at most), journal:
//! `.aida/land/main-watch.jsonl` (append-only, read by `aida land metrics`).
// trace:STORY-2897 | ai:claude

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

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

/// One default-branch commit, newest first in a history read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Commit {
    pub sha: String,
    pub subject: String,
}

/// What the watch reads and does outside the repository's state files.
/// [`Live`] is the real one; tests use a fake.
pub(crate) trait Ports {
    fn now(&self) -> DateTime<Utc>;
    /// First-parent commits of the freshly fetched default branch, newest
    /// first, stopping before `stop` (exclusive) and after `max`.
    fn history(&mut self, stop: Option<&str>, max: usize) -> Result<Vec<Commit>>;
    fn checks(&mut self, sha: &str) -> Result<CommitChecks>;
    fn failed_steps(&mut self, run_id: &str, job: &str) -> Result<Vec<String>>;
    fn rerun_failed(&mut self, run_id: &str) -> Result<()>;
    /// Urgent mail to the advisor and product seats; idempotent by `key`.
    fn mail(&mut self, key: &str, subject: &str, body: &str) -> Result<()>;
    /// The operator's direct notice; idempotent by `key`.
    fn notify_operator(&mut self, key: &str, title: &str, body: &str) -> Result<()>;
    /// File the fix spec (once per red commit) and queue it at the top of
    /// the implementer queue; returns its id.
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Episode {
    pub red_sha: String,
    pub subject: String,
    pub pr: Option<u64>,
    pub specs: Vec<String>,
    /// Required checks failing on the red commit when the episode opened.
    pub failing: Vec<String>,
    pub opened_at: DateTime<Utc>,
    /// Owning CI run id -> its attempt when the episode opened.
    pub attempts: BTreeMap<String, u64>,
    pub rerun_requested: bool,
    pub confirmed_at: Option<DateTime<Utc>>,
    pub fix_spec: Option<String>,
    /// `Some(why)` when a failing check failed at a compile step.
    pub unbuildable: Option<String>,
    /// The episode commit whose staged build failed the smoke test.
    pub smoke_failed: Option<String>,
    /// Later red commits (merges that landed while red).
    pub red_merges: Vec<String>,
    /// Revert reasons already escalated (one escalation per reason).
    pub recommended: Vec<String>,
}

/// One journal line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Event {
    pub at: DateTime<Utc>,
    /// `red_start`, `red_merge`, `confirmed`, `revert_recommended` or `red_end`.
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
    Red(Vec<Failed>),
    /// Pending, cancelled-only, missing or unreadable: never a transition.
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Failed {
    pub name: String,
    pub run_id: Option<String>,
    pub attempt: Option<u64>,
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
    let failed: Vec<Failed> = latest
        .iter()
        .filter(|c| c.outcome == CheckOutcome::Failed && !c.cancelled)
        .map(|c| Failed {
            name: c.name.clone(),
            run_id: c.run_id.clone(),
            attempt: c.run_attempt,
        })
        .collect();
    // A required check that failed makes the commit red; any other failed
    // check is reported beside it but alone it is not red.
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

fn read_state(root: &Path) -> Result<State> {
    match std::fs::read(root.join(STATE)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("{STATE} is not valid; fix or remove it")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
        Err(e) => Err(e).with_context(|| format!("reading {STATE}")),
    }
}

fn write_state(root: &Path, state: &State) -> Result<()> {
    let path = root.join(STATE);
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, serde_json::to_vec_pretty(state)?)?;
    std::fs::rename(&temp, &path).with_context(|| format!("writing {STATE}"))
}

fn append(root: &Path, event: &Event) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join(JOURNAL))
        .with_context(|| format!("opening {JOURNAL}"))?;
    let mut line = serde_json::to_vec(event)?;
    line.push(b'\n');
    file.write_all(&line)?;
    file.sync_data()?;
    Ok(())
}

/// Every journal event, oldest first.
pub(crate) fn read_journal(root: &Path) -> Result<Vec<Event>> {
    match std::fs::read_to_string(root.join(JOURNAL)) {
        Ok(text) => text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).with_context(|| format!("invalid line in {JOURNAL}")))
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
}

struct WatchLock(std::fs::File);
impl Drop for WatchLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
    }
}

fn event(at: DateTime<Utc>, kind: &str, red_sha: &str) -> Event {
    Event {
        at,
        event: kind.into(),
        red_sha: red_sha.into(),
        sha: None,
        outcome: None,
        reason: None,
        fix_spec: None,
    }
}

/// One watch pass. `required` is the CI-gated check set, `window` the fix
/// window. Returns what it observed and did, one line each. Only one pass
/// runs at a time; a second one returns at once.
pub(crate) fn pass(
    root: &Path,
    required: &[String],
    window: std::time::Duration,
    ports: &mut dyn Ports,
) -> Result<Vec<String>> {
    use fs2::FileExt;
    let dir = root.join(".aida/land");
    std::fs::create_dir_all(&dir)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(root.join(LOCK))?;
    if lock.try_lock_exclusive().is_err() {
        return Ok(vec!["red-main watch: another pass is running".into()]);
    }
    let _lock = WatchLock(lock);
    let mut state = read_state(root)?;
    let now = ports.now();
    let mut notes = Vec::new();

    // Newest first, until the first green commit (older history no longer
    // decides anything) or past the open episode's red commit.
    let commits = ports.history(state.last_green.as_deref(), SEARCH)?;
    let mut seen: Vec<(Commit, Verdict)> = Vec::new();
    for commit in commits {
        let verdict = judge(ports.checks(&commit.sha), required);
        let stop = verdict == Verdict::Green
            || state
                .episode
                .as_ref()
                .is_some_and(|e| e.red_sha == commit.sha);
        seen.push((commit, verdict));
        if stop {
            break;
        }
    }
    let newest = seen
        .iter()
        .position(|(_, v)| !matches!(v, Verdict::Unknown(_)));

    match newest.map(|i| &seen[i]) {
        Some((green, Verdict::Green)) => {
            if let Some(episode) = state.episode.take() {
                close(root, episode, &green.sha, &seen, now, ports, &mut notes)?;
            }
            state.last_green = Some(green.sha.clone());
            write_state(root, &state)?;
        }
        Some((_, Verdict::Red(_))) => {
            let start = newest.unwrap_or(0);
            // The oldest red commit after the newest green one.
            let reds: Vec<&(Commit, Verdict)> = seen[start..]
                .iter()
                .take_while(|(_, v)| *v != Verdict::Green)
                .filter(|(_, v)| matches!(v, Verdict::Red(_)))
                .collect();
            match state.episode.as_mut() {
                None => {
                    let (commit, verdict) = reds.last().copied().unwrap_or(&seen[start]);
                    let episode = open(root, commit, verdict, now, ports, &mut notes)?;
                    state.episode = Some(episode);
                    write_state(root, &state)?;
                }
                Some(episode) => {
                    for (commit, _) in reds.iter().rev() {
                        if commit.sha != episode.red_sha
                            && !episode.red_merges.contains(&commit.sha)
                        {
                            episode.red_merges.push(commit.sha.clone());
                            let mut e = event(now, "red_merge", &episode.red_sha);
                            e.sha = Some(commit.sha.clone());
                            append(root, &e)?;
                        }
                    }
                    write_state(root, &state)?;
                }
            }
        }
        _ => {
            let why = seen
                .first()
                .map(|(c, v)| match v {
                    Verdict::Unknown(why) => format!("{}: {why}", short(&c.sha)),
                    _ => String::new(),
                })
                .unwrap_or_else(|| "no new commits".into());
            notes.push(format!("red-main watch: main CI unknown ({why})"));
        }
    }

    let red_now = state.episode.as_ref().and_then(|episode| {
        seen.iter()
            .find(|(c, _)| c.sha == episode.red_sha)
            .map(|(_, v)| v.clone())
    });
    advance(
        root,
        &mut state,
        red_now.as_ref(),
        window,
        now,
        ports,
        &mut notes,
    )?;
    Ok(notes)
}

fn open(
    root: &Path,
    commit: &Commit,
    verdict: &Verdict,
    now: DateTime<Utc>,
    ports: &mut dyn Ports,
    notes: &mut Vec<String>,
) -> Result<Episode> {
    let failed = match verdict {
        Verdict::Red(failed) => failed.clone(),
        _ => Vec::new(),
    };
    let mut unbuildable = None;
    for f in &failed {
        let Some(run) = f.run_id.as_deref() else {
            continue;
        };
        match ports.failed_steps(run, &f.name) {
            Ok(steps) => {
                if let Some(step) = steps
                    .iter()
                    .find(|s| COMPILE_STEPS.iter().any(|p| s.starts_with(p)))
                {
                    unbuildable = Some(format!("`{}` failed at step `{step}`", f.name));
                    break;
                }
            }
            Err(e) => notes.push(format!(
                "red-main watch: the steps of `{}` could not be read: {e:#}",
                f.name
            )),
        }
    }
    let episode = Episode {
        red_sha: commit.sha.clone(),
        subject: commit.subject.clone(),
        pr: subject_pr(&commit.subject),
        specs: crate::pr_ship::extract_authorship_spec_ids_from_subject(&commit.subject),
        failing: failed.iter().map(|f| f.name.clone()).collect(),
        opened_at: now,
        attempts: failed
            .iter()
            .filter_map(|f| Some((f.run_id.clone()?, f.attempt?)))
            .collect(),
        rerun_requested: false,
        confirmed_at: None,
        fix_spec: None,
        unbuildable,
        smoke_failed: None,
        red_merges: Vec::new(),
        recommended: Vec::new(),
    };
    let body = format!(
        "Main is red at {} ({}).\nPR: {}\nSpecs: {}\nFailing: {}\n\nThe failed jobs are re-run once. \
         If the red survives that re-run or 30 minutes, a fix spec is filed at the top of the \
         implementer queue and the operator is notified. Merging continues meanwhile.",
        episode.red_sha,
        episode.subject,
        episode.pr.map_or("unknown".into(), |n| format!("#{n}")),
        if episode.specs.is_empty() { "none named".into() } else { episode.specs.join(", ") },
        episode.failing.join(", "),
    );
    ports.mail(
        &format!("main-red-{}", episode.red_sha),
        &format!(
            "main is red at {}: {}",
            short(&episode.red_sha),
            episode.failing.join(", ")
        ),
        &body,
    )?;
    // Journaled only once the alert is out: a failed mail leaves nothing
    // recorded, and the next pass opens (and mails, idempotently) again.
    append(root, &event(now, "red_start", &episode.red_sha))?;
    notes.push(format!(
        "red-main watch: main went red at {} ({})",
        short(&episode.red_sha),
        episode.failing.join(", ")
    ));
    Ok(episode)
}

/// Move an open episode on: the confirming re-run, confirmation, the fix
/// spec and the revert triggers. Each step is persisted before the next
/// external effect, and a failed notice or filing is retried on a later pass
/// rather than undoing the steps already taken.
fn advance(
    root: &Path,
    state: &mut State,
    red_now: Option<&Verdict>,
    window: std::time::Duration,
    now: DateTime<Utc>,
    ports: &mut dyn Ports,
    notes: &mut Vec<String>,
) -> Result<()> {
    let Some(mut episode) = state.episode.clone() else {
        return Ok(());
    };
    let persist = |state: &mut State, episode: &Episode| {
        state.episode = Some(episode.clone());
        write_state(root, state)
    };
    // One confirming re-run of the failed jobs.
    if !episode.rerun_requested {
        for run in episode.attempts.keys() {
            if let Err(e) = ports.rerun_failed(run) {
                notes.push(format!(
                    "red-main watch: CI run {run} was not re-run: {e:#}"
                ));
            }
        }
        episode.rerun_requested = true;
        persist(state, &episode)?;
    }
    if episode.confirmed_at.is_none() {
        let rerun_failed_again = matches!(red_now, Some(Verdict::Red(failed)) if failed.iter().any(|f| {
            match (f.run_id.as_ref().and_then(|r| episode.attempts.get(r)), f.attempt) {
                (Some(before), Some(now)) => now > *before,
                _ => false,
            }
        }));
        let aged = now - episode.opened_at >= Duration::minutes(CONFIRM_AFTER_MINUTES);
        if !(rerun_failed_again || aged) {
            return Ok(());
        }
        episode.confirmed_at = Some(now);
        persist(state, &episode)?;
        let mut e = event(now, "confirmed", &episode.red_sha);
        e.reason = Some(if rerun_failed_again {
            "re-run failed".into()
        } else {
            "30 minutes".into()
        });
        append(root, &e)?;
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
                episode.failing.join(", ")
            ),
            description: format!(
                "The red-main watch confirmed that main's CI is red at {} ({}).\n\
                 Failing: {}\n\nScope: make the failing checks green on main; no unrelated \
                 change. Revert instead only if the operator or advisor recommends it.",
                episode.red_sha,
                episode.subject,
                episode.failing.join(", ")
            ),
            fixes: episode.specs.clone(),
        };
        // Filing is idempotent per red commit (its tag), so a failure here
        // is retried on the next pass.
        match ports.file_fix_spec(&fix) {
            Ok(id) => {
                notes.push(format!("red-main watch: filed {id} to fix main"));
                episode.fix_spec = Some(id.clone());
                persist(state, &episode)?;
                let mut e = event(now, "fix_filed", &episode.red_sha);
                e.fix_spec = Some(id.clone());
                append(root, &e)?;
                if let Err(e) = ports.notify_operator(
                    &format!("main-red-{}", episode.red_sha),
                    "aida: main is red",
                    &format!(
                        "Main is red at {} ({}); fix spec {id} is at the top of the implementer queue.",
                        short(&episode.red_sha),
                        episode.failing.join(", ")
                    ),
                ) {
                    notes.push(format!("red-main watch: the operator was not notified: {e:#}"));
                }
            }
            Err(e) => notes.push(format!(
                "red-main watch: the fix spec was not filed; the next pass retries: {e:#}"
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
    let window_chrono = Duration::from_std(window).unwrap_or(Duration::hours(2));
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
    if now - episode.opened_at >= window_chrono {
        reasons.push((
            "fix-window-elapsed",
            format!(
                "no fix landed within the fix window ({} min)",
                window_chrono.num_minutes()
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
        let body = format!(
            "Revert recommended for main's red at {} ({}): {why}.\nRevert with an ordinary \
             reviewed PR: git revert --no-edit {}",
            episode.red_sha, episode.subject, episode.red_sha
        );
        let mail_key = format!("main-red-{}-revert-{key}", episode.red_sha);
        // Not recorded as escalated until the mail is out: a failed mail is
        // retried on the next pass.
        if let Err(e) = ports.mail(&mail_key, &format!("revert recommended: {why}"), &body) {
            notes.push(format!(
                "red-main watch: the revert recommendation was not mailed: {e:#}"
            ));
            continue;
        }
        if let Err(e) = ports.notify_operator(&mail_key, "aida: revert recommended on main", &body)
        {
            notes.push(format!(
                "red-main watch: the operator was not notified: {e:#}"
            ));
        }
        episode.recommended.push(key.into());
        persist(state, &episode)?;
        let mut e = event(now, "revert_recommended", &episode.red_sha);
        e.reason = Some(key.into());
        append(root, &e)?;
        notes.push(format!("red-main watch: revert recommended ({key})"));
    }
    persist(state, &episode)
}

fn close(
    root: &Path,
    episode: Episode,
    green: &str,
    seen: &[(Commit, Verdict)],
    now: DateTime<Utc>,
    ports: &mut dyn Ports,
    notes: &mut Vec<String>,
) -> Result<()> {
    let red_sha_green = seen
        .iter()
        .any(|(c, v)| c.sha == episode.red_sha && *v == Verdict::Green);
    let revert_subject = format!("Revert \"{}", episode.subject);
    let reverted = seen
        .iter()
        .any(|(c, _)| c.subject.starts_with(&revert_subject));
    let outcome = if episode.confirmed_at.is_none() {
        if red_sha_green {
            "flake"
        } else {
            "cleared"
        }
    } else if reverted {
        "reverted"
    } else {
        "fixed"
    };
    let minutes = (now - episode.opened_at).num_minutes();
    let mut body = format!(
        "Main is green again at {} after {minutes} min red (first red: {}). Outcome: {outcome}.",
        short(green),
        short(&episode.red_sha)
    );
    if outcome == "reverted" {
        if let Some(id) = &episode.fix_spec {
            body.push_str(&format!(
                " {id} stays open: re-scope it against the revert."
            ));
        }
    }
    ports.mail(
        &format!("main-red-{}-green", episode.red_sha),
        &format!("main is green again at {}", short(green)),
        &body,
    )?;
    // Journaled once the mail is out, so a failed mail retries the close.
    let mut e = event(now, "red_end", &episode.red_sha);
    e.sha = Some(green.to_string());
    e.outcome = Some(outcome.into());
    e.fix_spec = episode.fix_spec.clone();
    append(root, &e)?;
    if episode.fix_spec.is_some() {
        if let Err(e) = ports.notify_operator(
            &format!("main-red-{}-green", episode.red_sha),
            "aida: main is green again",
            &body,
        ) {
            notes.push(format!(
                "red-main watch: the operator was not notified: {e:#}"
            ));
        }
    }
    notes.push(format!("red-main watch: {body}"));
    Ok(())
}

/// `aida dev build` saw the build of `sha` fail its smoke test: when `sha`
/// belongs to the open episode, record it as a revert trigger.
pub(crate) fn note_smoke_failure(root: &Path, sha: &str) -> Result<bool> {
    use fs2::FileExt;
    if !root.join(STATE).exists() {
        return Ok(false);
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(root.join(LOCK))?;
    lock.lock_exclusive()?;
    let _lock = WatchLock(lock);
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

/// The red-main block of `aida land metrics` for `[since, until)`.
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
        merges: usize,
    }
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
            "red_merge" => slot.merges += 1,
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
        let end = ep.end.unwrap_or(until);
        if end <= since || start >= until {
            continue;
        }
        count += 1;
        red_seconds += (end.min(until) - start.max(since)).num_seconds().max(0);
        if ep.outcome.as_deref() == Some("flake") {
            flakes += 1;
            continue;
        }
        red_merges += 1 + ep.merges;
        if let (Some(end), Some(outcome)) = (ep.end, ep.outcome.as_deref()) {
            latencies.push((end - start).num_seconds());
            resolved += 1;
            if outcome == "reverted" {
                reverted += 1;
            }
        }
    }
    latencies.sort_unstable();
    let median = (!latencies.is_empty()).then(|| latencies[latencies.len() / 2]);
    serde_json::json!({
        "source": "red-main watch journal",
        "episodes": count,
        "flakes": flakes,
        "red_minutes": red_seconds / 60,
        "red_merges": red_merges,
        "fix_latency_seconds_median": median,
        "fix_latency_seconds_max": latencies.last(),
        "reverted": reverted,
        "resolved": resolved,
    })
}

/// The production ports.
pub(crate) struct Live {
    pub root: std::path::PathBuf,
    pub forge: Box<dyn crate::forge::Forge>,
}

impl Ports for Live {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }

    fn history(&mut self, stop: Option<&str>, max: usize) -> Result<Vec<Commit>> {
        let branch = aida_core::git_ops::default_branch_name(&self.root);
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(&self.root)
                .args(args)
                .output()
        };
        let fetched = git(&["fetch", "--quiet", "origin", &branch]).context("git fetch")?;
        anyhow::ensure!(
            fetched.status.success(),
            "fetching origin/{branch} failed: {}",
            String::from_utf8_lossy(&fetched.stderr).trim()
        );
        let tip = format!("origin/{branch}");
        let max = format!("--max-count={max}");
        let mut args = vec![
            "log",
            "--first-parent",
            &max,
            "--format=%H%x09%s",
            tip.as_str(),
        ];
        // A recorded green commit no longer in history (rewritten) is ignored.
        let exclude;
        if let Some(stop) = stop {
            let known =
                git(&["merge-base", "--is-ancestor", stop, &tip]).context("git merge-base")?;
            if known.status.success() {
                exclude = format!("^{stop}");
                args.push(&exclude);
            }
        }
        let out = git(&args).context("git log")?;
        anyhow::ensure!(
            out.status.success(),
            "reading {tip} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| l.split_once('\t'))
            .map(|(sha, subject)| Commit {
                sha: sha.to_string(),
                subject: subject.to_string(),
            })
            .collect())
    }

    fn checks(&mut self, sha: &str) -> Result<CommitChecks> {
        self.forge.commit_checks(sha)
    }

    fn failed_steps(&mut self, run_id: &str, job: &str) -> Result<Vec<String>> {
        self.forge.run_job_failed_steps(run_id, job)
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
        crate::notify::send_direct_bounded(
            &self.root,
            key,
            title,
            body,
            Some(std::time::Duration::from_secs(10)),
        )
        .map(|_| ())
    }

    fn file_fix_spec(&mut self, fix: &FixSpec) -> Result<String> {
        file_fix_spec(&self.root, fix)
    }

    fn fix_spec_state(&mut self, id: &str) -> Result<FixState> {
        let store = crate::detect_distributed_store_from(&self.root)
            .unwrap_or_else(|| self.root.join(".aida-store"));
        let store = aida_core::Storage::new(store).load()?;
        let req = store
            .get_requirement_unambiguous(id)?
            .with_context(|| format!("{id} is not in the store"))?;
        Ok(FixState {
            completed: req.status == aida_core::RequirementStatus::Completed,
            needs_design: req.tags.iter().any(|t| t == "needs-design")
                || req
                    .comments
                    .iter()
                    .any(|c| c.content.contains("#needs-design")),
        })
    }
}

/// The tag that keys a fix spec to its red commit (the filing dedup).
pub(crate) fn fix_tag(red_sha: &str) -> String {
    format!("main-red:{}", short(red_sha))
}

/// File the fix spec: Approved, high priority, tagged with its red commit,
/// linked `fixes` to the red commit's specs, at the top of the implementer
/// queue. An existing spec with the same tag is returned instead, so a pass
/// that crashed after filing never files twice.
fn file_fix_spec(root: &Path, fix: &FixSpec) -> Result<String> {
    use aida_core::{Requirement, RequirementStatus, RequirementType};
    let store_path =
        crate::detect_distributed_store_from(root).unwrap_or_else(|| root.join(".aida-store"));
    let storage = aida_core::Storage::new(store_path);
    let tag = fix_tag(&fix.red_sha);
    let mut store = storage.load()?;
    let display = |r: &Requirement| {
        r.agreed_id
            .clone()
            .or_else(|| r.spec_id.clone())
            .unwrap_or_else(|| r.id.to_string())
    };
    if let Some(existing) = store.requirements.iter().find(|r| r.tags.contains(&tag)) {
        return Ok(display(existing));
    }
    let mut req = Requirement::new(fix.title.clone(), fix.description.clone());
    req.req_type = RequirementType::Bug;
    req.status = RequirementStatus::Approved;
    req.priority = aida_core::RequirementPriority::High;
    req.owner = crate::get_default_author();
    req.tags.extend(["main-red".to_string(), tag]);
    aida_core::provenance::stamp_if_absent(&mut req);
    let prefix = store.get_type_prefix(&req.req_type);
    store.add_requirement_with_id(req, None, prefix.as_deref());
    let new = store
        .requirements
        .last()
        .context("the store did not add the fix spec")?;
    let (new_id, shown) = (new.id, display(new));
    for spec in &fix.fixes {
        if let Ok(Some(target)) = store
            .get_requirement_unambiguous(spec)
            .map(|r| r.map(|r| r.id))
        {
            let _ = store.add_relationship(
                &new_id,
                aida_core::RelationshipType::Custom("fixes".into()),
                &target,
                false,
            );
        }
    }
    storage.save(&store)?;
    let user = crate::current_user_id(None);
    let top = storage
        .queue_list(&user, true)?
        .first()
        .map(|e| e.position - 1000)
        .unwrap_or(1000);
    storage.queue_add(aida_core::models::QueueEntry {
        user_id: user.clone(),
        requirement_id: new_id,
        position: top,
        added_by: user,
        note: Some("main is red: fix forward first".into()),
        added_at: Utc::now(),
        for_role: Some("implementer".into()),
        for_scope: None,
        for_session: None,
        added_by_machine: None,
    })?;
    Ok(shown)
}

/// One pass with the production ports, for the land loop, the auto-train
/// worker and `aida land watch`.
pub(crate) fn live_pass(root: &Path) -> Result<Vec<String>> {
    let policy = crate::landing_policy::load(root);
    let mut ports = Live {
        root: root.to_path_buf(),
        forge: crate::forge::forge_for(root),
    };
    pass(
        root,
        &policy.ci_gated_checks,
        policy.red_main_fix_window,
        &mut ports,
    )
}

#[cfg(test)]
#[path = "tests/story_2897_main_watch_tests.rs"]
mod tests;
