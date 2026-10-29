//! BUG-014: a contained Claude reviewer in a repository without the egress
//! allowlist or the reviewer allow entries cannot reach the forge or run
//! `aida`, so it exits with no verdict and only a terse denial count. The
//! launcher checks both before it mints a lease or a worktree and refuses,
//! naming what is missing and the command that adds it.
//! trace:BUG-b6b1278890774ef993d2ebc327862470-014 | ai:claude

use anyhow::Result;
use std::path::Path;

/// The [`aida_core::scaffolding::REVIEWER_ALLOW_ENTRIES`] absent from the
/// repository's committed `.claude/settings.json` `permissions.allow` — the
/// file a review worktree checks out. A missing or unparseable file lacks
/// them all.
pub(crate) fn missing_reviewer_allow_entries(project_root: &Path) -> Vec<&'static str> {
    let allow: Vec<String> =
        std::fs::read_to_string(project_root.join(".claude").join("settings.json"))
            .ok()
            .and_then(|body| serde_json::from_str::<serde_json::Value>(&body).ok())
            .and_then(|v| v.pointer("/permissions/allow")?.as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|e| e.as_str().map(str::to_string))
            .collect();
    aida_core::scaffolding::REVIEWER_ALLOW_ENTRIES
        .iter()
        .copied()
        .filter(|entry| !allow.iter().any(|a| a == entry))
        .collect()
}

/// Refuse a contained reviewer launch the repository's config cannot carry.
pub(crate) fn preflight_contained_reviewer(project_root: &Path) -> Result<()> {
    let mut missing = Vec::new();
    if crate::session::contained_allowed_hosts(project_root).is_empty() {
        missing.push(
            "the contained egress allowlist (`[contained] allowed_hosts` in .aida/config.toml)"
                .to_string(),
        );
    }
    let absent = missing_reviewer_allow_entries(project_root);
    if !absent.is_empty() {
        missing.push(format!(
            "the reviewer allow entries {} (`permissions.allow` in .claude/settings.json)",
            absent
                .iter()
                .map(|e| format!("`{e}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if missing.is_empty() {
        return Ok(());
    }
    anyhow::bail!(
        "refusing to launch a contained reviewer: this repository lacks {}. Without it the \
         reviewer cannot reach the forge or run `aida`, and exits with no verdict. Run \
         `aida scaffold upgrade` to add it, commit .aida/config.toml and .claude/settings.json, \
         then re-run.",
        missing.join(" and ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALLOW: &str = r#"{"permissions": {"allow": ["Bash(aida)", "Bash(aida *)"]}}"#;
    const HOSTS: &str = "[contained]\nallowed_hosts = [\"api.github.com\"]\n";

    fn repo(config: &str, settings: Option<&str>) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
        std::fs::write(dir.path().join(".aida/config.toml"), config).unwrap();
        if let Some(body) = settings {
            std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
            std::fs::write(dir.path().join(".claude/settings.json"), body).unwrap();
        }
        dir
    }

    #[test]
    fn bug_014_preflight_passes_a_configured_repo() {
        let dir = repo(HOSTS, Some(ALLOW));
        preflight_contained_reviewer(dir.path()).unwrap();
    }

    #[test]
    fn bug_014_preflight_refuses_without_the_egress_allowlist() {
        let dir = repo("[forge]\n", Some(ALLOW));
        let err = preflight_contained_reviewer(dir.path())
            .unwrap_err()
            .to_string();
        assert!(err.contains("[contained] allowed_hosts"), "{err}");
        assert!(!err.contains("permissions.allow"), "{err}");
        assert!(err.contains("aida scaffold upgrade"), "{err}");
    }

    #[test]
    fn bug_014_preflight_refuses_without_the_reviewer_allow_entries() {
        let dir = repo(HOSTS, Some(r#"{"permissions": {"allow": ["Bash(aida)"]}}"#));
        let err = preflight_contained_reviewer(dir.path())
            .unwrap_err()
            .to_string();
        assert!(err.contains("`Bash(aida *)`"), "{err}");
        assert!(!err.contains("`Bash(aida)`"), "{err}");
        assert!(!err.contains("allowed_hosts"), "{err}");

        let bare = repo(HOSTS, None);
        let err = preflight_contained_reviewer(bare.path())
            .unwrap_err()
            .to_string();
        assert!(err.contains("permissions.allow"), "{err}");
    }

    /// A fresh scaffold passes: the init config section and the template
    /// master's allow entries (which the generated settings carry, see the
    /// core scaffold test) hold what the preflight requires.
    #[test]
    fn bug_014_init_scaffold_satisfies_the_preflight() {
        let settings = serde_json::json!({
            "permissions": {"allow": aida_core::scaffolding::template_permissions_allow()}
        })
        .to_string();
        let dir = repo(&crate::init_contained_config_section(), Some(&settings));
        preflight_contained_reviewer(dir.path()).unwrap();
    }

    /// `aida scaffold upgrade`'s config step adds the template hosts to a
    /// config without them, and leaves a project's own allowlist alone.
    #[test]
    fn bug_014_upgrade_adds_hosts_only_when_absent() {
        let dir = repo("[forge]\nprovider = \"github\"\n", Some(ALLOW));
        assert!(crate::ensure_contained_allowed_hosts(dir.path(), true).unwrap());
        assert!(
            preflight_contained_reviewer(dir.path()).is_err(),
            "dry run writes nothing"
        );
        assert!(crate::ensure_contained_allowed_hosts(dir.path(), false).unwrap());
        preflight_contained_reviewer(dir.path()).unwrap();
        assert_eq!(
            crate::session::contained_allowed_hosts(dir.path()),
            aida_core::scaffolding::template_contained_allowed_hosts()
        );

        let own = repo(HOSTS, Some(ALLOW));
        assert!(!crate::ensure_contained_allowed_hosts(own.path(), false).unwrap());
        assert_eq!(
            crate::session::contained_allowed_hosts(own.path()),
            vec!["api.github.com".to_string()]
        );
    }
}
