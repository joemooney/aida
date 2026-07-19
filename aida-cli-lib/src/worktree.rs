// trace:STORY-716 | ai:claude
//! `aida worktree` — the EPIC-55 workspace layer.
//!
//! A subcommand namespace that mirrors `git worktree` and houses AIDA's
//! worktree operations. The first slice (STORY-716) is **create-or-enter an
//! epic-scoped workspace**:
//!
//! - `aida worktree add <epic>` — create a git worktree off origin/main
//!   (default path `~/ai/aida-<epic-slug>`, default branch `<epic>-work`),
//!   then write the STORY-706 focus marker INSIDE the new worktree so it is
//!   auto-scoped to that epic. Plain subcommand: prints the path, does NOT cd
//!   (mirrors `git worktree add`). Idempotent — an existing worktree is
//!   re-affirmed, not re-created.
//! - `aida worktree enter <epic>` — the eval-wrapper that creates-if-missing
//!   then **cd's you in**. A subprocess cannot cd its parent shell, so this
//!   follows the existing auto-eval shell-function pattern (`aida role enter`
//!   / `aida dev activate`): the binary emits `cd '<path>'` on stdout and the
//!   `aida()` shell wrapper auto-evals it. MUST be run as bare
//!   `aida worktree enter <epic>` — NOT wrapped in `eval "$(...)"` (the
//!   function auto-evals; the no-double-eval convention, TASK-667).
//! - `aida worktree list` — AIDA-managed worktrees + each one's focus.
//!
//! This module owns the **pure derivation helpers** (path / branch / slug)
//! plus the porcelain parsing, all unit-testable without git. The thin
//! IO/dispatch layer (running `git worktree add`, printing, emitting shell)
//! lives in `main.rs` next to the other command handlers.
//!
//! STORY-714 (warm-pool) will land `aida worktree pool status` + a tiered
//! `aida worktree remove` as sibling subcommands under this same namespace —
//! the `WorktreeCommand` enum in `cli.rs` is shaped to leave room for them.

use std::path::{Path, PathBuf};

/// Normalize a raw epic argument to its canonical uppercase SPEC-ID form, used
/// as the focus label written into the new worktree's `.aida/focus`.
/// `epic-54` / `Epic-54` / `EPIC-54` -> `EPIC-54`. Best-effort (uppercase
/// only): the worktree slice intentionally does NOT hit the cache to validate
/// — the focus marker is written via the low-level `focus::write_focus_marker`,
/// not the validating `aida focus` path (which needs the store the fresh
/// worktree hasn't attached yet).
pub fn normalize_epic_label(raw: &str) -> String {
    raw.trim().to_ascii_uppercase()
}

