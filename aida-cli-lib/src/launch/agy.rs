//! AGY adapter data for the shared AIDA boundary, pending all-tool proof.
//! No separate enforcement engine and no native-containment credit.
// trace:TASK-3048 | ai:codex
use crate::session::ContainmentPosture;
use anyhow::Result;
use std::path::Path;

/// Requirements for the shared launcher's future AGY arm. These are deliberately
/// not applied by the Codex arm: its raw-store grants and API-key auth differ.
/// The incomplete auth inventory cannot become an egress allow list.
#[allow(dead_code)] // Adapter stays unlaunchable until independently measured.
pub(crate) struct Adapter {
    pub posture: ContainmentPosture,
    pub runtime_relative: &'static str,
    pub credential_relative: &'static str,
    pub observed_hosts: &'static [&'static str],
    pub approved_auth_hosts: &'static [&'static str],
    pub auth_inventory_complete: bool,
    pub writable_classes: &'static [&'static str],
    pub raw_git_writes: bool,
    pub raw_store_writes: bool,
    pub mediated_writes: &'static [&'static str],
    pub disabled_unless_confined: &'static [&'static str],
    pub observed_mode: &'static str,
    pub argv_prefix: &'static [&'static str],
}

pub(crate) const AGY: Adapter = Adapter {
    posture: ContainmentPosture::Aida,
    runtime_relative: ".gemini/antigravity-cli",
    credential_relative: "antigravity-oauth-token",
    observed_hosts: &["daily-cloudcode-pa.googleapis.com"],
    approved_auth_hosts: &[],
    auth_inventory_complete: false,
    writable_classes: &["workspace", "cargo-target", "scratch", "private-runtime"],
    raw_git_writes: false,
    raw_store_writes: false,
    mediated_writes: &["aida-cli-host-boundary", "deputy", "host-commit"],
    disabled_unless_confined: &["browser", "url", "mcp", "delegated-tools"],
    // Host evidence: auto unavailable; the CLI continues in default mode.
    // This mode has no all-tool conformance credit. trace:TASK-3048 | ai:codex
    observed_mode: "default (auto unavailable)",
    argv_prefix: &[
        "--sandbox",
        "--output-format",
        "stream-json",
        "--disable-slash-commands",
    ],
};

impl Adapter {
    /// No config value, synthetic receipt, or observed hostname can unlock this
    /// adapter. Removing this gate requires fresh integrated all-tool evidence,
    /// including the auth inventory and mediated-write positive controls.
    pub(crate) fn require_conformance(&self) -> Result<()> {
        anyhow::bail!(
            "agy shared AIDA boundary is pending fresh all-tool conformance; \
             raw Git/store writes are denied and require mediated host operations; \
             auth/refresh host inventory is incomplete; refusing to launch"
        )
    }

    /// Candidate inputs only; never a spawn path. Runtime/settings must live in
    /// launch-owned private state, with credentials mounted read-only by handle.
    /// This table does not inherit ~/.gemini, plugins, MCP, or daemon sockets.
    #[allow(dead_code)] // Used once the integrated adapter earns admission.
    pub(crate) fn candidate_argv(
        &self,
        private_runtime: &Path,
        prompt: &str,
    ) -> Result<Vec<String>> {
        anyhow::ensure!(
            crate::contained_boundary::root_rule_path(private_runtime).is_some(),
            "AGY requires an exact absolute private runtime path"
        );
        let mut args: Vec<String> = self.argv_prefix.iter().map(|s| (*s).into()).collect();
        args.extend([
            "--settings".into(),
            private_runtime
                .join("settings.json")
                .to_string_lossy()
                .into(),
            "-p".into(),
            prompt.into(),
        ]);
        Ok(args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agy_adapter_keeps_unmeasured_policy_closed() {
        assert_eq!(AGY.posture, ContainmentPosture::Aida);
        assert!(!AGY.auth_inventory_complete);
        assert!(AGY.approved_auth_hosts.is_empty());
        assert_eq!(AGY.observed_hosts, &["daily-cloudcode-pa.googleapis.com"]);
        assert!(!AGY.raw_git_writes && !AGY.raw_store_writes);
        assert_eq!(
            AGY.writable_classes,
            &["workspace", "cargo-target", "scratch", "private-runtime"]
        );
        assert_eq!(AGY.mediated_writes.len(), 3);
        assert_eq!(
            AGY.disabled_unless_confined,
            &["browser", "url", "mcp", "delegated-tools"]
        );
        assert!(AGY
            .require_conformance()
            .unwrap_err()
            .to_string()
            .contains("fresh all-tool conformance"));
    }

    #[test]
    fn agy_adapter_candidate_argv_has_final_prompt_and_no_bypass() {
        let runtime = Path::new("/private/agy-runtime");
        let prompt = "--dangerously-skip-permissions is prompt text";
        let args = AGY.candidate_argv(runtime, prompt).unwrap();
        assert_eq!(&args[args.len() - 2..], &["-p", prompt]);
        assert!(!args.iter().any(|s| s == "--dangerously-skip-permissions"));
        assert!(args
            .windows(2)
            .any(|a| a == ["--settings", "/private/agy-runtime/settings.json"]));
        assert!(!args.iter().any(|s| s == "--mode" || s == "auto"));
        assert_eq!(AGY.observed_mode, "default (auto unavailable)");
        assert_eq!(AGY.runtime_relative, ".gemini/antigravity-cli");
        assert_eq!(AGY.credential_relative, "antigravity-oauth-token");
        assert!(AGY.candidate_argv(Path::new("/"), prompt).is_err());
    }
}
