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
use std::path::{Path, PathBuf};

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
                    // BUG-92: strip a TOML inline `# comment` before parsing so
                    // `workflow_hints = true # note` matches `true` instead of
                    // silently falling through to the default.
                    let v = val
                        .split('#')
                        .next()
                        .unwrap_or("")
                        .trim()
                        .trim_matches('"')
                        .trim_matches('\'');
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
/// the dimmed info-glyph prefix; subsequent lines are continuation indents.
pub fn emit(project_root: Option<&Path>, lines: &[String]) {
    if !enabled(project_root) {
        return;
    }
    if lines.is_empty() {
        return;
    }
    let prefix = crate::glyph(crate::glyphs::Glyph::Info).dimmed();
    let label = "Workflow hint:".dimmed();
    eprintln!("\n{} {} {}", prefix, label, lines[0].dimmed());
    for line in &lines[1..] {
        eprintln!("  {}", line.dimmed());
    }
}

/// BUG-232: what `aida queue done` could determine about the current
/// branch's pull-request state. Lets the drained-queue hint sharpen the
/// generic "open a PR" nudge into a pointed "no PR open — run `/aida-pr`"
/// warning when the branch carries committed-but-unshipped work — the
/// failure mode where a `--zen` session left a spec Done but unmergeable.
/// trace:BUG-232 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrState {
    /// `gh` confirmed an open PR (carrying its number) for the branch.
    Open(u64),
    /// `gh` ran cleanly and reported no open PR for the branch.
    Absent,
    /// PR state is unknown — `gh` is missing / unauthenticated / errored,
    /// or the branch has no commits ahead of main worth probing for.
    Unknown,
}

