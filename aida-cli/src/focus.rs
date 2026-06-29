// trace:STORY-706 | ai:claude
//! Per-worktree focus context — the CLI side of `aida focus` (STORY-706).
//!
//! A *focus* is a persistent, per-worktree current-context (an epic or spec)
//! that scopes the read commands (`aida list`, `aida status`, `aida queue
//! list`) to that spec's transitive descendant subtree — the
//! kubectl-namespace / gcloud-config / aws-profile pattern applied to AIDA's
//! requirement graph. It unifies today's scattered scoping (`aida list
//! --parent`, `aida queue list --epic`, `AIDA_TUI_EPIC`) into one context.
//!
//! The make-or-break constraint is **loud visibility**: a silent persistent
//! focus is a footgun (you forget you're scoped and read a subset as if it is
//! everything). Every focus-scoped output therefore carries a header, and the
//! statusline surfaces the active focus — the focus is never silently applied.
//!
//! Storage: one line in `<project_root>/.aida/focus`, auto-gitignored by the
//! `.aida/*` deny-by-default convention (runtime per-clone state, never
//! tracked). Resolution precedence mirrors the TUI focus (STORY-697):
//! **`AIDA_FOCUS` env > `.aida/focus` marker > none**.
//!
//!

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Environment override for the active focus. Highest-precedence tier; a blank
/// value falls through to the marker file.
pub const FOCUS_ENV: &str = "AIDA_FOCUS";

// ----------------------------------------------------------- drift guard (STORY-717)

/// The `[focus] out_of_scope` policy that governs what happens when you START
/// work on a spec outside the active focus's subtree (STORY-717). The guard
/// fires at work-start moments only (`queue work`, `agent new --spec`, flipping
/// a spec to In Progress) -- never at commit time -- so cross-scope reads stay
/// free and only the act of *starting* work is nudged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutOfScopePolicy {
    /// No-op: the guard is silent. The escape hatch for operators who routinely
    /// work across scopes and don't want the nudge.
    Off,
    /// Print a nudge to stderr that suggests the fix, then PROCEED (default).
    #[default]
    Warn,
    /// Refuse the work-start with the nudge + a non-zero exit, unless `--force`.
    Block,
}

/// Parse the `[focus] out_of_scope` config value. Unknown / empty values fall
/// back to the default (`warn`) -- a typo softens to the safe default rather
/// than erroring at every work-start. Case-insensitive; `-`/`_` tolerated.
pub fn parse_out_of_scope_policy(raw: &str) -> OutOfScopePolicy {
    match raw.trim().to_ascii_lowercase().replace('-', "_").as_str() {
        "off" | "none" | "silent" => OutOfScopePolicy::Off,
        "block" | "refuse" | "hard" => OutOfScopePolicy::Block,
        // "warn" and anything unrecognized -> the safe default.
        _ => OutOfScopePolicy::Warn,
    }
}

/// The action the drift guard should take at a work-start moment. A PURE
/// decision over (policy, in-scope?, force?) so the policy matrix is
/// unit-testable without a backend or filesystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusGuardAction {
    /// Let the work-start proceed silently.
    Proceed,
    /// Print the nudge to stderr, then proceed.
    Warn,
    /// Refuse the work-start (non-zero exit).
    Block,
}

/// Decide what the drift guard does. `--force` ALWAYS overrides (proceeds
/// silently) regardless of policy; a spec already in the focus subtree always
/// proceeds; otherwise the policy decides. (The no-focus case is handled by the
/// caller -- it never reaches here.)
pub fn decide_focus_action(
    policy: OutOfScopePolicy,
    in_scope: bool,
    force: bool,
) -> FocusGuardAction {
    if in_scope || force {
        return FocusGuardAction::Proceed;
    }
    match policy {
        OutOfScopePolicy::Off => FocusGuardAction::Proceed,
        OutOfScopePolicy::Warn => FocusGuardAction::Warn,
        OutOfScopePolicy::Block => FocusGuardAction::Block,
    }
}

/// The membership predicate: is `target` inside the focus subtree? The subtree
/// set comes from the cache's `descendant_ids` closure (TASK-955), which
/// INCLUDES the focus root itself -- so the focus epic and every transitive
/// descendant are in-scope, and any other spec is out-of-scope. PURE (a set
/// membership test) so the predicate is unit-testable in isolation.
pub fn is_in_focus_scope(target: &Uuid, subtree: &HashSet<Uuid>) -> bool {
    subtree.contains(target)
}

/// Build the out-of-scope nudge: the core mismatch sentence plus, when a
/// suggested focus is known, the "Did you mean ..." fix. PURE (no color/glyph)
/// so the caller can prepend a glyph and append a mode-specific suffix and the
/// wording stays testable.
///
/// Shape: `STORY-714 is not under your current focus (EPIC-54). Did you mean
/// 'aida focus EPIC-56' first?`
pub fn out_of_scope_message(
    target_label: &str,
    focus_label: &str,
    suggested_focus: Option<&str>,
) -> String {
    let mut msg = format!("{target_label} is not under your current focus ({focus_label}).");
    if let Some(sug) = suggested_focus {
        msg.push_str(&format!(" Did you mean 'aida focus {sug}' first?"));
    }
    msg
}

