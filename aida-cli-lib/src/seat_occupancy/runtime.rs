//! Real-process bindings for the seat state machine (TASK-1607): who is
//! calling, the system probe, and the permit protected gateways take.
//!
//! trace:TASK-1607 trace:ADR-67 trace:ADR-70 | ai:claude

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use aida_core::process_probe::{ProbeSource, ProcessIdentity};

use super::config::SeatConfig;
use super::store::{SeatGuard, SeatStore};
use super::{Caller, ClaimNotice, Ctx, Gate, GrantRef, SeatOp, ORCHESTRATOR};

/// The validated grant (STORY-1473 resolver) as the state machine sees it.
pub(crate) fn grant_ref(project_root: &Path) -> Option<GrantRef> {
    crate::seat_authority::current_grant(project_root).map(|g| GrantRef {
        id: g.id.clone(),
        session_id: g.session_id.clone(),
        seat: aida_core::team::canonical_role(&g.seat),
        is_child: g.parent_grant_id.is_some(),
    })
}

/// The calling process: its ancestry, validated grant and lease scope.
pub(crate) fn current_caller(project_root: &Path) -> Caller {
    let chain = ProbeSource::system().ancestry(std::process::id());
    // trace:TASK-1607 | ai:codex
    // A child cannot borrow its parent's lease. Match only self..anchor,
    // requiring the persisted start identity, with active PID taking priority.
    let mut caller = Caller {
        chain,
        grant: grant_ref(project_root),
        lease_scope: None,
    };
    caller.lease_scope = super::ResolvedCaller::resolve(&caller).ok().and_then(|rc| {
        let leases = aida_core::liveness::read_session_leases(project_root);
        lease_scope_for_own(
            rc.own(),
            &leases,
            aida_core::liveness::process_start_identity,
        )
    });
    caller
}

fn lease_scope_for_own(
    own: &[aida_core::process_probe::ProcFacts],
    leases: &[aida_core::liveness::SessionLeaseLite],
    start: impl Fn(u32) -> Option<String>,
) -> Option<String> {
    // Nearest process wins. Conflicting scopes for one identity fail closed.
    for proc in own {
        let scopes: std::collections::BTreeSet<_> = leases
            .iter()
            .filter(|l| {
                let (pid, recorded) = match l.active_pid {
                    Some(pid) => (Some(pid), l.active_pid_start_time.as_deref()),
                    None => (l.creator_pid, l.creator_pid_start_time.as_deref()),
                };
                pid == Some(proc.identity.pid)
                    && recorded.is_some()
                    && start(proc.identity.pid).as_deref() == recorded
            })
            .map(|l| l.scope.as_str())
            .filter(|s| !s.is_empty())
            .collect();
        if scopes.len() > 1 {
            return None;
        }
        if let Some(scope) = scopes.first() {
            return Some((*scope).to_string());
        }
    }
    None
}

/// Run `f` with the system clock and probe.
pub(crate) fn with_system_ctx<T>(f: impl FnOnce(&Ctx) -> T) -> T {
    let source = ProbeSource::system();
    let probe = |id: &ProcessIdentity| source.probe(id);
    let ctx = Ctx {
        now: chrono::Utc::now(),
        probe: &probe,
        scope: None,
    };
    f(&ctx)
}

/// Permission to perform one protected op. For the holder it keeps the seat
/// locked until dropped, so the op commits under the lock (ADR-70): perform
/// the mutation, call [`SeatPermit::record_child`] for a spawn, then drop.
pub(crate) struct SeatPermit {
    guard: Option<SeatGuard>,
    pub claimed: Option<ClaimNotice>,
}

impl SeatPermit {
    fn pass() -> Self {
        SeatPermit {
            guard: None,
            claimed: None,
        }
    }

    /// Record a child spawned under the seat (holder permits only).
    pub fn record_child(&mut self, pid: u32, spec: Option<&str>, what: &str) -> Result<()> {
        let Some(guard) = self.guard.as_mut() else {
            return Ok(());
        };
        // A child we cannot identify is still running work; record what we
        // can see of it and let pruning treat an unreadable entry as Unknown.
        let identity = ProbeSource::system()
            .capture(pid)
            .unwrap_or(ProcessIdentity {
                pid,
                start: String::new(),
            });
        with_system_ctx(|ctx| {
            super::record_child(
                &mut guard.rec,
                ctx,
                identity,
                spec.map(str::to_string),
                what,
            )
        });
        guard.save()
    }
}