/// Pure builder for the drained-queue hint lines — split out so the
/// message selection is unit-testable without touching config or env.
/// `branch` + `pr` drive the BUG-232 PR-aware sharpening. trace:BUG-232
/// | ai:claude
fn queue_drained_hint_lines(
    kind: crate::forge::ForgeKind,
    role: Option<&str>,
    scope: Option<&str>,
    branch_commits_ahead: Option<u32>,
    branch: Option<&str>,
    pr: PrState,
) -> Vec<String> {
    let role_phrase = role.map(|r| format!("role:{}", r)).unwrap_or_default();
    let scope_phrase = scope.map(|s| format!(" @{}", s)).unwrap_or_default();
    let commits = branch_commits_ahead.unwrap_or(0);
    let commit_phrase = if commits > 0 {
        format!(
            " ({} commit{} on this branch)",
            commits,
            if commits == 1 { "" } else { "s" }
        )
    } else {
        String::new()
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
    let branch_phrase = branch
        .map(|b| format!("`{}`", b))
        .unwrap_or_else(|| "this branch".to_string());
    // STORY-508: forge-aware change-request noun ("PR"/"MR"/"change") and CLI
    // hints, so a GitLab/pure-git user isn't told to run `gh`.
    let noun = kind.change_noun();
    // trace:TASK-840 | ai:claude — route the warning marker through the registry.
    let warn = crate::glyphs::Glyph::Warning.render(crate::glyphs::active_profile(None));
    let body = match pr {
        // BUG-232: the branch has unshipped commits and the forge confirmed no
        // change exists — the spec is Done-but-unmergeable. Say so pointedly.
        PrState::Absent if commits > 0 => format!(
            "{warn} No {} is open for {} — the work is committed but unshipped. \
             Run `/aida-pr` to ship it, or it sits Done and unmergeable.",
            noun, branch_phrase
        ),
        // A change already exists — the next move is merge, not open.
        PrState::Open(n) => {
            let merge_hint = kind
                .change_cmd_hint("merge", &format!("{n} --squash"))
                .map(|c| format!(" (`{}`)", c))
                .unwrap_or_default();
            format!(
                "{} #{} is already open for this work — merge it once review is green{}, \
                 or pick a new cluster with `aida queue work <scope>`.",
                noun, n, merge_hint
            )
        }
        // Unknown PR state, or nothing committed to ship — the original
        // generic nudge.
        _ => {
            let create_hint = kind
                .change_cmd_hint("create", "")
                .map(|c| format!(" (or `{}`)", c))
                .unwrap_or_default();
            format!(
                "Open a {} with `/aida-pr`{}, or pick a new cluster \
                 with `aida queue work <scope>`.",
                noun, create_hint
            )
        }
    };
    vec![header, body]
}

/// BUG-285: pure decision for whether `aida queue done`'s PR check should
/// be bypassed. Takes ALL three flags `aida queue done` accepts —
/// `--yes`, `--force`, `--skip-pr-check` — so the invariant *"--yes does
/// NOT bypass the gate"* is encoded in the test matrix below. A regression
/// that wired `--yes` into the bypass path would flip
/// `bypass_with_yes_only` red.
///
/// `--yes` is the confirmation-skip for the interactive prompt that
/// follows the gate, not a gate override. The two flags that genuinely
/// bypass are `--force` (legacy, general escape hatch) and
/// `--skip-pr-check` (BUG-285, intent-named). Both bypass identically;
/// `--skip-pr-check` is the recommended name when the bypass is
/// specifically about the PR-check gate.
/// trace:BUG-285 | ai:claude
pub fn queue_done_should_bypass_pr_check(yes: bool, force: bool, skip_pr_check: bool) -> bool {
    let _ = yes;
    force || skip_pr_check
}

/// BUG-269 / BUG-285: pure decision for the `aida queue done` pre-check.
/// Returns `Some(error_lines)` when the call must be refused (committed-
/// but-unshipped work with no open PR), `None` to proceed.
///
/// The check fires only when **both** conditions hold: the branch has
/// commits ahead of `origin/main` AND `gh` has *confirmed* no open PR
/// (`PrState::Absent`). A `PrState::Unknown` (gh missing / unauthenticated
/// / network failure) proceeds — we never assert "no PR" on guesswork, so
/// the precheck cannot block legitimate `queue done` calls when the
/// detector itself is broken.
///
/// BUG-285: the refusal language is deliberately load-bearing for an LLM
/// implementer reading the message through `2>&1 | tail -N` (which masks
/// the non-zero exit). The first line carries the `error:` prefix and an
/// explicit `(exit 1)` so the tool result parses as a hard failure even
/// without the exit code; the second line carries an explicit "DO NOT
/// exit this session" instruction; the third line names the bypass flag.
/// Tonight's TASK-413 / TASK-416 drains showed an implementer reading a
/// softer refusal and composing a finish-state summary anyway — this
/// language is the substrate-side guardrail against that.
///
/// `display_id` is the short id (e.g. `BUG-249`) used verbatim in the
/// suggested follow-up commands; that's what the user typed and what they
/// want to re-type.
/// trace:BUG-269 BUG-285 | ai:claude
pub fn queue_done_precheck_error(
    display_id: &str,
    branch_commits_ahead: Option<u32>,
    pr: PrState,
) -> Option<Vec<String>> {
    let commits = branch_commits_ahead.unwrap_or(0);
    if commits == 0 {
        return None;
    }
    if !matches!(pr, PrState::Absent) {
        return None;
    }
    let summary = format!(
        "error: aida queue done refused (exit 1) — {} has {} local commit{} but no open PR.",
        display_id,
        commits,
        if commits == 1 { "" } else { "s" }
    );
    let action = format!(
        "Open the PR first: run `/aida-pr`, then re-run `aida queue done {}`. \
         DO NOT exit this session without opening the PR — the orchestrator will fail phase 1 otherwise.",
        display_id
    );
    let bypass = format!(
        "(Rare bypass when the spec was implemented on a different branch already merged: \
         `aida queue done {} --skip-pr-check`.)",
        display_id
    );
    Some(vec![summary, action, bypass])
}

/// TASK-500: the resolved outcome of the `aida queue done` PR-check gate.
/// Extracted from the inline match-tree in `QueueCommand::Done`'s handler
/// so the whole decision can be exercised in isolation — every skip path,
/// the refusal, and the proceed — without a real git/gh environment.
///
/// The caller maps each variant to its I/O: `Refuse` → print lines + exit 1,
/// `SilentSkip` → print the warning line, `Proceed` → do nothing.
/// trace:TASK-500 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueDoneGateDiagnose {
    /// All conditions resolved; the gate decided to refuse with these error lines.
    Refuse(Vec<String>),
    /// All conditions resolved; the gate decided to proceed (no commits ahead,
    /// PR already open, or PR state unknown so we never assert "no PR").
    Proceed,
    /// A condition could not be resolved; the gate skipped with a warning reason.
    /// The caller prints `warning_line` to stderr and proceeds.
    SilentSkip {
        reason: SkipReason,
        warning_line: String,
    },
}

