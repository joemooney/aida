//! Closed, model-free tracer. Nothing here can launch a vendor or stored command.
//! The filesystem/supervisor primitives are the intended shared backend; they
//! confer no applied-containment receipt until the later egress/grant slices.
// trace:TASK-2705 | ai:codex
mod filesystem;
mod supervisor;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProbeStatus {
    Ready,
    Unsupported,
    Refused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum ConfiguredPosture {
    Aida,
    Native,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum ProbeStage {
    Capability,
    TrustedPolicy,
    FilesystemSupervisor,
    Complete,
}

/// Whether the host lets the fixed lane build its nested isolation layer.
/// `Unknown` until the filesystem/supervisor stage runs. trace:TASK-2705 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IsolationState {
    Unknown,
    Available,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IsolationReason {
    HostPolicyDenied,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Isolation {
    pub state: IsolationState,
    pub reason: Option<IsolationReason>,
    pub phase: Option<String>,
    pub errno: Option<i32>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum NativeAdapter {
    None,
    Claude,
    Codex,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum IpcLayout {
    PrivateRuntime,
    PrivateDaemon,
    Unverified,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum CredentialLayout {
    Unverified,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Capability {
    native: NativeAdapter,
    ipc: IpcLayout,
    credentials: CredentialLayout,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Capabilities {
    version: u32,
    vendor: BTreeMap<String, Capability>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ProbeReport {
    version: u32,
    vendor: String,
    pub status: ProbeStatus,
    pub cause: String,
    policy_sha: Option<String>,
    configured_posture: Option<ConfiguredPosture>,
    stage: ProbeStage,
    capability: Option<Capability>,
    pub isolation: Isolation,
    // Always false, including a successful synthetic tracer. No runtime stamp
    // is accepted as authority for these fields.
    aida_containment_applied: bool,
    ordinary_launch_supported: bool,
    remaining_protections: [&'static str; 4],
    checks: Vec<String>,
}

/// Read one pinned policy snapshot; branch-local config cannot select/widen it.
/// Policy is reported, never executed. The probe itself uses only built-in argv.
// trace:TASK-2705 | ai:codex
pub(crate) fn probe(root: &Path, vendor: &str) -> ProbeReport {
    let mut report = ProbeReport {
        version: 1,
        vendor: vendor.into(),
        status: ProbeStatus::Refused,
        cause: String::new(),
        policy_sha: None,
        configured_posture: None,
        stage: ProbeStage::Capability,
        capability: None,
        isolation: Isolation {
            state: IsolationState::Unknown,
            reason: None,
            phase: None,
            errno: None,
        },
        aida_containment_applied: false,
        ordinary_launch_supported: false,
        remaining_protections: [
            "egress_and_local_ipc",
            "delegated_grant",
            "native_composition",
            "vendor_lifecycle",
        ],
        checks: Vec::new(),
    };
    let mut run = || -> anyhow::Result<supervisor::TracerOutcome> {
        let mut capabilities: Capabilities = toml::from_str(include_str!("vendors.toml"))?;
        anyhow::ensure!(capabilities.version == 1, "unsupported capability schema");
        let capability = capabilities
            .vendor
            .remove(vendor)
            .ok_or_else(|| anyhow::anyhow!("unsupported vendor capability"))?;
        report.capability = Some(capability);
        report.stage = ProbeStage::TrustedPolicy;
        let sha = crate::trusted_config::local_trusted_default_branch_sha(root)
            .ok_or_else(|| anyhow::anyhow!("trusted default-branch policy unavailable"))?;
        report.policy_sha = Some(sha.clone());
        let body = crate::trusted_config::read_config_at_sha(root, &sha)
            .ok_or_else(|| anyhow::anyhow!("trusted policy unreadable at pinned commit"))?;
        let cfg: toml::Value = toml::from_str(&body)
            .map_err(|_| anyhow::anyhow!("trusted containment policy is malformed TOML"))?;
        let contained = cfg
            .get("contained")
            .map(|v| {
                v.as_table()
                    .ok_or_else(|| anyhow::anyhow!("trusted contained policy must be a table"))
            })
            .transpose()?;
        let posture = contained
            .and_then(|c| c.get("vendors"))
            .map(|v| {
                v.as_table()
                    .ok_or_else(|| anyhow::anyhow!("trusted vendor policy must be a table"))
            })
            .transpose()?
            .and_then(|v| v.get(vendor))
            .map(|v| match v.as_str() {
                Some("native") => Ok(ConfiguredPosture::Native),
                Some("aida") => Ok(ConfiguredPosture::Aida),
                _ => anyhow::bail!("trusted vendor policy must be aida or native"),
            })
            .transpose()?;
        report.configured_posture = posture;
        report.stage = ProbeStage::FilesystemSupervisor;
        supervisor::run_probe()
    };
    match run() {
        Ok(supervisor::TracerOutcome::HostPolicyDenied { phase, errno }) => {
            // Typed host property, not a tracer failure and never a pass.
            report.cause = format!(
                "isolation unavailable: host policy denied nested user namespace \
                 (phase {phase}, errno {errno}); no fallback"
            );
            report.isolation = Isolation {
                state: IsolationState::Unavailable,
                reason: Some(IsolationReason::HostPolicyDenied),
                phase: Some(phase),
                errno: Some(errno),
            };
        }
        Ok(supervisor::TracerOutcome::Passed(checks)) => {
            report.isolation.state = IsolationState::Available;
            report.checks = checks;
            report.status = ProbeStatus::Ready;
            report.stage = ProbeStage::Complete;
            report.cause =
                "fixed filesystem/supervisor tracer passed; full backend remains unsupported"
                    .into();
        }
        Err(error) => {
            report.cause = format!("{error:#}");
            if report.cause == "unsupported vendor capability" {
                report.status = ProbeStatus::Unsupported;
            }
        }
    }
    report
}

#[cfg(test)]
mod tests;
