//! The activation report and record (QC1, STORY-1843 C2).
//!
//! Retirement uses a fresh writer-admission scan after fencing, independently
//! of the stored activation or its source build. The dry run is a
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
//! cannot be read or digested is *unverified*, unless its executable carries
//! [`WRITE_PERMIT_MARKER`] (BUG-2437): every binary that links admission
//! embeds that marker, so an older build, or the replaced binary a long-lived
//! lane still runs after `aida dev build` promotes, is known to admit when it
//! speaks this build's permit protocol. A replaced file stays readable through
//! `/proc/<pid>/exe`, so the check needs nothing from the writer itself.
//!
//! The record names the source build that wrote it (version and git SHA,
//! shared by the `aida` and `aida-server` built from one checkout and by a
//! re-promoted build of the same source). Reports distinguish a current
//! record from an older one; retirement always uses fresh admission evidence.
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

/// Present in every binary that takes a mail write permit:
/// [`super::admit_resolved`] references it, so any build that links admission
/// carries these bytes. A build that predates this marker (or the permit
/// protocol) does not carry it, so it never counts as admitting. The version
/// names the permit protocol; change it whenever an older admitting build
/// would no longer satisfy what retirement relies on, and builds of the old
/// protocol stop counting as admitting.
// trace:BUG-2437 | ai:claude
pub(crate) static WRITE_PERMIT_MARKER: &[u8] = b"\x01aida:mail-write-permit:protocol=1\x01";

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
    /// Whether the executable carries [`WRITE_PERMIT_MARKER`], read with the
    /// digest; `None` when unread or unreadable.
    pub permit_marker: Option<bool>,
    pub cmdline: Vec<String>,
}

/// The build this process runs, as [`classify`] compares writer hosts to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildIdentity {
    /// Device and inode of this process's executable.
    pub exe_id: (u64, u64),
    /// SHA-256 of its bytes, or `None` when it could not be read.
    pub digest: Option<String>,
    /// This build carries [`WRITE_PERMIT_MARKER`], so a writer that carries it
    /// too speaks the permit protocol this build's record vouches for. Only
    /// then does a writer's marker admit it (BUG-2437).
    pub permit_marker: bool,
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
    /// Mailbox tombs holding entries written after a retirement, and what
    /// `aida agent gc` would reap (see [`super::leftovers`]).
    // trace:TASK-b6b1278890774ef993d2ebc327862470-052 | ai:claude
    #[serde(default)]
    pub leftovers: super::leftovers::Leftovers,
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
        // BUG-2437: a writer whose executable carries this build's permit
        // protocol marker admits, even on another or a replaced binary.
        let marked = me.is_some_and(|me| me.permit_marker) && p.permit_marker == Some(true);
        let verdict = if p.pid == self_pid || marked {
            HostVerdict::Admitting
        } else if deleted {
            HostVerdict::Unverified {
                why: "it runs a binary that has since been replaced and does not carry \
                      this build's write-permit protocol"
                    .into(),
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
        leftovers: Default::default(),
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
            permit_marker: true,
        });
    let proc = Path::new("/proc");
    let (mut processes, incomplete) = scan_proc(proc, std::process::id());
    let me = me.map(|me| fill_digests(proc, &mut processes, me, &proc.join("self/exe")));
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
    report.leftovers = super::leftovers::scan(project_root, false); // trace:TASK-b6b1278890774ef993d2ebc327862470-052 | ai:claude
    report
}

/// Fresh writer admission evidence for retirement, regardless of whether an
/// activation is recorded or which source build recorded it. Called after
/// fencing under Lifecycle; an incomplete or unverified scan refuses.
/// Compatible permit markers admit older/replaced writers (BUG-2437).
/// Unit tests use [`FRESH_SCAN`] so they never depend on the host's processes.
// trace:TASK-3420 | ai:codex
pub(crate) fn fresh_scan_admits() -> bool {
    #[cfg(all(feature = "mailbox-test-support", not(test)))]
    if let Some(admitting) = TEST_WRITER_SCAN.with(|f| f.get()) {
        return admitting;
    }
    #[cfg(test)]
    {
        FRESH_SCAN.with(|f| f.get())
    }
    #[cfg(not(test))]
    {
        let Ok(meta) = std::fs::metadata("/proc/self/exe") else {
            return false;
        };
        let me = BuildIdentity {
            exe_id: (meta.dev(), meta.ino()),
            digest: None,
            permit_marker: true,
        };
        let proc = Path::new("/proc");
        let (mut processes, incomplete) = scan_proc(proc, std::process::id());
        let me = fill_digests(proc, &mut processes, me, &proc.join("self/exe"));
        classify(&processes, Some(&me), std::process::id(), incomplete, false).would_record
    }
}

