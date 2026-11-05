//! STORY-2315: may a relaunch end a finished lane's dead lease by itself?
//!
//! A rework (`aida queue work --steal`, which `queue rework --work` chains) or
//! a review launch (`aida session start --review N`) finds the PR's branch
//! still held by the lease of a lane that already shipped and exited. Plain
//! `session end` refused those leases, so every relaunch stopped on "could not
//! end dead lease … add --force". This module is the safe check an operator
//! ran by hand before adding `--force`:
//!
//! 1. the lease's owning process is verifiably gone (a live one is skipped);
//! 2. its seat holds no uncommitted change;
//! 3. the seat's HEAD is already in the PR (an ancestor of, or equal to, the
//!    PR branch's head on origin).
//!
//! When all three hold, nothing in the seat is lost by `session end --force`,
//! and the caller ends the lease itself. A dirty seat, or a seat with commits
//! the PR lacks, refuses with that reason. A lease whose death cannot be
//! proven, whose seat cannot be read, or whose PR head cannot be read, is not
//! judged here: the caller keeps its previous behavior (plain `session end`,
//! whose own guards apply).
//!
//! Both relaunch paths turn the verdict into an action through the one
//! mapping [`end_action`], so they cannot drift apart.
//!
//! `session end` itself is not changed: this module only decides whether its
//! callers pass `--force`.
// trace:STORY-2315 | ai:claude

use crate::leases::SessionLease;
use aida_core::liveness::StaleLeaseRecovery;
use std::path::Path;

/// Whether the lease's owning process is gone, from the same classifier
/// `aida ps` renders.
// trace:STORY-2315 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OwnerLiveness {
    Live,
    Dead,
    /// The lease recorded no pid, so death cannot be proven.
    Unknown,
}

/// What the host read from the lease's seat.
// trace:STORY-2315 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SeatReading {
    /// The seat directory is gone: nothing to lose.
    Missing,
    /// `dirty` lists every uncommitted change; `unpushed` names a seat
    /// clone's commits origin lacks (always empty for a worktree, whose
    /// commits live in the main repository); `head` is the seat's HEAD
    /// commit, `None` when unborn.
    Read {
        dirty: Vec<String>,
        unpushed: Vec<String>,
        head: Option<String>,
    },
    /// The seat could not be read: not judged here, so plain `session end`
    /// (whose own guards read the seat again) decides.
    Unreadable(String),
}

/// The verdict for one lease on the PR's branch.
// trace:STORY-2315 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PrLeaseCheck {
    /// Owner gone, seat clean and fully in the PR: end it with `--force`.
    SafeToForceEnd,
    /// A live process backs the lease: never ended here.
    Live,
    /// Ending it could lose work; the text says what.
    Refuse(String),
    /// Death or the PR head cannot be established; the caller keeps its
    /// previous handling.
    NotProvable(String),
}

/// The pure decision. `is_ancestor(seat_head, pr_head)` answers whether the
/// seat's HEAD is contained in the PR head.
// trace:STORY-2315 | ai:claude
pub(crate) fn classify(
    liveness: OwnerLiveness,
    seat: &SeatReading,
    pr_head: Option<&str>,
    is_ancestor: impl Fn(&str, &str) -> bool,
) -> PrLeaseCheck {
    match liveness {
        OwnerLiveness::Live => return PrLeaseCheck::Live,
        OwnerLiveness::Unknown => {
            return PrLeaseCheck::NotProvable(
                "the lease records no process id, so its session cannot be proven to have exited"
                    .to_string(),
            )
        }
        OwnerLiveness::Dead => {}
    }
    let (dirty, unpushed, head) = match seat {
        SeatReading::Missing => return PrLeaseCheck::SafeToForceEnd,
        SeatReading::Unreadable(why) => {
            return PrLeaseCheck::NotProvable(format!("its seat could not be read ({why})"))
        }
        SeatReading::Read {
            dirty,
            unpushed,
            head,
        } => (dirty, unpushed, head),
    };
    if !dirty.is_empty() {
        return PrLeaseCheck::Refuse(format!(
            "its seat has {} uncommitted change(s): {}",
            dirty.len(),
            dirty_sample(dirty)
        ));
    }
    if !unpushed.is_empty() {
        return PrLeaseCheck::Refuse(format!(
            "its seat has commits the PR lacks: {}",
            dirty_sample(unpushed)
        ));
    }
    let Some(head) = head.as_deref() else {
        // An unborn HEAD with a clean tree holds nothing.
        return PrLeaseCheck::SafeToForceEnd;
    };
    let Some(pr_head) = pr_head else {
        return PrLeaseCheck::NotProvable(
            "the PR branch has no head on origin to compare the seat against".to_string(),
        );
    };
    if head == pr_head || is_ancestor(head, pr_head) {
        PrLeaseCheck::SafeToForceEnd
    } else {
        PrLeaseCheck::Refuse(format!(
            "its seat's HEAD {} has commits the PR head {} lacks",
            short(head),
            short(pr_head)
        ))
    }
}

