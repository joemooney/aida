//! The activation dry-run report (QC1, STORY-1843 C2).
//!
//! A mail-bearing source refuses retirement until an operator records that
//! every writer host on the machine admits before it writes. This module
//! ships the first half only: a read-only report of the AIDA processes that
//! can write local mail and whether each is known to take a write permit. It
//! records nothing.
//!
//! A host is *known to admit* when it runs the same executable file (device
//! and inode) as the process producing the report, which is a build with
//! admission. Any other build, a replaced (deleted) binary, or a process
//! whose executable cannot be read is *unverified*.
//!
//! trace:STORY-1843 | ai:claude

use serde::Serialize;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// Executable names that can write local mail: the CLI (which also serves
/// MCP over stdio) and the REST server.
const WRITER_EXECUTABLES: &[&str] = &["aida", "aida-server"];

/// One running process as the report sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessView {
    pub pid: u32,
    /// The `/proc/<pid>/exe` link target, with any ` (deleted)` suffix.
    pub exe: Option<String>,
    /// Device and inode of the executable the process runs.
    pub exe_id: Option<(u64, u64)>,
    pub cmdline: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HostVerdict {
    Admitting,
    Unverified { why: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WriterHost {
    pub pid: u32,
    pub exe: String,
    pub command: String,
    pub verdict: HostVerdict,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActivationReport {
    pub dry_run: bool,
    /// An activation is already recorded under this repository's anchor.
    pub recorded: bool,
    pub hosts: Vec<WriterHost>,
    /// Why the process scan itself is incomplete, if it is.
    pub scan_incomplete: Option<String>,
    /// Every writer host is known to admit and the scan was complete.
    pub would_record: bool,
}

/// Classify `processes` against this process's executable `me`. Pure.
pub fn classify(
    processes: &[ProcessView],
    me: Option<(u64, u64)>,
    self_pid: u32,
    scan_incomplete: Option<String>,
    recorded: bool,
) -> ActivationReport {
    let mut hosts = Vec::new();
    for p in processes {
        let Some(exe) = &p.exe else { continue };
        let deleted = exe.ends_with(" (deleted)");
        let path = exe.trim_end_matches(" (deleted)");
        let name = Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if !WRITER_EXECUTABLES.contains(&name) {
            continue;
        }
        let verdict = if p.pid == self_pid {
            HostVerdict::Admitting
        } else if deleted {
            HostVerdict::Unverified {
                why: "it runs a binary that has since been replaced".into(),
            }
        } else {
            match (p.exe_id, me) {
                (Some(id), Some(me)) if id == me => HostVerdict::Admitting,
                (Some(_), Some(_)) => HostVerdict::Unverified {
                    why: "it runs a different build than this one".into(),
                },
                _ => HostVerdict::Unverified {
                    why: "its executable could not be identified".into(),
                },
            }
        };
        hosts.push(WriterHost {
            pid: p.pid,
            exe: exe.clone(),
            command: p
                .cmdline
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join(" "),
            verdict,
        });
    }
    hosts.sort_by_key(|h| h.pid);
    let would_record = scan_incomplete.is_none()
        && me.is_some()
        && hosts.iter().all(|h| h.verdict == HostVerdict::Admitting);
    ActivationReport {
        dry_run: true,
        recorded,
        hosts,
        scan_incomplete,
        would_record,
    }
}

/// Scan this machine's processes and report, read-only. `project_root`
/// selects the anchor whose recorded activation is shown.
pub fn dry_run_report(project_root: &Path) -> ActivationReport {
    let me = std::fs::metadata("/proc/self/exe")
        .ok()
        .map(|m| (m.dev(), m.ino()));
    let (processes, incomplete) = scan_proc(Path::new("/proc"));
    let recorded = super::resolve_source(project_root)
        .ok()
        .flatten()
        .is_some_and(|source| super::activated(&source.anchor).unwrap_or(false));
    classify(&processes, me, std::process::id(), incomplete, recorded)
}

/// Read every `/proc/<pid>` this user can see. A process that exits during
/// the scan is skipped; one whose `exe` link is unreadable (another user's,
/// or hidepid) is listed with no executable and marks the scan incomplete
/// only if its command line names an AIDA writer.
fn scan_proc(proc: &Path) -> (Vec<ProcessView>, Option<String>) {
    let entries = match std::fs::read_dir(proc) {
        Ok(entries) => entries,
        Err(e) => {
            return (
                Vec::new(),
                Some(format!("{} is unreadable: {e}", proc.display())),
            )
        }
    };
    let mut out = Vec::new();
    let mut incomplete = None;
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        let dir: PathBuf = entry.path();
        let cmdline: Vec<String> = match std::fs::read(dir.join("cmdline")) {
            Ok(raw) => raw
                .split(|b| *b == 0)
                .filter(|a| !a.is_empty())
                .map(|a| String::from_utf8_lossy(a).into_owned())
                .collect(),
            Err(_) => continue,
        };
        let exe = std::fs::read_link(dir.join("exe"))
            .ok()
            .map(|p| p.to_string_lossy().into_owned());
        if exe.is_none() {
            let names_writer = cmdline.first().is_some_and(|a| {
                Path::new(a)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| WRITER_EXECUTABLES.contains(&n))
            });
            if names_writer {
                incomplete.get_or_insert(format!(
                    "process {pid} looks like an AIDA writer but its executable is unreadable"
                ));
            }
            continue;
        }
        let exe_id = std::fs::metadata(dir.join("exe"))
            .ok()
            .map(|m| (m.dev(), m.ino()));
        out.push(ProcessView {
            pid,
            exe,
            exe_id,
            cmdline,
        });
    }
    (out, incomplete)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(pid: u32, exe: &str, id: (u64, u64)) -> ProcessView {
        ProcessView {
            pid,
            exe: Some(exe.into()),
            exe_id: Some(id),
            cmdline: vec![exe.into(), "mcp".into()],
        }
    }

    #[test]
    fn hosts_on_this_build_admit_and_others_are_unverified() {
        let me = (1, 10);
        let procs = vec![
            view(5, "/home/u/.local/bin/aida", me),
            view(6, "/home/u/.local/bin/aida-server", (1, 11)),
            view(7, "/home/u/.local/bin/aida (deleted)", me),
            view(8, "/usr/bin/bash", (1, 12)),
        ];
        let report = classify(&procs, Some(me), 1, None, false);
        let verdicts: Vec<_> = report.hosts.iter().map(|h| (h.pid, &h.verdict)).collect();
        assert_eq!(verdicts.len(), 3, "bash is not a writer host");
        assert_eq!(verdicts[0], (5, &HostVerdict::Admitting));
        assert!(
            matches!(verdicts[1].1, HostVerdict::Unverified { why } if why.contains("different build"))
        );
        assert!(
            matches!(verdicts[2].1, HostVerdict::Unverified { why } if why.contains("replaced"))
        );
        assert!(!report.would_record);
        assert!(report.dry_run);
    }

    #[test]
    fn only_a_complete_scan_of_admitting_hosts_would_record() {
        let me = (1, 10);
        let procs = vec![view(5, "/bin/aida", me)];
        assert!(classify(&procs, Some(me), 1, None, false).would_record);
        assert!(!classify(&procs, Some(me), 1, Some("hidepid".into()), false).would_record);
        assert!(!classify(&procs, None, 1, None, false).would_record);
    }
}
