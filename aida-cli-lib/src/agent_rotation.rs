//! Context rotation for `aida agent new --rotate` (STORY-101).
//!
//! A foreground launch can supervise its child across context rotations:
//! when the child exits after writing a fresh seat handoff, the launcher
//! relaunches the same vendor in the same worktree under the same handle and
//! lease, with the handoff loaded at SessionStart. An exit without a fresh
//! handoff, or after `aida session handoff --final`, is a real exit.
//!
//! This module holds the pure policy (what counts as a fresh handoff, the
//! crash and window guards), the operator countdown, and the relay-run glue
//! that keeps one mail recipient across generations (STORY-1843's relay
//! protocol, driven in-process: the launcher is the relay controller).
// trace:STORY-B6B1278890774EF993D2EBC327862470-101 | ai:claude

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// The generation (1-based) a supervised child runs as.
pub(crate) const GENERATION_ENV: &str = "AIDA_ROTATION_GENERATION";
/// The fresh handoff file a relaunched child loads at SessionStart.
pub(crate) const HANDOFF_ENV: &str = "AIDA_ROTATION_HANDOFF";
/// The native session id of the generation a relaunched child replaces.
pub(crate) const PREVIOUS_SESSION_ENV: &str = "AIDA_ROTATION_PREVIOUS_SESSION";
/// The `written=` stamp of [`HANDOFF_ENV`]'s file, as its log record names it.
pub(crate) const HANDOFF_STAMP_ENV: &str = "AIDA_ROTATION_HANDOFF_STAMP";
/// The seat the supervisor reads handoffs for. The child's handoff commands
/// and its SessionStart hook resolve their seat from it, so all three agree.
pub(crate) const SEAT_ENV: &str = "AIDA_ROTATION_SEAT";

/// PURE: the seat a supervised launch rotates on: its role, else the agent
/// type (a launch without `--role`).
pub(crate) fn supervisor_seat(role: Option<&str>, agent_type: &str) -> String {
    role.map(str::trim)
        .filter(|r| !r.is_empty())
        .map(|r| crate::canonical_role_name(&r.to_ascii_lowercase()))
        .unwrap_or_else(|| agent_type.to_string())
}

/// PURE: whether a launch is supervised. Claude only in this slice; an
/// explicit `--rotate` always is, while `[agents.claude] rotate = true` only
/// applies when stdin is a terminal, so a scripted or piped launch never
/// relaunches unless it asked to.
pub(crate) fn rotation_enabled(
    agent_type: &str,
    explicit: bool,
    config_default: bool,
    stdin_is_tty: bool,
) -> bool {
    agent_type == "claude" && (explicit || (config_default && stdin_is_tty))
}

/// PURE: a launch that resumes a session (`--resume`). An explicit
/// `--rotate` refuses it rather than being dropped; the config default is
/// simply off for the resumed session (`Ok`).
pub(crate) fn refuse_rotating_resume(explicit_rotate: bool) -> std::result::Result<(), String> {
    if explicit_rotate {
        return Err(
            "`--rotate` cannot supervise a resumed session; drop `--resume` to start a \
                    new supervised session, or drop `--rotate` to resume unsupervised"
                .into(),
        );
    }
    Ok(())
}

/// Relaunch limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RotationPolicy {
    /// At most this many relaunches per `window`.
    pub max_relaunches: usize,
    pub window: chrono::Duration,
    /// An exit sooner than this after spawn, with no fresh handoff, is a crash.
    pub crash_within: chrono::Duration,
    /// Seconds of the "relaunching in Ns" countdown.
    pub countdown_secs: u64,
}

impl Default for RotationPolicy {
    fn default() -> Self {
        Self {
            max_relaunches: 3,
            window: chrono::Duration::minutes(10),
            crash_within: chrono::Duration::seconds(60),
            countdown_secs: 5,
        }
    }
}

/// One record of `.aida/handoff/<seat>/log.jsonl`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub(crate) struct HandoffRecord {
    pub ts: DateTime<Utc>,
    pub seat: String,
    #[serde(default)]
    pub event: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub handle: Option<String>,
    #[serde(default)]
    pub native_session_id: Option<String>,
    #[serde(default, rename = "final")]
    pub final_: bool,
}

