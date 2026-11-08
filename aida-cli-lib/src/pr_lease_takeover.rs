//! Relaunch I/O adapter for the authoritative ownership record.
//!
//! The shared judge supplies process liveness and reclaim safety. Missing,
//! unreadable or superseded ownership refuses cleanup. A positive death proof
//! plus a clean, remotely covered tree permits ordinary guarded session end;
//! automated callers never receive a force override. Free work does not grant
//! permission to remove its old tree. Legacy pure fixtures remain test-only.
// trace:STORY-2325 | ai:codex

use crate::leases::SessionLease;

use std::path::Path;

/// Whether the lease's owning process is gone, from the same classifier
/// `aida ps` renders.
// trace:STORY-2315 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(test)]
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
    /// Owner proved dead, seat clean and covered: guarded end by safe rule.
    SafeToEnd,
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
#[cfg(test)]
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
        SeatReading::Missing => return PrLeaseCheck::SafeToEnd,
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
        return PrLeaseCheck::SafeToEnd;
    };
    let Some(pr_head) = pr_head else {
        return PrLeaseCheck::NotProvable(
            "the PR branch has no head on origin to compare the seat against".to_string(),
        );
    };
    if head == pr_head || is_ancestor(head, pr_head) {
        PrLeaseCheck::SafeToEnd
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
    // The record is authoritative. Free grants no permission to remove a tree;
    // retained handed-off seats still pass session_end's independent guards.
    // trace:STORY-2325 | ai:codex
    let record = match crate::ownership::reader::lease_record(main, lease) {
        Ok(Some(record)) => record,
        Ok(None) => {
            return PrLeaseCheck::NotProvable(
                "ownership record missing; on the host run `aida own migrate --dry-run`, then `aida own migrate --execute`".into(),
            )
        }
        Err(e) => {
            return PrLeaseCheck::NotProvable(format!("ownership evidence unreadable: {e:#}"))
        }
    };
    // An InPr record is free to acquire, but its old seat is still protected.
    // Only the handing-off session may use its own process evidence for cleanup.
    // trace:STORY-2325 | ai:codex
    let cleanup_record = if record
        .holder
        .as_ref()
        .is_some_and(|h| h.session == lease.id)
    {
        record.clone()
    } else if record.holder.is_none()
        && matches!(&record.phase, crate::ownership::Phase::InPr { handed_off_by: Some(session), .. }
            if session == &lease.id)
    {
        let mut cleanup = record.clone();
        cleanup.holder = Some(crate::ownership::dual::holder_for_lease(
            lease,
            &crate::ownership::dual::WriterFacts::here(),
            chrono::Utc::now(),
        ));
        cleanup
    } else {
        return PrLeaseCheck::NotProvable("this lease no longer holds the ownership record".into());
    };
    match crate::ownership::reader::owner(&cleanup_record) {
        crate::ownership::OwnerVerdict::Alive { .. } => return PrLeaseCheck::Live,
        crate::ownership::OwnerVerdict::Unknown { why } => return PrLeaseCheck::NotProvable(why),
        crate::ownership::OwnerVerdict::Dead { .. } => {}
    }
    let seat = read_seat(main, lease, pr_branch);
    let mut remotely_covered = false;
    if matches!(&seat, SeatReading::Read { dirty, .. } if dirty.is_empty()) {
        match crate::session_end::remote_missing_commits_for_lease(main, lease) {
            Ok(0) => remotely_covered = true,
            Ok(n) => {
                return PrLeaseCheck::Refuse(format!(
                    "its seat has {n} commit(s) that no remote has"
                ))
            }
            Err(e) => {
                return PrLeaseCheck::NotProvable(format!("remote coverage unreadable: {e:#}"))
            }
        }
    }

    let pr_head = match &seat {
        SeatReading::Read {
            dirty,
            unpushed,
            head: Some(_),
        } if dirty.is_empty() && unpushed.is_empty() => pr_head_on_origin(main, pr_branch),
        _ => None,
    };
    // Only non-PR implementer lanes may use all-remote coverage in place
    // of a PR head. The ownership judge still decides tree safety.
    // trace:STORY-2325 | ai:codex
    let covered_without_pr = remotely_covered
        && !lease.review_verb
        && record
            .holder
            .as_ref()
            .is_some_and(|h| h.role == crate::ownership::Role::Implementer)
        && !lease.scope.to_ascii_lowercase().starts_with("pr-")
        && !lease.scope.to_ascii_lowercase().starts_with("mr-")
        && matches!(&record.phase, crate::ownership::Phase::Leased);
    let tree = match seat {
        SeatReading::Missing => crate::ownership::TreeReading::Missing,
        SeatReading::Unreadable(why) => crate::ownership::TreeReading::Unreadable(why),
        SeatReading::Read {
            dirty,
            unpushed,
            head,
        } => crate::ownership::TreeReading::Read {
            dirty,
            unpushed,
            head,
        },
    };
    // The owner was judged above; the shared reclaim rule protects the tree.
    match crate::ownership::reclaim_dead_tree(
        &tree,
        pr_head.as_deref(),
        covered_without_pr,
        &|a, b| is_ancestor(main, a, b),
    ) {
        crate::ownership::Reclaim::EndBySafeRule => PrLeaseCheck::SafeToEnd,
        crate::ownership::Reclaim::Refuse(why) => PrLeaseCheck::Refuse(why),
        crate::ownership::Reclaim::Keep(why) => PrLeaseCheck::NotProvable(why),
        crate::ownership::Reclaim::Free => {
            PrLeaseCheck::NotProvable("free work does not authorize tree removal".into())
        }
    }
}

