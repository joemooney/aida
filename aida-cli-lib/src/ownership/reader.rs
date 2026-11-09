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
pub(crate) struct LeaseReader<'a> {
    root: &'a Path,
    reqs: std::cell::OnceCell<Option<aida_core::RequirementsStore>>,
}

impl<'a> LeaseReader<'a> {
    pub(crate) fn new(root: &'a Path) -> Self {
        Self {
            root,
            reqs: std::cell::OnceCell::new(),
        }
    }

    fn record(&self, lease: &crate::SessionLease) -> Result<Option<Ownership>> {
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
                self.reqs.get().is_none(),
                "lease scope could not be resolved through the spec store"
            );
            return Ok(None);
        };
        load(self.root, &key)
    }

    pub(crate) fn retained(&self, lease: &crate::SessionLease) -> bool {
        !matches!(self.pickup_owner(lease), OwnerVerdict::Dead { .. })
    }

    /// Temporary legacy compatibility for the four Part B pickup probes and
    /// adoption. Remove once every production holder writes an admitted witness.
    // trace:TASK-3420 | ai:codex
    fn pickup_owner(&self, lease: &crate::SessionLease) -> OwnerVerdict {
        let record = self.record(lease);
        let verdict = lease_owner_record(
            lease,
            record
                .as_ref()
                .map(Clone::clone)
                .map_err(|e| anyhow::anyhow!("{e:#}")),
        );
        let legacy_unknown = match &record {
            Ok(None) => lease.holder_record.is_none(),
            Ok(Some(record)) => record.holder.as_ref().is_some_and(|holder| {
                holder.session == lease.id
                    && holder.witnesses.iter().all(|w| w.admission.is_none())
                    && holder
                        .witnesses
                        .iter()
                        .all(|w| matches!(w.kind, super::WitnessKind::Process { .. }))
            }),
            Err(_) => false,
        };
        if matches!(verdict, OwnerVerdict::Unknown { .. }) && legacy_unknown {
            let live = crate::process_probe::probe_live_claude_sessions();
            return if crate::leases::lease_is_live(&live, Utc::now())(lease) {
                OwnerVerdict::Alive {
                    witness: "legacy lease rule".into(),
                }
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
