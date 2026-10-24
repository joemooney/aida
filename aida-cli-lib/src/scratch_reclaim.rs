//! TASK-2325 — reclaim agent scratch OUTSIDE the repository's `target/`
//! caches. On 2026-10-24 root hit 95% while `aida worktree reclaim` saw only
//! worktree caches; the rest was scratch no scan covered: per-job TMPDIR
//! build output under `~/.claude/jobs/<id>/tmp` (127 GiB), old session
//! scratch under the agent TMPDIR root (74 GiB), and an idle `aida` build
//! cache (23 GiB).
//!
//! Three row kinds, each with its own fail-closed rails:
//!
//! - **Job tmp** (`~/.claude/jobs/<id>/tmp`): listed only when the job's
//!   `state.json` parses to a TERMINAL state and nothing in the job dir was
//!   written inside the age window. Only the `tmp/` subdirectory is ever a
//!   row: `state.json`, `timeline.jsonl` and `~/.claude/projects` transcripts
//!   are the intent-harvesting source and are untouchable by construction.
//!   A live job (non-terminal, unreadable state, or recently updated) is
//!   never listed.
//! - **Session scratch** (children of the agent TMPDIR root, e.g.
//!   `/tmp/claude-1000/*`): an entry older than the age window. A project
//!   slug directory (its name starts with `-`) is never a row itself; its
//!   per-session children are the entries. The calling session's own scratch
//!   dir is always excluded, and age is the newest mtime anywhere in the
//!   tree, so a session still writing is never listed.
//! - **Build cache** (named directories such as `~/.cache/aida-impl-target`):
//!   listed when nothing in the tree was written inside the age window.
//!
//! The planner is pure over an injected [`ScratchSurvey`]; `apply` re-checks
//! every row against the survey's roots at the delete site, exactly as
//! worktree reclaim re-checks rail 2.
// trace:TASK-2325 | ai:claude

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::disk_emergency::{idle_for, walk_cache};

/// Scratch entries younger than this many days are never listed.
pub(crate) const DEFAULT_MIN_AGE_DAYS: u64 = 3;

/// The job states after which a job's scratch is reclaimable. Everything
/// else — `working`, `blocked`, an unknown or unreadable state — is live.
pub(crate) fn job_state_terminal(state: &str) -> bool {
    matches!(state, "done" | "failed" | "stopped" | "cancelled")
}

/// What kind of scratch a row is. The code is the WHY column and the
/// `--json` token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScratchKind {
    JobTmp,
    SessionScratch,
    BuildCache,
}

impl ScratchKind {
    pub(crate) fn code(self) -> &'static str {
        match self {
            ScratchKind::JobTmp => "job-tmp",
            ScratchKind::SessionScratch => "session-scratch",
            ScratchKind::BuildCache => "build-cache",
        }
    }
}

/// One deletable scratch entry: its size and the reason it qualifies.
#[derive(Debug, Clone)]
pub(crate) struct ScratchRow {
    pub path: PathBuf,
    pub bytes: u64,
    pub kind: ScratchKind,
    pub reason: String,
    /// How long the entry has gone unwritten, for the emergency ledger.
    pub idle: Duration,
}

/// Everything the planner needs, injected so tests run on fixture dirs.
#[derive(Debug, Clone)]
pub(crate) struct ScratchSurvey {
    /// `~/.claude/jobs`, when it exists.
    pub jobs_root: Option<PathBuf>,
    /// The agent TMPDIR root (e.g. `/tmp/claude-1000`), when it exists.
    pub tmp_root: Option<PathBuf>,
    /// Named build-cache directories (e.g. `~/.cache/aida-impl-target`).
    pub build_caches: Vec<PathBuf>,
    /// Directories that are live session scratch and must never be listed —
    /// at minimum the calling session's own TMPDIR entry.
    pub live_session_dirs: BTreeSet<PathBuf>,
    pub min_age: Duration,
    pub now: SystemTime,
}