#[cfg(test)]
thread_local! {
    /// What [`fresh_scan_admits`] answers in this test thread.
    // trace:BUG-2411 | ai:claude
    pub(crate) static FRESH_SCAN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static PROMOTION_PROC_ROOT: std::cell::RefCell<Option<PathBuf>> = const {
        std::cell::RefCell::new(None)
    };
}

// Scoped writer inventory evidence for downstream tests. The feature is
// enabled only by dev-dependencies; ordinary binaries expose no fixture API.
// The default remains the real process scan, even with the feature enabled.
// trace:TASK-3420 | ai:codex
#[cfg(feature = "mailbox-test-support")]
thread_local! {
    static TEST_WRITER_SCAN: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Restores the test thread's prior writer inventory on drop.
// trace:TASK-3420 | ai:codex
#[cfg(feature = "mailbox-test-support")]
#[doc(hidden)]
pub struct TestWriterScan {
    previous: Option<bool>,
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}

#[cfg(feature = "mailbox-test-support")]
impl Drop for TestWriterScan {
    fn drop(&mut self) {
        TEST_WRITER_SCAN.with(|f| f.set(self.previous));
    }
}

/// Supply explicit writer inventory evidence for this test thread only.
// trace:TASK-3420 | ai:codex
#[cfg(feature = "mailbox-test-support")]
#[doc(hidden)]
pub fn test_writer_scan(admitting: bool) -> TestWriterScan {
    TestWriterScan {
        previous: TEST_WRITER_SCAN.with(|f| f.replace(Some(admitting))),
        _thread: std::marker::PhantomData,
    }
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
    report: ActivationReport,
    now: &str,
) -> super::Result<ActivationReport> {
    record_named(anchor, report, super::source_build(), now)
}

/// BUG-2411 (advisor ruling C): after `aida dev build` promotes, re-record an
/// existing activation for the promoted build, so a rebuild does not strand
/// every mail-bearing checkout until an operator re-records by hand. The scan
/// is the one [`record`] runs, but judged against `promoted_exe` (the new live
/// binary) instead of this process, and the record names `promoted_source`
/// (the identity the promoted binary reports for itself). Nothing is written
/// unless every writer host runs the promoted build and the scan is complete;
/// the promoting process itself is the only exception, since it is the
/// recorder, writes no mail, and exits next. `Ok(None)`: this checkout has no
/// mail anchor or no activation was ever recorded, so there is nothing to
/// re-record.
// trace:BUG-2411 | ai:claude
pub fn rerecord_for_promoted_build(
    project_root: &Path,
    promoted_exe: &Path,
    promoted_source: &str,
) -> super::Result<Option<ActivationReport>> {
    let Some(source) = super::resolve_source(project_root)? else {
        return Ok(None);
    };
    if super::activation_state(&source.anchor)? == super::ActivationState::Absent {
        return Ok(None);
    }
    let meta = std::fs::metadata(promoted_exe)?;
    // The promoted build's own protocol decides which writers it may vouch
    // for, not this (promoting) process's: a marker admits only when the
    // promoted binary carries the same one (BUG-2437).
    let me = BuildIdentity {
        exe_id: (meta.dev(), meta.ino()),
        digest: None,
        permit_marker: scan_file_for_marker(promoted_exe) == Some(true),
    };
    // The test-only root keeps promotion admission independent of host writers.
    // trace:TASK-3111 trace:TASK-2833 | ai:codex
    #[cfg(test)]
    let proc_root = PROMOTION_PROC_ROOT.with(|root| root.borrow().clone());
    #[cfg(test)]
    let proc = proc_root.as_deref().unwrap_or(Path::new("/proc"));
    #[cfg(not(test))]
    let proc = Path::new("/proc");
    let (mut processes, incomplete) = scan_proc(proc, std::process::id());
    let me = fill_digests(proc, &mut processes, me, promoted_exe);
    let mut report = classify(&processes, Some(&me), std::process::id(), incomplete, false);
    report.this_build = Some(promoted_source.to_string());
    let source_build = Some(promoted_source).filter(|s| !s.trim().is_empty());
    record_named(
        &source.anchor,
        report,
        source_build,
        &chrono::Utc::now().to_rfc3339(),
    )
    .map(Some)
}

/// Write the record naming `source_build` when `report.would_record`.
fn record_named(
    anchor: &Path,
    mut report: ActivationReport,
    source_build: Option<&str>,
    now: &str,
) -> super::Result<ActivationReport> {
    report.dry_run = false;
    // A build that cannot name its source records nothing it would honour.
    let Some(source_build) = source_build else {
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
/// process's `me` (read from `self_exe`; writers from `proc`), each distinct
/// file once, so a byte-identical copy at another path is recognised as this
/// build, and note whether each carries [`WRITE_PERMIT_MARKER`]. A replaced
/// (deleted) binary is read too, through `/proc/<pid>/exe` (BUG-2437), but
/// only up to the marker and never hashed: its digest could not admit it.
/// Nothing is read when every writer shares this process's file. An
/// unreadable file keeps no digest and no marker, which [`classify`] reports
/// as unidentified (fail closed).
// trace:BUG-b6b1278890774ef993d2ebc327862470-010 | ai:claude
fn fill_digests(
    proc: &Path,
    processes: &mut [ProcessView],
    mut me: BuildIdentity,
    self_exe: &Path,
) -> BuildIdentity {
    let mut seen: HashMap<((u64, u64), bool), Inspection> = HashMap::new();
    for p in processes.iter_mut() {
        let Some(id) = p.exe_id else { continue };
        let Some(exe) = p.exe.as_deref() else {
            continue;
        };
        let deleted = exe.ends_with(" (deleted)");
        let writer = Path::new(exe.trim_end_matches(" (deleted)"))
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| WRITER_EXECUTABLES.contains(&n));
        if !writer || id == me.exe_id {
            continue;
        }
        if !deleted && me.digest.is_none() {
            me.digest = digest_file(self_exe);
        }
        let exe_link = proc.join(p.pid.to_string()).join("exe");
        let inspection = match seen.get(&(id, deleted)) {
            Some(found) => Some(found.clone()),
            None => inspect_writer(&exe_link, id, !deleted)
                .map(|found| seen.entry((id, deleted)).or_insert(found).clone()),
        };
        p.exe_digest = inspection.as_ref().and_then(|i| i.digest.clone());
        p.permit_marker = inspection.map(|i| i.permit_marker);
    }
    me
}

/// What one read of an executable shows: its SHA-256, when it was hashed, and
/// whether it carries [`WRITE_PERMIT_MARKER`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct Inspection {
    digest: Option<String>,
    permit_marker: bool,
}

/// Inspect the writer executable at `exe_link` only when the opened file is
/// still the `expected` (dev, ino) the scan recorded. A pid reused between
/// the scan and this read names another file: that is `None` (unknown, fail
/// closed), never a digest or marker attributed to `expected`. Only a
/// successful read is cached, so an unreadable host never hides a later
/// readable one. Without `hash`, the read stops at the marker.
// trace:TASK-b6b1278890774ef993d2ebc327862470-050 | ai:claude
fn inspect_writer(exe_link: &Path, expected: (u64, u64), hash: bool) -> Option<Inspection> {
    let mut file = std::fs::File::open(exe_link).ok()?;
    let meta = file.metadata().ok()?;
    if (meta.dev(), meta.ino()) != expected {
        return None;
    }
    if hash {
        return inspect_reader(&mut file, true);
    }
    // Deleted executables cannot be rewritten while running. Keep a bounded
    // cache with pinned inodes so repeated retirement scans do not reread old,
    // unmarked debug builds to EOF, nor confuse a recycled inode with one.
    // Metadata also invalidates entries for ordinary mutable test files.
    // trace:TASK-3112 | ai:codex
    type Stamp = (u64, u64, u64, i64, i64, i64, i64);
    static MARKERS: std::sync::Mutex<Vec<(Stamp, std::fs::File, Inspection)>> =
        std::sync::Mutex::new(Vec::new());
    let stamp = (
        meta.dev(),
        meta.ino(),
        meta.len(),
        meta.mtime(),
        meta.mtime_nsec(),
        meta.ctime(),
        meta.ctime_nsec(),
    );
    if let Some(found) = MARKERS
        .lock()
        .ok()?
        .iter()
        .find(|(key, _, _)| *key == stamp)
    {
        return Some(found.2.clone());
    }
    let inspection = inspect_reader(&mut file, false)?;
    let mut cache = MARKERS.lock().ok()?;
    if cache.len() == 16 {
        cache.remove(0);
    }
    cache.push((stamp, file, inspection.clone()));
    Some(inspection)
}

/// SHA-256 of a file's bytes, hex-encoded, or `None` when it cannot be read.
fn digest_file(path: &Path) -> Option<String> {
    inspect_reader(&mut std::fs::File::open(path).ok()?, true)?.digest
}

/// Whether the file at `path` carries [`WRITE_PERMIT_MARKER`], reading only
/// up to the first hit and hashing nothing; `None` when it cannot be read.
// trace:BUG-2437 | ai:claude
fn scan_file_for_marker(path: &Path) -> Option<bool> {
    inspect_reader(&mut std::fs::File::open(path).ok()?, false).map(|i| i.permit_marker)
}

/// Look for [`WRITE_PERMIT_MARKER`] in `reader` and, with `hash`, take the
/// SHA-256 of all of it in the same pass. Without `hash`, reading stops at
/// the first hit. The marker's last `len - 1` bytes of each block carry over,
/// so a marker that straddles two reads is still found.
// trace:BUG-2437 | ai:claude
fn inspect_reader(reader: &mut impl std::io::Read, hash: bool) -> Option<Inspection> {
    let needle = WRITE_PERMIT_MARKER;
    let keep = needle.len() - 1;
    let skip = horspool_skips(needle);
    let mut hasher = hash.then(Sha256::new);
    let mut window: Vec<u8> = Vec::with_capacity(keep + 64 * 1024);
    let mut block = vec![0u8; 64 * 1024];
    let mut found = false;
    loop {
        let n = match reader.read(&mut block) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return None,
        };
        if let Some(hasher) = hasher.as_mut() {
            hasher.update(&block[..n]);
        }
        if found {
            continue;
        }
        window.extend_from_slice(&block[..n]);
        found = contains(&window, needle, &skip);
        if found && hasher.is_none() {
            break;
        }
        let spent = window.len().saturating_sub(keep);
        window.drain(..spent);
    }
    Some(Inspection {
        digest: hasher.map(|h| format!("{:x}", h.finalize())),
        permit_marker: found,
    })
}

