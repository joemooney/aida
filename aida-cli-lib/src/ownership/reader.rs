//! Production ownership observations. Missing or unreadable evidence retains
//! ownership, except the explicitly scoped legacy pickup compatibility reader.
// trace:STORY-2325 | ai:codex
use super::{dual, HostFacts, OwnerVerdict, Ownership, Phase, WorkKey};
use anyhow::Result;
use chrono::Utc;
use std::path::Path;

pub(crate) fn load(root: &Path, key: &WorkKey) -> Result<Option<Ownership>> {
    match super::store::OwnershipStore::for_project(root)? {
        Some(store) => store.load(key),
        None => Ok(None),
    }
}

pub(crate) fn owner(record: &Ownership) -> OwnerVerdict {
    let writer = dual::WriterFacts::here();
    let probe = |id: &aida_core::process_probe::ProcessIdentity| {
        use aida_core::{
            proc_identity::{ProcIdentity, Verdict},
            process_probe::Probe,
        };
        let Some(id) = ProcIdentity::from_record(id, writer.pid_ns) else {
            return Probe::Unknown("insufficient identity".into());
        };
        match aida_core::proc_identity::observe(&id, true) {
            Verdict::Alive => Probe::Alive,
            Verdict::Dead(why) => Probe::Dead(format!("{why:?}")),
            Verdict::ForeignUnknown(why) => Probe::Unknown(format!("{why:?}")),
        }
    };
    let host = HostFacts {
        node_id: aida_core::proc_identity::local_node_id(),
        host: writer.host.clone(),
        boot: writer.boot.clone(),
        pid_ns: writer.pid_ns,
        probe: &probe,
    };
    record.holder.as_ref().map_or_else(
        || OwnerVerdict::Unknown {
            why: "no ownership holder".into(),
        },
        |holder| super::judge_owner(holder, &host, Utc::now()),
    )
}

/// Explicit release or positive death proof. Unknown and absent records retain
/// the marker/train; elapsed time alone never permits destructive cleanup.
pub(crate) fn ended(root: &Path, key: &WorkKey) -> Result<Option<String>> {
    let Some(record) = load(root, key)? else {
        return Ok(None);
    };
    Ok(ended_record(&record))
}

// trace:STORY-2325 | ai:codex
pub(crate) fn ended_record(record: &Ownership) -> Option<String> {
    if let Phase::Closed { why } = &record.phase {
        return Some(why.clone());
    }
    match owner(record) {
        OwnerVerdict::Dead { proof } => Some(proof),
        _ => None,
    }
}

pub(crate) fn lease_record(root: &Path, lease: &crate::SessionLease) -> Result<Option<Ownership>> {
    LeaseReader::new(root).record(lease)
}

// trace:TASK-3420 | ai:codex
#[derive(Clone, Copy)]
pub(crate) enum LegacyRule {
    Pickup,
    Adoption,
    NonStale,
    Review,
}

pub(crate) struct LeaseReader<'a> {
    root: &'a Path,
    reqs: std::cell::OnceCell<Option<aida_core::RequirementsStore>>,
    live: std::cell::OnceCell<Vec<crate::process_probe::LiveSession>>,
    registry: std::cell::OnceCell<Vec<crate::agent_registry::AgentRegistryEntry>>,
}

impl<'a> LeaseReader<'a> {
    pub(crate) fn new(root: &'a Path) -> Self {
        Self {
            root,
            reqs: std::cell::OnceCell::new(),
            live: std::cell::OnceCell::new(),
            registry: std::cell::OnceCell::new(),
        }
    }

