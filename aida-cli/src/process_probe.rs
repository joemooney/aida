//! Cross-platform process inspection for session liveness.
//!
//! STORY-69 foundation: enumerate live `claude` (Claude Code) processes, map
//! each to its working directory, and best-effort surface the active session
//! jsonl. Used by:
//!
//!   - `aida session list`            — mark live sessions in the table
//!   - `aida session prune`           — refuse to delete live jsonls
//!   - `aida session leases --verbose` — warn on stale (deleted) cwd
//!   - `aida session end` (BUG-61)    — refuse to remove a worktree with a
//!     live claude inside; offer --force
//!   - `aida session show` (STORY-68) — show whether the lease has a live
//!     claude attached
//!   - `aida session end` (STORY-73) — resolve the lease whose creator_pid
//!     is an ancestor of the calling shell
//!
//! Why mtime over fd inspection: tested on Linux 2026-05-09, `/proc/<pid>/fd/`
//! does NOT contain any per-session jsonl for live claude processes — only
//! `~/.claude/history.jsonl` is held open. Session jsonls are
//! opened-append-closed per tool call, so fd inspection always sees them
//! closed. The recent-mtime heuristic is the only viable signal.
//!
//! trace:STORY-69 | ai:claude

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use sysinfo::{ProcessRefreshKind, RefreshKind, System};

/// Live `claude` process candidate paired with its (best-effort) project cwd
/// and recently-touched session jsonl.
#[derive(Debug, Clone)]
pub struct LiveSession {
    pub pid: u32,
    /// Cwd of the process. If `stale_cwd` is true, this is the path with the
    /// `(deleted)` suffix stripped — i.e., the path the process WAS in before
    /// the inode was unlinked.
    pub cwd: PathBuf,
    /// Best-effort: the most-recently-touched session jsonl under
    /// `~/.claude/projects/<encoded-cwd>/`, if mtime within
    /// [`RECENT_JSONL_WINDOW`].
    pub jsonl: Option<PathBuf>,
    /// True if /proc reported the cwd as `<path> (deleted)` — i.e., the
    /// directory inode is unlinked but the process still holds it open.
    /// This is the signature of BUG-61 (session end removed the worktree
    /// but didn't terminate claude).
    pub stale_cwd: bool,
}

/// Window for "this jsonl was just written" — short enough that a quiescent
/// session won't be classified as live, long enough to absorb a normal
/// inter-tool-call gap.
pub const RECENT_JSONL_WINDOW: Duration = Duration::from_secs(60);

/// Enumerate live `claude` Claude Code processes on this host.
///
/// Returns an empty vec on platforms where sysinfo can't read process info
/// (Windows pre-Vista, restricted procfs). Never panics — degrades to mtime-
/// only callers without crashing.
pub fn probe_live_claude_sessions() -> Vec<LiveSession> {
    let mut sys = System::new_with_specifics(
        RefreshKind::new().with_processes(
            ProcessRefreshKind::new()
                .with_cwd(sysinfo::UpdateKind::Always)
                .with_cmd(sysinfo::UpdateKind::Always),
        ),
    );
    sys.refresh_processes_specifics(
        ProcessRefreshKind::new()
            .with_cwd(sysinfo::UpdateKind::Always)
            .with_cmd(sysinfo::UpdateKind::Always),
    );

    let mut out = Vec::new();
    for proc in sys.processes().values() {
        // sysinfo on Linux enumerates per-thread entries from /proc/<tgid>/task/*.
        // We only want process leaders — `thread_kind()` returns Some(_) for
        // worker threads, None for the actual process. Without this filter a
        // single multi-threaded `claude` shows up 16+ times.
        if proc.thread_kind().is_some() {
            continue;
        }
        if !is_claude_process(proc.name(), proc.cmd()) {
            continue;
        }
        let raw_cwd = match proc.cwd() {
            Some(p) => p.to_path_buf(),
            None => continue,
        };
        let (cwd, stale_cwd) = strip_deleted_suffix(&raw_cwd);
        let jsonl = if stale_cwd {
            None
        } else {
            recent_jsonl_in_project(&cwd)
        };
        out.push(LiveSession {
            pid: proc.pid().as_u32(),
            cwd,
            jsonl,
            stale_cwd,
        });
    }
    out
}

