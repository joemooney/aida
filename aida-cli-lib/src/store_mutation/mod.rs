//! Launch-owned authorization for the read-only typed channel. No ambient
//! session environment is changed and no request supplies authority selectors.
// trace:TASK-3225 | ai:codex
use anyhow::{Context, Result};
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

pub(super) const SOURCE: &str = include_str!("service.py");

struct PinnedRoot {
    path: PathBuf,
    file: File,
}
impl PinnedRoot {
    fn open(path: &Path) -> Result<Self> {
        let path = std::fs::canonicalize(path)?;
        let file = File::open(&path)?;
        anyhow::ensure!(file.metadata()?.is_dir(), "service root is not a directory");
        Ok(Self { path, file })
    }
    fn unchanged(&self) -> bool {
        self.path
            .metadata()
            .and_then(|now| {
                self.file
                    .metadata()
                    .map(|old| now.dev() == old.dev() && now.ino() == old.ino())
            })
            .unwrap_or(false)
    }
}

/// Only finalized host Command assignments select this opt-in. The endpoint
/// and epoch are freshly generated here, never taken from the environment.
pub(super) struct Session {
    pub(super) config: serde_json::Value,
    pub(super) endpoint: String,
    pub(super) epoch: String,
    helper: UnixStream,
    stopped: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    worker: Arc<Mutex<Option<String>>>,
}
impl Session {
    pub(super) fn prepare(child: &std::process::Command, root: &Path) -> Result<Self> {
        use crate::gate::seat_authority::{self, GrantCheck};
        let explicit = |key: &str| {
            child.get_envs().find_map(|(k, v)| {
                (k == key)
                    .then(|| v.map(|v| v.to_string_lossy().into_owned()))
                    .flatten()
            })
        };
        let grant_id = explicit(seat_authority::GRANT_ENV)
            .filter(|s| !s.is_empty())
            .context("store service requires a nonempty explicit grant binding")?;
        let subject = explicit("AIDA_USER")
            .filter(|s| !s.is_empty())
            .context("store service requires explicit subject")?;
        let role = explicit("AIDA_SESSION_ROLE").context("store service requires explicit seat")?;
        let main = crate::main_worktree_root_from(root);
        let project = PinnedRoot::open(&main)?;
        let worktree = PinnedRoot::open(root)?;
        let store = PinnedRoot::open(
            &seat_authority::roster_store(&main).map_err(|e| anyhow::anyhow!("{e}"))?,
        )?;
        let GrantCheck::Valid(grant) = seat_authority::grant_check_for(&main, &grant_id, &subject)
        else {
            anyhow::bail!("store service requires a current validated grant");
        };
        anyhow::ensure!(
            grant.seat == role && !grant.binding.is_empty(),
            "store service seat/binding refused"
        );
        // Refuse ambiguous/missing ownership rather than borrowing a sibling lease.
        let mut leases = Vec::new();
        for entry in std::fs::read_dir(crate::leases::leases_dir(&main))? {
            let path = entry?.path();
            let name = path
                .file_name()
                .and_then(|s| s.to_str())
                .context("invalid lease name")?;
            if !name.ends_with(".toml")
                || name.ends_with(".manifest.toml")
                || name.ends_with(".activity.toml")
            {
                continue;
            }
            let lease = read_lease(&path)?;
            anyhow::ensure!(
                path == crate::leases::lease_path(&main, &lease.id),
                "lease identity/path mismatch"
            );
            if std::fs::canonicalize(&lease.worktree_path)?.as_path() == root
                && lease.role.as_deref() == Some(role.as_str())
            {
                leases.push(lease);
            }
        }
        anyhow::ensure!(
            leases.len() == 1,
            "store service requires one corroborated worktree lease"
        );
        let lease = &leases[0];
        let lease_path = crate::leases::lease_path(&main, &lease.id);
        let lease_id = lease.id.clone();
        let scope = lease.scope.clone();
        let started = lease.started_at;
        let graph =
            crate::load_store_for_lookup(&main).context("store service spec unavailable")?;
        let specs: Vec<_> = graph
            .requirements
            .iter()
            .filter(|r| {
                r.id.to_string() == scope
                    || r.spec_id.as_deref() == Some(&scope)
                    || r.agreed_id.as_deref() == Some(&scope)
            })
            .map(|r| r.id.to_string())
            .collect();
        anyhow::ensure!(
            specs.len() == 1,
            "store service requires an owned canonical spec"
        );
        let epoch = uuid::Uuid::new_v4().to_string();
        let endpoint = format!("/run/aida-mutation/{epoch}/service");
        let (helper, mut host) = UnixStream::pair()?;
        host.set_read_timeout(Some(Duration::from_millis(100)))?;
        host.set_write_timeout(Some(Duration::from_secs(5)))?;
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let parent = grant.parent_grant_id;
        let worker = Arc::new(Mutex::new(None::<String>));
        let bound_worker = worker.clone();
        let config = serde_json::json!({"epoch":epoch,"specs":specs,"role":role,
            "policy_version":1,"build":env!("CARGO_PKG_VERSION"),"authority_fd":helper.as_raw_fd()});
        let thread = std::thread::Builder::new()
            .name("store-service-authority".into())
            .spawn(move || {
                while !stop.load(Ordering::Acquire) {
                    let mut request = [0u8; 1];
                    match host.read(&mut request) {
                        Ok(1) if request == [b'?'] => {}
                        Err(e)
                            if matches!(
                                e.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                            ) =>
                        {
                            continue
                        }
                        _ => break,
                    }
                    let valid = || -> Option<u8> {
                        if !project.unchanged()
                            || !worktree.unchanged()
                            || !store.unchanged()
                            || seat_authority::roster_store(&main)
                                .ok()
                                .and_then(|p| std::fs::canonicalize(p).ok())
                                .as_deref()
                                != Some(root_path(&store))
                        {
                            return None;
                        }
                        let Ok(live) = read_lease(&lease_path) else {
                            return None;
                        };
                        if live.id != lease_id
                            || live.scope != scope
                            || live.started_at != started
                            || live.role.as_deref() != Some(role.as_str())
                            || std::fs::canonicalize(&live.worktree_path).ok().as_deref()
                                != Some(root_path(&worktree))
                        {
                            return None;
                        }
                        match seat_authority::grant_check_for(&main, &grant_id, &subject) {
                            GrantCheck::Valid(g)
                                if g.seat == role
                                    && g.parent_grant_id == parent
                                    && !g.binding.is_empty()
                                    && bound_worker.lock().is_ok_and(|worker| {
                                        worker
                                            .as_ref()
                                            .is_none_or(|token| g.binding.contains(token))
                                    }) =>
                            {
                                Some(b'Y')
                            }
                            GrantCheck::Rejected { reason, .. } => Some(rejection_code(&reason)),
                            _ => None,
                        }
                    };
                    if host.write_all(&[valid().unwrap_or(b'N')]).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self {
            config,
            endpoint,
            epoch,
            helper,
            stopped,
            thread: Some(thread),
            worker,
        })
    }
    pub(super) fn bind_worker(&mut self, pid: u32, start: &str) -> Result<()> {
        *self
            .worker
            .lock()
            .map_err(|_| anyhow::anyhow!("service worker lock refused"))? =
            Some(worker_binding_token(pid, start)?);
        self.helper.set_read_timeout(Some(Duration::from_secs(5)))?;
        self.helper
            .set_write_timeout(Some(Duration::from_secs(5)))?;
        // The helper is held at the receipt barrier, so no other authority
        // request can race this host-owned check.
        self.helper.write_all(b"?")?;
        let mut reply = [0u8; 1];
        self.helper.read_exact(&mut reply)?;
        anyhow::ensure!(reply == [b'Y'], "service worker grant binding refused");
        Ok(())
    }
    pub(super) fn fd(&self) -> i32 {
        self.helper.as_raw_fd()
    }
}
fn worker_binding_token(pid: u32, receipt_start: &str) -> Result<String> {
    let start =
        aida_core::liveness::process_start_identity(pid).context("service worker disappeared")?;
    anyhow::ensure!(
        start.strip_prefix("linux-starttime:") == Some(receipt_start),
        "service worker changed"
    );
    Ok(crate::gate::review_authority::author_process_token(
        "proc",
        pid,
        Some(&start),
    ))
}

fn read_lease(path: &Path) -> Result<crate::leases::SessionLease> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)?;
    let md = file.metadata()?;
    anyhow::ensure!(
        md.is_file()
            && md.nlink() == 1
            && md.uid() == unsafe { libc::getuid() }
            && md.len() <= 65536,
        "store service lease evidence refused"
    );
    let mut body = String::new();
    file.take(65537).read_to_string(&mut body)?;
    anyhow::ensure!(
        body.len() <= 65536,
        "store service lease evidence oversized"
    );
    Ok(toml::from_str(&body)?)
}