    fn key(&self, lease: &crate::SessionLease) -> Result<Option<WorkKey>> {
        let resolve = |label: &str| {
            uuid::Uuid::parse_str(label.trim())
                .ok()
                .map(crate::gate::canon::SpecKey)
                .or_else(|| {
                    self.reqs
                        .get_or_init(|| crate::load_store_for_lookup(self.root))
                        .as_ref()
                        .and_then(|s| dual::spec_key_in_store(s, label))
                })
        };
        let Some((key, _)) = dual::key_for_lease(lease, &resolve) else {
            anyhow::ensure!(
                !matches!(self.reqs.get(), Some(None)),
                "lease scope could not be resolved through the spec store"
            );
            return Ok(None);
        };
        Ok(Some(key))
    }

    fn record(&self, lease: &crate::SessionLease) -> Result<Option<Ownership>> {
        match self.key(lease)? {
            Some(key) => load(self.root, &key),
            None => Ok(None),
        }
    }

    pub(crate) fn retained(&self, lease: &crate::SessionLease) -> bool {
        !matches!(
            self.pickup_owner(lease, LegacyRule::Pickup),
            OwnerVerdict::Dead { .. }
        )
    }

    /// Temporary legacy compatibility for the four Part B pickup probes and
    /// adoption, including explicitly refused legacy captures. Remove once every
    /// production holder writes an admitted witness.
    // trace:TASK-3420 | ai:codex
    pub(crate) fn pickup_owner(
        &self,
        lease: &crate::SessionLease,
        rule: LegacyRule,
    ) -> OwnerVerdict {
        let record = self.record(lease);
        // A bound supervisor receipt positively ends this review generation.
        // It grants no authority over a replacement holder or mismatched receipt.
        // trace:TASK-3420 | ai:codex
        if let Ok(Some(record)) = &record {
            if matches!(record.key, WorkKey::Review { .. })
                && record.holder.is_none()
                && matches!(&record.phase, Phase::Closed { why } if review_completion_matches(lease, why))
            {
                return OwnerVerdict::Dead {
                    proof: "lease-bound review completion".into(),
                };
            }
        }
        let verdict = lease_owner_record(
            lease,
            record
                .as_ref()
                .map(Clone::clone)
                .map_err(|e| anyhow::anyhow!("{e:#}")),
        );
        let unadmitted = |holder: &super::Holder| {
            holder.witnesses.iter().all(|w| {
                matches!(w.kind, super::WitnessKind::Process { .. })
                    && w.admission.as_ref().is_none_or(|a| a.refusal().is_some())
            })
        };
        let legacy_unknown = match &record {
            Ok(None) => {
                matches!(self.key(lease), Ok(None))
                    || unadmitted(&dual::holder_for_lease(
                        lease,
                        &dual::WriterFacts::here(),
                        Utc::now(),
                    ))
            }
            Ok(Some(record)) => match record.holder.as_ref() {
                Some(holder) => holder.session == lease.id && unadmitted(holder),
                None if matches!(&record.phase, Phase::InPr { handed_off_by: Some(id), .. } if id == &lease.id) =>
                {
                    // Handoff clears the holder; judge the same reconstructed
                    // production envelope as lease_owner_record below.
                    unadmitted(&dual::holder_for_lease(
                        lease,
                        &dual::WriterFacts::here(),
                        Utc::now(),
                    ))
                }
                None => false,
            },
            Err(_) => false,
        };
        if matches!(verdict, OwnerVerdict::Unknown { .. }) && legacy_unknown {
            let live = self
                .live
                .get_or_init(crate::process_probe::probe_live_claude_sessions);
            let now = Utc::now();
            let retained = match rule {
                LegacyRule::Pickup => crate::leases::lease_is_live(live, now)(lease),
                LegacyRule::Adoption => {
                    let report = crate::leases::stale_lease_recovery_for_lease_with(lease, live);
                    match report.base_verdict {
                        crate::StaleLeaseRecovery::Live => {
                            return OwnerVerdict::Alive {
                                witness: "a live process still backs it".into(),
                            }
                        }
                        crate::StaleLeaseRecovery::UnknownLiveness => {
                            return OwnerVerdict::Unknown {
                                why: crate::leases::orphan_adoption_refusal(&report)
                                    .unwrap()
                                    .into(),
                            }
                        }
                        _ => false,
                    }
                }
                LegacyRule::NonStale | LegacyRule::Review => {
                    !matches!(
                        crate::lease_state_for(lease, live, now),
                        crate::LeaseState::Stale
                    ) || (matches!(rule, LegacyRule::Review)
                        && !lease.worktree_path.as_os_str().is_empty()
                        && self
                            .registry
                            .get_or_init(|| {
                                crate::agent_registry::load_live_identity_entries(self.root)
                            })
                            .iter()
                            .any(|a| a.worktree_path.starts_with(&lease.worktree_path)))
                }
            };
            return if retained {
                verdict
            } else {
                OwnerVerdict::Dead {
                    proof: "legacy lease rule".into(),
                }
            };
        }
        verdict
    }
}