/// Gate `op` for the calling process on `seat` (normally the orchestrator).
/// Prints the C2 notice when the call claimed the seat.
pub(crate) fn authorize(project_root: &Path, seat: &str, op: SeatOp) -> Result<SeatPermit> {
    let caller = current_caller(project_root);
    authorize_as(project_root, seat, op, &caller, true)
}

pub(crate) fn authorize_as(
    project_root: &Path,
    seat: &str,
    op: SeatOp,
    caller: &Caller,
    announce: bool,
) -> Result<SeatPermit> {
    authorize_as_scoped(project_root, seat, op, caller, announce, None)
}

/// Core entry for gateways with an existing persisted local review snapshot.
/// Construct the graph before entry: no store refresh/network while locked.
// trace:TASK-1607 | ai:codex
pub(crate) fn authorize_as_scoped(
    project_root: &Path,
    seat: &str,
    op: SeatOp,
    caller: &Caller,
    announce: bool,
    scope: Option<&super::scope::ChildScopeGraph>,
) -> Result<SeatPermit> {
    let claims_possible = matches!(op, SeatOp::LoopStart { .. } | SeatOp::HandoffWrite)
        || caller.grant.as_ref().is_some_and(|g| g.seat == seat);
    let store = match SeatStore::for_project_opt(project_root)? {
        Some(store) => store,
        // No repository, so no record, holder or tombstone can exist here.
        None if !claims_possible => return Ok(SeatPermit::pass()),
        None => SeatStore::for_project(project_root)?,
    };
    let cfg = SeatConfig::load(project_root)
        .context("seat occupancy configuration is invalid; refusing protected operations")?;
    if !cfg.is_single(seat) {
        return Ok(SeatPermit::pass());
    }
    if !claims_possible {
        // Fast path for ordinary work: with no holder, former holder or child
        // recorded, the caller cannot be related to the seat, and a claim
        // racing this read cannot make it related.
        let rec = store.peek(seat)?;
        if rec.holder.is_none() && rec.tombstones.is_empty() && rec.children.is_empty() {
            return Ok(SeatPermit::pass());
        }
    }
    let mut guard = store.lock(seat)?;
    let gate = with_system_ctx(|ctx| {
        let ctx = Ctx {
            now: ctx.now,
            probe: ctx.probe,
            scope,
        };
        super::gate(&mut guard.rec, &ctx, caller, &op)
    });
    guard.save()?;
    match gate {
        Ok(Gate::Pass) => Ok(SeatPermit::pass()),
        Ok(Gate::Holder { claimed }) => {
            if announce {
                if let Some(n) = &claimed {
                    eprintln!("{n}");
                }
            }
            Ok(SeatPermit {
                guard: Some(guard),
                claimed,
            })
        }
        Err(refusal) => Err(anyhow::Error::new(refusal)),
    }
}

/// Convenience for the orchestrator seat.
pub(crate) fn authorize_orchestrator(project_root: &Path, op: SeatOp) -> Result<SeatPermit> {
    authorize(project_root, ORCHESTRATOR, op)
}

/// Where the seat records for `project_root` live.
pub(crate) fn store_dir(project_root: &Path) -> Result<PathBuf> {
    Ok(SeatStore::for_project(project_root)?.dir().to_path_buf())
}

#[cfg(test)]
mod tests {
    //! Store + lock + real-probe tests. Every process involved is a `sleep`
    //! child this test spawns; no existing peer session is ever a target.
    // trace:TASK-1607 | ai:claude
    use super::*;
    use crate::seat_occupancy::{ClaimKind, SeatOp};
    use aida_core::process_probe::ProcFacts;
    use std::process::{Child, Command};

