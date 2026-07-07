//! The `[agents] vendor` default-vendor knob (STORY-761).
//!
//! Codex-first (or codex-mandated) machines need ONE set-once knob instead of
//! configuring each launch surface separately (`[orchestrator] headless_vendor`,
//! `[tui] vendor`, per-command `--vendor`, …). The knob lives in `agents.toml`
//! alongside the other agent-launch posture keys (`bypass`, `contained`,
//! `default_flags`): user base `~/.aida/agents.toml`, overridable by the
//! project `.aida/agents.toml` — the same precedence as STORY-495's `bypass`.
//!
//! Per-surface config and flags keep priority; this knob only replaces the
//! built-in `claude` fallback at the bottom of each surface's chain:
//!
//! ```text
//! flag > env > per-surface config > [agents] vendor > claude
//! ```
//!
//! Lives in aida-core so both `aida-cli` and `aida-tui` resolve identically.
// trace:STORY-761 | ai:claude

use std::path::Path;

/// Vendor tokens the knob accepts. Surfaces that support fewer vendors (e.g.
/// the TUI hosts only claude/codex tabs) validate again on their side; an
/// unrecognized value here is ignored (fall through to the built-in default)
/// rather than erroring, matching the per-surface knobs' tolerance.
const KNOWN_VENDORS: &[&str] = &["claude", "codex"];

/// Read `[agents] vendor` from one agents.toml file. `None` when the file,
/// table, or key is absent, unparseable, or carries an unrecognized vendor.
fn vendor_from_file(path: &Path) -> Option<String> {
    let body = std::fs::read_to_string(path).ok()?;
    let value: toml::Value = toml::from_str(&body).ok()?;
    let raw = value.get("agents")?.get("vendor")?.as_str()?;
    let token = raw.trim().to_ascii_lowercase();
    KNOWN_VENDORS.contains(&token.as_str()).then_some(token)
}

/// Resolve the default vendor from explicit file paths — the testable core.
/// Project wins over global; absent everywhere is `None` (callers keep their
/// built-in default).
pub fn resolve_default_vendor_from(
    global_agents_toml: Option<&Path>,
    project_agents_toml: Option<&Path>,
) -> Option<String> {
    project_agents_toml
        .and_then(vendor_from_file)
        .or_else(|| global_agents_toml.and_then(vendor_from_file))
}

/// Resolve the default vendor for a project: project `.aida/agents.toml`
/// overrides the user-global `~/.aida/agents.toml`; `None` when neither sets
/// a recognized `[agents] vendor`.
#[cfg(feature = "native")]
pub fn resolve_default_vendor(project_root: &Path) -> Option<String> {
    let global = dirs::home_dir().map(|h| h.join(".aida").join("agents.toml"));
    let project = project_root.join(".aida").join("agents.toml");
    resolve_default_vendor_from(global.as_deref(), Some(&project))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn absent_everywhere_is_none() {
        assert_eq!(resolve_default_vendor_from(None, None), None);
    }

    #[test]
    fn global_sets_codex() {
        let tmp = tempfile::tempdir().unwrap();
        let g = write(tmp.path(), "g.toml", "[agents]\nvendor = \"codex\"\n");
        assert_eq!(
            resolve_default_vendor_from(Some(&g), None).as_deref(),
            Some("codex")
        );
    }

    #[test]
    fn project_overrides_global() {
        let tmp = tempfile::tempdir().unwrap();
        let g = write(tmp.path(), "g.toml", "[agents]\nvendor = \"codex\"\n");
        let p = write(tmp.path(), "p.toml", "[agents]\nvendor = \"claude\"\n");
        assert_eq!(
            resolve_default_vendor_from(Some(&g), Some(&p)).as_deref(),
            Some("claude")
        );
    }

    #[test]
    fn unrecognized_vendor_is_ignored_and_falls_back_to_global() {
        let tmp = tempfile::tempdir().unwrap();
        let g = write(tmp.path(), "g.toml", "[agents]\nvendor = \"codex\"\n");
        let p = write(tmp.path(), "p.toml", "[agents]\nvendor = \"gemini\"\n");
        // Project's unknown token doesn't shadow the recognized global value.
        assert_eq!(
            resolve_default_vendor_from(Some(&g), Some(&p)).as_deref(),
            Some("codex")
        );
    }

    #[test]
    fn case_and_whitespace_tolerant() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write(tmp.path(), "p.toml", "[agents]\nvendor = \" Codex \"\n");
        assert_eq!(
            resolve_default_vendor_from(None, Some(&p)).as_deref(),
            Some("codex")
        );
    }

    #[test]
    fn coexists_with_other_agents_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write(
            tmp.path(),
            "p.toml",
            "[agents]\nbypass = true\nvendor = \"codex\"\n\n[agents.claude]\ndefault_flags = [\"--foo\"]\n",
        );
        assert_eq!(
            resolve_default_vendor_from(None, Some(&p)).as_deref(),
            Some("codex")
        );
    }

    #[test]
    fn missing_file_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        let ghost = tmp.path().join("nope.toml");
        assert_eq!(resolve_default_vendor_from(None, Some(&ghost)), None);
    }
}
