//! The one pure liveness and reclaim rule (STORY-2325 PR-A).
//!
//! Nothing here reads a file, a process, git, or the clock: every fact is
//! injected, so the whole decision is a table the tests can walk.
// trace:STORY-2325 | ai:claude

use super::types::{
    Holder, HostFacts, OwnerVerdict, Ownership, Phase, Reclaim, TreeReading, Verdict, Witness,
    WitnessKind, WitnessReading,
};
use aida_core::process_probe::{parse_start_identity, ProcessIdentity};
use chrono::{DateTime, Utc};

/// The longest a single tool call may run before its harness kills it
/// (Claude Code's Bash ceiling, 2 hours). A seat inside one long call writes
/// no heartbeat, so a heartbeat window shorter than this would read a
/// working seat as dead (A3).
// trace:STORY-2325 | ai:claude
pub(crate) const LONGEST_TOOL_TIMEOUT_SECS: u64 = 2 * 60 * 60;

/// How long after its last beat a heartbeat still reads alive: at least
/// three intervals, and never shorter than the longest tool timeout (A3).
// trace:STORY-2325 | ai:claude
pub(crate) fn heartbeat_window(every_secs: u64) -> u64 {
    every_secs.saturating_mul(3).max(LONGEST_TOOL_TIMEOUT_SECS)
}

/// Build the identity envelope in established PID coordinates. This alone
/// supplies no holder admission; writers attach cooperative evidence for
/// generation-bound judgment. Unadmitted process envelopes remain Unknown.
// trace:STORY-2537 trace:BUG-2458 | ai:codex
pub(crate) fn process_witness(
    writer_pid_ns: Option<u64>,
    host: &str,
    id: ProcessIdentity,
    now: DateTime<Utc>,
) -> Option<Witness> {
    writer_pid_ns.filter(|ns| *ns != 0)?;
    parse_start_identity(&id.start)?;
    Some(Witness {
        admission: None,
        kind: WitnessKind::Process {
            host: host.to_string(),
            id,
            pid_ns: writer_pid_ns,
        },
        recorded_at: now,
    })
}

/// One witness, read from `host` at `now`.
// trace:STORY-2325 | ai:claude
pub(crate) fn judge_witness(
    witness: &Witness,
    host: &HostFacts<'_>,
    now: DateTime<Utc>,
) -> WitnessReading {
    match &witness.kind {
        WitnessKind::Process {
            host: rec_host,
            id,
            pid_ns,
        } => {
            let Some(record) = witness.admission.as_ref() else {
                return WitnessReading::Unknown(
                    "holder admission unavailable; cooperative rebind required".into(),
                );
            };
            let identity = match record.validate() {
                Ok(id) => id,
                Err(why) => return WitnessReading::Unknown(format!("holder refused: {why:?}")),
            };
            if !record.matches_node(host.node_id.as_deref()) {
                return WitnessReading::Unknown("holder runs on another node".into());
            }
            if identity.legacy() != *id || Some(identity.pidns_inode) != *pid_ns {
                return WitnessReading::Unknown(
                    "holder generation does not match process witness".into(),
                );
            }
            judge_process(rec_host, id, *pid_ns, host)
        }
        WitnessKind::Heartbeat { at, every_secs } => {
            if *every_secs == 0 {
                return WitnessReading::Unknown("the heartbeat records no interval".to_string());
            }
            // A beat dated in the future (clock skew) is fresh.
            let elapsed = (now - *at).num_seconds().max(0) as u64;
            let window = heartbeat_window(*every_secs);
            if elapsed > window {
                WitnessReading::Dead(format!(
                    "its heartbeat stopped {elapsed}s ago (expires after {window}s)"
                ))
            } else {
                WitnessReading::Alive
            }
        }
    }
}

fn judge_process(
    rec_host: &str,
    id: &ProcessIdentity,
    rec_ns: Option<u64>,
    host: &HostFacts<'_>,
) -> WitnessReading {
    use aida_core::proc_identity::{judge, ObserverFacts, ProcIdentity, Verdict};
    let Some(identity) = ProcIdentity::from_record(id, rec_ns) else {
        return WitnessReading::Unknown("insufficient process identity".into());
    };
    let facts = ObserverFacts {
        same_node: rec_host == host.host,
        boot_id: host.boot.as_deref(),
        pidns_inode: host.pid_ns,
        complete_procfs: true,
    };
    match judge(&identity, &facts, || (host.probe)(id)) {
        Verdict::Alive => WitnessReading::Alive,
        Verdict::Dead(why) => WitnessReading::Dead(format!("{why:?}")),
        Verdict::ForeignUnknown(why) => WitnessReading::Unknown(format!("{why:?}")),
    }
}

fn describe(witness: &Witness) -> String {
    match &witness.kind {
        WitnessKind::Process { id, .. } => format!("{id} is running"),
        WitnessKind::Heartbeat { at, .. } => format!("heartbeat at {}", at.to_rfc3339()),
    }
}

/// PR-C0: an unadmitted legacy holder loses work ownership when its session
/// has no lease. This releases only the record, never the seat or its tree.
/// Admitted (including refused) identities and merge-lease holders retain
/// their own lifecycle rules.
// trace:STORY-2325 | ai:codex
pub(crate) fn legacy_release_by_rule(holder: &Holder, has_lease: bool) -> bool {
    !has_lease
        && !holder.session.starts_with("merge-lease:")
        && holder.witnesses.iter().all(|w| w.admission.is_none())
}

