//! Tri-state process identity probe for seat occupancy (TASK-1607).
//!
//! The older liveness helpers answer a boolean: `pid_is_alive` maps a probe
//! error to `false` and `process_identity_is_alive` maps a missing start time
//! to `true`. Neither is evidence strong enough to hand a seat to someone else
//! or to keep one fenced, so this module answers [`Probe::Alive`],
//! [`Probe::Dead`] or [`Probe::Unknown`] and always keeps the reason.
//!
//! A [`ProcessIdentity`] is a PID plus a start identity that cannot be shared
//! by two processes: on Linux, the boot ID and the process's start time in
//! clock ticks since boot (`/proc/<pid>/stat` field 22). A recycled PID has a
//! different start tick, and a reboot changes the boot ID.
//!
//! Platforms without a probe return `Unknown` (ADR-69: fail closed). The
//! diagnosis names the platform, the missing evidence and the follow-up spec.
//!
//! trace:TASK-1607 trace:ADR-67 trace:ADR-69 | ai:claude

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::liveness::{harness_kind, AgentKind};

/// A process, named so that a recycled PID never matches.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub pid: u32,
    /// `linux:<boot-id>:<start-ticks>`. Opaque to callers; compare whole.
    pub start: String,
}

impl std::fmt::Display for ProcessIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "pid {}", self.pid)
    }
}

/// The answer to "is this recorded process still running?".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    /// Positive evidence: the PID exists and its start identity matches.
    Alive,
    /// Positive evidence the recorded process has ended (gone, zombie, PID
    /// reused, or the host rebooted).
    Dead(String),
    /// No proof either way. Callers must refuse rather than guess.
    Unknown(String),
}

impl Probe {
    pub fn label(&self) -> &'static str {
        match self {
            Probe::Alive => "alive",
            Probe::Dead(_) => "dead",
            Probe::Unknown(_) => "unknown",
        }
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Probe::Alive => None,
            Probe::Dead(r) | Probe::Unknown(r) => Some(r),
        }
    }
}

/// One process in an ancestry walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcFacts {
    pub identity: ProcessIdentity,
    pub ppid: u32,
    pub name: String,
    pub cmd: Vec<String>,
}

impl ProcFacts {
    pub fn harness(&self) -> Option<AgentKind> {
        harness_kind(&self.name, &self.cmd)
    }

    /// Short human label: the executable's basename, else the comm name.
    pub fn command_label(&self) -> String {
        self.cmd
            .first()
            .and_then(|a| Path::new(a).file_name())
            .and_then(|s| s.to_str())
            .map(str::to_string)
            .unwrap_or_else(|| self.name.clone())
    }
}

/// Where process facts come from. `Proc` reads a procfs root (the real
/// `/proc`, or a fixture directory in tests); `Unsupported` is every platform
/// without a probe.
#[derive(Debug, Clone)]
pub enum ProbeSource {
    Proc(PathBuf),
    Unsupported(&'static str),
}

/// Ancestry walks stop here even if the parent chain is malformed.
const MAX_ANCESTRY: usize = 128;

enum StatRead {
    Found {
        state: char,
        ppid: u32,
        start_ticks: u64,
        name: String,
    },
    Missing,
}

impl ProbeSource {
    /// The probe for the running host.
    pub fn system() -> Self {
        if cfg!(target_os = "linux") {
            ProbeSource::Proc(PathBuf::from("/proc"))
        } else {
            ProbeSource::Unsupported(std::env::consts::OS)
        }
    }

    /// A probe over a fixture procfs tree (tests).
    pub fn at(root: impl Into<PathBuf>) -> Self {
        ProbeSource::Proc(root.into())
    }

    /// Capture the identity of a live process. `Err` carries the reason the
    /// identity could not be established; callers treat it as Unknown.
    pub fn capture(&self, pid: u32) -> Result<ProcessIdentity, String> {
        self.facts(pid).map(|f| f.identity)
    }