/// How a relaunch ends one lease blocking the PR.
// trace:STORY-2315 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LeaseEndAction {
    /// Guarded session end: the dead seat holds no unpublished work.
    SafeEnd,
    /// Stop the relaunch with this message: ending the lease could lose work.
    Bail(String),
}

/// Both relaunch paths retain live or unknown ownership. SafeEnd still
/// passes every ordinary session-end tree and live-process guard.
// trace:STORY-2315 | ai:claude
pub(crate) fn end_action(
    lease: &SessionLease,
    pr_branch: &str,
    check: &PrLeaseCheck,
) -> LeaseEndAction {
    match check {
        PrLeaseCheck::SafeToEnd => LeaseEndAction::SafeEnd,
        PrLeaseCheck::Refuse(reason) => {
            LeaseEndAction::Bail(refusal_message(lease, pr_branch, reason))
        }
        PrLeaseCheck::Live => {
            LeaseEndAction::Bail(format!("ownership holder is alive; wait for voluntary handoff or have its owner run `aida session end {}`", lease.id))
        }
        PrLeaseCheck::NotProvable(why) => {
            LeaseEndAction::Bail(format!("ownership retained: {why}; inspect `aida session end {}`", lease.id))
        }
    }
}

/// Read and decide for one lease. `pr_branch` is `None` when the PR a
/// reviewer's `pr-N` lease reviews cannot be named, ownership is retained.
// trace:STORY-2315 | ai:claude
pub(crate) fn plan_lease_end(
    main: &Path,
    lease: &SessionLease,
    pr_branch: Option<&str>,
) -> LeaseEndAction {
    match pr_branch {
        Some(pr_branch) => {
            let check = check_pr_lease(main, lease, pr_branch);
            let action = end_action(lease, pr_branch, &check);
            match action {
                LeaseEndAction::Bail(mut message) if matches!(check, PrLeaseCheck::Refuse(_)) => {
                    message.push_str(&crate::queue_cmd::scope_conflict_resume_hint(
                        &lease.scope,
                        &lease.id,
                        crate::leases::lease_adoption_refusal(main, lease),
                        None,
                        lease.tree_kind == crate::seat_clone::TreeKind::Clone,
                    ));
                    LeaseEndAction::Bail(message)
                }
                action => action,
            }
        }
        None => LeaseEndAction::Bail("PR branch unknown; ownership retained".into()),
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
         ended automatically: {}. Commit and push the work on its branch, or discard it with \
         `aida session end {} --force --discard-unpushed`, then retry",
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

/// Only a linked, non-claim reviewer lease gets forge-head coverage.
// trace:BUG-2469 | ai:codex
pub(crate) fn reviewer_target(
    lease: &SessionLease,
) -> Option<(crate::session_start::ReviewForge, u64)> {
    if !lease.review_verb
        || lease.claim_verb
        || lease.tree_kind != crate::seat_clone::TreeKind::Worktree
    {
        return None;
    }
    parse_review_branch(&lease.branch)
}

/// Fetch a current PR/MR head without changing the occupied review branch or
/// shared FETCH_HEAD. Invocation-private refs prevent concurrent fetch races.
// trace:BUG-2469 | ai:codex
pub(crate) fn fetch_review_head(
    main: &Path,
    forge: crate::session_start::ReviewForge,
    n: u64,
) -> anyhow::Result<String> {
    use aida_core::git_ops::RemoteCredentialsExt as _;
    let target = format!("refs/aida/review-coverage/{}", uuid::Uuid::new_v4());
    let refspec = format!("refs/{}:{target}", forge.pr_head_ref(n));
    let result = (|| {
        let mut command = aida_core::git_ops::host_git_command(main);
        command
            .args([
                "fetch",
                "--no-write-fetch-head",
                "--no-tags",
                "origin",
                &refspec,
            ])
            .remote_credentials(main, "origin");
        let out =
            crate::command_output_with_timeout(command, std::time::Duration::from_secs(20))
                .ok_or_else(|| anyhow::anyhow!("review-head fetch failed to start or timed out"))?;
        anyhow::ensure!(
            out.status.success(),
            "cannot fetch review head: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
        git_stdout(
            main,
            &["rev-parse", "--verify", &format!("{target}^{{commit}}")],
        )
        .ok_or_else(|| anyhow::anyhow!("cannot resolve fetched review head"))
    })();
    let cleanup = aida_core::git_ops::host_git_command(main)
        .args(["update-ref", "-d", &target])
        .output();
    let head = result?;
    anyhow::ensure!(
        cleanup?.status.success(),
        "cannot release temporary review-head ref"
    );
    Ok(head)
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
    // Apply the same already-pushed index exception as session end.
    // trace:BUG-2450 | ai:codex
    let dirty = if crate::session_end::staged_tree_is_pushed(path, pr_branch, &dirty) {
        Vec::new()
    } else {
        dirty
    };
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

/// Leave unsafe dead seats intact and route the exact lease/reason once.
// trace:BUG-2450 | ai:codex
pub(crate) fn notify_advisor(
    main: &Path,
    lease: &SessionLease,
    reason: &str,
) -> anyhow::Result<()> {
    use aida_core::mailbox::{Intent, Message, Recipient, SenderSource};
    use sha2::{Digest, Sha256};
    let id = format!(
        "review-lease-{:x}",
        Sha256::digest(format!("{}\0{reason}", lease.id))
    );
    let message = Message {
        id: id.clone(),
        thread_id: id,
        from: "review-launch".into(),
        to: Recipient::Agent("advisor".into()),
        timestamp: chrono::Utc::now().timestamp_millis(),
        in_reply_to: None,
        subject: Some(format!("Review blocked by retained lease {}", lease.id)),
        body: reason.into(),
        urgent: false,
        intent: Intent::Request,
        retracted: false,
        deleted: false,
        archived: false,
        from_source: SenderSource::Explicit,
        from_role: None,
        relayed_from: None,
    };
    // Keep an existing notice's archive/retraction state and timestamp.
    if aida_core::mailbox::read_local_messages(main)?
        .iter()
        .any(|old| old.id == message.id)
    {
        return Ok(());
    }
    match aida_core::mailbox::write_local_message(main, &message) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error.into()),
    }
}
