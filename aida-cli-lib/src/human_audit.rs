//! `aida human audit` — trigger the `/aida-human-audit` reconcile pass at the
//! advisor session. Two mechanisms the operator chose:
//!
//! 1. **Enqueue** (default, durable) — post a directive onto the existing
//!    worker-directive substrate (`.aida/worker.cmd`, see [`crate::worker`]).
//!    The polling advisor surfaces it via `aida worker directives` (the
//!    `render_human` poll view) and its per-turn directive check, then runs the
//!    skill. Works headless + cross-vendor; latency is up to one poll cycle.
//!
//! 2. **`--inject`** (opt-in, immediate) — `tmux send-keys` the slash command
//!    straight into the advisor's registered pane, so it fires even when the
//!    advisor is idle. The pane is recorded at advisor session start from the
//!    `TMUX_PANE` env; when no pane is registered (not in tmux, unset env), the
//!    inject path prints a clear message and falls back to the enqueue path — it
//!    never hard-fails.
//!
//! The pure bits — the directive payload builder, the pane read/write, and the
//! `send-keys` argv builder — live here so the unit tests exercise them without
//! filesystem or tmux I/O.
//!
//! trace:STORY-768 | ai:claude

use std::path::{Path, PathBuf};

/// The advisor skill command an audit request runs.
pub(crate) const HUMAN_AUDIT_SKILL: &str = "/aida-human-audit";

/// The directive verb `aida human audit` posts onto `.aida/worker.cmd`. The
/// polling advisor treats it as an actionable "run the human-audit pass"
/// request; the worker's `render_human` shows it verbatim.
pub(crate) const HUMAN_AUDIT_VERB: &str = "human-audit";

/// Well-known file (under `.aida/`) recording the advisor session's tmux pane
/// id. Runtime per-clone state — covered by the deny-by-default `.aida/*`
/// gitignore rule, no allow-list entry needed.
const ADVISOR_PANE_FILE: &str = "advisor-pane";

/// Pure builder for the directive line written to `.aida/worker.cmd`. Kept
/// pure so tests assert the verb + skill payload without filesystem I/O.
pub(crate) fn directive_line() -> String {
    format!("{HUMAN_AUDIT_VERB} {HUMAN_AUDIT_SKILL}")
}

/// Enqueue the human-audit directive onto the worker-directive channel
/// (`.aida/worker.cmd`). The polling advisor surfaces it via `aida worker
/// directives` and its per-turn check, then runs the skill.
pub(crate) fn post_directive_line_enqueue(project_root: &Path) -> std::io::Result<()> {
    crate::worker::post_directive_line(project_root, &directive_line())
}

/// Path of the advisor-pane file under `project_root`.
pub(crate) fn advisor_pane_path(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join(ADVISOR_PANE_FILE)
}

/// Record `TMUX_PANE` (from the process env) into the advisor-pane file.
/// Idempotent — overwrites with the current pane each call. Returns
/// `Ok(Some(pane))` when a pane was written, `Ok(None)` when `TMUX_PANE` is
/// unset or blank (a no-op — never an error, so a non-tmux advisor session
/// start is unaffected).
pub(crate) fn register_pane_from_env(project_root: &Path) -> std::io::Result<Option<String>> {
    let pane = match std::env::var("TMUX_PANE") {
        Ok(p) if !p.trim().is_empty() => p.trim().to_string(),
        _ => return Ok(None),
    };
    let path = advisor_pane_path(project_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, format!("{pane}\n"))?;
    Ok(Some(pane))
}

