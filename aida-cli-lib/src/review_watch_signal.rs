//! Verified signalling for a superseded review. Each signal goes through a
//! pidfd opened before the process identity is re-checked in /proc, so a
//! reused PID is never signalled; members of the review's process group that
//! were never recorded are signalled only once ancestry proves them ours.
//! pidfd and /proc are Linux-only: elsewhere every request holds.
// trace:TASK-1945 | ai:claude
use super::Identity;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Signal {
    Term,
    Kill,
}

/// The diagnostic a non-Linux watcher holds with.
pub(super) const UNSUPPORTED: &str =
    "superseding a review needs Linux pidfd and /proc; held without signalling";
// The scheme `aida_core::liveness::process_start_identity_exact` writes on Linux.
const LINUX_START: &str = "linux-starttime:";
// Bound on the parent walk that proves a group member descends from us.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const ANCESTRY_LIMIT: usize = 64;

/// Kernel start ticks of a recorded identity.
fn ticks(start: &str) -> Option<u64> {
    start.strip_prefix(LINUX_START)?.parse().ok()
}

/// A process started no earlier than `anchor`. Start ticks count from boot,
/// so they compare only within one boot; another boot is never ours.
// trace:TASK-1945 | ai:claude
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(super) fn started_not_before(start: u64, boot: &str, anchor: &Identity) -> bool {
    boot == anchor.boot && ticks(&anchor.start).is_some_and(|anchor| start >= anchor)
}

#[cfg(test)]
thread_local! {
    // Tests inject a kernel without pidfd_open (ENOSYS).
    pub(super) static NO_PIDFD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
#[cfg(test)]
static TEST_GROUPS: std::sync::Mutex<std::collections::BTreeSet<u32>> =
    std::sync::Mutex::new(std::collections::BTreeSet::new());

/// Register a process group a test created; only its members may be signalled.
#[cfg(test)]
pub(super) fn allow_test_group(pgid: u32) {
    TEST_GROUPS.lock().unwrap().insert(pgid);
}

/// The test blast-radius guard: a signal to anything outside a test-created
/// process group is a bug in the verify code, never a developer's process.
#[cfg(test)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn test_guard(pid: u32, pgrp: u32) {
    if !TEST_GROUPS.lock().unwrap().contains(&pgrp) {
        panic!("review-watch test refused to signal pid {pid}: process group {pgrp} was not created by the test");
    }
}

/// A fixture's process group is gone: its number may be reused.
#[cfg(test)]
pub(super) fn forget_test_group(pgid: u32) {
    TEST_GROUPS.lock().unwrap().remove(&pgid);
}

/// Signal a fixture process directly, through the same guard.
#[cfg(all(test, target_os = "linux"))]
pub(super) fn test_signal(pid: u32, signal: Signal) {
    let Some(stat) = imp::stat(pid) else {
        return;
    };
    test_guard(pid, stat.pgrp);
    // SAFETY: kill takes no pointers; the guard confined the target.
    unsafe { libc::kill(pid as libc::pid_t, imp::number(signal)) };
}

/// What one verified signal request did.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Sent,
    /// The recorded process no longer runs: exited, a zombie, a reused PID
    /// or an earlier boot. Nothing was signalled.
    Gone,
    /// Not signalled: why it could not be proved safe.
    Held(String),
}

/// Signal the attempt's processes: proven members of `pgid` first (while
/// their recorded ancestors still live to prove them), then the recorded
/// processes in the given leaf-first order. Returns every hold cause.
// trace:TASK-1945 | ai:claude
pub(super) fn signal_attempt(
    recorded: &[&Identity],
    pgid: Option<u32>,
    anchor: Option<&Identity>,
    signal: Signal,
) -> Vec<String> {
    #[cfg(target_os = "linux")]
    {
        imp::signal_attempt(recorded, pgid, anchor, signal)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (recorded, pgid, anchor, signal);
        vec![UNSUPPORTED.to_string()]
    }
}

