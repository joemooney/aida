//! Boot-local, namespace-qualified process observation and cooperative holder admission.
//! Observation is evidence, never ownership or signal authority. Legacy probes remain
//! compatibility adapters until their individual consumer migrations.
// trace:STORY-2537 trace:BUG-2458 | ai:codex

use crate::process_probe::{boot_id, is_exited, read_stat, Probe, ProcessIdentity, StatRead};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProcIdentity {
    pub boot_id: String,
    /// Coordinates of `pid`, not the target's innermost namespace.
    pub pidns_inode: u64,
    pub pid: u32,
    pub starttime: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeadReason {
    Reboot,
    Absent,
    Exited,
    Reused,
    Observed(String),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnknownReason {
    OtherNode,
    MissingIdentity,
    NamespaceMismatch,
    IncompleteProcfs,
    Unreadable(String),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Alive,
    Dead(DeadReason),
    ForeignUnknown(UnknownReason),
}

/// Only the caller's trusted local provenance can authorize reboot inference.
/// A hostname or a boot UUID supplied by another machine does not establish it.
pub struct ObserverFacts<'a> {
    pub same_node: bool,
    pub boot_id: Option<&'a str>,
    pub pidns_inode: Option<u64>,
    pub complete_procfs: bool,
}

/// Registry wire-version boundary. Older mail readers compare the persisted
/// ID verbatim with the filename and refuse this prefix, including session-ID
/// selection paths that did not inspect process evidence. In-memory IDs and
/// filenames remain stable; legacy unprefixed rows remain readable as unknown.
// trace:TASK-3366 | ai:codex
pub fn registry_wire_id(id: &str) -> String {
    format!("proc-identity-v1:{id}")
}
pub fn deserialize_registry_id<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    let id = String::deserialize(deserializer)?;
    if let Some(id) = id.strip_prefix("proc-identity-v1:") {
        return Ok(id.to_owned());
    }
    // Preserve unsupported versions verbatim: strict filename checks refuse
    // them, while registry retention readers must keep an unknown row rather
    // than silently dropping it as a parse failure.
    Ok(id)
}

fn valid_boot_id(boot: &str) -> bool {
    uuid::Uuid::parse_str(boot).is_ok_and(|id| id.hyphenated().to_string() == boot)
}

impl ProcIdentity {
    /// Observe a currently live process in the reader's proven local coordinates.
    /// This supplies identity evidence only, never cooperative holder admission.
    // trace:TASK-3225 | ai:codex
    pub fn capture(pid: u32) -> Result<Self, String> {
        let view = LocalView::read()?;
        let record = crate::process_probe::ProbeSource::system().capture(pid)?;
        let id = Self::from_record(&record, Some(view.namespace))
            .ok_or("incomplete process identity")?;
        match observe(&id, true) {
            Verdict::Alive => Ok(id),
            verdict => Err(format!("process identity unavailable: {verdict:?}")),
        }
    }

    /// Capture a retained child, including its unreaped exited state. The Child
    /// must not have been waited on: its zombie pins the PID until the caller
    /// records the receipt. This is observation, never holder admission.
    // trace:TASK-3365 | ai:codex
    pub fn capture_child(child: &std::process::Child) -> Result<Self, String> {
        let view = LocalView::read()?;
        let pid = child.id();
        let root = Path::new("/proc");
        let boot = boot_id(root)?;
        let StatRead::Found {
            start_ticks, ppid, ..
        } = read_stat(root, pid)?
        else {
            return Err("retained child is absent".into());
        };
        if ppid != std::process::id() {
            return Err("process is not our retained child".into());
        }
        let id = Self::from_record(
            &ProcessIdentity {
                pid,
                start: format!("linux:{boot}:{start_ticks}"),
            },
            Some(view.namespace),
        )
        .ok_or("incomplete child identity")?;
        match observe(&id, true) {
            Verdict::Alive | Verdict::Dead(DeadReason::Exited) => Ok(id),
            verdict => Err(format!("child identity unavailable: {verdict:?}")),
        }
    }

