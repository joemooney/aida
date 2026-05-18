//! Workflow hints printed inline at recognized state-transition moments.
//!
//! The skill-side `/aida-pickup`, `/aida-pr`, `/aida-review` surfaces print
//! structured "Next steps" blocks when an agent runs them (TASK-87). This
//! module mirrors the same hints from the CLI itself, so a user running
//! `aida queue done` / `aida edit --status completed` / `aida session end`
//! directly (no agent) still sees the natural next action.
//!
//! Design rules:
//!   1. Always goes to stderr — never stdout. Anything that pipes `aida`
//!      into another tool stays clean.
//!   2. Only fires on a STATE-TRANSITION moment, not on every invocation.
//!      The caller is responsible for verifying the precondition (queue
//!      just emptied, PR just filed) before calling into here.
//!   3. Disable via `AIDA_HINTS=false` env or `[hints] workflow_hints =
//!      false` in `.aida/config.toml`. Env wins when both are set.
//!   4. Hints are concrete: name the command, name the IDs. Generic
//!      "you might want to think about a PR" copy is worse than silence.
//!
//! trace:STORY-106 | ai:claude

use colored::Colorize;
use std::path::Path;

/// Resolve whether workflow hints should print. Order of precedence:
///   1. `AIDA_HINTS=false` (or `0`, `no`, `off`) → disabled
///   2. `AIDA_HINTS=true`  (or `1`, `yes`, `on`) → enabled
///   3. `.aida/config.toml` `[hints] workflow_hints = false` → disabled
///   4. default → enabled
///
/// `project_root` is the directory containing `.aida/`. Pass `None` when
/// the caller doesn't have it resolved — config is then skipped (env-only).
pub fn enabled(project_root: Option<&Path>) -> bool {
    if let Ok(raw) = std::env::var("AIDA_HINTS") {
        match raw.trim().to_ascii_lowercase().as_str() {
            "false" | "0" | "no" | "off" => return false,
            "true" | "1" | "yes" | "on" => return true,
            _ => {}
        }
    }
    if let Some(root) = project_root {
        if let Some(false) = read_config_workflow_hints(root) {
            return false;
        }
    }
    true
}

/// Parse `[hints] workflow_hints = true/false` from `.aida/config.toml`.
/// Returns `None` when the file or key isn't present (caller falls back to
/// the default). Same line-by-line pattern as `read_id_format_settings`.
fn read_config_workflow_hints(project_root: &Path) -> Option<bool> {
    let config_path = project_root.join(".aida").join("config.toml");
    let content = std::fs::read_to_string(&config_path).ok()?;
    let mut in_hints = false;
    for raw in content.lines() {
        let line = raw.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(stripped) = line.strip_prefix('[') {
            in_hints = stripped.trim_end_matches(']').trim() == "hints";
            continue;
        }
        if in_hints {
            if let Some((key, val)) = line.split_once('=') {
                if key.trim() == "workflow_hints" {
                    let v = val.trim().trim_matches('"').trim_matches('\'');
                    return match v.to_ascii_lowercase().as_str() {
                        "true" => Some(true),
                        "false" => Some(false),
                        _ => None,
                    };
                }
            }
        }
    }
    None
}

