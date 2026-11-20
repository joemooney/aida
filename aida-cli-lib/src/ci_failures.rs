//! Failing-test digest for a PR's red CI, fetched host-side by the launcher.
//!
//! A contained lane cannot run `gh run view --log` / `--log-failed`: both
//! download the log zip into gh's cache first, the cache path is read-only in
//! the sandbox, and `--log-failed` swallows the error and prints nothing. The
//! launcher builds the lane's prompt before the lane starts, so it fetches the
//! failed jobs here and writes their failing test names and panic lines into
//! the rework and reviewer prompts. Every call goes through `gh api`, which
//! streams the job log as plain text and writes no cache entry, so this also
//! works when the launcher itself runs contained.
// trace:BUG-2380 | ai:claude

use crate::process_retry::RetryEtxtbsy;

/// Matched lines kept per failed job.
const MAX_LINES_PER_JOB: usize = 40;
/// Upper bound on the whole section, so a pathological log cannot flood the
/// prompt.
const MAX_SECTION_BYTES: usize = 12_000;
/// Longest single quoted line.
const MAX_LINE_CHARS: usize = 400;

/// One check run on the PR head commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CheckRun {
    pub id: u64,
    pub name: String,
    /// `queued`, `in_progress`, `completed`, …
    pub status: String,
    /// `success`, `failure`, `timed_out`, … (empty while not completed).
    pub conclusion: String,
    pub html_url: String,
}

impl CheckRun {
    fn failed(&self) -> bool {
        matches!(
            self.conclusion.as_str(),
            "failure" | "timed_out" | "startup_failure"
        )
    }

    fn pending(&self) -> bool {
        self.status != "completed"
    }
}

/// What the log of one failed job yielded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum JobLog {
    /// The log was read; these are its failing-test and panic lines.
    Lines(Vec<String>),
    /// The log could not be read (expired, still uploading, not an Actions
    /// job); the reason is gh's first stderr line.
    Unavailable(String),
}

/// What the launcher learned about the PR head's CI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CiFailures {
    /// Nothing failed and nothing is pending: no section.
    Green,
    /// The PR head or its check runs could not be read.
    Unreadable(String),
    /// The head commit, its failed jobs with their logs, and how many jobs
    /// are still running.
    Report {
        sha: String,
        failed: Vec<(CheckRun, JobLog)>,
        pending: usize,
    },
}

/// Drop GitHub's `2026-10-31T01:02:03.4567890Z ` log-line prefix.
fn strip_timestamp(line: &str) -> &str {
    let Some((head, rest)) = line.split_once(' ') else {
        return line;
    };
    let looks_like_ts = head.len() >= 20
        && head.as_bytes()[0].is_ascii_digit()
        && head.contains('T')
        && head.ends_with('Z');
    if looks_like_ts {
        rest
    } else {
        line
    }
}

fn clip(line: &str) -> String {
    if line.chars().count() <= MAX_LINE_CHARS {
        return line.to_string();
    }
    let mut out: String = line.chars().take(MAX_LINE_CHARS).collect();
    out.push('…');
    out
}

/// A test-runner verdict line naming one failing test: libtest's
/// `test a::b ... FAILED` and nextest's `FAIL [ 0.5s] crate a::b` (also its
/// `TIMEOUT` / signal variants).
fn is_failed_test_line(line: &str) -> bool {
    if line.starts_with("test ") && line.ends_with("... FAILED") {
        return true;
    }
    ["FAIL [", "TIMEOUT [", "SIGSEGV [", "SIGABRT [", "SIGKILL ["]
        .iter()
        .any(|p| line.starts_with(p))
}

/// The failing test names, panic lines (each with the message line that
/// follows it), `error: test failed` lines and the first `##[error]` from a
/// job log, de-duplicated and capped. Gate self-test output (`stray/`, `leak`
/// lines) never matches these patterns, so it is not reported as a failure.
pub(crate) fn extract_failure_lines(log: &str) -> Vec<String> {
    let lines: Vec<&str> = log.lines().map(|l| strip_timestamp(l).trim()).collect();
    let mut out: Vec<String> = Vec::new();
    let mut seen_error_annotation = false;
    let push = |out: &mut Vec<String>, line: &str| {
        let clipped = clip(line);
        if !out.contains(&clipped) {
            out.push(clipped);
        }
    };
    for (i, line) in lines.iter().enumerate() {
        if out.len() >= MAX_LINES_PER_JOB {
            break;
        }
        if is_failed_test_line(line) || line.starts_with("error: test failed") {
            push(&mut out, line);
        } else if line.contains("panicked at") {
            push(&mut out, line);
            // Since Rust 1.73 the panic message is on the next line.
            if line.ends_with(':') {
                if let Some(msg) = lines.get(i + 1).filter(|m| !m.is_empty()) {
                    push(&mut out, msg);
                }
            }
        } else if line.starts_with("##[error]") && !seen_error_annotation {
            seen_error_annotation = true;
            push(&mut out, line);
        }
    }
    out.truncate(MAX_LINES_PER_JOB);
    out
}

