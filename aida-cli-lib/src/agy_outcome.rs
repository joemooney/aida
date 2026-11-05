//! Structured AGY evidence is independent of process exit and phase handshakes.
// trace:TASK-2838 | ai:codex
use std::collections::BTreeMap;
use std::io::{BufRead, Read, Write};
use std::path::Path;
use std::process::ExitStatus;

pub(crate) const INCOMPLETE: i32 = 65;
const MAX_LINE: usize = 1024 * 1024;
const MAX_TOOLS: usize = 256;

#[derive(Default, Debug)]
pub(crate) struct Evidence {
    terminal: Option<bool>,
    denied: bool,
    unresolved: BTreeMap<String, Option<u64>>,
    errors: usize,
    invalid: bool,
}

impl Evidence {
    fn line(&mut self, line: &[u8], quiet: bool) {
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(line) else {
            self.invalid = true;
            return;
        };
        match value["event"].as_str() {
            Some("result") => {
                let result = &value["result"];
                self.terminal = match result["status"].as_str() {
                    Some("SUCCESS") => Some(true),
                    Some("ERROR" | "FAILURE" | "FAILED" | "CANCELLED") => Some(false),
                    _ => None,
                };
                self.denied |= match result.get("denied_actions") {
                    None => false,
                    Some(serde_json::Value::Array(actions)) => !actions.is_empty(),
                    Some(_) => {
                        self.invalid = true;
                        true
                    }
                };
                if quiet {
                    if let Some(response) = result["response"].as_str() {
                        println!("{response}");
                    }
                }
            }
            Some("step_update") => {
                let step = &value["step_update"];
                if step["step_type"] != "tool" {
                    return;
                }
                let name = step["tool_name"]
                    .as_str()
                    .or(step["tool_info"]["name"].as_str());
                let failed =
                    step["state"] == "ERROR" || step["tool_info"]["error"]["type"] == "TOOL_ERROR";
                if failed {
                    self.errors = self.errors.saturating_add(1);
                    if let Some(name) = name.filter(|n| n.len() <= 256) {
                        if self.unresolved.len() < MAX_TOOLS {
                            self.unresolved
                                .insert(name.to_owned(), step["step_index"].as_u64());
                        } else {
                            self.invalid = true;
                        }
                    } else {
                        self.invalid = true;
                    }
                } else if step["state"] == "DONE" && step["tool_info"]["error"].is_null() {
                    if let Some(name) = name {
                        // DONE on the original failed step is not a retry.
                        if self.unresolved.get(name).is_some_and(|failed| {
                            failed
                                .zip(step["step_index"].as_u64())
                                .is_some_and(|(before, after)| after > before)
                        }) {
                            self.unresolved.remove(name);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Bounded memory, including malformed lines; never interpret model prose.
    pub(crate) fn read(
        reader: impl Read,
        mut forward: Option<&mut dyn Write>,
        quiet: bool,
    ) -> std::io::Result<Self> {
        let mut evidence = Self::default();
        let mut reader = std::io::BufReader::new(reader);
        let mut line = Vec::new();
        let mut overflow = false;
        loop {
            let bytes = match reader.fill_buf() {
                Ok(bytes) => bytes,
                Err(error) => {
                    eprintln!("AGY outcome: incomplete stdout evidence: {error}");
                    evidence.invalid = true;
                    break;
                }
            };
            if bytes.is_empty() {
                if !line.is_empty() || overflow {
                    evidence.invalid = true; // truncated JSONL is not terminal evidence
                }
                break;
            }
            let end = bytes.iter().position(|b| *b == b'\n');
            let count = end.map_or(bytes.len(), |i| i + 1);
            if let Some(writer) = forward.as_deref_mut() {
                if let Err(error) = writer
                    .write_all(&bytes[..count])
                    .and_then(|()| writer.flush())
                {
                    eprintln!("AGY outcome: log forwarding failed: {error}");
                    evidence.invalid = true;
                    forward = None;
                }
            }
            if !overflow && line.len() + count <= MAX_LINE {
                line.extend_from_slice(&bytes[..count]);
            } else {
                overflow = true;
                evidence.invalid = true;
                line.clear();
            }
            reader.consume(count);
            if end.is_some() {
                if !overflow {
                    evidence.line(&line, quiet);
                }
                line.clear();
                overflow = false;
            }
        }
        Ok(evidence)
    }

    pub(crate) fn status(&self, process: ExitStatus) -> ExitStatus {
        if self.errors > 0 || self.denied || self.invalid || self.terminal != Some(true) {
            eprintln!(
                "AGY outcome: terminal={:?}, denied_actions={}, tool_errors={}, unresolved_tools={:?}, invalid_stream={}",
                self.terminal, self.denied, self.errors, self.unresolved, self.invalid
            );
        }
        if !process.success() {
            return process;
        }
        let code = if self.terminal == Some(false) {
            1
        } else if self.terminal.is_none()
            || self.denied
            || !self.unresolved.is_empty()
            || self.invalid
        {
            eprintln!("AGY outcome: incomplete (missing/invalid terminal evidence or unresolved tool failure)");
            INCOMPLETE
        } else {
            0
        };
        use std::os::unix::process::ExitStatusExt;
        ExitStatus::from_raw(code << 8)
    }
}

pub(crate) fn log_status(path: &Path, process: ExitStatus) -> ExitStatus {
    match std::fs::File::open(path).and_then(|file| Evidence::read(file, None, false)) {
        Ok(evidence) => evidence.status(process),
        Err(error) => {
            eprintln!("AGY outcome: unreadable evidence: {error}");
            Evidence::default().status(process)
        }
    }
}

// trace:TASK-2838 | ai:codex
// Only incomplete AGY evidence may proceed to the existing fresh-verdict gate.
pub(crate) fn reviewer_may_check_verdict(
    vendor: crate::session::HeadlessVendor,
    status: ExitStatus,
) -> bool {
    status.success()
        || (vendor == crate::session::HeadlessVendor::Agy && status.code() == Some(INCOMPLETE))
}

// A descendant can retain stdout after the vendor exits. Polling allows the
// reader to be cancelled without leaking a blocked thread or killing helpers.
struct CoordinatorPipe {
    output: std::process::ChildStdout,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Read for CoordinatorPipe {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        use std::os::fd::AsRawFd;
        use std::sync::atomic::Ordering;
        loop {
            if self.stop.load(Ordering::Acquire) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "stdout remained open after AGY exited",
                ));
            }
            let mut fd = libc::pollfd {
                fd: self.output.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: fd points to one initialized pollfd for the owned pipe.
            let ready = unsafe { libc::poll(&mut fd, 1, 50) };
            if ready > 0 {
                return self.output.read(bytes);
            }
            if ready < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
        }
    }
}

// trace:TASK-2838 | ai:codex
pub(crate) fn coordinator_status(
    child: &mut std::process::Child,
    mut forward: Option<std::fs::File>,
) -> std::io::Result<ExitStatus> {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    };
    let stop = Arc::new(AtomicBool::new(false));
    let output = CoordinatorPipe {
        output: child.stdout.take().expect("AGY stdout was piped"),
        stop: stop.clone(),
    };
    let (send, receive) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let quiet = forward.is_none();
        let evidence = Evidence::read(
            output,
            forward.as_mut().map(|file| file as &mut dyn Write),
            quiet,
        );
        let _ = send.send(evidence);
    });
    let status = child.wait();
    let received = receive.recv_timeout(std::time::Duration::from_millis(250));
    stop.store(true, Ordering::Release);
    let _ = reader.join();
    // Cancellation retains any terminal failure already parsed before the
    // incomplete tail. It must never turn explicit failure into exit 65.
    let evidence = match received.or_else(|_| receive.try_recv().map_err(|_| ())) {
        Ok(Ok(evidence)) => evidence,
        other => {
            eprintln!("AGY outcome: incomplete stdout evidence: {other:?}");
            Evidence {
                invalid: true,
                ..Evidence::default()
            }
        }
    };
    Ok(evidence.status(status?))
}

#[cfg(test)]
#[path = "tests/task_2838_agy_outcome_tests.rs"]
mod tests;