/// Write `[hints] workflow_hints = <value>` into `.aida/config.toml`,
/// creating the section if it doesn't exist and replacing the line if it
/// does. Idempotent. Returns the prior value (or `None` when no key was
/// previously set).
pub fn persist_setting(project_root: &Path, value: bool) -> anyhow::Result<Option<bool>> {
    let config_path = project_root.join(".aida").join("config.toml");
    let prior = read_config_workflow_hints(project_root);
    let existing = std::fs::read_to_string(&config_path).unwrap_or_default();
    let new_value_line = format!("workflow_hints = {}", value);

    let mut out_lines: Vec<String> = Vec::with_capacity(existing.lines().count() + 4);
    let mut in_hints = false;
    let mut wrote_key = false;
    let mut saw_hints_section = false;
    for raw in existing.lines() {
        let trimmed = raw.trim();
        if let Some(stripped) = trimmed.strip_prefix('[') {
            // Exiting any prior section we were tracking.
            let header = stripped.trim_end_matches(']').trim();
            in_hints = header == "hints";
            if in_hints {
                saw_hints_section = true;
            }
            out_lines.push(raw.to_string());
            continue;
        }
        if in_hints {
            if let Some((key, _)) = trimmed.split_once('=') {
                if key.trim() == "workflow_hints" {
                    out_lines.push(new_value_line.clone());
                    wrote_key = true;
                    continue;
                }
            }
        }
        out_lines.push(raw.to_string());
    }
    if !wrote_key {
        if !saw_hints_section {
            if !out_lines.is_empty() && !out_lines.last().map(|s| s.is_empty()).unwrap_or(true) {
                out_lines.push(String::new());
            }
            out_lines.push("[hints]".to_string());
        }
        out_lines.push(new_value_line);
    }
    let mut serialized = out_lines.join("\n");
    if !serialized.ends_with('\n') {
        serialized.push('\n');
    }
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    // Atomic write — uniform with the concurrent-writer paths. trace:TASK-331 | ai:claude
    aida_core::write_atomic(&config_path, serialized)?;
    Ok(prior)
}

/// Emit a hint block to stderr. No-op when disabled. The first line gets
/// the dimmed `ⓘ` prefix; subsequent lines are continuation indents.
pub fn emit(project_root: Option<&Path>, lines: &[String]) {
    if !enabled(project_root) {
        return;
    }
    if lines.is_empty() {
        return;
    }
    let prefix = "ⓘ".dimmed();
    let label = "Workflow hint:".dimmed();
    eprintln!("\n{} {} {}", prefix, label, lines[0].dimmed());
    for line in &lines[1..] {
        eprintln!("  {}", line.dimmed());
    }
}

/// Hint after `queue done` (or any other op that just emptied the queue
/// for the active role+scope). Caller is responsible for verifying the
/// queue is actually empty before calling — we trust the caller.
pub fn after_queue_drained(
    project_root: Option<&Path>,
    role: Option<&str>,
    scope: Option<&str>,
    branch_commits_ahead: Option<u32>,
) {
    if !enabled(project_root) {
        return;
    }
    let role_phrase = role.map(|r| format!("role:{}", r)).unwrap_or_default();
    let scope_phrase = scope.map(|s| format!(" @{}", s)).unwrap_or_default();
    let commit_phrase = match branch_commits_ahead {
        Some(n) if n > 0 => format!(
            " ({} commit{} on this branch)",
            n,
            if n == 1 { "" } else { "s" }
        ),
        _ => String::new(),
    };
    let header = format!(
        "queue is now empty for {}{}{}.",
        if role_phrase.is_empty() {
            "the active role".to_string()
        } else {
            role_phrase
        },
        scope_phrase,
        commit_phrase
    );
    let body = "Open a PR with `/aida-pr` (or `gh pr create`), or pick a new cluster with `aida queue work <scope>`.".to_string();
    emit(project_root, &[header, body]);
}

