//! Session-claim heartbeat: a running lane keeps its shared claim fresh.
//!
//! A session claim is written once, with `heartbeat_at == started_at`. Another
//! host cannot probe the holder's process, so it judges the claim by heartbeat
//! age alone: without a refresh, every lane looked dead to other hosts 30
//! minutes after it started, and a review watcher there reviewed the head the
//! lane was still working on. The lane's launcher refreshes the claim for as
//! long as it runs; when the launcher dies the heartbeat stops and the TTL
//! applies as before.
// trace:BUG-2706 | ai:claude
use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::Utc;

use super::{
    commit_pending_admission_writes, hostname, refresh_store, Claim, DEFAULT_TTL_SECS,
    LEASES_SUBDIR,
};

/// Seconds between refreshes: a third of the claim TTL, so two refreshes in a
/// row may fail (a rejected push, an unreachable store) before another host
/// sees the claim expire.
pub(crate) const SESSION_HEARTBEAT_INTERVAL_SECS: u64 = DEFAULT_TTL_SECS / 3;

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

const MAX_CLAIM_BYTES: u64 = 65536;

struct BoundClaim {
    directory: File,
    file: File,
    path: PathBuf,
    claim: Claim,
}

fn handle_path(file: &File) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()))
}

fn bound_claim(
    store_root: &Path,
    scope: &str,
    lease_id: &str,
    host: &str,
) -> Result<Option<BoundClaim>> {
    let Some(directory) = claim_directory(store_root)? else {
        return Ok(None);
    };
    let path = handle_path(&directory).join(format!("{}.toml", super::sanitize_scope(scope)));
    let (file, claim) = match read_record(&path, &mut 0) {
        Ok(pair) => pair,
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            return Ok(None)
        }
        Err(error) => return Err(error),
    };
    if claim.scope != scope
        || claim.review_verb
        || claim.host != host
        || claim.lease_id.as_deref() != Some(lease_id)
    {
        return Ok(None);
    }
    Ok(Some(BoundClaim {
        directory,
        file,
        path,
        claim,
    }))
}

pub(super) fn strict_claims(store_root: &Path) -> Result<crate::lane_hosts::Inventory> {
    let mut claims = Vec::new();
    let incomplete = scan_claims(store_root, |_, _, _, claim| {
        claims.push(claim);
        Ok(())
    })?;
    Ok(crate::lane_hosts::Inventory { claims, incomplete })
}

fn claim_directory(store_root: &Path) -> Result<Option<File>> {
    let mut directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY)
        .open(store_root)?;
    for component in LEASES_SUBDIR.split('/') {
        directory = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(handle_path(&directory).join(component))
        {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
    }
    Ok(Some(directory))
}

fn scan_claims(
    store_root: &Path,
    mut visit: impl FnMut(&File, File, PathBuf, Claim) -> Result<()>,
) -> Result<Option<String>> {
    let Some(directory) = claim_directory(store_root)? else {
        return Ok(None);
    };
    let mut total = 0u64;
    let mut incomplete = None;
    for (index, entry) in std::fs::read_dir(handle_path(&directory))?.enumerate() {
        anyhow::ensure!(index < 20_000, "claim inventory limit exceeded");
        let path = entry?.path();
        if path.extension().is_none_or(|ext| ext != "toml") {
            continue;
        }
        let (file, claim) = match read_record(&path, &mut total) {
            Ok(pair) => pair,
            Err(_) => {
                incomplete = Some("invalid or unreadable claim file".into());
                anyhow::ensure!(total <= 32 << 20, "claim inventory too large");
                continue;
            }
        };
        visit(&directory, file, path, claim)?;
    }
    Ok(incomplete)
}

