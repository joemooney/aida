//! A real tool call, through the lane's launch adapter, before implementer work.
// trace:BUG-2652 | ai:codex
use super::*;
use std::io::Read;
use std::process::Stdio;
use std::time::{Duration, Instant};

const COMMAND: &str = "git status --porcelain";
const PROMPT: &str = "Preflight only. Use Bash exactly once to run git status --porcelain. Do not edit files, invoke other tools, or do any other work. Then stop.";
const LIMIT: u64 = 1024 * 1024;

struct ProbeSession(String);
impl Drop for ProbeSession {
    fn drop(&mut self) {
        record_headless_session_end(&headless_worktree_root(), &self.0);
    }
}

pub(super) fn required(vendor: HeadlessVendor, contained: bool, seat: &str) -> bool {
    vendor == HeadlessVendor::Claude
        && contained
        && crate::canonical_role_name(seat) == "implementer"
}

pub(super) fn run(
    host_main: Option<&Path>,
    model: Option<&str>,
    effort: Option<&str>,
    wrapper: bool,
) -> Result<()> {
    eprintln!("  preflight: checking headless Bash permission (45s limit)");
    let _reservations = child_reservation_guard();
    let session = ProbeSession(uuid::Uuid::now_v7().to_string());
    let _composition = wrapper.then(ClaudeWrapperComposition::enter);
    // No work grant is needed to read status. Never loan the implementer's
    // publication authority to the probe or resume the work session.
    let mut cmd = build_exec_vendor_headless_command_for(
        host_main,
        HeadlessVendor::Claude,
        PROMPT,
        &session.0,
        true,
        model,
        effort,
        "implementer",
        "",
        false,
    )?;
    let (status, output) = if wrapper {
        let mut log = tempfile::tempfile()?;
        let mut run = crate::containment::launch::PreparedLaunch::prepare(
            &mut cmd,
            &headless_worktree_root(),
            host_main,
        )?
        .spawn(Stdio::from(log.try_clone()?))?;
        anyhow::ensure!(
            containment_applied(HeadlessVendor::Claude, true, Some(run.receipt())),
            "Bash preflight: missing applied receipt"
        );
        run.release()?;
        let deadline = Instant::now() + Duration::from_secs(45);
        let status = loop {
            if let Some(status) = run.child.try_wait()? {
                break status;
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "Bash preflight timed out; refusing implementer start"
            );
            anyhow::ensure!(
                log.metadata()?.len() <= LIMIT,
                "Bash preflight output exceeded limit; refusing implementer start"
            );
            std::thread::sleep(Duration::from_millis(100));
        };
        let status = run.vendor_exit(status)?;
        use std::io::{Seek, SeekFrom};
        log.seek(SeekFrom::Start(0))?;
        let mut bytes = Vec::new();
        log.take(LIMIT + 1).read_to_end(&mut bytes)?;
        (status, bytes)
    } else {
        match crate::command_output_with_timeout_capped(cmd, Duration::from_secs(45), LIMIT) {
            crate::BoundedCommandOutput::Completed(output) => (output.status, output.stdout),
            crate::BoundedCommandOutput::SpawnFailed => anyhow::bail!(
                "Bash preflight harness could not be spawned; refusing implementer start"
            ),
            crate::BoundedCommandOutput::TimedOut => anyhow::bail!(
                "Bash preflight could not complete within 45s; refusing implementer start"
            ),
        }
    };
    validate(&output).map_err(|cause| {
        anyhow::anyhow!("headless Bash preflight refused implementer start: {cause:#}")
    })?;
    anyhow::ensure!(
        status.success(),
        "Bash preflight child failed ({status}); refusing implementer start"
    );
    eprintln!("  preflight: Bash git status passed");
    Ok(())
}