    /// Identity, parent, name and argv of one live process.
    pub fn facts(&self, pid: u32) -> Result<ProcFacts, String> {
        let root = self.root()?;
        let boot = boot_id(root)?;
        match read_stat(root, pid)? {
            StatRead::Missing => Err(format!("process {pid} does not exist")),
            StatRead::Found { state, .. } if is_exited(state) => {
                Err(format!("process {pid} has exited"))
            }
            StatRead::Found {
                ppid,
                start_ticks,
                name,
                ..
            } => Ok(ProcFacts {
                identity: ProcessIdentity {
                    pid,
                    start: format!("linux:{boot}:{start_ticks}"),
                },
                ppid,
                name,
                cmd: read_cmdline(root, pid),
            }),
        }
    }

    /// Is the recorded process still the running one?
    pub fn probe(&self, id: &ProcessIdentity) -> Probe {
        let root = match self.root() {
            Ok(root) => root,
            Err(why) => return Probe::Unknown(why),
        };
        let Some((rec_boot, rec_ticks)) = parse_start(&id.start) else {
            return Probe::Unknown(format!(
                "the recorded identity for pid {} is not a Linux start identity",
                id.pid
            ));
        };
        let boot = match boot_id(root) {
            Ok(boot) => boot,
            Err(why) => return Probe::Unknown(why),
        };
        if boot != rec_boot {
            return Probe::Dead(format!(
                "the host rebooted since pid {} was recorded",
                id.pid
            ));
        }
        match read_stat(root, id.pid) {
            Err(why) => Probe::Unknown(why),
            Ok(StatRead::Missing) => Probe::Dead(format!("process {} no longer exists", id.pid)),
            Ok(StatRead::Found { state, .. }) if is_exited(state) => {
                Probe::Dead(format!("process {} has exited", id.pid))
            }
            Ok(StatRead::Found { start_ticks, .. }) if start_ticks != rec_ticks => {
                Probe::Dead(format!("pid {} now belongs to a different process", id.pid))
            }
            Ok(StatRead::Found { .. }) => Probe::Alive,
        }
    }

    /// `pid` and each of its ancestors, nearest first. Any unreadable link
    /// fails the whole walk: a partial chain could hide a fenced ancestor.
    pub fn ancestry(&self, pid: u32) -> Result<Vec<ProcFacts>, String> {
        let mut chain = Vec::new();
        let mut next = pid;
        while next != 0 {
            if chain.len() >= MAX_ANCESTRY {
                return Err(format!(
                    "process ancestry of pid {pid} is deeper than {MAX_ANCESTRY}"
                ));
            }
            let facts = self.facts(next)?;
            next = if facts.ppid == next { 0 } else { facts.ppid };
            chain.push(facts);
        }
        Ok(chain)
    }

    fn root(&self) -> Result<&Path, String> {
        match self {
            ProbeSource::Proc(root) => Ok(root),
            ProbeSource::Unsupported(os) => Err(unsupported_reason(os)),
        }
    }
}

/// ADR-69 refusal text: platform, missing evidence, follow-up spec.
pub fn unsupported_reason(os: &str) -> String {
    let follow_up = match os {
        "macos" => " (macOS probe: TASK-1609)",
        "windows" => " (Windows probe: TASK-1610)",
        _ => "",
    };
    format!(
        "seat occupancy cannot verify process identity on {os}: no process start-time probe exists for this platform yet{follow_up}"
    )
}

fn is_exited(state: char) -> bool {
    matches!(state, 'Z' | 'X' | 'x')
}

fn parse_start(start: &str) -> Option<(&str, u64)> {
    let rest = start.strip_prefix("linux:")?;
    let (boot, ticks) = rest.rsplit_once(':')?;
    Some((boot, ticks.parse().ok()?))
}

fn boot_id(root: &Path) -> Result<String, String> {
    let path = root.join("sys/kernel/random/boot_id");
    std::fs::read_to_string(&path)
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("cannot read the host boot id from {}", path.display()))
}

