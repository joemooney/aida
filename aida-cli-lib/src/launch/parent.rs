/// Arm and judge processes must die even if compete is killed without cleanup.
/// Arm before exec and recheck the parent to close the fork/prctl race.
// trace:BUG-2439 | ai:codex
pub(crate) fn bind_child_to_parent(cmd: &mut std::process::Command) {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        let parent = std::process::id() as libc::pid_t;
        // SAFETY: the post-fork closure uses only async-signal-safe syscalls,
        // allocates nothing, and changes only the child process's death signal.
        unsafe {
            cmd.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::getppid() != parent {
                    return Err(std::io::Error::from_raw_os_error(libc::ECHILD));
                }
                Ok(())
            });
        }
    }
}