fn read_record(path: &Path, total: &mut u64) -> Result<(File, Claim)> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    anyhow::ensure!(
        meta.is_file() && meta.nlink() == 1 && meta.len() <= MAX_CLAIM_BYTES,
        "invalid claim file"
    );
    let mut data = String::new();
    (&file)
        .take(MAX_CLAIM_BYTES + 1)
        .read_to_string(&mut data)?;
    anyhow::ensure!(data.len() as u64 <= MAX_CLAIM_BYTES, "oversized claim");
    *total += data.len() as u64;
    anyhow::ensure!(*total <= 32 << 20, "claim inventory too large");
    let claim: Claim = toml::from_str(&data)?;
    Ok((file, claim))
}

impl BoundClaim {
    fn replace(&self) -> Result<()> {
        let mut sibling = tempfile::NamedTempFile::new_in(handle_path(&self.directory))?;
        sibling.write_all(toml::to_string_pretty(&self.claim)?.as_bytes())?;
        sibling.as_file().sync_all()?;
        let old = self.file.metadata()?;
        let current = std::fs::symlink_metadata(&self.path)?;
        anyhow::ensure!(
            current.ino() == old.ino() && current.dev() == old.dev() && current.nlink() == 1,
            "claim changed during refresh"
        );
        sibling.persist(&self.path)?;
        Ok(())
    }
}

/// Refresh `heartbeat_at` on the session claim this host holds for `lease_id`,
/// then commit and push it. `Ok(false)` when there is nothing to refresh: no
/// shared store, the store is unreachable, or the claim is gone or now belongs
/// to another lease (a successor's claim is never touched).
///
/// The launcher is a host process, so like claim admission it first commits
/// the store writes its contained lanes left pending; a store with pending
/// tracked changes cannot be refreshed from its origin.
// trace:BUG-2706 | ai:claude
pub(crate) fn heartbeat_session_claim(
    store_root: &Path,
    scope: &str,
    lease_id: &str,
) -> Result<bool> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    aida_core::git_ops::with_remote_deadline(deadline, crate::command_output_with_timeout, || {
        refresh(store_root, scope, lease_id)
    })
}

fn refresh(store_root: &Path, scope: &str, lease_id: &str) -> Result<bool> {
    if lease_id.is_empty()
        || !store_root.join(".git").try_exists().unwrap_or(false)
        || !aida_core::git_ops::has_remote(store_root, "origin")
    {
        return Ok(false);
    }
    let branch = aida_core::git_ops::current_branch(store_root)?;
    let host = hostname();
    let committed_sha =
        aida_core::db::with_store_write_lock_bounded(store_root, || -> Result<Option<String>> {
            commit_pending_admission_writes(store_root)?;
            if refresh_store(store_root, &branch)?.is_err() {
                return Ok(None);
            }
            let Some(mut bound) = bound_claim(store_root, scope, lease_id, &host)? else {
                return Ok(None);
            };
            bound.claim.heartbeat_at = Utc::now().to_rfc3339();
            bound.replace()?;
            let Some(name) = bound.path.file_name().and_then(|n| n.to_str()) else {
                return Ok(None);
            };
            let rel = format!("{LEASES_SUBDIR}/{name}");
            aida_core::git_ops::add(store_root, &[&rel])?;
            let msg = format!("chore(coordination): heartbeat lease {}", bound.claim.scope);
            if !aida_core::git_ops::commit(store_root, &msg)? {
                return Ok(None);
            }
            Ok(Some(aida_core::git_ops::head_sha(store_root)?))
        })?;
    let Some(sha) = committed_sha else {
        return Ok(false);
    };
    // A rejected push leaves the commit for the next store writer to publish.
    aida_core::git_ops::push_exact(store_root, "origin", &sha, &branch)
}