    /// Read an existing exact record; never recapture today's process at an old PID.
    pub fn from_record(id: &ProcessIdentity, namespace: Option<u64>) -> Option<Self> {
        let (boot, ticks) = crate::process_probe::parse_start_identity(&id.start)?;
        let ns = namespace.filter(|n| *n != 0)?;
        if !valid_boot_id(boot) || id.pid == 0 || id.pid > i32::MAX as u32 {
            return None;
        }
        Some(Self {
            boot_id: boot.into(),
            pidns_inode: ns,
            pid: id.pid,
            starttime: ticks,
        })
    }

    /// Decode an existing registry record without probing or upgrading its PID.
    // trace:TASK-3366 | ai:codex
    pub fn from_registry(pid: u32, start: Option<&str>, namespace: Option<u64>) -> Option<Self> {
        Self::from_record(
            &ProcessIdentity {
                pid,
                start: start?.to_owned(),
            },
            namespace,
        )
    }

    /// Raw-tick transport for existing grant, lease and display adapters.
    // trace:TASK-3366 | ai:codex
    pub fn legacy_ticks_start(&self) -> String {
        format!("linux-starttime:{}", self.starttime)
    }

    pub fn legacy(&self) -> ProcessIdentity {
        ProcessIdentity {
            pid: self.pid,
            start: format!("linux:{}:{}", self.boot_id, self.starttime),
        }
    }
}

/// Advisory compatibility evidence for incomplete local registry rows.
/// This never supplies a full identity or authorizes a reader. Namespace
/// admission and a verified procfs view precede absence/start-time checks.
// trace:TASK-3366 | ai:codex
pub fn observe_legacy_registry(pid: u32, start: Option<&str>, namespace: Option<u64>) -> Verdict {
    let unknown = || Verdict::ForeignUnknown(UnknownReason::MissingIdentity);
    // These values cannot name a Linux process in any namespace.
    if pid == 0 || pid > i32::MAX as u32 {
        return Verdict::Dead(DeadReason::Absent);
    }
    if !crate::pid_observer::PidObserver::current().can_observe(namespace) {
        return Verdict::ForeignUnknown(UnknownReason::NamespaceMismatch);
    }
    if LocalView::read().is_err() {
        return Verdict::ForeignUnknown(UnknownReason::IncompleteProcfs);
    }
    let root = Path::new("/proc");
    match read_stat(root, pid) {
        Ok(StatRead::Missing) => Verdict::Dead(DeadReason::Absent),
        Ok(StatRead::Found { state, .. }) if is_exited(state) => Verdict::Dead(DeadReason::Exited),
        Ok(StatRead::Found { start_ticks, .. }) => {
            if let Some(recorded) = start {
                if let Some((boot, ticks)) = crate::process_probe::parse_start_identity(recorded) {
                    if !valid_boot_id(boot) {
                        return unknown();
                    }
                    let Ok(current) = boot_id(root) else {
                        return unknown();
                    };
                    if current != boot {
                        return Verdict::Dead(DeadReason::Reboot);
                    }
                    if ticks != start_ticks {
                        return Verdict::Dead(DeadReason::Reused);
                    }
                    return Verdict::Alive;
                } else if let Some(ticks) = recorded
                    .strip_prefix("linux-starttime:")
                    .and_then(|ticks| ticks.parse::<u64>().ok())
                {
                    if ticks != start_ticks {
                        return Verdict::Dead(DeadReason::Reused);
                    }
                    return Verdict::Alive;
                } else if chrono::DateTime::parse_from_rfc3339(recorded).is_ok() {
                    // Historical wall-clock starts retain their existing comparison
                    // adapter, after local coordinate admission. Advisory only.
                    if crate::liveness::process_provably_dead(pid, Some(recorded)) {
                        return Verdict::Dead(DeadReason::Reused);
                    }
                    if crate::liveness::process_identity_matches_strict(pid, recorded) {
                        return Verdict::Alive;
                    }
                }
            }
            unknown()
        }
        Err(why) => Verdict::ForeignUnknown(UnknownReason::Unreadable(why)),
    }
}

