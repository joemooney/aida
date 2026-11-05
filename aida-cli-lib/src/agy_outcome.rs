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
            let bytes = reader.fill_buf()?;
            if bytes.is_empty() {
                if !line.is_empty() || overflow {
                    evidence.invalid = true; // truncated JSONL is not terminal evidence
                }
                break;
            }
            let end = bytes.iter().position(|b| *b == b'\n');
            let count = end.map_or(bytes.len(), |i| i + 1);
            if let Some(writer) = forward.as_deref_mut() {
                writer.write_all(&bytes[..count])?;
                writer.flush()?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    const SUCCESS: &str =
        "{\"event\":\"result\",\"result\":{\"status\":\"SUCCESS\",\"response\":\"\"}}\n";
    const ERROR: &str = "{\"event\":\"step_update\",\"step_update\":{\"step_type\":\"tool\",\"tool_name\":\"run_command\",\"step_index\":2,\"state\":\"ERROR\",\"tool_info\":{\"error\":{\"type\":\"TOOL_ERROR\",\"message\":\"agentapi: read-only file system\"}}}}\n";
    const RETRY: &str = "{\"event\":\"step_update\",\"step_update\":{\"step_type\":\"tool\",\"tool_name\":\"run_command\",\"step_index\":3,\"state\":\"DONE\"}}\n";
    const DENIED: &str = "{\"event\":\"result\",\"result\":{\"status\":\"SUCCESS\",\"response\":\"\",\"denied_actions\":[{\"action\":\"command\",\"display_name\":\"RunCommand\"}]}}\n";

    fn outcome(stream: &str, code: i32) -> i32 {
        Evidence::read(stream.as_bytes(), None, false)
            .unwrap()
            .status(ExitStatus::from_raw(code << 8))
            .code()
            .unwrap()
    }

    #[test]
    fn task_2838_structured_evidence_and_launch_wiring() {
        assert_eq!(outcome(SUCCESS, 0), 0);
        assert_eq!(
            crate::headless_linger::parse_final_result(SUCCESS)
                .unwrap()
                .is_error,
            false
        );
        assert_eq!(outcome(&format!("{DENIED}{SUCCESS}"), 0), INCOMPLETE);
        assert_eq!(
            outcome(
                &format!(
                    "{ERROR}{}{SUCCESS}",
                    RETRY.replace("step_index\":3", "step_index\":2")
                ),
                0
            ),
            INCOMPLETE
        );
        assert_eq!(outcome(DENIED, 0), INCOMPLETE);
        assert_eq!(outcome(&format!("{ERROR}{SUCCESS}"), 0), INCOMPLETE);
        assert_eq!(outcome(&format!("{ERROR}{RETRY}{SUCCESS}"), 0), 0);
        // A different successful tool is not recovery of run_command.
        assert_eq!(
            outcome(
                &format!(
                    "{ERROR}{}{SUCCESS}",
                    RETRY.replace("run_command", "view_file")
                ),
                0
            ),
            INCOMPLETE
        );
        for stream in [
            "",
            "{}\n",
            "not json\n",
            "{\"event\":\"result\"}\n",
            SUCCESS.trim_end(),
        ] {
            assert_eq!(outcome(stream, 0), INCOMPLETE, "{stream}");
        }
        assert_eq!(outcome(&SUCCESS.replace("SUCCESS", "ERROR"), 0), 1);
        assert_eq!(
            outcome(
                &format!("{SUCCESS}{}", SUCCESS.replace("SUCCESS", "FAILED")),
                0
            ),
            1
        );
        assert_eq!(outcome(SUCCESS, 7), 7);
        assert_eq!(outcome(DENIED, 7), 7);
        assert_eq!(
            outcome(&format!("{}\n{SUCCESS}", "x".repeat(MAX_LINE + 1)), 0),
            INCOMPLETE
        );
        // Arbitrary output/prose mentioning tool errors is not a tool event.
        assert_eq!(
            outcome(
                &SUCCESS.replace(
                    "\"response\":\"\"",
                    "\"response\":\"TOOL_ERROR run_command\""
                ),
                0
            ),
            0
        );

        // Historical errors coexist with the existing trusted PR handshake.
        let incomplete = outcome(&format!("{ERROR}{SUCCESS}"), 0);
        let (line, code) = crate::standalone_implementer::report(
            &crate::standalone_implementer::Outcome::PrOpened {
                pr: 42,
                head: Some("abc".into()),
            },
            Some(incomplete),
            None,
        );
        assert_eq!(code, 0);
        assert!(line.contains("pr-opened"));

        // Exercise the real coordinator spawn/status seam, with no live vendor,
        // grants, environment changes, model polling or sandbox expansion.
        let dir = tempfile::tempdir().unwrap();
        for (stream, expected) in [
            (DENIED.to_owned(), INCOMPLETE),
            (format!("{ERROR}{SUCCESS}"), INCOMPLETE),
            (format!("{ERROR}{RETRY}{SUCCESS}"), 0),
        ] {
            let input = dir.path().join("input.jsonl");
            let output = dir.path().join("output.jsonl");
            std::fs::write(&input, &stream).unwrap();
            let mut command = std::process::Command::new("/bin/sh");
            command
                .args(["-c", "cat -- \"$1\"", "stream-json"])
                .arg(&input);
            let status = crate::session::coordinator_status_with_handle_guard(
                crate::session::HeadlessVendor::Agy,
                &mut command,
                dir.path(),
                Some(std::fs::File::create(&output).unwrap()),
                None,
            )
            .unwrap();
            assert_eq!(status.code(), Some(expected));
            assert_eq!(std::fs::read_to_string(output).unwrap(), stream);
            assert_eq!(
                log_status(&input, ExitStatus::from_raw(0)).code(),
                Some(expected)
            );
        }
        let args = crate::session::agy_headless_args_with_effort("prompt", Some("high"));
        assert_eq!(&args[args.len() - 2..], &["-p", "prompt"]);
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--output-format", "stream-json"]));
    }
}
