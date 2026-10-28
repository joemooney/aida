//! The activation report and record (QC1, STORY-1843 C2).
//!
//! A mail-bearing source refuses retirement until an operator records that
//! every writer host on the machine admits before it writes. The dry run is a
//! read-only report of the AIDA processes that can write local mail and
//! whether each is known to take a write permit; [`record`] re-runs that scan
//! and writes `activation.json` only when the report would record. The CLI
//! lets only a human operator at a terminal record.
//!
//! A host is *known to admit* when it runs the same build as the process
//! producing the report, which is a build with admission: the same executable
//! file (device and inode), or a byte-identical copy of it at another path
//! (the same SHA-256 content digest), such as `target/agent/aida` and the
//! `aida dev build` promoted copy under `~/.local/share/aida/live/`. Any
//! other build, a replaced (deleted) binary, or a process whose executable
//! cannot be read or digested is *unverified*. The record names the
//! source build that wrote it (version and git SHA, shared by the `aida` and
//! `aida-server` built from one checkout and by a re-promoted build of the
//! same source), and only that source build honours it: after an upgrade or
//! `aida dev rollback` to other source, that build records its own.
//!
//! trace:STORY-1843 | ai:claude

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
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
    /// SHA-256 of the executable's bytes, filled only for writer hosts whose
    /// file differs from this process's; `None` when unread or unreadable.
    pub exe_digest: Option<String>,
    pub cmdline: Vec<String>,
}

/// The build this process runs, as [`classify`] compares writer hosts to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildIdentity {
    /// Device and inode of this process's executable.
    pub exe_id: (u64, u64),
    /// SHA-256 of its bytes, or `None` when it could not be read.
    pub digest: Option<String>,
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
    /// An activation is already recorded under this repository's anchor,
    /// for this source build.
    pub recorded: bool,
    /// An activation is recorded, but by another source build (its identity,
    /// or `"unnamed"` for a record that names none), so this build does not
    /// honour it until it is recorded again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recorded_by_other_build: Option<String>,
    /// This process's source build, the identity a record now would name.
    pub this_build: Option<String>,
    pub hosts: Vec<WriterHost>,
    /// Why the process scan itself is incomplete, if it is.
    pub scan_incomplete: Option<String>,
    /// Every writer host is known to admit and the scan was complete.
    pub would_record: bool,
}

/// Whether `p` runs the build `me` runs: the same executable file, or a
/// byte-identical copy at another path. `None` means unknown (the digest of
/// either side could not be read), which refuses like a different build.
// trace:BUG-b6b1278890774ef993d2ebc327862470-010 | ai:claude
fn same_build(p: &ProcessView, me: &BuildIdentity) -> Option<bool> {
    let id = p.exe_id?;
    if id == me.exe_id {
        return Some(true);
    }
    match (&p.exe_digest, &me.digest) {
        (Some(theirs), Some(mine)) => Some(theirs == mine),
        _ => None,
    }
}

