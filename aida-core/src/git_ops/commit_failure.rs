//! Reporting and retrying a failed `git commit`.
//!
//! A concurrent launcher once surfaced `git commit failed: ✓ Commit message
//! format valid` — the commit-msg hook's success note, ANSI codes and all,
//! with git's real refusal and exit code dropped. The error now carries the
//! exit code and both streams with ANSI stripped and hook success notes
//! omitted, and a lock held by a concurrent git process is retried with a
//! bounded backoff instead of failing the launch.
// trace:BUG-034 | ai:claude

use super::GitResult;
use anyhow::Result;
use std::time::Duration;

/// Delays between attempts while another git process holds a lock: five
/// retries, about three seconds in all.
pub(super) const LOCK_BACKOFF_MS: [u64; 5] = [100, 200, 400, 800, 1600];

/// Run `run` (one `git commit`) and rerun it after each [`LOCK_BACKOFF_MS`]
/// delay while it fails on lock contention. A failed commit writes nothing,
/// so rerunning it is safe. Any other failure returns at once.
pub(super) fn with_lock_backoff(
    mut run: impl FnMut() -> Result<GitResult>,
    mut sleep: impl FnMut(Duration),
) -> Result<GitResult> {
    let mut result = run()?;
    for delay_ms in LOCK_BACKOFF_MS {
        if result.success || !is_lock_contention(&result.stderr) {
            break;
        }
        sleep(Duration::from_millis(delay_ms));
        result = run()?;
    }
    Ok(result)
}

/// True when git refused because a concurrent process holds an index, `HEAD`
/// or ref `.lock` file.
fn is_lock_contention(stderr: &str) -> bool {
    // external-prose-classifier: git_ops::commit_failure::is_lock_contention
    let tokens = crate::external_tool_output::GIT_LOCK_CONTENTION;
    crate::external_tool_output::contains_any_case_insensitive(stderr, &tokens[..1])
        && crate::external_tool_output::contains_any_case_insensitive(stderr, &tokens[1..])
}

/// The error for a failed commit: exit code, then git's stderr and stdout.
pub(super) fn error(result: &GitResult) -> anyhow::Error {
    let code = result
        .code
        .map_or_else(|| "killed by a signal".to_string(), |c| format!("exit {c}"));
    let streams: Vec<String> = [("stderr", &result.stderr), ("stdout", &result.stdout)]
        .into_iter()
        .filter_map(|(name, text)| {
            let text = without_hook_notes(text);
            (!text.is_empty()).then(|| format!("{name}: {text}"))
        })
        .collect();
    let detail = if streams.is_empty() {
        "git printed no error".to_string()
    } else {
        streams.join("; ")
    };
    anyhow::anyhow!("git commit failed ({code}): {detail}")
}

/// `text` with ANSI escapes removed and hook success notes (`✓ …` lines)
/// dropped: they say what passed, never why the commit failed.
fn without_hook_notes(text: &str) -> String {
    strip_ansi(text)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('✓'))
        .collect::<Vec<_>>()
        .join(" | ")
}

/// Remove ANSI CSI escape sequences (`ESC [ … final-byte`).
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    /// A fake `git` that prints the commit-msg hook's coloured success line
    /// on stdout and `stderr_line` on stderr, then exits `code` — until
    /// `fail_times` runs have happened, after which it succeeds.
    fn fake_git(dir: &Path, stderr_line: &str, code: i32, fail_times: u32) -> PathBuf {
        let script = dir.join("git");
        let counter = dir.join("runs");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 n=$(cat '{counter}' 2>/dev/null || echo 0); n=$((n+1)); echo $n > '{counter}'\n\
                 printf '\\033[0;32m✓ Commit message format valid\\033[0m\\n'\n\
                 if [ $n -le {fail_times} ]; then printf '%s\\n' \"{stderr_line}\" >&2; exit {code}; fi\n\
                 exit 0\n",
                counter = counter.display(),
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    /// The fake ignores its arguments, so none are passed.
    fn run(script: &Path) -> Result<GitResult> {
        super::super::run_git_result(Command::new(script), &[], &[])
    }

    #[test]
    fn bug034_failure_reports_stderr_and_exit_code_not_the_hook_line() {
        let dir = tempfile::tempdir().unwrap();
        let git = fake_git(
            dir.path(),
            "fatal: could not parse HEAD: bad object",
            128,
            u32::MAX,
        );
        let result = run(&git).unwrap();
        assert!(!result.success);
        let message = format!("{:#}", error(&result));
        assert!(message.contains("exit 128"), "{message}");
        assert!(
            message.contains("fatal: could not parse HEAD: bad object"),
            "{message}"
        );
        assert!(!message.contains('\u{1b}'), "ANSI leaked: {message:?}");
        assert!(
            !message.contains("Commit message format valid"),
            "{message}"
        );
    }

    #[test]
    fn bug034_lock_contention_retries_with_backoff_then_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let git = fake_git(
            dir.path(),
            "fatal: cannot lock ref 'HEAD': Unable to create '/r/.git/HEAD.lock': File exists.",
            128,
            2,
        );
        let mut slept = Vec::new();
        let result = with_lock_backoff(|| run(&git), |d| slept.push(d)).unwrap();
        assert!(result.success);
        assert_eq!(
            slept,
            vec![Duration::from_millis(100), Duration::from_millis(200)]
        );
    }

    #[test]
    fn bug034_lock_contention_retries_are_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let git = fake_git(
            dir.path(),
            "fatal: Unable to create '/r/.git/index.lock': File exists.",
            128,
            u32::MAX,
        );
        let mut slept = Vec::new();
        let result = with_lock_backoff(|| run(&git), |d| slept.push(d)).unwrap();
        assert!(!result.success);
        assert_eq!(slept.len(), LOCK_BACKOFF_MS.len());
        let runs = std::fs::read_to_string(dir.path().join("runs")).unwrap();
        assert_eq!(runs.trim(), (LOCK_BACKOFF_MS.len() + 1).to_string());
    }

    #[test]
    fn bug034_other_failures_are_not_retried() {
        let dir = tempfile::tempdir().unwrap();
        let git = fake_git(dir.path(), "error: hook refused the commit", 1, u32::MAX);
        let mut slept = Vec::new();
        let result = with_lock_backoff(|| run(&git), |d| slept.push(d)).unwrap();
        assert!(!result.success);
        assert!(slept.is_empty());
        assert!(format!("{:#}", error(&result)).contains("exit 1"));
    }

    #[test]
    fn bug034_empty_output_still_names_the_exit_code() {
        let result = GitResult {
            success: false,
            stdout: "\u{1b}[0;32m✓ Commit message format valid\u{1b}[0m".into(),
            stderr: String::new(),
            code: Some(1),
        };
        assert_eq!(
            format!("{:#}", error(&result)),
            "git commit failed (exit 1): git printed no error"
        );
    }
}