impl ScratchSurvey {
    /// A survey with nothing to scan; `plan` on it returns nothing.
    pub(crate) fn empty(now: SystemTime) -> Self {
        Self {
            jobs_root: None,
            tmp_root: None,
            build_caches: Vec::new(),
            live_session_dirs: BTreeSet::new(),
            min_age: Duration::from_secs(DEFAULT_MIN_AGE_DAYS * 24 * 3600),
            now,
        }
    }
}

fn days(d: Duration) -> u64 {
    d.as_secs() / (24 * 3600)
}

/// The newest write anywhere under `path` (the walk also prices the row).
/// A plain file prices as its own length — the walk only sums directory
/// entries (PR-2906 round 1).
fn aged(survey: &ScratchSurvey, path: &Path) -> Option<(u64, Duration)> {
    let md = std::fs::symlink_metadata(path).ok()?;
    if md.file_type().is_symlink() {
        return None;
    }
    let walk = walk_cache(path);
    let idle = idle_for(&walk, survey.now)?;
    let bytes = if md.is_file() { md.len() } else { walk.bytes };
    (idle >= survey.min_age).then_some((bytes, idle))
}

/// `(a)` — terminal jobs' `tmp/` dirs. Held rows say why a job with scratch
/// stays: the operator under disk pressure needs the reason, not a silence.
fn plan_jobs(
    survey: &ScratchSurvey,
    rows: &mut Vec<ScratchRow>,
    held: &mut Vec<(PathBuf, String)>,
) {
    let Some(jobs_root) = &survey.jobs_root else {
        return;
    };
    let entries = match std::fs::read_dir(jobs_root) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let job = entry.path();
        let tmp = job.join("tmp");
        match std::fs::symlink_metadata(&tmp) {
            Ok(m) if m.is_dir() && !m.file_type().is_symlink() => {}
            // No scratch (or a symlink posing as it): nothing to say.
            _ => continue,
        }
        let state = std::fs::read_to_string(job.join("state.json"))
            .ok()
            .and_then(|body| serde_json::from_str::<serde_json::Value>(&body).ok())
            .and_then(|v| v.get("state").and_then(|s| s.as_str()).map(str::to_owned));
        let Some(state) = state else {
            held.push((
                tmp,
                "job state unreadable — a live job is never listed".into(),
            ));
            continue;
        };
        if !job_state_terminal(&state) {
            held.push((tmp, format!("job state `{state}` is not terminal")));
            continue;
        }
        // Age over the WHOLE job dir: a terminal job whose state.json or
        // timeline was just written is still settling.
        let Some((_, job_idle)) = aged(survey, &job) else {
            held.push((tmp, "job updated inside the age window".into()));
            continue;
        };
        let bytes = walk_cache(&tmp).bytes;
        rows.push(ScratchRow {
            path: tmp,
            bytes,
            kind: ScratchKind::JobTmp,
            reason: format!("terminal job ({state}), idle {} d", days(job_idle)),
            idle: job_idle,
        });
    }
}

/// `(b)` — aged entries under the TMPDIR root. Entries inside the age window
/// are silently kept (they are the common case, not news); live session dirs
/// are held with the reason.
fn plan_tmp(survey: &ScratchSurvey, rows: &mut Vec<ScratchRow>, held: &mut Vec<(PathBuf, String)>) {
    let Some(tmp_root) = &survey.tmp_root else {
        return;
    };
    let top = match std::fs::read_dir(tmp_root) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    let mut entries: Vec<PathBuf> = Vec::new();
    for entry in top.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        if ft.is_symlink() {
            continue;
        }
        // A project-slug dir holds per-session scratch: its children are the
        // entries, so one old session never pins a whole project's scratch.
        let slug_dir = entry
            .file_name()
            .to_str()
            .is_some_and(|n| n.starts_with('-'))
            && ft.is_dir();
        if slug_dir {
            entries.extend(
                std::fs::read_dir(&path)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .filter(|e| e.file_type().is_ok_and(|t| !t.is_symlink()))
                    .map(|e| e.path()),
            );
        } else {
            entries.push(path);
        }
    }
    for path in entries {
        if survey.live_session_dirs.contains(&path) {
            held.push((path, "live session scratch (this session)".into()));
            continue;
        }
        let Some((bytes, idle)) = aged(survey, &path) else {
            continue;
        };
        rows.push(ScratchRow {
            path,
            bytes,
            kind: ScratchKind::SessionScratch,
            reason: format!("session scratch idle {} d", days(idle)),
            idle,
        });
    }
}

