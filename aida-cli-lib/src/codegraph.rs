//! Shared primary-checkout code navigation. This link grants no sandbox access;
//! containment read admission is a separate boundary.
// trace:STORY-2630 | ai:codex
use crate::doctor_root::DoctorFinding;
use anyhow::{Context, Result};
use rusqlite::OptionalExtension;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub(crate) const CATEGORY: &str = "codegraph";
const INDEX: &str = ".codegraph/codegraph.db";
const MAX_AGE: Duration = Duration::from_secs(3600);

// The proxy descriptor is observational only: no write or launch authority.
// trace:STORY-2630 | ai:codex
#[derive(serde::Deserialize, serde::Serialize)]
pub(crate) struct LaneHostStatus {
    protocol: u32,
    indexed_at: f64,
    captured_at: f64,
    observed_at: f64,
    refresh_error: Option<String>,
}

pub(crate) fn lane_host_status(root: &Path) -> Result<LaneHostStatus> {
    use std::io::Read;
    let path = root.join(".codegraph/lane-proxy.json");
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(&path).with_context(|| {
        format!(
            "Cannot open codegraph proxy descriptor {}; ask the host to deploy or repair the codegraph query proxy and its lane link",
            path.display()
        )
    })?;
    anyhow::ensure!(
        file.metadata()?.is_file(),
        "proxy status is not a regular file"
    );
    let mut body = Vec::new();
    file.take(8193).read_to_end(&mut body)?;
    anyhow::ensure!(body.len() <= 8192, "proxy status exceeds size limit");
    let status: LaneHostStatus = serde_json::from_slice(&body)?;
    anyhow::ensure!(status.protocol == 1, "unsupported codegraph proxy protocol");
    Ok(status)
}

impl LaneHostStatus {
    fn age(at: f64, now: SystemTime) -> Result<u64> {
        let now = now.duration_since(SystemTime::UNIX_EPOCH)?.as_secs_f64();
        anyhow::ensure!(
            at.is_finite() && at > 0.0 && at <= now + 5.0,
            "invalid codegraph proxy timestamp"
        );
        Ok((now - at).max(0.0) as u64)
    }

    pub(crate) fn report(&self, now: SystemTime) -> Result<serde_json::Value> {
        Ok(serde_json::json!({
            "snapshot_age_seconds": Self::age(self.indexed_at, now)?,
            "snapshot_captured_age_seconds": Self::age(self.captured_at, now)?,
            "host_observation_age_seconds": Self::age(self.observed_at, now)?,
            "refresh_error": self.refresh_error,
            "index_access": "read-only; queries via host proxy",
        }))
    }
}

/// A bounded real socket round trip, including a full backlog or stalled peer.
/// No subprocess, ambient input or caller-selected socket address.
// trace:STORY-2630 | ai:codex
#[cfg(unix)]
fn probe_proxy(root: &Path) -> Result<()> {
    use std::io::{Read, Write};
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::{ffi::OsStrExt, net::UnixStream};
    let path = root
        .join(".codegraph")
        .canonicalize()?
        .join("lane-query.sock");
    let bytes = path.as_os_str().as_bytes();
    // SAFETY: zero is a valid initial sockaddr_un; path is copied within bounds.
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    anyhow::ensure!(
        bytes.len() < address.sun_path.len(),
        "proxy socket path too long"
    );
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (target, byte) in address.sun_path.iter_mut().zip(bytes) {
        *target = *byte as libc::c_char;
    }
    // SAFETY: no pointers; the returned descriptor is owned exactly once below.
    let fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            0,
        )
    };
    anyhow::ensure!(
        fd >= 0,
        "create proxy socket: {}",
        std::io::Error::last_os_error()
    );
    // SAFETY: fd is a newly owned socket, not shared with any other owner.
    let mut stream = unsafe { UnixStream::from_raw_fd(fd) };
    // SAFETY: address points to an initialized sockaddr_un of the given length.
    let connected = unsafe {
        libc::connect(
            stream.as_raw_fd(),
            &address as *const _ as *const libc::sockaddr,
            std::mem::size_of_val(&address) as libc::socklen_t,
        )
    };
    anyhow::ensure!(
        connected == 0,
        "connect proxy: {}",
        std::io::Error::last_os_error()
    );
    stream.write_all(b"{\"op\":\"status\"}\n")?;
    let deadline = std::time::Instant::now() + Duration::from_millis(250);
    let mut body = Vec::new();
    loop {
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "proxy status timed out"
        );
        let mut buffer = [0; 1024];
        match stream.read(&mut buffer) {
            Ok(0) => anyhow::bail!(
                "proxy closed without a status response (service may be saturated; retry)"
            ),
            Ok(n) => body.extend_from_slice(&buffer[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            }
            Err(e) => return Err(e.into()),
        }
        anyhow::ensure!(body.len() <= 8192, "proxy status response too large");
        if let Some(end) = body.iter().position(|b| *b == b'\n') {
            let response: serde_json::Value = serde_json::from_slice(&body[..end])?;
            anyhow::ensure!(
                response["rc"] == 0 && response["protocol"] == 1,
                "proxy refused status: {}",
                response["output"]
            );
            return Ok(());
        }
    }
}

