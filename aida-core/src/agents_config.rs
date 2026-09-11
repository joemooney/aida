//! The `[agents] vendor` default-vendor knob (STORY-761).
//!
//! Codex-first (or codex-mandated) machines need ONE set-once knob instead of
//! configuring each launch surface separately (`[orchestrator] headless_vendor`,
//! `[tui] vendor`, per-command `--vendor`, …). The knob lives in `agents.toml`
//! alongside the other agent-launch posture keys (`bypass`, `contained`,
//! `default_flags`): user base `~/.aida/agents.toml`, overridable by the
//! project `.aida/agents.toml` — the same precedence as STORY-495's `bypass`.
//!
//! BUG-704: because `.aida/agents.toml` is gitignored (it carries per-clone
//! permission-posture keys), a fresh drain/pool worktree never sees the
//! project-level knob and headless phases silently fell back to claude. So the
//! knob is ALSO read from the TRACKED, worktree-inherited `.aida/config.toml`
//! `[agents] vendor` — the team-shared project knob. Precedence:
//! project `agents.toml` > project `config.toml` > global `agents.toml`.
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

/// Seat names used when resolving per-agent launch tuning.
///
/// The token is intentionally the aida-core role name so config can use
/// `[agents.<vendor>.seats] implementer = { ... }` without another mapping
/// layer.
// trace:STORY-1033 | ai:codex
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentSeat {
    Implementer,
    Reviewer,
    Advisor,
    Integrator,
    Product,
    Trivial,
}

impl AgentSeat {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentSeat::Implementer => "implementer",
            AgentSeat::Reviewer => "reviewer",
            AgentSeat::Advisor => "advisor",
            AgentSeat::Integrator => "integrator",
            AgentSeat::Product => "product",
            AgentSeat::Trivial => "trivial",
        }
    }
}

/// Resolved launch tuning for a vendor/seat pair.
///
/// Both fields are opaque passthrough strings. `None` means AIDA should omit
/// the native flag and let the vendor CLI choose its default.
// trace:STORY-1033 | ai:codex
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResolvedAgentTuning {
    pub model: Option<String>,
    pub effort: Option<String>,
}

/// Read `[agents] vendor` from one agents.toml file. `None` when the file,
/// table, or key is absent, unparseable, or carries an unrecognized vendor.
fn vendor_from_file(path: &Path) -> Option<String> {
    let body = std::fs::read_to_string(path).ok()?;
    let value: toml::Value = toml::from_str(&body).ok()?;
    let raw = value.get("agents")?.get("vendor")?.as_str()?;
    let token = raw.trim().to_ascii_lowercase();
    KNOWN_VENDORS.contains(&token.as_str()).then_some(token)
}

fn model_from_file(path: &Path, vendor: &str) -> Option<Option<String>> {
    let body = std::fs::read_to_string(path).ok()?;
    let value: toml::Value = toml::from_str(&body).ok()?;
    let raw = value
        .get("agents")?
        .get(vendor.trim().to_ascii_lowercase())?
        .get("model")?
        .as_str()?;
    let model = raw.trim();
    Some((!model.is_empty()).then_some(model.to_string()))
}

fn string_value(table: &toml::Value, key: &str) -> Option<Option<String>> {
    let raw = table.get(key)?.as_str()?;
    let value = raw.trim();
    Some((!value.is_empty()).then_some(value.to_string()))
}

fn tuning_from_file(path: &Path, vendor: &str, seat: AgentSeat) -> Option<ResolvedAgentTuning> {
    let body = std::fs::read_to_string(path).ok()?;
    let value: toml::Value = toml::from_str(&body).ok()?;
    let vendor_table = value
        .get("agents")?
        .get(vendor.trim().to_ascii_lowercase())?;
    let seat_table = vendor_table
        .get("seats")
        .and_then(|seats| seats.get(seat.as_str()));

    let model = seat_table
        .and_then(|table| string_value(table, "model"))
        .or_else(|| string_value(vendor_table, "model"));
    let effort = seat_table.and_then(|table| string_value(table, "effort"));

    if model.is_none() && effort.is_none() {
        return None;
    }

    Some(ResolvedAgentTuning {
        model: model.unwrap_or(None),
        effort: effort.unwrap_or(None),
    })
}