/// `(c)` — named build caches not written in the age window.
fn plan_build_caches(survey: &ScratchSurvey, rows: &mut Vec<ScratchRow>) {
    for cache in &survey.build_caches {
        match std::fs::symlink_metadata(cache) {
            Ok(m) if m.is_dir() && !m.file_type().is_symlink() => {}
            _ => continue,
        }
        let Some((bytes, idle)) = aged(survey, cache) else {
            continue;
        };
        rows.push(ScratchRow {
            path: cache.clone(),
            bytes,
            kind: ScratchKind::BuildCache,
            reason: format!("build cache unused for {} d", days(idle)),
            idle,
        });
    }
}

/// Classify every scratch entry: the deletable rows (largest first) and what
/// was held back with why.
pub(crate) fn plan(survey: &ScratchSurvey) -> (Vec<ScratchRow>, Vec<(PathBuf, String)>) {
    let mut rows = Vec::new();
    let mut held = Vec::new();
    plan_jobs(survey, &mut rows, &mut held);
    plan_tmp(survey, &mut rows, &mut held);
    plan_build_caches(survey, &mut rows);
    rows.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    (rows, held)
}

/// Bail unless `row` is a path this module may delete, judged ONLY against
/// the survey's roots — the delete-site re-check, independent of how the
/// plan was built. A job row must be exactly `<jobs_root>/<id>/tmp` (so the
/// job's `state.json`, `timeline.jsonl` and transcripts are out of reach), a
/// session row a child or slug-grandchild of the TMPDIR root, and a build
/// cache one of the named dirs.
pub(crate) fn check(survey: &ScratchSurvey, row: &ScratchRow) -> anyhow::Result<()> {
    let refuse = |why: &str| -> anyhow::Result<()> {
        anyhow::bail!(
            "refusing to delete {}: {why}; reaching this means the scratch plan was built wrong",
            row.path.display()
        )
    };
    if std::fs::symlink_metadata(&row.path).is_ok_and(|m| m.file_type().is_symlink()) {
        return refuse("it is a symlink");
    }
    match row.kind {
        ScratchKind::JobTmp => {
            let Some(jobs_root) = &survey.jobs_root else {
                return refuse("no jobs root was surveyed");
            };
            if row.path.file_name().is_none_or(|n| n != "tmp")
                || row.path.parent().and_then(Path::parent) != Some(jobs_root.as_path())
            {
                return refuse("it is not a job's own tmp/ directory");
            }
        }
        ScratchKind::SessionScratch => {
            let Some(tmp_root) = &survey.tmp_root else {
                return refuse("no TMPDIR root was surveyed");
            };
            let parent = row.path.parent();
            let grandparent = parent.and_then(Path::parent);
            if parent != Some(tmp_root.as_path()) && grandparent != Some(tmp_root.as_path()) {
                return refuse("it is not directly under the TMPDIR root");
            }
            if survey.live_session_dirs.contains(&row.path) {
                return refuse("it is live session scratch");
            }
        }
        ScratchKind::BuildCache => {
            if !survey.build_caches.iter().any(|c| c == &row.path) {
                return refuse("it is not a surveyed build cache");
            }
        }
    }
    Ok(())
}

/// What `apply` did across the scratch rows.
#[derive(Debug, Clone, Default)]
pub(crate) struct ScratchReclaimed {
    pub deleted: Vec<(PathBuf, u64)>,
    pub failed: Vec<(PathBuf, String)>,
    /// Entries left alone because `--target-pct` was already satisfied.
    pub stopped_early: Vec<PathBuf>,
}