/// The generation being judged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GenerationRun<'a> {
    pub seat: &'a str,
    pub handle: &'a str,
    /// The native session id the launcher seeded, when the vendor takes one.
    pub native_session_id: Option<&'a str>,
    pub spawned_at: DateTime<Utc>,
}

/// PURE: the newest handoff this generation wrote. Seat and handle must
/// match; the record must carry this generation's seeded session id, or,
/// when the vendor exposes none, be written after the spawn. A sibling
/// seat's, another handle's, or an older session's handoff never matches.
pub(crate) fn fresh_handoff<'r>(
    records: &'r [HandoffRecord],
    run: &GenerationRun<'_>,
) -> Option<&'r HandoffRecord> {
    records.iter().rev().find(|r| {
        r.event == "handoff"
            && r.seat == run.seat
            && r.handle.as_deref() == Some(run.handle)
            && match (run.native_session_id, r.native_session_id.as_deref()) {
                (Some(own), Some(theirs)) => own == theirs,
                (Some(_), None) => false,
                (None, _) => r.ts >= run.spawned_at,
            }
    })
}

/// What the supervisor does after a generation exits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RotationDecision {
    /// No fresh handoff: a real exit.
    Exit,
    /// A fresh `--final` handoff: a real exit, as the agent asked.
    Final,
    /// A fast exit without a handoff: never relaunched.
    Crash { after_secs: i64 },
    /// The window already holds `max_relaunches` relaunches.
    Limit { relaunches: usize, window_mins: i64 },
    /// Relaunch with exactly this handoff, identified by its log stamp.
    Relaunch {
        handoff: PathBuf,
        stamp: DateTime<Utc>,
    },
}

/// PURE: decide after a generation exits. `relaunches` are the times of
/// earlier relaunches in this run. A non-zero exit with a fresh handoff
/// still relaunches: the handoff states the agent's intent.
pub(crate) fn rotation_decision(
    run: &GenerationRun<'_>,
    exited_at: DateTime<Utc>,
    records: &[HandoffRecord],
    relaunches: &[DateTime<Utc>],
    policy: &RotationPolicy,
) -> RotationDecision {
    let Some(fresh) = fresh_handoff(records, run) else {
        let ran = exited_at.signed_duration_since(run.spawned_at);
        if ran < policy.crash_within {
            return RotationDecision::Crash {
                after_secs: ran.num_seconds().max(0),
            };
        }
        return RotationDecision::Exit;
    };
    if fresh.final_ {
        return RotationDecision::Final;
    }
    let recent = relaunches
        .iter()
        .filter(|at| exited_at.signed_duration_since(**at) < policy.window)
        .count();
    if recent >= policy.max_relaunches {
        return RotationDecision::Limit {
            relaunches: recent,
            window_mins: policy.window.num_minutes(),
        };
    }
    RotationDecision::Relaunch {
        handoff: PathBuf::from(&fresh.path),
        stamp: fresh.ts,
    }
}

/// The `written=` timestamp in a handoff file's header comment.
fn header_stamp(content: &str) -> Option<DateTime<Utc>> {
    let header = content.lines().next()?;
    let rest = header.strip_prefix("<!-- seat handoff:")?;
    let written = rest
        .split_whitespace()
        .find_map(|kv| kv.strip_prefix("written="))?;
    DateTime::parse_from_rfc3339(written)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

/// The handoff a relaunched generation loads: exactly the file the
/// supervisor matched, and only while its header still carries the stamp the
/// supervisor saw. `Err` is the one-line reason nothing is loaded. A newer
/// handoff on the same seat (a sibling session's) never stands in for it.
pub(crate) fn matched_handoff(path: &str, stamp: &str) -> std::result::Result<String, String> {
    let path = path.trim();
    if path.is_empty() {
        return Err("the supervisor named no handoff file".into());
    }
    let want = DateTime::parse_from_rfc3339(stamp.trim())
        .map(|t| t.with_timezone(&Utc))
        .map_err(|_| format!("the handoff stamp `{}` is not a timestamp", stamp.trim()))?;
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!("the matched handoff {path} is gone"));
        }
        Err(e) => return Err(format!("the matched handoff {path} is unreadable: {e}")),
    };
    match header_stamp(&content) {
        Some(got) if got == want => Ok(content),
        Some(got) => Err(format!(
            "the handoff at {path} now carries stamp {} instead of {}",
            got.to_rfc3339(),
            want.to_rfc3339()
        )),
        None => Err(format!("the handoff at {path} has no stamp")),
    }
}

