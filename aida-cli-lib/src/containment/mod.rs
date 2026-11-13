//! Fixed model-free diagnostics and the separate headless launch owner.
//! Diagnostic success never grants applied credit. Only `launch` validates a
//! production receipt before releasing the contained vendor exec barrier.
// trace:TASK-2705 | ai:codex
mod filesystem;
// TASK-2708 / STORY-2326: the contained launcher lives in `launch/`; this
// alias keeps `containment::launch::…` paths. trace:TASK-2708 | ai:claude
pub(crate) use crate::launch::contained as launch;
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
    EgressLocalIpc,
    Complete,
}

/// Whether the host lets the fixed lane build its nested isolation layer.
/// `Unknown` until the nested user-namespace preflight completes (an earlier
/// refusal, or a preflight that itself fails, leaves it unknown). `Available`
/// once the preflight passes, even if the full lane later fails or times out:
/// that failure is the tracer's, reported in `cause`. trace:TASK-2705 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IsolationState {
    Unknown,
    Available,
    Unavailable,
}

/// Typed host property behind `IsolationState::Unavailable`. Only nested
/// `unshare(CLONE_NEWUSER)` failures are classified; anything else is an
/// ordinary refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IsolationReason {
    /// EPERM/EACCES: host policy (e.g. AppArmor `unpriv_bwrap`) denies it.
    HostPolicyDenied,
    /// ENOSPC: the host's user-namespace limit is exhausted.
    HostResourceLimit,
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
/// Missing declarations retain the existing native layer. Only a signed,
/// shipped outer-only row may request permission bypass inside AIDA's boundary.
// trace:TASK-3048 | ai:codex
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NativeLayer {
    #[default]
    Required,
    None,
}

pub(crate) fn native_layer(vendor: &str) -> anyhow::Result<NativeLayer> {
    let data: Capabilities = toml::from_str(include_str!("vendors.toml"))?;
    anyhow::ensure!(data.version == 1, "unsupported capability schema");
    Ok(data
        .vendor
        .get(vendor)
        .ok_or_else(|| anyhow::anyhow!("unsupported vendor capability: {vendor}"))?
        .native_layer)
}

/// Data for an outer-only vendor on the common boundary. Host inventories are
/// evidence, not implicit egress grants: trusted policy must allow each host.
// trace:TASK-3048 | ai:codex
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuntimePolicy {
    pub relative: String,
    pub credential: String,
    pub auth_hosts: Vec<String>,
    pub auth_inventory_complete: bool,
    pub raw_store_writes: bool,
    pub raw_git_writes: bool,
    pub argv_prefix: Vec<String>,
    pub prompt_options: Vec<String>,
    pub tool_denies: Vec<String>,
}

/// Legacy launchers may not supply an outer-only vendor's permission bypass.
/// Only the common wrapper adds it, behind its receipt/exec barrier. Inspect
/// option names, never value-taking prompt contents (including joined forms).
// trace:TASK-3048 | ai:codex
pub(crate) fn refuse_unreceipted_bypass(vendor: &str, args: &[String]) -> anyhow::Result<()> {
    if native_layer(vendor)? != NativeLayer::None {
        return Ok(());
    }
    let policy = runtime_policy(vendor)?;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let (name, joined) = arg
            .split_once('=')
            .map_or((arg.as_str(), false), |(name, _)| (name, true));
        if !name.starts_with('-') {
            continue;
        }
        // Go flags accept one or two leading dashes. Normalize both the
        // forbidden options and value-taking prompt options before matching.
        let name = name.trim_start_matches('-');
        if policy
            .prompt_options
            .iter()
            .any(|option| option.trim_start_matches('-') == name)
        {
            if !joined {
                let _ = args.next();
            }
            continue;
        }
        anyhow::ensure!(
            !name.starts_with("dangerously-"),
            "{vendor} permission bypass requires an applied AIDA ContainmentReceipt from the shared wrapper; legacy launch refused"
        );
    }
    Ok(())
}

pub(crate) fn runtime_policy(vendor: &str) -> anyhow::Result<RuntimePolicy> {
    let data: Capabilities = toml::from_str(include_str!("vendors.toml"))?;
    anyhow::ensure!(data.version == 1, "unsupported capability schema");
    let capability = data
        .vendor
        .get(vendor)
        .ok_or_else(|| anyhow::anyhow!("unsupported vendor capability: {vendor}"))?;
    anyhow::ensure!(
        capability.native_layer == NativeLayer::None,
        "private runtime policy requires an outer-only descriptor"
    );
    let runtime = capability
        .runtime
        .clone()
        .ok_or_else(|| anyhow::anyhow!("{vendor} has no private runtime policy"))?;
    runtime.validate()?;
    Ok(runtime)
}

/// Mask the host parent of every shipped private runtime for every vendor.
/// A new descriptor cannot leave its credentials visible to an older vendor.
// trace:TASK-3048 | ai:codex
pub(crate) fn runtime_host_parents(home: &Path) -> anyhow::Result<Vec<std::path::PathBuf>> {
    let data: Capabilities = toml::from_str(include_str!("vendors.toml"))?;
    anyhow::ensure!(data.version == 1, "unsupported capability schema");
    let mut paths = Vec::new();
    for capability in data.vendor.values() {
        if let Some(runtime) = &capability.runtime {
            runtime.validate()?;
            let parent = Path::new(&runtime.relative)
                .components()
                .next()
                .ok_or_else(|| anyhow::anyhow!("runtime parent missing"))?;
            paths.push(home.join(parent.as_os_str()));
        }
    }
    Ok(paths)
}