/// Parse `gh api repos/{owner}/{repo}/commits/<sha>/check-runs` output.
pub(crate) fn parse_check_runs(json: &str) -> Result<Vec<CheckRun>, String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("unparseable check-runs JSON: {e}"))?;
    let runs = value
        .get("check_runs")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "check-runs JSON has no `check_runs` array".to_string())?;
    let text = |run: &serde_json::Value, key: &str| {
        run.get(key)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    Ok(runs
        .iter()
        .filter_map(|run| {
            Some(CheckRun {
                id: run.get("id")?.as_u64()?,
                name: text(run, "name"),
                status: text(run, "status"),
                conclusion: text(run, "conclusion"),
                html_url: text(run, "html_url"),
            })
        })
        .collect())
}

/// The prompt section, or `None` when CI is green.
pub(crate) fn render_section(pr: u64, report: &CiFailures) -> Option<String> {
    let body = match report {
        CiFailures::Green => return None,
        CiFailures::Unreadable(why) => format!(
            "## CI failures for PR #{pr}\n\
             CI status could not be read ({why}). No failure logs are available here; \
             do not guess which test failed."
        ),
        CiFailures::Report {
            sha,
            failed,
            pending,
        } => {
            let short = &sha[..sha.len().min(12)];
            let mut s = format!("## CI failures at {short}\n");
            if failed.is_empty() {
                s.push_str(&format!(
                    "No CI job has failed yet: {pending} job(s) still running at {short}, \
                     so no failure logs are available. Check again once CI finishes."
                ));
                return Some(s);
            }
            s.push_str(&format!(
                "{} CI job(s) failed on PR #{pr} at {short}. Failing tests and panic lines, \
                 fetched from the job logs by the launcher:\n",
                failed.len()
            ));
            if *pending > 0 {
                s.push_str(&format!(
                    "({pending} other job(s) were still running; their results are not \
                     included.)\n"
                ));
            }
            for (run, log) in failed {
                s.push_str(&format!(
                    "\n### {} ({}, job {})\n",
                    run.name, run.conclusion, run.id
                ));
                match log {
                    JobLog::Lines(lines) if lines.is_empty() => s.push_str(
                        "The log has no failing-test, panic or error lines. Read the full \
                         log: ",
                    ),
                    JobLog::Lines(lines) => {
                        for line in lines {
                            s.push_str(&format!("- `{}`\n", line.replace('`', "'")));
                        }
                        continue;
                    }
                    JobLog::Unavailable(why) => s.push_str(&format!(
                        "Log unavailable ({why}); it may have expired or still be \
                         uploading. Job page: "
                    )),
                }
                s.push_str(&run.html_url);
                s.push('\n');
            }
            s
        }
    };
    Some(tail_cap(body))
}

fn tail_cap(mut s: String) -> String {
    if s.len() <= MAX_SECTION_BYTES {
        return s;
    }
    let mut end = MAX_SECTION_BYTES;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
    s.push_str("\n…(CI failure section truncated)\n");
    s
}

fn first_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("no output")
        .to_string()
}