/// Read every input of [`classify`] for `lease` and decide. `pr_branch` is the
/// PR's source branch on origin (a reviewer's `pr-N` lease is judged against
/// the PR it reviews, not its local branch).
// trace:STORY-2315 | ai:claude
pub(crate) fn check_pr_lease(main: &Path, lease: &SessionLease, pr_branch: &str) -> PrLeaseCheck {
    let liveness = owner_liveness(lease);
    if liveness != OwnerLiveness::Dead {
        return classify(liveness, &SeatReading::Missing, None, |_, _| false);
    }
    let seat = read_seat(main, lease, pr_branch);
    let pr_head = match &seat {
        SeatReading::Read {
            dirty,
            unpushed,
            head: Some(_),
        } if dirty.is_empty() && unpushed.is_empty() => pr_head_on_origin(main, pr_branch),
        _ => None,
    };
    classify(liveness, &seat, pr_head.as_deref(), |seat_head, pr_head| {
        is_ancestor(main, seat_head, pr_head)
    })
}

/// How a relaunch ends one lease blocking the PR.
// trace:STORY-2315 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LeaseEndAction {
    /// `session end --force`: the seat holds nothing the PR lacks.
    ForceEnd,
    /// Plain `session end`, whose own guards (dirty tree, live process)
    /// decide, as before STORY-2315.
    PlainEnd,
    /// Stop the relaunch with this message: ending the lease could lose work.
    Bail(String),
}

/// The one verdict-to-action mapping both relaunch paths (`queue work
/// --steal` and the review launch) use. Only a provable risk to work bails;
/// a live or unjudged lease keeps the plain end and its guards.
// trace:STORY-2315 | ai:claude
pub(crate) fn end_action(
    lease: &SessionLease,
    pr_branch: &str,
    check: &PrLeaseCheck,
) -> LeaseEndAction {
    match check {
        PrLeaseCheck::SafeToForceEnd => LeaseEndAction::ForceEnd,
        PrLeaseCheck::Refuse(reason) => {
            LeaseEndAction::Bail(refusal_message(lease, pr_branch, reason))
        }
        PrLeaseCheck::Live | PrLeaseCheck::NotProvable(_) => LeaseEndAction::PlainEnd,
    }
}

/// Read and decide for one lease. `pr_branch` is `None` when the PR a
/// reviewer's `pr-N` lease reviews cannot be named: the plain end applies.
// trace:STORY-2315 | ai:claude
pub(crate) fn plan_lease_end(
    main: &Path,
    lease: &SessionLease,
    pr_branch: Option<&str>,
) -> LeaseEndAction {
    match pr_branch {
        Some(pr_branch) => end_action(lease, pr_branch, &check_pr_lease(main, lease, pr_branch)),
        None => LeaseEndAction::PlainEnd,
    }
}

/// Every lease blocking a review launch: those on the PR's source branch,
/// then a previous reviewer's on the local review branch (`pr-N`), each once.
// trace:STORY-2315 | ai:claude
pub(crate) fn merge_review_conflicts(
    mut on_source: Vec<SessionLease>,
    on_review_branch: Vec<SessionLease>,
) -> Vec<SessionLease> {
    for l in on_review_branch {
        if !on_source.iter().any(|c| c.id == l.id) {
            on_source.push(l);
        }
    }
    on_source
}

/// The one-line refusal a relaunch bails with. It names the lease's own
/// branch (a reviewer's `pr-N`, say) and the PR branch it was judged against.
// trace:STORY-2315 | ai:claude
pub(crate) fn refusal_message(lease: &SessionLease, pr_branch: &str, reason: &str) -> String {
    let judged = if lease.branch == pr_branch {
        String::new()
    } else {
        format!(" (PR branch `{pr_branch}`)")
    };
    format!(
        "lease {} ({}, seat {}) holds branch `{}`{} and its session has exited, but it was not \
         ended automatically: {}. Commit and push the work to the PR, or discard it with \
         `aida session end {} --force`, then retry",
        lease.id,
        lease.role.as_deref().unwrap_or("(unset)"),
        lease.worktree_path.display(),
        lease.branch,
        judged,
        reason,
        lease.id,
    )
}

/// The PR branch a lease is judged against: its own branch, except a
/// reviewer's local `pr-N` / `mr-N` branch, whose PR's source branch is asked
/// of the forge (`None` when that cannot be read).
// trace:STORY-2315 | ai:claude
pub(crate) fn pr_branch_for_lease(main: &Path, lease: &SessionLease) -> Option<String> {
    let (forge, n) = match parse_review_branch(&lease.branch) {
        Some(found) => found,
        None => return Some(lease.branch.clone()),
    };
    crate::session_start::query_pr_source_branch(forge, n, main)
}