/// Heuristic: does this look like a Claude Code process? `claude` matches by
/// name on Linux (the binary is literally named `claude`); on macOS the name
/// can be truncated. Falling back to scanning the command line for a
/// `claude` token catches edge cases without false-positiving on `clauded`
/// or `claude-something`.
fn is_claude_process(name: &str, cmd: &[String]) -> bool {
    if name == "claude" || name == "Claude" || name == "Claude Code" {
        return true;
    }
    cmd.iter().any(|arg| {
        let bare = Path::new(arg)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(arg);
        bare == "claude" || bare == "Claude" || bare == "Claude Code"
    })
}

/// Linux's procfs reports the cwd of a process whose directory has been
/// unlinked as `<original-path> (deleted)`. sysinfo preserves this in the
/// PathBuf verbatim. Detect it, strip the suffix, and signal staleness so
/// the caller can warn (BUG-61) or skip.
fn strip_deleted_suffix(p: &Path) -> (PathBuf, bool) {
    let s = p.to_string_lossy();
    if let Some(real) = s.strip_suffix(" (deleted)") {
        (PathBuf::from(real), true)
    } else {
        (p.to_path_buf(), false)
    }
}

/// Find the most-recently-modified `*.jsonl` under
/// `~/.claude/projects/<encoded-cwd>/`, if its mtime is within
/// [`RECENT_JSONL_WINDOW`] of now. Returns `None` when no project dir
/// exists, no jsonl is present, or the newest one is older than the window.
///
/// Encoding follows Claude Code's own convention: replace each `/` in the
/// absolute cwd with `-`, including the leading slash. So
/// `/home/joe/ai/aida-epic-20` → `-home-joe-ai-aida-epic-20`. The decoding
/// is ambiguous (a literal `-` in the path is indistinguishable from `/`),
/// but we only need to encode forwards, not decode.
pub fn recent_jsonl_in_project(cwd: &Path) -> Option<PathBuf> {
    let proj_dir = claude_projects_dir_for_cwd(cwd)?;
    let now = SystemTime::now();
    let mut newest: Option<(SystemTime, PathBuf)> = None;
    let entries = std::fs::read_dir(&proj_dir).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        let mtime = entry.metadata().and_then(|m| m.modified()).ok();
        if let Some(t) = mtime {
            if newest.as_ref().map(|(prev, _)| t > *prev).unwrap_or(true) {
                newest = Some((t, p));
            }
        }
    }
    let (mtime, path) = newest?;
    let age = now.duration_since(mtime).unwrap_or(Duration::ZERO);
    if age <= RECENT_JSONL_WINDOW {
        Some(path)
    } else {
        None
    }
}

/// Path to `~/.claude/projects/<encoded-cwd>/` for a given absolute cwd, or
/// `None` if HOME isn't set or the directory doesn't exist.
pub fn claude_projects_dir_for_cwd(cwd: &Path) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let encoded = encode_cwd_for_projects(cwd);
    let candidate = home.join(".claude").join("projects").join(encoded);
    if candidate.is_dir() {
        Some(candidate)
    } else {
        None
    }
}

/// Encode a cwd to Claude Code's project-dir naming convention: replace each
/// `/` (including leading) with `-`.
pub fn encode_cwd_for_projects(cwd: &Path) -> String {
    let s = cwd.to_string_lossy().to_string();
    // Claude Code's project directory slug replaces path separators with
    // hyphens. Tests and cross-platform paths can contain either separator
    // spelling, so normalize both rather than keying off MAIN_SEPARATOR.
    // trace:BUG-346 | ai:codex
    s.replace(['\\', '/'], "-")
}