/// Delete `rows`, re-checking each against the survey first. When
/// `stop_at_pct` is set, `disk_pct` is probed on each row's parent directory
/// before deleting and the row is skipped once its filesystem is at or below
/// the target percentage, matching the worktree-cache tier. A check refusal
/// fails the whole run, like worktree reclaim's rail-2 re-check.
pub(crate) fn apply(
    survey: &ScratchSurvey,
    rows: &[ScratchRow],
    stop_at_pct: Option<u8>,
    disk_pct: impl Fn(&Path) -> Option<u8>,
    remove: impl Fn(&Path) -> std::io::Result<()>,
) -> anyhow::Result<ScratchReclaimed> {
    let mut out = ScratchReclaimed::default();
    for row in rows {
        check(survey, row)?;
        if let Some(floor) = stop_at_pct {
            let probe_path = row.path.parent().unwrap_or(&row.path);
            if let Some(pct) = disk_pct(probe_path) {
                if pct <= floor {
                    out.stopped_early.push(row.path.clone());
                    continue;
                }
            }
        }
        match remove(&row.path) {
            Ok(()) => out.deleted.push((row.path.clone(), row.bytes)),
            Err(e) => out.failed.push((row.path.clone(), e.to_string())),
        }
    }
    Ok(out)
}

/// The agent TMPDIR root for this process: the ancestor of `$TMPDIR` whose
/// name starts with `claude-` (e.g. `/tmp/claude-1000` from a session's
/// `/tmp/claude-1000/<slug>/<id>/scratchpad`). `None` when TMPDIR is unset
/// or no such ancestor exists — the session-scratch scan is then skipped
/// rather than guessed.
pub(crate) fn tmp_root_from(tmpdir: Option<&Path>) -> Option<PathBuf> {
    let tmpdir = tmpdir?;
    tmpdir
        .ancestors()
        .find(|a| {
            a.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("claude-"))
        })
        .map(Path::to_path_buf)
}

/// The calling session's own scratch entry under `tmp_root` (the child or
/// slug-grandchild of the root that `$TMPDIR` sits inside), to exclude.
pub(crate) fn own_session_dir(tmp_root: &Path, tmpdir: Option<&Path>) -> Option<PathBuf> {
    let tmpdir = tmpdir?;
    let rel = tmpdir.strip_prefix(tmp_root).ok()?;
    let mut components = rel.components();
    let first = components.next()?;
    let first_path = tmp_root.join(first);
    let slug = first
        .as_os_str()
        .to_str()
        .is_some_and(|n| n.starts_with('-'));
    if !slug {
        return Some(first_path);
    }
    components.next().map(|second| first_path.join(second))
}

/// Build the production survey: `~/.claude/jobs`, the TMPDIR root, and the
/// known `aida` build caches (`~/.cache/aida-impl-target`), with this
/// session's own scratch dir excluded.
pub(crate) fn survey_from_env(min_age_days: u64, now: SystemTime) -> ScratchSurvey {
    let home = crate::home_dir();
    let jobs_root = home
        .as_deref()
        .map(|h| h.join(".claude").join("jobs"))
        .filter(|p| p.is_dir());
    let tmpdir = std::env::var_os("TMPDIR").map(PathBuf::from);
    let tmp_root = tmp_root_from(tmpdir.as_deref()).filter(|p| p.is_dir());
    let mut live_session_dirs = BTreeSet::new();
    if let Some(root) = &tmp_root {
        if let Some(own) = own_session_dir(root, tmpdir.as_deref()) {
            live_session_dirs.insert(own);
        }
    }
    let build_caches = home
        .as_deref()
        .map(|h| h.join(".cache").join("aida-impl-target"))
        .filter(|p| p.is_dir())
        .into_iter()
        .collect();
    ScratchSurvey {
        jobs_root,
        tmp_root,
        build_caches,
        live_session_dirs,
        min_age: Duration::from_secs(min_age_days.max(1) * 24 * 3600),
        now,
    }
}

#[cfg(test)]
#[path = "tests/task_2325_scratch_reclaim_tests.rs"]
mod tests;