/// Classify `processes` against this process's build `me`. Pure.
pub fn classify(
    processes: &[ProcessView],
    me: Option<&BuildIdentity>,
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
            match me.and_then(|me| same_build(p, me)) {
                Some(true) => HostVerdict::Admitting,
                Some(false) => HostVerdict::Unverified {
                    why: "it runs a different build than this one".into(),
                },
                None => HostVerdict::Unverified {
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
        recorded_by_other_build: None,
        this_build: super::source_build().map(str::to_string),
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
        .map(|m| BuildIdentity {
            exe_id: (m.dev(), m.ino()),
            digest: None,
        });
    let (mut processes, incomplete) = scan_proc(Path::new("/proc"), std::process::id());
    let me = me.map(|me| fill_digests(&mut processes, me, Path::new("/proc/self/exe")));
    let state = super::resolve_source(project_root)
        .ok()
        .flatten()
        .and_then(|source| super::activation_state(&source.anchor).ok());
    let recorded = state == Some(super::ActivationState::Current);
    let mut report = classify(
        &processes,
        me.as_ref(),
        std::process::id(),
        incomplete,
        recorded,
    );
    if let Some(super::ActivationState::OtherBuild { recorded, .. }) = state {
        report.recorded_by_other_build = Some(recorded.unwrap_or_else(|| "unnamed".into()));
    }
    report
}

/// Record the activation under `project_root`'s anchor. A fresh scan runs
/// first, and nothing is written unless it shows every writer host admitting
/// with a complete scan; the returned report says which (`recorded`). The
/// caller has established that a human operator asked for this.
// trace:STORY-1843 | ai:claude
pub fn record(project_root: &Path) -> super::Result<ActivationReport> {
    let report = dry_run_report(project_root);
    let source = super::resolve_source(project_root)?.ok_or_else(|| {
        super::LifecycleError::Io(std::io::Error::other(
            "this checkout has no mail anchor, so no activation can be recorded",
        ))
    })?;
    record_at(&source.anchor, report, &chrono::Utc::now().to_rfc3339())
}

/// [`record`] against an already-made report: write the record under the
/// Lifecycle lock when `report.would_record`, else leave the anchor as is.
pub(crate) fn record_at(
    anchor: &Path,
    mut report: ActivationReport,
    now: &str,
) -> super::Result<ActivationReport> {
    report.dry_run = false;
    // A build that cannot name its source records nothing it would honour.
    let Some(source_build) = super::source_build() else {
        report.would_record = false;
        return Ok(report);
    };
    if !report.would_record {
        return Ok(report);
    }
    let _held = super::lock(anchor, super::LIFECYCLE_LOCK_TIMEOUT)?;
    super::write_json(
        &anchor.join(super::ACTIVATION_FILE),
        &super::Activation {
            schema: super::SCHEMA,
            all_writer_hosts_admitting: true,
            recorded_at: now.to_string(),
            source_build: Some(source_build.to_string()),
        },
    )?;
    report.recorded = true;
    report.recorded_by_other_build = None;
    Ok(report)
}

/// Digest the executables of writer hosts that run a file other than this
/// process's `me` (read from `self_exe`), each distinct file once, so a
/// byte-identical copy at another path is recognised as this build. Nothing
/// is read when every writer shares this process's file. An unreadable file
/// keeps no digest, which [`classify`] reports as unidentified (fail closed).
// trace:BUG-b6b1278890774ef993d2ebc327862470-010 | ai:claude
fn fill_digests(
    processes: &mut [ProcessView],
    mut me: BuildIdentity,
    self_exe: &Path,
) -> BuildIdentity {
    let mut digests: HashMap<(u64, u64), Option<String>> = HashMap::new();
    for p in processes.iter_mut() {
        let Some(id) = p.exe_id else { continue };
        let writer = p.exe.as_deref().is_some_and(|exe| {
            !exe.ends_with(" (deleted)")
                && Path::new(exe)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| WRITER_EXECUTABLES.contains(&n))
        });
        if !writer || id == me.exe_id {
            continue;
        }
        if me.digest.is_none() {
            me.digest = digest_file(self_exe);
        }
        let exe_link = Path::new("/proc").join(p.pid.to_string()).join("exe");
        p.exe_digest = digests
            .entry(id)
            .or_insert_with(|| digest_file(&exe_link))
            .clone();
    }
    me
}

/// SHA-256 of a file's bytes, hex-encoded, or `None` when it cannot be read.
fn digest_file(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).ok()?;
    Some(format!("{:x}", hasher.finalize()))
}

/// Read every `/proc/<pid>` this user can see. Unknown means refuse
/// (signoff 48 Q2), so the scan is marked incomplete, and nothing would be
/// recorded, when this process runs in a separate pid namespace (writers
/// outside it are hidden; [`crate::worktree_holders::TRUST_PID_NAMESPACE_ENV`]
/// does not waive this, since activation covers the whole machine), when it
/// cannot see its own pid (a `/proc` of another namespace or a restricted
/// mount), when a command line cannot be read for any reason but the process
/// having exited, or when a process whose `exe` link is unreadable names an
/// AIDA writer. A process that exits during the scan is skipped.
// trace:STORY-1843 | ai:claude
fn scan_proc(proc: &Path, self_pid: u32) -> (Vec<ProcessView>, Option<String>) {
    let entries = match std::fs::read_dir(proc) {
        Ok(entries) => entries,
        Err(e) => {
            return (
                Vec::new(),
                Some(format!("{} is unreadable: {e}", proc.display())),
            )
        }
    };
    let mut incomplete = crate::worktree_holders::separate_pid_namespace(proc, self_pid);
    if !proc.join(self_pid.to_string()).exists() {
        incomplete.get_or_insert(format!(
            "this process cannot see itself in {}, so other processes may be hidden",
            proc.display()
        ));
    }
    let mut out = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                incomplete.get_or_insert(format!("{} could not be listed: {e}", proc.display()));
                continue;
            }
        };
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
            Err(e) if exited(&e) => continue,
            Err(e) => {
                incomplete.get_or_insert(format!(
                    "the command line of process {pid} could not be read: {e}"
                ));
                continue;
            }
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
            exe_digest: None,
            cmdline,
        });
    }
    (out, incomplete)
}