/// Admitted records select one process generation. Legacy rows retain their
/// heartbeat policy; activity cannot rehabilitate an admitted dead worker.
// trace:STORY-2537 trace:BUG-2458 | ai:codex
pub(crate) fn judge_owner(
    holder: &Holder,
    host: &HostFacts<'_>,
    now: DateTime<Utc>,
) -> OwnerVerdict {
    // Legacy rows keep the existing heartbeat/combined-witness policy until
    // cutover. Admitted process generations below never borrow activity from
    // another witness. trace:STORY-2537 trace:BUG-2458 | ai:codex
    if holder.witnesses.iter().all(|w| w.admission.is_none()) {
        let mut dead = Vec::new();
        let mut unknown = Vec::new();
        for witness in &holder.witnesses {
            match judge_witness(witness, host, now) {
                WitnessReading::Alive => {
                    return OwnerVerdict::Alive {
                        witness: describe(witness),
                    }
                }
                WitnessReading::Dead(why) => dead.push(why),
                WitnessReading::Unknown(why) => unknown.push(why),
            }
        }
        return if !dead.is_empty() {
            OwnerVerdict::Dead {
                proof: dead.join("; "),
            }
        } else {
            OwnerVerdict::Unknown {
                why: if unknown.is_empty() {
                    "the holder records no witness".into()
                } else {
                    unknown.join("; ")
                },
            }
        };
    }
    // trace:STORY-2537 trace:BUG-2458 | ai:codex
    let processes: Vec<_> = holder
        .witnesses
        .iter()
        .filter(|w| matches!(w.kind, WitnessKind::Process { .. }))
        .collect();
    if processes.len() != 1 {
        return OwnerVerdict::Unknown {
            why: "one admitted holder generation is required; operator repair required".into(),
        };
    }
    // A heartbeat belongs to activity policy; it never overrides this holder's
    // death, missing admission or refusal.
    return match judge_witness(processes[0], host, now) {
        WitnessReading::Alive => OwnerVerdict::Alive {
            witness: describe(processes[0]),
        },
        WitnessReading::Dead(proof) => OwnerVerdict::Dead { proof },
        WitnessReading::Unknown(why) => OwnerVerdict::Unknown { why },
    };
}

/// The judge. `pr_head` is the head on origin of the PR the work is in (or
/// is reviewed against), when there is one; `is_ancestor(a, b)` answers
/// whether commit `a` is contained in `b`.
// trace:STORY-2325 | ai:claude
pub(crate) fn judge(
    rec: &Ownership,
    host: &HostFacts<'_>,
    tree: &TreeReading,
    pr_head: Option<&str>,
    is_ancestor: &dyn Fn(&str, &str) -> bool,
    now: DateTime<Utc>,
) -> Verdict {
    let Some(holder) = rec.holder.as_ref() else {
        return Verdict {
            owner: OwnerVerdict::Unknown {
                why: "no holder is recorded".to_string(),
            },
            reclaim: Reclaim::Free,
        };
    };
    let owner = judge_owner(holder, host, now);
    let reclaim = match (&rec.phase, &owner) {
        // The work belongs to the PR, or is finished: the old seat blocks
        // nothing. Free never touches its tree (A7).
        (Phase::InPr { .. } | Phase::Closed { .. }, _) => Reclaim::Free,
        (_, OwnerVerdict::Alive { witness }) => {
            Reclaim::Keep(format!("the holder is running ({witness})"))
        }
        (_, OwnerVerdict::Unknown { why }) => {
            Reclaim::Keep(format!("the holder cannot be proven gone: {why}"))
        }
        (_, OwnerVerdict::Dead { .. }) => reclaim_dead_tree(tree, pr_head, is_ancestor),
    };
    Verdict { owner, reclaim }
}

/// A dead holder's tree: ended by rule only when it holds nothing the PR
/// lacks. Today's `pr_lease_takeover::classify`, plus a non-git path.
pub(crate) fn reclaim_dead_tree(
    tree: &TreeReading,
    pr_head: Option<&str>,
    is_ancestor: &dyn Fn(&str, &str) -> bool,
) -> Reclaim {
    let (dirty, unpushed, head) = match tree {
        TreeReading::Missing => return Reclaim::EndBySafeRule,
        TreeReading::Unreadable(why) => {
            return Reclaim::Keep(format!("its tree could not be read ({why})"))
        }
        TreeReading::NotAGitTree => {
            return Reclaim::Refuse(
                "its tree is not a git checkout; an operator must archive or remove it".to_string(),
            )
        }
        TreeReading::Read {
            dirty,
            unpushed,
            head,
        } => (dirty, unpushed, head),
    };
    if !dirty.is_empty() {
        return Reclaim::Refuse(format!(
            "its tree has {} uncommitted change(s): {}",
            dirty.len(),
            sample(dirty)
        ));
    }
    if !unpushed.is_empty() {
        return Reclaim::Refuse(format!(
            "its tree has commits origin lacks: {}",
            sample(unpushed)
        ));
    }
    let Some(head) = head.as_deref() else {
        // An unborn HEAD with a clean tree holds nothing.
        return Reclaim::EndBySafeRule;
    };
    let Some(pr_head) = pr_head else {
        return Reclaim::Keep(
            "there is no PR head on origin to compare its tree against".to_string(),
        );
    };
    if head == pr_head || is_ancestor(head, pr_head) {
        Reclaim::EndBySafeRule
    } else {
        Reclaim::Refuse(format!(
            "its HEAD {} has commits the PR head {} lacks",
            short(head),
            short(pr_head)
        ))
    }
}

fn sample(items: &[String]) -> String {
    const MAX: usize = 5;
    let shown: Vec<&str> = items.iter().take(MAX).map(|s| s.trim()).collect();
    if items.len() > MAX {
        format!("{}, … (+{} more)", shown.join("; "), items.len() - MAX)
    } else {
        shown.join("; ")
    }
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(10)]
}