/// Boyer-Moore-Horspool shift table: how far the search may jump when the
/// byte under the needle's last position is `b`. It keeps the scan of a large
/// executable cheap even in an unoptimised build.
fn horspool_skips(needle: &[u8]) -> [usize; 256] {
    let mut skip = [needle.len(); 256];
    let last = needle.len() - 1;
    for (j, &b) in needle[..last].iter().enumerate() {
        skip[usize::from(b)] = last - j;
    }
    skip
}

fn contains(haystack: &[u8], needle: &[u8], skip: &[usize; 256]) -> bool {
    let m = needle.len();
    let last = m - 1;
    let mut i = 0;
    while i + m <= haystack.len() {
        let b = haystack[i + last];
        if b == needle[last] && haystack[i..i + last] == needle[..last] {
            return true;
        }
        i += skip[usize::from(b)];
    }
    false
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
            permit_marker: None,
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
            permit_marker: None,
            cmdline: vec![exe.into(), "mcp".into()],
        }
    }

    fn ident(id: (u64, u64)) -> BuildIdentity {
        BuildIdentity {
            exe_id: id,
            digest: None,
            permit_marker: true,
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
            permit_marker: true,
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
            permit_marker: true,
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
        let empty_proc = tempfile::tempdir().unwrap();
        let me = fill_digests(empty_proc.path(), &mut procs, me, self_exe.path());
        assert_eq!(me.digest, digest_file(self_exe.path()));
        assert_eq!(procs[0].exe_digest, None, "this process's own file");
        assert_eq!(procs[1].exe_digest, None, "no such process: unreadable");
        assert_eq!(procs[2].exe_digest, None, "not a writer");
    }

    /// A `/proc` whose `<pid>/exe` links to a small writer binary, with its
    /// (dev, ino) as the scan would record it.
    fn proc_with_writer(pid: u32) -> (tempfile::TempDir, PathBuf, (u64, u64)) {
        let proc = tempfile::tempdir().unwrap();
        let exe = proc.path().join("aida");
        std::fs::write(&exe, b"build-a").unwrap();
        std::fs::create_dir(proc.path().join(pid.to_string())).unwrap();
        std::os::unix::fs::symlink(&exe, proc.path().join(pid.to_string()).join("exe")).unwrap();
        let m = std::fs::metadata(&exe).unwrap();
        (proc, exe, (m.dev(), m.ino()))
    }

    /// A readable foreign writer is digested once per exe_id and the cache
    /// serves every later host on that id; a pid whose exe is no longer the
    /// recorded file (reused) is unknown and leaves nothing cached.
    // trace:TASK-b6b1278890774ef993d2ebc327862470-050 | ai:claude
    // trace:TASK-b6b1278890774ef993d2ebc327862470-051 | ai:claude
    #[test]
    fn fill_digests_verifies_the_exe_identity_and_reuses_the_cache() {
        let (proc, exe, id) = proc_with_writer(5);
        let wrong = (id.0, id.1.wrapping_add(1));
        let mut procs = vec![
            view(5, "/x/aida", wrong),
            view(5, "/x/aida", id),
            view(6, "/x/aida", id),
        ];
        fill_digests(proc.path(), &mut procs, ident((1, 10)), &exe);
        let expected = digest_file(&exe);
        assert!(expected.is_some());
        assert_eq!(procs[0].exe_digest, None, "a reused pid: unknown");
        assert_eq!(procs[1].exe_digest, expected);
        assert_eq!(procs[2].exe_digest, expected, "pid 6 unreadable: cached");
        let link = proc.path().join("5/exe");
        assert_eq!(inspect_writer(&link, wrong, true), None);
    }

    /// Only a digest is cached: an unreadable first host on an exe_id does
    /// not leave a readable second host on it unidentified.
    // trace:TASK-b6b1278890774ef993d2ebc327862470-050 | ai:claude
    #[test]
    fn an_unreadable_host_does_not_hide_a_readable_one_on_the_same_id() {
        let (proc, exe, id) = proc_with_writer(5);
        let mut procs = vec![view(6, "/x/aida", id), view(5, "/x/aida", id)];
        fill_digests(proc.path(), &mut procs, ident((1, 10)), &exe);
        assert_eq!(procs[0].exe_digest, None, "pid 6 unreadable");
        assert_eq!(procs[1].exe_digest, digest_file(&exe));
    }

    /// BUG-2437: after a promote, lanes still run the replaced binary of a
    /// build that takes permits. A writer whose executable carries this
    /// build's permit protocol marker admits, replaced or not; one without it,
    /// or unread, still refuses; and a marker vouches for nothing when the
    /// build judging it does not carry the same protocol.
    // trace:BUG-2437 | ai:claude
    #[test]
    fn a_replaced_or_older_build_carrying_the_permit_marker_admits() {
        let me = ident((1, 10));
        let marked = |pid, exe: &str, marker| ProcessView {
            permit_marker: marker,
            exe_digest: Some("old".into()),
            ..view(pid, exe, (1, 20))
        };
        let procs = vec![
            marked(5, "/live/aida (deleted)", Some(true)),
            marked(6, "/old/aida-server", Some(true)),
        ];
        let report = classify(&procs, Some(&me), 1, None, false);
        assert!(report.would_record, "{report:?}");

        for (marker, why) in [(Some(false), "replaced"), (None, "replaced")] {
            let procs = [marked(5, "/live/aida (deleted)", marker)];
            let report = classify(&procs, Some(&me), 1, None, false);
            assert!(!report.would_record);
            assert!(
                matches!(&report.hosts[0].verdict, HostVerdict::Unverified { why: w } if w.contains(why))
            );
        }
        let unmarked = [marked(6, "/old/aida", Some(false))];
        assert!(!classify(&unmarked, Some(&me), 1, None, false).would_record);

        let other_protocol = BuildIdentity {
            permit_marker: false,
            ..me
        };
        assert!(!classify(&procs, Some(&other_protocol), 1, None, false).would_record);
    }

    // Exercise the public promotion entry with real executable bytes and a
    // private proc tree. An unreadable executable is a directory: metadata
    // succeeds, but marker/digest reads fail even when tests run as root.
    // trace:TASK-3111 trace:TASK-2833 | ai:codex
    #[test]
    fn promotion_uses_the_promoted_executables_marker() {
        struct ResetProc;
        impl Drop for ResetProc {
            fn drop(&mut self) {
                PROMOTION_PROC_ROOT.with(|root| *root.borrow_mut() = None);
            }
        }
        let _reset = ResetProc;
        for kind in ["marked", "unmarked", "unreadable"] {
            for same_exe in [true, false] {
                let repo = super::super::tests::repo();
                super::super::tests::activate(repo.path());
                let anchor = super::super::tests::source(repo.path()).anchor;
                let activation = anchor.join(super::super::ACTIVATION_FILE);
                let before = std::fs::read(&activation).unwrap();
                let files = tempfile::tempdir().unwrap();
                let promoted = files.path().join("promoted/aida");
                std::fs::create_dir_all(promoted.parent().unwrap()).unwrap();
                if kind == "unreadable" {
                    std::fs::create_dir(&promoted).unwrap();
                } else {
                    let mut bytes = b"promoted build".to_vec();
                    if kind == "marked" {
                        bytes.extend_from_slice(WRITE_PERMIT_MARKER);
                    }
                    std::fs::write(&promoted, bytes).unwrap();
                }
                let writer = if same_exe {
                    promoted.clone()
                } else {
                    let writer = files.path().join("older/aida");
                    std::fs::create_dir_all(writer.parent().unwrap()).unwrap();
                    let mut bytes = b"different older build".to_vec();
                    bytes.extend_from_slice(WRITE_PERMIT_MARKER);
                    std::fs::write(&writer, bytes).unwrap();
                    writer
                };
                let pid = std::process::id();
                let proc = fake_proc(pid, "pid:[4026531836]");
                let other = proc.path().join((pid + 1).to_string());
                std::fs::create_dir(&other).unwrap();
                std::fs::write(other.join("cmdline"), b"aida\0mailbox\0").unwrap();
                std::os::unix::fs::symlink(&writer, other.join("exe")).unwrap();
                PROMOTION_PROC_ROOT.with(|root| *root.borrow_mut() = Some(proc.path().into()));
                let report = rerecord_for_promoted_build(repo.path(), &promoted, "promoted-source")
                    .unwrap()
                    .unwrap();
                let admitted = same_exe || kind == "marked";
                assert_eq!(report.scan_incomplete, None, "{report:?}");
                assert_eq!(report.hosts.len(), 1, "{report:?}");
                assert_eq!(
                    report.would_record, admitted,
                    "{kind}/{same_exe}: {report:?}"
                );
                assert_eq!(report.recorded, admitted, "{report:?}");
                if admitted {
                    let written: super::super::Activation =
                        serde_json::from_slice(&std::fs::read(&activation).unwrap()).unwrap();
                    assert_eq!(written.source_build.as_deref(), Some("promoted-source"));
                } else {
                    assert_eq!(std::fs::read(&activation).unwrap(), before);
                }
            }
        }
    }

    /// The marker is found wherever it sits, including across two reads, and
    /// the digest is the plain SHA-256 of the bytes.
    // trace:BUG-2437 | ai:claude
    #[test]
    fn inspect_reader_finds_the_marker_across_a_block_boundary() {
        for at in [0, 10, 64 * 1024 - 5, 64 * 1024, 200_000] {
            let mut bytes = vec![0u8; at];
            bytes.extend_from_slice(WRITE_PERMIT_MARKER);
            bytes.extend_from_slice(&[7u8; 100]);
            let found = inspect_reader(&mut std::io::Cursor::new(&bytes), true).unwrap();
            assert!(found.permit_marker, "marker at {at}");
            assert_eq!(found.digest, Some(format!("{:x}", Sha256::digest(&bytes))));
            let scanned = inspect_reader(&mut std::io::Cursor::new(&bytes), false).unwrap();
            assert!(scanned.permit_marker, "scan: marker at {at}");
            assert_eq!(scanned.digest, None, "a marker-only scan hashes nothing");
        }
        let mut cut = vec![0u8; 64 * 1024 - 3];
        cut.extend_from_slice(&WRITE_PERMIT_MARKER[..WRITE_PERMIT_MARKER.len() - 1]);
        for hash in [true, false] {
            let found = inspect_reader(&mut std::io::Cursor::new(&cut), hash).unwrap();
            assert!(!found.permit_marker, "an incomplete marker is no marker");
        }
    }

    /// A marker-only scan stops at the first hit: a reader that fails once
    /// the marker has been delivered is never read that far.
    // trace:BUG-2437 | ai:claude
    #[test]
    fn a_marker_scan_stops_at_the_first_hit() {
        struct FailsAfter(std::io::Cursor<Vec<u8>>);
        impl std::io::Read for FailsAfter {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                match self.0.read(buf)? {
                    0 => Err(std::io::Error::other("read past the marker")),
                    n => Ok(n),
                }
            }
        }
        let mut bytes = vec![0u8; 1000];
        bytes.extend_from_slice(WRITE_PERMIT_MARKER);
        let scanned = inspect_reader(&mut FailsAfter(std::io::Cursor::new(bytes.clone())), false);
        assert_eq!(scanned.map(|i| i.permit_marker), Some(true));
        let hashed = inspect_reader(&mut FailsAfter(std::io::Cursor::new(bytes)), true);
        assert_eq!(hashed, None, "a hashing read goes to the end");
    }

    /// Smoke check on a real executable: this test binary links admission,
    /// so a marker-only scan of it (which stops at the first hit and hashes
    /// nothing, keeping this fast in a debug build) finds the marker.
    // trace:BUG-2437 | ai:claude
    #[test]
    fn a_binary_that_links_admission_carries_the_permit_marker() {
        let exe = std::env::current_exe().expect("own executable path");
        assert_eq!(scan_file_for_marker(&exe), Some(true));
    }

    /// The scan reads a replaced writer's executable through `/proc/<pid>/exe`
    /// and records its marker, without hashing it: a replaced binary's digest
    /// could not admit it.
    // trace:BUG-2437 | ai:claude
    // trace:TASK-3112 | ai:codex
    #[test]
    fn task3112_marker_cache_invalidates_changed_content_and_checks_identity() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), b"old unmarked build").unwrap();
        let meta = file.as_file().metadata().unwrap();
        let id = (meta.dev(), meta.ino());
        for _ in 0..2 {
            assert_eq!(
                inspect_writer(file.path(), id, false)
                    .unwrap()
                    .permit_marker,
                false
            );
        }
        std::fs::write(file.path(), WRITE_PERMIT_MARKER).unwrap();
        assert!(
            inspect_writer(file.path(), id, false)
                .unwrap()
                .permit_marker
        );
        assert!(inspect_writer(file.path(), (id.0, id.1 + 1), false).is_none());
    }

    #[test]
    fn fill_digests_reads_the_marker_of_a_replaced_writer() {
        let (proc, exe, id) = proc_with_writer(5);
        let mut bytes = b"build-old".to_vec();
        bytes.extend_from_slice(WRITE_PERMIT_MARKER);
        std::fs::write(&exe, &bytes).unwrap();
        let mut procs = vec![view(5, "/x/aida (deleted)", id)];
        let me = fill_digests(proc.path(), &mut procs, ident((1, 10)), &exe);
        assert_eq!(procs[0].permit_marker, Some(true));
        assert_eq!(procs[0].exe_digest, None);
        assert!(classify(&procs, Some(&me), 1, None, false).would_record);
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