pub(crate) fn lane_host_report(root: &Path) -> Result<serde_json::Value> {
    let mut report = lane_host_status(root)?.report(SystemTime::now())?;
    let probe = probe_proxy(root);
    report["query_reachable"] = serde_json::json!(probe.is_ok());
    report["query_error"] = serde_json::json!(probe.err().map(|e| format!("{e:#}")));
    Ok(report)
}

fn assess_proxy(root: &Path, now: SystemTime) -> Vec<DoctorFinding> {
    let report = lane_host_status(root).and_then(|status| status.report(now));
    let action = "Check the host codegraph proxy service and snapshot refresh; do not grant the seat index write access.";
    match report {
        Err(error) => vec![finding(
            "unreachable",
            format!("codegraph proxy status unreachable: {error:#}"),
            action,
        )],
        Ok(report) => {
            let mut findings = Vec::new();
            if report["host_observation_age_seconds"]
                .as_u64()
                .unwrap_or(u64::MAX)
                > 30
            {
                findings.push(finding(
                    "unreachable",
                    "codegraph host proxy has no recent heartbeat or socket".into(),
                    action,
                ));
            } else if let Err(error) = probe_proxy(root) {
                findings.push(finding(
                    "unreachable",
                    format!("codegraph host proxy unreachable from this seat: {error:#}"),
                    action,
                ));
            }
            if !report["refresh_error"].is_null() {
                findings.push(finding(
                    "snapshot-refresh",
                    format!(
                        "codegraph snapshot refresh failed: {}",
                        report["refresh_error"]
                    ),
                    action,
                ));
            }
            if report["snapshot_age_seconds"].as_u64().unwrap_or(u64::MAX) > MAX_AGE.as_secs() {
                findings.push(finding(
                    "stale",
                    format!(
                        "codegraph host snapshot is {} seconds old (over one hour)",
                        report["snapshot_age_seconds"]
                    ),
                    action,
                ));
            }
            findings
        }
    }
}

/// Plan the link for the existing seat-clone/linked-worktree setup writer. Never copy or
/// build a private index, replace existing content, or chmod the shared index.
/// A symlink is routing only, not a read-only security boundary.
#[cfg(unix)]
pub(crate) fn primary_index_link(
    primary: &Path,
    seat: &Path,
) -> Result<Option<(PathBuf, PathBuf)>> {
    if primary == seat || !primary.join(INDEX).is_file() {
        return Ok(None);
    }
    let source = primary.join(".codegraph").canonicalize()?;
    let destination = seat.join(".codegraph");
    match std::fs::symlink_metadata(&destination) {
        Ok(meta) => {
            anyhow::ensure!(
                meta.file_type().is_symlink() && destination.canonicalize().ok().as_ref() == Some(&source),
                "{} already exists and does not link to the primary codegraph index; preserve it and resolve manually",
                destination.display()
            );
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Some((source, destination)));
        }
        Err(e) => return Err(e.into()),
    }
    Ok(None)
}

pub(crate) fn scan(root: &Path) -> Vec<DoctorFinding> {
    // Project intent, not an installed binary or leftover index, opts into
    // diagnostics. Resolve intent on main while probing access from the seat.
    if !project_uses_codegraph(root)
        && !project_uses_codegraph(&crate::main_worktree_root_from(root))
    {
        return Vec::new();
    }
    let binary = crate::which_binary("codegraph");
    assess_with_client_routing(
        root,
        &crate::main_worktree_root_from(root),
        binary.as_deref().map(Path::new),
        SystemTime::now(),
    )
}

