//! Jobs written by the legacy bash runner (`~/bin/aida-deputy-shell`).
//!
//! That runner shares the queue directory but records each job as
//! `jobs/<id>/meta.json` with a non-UUID id (`20261115T205449-810af4`) and
//! writes only `runner.pid`. When it dies mid-job nothing rewrites the record,
//! so the job reads `running` forever. This module moves such a record to
//! `abandoned`, but only on positive evidence that the runner which claimed
//! it is gone. Each job is judged by its own claiming runner, not by whoever
//! `runner.pid` names now: a restarted runner rewrites `runner.pid` at
//! startup and never resumes old jobs, so a record last written before the
//! live runner's process started belongs to an earlier, dead incarnation.
//! The job is orphaned when the `runner.pid` process is gone (or the pid now
//! belongs to a process that is not an `aida-deputy-shell` runner), or when
//! the record predates the live runner's start. The dead runner's pid is kept
//! when it is the one that claimed the job; a job of an earlier incarnation
//! records only the runner that replaced it, never that runner as its owner.
//! A runner that claimed the job and still lives, a sandbox that cannot see
//! host processes, a missing or unreadable `runner.pid`, an unrecognised
//! runner command line, or a record that cannot be dated leaves the job
//! alone. The script is never re-run. Every other field of the record is kept.
// trace:BUG-2625 | ai:claude

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use std::io::Write;
use std::path::{Path, PathBuf};

use super::{DeadDeputy, JobState, Queue, StrandedJob, OPERATOR_TTY, RC_CANCELLED};
use crate::deputy::{check_private_dir, validate_id, write_private_new};

/// The status a recovered legacy job is given.
pub(crate) const ABANDONED: &str = "abandoned";

/// Clock slack when comparing a record's write time with a runner's start.
const SKEW: chrono::Duration = chrono::Duration::seconds(2);

/// How far `runner.pid`'s write time may sit from its process's start time
/// (derived from the boot time, which drifts) and still date that runner.
const PID_FILE_WINDOW: chrono::Duration = chrono::Duration::seconds(60);

/// What the legacy runner named in `runner.pid` is now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LegacyRunnerFate {
    /// A live runner; `since` is when it wrote `runner.pid` at startup,
    /// when that write time agrees with the process's start time.
    Alive {
        pid: u32,
        since: Option<DateTime<Utc>>,
    },
    /// The runner is gone; `since` is when it wrote `runner.pid`, i.e. when
    /// that incarnation started, when readable.
    Dead {
        pid: u32,
        since: Option<DateTime<Utc>>,
    },
    Unknown(&'static str),
}

/// What became of the runner that claimed one running legacy job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LegacyJobFate {
    /// The live runner claimed it (or the record is too close to its start
    /// to say otherwise): left running.
    Alive(u32),
    /// The claiming runner is dead. `pid` is that runner when known;
    /// `successor` is the later runner `runner.pid` names instead.
    Orphaned {
        pid: Option<u32>,
        successor: Option<u32>,
    },
    Unknown(&'static str),
}

/// Judge one running record, last written at `written`, by the runner that
/// claimed it. A runner rewrites `runner.pid` at startup and never resumes
/// another incarnation's jobs, so a record written before the named runner
/// started was claimed by an earlier runner, which is gone.
pub(crate) fn judge(fate: &LegacyRunnerFate, written: Option<DateTime<Utc>>) -> LegacyJobFate {
    let predates = |since: &Option<DateTime<Utc>>| matches!((written, since), (Some(w), Some(s)) if w + SKEW < *s);
    match fate {
        LegacyRunnerFate::Unknown(why) => LegacyJobFate::Unknown(why),
        LegacyRunnerFate::Alive { pid, since } if predates(since) => LegacyJobFate::Orphaned {
            pid: None,
            successor: Some(*pid),
        },
        LegacyRunnerFate::Alive { .. } if written.is_none() => {
            LegacyJobFate::Unknown("cannot date the job record")
        }
        LegacyRunnerFate::Alive { pid, .. } => LegacyJobFate::Alive(*pid),
        LegacyRunnerFate::Dead { pid, since } if predates(since) => LegacyJobFate::Orphaned {
            pid: None,
            successor: Some(*pid),
        },
        LegacyRunnerFate::Dead { pid, .. } => LegacyJobFate::Orphaned {
            pid: Some(*pid),
            successor: None,
        },
    }
}