/// Build the post-`session end` PR hint as ready-to-print lines.
///
/// The hint is a SEQUENTIAL step chain (review → merge → pull), so it
/// renders as a numbered list — `▶` on step 1 (the primary action),
/// `↓` on the flow-continuation steps — followed by an indented
/// self-merge sidebar for solo developers with no separate reviewer.
/// This is deliberately NOT the Path/Action/Why table format (TASK-260):
/// that shape is for parallel choices (pick one of N); this is a do-all
/// sequence. trace:TASK-267 | ai:claude
///
/// `tty` picks the form — the multi-line numbered block for an
/// interactive terminal, a single-line summary when stderr is piped.
/// `covered_specs` are the delivered `(REQ-ID)` trailers the PR carries;
/// they name the `aida pull` auto-bump targets when known. Pure (no I/O)
/// so it is unit-testable.
fn session_end_pr_hint_lines(pr_number: u64, covered_specs: &[String], tty: bool) -> Vec<String> {
    let merge_cmd = format!("gh pr merge {} --squash --delete-branch", pr_number);
    let bump_phrase = match covered_specs {
        [] => "auto-bumps the merged spec → Completed".to_string(),
        [one] => format!("auto-bumps {} → Completed", one),
        many => format!("auto-bumps {} → Completed", many.join(", ")),
    };
    if !tty {
        // Piped output: collapse the whole chain onto one scannable line.
        return vec![format!(
            "PR #{} next: `aida queue work PR-{}` → `{}` → `aida pull` \
             (or self-merge: `{} && aida pull`).",
            pr_number, pr_number, merge_cmd, merge_cmd
        )];
    }
    vec![
        format!("Next steps for PR #{}:", pr_number),
        format!(
            "  1. ▶ {:<14}   aida queue work PR-{}",
            "Start review", pr_number
        ),
        format!("  2. ↓ {:<14}   {}", "After approval", merge_cmd),
        format!("  3. ↓ {:<14}   aida pull   ({})", "Then pull", bump_phrase),
        String::new(),
        "  Or self-merge (solo dev — skip review):".to_string(),
        format!("     {} && aida pull", merge_cmd),
    ]
}