/// Refreshes one lane's session claim until dropped.
// trace:BUG-2706 | ai:claude
#[derive(Debug)]
pub(crate) struct SessionClaimHeartbeat {
    /// Dropping the sender wakes the thread at once, wherever it is waiting.
    stop: Option<std::sync::mpsc::Sender<()>>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl SessionClaimHeartbeat {
    /// Start refreshing the claim bound to `lease_id` every
    /// [`SESSION_HEARTBEAT_INTERVAL_SECS`]. The claim was written moments ago
    /// at session start, so the first refresh is one interval away.
    pub(crate) fn start(store_root: PathBuf, lease_id: String) -> Self {
        let project = store_root.parent().map(Path::to_owned);
        Self::watch(
            store_root,
            std::time::Duration::from_secs(SESSION_HEARTBEAT_INTERVAL_SECS),
            move || {
                let lease =
                    crate::leases::read_exact_lease(project.as_deref()?, &lease_id).ok()??;
                (!lease.review_verb).then_some((lease.scope, lease.id))
            },
        )
    }

    #[cfg(test)]
    fn start_every(store_root: PathBuf, lease_id: String, interval: std::time::Duration) -> Self {
        Self::watch(store_root, interval, move || {
            Some(("BUG-2706".into(), lease_id.clone()))
        })
    }

    pub(crate) fn drain(root: PathBuf, session: String) -> Self {
        Self::drain_every(
            root,
            session,
            std::time::Duration::from_secs(SESSION_HEARTBEAT_INTERVAL_SECS),
        )
    }

    fn drain_every(root: PathBuf, session: String, interval: std::time::Duration) -> Self {
        Self::watch(root.join(".aida-store"), interval, move || {
            let (id, _, _, _) = crate::find_orchestrated_lease(&root, &session)?;
            let lease = crate::leases::read_exact_lease(&root, &id).ok()??;
            (!lease.review_verb).then_some((lease.scope, lease.id))
        })
    }

    fn watch(
        store_root: PathBuf,
        interval: std::time::Duration,
        lease: impl Fn() -> Option<(String, String)> + Send + 'static,
    ) -> Self {
        let (stop, stopped) = std::sync::mpsc::channel::<()>();
        let handle = std::thread::spawn(move || {
            while let Err(std::sync::mpsc::RecvTimeoutError::Timeout) =
                stopped.recv_timeout(interval)
            {
                let Some((scope, lease_id)) = lease() else {
                    continue;
                };
                if let Err(error) = heartbeat_session_claim(&store_root, &scope, &lease_id) {
                    eprintln!(
                        "warning: could not refresh the shared claim for lease {lease_id}: {error:#}"
                    );
                }
            }
        });
        Self {
            stop: Some(stop),
            handle: Some(handle),
        }
    }
}

impl Drop for SessionClaimHeartbeat {
    fn drop(&mut self) {
        // A refresh already under way finishes first; it is one store commit.
        drop(self.stop.take());
        if let Some(handle) = self.handle.take() {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(12);
            while !handle.is_finished() && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            if handle.is_finished() {
                let _ = handle.join();
            } else {
                eprintln!("warning: claim heartbeat still finishing local store work at shutdown");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{acquire_claim_bound, claim_path, read_claim};
    use super::*;

    /// A project whose store has a bare origin, as two hosts would share.
    fn shared_store(temp: &Path) -> (PathBuf, PathBuf) {
        let hub = temp.join("hub.git");
        let root = temp.join("project");
        let store = root.join(".aida-store");
        std::fs::create_dir_all(&store).unwrap();
        let run = |cwd: &Path, args: &[&str]| {
            let out = crate::test_git::git_command()
                .current_dir(cwd)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(temp, &["init", "-q", "--bare", hub.to_str().unwrap()]);
        run(&store, &["init", "-q", "-b", "aida-store"]);
        run(&store, &["config", "user.name", "fixture"]);
        run(&store, &["config", "user.email", "fixture@test"]);
        run(&store, &["commit", "-q", "--allow-empty", "-m", "seed"]);
        run(&store, &["remote", "add", "origin", hub.to_str().unwrap()]);
        run(&store, &["push", "-q", "-u", "origin", "aida-store"]);
        (root, store)
    }

    /// The claim as the origin has it, which is all another host can see.
    fn published(store: &Path, scope: &str) -> Claim {
        let hub = store.parent().unwrap().parent().unwrap().join("hub.git");
        let rel = format!(
            "aida-store:{LEASES_SUBDIR}/{}.toml",
            super::super::sanitize_scope(scope)
        );
        let out = crate::test_git::git_command()
            .current_dir(&hub)
            .args(["show", &rel])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        toml::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap()
    }

    fn age(store: &Path, scope: &str, minutes: i64) -> Claim {
        let path = claim_path(store, scope);
        let mut claim = read_claim(&path).unwrap();
        claim.heartbeat_at = (Utc::now() - chrono::Duration::minutes(minutes)).to_rfc3339();
        std::fs::write(&path, toml::to_string_pretty(&claim).unwrap()).unwrap();
        aida_core::git_ops::add(store, &[LEASES_SUBDIR]).unwrap();
        aida_core::git_ops::commit(store, "age claim").unwrap();
        assert!(aida_core::git_ops::push(store, "origin", "aida-store").unwrap());
        claim
    }

    #[test]
    fn bug2706_heartbeat_refreshes_and_publishes_only_the_lanes_own_claim() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let (root, store) = shared_store(temp.path());
        for (scope, lease) in [("BUG-2706", "lease-mine"), ("BUG-1", "lease-other")] {
            acquire_claim_bound(
                &store,
                scope,
                &root,
                "implementer",
                false,
                false,
                Some(lease),
            )
            .unwrap();
        }
        // 45 minutes in: past the 30-minute TTL, as a long lane's claim was.
        std::fs::write(claim_path(&store, "BUG-2"), "[").unwrap();
        let mine = age(&store, "BUG-2706", 45);
        let other = age(&store, "BUG-1", 45);

        assert!(heartbeat_session_claim(&store, "BUG-2706", "lease-mine").unwrap());

        let refreshed = published(&store, "BUG-2706");
        let beat = chrono::DateTime::parse_from_rfc3339(&refreshed.heartbeat_at).unwrap();
        assert!(Utc::now().signed_duration_since(beat).num_seconds() < 60);
        // Only the heartbeat moved: same claim, same lease, same start.
        assert_eq!(
            Claim {
                heartbeat_at: mine.heartbeat_at.clone(),
                ..refreshed
            },
            mine
        );
        assert_eq!(published(&store, "BUG-1"), other);
    }

    #[test]
    fn bug2706_heartbeat_leaves_released_foreign_and_reviewer_claims_alone() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let (root, store) = shared_store(temp.path());
        // No claim at all, an empty lease id, and no store are all no-ops.
        assert!(!heartbeat_session_claim(&store, "BUG-2706", "lease-mine").unwrap());
        assert!(!heartbeat_session_claim(&store, "BUG-2706", "").unwrap());
        assert!(
            !heartbeat_session_claim(&temp.path().join("absent"), "BUG-2706", "lease-mine")
                .unwrap()
        );

        acquire_claim_bound(
            &store,
            "BUG-2706",
            &root,
            "implementer",
            false,
            false,
            Some("lease-mine"),
        )
        .unwrap();
        let path = claim_path(&store, "BUG-2706");
        let rewrite = |edit: &dyn Fn(&mut Claim)| {
            let mut claim = read_claim(&path).unwrap();
            edit(&mut claim);
            std::fs::write(&path, toml::to_string_pretty(&claim).unwrap()).unwrap();
            aida_core::git_ops::add(&store, &[LEASES_SUBDIR]).unwrap();
            aida_core::git_ops::commit(&store, "edit claim").unwrap();
            assert!(aida_core::git_ops::push(&store, "origin", "aida-store").unwrap());
            claim
        };
        // The same lease id minted on another host is that host's to refresh.
        let foreign = rewrite(&|claim| claim.host = "other-lane-host".into());
        assert!(!heartbeat_session_claim(&store, "BUG-2706", "lease-mine").unwrap());
        assert_eq!(read_claim(&path).unwrap(), foreign);
        // A successor lease took the spec over.
        let successor = rewrite(&|claim| {
            claim.host = hostname();
            claim.lease_id = Some("lease-next".into());
        });
        assert!(!heartbeat_session_claim(&store, "BUG-2706", "lease-mine").unwrap());
        assert_eq!(read_claim(&path).unwrap(), successor);
        // A review claim is not a lane.
        let review = rewrite(&|claim| {
            claim.lease_id = Some("lease-mine".into());
            claim.review_verb = true;
        });
        assert!(!heartbeat_session_claim(&store, "BUG-2706", "lease-mine").unwrap());
        assert_eq!(read_claim(&path).unwrap(), review);
    }

    #[test]
    fn bug2706_verified_claim_replacement_keeps_old_readers_and_refuses_links() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let (root, store) = shared_store(temp.path());
        acquire_claim_bound(
            &store,
            "BUG-2706",
            &root,
            "implementer",
            false,
            false,
            Some("mine"),
        )
        .unwrap();
        let path = claim_path(&store, "BUG-2706");
        let mut bound = bound_claim(&store, "BUG-2706", "mine", &hostname())
            .unwrap()
            .unwrap();
        let old = bound.claim.clone();
        bound.claim.heartbeat_at = Utc::now().to_rfc3339();
        bound.replace().unwrap();
        let mut previous = String::new();
        use std::io::{Seek, SeekFrom};
        bound.file.seek(SeekFrom::Start(0)).unwrap();
        bound.file.read_to_string(&mut previous).unwrap();
        assert_eq!(toml::from_str::<Claim>(&previous).unwrap(), old);
        assert_eq!(read_claim(&path).unwrap(), bound.claim);
        let outside = temp.path().join("outside");
        std::fs::rename(&path, &outside).unwrap();
        std::os::unix::fs::symlink(&outside, &path).unwrap();
        assert!(bound.replace().is_err());
        assert!(bound_claim(&store, "BUG-2706", "mine", &hostname()).is_err());
        assert_eq!(read_claim(&outside).unwrap(), bound.claim);
        std::fs::remove_file(&path).unwrap();
        let fifo = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: fifo is a live NUL-terminated pathname.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        assert!(bound_claim(&store, "BUG-2706", "mine", &hostname()).is_err());
    }

    #[test]
    fn bug2706_observer_reads_first_claim_and_refresh_from_origin_without_pulling() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let (root, store) = shared_store(temp.path());
        let observer = temp.path().join("observer");
        let status = crate::test_git::git_command()
            .args(["clone", "--branch", "aida-store"])
            .arg(temp.path().join("hub.git"))
            .arg(&observer)
            .status()
            .unwrap();
        assert!(status.success());
        let tip = aida_core::git_ops::head_sha(&observer).unwrap();
        acquire_claim_bound(
            &store,
            "BUG-2706",
            &root,
            "implementer",
            false,
            false,
            Some("mine"),
        )
        .unwrap();
        let aged = age(&store, "BUG-2706", 70);
        assert_eq!(
            crate::lane_hosts::published_claims(&observer)
                .unwrap()
                .claims,
            vec![aged.clone()]
        );
        assert!(heartbeat_session_claim(&store, "BUG-2706", "mine").unwrap());
        std::fs::write(claim_path(&store, "BUG-1"), vec![b'x'; 65537]).unwrap();
        aida_core::git_ops::add(&store, &[LEASES_SUBDIR]).unwrap();
        aida_core::git_ops::commit(&store, "oversized sibling").unwrap();
        assert!(aida_core::git_ops::push(&store, "origin", "aida-store").unwrap());
        let claims = crate::lane_hosts::published_claims(&observer).unwrap();
        assert!(claims.incomplete.is_some());
        assert_eq!(claims.claims, vec![published(&store, "BUG-2706")]);
        assert_ne!(claims.claims[0].heartbeat_at, aged.heartbeat_at);
        assert_eq!(aida_core::git_ops::head_sha(&observer).unwrap(), tip);
    }