// A lane with a descriptor needs the host client, even when PATH is empty.
// Suppress the upstream-install advice only when lane routing supplies the fix.
// trace:STORY-2630 | ai:codex
fn assess_with_client_routing(
    root: &Path,
    primary: &Path,
    binary: Option<&Path>,
    now: SystemTime,
) -> Vec<DoctorFinding> {
    let routing = client_routing_findings(root, primary, binary);
    let client = primary.join(".codegraph/proxy-state/bin/codegraph");
    let mut findings = assess(
        root,
        binary.is_some() || (root != primary && is_bundled_proxy(&client)),
        now,
    );
    if !routing.is_empty() {
        findings.retain(|finding| finding.id != "missing");
    }
    findings.extend(routing);
    findings
}

// The primary keeps upstream on PATH for indexing; only lanes need the client.
// trace:STORY-2630 | ai:codex
fn client_routing_findings(
    root: &Path,
    primary: &Path,
    _binary: Option<&Path>,
) -> Vec<DoctorFinding> {
    let mut findings = Vec::new();
    if root != primary
        && std::fs::symlink_metadata(root.join(".codegraph/lane-proxy.json")).is_ok()
        && !is_bundled_proxy(&primary.join(".codegraph/proxy-state/bin/codegraph"))
    {
        findings.push(finding(
            "client-routing",
            "Reviewed host query client is missing or differs from the bundled client".into(),
            "Ask the host to deploy the reviewed absolute query-client path at the primary checkout’s .codegraph/proxy-state/bin/codegraph outside seat writable roots, and invoke that absolute executable; keep upstream on the primary host PATH.",
        ));
    }
    findings
}

// Compare bounded data only; never execute an unknown PATH tool for diagnosis.
// This identifies the deployed client, and grants no launch or sandbox authority.
// trace:STORY-2630 | ai:codex
fn is_bundled_proxy(path: &Path) -> bool {
    use std::io::Read;
    const CLIENT: &[u8] = include_bytes!("../../scripts/codegraph/codegraph");
    let read = || -> std::io::Result<Vec<u8>> {
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        let file = options.open(path)?;
        if !file.metadata()?.is_file() {
            return Ok(Vec::new());
        }
        let mut body = Vec::new();
        file.take(CLIENT.len() as u64 + 1).read_to_end(&mut body)?;
        Ok(body)
    };
    read().is_ok_and(|body| body == CLIENT)
}

fn project_uses_codegraph(root: &Path) -> bool {
    std::fs::read_to_string(root.join(".gitignore")).is_ok_and(|body| {
        body.lines().any(|line| {
            matches!(
                line.trim(),
                ".codegraph" | ".codegraph/" | "/.codegraph" | "/.codegraph/"
            )
        })
    })
}

fn finding(id: &str, summary: String, action: &str) -> DoctorFinding {
    DoctorFinding {
        category: CATEGORY.into(),
        id: id.into(),
        summary,
        action: action.into(),
        safe_heal: false,
    }
}

