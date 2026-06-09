//! Worker-directive file (TASK-294) — `.aida/worker.cmd`.
//!
//! # The problem
//!
//! `aida queue work --auto-complete` is the autonomous-drain entry point, but
//! it has no way to be steered while it runs. A user who wants to chain a
//! second drain after the current one — or pause the worker mid-night, or stop
//! it at the end of the current spec — has no control channel. The work queue
//! itself is the wrong place: it holds declarative specs, not imperative
//! verbs, and intermixing them muddies `aida queue list`.
//!
//! # The directive file
//!
//! `.aida/worker.cmd` is the control channel for the `aida-worker` shell
//! function (emitted by `aida dev shell-init`). It is a FIFO — one directive
//! per line — that the worker reads at the top of each loop iteration:
//!
//! - `drain` (or an absent / empty file) → pick the queue head and run a full
//!   `aida queue work --auto-complete` lifecycle.
//! - `drain <args>` → run a *scoped* drain (`aida queue work <args>
//!   --auto-complete`); the line is consumed (popped) on completion so a user
//!   can write a whole overnight plan into the file as a heredoc.
//! - `pause` → sleep and re-check (the line persists; the worker stays paused
//!   until the user edits the file).
//! - `exit` → return 0 (the line persists; informational).
//! - anything else → defensively treated as `pause`.
//!
//! Blank lines and `#`-prefixed comment lines are skipped, so a user can
//! annotate their overnight plan.
//!
//! # The `aida worker` subcommand
//!
//! The 2026-05-18 design comment calls visibility "the load-bearing
//! requirement": a control file you cannot inspect is a bad control channel.
//! [`render_human`] / [`render_json`] back the `aida worker directives`
//! subcommand, and the same parsed directives surface in `aida status` and
//! `aida drain status` so a user can see "what will the worker do next?"
//! without `cat`ing a runtime file.
//!
//! Mirrors [`crate::drain_state`] (STORY-301) exactly: a `.aida/` runtime
//! file, a pre-storage subcommand, `parse` / `render_human` / `render_json`.
//! Pure functions here — the command handler in `main.rs` does the I/O.
//!
//! trace:TASK-294 | ai:claude

use std::path::{Path, PathBuf};

use serde::Serialize;

/// File name under `.aida/` holding pending worker directives. Gitignored by
/// the deny-by-default `.aida/*` rule — pure per-clone runtime state.
const WORKER_CMD_FILE: &str = "worker.cmd";

/// Path of the worker-directive file under `project_root`.
pub(crate) fn worker_cmd_path(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join(WORKER_CMD_FILE)
}

/// One parsed directive line.
// trace:TASK-714
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ts_rs_forge::TS)]
pub(crate) struct Directive {
    /// First whitespace-separated word of the line — `drain`, `pause`,
    /// `exit`, or any other word (defensively treated as `pause` by the
    /// worker).
    pub(crate) verb: String,
    /// Remaining words. Forwarded verbatim to `aida queue work` for a
    /// `drain <args>` line.
    pub(crate) args: Vec<String>,
    /// The original line text, for diagnostics.
    pub(crate) raw: String,
}

impl Directive {
    /// Human-friendly one-line summary — `drain batch:x --zen` shows args,
    /// bare verbs show just the verb.
    fn summary(&self) -> String {
        if self.args.is_empty() {
            self.verb.clone()
        } else {
            format!("{} {}", self.verb, self.args.join(" "))
        }
    }
}

/// Parse `.aida/worker.cmd` into a list of directives in file order.
/// Blank lines and `#`-prefixed comments are skipped. An absent file (or any
/// read error) is treated as no directives — fails safe to "queue empty".
pub(crate) fn parse_directives(path: &Path) -> Vec<Directive> {
    let body = match std::fs::read_to_string(path) {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };
    parse_directives_from_str(&body)
}

/// Pure: parse a directive-file body into a list of directives. Split out so
/// the unit tests do not have to round-trip through the filesystem.
pub(crate) fn parse_directives_from_str(body: &str) -> Vec<Directive> {
    let mut out = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut words = trimmed.split_whitespace();
        let verb = match words.next() {
            Some(w) => w.to_string(),
            None => continue,
        };
        let args: Vec<String> = words.map(|w| w.to_string()).collect();
        out.push(Directive {
            verb,
            args,
            raw: trimmed.to_string(),
        });
    }
    out
}

/// Render the human summary for `aida worker directives`. Returns the empty
/// string when there is nothing pending — the caller prints "No pending
/// directives." in that case so the empty-state copy is consistent across the
/// command and the status-line surfacing.
pub(crate) fn render_human(directives: &[Directive]) -> String {
    if directives.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    out.push_str(&format!(
        "{} pending directive{}:\n",
        directives.len(),
        if directives.len() == 1 { "" } else { "s" }
    ));
    for (i, d) in directives.iter().enumerate() {
        out.push_str(&format!("  {}. {}\n", i + 1, d.summary()));
    }
    out
}