fn read_stat(root: &Path, pid: u32) -> Result<StatRead, String> {
    let path = root.join(pid.to_string()).join("stat");
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(StatRead::Missing),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    let malformed = || format!("malformed {}", path.display());
    let open = raw.find('(').ok_or_else(malformed)?;
    let close = raw.rfind(')').ok_or_else(malformed)?;
    if close < open {
        return Err(malformed());
    }
    let name = raw[open + 1..close].to_string();
    let fields: Vec<&str> = raw[close + 1..].split_whitespace().collect();
    // After the comm: [0]=state (field 3), [1]=ppid (field 4), [19]=starttime (field 22).
    let state = fields
        .first()
        .and_then(|s| s.chars().next())
        .ok_or_else(malformed)?;
    let ppid = fields
        .get(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(malformed)?;
    let start_ticks = fields
        .get(19)
        .and_then(|s| s.parse().ok())
        .ok_or_else(malformed)?;
    Ok(StatRead::Found {
        state,
        ppid,
        start_ticks,
        name,
    })
}

fn read_cmdline(root: &Path, pid: u32) -> Vec<String> {
    std::fs::read(root.join(pid.to_string()).join("cmdline"))
        .map(|bytes| {
            bytes
                .split(|b| *b == 0)
                .filter(|a| !a.is_empty())
                .map(|a| String::from_utf8_lossy(a).into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// Fixture procfs builder shared by this crate's and the CLI's tests.
#[doc(hidden)]
pub mod fixture {
    use std::path::{Path, PathBuf};

    pub struct FakeProc {
        pub root: PathBuf,
    }

    impl FakeProc {
        pub fn new(root: &Path, boot_id: &str) -> Self {
            let random = root.join("sys/kernel/random");
            std::fs::create_dir_all(&random).unwrap();
            std::fs::write(random.join("boot_id"), format!("{boot_id}\n")).unwrap();
            FakeProc {
                root: root.to_path_buf(),
            }
        }

        /// Add (or replace) a process entry.
        pub fn add(
            &self,
            pid: u32,
            ppid: u32,
            name: &str,
            state: char,
            start_ticks: u64,
            cmd: &[&str],
        ) {
            let dir = self.root.join(pid.to_string());
            std::fs::create_dir_all(&dir).unwrap();
            // Fields 5..21 are irrelevant to the probe; fill them with zeros.
            let filler = vec!["0"; 17].join(" ");
            std::fs::write(
                dir.join("stat"),
                format!("{pid} ({name}) {state} {ppid} {filler} {start_ticks} 0 0\n"),
            )
            .unwrap();
            let mut argv = Vec::new();
            for arg in cmd {
                argv.extend_from_slice(arg.as_bytes());
                argv.push(0);
            }
            std::fs::write(dir.join("cmdline"), argv).unwrap();
        }

        pub fn remove(&self, pid: u32) {
            let _ = std::fs::remove_dir_all(self.root.join(pid.to_string()));
        }

        pub fn set_boot_id(&self, boot_id: &str) {
            std::fs::write(self.root.join("sys/kernel/random/boot_id"), boot_id).unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::FakeProc;
    use super::*;

    fn fake() -> (tempfile::TempDir, FakeProc, ProbeSource) {
        let dir = tempfile::tempdir().unwrap();
        let proc = FakeProc::new(dir.path(), "boot-a");
        let source = ProbeSource::at(dir.path());
        (dir, proc, source)
    }

    #[test]
    fn live_process_with_matching_start_is_alive() {
        let (_d, proc, src) = fake();
        proc.add(42, 1, "claude", 'S', 1000, &["claude"]);
        let id = src.capture(42).unwrap();
        assert_eq!(id.start, "linux:boot-a:1000");
        assert_eq!(src.probe(&id), Probe::Alive);
    }

    #[test]
    fn missing_pid_is_dead() {
        let (_d, proc, src) = fake();
        proc.add(42, 1, "claude", 'S', 1000, &[]);
        let id = src.capture(42).unwrap();
        proc.remove(42);
        assert!(matches!(src.probe(&id), Probe::Dead(_)));
    }

    #[test]
    fn reused_pid_is_dead_not_alive() {
        let (_d, proc, src) = fake();
        proc.add(42, 1, "claude", 'S', 1000, &[]);
        let id = src.capture(42).unwrap();
        proc.add(42, 1, "bash", 'S', 2000, &[]);
        let probe = src.probe(&id);
        assert!(
            matches!(&probe, Probe::Dead(r) if r.contains("different process")),
            "{probe:?}"
        );
    }

    #[test]
    fn zombie_is_dead() {
        let (_d, proc, src) = fake();
        proc.add(42, 1, "claude", 'S', 1000, &[]);
        let id = src.capture(42).unwrap();
        proc.add(42, 1, "claude", 'Z', 1000, &[]);
        assert!(matches!(src.probe(&id), Probe::Dead(_)));
    }

    #[test]
    fn reboot_is_dead() {
        let (_d, proc, src) = fake();
        proc.add(42, 1, "claude", 'S', 1000, &[]);
        let id = src.capture(42).unwrap();
        proc.set_boot_id("boot-b");
        assert!(matches!(src.probe(&id), Probe::Dead(_)));
    }

    #[test]
    fn missing_identity_is_unknown_not_alive() {
        let (_d, proc, src) = fake();
        proc.add(42, 1, "claude", 'S', 1000, &[]);
        let id = ProcessIdentity {
            pid: 42,
            start: String::new(),
        };
        assert!(matches!(src.probe(&id), Probe::Unknown(_)));
    }

    #[test]
    fn unreadable_boot_id_is_unknown() {
        let (dir, proc, src) = fake();
        proc.add(42, 1, "claude", 'S', 1000, &[]);
        let id = src.capture(42).unwrap();
        std::fs::remove_file(dir.path().join("sys/kernel/random/boot_id")).unwrap();
        assert!(matches!(src.probe(&id), Probe::Unknown(_)));
        assert!(src.capture(42).is_err());
    }

    #[test]
    fn malformed_stat_is_unknown_not_dead() {
        let (dir, proc, src) = fake();
        proc.add(42, 1, "claude", 'S', 1000, &[]);
        let id = src.capture(42).unwrap();
        std::fs::write(dir.path().join("42/stat"), "garbage").unwrap();
        assert!(matches!(src.probe(&id), Probe::Unknown(_)));
    }

    #[test]
    fn unsupported_platform_is_unknown_and_cites_follow_up() {
        let src = ProbeSource::Unsupported("macos");
        let id = ProcessIdentity {
            pid: 1,
            start: "linux:x:1".into(),
        };
        let probe = src.probe(&id);
        assert!(
            matches!(&probe, Probe::Unknown(r) if r.contains("macos") && r.contains("TASK-1609")),
            "{probe:?}"
        );
        assert!(ProbeSource::Unsupported("windows")
            .capture(1)
            .unwrap_err()
            .contains("TASK-1610"));
    }

    #[test]
    fn ancestry_walks_to_root_and_classifies_harness() {
        let (_d, proc, src) = fake();
        proc.add(1, 0, "systemd", 'S', 1, &["/sbin/init"]);
        proc.add(10, 1, "claude", 'S', 50, &["claude"]);
        proc.add(20, 10, "bash", 'S', 60, &["/bin/bash"]);
        proc.add(30, 20, "aida", 'R', 70, &["aida", "queue", "add"]);
        let chain = src.ancestry(30).unwrap();
        let pids: Vec<u32> = chain.iter().map(|f| f.identity.pid).collect();
        assert_eq!(pids, vec![30, 20, 10, 1]);
        assert_eq!(chain[2].harness(), Some(AgentKind::Claude));
        assert_eq!(chain[0].harness(), None);
    }

    #[test]
    fn ancestry_with_a_vanished_link_fails() {
        let (_d, proc, src) = fake();
        proc.add(30, 20, "aida", 'R', 70, &["aida"]);
        assert!(src.ancestry(30).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn real_proc_probe_sees_this_process_alive() {
        let src = ProbeSource::system();
        let id = src.capture(std::process::id()).unwrap();
        assert_eq!(src.probe(&id), Probe::Alive);
        assert!(!src.ancestry(std::process::id()).unwrap().is_empty());
    }
}