/// Live (non-zombie) processes in `pgid`, recorded or not.
pub(super) fn group_members(pgid: u32) -> Result<Vec<u32>, String> {
    #[cfg(target_os = "linux")]
    {
        imp::group_members(pgid)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pgid;
        Err(UNSUPPORTED.to_string())
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{Identity, Outcome, Signal, ANCESTRY_LIMIT};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    pub(super) struct Stat {
        state: String,
        ppid: u32,
        pub(super) pgrp: u32,
        start: u64,
    }

    /// Fields 3, 4, 5 and 22 of /proc/<pid>/stat, parsed after the last `)`
    /// because the command name may contain one.
    pub(super) fn stat(pid: u32) -> Option<Stat> {
        let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let fields: Vec<&str> = text[text.rfind(')')? + 1..].split_whitespace().collect();
        Some(Stat {
            state: fields.first()?.to_string(),
            ppid: fields.get(1)?.parse().ok()?,
            pgrp: fields.get(2)?.parse().ok()?,
            start: fields.get(19)?.parse().ok()?,
        })
    }

    pub(super) fn number(signal: Signal) -> i32 {
        match signal {
            Signal::Term => libc::SIGTERM,
            Signal::Kill => libc::SIGKILL,
        }
    }

    /// Real and effective uid are both ours.
    fn ours(pid: u32) -> bool {
        let euid = super::super::effective_uid().to_string();
        std::fs::read_to_string(format!("/proc/{pid}/status"))
            .ok()
            .and_then(|status| {
                let line = status.lines().find_map(|l| l.strip_prefix("Uid:"))?;
                let ids: Vec<&str> = line.split_whitespace().collect();
                Some(ids.len() >= 2 && ids[0] == euid && ids[1] == euid)
            })
            .unwrap_or(false)
    }

    /// `Ok(None)`: no such process.
    fn pin(pid: u32) -> Result<Option<OwnedFd>, String> {
        let unavailable =
            || format!("pidfd_open is unavailable (ENOSYS); pid {pid} held without signalling");
        #[cfg(test)]
        if super::NO_PIDFD.with(|c| c.get()) {
            return Err(unavailable());
        }
        // SAFETY: pidfd_open takes a pid and flags, no pointers.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0) } as i32;
        if fd >= 0 {
            // SAFETY: the kernel just returned this descriptor to us.
            return Ok(Some(unsafe { OwnedFd::from_raw_fd(fd) }));
        }
        let error = std::io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::ESRCH) => Ok(None),
            Some(libc::ENOSYS) => Err(unavailable()),
            _ => Err(format!(
                "cannot pin pid {pid}: {error}; held without signalling"
            )),
        }
    }

    /// Hard floors on every signal path.
    fn floors(pid: u32, pgrp: u32) -> Result<(), String> {
        // SAFETY: getpgrp takes no arguments and cannot fail.
        let own_group = unsafe { libc::getpgrp() } as u32;
        if pid <= 1 || pid == std::process::id() || pgrp <= 1 || pgrp == own_group {
            return Err(format!(
                "refused to signal pid {pid} in process group {pgrp}: outside the review's own processes"
            ));
        }
        Ok(())
    }

    fn send(fd: &OwnedFd, pid: u32, pgrp: u32, signal: Signal) -> Outcome {
        if let Err(cause) = floors(pid, pgrp) {
            return Outcome::Held(cause);
        }
        #[cfg(test)]
        super::test_guard(pid, pgrp);
        // SAFETY: a valid pidfd, a signal number, a null siginfo and no flags.
        let sent = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                fd.as_raw_fd(),
                number(signal),
                std::ptr::null::<libc::siginfo_t>(),
                0,
            )
        };
        if sent == 0 {
            return Outcome::Sent;
        }
        let error = std::io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::ESRCH) => Outcome::Gone,
            _ => Outcome::Held(format!("signalling pid {pid} failed: {error}")),
        }
    }

    /// Pin a recorded process and prove it is still exactly that process,
    /// ours, and in the recorded group. `Ok(None)`: it is gone.
    fn verify(identity: &Identity, pgid: Option<u32>) -> Result<Option<(OwnedFd, Stat)>, String> {
        if super::super::boot_id().ok().as_deref() != Some(identity.boot.as_str()) {
            return Ok(None);
        }
        let Some(fd) = pin(identity.pid)? else {
            return Ok(None);
        };
        // Read only after the pin: a matching start identity now proves the
        // descriptor refers to the recorded process, never a reused PID.
        let Some(stat) = stat(identity.pid) else {
            return Ok(None);
        };
        if stat.state == "Z" || super::ticks(&identity.start) != Some(stat.start) {
            return Ok(None);
        }
        if !ours(identity.pid) {
            return Err(format!(
                "pid {} belongs to another user; held without signalling",
                identity.pid
            ));
        }
        if let Some(pgid) = pgid.filter(|pgid| *pgid != stat.pgrp) {
            return Err(format!(
                "pid {} is in process group {}, not the recorded {pgid}; held without signalling",
                identity.pid, stat.pgrp
            ));
        }
        Ok(Some((fd, stat)))
    }

    /// An unrecorded group member is ours: same uid, started no earlier than
    /// the dispatcher on this boot, and its parent chain reaches a recorded
    /// process that is still exactly alive.
    fn proven(pid: u32, stat: &Stat, recorded: &[&Identity], anchor: Option<&Identity>) -> bool {
        let Some(anchor) = anchor else { return false };
        let Ok(boot) = super::super::boot_id() else {
            return false;
        };
        if !ours(pid) || !super::started_not_before(stat.start, &boot, anchor) {
            return false;
        }
        let mut parent = stat.ppid;
        for _ in 0..ANCESTRY_LIMIT {
            if parent <= 1 {
                return false;
            }
            if recorded.iter().any(|r| r.pid == parent && r.alive()) {
                return true;
            }
            match self::stat(parent) {
                Some(next) => parent = next.ppid,
                None => return false,
            }
        }
        false
    }

    pub(super) fn group_members(pgid: u32) -> Result<Vec<u32>, String> {
        let entries = std::fs::read_dir("/proc").map_err(|e| format!("cannot scan /proc: {e}"))?;
        let mut members = Vec::new();
        for entry in entries.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|s| s.parse::<u32>().ok())
            else {
                continue;
            };
            if stat(pid).is_some_and(|s| s.pgrp == pgid && s.state != "Z") {
                members.push(pid);
            }
        }
        Ok(members)
    }

    pub(super) fn signal_attempt(
        recorded: &[&Identity],
        pgid: Option<u32>,
        anchor: Option<&Identity>,
        signal: Signal,
    ) -> Vec<String> {
        let mut held = Vec::new();
        let mut targets: Vec<(OwnedFd, u32, u32)> = Vec::new();
        // An attempt recorded before process groups were kept gets
        // per-identity signals only.
        if let Some(pgid) = pgid {
            match group_members(pgid) {
                Err(cause) => held.push(cause),
                Ok(members) => {
                    for pid in members {
                        if recorded.iter().any(|r| r.pid == pid) {
                            continue;
                        }
                        let fd = match pin(pid) {
                            Ok(Some(fd)) => fd,
                            Ok(None) => continue,
                            Err(cause) => {
                                held.push(cause);
                                continue;
                            }
                        };
                        match stat(pid) {
                            Some(s) if s.pgrp != pgid || s.state == "Z" => {}
                            Some(s) if proven(pid, &s, recorded, anchor) => {
                                targets.push((fd, pid, s.pgrp))
                            }
                            Some(_) => held.push(format!(
                                "process {pid} in the review's group {pgid} cannot be proved ours; not signalled"
                            )),
                            None => {}
                        }
                    }
                }
            }
        }
        for identity in recorded {
            match verify(identity, pgid) {
                Ok(Some((fd, s))) => targets.push((fd, identity.pid, s.pgrp)),
                Ok(None) => {}
                Err(cause) => held.push(cause),
            }
        }
        for (fd, pid, pgrp) in &targets {
            if let Outcome::Held(cause) = send(fd, *pid, *pgrp, signal) {
                held.push(cause);
            }
        }
        held
    }
}