/// Coordinate/provenance admission always precedes the numeric PID lookup.
/// The closure supplies only a process observation, never holder admission.
pub fn judge(
    id: &ProcIdentity,
    observer: &ObserverFacts<'_>,
    probe: impl FnOnce() -> Probe,
) -> Verdict {
    if !observer.same_node {
        return Verdict::ForeignUnknown(UnknownReason::OtherNode);
    }
    if id.pid == 0 || id.pid > i32::MAX as u32 || id.pidns_inode == 0 || !valid_boot_id(&id.boot_id)
    {
        return Verdict::ForeignUnknown(UnknownReason::MissingIdentity);
    }
    let Some(boot) = observer.boot_id.filter(|b| valid_boot_id(b)) else {
        return Verdict::ForeignUnknown(UnknownReason::MissingIdentity);
    };
    if boot != id.boot_id {
        return Verdict::Dead(DeadReason::Reboot);
    }
    let Some(ns) = observer.pidns_inode else {
        return Verdict::ForeignUnknown(UnknownReason::MissingIdentity);
    };
    if ns != id.pidns_inode {
        return Verdict::ForeignUnknown(UnknownReason::NamespaceMismatch);
    }
    if !observer.complete_procfs {
        return Verdict::ForeignUnknown(UnknownReason::IncompleteProcfs);
    }
    match probe() {
        Probe::Alive => Verdict::Alive,
        Probe::Dead(why) => Verdict::Dead(DeadReason::Observed(why)),
        Probe::Unknown(why) => Verdict::ForeignUnknown(UnknownReason::Unreadable(why)),
    }
}

/// Direct Linux observer. Holds namespace handles while checking coordinates.
/// No cross-namespace translation or wall clock participates in this operation.
pub fn observe(id: &ProcIdentity, same_node: bool) -> Verdict {
    if !same_node {
        return Verdict::ForeignUnknown(UnknownReason::OtherNode);
    }
    let root = Path::new("/proc");
    let boot = boot_id(root).ok();
    // Trusted reboot evidence does not depend on a currently readable PID view.
    if boot
        .as_deref()
        .is_some_and(|boot| valid_boot_id(boot) && boot != id.boot_id)
        && id.pid != 0
        && id.pid <= i32::MAX as u32
        && id.pidns_inode != 0
        && valid_boot_id(&id.boot_id)
    {
        return Verdict::Dead(DeadReason::Reboot);
    }
    let view = match LocalView::read() {
        Ok(view) => view,
        Err(why) => {
            return Verdict::ForeignUnknown(
                if why.contains("coordinates") || why.contains("masked") || why.contains("mount") {
                    UnknownReason::IncompleteProcfs
                } else {
                    UnknownReason::Unreadable(why)
                },
            )
        }
    };
    let facts = ObserverFacts {
        same_node,
        boot_id: boot.as_deref(),
        pidns_inode: Some(view.namespace),
        complete_procfs: true,
    };
    let admission = judge(id, &facts, || Probe::Alive);
    if admission != Verdict::Alive {
        return admission;
    }
    let verdict = match read_stat(root, id.pid) {
        Ok(StatRead::Missing) => Verdict::Dead(DeadReason::Absent),
        Ok(StatRead::Found { state, .. }) if is_exited(state) => Verdict::Dead(DeadReason::Exited),
        Ok(StatRead::Found { start_ticks, .. }) if start_ticks != id.starttime => {
            Verdict::Dead(DeadReason::Reused)
        }
        Ok(_) => Verdict::Alive,
        Err(e) => Verdict::ForeignUnknown(UnknownReason::Unreadable(e)),
    };
    // A namespace/procfs replacement or reboot during the read is not death evidence.
    if boot_id(root).ok() != boot
        || LocalView::read().ok().map(|v| v.namespace) != Some(id.pidns_inode)
    {
        return Verdict::ForeignUnknown(UnknownReason::IncompleteProcfs);
    }
    verdict
}

