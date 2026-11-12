// ----------------------------------------------------------------------------
// `aida dev` — developer-only commands: pyenv-style activate of an in-repo
// build, foreground supervisor for aida-server + vite, shell-init helpers.
// trace:EPIC-1-001 | ai:claude
// ----------------------------------------------------------------------------

// ----------------------------------------------------------------------------
// `aida help-all` — full command inventory grouped by topic. Includes the
// commands that are #[clap(hide = true)] in the default `aida --help`.
// trace:EPIC-1-001 | ai:claude
// ----------------------------------------------------------------------------

// ----------------------------------------------------------------------------
// `aida role` — persistent personas / hats. State lives at
// <project>/.aida/roles/<name>.toml. Resume by name to restore working
// directory and surface the role in the statusline.
// trace:EPIC-1-001 | ai:claude
// ----------------------------------------------------------------------------

pub(crate) const ACTIVITY_MAX: usize = 10;

/// True if `line` (already trimmed) looks like a TOML `key = value`
/// assignment — a bare-key identifier followed by `=`. The role-file
/// salvage path uses it to tell a real field from injected junk such as
/// the stray `"` a torn write left behind in BUG-228.
/// trace:BUG-228 | ai:claude
pub(crate) fn is_toml_kv_line(line: &str) -> bool {
    match line.split_once('=') {
        Some((key, _)) => {
            let key = key.trim();
            !key.is_empty()
                && key
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        }
        None => false,
    }
}

/// TASK-713: terminal width for laying out the picker. Honors `$COLUMNS`
/// (exported by interactive shells), clamps to a sane floor, and falls back
/// to 80 when unknown. Kept separate so the layout function stays pure.
pub(crate) fn picker_terminal_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|c| c.trim().parse::<usize>().ok())
        .filter(|w| *w >= 20)
        .unwrap_or(80)
}

// ----------------------------------------------------------------------------
// EPIC-20 v1 — scoped session leases.
// trace:EPIC-20 | ai:claude
// ----------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Per-session role activity log (STORY-56)
//
// Project-level role activity (`.aida/roles/<name>.toml`'s `activity` field)
// is shared by every shell that has the role active. When two sessions for
// the same project both run `role:implementer`, they fight over the same
// "current spec" — last writer wins, and `aida statusline`'s @SPEC segment
// flips between specs depending on which session most recently touched
// something. STORY-56 splits that activity stream: while a shell is inside
// a session lease's worktree, role activity goes to a session-local log
// (`.aida/sessions/<id>.activity.toml`) instead. Statusline reads the
// session log when in-session, the project log otherwise. `session end`
// flattens unique (spec_id) entries from the session log back into each
// participating role's project-level activity (newest first, dedupe,
// truncate to ACTIVITY_MAX) so long-term recent-activity views still show
// what was worked on under the closed session.
// trace:STORY-56 | ai:claude
// ---------------------------------------------------------------------------