/// A read error that means the process exited during the scan.
fn exited(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::NotFound || e.raw_os_error() == Some(3 /* ESRCH */)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(pid: u32, exe: &str, id: (u64, u64)) -> ProcessView {
        ProcessView {
            pid,
            exe: Some(exe.into()),
            exe_id: Some(id),
            exe_digest: None,
            cmdline: vec![exe.into(), "mcp".into()],
        }
    }

    fn ident(id: (u64, u64)) -> BuildIdentity {
        BuildIdentity {
            exe_id: id,
            digest: None,
        }
    }

    #[test]
    fn hosts_on_this_build_admit_and_others_are_unverified() {
        let me = (1, 10);
        let mut other = view(6, "/home/u/.local/bin/aida-server", (1, 11));
        other.exe_digest = Some("b".into());
        let procs = vec![
            view(5, "/home/u/.local/bin/aida", me),
            other,
            view(7, "/home/u/.local/bin/aida (deleted)", me),
            view(8, "/usr/bin/bash", (1, 12)),
        ];
        let me = BuildIdentity {
            exe_id: me,
            digest: Some("a".into()),
        };
        let report = classify(&procs, Some(&me), 1, None, false);
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

    /// BUG-010: the dev build at `target/agent/aida` and its promoted copy
    /// under `live/` are two files with one content; each admits the other.
    /// A copy with different bytes is a different build, and a host whose
    /// executable cannot be digested stays unidentified (F7: fail closed).
    // trace:BUG-b6b1278890774ef993d2ebc327862470-010 | ai:claude
    #[test]
    fn identical_copies_at_two_paths_admit_and_different_bytes_refuse() {
        let dir = tempfile::tempdir().unwrap();
        let dev = dir.path().join("target/agent/aida");
        let live = dir.path().join("live/aida");
        let other = dir.path().join("other/aida");
        for (path, bytes) in [
            (&dev, b"build-a"),
            (&live, b"build-a"),
            (&other, b"build-b"),
        ] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        let ino = |p: &Path| {
            let m = std::fs::metadata(p).unwrap();
            (m.dev(), m.ino())
        };
        let me = BuildIdentity {
            exe_id: ino(&dev),
            digest: digest_file(&dev),
        };
        let host = |pid, path: &Path, digest| ProcessView {
            exe_digest: digest,
            ..view(pid, path.to_str().unwrap(), ino(path))
        };
        assert_ne!(me.exe_id, ino(&live), "two files, not one");

        let procs = vec![
            host(5, &live, digest_file(&live)),
            host(6, &other, digest_file(&other)),
            host(7, &live, None),
        ];
        let report = classify(&procs, Some(&me), 1, None, false);
        let verdicts: Vec<_> = report.hosts.iter().map(|h| &h.verdict).collect();
        assert_eq!(verdicts[0], &HostVerdict::Admitting, "an identical copy");
        assert!(
            matches!(verdicts[1], HostVerdict::Unverified { why } if why.contains("different build"))
        );
        assert!(
            matches!(verdicts[2], HostVerdict::Unverified { why } if why.contains("could not be identified"))
        );
        assert!(!report.would_record);

        let copy_only = classify(&procs[..1], Some(&me), 1, None, false);
        assert!(copy_only.would_record, "{copy_only:?}");

        // This process's own digest unknown: unknown, never admitted.
        let blind = ident(me.exe_id);
        let report = classify(&procs[..1], Some(&blind), 1, None, false);
        assert!(!report.would_record);
    }

    #[test]
    fn fill_digests_reads_only_writers_on_another_file() {
        let me = ident((1, 10));
        let mut procs = vec![
            view(5, "/x/aida", (1, 10)),
            view(u32::MAX, "/x/aida", (1, 11)),
            view(7, "/usr/bin/bash", (1, 12)),
        ];
        let self_exe = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(self_exe.path(), b"build-a").unwrap();
        let me = fill_digests(&mut procs, me, self_exe.path());
        assert_eq!(me.digest, digest_file(self_exe.path()));
        assert_eq!(procs[0].exe_digest, None, "this process's own file");
        assert_eq!(procs[1].exe_digest, None, "no such process: unreadable");
        assert_eq!(procs[2].exe_digest, None, "not a writer");
    }

    fn fake_proc(self_pid: u32, ns: &str) -> tempfile::TempDir {
        let proc = tempfile::tempdir().unwrap();
        let me = proc.path().join(self_pid.to_string());
        std::fs::create_dir_all(me.join("ns")).unwrap();
        std::fs::write(me.join("cmdline"), b"aida\0mailbox\0").unwrap();
        std::os::unix::fs::symlink("/bin/true", me.join("exe")).unwrap();
        std::os::unix::fs::symlink(ns, me.join("ns").join("pid")).unwrap();
        proc
    }

    #[test]
    fn a_scan_from_a_separate_pid_namespace_is_incomplete_and_would_not_record() {
        // F7: writers outside the namespace are hidden, so the report fails
        // closed even though every visible host admits.
        let proc = fake_proc(42, "pid:[4026532000]");
        let (procs, incomplete) = scan_proc(proc.path(), 42);
        let why = incomplete.expect("a hidden namespace is incomplete");
        assert!(why.contains("pid namespace"), "{why}");
        let me = procs[0].exe_id.map(ident);
        assert!(!classify(&procs, me.as_ref(), 42, Some(why), false).would_record);

        let host = fake_proc(42, "pid:[4026531836]");
        assert_eq!(scan_proc(host.path(), 42).1, None, "the host namespace");
    }

    #[test]
    fn an_unreadable_cmdline_or_an_invisible_self_marks_the_scan_incomplete() {
        let proc = fake_proc(42, "pid:[4026531836]");
        // A cmdline that is a directory fails to read with an error other
        // than the process having exited.
        std::fs::create_dir_all(proc.path().join("77").join("cmdline")).unwrap();
        let why = scan_proc(proc.path(), 42).1.expect("incomplete");
        assert!(why.contains("process 77"), "{why}");

        // A process that exited (its directory is gone) is not a gap.
        std::fs::remove_dir_all(proc.path().join("77")).unwrap();
        std::fs::create_dir_all(proc.path().join("78")).unwrap();
        assert_eq!(scan_proc(proc.path(), 42).1, None);

        // A /proc that does not show this process is incomplete.
        let why = scan_proc(proc.path(), 43).1.expect("incomplete");
        assert!(why.contains("cannot see itself"), "{why}");
    }

    #[test]
    fn record_writes_the_activation_only_when_the_report_would_record() {
        super::super::set_source_build("0.0.0", "test", false);
        let anchor = tempfile::tempdir().unwrap();
        let me = ident((1, 10));
        let refused = classify(&[view(5, "/bin/aida", (1, 11))], Some(&me), 1, None, false);
        let out = record_at(anchor.path(), refused, "t").unwrap();
        assert!(!out.recorded && !out.dry_run);
        assert!(!super::super::activated(anchor.path()).unwrap());

        let admitted = classify(
            &[view(5, "/bin/aida", me.exe_id)],
            Some(&me),
            1,
            None,
            false,
        );
        let out = record_at(anchor.path(), admitted, "t").unwrap();
        assert!(out.recorded);
        assert!(super::super::activated(anchor.path()).unwrap());
        let written: super::super::Activation = serde_json::from_slice(
            &std::fs::read(anchor.path().join(super::super::ACTIVATION_FILE)).unwrap(),
        )
        .unwrap();
        assert_eq!(
            written.source_build.as_deref(),
            super::super::source_build()
        );
    }

    /// R4-F5: a dirty build has no source identity, so it neither records
    /// an activation nor honours one another dirty build of the same SHA
    /// recorded (their local edits may differ).
    #[test]
    fn r4_f5_a_dirty_build_never_names_a_source_build() {
        use super::super::source_build_id;
        assert_eq!(source_build_id("1.0.0", "abc1234", true), None);
        assert_eq!(source_build_id("1.0.0", "unknown", false), None);
        assert_eq!(
            source_build_id("1.0.0", " abc1234 ", false).as_deref(),
            Some("1.0.0+abc1234")
        );
        // No `-dirty` identity exists any more, so an old `-dirty` record is
        // honoured by no build.
        let anchor = tempfile::tempdir().unwrap();
        super::super::write_json(
            &anchor.path().join(super::super::ACTIVATION_FILE),
            &super::super::Activation {
                schema: super::super::SCHEMA,
                all_writer_hosts_admitting: true,
                recorded_at: "t".into(),
                source_build: Some("0.0.0+test-dirty".into()),
            },
        )
        .unwrap();
        super::super::set_source_build("0.0.0", "test", false);
        assert!(!super::super::activated(anchor.path()).unwrap());
    }

    #[test]
    fn only_a_complete_scan_of_admitting_hosts_would_record() {
        let me = (1, 10);
        let procs = vec![view(5, "/bin/aida", me)];
        assert!(classify(&procs, Some(&ident(me)), 1, None, false).would_record);
        assert!(!classify(&procs, Some(&ident(me)), 1, Some("hidepid".into()), false).would_record);
        assert!(!classify(&procs, None, 1, None, false).would_record);
    }
}