fn validate(bytes: &[u8]) -> Result<()> {
    anyhow::ensure!(bytes.len() as u64 <= LIMIT, "probe output exceeded limit");
    let mut ids = Vec::new();
    let mut results = Vec::new();
    for line in std::str::from_utf8(bytes)?
        .lines()
        .filter(|line| !line.trim().is_empty())
    {
        let event: serde_json::Value = serde_json::from_str(line)?;
        if let Some(denials) = event.get("permission_denials").and_then(|d| d.as_array()) {
            anyhow::ensure!(
                !denials.iter().any(|d| d["tool_name"] == "Bash"),
                "Bash permission denied by generated or project settings under dontAsk"
            );
        }
        anyhow::ensure!(
            event["api_error_status"] != 401 && event["error_status"] != 401,
            "probe authentication failed (HTTP 401); refresh host credentials before retrying"
        );
        anyhow::ensure!(event["type"] != "result" || event["is_error"] != true,
            "probe harness reported an error; check Bash permissions, authentication and sandbox availability");
        if let Some(content) = event["message"]["content"].as_array() {
            for block in content {
                if block["type"] == "tool_use"
                    && block["name"] == "Bash"
                    && block["input"]["command"] == COMMAND
                {
                    ids.push(
                        block["id"]
                            .as_str()
                            .context("Bash tool call has no id")?
                            .to_string(),
                    );
                }
                if block["type"] == "tool_result" {
                    results.push(block.clone());
                }
            }
        }
    }
    anyhow::ensure!(
        ids.len() == 1,
        "expected one Bash git status call; received {}",
        ids.len()
    );
    let matching: Vec<_> = results
        .iter()
        .filter(|r| r["tool_use_id"] == ids[0])
        .collect();
    anyhow::ensure!(matching.len() == 1 && matching[0]["is_error"] != true,
        "Bash git status failed or was denied; check dontAsk permission rules and sandbox availability");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const USE: &str = r#"{"message":{"content":[{"type":"tool_use","name":"Bash","id":"b1","input":{"command":"git status --porcelain"}}]}}"#;
    #[test]
    fn bash_probe_routes_only_contained_implementers() {
        assert!(required(HeadlessVendor::Claude, true, "implementer"));
        assert!(!required(HeadlessVendor::Claude, false, "implementer"));
        assert!(!required(HeadlessVendor::Claude, true, "reviewer"));
        assert!(!required(HeadlessVendor::Codex, true, "implementer"));
        assert!(!required(HeadlessVendor::Agy, true, "implementer"));
    }
    #[test]
    fn bash_probe_requires_successful_correlated_result() {
        let result = r#"{"message":{"content":[{"type":"tool_result","tool_use_id":"b1","content":"","is_error":false}]}}"#;
        assert!(validate(format!("{USE}\n{result}\n").as_bytes()).is_ok());
        for input in [
            String::new(),
            USE.into(),
            format!("{USE}\n{}", result.replace("b1", "b2")),
            format!("{USE}\n{}", result.replace("false", "true")),
            format!("{USE}\n{USE}\n{result}"),
            format!("{USE}\n{result}\n{result}"),
            format!("{USE}\n{result}\n{{\"type\":\"result\",\"is_error\":true}}"),
            "not json".into(),
            format!("{USE}\n{result}\n{{\"permission_denials\":[{{\"tool_name\":\"Bash\"}}]}}"),
        ] {
            assert!(validate(input.as_bytes()).is_err(), "{input}");
        }
    }

    /// Run from an authenticated Linux host with Claude's sandbox available.
    /// Opt in with libtest's `--ignored` and select this test by name.
    // trace:BUG-2652 | ai:codex
    #[test]
    #[ignore = "host conformance: invokes authenticated Claude and its sandbox"]
    fn headless_bash_git_status_conformance() {
        use std::fs;
        use std::process::Command;

        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().canonicalize().unwrap();
        assert!(Command::new("git")
            .args(["init", "--quiet", "--template="])
            .current_dir(&root)
            .status()
            .unwrap()
            .success());

        // Use the repository's project settings and pre-tool hooks in the
        // disposable checkout so Claude exercises the project/argv merge.
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let project = root.join(".claude");
        fs::create_dir_all(project.join("hooks")).unwrap();
        fs::copy(
            workspace.join(".claude/settings.json"),
            project.join("settings.json"),
        )
        .unwrap();
        for entry in fs::read_dir(workspace.join(".claude/hooks")).unwrap() {
            let entry = entry.unwrap();
            if entry.path().is_file() {
                fs::copy(entry.path(), project.join("hooks").join(entry.file_name())).unwrap();
            }
        }

        let _cwd = contained_cwd_seam::pin(&root);
        let session = uuid::Uuid::now_v7().to_string();
        let args = claude_headless_args_for_roots(
            None,
            PROMPT,
            &session,
            Some(std::slice::from_ref(&root)),
            None,
            None,
        )
        .expect("compose the production contained Claude settings");
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--permission-mode", "dontAsk"]),
            "conformance must exercise dontAsk"
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--setting-sources", "project"]),
            "conformance must load project settings"
        );
        let mut command = Command::new("claude");
        command
            .args(args)
            .current_dir(&root)
            .env(
                "HOME",
                crate::test_home::operator_real_home().expect("resolve Claude's auth home"),
            )
            .env("AIDA_PROJECT_ROOT", &root)
            .env("AIDA_SESSION_PROJECT", &root)
            .env("AIDA_CONTAINED", "1")
            .env(crate::gate::seat_authority::GRANT_ENV, "")
            .env(DISABLE_BACKGROUND_TASKS_ENV, "1");
        let crate::BoundedCommandOutput::Completed(output) =
            crate::command_output_with_timeout_capped(command, Duration::from_secs(45), LIMIT)
        else {
            panic!("Claude conformance could not capture a bounded response");
        };
        validate(&output.stdout)
            .expect("Claude conformance requires a successful Bash tool result");
        assert!(
            output.status.success(),
            "Claude exited with {}",
            output.status
        );
    }
}