/// When `pid` started, from `proc_root`'s stat files.
fn process_start(proc_root: &Path, pid: u32) -> Option<DateTime<Utc>> {
    let stat = std::fs::read_to_string(proc_root.join(pid.to_string()).join("stat")).ok()?;
    // Field 22, counted after the parenthesised command name.
    let ticks: i64 = stat
        .get(stat.rfind(')')? + 1..)?
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()?;
    let btime: i64 = std::fs::read_to_string(proc_root.join("stat"))
        .ok()?
        .lines()
        .find_map(|l| l.strip_prefix("btime "))?
        .trim()
        .parse()
        .ok()?;
    #[cfg(unix)]
    // SAFETY: sysconf reads a constant system value.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    #[cfg(not(unix))]
    let hz = 100;
    if hz <= 0 {
        return None;
    }
    let millis = btime.checked_mul(1000)? + ticks.checked_mul(1000)? / i64::from(hz);
    DateTime::from_timestamp_millis(millis)
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
        let pid_file = self.root.join("runner.pid");
        let pid = match std::fs::read_to_string(&pid_file) {
            Ok(raw) => match raw.trim().parse::<u32>() {
                Ok(pid) if pid > 0 => pid,
                _ => return LegacyRunnerFate::Unknown("invalid runner.pid"),
            },
            Err(e) if e.kind() == NotFound => {
                return LegacyRunnerFate::Unknown("no runner.pid names the legacy runner")
            }
            Err(_) => return LegacyRunnerFate::Unknown("cannot read runner.pid"),
        };
        let dead = || LegacyRunnerFate::Dead {
            pid,
            since: modified(&pid_file),
        };
        let cmdline = match std::fs::read(proc_root.join(pid.to_string()).join("cmdline")) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == NotFound => return dead(),
            Err(_) => return LegacyRunnerFate::Unknown("cannot read runner command line"),
        };
        if self.is_legacy_runner_cmdline(&cmdline) {
            // Date the runner by its pid file (the same clock as the job
            // records), but only when that file was written as this
            // process started: a later rewrite would date it too late.
            let since = match (modified(&pid_file), process_start(proc_root, pid)) {
                (Some(written), Some(started)) if (written - started).abs() <= PID_FILE_WINDOW => {
                    Some(written)
                }
                _ => None,
            };
            return LegacyRunnerFate::Alive { pid, since };
        }
        // Some other launch form of the runner is not proof of death.
        if cmdline
            .windows(b"aida-deputy-shell".len())
            .any(|w| w == b"aida-deputy-shell")
        {
            return LegacyRunnerFate::Unknown("runner.pid names an unrecognised runner command");
        }
        // The pid was reused (or is an unreaped zombie): the runner is gone.
        dead()
    }

    pub(crate) fn legacy_runner_fate(&self) -> LegacyRunnerFate {
        self.legacy_runner_fate_with(
            Path::new("/proc"),
            aida_core::pid_observer::PidObserver::current(),
        )
    }

    // trace:TASK-3801 | ai:codex
    pub(super) fn dead_legacy_deputy(
        &self,
        proc_root: &Path,
        observer: aida_core::pid_observer::PidObserver,
    ) -> Option<DeadDeputy> {
        let fate = || self.legacy_runner_fate_with(proc_root, observer);
        let observed = fate();
        let LegacyRunnerFate::Dead {
            pid,
            since: Some(recorded_at),
        } = observed
        else {
            return None;
        };
        let stranded_jobs: Vec<_> = self
            .legacy_job_ids()
            .into_iter()
            .filter_map(|job| {
                let meta = self.read_legacy_meta(&job)?;
                let previous_state = match legacy_status(&meta).as_deref() {
                    Some("queued") => JobState::Queued,
                    Some("running") => JobState::Running,
                    _ => return None,
                };
                Some(StrandedJob {
                    job,
                    previous_state,
                })
            })
            .collect();
        // Discard the snapshot if either kind of runner took over.
        if stranded_jobs.is_empty()
            || !std::fs::symlink_metadata(self.runner_json())
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
            || fate() != observed
        {
            return None;
        }
        Some(DeadDeputy {
            deputy: self.name.clone(),
            last_runner_pid: pid,
            runner_recorded_at: recorded_at,
            dead_as_of: Utc::now(),
            stranded_jobs,
        })
    }

    /// When the legacy record was last written: the later of its file time
    /// and any start time it records, so a doubtful date never looks older.
    pub(crate) fn legacy_written(
        &self,
        id: &str,
        meta: &serde_json::Map<String, serde_json::Value>,
    ) -> Option<DateTime<Utc>> {
        let file = modified(&self.legacy_meta_path(id))?;
        let recorded = ["started_at", "claimed_at"]
            .iter()
            .filter_map(|k| meta.get(*k)?.as_str())
            .filter_map(|v| DateTime::parse_from_rfc3339(v.trim()).ok())
            .map(|t| t.with_timezone(&Utc))
            .max();
        Some(recorded.map_or(file, |r| r.max(file)))
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

fn modified(path: &Path) -> Option<DateTime<Utc>> {
    Some(std::fs::metadata(path).ok()?.modified().ok()?.into())
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

/// Rewrite one `running` legacy record as abandoned by its dead runner: `pid`
/// when that runner is known, else the `successor` that replaced it.
/// Returns false when the record is not (or no longer) running.
fn abandon(
    q: &Queue,
    id: &str,
    pid: Option<u32>,
    successor: Option<u32>,
    now: DateTime<Utc>,
) -> Result<bool> {
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
    if let Some(pid) = pid {
        meta.insert("runner_pid".into(), pid.into());
    }
    if let Some(successor) = successor {
        meta.insert("superseded_by_runner_pid".into(), successor.into());
    }
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
        r.detail = Some(serde_json::json!({
            "pid": pid,
            "superseded_by": successor,
            "legacy": true,
        }));
    });
    Ok(true)
}

