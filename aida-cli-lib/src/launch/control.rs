//! Same-seat AIDA control children. A role string grants no inheritance.
// trace:STORY-2326 | ai:codex
use anyhow::{Context, Result};
use std::{path::Path, process::Command};

pub(crate) fn control_child(program: impl AsRef<Path>) -> Result<Command> {
    // Linux-only repository: this pins the actual running image, not AIDA_BIN,
    // PATH, a vendor executable, or a role supplied by a caller.
    let current = std::fs::canonicalize("/proc/self/exe")
        .context("cannot resolve the running AIDA executable for a control child")?;
    control_child_with_current(
        program.as_ref(),
        &current,
        std::env::var_os(aida_core::launch_policy::GRANT_ENV).as_deref(),
    )
}

/// Unit phase-driver fixtures already inject their coordinating executable.
/// Treat that fixture as the current image only in test builds. The production
/// entry and its refusal tests always resolve the real /proc/self/exe.
#[cfg(test)]
pub(crate) fn control_child_for_test(program: &Path) -> Result<Command> {
    let current = program.canonicalize()?;
    control_child_with_current(
        program,
        &current,
        std::env::var_os(aida_core::launch_policy::GRANT_ENV).as_deref(),
    )
}

fn control_child_with_current(
    program: &Path,
    current: &Path,
    grant: Option<&std::ffi::OsStr>,
) -> Result<Command> {
    let program = program
        .canonicalize()
        .context("cannot resolve AIDA control child program")?;
    anyhow::ensure!(
        program == current,
        "control inheritance requires the canonical current AIDA executable"
    );
    let mut child = Command::new(program);
    for (key, value) in aida_core::launch_policy::control_env(grant) {
        match value {
            Some(value) => {
                child.env(key, value);
            }
            None => {
                child.env_remove(key);
            }
        }
    }
    Ok(child)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn story_2326_control_child_pins_program_and_forwards_only_selector() {
        let dir = tempfile::tempdir().unwrap();
        let aida = dir.path().join("aida");
        let vendor = dir.path().join("codex");
        std::fs::write(&aida, "").unwrap();
        std::fs::write(&vendor, "").unwrap();
        let current = aida.canonicalize().unwrap();
        // The public entry still pins the real process image in a test build;
        // only the explicit phase-fixture injection may name these fakes.
        assert!(control_child(&vendor).is_err());
        assert!(control_child(&aida).is_err());
        assert!(control_child(crate::aida_bin::current_executable_path().unwrap()).is_ok());
        for selector in [
            None,
            Some(std::ffi::OsStr::new("")),
            Some(std::ffi::OsStr::new("selector")),
        ] {
            let cmd = control_child_with_current(&aida, &current, selector).unwrap();
            let env: std::collections::BTreeMap<_, _> = cmd.get_envs().collect();
            assert_eq!(
                env.get(std::ffi::OsStr::new("AIDA_SESSION_ROLE")),
                Some(&None)
            );
            assert_eq!(
                env.get(std::ffi::OsStr::new("AIDA_SESSION_GRANT")),
                Some(&selector)
            );
        }
        assert!(control_child_with_current(&vendor, &current, None).is_err());
        assert!(control_child_with_current(&dir.path().join("missing"), &current, None).is_err());
        std::os::unix::fs::symlink(&aida, dir.path().join("alias")).unwrap();
        assert!(control_child_with_current(&dir.path().join("alias"), &current, None).is_ok());
    }
}