/// Resolve the default vendor from explicit file paths — the testable core.
/// Precedence, highest first:
///   1. project `.aida/agents.toml` — the per-clone personal knob (gitignored,
///      alongside the permission-posture keys, so it is NOT visible to a fresh
///      drain/pool worktree);
///   2. project `.aida/config.toml` — the TRACKED, worktree-inherited team knob
///      (BUG-704: this is the one a headless drain phase running in a pool
///      worktree can actually see; without it the project knob silently missed
///      worktrees and phases fell back to claude);
///   3. global `~/.aida/agents.toml` — the machine-wide default.
/// Absent everywhere is `None` (callers keep their built-in default).
// trace:BUG-704 | ai:claude
pub fn resolve_default_vendor_from(
    global_agents_toml: Option<&Path>,
    project_config_toml: Option<&Path>,
    project_agents_toml: Option<&Path>,
) -> Option<String> {
    project_agents_toml
        .and_then(vendor_from_file)
        .or_else(|| project_config_toml.and_then(vendor_from_file))
        .or_else(|| global_agents_toml.and_then(vendor_from_file))
}

/// Resolve `[agents.<vendor>] model` as an opaque passthrough string.
///
/// Precedence mirrors the default-vendor chain:
/// project `.aida/agents.toml` > project `.aida/config.toml` > global
/// `~/.aida/agents.toml`. Empty strings are treated as unset so the native
/// vendor default remains unchanged. AIDA never validates model names.
// trace:STORY-1003 | ai:codex
pub fn resolve_vendor_model_from(
    global_agents_toml: Option<&Path>,
    project_config_toml: Option<&Path>,
    project_agents_toml: Option<&Path>,
    vendor: &str,
) -> Option<String> {
    let vendor = vendor.trim().to_ascii_lowercase();
    if vendor.is_empty() {
        return None;
    }
    if let Some(model) = project_agents_toml.and_then(|p| model_from_file(p, &vendor)) {
        return model;
    }
    if let Some(model) = project_config_toml.and_then(|p| model_from_file(p, &vendor)) {
        return model;
    }
    if let Some(model) = global_agents_toml.and_then(|p| model_from_file(p, &vendor)) {
        return model;
    }
    None
}

/// Resolve `[agents.<vendor>]` launch tuning for a specific seat.
///
/// Resolution order:
///   1. project `.aida/agents.toml`
///   2. project `.aida/config.toml`
///   3. global `~/.aida/agents.toml`
///
/// Inside a selected file, seat values override the vendor default model:
/// `[agents.<vendor>.seats].<seat>.model` > `[agents.<vendor>].model`.
/// Effort is seat-scoped only. An explicitly empty string shadows lower files
/// and selects the vendor default for that field.
// trace:STORY-1033 | ai:codex
pub fn resolve_agent_tuning_from(
    global_agents_toml: Option<&Path>,
    project_config_toml: Option<&Path>,
    project_agents_toml: Option<&Path>,
    vendor: &str,
    seat: AgentSeat,
) -> ResolvedAgentTuning {
    let vendor = vendor.trim().to_ascii_lowercase();
    if vendor.is_empty() {
        return ResolvedAgentTuning::default();
    }
    if let Some(tuning) = project_agents_toml.and_then(|p| tuning_from_file(p, &vendor, seat)) {
        return tuning;
    }
    if let Some(tuning) = project_config_toml.and_then(|p| tuning_from_file(p, &vendor, seat)) {
        return tuning;
    }
    if let Some(tuning) = global_agents_toml.and_then(|p| tuning_from_file(p, &vendor, seat)) {
        return tuning;
    }
    ResolvedAgentTuning::default()
}