/// How a supervised run ended.
#[derive(Debug)]
pub(crate) enum RotationOutcome {
    /// The last generation's exit (a real exit, a crash or a limit stop).
    Exited(std::process::ExitStatus),
    /// The operator stopped the relaunch countdown; the lease is released.
    Cancelled,
}

/// The policy a supervisor runs under, and a probe called in each gap
/// between generations (after the countdown, before the next spawn) with
/// the generation about to start. The probe is a no-op outside tests.
pub(crate) struct Supervision<'a> {
    pub policy: RotationPolicy,
    pub on_gap: &'a mut dyn FnMut(u64),
}

/// PURE: the line the supervisor prints for a decision that stops it.
pub(crate) fn stop_message(handle: &str, generation: u64, decision: &RotationDecision) -> String {
    match decision {
        RotationDecision::Exit => format!("{handle} #{generation} exited without a fresh handoff"),
        RotationDecision::Final => {
            format!("{handle} #{generation} wrote a final handoff; not relaunching")
        }
        RotationDecision::Crash { after_secs } => format!(
            "stopped: {handle} #{generation} exited after {after_secs}s without a handoff; \
             not relaunching"
        ),
        RotationDecision::Limit {
            relaunches,
            window_mins,
        } => format!(
            "stopped: {relaunches} relaunches in {window_mins} min (limit); \
             {handle} is not relaunched again"
        ),
        RotationDecision::Relaunch { .. } => {
            format!("relaunching {handle} #{}", generation + 1)
        }
    }
}

/// Read a seat's handoff log. Unparseable lines are skipped.
pub(crate) fn read_handoff_log(project_root: &Path, seat: &str) -> Vec<HandoffRecord> {
    let Ok(seat) = crate::seat_rotation::sanitize_seat(seat) else {
        return Vec::new();
    };
    let path = project_root
        .join(".aida")
        .join("handoff")
        .join(seat)
        .join("log.jsonl");
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l.trim()).ok())
        .collect()
}

/// Run the "relaunching in Ns" countdown. Returns `false` when `stop` was
/// raised (Ctrl-C) before it finished.
pub(crate) fn countdown(
    handle: &str,
    next_generation: u64,
    secs: u64,
    stop: &AtomicBool,
    tick: &mut dyn FnMut(),
    out: &mut dyn Write,
) -> bool {
    for left in (1..=secs).rev() {
        if stop.load(Ordering::SeqCst) {
            let _ = writeln!(out);
            return false;
        }
        let _ = write!(
            out,
            "\rrelaunching {handle} #{next_generation} in {left}s, Ctrl-C to stop "
        );
        let _ = out.flush();
        tick();
    }
    let _ = writeln!(out);
    !stop.load(Ordering::SeqCst)
}

/// A Ctrl-C/SIGTERM flag for the countdown, active while it is held.
pub(crate) struct StopFlag {
    pub flag: Arc<AtomicBool>,
    ids: Vec<signal_hook::SigId>,
}

impl StopFlag {
    pub(crate) fn install() -> Result<Self> {
        let flag = Arc::new(AtomicBool::new(false));
        let mut ids = Vec::new();
        for sig in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
            ids.push(
                signal_hook::flag::register(sig, Arc::clone(&flag))
                    .context("installing the rotation countdown's Ctrl-C handler")?,
            );
        }
        Ok(Self { flag, ids })
    }
}

impl Drop for StopFlag {
    fn drop(&mut self) {
        for id in self.ids.drain(..) {
            signal_hook::low_level::unregister(id);
        }
    }
}

/// The relay run that keeps one mail recipient across generations. The
/// launcher is its controller and holds its run lock for the whole run.
pub(crate) struct RotationRun {
    project_root: PathBuf,
    pub run_id: String,
    capability: String,
    lock_path: PathBuf,
    _lock: std::fs::File,
    ended: bool,
}

