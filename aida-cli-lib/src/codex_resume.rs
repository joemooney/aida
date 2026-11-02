//! BUG-2386: binding an AIDA session to the Codex thread it can be resumed by.
//!
//! Claude takes the AIDA-minted id as `--session-id` and resumes by that id.
//! Codex ignores it and mints its own thread id, so a Codex resume needs that
//! id bound to the AIDA session first. The canonical source is the
//! `thread.started` event `codex exec --json` writes first into the headless
//! log; it is stamped on the lease as `vendor_session_id`. When neither the
//! lease nor the log carries it, the fallback is the one rollout under
//! `~/.codex/sessions` started in the session's tree since its launch. An
//! ambiguous or missing result refuses with a named error, never a guess.
// trace:BUG-2386 | ai:claude

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};

/// The lease key the Codex thread id is stamped under.
pub(crate) const LEASE_VENDOR_SESSION_KEY: &str = "vendor_session_id";

/// The thread id from the first `thread.started` event of a `codex exec
/// --json` log, if it has one.
pub(crate) fn thread_id_from_log(path: &Path) -> Option<String> {
    let body = std::fs::read_to_string(path).ok()?;
    thread_id_from_stream(&body)
}

/// [`thread_id_from_log`] over the log's text.
pub(crate) fn thread_id_from_stream(body: &str) -> Option<String> {
    body.lines().find_map(|line| {
        let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
        if value.get("type")?.as_str()? != "thread.started" {
            return None;
        }
        let id = value.get("thread_id")?.as_str()?.trim();
        valid_thread_id(id).then(|| id.to_string())
    })
}

/// A thread id is what `codex resume` takes as a positional: refuse anything
/// that could read as a flag or carry shell/path metacharacters.
pub(crate) fn valid_thread_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('-')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// One Codex rollout's identity, read from its first `session_meta` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodexRollout {
    pub(crate) id: String,
    pub(crate) cwd: Option<PathBuf>,
    pub(crate) started: Option<chrono::DateTime<chrono::Utc>>,
    pub(crate) path: PathBuf,
}

/// Every rollout under `root` whose `session_meta` names a thread id.
pub(crate) fn list_rollouts(root: &Path) -> Vec<CodexRollout> {
    rollouts_named(root, |_| true)
}

/// The rollouts under `root` whose file name passes `keep`.
fn rollouts_named(root: &Path, keep: impl Fn(&str) -> bool) -> Vec<CodexRollout> {
    let mut out = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                dirs.push(path);
                continue;
            }
            let is_rollout = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("rollout-") && n.ends_with(".jsonl") && keep(n));
            if !kind.is_file() || !is_rollout {
                continue;
            }
            if let Some(rollout) = read_rollout_meta(&path) {
                out.push(rollout);
            }
        }
    }
    out
}

fn read_rollout_meta(path: &Path) -> Option<CodexRollout> {
    use std::io::{BufRead, Read};
    let file = std::fs::File::open(path).ok()?;
    let mut first = String::new();
    std::io::BufReader::new(file)
        .take(1024 * 1024)
        .read_line(&mut first)
        .ok()?;
    let value: serde_json::Value = serde_json::from_str(first.trim()).ok()?;
    if value.get("type")?.as_str()? != "session_meta" {
        return None;
    }
    let payload = value.get("payload")?;
    let id = payload.get("id")?.as_str()?.to_string();
    if !valid_thread_id(&id) {
        return None;
    }
    let started = payload
        .get("timestamp")
        .or_else(|| value.get("timestamp"))
        .and_then(|t| t.as_str())
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .map(|t| t.with_timezone(&chrono::Utc));
    Some(CodexRollout {
        id,
        cwd: payload
            .get("cwd")
            .and_then(|c| c.as_str())
            .map(PathBuf::from),
        started,
        path: path.to_path_buf(),
    })
}

/// The rollout for thread `id` under `root`, if Codex recorded one.
/// Codex names each rollout after its thread, so only those files are read.
pub(crate) fn find_rollout(root: &Path, id: &str) -> Option<CodexRollout> {
    if !valid_thread_id(id) {
        return None;
    }
    rollouts_named(root, |name| name.contains(id))
        .into_iter()
        .find(|r| r.id == id)
}

/// How an id handed to a resume verb is resumed: the vendor that recorded
/// it and the id that vendor takes. A Codex thread id (it has a rollout) is
/// Codex's own; an AIDA session id whose headless log started a Codex thread
/// resumes that thread; anything else is a Claude session id.
pub(crate) fn recorded_resume(
    project_root: &Path,
    id: &str,
) -> (crate::session::HeadlessVendor, String) {
    use crate::session::HeadlessVendor;
    let rollout =
        crate::vendor_activity::codex_sessions_root().and_then(|root| find_rollout(&root, id));
    if rollout.is_some() {
        return (HeadlessVendor::Codex, id.to_string());
    }
    if let Some(thread) = headless_logs_for(project_root, id)
        .iter()
        .find_map(|log| thread_id_from_log(log))
    {
        return (HeadlessVendor::Codex, thread);
    }
    (HeadlessVendor::Claude, id.to_string())
}

