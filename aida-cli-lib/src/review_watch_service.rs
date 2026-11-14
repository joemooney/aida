//! Review-watch service using the shared advisor loop supervisor.
// trace:STORY-2691 | ai:codex
use std::sync::{atomic::AtomicBool, Arc};

/// Trusted context shared by the socket and both service supervisors. Deputy
/// grant refresh and shutdown affect both without retaining an ambient selector.
// trace:STORY-2571 STORY-2691 | ai:codex
#[derive(Clone)]
pub(crate) struct HostAuthority {
    pub(crate) project: PathBuf,
    grant: Arc<std::sync::Mutex<String>>,
    cfg: crate::deputy_shell::runner::RunnerConfig,
    pub(super) active: Arc<AtomicBool>,
}
impl HostAuthority {
    pub(crate) fn new(cfg: &crate::deputy_shell::runner::RunnerConfig) -> Self {
        Self {
            project: cfg.project_root.clone(),
            grant: Arc::new(std::sync::Mutex::new(cfg.grant_id.clone())),
            cfg: cfg.clone(),
            active: Arc::new(AtomicBool::new(true)),
        }
    }
    pub(crate) fn refreshed(&self, id: &str) {
        if let Ok(mut grant) = self.grant.lock() {
            *grant = id.into();
        }
    }
    pub(crate) fn validate(&self) -> Result<()> {
        anyhow::ensure!(self.active.load(Ordering::SeqCst), "deputy stopped");
        let id = self
            .grant
            .lock()
            .map_err(|_| anyhow::anyhow!("deputy authority unavailable"))?;
        match (self.cfg.grant)(&id) {
            crate::deputy::worker::GrantState::Valid { expires_at } => {
                anyhow::ensure!(
                    expires_at.is_none_or(|at| at > (self.cfg.clock)()),
                    "deputy authority expired"
                );
            }
            _ => anyhow::bail!("deputy authority ended"),
        }
        anyhow::ensure!((self.cfg.mandate)().is_none(), "deputy mandate refused");
        Ok(())
    }
}

const TRAIN: bool = false;
include!("advisor_loop_service.rs");

