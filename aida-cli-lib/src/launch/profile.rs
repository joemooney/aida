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

impl crate::lane_job::LaneJobs {
    /// Give the vendor child its lane's public lease selector, so
    /// `aida lane job` resolves the host runner without a path or token.
    /// Keep protected child identity projection inside the launcher.
    // trace:BUG-2383 STORY-2640 STORY-2326 | ai:codex
    pub(crate) fn apply_selector(&self, cmd: &mut Command) {
        cmd.env("AIDA_SESSION_ID", &self.lease_id);
    }
}

/// A headless harness can ignore or block terminal interrupts. Reset that
/// state only in the sandboxed gate child so its PTY fixtures can receive them.
/// Use the CI file-creation mask too: a shared-write host mask otherwise makes
/// ownership-sensitive fixtures reject their own freshly created directories.
// trace:STORY-2640 | ai:codex
#[cfg(unix)]
pub(crate) fn prepare_fast_gate_signals(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: the child hook uses only umask, signal operations and stack data;
    // it captures no state and does not allocate or acquire locks.
    unsafe {
        cmd.pre_exec(|| {
            libc::umask(0o022);
            let mut signals: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut signals);
            for signal in [libc::SIGINT, libc::SIGQUIT] {
                if libc::signal(signal, libc::SIG_DFL) == libc::SIG_ERR {
                    return Err(std::io::Error::last_os_error());
                }
                libc::sigaddset(&mut signals, signal);
            }
            if libc::sigprocmask(libc::SIG_UNBLOCK, &signals, std::ptr::null_mut()) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

/// The effectful half of the vendor profile, keyed by the existing closed enum.
// trace:STORY-2539 | ai:codex
pub(crate) struct VendorProfile(&'static aida_core::vendors::VendorProfile);

impl VendorProfile {
    pub(crate) fn for_vendor(vendor: LaunchVendor) -> Self {
        Self(aida_core::vendors::profile(vendor))
    }

    /// Exact non-AIDA configuration for each vendor; no prefix forwarding.
    // trace:STORY-2326 | ai:codex
    pub(crate) fn env_policy(
        &self,
        kind: aida_core::launch_policy::LaunchKind,
    ) -> Result<aida_core::launch_policy::EnvPolicy> {
        use aida_core::launch_policy::{
            EnvPolicy, EnvRecipient, Owned, AMBIENT_AIDA_KEYS, OWNED_KEYS,
        };
        const COMMON: &[&str] = &[
            "PATH",
            "HOME",
            "USER",
            "LOGNAME",
            "SHELL",
            "TERM",
            "COLORTERM",
            "LANG",
            "LC_ALL",
            "LC_CTYPE",
            "TZ",
            "TMPDIR",
            "TMP",
            "TEMP",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
            "XDG_STATE_HOME",
            "XDG_RUNTIME_DIR",
            "SSH_AUTH_SOCK",
            "GIT_SSH_COMMAND",
            "GIT_CONFIG_GLOBAL",
            "CARGO_HOME",
            "RUSTUP_HOME",
            "CARGO_TARGET_DIR",
            "RUSTC_WRAPPER",
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "NO_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
            "no_proxy",
            "SSL_CERT_FILE",
            "SSL_CERT_DIR",
        ];
        let vendor_keys = aida_core::vendors::VendorValues::<&[&str]> {
            claude: &[
                "CLAUDE_CONFIG_DIR",
                "ANTHROPIC_API_KEY",
                "ANTHROPIC_BASE_URL",
                "CLAUDE_CODE_OAUTH_TOKEN",
            ],
            codex: &["CODEX_HOME", "OPENAI_API_KEY", "OPENAI_BASE_URL"],
            agy: &[
                "GEMINI_API_KEY",
                "GOOGLE_API_KEY",
                "GOOGLE_APPLICATION_CREDENTIALS",
            ],
        }
        .get(self.0.vendor);
        Ok(EnvPolicy::new(
            self.0.vendor,
            kind,
            EnvRecipient::VendorProcess,
            COMMON
                .iter()
                .chain(vendor_keys)
                .chain(AMBIENT_AIDA_KEYS)
                .copied()
                .chain(
                    OWNED_KEYS
                        .iter()
                        .filter(|(_, how)| *how == Owned::Forwarded)
                        .map(|(key, _)| *key),
                )
                .map(std::ffi::OsString::from),
        )?)
    }

    // trace:STORY-2326 | ai:codex
    pub(crate) fn clears_environment(&self) -> bool {
        aida_core::vendors::VendorValues {
            claude: true,
            codex: false,
            agy: false,
        }
        .get(self.0.vendor)
    }

    // trace:STORY-2326 | ai:codex
    pub(crate) fn child_env(
        &self,
        kind: aida_core::launch_policy::LaunchKind,
        parent: &aida_core::exec_context::Environment,
        resources: Option<&super::resources::SandboxResources>,
    ) -> Result<aida_core::launch_policy::ChildEnv> {
        let policy = self.env_policy(kind)?;
        Ok(match resources {
            Some(resources) => resources.child_env(&policy, parent)?,
            None => policy.project(parent, [])?,
        })
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

/// Command exposes no inheritance getter. Removing an unset reserved probe
/// is recorded only on inheriting commands; cleared commands discard it.
/// Command has no API to erase that removal entry without clearing all env
/// decisions, so inheriting commands retain this harmless reserved tombstone.
// trace:STORY-2326 | ai:codex
pub(crate) fn environment_is_cleared(child: &mut Command) -> bool {
    const PROBE: &str = "__AIDA_LAUNCH_INHERITANCE_PROBE";
    child.env_remove(PROBE);
    !child.get_envs().any(|(key, _)| key == PROBE)
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
    let cleared = environment_is_cleared(child);
    let mut replacement = Command::new(child.get_program());
    if cleared {
        replacement.env_clear();
    }
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
    // Managed constructors configure process hooks only after finalization.
    // Every managed caller finalizes before configuring stdio or process
    // hooks. Rebuilding preserves the explicit env, cwd and wrapper argv.
    *child = replacement;
    Ok(())
}

#[cfg(test)]
#[path = "../tests/story_2326_profile_tests.rs"]
mod tests;
