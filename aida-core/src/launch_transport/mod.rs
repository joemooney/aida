//! Unwired waiting-child infrastructure. No production release exists.
//!
//! Linux host profile only; unsupported kernels/images fail before lifecycle
//! writes. This module never accesses the store, queues, hooks or manifests.
//! The future common publisher must supply its private CommittedDispatch;
//! Git-only PreparedGitStep credentials cannot substitute for that type.
// trace:TASK-1612 | ai:codex
// trace:BUG-1808 | ai:codex

#[cfg(target_os = "linux")]
mod image;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use image::{ImageIdentity, SealedExecImage};
#[cfg(target_os = "linux")]
pub use linux::{Cancellation, HostProfile, LaunchDescription, ProcessIdentity, WaitingChild};

/// Must run before CLI initialization. An ordinary invocation returns without
/// effects. A bootstrap invocation exits here, including malformed requests.
/// The argument selects a parser, never grants permission to execute a leaf.
pub fn bootstrap_if_requested() {
    if std::env::args_os().nth(1).as_deref() != Some(std::ffi::OsStr::new("--aida-waiting-child")) {
        return;
    }
    #[cfg(target_os = "linux")]
    linux::bootstrap_exit();
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("waiting-child transport requires the supported Linux host profile");
        std::process::exit(126);
    }
}

#[cfg(target_os = "linux")]
fn require_supported_build() -> anyhow::Result<()> {
    anyhow::ensure!(
        cfg!(all(target_arch = "x86_64", target_env = "gnu")),
        "waiting-child transport requires the validated Linux x86_64 GNU profile"
    );
    Ok(())
}