/// Read-only and store-free: no codegraph subprocess, indexer or repair. Use
/// the database mtime for freshness; a WAL write can be newer than its db.
fn assess(root: &Path, binary_present: bool, now: SystemTime) -> Vec<DoctorFinding> {
    let mut findings = Vec::new();
    // An opted-in project on a new host still needs setup guidance.
    // trace:STORY-2630 | ai:codex
    if !binary_present {
        findings.push(finding(
            "missing",
            "codegraph is missing from PATH".into(),
            "Install codegraph on this host; maintain its index in the primary checkout.",
        ));
        if std::fs::symlink_metadata(root.join(".codegraph"))
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        {
            return findings;
        }
    }
    if std::fs::symlink_metadata(root.join(".codegraph/lane-proxy.json")).is_ok() {
        findings.extend(assess_proxy(root, now));
        return findings;
    }
    let path = root.join(INDEX);
    let reachable = (|| -> Result<SystemTime> {
        let meta = std::fs::metadata(&path)?;
        anyhow::ensure!(meta.is_file(), "index is not a regular file");
        let conn = rusqlite::Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        conn.busy_timeout(Duration::from_millis(100))?;
        // Probe the files table without scanning a potentially large graph.
        // An empty but valid index is reachable too.
        let _: Option<i64> = conn
            .query_row("SELECT 1 FROM files LIMIT 1", [], |row| row.get(0))
            .optional()?;
        let mut modified = meta.modified()?;
        if let Ok(wal) = std::fs::metadata(root.join(".codegraph/codegraph.db-wal")) {
            if wal.len() > 0 {
                modified = modified.max(wal.modified()?);
            }
        }
        Ok(modified)
    })();
    match reachable {
        Err(error) => findings.push(finding("unreachable", format!("codegraph index unreachable from {}: {error:#}", root.display()),
            "Check the primary index, seat .codegraph link and read-only containment access; do not initialize a lane index.")),
        Ok(modified) if now.duration_since(modified).is_ok_and(|age| age > MAX_AGE) => {
            findings.push(finding("stale", format!("codegraph index at {} is older than one hour", path.display()),
                "Check the primary checkout's codegraph sync job; refresh only on the host."));
        }
        Ok(_) => {}
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    // trace:STORY-2630 | ai:codex
    #[test]
    fn proxy_client_identity_refuses_upstream_and_nonregular_files() {
        let tmp = tempfile::tempdir().unwrap();
        let client = tmp.path().join("codegraph");
        std::fs::write(&client, include_bytes!("../../scripts/codegraph/codegraph")).unwrap();
        assert!(is_bundled_proxy(&client));
        std::fs::write(&client, "#!/bin/sh\necho upstream\n").unwrap();
        assert!(!is_bundled_proxy(&client));
        assert!(!is_bundled_proxy(tmp.path()));
        assert!(!is_bundled_proxy(&tmp.path().join("missing")));
    }

    // trace:STORY-2630 | ai:codex
    #[test]
    fn lane_host_missing_or_dangling_descriptor_names_host_recovery() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".codegraph/lane-proxy.json");
        for dangling in [false, true] {
            if dangling {
                std::fs::create_dir(tmp.path().join(".codegraph")).unwrap();
                std::os::unix::fs::symlink("absent", &path).unwrap();
            }
            let error = lane_host_report(tmp.path()).unwrap_err();
            let message = format!("{error:#}");
            assert!(message.contains(&path.display().to_string()), "{message}");
            assert!(
                message.contains("ask the host to deploy or repair"),
                "{message}"
            );
            assert!(
                error.chain().count() > 1,
                "preserve the OS error: {message}"
            );
        }
    }

    // trace:STORY-2630 | ai:codex
    #[test]
    fn proxy_reports_source_age_and_rejects_stale_or_invalid_heartbeats() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir(root.join(".codegraph")).unwrap();
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let seconds = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let mut status = serde_json::json!({"protocol": 1, "indexed_at": seconds - 7200.0,
            "captured_at": seconds, "observed_at": seconds, "refresh_error": null});
        let path = root.join(".codegraph/lane-proxy.json");
        std::fs::write(&path, status.to_string()).unwrap();
        let report = lane_host_status(root).unwrap().report(now).unwrap();
        assert_eq!(report["snapshot_age_seconds"], 7200);
        assert_eq!(report["snapshot_captured_age_seconds"], 0);
        // A fresh capture never makes a stale index fresh; no socket also refuses.
        let findings = assess_proxy(root, now);
        assert!(findings.iter().any(|f| f.id == "stale"));
        assert!(findings.iter().any(|f| f.id == "unreachable"));
        status["indexed_at"] = serde_json::json!(seconds + 60.0);
        std::fs::write(&path, status.to_string()).unwrap();
        assert!(lane_host_status(root).unwrap().report(now).is_err());
        std::fs::write(&path, "x".repeat(8193)).unwrap();
        assert!(lane_host_status(root).is_err());
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink("absent", &path).unwrap();
        assert!(lane_host_status(root).is_err());
    }

    #[test]
    fn lane_host_doctor_flag_parses_and_conflicts_with_heal() {
        let cli = crate::TEST_CLI
            .try_parse_from(["aida", "doctor", "--lane-codegraph", "--json"])
            .unwrap();
        assert!(
            cli.command
                .registered::<crate::doctor_cmd::DoctorArgs>()
                .lane_codegraph
        );
        assert!(crate::TEST_CLI
            .try_parse_from(["aida", "doctor", "--lane-codegraph", "--heal"])
            .is_err());
    }

    #[test]
    fn documented_host_path_keeps_upstream_while_lanes_require_proxy() {
        let tmp = tempfile::tempdir().unwrap();
        let primary = tmp.path().join("primary");
        let seat = tmp.path().join("seat");
        for root in [&primary, &seat] {
            std::fs::create_dir_all(root.join(".codegraph")).unwrap();
            std::fs::write(root.join(".codegraph/lane-proxy.json"), "{}").unwrap();
        }
        let upstream = tmp.path().join("upstream");
        std::fs::write(&upstream, "upstream CLI").unwrap();
        assert!(client_routing_findings(&primary, &primary, Some(&upstream)).is_empty());
        assert!(client_routing_findings(&primary, &primary, None).is_empty());
        let findings = assess_with_client_routing(&seat, &primary, None, SystemTime::now());
        assert!(findings.iter().any(|f| {
            f.id == "client-routing" && f.action.contains("reviewed absolute query-client path")
        }));
        assert!(!findings
            .iter()
            .any(|f| f.action.contains("Install codegraph")));
        let primary_findings =
            assess_with_client_routing(&primary, &primary, None, SystemTime::now());
        assert!(primary_findings.iter().any(|f| f.id == "missing"));
        assert_eq!(
            client_routing_findings(&seat, &primary, Some(&upstream))[0].id,
            "client-routing"
        );
        let client = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/codegraph/codegraph");
        assert!(!client_routing_findings(&seat, &primary, Some(&client)).is_empty());
        let deployed = primary.join(".codegraph/proxy-state/bin/codegraph");
        std::fs::create_dir_all(deployed.parent().unwrap()).unwrap();
        std::fs::copy(client, &deployed).unwrap();
        for binary in [None, Some(upstream.as_path())] {
            assert!(client_routing_findings(&seat, &primary, binary).is_empty());
            assert!(
                !assess_with_client_routing(&seat, &primary, binary, SystemTime::now())
                    .iter()
                    .any(|f| f.id == "missing" || f.id == "client-routing")
            );
        }
    }

    fn index(root: &Path) {
        std::fs::create_dir_all(root.join(".codegraph")).unwrap();
        rusqlite::Connection::open(root.join(INDEX))
            .unwrap()
            .execute_batch("CREATE TABLE files (path TEXT);")
            .unwrap();
    }

    #[test]
    fn seat_uses_primary_index_without_copying_or_replacing() {
        let tmp = tempfile::tempdir().unwrap();
        let primary = tmp.path().join("primary");
        let seat = tmp.path().join("seat");
        std::fs::create_dir_all(&seat).unwrap();
        index(&primary);
        aida_core::git_ops::init(&seat).unwrap();
        crate::worktree_gc::link_worktree_runtime_state(&primary, &seat).unwrap();
        crate::worktree_gc::link_worktree_runtime_state(&primary, &seat).unwrap();
        assert!(seat.join(".codegraph").is_symlink());
        assert_eq!(
            seat.join(INDEX).canonicalize().unwrap(),
            primary.join(INDEX).canonicalize().unwrap()
        );
        assert!(assess(&seat, true, SystemTime::now()).is_empty());
        // Primary removal is visible immediately, rather than leaving a stale copy.
        std::fs::remove_file(primary.join(INDEX)).unwrap();
        assert_eq!(assess(&seat, true, SystemTime::now())[0].id, "unreachable");
    }

    #[test]
    fn setup_skips_absent_index_and_preserves_conflicting_content() {
        let tmp = tempfile::tempdir().unwrap();
        let primary = tmp.path().join("primary");
        let seat = tmp.path().join("seat");
        std::fs::create_dir_all(&seat).unwrap();
        crate::worktree_gc::link_worktree_runtime_state(&primary, &seat).unwrap();
        assert!(!seat.join(".codegraph").exists());
        index(&primary);
        index(&seat);
        std::fs::create_dir(primary.join(".aida-store")).unwrap();
        std::fs::create_dir_all(primary.join(".aida/sessions")).unwrap();
        crate::worktree_gc::link_worktree_runtime_state(&primary, &seat).unwrap();
        assert!(seat.join(".aida-store").is_symlink());
        assert!(seat.join(".aida/sessions").is_symlink());
        assert!(seat.join(INDEX).is_file());
        assert!(!seat.join(".codegraph").is_symlink());
        // A foreign link (including a dangling one) is preserved too.
        std::fs::remove_dir_all(seat.join(".codegraph")).unwrap();
        let foreign = tmp.path().join("foreign");
        for exists in [false, true] {
            if exists {
                std::fs::create_dir(&foreign).unwrap();
            }
            std::os::unix::fs::symlink(&foreign, seat.join(".codegraph")).unwrap();
            std::fs::remove_file(seat.join(".aida-store")).unwrap();
            std::fs::remove_file(seat.join(".aida/sessions")).unwrap();
            crate::worktree_gc::link_worktree_runtime_state(&primary, &seat).unwrap();
            assert_eq!(
                std::fs::read_link(seat.join(".codegraph")).unwrap(),
                foreign
            );
            assert!(seat.join(".aida-store").is_symlink());
            assert!(seat.join(".aida/sessions").is_symlink());
            std::fs::remove_file(seat.join(".codegraph")).unwrap();
        }
    }

    #[test]
    fn doctor_reports_missing_unreachable_stale_and_corrupt() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let absent = assess(root, false, SystemTime::now());
        assert_eq!(
            absent.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
            ["missing"]
        );
        assert_eq!(assess(root, true, SystemTime::now())[0].id, "unreachable");
        assert!(
            !root.join(".codegraph").exists(),
            "doctor must not create an index"
        );
        std::fs::create_dir(root.join(".codegraph")).unwrap();
        let findings = assess(root, false, SystemTime::now());
        assert_eq!(
            findings.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
            ["missing", "unreachable"]
        );
        index(root);
        assert_eq!(assess(root, false, SystemTime::now())[0].id, "missing");
        let modified = std::fs::metadata(root.join(INDEX))
            .unwrap()
            .modified()
            .unwrap();
        assert!(assess(root, true, modified + MAX_AGE).is_empty());
        assert_eq!(
            assess(root, true, modified + MAX_AGE + Duration::from_secs(1))[0].id,
            "stale"
        );
        std::fs::write(root.join(INDEX), "not sqlite").unwrap();
        assert_eq!(assess(root, true, SystemTime::now())[0].id, "unreachable");
    }

    #[test]
    fn doctor_requires_project_opt_in() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        aida_core::git_ops::init(root).unwrap();
        // Neither host installation nor an incidental index opts a project in.
        assert!(scan(root).is_empty());
        index(root);
        assert!(scan(root).is_empty());
        std::fs::write(root.join(".gitignore"), "# .codegraph\n!.codegraph\n").unwrap();
        assert!(scan(root).is_empty());
        for declaration in [".codegraph", ".codegraph/", "/.codegraph", "/.codegraph/"] {
            std::fs::write(root.join(".gitignore"), format!("{declaration}\n")).unwrap();
            assert!(project_uses_codegraph(root));
        }
        std::fs::remove_dir_all(root.join(".codegraph")).unwrap();
        assert!(
            !scan(root).is_empty(),
            "opted-in projects need setup guidance"
        );
    }

    #[test]
    fn doctor_proxy_round_trip_and_stalled_peer_are_bounded() {
        use std::io::{BufRead, Write};
        use std::os::unix::net::UnixListener;
        let tmp = tempfile::tempdir().unwrap();
        let graph = tmp.path().join(".codegraph");
        std::fs::create_dir(&graph).unwrap();
        let socket = graph.join("lane-query.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let peer = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            std::io::BufReader::new(&stream)
                .read_line(&mut request)
                .unwrap();
            assert_eq!(request, "{\"op\":\"status\"}\n");
            stream.write_all(b"{\"rc\":0,\"protocol\":1}\n").unwrap();
        });
        assert!(probe_proxy(tmp.path()).is_ok());
        peer.join().unwrap();
        std::fs::remove_file(&socket).unwrap();
        let listener = UnixListener::bind(&socket).unwrap();
        let peer = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            std::thread::sleep(Duration::from_millis(400));
            drop(stream);
        });
        assert!(probe_proxy(tmp.path())
            .unwrap_err()
            .to_string()
            .contains("timed out"));
        peer.join().unwrap();
        std::fs::remove_file(socket).unwrap();
        assert!(probe_proxy(tmp.path()).is_err());
    }

    #[test]
    fn assembled_skill_prompts_keep_invocation_first_and_include_navigation() {
        use crate::queue_cmd::{QueueWorkMode, QueueWorkPlan};
        use crate::session_start::ReviewForge;
        let plan = QueueWorkPlan {
            mode: QueueWorkMode::Item,
            entries: Vec::new(),
            scope: "STORY-2630".into(),
            review_target: Some((ReviewForge::GitHub, 3607)),
            anchor_display: "STORY-2630".into(),
            anchor_title: "navigation".into(),
        };
        for (role, gate) in [
            ("implementer", None),
            ("reviewer", None),
            ("security", Some("security")),
        ] {
            let _env = crate::test_env::EnvVarsGuard::apply(&[
                ("AIDA_AGENT_GATE_NAME", gate),
                ("AIDA_AGENT_GATE_ROLE", gate),
                ("AIDA_REVIEW_ROUND", Some("1")),
            ]);
            let prompt = crate::rework_direction::assemble_pickup_prompt(
                &crate::rework_direction::PickupInputs {
                    plan: &plan,
                    role,
                    plan_only: false,
                    guided: false,
                    review_findings: crate::rework_direction::ReviewResult::Unavailable,
                    spec_comments: &[],
                    conflict: None,
                    ci_failures: None,
                },
            );
            let invocation = if role == "implementer" {
                "/aida-pickup STORY-2630"
            } else {
                "/aida-review --pr 3607"
            };
            assert_eq!(prompt.lines().next(), Some(invocation), "{prompt}");
            if gate.is_some() {
                assert!(prompt.contains("AIDA_REVIEW_VERDICT_FILE"), "{prompt}");
            } else if role == "reviewer" {
                assert!(prompt.contains("ROUND 1"), "{prompt}");
            }
            assert!(prompt.contains(
                include_str!("../../aida-core/templates/dispatch/code-navigation.md").trim()
            ));
        }
    }

    // trace:STORY-2630 | ai:codex
    #[test]
    fn root_agents_navigation_matches_template_master() {
        let agents = include_str!("../../AGENTS.md");
        let navigation = agents
            .split("For code navigation, ")
            .nth(1)
            .unwrap()
            .split("\n<!-- trace:STORY-2630")
            .next()
            .unwrap();
        assert!(navigation.contains("/home/joe/ai/aida/.codegraph/proxy-state/bin/codegraph"));
        let shared = include_str!("../../aida-core/templates/dispatch/code-navigation.md");
        assert!(!shared.contains("/home/joe"));
        assert!(shared.contains("absolute path supplied by this project"));
    }

    // trace:STORY-2630 | ai:codex
    #[test]
    fn reentered_old_seat_excludes_navigation_link_idempotently() {
        let tmp = tempfile::tempdir().unwrap();
        let primary = tmp.path().join("primary");
        let seat = tmp.path().join("seat");
        index(&primary);
        aida_core::git_ops::init(&seat).unwrap();
        std::fs::write(seat.join(".gitignore"), ".codegraph/\n").unwrap();
        let exclude = seat.join(".git/info/exclude");
        std::fs::write(&exclude, "# old seat\n/.aida/*\n.aida-store\n").unwrap();
        for _ in 0..2 {
            crate::worktree_gc::link_worktree_runtime_state(&primary, &seat).unwrap();
            assert!(seat.join(".codegraph").is_symlink());
            let ignored = crate::test_git::git_command()
                .current_dir(&seat)
                .args(["check-ignore", ".codegraph"])
                .output()
                .unwrap();
            assert!(ignored.status.success(), "{ignored:?}");
        }
        let body = std::fs::read_to_string(exclude).unwrap();
        assert!(body.starts_with("# old seat\n"), "{body}");
        assert_eq!(body.lines().filter(|line| *line == ".codegraph").count(), 1);
    }

    #[test]
    fn wal_activity_is_fresh() {
        let tmp = tempfile::tempdir().unwrap();
        index(tmp.path());
        let old = SystemTime::now() - Duration::from_secs(7200);
        std::fs::File::open(tmp.path().join(INDEX))
            .unwrap()
            .set_modified(old)
            .unwrap();
        // A valid live WAL is held open by its writer.
        let conn = rusqlite::Connection::open(tmp.path().join(INDEX)).unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL; INSERT INTO files VALUES ('fresh.rs');")
            .unwrap();
        std::fs::File::open(tmp.path().join(INDEX))
            .unwrap()
            .set_modified(old)
            .unwrap();
        assert!(assess(tmp.path(), true, SystemTime::now()).is_empty());
        assert_eq!(
            crate::normalize_doctor_category("code-graph").unwrap(),
            CATEGORY
        );
    }
}
