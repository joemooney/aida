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
    commit_pending_admission_writes, hostname, leases_dir, read_claim, refresh_store, Claim,
    DEFAULT_TTL_SECS, LEASES_SUBDIR,
};

/// Seconds between refreshes: a third of the claim TTL, so two refreshes in a
/// row may fail (a rejected push, an unreachable store) before another host
/// sees the claim expire.
pub(crate) const SESSION_HEARTBEAT_INTERVAL_SECS: u64 = DEFAULT_TTL_SECS / 3;

/// The claim file this host holds for local lease `lease_id`, if any.
fn bound_claim(store_root: &Path, lease_id: &str, host: &str) -> Option<(PathBuf, Claim)> {
    let entries = std::fs::read_dir(leases_dir(store_root)).ok()?;
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("toml"))
        .filter_map(|path| read_claim(&path).map(|claim| (path, claim)))
        .find(|(_, claim)| {
            !claim.review_verb && claim.host == host && claim.lease_id.as_deref() == Some(lease_id)
        })
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
pub(crate) fn heartbeat_session_claim(store_root: &Path, lease_id: &str) -> Result<bool> {
    if lease_id.is_empty()
        || !store_root.join(".git").try_exists().unwrap_or(false)
        || !aida_core::git_ops::has_remote(store_root, "origin")
    {
        return Ok(false);
    }
    let branch =
        aida_core::git_ops::current_branch(store_root).unwrap_or_else(|_| "aida-store".to_string());
    let host = hostname();
    let committed_sha =
        aida_core::db::with_store_write_lock(store_root, || -> Result<Option<String>> {
            commit_pending_admission_writes(store_root)?;
            if refresh_store(store_root, &branch)?.is_err() {
                return Ok(None);
            }
            let Some((path, mut claim)) = bound_claim(store_root, lease_id, &host) else {
                return Ok(None);
            };
            claim.heartbeat_at = Utc::now().to_rfc3339();
            std::fs::write(&path, toml::to_string_pretty(&claim)?)?;
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                return Ok(None);
            };
            let rel = format!("{LEASES_SUBDIR}/{name}");
            aida_core::git_ops::add(store_root, &[&rel])?;
            let msg = format!("chore(coordination): heartbeat lease {}", claim.scope);
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
        Self::start_every(
            store_root,
            lease_id,
            std::time::Duration::from_secs(SESSION_HEARTBEAT_INTERVAL_SECS),
        )
    }

    fn start_every(store_root: PathBuf, lease_id: String, interval: std::time::Duration) -> Self {
        let (stop, stopped) = std::sync::mpsc::channel::<()>();
        let handle = std::thread::spawn(move || {
            while let Err(std::sync::mpsc::RecvTimeoutError::Timeout) =
                stopped.recv_timeout(interval)
            {
                if let Err(error) = heartbeat_session_claim(&store_root, &lease_id) {
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
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{acquire_claim_bound, claim_path};
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
        let mine = age(&store, "BUG-2706", 45);
        let other = age(&store, "BUG-1", 45);

        assert!(heartbeat_session_claim(&store, "lease-mine").unwrap());

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
        assert!(!heartbeat_session_claim(&store, "lease-mine").unwrap());
        assert!(!heartbeat_session_claim(&store, "").unwrap());
        assert!(!heartbeat_session_claim(&temp.path().join("absent"), "lease-mine").unwrap());

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
        assert!(!heartbeat_session_claim(&store, "lease-mine").unwrap());
        assert_eq!(read_claim(&path).unwrap(), foreign);
        // A successor lease took the spec over.
        let successor = rewrite(&|claim| {
            claim.host = hostname();
            claim.lease_id = Some("lease-next".into());
        });
        assert!(!heartbeat_session_claim(&store, "lease-mine").unwrap());
        assert_eq!(read_claim(&path).unwrap(), successor);
        // A review claim is not a lane.
        let review = rewrite(&|claim| {
            claim.lease_id = Some("lease-mine".into());
            claim.review_verb = true;
        });
        assert!(!heartbeat_session_claim(&store, "lease-mine").unwrap());
        assert_eq!(read_claim(&path).unwrap(), review);
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