    #[test]
    fn bug2706_slow_fetch_releases_the_store_lock_at_the_remote_deadline() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let (_, store) = shared_store(temp.path());
        fn stalled_fetch(
            command: std::process::Command,
            timeout: std::time::Duration,
        ) -> Option<std::process::Output> {
            assert!(command.get_args().any(|arg| arg == "fetch"));
            let mut stalled = std::process::Command::new("sleep");
            stalled.arg("60");
            crate::command_output_with_timeout(stalled, timeout)
        }
        let start = std::time::Instant::now();
        let refreshed = aida_core::git_ops::with_remote_deadline(
            start + std::time::Duration::from_millis(500),
            stalled_fetch,
            || refresh(&store, "BUG-2706", "mine"),
        )
        .unwrap();
        assert!(!refreshed);
        assert!(start.elapsed() >= std::time::Duration::from_millis(500));
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
        aida_core::db::with_store_write_lock_bounded(&store, || Ok(())).unwrap();
    }

    #[test]
    fn bug2706_long_drain_refreshes_the_lease_bound_to_its_session() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let (root, store) = shared_store(temp.path());
        let id = "019e9999-cccc";
        let session = "session-for-drain";
        let sessions = root.join(".aida/sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(
            sessions.join(format!("{id}.toml")),
            format!("id = '{id}'\nbranch = 'bug-2706'\nworktree_path = '.'\nscope = 'BUG-2706'\nslug = 'bug-2706'\nowner = 'fixture'\nhostname = 'fixture'\nstarted_at = '2026-01-01T00:00:00Z'\n"),
        )
        .unwrap();
        let manifest = crate::session_manifest::SessionManifest {
            session_id: id.into(),
            planned_at: Utc::now(),
            plan_source: "queue work".into(),
            claude_session_id: Some(session.into()),
            batch_name: None,
            agent_name: None,
            plan: None,
            seat: None,
            items: vec![],
        };
        crate::session_manifest::save(
            &crate::session_manifest::manifest_path(&root, id),
            &manifest,
        )
        .unwrap();
        acquire_claim_bound(
            &store,
            "BUG-2706",
            &root,
            "implementer",
            false,
            false,
            Some(id),
        )
        .unwrap();
        let aged = age(&store, "BUG-2706", 75);
        let _guard = SessionClaimHeartbeat::drain_every(
            root,
            session.into(),
            std::time::Duration::from_millis(200),
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while published(&store, "BUG-2706").heartbeat_at == aged.heartbeat_at {
            assert!(
                std::time::Instant::now() < deadline,
                "drain lease did not refresh"
            );
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    #[test]
    fn bug2706_guard_beats_while_held_and_stops_on_drop() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let (root, store) = shared_store(temp.path());
        acquire_claim_bound(
            &store,
            "BUG-2706",
            &root,
            "implementer",
            false,
            false,
            Some("lease-mine"),
        )
        .unwrap();
        let aged = age(&store, "BUG-2706", 45);
        let guard = SessionClaimHeartbeat::start_every(
            store.clone(),
            "lease-mine".into(),
            std::time::Duration::from_millis(300),
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while published(&store, "BUG-2706").heartbeat_at == aged.heartbeat_at {
            assert!(
                std::time::Instant::now() < deadline,
                "the running guard never refreshed the claim"
            );
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        drop(guard);
        let after_drop = published(&store, "BUG-2706").heartbeat_at;
        std::thread::sleep(std::time::Duration::from_millis(1200));
        assert_eq!(published(&store, "BUG-2706").heartbeat_at, after_drop);
    }
}