    fn git_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            assert!(Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .output()
                .unwrap()
                .status
                .success());
        };
        run(&["init", "-q", "-b", "main"]);
        run(&[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "x",
        ]);
        dir
    }

    struct Anchor(Child);

    impl Anchor {
        fn spawn() -> Self {
            Anchor(Command::new("sleep").arg("300").spawn().unwrap())
        }
        fn facts(&self) -> ProcFacts {
            ProbeSource::system().facts(self.0.id()).unwrap()
        }
        fn kill(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    impl Drop for Anchor {
        fn drop(&mut self) {
            self.kill();
        }
    }

    /// A caller anchored at `anchor` (no harness in the chain, so the parent
    /// of chain[0] is the anchor) carrying an orchestrator grant. `chain[0]`
    /// is the loop process a LoopStart binds.
    fn caller_at(anchor: &Anchor, session: &str) -> Caller {
        caller_in(anchor, anchor, session)
    }

    fn caller_in(looper: &Anchor, anchor: &Anchor, session: &str) -> Caller {
        Caller {
            chain: Ok(vec![looper.facts(), anchor.facts()]),
            grant: Some(GrantRef {
                id: format!("g-{session}"),
                session_id: session.to_string(),
                seat: ORCHESTRATOR.to_string(),
                is_child: false,
            }),
            lease_scope: None,
        }
    }

    fn loop_op() -> SeatOp {
        SeatOp::LoopStart {
            what: "start a burndown run".into(),
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn concurrent_sibling_claims_produce_exactly_one_holder() {
        let repo = git_repo();
        let anchors: Vec<Anchor> = (0..8).map(|_| Anchor::spawn()).collect();
        let callers: Vec<Caller> = anchors
            .iter()
            .enumerate()
            .map(|(i, a)| caller_at(a, &format!("sess-{i}")))
            .collect();
        let root = repo.path().to_path_buf();
        let wins: Vec<bool> = std::thread::scope(|s| {
            let handles: Vec<_> = callers
                .iter()
                .map(|c| {
                    let root = root.clone();
                    s.spawn(move || {
                        authorize_as(&root, ORCHESTRATOR, loop_op(), c, false)
                            .map(|p| p.claimed.is_some())
                            .unwrap_or(false)
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        assert_eq!(wins.iter().filter(|w| **w).count(), 1, "{wins:?}");
        let rec = SeatStore::for_project(&root)
            .unwrap()
            .peek(ORCHESTRATOR)
            .unwrap();
        assert_eq!(rec.generation, 1);
        let winner = wins.iter().position(|w| *w).unwrap();
        assert_eq!(rec.holder.unwrap().grant_session, format!("sess-{winner}"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_dead_holder_process_is_replaced_and_a_live_one_is_not() {
        let repo = git_repo();
        let mut a = Anchor::spawn();
        let mut a_loop = Anchor::spawn();
        let b = Anchor::spawn();
        let ca = caller_in(&a_loop, &a, "sess-a");
        let cb = caller_at(&b, "sess-b");
        authorize_as(repo.path(), ORCHESTRATOR, loop_op(), &ca, false).unwrap();
        let refused = authorize_as(repo.path(), ORCHESTRATOR, loop_op(), &cb, false)
            .err()
            .expect("live holder refuses")
            .to_string();
        assert!(
            refused.contains("held by another live session"),
            "{refused}"
        );
        a.kill();
        assert!(
            authorize_as(repo.path(), ORCHESTRATOR, loop_op(), &cb, false).is_err(),
            "the holder's detached loop is still running"
        );
        a_loop.kill();
        let permit = authorize_as(repo.path(), ORCHESTRATOR, loop_op(), &cb, false).unwrap();
        assert_eq!(permit.claimed.unwrap().how, ClaimKind::DeadReplacement);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_holders_permit_keeps_the_seat_locked_until_dropped() {
        let repo = git_repo();
        let a = Anchor::spawn();
        let permit = authorize_as(
            repo.path(),
            ORCHESTRATOR,
            loop_op(),
            &caller_at(&a, "sess-a"),
            false,
        )
        .unwrap();
        let lock = SeatStore::for_project(repo.path())
            .unwrap()
            .dir()
            .join("orchestrator.lock");
        let other = std::fs::OpenOptions::new().write(true).open(&lock).unwrap();
        use fs2::FileExt;
        assert!(
            other.try_lock_exclusive().is_err(),
            "ADR-70: commit under the lock"
        );
        drop(permit);
        // Parallel tests spawn processes while this guard is live. Between
        // fork and exec a child inherits the flock's open file description;
        // dropping our File is insufficient until that child execs/closes it.
        // Retry only contention, with a hard deadline so a leaked guard fails.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match other.try_lock_exclusive() {
                Ok(()) => break,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(10))
                }
                Err(e) => panic!("seat lock remained held after permit drop: {e}"),
            }
        }
    }

    // trace:TASK-1607 | ai:codex
    fn review_graph(repo: &Path) -> super::super::scope::ChildScopeGraph {
        use aida_core::{Relationship, RelationshipType, Requirement, RequirementsStore, Storage};
        let mut task = Requirement::new("assigned task".into(), String::new());
        task.spec_id = Some("TASK-7".into());
        let mut sibling = Requirement::new("same PR spec".into(), String::new());
        sibling.spec_id = Some("TASK-8".into());
        let mut unrelated = Requirement::new("unrelated task".into(), String::new());
        unrelated.spec_id = Some("TASK-99".into());
        let mut review = Requirement::new("Review PR-42: assigned task".into(), String::new());
        review.spec_id = Some("STORY-42".into());
        review.relationships = [&task, &sibling]
            .iter()
            .map(|r| Relationship {
                rel_type: RelationshipType::Custom("implements".into()),
                target_id: r.id,
                created_at: None,
                created_by: None,
            })
            .collect();
        let mut store = RequirementsStore::new();
        store.requirements = vec![task, sibling, unrelated, review];
        // Exercise a real local persisted snapshot, never the live store.
        let storage = Storage::new(repo.join("requirements.yaml"));
        storage.save(&store).unwrap();
        super::super::scope::ChildScopeGraph::from_store(&storage.load().unwrap())
    }

    fn drain_child_fixture(repo: &Path, assigned: &str) -> (Anchor, Anchor, Caller) {
        let parent = Anchor::spawn();
        let child = Anchor::spawn();
        let holder = caller_at(&parent, "parent-session");
        let mut permit = authorize_as(repo, ORCHESTRATOR, loop_op(), &holder, false).unwrap();
        permit
            .record_child(child.0.id(), Some(assigned), "drain phase")
            .unwrap();
        drop(permit);
        let mut facts = child.facts();
        facts.name = "codex".into();
        facts.cmd = vec!["codex".into()];
        let caller = Caller {
            chain: Ok(vec![facts.clone(), facts, parent.facts()]),
            grant: None,
            lease_scope: Some("TASK-99".into()), // recorded spawn wins over a mismatched lease
        };
        (parent, child, caller)
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn drain_child_implementer_session_end_routes_its_review_story() {
        let repo = git_repo();
        let graph = review_graph(repo.path());
        let (_parent, _child, caller) = drain_child_fixture(repo.path(), "TASK-7");
        for (target, what) in [
            ("TASK-7", "queue done TASK-7"),
            ("TASK-7", "edit TASK-7 --status done"),
            ("STORY-42", "session end: auto-queue review for PR-42"),
        ] {
            let permit = authorize_as_scoped(
                repo.path(),
                ORCHESTRATOR,
                SeatOp::Route {
                    spec: Some(target.into()),
                    what: what.into(),
                },
                &caller,
                false,
                Some(&graph),
            )
            .unwrap();
            assert!(
                permit.guard.is_none() && permit.claimed.is_none(),
                "child does not borrow holder lock"
            );
        }
        let rec = SeatStore::for_project(repo.path())
            .unwrap()
            .peek(ORCHESTRATOR)
            .unwrap();
        assert_eq!(rec.generation, 1);
        assert_eq!(rec.holder.unwrap().grant_session, "parent-session");
        assert!(authorize_as_scoped(
            repo.path(),
            ORCHESTRATOR,
            SeatOp::Route {
                spec: Some("TASK-99".into()),
                what: "route unrelated task".into()
            },
            &caller,
            false,
            Some(&graph)
        )
        .is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn drain_child_reviewer_routes_pr_spec_rework_and_punt_without_peer_authority() {
        for assigned in ["PR-42", "STORY-42"] {
            let repo = git_repo();
            let graph = review_graph(repo.path());
            let (_parent, _child, caller) = drain_child_fixture(repo.path(), assigned);
            for (target, what) in [
                ("TASK-7", "review record request-changes for PR-42"),
                ("TASK-7", "queue rework TASK-7 --for implementer"),
                ("TASK-8", "queue rework TASK-8 --for implementer"),
                ("TASK-7", "punt TASK-7: route to advisor"),
                ("TASK-7", "advise TASK-7: route decision/rework"),
                ("STORY-42", "queue done STORY-42"),
            ] {
                assert!(
                    authorize_as_scoped(
                        repo.path(),
                        ORCHESTRATOR,
                        SeatOp::Route {
                            spec: Some(target.into()),
                            what: what.into()
                        },
                        &caller,
                        false,
                        Some(&graph)
                    )
                    .is_ok(),
                    "{assigned}: {what}"
                );
            }
            for op in [
                SeatOp::Route {
                    spec: Some("TASK-99".into()),
                    what: "queue rework TASK-7 --for implementer".into(),
                },
                SeatOp::Route {
                    spec: None,
                    what: "queue clear".into(),
                },
                SeatOp::Spawn {
                    spec: Some("TASK-7".into()),
                    what: "spawn reviewer peer".into(),
                },
                loop_op(),
                SeatOp::HandoffWrite,
            ] {
                assert!(
                    authorize_as_scoped(
                        repo.path(),
                        ORCHESTRATOR,
                        op.clone(),
                        &caller,
                        false,
                        Some(&graph)
                    )
                    .is_err(),
                    "{assigned}: {op:?}"
                );
            }
        }
    }

    #[test]
    fn lease_scope_fallback_requires_own_process_start_and_unambiguous_scope() {
        use aida_core::liveness::SessionLeaseLite;
        let fact = ProcFacts {
            identity: ProcessIdentity {
                pid: 7,
                start: "probe-id".into(),
            },
            ppid: 1,
            name: "codex".into(),
            cmd: vec!["codex".into()],
        };
        let lease = |pid, start: Option<&str>, scope: &str| -> SessionLeaseLite {
            toml::from_str(&format!(
                "scope = {scope:?}\nstarted_at = \"2026-10-07T00:00:00Z\"\ncreator_pid = {pid}\n{}",
                start
                    .map(|s| format!("creator_pid_start_time = {s:?}\n"))
                    .unwrap_or_default()
            ))
            .unwrap()
        };
        let start = |_: u32| Some("current-start".to_string());
        let own = [fact];
        for leases in [
            vec![lease(1, Some("current-start"), "PARENT")],
            vec![lease(7, None, "TASK-7")],
            vec![lease(7, Some("old-start"), "TASK-7")],
            vec![
                lease(7, Some("current-start"), "TASK-7"),
                lease(7, Some("current-start"), "TASK-8"),
            ],
        ] {
            assert_eq!(lease_scope_for_own(&own, &leases, start), None);
        }
        assert_eq!(
            lease_scope_for_own(&own, &[lease(7, Some("current-start"), "TASK-7")], start),
            Some("TASK-7".into())
        );
    }

    // trace:TASK-1607 | ai:codex
    #[cfg(target_os = "linux")]
    #[test]
    fn unattended_schedule_tick_and_supervisor_redrive_loopstarts_fail_closed() {
        let repo = git_repo();
        let timer = Anchor::spawn();
        let mut timer_caller = caller_at(&timer, "unused-env-session");
        timer_caller.grant = None; // systemd/cron has no validated dispatch grant
        for what in [
            "schedule tick -> shift tick",
            "supervisor launch_redrive: queue work --auto-complete",
        ] {
            let err = authorize_as(
                repo.path(),
                ORCHESTRATOR,
                SeatOp::LoopStart { what: what.into() },
                &timer_caller,
                false,
            )
            .err()
            .expect("unattended launcher must not invent authority")
            .to_string();
            assert!(err.contains("validated role grant"), "{what}: {err}");
        }
        let parent = Anchor::spawn();
        authorize_as(
            repo.path(),
            ORCHESTRATOR,
            loop_op(),
            &caller_at(&parent, "holder"),
            false,
        )
        .unwrap();
        // A redrive process in the holder's lineage does not gain a grant
        // from ancestry (or from a role/session environment label).
        timer_caller.chain.as_mut().unwrap().push(parent.facts());
        let err = authorize_as(
            repo.path(),
            ORCHESTRATOR,
            SeatOp::LoopStart {
                what: "supervisor launch_redrive".into(),
            },
            &timer_caller,
            false,
        )
        .err()
        .expect("redrive cannot originate another loop")
        .to_string();
        assert!(err.contains("delegated worker"), "{err}");
        let rec = SeatStore::for_project(repo.path())
            .unwrap()
            .peek(ORCHESTRATOR)
            .unwrap();
        assert_eq!(rec.generation, 1);
        assert_eq!(rec.holder.unwrap().grant_session, "holder");
    }

    #[test]
    fn ordinary_routing_outside_a_repository_passes_without_state() {
        let dir = tempfile::tempdir().unwrap();
        let caller = Caller {
            chain: Err("not needed".into()),
            grant: None,
            lease_scope: None,
        };
        let op = SeatOp::Route {
            spec: Some("TASK-1".into()),
            what: "enqueue TASK-1".into(),
        };
        let out = authorize_as(dir.path(), ORCHESTRATOR, op, &caller, false);
        assert!(out.is_ok(), "{:?} in {}", out.err(), dir.path().display());
        assert!(!dir.path().join(".aida").exists());
    }

    #[test]
    fn kill_is_refused_everywhere_before_any_state_is_read() {
        use crate::cli::SeatCommand;
        for cmd in [
            SeatCommand::Takeover {
                seat: "orchestrator".into(),
                force: true,
                kill: true,
                grace_secs: None,
                request: None,
                no_wait: true,
            },
            SeatCommand::Claim {
                seat: "orchestrator".into(),
                force: true,
                kill: true,
                grace_secs: None,
                no_wait: true,
            },
        ] {
            let err = crate::seat_occupancy::cmd::dispatch(&cmd)
                .unwrap_err()
                .to_string();
            assert!(err.contains("--kill"), "{err}");
            assert!(err.contains("not available"), "{err}");
        }
    }
}