/// Recover each given running legacy job whose claiming runner is
/// positively dead. Returns each job it judged with its verdict; jobs it
/// moved to abandoned are the `Orphaned` ones.
fn recover_ids(
    q: &Queue,
    ids: &[String],
    fate: impl FnOnce() -> LegacyRunnerFate,
    now: DateTime<Utc>,
) -> Vec<(String, LegacyJobFate)> {
    let running: Vec<(&String, Option<DateTime<Utc>>)> = ids
        .iter()
        .filter_map(|id| {
            let meta = q.read_legacy_meta(id)?;
            (legacy_status(&meta).as_deref() == Some("running"))
                .then(|| (id, q.legacy_written(id, &meta)))
        })
        .collect();
    if running.is_empty() {
        return Vec::new();
    }
    let fate = fate();
    let mut out = Vec::new();
    for (id, written) in running {
        let verdict = judge(&fate, written);
        if let LegacyJobFate::Orphaned { pid, successor } = verdict {
            match abandon(q, id, pid, successor, now) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(e) => {
                    eprintln!(
                        "deputy shell {}: could not recover legacy job {id}: {e:#}",
                        q.name
                    );
                    continue;
                }
            }
        }
        out.push((id.clone(), verdict));
    }
    out
}

/// Every running legacy job whose claiming runner is dead, observed via
/// `proc_root`. Returns the abandoned jobs with their verdicts.
pub(super) fn recover_with(
    q: &Queue,
    now: DateTime<Utc>,
    proc_root: &Path,
    observer: aida_core::pid_observer::PidObserver,
) -> Vec<(String, LegacyJobFate)> {
    recover_ids(
        q,
        &q.legacy_job_ids(),
        || q.legacy_runner_fate_with(proc_root, observer),
        now,
    )
    .into_iter()
    .filter(|(_, v)| matches!(v, LegacyJobFate::Orphaned { .. }))
    .collect()
}

/// [`recover_with`] against this host's `/proc`.
pub(crate) fn recover(q: &Queue, now: DateTime<Utc>) -> Vec<(String, LegacyJobFate)> {
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
    let verdict = recover_ids(q, &[id.to_string()], fate, Utc::now())
        .pop()
        .map(|(_, v)| v);
    let Some(meta) = q.read_legacy_meta(id) else {
        bail!("legacy script job {id} has an unreadable meta.json");
    };
    let status = legacy_status(&meta).unwrap_or_else(|| "unknown".into());
    writeln!(out, "script job {id}: {status} (legacy bash runner)")?;
    if let Some(rc) = meta.get("rc").and_then(serde_json::Value::as_i64) {
        let reason = meta.get("reason").and_then(|v| v.as_str()).unwrap_or("-");
        writeln!(out, "  rc:         {rc} ({reason})")?;
    }
    let pid_of = |k: &str| meta.get(k).and_then(serde_json::Value::as_u64);
    if status == ABANDONED {
        match (pid_of("runner_pid"), pid_of("superseded_by_runner_pid")) {
            (Some(pid), _) => {
                writeln!(out, "  runner:     pid {pid} (died during job; not re-run)")?
            }
            (None, Some(next)) => writeln!(
                out,
                "  runner:     an earlier runner died during job (replaced by pid {next}); not re-run"
            )?,
            (None, None) => {}
        }
    } else if status == "running" {
        match verdict {
            Some(LegacyJobFate::Alive(pid)) => {
                writeln!(out, "  runner:     live, pid {pid} (legacy bash runner)")?
            }
            Some(LegacyJobFate::Unknown(why)) => {
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