fn gh_output(
    gh: &std::path::Path,
    root: &std::path::Path,
    args: &[&str],
) -> Result<String, String> {
    let out = std::process::Command::new(gh)
        .current_dir(root)
        .args(args)
        .output_retrying_etxtbsy()
        .map_err(|e| format!("cannot run gh: {e}"))?;
    if !out.status.success() {
        return Err(first_line(&out.stderr));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Read the PR head's check runs and the logs of its failed jobs.
pub(crate) fn collect(root: &std::path::Path, pr: u64) -> CiFailures {
    // Unit tests never reach a real forge: only a stubbed gh is used.
    if cfg!(test) && std::env::var_os("AIDA_TEST_GH_BINARY").is_none() {
        return CiFailures::Unreadable("gh is not stubbed in this test".into());
    }
    let Some(gh) = crate::resolve_gh_binary() else {
        return CiFailures::Unreadable("gh is not installed".into());
    };
    let pr_arg = pr.to_string();
    let sha = match gh_output(
        &gh,
        root,
        &[
            "pr",
            "view",
            &pr_arg,
            "--json",
            "headRefOid",
            "-q",
            ".headRefOid",
        ],
    ) {
        Ok(sha) if !sha.trim().is_empty() => sha.trim().to_string(),
        Ok(_) => return CiFailures::Unreadable(format!("PR #{pr} has no head commit")),
        Err(why) => return CiFailures::Unreadable(why),
    };
    let runs_path = format!("repos/{{owner}}/{{repo}}/commits/{sha}/check-runs?per_page=100");
    let runs =
        match gh_output(&gh, root, &["api", &runs_path]).and_then(|json| parse_check_runs(&json)) {
            Ok(runs) => runs,
            Err(why) => return CiFailures::Unreadable(why),
        };
    let pending = runs.iter().filter(|r| r.pending()).count();
    let failed: Vec<(CheckRun, JobLog)> = runs
        .into_iter()
        .filter(CheckRun::failed)
        .map(|run| {
            let logs_path = format!("repos/{{owner}}/{{repo}}/actions/jobs/{}/logs", run.id);
            let log = match gh_output(&gh, root, &["api", &logs_path]) {
                Ok(text) => JobLog::Lines(extract_failure_lines(&text)),
                Err(why) => JobLog::Unavailable(why),
            };
            (run, log)
        })
        .collect();
    if failed.is_empty() && pending == 0 {
        return CiFailures::Green;
    }
    CiFailures::Report {
        sha,
        failed,
        pending,
    }
}

/// The section for PR `pr`, ready to place in a prompt, or `None` when CI is
/// green.
pub(crate) fn section_for_pr(root: &std::path::Path, pr: u64) -> Option<String> {
    render_section(pr, &collect(root, pr))
}

/// The section for the open PR that implements `spec`, for a rework pickup.
/// `None` off GitHub, when no open PR is found, or when its CI is green.
pub(crate) fn section_for_spec(root: &std::path::Path, spec: &str) -> Option<String> {
    if cfg!(test) && std::env::var_os("AIDA_TEST_GH_BINARY").is_none() {
        return None;
    }
    if crate::forge::resolve_forge_kind(root) != crate::forge::ForgeKind::GitHub {
        return None;
    }
    match crate::pr_lookup::detect_open_pr_for_spec(root, spec) {
        crate::pr_lookup::PrLookup::Found(pr) => section_for_pr(root, pr.number),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "\
2026-10-31T01:02:03.4567890Z ##[group]Run tests
2026-10-31T01:02:04.0000000Z stray/ file left by the gate self-test
2026-10-31T01:02:04.1000000Z leak check: 0 leaked
2026-10-31T01:02:05.0000000Z test queue_cmd::tests::ok ... ok
2026-10-31T01:02:05.1000000Z test queue_cmd::tests::rework_round ... FAILED
2026-10-31T01:02:05.2000000Z         FAIL [   0.511s] aida-cli-lib queue_cmd::tests::rework_round
2026-10-31T01:02:05.3000000Z thread 'queue_cmd::tests::rework_round' panicked at aida-cli-lib/src/queue_cmd.rs:10:5:
2026-10-31T01:02:05.4000000Z assertion `left == right` failed
2026-10-31T01:02:05.5000000Z         FAIL [   0.511s] aida-cli-lib queue_cmd::tests::rework_round
2026-10-31T01:02:05.6000000Z error: test failed, to rerun pass `-p aida-cli-lib --lib`
2026-10-31T01:02:05.7000000Z ##[error]Process completed with exit code 100.
2026-10-31T01:02:05.8000000Z ##[error]second annotation
";

    #[test]
    fn extracts_failing_tests_and_panic_lines_without_self_test_noise() {
        let lines = extract_failure_lines(LOG);
        assert_eq!(
            lines,
            vec![
                "test queue_cmd::tests::rework_round ... FAILED",
                "FAIL [   0.511s] aida-cli-lib queue_cmd::tests::rework_round",
                "thread 'queue_cmd::tests::rework_round' panicked at aida-cli-lib/src/queue_cmd.rs:10:5:",
                "assertion `left == right` failed",
                "error: test failed, to rerun pass `-p aida-cli-lib --lib`",
                "##[error]Process completed with exit code 100.",
            ]
        );
        assert!(!lines
            .iter()
            .any(|l| l.contains("stray/") || l.contains("leak")));
    }

    #[test]
    fn green_ci_renders_no_section() {
        assert_eq!(render_section(7, &CiFailures::Green), None);
    }

    #[test]
    fn in_progress_and_unreadable_ci_say_logs_are_unavailable() {
        let running = render_section(
            7,
            &CiFailures::Report {
                sha: "abcdef1234567890".into(),
                failed: vec![],
                pending: 3,
            },
        )
        .unwrap();
        assert!(
            running.starts_with("## CI failures at abcdef123456\n"),
            "{running}"
        );
        assert!(running.contains("3 job(s) still running"), "{running}");
        assert!(
            running.contains("no failure logs are available"),
            "{running}"
        );

        let unreadable = render_section(7, &CiFailures::Unreadable("HTTP 404".into())).unwrap();
        assert!(
            unreadable.contains("could not be read (HTTP 404)"),
            "{unreadable}"
        );
    }

    #[test]
    fn an_expired_job_log_is_named_as_unavailable() {
        let run = CheckRun {
            id: 42,
            name: "lib-0".into(),
            status: "completed".into(),
            conclusion: "failure".into(),
            html_url: "https://example.test/job/42".into(),
        };
        let text = render_section(
            7,
            &CiFailures::Report {
                sha: "abc".into(),
                failed: vec![(run, JobLog::Unavailable("HTTP 410: Gone".into()))],
                pending: 0,
            },
        )
        .unwrap();
        assert!(text.contains("### lib-0 (failure, job 42)"), "{text}");
        assert!(
            text.contains("Log unavailable (HTTP 410: Gone); it may have expired"),
            "{text}"
        );
        assert!(text.contains("https://example.test/job/42"), "{text}");
    }

    /// A stubbed gh: the PR head, two check runs (one failed, one green), and
    /// the failed job's log — read only through `gh api`, never `gh run view`.
    #[cfg_attr(windows, ignore = "fake-gh harness is a bash script")]
    #[test]
    fn stubbed_gh_failed_job_log_becomes_the_ci_failures_section() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("job.log");
        std::fs::write(&log, LOG).unwrap();
        let fake = tmp.path().join("fake-gh");
        crate::test_exec::write_executable(
            &fake,
            &format!(
                r#"#!/usr/bin/env bash
case "$*" in
  *"run view"*) echo "run view must not be used" >&2; exit 9 ;;
  "pr view 31 --json headRefOid -q .headRefOid") echo deadbeefcafe0001 ;;
  "api repos/{{owner}}/{{repo}}/commits/deadbeefcafe0001/check-runs?per_page=100")
    printf '%s' '{{"check_runs":[{{"id":901,"name":"lib-0","status":"completed","conclusion":"failure","html_url":"u901"}},{{"id":902,"name":"fmt","status":"completed","conclusion":"success","html_url":"u902"}}]}}' ;;
  "api repos/{{owner}}/{{repo}}/actions/jobs/901/logs") cat '{}' ;;
  *) echo "unexpected: $*" >&2; exit 1 ;;
esac
"#,
                log.display()
            ),
        );
        let _env =
            crate::test_env::EnvVarsGuard::set(&[("AIDA_TEST_GH_BINARY", fake.to_str().unwrap())]);
        let section = section_for_pr(tmp.path(), 31).expect("red CI renders a section");
        assert!(
            section.starts_with("## CI failures at deadbeefcafe\n"),
            "{section}"
        );
        assert!(
            section.contains("1 CI job(s) failed on PR #31"),
            "{section}"
        );
        assert!(
            section.contains("### lib-0 (failure, job 901)"),
            "{section}"
        );
        assert!(
            section.contains("- `test queue_cmd::tests::rework_round ... FAILED`"),
            "{section}"
        );
        assert!(
            section.contains("panicked at aida-cli-lib/src/queue_cmd.rs:10:5:"),
            "{section}"
        );
        assert!(
            section.contains("assertion 'left == right' failed"),
            "{section}"
        );
        assert!(!section.contains("fmt"), "{section}");
    }
}