/// The per-worktree focus marker path: `<project_root>/.aida/focus`. A pure
/// path-builder (no IO) so the path logic is unit-testable. The file is
/// auto-gitignored by the `.aida/*` deny-by-default convention, so it is never
/// tracked.
pub fn focus_marker_path(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join("focus")
}

/// Read the per-worktree focus marker (the first non-empty line of
/// `.aida/focus`), or `None` when it is absent / empty. A thin FS wrapper; the
/// precedence logic it feeds lives in the pure [`resolve_focus_precedence`].
pub fn read_focus_marker(project_root: &Path) -> Option<String> {
    let content = std::fs::read_to_string(focus_marker_path(project_root)).ok()?;
    let line = content.lines().map(str::trim).find(|l| !l.is_empty())?;
    Some(line.to_string())
}

/// Write the per-worktree focus marker (one line = `spec`), creating `.aida/`
/// if needed. Surfaces write errors so `aida focus <spec>` can report a
/// persistence failure rather than silently no-op'ing (focus IS load-bearing
/// here, unlike the TUI's best-effort marker).
pub fn write_focus_marker(project_root: &Path, spec: &str) -> std::io::Result<()> {
    let path = focus_marker_path(project_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, format!("{}\n", spec.trim()))
}

/// Clear the per-worktree focus marker (remove the file). Returns whether a
/// marker was actually present, so the caller can distinguish "cleared" from
/// "there was nothing set". A missing file is not an error.
pub fn clear_focus_marker(project_root: &Path) -> bool {
    let path = focus_marker_path(project_root);
    if path.exists() {
        let _ = std::fs::remove_file(&path);
        true
    } else {
        false
    }
}