/// TASK-500: which precondition could not be resolved, so the gate skipped.
/// One variant per `if let`/`match` arm in the original inline tree, so a
/// test can assert the right warning fires for each failure mode.
/// trace:TASK-500 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// `find_project_root` failed — no `.aida/` anchor to resolve git from.
    ProjectRootNotFound,
    /// The current branch couldn't be read at the project root.
    BranchUndetectable,
    /// `rev-list` against `origin/main` failed (and the branch wasn't the
    /// intentional `main`/`HEAD` no-op skip), so commits-ahead is unknown.
    CommitsAheadFailed,
    /// The forge change-lookup couldn't confirm PR state (`gh` missing /
    /// unauthenticated / unreachable). The gate proceeds, but warns first.
    GhUnknown,
}

/// TASK-500: pure decision for the `aida queue done` PR-check gate. Mirrors
/// the inline tree that used to live in `QueueCommand::Done`'s handler but
/// with the I/O dependencies injected as closures, so every branch is unit-
/// testable without a real git/gh environment.
///
/// Resolution order (each step can short-circuit to a `SilentSkip`):
///   1. `project_root` — `Err` → `SilentSkip { ProjectRootNotFound }`.
///   2. `branch_at_root` — `None` → `SilentSkip { BranchUndetectable }`.
///   3. `commits_ahead_of_main` — `None` for a non-`main`/`HEAD` branch →
///      `SilentSkip { CommitsAheadFailed }`. `None` for `main`/`HEAD` is the
///      intentional no-op skip and yields `Proceed` silently.
///   4. commits-ahead `== 0` → `Proceed` (nothing to ship).
///   5. otherwise look up the PR via `pr_lookup_for_branch`:
///      - `PrState::Unknown` → `SilentSkip { GhUnknown }` (warn, then the
///        caller proceeds — we never assert "no PR" on guesswork);
///      - else delegate to [`queue_done_precheck_error`]: `Some(lines)` →
///        `Refuse(lines)`, `None` → `Proceed`.
///
/// No behavior change vs the inline tree (PR-242): same warnings, same
/// refusals. The closures let the caller keep using the real
/// `find_project_root` / `current_branch_at` / `branch_commits_ahead_main` /
/// `change_lookup_for_branch` helpers while tests inject fakes.
/// trace:TASK-500 | ai:claude
pub fn queue_done_precheck_diagnose(
    display_id: &str,
    project_root: anyhow::Result<PathBuf>,
    branch_at_root: impl FnOnce(&Path) -> Option<String>,
    commits_ahead_of_main: impl FnOnce(&Path, &str) -> Option<u32>,
    pr_lookup_for_branch: impl FnOnce(&Path, &str) -> PrState,
) -> QueueDoneGateDiagnose {
    let project_root = match project_root {
        Err(e) => {
            return QueueDoneGateDiagnose::SilentSkip {
                reason: SkipReason::ProjectRootNotFound,
                warning_line: format!(
                    "{} queue-done PR-check skipped: find_project_root failed ({}). \
                     Gate did not fire; recovery responsibility is on the operator.",
                    "warning:".yellow().bold(),
                    e
                ),
            };
        }
        Ok(root) => root,
    };

    let branch = match branch_at_root(&project_root) {
        None => {
            return QueueDoneGateDiagnose::SilentSkip {
                reason: SkipReason::BranchUndetectable,
                warning_line: format!(
                    "{} queue-done PR-check skipped: current_branch_at returned None \
                     in {}. Gate did not fire.",
                    "warning:".yellow().bold(),
                    project_root.display()
                ),
            };
        }
        Some(branch) => branch,
    };

    let commits_ahead = commits_ahead_of_main(&project_root, &branch);
    // `commits_ahead` is None when the branch is `main`/`HEAD` (intentional
    // skip — proceed silently) OR when rev-list itself failed (warn).
    if commits_ahead.is_none() {
        if branch != "main" && branch != "HEAD" {
            return QueueDoneGateDiagnose::SilentSkip {
                reason: SkipReason::CommitsAheadFailed,
                warning_line: format!(
                    "{} queue-done PR-check skipped: branch_commits_ahead_main \
                     returned None for branch `{}` (rev-list failed or origin/main \
                     unresolved). Gate did not fire.",
                    "warning:".yellow().bold(),
                    branch
                ),
            };
        }
        return QueueDoneGateDiagnose::Proceed;
    }

    if commits_ahead.unwrap_or(0) == 0 {
        return QueueDoneGateDiagnose::Proceed;
    }

    let pr_state = pr_lookup_for_branch(&project_root, &branch);
    if matches!(pr_state, PrState::Unknown) {
        return QueueDoneGateDiagnose::SilentSkip {
            reason: SkipReason::GhUnknown,
            warning_line: format!(
                "{} queue-done PR-check proceeding without `gh` confirmation \
                 (lookup unknown). Gate may not fire if PR actually missing.",
                "warning:".yellow().bold()
            ),
        };
    }

    match queue_done_precheck_error(display_id, commits_ahead, pr_state) {
        Some(lines) => QueueDoneGateDiagnose::Refuse(lines),
        None => QueueDoneGateDiagnose::Proceed,
    }
}