/// Slug used in the default worktree directory name: lowercased, with every
/// non-alphanumeric character dropped. `EPIC-54` -> `epic54`.
pub fn epic_slug(raw: &str) -> String {
    raw.trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Default branch name for an epic worktree: `EPIC-54` -> `epic-54-work`.
/// The canonical id lowercased plus a `-work` suffix.
pub fn default_branch(raw: &str) -> String {
    format!("{}-work", normalize_epic_label(raw).to_ascii_lowercase())
}

/// Default worktree path: `<home>/ai/aida-<slug>`. With `home = ~`,
/// `EPIC-54` -> `~/ai/aida-epic54`. Sibling of the main `~/ai/aida` clone, so
/// epic worktrees land as siblings rather than nesting.
pub fn default_worktree_path(home: &Path, raw: &str) -> PathBuf {
    home.join("ai").join(format!("aida-{}", epic_slug(raw)))
}

/// Parse `git worktree list --porcelain` stdout into the list of registered
/// worktree paths. Each record opens with a `worktree <path>` line; we collect
/// those. Pure so the porcelain handling is unit-testable without git.
pub fn parse_worktree_paths(porcelain: &str) -> Vec<PathBuf> {
    porcelain
        .lines()
        .filter_map(|l| l.strip_prefix("worktree "))
        .map(|p| PathBuf::from(p.trim()))
        .collect()
}

/// Whether `target` is already a registered worktree in `porcelain` output.
/// Compares canonicalized paths when possible (so `~/ai/aida-epic54` and a
/// symlink-resolved form match), falling back to a literal compare.
pub fn is_registered(porcelain: &str, target: &Path) -> bool {
    let want = canonical_or_self(target);
    parse_worktree_paths(porcelain)
        .iter()
        .any(|p| canonical_or_self(p) == want)
}

/// Canonicalize a path, falling back to the path itself when it does not yet
/// exist (so comparisons work for not-yet-created targets too).
pub(crate) fn canonical_or_self(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

// --- ambient worktree PS1 indicator + symmetric exit payload -----------------
//
// `worktree enter` splices an always-visible `(wt:<FOCUS>) ` segment into PS1
// so a shell standing inside a scoped worktree can't be mistaken for the main
// checkout; `worktree exit` strips it again and clears the session env exports
// enter applied. All pure string builders so the emitted-shell contract is
// unit-testable without git or a live shell.
// trace:TASK-1160 | ai:claude

/// Escape a string for interpolation inside a single-quoted shell word
/// (`'` -> `'\''`). Local twin of the crate-root helper — kept here so the
/// pure module stays dependency-free.
// trace:TASK-1160 | ai:claude
fn wt_single_quote(s: &str) -> String {
    s.replace('\'', "'\\''")
}

/// The literal PS1 segment spliced in for a worktree scoped to `focus`:
/// `(wt:EPIC-54) `. Recorded verbatim in `AIDA_WT_PS1_PREFIX` so exit can
/// strip exactly what enter added (the splice-in pattern shared with
/// `AIDA_DEV_PS1_PREFIX`).
// trace:TASK-1160 | ai:claude
pub fn wt_ps1_prefix(focus: &str) -> String {
    format!("(wt:{}) ", focus.trim())
}

/// Shell block that strips EVERY `(wt:...) ` token from PS1 — the same
/// walk-and-strip loop the role/dev prompt hygiene uses, so repeated enters
/// (or an enter after a bare `cd` out) never stack duplicate segments.
// trace:TASK-1160 | ai:claude
fn ps1_wt_strip_loop() -> String {
    concat!(
        "if [ -n \"${PS1+x}\" ]; then\n",
        "    while case \"$PS1\" in *'(wt:'*') '*) true;; *) false;; esac; do\n",
        "        _aida_old_ps1=\"$PS1\"\n",
        "        _aida_after=\"${PS1#*'(wt:'}\"\n",
        "        _aida_tag=\"${_aida_after%%') '*}\"\n",
        "        PS1=\"${PS1//'(wt:'$_aida_tag') '/}\"\n",
        "        [ \"$PS1\" = \"$_aida_old_ps1\" ] && break\n",
        "    done\n",
        "    unset _aida_old_ps1 _aida_after _aida_tag\n",
        "fi\n"
    )
    .to_string()
}

/// Shell block `worktree enter` appends to its eval'd payload: strip any
/// stale `(wt:...) ` segments, then splice `(wt:<focus>) ` onto the front of
/// PS1 and record it in `AIDA_WT_PS1_PREFIX`. Idempotent — re-entering never
/// duplicates the segment.
// trace:TASK-1160 | ai:claude
pub fn ps1_wt_splice_block(focus: &str) -> String {
    format!(
        "{strip}if [ -n \"${{PS1+x}}\" ]; then\n\
        \x20   export AIDA_WT_PS1_PREFIX='{prefix}'\n\
        \x20   export PS1=\"$AIDA_WT_PS1_PREFIX$PS1\"\n\
        fi\n",
        strip = ps1_wt_strip_loop(),
        prefix = wt_single_quote(&wt_ps1_prefix(focus)),
    )
}

/// Shell block `worktree exit` emits to remove the indicator: strip every
/// `(wt:...) ` segment from PS1 and drop the recorded prefix marker.
// trace:TASK-1160 | ai:claude
pub fn ps1_wt_strip_block() -> String {
    format!("{}unset AIDA_WT_PS1_PREFIX\n", ps1_wt_strip_loop())
}

/// The full stdout payload for `aida worktree exit`, auto-evaled by the
/// `aida()` wrapper: cd back to the main checkout, unset the session env
/// exports `enter` applied (always `AIDA_SESSION_ID` + `CARGO_TARGET_DIR`,
/// plus any extra names derived from the worktree's `session-env.sh`), and
/// strip the PS1 worktree segment. The session lease itself is untouched.
// trace:TASK-1160 | ai:claude
pub fn exit_shell_payload(main_root: &Path, extra_unsets: &[String]) -> String {
    let mut names: Vec<&str> = vec!["AIDA_SESSION_ID", "CARGO_TARGET_DIR"];
    for n in extra_unsets {
        let n = n.trim();
        if !n.is_empty() && !names.contains(&n) {
            names.push(n);
        }
    }
    format!(
        "cd '{}'\nunset {}\n{}",
        wt_single_quote(&main_root.display().to_string()),
        names.join(" "),
        ps1_wt_strip_block(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_uppercases_canonical_id() {
        assert_eq!(normalize_epic_label("epic-54"), "EPIC-54");
        assert_eq!(normalize_epic_label("EPIC-54"), "EPIC-54");
        assert_eq!(normalize_epic_label("  Epic-54  "), "EPIC-54");
    }

    #[test]
    fn slug_drops_non_alphanumeric_and_lowercases() {
        assert_eq!(epic_slug("EPIC-54"), "epic54");
        assert_eq!(epic_slug("epic-54"), "epic54");
        assert_eq!(epic_slug("STORY-716"), "story716");
    }

    #[test]
    fn default_branch_is_lowercase_dash_work() {
        assert_eq!(default_branch("EPIC-54"), "epic-54-work");
        assert_eq!(default_branch("epic-54"), "epic-54-work");
        assert_eq!(default_branch("STORY-716"), "story-716-work");
    }

    #[test]
    fn default_path_is_sibling_under_ai() {
        let home = Path::new("/home/joe");
        assert_eq!(
            default_worktree_path(home, "EPIC-54"),
            PathBuf::from("/home/joe/ai/aida-epic54")
        );
        assert_eq!(
            default_worktree_path(home, "epic-54"),
            PathBuf::from("/home/joe/ai/aida-epic54")
        );
    }

    #[test]
    fn parse_porcelain_collects_worktree_paths() {
        let porcelain = "\
worktree /home/joe/ai/aida
HEAD abc123
branch refs/heads/main

worktree /home/joe/ai/aida-epic54
HEAD def456
branch refs/heads/epic-54-work
";
        let paths = parse_worktree_paths(porcelain);
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/home/joe/ai/aida"),
                PathBuf::from("/home/joe/ai/aida-epic54"),
            ]
        );
    }

    // trace:TASK-1160 | ai:claude
    #[test]
    fn exit_payload_cds_to_main_root_and_unsets_session_env() {
        let payload = exit_shell_payload(Path::new("/home/joe/ai/aida"), &[]);
        assert!(payload.starts_with("cd '/home/joe/ai/aida'\n"));
        assert!(payload.contains("unset AIDA_SESSION_ID CARGO_TARGET_DIR\n"));
        assert!(payload.contains("unset AIDA_WT_PS1_PREFIX\n"));
    }

    // trace:TASK-1160 | ai:claude
    #[test]
    fn exit_payload_includes_extra_env_names_deduped() {
        let payload = exit_shell_payload(
            Path::new("/home/joe/ai/aida"),
            &[
                "CARGO_TARGET_DIR".to_string(), // already in the base set
                "AIDA_AGENT_TYPE".to_string(),
                "".to_string(), // blank names are dropped
            ],
        );
        assert!(payload.contains("unset AIDA_SESSION_ID CARGO_TARGET_DIR AIDA_AGENT_TYPE\n"));
    }

    // trace:TASK-1160 | ai:claude
    #[test]
    fn exit_payload_escapes_apostrophes_in_main_root() {
        let payload = exit_shell_payload(Path::new("/tmp/o'brien/aida"), &[]);
        assert!(payload.starts_with("cd '/tmp/o'\\''brien/aida'\n"));
    }

    // trace:TASK-1160 | ai:claude
    #[test]
    fn wt_ps1_prefix_shape() {
        assert_eq!(wt_ps1_prefix("BUG-756"), "(wt:BUG-756) ");
        assert_eq!(wt_ps1_prefix("  EPIC-54  "), "(wt:EPIC-54) ");
    }

    /// Drive the emitted shell through a real bash: splice, re-splice (must
    /// not duplicate), then strip — PS1 must round-trip back to the original
    /// and compose with a dev-activate style `(aida-release) ` prefix.
    // trace:TASK-1160 | ai:claude
    #[test]
    fn ps1_splice_then_strip_round_trips_in_bash() {
        let script = format!(
            "PS1='(aida-release) \\u@\\h$ '\n\
             _orig=\"$PS1\"\n\
             {splice}\
             {splice_again}\
             echo \"spliced:$PS1\"\n\
             echo \"marker:${{AIDA_WT_PS1_PREFIX-unset}}\"\n\
             {strip}\
             echo \"stripped:$PS1\"\n\
             echo \"marker2:${{AIDA_WT_PS1_PREFIX-unset}}\"\n\
             [ \"$PS1\" = \"$_orig\" ] && echo roundtrip:ok\n",
            splice = ps1_wt_splice_block("TASK-42"),
            splice_again = ps1_wt_splice_block("TASK-42"),
            strip = ps1_wt_strip_block(),
        );
        let out = std::process::Command::new("bash")
            .arg("-c")
            .arg(&script)
            .output()
            .expect("bash available");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("spliced:(wt:TASK-42) (aida-release) \\u@\\h$ "),
            "one (deduped) wt segment composed with the dev prefix; got:\n{stdout}"
        );
        assert!(stdout.contains("marker:(wt:TASK-42) "));
        assert!(stdout.contains("stripped:(aida-release) \\u@\\h$ "));
        assert!(stdout.contains("marker2:unset"));
        assert!(
            stdout.contains("roundtrip:ok"),
            "PS1 round-trips:\n{stdout}"
        );
    }

    #[test]
    fn is_registered_matches_a_listed_path() {
        let porcelain = "worktree /tmp/does-not-exist-aida-epic54\nHEAD abc\n";
        assert!(is_registered(
            porcelain,
            Path::new("/tmp/does-not-exist-aida-epic54")
        ));
        assert!(!is_registered(
            porcelain,
            Path::new("/tmp/does-not-exist-aida-epic99")
        ));
    }
}