/// Re-entry must prove that this exact lease's holder ended. A different
/// generation, missing record or failed read retains the lane; a process
/// found in the same cwd is not evidence about the recorded owner.
// trace:STORY-2325 | ai:codex
pub(crate) fn lease_owner(root: &Path, lease: &crate::SessionLease) -> OwnerVerdict {
    lease_owner_record(lease, lease_record(root, lease))
}

fn lease_owner_record(
    lease: &crate::SessionLease,
    record: Result<Option<Ownership>>,
) -> OwnerVerdict {
    match record {
        Ok(Some(record)) => match record.holder.as_ref() {
            Some(holder) if holder.session == lease.id => owner(&record),
            None if matches!(&record.phase, Phase::InPr { handed_off_by: Some(session), .. } if session == &lease.id) =>
            {
                let mut cleanup = record;
                cleanup.holder = Some(dual::holder_for_lease(
                    lease,
                    &dual::WriterFacts::here(),
                    Utc::now(),
                ));
                owner(&cleanup)
            }
            _ => OwnerVerdict::Unknown {
                why: "ownership record no longer names this session".into(),
            },
        },
        Ok(None) => OwnerVerdict::Unknown {
            why: "ownership record missing; on the host run `aida own migrate --dry-run`, then `aida own migrate --execute`; inspect `aida session leases --all` for unresolved holder bindings".into(),
        },
        Err(error) => OwnerVerdict::Unknown {
            why: format!("ownership evidence unreadable: {error:#}"),
        },
    }
}

/// Review cleanup also accepts a supervisor's lease-bound completion receipt. Closed
/// records have no holder, so `lease_owner` intentionally cannot prove death.
/// The caller must still guard the disposable tree and all its live holders.
// trace:BUG-2498 | ai:codex
pub(crate) fn review_lease_ended(root: &Path, lease: &crate::SessionLease) -> bool {
    if crate::pr_lease_takeover::reviewer_target(lease).is_none() {
        return false;
    }
    if let Ok(Some(record)) = lease_record(root, lease) {
        if matches!(record.key, WorkKey::Review { .. })
            && matches!(&record.phase, Phase::Closed { why } if review_completion_matches(lease, why))
            && record.holder.is_none()
        {
            return true;
        }
    }
    matches!(lease_owner(root, lease), OwnerVerdict::Dead { .. })
}

// Bind completion to the lease identity and worker generation. A
// best-effort acquire failure on a later same-head review cannot reuse it.
// trace:BUG-2498 | ai:codex
fn review_finished_reason(lease: &crate::SessionLease) -> Result<String> {
    use sha2::{Digest, Sha256};
    // Mail-retention bookkeeping does not change the worker generation.
    // All identity, authority and process fields remain in the binding.
    // trace:BUG-2498 | ai:codex
    let mut lease = lease.clone();
    lease.ending_retained_at = None;
    lease.ending_retained_reason = None;
    Ok(format!(
        "review supervisor finished lease {}:{:x}",
        lease.id,
        Sha256::digest(toml::to_string(&lease)?.as_bytes())
    ))
}