/// Hint after `queue done` (or any other op that just emptied the queue
/// for the active role+scope). Caller is responsible for verifying the
/// queue is actually empty before calling — we trust the caller.
///
/// BUG-232: `branch` + `pr` let the hint distinguish "committed but no PR
/// open" (a pointed warning — the spec would otherwise sit Done-but-
/// unshipped) from "PR already open" (merge it) and the generic case.
/// trace:BUG-232 | ai:claude
pub fn after_queue_drained(
    project_root: Option<&Path>,
    role: Option<&str>,
    scope: Option<&str>,
    branch_commits_ahead: Option<u32>,
    branch: Option<&str>,
    pr: PrState,
) {
    if !enabled(project_root) {
        return;
    }
    // STORY-508: resolve the active forge so the hint names the right CLI/noun.
    let kind = project_root
        .map(crate::forge::resolve_forge_kind)
        .unwrap_or(crate::forge::ForgeKind::None);
    let lines = queue_drained_hint_lines(kind, role, scope, branch_commits_ahead, branch, pr);
    emit(project_root, &lines);
}

/// Build the post-`session end` PR hint as ready-to-print lines.
///
/// The hint is a SEQUENTIAL step chain (review → merge → pull), so it
/// renders as a numbered list — a primary-action marker on step 1,
/// a flow-continuation marker on the rest — followed by an indented
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
fn session_end_pr_hint_lines(
    kind: crate::forge::ForgeKind,
    pr_number: u64,
    covered_specs: &[String],
    tty: bool,
) -> Vec<String> {
    // STORY-508: forge-aware merge command. pure-git has no forge CLI, so fall
    // back to a forge-neutral phrasing that names no wrong binary.
    let merge_cmd = kind
        .change_cmd_hint("merge", &format!("{} --squash --delete-branch", pr_number))
        .unwrap_or_else(|| "merge it to your default branch".to_string());
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
    // trace:TASK-840 | ai:claude — route the primary-action marker through the registry.
    let active = crate::glyphs::Glyph::FlowActive.render(crate::glyphs::active_profile(None));
    vec![
        format!("Next steps for PR #{}:", pr_number),
        format!(
            "  1. {active} {:<14}   aida queue work PR-{}",
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
    // STORY-508: resolve the active forge for forge-aware merge hint text.
    let kind = project_root
        .map(crate::forge::resolve_forge_kind)
        .unwrap_or(crate::forge::ForgeKind::None);
    let lines = session_end_pr_hint_lines(kind, pr_number, covered_specs, tty);
    if !tty {
        // Single-line summary routes through `emit` for the standard
        // info-glyph `Workflow hint:` prefix.
        emit(project_root, &lines);
        return;
    }
    // TTY: numbered block under its own `Next steps for PR #N:` header
    // (no "Workflow hint:" label — the numbered list is self-describing).
    eprintln!();
    eprintln!(
        "{} {}",
        crate::glyph(crate::glyphs::Glyph::Info).dimmed(),
        lines[0].dimmed()
    );
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
        // BUG-697: shared process-global env lock (was a module-local mutex).
        let _guard = crate::test_env::env_lock();
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

    /// BUG-92: a TOML inline `# comment` after the value is stripped before
    /// parsing, so `workflow_hints = true # note` reads as `true` instead of
    /// silently falling through to the default.
    #[test]
    fn read_config_strips_inline_comment() {
        let td = TempDir::new().unwrap();
        write_config(
            td.path(),
            "[hints]\nworkflow_hints = false # turned off for CI\n",
        );
        assert_eq!(read_config_workflow_hints(td.path()), Some(false));
        write_config(td.path(), "[hints]\nworkflow_hints = true  # back on\n");
        assert_eq!(read_config_workflow_hints(td.path()), Some(true));
        write_config(td.path(), "[hints]\nworkflow_hints = \"true\" # quoted\n");
        assert_eq!(read_config_workflow_hints(td.path()), Some(true));
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
        let lines = session_end_pr_hint_lines(crate::forge::ForgeKind::GitHub, 47, &specs, true);
        // Header + 3 numbered steps + blank + sidebar header + sidebar cmd.
        assert_eq!(lines.len(), 7);
        assert_eq!(lines[0], "Next steps for PR #47:");
        let active = crate::glyphs::Glyph::FlowActive.render(crate::glyphs::active_profile(None));
        assert!(lines[1].contains(&format!("1. {active}")));
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
        let lines = session_end_pr_hint_lines(crate::forge::ForgeKind::GitHub, 47, &specs, false);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("aida queue work PR-47"));
        assert!(lines[0].contains("gh pr merge 47 --squash --delete-branch"));
        assert!(lines[0].contains("aida pull"));
        assert!(lines[0].contains("self-merge"));
    }

    #[test]
    fn session_end_hint_names_multiple_covered_specs() {
        let specs = vec!["TASK-259".to_string(), "BUG-113".to_string()];
        let lines = session_end_pr_hint_lines(crate::forge::ForgeKind::GitHub, 47, &specs, true);
        assert!(lines[3].contains("auto-bumps TASK-259, BUG-113 → Completed"));
    }

    #[test]
    fn session_end_hint_generic_when_no_covered_specs() {
        let lines = session_end_pr_hint_lines(crate::forge::ForgeKind::GitHub, 47, &[], true);
        assert!(lines[3].contains("auto-bumps the merged spec → Completed"));
    }

    // BUG-232: `queue done` on a branch with commits ahead of main and no
    // open PR fires the pointed "no PR open — run /aida-pr" warning, so a
    // `--zen` session can't quietly leave the spec committed-but-unshipped.
    #[test]
    fn queue_drained_warns_when_commits_ahead_and_no_pr() {
        let lines = queue_drained_hint_lines(
            crate::forge::ForgeKind::GitHub,
            Some("implementer"),
            None,
            Some(1),
            Some("task-264"),
            PrState::Absent,
        );
        assert!(lines[0].contains("queue is now empty for role:implementer"));
        assert!(lines[0].contains("(1 commit on this branch)"));
        let warn = crate::glyphs::Glyph::Warning.render(crate::glyphs::active_profile(None));
        assert!(lines[1].contains(&format!("{warn} No PR is open for `task-264`")));
        assert!(lines[1].contains("committed but unshipped"));
        assert!(lines[1].contains("/aida-pr"));
    }

    // An already-open PR redirects the hint to "merge it", not "open one".
    #[test]
    fn queue_drained_points_at_merge_when_pr_open() {
        let lines = queue_drained_hint_lines(
            crate::forge::ForgeKind::GitHub,
            Some("implementer"),
            None,
            Some(2),
            Some("task-264"),
            PrState::Open(97),
        );
        assert!(lines[1].contains("PR #97 is already open"));
        assert!(lines[1].contains("gh pr merge 97 --squash"));
        let warn = crate::glyphs::Glyph::Warning.render(crate::glyphs::active_profile(None));
        assert!(!lines[1].contains(warn));
    }

    // STORY-508: the same hint is forge-aware — GitLab gets MR/glab, pure-git
    // names no forge CLI at all.
    #[test]
    fn queue_drained_hint_is_forge_aware() {
        let gitlab = queue_drained_hint_lines(
            crate::forge::ForgeKind::GitLab,
            Some("implementer"),
            None,
            Some(2),
            Some("task-264"),
            PrState::Open(97),
        );
        assert!(gitlab[1].contains("MR #97 is already open"), "{:?}", gitlab);
        assert!(gitlab[1].contains("glab mr merge 97 --squash"));
        assert!(!gitlab[1].contains("gh pr"));

        let pure_git = queue_drained_hint_lines(
            crate::forge::ForgeKind::None,
            Some("implementer"),
            None,
            Some(2),
            Some("task-264"),
            PrState::Open(97),
        );
        // pure-git: forge-neutral "change", and no forge CLI named.
        assert!(pure_git[1].contains("change #97 is already open"));
        assert!(!pure_git[1].contains("gh "));
        assert!(!pure_git[1].contains("glab "));
        // create-path likewise names no forge CLI for pure-git.
        let pg_create = queue_drained_hint_lines(
            crate::forge::ForgeKind::None,
            Some("implementer"),
            None,
            Some(0),
            Some("task-264"),
            PrState::Unknown,
        );
        assert!(pg_create[1].contains("Open a change with `/aida-pr`"));
        assert!(!pg_create[1].contains("gh "));
        assert!(!pg_create[1].contains("glab "));
    }

    // gh missing / unauthenticated → PrState::Unknown → fall back to the
    // generic nudge rather than asserting a PR is (or isn't) there.
    #[test]
    fn queue_drained_generic_when_pr_state_unknown() {
        let lines = queue_drained_hint_lines(
            crate::forge::ForgeKind::GitHub,
            Some("implementer"),
            None,
            Some(1),
            Some("task-264"),
            PrState::Unknown,
        );
        assert!(lines[1].contains("Open a PR with `/aida-pr`"));
        let warn = crate::glyphs::Glyph::Warning.render(crate::glyphs::active_profile(None));
        assert!(!lines[1].contains(warn));
    }

    // No commits ahead → nothing to ship → no warning even when `gh`
    // reports no PR (the branch is level with main).
    #[test]
    fn queue_drained_no_warning_when_nothing_to_ship() {
        let lines = queue_drained_hint_lines(
            crate::forge::ForgeKind::GitHub,
            Some("implementer"),
            None,
            Some(0),
            Some("task-264"),
            PrState::Absent,
        );
        assert!(!lines[0].contains("commit on this branch"));
        assert!(lines[1].contains("Open a PR with `/aida-pr`"));
        let warn = crate::glyphs::Glyph::Warning.render(crate::glyphs::active_profile(None));
        assert!(!lines[1].contains(warn));
    }

    // BUG-269: simulates the BUG-249 scenario — branch has commits ahead
    // of main and `gh` confirmed no open PR. The pre-check refuses with an
    // actionable error naming the spec, the next command, and the bypass.
    #[test]
    fn precheck_refuses_when_commits_ahead_and_no_pr() {
        let result = queue_done_precheck_error("BUG-249", Some(1), PrState::Absent);
        let lines = result.expect("expected refusal");
        assert!(lines[0].contains("BUG-249 has 1 local commit but no open PR"));
        assert!(lines[1].contains("/aida-pr"));
        assert!(lines[1].contains("aida queue done BUG-249"));
        assert!(lines[2].contains("--skip-pr-check"));
        assert!(lines[2].contains("aida queue done BUG-249 --skip-pr-check"));
    }

    // BUG-285: the refusal text must parse as a hard error even when an
    // LLM reads the message through `2>&1 | tail -N` (which masks the
    // non-zero exit). Pin the load-bearing tokens so a softer rewording
    // can't silently regress the substrate guardrail.
    #[test]
    fn precheck_message_is_loud_for_llm_consumers() {
        let lines = queue_done_precheck_error("BUG-249", Some(1), PrState::Absent).unwrap();
        assert!(
            lines[0].starts_with("error:"),
            "first line must start with `error:` so the tool result parses as a failure even when the exit code is masked by a pipe: {:?}",
            lines[0]
        );
        assert!(
            lines[0].contains("(exit 1)"),
            "first line must explicitly state `(exit 1)` so the model can't read past it: {:?}",
            lines[0]
        );
        assert!(
            lines[1].contains("DO NOT exit"),
            "second line must explicitly forbid exiting the session — guards against the TASK-413 / TASK-416 finish-state-summary failure mode: {:?}",
            lines[1]
        );
        assert!(
            lines[1].contains("orchestrator will fail phase 1"),
            "second line must name the orchestrator-side consequence: {:?}",
            lines[1]
        );
    }

    // Plural "commits" when more than one is ahead.
    #[test]
    fn precheck_pluralizes_commits() {
        let lines = queue_done_precheck_error("STORY-42", Some(3), PrState::Absent).unwrap();
        assert!(lines[0].contains("3 local commits but no open PR"));
    }

    // BUG-269 happy path: an open PR already references the spec.
    #[test]
    fn precheck_proceeds_when_pr_open() {
        let result = queue_done_precheck_error("BUG-249", Some(1), PrState::Open(42));
        assert!(result.is_none());
    }

    // No commits ahead → spec was a no-op or already shipped via another
    // branch — proceed silently (existing behavior).
    #[test]
    fn precheck_proceeds_when_no_commits_ahead() {
        let result = queue_done_precheck_error("BUG-249", Some(0), PrState::Absent);
        assert!(result.is_none());
    }

    // `gh` unreachable / missing / unauthenticated → cannot assert no-PR
    // on guesswork. Proceed rather than risk blocking a legitimate done.
    #[test]
    fn precheck_proceeds_when_pr_state_unknown() {
        let result = queue_done_precheck_error("BUG-249", Some(2), PrState::Unknown);
        assert!(result.is_none());
    }

    // Commits-ahead unknown (helper returned None) → proceed silently.
    #[test]
    fn precheck_proceeds_when_commits_unknown() {
        let result = queue_done_precheck_error("BUG-249", None, PrState::Absent);
        assert!(result.is_none());
    }

    // BUG-285 acceptance: `--yes` does NOT bypass the gate. The whole
    // matrix is exhausted here so a regression on any cell fails loudly.
    // `(yes=true, force=false, skip_pr_check=false)` is the exact
    // condition tonight's TASK-413 drain hit; it MUST run the gate.
    #[test]
    fn bypass_with_yes_only() {
        // Default — no bypass flags — runs the gate.
        assert!(!queue_done_should_bypass_pr_check(false, false, false));
        // The BUG-285 case: --yes alone does NOT bypass. This is the
        // load-bearing assertion — the spec was filed because the model
        // hypothesized --yes was bypassing; the gate must fire regardless.
        assert!(
            !queue_done_should_bypass_pr_check(true, false, false),
            "--yes alone must NOT bypass the PR check (BUG-285)"
        );
    }

    #[test]
    fn bypass_with_force() {
        assert!(queue_done_should_bypass_pr_check(false, true, false));
        // --yes + --force: still bypasses (the --force is what's doing it).
        assert!(queue_done_should_bypass_pr_check(true, true, false));
    }

    #[test]
    fn bypass_with_skip_pr_check() {
        assert!(queue_done_should_bypass_pr_check(false, false, true));
        // --yes + --skip-pr-check: still bypasses (the skip flag is doing it).
        assert!(queue_done_should_bypass_pr_check(true, false, true));
    }

    #[test]
    fn bypass_with_both_force_and_skip_pr_check() {
        // Both flags are equivalent for this gate; passing both is a no-op
        // duplicate, never a contradiction. Verify they compose cleanly.
        assert!(queue_done_should_bypass_pr_check(false, true, true));
        assert!(queue_done_should_bypass_pr_check(true, true, true));
    }

    // TASK-500: the diagnose function resolves the whole gate tree from
    // injected closures, so each skip path / refusal / proceed is exercised
    // here with no real git/gh. Helpers fabricate the closure inputs.

    fn root_ok() -> anyhow::Result<PathBuf> {
        Ok(PathBuf::from("/tmp/fake-project"))
    }

    // SkipReason::ProjectRootNotFound — find_project_root failed.
    #[test]
    fn diagnose_skips_when_project_root_not_found() {
        let result = queue_done_precheck_diagnose(
            "BUG-249",
            Err(anyhow::anyhow!("no .aida anchor")),
            |_| panic!("branch lookup must not run when root resolution failed"),
            |_, _| panic!("commits-ahead must not run"),
            |_, _| panic!("pr lookup must not run"),
        );
        match result {
            QueueDoneGateDiagnose::SilentSkip {
                reason,
                warning_line,
            } => {
                assert_eq!(reason, SkipReason::ProjectRootNotFound);
                assert!(warning_line.contains("find_project_root failed"));
                assert!(warning_line.contains("no .aida anchor"));
            }
            other => panic!("expected SilentSkip(ProjectRootNotFound), got {other:?}"),
        }
    }

    // SkipReason::BranchUndetectable — current branch couldn't be read.
    #[test]
    fn diagnose_skips_when_branch_undetectable() {
        let result = queue_done_precheck_diagnose(
            "BUG-249",
            root_ok(),
            |_| None,
            |_, _| panic!("commits-ahead must not run when branch is undetectable"),
            |_, _| panic!("pr lookup must not run"),
        );
        match result {
            QueueDoneGateDiagnose::SilentSkip {
                reason,
                warning_line,
            } => {
                assert_eq!(reason, SkipReason::BranchUndetectable);
                assert!(warning_line.contains("current_branch_at returned None"));
            }
            other => panic!("expected SilentSkip(BranchUndetectable), got {other:?}"),
        }
    }

    // SkipReason::CommitsAheadFailed — rev-list failed on a real branch.
    #[test]
    fn diagnose_skips_when_commits_ahead_failed() {
        let result = queue_done_precheck_diagnose(
            "BUG-249",
            root_ok(),
            |_| Some("feature-x".to_string()),
            |_, _| None,
            |_, _| panic!("pr lookup must not run when commits-ahead is unknown"),
        );
        match result {
            QueueDoneGateDiagnose::SilentSkip {
                reason,
                warning_line,
            } => {
                assert_eq!(reason, SkipReason::CommitsAheadFailed);
                assert!(warning_line.contains("branch_commits_ahead_main"));
                assert!(warning_line.contains("feature-x"));
            }
            other => panic!("expected SilentSkip(CommitsAheadFailed), got {other:?}"),
        }
    }

    // commits-ahead None on `main`/`HEAD` is the intentional no-op skip:
    // proceed silently, NOT a CommitsAheadFailed warning.
    #[test]
    fn diagnose_proceeds_silently_on_main_branch() {
        for branch in ["main", "HEAD"] {
            let result = queue_done_precheck_diagnose(
                "BUG-249",
                root_ok(),
                |_| Some(branch.to_string()),
                |_, _| None,
                |_, _| panic!("pr lookup must not run on main/HEAD"),
            );
            assert_eq!(
                result,
                QueueDoneGateDiagnose::Proceed,
                "branch {branch} should proceed silently"
            );
        }
    }

    // SkipReason::GhUnknown — forge lookup couldn't confirm PR state; warn,
    // then the caller proceeds (we never assert "no PR" on guesswork).
    #[test]
    fn diagnose_skips_when_gh_unknown() {
        let result = queue_done_precheck_diagnose(
            "BUG-249",
            root_ok(),
            |_| Some("feature-x".to_string()),
            |_, _| Some(2),
            |_, _| PrState::Unknown,
        );
        match result {
            QueueDoneGateDiagnose::SilentSkip {
                reason,
                warning_line,
            } => {
                assert_eq!(reason, SkipReason::GhUnknown);
                assert!(warning_line.contains("without `gh` confirmation"));
            }
            other => panic!("expected SilentSkip(GhUnknown), got {other:?}"),
        }
    }

    // Refuse — commits ahead AND forge confirmed no open PR. The lines are
    // exactly queue_done_precheck_error's, so they compose with its tests.
    #[test]
    fn diagnose_refuses_when_commits_ahead_and_no_pr() {
        let result = queue_done_precheck_diagnose(
            "BUG-249",
            root_ok(),
            |_| Some("feature-x".to_string()),
            |_, _| Some(1),
            |_, _| PrState::Absent,
        );
        match result {
            QueueDoneGateDiagnose::Refuse(lines) => {
                assert_eq!(
                    lines,
                    queue_done_precheck_error("BUG-249", Some(1), PrState::Absent).unwrap(),
                    "Refuse lines must match queue_done_precheck_error verbatim"
                );
                assert!(lines[0].contains("BUG-249 has 1 local commit but no open PR"));
            }
            other => panic!("expected Refuse, got {other:?}"),
        }
    }

    // Proceed — commits ahead but a PR is already open.
    #[test]
    fn diagnose_proceeds_when_pr_open() {
        let result = queue_done_precheck_diagnose(
            "BUG-249",
            root_ok(),
            |_| Some("feature-x".to_string()),
            |_, _| Some(3),
            |_, _| PrState::Open(42),
        );
        assert_eq!(result, QueueDoneGateDiagnose::Proceed);
    }

    // Proceed — branch is level with main (zero commits ahead), so there's
    // nothing to ship; the PR lookup is never consulted.
    #[test]
    fn diagnose_proceeds_when_no_commits_ahead() {
        let result = queue_done_precheck_diagnose(
            "BUG-249",
            root_ok(),
            |_| Some("feature-x".to_string()),
            |_, _| Some(0),
            |_, _| panic!("pr lookup must not run when nothing is ahead"),
        );
        assert_eq!(result, QueueDoneGateDiagnose::Proceed);
    }
}
