//! Per-vendor settings `aida init --report-allowlists` tailors, kept behind
//! the vendor boundary so the init report names no vendor itself.
// trace:STORY-2838 | ai:claude
use crate::config_commit::CONFIG_REL_PATH;

/// One `(file, key, entries, scalar)` setting init would write; a scalar
/// setting holds one value rather than a list.
pub(crate) type AllowlistSetting = (&'static str, &'static str, Vec<String>, bool);

/// The settings init would tailor for `agent` (an init agent-selection name)
/// given the detected forge `hosts`. An unknown agent gets none.
pub(crate) fn allowlist_settings(agent: &str, hosts: &[String]) -> Vec<AllowlistSetting> {
    let contained = || {
        (
            CONFIG_REL_PATH,
            "contained.allowed_hosts",
            hosts.to_vec(),
            false,
        )
    };
    match agent {
        "claude" => vec![
            (
                ".claude/settings.json",
                "sandbox.network.allowedDomains",
                hosts.to_vec(),
                false,
            ),
            contained(),
        ],
        "codex" => vec![
            contained(),
            (
                CONFIG_REL_PATH,
                "contained.vendors.codex",
                vec!["native".to_string()],
                true,
            ),
        ],
        "antigravity" => vec![contained()],
        _ => Vec::new(),
    }
}

/// Whether the project's configured default vendor is Codex.
pub(crate) fn project_defaults_to_codex(project_root: &std::path::Path) -> bool {
    aida_core::agents_config::resolve_default_vendor(project_root).as_deref() == Some("codex")
}
