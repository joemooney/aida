//! Effectful vendor launch profile. Finalization happens after all child
//! metadata decisions and before stdio or process hooks, since Codex rebuilds
//! its command. R5 extends this adapter; R4 owns ambient input injection.
// trace:STORY-2326 | ai:codex
use crate::codex_tool_env::{
    codex_argv_start, codex_tool_env_overrides, preflight, refuse_baseline_conflicts,
    refuse_unwired_aida_posture, TOOL_ENV,
};
use crate::gate::seat_authority::GRANT_ENV;
use aida_core::launch_policy::LaunchVendor;
use anyhow::Result;
use std::process::Command;

/// The effectful half of the vendor profile, keyed by the existing closed enum.
// trace:STORY-2539 | ai:codex
pub(crate) struct VendorProfile(&'static aida_core::vendors::VendorProfile);

impl VendorProfile {
    pub(crate) fn for_vendor(vendor: LaunchVendor) -> Self {
        Self(aida_core::vendors::profile(vendor))
    }

    /// Last step before spawn/exec. Receipt-owning AIDA Codex launches use
    /// containment::launch instead; this adapter deliberately refuses them.
    pub(crate) fn finalize(&self, child: &mut Command) -> Result<()> {
        match self.0.vendor {
            LaunchVendor::Codex => prepare_launch(child),
            LaunchVendor::Claude | LaunchVendor::Agy => Ok(()),
        }
    }
}

/// Codex entry point after command metadata is complete. The finalizer
/// implementation is private to this profile, never site-owned.
pub(crate) fn finalize_codex(child: &mut Command) -> Result<()> {
    VendorProfile::for_vendor(LaunchVendor::Codex).finalize(child)
}

/// Call once, after the child grant, identity and customization are final,
/// immediately before spawn/exec. Other vendors never enter this adapter.
fn prepare_launch(child: &mut Command) -> Result<()> {
    refuse_unwired_aida_posture(child)?;
    // The child Command is the full source of truth for whitelisted metadata:
    // under Codex's default inherit="all", a key the launcher process has but
    // the child never assigned would otherwise leak into the tool shell and
    // fail the context digest (or present the launcher's own context).
    // trace:BUG-2398 | ai:claude
    for key in TOOL_ENV {
        if !child.get_envs().any(|(k, _)| k == *key) {
            child.env_remove(key);
        }
    }
    let overrides = codex_tool_env_overrides(child)?;
    let grant = child
        .get_envs()
        .find(|(key, _)| *key == GRANT_ENV)
        .and_then(|(_, value)| value);
    anyhow::ensure!(grant.is_some(), "Codex child has no explicit grant decision; provide its delegated grant or an explicit empty baseline");
    if let Some(seat) = child
        .get_envs()
        .find(|(key, _)| *key == "AIDA_SESSION_ROLE")
        .and_then(|(_, value)| value)
        .and_then(|value| value.to_str())
    {
        anyhow::ensure!(
            crate::gate::seat_authority::is_baseline_seat(seat) || !grant.unwrap().is_empty(),
            "Codex requested seat requires its own nonempty delegated grant"
        );
    }
    if grant.unwrap().is_empty() {
        refuse_baseline_conflicts(child)?;
    }
    preflight(child, &overrides)?;
    let prefix = codex_argv_start(child)?;
    let original: Vec<_> = child.get_args().map(|arg| arg.to_owned()).collect();
    let mut replacement = Command::new(child.get_program());
    if let Some(cwd) = child.get_current_dir() {
        replacement.current_dir(cwd);
    }
    for (key, value) in child.get_envs() {
        match value {
            Some(value) => {
                replacement.env(key, value);
            }
            None => {
                replacement.env_remove(key);
            }
        }
    }
    replacement
        .args(&original[..prefix])
        .args(overrides)
        .args(&original[prefix..]);
    // Managed constructors use default env inheritance and no process hooks.
    // Every managed caller finalizes before configuring stdio or process
    // hooks. Rebuilding preserves the explicit env, cwd and wrapper argv.
    *child = replacement;
    Ok(())
}

#[cfg(test)]
#[path = "../tests/story_2326_profile_tests.rs"]
mod tests;
