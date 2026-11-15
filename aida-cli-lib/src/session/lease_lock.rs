/// BUG-741: `codex exec` is not discoverable by the Claude-specific worktree
/// probe, so record the actual spawned child PID on the existing session lease.
/// `creator_pid` remains the session-start shell/host PID for end-resolution.
// trace:BUG-741 | ai:codex
fn stamp_lease_active_pid(lease_id: &str, pid: u32) -> Result<()> {
    let project_root = crate::find_project_root()?;
    stamp_lease_active_pid_at(&project_root, lease_id, pid)
}

/// BUG-2139: record the headless child's grant id on its lease, so a lease
/// end or reap revokes it even when the launcher crashed before the child's
/// exit could. Modeled on `SessionLease.child_grant_id` (BUG-2165); a lease
/// without the key is a legacy or interactive lease with nothing to revoke.
// trace:BUG-2139 | ai:claude
// trace:BUG-2165 | ai:antigravity
pub(crate) const LEASE_CHILD_GRANT_KEY: &str = "child_grant_id";

/// TASK-1798: the orchestrator's CI-fix child grant, stamped beside the
/// implementer's own [`LEASE_CHILD_GRANT_KEY`] (never over it), so a launcher
/// killed mid CI-fix still has that grant revoked by the lease end or reap.
// trace:TASK-1952 | trace:TASK-1798 | ai:claude
pub(crate) const LEASE_CI_FIX_CHILD_GRANT_KEY: &str = "ci_fix_child_grant_id";