/// `pr-N` → GitHub PR N; `mr-N` → GitLab MR N.
// trace:STORY-2315 | ai:claude
pub(crate) fn parse_review_branch(
    branch: &str,
) -> Option<(crate::session_start::ReviewForge, u64)> {
    use crate::session_start::ReviewForge;
    let (forge, digits) = if let Some(rest) = branch.strip_prefix("pr-") {
        (ReviewForge::GitHub, rest)
    } else if let Some(rest) = branch.strip_prefix("mr-") {
        (ReviewForge::GitLab, rest)
    } else {
        return None;
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok().map(|n| (forge, n))
}

fn owner_liveness(lease: &SessionLease) -> OwnerLiveness {
    match crate::leases::stale_lease_recovery_for_lease(lease).verdict {
        StaleLeaseRecovery::Live => OwnerLiveness::Live,
        StaleLeaseRecovery::UnknownLiveness => OwnerLiveness::Unknown,
        StaleLeaseRecovery::ReclaimableClean { .. } | StaleLeaseRecovery::StaleDirty { .. } => {
            OwnerLiveness::Dead
        }
    }
}

/// Read the seat. Host git never runs in a seat clone: a clone is read through
/// the sandboxed probe, as `session end` reads it.
// trace:STORY-2315 | ai:claude
// trace:TASK-2032 | ai:claude
pub(crate) fn read_seat(main: &Path, lease: &SessionLease, pr_branch: &str) -> SeatReading {
    let path = &lease.worktree_path;
    if path.as_os_str().is_empty() || !path.exists() {
        return SeatReading::Missing;
    }
    if lease.tree_kind == crate::seat_clone::TreeKind::Clone {
        let Some(clone) = crate::seat_clone::host_recorded_clone(main, path) else {
            return SeatReading::Unreadable(
                "it is a seat clone the host does not recognise".to_string(),
            );
        };
        return match crate::seat_clone::probe_counting_origin(
            &clone.main,
            &clone.root,
            Some(pr_branch),
            &[],
            None,
        ) {
            crate::seat_clone::Probe::Known(facts) => {
                let mut unpushed = facts.unpushed_refs;
                if facts.unpushed > 0 && unpushed.is_empty() {
                    unpushed.push(format!(
                        "{} commit(s) that origin does not have",
                        facts.unpushed
                    ));
                }
                SeatReading::Read {
                    dirty: facts.dirty,
                    unpushed,
                    head: facts.head,
                }
            }
            crate::seat_clone::Probe::Unknown(why)
            | crate::seat_clone::Probe::HasSubmodules(why) => SeatReading::Unreadable(why),
        };
    }
    let dirty = crate::session_end::dirty_entries_ignoring_runtime_links(
        path,
        crate::status_sections::worktree_dirty_entries(path),
    );
    match git_stdout(path, &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"]) {
        Some(head) => SeatReading::Read {
            dirty,
            unpushed: Vec::new(),
            head: Some(head),
        },
        None if dirty.is_empty() && git_stdout(path, &["rev-parse", "--git-dir"]).is_some() => {
            SeatReading::Read {
                dirty,
                unpushed: Vec::new(),
                head: None,
            }
        }
        None => SeatReading::Unreadable(format!("git could not read HEAD in {}", path.display())),
    }
}

/// The PR branch's head on origin, refreshed first (best effort, bounded).
// trace:STORY-2315 | ai:claude
pub(crate) fn pr_head_on_origin(main: &Path, pr_branch: &str) -> Option<String> {
    let _ = crate::seat_clone::refresh_origin_branch(
        main,
        pr_branch,
        std::time::Duration::from_secs(20),
    );
    let tracking = format!("refs/remotes/origin/{pr_branch}^{{commit}}");
    git_stdout(main, &["rev-parse", "--verify", "--quiet", &tracking])
}

/// Is `seat_head` contained in `pr_head`? A seat commit main does not have
/// (a clone's unpushed work) reads `false`.
// trace:STORY-2315 | ai:claude
pub(crate) fn is_ancestor(main: &Path, seat_head: &str, pr_head: &str) -> bool {
    aida_core::git_ops::host_git_command(main)
        .args(["merge-base", "--is-ancestor", seat_head, pr_head])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Host git through the hardened builder, which refuses a seat clone.
// trace:STORY-2315 | ai:claude
fn git_stdout(dir: &Path, args: &[&str]) -> Option<String> {
    let out = aida_core::git_ops::host_git_command(dir)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn dirty_sample(dirty: &[String]) -> String {
    const MAX: usize = 5;
    let shown: Vec<&str> = dirty.iter().take(MAX).map(|s| s.trim()).collect();
    if dirty.len() > MAX {
        format!("{}, … (+{} more)", shown.join("; "), dirty.len() - MAX)
    } else {
        shown.join("; ")
    }
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(10)]
}

#[cfg(test)]
#[path = "tests/story_2315_pr_lease_takeover_tests.rs"]
mod story_2315_pr_lease_takeover_tests;
