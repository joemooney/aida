// trace:TASK-2838 | ai:codex
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
#[cfg(target_os = "linux")]
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

// trace:TASK-2838 | ai:codex
#[test]
#[cfg(target_os = "linux")]
fn task_2838_reviewer_exit_requires_fresh_verdict() {
    use crate::session::HeadlessVendor::{Agy, Claude, Codex};
    for vendor in [Agy, Claude, Codex] {
        for code in [0, INCOMPLETE, 1, 7] {
            let status = ExitStatus::from_raw(code << 8);
            let may_check = code == 0 || (vendor == Agy && code == INCOMPLETE);
            assert_eq!(reviewer_may_check_verdict(vendor, status), may_check);
            for verdict in [false, true] {
                let expected = if code == 0 || (may_check && verdict) {
                    None
                } else {
                    Some(code)
                };
                assert_eq!(
                    crate::queue_cmd::reviewer_vendor_exit_code(vendor, status, verdict),
                    expected,
                    "{vendor:?} code={code} verdict={verdict}"
                );
            }
        }
    }
}

// trace:TASK-2838 | ai:codex
#[test]
#[cfg(target_os = "linux")]
fn task_2838_coordinator_bounds_descendant_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("input");
    let pid_path = dir.path().join("pid");
    for (stream, expected) in [
        (SUCCESS.to_owned(), INCOMPLETE),
        (SUCCESS.replace("SUCCESS", "ERROR"), 1),
    ] {
        std::fs::write(&input, stream).unwrap();
        let mut cmd = std::process::Command::new("/bin/sh");
        cmd.args([
            "-c",
            "sleep 30 & echo $! > \"$2\"; cat -- \"$1\"",
            "stream-json",
        ])
        .arg(&input)
        .arg(&pid_path);
        let started = std::time::Instant::now();
        let status = crate::session::coordinator_status_with_handle_guard(
            crate::session::HeadlessVendor::Agy,
            &mut cmd,
            dir.path(),
            Some(std::fs::File::create(dir.path().join("output")).unwrap()),
            None,
        );
        let elapsed = started.elapsed();
        let pid: i32 = std::fs::read_to_string(&pid_path)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        // SAFETY: this PID belongs to the sleep started by this fixture.
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
        assert_eq!(status.unwrap().code(), Some(expected));
        assert!(elapsed < std::time::Duration::from_secs(3), "{elapsed:?}");
    }
}

// trace:TASK-2838 | ai:codex
#[test]
#[cfg(target_os = "linux")]
fn task_2838_forward_failure_drains_and_preserves_process_status() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("input");
    // Exceeds pipe capacity: dropping the reader early would break cat.
    std::fs::write(&input, SUCCESS.repeat(4096)).unwrap();
    for (exit, expected) in [(0, INCOMPLETE), (7, 7)] {
        let mut cmd = std::process::Command::new("/bin/sh");
        cmd.args(["-c", "cat -- \"$1\" || exit 9; exit \"$2\"", "stream-json"])
            .arg(&input)
            .arg(exit.to_string());
        let status = crate::session::coordinator_status_with_handle_guard(
            crate::session::HeadlessVendor::Agy,
            &mut cmd,
            dir.path(),
            Some(
                std::fs::OpenOptions::new()
                    .write(true)
                    .open("/dev/full")
                    .unwrap(),
            ),
            None,
        )
        .unwrap();
        assert_eq!(status.code(), Some(expected));
    }
    struct FailedWriter;
    impl std::io::Write for FailedWriter {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::from_raw_os_error(libc::EIO))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let evidence = Evidence::read(
        SUCCESS.replace("SUCCESS", "ERROR").as_bytes(),
        Some(&mut FailedWriter),
        false,
    )
    .unwrap();
    assert_eq!(evidence.status(ExitStatus::from_raw(0)).code(), Some(1));
}

