//! Jobs written by the legacy bash runner (`~/bin/aida-deputy-shell`).
//!
//! That runner shares the queue directory but records each job as
//! `jobs/<id>/meta.json` with a non-UUID id (`20261115T205449-810af4`) and
//! writes only `runner.pid`. When it dies mid-job nothing rewrites the record,
//! so the job reads `running` forever. This module moves such a record to
//! `abandoned` and keeps the dead runner's pid, but only on positive evidence
//! that the runner is gone: the pid has no process, or it now belongs to a
//! process that is not an `aida-deputy-shell` runner. A live runner, a
//! sandbox that cannot see host processes, a missing or unreadable
//! `runner.pid`, or an unrecognised runner command line leaves the job alone.
//! The script is never re-run. Every other field of the record is kept.
// trace:BUG-2625 | ai:claude

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use std::io::Write;
use std::path::{Path, PathBuf};

use super::{Queue, OPERATOR_TTY, RC_CANCELLED};
use crate::deputy::{check_private_dir, validate_id, write_private_new};

/// The status a recovered legacy job is given.
pub(crate) const ABANDONED: &str = "abandoned";

/// What the legacy runner named in `runner.pid` is now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LegacyRunnerFate {
    Alive(u32),
    Dead(u32),
    Unknown(&'static str),
}

/// A legacy job id: the bash runner's `<timestamp>-<hex>` form, or any other
/// plain directory name that is not a native UUID id.
pub(crate) fn is_legacy_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && !id.starts_with('.')
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        && validate_id(id).is_err()
}

impl Queue {
    pub(crate) fn legacy_meta_path(&self, id: &str) -> PathBuf {
        self.job_dir(id).join("meta.json")
    }

    /// Legacy job directories (holding a regular `meta.json`), oldest first.
    pub(crate) fn legacy_job_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = std::fs::read_dir(self.jobs())
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|id| is_legacy_id(id))
            .filter(|id| {
                std::fs::symlink_metadata(self.legacy_meta_path(id)).is_ok_and(|m| m.is_file())
            })
            .collect();
        ids.sort();
        ids
    }

    /// The legacy runner's fate, from `runner.pid` and `proc_root`.
    pub(super) fn legacy_runner_fate_with(
        &self,
        proc_root: &Path,
        observer: aida_core::pid_observer::PidObserver,
    ) -> LegacyRunnerFate {
        use std::io::ErrorKind::NotFound;
        if !observer.can_observe(None) {
            return LegacyRunnerFate::Unknown("cannot see host processes from this sandbox");
        }
        if std::fs::read(proc_root.join("self/stat")).is_err() {
            return LegacyRunnerFate::Unknown("cannot read process information from /proc");
        }
        let pid = match std::fs::read_to_string(self.root.join("runner.pid")) {
            Ok(raw) => match raw.trim().parse::<u32>() {
                Ok(pid) if pid > 0 => pid,
                _ => return LegacyRunnerFate::Unknown("invalid runner.pid"),
            },
            Err(e) if e.kind() == NotFound => {
                return LegacyRunnerFate::Unknown("no runner.pid names the legacy runner")
            }
            Err(_) => return LegacyRunnerFate::Unknown("cannot read runner.pid"),
        };
        let cmdline = match std::fs::read(proc_root.join(pid.to_string()).join("cmdline")) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == NotFound => return LegacyRunnerFate::Dead(pid),
            Err(_) => return LegacyRunnerFate::Unknown("cannot read runner command line"),
        };
        if self.is_legacy_runner_cmdline(&cmdline) {
            return LegacyRunnerFate::Alive(pid);
        }
        // Some other launch form of the runner is not proof of death.
        if cmdline
            .windows(b"aida-deputy-shell".len())
            .any(|w| w == b"aida-deputy-shell")
        {
            return LegacyRunnerFate::Unknown("runner.pid names an unrecognised runner command");
        }
        // The pid was reused (or is an unreaped zombie): the runner is gone.
        LegacyRunnerFate::Dead(pid)
    }

    pub(crate) fn legacy_runner_fate(&self) -> LegacyRunnerFate {
        self.legacy_runner_fate_with(
            Path::new("/proc"),
            aida_core::pid_observer::PidObserver::current(),
        )
    }

    /// The legacy record as a JSON object, when it is one.
    pub(crate) fn read_legacy_meta(
        &self,
        id: &str,
    ) -> Option<serde_json::Map<String, serde_json::Value>> {
        let bytes = std::fs::read(self.legacy_meta_path(id)).ok()?;
        match serde_json::from_slice(&bytes).ok()? {
            serde_json::Value::Object(map) => Some(map),
            _ => None,
        }
    }
}

/// The key holding a legacy record's status (`status`, else `state`).
fn status_key(meta: &serde_json::Map<String, serde_json::Value>) -> Option<&'static str> {
    ["status", "state"]
        .into_iter()
        .find(|k| meta.get(*k).is_some_and(serde_json::Value::is_string))
}

/// The legacy record's status, trimmed and lowercased.
pub(crate) fn legacy_status(meta: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    let key = status_key(meta)?;
    Some(meta.get(key)?.as_str()?.trim().to_ascii_lowercase())
}