impl RotationRun {
    /// Bind `handle` as a relay run owned by this process. The launch's own
    /// unbound reservation of `handle` is released first, so the run takes
    /// the label over.
    pub(crate) fn bind(project_root: &Path, handle: &str) -> Result<Self> {
        use fs2::FileExt;
        let pid = std::process::id();
        let dir = crate::agent_registry::agents_dir(project_root).join("rotation-locks");
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let lock_path = dir.join(format!("{}.lock", uuid::Uuid::new_v4()));
        let mut lock = std::fs::File::create(&lock_path)
            .with_context(|| format!("creating {}", lock_path.display()))?;
        writeln!(lock, "{pid}")?;
        lock.flush()?;
        lock.lock_exclusive()
            .context("taking the rotation run lock")?;
        if let Some(own) = crate::agent_handle::load_reservation(project_root, handle)
            .filter(|r| r.launcher_pid == Some(pid) && r.native_session_id.is_none())
        {
            crate::agent_handle::release_unbound(project_root, &own)?;
        }
        let outcome = crate::relay_mail::bind(
            project_root,
            handle,
            pid,
            &lock_path,
            crate::relay_mail::PROTOCOL,
        )
        .with_context(|| format!("holding handle {handle} for rotation"))?;
        Ok(Self {
            project_root: project_root.to_path_buf(),
            run_id: outcome.run,
            capability: outcome.capability,
            lock_path,
            _lock: lock,
            ended: false,
        })
    }

    /// Bind the next generation's seeded native session to the run.
    pub(crate) fn bind_child(&self, native_session_id: Option<&str>) -> Result<u64> {
        let outcome = crate::relay_mail::child_bind(
            &self.project_root,
            &self.run_id,
            &self.capability,
            native_session_id,
        )?;
        Ok(outcome.generation)
    }

    /// Record the run's terminal generation. Idempotent.
    pub(crate) fn end(&mut self, reason: crate::relay_mail::TerminalReason) {
        if self.ended {
            return;
        }
        self.ended = true;
        if let Err(e) =
            crate::relay_mail::end(&self.project_root, &self.run_id, &self.capability, reason)
        {
            eprintln!("warning: rotation run {} not ended: {e:#}", self.run_id);
        }
    }
}

impl Drop for RotationRun {
    fn drop(&mut self) {
        self.end(crate::relay_mail::TerminalReason::Exit);
        let _ = std::fs::remove_file(&self.lock_path);
    }
}

/// A live rotation's position, as `aida agent ls` shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RotationStatus {
    /// The generation now bound (1 for the first child).
    pub generation: u64,
    /// When the current generation replaced the previous one.
    pub last_rotated_at: Option<DateTime<Utc>>,
}

/// PURE: a run's status from its child bindings, newest last.
pub(crate) fn rotation_status(
    children: &[crate::relay_mail::ChildBinding],
) -> Option<RotationStatus> {
    let last = children.last()?;
    // Run generation 1 is the bind; each child bind advances it by one.
    let generation = last.generation.saturating_sub(1).max(1);
    Some(RotationStatus {
        generation,
        last_rotated_at: (generation >= 2).then_some(last.bound_at),
    })
}

/// Live relay runs by label (the handle they hold).
pub(crate) fn rotation_census(
    project_root: &Path,
) -> std::collections::BTreeMap<String, RotationStatus> {
    let (runs, _warnings) = crate::relay_mail::accepting_runs(project_root);
    runs.into_iter()
        .filter_map(|run| {
            let record = crate::relay_mail::load_run(project_root, &run.run_id).ok()??;
            Some((run.label, rotation_status(&record.children)?))
        })
        .collect()
}

/// PURE: the `aida agent ls` handle cell for a rotated session: `h #3`.
pub(crate) fn ls_handle_label(handle: &str, status: Option<&RotationStatus>) -> String {
    match status {
        Some(s) if s.generation >= 2 => format!("{handle} #{}", s.generation),
        _ => handle.to_string(),
    }
}

/// PURE: the trailing `ls` note naming the last rotation.
pub(crate) fn ls_rotation_note(status: Option<&RotationStatus>, now: DateTime<Utc>) -> String {
    match status.and_then(|s| s.last_rotated_at) {
        Some(at) => format!(
            "  rotated {} ago",
            crate::agent_registry::humanize_elapsed(crate::agent_registry::elapsed_secs_clamped(
                now, at
            ))
        ),
        None => String::new(),
    }
}

#[cfg(test)]
#[path = "tests/agent_rotation_tests.rs"]
mod tests;