// trace:TASK-2838 | ai:codex
#[test]
#[cfg(target_os = "linux")]
fn task_2838_delayed_reader_drains_buffered_evidence_after_stop() {
    use std::sync::{atomic::AtomicBool, Arc};
    for (stream, expected) in [
        (format!("{ERROR}{RETRY}{SUCCESS}"), 0),
        (SUCCESS.replace("SUCCESS", "ERROR"), 1),
    ] {
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "printf '%s' \"$1\"", "buffered-evidence", &stream])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let pipe = CoordinatorPipe {
            output: child.stdout.take().unwrap(),
            stop: Arc::new(AtomicBool::new(false)),
        };
        let process = child.wait().unwrap();
        // No reader runs until after the coordinator's entire post-exit grace.
        // stdout contains the complete stream, even though cancellation is set.
        std::thread::sleep(std::time::Duration::from_millis(300));
        pipe.stop.store(true, std::sync::atomic::Ordering::Release);
        let evidence = Evidence::read(pipe, None, false).unwrap();
        assert!(!evidence.invalid, "{evidence:?}");
        assert_eq!(evidence.status(process).code(), Some(expected));
    }
}

// trace:TASK-2838 | ai:codex
#[test]
fn task_2838_distinct_tool_limit_fails_closed_after_recovery() {
    for count in [MAX_TOOLS, MAX_TOOLS + 1] {
        let mut evidence = Evidence::default();
        for index in 0..count {
            evidence.line(
                ERROR
                    .replace("run_command", &format!("tool_{index}"))
                    .as_bytes(),
                false,
            );
        }
        assert_eq!(evidence.unresolved.len(), MAX_TOOLS);
        assert_eq!(evidence.invalid, count > MAX_TOOLS);
        // A repeated failure at capacity is still the same tool. Retain its
        // latest step so an earlier DONE cannot incorrectly clear the error.
        evidence.line(
            ERROR
                .replace("run_command", "tool_0")
                .replace("step_index\":2", "step_index\":4")
                .as_bytes(),
            false,
        );
        assert_eq!(evidence.invalid, count > MAX_TOOLS);
        for index in 0..count {
            evidence.line(
                RETRY
                    .replace("run_command", &format!("tool_{index}"))
                    .as_bytes(),
                false,
            );
        }
        evidence.line(SUCCESS.as_bytes(), false);
        assert_eq!(evidence.unresolved.len(), 1);
        assert_eq!(evidence.unresolved.get("tool_0"), Some(&Some(4)));
        assert_eq!(
            evidence.status(ExitStatus::from_raw(0)).code(),
            Some(INCOMPLETE)
        );
        evidence.line(
            RETRY
                .replace("run_command", "tool_0")
                .replace("step_index\":3", "step_index\":5")
                .as_bytes(),
            false,
        );
        assert!(evidence.unresolved.is_empty());
        assert_eq!(
            evidence.status(ExitStatus::from_raw(0)).code(),
            Some(if count > MAX_TOOLS { INCOMPLETE } else { 0 })
        );
    }
}

// trace:TASK-2838 | ai:codex
#[test]
#[cfg(target_os = "linux")]
fn task_2838_tee_disabled_diagnostics_reach_stderr() {
    const CHILD_MODE: &str = "AIDA_TASK_2838_DIAGNOSTIC_CHILD";
    if let Ok(mode) = std::env::var(CHILD_MODE) {
        let dir = tempfile::tempdir().unwrap();
        let status = if mode == "direct" {
            let log = dir.path().join("headless.log");
            std::fs::write(&log, DENIED).unwrap();
            log_status(&log, ExitStatus::from_raw(0))
        } else {
            let mut command = std::process::Command::new("/bin/sh");
            command.args(["-c", "printf '%s' \"$1\"", "stream-json", DENIED]);
            crate::session::coordinator_status_with_handle_guard(
                crate::session::HeadlessVendor::Agy,
                &mut command,
                dir.path(),
                None,
                None,
            )
            .unwrap()
        };
        assert_eq!(status.code(), Some(INCOMPLETE));
        return;
    }
    // libtest normally captures eprintln; a child with nocapture proves these
    // diagnostics actually reach stderr with both log tee and forwarding off.
    // Re-exec libtest even when the lane's AIDA_BIN selects the production CLI.
    // trace:TASK-b6b1278890774ef993d2ebc327862470-872 | ai:codex
    for mode in ["direct", "coordinator"] {
        let output = std::process::Command::new(
            crate::aida_bin::current_executable_path().expect("test executable"),
        )
        .args([
            "--exact",
            "agy_outcome::tests::task_2838_tee_disabled_diagnostics_reach_stderr",
            "--nocapture",
        ])
        .env(CHILD_MODE, mode)
        .output()
        .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{mode}: {stderr}");
        assert!(
            stderr.contains("AGY outcome:")
                && stderr.contains("denied_actions=true")
                && stderr.contains("incomplete"),
            "{mode}: {stderr}"
        );
    }
}