/// Rewrite one `running` legacy record as abandoned by dead runner `pid`.
/// Returns false when the record is not (or no longer) running.
fn abandon(q: &Queue, id: &str, pid: u32, now: DateTime<Utc>) -> Result<bool> {
    check_private_dir(&q.job_dir(id))?;
    let Some(mut meta) = q.read_legacy_meta(id) else {
        return Ok(false);
    };
    if legacy_status(&meta).as_deref() != Some("running") {
        return Ok(false);
    }
    let key = status_key(&meta).expect("status read above");
    meta.insert(key.into(), ABANDONED.into());
    meta.insert("reason".into(), "orphaned".into());
    meta.insert("runner_pid".into(), pid.into());
    meta.insert("finished_at".into(), now.to_rfc3339().into());
    if meta.get("rc").is_none_or(serde_json::Value::is_null) {
        meta.insert("rc".into(), RC_CANCELLED.into());
    }
    let mut bytes = serde_json::to_vec_pretty(&serde_json::Value::Object(meta))?;
    bytes.push(b'\n');
    let tmp = q
        .job_dir(id)
        .join(format!(".meta.{}.tmp", uuid::Uuid::new_v4()));
    write_private_new(&tmp, &bytes)?;
    if let Err(e) = std::fs::rename(&tmp, q.legacy_meta_path(id)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).context("write the legacy job record");
    }
    q.audit_event(now, "orphaned", Some(id), OPERATOR_TTY, |r| {
        r.rc = Some(RC_CANCELLED);
        r.reason = Some(
            "orphaned: the legacy bash runner died while this job ran; it was not re-run".into(),
        );
        r.detail = Some(serde_json::json!({ "pid": pid, "legacy": true }));
    });
    Ok(true)
}

/// Recover the given legacy jobs when the runner is positively dead.
/// Returns `(id, dead runner pid)` for each job it moved to abandoned.
fn recover_ids(
    q: &Queue,
    ids: &[String],
    fate: impl FnOnce() -> LegacyRunnerFate,
    now: DateTime<Utc>,
) -> Vec<(String, u32)> {
    let running: Vec<&String> = ids
        .iter()
        .filter(|id| {
            q.read_legacy_meta(id)
                .is_some_and(|m| legacy_status(&m).as_deref() == Some("running"))
        })
        .collect();
    if running.is_empty() {
        return Vec::new();
    }
    let LegacyRunnerFate::Dead(pid) = fate() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for id in running {
        match abandon(q, id, pid, now) {
            Ok(true) => out.push((id.clone(), pid)),
            Ok(false) => {}
            Err(e) => eprintln!(
                "deputy shell {}: could not recover legacy job {id}: {e:#}",
                q.name
            ),
        }
    }
    out
}

/// Every running legacy job of a dead legacy runner, observed via `proc_root`.
pub(super) fn recover_with(
    q: &Queue,
    now: DateTime<Utc>,
    proc_root: &Path,
    observer: aida_core::pid_observer::PidObserver,
) -> Vec<(String, u32)> {
    recover_ids(
        q,
        &q.legacy_job_ids(),
        || q.legacy_runner_fate_with(proc_root, observer),
        now,
    )
}

/// [`recover_with`] against this host's `/proc`.
pub(crate) fn recover(q: &Queue, now: DateTime<Utc>) -> Vec<(String, u32)> {
    recover_with(
        q,
        now,
        Path::new("/proc"),
        aida_core::pid_observer::PidObserver::current(),
    )
}

/// `aida deputy shell status <legacy id>`: recover the job if its runner is
/// dead, then print the record.
pub(crate) fn status_with(
    q: &Queue,
    id: &str,
    fate: impl FnOnce() -> LegacyRunnerFate,
    out: &mut dyn Write,
) -> Result<()> {
    if !is_legacy_id(id) || !q.legacy_job_ids().iter().any(|i| i == id) {
        bail!("no script job {id} in deputy {:?}", q.name);
    }
    let mut fate_seen = None;
    recover_ids(
        q,
        &[id.to_string()],
        || {
            let f = fate();
            fate_seen = Some(f.clone());
            f
        },
        Utc::now(),
    );
    let Some(meta) = q.read_legacy_meta(id) else {
        bail!("legacy script job {id} has an unreadable meta.json");
    };
    let status = legacy_status(&meta).unwrap_or_else(|| "unknown".into());
    writeln!(out, "script job {id}: {status} (legacy bash runner)")?;
    if let Some(rc) = meta.get("rc").and_then(serde_json::Value::as_i64) {
        let reason = meta.get("reason").and_then(|v| v.as_str()).unwrap_or("-");
        writeln!(out, "  rc:         {rc} ({reason})")?;
    }
    if status == ABANDONED {
        if let Some(pid) = meta.get("runner_pid").and_then(serde_json::Value::as_u64) {
            writeln!(out, "  runner:     pid {pid} (died during job; not re-run)")?;
        }
    } else if status == "running" {
        match fate_seen {
            Some(LegacyRunnerFate::Alive(pid)) => {
                writeln!(out, "  runner:     live, pid {pid} (legacy bash runner)")?
            }
            Some(LegacyRunnerFate::Unknown(why)) => {
                writeln!(out, "  runner:     UNKNOWN ({why}); left running")?
            }
            _ => {}
        }
    }
    writeln!(out, "  record:     {}", q.legacy_meta_path(id).display())?;
    Ok(())
}

/// [`status_with`] against this host's `/proc`.
pub(crate) fn status(q: &Queue, id: &str, out: &mut dyn Write) -> Result<()> {
    status_with(q, id, || q.legacy_runner_fate(), out)
}