/// Resolve the active focus from the (already-read) `AIDA_FOCUS` env value and
/// the marker-file contents, with precedence **env > marker > none**. A blank /
/// whitespace-only value at either tier is ignored (falls through). PURE — both
/// inputs are passed in, so the precedence logic is unit-testable without
/// touching the environment or the filesystem.
pub fn resolve_focus_precedence(env: Option<&str>, marker: Option<&str>) -> Option<String> {
    for tier in [env, marker] {
        if let Some(v) = tier {
            let t = v.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    None
}

/// Resolve the active focus for `project_root`: reads `AIDA_FOCUS` from the
/// environment and the `.aida/focus` marker, applies the precedence in
/// [`resolve_focus_precedence`]. `None` when no focus is set at any tier.
pub fn resolve_focus(project_root: &Path) -> Option<String> {
    let env = std::env::var(FOCUS_ENV).ok();
    let marker = read_focus_marker(project_root);
    resolve_focus_precedence(env.as_deref(), marker.as_deref())
}

/// Build the loud one-line header that every focus-scoped command prints above
/// its output, so a persistent focus can never silently subset a view. Shape:
/// `focused: EPIC-X — showing N of M  (--all / --no-focus to widen)`, with a
/// leading focus-marker glyph. `shown`/`total` are the post-/pre-focus row
/// counts for the current filter set.
//
pub fn focus_header(focus_label: &str, shown: usize, total: usize) -> String {
    format!(
        "\u{25b8} focused: {focus_label} \u{2014} showing {shown} of {total}  \
         (--all / --no-focus to widen)"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_root() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "aida-focus-test-{}-{}",
            std::process::id(),
            // a coarse uniqueness nonce so parallel tests don't collide
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn marker_path_is_under_dot_aida() {
        let p = focus_marker_path(Path::new("/proj"));
        assert_eq!(p, Path::new("/proj/.aida/focus"));
    }

    #[test]
    fn set_show_clear_roundtrip() {
        let root = tmp_root();
        // show before set: nothing
        assert_eq!(read_focus_marker(&root), None);
        // set
        write_focus_marker(&root, "EPIC-55").unwrap();
        assert_eq!(read_focus_marker(&root).as_deref(), Some("EPIC-55"));
        // clear reports it removed something
        assert!(clear_focus_marker(&root));
        assert_eq!(read_focus_marker(&root), None);
        // clearing again reports nothing was set
        assert!(!clear_focus_marker(&root));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn write_trims_and_single_lines() {
        let root = tmp_root();
        write_focus_marker(&root, "  STORY-706  ").unwrap();
        let raw = std::fs::read_to_string(focus_marker_path(&root)).unwrap();
        assert_eq!(raw, "STORY-706\n");
        assert_eq!(read_focus_marker(&root).as_deref(), Some("STORY-706"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn blank_marker_reads_as_none() {
        let root = tmp_root();
        std::fs::create_dir_all(focus_marker_path(&root).parent().unwrap()).unwrap();
        std::fs::write(focus_marker_path(&root), "   \n\n").unwrap();
        assert_eq!(read_focus_marker(&root), None);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn precedence_env_beats_marker() {
        assert_eq!(
            resolve_focus_precedence(Some("EPIC-1"), Some("EPIC-2")).as_deref(),
            Some("EPIC-1")
        );
    }

    #[test]
    fn precedence_blank_env_falls_through_to_marker() {
        assert_eq!(
            resolve_focus_precedence(Some("   "), Some("EPIC-2")).as_deref(),
            Some("EPIC-2")
        );
    }

    #[test]
    fn precedence_none_when_both_absent_or_blank() {
        assert_eq!(resolve_focus_precedence(None, None), None);
        assert_eq!(resolve_focus_precedence(Some(""), Some("  ")), None);
    }

    #[test]
    fn header_is_loud_and_names_the_escape() {
        let h = focus_header("EPIC-55", 13, 2400);
        assert!(h.contains("focused: EPIC-55"));
        assert!(h.contains("showing 13 of 2400"));
        assert!(h.contains("--all"));
        assert!(h.contains("--no-focus"));
    }

    // ----------------------------------------------- drift guard (STORY-717)

    #[test]
    fn policy_parse_warn_is_default_for_unknown_and_blank() {
        assert_eq!(parse_out_of_scope_policy("warn"), OutOfScopePolicy::Warn);
        assert_eq!(parse_out_of_scope_policy(""), OutOfScopePolicy::Warn);
        assert_eq!(
            parse_out_of_scope_policy("nonsense"),
            OutOfScopePolicy::Warn
        );
        assert_eq!(OutOfScopePolicy::default(), OutOfScopePolicy::Warn);
    }

    #[test]
    fn policy_parse_off_and_block_and_case_insensitive() {
        assert_eq!(parse_out_of_scope_policy("off"), OutOfScopePolicy::Off);
        assert_eq!(parse_out_of_scope_policy(" OFF "), OutOfScopePolicy::Off);
        assert_eq!(parse_out_of_scope_policy("block"), OutOfScopePolicy::Block);
        assert_eq!(parse_out_of_scope_policy("Block"), OutOfScopePolicy::Block);
    }

    #[test]
    fn membership_predicate_uses_the_subtree_set() {
        let root = Uuid::new_v4();
        let child = Uuid::new_v4();
        let outsider = Uuid::new_v4();
        // descendant_ids includes the root itself + descendants.
        let subtree: HashSet<Uuid> = [root, child].into_iter().collect();
        assert!(is_in_focus_scope(&root, &subtree)); // the focus epic itself
        assert!(is_in_focus_scope(&child, &subtree)); // a descendant
        assert!(!is_in_focus_scope(&outsider, &subtree)); // EPIC-B spec
    }

    #[test]
    fn decide_in_scope_always_proceeds_regardless_of_policy() {
        for policy in [
            OutOfScopePolicy::Off,
            OutOfScopePolicy::Warn,
            OutOfScopePolicy::Block,
        ] {
            assert_eq!(
                decide_focus_action(policy, /* in_scope */ true, /* force */ false),
                FocusGuardAction::Proceed
            );
        }
    }

    #[test]
    fn decide_off_is_silent_when_out_of_scope() {
        assert_eq!(
            decide_focus_action(OutOfScopePolicy::Off, false, false),
            FocusGuardAction::Proceed
        );
    }

    #[test]
    fn decide_warn_nudges_when_out_of_scope() {
        assert_eq!(
            decide_focus_action(OutOfScopePolicy::Warn, false, false),
            FocusGuardAction::Warn
        );
    }

    #[test]
    fn decide_block_refuses_when_out_of_scope() {
        assert_eq!(
            decide_focus_action(OutOfScopePolicy::Block, false, false),
            FocusGuardAction::Block
        );
    }

    #[test]
    fn decide_force_overrides_every_policy() {
        for policy in [
            OutOfScopePolicy::Off,
            OutOfScopePolicy::Warn,
            OutOfScopePolicy::Block,
        ] {
            assert_eq!(
                decide_focus_action(policy, /* in_scope */ false, /* force */ true),
                FocusGuardAction::Proceed
            );
        }
    }

    #[test]
    fn out_of_scope_message_names_mismatch_and_suggests_fix() {
        let m = out_of_scope_message("STORY-714", "EPIC-54", Some("EPIC-56"));
        assert!(m.contains("STORY-714 is not under your current focus (EPIC-54)."));
        assert!(m.contains("Did you mean 'aida focus EPIC-56' first?"));
    }

    #[test]
    fn out_of_scope_message_omits_suggestion_when_unknown() {
        let m = out_of_scope_message("STORY-714", "EPIC-54", None);
        assert!(m.contains("STORY-714 is not under your current focus (EPIC-54)."));
        assert!(!m.contains("Did you mean"));
    }
}
