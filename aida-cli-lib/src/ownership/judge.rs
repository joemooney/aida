//! The one pure liveness and reclaim rule (STORY-2325 PR-A).
//!
//! Nothing here reads a file, a process, git, or the clock: every fact is
//! injected, so the whole decision is a table the tests can walk.
// trace:STORY-2325 | ai:claude

use super::types::{
    Holder, HostFacts, OwnerVerdict, Ownership, Phase, Reclaim, TreeReading, Verdict, Witness,
    WitnessKind, WitnessReading,
};
use aida_core::process_probe::{
    parse_start_identity, Probe, ProcessIdentity, INITIAL_PID_NAMESPACE_INODE,
};
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

/// A1: the process witness a writer may record for `id`. Witnesses are
/// recorded from the host's view only: a writer in any pid namespace other
/// than the host's initial one (a sandboxed CLI call, a container) records
/// none, because the pid it sees names nothing a host-side reader can probe.
/// A process with no readable start identity is not recorded either; it
/// could never prove anything (A4).
// trace:STORY-2325 | ai:claude
pub(crate) fn process_witness(
    writer_pid_ns: Option<u64>,
    host: &str,
    id: ProcessIdentity,
    now: DateTime<Utc>,
) -> Option<Witness> {
    if writer_pid_ns != Some(INITIAL_PID_NAMESPACE_INODE) {
        return None;
    }
    parse_start_identity(&id.start)?;
    Some(Witness {
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
        } => judge_process(rec_host, id, *pid_ns, host),
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
    if rec_host != host.host {
        return WitnessReading::Unknown(format!("{id} runs on another host ({rec_host})"));
    }
    let Some((rec_boot, _)) = parse_start_identity(&id.start) else {
        return WitnessReading::Unknown(format!("{id} has no recorded start identity"));
    };
    let Some(boot) = host.boot.as_deref() else {
        return WitnessReading::Unknown("this host's boot id cannot be read".to_string());
    };
    // A reboot ends every process on the host, in every namespace.
    if boot != rec_boot {
        return WitnessReading::Dead(format!("the host rebooted since {id} was recorded"));
    }
    match (rec_ns, host.pid_ns) {
        (Some(rec), Some(ours)) if rec == ours => {}
        (None, _) => return WitnessReading::Unknown(format!("{id} has no recorded pid namespace")),
        (_, None) => {
            return WitnessReading::Unknown(
                "this reader's pid namespace cannot be read".to_string(),
            )
        }
        (Some(_), Some(_)) => {
            return WitnessReading::Unknown(format!(
                "{id} was recorded in another pid namespace, so it cannot be probed from here"
            ))
        }
    }
    match (host.probe)(id) {
        Probe::Alive => WitnessReading::Alive,
        Probe::Dead(why) => WitnessReading::Dead(why),
        Probe::Unknown(why) => WitnessReading::Unknown(why),
    }
}

fn describe(witness: &Witness) -> String {
    match &witness.kind {
        WitnessKind::Process { id, .. } => format!("{id} is running"),
        WitnessKind::Heartbeat { at, .. } => format!("heartbeat at {}", at.to_rfc3339()),
    }
}

/// A2: combine every witness. An Alive witness wins; otherwise the owner is
/// Dead when at least one witness is Dead (the Unknown ones abstain);
/// otherwise Unknown. No witness at all is Unknown.
// trace:STORY-2325 | ai:claude
pub(crate) fn judge_owner(
    holder: &Holder,
    host: &HostFacts<'_>,
    now: DateTime<Utc>,
) -> OwnerVerdict {
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
    if !dead.is_empty() {
        OwnerVerdict::Dead {
            proof: dead.join("; "),
        }
    } else if unknown.is_empty() {
        OwnerVerdict::Unknown {
            why: "the holder records no witness".to_string(),
        }
    } else {
        OwnerVerdict::Unknown {
            why: unknown.join("; "),
        }
    }
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
fn reclaim_dead_tree(
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
