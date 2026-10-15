//! Statusline helper logic (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

pub(crate) fn statusline_project_root() -> std::path::PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| ".".into());
    statusline_project_root_from_with_roots(&cwd, &aida_core::store_locate::real_temp_roots())
}

/// [`statusline_project_root`], parameterized on the starting directory and
/// the temp roots to guard against.
///
/// BUG-1598: `aida role` / `aida statusline` (this function's callers —
/// `handle_role_command`, `statusline_cmd.rs`, and the init tail:
/// `scaffold_starter_roles`, `refresh_agent_packs`,
/// `register_project_in_global_registry`) must not adopt a stray
/// `.aida/config.toml` sitting directly in a temp root when run from a
/// `mktemp -d`-rooted cwd. The guard breaks the walk-up at a temp root and
/// falls through to the SAME `cwd` fallback already used when no marker is
/// found at all — a temp root is treated exactly like "nothing found up
/// there", never a special error. Factored out as `_with_roots` so a test
/// can exercise the guard against a fake root without mutating `TMPDIR` or
/// touching the real, shared system temp dir.
// trace:BUG-1598 | ai:claude
pub(crate) fn statusline_project_root_from_with_roots(
    cwd: &std::path::Path,
    temp_roots: &[std::path::PathBuf],
) -> std::path::PathBuf {
    // Roles + statusline live in the project that is the user's CWD
    // (or any ancestor with `.aida/config.toml`). Falls back to CWD if
    // no marker is found — including when the walk-up hits a temp root.
    // Canonicalize the root set ONCE, before the loop — not on every
    // ancestor level.
    let canonical_roots = aida_core::store_locate::canonicalize_roots(temp_roots);
    let mut probe = cwd.to_path_buf();
    for _ in 0..8 {
        if aida_core::store_locate::is_in_canonical_roots(&probe, &canonical_roots) {
            break;
        }
        if probe.join(".aida").join("config.toml").exists() {
            return probe;
        }
        match probe.parent() {
            Some(p) => probe = p.to_path_buf(),
            None => break,
        }
    }
    cwd.to_path_buf()
}

/// TASK-244: read `[statusline] role_mismatch_warning` from
/// `.aida/config.toml`. Defaults to `true` (warn on mismatch) when the
/// key, section, or file is absent. trace:TASK-244 | ai:claude
pub(crate) fn statusline_role_mismatch_enabled(project_dir: &std::path::Path) -> bool {
    let Ok(content) = std::fs::read_to_string(project_dir.join(".aida").join("config.toml")) else {
        return true;
    };
    let mut in_section = false;
    for raw in content.lines() {
        let line = strip_toml_inline_comment(raw).trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            in_section = rest.trim_end_matches(']').trim() == "statusline";
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some(rest) = line.strip_prefix("role_mismatch_warning") {
            if let Some(val) = rest.split('=').nth(1) {
                let v = val
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_ascii_lowercase();
                return !matches!(v.as_str(), "false" | "0" | "no" | "off");
            }
        }
    }
    true
}

/// Read `[statusline] base_freshness_check` (on/off) and
/// `threshold_warn` (commits behind `origin/main` at/above which the
/// statusline surfaces the `base behind by N` indicator) from
/// `.aida/config.toml`. Defaults to enabled + [`BASE_BEHIND_STATUSLINE_THRESHOLD`]
/// when the keys, section, or file are absent — so the feature is on by
/// default and a project can dial it down or off without a code change.
/// Pure over the file contents; best-effort (any parse miss keeps the
/// default).
// trace:TASK-101 | ai:claude
pub(crate) fn statusline_base_freshness_config(project_dir: &std::path::Path) -> (bool, u32) {
    let default = (true, BASE_BEHIND_STATUSLINE_THRESHOLD);
    let Ok(content) = std::fs::read_to_string(project_dir.join(".aida").join("config.toml")) else {
        return default;
    };
    let (mut enabled, mut threshold) = default;
    let mut in_section = false;
    for raw in content.lines() {
        let line = strip_toml_inline_comment(raw).trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            in_section = rest.trim_end_matches(']').trim() == "statusline";
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some(rest) = line.strip_prefix("base_freshness_check") {
            if let Some(val) = rest.split('=').nth(1) {
                let v = val
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_ascii_lowercase();
                enabled = !matches!(v.as_str(), "false" | "0" | "no" | "off");
            }
        } else if let Some(rest) = line.strip_prefix("threshold_warn") {
            if let Some(val) = rest.split('=').nth(1) {
                if let Ok(n) = val
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .parse::<u32>()
                {
                    threshold = n.max(1);
                }
            }
        }
    }
    (enabled, threshold)
}

/// they disagree — and the warning is enabled — both are shown with a
/// warning glyph so three-way role confusion (shell vs session vs resumed
/// conversation) is visible at a glance. Returns `(text, is_mismatch)`
/// so the caller picks the colour. Pure — unit-tested independent of
/// the statusline IO. trace:TASK-244 | ai:claude
pub(crate) fn role_segment_text(
    shell_role: &str,
    session_role: Option<&str>,
    warn_enabled: bool,
) -> (String, bool) {
    // BUG-519: `general-purpose` is the harness's generic fallback agent_type
    // (worktree_lease.rs), written onto the lease `role` for a subagent that
    // carries no AIDA role. When the operator deliberately started a
    // general-purpose session, warn-glyphing it as a role mismatch reads as
    // alarmist noise — it isn't a misrouted reviewer/implementer, it's the
    // intended generic seat. Only warn when the session role names a real,
    // scoped AIDA role that disagrees with the shell. trace:BUG-519
    let mismatch = warn_enabled
        && session_role
            .map(|s| {
                !s.eq_ignore_ascii_case(shell_role) && !s.eq_ignore_ascii_case("general-purpose")
            })
            .unwrap_or(false);
    if mismatch {
        (
            format!(
                "role:{} {} session:{}",
                shell_role,
                crate::glyph(crate::glyphs::Glyph::Warning),
                session_role.unwrap()
            ),
            true,
        )
    } else {
        (format!("role:{}", shell_role), false)
    }
}

/// TASK-306: the orchestrator-context badge for the statusline. Built for a
/// corroborated `--auto-complete` phase session; the caller colors the
/// fields. `phase` is the 1-based phase index (`AIDA_AUTO_COMPLETE_PHASE`),
/// `no_human_mode` the `--no-human` scope slug (`AIDA_NO_HUMAN_MODE`).
/// trace:TASK-306 | ai:claude
pub(crate) struct OrchestratorBadge {
    /// `auto:N/6 <phase-name>` — the phase indicator. `auto:?/6` when the
    /// phase env var is missing or unparseable (defensive — the orchestrator
    /// always sets it on the children it spawns).
    pub(crate) phase: String,
    /// `no-human:<mode>` — present only when `--no-human` is in effect.
    pub(crate) no_human: Option<String>,
    /// `pause-here` — the cue that the user is expected to act in this phase.
    /// A statusline only renders for an interactive session, so an
    /// orchestrated badge always carries it.
    pub(crate) pause: &'static str,
}