struct LocalView {
    namespace: u64,
    _self_ns: std::fs::File,
    _init_ns: Option<std::fs::File>,
}
impl LocalView {
    fn read() -> Result<Self, String> {
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::MetadataExt;
            let self_ns = std::fs::File::open("/proc/self/ns/pid").map_err(|e| e.to_string())?;
            // An unprivileged host reader may not open root-owned pid 1's
            // namespace. Self/NSpid and the procfs mount prove coordinates;
            // a readable init handle supplies an additional consistency check.
            let init_ns = std::fs::File::open("/proc/1/ns/pid").ok();
            let namespace = self_ns.metadata().map_err(|e| e.to_string())?.ino();
            if init_ns
                .as_ref()
                .is_some_and(|ns| ns.metadata().map(|m| m.ino()).ok() != Some(namespace))
            {
                return Err("procfs uses ancestor coordinates".into());
            }
            let self_pid = std::fs::read_link("/proc/self").map_err(|e| e.to_string())?;
            if self_pid.to_str().and_then(|pid| pid.parse::<u32>().ok()) != Some(std::process::id())
            {
                return Err("procfs coordinates do not match the reader".into());
            }
            let status = std::fs::read_to_string("/proc/self/status").map_err(|e| e.to_string())?;
            let pids = nspids(&status).ok_or("missing NSpid")?;
            if pids != [std::process::id()] {
                return Err("procfs coordinates do not match the reader".into());
            }
            let mounts =
                std::fs::read_to_string("/proc/self/mountinfo").map_err(|e| e.to_string())?;
            let mut proc_mount = false;
            for line in mounts.lines() {
                let fields: Vec<_> = line.split_whitespace().collect();
                let Some(path) = fields.get(4) else {
                    return Err("malformed mountinfo".into());
                };
                if *path == "/proc" {
                    proc_mount = line.contains(" - proc ")
                        && !line.contains("hidepid=")
                        && fields.get(3) == Some(&"/");
                }
                if path
                    .strip_prefix("/proc/")
                    .and_then(|p| p.split('/').next())
                    .is_some_and(|p| p == "self" || p == "thread-self" || p.parse::<u32>().is_ok())
                {
                    return Err("masked process view".into());
                }
            }
            if !proc_mount {
                return Err("unverified procfs mount".into());
            }
            Ok(Self {
                namespace,
                _self_ns: self_ns,
                _init_ns: init_ns,
            })
        }
        #[cfg(not(target_os = "linux"))]
        Err("no Linux procfs".into())
    }
}

