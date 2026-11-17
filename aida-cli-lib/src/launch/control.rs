//! Same-seat AIDA control children. A role string grants no inheritance.
// trace:STORY-2326 | ai:codex
use anyhow::{Context, Result};
use std::{os::unix::process::CommandExt, path::Path, process::Command};

pub(crate) fn control_child() -> Result<Command> {
    // Execute the kernel-held image itself. Cached paths, AIDA_BIN and build
    // profile overrides may name a replacement after promote/rollback. Never
    // canonicalize this magic link: even an unlinked running image is usable.
    control_child_with_current(
        Path::new("/proc/self/exe"),
        crate::gate::seat_authority::current_grant_selector().as_deref(),
    )
}

/// Long-lived services follow promotions, unlike invocation-scoped children.
// trace:BUG-2645 | ai:codex
pub(crate) fn live_service_child() -> Result<Command> {
    control_child_with_current(
        &crate::dev_build::default_live_dir()?.join("aida"),
        crate::gate::seat_authority::current_grant_selector().as_deref(),
    )
}

// Compare image coordinates, including deleted executables. No process
// is signalled: controllers yield at their normal admission/pass boundary.
// trace:BUG-2645 | ai:codex
pub(crate) fn service_image_is_live(image: &Path, live: &Path) -> Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let image = std::fs::metadata(image)?;
    let live = std::fs::metadata(live)?;
    Ok(image.dev() == live.dev() && image.ino() == live.ino())
}

/// Detach a control child and inherit its private handoff socket at `target`.
// trace:TASK-3219 | ai:codex
pub(crate) fn detach_with_socket(helper: &mut Command, fd: i32, target: i32) {
    // SAFETY: only async-signal-safe syscalls between fork and exec.
    unsafe {
        helper.pre_exec(move || {
            if libc::setsid() < 0
                || (fd != target && libc::dup2(fd, target) < 0)
                || libc::fcntl(target, libc::F_SETFD, 0) < 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

/// Configure a supervised foreground worker for noninteractive print mode.
pub(crate) fn background_print(command: &mut Command, root: &Path) {
    let (ceiling_key, ceiling_value) = crate::bg_wait_ceiling_env(Some(root));
    command
        .arg("--print")
        .env("AIDA_HEADLESS", "1")
        .envs(crate::headless_wait_envs(ceiling_key, &ceiling_value));
}

/// Only phase-driver unit fixtures may inject a coordinating executable.
#[cfg(test)]
pub(crate) fn control_child_for_test(program: &Path) -> Result<Command> {
    let current = program.canonicalize()?;
    control_child_with_current(
        &current,
        crate::gate::seat_authority::current_grant_selector().as_deref(),
    )
}

fn control_child_with_current(current: &Path, grant: Option<&std::ffi::OsStr>) -> Result<Command> {
    std::fs::metadata(current)
        .context("cannot access the running AIDA executable for a control child")?;
    let mut child = Command::new(current);
    child.arg0("aida");
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
    // trace:BUG-2645 | ai:codex
    #[test]
    fn bug2645_service_detects_promotion_and_deleted_image() {
        let dir = tempfile::tempdir().unwrap();
        let live = dir.path().join("aida");
        let running = dir.path().join("running");
        std::fs::write(&live, "old").unwrap();
        std::fs::hard_link(&live, &running).unwrap();
        assert!(service_image_is_live(&running, &live).unwrap());
        let replacement = dir.path().join("incoming");
        std::fs::write(&replacement, "new").unwrap();
        std::fs::rename(&replacement, &live).unwrap();
        assert!(!service_image_is_live(&running, &live).unwrap());
        use std::os::fd::AsRawFd;
        let held = std::fs::File::open(&running).unwrap();
        std::fs::remove_file(&running).unwrap();
        let deleted = std::path::PathBuf::from(format!("/proc/self/fd/{}", held.as_raw_fd()));
        assert!(!service_image_is_live(&deleted, &live).unwrap());
        std::fs::remove_file(&live).unwrap();
        assert!(service_image_is_live(&deleted, &live).is_err());
        // Kernel-held deleted images stay observable through procfs.
        assert!(
            service_image_is_live(Path::new("/proc/self/exe"), Path::new("/proc/self/exe"))
                .unwrap()
        );
    }

    // trace:BUG-2645 | ai:codex
    #[test]
    fn bug2645_service_launch_uses_live_path_and_control_environment() {
        let dir = tempfile::tempdir().unwrap();
        let mut env = crate::test_env::EnvVarsGuard::apply(&[]);
        env.set_key("AIDA_DEV_LIVE_DIR", dir.path().as_os_str());
        let live = dir.path().join("aida");
        assert!(live_service_child().is_err());
        std::fs::write(&live, "fixture").unwrap();
        let command = live_service_child().unwrap();
        assert_eq!(command.get_program(), live.as_os_str());
        let environment: std::collections::BTreeMap<_, _> = command.get_envs().collect();
        assert_eq!(
            environment.get(std::ffi::OsStr::new("AIDA_SESSION_ROLE")),
            Some(&None)
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn story_2326_control_child_pins_program_and_forwards_only_selector() {
        for selector in [
            None,
            Some(std::ffi::OsStr::new("")),
            Some(std::ffi::OsStr::new("selector")),
        ] {
            let cmd = control_child_with_current(Path::new("/proc/self/exe"), selector).unwrap();
            assert_eq!(cmd.get_program(), "/proc/self/exe");
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
        assert_eq!(control_child().unwrap().get_program(), "/proc/self/exe");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn story_2326_control_child_survives_running_image_replacement() {
        const CHILD: &str = "AIDA_TEST_CONTROL_REPLACEMENT";
        if std::env::var_os(CHILD).is_some() {
            let live = std::fs::read_link("/proc/self/exe").unwrap();
            let previous = live.with_file_name("previous");
            std::fs::rename(&live, &previous).unwrap();
            std::fs::copy("/bin/false", &live).unwrap();
            for deleted in [false, true] {
                if deleted {
                    std::fs::remove_file(&previous).unwrap();
                }
                let out = control_child().unwrap()
                    .args(["--list", "--exact", "launch::control::tests::story_2326_control_child_pins_program_and_forwards_only_selector"])
                    .env_remove(CHILD).output().unwrap();
                assert!(out.status.success(), "deleted={deleted}: {out:?}");
                assert!(String::from_utf8_lossy(&out.stdout)
                    .contains("story_2326_control_child_pins_program"));
            }
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let live = dir.path().join("aida");
        std::fs::copy("/proc/self/exe", &live).unwrap();
        let status = Command::new(&live).args(["--exact", "launch::control::tests::story_2326_control_child_survives_running_image_replacement", "--nocapture"])
            .env(CHILD, "1").status().unwrap();
        assert!(status.success());
    }
}