/// Walk the chain of parent PIDs starting from `start` (typically the PID of
/// the calling shell — get with `std::process::id()`'s parent if you can, or
/// just `start = std::process::id()` and accept that the immediate parent is
/// usually `aida` itself which is fine: ancestors of aida are ancestors of
/// the shell that invoked it).
///
/// The chain INCLUDES the start pid. Stops at PID 1 (init) or when a PPid
/// can't be read. Used by STORY-73 to match a calling shell against any
/// session lease's `creator_pid`.
pub fn walk_ancestor_pids(start: u32) -> Vec<u32> {
    let mut sys =
        System::new_with_specifics(RefreshKind::new().with_processes(ProcessRefreshKind::new()));
    sys.refresh_processes_specifics(ProcessRefreshKind::new());
    let mut chain = Vec::new();
    let mut cur = sysinfo::Pid::from_u32(start);
    let mut seen = std::collections::HashSet::new();
    while seen.insert(cur) {
        chain.push(cur.as_u32());
        let Some(proc) = sys.process(cur) else { break };
        let Some(parent) = proc.parent() else { break };
        if parent == sysinfo::Pid::from_u32(1) || parent == cur {
            chain.push(parent.as_u32());
            break;
        }
        cur = parent;
    }
    chain
}

/// Is process `pid` currently alive? A thin wrapper over `sysinfo`'s process
/// table — `true` iff the kernel still has an entry for `pid`.
///
/// Used by BUG-233's orchestrator-run corroboration: a child trusts its
/// `AIDA_AUTO_COMPLETE_TOKEN` only when the marker file it names records a PID
/// that is still running. Returns `false` on platforms where the process table
/// can't be read — corroboration fails safe (treat as not-orchestrated).
/// trace:BUG-233 | ai:claude
pub fn pid_is_alive(pid: u32) -> bool {
    use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, System};
    let mut sys =
        System::new_with_specifics(RefreshKind::new().with_processes(ProcessRefreshKind::new()));
    sys.refresh_processes_specifics(ProcessRefreshKind::new());
    sys.process(Pid::from_u32(pid)).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_cwd_replaces_slashes() {
        let p = Path::new("/home/joe/ai/aida-epic-20");
        assert_eq!(encode_cwd_for_projects(p), "-home-joe-ai-aida-epic-20");
    }

    #[test]
    fn encode_cwd_replaces_windows_backslashes() {
        let p = Path::new(r"C:\Users\joe\ai\aida-epic-20");
        assert_eq!(encode_cwd_for_projects(p), "C:-Users-joe-ai-aida-epic-20");
    }

    #[test]
    fn encode_cwd_handles_root() {
        let p = Path::new("/");
        assert_eq!(encode_cwd_for_projects(p), "-");
    }

    #[test]
    fn strip_deleted_recognises_proc_suffix() {
        let (real, stale) = strip_deleted_suffix(Path::new("/foo/bar (deleted)"));
        assert_eq!(real, Path::new("/foo/bar"));
        assert!(stale);
    }

    #[test]
    fn strip_deleted_passthrough_when_clean() {
        let (real, stale) = strip_deleted_suffix(Path::new("/foo/bar"));
        assert_eq!(real, Path::new("/foo/bar"));
        assert!(!stale);
    }

    #[test]
    fn is_claude_process_matches_name() {
        assert!(is_claude_process("claude", &[]));
        assert!(is_claude_process("Claude", &[]));
        assert!(!is_claude_process("clauded", &[]));
        assert!(!is_claude_process("claude-helper", &[]));
    }

    #[test]
    fn is_claude_process_falls_back_to_cmd() {
        assert!(is_claude_process(
            "node",
            &[
                "/usr/local/bin/claude".to_string(),
                "--something".to_string()
            ],
        ));
        assert!(!is_claude_process(
            "node",
            &["/usr/local/bin/clauded".to_string()]
        ));
    }

    #[test]
    fn walk_ancestor_pids_includes_self_and_terminates() {
        let me = std::process::id();
        let chain = walk_ancestor_pids(me);
        assert!(!chain.is_empty());
        assert_eq!(chain[0], me);
        // Should reach init (PID 1) or some root, not loop forever.
        assert!(chain.len() < 100);
    }

    #[test]
    fn pid_is_alive_true_for_self() {
        assert!(pid_is_alive(std::process::id()));
    }

    #[test]
    fn pid_is_alive_false_for_unused_pid() {
        // A PID near u32::MAX is astronomically unlikely to be in use — well
        // above any real `pid_max` (Linux caps at ~4M, far below this).
        assert!(!pid_is_alive(u32::MAX - 1));
    }
}