fn nspids(status: &str) -> Option<Vec<u32>> {
    status
        .lines()
        .find_map(|l| l.strip_prefix("NSpid:"))?
        .split_whitespace()
        .map(str::parse)
        .collect::<Result<Vec<_>, _>>()
        .ok()
        .filter(|v| !v.is_empty())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HolderRefusal {
    InitOrManager,
    Subreaper,
    EvidenceUnavailable(String),
    IdentityChanged,
}

/// Persisted lifecycle assurance, not a capability. Generation changes on every
/// worker handoff; no creator witness may be combined with it. Legacy missing
/// evidence refuses admission. Private fields prevent arbitrary probe closures
/// from manufacturing admitted holders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HolderRecord {
    #[serde(default)]
    version: u8,
    #[serde(default)]
    node_id: String,
    identity: Option<ProcIdentity>,
    generation: String,
    refusal: Option<HolderRefusal>,
}
impl HolderRecord {
    pub fn matches_node(&self, node: Option<&str>) -> bool {
        node.is_some_and(|node| !node.is_empty() && node == self.node_id)
    }
    pub fn is_local_node(&self) -> bool {
        self.matches_node(local_node_id().as_deref())
    }
    pub fn observe(&self) -> Result<Verdict, HolderRefusal> {
        Ok(observe(self.validate()?, self.is_local_node()))
    }
    pub fn identity(&self) -> Option<&ProcIdentity> {
        self.identity.as_ref()
    }
    pub fn generation(&self) -> &str {
        &self.generation
    }
    pub fn refusal(&self) -> Option<&HolderRefusal> {
        self.refusal.as_ref()
    }
    pub fn refused(reason: HolderRefusal) -> Self {
        Self {
            version: 1,
            node_id: String::new(),
            identity: None,
            generation: uuid::Uuid::new_v4().to_string(),
            refusal: Some(reason),
        }
    }
    pub fn validate(&self) -> Result<&ProcIdentity, HolderRefusal> {
        if let Some(r) = &self.refusal {
            return Err(r.clone());
        }
        let id = self
            .identity
            .as_ref()
            .ok_or_else(|| HolderRefusal::EvidenceUnavailable("no holder identity".into()))?;
        if id.pid <= 1 {
            return Err(HolderRefusal::InitOrManager);
        }
        if self.version != 1
            || self.node_id.is_empty()
            || self.generation.is_empty()
            || !valid_boot_id(&id.boot_id)
            || id.pidns_inode == 0
        {
            return Err(HolderRefusal::EvidenceUnavailable(
                "incomplete holder record".into(),
            ));
        }
        Ok(id)
    }
    /// Only self can query PR_GET_CHILD_SUBREAPER. Arbitrary remote PIDs refuse.
    pub fn capture(pid: u32) -> Self {
        if pid <= 1 {
            return Self::refused(HolderRefusal::InitOrManager);
        }
        if pid != std::process::id() {
            return Self::refused(HolderRefusal::EvidenceUnavailable(
                "target has no cooperative handoff".into(),
            ));
        }
        #[cfg(target_os = "linux")]
        {
            let mut subreaper: libc::c_int = 0;
            // SAFETY: prctl writes one c_int through this live pointer.
            if unsafe { libc::prctl(libc::PR_GET_CHILD_SUBREAPER, &mut subreaper, 0, 0, 0) } != 0 {
                return Self::refused(HolderRefusal::EvidenceUnavailable(
                    "cannot query self subreaper state".into(),
                ));
            }
            if subreaper != 0 {
                return Self::refused(HolderRefusal::Subreaper);
            }
            return capture_checked(pid);
        }
        #[cfg(not(target_os = "linux"))]
        Self::refused(HolderRefusal::EvidenceUnavailable(
            "Linux capture required".into(),
        ))
    }
}

fn invalid_holder_target(pid: u32, nspids: &[u32], name: &str, executable: &str) -> bool {
    pid <= 1
        || nspids.contains(&1)
        || name == "systemd"
        || [
            "systemd",
            "init",
            "tini",
            "dumb-init",
            "docker-init",
            "catatonit",
            "runit",
            "openrc-init",
            "s6-svscan",
        ]
        .contains(&executable)
}

fn capture_checked(pid: u32) -> HolderRecord {
    let capture = || -> Result<ProcIdentity, HolderRefusal> {
        local_node_id().ok_or_else(|| {
            HolderRefusal::EvidenceUnavailable("local node identity unavailable".into())
        })?;
        let view = LocalView::read().map_err(HolderRefusal::EvidenceUnavailable)?;
        let src = crate::process_probe::ProbeSource::system();
        let facts = src.facts(pid).map_err(HolderRefusal::EvidenceUnavailable)?;
        let status = std::fs::read_to_string(format!("/proc/{pid}/status"))
            .map_err(|e| HolderRefusal::EvidenceUnavailable(e.to_string()))?;
        let pids = nspids(&status)
            .ok_or_else(|| HolderRefusal::EvidenceUnavailable("missing target NSpid".into()))?;
        let label = facts.command_label();
        if invalid_holder_target(pid, &pids, &facts.name, &label) {
            return Err(HolderRefusal::InitOrManager);
        }
        if src.pid_namespace(pid).ok() != Some(view.namespace) || pids != [pid] {
            return Err(HolderRefusal::EvidenceUnavailable(
                "target coordinates are not local".into(),
            ));
        }
        let id = ProcIdentity::from_record(&facts.identity, Some(view.namespace))
            .ok_or(HolderRefusal::IdentityChanged)?;
        if observe(&id, true) != Verdict::Alive {
            return Err(HolderRefusal::IdentityChanged);
        }
        Ok(id)
    };
    match capture() {
        Ok(identity) => HolderRecord {
            version: 1,
            node_id: local_node_id().unwrap_or_default(),
            identity: Some(identity),
            generation: uuid::Uuid::new_v4().to_string(),
            refusal: None,
        },
        Err(reason) => HolderRecord::refused(reason),
    }
}