/// Render the `--json` payload for `aida worker directives`.
pub(crate) fn render_json(directives: &[Directive]) -> String {
    serde_json::to_string_pretty(directives).unwrap_or_else(|_| "[]".to_string())
}

/// One-line summary suitable for `aida status` / `aida drain status` —
/// `Worker directives: N pending (next: <verb>)` when non-empty, or `None`
/// when the file is empty/absent so the caller can skip the line entirely.
pub(crate) fn status_line(directives: &[Directive]) -> Option<String> {
    let next = directives.first()?;
    Some(format!(
        "Worker directives: {} pending (next: {})",
        directives.len(),
        next.summary()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    // AC: absent file → empty vec, no panic.
    #[test]
    fn parse_directives_absent_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = worker_cmd_path(dir.path());
        assert_eq!(parse_directives(&path), Vec::<Directive>::new());
    }

    // AC: bare `drain` line parses with no args.
    #[test]
    fn parse_directives_bare_drain() {
        let parsed = parse_directives_from_str("drain\n");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].verb, "drain");
        assert!(parsed[0].args.is_empty());
        assert_eq!(parsed[0].raw, "drain");
    }

    // AC: a `drain <args>` line preserves the args verbatim for `aida queue
    // work` forwarding (the directive line *is* the configuration channel).
    #[test]
    fn parse_directives_scoped_drain_keeps_args() {
        let parsed = parse_directives_from_str("drain batch:autonomy-modes --zen\n");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].verb, "drain");
        assert_eq!(parsed[0].args, vec!["batch:autonomy-modes", "--zen"]);
    }

    // AC: FIFO order survives parsing — slot 1 is the next directive.
    #[test]
    fn parse_directives_fifo_order_preserved() {
        let body = "drain batch:b --zen\ndrain batch:c\nexit\n";
        let parsed = parse_directives_from_str(body);
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].args, vec!["batch:b", "--zen"]);
        assert_eq!(parsed[1].args, vec!["batch:c"]);
        assert_eq!(parsed[2].verb, "exit");
    }

    // Blank lines and `#`-prefixed comments are skipped so a user can
    // annotate the overnight plan inline.
    #[test]
    fn parse_directives_skips_blanks_and_comments() {
        let body = "# overnight plan\n\ndrain batch:b\n# pause here\nexit\n";
        let parsed = parse_directives_from_str(body);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].verb, "drain");
        assert_eq!(parsed[1].verb, "exit");
    }

    // Control verbs (pause / exit / unknown) parse identically to `drain` —
    // the worker case-splits on the verb word, not the parser.
    #[test]
    fn parse_directives_pause_and_exit_and_unknown() {
        let parsed = parse_directives_from_str("pause\nexit\nblorf\n");
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].verb, "pause");
        assert_eq!(parsed[1].verb, "exit");
        assert_eq!(parsed[2].verb, "blorf");
    }

    // AC: --json mode is round-trippable for machine consumers.
    #[test]
    fn render_json_round_trips() {
        let parsed = parse_directives_from_str("drain batch:b --zen\nexit\n");
        let json = render_json(&parsed);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let arr = value.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["verb"], "drain");
        assert_eq!(arr[0]["args"][0], "batch:b");
        assert_eq!(arr[1]["verb"], "exit");
    }

    // The human renderer counts pluralization and shows args inline.
    #[test]
    fn render_human_shows_count_and_args() {
        let parsed = parse_directives_from_str("drain batch:b --zen\ndrain batch:c\n");
        let out = render_human(&parsed);
        assert!(out.contains("2 pending directives:"));
        assert!(out.contains("drain batch:b --zen"));
        assert!(out.contains("drain batch:c"));
    }

    // Empty → empty string; the caller prints "No pending directives." once,
    // so the empty-state copy is identical across the CLI and status lines.
    #[test]
    fn render_human_empty_returns_empty_string() {
        assert_eq!(render_human(&[]), "");
    }

    // Singular pluralization at N=1.
    #[test]
    fn render_human_singular_at_one() {
        let parsed = parse_directives_from_str("exit\n");
        let out = render_human(&parsed);
        assert!(out.contains("1 pending directive:"));
        assert!(!out.contains("directives:"));
    }

    // `status_line` is None when the file is empty so the caller can skip
    // the line entirely (quiet projects stay quiet).
    #[test]
    fn status_line_none_when_empty() {
        assert_eq!(status_line(&[]), None);
    }

    // `status_line` names the next directive (the FIFO head) — the user
    // sees what the worker will run next without leaving `aida status`.
    #[test]
    fn status_line_names_next_directive() {
        let parsed = parse_directives_from_str("drain batch:b --zen\ndrain batch:c\n");
        let line = status_line(&parsed).unwrap();
        assert!(line.contains("Worker directives: 2 pending"));
        assert!(line.contains("next: drain batch:b --zen"));
    }
}