#[cfg(test)]
#[cfg(target_os = "linux")]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, crate::test_env::AmbientGuard, SeatGrant) {
        let temp = tempfile::tempdir().unwrap();
        let guard = crate::test_env::AmbientGuard::hermetic_with_seat(
            temp.path(),
            "advisor",
            &["advisor", "reviewer"],
        );
        let grant = seat_authority::grant_check(temp.path())
            .into_valid()
            .unwrap();
        (temp, guard, grant)
    }
    fn successor(root: &Path, old: &SeatGrant) -> SeatGrant {
        let id = seat_authority::test_support::mint_grant_for(
            root,
            &old.subject,
            "advisor",
            &["advisor", "reviewer"],
        );
        seat_authority::read_grant(&id).unwrap()
    }

    struct ControllerScope;
    impl Drop for ControllerScope {
        fn drop(&mut self) {
            CONTROLLER.with(|id| *id.borrow_mut() = None);
        }
    }
    fn controller(
        root: &Path,
        guard: &mut crate::test_env::AmbientGuard,
        parent: &SeatGrant,
    ) -> ControllerScope {
        rebind(root, "default", parent).unwrap();
        let child =
            seat_authority::issue_review_watch_child(root, &parent.id, &parent.subject).unwrap();
        let dir = directory(root, false).unwrap().unwrap();
        let mut state = read(&dir).unwrap().unwrap();
        state.child_grant = Some(child.id.clone());
        state.worker = Some(Identity::current().unwrap());
        state.host = Some(Identity::current().unwrap());
        state.checked_at = Some(Utc::now());
        write(&dir, &state).unwrap();
        guard.set_key(seat_authority::GRANT_ENV, &child.id);
        guard.set_key("AIDA_USER", &child.subject);
        guard.set_key("AIDA_AGENT_NAME", &child.subject);
        CONTROLLER.with(|id| *id.borrow_mut() = Some(child.id));
        ControllerScope
    }
    fn change(id: u64) -> forge::Change {
        forge::Change {
            id,
            head: format!("{id:040x}"),
            base: "main".into(),
            author: "writer".into(),
            gitlab: false,
            overlap: Some(false),
            behind: Some(0),
            head_branch: None,
        }
    }

    // Foreign process coordinates are uncertainty, never deputy death evidence.
    // trace:STORY-2571 | ai:codex
    #[test]
    fn story2571_foreign_host_is_unknown_and_refuses_admission() {
        let (temp, _guard, parent) = fixture();
        rebind(temp.path(), "default", &parent).unwrap();
        let dir = directory(temp.path(), false).unwrap().unwrap();
        let mut state = read(&dir).unwrap().unwrap();
        let mut host = Identity::current().unwrap();
        host.pid_namespace = Some(u64::MAX);
        state.host = Some(host);
        state.checked_at = Some(Utc::now());
        assert_eq!(host_refusal(&state), Some("unknown"));
        assert!(!host_fresh(&state));
        state.checked_at = Some(Utc::now() - chrono::Duration::seconds(30));
        assert_eq!(host_refusal(&state), Some("unknown"));
        state.host_active = false;
        assert_eq!(host_refusal(&state), Some("blocked-deputy"));
    }

    // Retiring a controller must not invalidate keys inherited by descendants.
    // trace:STORY-2571 | ai:codex
    #[test]
    fn story2571_retired_controller_key_survives_until_reviews_finish() {
        let (temp, _guard, parent) = fixture();
        let root = temp.path();
        rebind(root, "default", &parent).unwrap();
        let dir = directory(root, false).unwrap().unwrap();
        let store = Store::open(root, "github:github.com/owner/repo").unwrap();
        assert!(matches!(
            select(&store, &change(1), None, 2).unwrap(),
            Action::Dispatch(_)
        ));
        let grant =
            seat_authority::issue_review_watch_child(root, &parent.id, &parent.subject).unwrap();
        let reviewer = seat_authority::issue_child_from(
            grant.clone(),
            "reviewer",
            "key-retention-reviewer",
            false,
        )
        .unwrap();
        let mut state = read(&dir).unwrap().unwrap();
        state.retired_grants.push(grant.id.clone());
        write(&dir, &state).unwrap();
        tick(root, "default", false, &mut None).unwrap();
        if crate::grant_keyring::available() {
            for id in [&grant.id, &reviewer.id] {
                assert!(matches!(
                    crate::grant_keyring::read(id),
                    crate::grant_keyring::KeyRead::Held(_)
                ));
            }
        }
        assert!(seat_authority::read_grant(&grant.id)
            .unwrap()
            .revoked_at
            .is_none());
        store.release("PR-1").unwrap();
        tick(root, "default", false, &mut None).unwrap();
        assert!(seat_authority::read_grant(&grant.id)
            .unwrap()
            .revoked_at
            .is_some());
        assert!(!matches!(
            crate::grant_keyring::read(&grant.id),
            crate::grant_keyring::KeyRead::Held(_)
        ));
        seat_authority::revoke_by_id(&reviewer.id).unwrap();
    }

    // Ordinary watchers can advance old supersede work after losing delegation.
    // trace:STORY-2571 | ai:codex
    #[test]
    fn story2571_ordinary_watcher_supersedes_without_delegation() {
        let (temp, mut guard, _parent) = fixture();
        let root = temp.path();
        for args in [
            vec!["init", "-q"],
            vec![
                "remote",
                "add",
                "origin",
                "https://github.com/owner/repo.git",
            ],
        ] {
            assert!(Command::new("git")
                .current_dir(root)
                .args(args)
                .status()
                .unwrap()
                .success());
        }
        let repo = forge::Repository::load(root).unwrap();
        let store = Store::open(root, &repo.identity).unwrap();
        assert!(matches!(
            select(&store, &change(1), None, 2).unwrap(),
            Action::Dispatch(_)
        ));
        let mut reviews = store.load().unwrap();
        let attempt = &mut reviews.entries.get_mut("PR-1").unwrap().attempts[0];
        attempt.phase = Phase::Superseding;
        attempt.supersede = Some(Supersede {
            by: change(2).head,
            requested_at: Utc::now(),
            term_at: None,
            kill_at: None,
            diagnostic: None,
        });
        store
            .dir
            .write(STATE, &serde_json::to_vec_pretty(&reviews).unwrap())
            .unwrap();
        guard.set_key(seat_authority::GRANT_ENV, "missing-grant");
        assert!(seat_authority::validate_child_delegation(root, "reviewer").is_err());
        watch_change(
            root,
            &repo,
            &store,
            &ReviewWatchArgs {
                once: true,
                interval: 10,
                max_concurrent: Some(2),
                vendor: None,
                release: None,
                launch_reservation: None,
            },
            &mut Vec::new(),
            &update::AuthorLiveness::default(),
            change(1),
            None,
        )
        .unwrap();
        assert!(store.load().unwrap().entries["PR-1"].attempts[0]
            .supersede
            .as_ref()
            .unwrap()
            .diagnostic
            .is_some());
    }

    // Failures before spawn must never spin or leak newly minted authority.
    // trace:STORY-2571 | ai:codex
    #[test]
    fn story2571_pre_spawn_failures_back_off_and_revoke() {
        for failure in ["log", "launch", "reservation", "spawn"] {
            let (temp, _guard, parent) = fixture();
            let root = temp.path();
            rebind(root, "default", &parent).unwrap();
            let dir = directory(root, false).unwrap().unwrap();
            if failure == "log" {
                std::fs::create_dir(dir.path().join("service.log")).unwrap();
            }
            let mut minted = None;
            let mut child = None;
            let result = tick_with_launcher(root, "default", true, &mut child, |root, parent| {
                assert_ne!(failure, "log", "log failure must precede minting");
                if failure == "launch" {
                    anyhow::bail!("launcher failed");
                }
                let grant =
                    seat_authority::issue_review_watch_child(root, &parent.id, &parent.subject)?;
                minted = Some(grant.id.clone());
                if failure == "reservation" {
                    // Replacing the receipt with a directory makes reservation and
                    // recovery persistence fail; revocation must still happen.
                    std::fs::remove_file(dir.path().join(SERVICE))?;
                    std::fs::create_dir(dir.path().join(SERVICE))?;
                }
                Ok((grant, Command::new("/nonexistent/aida-review-watch-test")))
            });
            assert!(result.is_err());
            assert!(child.is_none());
            if let Some(id) = minted {
                assert!(seat_authority::read_grant(&id)
                    .unwrap()
                    .revoked_at
                    .is_some());
            }
            if failure != "reservation" {
                let state = read(&dir).unwrap().unwrap();
                assert!(state.child_grant.is_none());
                assert!(state.retry_after > Utc::now());
            }
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn story2571_reaped_exit_and_confirmed_dead_revoke_controller_grant() {
        for reap_in_tick in [true, false] {
            let (temp, _guard, parent) = fixture();
            let root = temp.path();
            rebind(root, "default", &parent).unwrap();
            let dir = directory(root, false).unwrap().unwrap();
            let grant = seat_authority::issue_review_watch_child(root, &parent.id, &parent.subject)
                .unwrap();
            let mut process = Command::new("sh").args(["-c", "exit 7"]).spawn().unwrap();
            let mut state = read(&dir).unwrap().unwrap();
            state.worker = Some(Identity::child(&process).unwrap());
            state.child_grant = Some(grant.id.clone());
            write(&dir, &state).unwrap();
            if !reap_in_tick {
                process.wait().unwrap();
            }
            let mut child = reap_in_tick.then_some(process);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            loop {
                tick(root, "default", false, &mut child).unwrap();
                if read(&dir).unwrap().unwrap().child_grant.is_none() {
                    break;
                }
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert!(seat_authority::read_grant(&grant.id)
                .unwrap()
                .revoked_at
                .is_some());
            assert!(child.is_none());
            let stopped = read(&dir).unwrap().unwrap();
            assert!(stopped.worker.is_none());
            if reap_in_tick {
                assert_eq!(stopped.last_exit, Some(7));
            }
            assert!(stopped.retry_after > Utc::now());
        }
    }

    // Repeated startup exits behind an unclaimed reservation retain keys
    // and stop growing retained ancestry, without revoking a live reviewer.
    // trace:STORY-2571 | ai:codex
    #[test]
    fn story2571_repeated_controller_exits_retain_keys_until_cleanup() {
        let (temp, _guard, parent) = fixture();
        let root = temp.path();
        rebind(root, "default", &parent).unwrap();
        let dir = directory(root, false).unwrap().unwrap();
        let store = Store::open(root, "github:github.com/owner/repo").unwrap();
        assert!(matches!(
            select(&store, &change(1), None, 2).unwrap(),
            Action::Dispatch(_)
        ));
        let mut controller = None;
        let mut grants = Vec::new();
        let mut reviewer: Option<SeatGrant> = None;
        let mut installed_keys = 0;
        for _ in 0..MAX_RETIRED_GRANTS * 3 {
            let mut state = read(&dir).unwrap().unwrap();
            state.retry_after = Utc::now() - chrono::Duration::seconds(1);
            write(&dir, &state).unwrap();
            tick_with_launcher(root, "default", true, &mut controller, |root, parent| {
                let grant =
                    seat_authority::issue_review_watch_child(root, &parent.id, &parent.subject)?;
                if crate::grant_keyring::available() {
                    assert!(matches!(
                        crate::grant_keyring::read(&grant.id),
                        crate::grant_keyring::KeyRead::Held(_)
                    ));
                    installed_keys += 1;
                }
                if reviewer.is_none() {
                    reviewer = Some(seat_authority::issue_child_from(
                        grant.clone(),
                        "reviewer",
                        "surviving-reviewer",
                        false,
                    )?);
                }
                grants.push(grant.id.clone());
                let mut command = Command::new("sh");
                command.args(["-c", "exit 7"]);
                Ok((grant, command))
            })
            .unwrap();
            if let Some(child) = controller.as_mut() {
                child.wait().unwrap();
            }
            // Observe exit through the production tick and retain the orphan.
            tick(root, "default", true, &mut controller).unwrap();
            assert!(controller.is_none());
            let state = read(&dir).unwrap().unwrap();
            assert_eq!(state.retired_grants, grants);
            assert!(state.retired_grants.len() <= MAX_RETIRED_GRANTS);
            assert!(state.child_grant.is_none());
            for id in &grants {
                if crate::grant_keyring::available() {
                    assert!(matches!(
                        crate::grant_keyring::read(id),
                        crate::grant_keyring::KeyRead::Held(_)
                    ));
                }
                assert!(seat_authority::read_grant(id).unwrap().revoked_at.is_none());
            }
            let reviewer = reviewer.as_ref().unwrap();
            assert!(seat_authority::validated_grant(
                root,
                &reviewer.id,
                &reviewer.subject,
                Utc::now()
            )
            .is_some());
            if crate::grant_keyring::available() {
                assert!(matches!(
                    crate::grant_keyring::read(&reviewer.id),
                    crate::grant_keyring::KeyRead::Held(_)
                ));
            }
        }
        assert_eq!(grants.len(), MAX_RETIRED_GRANTS);
        assert_eq!(
            store.load().unwrap().entries["PR-1"].attempts[0].phase,
            Phase::Reserved
        );
        assert_eq!(review_blockers(&dir).unwrap().len(), 1);
        if installed_keys == 0 {
            eprintln!("kernel keyring unavailable; exercised bounded process-bound grants");
        }

        // Operator recovery clears the unclaimed reservation and lets cleanup
        // revoke the retired records before admission resumes.
        assert_eq!(store.release("PR-1").unwrap(), 1);
        tick_with_launcher(root, "default", true, &mut controller, |root, parent| {
            for id in &grants {
                assert!(!matches!(
                    crate::grant_keyring::read(id),
                    crate::grant_keyring::KeyRead::Held(_)
                ));
            }
            assert!(grants
                .iter()
                .all(|id| seat_authority::read_grant(id).unwrap().revoked_at.is_some()));
            let grant =
                seat_authority::issue_review_watch_child(root, &parent.id, &parent.subject)?;
            let mut command = Command::new("sh");
            command.args(["-c", "exit 0"]);
            Ok((grant, command))
        })
        .unwrap();
        assert!(read(&dir).unwrap().unwrap().retired_grants.is_empty());
        assert!(controller.is_some(), "recovery must restore admission");
        controller.as_mut().unwrap().wait().unwrap();
        tick(root, "default", false, &mut controller).unwrap();
        seat_authority::revoke_by_id(&reviewer.unwrap().id).unwrap();
    }

    // Exercise the actual reviewer ancestry, not just the controller record.
    // trace:STORY-2571 | ai:codex
    #[test]
    fn story2571_live_reviewer_survives_controller_reap_and_rotation() {
        for reap_in_tick in [true, false] {
            for rotate in [true, false] {
                let (temp, _guard, parent) = fixture();
                let root = temp.path();
                rebind(root, "default", &parent).unwrap();
                let dir = directory(root, false).unwrap().unwrap();
                let grant =
                    seat_authority::issue_review_watch_child(root, &parent.id, &parent.subject)
                        .unwrap();
                let reviewer = seat_authority::issue_child_from(
                    grant.clone(),
                    "reviewer",
                    "live-reviewer",
                    false,
                )
                .unwrap();
                assert_eq!(reviewer.parent_grant_id.as_ref(), Some(&grant.id));
                let mut process = Command::new("sh").args(["-c", "exit 7"]).spawn().unwrap();
                let mut state = read(&dir).unwrap().unwrap();
                state.worker = Some(Identity::child(&process).unwrap());
                state.child_grant = Some(grant.id.clone());
                write(&dir, &state).unwrap();
                let store = Store::open(root, "github:github.com/owner/repo").unwrap();
                assert!(matches!(
                    select(&store, &change(1), None, 2).unwrap(),
                    Action::Dispatch(_)
                ));
                let mut reviews = store.load().unwrap();
                let attempt = &mut reviews.entries.get_mut("PR-1").unwrap().attempts[0];
                attempt.phase = Phase::Running;
                attempt.identity = Some(Identity::current().unwrap());
                attempt.reviewer_spawning = true;
                attempt.reviewer = Some(Identity::current().unwrap());
                dir.write(STATE, &serde_json::to_vec_pretty(&reviews).unwrap())
                    .unwrap();
                if rotate {
                    rebind(root, "default", &successor(root, &parent)).unwrap();
                }
                if !reap_in_tick {
                    process.wait().unwrap();
                }
                let mut child = reap_in_tick.then_some(process);
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
                loop {
                    tick(root, "default", false, &mut child).unwrap();
                    if child.is_none() {
                        break;
                    }
                    assert!(std::time::Instant::now() < deadline);
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                assert!(
                    seat_authority::validated_grant(
                        root,
                        &reviewer.id,
                        &reviewer.subject,
                        Utc::now()
                    )
                    .is_some(),
                    "reviewer must retain full-chain authority after controller exit"
                );
                assert_eq!(
                    read(&dir).unwrap().unwrap().retired_grants,
                    vec![grant.id.clone()]
                );
                // An unrecorded spawn is also protected across supervisor ticks.
                let attempt = &mut reviews.entries.get_mut("PR-1").unwrap().attempts[0];
                attempt.reviewer = None;
                dir.write(STATE, &serde_json::to_vec_pretty(&reviews).unwrap())
                    .unwrap();
                tick(root, "default", false, &mut child).unwrap();
                assert!(seat_authority::validated_grant(
                    root,
                    &reviewer.id,
                    &reviewer.subject,
                    Utc::now()
                )
                .is_some());
                // Once the review is terminal and all identities are gone,
                // revocation and ordinary restart backoff may proceed.
                let attempt = &mut reviews.entries.get_mut("PR-1").unwrap().attempts[0];
                attempt.phase = Phase::Exited;
                attempt.identity = None;
                attempt.reviewer_spawning = false;
                dir.write(STATE, &serde_json::to_vec_pretty(&reviews).unwrap())
                    .unwrap();
                tick(root, "default", false, &mut child).unwrap();
                assert!(read(&dir).unwrap().unwrap().child_grant.is_none());
                assert!(seat_authority::read_grant(&grant.id)
                    .unwrap()
                    .revoked_at
                    .is_some());
            }
        }
    }

    // Both crash windows previously prevented all replacement controllers.
    // Exercise the real supervisor spawn boundary, retaining the old grant
    // until the same descendant evidence used in production proves it safe.
    // trace:STORY-2571 | ai:codex
    #[test]
    fn story2571_crash_mid_superseding_or_unpinned_reservation_restarts() {
        for (superseding, retained_child) in
            [(true, false), (false, false), (true, true), (false, true)]
        {
            let (temp, _guard, parent) = fixture();
            let root = temp.path();
            rebind(root, "default", &parent).unwrap();
            let dir = directory(root, false).unwrap().unwrap();
            let grant = seat_authority::issue_review_watch_child(root, &parent.id, &parent.subject)
                .unwrap();
            let reviewer = seat_authority::issue_child_from(
                grant.clone(),
                "reviewer",
                "surviving-reviewer",
                false,
            )
            .unwrap();
            let mut process = Command::new("sh").args(["-c", "exit 7"]).spawn().unwrap();
            let exited_identity = Identity::child(&process).unwrap();
            process.wait().unwrap();
            let mut state = read(&dir).unwrap().unwrap();
            state.worker = Some(exited_identity.clone());
            state.child_grant = Some(grant.id.clone());
            write(&dir, &state).unwrap();

            let store = Store::open(root, "github:github.com/owner/repo").unwrap();
            let Action::Dispatch(reservation) = select(&store, &change(1), None, 2).unwrap() else {
                panic!("expected reservation")
            };
            let mut reviews = store.load().unwrap();
            if superseding {
                let attempt = &mut reviews.entries.get_mut("PR-1").unwrap().attempts[0];
                attempt.phase = Phase::Superseding;
                attempt.identity = Some(exited_identity.clone());
                attempt.reviewer_spawning = true;
                attempt.reviewer = Some(Identity::current().unwrap());
                attempt.supersede = Some(Supersede {
                    by: change(2).head,
                    requested_at: Utc::now(),
                    term_at: None,
                    kill_at: None,
                    diagnostic: None,
                });
                dir.write(STATE, &serde_json::to_vec_pretty(&reviews).unwrap())
                    .unwrap();
            }
            // Cover both recovery from the persisted dead identity and the
            // live supervisor observing its retained Child's exit status.
            let mut controller = if retained_child { Some(process) } else { None };
            tick(root, "default", true, &mut controller).unwrap();
            let mut retired = read(&dir).unwrap().unwrap();
            assert!(retired.worker.is_none());
            assert!(retired.child_grant.is_none());
            assert_eq!(retired.retired_grants, vec![grant.id.clone()]);
            assert!(retired.retry_after > Utc::now());
            assert!(controller.is_none());
            assert!(review_blockers(&dir)
                .unwrap()
                .iter()
                .any(|b| b.contains("PR-1") && b.contains(&reservation.token)));

            // Advance the clock past normal backoff without sleeping. Exercise
            // production supervision and spawn with a harmless controller;
            // drive restart reconciliation explicitly below.
            retired.retry_after = Utc::now() - chrono::Duration::seconds(1);
            write(&dir, &retired).unwrap();
            tick_with_launcher(root, "default", true, &mut controller, |root, parent| {
                let grant =
                    seat_authority::issue_review_watch_child(root, &parent.id, &parent.subject)?;
                let mut command = Command::new("sh");
                command.args(["-c", "exit 0"]);
                Ok((grant, command))
            })
            .unwrap();
            let running = read(&dir).unwrap().unwrap();
            assert!(
                controller.is_some(),
                "unresolved attempts must permit replacement"
            );
            assert!(running.worker.is_some());
            assert_ne!(running.child_grant.as_ref(), Some(&grant.id));
            assert_eq!(running.retired_grants, vec![grant.id.clone()]);
            assert!(
                seat_authority::validated_grant(root, &reviewer.id, &reviewer.subject, Utc::now(),)
                    .is_some(),
                "replacement must preserve reviewer ancestry"
            );
            controller.as_mut().unwrap().wait().unwrap();

            if superseding {
                // The final reviewer has now exited. Restart reconciliation
                // finishes persisted intent without needing the old watcher.
                let attempt = &mut reviews.entries.get_mut("PR-1").unwrap().attempts[0];
                attempt.reviewer = Some(exited_identity);
                dir.write(STATE, &serde_json::to_vec_pretty(&reviews).unwrap())
                    .unwrap();
                tick(root, "default", false, &mut controller).unwrap();
                assert_eq!(
                    store.load().unwrap().entries["PR-1"].attempts[0].phase,
                    Phase::Superseded
                );
                assert!(seat_authority::read_grant(&grant.id)
                    .unwrap()
                    .revoked_at
                    .is_some());
                assert!(read(&dir).unwrap().unwrap().retired_grants.is_empty());
            } else {
                // No dispatcher receipt could mean a spawn/persist crash.
                // Keep that capacity and old grant, but never block restart.
                tick(root, "default", false, &mut controller).unwrap();
                assert_eq!(
                    store.load().unwrap().entries["PR-1"].attempts[0].phase,
                    Phase::Reserved
                );
                assert!(seat_authority::read_grant(&grant.id)
                    .unwrap()
                    .revoked_at
                    .is_none());
            }
        }
    }

    // Restart itself completes persisted supersede intent when no descendant
    // remains; the replacement must not be needed to release the old grant.
    // trace:STORY-2571 | ai:codex
    #[test]
    fn story2571_restart_finishes_dead_superseding_before_launch() {
        let (temp, _guard, parent) = fixture();
        let root = temp.path();
        rebind(root, "default", &parent).unwrap();
        let dir = directory(root, false).unwrap().unwrap();
        let grant =
            seat_authority::issue_review_watch_child(root, &parent.id, &parent.subject).unwrap();
        let mut process = Command::new("sh").args(["-c", "exit 7"]).spawn().unwrap();
        let identity = Identity::child(&process).unwrap();
        process.wait().unwrap();
        let mut service = read(&dir).unwrap().unwrap();
        service.worker = Some(identity.clone());
        service.child_grant = Some(grant.id.clone());
        write(&dir, &service).unwrap();

        let store = Store::open(root, "github:github.com/owner/repo").unwrap();
        assert!(matches!(
            select(&store, &change(1), None, 2).unwrap(),
            Action::Dispatch(_)
        ));
        let mut reviews = store.load().unwrap();
        let attempt = &mut reviews.entries.get_mut("PR-1").unwrap().attempts[0];
        attempt.phase = Phase::Superseding;
        attempt.identity = Some(identity);
        attempt.supersede = Some(Supersede {
            by: change(2).head,
            requested_at: Utc::now(),
            term_at: None,
            kill_at: None,
            diagnostic: None,
        });
        dir.write(STATE, &serde_json::to_vec_pretty(&reviews).unwrap())
            .unwrap();

        let mut controller = None;
        tick(root, "default", true, &mut controller).unwrap();
        assert_eq!(
            store.load().unwrap().entries["PR-1"].attempts[0].phase,
            Phase::Superseded
        );
        assert!(seat_authority::read_grant(&grant.id)
            .unwrap()
            .revoked_at
            .is_some());
        let mut retired = read(&dir).unwrap().unwrap();
        assert!(retired.retired_grants.is_empty());
        assert!(controller.is_none());
        retired.retry_after = Utc::now() - chrono::Duration::seconds(1);
        write(&dir, &retired).unwrap();
        tick_with_launcher(root, "default", true, &mut controller, |root, parent| {
            let grant =
                seat_authority::issue_review_watch_child(root, &parent.id, &parent.subject)?;
            let mut command = Command::new("sh");
            command.args(["-c", "exit 0"]);
            Ok((grant, command))
        })
        .unwrap();
        controller
            .as_mut()
            .expect("replacement started")
            .wait()
            .unwrap();
    }

    #[test]
    fn story2571_sub_advisor_does_not_narrow_binding_before_rotation() {
        let temp = tempfile::tempdir().unwrap();
        let mut guard = crate::test_env::AmbientGuard::hermetic_with_seat(
            temp.path(),
            "advisor",
            &["advisor", "reviewer", "implementer"],
        );
        let root = temp.path();
        let operator = seat_authority::grant_check(root).into_valid().unwrap();
        rebind(root, "default", &operator).unwrap();
        let advisor =
            seat_authority::issue_child_from(operator.clone(), "advisor", "first-advisor", false)
                .unwrap();
        rebind_after_launch_with(root, &advisor, false, |d, r, g, _| rebind(r, d, g)).unwrap();
        let sub =
            seat_authority::issue_child_from(advisor.clone(), "advisor", "sub-advisor", false)
                .unwrap();
        // Explicitly narrow the sub-advisor's delegation as a real --delegate-seat launch does.
        guard.set_key(seat_authority::GRANT_ENV, &advisor.id);
        guard.set_key("AIDA_USER", &advisor.subject);
        guard.set_key("AIDA_AGENT_NAME", &advisor.subject);
        let narrow = seat_authority::issue_launch_child_with_delegation(
            root,
            "advisor",
            "narrow-advisor",
            false,
            &["advisor".into(), "reviewer".into()],
        )
        .unwrap();
        for sub in [sub, narrow] {
            assert!(
                rebind_after_launch_with(root, &sub, false, |d, r, g, _| rebind(r, d, g))
                    .unwrap_err()
                    .to_string()
                    .contains("sub-advisor")
            );
        }
        let next = seat_authority::issue_child_from(operator, "advisor", "rotated-advisor", false)
            .unwrap();
        rebind_after_launch_with(root, &next, false, |d, r, g, _| rebind(r, d, g)).unwrap();
        let dir = directory(root, false).unwrap().unwrap();
        assert_eq!(read(&dir).unwrap().unwrap().binding.grant, next.id);
        assert_eq!(
            read(&dir).unwrap().unwrap().binding.delegations,
            advisor.delegable_seats
        );
    }

    #[test]
    fn story2571_controller_select_after_rotation_stops_pass() {
        let (temp, mut guard, parent) = fixture();
        let root = temp.path();
        let _scope = controller(root, &mut guard, &parent);
        check_controller(root).unwrap();
        seat_authority::validate_child_delegation(root, "reviewer").unwrap();
        let store = Store::open(root, "github:github.com/owner/repo").unwrap();
        assert!(matches!(
            select(&store, &change(1), None, 2).unwrap(),
            Action::Dispatch(_)
        ));
        let next = successor(root, &parent);
        rebind(root, "default", &next).unwrap();
        let mut visited = Vec::new();
        let error = watch_changes(&store, vec![change(2), change(3)], |c| {
            visited.push(c.id);
            select(&store, &c, None, 2).map(|_| ())
        })
        .unwrap_err();
        assert!(stops_watcher(&error), "{error:#}");
        assert!(format!("{error:#}").contains("superseded"));
        assert_eq!(visited, [2]);
        assert!(stops_watcher(&check_controller(root).unwrap_err()));
    }

    #[test]
    fn story2571_watch_change_checks_controller_reviewer_delegation_before_forge() {
        let (temp, mut guard, parent) = fixture();
        let root = temp.path();
        let narrow = seat_authority::issue_launch_child_with_delegation(
            root,
            "advisor",
            "narrow-controller",
            false,
            &["advisor".into()],
        )
        .unwrap();
        let _scope = controller(root, &mut guard, &parent);
        let dir = directory(root, false).unwrap().unwrap();
        let mut state = read(&dir).unwrap().unwrap();
        state.child_grant = Some(narrow.id.clone());
        write(&dir, &state).unwrap();
        guard.set_key(seat_authority::GRANT_ENV, &narrow.id);
        guard.set_key("AIDA_USER", &narrow.subject);
        guard.set_key("AIDA_AGENT_NAME", &narrow.subject);
        CONTROLLER.with(|id| *id.borrow_mut() = Some(narrow.id.clone()));
        check_controller(root).unwrap();
        for args in [
            vec!["init", "-q"],
            vec![
                "remote",
                "add",
                "origin",
                "https://github.com/owner/repo.git",
            ],
        ] {
            assert!(Command::new("git")
                .current_dir(root)
                .args(args)
                .status()
                .unwrap()
                .success());
        }
        let repo = forge::Repository::load(root).unwrap();
        let store = Store::open(root, &repo.identity).unwrap();
        let args = ReviewWatchArgs {
            once: true,
            interval: 10,
            max_concurrent: Some(2),
            vendor: None,
            release: None,
            launch_reservation: None,
        };
        let error = watch_change(
            root,
            &repo,
            &store,
            &args,
            &mut Vec::new(),
            &update::AuthorLiveness::default(),
            change(1),
            None,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("reviewer"), "{error:#}");
        assert!(!stops_watcher(&error), "delegation refusal remains per-PR");
        assert!(
            store.load().unwrap().entries.is_empty(),
            "no PR admitted before delegation validation"
        );
    }

    #[test]
    fn story2571_launcher_bootstraps_and_takes_over_operator_binding() {
        let (temp, _guard, operator) = fixture();
        let root = temp.path();
        assert!(Command::new("git")
            .current_dir(root)
            .args(["init", "-q"])
            .status()
            .unwrap()
            .success());
        // An unused service with no deputy must not warn on advisor launch.
        rebind_after_launch(root, &operator).unwrap();
        rebind_after_launch_with(root, &operator, false, |_, _, _, _| {
            panic!("no deputy or service: do not send a request")
        })
        .unwrap();
        assert!(
            rebind_after_launch_with(root, &operator, true, |_, _, _, configured| {
                assert!(!configured);
                anyhow::bail!("deputy answered with a refusal")
            })
            .is_err()
        );
        let send =
            |deputy: &str, root: &Path, grant: &SeatGrant, _: bool| rebind(root, deputy, grant);
        rebind_after_launch_with(root, &operator, true, send).unwrap();
        assert!(
            rebind_after_launch_with(root, &operator, false, |_, _, _, configured| {
                assert!(configured);
                anyhow::bail!("bound deputy unavailable")
            })
            .is_err()
        );
        let next = seat_authority::issue_launch_child_with_delegation(
            root,
            "advisor",
            "new-advisor-handle",
            false,
            &[],
        )
        .unwrap();
        rebind_after_launch_with(root, &next, false, send).unwrap();
        let dir = directory(root, false).unwrap().unwrap();
        assert_eq!(
            read(&dir).unwrap().unwrap().binding.subject,
            "new-advisor-handle"
        );
        assert!(rebind_after_launch_with(root, &operator, true, send).is_err());
        assert_eq!(read(&dir).unwrap().unwrap().binding.grant, next.id);
        let source = include_str!("agent_launch.rs");
        assert!(source.contains("service::rebind_after_launch(&plan.project_root, &grant)"));
    }

    #[test]
    fn story2571_unchanged_tick_does_not_replace_service_or_marker() {
        use std::os::unix::fs::MetadataExt;
        let (temp, _guard, parent) = fixture();
        rebind(temp.path(), "default", &parent).unwrap();
        let dir = directory(temp.path(), false).unwrap().unwrap();
        let mut child = None;
        tick(temp.path(), "default", false, &mut child).unwrap();
        let receipt = std::fs::metadata(dir.path().join(SERVICE)).unwrap();
        let marker = std::fs::metadata(dir.path().join("service-initialized")).unwrap();
        tick(temp.path(), "default", false, &mut child).unwrap();
        assert_eq!(
            std::fs::metadata(dir.path().join(SERVICE)).unwrap().ino(),
            receipt.ino()
        );
        assert_eq!(
            std::fs::metadata(dir.path().join("service-initialized"))
                .unwrap()
                .modified()
                .unwrap(),
            marker.modified().unwrap()
        );
    }

    // The real persisted binding moves forward while an active/ambiguous
    // controller receipt and ordinary PR reservations survive untouched.
    #[test]
    fn story2571_rotation_rebind_preserves_receipts_and_refuses_stale_successor() {
        let (temp, _guard, first) = fixture();
        let root = temp.path();
        rebind(root, "default", &first).unwrap();
        let dir = directory(root, false).unwrap().unwrap();
        let mut state = read(&dir).unwrap().unwrap();
        state.worker = Some(Identity::current().unwrap());
        let child =
            seat_authority::issue_review_watch_child(root, &first.id, &first.subject).unwrap();
        state.child_grant = Some(child.id.clone());
        check_binding(root, &state, &child).unwrap();
        write(&dir, &state).unwrap();
        let store = Store::open(root, "github:github.com/owner/repo").unwrap();
        let change = forge::Change {
            id: 1,
            head: "1".repeat(40),
            base: "main".into(),
            author: "writer".into(),
            gitlab: false,
            overlap: Some(false),
            behind: Some(0),
            head_branch: None,
        };
        assert!(matches!(
            select(&store, &change, None, 2).unwrap(),
            Action::Dispatch(_)
        ));
        let before = std::fs::read(root.join(".aida/review-watch/state.json")).unwrap();
        let next = successor(root, &first);
        rebind(root, "default", &next).unwrap();
        let rebound = read(&dir).unwrap().unwrap();
        assert_eq!(rebound.binding.grant, next.id);
        assert_eq!(rebound.worker, state.worker);
        assert_eq!(rebound.child_grant, state.child_grant);
        assert!(check_binding(root, &rebound, &child)
            .unwrap_err()
            .to_string()
            .contains("superseded"));
        let mut replacement = None;
        tick(root, "default", true, &mut replacement).unwrap();
        assert!(
            replacement.is_none(),
            "a live prior controller must block replacement"
        );
        reconcile_reviews(root, &dir).unwrap();
        assert!(rebind(root, "default", &first)
            .unwrap_err()
            .to_string()
            .contains("stale"));
        assert_eq!(read(&dir).unwrap().unwrap().binding.grant, next.id);
        assert_eq!(
            std::fs::read(root.join(".aida/review-watch/state.json")).unwrap(),
            before
        );
    }

    #[test]
    // Rotation must recover from an invalid old parent without borrowing its
    // authority or discarding unresolved reviews. trace:STORY-2571 | ai:codex
    fn story2571_rotation_recovers_revoked_parent_after_controller_exit() {
        let (temp, _guard, first) = fixture();
        let root = temp.path();
        rebind(root, "default", &first).unwrap();
        let dir = directory(root, false).unwrap().unwrap();
        let child =
            seat_authority::issue_review_watch_child(root, &first.id, &first.subject).unwrap();
        let mut process = Command::new("sh").args(["-c", "exit 0"]).spawn().unwrap();
        let mut state = read(&dir).unwrap().unwrap();
        state.worker = Some(Identity::child(&process).unwrap());
        state.child_grant = Some(child.id.clone());
        write(&dir, &state).unwrap();
        process.wait().unwrap();

        let store = Store::open(root, "github:github.com/owner/repo").unwrap();
        assert!(matches!(
            select(&store, &change(1), None, 2).unwrap(),
            Action::Dispatch(_)
        ));
        let reservations = std::fs::read(dir.path().join(STATE)).unwrap();
        let next = successor(root, &first);
        seat_authority::revoke_by_id(&first.id).unwrap();
        assert!(state.binding.validate(root).is_err());
        rebind(root, "default", &next).unwrap();

        let mut controller = None;
        tick(root, "default", true, &mut controller).unwrap();
        let rebound = read(&dir).unwrap().unwrap();
        assert_eq!(rebound.binding.validate(root).unwrap().id, next.id);
        assert!(rebound.worker.is_none());
        assert!(rebound.child_grant.is_none());
        assert_eq!(rebound.retired_grants, vec![child.id.clone()]);
        assert!(controller.is_none(), "restart observes ordinary backoff");
        reconcile_reviews(root, &dir).unwrap();
        assert_eq!(std::fs::read(dir.path().join(STATE)).unwrap(), reservations);
        let mut reviews = store.load().unwrap();
        reviews.entries.get_mut("PR-1").unwrap().attempts[0].phase = Phase::Exited;
        dir.write(STATE, &serde_json::to_vec_pretty(&reviews).unwrap())
            .unwrap();
        tick(root, "default", true, &mut controller).unwrap();
        let stopped = read(&dir).unwrap().unwrap();
        assert!(stopped.child_grant.is_none());
        assert!(stopped.retry_after > Utc::now());
        assert!(seat_authority::read_grant(&child.id)
            .unwrap()
            .revoked_at
            .is_some());
        assert!(rebind(root, "default", &first).is_err());
        assert_eq!(read(&dir).unwrap().unwrap().binding.grant, next.id);
    }

    #[test]
    fn story2571_revoked_successor_and_widening_refuse_without_mutation() {
        let (temp, _guard, first) = fixture();
        rebind(temp.path(), "default", &first).unwrap();
        let next = successor(temp.path(), &first);
        seat_authority::revoke_by_id(&next.id).unwrap();
        assert!(rebind(temp.path(), "default", &next).is_err());
        let mut wide = Binding::from_grant(&first);
        wide.grant = "another".into();
        wide.issued_at += chrono::Duration::seconds(1);
        wide.delegations.push("implementer".into());
        assert!(wide.succeeds(&Binding::from_grant(&first)).is_err());
        let dir = directory(temp.path(), false).unwrap().unwrap();
        assert_eq!(read(&dir).unwrap().unwrap().binding.grant, first.id);
    }

    #[test]
    fn story2571_ambiguous_spawn_is_not_replaced_and_unsafe_state_refuses() {
        use std::os::unix::fs::symlink;
        let (temp, _guard, first) = fixture();
        rebind(temp.path(), "default", &first).unwrap();
        let dir = directory(temp.path(), false).unwrap().unwrap();
        let mut state = read(&dir).unwrap().unwrap();
        state.child_grant = Some("ambiguous".into());
        write(&dir, &state).unwrap();
        let mut child = None;
        tick(temp.path(), "default", true, &mut child).unwrap();
        assert!(child.is_none());
        assert_eq!(
            read(&dir).unwrap().unwrap().child_grant.as_deref(),
            Some("ambiguous")
        );
        std::fs::remove_file(dir.path().join(SERVICE)).unwrap();
        symlink("missing", dir.path().join(SERVICE)).unwrap();
        assert!(rebind(temp.path(), "default", &first).is_err());
    }

    #[test]
    fn story2571_pr_refusal_keeps_next_pr_and_next_pass_available() {
        let (temp, mut guard, first) = fixture();
        let _scope = controller(temp.path(), &mut guard, &first);
        let store = Store::open(temp.path(), "github:github.com/owner/repo").unwrap();
        let change = |id| forge::Change {
            id,
            head: format!("{id:040x}"),
            base: "main".into(),
            author: "writer".into(),
            gitlab: false,
            overlap: Some(false),
            behind: Some(0),
            head_branch: None,
        };
        let mut handled = Vec::new();
        for _ in 0..2 {
            watch_changes(&store, vec![change(1), change(2)], |c| {
                check_controller(temp.path())?;
                seat_authority::validate_child_delegation(temp.path(), "reviewer")?;
                let mut c = c;
                if c.id == 1 {
                    c.overlap = None;
                }
                select(&store, &c, None, 2)?;
                handled.push(c.id);
                Ok(())
            })
            .unwrap();
        }
        assert_eq!(handled, [2, 2]);
        let dir = directory(temp.path(), false).unwrap().unwrap();
        assert_eq!(read(&dir).unwrap().unwrap().binding.grant, first.id);
    }
}