/// TASK-2217: serialize the read-modify-write lease stampers (role, child
/// grant, active pid) under one lock file in the sessions directory, so two
/// stamps on one lease never lose each other's field. `write_atomic` replaces
/// the lease file, so the lock cannot be the lease file itself.
// trace:STORY-1662 | ai:claude
pub(crate) fn with_lease_write_lock<T>(
    project_root: &Path,
    write: impl FnOnce() -> Result<T>,
) -> Result<T> {
    use fs2::FileExt;
    let dir = project_root.join(".aida").join("sessions");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let lock_path = dir.join(LEASE_WRITE_LOCK);
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("opening {}", lock_path.display()))?;
    // Polled tightly only until the disk-saturation notice has had its one
    // look, so a long wait can say when the disk, not a stuck holder, is the
    // cause; then a plain blocking wait, so contended stamps hand the lock
    // over at once. trace:BUG-HOME-041 | ai:claude
    let mut notice = aida_core::io_pressure::LockWaitNotice::new();
    let started = std::time::Instant::now();
    loop {
        match lock.try_lock_exclusive() {
            Ok(()) => break,
            Err(e) if aida_core::file_lock::is_lock_contended(&e) => {
                notice.tick("session lease lock", || None);
                if started.elapsed() > aida_core::io_pressure::LOCK_WAIT_NOTICE_AFTER {
                    lock.lock_exclusive()
                        .with_context(|| format!("locking {}", lock_path.display()))?;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            Err(e) => {
                return Err(e).with_context(|| format!("locking {}", lock_path.display()));
            }
        }
    }
    let out = write();
    let _ = FileExt::unlock(&lock);
    out
}

/// The lease-write lock file's name under `.aida/sessions/`.
pub(crate) const LEASE_WRITE_LOCK: &str = ".lease-write.lock";

// trace:BUG-2139 | ai:claude
pub(crate) fn stamp_lease_child_grant_at(
    project_root: &Path,
    lease_id: &str,
    grant_id: &str,
) -> Result<()> {
    set_lease_grant_key_at(
        project_root,
        lease_id,
        LEASE_CHILD_GRANT_KEY,
        Some(grant_id),
    )
}

/// Set (`Some`) or remove (`None`) the grant-id `key` on lease `lease_id`.
/// A missing or unparseable lease is an error, so a caller that must stamp
/// before launching refuses instead of running unstamped.
// trace:BUG-2139 | ai:claude
// trace:TASK-1952 | trace:TASK-1798 | ai:claude
pub(crate) fn set_lease_grant_key_at(
    project_root: &Path,
    lease_id: &str,
    key: &str,
    grant_id: Option<&str>,
) -> Result<()> {
    use toml_edit::{value, DocumentMut};
    let path = project_root
        .join(".aida")
        .join("sessions")
        .join(format!("{lease_id}.toml"));
    with_lease_write_lock(project_root, || {
        let body = std::fs::read_to_string(&path)
            .with_context(|| format!("reading lease {}", path.display()))?;
        let mut doc = body
            .parse::<DocumentMut>()
            .with_context(|| format!("parsing lease {}", path.display()))?;
        match grant_id {
            Some(id) => doc[key] = value(id),
            None => {
                doc.remove(key);
            }
        }
        aida_core::write_atomic(&path, doc.to_string())
            .with_context(|| format!("writing lease {}", path.display()))
    })
}

/// BUG-2139: revoke the child grant stamped on lease file `lease_file`, if
/// any. Best-effort: called just before a lease is removed (session end,
/// force cleanup, record-only removal, reap); a missing or unreadable lease
/// or an unstamped one is a quiet no-op.
// trace:BUG-2139 | ai:claude
pub(crate) fn revoke_lease_child_grant(lease_file: &Path) {
    let Ok(body) = std::fs::read_to_string(lease_file) else {
        return;
    };
    let Ok(table) = body.parse::<toml::Table>() else {
        return;
    };
    // TASK-1798: the CI-fix child's grant too; a lease written before that
    // key existed carries only the implementer's.
    // trace:TASK-1952 | trace:TASK-1798 | ai:claude
    for key in [LEASE_CHILD_GRANT_KEY, LEASE_CI_FIX_CHILD_GRANT_KEY] {
        if let Some(id) = table
            .get(key)
            .and_then(|v| v.as_str())
            .filter(|id| !id.is_empty())
        {
            revoke_headless_child_grant(id);
        }
    }
}

// trace:BUG-741 | ai:codex
pub(crate) fn stamp_lease_active_pid_at(
    project_root: &Path,
    lease_id: &str,
    pid: u32,
) -> Result<()> {
    let holder = aida_core::proc_identity::HolderRecord::capture(pid);
    stamp_lease_holder_at(project_root, lease_id, pid, &holder)
}

// trace:STORY-2537 trace:BUG-2458 | ai:codex
pub(crate) fn stamp_lease_holder_at(
    project_root: &Path,
    lease_id: &str,
    pid: u32,
    holder: &aida_core::proc_identity::HolderRecord,
) -> Result<()> {
    use toml_edit::{value, DocumentMut};

    let path = project_root
        .join(".aida")
        .join("sessions")
        .join(format!("{lease_id}.toml"));
    let start_time = crate::process_probe::process_start_identity(pid);
    // trace:STORY-1662 | ai:claude
    with_lease_write_lock(project_root, || {
        let body = std::fs::read_to_string(&path)
            .with_context(|| format!("reading lease {}", path.display()))?;
        let mut doc = body
            .parse::<DocumentMut>()
            .with_context(|| format!("parsing lease {}", path.display()))?;
        // trace:STORY-2537 trace:BUG-2458 | ai:codex
        let prior = doc
            .get("holder_record")
            .and_then(|v| v.as_str())
            .and_then(|raw| {
                serde_json::from_str::<aida_core::proc_identity::HolderRecord>(raw).ok()
            });
        let prior = prior.as_ref().filter(|record| record.validate().is_ok());
        if let Some(prior) = prior {
            let id = prior.validate().expect("validated holder");
            if (id.pid, format!("linux-starttime:{}", id.starttime).as_str())
                != (pid, start_time.as_deref().unwrap_or_default())
            {
                anyhow::ensure!(matches!(prior.observe(), Ok(aida_core::proc_identity::Verdict::Dead(_))),
                    "lease {lease_id} retains another admitted worker generation; refusing replacement");
            }
        }
        let selected = prior
            .filter(|record| {
                record.identity().is_some_and(|id| {
                    id.pid == pid
                        && Some(format!("linux-starttime:{}", id.starttime).as_str())
                            == start_time.as_deref()
                })
            })
            .unwrap_or(holder);
        doc["active_pid"] = value(i64::from(pid));
        doc["holder_record"] = value(serde_json::to_string(selected)?);
        // trace:BUG-2425 | ai:codex
        if let Some(ns) = crate::agent_registry::current_pid_namespace() {
            doc["pid_namespace"] = value(ns as i64);
        } else {
            doc.remove("pid_namespace");
        }
        if let Some(start_time) = &start_time {
            doc["active_pid_start_time"] = value(start_time.clone());
        } else {
            doc.remove("active_pid_start_time");
        }
        // BUG-2419: the pid was read in this process's pid namespace.
        // trace:BUG-2419 | ai:claude
        match crate::gate::review_authority::origin_pid_ns() {
            Some(ns) => doc["active_pid_ns"] = value(ns),
            None => {
                doc.remove("active_pid_ns");
            }
        }
        // STORY-1429: atomic, so a reader never sees a half-rewritten lease.
        // trace:STORY-1429 | ai:claude
        aida_core::write_atomic(&path, doc.to_string())
            .with_context(|| format!("writing lease {}", path.display()))?;
        if let Ok(lease) = toml::from_str::<crate::SessionLease>(&doc.to_string()) {
            crate::ownership::dual::lease_written(project_root, &lease);
        }
        Ok(())
    })?;
    // BUG-1918: the spawned worker — not the launcher that claimed — works
    // this lease; record its process as one of the lease's authors so the
    // ledger outlives the lease.
    // A pid read inside a sandbox's pid namespace is qualified with it.
    // trace:BUG-1918 trace:BUG-2419 | ai:claude
    crate::gate::review_authority::record_lease_identity(
        project_root,
        lease_id,
        &[crate::gate::review_authority::author_process_token(
            "proc",
            pid,
            start_time.as_deref(),
        )],
    );
    Ok(())
}

/// STORY-2254: stamp a detached worker onto its lease, strictly. Under the
/// lease write lock: the lease must already carry `expected_grant` as its
/// child grant (stamped by the launch that minted it), no OTHER live process
/// may hold `active_pid`, and both `active_pid` and the REQUIRED
/// `active_pid_start_time` are written together (never an old start beside a
/// new pid). The persisted lease is re-read and verified before returning.
/// Authorship is recorded separately (one resource lock at a time).
// trace:STORY-2254 | ai:claude
#[cfg(test)]
pub(crate) fn stamp_lease_detached_worker_at(
    project_root: &Path,
    lease_id: &str,
    expected_grant: &str,
    pid: u32,
    start: &str,
) -> Result<()> {
    // Legacy test callers may supply synthetic start tokens. They exercise
    // namespace/authorship transport, not cooperative admission; never pair a
    // freshly captured generation with their supplied token.
    // trace:STORY-2537 trace:BUG-2458 | ai:codex
    let holder = aida_core::proc_identity::HolderRecord::refused(
        aida_core::proc_identity::HolderRefusal::EvidenceUnavailable(
            "legacy detached stamp has no cooperative handoff".into(),
        ),
    );
    stamp_lease_detached_worker_with_holder_at(
        project_root,
        lease_id,
        expected_grant,
        pid,
        start,
        &holder,
    )
}

// A spawn receipt must accompany foreign-PID admission; capture(pid) alone
// cannot attest a child. trace:STORY-2537 trace:BUG-2458 | ai:codex
pub(crate) fn stamp_lease_detached_worker_with_holder_at(
    project_root: &Path,
    lease_id: &str,
    expected_grant: &str,
    pid: u32,
    start: &str,
    holder: &aida_core::proc_identity::HolderRecord,
) -> Result<()> {
    use toml_edit::{value, DocumentMut};
    anyhow::ensure!(
        !start.is_empty() && !expected_grant.is_empty(),
        "detached worker lease stamp needs the worker start identity and its grant"
    );
    let path = project_root
        .join(".aida")
        .join("sessions")
        .join(format!("{lease_id}.toml"));
    with_lease_write_lock(project_root, || {
        let body = std::fs::read_to_string(&path)
            .with_context(|| format!("reading lease {}", path.display()))?;
        let mut doc = body
            .parse::<DocumentMut>()
            .with_context(|| format!("parsing lease {}", path.display()))?;
        anyhow::ensure!(
            doc.get(LEASE_CHILD_GRANT_KEY).and_then(|v| v.as_str()) == Some(expected_grant),
            "lease {lease_id} does not carry this launch's child grant; refusing to stamp its worker"
        );
        let current_pid = doc
            .get("active_pid")
            .and_then(|v| v.as_integer())
            .and_then(|p| u32::try_from(p).ok());
        let current_start = doc
            .get("active_pid_start_time")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        if let (Some(other), Some(other_start)) = (current_pid, current_start.as_deref()) {
            if (other, other_start) != (pid, start)
                && crate::process_probe::process_start_identity(other).as_deref()
                    == Some(other_start)
            {
                anyhow::bail!(
                    "lease {lease_id} is held by live process {other}; refusing to stamp a second worker"
                );
            }
        }
        // trace:STORY-2537 trace:BUG-2458 | ai:codex
        if let Ok(identity) = holder.validate() {
            anyhow::ensure!(
                identity.pid == pid && start == format!("linux-starttime:{}", identity.starttime),
                "detached worker holder does not match its stamped identity"
            );
        }
        doc["holder_record"] = value(serde_json::to_string(holder)?);
        doc["active_pid"] = value(i64::from(pid));
        doc["active_pid_start_time"] = value(start);
        if let Some(ns) = crate::agent_registry::current_pid_namespace() {
            doc["pid_namespace"] = value(ns as i64);
        } else {
            doc.remove("pid_namespace");
        }
        // BUG-2419: the pid was read in this process's pid namespace; without
        // it `lease_tokens` drops the worker's proc token.
        // trace:TASK-2799 | ai:claude
        match crate::gate::review_authority::origin_pid_ns() {
            Some(ns) => doc["active_pid_ns"] = value(ns),
            None => {
                doc.remove("active_pid_ns");
            }
        }
        aida_core::write_atomic(&path, doc.to_string())
            .with_context(|| format!("writing lease {}", path.display()))?;
        let back: toml::Table = std::fs::read_to_string(&path)
            .with_context(|| format!("re-reading lease {}", path.display()))?
            .parse()
            .with_context(|| format!("parsing lease {}", path.display()))?;
        anyhow::ensure!(
            back.get("active_pid").and_then(|v| v.as_integer()) == Some(i64::from(pid))
                && back.get("active_pid_start_time").and_then(|v| v.as_str()) == Some(start)
                && back.get(LEASE_CHILD_GRANT_KEY).and_then(|v| v.as_str()) == Some(expected_grant),
            "lease {lease_id} did not persist the detached worker stamp"
        );
        if let Ok(lease) = toml::from_str::<crate::SessionLease>(&doc.to_string()) {
            crate::ownership::dual::lease_written(project_root, &lease);
        }
        Ok(())
    })
}
