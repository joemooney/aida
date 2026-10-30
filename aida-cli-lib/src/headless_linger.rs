//! Reap a headless vendor that lingers after its final result.
//!
//! A headless `claude -p` writes its terminal `{"type":"result"}` event and
//! should exit, but a child it spawned (an `aida mcp-serve` MCP server) can
//! keep it alive indefinitely. The launcher waiting on it then holds its
//! lease, its lane record and its standalone-implementer cap slot long after
//! the work finished (BUG-027: 31 minutes, until a human killed it).
//!
//! [`wait_reaping_after_result`] replaces a bare `child.wait()`: it watches
//! the child's stream-json log, and once the final result is logged it gives
//! the vendor a bounded grace period to exit on its own, then terminates the
//! vendor's whole process tree, MCP children included.
//!
//! trace:BUG-B6B1278890774EF993D2EBC327862470-027 | ai:claude

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus};
use std::time::{Duration, Instant};

/// How long a vendor may keep running after its final result before its
/// tree is terminated.
pub(crate) const DEFAULT_RESULT_GRACE: Duration = Duration::from_secs(30);

/// Overrides [`DEFAULT_RESULT_GRACE`], in whole seconds.
pub(crate) const RESULT_GRACE_ENV: &str = "AIDA_HEADLESS_RESULT_GRACE_SECS";

/// How long a SIGTERMed tree may take to exit before it is SIGKILLed.
const TERM_GRACE: Duration = Duration::from_secs(5);

const POLL: Duration = Duration::from_millis(250);

/// The grace period after the final result: [`RESULT_GRACE_ENV`] when set
/// to a whole number of seconds, else [`DEFAULT_RESULT_GRACE`].
pub(crate) fn result_grace() -> Duration {
    std::env::var(RESULT_GRACE_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_RESULT_GRACE)
}

/// The vendor's final result event, as read from its stream-json log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FinalResult {
    pub(crate) is_error: bool,
}

/// `line` as a final result event: stream-json `{"type":"result", ...}`.
pub(crate) fn parse_final_result(line: &str) -> Option<FinalResult> {
    // Cheap prefilter: most lines are assistant / tool events.
    if !line.contains("\"result\"") {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    if value.get("type").and_then(|t| t.as_str()) != Some("result") {
        return None;
    }
    Some(FinalResult {
        is_error: value
            .get("is_error")
            .and_then(|e| e.as_bool())
            .unwrap_or(false),
    })
}

/// Whether the log at `path` already holds a final result event.
pub(crate) fn log_has_final_result(path: &Path) -> bool {
    let mut watch = FinalResultWatch::new(path);
    watch.poll().is_some()
}

/// Incrementally scans a growing log for the final result event.
pub(crate) struct FinalResultWatch {
    path: PathBuf,
    pos: u64,
    partial: Vec<u8>,
    found: Option<FinalResult>,
}

impl FinalResultWatch {
    pub(crate) fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            pos: 0,
            partial: Vec::new(),
            found: None,
        }
    }

    /// Read what was appended since the last poll; the final result once
    /// any complete line carried it.
    pub(crate) fn poll(&mut self) -> Option<FinalResult> {
        if self.found.is_some() {
            return self.found;
        }
        let Ok(mut file) = std::fs::File::open(&self.path) else {
            return None;
        };
        if file.seek(SeekFrom::Start(self.pos)).is_err() {
            return None;
        }
        let mut buf = Vec::new();
        let Ok(read) = file.read_to_end(&mut buf) else {
            return None;
        };
        self.pos += read as u64;
        self.partial.extend_from_slice(&buf);
        while let Some(nl) = self.partial.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=nl).collect();
            if let Some(found) = parse_final_result(&String::from_utf8_lossy(&line)) {
                self.found = Some(found);
                self.partial.clear();
                break;
            }
        }
        self.found
    }
}

/// How [`wait_reaping_after_result`] ended.
#[derive(Debug)]
pub(crate) struct LingerWait {
    /// The child's status. When the child was terminated after a final
    /// result, this is the result's verdict (success unless `is_error`), not
    /// the termination signal.
    pub(crate) status: ExitStatus,
    /// The child outlived the grace period and its tree was terminated.
    pub(crate) reaped: bool,
}

/// Wait for `child`, whose stream-json output goes to `log_path`. Once the
/// final result is logged, the child gets `grace` to exit; past it, its
/// whole process tree is SIGTERMed, then SIGKILLed.
#[cfg(unix)]
pub(crate) fn wait_reaping_after_result(
    child: &mut Child,
    log_path: &Path,
    grace: Duration,
) -> std::io::Result<LingerWait> {
    let mut watch = FinalResultWatch::new(log_path);
    let mut result_seen: Option<(Instant, FinalResult)> = None;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(LingerWait {
                status,
                reaped: false,
            });
        }
        if result_seen.is_none() {
            result_seen = watch.poll().map(|r| (Instant::now(), r));
        }
        if let Some((seen_at, result)) = result_seen {
            if seen_at.elapsed() >= grace {
                terminate_tree(child)?;
                use std::os::unix::process::ExitStatusExt;
                let code = i32::from(result.is_error);
                return Ok(LingerWait {
                    status: ExitStatus::from_raw(code << 8),
                    reaped: true,
                });
            }
        }
        std::thread::sleep(POLL);
    }
}