impl RuntimePolicy {
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.prompt_options.is_empty()
                && self
                    .prompt_options
                    .iter()
                    .all(|option| option.starts_with('-')
                        && !option.trim_start_matches('-').starts_with("dangerously-")
                        && !option.contains('=')
                        && !option.chars().any(char::is_whitespace)),
            "runtime prompt options must be option names"
        );
        for path in [&self.relative, &self.credential] {
            anyhow::ensure!(
                !path.is_empty()
                    && Path::new(path)
                        .components()
                        .all(|c| matches!(c, std::path::Component::Normal(_))),
                "runtime paths must be relative normal components"
            );
        }
        anyhow::ensure!(
            Path::new(&self.credential).components().count() == 1,
            "runtime credential must be a single leaf"
        );
        anyhow::ensure!(
            !self.raw_store_writes && !self.raw_git_writes,
            "outer-only runtime requires mediated Git/store writes"
        );
        anyhow::ensure!(
            !self.auth_hosts.is_empty(),
            "runtime auth host inventory is empty"
        );
        for host in &self.auth_hosts {
            crate::launch::contained::validate_allowed_host(host)?;
        }
        anyhow::ensure!(
            self.tool_denies == ["mcp(*)", "read_url(*)", "execute_url(*)"],
            "outer-only runtime requires MCP, URL and browser tool denies"
        );
        Ok(())
    }

    pub(crate) fn require_auth_policy(&self, allowed: &[String]) -> anyhow::Result<()> {
        self.validate()?;
        anyhow::ensure!(
            self.auth_inventory_complete,
            "outer-only auth/refresh host inventory is incomplete; refusing to launch"
        );
        for host in &self.auth_hosts {
            anyhow::ensure!(
                allowed.contains(host),
                "outer-only auth host {host} requires trusted contained.allowed_hosts approval"
            );
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Capability {
    native: NativeAdapter,
    #[serde(default)]
    native_layer: NativeLayer,
    ipc: IpcLayout,
    credentials: CredentialLayout,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    runtime: Option<RuntimePolicy>,
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
    remaining_protections: Vec<&'static str>,
    checks: Vec<String>,
}

/// Read one pinned policy snapshot; branch-local config cannot select/widen it.
/// Policy is reported, never executed. The probe itself uses only built-in argv.
// trace:TASK-2705 | ai:codex
pub(crate) fn probe(root: &Path, vendor: &str) -> ProbeReport {
    probe_with_runners(
        root,
        vendor,
        |isolation| supervisor::run_probe(|| isolation.state = IsolationState::Available),
        || supervisor::run_egress_probe("normal"),
    )
}

// Separate fixed runners allow report-credit tests without host isolation support.
// trace:TASK-2706 | ai:codex
fn probe_with_runners(
    root: &Path,
    vendor: &str,
    filesystem: impl FnOnce(&mut Isolation) -> anyhow::Result<supervisor::TracerOutcome>,
    egress: impl FnOnce() -> anyhow::Result<Vec<String>>,
) -> ProbeReport {
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
        remaining_protections: vec![
            "egress_and_local_ipc",
            "delegated_grant",
            "native_composition",
            "vendor_lifecycle",
        ],
        checks: Vec::new(),
    };
    let run = || -> anyhow::Result<supervisor::TracerOutcome> {
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
        filesystem(&mut report.isolation)
    };
    match run() {
        Ok(supervisor::TracerOutcome::IsolationUnavailable {
            reason,
            phase,
            errno,
        }) => {
            // Typed host property, not a tracer failure and never a pass.
            let what = match reason {
                IsolationReason::HostPolicyDenied => "host policy denied nested user namespace",
                IsolationReason::HostResourceLimit => {
                    "host resource limit: user namespace limit exhausted"
                }
            };
            report.cause = format!(
                "isolation unavailable: {what} (phase {phase}, errno {errno}); no fallback"
            );
            report.isolation = Isolation {
                state: IsolationState::Unavailable,
                reason: Some(reason),
                phase: Some(phase),
                errno: Some(errno),
            };
        }
        Ok(supervisor::TracerOutcome::Passed(checks)) => {
            debug_assert_eq!(report.isolation.state, IsolationState::Available);
            report.checks = checks;
            // trace:TASK-2706 | ai:codex — a filesystem pass alone cannot
            // report the extended tracer ready. Preserve actual A coverage
            // on a B refusal; never credit unfinished protections.
            report.stage = ProbeStage::EgressLocalIpc;
            match egress() {
                Ok(checks) => {
                    report.checks.extend(checks);
                    report
                        .remaining_protections
                        .retain(|p| *p != "egress_and_local_ipc");
                    report.status = ProbeStatus::Ready;
                    report.stage = ProbeStage::Complete;
                    report.cause = "fixed filesystem/supervisor/egress/IPC tracer passed; full backend remains unsupported".into();
                }
                Err(error) => report.cause = format!("{error:#}"),
            }
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