/// Read the registered advisor pane, if any. `None` when the file is absent or
/// blank.
pub(crate) fn read_pane(project_root: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(advisor_pane_path(project_root)).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Pure builder for the `tmux send-keys` argv (including the `tmux` program
/// name) that injects the skill command into `pane`, then presses Enter.
pub(crate) fn inject_send_keys_argv(pane: &str) -> Vec<String> {
    vec![
        "tmux".to_string(),
        "send-keys".to_string(),
        "-t".to_string(),
        pane.to_string(),
        HUMAN_AUDIT_SKILL.to_string(),
        "Enter".to_string(),
    ]
}

/// Resolution of `aida human audit --inject`: either inject via the registered
/// pane's send-keys argv, or fall back to the enqueue path when no pane is
/// registered. Pure over the resolved pane so the dispatch decision is
/// testable without env or tmux.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum InjectPlan {
    /// A pane is registered — run this `tmux send-keys` argv.
    Inject(Vec<String>),
    /// No usable pane — the caller should print a message and enqueue instead.
    Fallback,
}

/// Decide the inject path from a resolved pane. A present, non-blank pane
/// yields the send-keys argv; anything else falls back to enqueue.
pub(crate) fn plan_inject(pane: Option<String>) -> InjectPlan {
    match pane {
        Some(p) if !p.trim().is_empty() => InjectPlan::Inject(inject_send_keys_argv(p.trim())),
        _ => InjectPlan::Fallback,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // AC: the posted directive carries the right verb + skill payload so the
    // advisor's poll surface shows an actionable "run /aida-human-audit"
    // request.
    #[test]
    fn directive_line_has_verb_and_skill() {
        let line = directive_line();
        assert_eq!(line, "human-audit /aida-human-audit");
        // The worker parses the first word as the verb, the rest as args.
        let mut words = line.split_whitespace();
        assert_eq!(words.next(), Some(HUMAN_AUDIT_VERB));
        assert_eq!(words.next(), Some(HUMAN_AUDIT_SKILL));
        assert_eq!(words.next(), None);
    }

    // The posted directive round-trips through the real worker parser and
    // surfaces via `render_human` (the advisor's existing poll view).
    #[test]
    fn directive_line_surfaces_in_worker_render_human() {
        let parsed = crate::worker::parse_directives_from_str(&format!("{}\n", directive_line()));
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].verb, HUMAN_AUDIT_VERB);
        assert_eq!(parsed[0].args, vec![HUMAN_AUDIT_SKILL.to_string()]);
        let human = crate::worker::render_human(&parsed);
        assert!(human.contains(HUMAN_AUDIT_SKILL), "poll view: {human}");
    }

    // AC: `--inject` builds the correct `tmux send-keys` argv from a
    // registered pane.
    #[test]
    fn inject_send_keys_argv_is_correct() {
        assert_eq!(
            inject_send_keys_argv("%3"),
            vec![
                "tmux".to_string(),
                "send-keys".to_string(),
                "-t".to_string(),
                "%3".to_string(),
                "/aida-human-audit".to_string(),
                "Enter".to_string(),
            ]
        );
    }

    // AC: a registered pane drives the inject argv.
    #[test]
    fn plan_inject_with_pane_injects() {
        match plan_inject(Some("%7".to_string())) {
            InjectPlan::Inject(argv) => assert_eq!(argv, inject_send_keys_argv("%7")),
            InjectPlan::Fallback => panic!("expected inject with a registered pane"),
        }
    }

    // AC: the not-in-tmux / no-pane fallback path — a missing or blank pane
    // falls back to enqueue rather than hard-failing.
    #[test]
    fn plan_inject_without_pane_falls_back() {
        assert_eq!(plan_inject(None), InjectPlan::Fallback);
        assert_eq!(plan_inject(Some(String::new())), InjectPlan::Fallback);
        assert_eq!(plan_inject(Some("   ".to_string())), InjectPlan::Fallback);
    }

    // Pane registration is a no-op (Ok(None)) when TMUX_PANE is unset — an
    // advisor session started outside tmux writes nothing and never errors.
    #[test]
    fn register_pane_no_op_when_tmux_pane_unset() {
        let dir = tempfile::tempdir().unwrap();
        // Guard the env var so the test is self-contained regardless of the
        // ambient shell.
        let prev = std::env::var("TMUX_PANE").ok();
        std::env::remove_var("TMUX_PANE");
        let got = register_pane_from_env(dir.path()).unwrap();
        if let Some(p) = prev {
            std::env::set_var("TMUX_PANE", p);
        }
        assert_eq!(got, None);
        assert!(read_pane(dir.path()).is_none());
        assert!(!advisor_pane_path(dir.path()).exists());
    }
}
