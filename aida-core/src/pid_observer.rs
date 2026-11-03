//! The one judge of whether a recorded PID can be probed from this process.
//!
//! A PID only means something inside the PID namespace it was recorded in.
//! Records stamp that namespace (`pid_namespace`); a reader in a different
//! namespace (a sandboxed command) cannot prove the process alive or dead and
//! must skip the PID probe. Unstamped legacy records are judged only from a
//! non-isolated initial namespace: an isolated sandbox can show the initial
//! namespace's inode while still seeing only its own processes (BUG-2212).
//!
//! Moved here from `aida-cli-lib::agent_registry` so `aida-core` liveness,
//! session reap, worktree GC, claims and agent GC all share one judge.
//! The mandate gate keeps its own floor copy of [`PidNamespaceProbe`]
//! (`gate/dispatch_deps.rs`), held in parity by a cli test.
//!
//! trace:BUG-2425 trace:BUG-2212 trace:STORY-1843 | ai:claude

use crate::process_probe::INITIAL_PID_NAMESPACE_INODE;

/// The PID namespace inode of this process, or `None` where it cannot be read.
// trace:STORY-1843 trace:BUG-2425 | ai:claude
pub fn current_pid_namespace() -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata("/proc/self/ns/pid").ok().map(|m| m.ino())
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// Raw `/proc` facts that say whether this process sits in a PID namespace
/// of its own (a sandboxed shell sees only its own few pids).
// trace:BUG-2212 | ai:claude
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PidNamespaceProbe {
    pub self_ns: Option<u64>,
    /// `None` when `/proc/1/ns/pid` is unreadable (pid 1 owned by another user).
    pub pid1_ns: Option<u64>,
    /// Entries on the `NSpid:` line of `/proc/self/status`.
    pub nspid_levels: usize,
    pub pid1_comm: Option<String>,
}

impl PidNamespaceProbe {
    pub fn read() -> Self {
        #[cfg(unix)]
        let ns = |path: &str| {
            use std::os::unix::fs::MetadataExt;
            std::fs::metadata(path).ok().map(|m| m.ino())
        };
        #[cfg(not(unix))]
        let ns = |_: &str| None;
        let nspid_levels = std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|body| {
                body.lines()
                    .find_map(|l| l.strip_prefix("NSpid:"))
                    .map(|rest| rest.split_whitespace().count())
            })
            .unwrap_or(0);
        Self {
            self_ns: current_pid_namespace(),
            pid1_ns: ns("/proc/1/ns/pid"),
            nspid_levels,
            pid1_comm: std::fs::read_to_string("/proc/1/comm")
                .ok()
                .map(|c| c.trim().to_string()),
        }
    }

    /// Isolated when pid 1 lives in another namespace or the kernel reports
    /// more than one pid level. Only when `/proc/1/ns/pid` is unreadable does
    /// pid 1's name decide: anything but a known init means a sandbox.
    // trace:TASK-2272 | ai:claude
    pub fn is_isolated(&self) -> bool {
        const INITS: [&str; 10] = [
            "systemd",
            "init",
            "tini",
            "dumb-init",
            "docker-init",
            "catatonit",
            "runit",
            "openrc-init",
            "s6-svscan",
            "busybox",
        ];
        match (self.self_ns, self.pid1_ns) {
            (Some(a), Some(b)) if a != b => return true,
            _ => {}
        }
        if self.nspid_levels > 1 {
            return true;
        }
        if self.pid1_ns.is_some() {
            return false;
        }
        self.pid1_comm
            .as_deref()
            .is_some_and(|comm| !INITS.contains(&comm))
    }
}

/// The PID view a liveness check runs from: which namespace this process is
/// in, and whether that namespace is an isolated sandbox.
// trace:BUG-2212 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PidObserver {
    pub namespace: Option<u64>,
    pub isolated: bool,
}

impl PidObserver {
    /// This process's observer. Read once: a process never changes PID
    /// namespace, and every lease/claim judgement asks.
    // trace:BUG-2425 | ai:claude
    pub fn current() -> Self {
        static CURRENT: std::sync::OnceLock<PidObserver> = std::sync::OnceLock::new();
        *CURRENT.get_or_init(|| Self::from_probe(&PidNamespaceProbe::read()))
    }

    pub fn from_probe(probe: &PidNamespaceProbe) -> Self {
        Self {
            namespace: probe.self_ns,
            isolated: probe.is_isolated(),
        }
    }

    /// A row's process is visible only from the namespace it was stamped in;
    /// an unstamped legacy row only from a non-isolated initial namespace.
    pub fn can_observe(&self, recorded: Option<u64>) -> bool {
        match (recorded, self.namespace) {
            (Some(recorded), Some(current)) => recorded == current,
            (None, Some(INITIAL_PID_NAMESPACE_INODE)) => !self.isolated,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // BUG-2212 masked sandbox: an isolated observer showing the initial
    // namespace's inode cannot vouch for an unstamped legacy record.
    // trace:BUG-2425 | ai:claude
    #[test]
    fn isolated_initial_namespace_cannot_observe_unstamped_records() {
        let masked = PidObserver {
            namespace: Some(INITIAL_PID_NAMESPACE_INODE),
            isolated: true,
        };
        let host = PidObserver {
            isolated: false,
            ..masked
        };
        assert!(!masked.can_observe(None));
        assert!(host.can_observe(None));
        assert!(masked.can_observe(Some(INITIAL_PID_NAMESPACE_INODE)));
        assert!(!host.can_observe(Some(7)));
    }
}