/// Hint after `aida session end` filed (or found an existing) review
/// story for a PR on the just-ended branch. Shows the FULL remaining
/// path — start review, merge, pull — plus the self-merge alternative,
/// so the user isn't left one step short with the spec stuck at Done.
/// trace:TASK-267 | ai:claude
pub fn after_session_end_with_pr(
    project_root: Option<&Path>,
    pr_number: u64,
    covered_specs: &[String],
) {
    if !enabled(project_root) {
        return;
    }
    let tty = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let lines = session_end_pr_hint_lines(pr_number, covered_specs, tty);
    if !tty {
        // Single-line summary routes through `emit` for the standard
        // `ⓘ Workflow hint:` prefix.
        emit(project_root, &lines);
        return;
    }
    // TTY: numbered block under its own `Next steps for PR #N:` header
    // (no "Workflow hint:" label — the numbered list is self-describing).
    eprintln!();
    eprintln!("{} {}", "ⓘ".dimmed(), lines[0].dimmed());
    for line in &lines[1..] {
        eprintln!("{}", line.dimmed());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    /// Serialize tests that mutate `AIDA_HINTS`. cargo runs tests in
    /// parallel within a single process, so without this they trample
    /// each other's env state intermittently.
    fn with_hints_env<R>(val: Option<&str>, f: impl FnOnce() -> R) -> R {
        static LOCK: Mutex<()> = Mutex::new(());
        let _guard = LOCK.lock().unwrap();
        let prev = std::env::var("AIDA_HINTS").ok();
        match val {
            Some(v) => std::env::set_var("AIDA_HINTS", v),
            None => std::env::remove_var("AIDA_HINTS"),
        }
        let result = f();
        match prev {
            Some(v) => std::env::set_var("AIDA_HINTS", v),
            None => std::env::remove_var("AIDA_HINTS"),
        }
        result
    }

    fn write_config(dir: &Path, body: &str) {
        let aida_dir = dir.join(".aida");
        std::fs::create_dir_all(&aida_dir).unwrap();
        std::fs::write(aida_dir.join("config.toml"), body).unwrap();
    }

    #[test]
    fn enabled_default_when_no_signal() {
        let td = TempDir::new().unwrap();
        with_hints_env(None, || {
            assert!(enabled(Some(td.path())));
        });
    }

    #[test]
    fn env_false_disables() {
        let td = TempDir::new().unwrap();
        with_hints_env(Some("false"), || {
            assert!(!enabled(Some(td.path())));
        });
    }

    #[test]
    fn env_zero_disables() {
        let td = TempDir::new().unwrap();
        with_hints_env(Some("0"), || {
            assert!(!enabled(Some(td.path())));
        });
    }

    #[test]
    fn config_false_disables_when_env_unset() {
        let td = TempDir::new().unwrap();
        write_config(td.path(), "[hints]\nworkflow_hints = false\n");
        with_hints_env(None, || {
            assert!(!enabled(Some(td.path())));
        });
    }

    #[test]
    fn env_true_overrides_config_false() {
        let td = TempDir::new().unwrap();
        write_config(td.path(), "[hints]\nworkflow_hints = false\n");
        with_hints_env(Some("true"), || {
            assert!(enabled(Some(td.path())));
        });
    }

    #[test]
    fn config_true_explicit_enables() {
        let td = TempDir::new().unwrap();
        write_config(td.path(), "[hints]\nworkflow_hints = true\n");
        with_hints_env(None, || {
            assert!(enabled(Some(td.path())));
        });
    }

    #[test]
    fn persist_setting_creates_section_in_existing_config() {
        let td = TempDir::new().unwrap();
        write_config(td.path(), "[deployment]\nmode = \"distributed\"\n");
        let prior = persist_setting(td.path(), false).unwrap();
        assert_eq!(prior, None);
        let body = std::fs::read_to_string(td.path().join(".aida/config.toml")).unwrap();
        assert!(body.contains("[deployment]"));
        assert!(body.contains("[hints]"));
        assert!(body.contains("workflow_hints = false"));
        assert_eq!(read_config_workflow_hints(td.path()), Some(false));
    }

    #[test]
    fn persist_setting_replaces_existing_key() {
        let td = TempDir::new().unwrap();
        write_config(td.path(), "[hints]\nworkflow_hints = false\nother = 1\n");
        let prior = persist_setting(td.path(), true).unwrap();
        assert_eq!(prior, Some(false));
        let body = std::fs::read_to_string(td.path().join(".aida/config.toml")).unwrap();
        // Exactly one workflow_hints line, set to true.
        assert_eq!(body.matches("workflow_hints").count(), 1);
        assert!(body.contains("workflow_hints = true"));
        assert!(body.contains("other = 1"));
    }

    // TASK-267: the session-end PR hint shows the full review → merge →
    // pull path, with a self-merge sidebar, and degrades to one line
    // when stderr is piped.
    #[test]
    fn session_end_hint_tty_is_numbered_sequential_block() {
        let specs = vec!["TASK-259".to_string()];
        let lines = session_end_pr_hint_lines(47, &specs, true);
        // Header + 3 numbered steps + blank + sidebar header + sidebar cmd.
        assert_eq!(lines.len(), 7);
        assert_eq!(lines[0], "Next steps for PR #47:");
        assert!(lines[1].contains("1. ▶"));
        assert!(lines[1].contains("aida queue work PR-47"));
        assert!(lines[2].contains("2. ↓"));
        assert!(lines[2].contains("gh pr merge 47 --squash --delete-branch"));
        assert!(lines[3].contains("3. ↓"));
        assert!(lines[3].contains("aida pull"));
        assert!(lines[3].contains("auto-bumps TASK-259 → Completed"));
        assert!(lines[4].is_empty());
        assert!(lines[5].contains("self-merge"));
        assert!(lines[6].contains("gh pr merge 47 --squash --delete-branch && aida pull"));
    }

    #[test]
    fn session_end_hint_non_tty_is_single_line() {
        let specs = vec!["TASK-259".to_string()];
        let lines = session_end_pr_hint_lines(47, &specs, false);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("aida queue work PR-47"));
        assert!(lines[0].contains("gh pr merge 47 --squash --delete-branch"));
        assert!(lines[0].contains("aida pull"));
        assert!(lines[0].contains("self-merge"));
    }

    #[test]
    fn session_end_hint_names_multiple_covered_specs() {
        let specs = vec!["TASK-259".to_string(), "BUG-113".to_string()];
        let lines = session_end_pr_hint_lines(47, &specs, true);
        assert!(lines[3].contains("auto-bumps TASK-259, BUG-113 → Completed"));
    }

    #[test]
    fn session_end_hint_generic_when_no_covered_specs() {
        let lines = session_end_pr_hint_lines(47, &[], true);
        assert!(lines[3].contains("auto-bumps the merged spec → Completed"));
    }
}