/// The one thread started in `cwd` at or after `since`. Zero or several
/// candidates refuse: resuming the wrong thread replays another session.
pub(crate) fn unique_rollout_in(
    rollouts: &[CodexRollout],
    cwd: &Path,
    since: Option<SystemTime>,
) -> Result<String> {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let want = canon(cwd);
    let since = since.map(chrono::DateTime::<chrono::Utc>::from);
    let matches: Vec<&CodexRollout> = rollouts
        .iter()
        .filter(|r| r.cwd.as_deref().map(canon).as_deref() == Some(want.as_path()))
        .filter(|r| match (since, r.started) {
            (Some(since), Some(started)) => started >= since,
            (Some(_), None) => false,
            (None, _) => true,
        })
        .collect();
    match matches.as_slice() {
        [one] => Ok(one.id.clone()),
        [] => anyhow::bail!(
            "no Codex thread is bound to this session: its lease and headless log carry no \
             thread id, and no Codex rollout started in {} since its launch. Start a fresh run \
             instead of resuming.",
            cwd.display()
        ),
        many => anyhow::bail!(
            "cannot tell which Codex thread to resume: {} rollouts started in {} since this \
             session's launch ({}). Resume one by id with `codex resume <id>`, or start a fresh \
             run.",
            many.len(),
            cwd.display(),
            many.iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Where a Codex resume looks for the thread bound to an AIDA session.
pub(crate) struct ThreadLookup<'a> {
    /// The main checkout whose `.aida/` holds the lease and headless logs.
    pub(crate) project_root: &'a Path,
    /// The AIDA-minted session id the run launched with.
    pub(crate) session_id: &'a str,
    /// The run's lease, when it had one.
    pub(crate) lease_id: Option<&'a str>,
    /// The tree the run worked in (the rollout fallback's cwd filter).
    pub(crate) worktree: &'a Path,
    /// When the run launched (the rollout fallback's lower bound).
    pub(crate) since: Option<SystemTime>,
}

/// Resolve the Codex thread id bound to an AIDA session: the lease stamp,
/// then the headless log's `thread.started`, then a unique rollout.
pub(crate) fn resolve_thread_id(lookup: &ThreadLookup<'_>) -> Result<String> {
    if let Some(id) = lookup
        .lease_id
        .and_then(|lease| lease_vendor_session_id(lookup.project_root, lease))
    {
        return Ok(id);
    }
    if let Some(id) = headless_logs_for(lookup.project_root, lookup.session_id)
        .iter()
        .find_map(|log| thread_id_from_log(log))
    {
        return Ok(id);
    }
    let root = crate::vendor_activity::codex_sessions_root()
        .context("HOME not set; cannot locate Codex sessions")?;
    unique_rollout_in(&list_rollouts(&root), lookup.worktree, lookup.since)
}

/// The headless logs a run with AIDA session `session_id` wrote, first run
/// first: `<branch>-<id>.jsonl` (its resumes log beside it).
pub(crate) fn headless_logs_for(project_root: &Path, session_id: &str) -> Vec<PathBuf> {
    let dir = project_root.join(".aida").join("headless-logs");
    let suffix = format!("-{session_id}.jsonl");
    let mut logs: Vec<PathBuf> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(&suffix))
        })
        .collect();
    logs.sort();
    logs
}

fn lease_path(project_root: &Path, lease_id: &str) -> PathBuf {
    crate::leases::leases_dir(project_root).join(format!("{lease_id}.toml"))
}

/// The Codex thread id stamped on `lease_id`, if any. A lease without the key
/// (older, or not a Codex run) reads as unbound.
pub(crate) fn lease_vendor_session_id(project_root: &Path, lease_id: &str) -> Option<String> {
    let body = std::fs::read_to_string(lease_path(project_root, lease_id)).ok()?;
    let doc = body.parse::<toml_edit::DocumentMut>().ok()?;
    let id = doc
        .get(LEASE_VENDOR_SESSION_KEY)?
        .as_str()?
        .trim()
        .to_string();
    valid_thread_id(&id).then_some(id)
}

/// Stamp the Codex thread id on `lease_id`, keeping every other key.
pub(crate) fn stamp_lease_vendor_session_id(
    project_root: &Path,
    lease_id: &str,
    thread_id: &str,
) -> Result<()> {
    anyhow::ensure!(
        valid_thread_id(thread_id),
        "refusing to record `{thread_id}`: not a Codex thread id"
    );
    let path = lease_path(project_root, lease_id);
    crate::session::with_lease_write_lock(project_root, || {
        let body = std::fs::read_to_string(&path)
            .with_context(|| format!("reading lease {}", path.display()))?;
        let mut doc = body
            .parse::<toml_edit::DocumentMut>()
            .with_context(|| format!("parsing lease {}", path.display()))?;
        doc[LEASE_VENDOR_SESSION_KEY] = toml_edit::value(thread_id);
        aida_core::write_atomic(&path, doc.to_string())
            .with_context(|| format!("writing lease {}", path.display()))
    })
}

