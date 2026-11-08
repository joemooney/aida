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

/// PURE: whether `agent_type` can be supervised across rotations.
// trace:STORY-B6B1278890774EF993D2EBC327862470-150 | ai:claude
pub(crate) fn rotation_supported(agent_type: &str) -> bool {
    matches!(agent_type, "claude" | "codex")
}

/// PURE: whether a launch is supervised. Claude and Codex only; an explicit
/// `--rotate` always is, while `[agents.<vendor>] rotate = true` only applies
/// when stdin is a terminal, so a scripted or piped launch never relaunches
/// unless it asked to.
// trace:STORY-B6B1278890774EF993D2EBC327862470-150 | ai:claude
pub(crate) fn rotation_enabled(
    agent_type: &str,
    explicit: bool,
    config_default: bool,
    stdin_is_tty: bool,
) -> bool {
    rotation_supported(agent_type) && (explicit || (config_default && stdin_is_tty))
}

/// PURE: the native session id a generation's handoff must carry. Claude's
/// is seeded at launch; Codex's thread id is known only after startup, when
/// its SessionStart hook records it on the generation's registry row
/// (`learned`). With neither, freshness falls back to `ts >= spawn` plus the
/// handle match.
// trace:STORY-B6B1278890774EF993D2EBC327862470-150 | ai:claude
pub(crate) fn generation_session_id(seeded: Option<&str>, learned: Option<&str>) -> Option<String> {
    seeded
        .or(learned)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
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
    /// The generation's native session id: seeded by the launcher (Claude),
    /// or the thread the child's hook recorded (Codex); see
    /// [`generation_session_id`].
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

// trace:STORY-2653 | ai:codex
fn bounded_rotation_file(path: &Path, limit: u64) -> std::io::Result<String> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(std::io::Error::other(
            "rotation evidence is not a bounded regular file",
        ));
    }
    let mut content = String::new();
    file.take(limit + 1).read_to_string(&mut content)?;
    if content.len() as u64 > limit {
        return Err(std::io::Error::other(
            "rotation evidence grew beyond its limit",
        ));
    }
    Ok(content)
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
    let content = match bounded_rotation_file(Path::new(path), 32_000) {
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

// trace:STORY-2653 | ai:codex
#[derive(serde::Serialize, serde::Deserialize)]
struct TurnRotation {
    session: String,
    notice: DateTime<Utc>,
    boundary: Option<DateTime<Utc>>,
}

fn turn_path(root: &Path, seat: &str, handle: &str, generation: u64) -> Option<PathBuf> {
    let seat = crate::seat_rotation::sanitize_seat(seat).ok()?;
    // Handles are data, never path components supplied unchecked by a hook.
    let handle = crate::seat_rotation::sanitize_seat(handle).ok()?;
    Some(
        root.join(".aida/handoff")
            .join(seat)
            .join(format!(".rotation-{handle}-{generation}.json")),
    )
}

fn read_turn(path: &Path) -> Option<TurnRotation> {
    serde_json::from_str(&bounded_rotation_file(path, 4096).ok()?).ok()
}

/// Called at the existing hook entry boundary. Stop only stamps a boundary;
/// the launcher owns signalling and reaping its own child.
// trace:STORY-2653 | ai:codex
pub(crate) fn hook_turn(
    root: &Path,
    seat: &str,
    payload: &str,
    notice_active: bool,
    writer: &crate::seat_rotation::HandoffWriter,
) -> Result<()> {
    let value: serde_json::Value = serde_json::from_str(payload)?;
    let Some(session) = value
        .get("session_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    else {
        return Ok(());
    };
    let (Some(handle), Some(generation)) = (writer.handle.as_deref(), writer.generation) else {
        return Ok(());
    };
    let Some(path) = turn_path(root, seat, handle, generation) else {
        return Ok(());
    };
    let now = Utc::now();
    let mut state = read_turn(&path).filter(|s| s.session == session);
    if value.get("hook_event_name").and_then(|v| v.as_str()) == Some("Stop") {
        let Some(mut current) = state else {
            return Ok(());
        };
        let records = read_handoff_log(root, seat);
        let run = GenerationRun {
            seat,
            handle,
            native_session_id: Some(session),
            spawned_at: current.notice,
        };
        let Some(handoff) =
            fresh_handoff(&records, &run).filter(|h| !h.final_ && h.ts > current.notice)
        else {
            return Ok(());
        };
        let handoff_path = root.join(&handoff.path);
        let mut content =
            matched_handoff(&handoff_path.to_string_lossy(), &handoff.ts.to_rfc3339())
                .map_err(anyhow::Error::msg)?;
        if !content.contains("<!-- automatic rotation context -->") {
            content.push_str(&rotation_context(
                &value,
                crate::gate::seat_authority::current_grant_id().as_deref(),
            ));
            let tmp = handoff_path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
            std::fs::write(&tmp, &content)?;
            std::fs::rename(tmp, &handoff_path)?;
            // The supervisor loads this exact stamped file. Leave latest.md
            // alone: a concurrent sibling may have published a newer handoff.
        }
        current.boundary = Some(now);
        state = Some(current);
    } else if notice_active && state.is_none() {
        state = Some(TurnRotation {
            session: session.into(),
            notice: now,
            boundary: None,
        });
    } else {
        // A compacted/below-ceiling turn disarms the earlier notice. Missing
        // usage evidence also fails closed rather than extending old intent.
        if !notice_active && state.is_some() {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        return Ok(());
    }
    std::fs::create_dir_all(path.parent().expect("turn parent"))?;
    let tmp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    std::fs::write(&tmp, serde_json::to_vec(&state.expect("turn state"))?)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

/// Snapshot only goal tool arguments, never arbitrary transcript text or
/// process environments. Missing evidence is explicit, not a claim of no goal.
// trace:STORY-2653 | ai:codex
fn rotation_context(payload: &serde_json::Value, grant: Option<&str>) -> String {
    use std::io::{Read, Seek, SeekFrom};
    fn goals(value: &serde_json::Value, found: &mut Vec<String>) {
        match value {
            serde_json::Value::Object(map) => {
                if let Some(name) = map.get("name").and_then(|v| v.as_str()) {
                    if matches!(
                        name.rsplit('.').next(),
                        Some("create_goal" | "update_goal" | "Goal")
                    ) {
                        let args = map.get("arguments").or_else(|| map.get("input"));
                        if let Some(args) = args {
                            if matches!(name.rsplit('.').next(), Some("create_goal" | "Goal")) {
                                found.clear();
                            }
                            found.push(format!("{name}: {args}"));
                        }
                    }
                }
                for value in map.values() {
                    goals(value, found);
                }
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    goals(value, found);
                }
            }
            _ => {}
        }
    }
    let mut found = Vec::new();
    if let Some(path) = payload.get("transcript_path").and_then(|v| v.as_str()) {
        if let Ok(mut file) = std::fs::File::open(path) {
            let start = file
                .metadata()
                .map(|m| m.len().saturating_sub(512 * 1024))
                .unwrap_or(0);
            if file.seek(SeekFrom::Start(start)).is_ok() {
                let mut tail = String::new();
                if file.take(512 * 1024).read_to_string(&mut tail).is_ok() {
                    for (index, line) in tail.lines().enumerate() {
                        if start > 0 && index == 0 {
                            continue;
                        }
                        if let Ok(value) = serde_json::from_str(line) {
                            goals(&value, &mut found);
                        }
                    }
                }
            }
        }
    }
    let mut out = String::from("\n<!-- automatic rotation context -->\n## Session goal evidence\n");
    if found.is_empty() {
        out.push_str("Goal state unknown: no goal tool record in the bounded transcript tail. Recover any session goal from the previous session before continuing.\n");
    } else {
        out.push_str("Recent goal tool records (evidence only; verify current state and recreate any active session goal):\n");
        // Keep the latest creation as well as subsequent updates when possible.
        let evidence = found.join("\n");
        let mut end = evidence.len().min(4096);
        while !evidence.is_char_boundary(end) {
            end -= 1;
        }
        out.push_str(&evidence[..end]);
        if end < evidence.len() {
            out.push_str("\n[goal evidence truncated; recover from previous session]");
        }
        out.push('\n');
    }
    out.push_str("\n## Processes sharing the outgoing grant\nBest-effort local procfs snapshot; processes are not rebound. Restart needed watchers under the new grant.\n");
    let Some(grant) = grant else {
        out.push_str("Grant unavailable; process inventory unknown.\n");
        return out;
    };
    let Ok(entries) = std::fs::read_dir("/proc") else {
        out.push_str("Procfs unavailable; process inventory unknown.\n");
        return out;
    };
    let mut pids = Vec::new();
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        if crate::ps_integrate::read_pid_environ_vars(pid).is_some_and(|vars| {
            vars.get(crate::gate::seat_authority::GRANT_ENV)
                .map(String::as_str)
                == Some(grant)
        }) {
            pids.push(pid);
        }
    }
    pids.sort_unstable();
    for pid in pids.iter().take(32) {
        let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
        let comm: String = comm.chars().filter(|c| !c.is_control()).take(80).collect();
        out.push_str(&format!("- PID {pid} ({comm})\n"));
    }
    if pids.is_empty() {
        out.push_str("No readable matching process environment observed.\n");
    }
    if pids.len() > 32 {
        out.push_str("[process list truncated]\n");
    }
    out
}

/// A bounded poller independent of vendor Stop decisions. It never signals a
/// PID discovered from disk: only the pidfd-pinned child this launcher spawned.
// trace:STORY-2653 | ai:codex
pub(crate) struct TurnMonitor {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl TurnMonitor {
    pub(crate) fn start(
        root: PathBuf,
        seat: String,
        handle: String,
        generation: u64,
        spawned_at: DateTime<Utc>,
        child_pid: u32,
        expected_session: Option<String>,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        // A pidfd pins the child identity even if wait() reaps it before the
        // monitor joins. Never fall back to signalling a recyclable numeric PID.
        use std::os::fd::{AsRawFd, FromRawFd};
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, child_pid, 0) };
        let pidfd = (fd >= 0).then(|| unsafe { std::os::fd::OwnedFd::from_raw_fd(fd as i32) });
        let thread = std::thread::spawn(move || {
            let Some(pidfd) = pidfd else {
                eprintln!("warning: rotation cannot pin child identity; automatic exit disabled");
                return;
            };
            let Some(path) = turn_path(&root, &seat, &handle, generation) else {
                return;
            };
            while !stopped.load(Ordering::SeqCst) {
                if let Some(state) = read_turn(&path).filter(|state| {
                    state.notice >= spawned_at && state.boundary.is_some_and(|b| b > state.notice)
                }) {
                    let session = expected_session.clone().or_else(|| {
                        crate::agent_registry::load_entries(&root)
                            .into_iter()
                            .find(|(_, row)| {
                                row.pid == child_pid && row.name.as_deref() == Some(&handle)
                            })
                            .and_then(|(_, row)| row.native_session_id.or(row.claude_session_id))
                    });
                    if session.as_deref() == Some(state.session.as_str()) {
                        let records = read_handoff_log(&root, &seat);
                        let run = GenerationRun {
                            seat: &seat,
                            handle: &handle,
                            native_session_id: Some(&state.session),
                            spawned_at,
                        };
                        if let Some(handoff) = fresh_handoff(&records, &run).filter(|h| {
                            !h.final_ && h.ts > state.notice && Some(h.ts) <= state.boundary
                        }) {
                            let handoff_path = root.join(&handoff.path);
                            if matched_handoff(
                                &handoff_path.to_string_lossy(),
                                &handoff.ts.to_rfc3339(),
                            )
                            .is_ok()
                            {
                                eprintln!("rotation: fresh handoff at turn boundary; ending {handle} #{generation}");
                                unsafe {
                                    libc::syscall(
                                        libc::SYS_pidfd_send_signal,
                                        pidfd.as_raw_fd(),
                                        libc::SIGTERM,
                                        std::ptr::null::<libc::siginfo_t>(),
                                        0,
                                    );
                                }
                                // A vendor/goal handler may ignore TERM. Only the
                                // pinned child receives escalation, never its process group.
                                for _ in 0..40 {
                                    if stopped.load(Ordering::SeqCst) {
                                        return;
                                    }
                                    std::thread::sleep(std::time::Duration::from_millis(50));
                                }
                                unsafe {
                                    libc::syscall(
                                        libc::SYS_pidfd_send_signal,
                                        pidfd.as_raw_fd(),
                                        libc::SIGKILL,
                                        std::ptr::null::<libc::siginfo_t>(),
                                        0,
                                    );
                                }
                                return;
                            }
                        }
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for TurnMonitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
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
    use std::io::{Read, Seek, SeekFrom};
    use std::os::unix::fs::OpenOptionsExt;
    let read_tail = || -> std::io::Result<String> {
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(std::io::Error::other("handoff log is not regular"));
        }
        let start = metadata.len().saturating_sub(1024 * 1024);
        file.seek(SeekFrom::Start(start))?;
        let mut content = String::new();
        file.take(1024 * 1024).read_to_string(&mut content)?;
        if start > 0 {
            content = content
                .split_once('\n')
                .map(|(_, tail)| tail.to_string())
                .unwrap_or_default();
        }
        Ok(content)
    };
    read_tail()
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

/// Serializes command-level supervisor tests: a supervisor forwards a
/// SIGINT the test process receives to its child, so one test's Ctrl-C
/// would kill another test's running generation.
// trace:STORY-B6B1278890774EF993D2EBC327862470-150 | ai:claude
#[cfg(test)]
pub(crate) fn supervisor_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
#[path = "tests/agent_rotation_tests.rs"]
mod tests;