// A completion receipt never overrides a still-live recorded worker, including
// one whose cwd has left the review tree. Missing worker evidence is covered by
// the supervisor receipt; foreign/unreadable recorded workers remain protected.
// trace:BUG-2498 | ai:codex
fn review_completion_matches(lease: &crate::SessionLease, why: &str) -> bool {
    if !review_finished_reason(lease).is_ok_and(|expected| why == expected) {
        return false;
    }
    if crate::leases::lease_holder_record(lease).is_some() {
        return matches!(
            crate::leases::lease_process_verdict(lease),
            Ok(aida_core::proc_identity::Verdict::Dead(_))
        );
    }
    lease.active_pid.is_none_or(|pid| {
        pid != 0
            && aida_core::pid_observer::PidObserver::current().can_observe(lease.pid_namespace)
            // Completion supplies the lifecycle authority; the shared observer
            // only vetoes cleanup unless the recorded worker is provably gone.
            // trace:BUG-2498 | ai:codex
            && matches!(
                aida_core::proc_identity::observe_legacy_registry(
                    pid,
                    lease.active_pid_start_time.as_deref(),
                    lease.pid_namespace,
                ),
                aida_core::proc_identity::Verdict::Dead(_)
            )
    })
}

/// Called after the supervisor observed reviewer exit, or the watcher proved
/// the captured attempt gone. Keep positive completion evidence even when guarded
/// teardown retains the tree. Missing/replaced ownership is never repaired here.
// trace:BUG-2498 | ai:codex
pub(crate) fn review_finished(root: &Path, lease: &crate::SessionLease) -> Result<()> {
    use super::store::{Expect, OwnershipStore};
    // Seat clones cannot publish host ownership evidence. trace:TASK-3777 | ai:codex
    anyhow::ensure!(
        !crate::session::process_contained(),
        "a contained seat never writes the ownership records"
    );
    anyhow::ensure!(
        crate::pr_lease_takeover::reviewer_target(lease).is_some(),
        "not a review lease"
    );
    crate::session::with_lease_write_lock(root, || {
        let current: crate::SessionLease = toml::from_str(&std::fs::read_to_string(
            crate::lease_path(root, &lease.id),
        )?)?;
        anyhow::ensure!(
            review_finished_reason(&current)? == review_finished_reason(lease)?,
            "review lease changed before completion receipt"
        );
        let record = lease_record(root, lease)?
            .ok_or_else(|| anyhow::anyhow!("review ownership missing"))?;
        anyhow::ensure!(
            matches!(record.key, WorkKey::Review { .. })
                && matches!(record.phase, Phase::Reviewing { .. })
                && record
                    .holder
                    .as_ref()
                    .is_some_and(|h| h.session == lease.id),
            "review ownership changed before completion receipt"
        );
        let why = review_finished_reason(lease)?;
        OwnershipStore::for_project(root)?
            .ok_or_else(|| anyhow::anyhow!("ownership store unavailable"))?
            .update(
                &record.key,
                "review_finished",
                Expect::Generation(record.generation),
                |cur| super::ops::release(cur, &lease.id, &why),
            )?;
        Ok(())
    })
}

/// Journal cleanup of a disposable review tree, accepting bound completion
/// as well as the ordinary positive death proof.
// trace:BUG-2498 | ai:codex
pub(crate) fn journal_review_reclaim(root: &Path, lease: &crate::SessionLease) -> Result<()> {
    let record = lease_record(root, lease)?
        .ok_or_else(|| anyhow::anyhow!("ownership record missing; reclaim refused"))?;
    // Lease-bound supervisor completion already records its release. It
    // needs no death journal entry; tree safety remains the caller's job.
    // trace:BUG-2498 | ai:codex
    if matches!(record.key, WorkKey::Review { .. })
        && matches!(&record.phase, Phase::Closed { why } if review_completion_matches(lease, why))
        && record.holder.is_none()
        && crate::pr_lease_takeover::reviewer_target(lease).is_some()
    {
        return Ok(());
    }
    journal_reclaim(root, lease)
}