fn rejection_code(reason: &crate::gate::seat_authority::GrantRejection) -> u8 {
    use crate::gate::seat_authority::GrantRejection;
    match reason {
        GrantRejection::Expired { .. } => b'E',
        GrantRejection::Revoked { .. } => b'R',
        GrantRejection::Parent(parent) => rejection_code(parent),
        _ => b'N',
    }
}
fn root_path(root: &PinnedRoot) -> &Path {
    &root.path
}
impl Drop for Session {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        let _ = self.helper.shutdown(std::net::Shutdown::Both);
        if let Some(thread) = self.thread.take() {
            // No request can use the shut-down channel. Existing authority
            // readers may block on external storage; never make teardown wait
            // indefinitely for one. There is just one guardian per launch.
            let end = std::time::Instant::now() + Duration::from_millis(200);
            while !thread.is_finished() && std::time::Instant::now() < end {
                std::thread::sleep(Duration::from_millis(5));
            }
            if thread.is_finished() {
                let _ = thread.join();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // trace:TASK-3225 | ai:codex
    #[test]
    fn task_3225_worker_binding_uses_canonical_start_identity() {
        let pid = std::process::id();
        let start = aida_core::liveness::process_start_identity(pid).unwrap();
        let raw = start.strip_prefix("linux-starttime:").unwrap();
        assert_eq!(
            worker_binding_token(pid, raw).unwrap(),
            crate::gate::review_authority::author_process_token("proc", pid, Some(&start))
        );
        assert!(worker_binding_token(pid, "changed").is_err());
    }
    // trace:TASK-3225 | ai:codex
    #[test]
    fn task_3225_authority_errors_preserve_parent_expiry_and_revocation() {
        use crate::gate::seat_authority::GrantRejection;
        let at = chrono::Utc::now();
        assert_eq!(rejection_code(&GrantRejection::Expired { at }), b'E');
        assert_eq!(
            rejection_code(&GrantRejection::Parent(Box::new(GrantRejection::Revoked {
                at
            }))),
            b'R'
        );
        assert_eq!(rejection_code(&GrantRejection::Unreadable), b'N');
    }
}