/// Install an async-signal-safe cooperative self check immediately before exec.
/// The retained Child prevents PID reuse until capture; no env-carried PID or
/// arbitrary callback can attest a holder. Exec preserves PID/start identity.
#[cfg(target_os = "linux")]
pub struct PendingHolder {
    read: std::os::fd::OwnedFd,
}
#[cfg(target_os = "linux")]
pub fn prepare_command(command: &mut std::process::Command) -> std::io::Result<PendingHolder> {
    use std::os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::process::CommandExt,
    };
    let mut fds = [-1; 2];
    // SAFETY: pipe2 initializes both descriptors on success.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let read = unsafe { OwnedFd::from_raw_fd(fds[0]) };
    let write = unsafe { OwnedFd::from_raw_fd(fds[1]) };
    // SAFETY: child hook uses only prctl/write and stack values, no allocator/locks.
    unsafe {
        command.pre_exec(move || {
            let mut subreaper: libc::c_int = 0;
            let result = libc::prctl(libc::PR_GET_CHILD_SUBREAPER, &mut subreaper, 0, 0, 0);
            let status: u32 = if result != 0 {
                0
            } else if subreaper == 0 {
                1
            } else {
                2
            };
            let receipt = [libc::getpid() as u32, status];
            if libc::write(write.as_raw_fd(), receipt.as_ptr().cast(), 8) != 8 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(PendingHolder { read })
}
#[cfg(target_os = "linux")]
impl PendingHolder {
    pub fn finish(self, child: &mut std::process::Child) -> HolderRecord {
        use std::os::fd::AsRawFd;
        let mut receipt = [0u32; 2];
        // SAFETY: fixed-size destination is valid; pipe is nonblocking.
        let n = unsafe { libc::read(self.read.as_raw_fd(), receipt.as_mut_ptr().cast(), 8) };
        if n != 8 || receipt[0] != child.id() || receipt[1] == 0 {
            return HolderRecord::refused(HolderRefusal::EvidenceUnavailable(
                "child supplied no identity-bound receipt".into(),
            ));
        }
        if receipt[1] == 2 {
            return HolderRecord::refused(HolderRefusal::Subreaper);
        }
        if !matches!(child.try_wait(), Ok(None)) {
            return HolderRecord::refused(HolderRefusal::IdentityChanged);
        }
        let record = capture_checked(child.id());
        if !matches!(child.try_wait(), Ok(None)) {
            return HolderRecord::refused(HolderRefusal::IdentityChanged);
        }
        record
    }
}

// The host-provided machine identity supplies local provenance, not authorization.
pub fn local_node_id() -> Option<String> {
    std::fs::read_to_string("/etc/machine-id")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// Validate the same selected worker in every lease reader and ownership writer.
// trace:STORY-2537 trace:BUG-2458 | ai:codex
pub fn recorded_holder(
    raw: &str,
    pid: Option<u32>,
    start: Option<&str>,
    namespace: Option<u64>,
) -> Result<HolderRecord, HolderRefusal> {
    let record: HolderRecord = serde_json::from_str(raw)
        .map_err(|e| HolderRefusal::EvidenceUnavailable(format!("invalid holder record: {e}")))?;
    let id = record.validate()?;
    if pid != Some(id.pid)
        || start != Some(format!("linux-starttime:{}", id.starttime).as_str())
        || namespace != Some(id.pidns_inode)
    {
        return Err(HolderRefusal::IdentityChanged);
    }
    Ok(record)
}

#[cfg(test)]
#[path = "proc_identity_tests.rs"]
mod tests;