/// Resolve the default vendor for a project: project `.aida/agents.toml`
/// overrides the user-global `~/.aida/agents.toml`; `None` when neither sets
/// a recognized `[agents] vendor`.
#[cfg(feature = "native")]
pub fn resolve_default_vendor(project_root: &Path) -> Option<String> {
    let global = dirs::home_dir().map(|h| h.join(".aida").join("agents.toml"));
    let project_config = project_root.join(".aida").join("config.toml");
    let project_agents = project_root.join(".aida").join("agents.toml");
    resolve_default_vendor_from(
        global.as_deref(),
        Some(&project_config),
        Some(&project_agents),
    )
}

/// Resolve the model configured for a vendor in this project.
// trace:STORY-1003 | ai:codex
#[cfg(feature = "native")]
pub fn resolve_vendor_model(project_root: &Path, vendor: &str) -> Option<String> {
    let global = dirs::home_dir().map(|h| h.join(".aida").join("agents.toml"));
    let project_config = project_root.join(".aida").join("config.toml");
    let project_agents = project_root.join(".aida").join("agents.toml");
    resolve_vendor_model_from(
        global.as_deref(),
        Some(&project_config),
        Some(&project_agents),
        vendor,
    )
}

/// Resolve the model/effort configured for a vendor + seat in this project.
// trace:STORY-1033 | ai:codex
#[cfg(feature = "native")]
pub fn resolve_agent_tuning(
    project_root: &Path,
    vendor: &str,
    seat: AgentSeat,
) -> ResolvedAgentTuning {
    let global = dirs::home_dir().map(|h| h.join(".aida").join("agents.toml"));
    let project_config = project_root.join(".aida").join("config.toml");
    let project_agents = project_root.join(".aida").join("agents.toml");
    resolve_agent_tuning_from(
        global.as_deref(),
        Some(&project_config),
        Some(&project_agents),
        vendor,
        seat,
    )
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
        assert_eq!(resolve_default_vendor_from(None, None, None), None);
    }

    #[test]
    fn global_sets_codex() {
        let tmp = tempfile::tempdir().unwrap();
        let g = write(tmp.path(), "g.toml", "[agents]\nvendor = \"codex\"\n");
        assert_eq!(
            resolve_default_vendor_from(Some(&g), None, None).as_deref(),
            Some("codex")
        );
    }

    #[test]
    fn project_overrides_global() {
        let tmp = tempfile::tempdir().unwrap();
        let g = write(tmp.path(), "g.toml", "[agents]\nvendor = \"codex\"\n");
        let p = write(tmp.path(), "p.toml", "[agents]\nvendor = \"claude\"\n");
        assert_eq!(
            resolve_default_vendor_from(Some(&g), None, Some(&p)).as_deref(),
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
            resolve_default_vendor_from(Some(&g), None, Some(&p)).as_deref(),
            Some("codex")
        );
    }

    #[test]
    fn case_and_whitespace_tolerant() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write(tmp.path(), "p.toml", "[agents]\nvendor = \" Codex \"\n");
        assert_eq!(
            resolve_default_vendor_from(None, None, Some(&p)).as_deref(),
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
            resolve_default_vendor_from(None, None, Some(&p)).as_deref(),
            Some("codex")
        );
    }

    #[test]
    fn missing_file_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        let ghost = tmp.path().join("nope.toml");
        assert_eq!(resolve_default_vendor_from(None, None, Some(&ghost)), None);
    }

    // BUG-704: the tracked project config.toml supplies the knob when the
    // gitignored agents.toml is absent — the exact state of a fresh drain/pool
    // worktree, where the silent claude-fallback used to happen.
    #[test]
    fn tracked_config_toml_supplies_knob_when_agents_toml_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = write(tmp.path(), "config.toml", "[agents]\nvendor = \"codex\"\n");
        let absent_agents = tmp.path().join("agents.toml"); // never created
        assert_eq!(
            resolve_default_vendor_from(None, Some(&cfg), Some(&absent_agents)).as_deref(),
            Some("codex"),
            "a worktree with only the tracked config.toml must still resolve the knob"
        );
    }

    // Full precedence: per-clone agents.toml beats tracked config.toml beats global.
    #[test]
    fn precedence_agents_over_config_over_global() {
        let tmp = tempfile::tempdir().unwrap();
        let g = write(tmp.path(), "g.toml", "[agents]\nvendor = \"claude\"\n");
        let cfg = write(tmp.path(), "config.toml", "[agents]\nvendor = \"codex\"\n");
        // config beats global.
        assert_eq!(
            resolve_default_vendor_from(Some(&g), Some(&cfg), None).as_deref(),
            Some("codex")
        );
        // per-clone agents.toml beats config.
        let ag = write(tmp.path(), "agents.toml", "[agents]\nvendor = \"claude\"\n");
        assert_eq!(
            resolve_default_vendor_from(Some(&g), Some(&cfg), Some(&ag)).as_deref(),
            Some("claude")
        );
    }

    #[test]
    fn model_resolves_per_vendor_as_opaque_value() {
        let tmp = tempfile::tempdir().unwrap();
        let g = write(
            tmp.path(),
            "g.toml",
            "[agents.codex]\nmodel = \"global-codex\"\n",
        );
        let cfg = write(
            tmp.path(),
            "config.toml",
            "[agents.codex]\nmodel = \"team/codex-prod\"\n",
        );
        let ag = write(
            tmp.path(),
            "agents.toml",
            "[agents.codex]\nmodel = \"local alias\"\n[agents.claude]\nmodel = \"opus\"\n",
        );
        assert_eq!(
            resolve_vendor_model_from(Some(&g), Some(&cfg), Some(&ag), "codex").as_deref(),
            Some("local alias")
        );
        assert_eq!(
            resolve_vendor_model_from(Some(&g), Some(&cfg), Some(&ag), "claude").as_deref(),
            Some("opus")
        );
    }

    #[test]
    fn model_empty_string_selects_vendor_default() {
        let tmp = tempfile::tempdir().unwrap();
        let g = write(
            tmp.path(),
            "g.toml",
            "[agents.codex]\nmodel = \"global-codex\"\n",
        );
        let p = write(tmp.path(), "p.toml", "[agents.codex]\nmodel = \"\"\n");
        assert_eq!(
            resolve_vendor_model_from(Some(&g), None, Some(&p), "codex"),
            None
        );
    }

    #[test]
    fn seat_tuning_prefers_seat_then_vendor_default() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write(
            tmp.path(),
            "agents.toml",
            r#"[agents.claude]
model = "opus"

[agents.claude.seats]
implementer = { model = "sonnet", effort = "medium" }
reviewer = { effort = "high" }
"#,
        );

        assert_eq!(
            resolve_agent_tuning_from(None, None, Some(&p), "claude", AgentSeat::Implementer),
            ResolvedAgentTuning {
                model: Some("sonnet".to_string()),
                effort: Some("medium".to_string()),
            }
        );
        assert_eq!(
            resolve_agent_tuning_from(None, None, Some(&p), "claude", AgentSeat::Reviewer),
            ResolvedAgentTuning {
                model: Some("opus".to_string()),
                effort: Some("high".to_string()),
            }
        );
    }

    #[test]
    fn seat_tuning_file_precedence_and_empty_shadowing() {
        let tmp = tempfile::tempdir().unwrap();
        let g = write(
            tmp.path(),
            "g.toml",
            "[agents.codex]\nmodel = \"global\"\n[agents.codex.seats]\nimplementer = { effort = \"high\" }\n",
        );
        let cfg = write(
            tmp.path(),
            "config.toml",
            "[agents.codex]\nmodel = \"team\"\n[agents.codex.seats]\nimplementer = { model = \"\", effort = \"\" }\n",
        );
        assert_eq!(
            resolve_agent_tuning_from(Some(&g), Some(&cfg), None, "codex", AgentSeat::Implementer),
            ResolvedAgentTuning {
                model: None,
                effort: None,
            }
        );
    }
}