#[cfg(not(unix))]
pub(crate) fn wait_reaping_after_result(
    child: &mut Child,
    _log_path: &Path,
    _grace: Duration,
) -> std::io::Result<LingerWait> {
    Ok(LingerWait {
        status: child.wait()?,
        reaped: false,
    })
}

/// SIGTERM `child`'s tree, give it [`TERM_GRACE`], then SIGKILL whatever of
/// the original tree is still the same process, and reap `child`. A
/// descendant orphaned by its parent's exit is still in the collected tree,
/// so it is killed too.
#[cfg(unix)]
fn terminate_tree(child: &mut Child) -> std::io::Result<()> {
    let root = child.id();
    let tree: Vec<(u32, Option<String>)> = crate::exit_signal::collect_process_tree(root)
        .into_iter()
        .map(|pid| (pid, aida_core::liveness::process_start_identity(pid)))
        .collect();
    let pids: Vec<u32> = tree.iter().map(|(pid, _)| *pid).collect();
    crate::forwarded_signals::signal_child_tree(root, &pids, libc::SIGTERM);
    let deadline = Instant::now() + TERM_GRACE;
    let tree_alive = |tree: &[(u32, Option<String>)]| -> Vec<u32> {
        tree.iter()
            .filter(|(pid, start)| {
                start.is_some()
                    && aida_core::liveness::process_identity_is_alive(*pid, start.as_deref())
            })
            .map(|(pid, _)| *pid)
            .collect()
    };
    while Instant::now() < deadline {
        let _ = child.try_wait()?;
        if tree_alive(&tree).is_empty() {
            break;
        }
        std::thread::sleep(POLL);
    }
    let survivors = tree_alive(&tree);
    if !survivors.is_empty() {
        crate::forwarded_signals::signal_child_tree(root, &survivors, libc::SIGKILL);
    }
    let _ = child.kill();
    child.wait().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_final_result_reads_the_result_event_only() {
        assert_eq!(
            parse_final_result(r#"{"type":"result","subtype":"success","is_error":false}"#),
            Some(FinalResult { is_error: false })
        );
        assert_eq!(
            parse_final_result(r#"{"type":"result","is_error":true}"#),
            Some(FinalResult { is_error: true })
        );
        // A tool result carries a "result" key but is not the final event.
        assert_eq!(
            parse_final_result(r#"{"type":"user","message":{"result":"x"}}"#),
            None
        );
        assert_eq!(parse_final_result("not json \"result\""), None);
    }

    #[test]
    fn watch_sees_a_result_appended_after_a_partial_line() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("run.jsonl");
        let mut file = std::fs::File::create(&log).unwrap();
        writeln!(file, r#"{{"type":"assistant"}}"#).unwrap();
        write!(file, r#"{{"type":"res"#).unwrap();
        file.flush().unwrap();
        let mut watch = FinalResultWatch::new(&log);
        assert_eq!(watch.poll(), None);
        assert!(!log_has_final_result(&log));
        writeln!(file, r#"ult","is_error":false}}"#).unwrap();
        file.flush().unwrap();
        assert_eq!(watch.poll(), Some(FinalResult { is_error: false }));
        assert!(log_has_final_result(&log));
    }

    /// The BUG-027 shape: the vendor logs its result, then stays alive on a
    /// child of its own. Past the grace the whole tree is gone and the
    /// status is the result's verdict.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_vendor_lingering_after_its_result_is_reaped_with_its_children() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("run.jsonl");
        let grandchild_pid = dir.path().join("grandchild.pid");
        let script = format!(
            "sleep 300 & echo $! > {gc}; \
             echo '{{\"type\":\"result\",\"is_error\":false}}' >> {log}; \
             wait",
            gc = grandchild_pid.display(),
            log = log.display()
        );
        let mut child = std::process::Command::new("sh")
            .arg("-c")
            .arg(&script)
            .spawn()
            .unwrap();
        let started = Instant::now();
        let out = wait_reaping_after_result(&mut child, &log, Duration::from_millis(200)).unwrap();
        assert!(out.reaped, "the lingering vendor must be terminated");
        assert!(out.status.success(), "a clean result reads as success");
        assert!(started.elapsed() < Duration::from_secs(20));
        let gc: u32 = std::fs::read_to_string(&grandchild_pid)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while aida_core::liveness::process_identity_is_alive(gc, None) && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            !aida_core::liveness::process_identity_is_alive(gc, None),
            "the vendor's child (the MCP server's stand-in) must be killed too"
        );
    }

    /// A vendor that exits on its own is waited normally, result or not.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_vendor_that_exits_is_not_reaped() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("run.jsonl");
        let mut child = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!(
                "echo '{{\"type\":\"result\",\"is_error\":false}}' >> {}; exit 3",
                log.display()
            ))
            .spawn()
            .unwrap();
        let out = wait_reaping_after_result(&mut child, &log, Duration::from_secs(30)).unwrap();
        assert!(!out.reaped);
        assert_eq!(out.status.code(), Some(3));
    }
}