/// Record a reclaim attempt with positive death proof before guarded retirement. CAS refuses a
/// replacement during preflight; the holder remains until session_end succeeds.
/// This journal entry grants no permission to discard a tree or pending mail.
// trace:STORY-2325 | ai:codex
pub(crate) fn journal_reclaim(root: &Path, lease: &crate::SessionLease) -> Result<()> {
    use super::store::{Expect, OwnershipStore};
    let record = lease_record(root, lease)?
        .ok_or_else(|| anyhow::anyhow!("ownership record missing; reclaim refused"))?;
    let proof = match lease_owner(root, lease) {
        OwnerVerdict::Dead { proof } => proof,
        verdict => anyhow::bail!("ownership reclaim requires positive death proof: {verdict:?}"),
    };
    let store = OwnershipStore::for_project(root)?
        .ok_or_else(|| anyhow::anyhow!("ownership store unavailable"))?;
    store.update(
        &record.key,
        "reclaim_attempt",
        Expect::Generation(record.generation),
        |current| {
            current.map(|current| {
                (
                    current.clone(),
                    Some(format!(
                        "session {} reclaim attempt (guarded session end still pending): {proof}",
                        lease.id
                    )),
                )
            })
        },
    )?;
    Ok(())
}

/// A rotation may continue only while its recorded holder is alive. Re-read
/// after the countdown, so a handoff or replacement in the gap stops relaunch.
// trace:STORY-2325 | ai:codex
pub(crate) fn require_rotation_owner(root: &Path, session: &str) -> Result<()> {
    let body = std::fs::read_to_string(crate::lease_path(root, session))?;
    let lease: crate::SessionLease = toml::from_str(&body)?;
    anyhow::ensure!(lease.id == session, "rotation lease identity changed");
    let record = lease_record(root, &lease)?
        .ok_or_else(|| anyhow::anyhow!("rotation ownership record missing"))?;
    anyhow::ensure!(
        record
            .holder
            .as_ref()
            .is_some_and(|holder| holder.session == session),
        "rotation ownership transferred or ended"
    );
    match owner(&record) {
        OwnerVerdict::Alive { .. } => Ok(()),
        verdict => anyhow::bail!("rotation ownership is not alive: {verdict:?}"),
    }
}

pub(crate) fn owner_label(verdict: &OwnerVerdict) -> String {
    match verdict {
        OwnerVerdict::Alive { witness } => format!("live ({witness})"),
        OwnerVerdict::Dead { proof } => format!("dead ({proof})"),
        OwnerVerdict::Unknown { why } => format!("unknown ({why})"),
    }
}

#[cfg(test)]
pub(crate) fn seed_lease_heartbeat(
    root: &Path,
    lease: &crate::SessionLease,
    key: WorkKey,
    alive: bool,
) {
    let at = Utc::now() - chrono::Duration::days(if alive { 0 } else { 2 });
    let mut holder = dual::holder_for_lease(lease, &dual::WriterFacts::here(), at);
    holder.witnesses = vec![super::Witness {
        admission: None,
        kind: super::WitnessKind::Heartbeat { at, every_secs: 10 },
        recorded_at: at,
    }];
    super::store::OwnershipStore::for_project(root)
        .unwrap()
        .unwrap()
        .update(&key, "test", super::store::Expect::Any, |_| {
            Some((
                Ownership {
                    key: key.clone(),
                    generation: 0,
                    holder: Some(holder),
                    phase: Phase::Leased,
                },
                None,
            ))
        })
        .unwrap();
}

/// Pickup and review observations use the scoped legacy compatibility reader;
/// admitted owners and non-legacy unknown evidence keep the ownership verdict.
// trace:TASK-3420 | ai:codex
pub(crate) fn lease_retained(root: &Path, lease: &crate::SessionLease) -> bool {
    LeaseReader::new(root).retained(lease)
}