/// After a Codex run on `lease_id` wrote `log`, bind the thread it started to
/// the lease. Best-effort: a failure warns, and a resume falls back to the
/// log and rollout scan.
pub(crate) fn bind_thread_from_log(project_root: &Path, lease_id: &str, log: &Path) {
    let Some(thread) = thread_id_from_log(log) else {
        return;
    };
    if lease_vendor_session_id(project_root, lease_id).as_deref() == Some(thread.as_str()) {
        return;
    }
    if let Err(e) = stamp_lease_vendor_session_id(project_root, lease_id, &thread) {
        eprintln!("Warning: could not record the Codex thread id on lease {lease_id}: {e:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_rollout(dir: &Path, id: &str, cwd: &Path, ts: &str) {
        std::fs::create_dir_all(dir).unwrap();
        let line = serde_json::json!({
            "timestamp": ts,
            "type": "session_meta",
            "payload": {"id": id, "timestamp": ts, "cwd": cwd},
        });
        std::fs::write(
            dir.join(format!("rollout-2026-11-01T00-00-00-{id}.jsonl")),
            format!("{line}\n{{\"type\":\"event_msg\"}}\n"),
        )
        .unwrap();
    }

    #[test]
    fn thread_id_is_read_from_the_first_thread_started_event() {
        let body = "{\"type\":\"thread.started\",\"thread_id\":\"019a-thread\"}\n\
                    {\"type\":\"turn.started\"}\n\
                    {\"type\":\"thread.started\",\"thread_id\":\"other\"}\n";
        assert_eq!(thread_id_from_stream(body).as_deref(), Some("019a-thread"));
        assert_eq!(thread_id_from_stream("{\"type\":\"turn.started\"}\n"), None);
        // A flag-shaped id is never taken.
        assert_eq!(
            thread_id_from_stream("{\"type\":\"thread.started\",\"thread_id\":\"--last\"}"),
            None
        );
    }

    #[test]
    fn rollout_fallback_finds_one_refuses_none_and_ambiguous() {
        let tmp = tempfile::tempdir().unwrap();
        let tree = tmp.path().join("seat");
        let other = tmp.path().join("other");
        std::fs::create_dir_all(&tree).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let root = tmp.path().join("sessions");
        write_rollout(
            &root.join("2026/11/01"),
            "old-thread",
            &tree,
            "2026-10-01T00:00:00Z",
        );
        write_rollout(
            &root.join("2026/11/01"),
            "new-thread",
            &tree,
            "2026-11-01T12:00:00Z",
        );
        write_rollout(
            &root.join("2026/11/02"),
            "elsewhere",
            &other,
            "2026-11-01T12:00:00Z",
        );
        let rollouts = list_rollouts(&root);
        assert_eq!(rollouts.len(), 3);
        let since: SystemTime = chrono::DateTime::parse_from_rfc3339("2026-11-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc)
            .into();
        assert_eq!(
            unique_rollout_in(&rollouts, &tree, Some(since)).unwrap(),
            "new-thread"
        );
        let missing = unique_rollout_in(&rollouts, &tmp.path().join("none"), Some(since))
            .unwrap_err()
            .to_string();
        assert!(missing.contains("no Codex thread is bound"), "{missing}");
        let ambiguous = unique_rollout_in(&rollouts, &tree, None)
            .unwrap_err()
            .to_string();
        assert!(ambiguous.contains("2 rollouts"), "{ambiguous}");
        assert_eq!(
            find_rollout(&root, "elsewhere").map(|r| r.cwd),
            Some(Some(other))
        );
    }

    #[test]
    fn lease_stamp_round_trips_and_resolution_prefers_it() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = crate::leases::leases_dir(tmp.path());
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(
            sessions.join("lease1.toml"),
            "id = \"lease1\"\nscope = \"BUG-1\"\n",
        )
        .unwrap();
        assert_eq!(lease_vendor_session_id(tmp.path(), "lease1"), None);
        let logs = tmp.path().join(".aida").join("headless-logs");
        std::fs::create_dir_all(&logs).unwrap();
        let log = logs.join("branch-sid-1.jsonl");
        std::fs::write(
            &log,
            "{\"type\":\"thread.started\",\"thread_id\":\"from-log\"}\n",
        )
        .unwrap();
        let lookup = ThreadLookup {
            project_root: tmp.path(),
            session_id: "sid-1",
            lease_id: Some("lease1"),
            worktree: tmp.path(),
            since: None,
        };
        // Unbound lease: the headless log answers.
        assert_eq!(resolve_thread_id(&lookup).unwrap(), "from-log");
        bind_thread_from_log(tmp.path(), "lease1", &log);
        let body = std::fs::read_to_string(sessions.join("lease1.toml")).unwrap();
        assert!(body.contains("scope = \"BUG-1\""), "{body}");
        assert_eq!(
            lease_vendor_session_id(tmp.path(), "lease1").as_deref(),
            Some("from-log")
        );
        stamp_lease_vendor_session_id(tmp.path(), "lease1", "from-lease").unwrap();
        assert_eq!(resolve_thread_id(&lookup).unwrap(), "from-lease");
        assert!(stamp_lease_vendor_session_id(tmp.path(), "lease1", "a b").is_err());
    }
}
