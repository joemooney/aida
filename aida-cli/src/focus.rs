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

use std::path::{Path, PathBuf};

/// Environment override for the active focus. Highest-precedence tier; a blank
/// value falls through to the marker file.
pub const FOCUS_ENV: &str = "AIDA_FOCUS";

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
}