#[cfg(test)]
mod tests {
    use super::*;
    /// A successfully loaded store may legitimately contain no spec for a free-form
    /// scope. Only a failed store read makes resolution evidence unreadable.
    // trace:TASK-3420 | ai:codex
    #[cfg(test)]
    #[test]
    fn task_3420_free_form_scope_is_not_a_store_failure() {
        let root = tempfile::tempdir().unwrap();
        assert!(std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root.path())
            .status()
            .unwrap()
            .success());
        let _ambient =
            crate::test_env::AmbientGuard::hermetic_with_seat(root.path(), "implementer", &[]);
        let lease: crate::SessionLease = toml::from_str(
            r#"id = "free-form"
scope = "harness-worktree"
slug = "harness"
owner = "test@example.com"
worktree_path = ""
branch = ""
started_at = "2026-11-09T00:00:00Z"
hostname = "test"
"#,
        )
        .unwrap();
        let reader = LeaseReader::new(root.path());
        reader
            .reqs
            .set(Some(aida_core::RequirementsStore::default()))
            .ok()
            .unwrap();
        assert!(reader.record(&lease).unwrap().is_none());
        let mut command = std::process::Command::new("sleep");
        command.arg("30");
        let capture = aida_core::proc_identity::prepare_command(&mut command).unwrap();
        let mut child = command.spawn().unwrap();
        let admission = capture.finish(&mut child);
        admission.validate().unwrap();
        let mut lease = lease;
        lease.active_pid = Some(child.id());
        lease.active_pid_start_time = crate::process_probe::process_start_identity(child.id());
        lease.hostname = crate::coordination::hostname();
        lease.pid_namespace = dual::WriterFacts::here().pid_ns;
        lease.holder_record = Some(serde_json::to_string(&admission).unwrap());
        child.kill().unwrap();
        child.wait().unwrap();
        for scope in ["harness-worktree", "PR-3420"] {
            lease.scope = scope.into();
            lease.review_verb = scope.starts_with("PR-");
            for rule in [
                LegacyRule::Pickup,
                LegacyRule::NonStale,
                LegacyRule::Review,
                LegacyRule::Adoption,
            ] {
                assert!(
                    matches!(reader.pickup_owner(&lease, rule), OwnerVerdict::Dead { .. }),
                    "unkeyed dead admitted holder: {scope}"
                );
            }
        }
        // A resolvable key with missing ownership evidence is a different Unknown:
        // the unkeyed compatibility path must not release it.
        lease.scope = "PR-3420".into();
        lease.review_verb = false;
        assert!(reader.record(&lease).unwrap().is_none());
        assert!(reader.retained(&lease));
        lease.scope = "harness-worktree".into();
        let failed = LeaseReader::new(root.path());
        failed.reqs.set(None).ok().unwrap();
        assert!(failed.record(&lease).is_err());
    }

    /// The pre-migration reviewer rule attributes a live Claude session to its
    /// worktree even after the creator crashes; a session elsewhere cannot retain it.
    // trace:TASK-3420 trace:BUG-2206 | ai:codex
    #[cfg(test)]
    #[test]
    fn task_3420_legacy_reviewer_live_claude_worktree() {
        let root = tempfile::tempdir().unwrap();
        let _ambient =
            crate::test_env::AmbientGuard::hermetic_with_seat(root.path(), "implementer", &[]);
        let tree = root.path().join("review");
        std::fs::create_dir(&tree).unwrap();
        let mut lease: crate::SessionLease = toml::from_str(
            r#"id = "review-legacy"
scope = "PR-3420"
slug = "review"
owner = "test@example.com"
worktree_path = ""
branch = ""
started_at = "2026-11-09T00:00:00Z"
hostname = "test"
"#,
        )
        .unwrap();
        lease.worktree_path = tree.clone();
        lease.hostname = crate::coordination::hostname();
        lease.review_verb = true;
        lease.creator_pid = Some(u32::MAX);
        for (cwd, retained) in [(tree, true), (root.path().join("elsewhere"), false)] {
            let reader = LeaseReader::new(root.path());
            reader
                .reqs
                .set(Some(aida_core::RequirementsStore::default()))
                .ok()
                .unwrap();
            reader
                .live
                .set(vec![crate::process_probe::LiveSession {
                    pid: u32::MAX - 1,
                    cwd,
                    jsonl: None,
                    stale_cwd: false,
                }])
                .ok()
                .unwrap();
            assert_eq!(
                !matches!(
                    reader.pickup_owner(&lease, LegacyRule::Review),
                    OwnerVerdict::Dead { .. }
                ),
                retained
            );
        }
    }

    // trace:TASK-3420 | ai:codex
    #[cfg(test)]
    #[test]
    fn task_3420_closed_review_receipt_and_invalid_admission() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(root, "implementer", &[]);
        assert!(crate::test_git::git_command()
            .args(["init", "-q"])
            .current_dir(root)
            .status()
            .unwrap()
            .success());
        let mut lease: crate::SessionLease = toml::from_str(
            r#"id = "closed-review"
scope = "PR-3420"
slug = "review"
owner = "test@example.com"
worktree_path = ""
branch = ""
started_at = "2026-11-09T00:00:00Z"
hostname = "test"
"#,
        )
        .unwrap();
        lease.review_verb = true;
        lease.pr_head_sha = Some("head".into());
        lease.creator_pid = Some(u32::MAX);
        lease.hostname = crate::coordination::hostname();
        let reader = LeaseReader::new(root);
        let key = reader.key(&lease).unwrap().unwrap();
        assert!(matches!(key, WorkKey::Review { .. }));
        let store = crate::ownership::store::OwnershipStore::for_project(root)
            .unwrap()
            .unwrap();
        for bound in [true, false] {
            let why = if bound {
                review_finished_reason(&lease).unwrap()
            } else {
                "different generation".into()
            };
            store
                .update(&key, "test", crate::ownership::store::Expect::Any, |_| {
                    Some((
                        Ownership {
                            key: key.clone(),
                            generation: 0,
                            holder: None,
                            phase: Phase::Closed { why },
                        },
                        None,
                    ))
                })
                .unwrap();
            for rule in [LegacyRule::Pickup, LegacyRule::NonStale, LegacyRule::Review] {
                assert_eq!(
                    matches!(reader.pickup_owner(&lease, rule), OwnerVerdict::Dead { .. }),
                    bound
                );
            }
        }
        let mut command = std::process::Command::new("sleep");
        command.arg("30");
        let capture = aida_core::proc_identity::prepare_command(&mut command).unwrap();
        let mut child = command.spawn().unwrap();
        let admission = capture.finish(&mut child);
        admission.validate().unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
        // Both malformed JSON and an admitted envelope for a different PID must
        // retain ownership even when the lease's active PID is dead.
        lease.active_pid = Some(u32::MAX);
        for raw in [
            "invalid JSON".into(),
            serde_json::to_string(&admission).unwrap(),
        ] {
            lease.holder_record = Some(raw);
            let holder = dual::holder_for_lease(&lease, &dual::WriterFacts::here(), Utc::now());
            store
                .update(&key, "test", crate::ownership::store::Expect::Any, |_| {
                    Some((
                        Ownership {
                            key: key.clone(),
                            generation: 0,
                            holder: Some(holder),
                            phase: Phase::Leased,
                        },
                        None,
                    ))
                })
                .unwrap();
            for rule in [
                LegacyRule::Pickup,
                LegacyRule::NonStale,
                LegacyRule::Review,
                LegacyRule::Adoption,
            ] {
                assert!(!matches!(
                    reader.pickup_owner(&lease, rule),
                    OwnerVerdict::Dead { .. }
                ));
            }
            lease.worktree_path = root.to_path_buf();
            assert!(crate::leases::lease_adoption_refusal(root, &lease).is_some());
        }
    }
}
