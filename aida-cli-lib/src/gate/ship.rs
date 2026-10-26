//! Ship, hold-release, drive-ownership, and approval gate decisions for `aida pr ship`.
// trace:STORY-1889 | ai:claude
// mandate-gate

// trace:TASK-1602 | ai:codex
#[derive(Debug, Clone)]
pub(crate) struct ShipDriveBlock {
    pub(crate) reason: ShipMergeBlockReason,
    pub(crate) detail: String,
    pub(crate) timed_out: bool,
}

/// The existing PR-keyed guard, with evidence tied to the owning member.
/// Never attribute the current sibling's session/log to a different PR.
// trace:TASK-1253, TASK-1292, TASK-1602 | ai:codex
pub(crate) fn probe_ship_drive_guard(
    root: &std::path::Path,
    pr: u64,
    cwd_scope: Option<&str>,
    drive_seat: bool,
    allow: bool,
) -> Option<ShipDriveBlock> {
    let lock = match crate::drain_lock::probe_lock(root) {
        crate::drain_lock::LockStatus::Running(lock) => Some(lock),
        _ => None,
    };
    let state = lock
        .as_ref()
        .and_then(|_| crate::drain_state::DrainState::read(root));
    let members = state
        .as_ref()
        .map(|s| {
            s.members
                .iter()
                .filter(|m| m.is_running())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let hold = crate::gate::merge_hold_core::read_hold(root, pr);
    let owners = members
        .iter()
        .filter(|m| {
            m.pr.map(u64::from) == Some(pr)
                || hold.as_ref().is_some_and(|reason| reason.contains(&m.spec))
        })
        .copied()
        .collect::<Vec<_>>();
    let caller =
        drive_seat || cwd_scope.is_some_and(|scope| members.iter().any(|m| m.spec == scope));
    let reason = ship_merge_block_reason(caller, !owners.is_empty(), allow)?;
    let mut detail = String::from("drive seat cannot wait for its own pipeline");
    if let (Some(lock), Some(state)) = (lock, state.as_ref()) {
        detail = format!(
            "live drive PID {} (wave {}); {}",
            lock.pid,
            if lock.wave_id.is_empty() {
                "unknown"
            } else {
                &lock.wave_id
            },
            lock.command
        );
        for member in owners {
            let current = state.current.as_deref() == Some(member.spec.as_str());
            let phase = crate::drain_state::drain_phase_display(&member.state);
            detail.push_str(&format!("; {} phase {phase}", member.spec));
            if current {
                // `current` is rewritten at phase entry independently of
                // `run_uuid`; overlap can leave a sibling's token here.
                // The snapshot cannot corroborate a spec/run association.
                // trace:TASK-1602 | ai:codex
                detail.push_str("; run unavailable (shared run identity is uncorroborated)");
                let activity = state.current_session_id.as_deref().and_then(|session| {
                    let vendor = state
                        .current_vendor
                        .as_deref()
                        .and_then(crate::session::HeadlessVendor::parse)
                        .unwrap_or_else(|| crate::session::resolve_headless_vendor(root));
                    crate::vendor_activity::snapshot(
                        vendor,
                        &crate::vendor_activity::VendorActivityContext::new(root, session),
                    )
                    .last_activity
                });
                if let Some(age) = activity.and_then(|at| at.elapsed().ok()) {
                    detail.push_str(&format!(
                        "; headless activity last written {}s ago",
                        age.as_secs()
                    ));
                } else {
                    detail.push_str("; headless activity unavailable (PID liveness does not establish progress)");
                }
            } else {
                detail.push_str("; member session/activity unavailable (current session belongs to another member)");
            }
        }
    }
    Some(ShipDriveBlock {
        reason,
        detail,
        timed_out: false,
    })
}

/// A bounded ownership wait. Every iteration observes fresh state and reports
/// progress; release continues into the ordinary ship gates. Seats fail closed
/// immediately, including with --wait. The override is handled by the probe.
// trace:TASK-1602 | ai:codex
pub(crate) fn wait_for_drive_release(
    budget: Option<std::time::Duration>,
    interval: std::time::Duration,
    mut probe: impl FnMut() -> Option<ShipDriveBlock>,
    mut progress: impl FnMut(&ShipDriveBlock, f64),
) -> Result<(), ShipDriveBlock> {
    let started = std::time::Instant::now();
    loop {
        let Some(mut block) = probe() else {
            return Ok(());
        };
        if block.reason == ShipMergeBlockReason::DriveSeat {
            return Err(block);
        }
        let Some(budget) = budget else {
            return Err(block);
        };
        let remaining = budget.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            block.timed_out = true;
            return Err(block);
        }
        progress(&block, remaining.as_secs_f64());
        std::thread::sleep(
            interval
                .min(remaining)
                .min(std::time::Duration::from_secs(60)),
        );
    }
}

// trace:TASK-1606 | ai:codex
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub(crate) enum ShipRefusal {
    CiRed = 20,
    CiTimeout = 21,
    NotMergeable = 22,
    Review = 23,
    StaleDefinition = 24,
    MergeHold = 25,
    /// STORY-1690: the head is behind its base (or that could not be
    /// computed) and no CI definition changed.
    // trace:STORY-1690 | ai:claude
    BehindBase = 26,
}

#[derive(Debug)]
pub(crate) struct ShipFailure {
    pub reason: ShipRefusal,
    pub message: String,
}

impl std::fmt::Display for ShipFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.message.fmt(formatter)
    }
}
impl std::error::Error for ShipFailure {}

impl ShipRefusal {
    pub(crate) fn error(self, message: impl std::fmt::Display) -> anyhow::Error {
        ShipFailure {
            reason: self,
            message: message.to_string(),
        }
        .into()
    }
}

/// STORY-1663: rebase merges must always be pinned to a non-empty head SHA
/// (`--match-head-commit <SHA>`) so a concurrent push never rebases unreviewed
/// commits onto the base branch.
// trace:STORY-1663 | ai:antigravity
pub(crate) fn require_rebase_head_pin(
    pr_number: u64,
    method: crate::forge::MergeMethod,
    match_head: Option<&str>,
) -> Result<(), String> {
    if method != crate::forge::MergeMethod::Rebase {
        return Ok(());
    }
    match match_head.map(str::trim).filter(|sha| !sha.is_empty()) {
        Some(_) => Ok(()),
        None => Err(format!(
            "PR-{pr_number}: refusing to rebase-merge without a pinned head commit (`--match-head-commit`); \
             the PR head SHA could not be resolved."
        )),
    }
}

/// STORY-1663: refuse when the PR head moved between preflight/CI and the
/// under-lease merge step (a new commit was pushed after CI or preflight).
// trace:STORY-1663 | ai:antigravity
pub(crate) fn stale_head_after_ci_refusal(
    pr_number: u64,
    pre_ci_head: Option<&str>,
    merge_head: Option<&str>,
) -> Option<String> {
    // TASK-1623: an unreadable head at either point cannot prove CI and
    // review covered the head being merged, so it refuses (fail closed)
    // instead of skipping the check. trace:TASK-1951 trace:TASK-1623 | ai:claude
    fn read(head: Option<&str>) -> Option<&str> {
        head.map(str::trim).filter(|s| !s.is_empty())
    }
    let (Some(pre), Some(post)) = (read(pre_ci_head), read(merge_head)) else {
        let at = if read(pre_ci_head).is_none() {
            "preflight"
        } else {
            "merge"
        };
        return Some(format!(
            "PR-{pr_number}'s head could not be read at {at}, so the merge cannot be \
             pinned to the head CI and review covered — refusing. Re-run \
             `aida pr ship {pr_number}` once the forge answers."
        ));
    };
    if pre.eq_ignore_ascii_case(post) {
        None
    } else {
        Some(format!(
            "PR-{pr_number}'s head moved from `{pre}` to `{post}` after preflight/CI — \
             refusing stale head. Re-run `aida pr ship {pr_number}` after CI and review \
             cover `{post}`."
        ))
    }
}

/// Why `aida pr ship` must leave a PR open for the drive's independent reviewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShipMergeBlockReason {
    DriveSeat,
    DriveOwnedPr,
}

/// TASK-1292: whether a reviewer phase is currently live against a given PR
/// number — the pure decision behind `aida pr ship`'s drive-ownership check.
///
/// The 2026-09-18 near-miss: a reviewer phase running under BUG-1236 was
/// live on PR #1948, but the drive's "current spec" bookkeeping pointed at
/// STORY-1221 (shelved). A check that asks "does the drive's current spec
/// own this PR" misses that case entirely; the only question that can't miss
/// it is "is ANY live member — whichever spec it belongs to — phase-bound to
/// THIS PR". That's what this function answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewerLiveness {
    /// A live (`in-phase-*`) member is phase-bound to exactly this PR.
    OnThisPr,
    /// At least one member is live and phase-bound to a PR — just not this
    /// one. Not grounds to block a ship of an unrelated PR.
    Elsewhere,
    /// No live member is phase-bound to any PR (including: no drive at all).
    None,
}

/// `members` is `(is_running, pr)` per drive member. The caller derives
/// liveness through `DrainMember::is_running`, so this guard cannot drift from
/// the state producer's format. `pr` is the member's currently-bound PR
/// (STORY-1033/TASK-1292 binds it live, at phase-entry, not only at the
/// member's terminal outcome).
// trace:TASK-1292 | ai:claude
// trace:BUG-1429 | ai:codex
pub fn reviewer_liveness_for_pr(
    members: impl IntoIterator<Item = (bool, Option<u32>)>,
    pr: u32,
) -> ReviewerLiveness {
    let mut saw_other_live_pr = false;
    for (is_running, member_pr) in members {
        if !is_running {
            continue;
        }
        match member_pr {
            Some(bound) if bound == pr => return ReviewerLiveness::OnThisPr,
            Some(_) => saw_other_live_pr = true,
            None => {}
        }
    }
    if saw_other_live_pr {
        ReviewerLiveness::Elsewhere
    } else {
        ReviewerLiveness::None
    }
}

/// BUG-1566: `aida pr ship` releasing a supervised merge-hold is the SAME
/// integrity floor as `aida merge-hold clear` — a human at an interactive
/// terminal. Without that authority the ship refuses before touching the hold,
/// and points at the human release path. There is no carve-out: dispatch,
/// advisor, reviewer and headless seats all hit this, so the two release
/// paths can never disagree on who may clear a hold. `Some(msg)` = refuse.
// trace:BUG-1566 | ai:claude
pub fn ship_hold_release_refusal(
    hold_present: bool,
    integrity_floor_authority: bool,
    pr: u64,
) -> Option<String> {
    if !hold_present || integrity_floor_authority {
        return None;
    }
    Some(format!(
        "PR-{pr} is under a supervised merge-hold; `aida pr ship` will not release it without \
         a human at an interactive terminal (the same integrity floor as `aida merge-hold clear`). \
         A human must run `aida merge-hold clear {pr}`, then re-run `aida pr ship {pr}`."
    ))
}

/// BUG-1499: the release-step decision. A hold that exists ONLY as the forge
/// label is refused for every caller, a human included: `pr ship` would drop
/// the label with no clearance record, so it is sent to
/// `aida merge-hold clear <PR>` (human floor + recorded clearance). A marker
/// hold falls through to the BUG-1566 floor. `Some(msg)` = refuse.
// trace:BUG-1499 | ai:claude
pub fn ship_hold_release_refusal_for(
    marker_present: bool,
    label_only: bool,
    integrity_floor_authority: bool,
    pr: u64,
) -> Option<String> {
    if !marker_present && label_only {
        return Some(format!(
            "PR-{pr} is held by the `aida:merge-hold` label alone (no marker, no recorded reason); \
             `aida pr ship` will not release it. A human clears it with `aida merge-hold clear {pr}` \
             (which records the clearance), then re-runs `aida pr ship {pr}`."
        ));
    }
    ship_hold_release_refusal(marker_present, integrity_floor_authority, pr)
}

/// BUG-1532: ANY merge-hold marker binds `aida pr ship` — whether or not a
/// drive is live and whatever its reason text says (the old guard honoured a
/// marker only when its prose named a running drive member's spec, so with
/// the lane stopped no hold bound the client side at all).
///
/// The question is "is this command the decision the hold is waiting for?":
///   - no human at a terminal → never (BUG-1566 floor): refuse, before CI
///     is watched, the red gate discounted, or the hold touched;
///   - a human, on a SUPERVISION or DECISION hold → yes: the explicit merge
///     is that decision, so the BUG-1167 release proceeds;
///   - a human, on a REFUSAL hold (typed rework, or an untyped legacy marker
///     — unknown is not permission) → only when a FRESH APPROVED verdict is
///     recorded at the PR's current head (criterion 4, decided by
///     `release_of`, i.e. [`crate::merge_hold::refusal_release`]). A person
///     running a merge is not the reviewer's answer; a new verdict at the
///     head is. Otherwise refuse, naming the verdict on record and its sha —
///     or its missing sha (criterion 11);
///   - a human, on a RECUSAL hold or a malformed marker → no: those are
///     released by an independent reader / a deliberate
///     `aida merge-hold clear <PR>` (recorded).
///
/// `release_of` is called only for a refusal hold with a human present, so
/// the forge head read it needs is never made otherwise.
/// `AIDA_PR_SHIP_ALLOW_IN_DRIVE` does not reach this gate. `None` = proceed.
// trace:BUG-1532 | ai:claude
pub(crate) fn ship_hold_gate(
    hold: Option<&crate::gate::merge_hold_core::MergeHoldRecord>,
    integrity_floor_authority: bool,
    pr: u64,
    release_of: impl FnOnce(
        &crate::gate::merge_hold_core::MergeHoldRecord,
    ) -> crate::gate::merge_hold_core::RefusalRelease,
) -> Option<String> {
    use crate::gate::merge_hold_core::{HoldReasonKind, RefusalRelease};
    let hold = hold?;
    if !integrity_floor_authority {
        return ship_hold_release_refusal(true, false, pr);
    }
    let releasable = !hold.legacy
        && matches!(
            hold.reason_kind,
            HoldReasonKind::Supervision | HoldReasonKind::Decision
        );
    if releasable {
        return None;
    }
    let kind = if hold.legacy {
        "an untyped legacy hold (read as a refusal)".to_string()
    } else {
        format!("a {} hold", hold.reason_kind.as_str())
    };
    if hold.is_refusal() {
        return match release_of(hold) {
            RefusalRelease::Released(_) => None,
            RefusalRelease::Held(why) => Some(format!(
                "PR-{pr} is under {kind}. A reviewer refusal is released only by a fresh APPROVED \
                 verdict recorded at the PR's current head, and {why}. (Marker summary from \
                 placement, not current: {}.) Once that verdict is recorded, re-run \
                 `aida pr ship {pr}`. A human may instead override deliberately with \
                 `aida merge-hold clear {pr}` (recorded).",
                hold.detail.trim()
            )),
        };
    }
    Some(format!(
        "PR-{pr} is under {kind}: {}. `aida pr ship` releases only a supervision or decision \
         hold, or a refusal answered by a fresh approving verdict at the current head — merging \
         is not the decision this one waits for. Once its release condition is met, a human \
         clears it with `aida merge-hold clear {pr}` (recorded), then re-runs `aida pr ship {pr}`.",
        hold.detail.trim()
    ))
}

/// BUG-1532: the merge pin when a refusal hold is released by a fresh
/// verdict. The verdict covers exactly `head_sha`, so the merge is pinned
/// there (overriding the TASK-1448 pin, which is None when the verdict's key
/// is not among its candidates); with no readable head the release is
/// refused. Not verdict-released → the TASK-1448 pin stands unchanged.
// trace:BUG-1532 | ai:claude
pub(crate) fn refusal_release_pin(
    released_by_verdict: bool,
    match_head: Option<String>,
    head_sha: Option<&str>,
    pr: u64,
) -> Result<Option<String>, String> {
    if !released_by_verdict {
        return Ok(match_head);
    }
    match head_sha.map(str::trim).filter(|h| !h.is_empty()) {
        Some(head) => Ok(Some(head.to_string())),
        None => Err(format!(
            "PR-{pr}: the reviewer-refusal hold's release rests on a verdict at the PR head, but \
             the head could not be read to pin the merge to it; the hold stays. Re-run \
             `aida pr ship {pr}`."
        )),
    }
}

/// BUG-710/BUG-716/TASK-1253: a drive seat may not merge any PR, and no caller
/// may merge the live drive's own PR. Merely observing an unrelated live drain
/// is not grounds to block: those merges serialize on the merge lease.
// trace:BUG-710 trace:BUG-716 trace:TASK-1253 | ai:codex
pub fn ship_merge_block_reason(
    caller_is_drive_seat: bool,
    pr_is_drive_owned: bool,
    override_allow: bool,
) -> Option<ShipMergeBlockReason> {
    if override_allow {
        None
    } else if caller_is_drive_seat {
        Some(ShipMergeBlockReason::DriveSeat)
    } else if pr_is_drive_owned {
        Some(ShipMergeBlockReason::DriveOwnedPr)
    } else {
        None
    }
}

/// STORY-529: a spec carrying this tag must NOT be auto-merged by `aida pr
/// ship` — the PR is left a draft for a human to review + merge. Enforces the
/// draft-for-review handoff that briefs alone couldn't (handed-off agents kept
/// self-merging draft-for-review work). trace:STORY-529 | ai:claude
pub const DRAFT_ONLY_TAG: &str = "review:draft-only";

/// True iff any of `tags` marks the spec draft-only (case-insensitive match on
/// [`DRAFT_ONLY_TAG`]). The pure heart of the ship-time draft gate.
/// trace:STORY-529 | ai:claude
pub fn is_draft_only_tagged(tags: &[String]) -> bool {
    tags.iter()
        .any(|t| t.trim().eq_ignore_ascii_case(DRAFT_ONLY_TAG))
}

/// The supervised-merge gate's pure heart. The keystone "Gate + PR, do NOT
/// merge — advisor reviews first" directive used to live only in spec PROSE,
/// which the ship cadence never read — it auto-merged keystone work straight
/// past it. The machine-readable marker is the spec's `execution_mode` (the
/// advisor's bless-time routing, STORY-776): only `drain` licenses an
/// unattended merge. Every other mode — and, fail safe, NO mode at all —
/// holds the merge for an explicit human/advisor.
// trace:BUG-727 | ai:claude
pub fn merge_requires_supervision(mode: Option<aida_core::ExecutionMode>) -> bool {
    mode != Some(aida_core::ExecutionMode::Drain)
}

/// The mode label used in the supervised-merge refusal message — the mode's
/// name, or `unset (supervised)` for a spec that never had one (an unset mode
/// is supervised, fail safe).
// trace:BUG-727 | ai:claude
pub fn supervision_mode_label(mode: Option<aida_core::ExecutionMode>) -> String {
    match mode {
        Some(m) => m.to_string(),
        None => "unset (supervised)".to_string(),
    }
}

/// The `aida pr ship` supervised-merge decision: which of the specs behind the
/// commits about to ship hold the auto-merge, as `(spec-id, mode-label)`
/// pairs. Empty ⇒ the merge may proceed. An interactive human at a TTY IS the
/// explicit human/advisor merge the marker asks for, so the gate never fires
/// there — it fires for non-interactive (automation) callers whose specs carry
/// any `execution_mode` but `drain` (or none at all, the fail-safe).
// trace:BUG-727 | ai:claude
pub fn supervised_merge_holds(
    specs: &[(String, Option<aida_core::ExecutionMode>)],
    interactive_tty: bool,
) -> Vec<(String, String)> {
    if interactive_tty {
        return Vec::new();
    }
    specs
        .iter()
        .filter(|(_, mode)| merge_requires_supervision(*mode))
        .map(|(id, mode)| (id.clone(), supervision_mode_label(*mode)))
        .collect()
}

/// Why a merge path refuses a PR whose newest recorded APPROVAL does not
/// provably cover the PR's current head (PRIN-5: an approval that cannot be
/// placed against the head is not evidence the head was reviewed).
///
/// Derived ONLY from `awaiting_you::classify_pr_review` (BUG-1549) — the
/// same classifier the awaiting surface uses — so the merge gate and the
/// "stale approval" row cannot disagree about which approvals are stale.
// trace:TASK-1448 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ApprovalHeadRefusal {
    /// The approval was recorded at a commit the head has since moved past.
    Moved {
        reviewed_sha: String,
        head_sha: String,
    },
    /// The approval records no `reviewed_sha`, so it cannot be tied to any
    /// commit. `head_sha` is empty when the head was unreadable too.
    NoReviewedSha {
        head_sha: String,
    },
    /// The PR's current head could not be read, so no approval can be shown
    /// to cover it.
    HeadUnreadable {
        reviewed_sha: String,
    },
    /// Both shas are present but too short to compare with confidence.
    Incomparable {
        reviewed_sha: String,
        head_sha: String,
    },
    // trace:BUG-1802 | ai:antigravity
    Unauthorized {
        recorded_by: String,
    },
}

/// TASK-1448: does the newest recorded APPROVAL among `candidates` cover
/// `head`? `None` = the merge may proceed on this gate: either the newest
/// approval was recorded at the head, or there is no approval at all (a PR
/// merged without delegated review is governed by the other ship gates, not
/// this one). `Some` = refuse.
///
/// Only approvals are classified: a refusal is a different question with a
/// different gate (TASK-1169 / the orchestrator's own reviewer phase), and
/// letting a refusal shadow the approval here would let a stale approval
/// merge whenever a refusal also exists.
// trace:TASK-1448 | ai:claude
pub(crate) fn approval_head_refusal(
    candidates: &[crate::gate::review_verdict::RecordedVerdict],
    head: Option<&str>,
) -> Option<ApprovalHeadRefusal> {
    use crate::awaiting_you::{classify_pr_review, PrReviewRow};
    let approvals = open_approvals(candidates);
    let head_sha = head.map(str::trim).unwrap_or("").to_string();
    let decision = classify_pr_review(&approvals, head);

    if let Some(newest) = newest_approval(&approvals) {
        if newest
            .recorded_by
            .as_deref()
            .unwrap_or("")
            .contains("implementer seat")
        {
            return Some(ApprovalHeadRefusal::Unauthorized {
                recorded_by: newest.recorded_by.clone().unwrap_or_default(),
            });
        }
    }

    let row = decision.row?;
    let reviewed_sha = match &row {
        PrReviewRow::Stale { reviewed_sha }
        | PrReviewRow::Unverifiable { reviewed_sha }
        | PrReviewRow::Blocked { reviewed_sha, .. }
        | PrReviewRow::ReworkReady { reviewed_sha, .. } => reviewed_sha.trim().to_string(),
    };
    Some(match row {
        PrReviewRow::Stale { .. } => ApprovalHeadRefusal::Moved {
            reviewed_sha,
            head_sha,
        },
        // Every other row fails closed. Only `Unverifiable` is reachable from
        // an approvals-only candidate set; the rest are listed so a future
        // classifier change cannot silently turn into "merge".
        _ if reviewed_sha.is_empty() => ApprovalHeadRefusal::NoReviewedSha { head_sha },
        _ if head_sha.is_empty() => ApprovalHeadRefusal::HeadUnreadable { reviewed_sha },
        _ => ApprovalHeadRefusal::Incomparable {
            reviewed_sha,
            head_sha,
        },
    })
}

/// BUG-1918: the newest of `approvals` by PARSED `recorded_at` (RFC 3339,
/// any offset). A string comparison misorders offsets (`…T01:00:00-07:00` is
/// later than `…T05:00:00Z`); an unreadable timestamp sorts oldest.
// trace:BUG-1918 | ai:claude
pub(crate) fn newest_approval(
    approvals: &[crate::gate::review_verdict::RecordedVerdict],
) -> Option<&crate::gate::review_verdict::RecordedVerdict> {
    approvals.iter().max_by_key(|a| {
        a.recorded_at
            .as_deref()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t.trim()).ok())
            .map(|t| t.with_timezone(&chrono::Utc))
    })
}

/// TASK-1458: the approvals a merge gate weighs — every APPROVED verdict not
/// yet closed by a merge. A closed approval belongs to a PR that already
/// landed (BUG-1529 treats closed refusals the same way), so it is history,
/// not evidence about this PR's head.
// trace:TASK-1458 | ai:claude
fn open_approvals(
    candidates: &[crate::gate::review_verdict::RecordedVerdict],
) -> Vec<crate::gate::review_verdict::RecordedVerdict> {
    candidates
        .iter()
        .filter(|v| v.kind.approves() && !v.is_closed())
        .cloned()
        .collect()
}

/// TASK-1458: the commit a merge must be pinned to (`MergeOptions.match_head`)
/// once the approval gate has passed. `Some` only when an open approval
/// exists AND the newest one covers `head` — then the pin is that approved
/// commit, spelled as the longer of the reviewed and head shas (the forge
/// wants a full sha). `None` when there is no approval to pin to, when the
/// head is unknown, or when the gate would refuse.
// trace:TASK-1458 | ai:claude
pub(crate) fn approved_match_head(
    candidates: &[crate::gate::review_verdict::RecordedVerdict],
    head: Option<&str>,
) -> Option<String> {
    let head = head.map(str::trim).filter(|h| !h.is_empty())?;
    let approvals = open_approvals(candidates);
    if approvals.is_empty() || approval_head_refusal(candidates, Some(head)).is_some() {
        return None;
    }
    let reviewed = approvals
        .iter()
        .filter_map(|v| v.reviewed_sha.as_deref().map(str::trim))
        .filter(|sha| crate::forge::head_matches_pin(sha, head))
        .max_by_key(|sha| sha.len())
        .unwrap_or(head);
    Some(if reviewed.len() > head.len() {
        reviewed.to_string()
    } else {
        head.to_string()
    })
}

// ── BUG-1918: approval-authority gate ──────────────────────────────────────

/// BUG-1918: one session bound to authoring the specs a PR ships — an
/// authoring lease scoped to one of them or working on the PR's branch, live
/// or durably recorded when it was taken. Identity is a set of tokens (see
/// `review_authority`), never a label.
// trace:BUG-1918 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct AuthorSession {
    /// Human-readable origin, for diagnostics only.
    pub label: String,
    pub tokens: Vec<String>,
}

impl AuthorSession {
    /// Does `identity` (a recorder's tokens) name this session?
    // trace:BUG-1918 | ai:claude
    pub(crate) fn matches(&self, identity: &[String]) -> bool {
        crate::gate::review_authority::identities_overlap(&self.tokens, identity)
    }
}

/// BUG-1918: what `aida pr ship` requires of the approval it merges on, beyond
/// covering the head (TASK-1448).
// trace:BUG-1918 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ShipApprovalPolicy {
    /// Sessions bound to authoring the shipped specs.
    pub author_sessions: Vec<AuthorSession>,
    /// Authority keys (`RecorderAttestation::authority_key`) whose seat grant
    /// or human-review receipt re-validated against its durable store, each
    /// mapped to the recorder identity the author check uses (the receipt's
    /// copy for a human approval). An attestation whose key is absent is
    /// treated as unattested, whatever its stored booleans say.
    pub verified_authority: std::collections::BTreeMap<String, Vec<String>>,
    /// Committer date of the commit the newest approval REVIEWED (its
    /// `reviewed_sha`). An approval recorded before that commit existed
    /// cannot have reviewed it (a backdated `recorded_at`); `None`
    /// (unreadable) refuses. Deliberately not the head's date: an honest
    /// approval of an older commit must still reach the head-coverage gate
    /// and its `--override-stale-approval` (TASK-1458).
    pub reviewed_committed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Identity tokens of the session running the merge. Only a shipper that
    /// is anchored and is not an author may use `--override-stale-approval`
    /// ([`stale_override_refusal`]).
    pub shipper: Vec<String>,
    /// STORY-1655: solo-human self-approval is in force for this project at
    /// ship time — the global key re-read now, and never for the AIDA
    /// repository. Off by default.
    pub solo_human: bool,
    /// STORY-1655: authority keys of solo approvals whose receipt re-validated
    /// and itself records a solo approval (`verified_solo_keys`).
    pub verified_solo: std::collections::BTreeSet<String>,
}

/// BUG-1918: why the shipping session may NOT use `--override-stale-approval`
/// — `None` when it may. The override ships commits no approval covered, so
/// the session invoking it is the only check on them: it must name itself
/// (an identity anchor) and must not be one of the work's authors. An author
/// would otherwise ship its own unreviewed commits on top of an older
/// independent approval.
// trace:BUG-1918 | ai:claude
pub(crate) fn stale_override_refusal(policy: &ShipApprovalPolicy) -> Option<String> {
    if !crate::gate::review_authority::has_identity_anchor(&policy.shipper) {
        return Some(
            "this session has no identity AIDA can bind the override to (no agent session id \
             and no process ancestry)"
                .to_string(),
        );
    }
    if let Some(author) = policy
        .author_sessions
        .iter()
        .find(|a| a.matches(&policy.shipper))
    {
        return Some(format!(
            "this session authored the work ({}), so it cannot override a stale approval of \
             it — get a fresh approval of the head from an independent reviewer",
            author.label
        ));
    }
    None
}

/// BUG-1918: why `aida pr ship` refuses on approval AUTHORITY. Distinct from
/// [`ApprovalHeadRefusal`]: `--override-stale-approval` overrides only a
/// head-coverage refusal, never one of these.
// trace:BUG-1918 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ApprovalAuthorityRefusal {
    /// No open approval at all. There is no opt-out: every merge path needs
    /// an attested, non-author approval.
    NoApproval,
    /// The newest approval carries no bound, verifiable recorder identity —
    /// `reason` says which part failed.
    Unattested { recorded_by: String, reason: String },
    /// The newest approval was recorded by a session bound to authoring the
    /// shipped specs — a self-approval.
    AuthorSession { recorded_by: String },
}

/// Clock skew tolerated between a verdict's `recorded_at` and this host.
const RECORDED_AT_SKEW_SECS: i64 = 300;

/// BUG-1918: does the newest open approval among `candidates` carry the
/// authority of a non-author seat? `None` = proceed on this gate. Fails
/// closed: no approval refuses (no tag opts out), and an approval whose
/// recorder cannot be bound to an authorized, non-author, anchored identity
/// refuses whatever its `recorded_by` text says.
// trace:BUG-1918 | ai:claude
pub(crate) fn approval_authority_refusal(
    candidates: &[crate::gate::review_verdict::RecordedVerdict],
    policy: &ShipApprovalPolicy,
) -> Option<ApprovalAuthorityRefusal> {
    let approvals = open_approvals(candidates);
    let Some(newest) = newest_approval(&approvals) else {
        return Some(ApprovalAuthorityRefusal::NoApproval);
    };
    let recorded_by = newest.recorded_by.clone().unwrap_or_default();
    let unattested = |reason: &str| {
        Some(ApprovalAuthorityRefusal::Unattested {
            recorded_by: recorded_by.clone(),
            reason: reason.to_string(),
        })
    };
    let Some(attestation) = newest.attestation.as_ref() else {
        return unattested("it was not recorded through `aida review record`");
    };
    // STORY-1655: a verified solo-human self-approval is exempt from the
    // author check ONLY; every other authority check below still applies.
    // trace:STORY-1655 | ai:claude
    let solo = solo_self_approval(newest, policy);
    if !solo
        && policy
            .author_sessions
            .iter()
            .any(|s| s.matches(&attestation.identity))
    {
        return Some(ApprovalAuthorityRefusal::AuthorSession { recorded_by });
    }
    let Some(identity) = attestation
        .authority_key(newest.recorded_at.as_deref())
        .and_then(|k| policy.verified_authority.get(&k))
    else {
        return unattested("its seat grant or human-review receipt did not re-validate");
    };
    if !crate::gate::review_authority::has_identity_anchor(identity)
        || !crate::gate::review_authority::has_identity_anchor(&attestation.identity)
    {
        return unattested("it names no recorder identity anchored to a session or process");
    }
    if !solo && policy.author_sessions.iter().any(|s| s.matches(identity)) {
        return Some(ApprovalAuthorityRefusal::AuthorSession { recorded_by });
    }
    if !attestation.authorizes(newest.reviewed_sha.as_deref()) {
        return unattested("it was not made by a non-implementer seat or a human for this commit");
    }
    let Some(recorded_at) = newest
        .recorded_at
        .as_deref()
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t.trim()).ok())
        .map(|t| t.with_timezone(&chrono::Utc))
    else {
        return unattested("its recording time is missing or unreadable");
    };
    let Some(reviewed_at) = policy.reviewed_committed_at else {
        return unattested("the reviewed commit's date could not be read (fetch the PR head)");
    };
    if recorded_at < reviewed_at {
        return unattested("it is dated before the commit it claims to approve");
    }
    if recorded_at > chrono::Utc::now() + chrono::Duration::seconds(RECORDED_AT_SKEW_SECS) {
        return unattested("it is dated in the future");
    }
    None
}

/// STORY-1655: is `approval` a solo-human self-approval the ship gate may
/// honour? The mode must be in force now, the attestation must say solo and
/// human-at-TTY, and its receipt must have re-validated as a solo receipt.
// trace:STORY-1655 | ai:claude
pub(crate) fn solo_self_approval(
    approval: &crate::gate::review_verdict::RecordedVerdict,
    policy: &ShipApprovalPolicy,
) -> bool {
    let Some(att) = approval.attestation.as_ref() else {
        return false;
    };
    policy.solo_human
        && att.solo_human
        && att.human_at_tty
        && att
            .authority_key(approval.recorded_at.as_deref())
            .is_some_and(|k| k.starts_with("receipt:") && policy.verified_solo.contains(&k))
}

/// STORY-1655: the line `aida pr ship` prints when the approval it merges on
/// is a solo-human self-approval; `None` otherwise.
// trace:STORY-1655 | ai:claude
pub(crate) fn solo_approval_notice(
    candidates: &[crate::gate::review_verdict::RecordedVerdict],
    policy: &ShipApprovalPolicy,
) -> Option<String> {
    let approvals = open_approvals(candidates);
    let newest = newest_approval(&approvals)?;
    solo_self_approval(newest, policy).then(|| {
        format!(
            "approval: solo-human self-approval by `{}` (review.solo_human_self_approval is on)",
            newest
                .recorded_by
                .as_deref()
                .unwrap_or("a human at a terminal")
        )
    })
}

/// BUG-1918: the refusal text for an [`ApprovalAuthorityRefusal`].
// trace:BUG-1918 | ai:claude
pub(crate) fn approval_authority_refusal_message(
    pr: u64,
    refusal: &ApprovalAuthorityRefusal,
) -> String {
    let who = |r: &str| {
        if r.trim().is_empty() {
            "an unknown recorder".to_string()
        } else {
            format!("`{r}`")
        }
    };
    match refusal {
        ApprovalAuthorityRefusal::NoApproval => format!(
            "PR-{pr} has no approval recorded at its head. An independent reviewer or a human \
             at a terminal must record one (`aida review record <SPEC> --pr {pr} --verdict \
             approved`) from a session other than the one that implemented it."
        ),
        ApprovalAuthorityRefusal::Unattested {
            recorded_by,
            reason,
        } => format!(
            "PR-{pr}'s approval (recorded by {}) carries no verified reviewer identity for this \
             commit: {reason}. Re-record it with `aida review record` from a reviewer seat, or as \
             a human at a terminal, from a session other than the one that implemented it.",
            who(recorded_by)
        ),
        ApprovalAuthorityRefusal::AuthorSession { recorded_by } => format!(
            "PR-{pr}'s approval (recorded by {}) came from the session that implemented it — an \
             implementer cannot approve its own work. An independent reviewer must record the \
             approval.",
            who(recorded_by)
        ),
    }
}

/// TASK-1458: the reviewed and head shas a refusal names, for the durable
/// `--override-stale-approval` record. Empty string = that sha was missing.
// trace:TASK-1458 | ai:claude
pub(crate) fn approval_head_refusal_shas(refusal: &ApprovalHeadRefusal) -> (String, String) {
    match refusal {
        ApprovalHeadRefusal::Moved {
            reviewed_sha,
            head_sha,
        }
        | ApprovalHeadRefusal::Incomparable {
            reviewed_sha,
            head_sha,
        } => (reviewed_sha.clone(), head_sha.clone()),
        ApprovalHeadRefusal::NoReviewedSha { head_sha } => (String::new(), head_sha.clone()),
        ApprovalHeadRefusal::HeadUnreadable { reviewed_sha } => {
            (reviewed_sha.clone(), String::new())
        }
        ApprovalHeadRefusal::Unauthorized { .. } => (String::new(), String::new()),
    }
}

/// TASK-1458: the `.aida/advisor-activity.jsonl` line that durably records an
/// `aida pr ship --override-stale-approval` — which PR, the approval's
/// reviewed sha, the head it was overridden onto, and why the gate refused.
/// Written BEFORE the merge, so the override is on record even when the
/// merge then fails.
// trace:TASK-1458 | ai:claude
pub(crate) fn format_stale_approval_override_event(
    now_iso: &str,
    pr_number: u64,
    refusal: &ApprovalHeadRefusal,
) -> String {
    let (reviewed_sha, head_sha) = approval_head_refusal_shas(refusal);
    serde_json::json!({
        "ts": now_iso,
        "command": "aida pr ship",
        "step": "pr-merge-override-stale-approval",
        "status": "overridden",
        "pr": pr_number,
        "reviewed_sha": reviewed_sha,
        "head_sha": head_sha,
        "detail": approval_head_refusal_message(pr_number, refusal),
    })
    .to_string()
}

/// TASK-1448: the refusal text both merge paths print. Names both shas (or
/// says which is missing) and says what to do: re-review the current head.
// trace:TASK-1448 | ai:claude
pub(crate) fn approval_head_refusal_message(pr: u64, refusal: &ApprovalHeadRefusal) -> String {
    let short = |s: &str| crate::gate::review_verdict::short_sha(s).to_string();
    let why = match refusal {
        ApprovalHeadRefusal::Moved {
            reviewed_sha,
            head_sha,
        } => format!(
            "its approval was recorded at {} but the PR head is now {} — the new commits were never reviewed",
            short(reviewed_sha),
            short(head_sha)
        ),
        ApprovalHeadRefusal::NoReviewedSha { head_sha } => format!(
            "its approval records no reviewed commit, so it cannot be shown to cover the PR head ({})",
            if head_sha.is_empty() {
                "unreadable".to_string()
            } else {
                short(head_sha)
            }
        ),
        ApprovalHeadRefusal::HeadUnreadable { reviewed_sha } => format!(
            "its approval was recorded at {} but the PR's current head could not be read, so the \
             approval cannot be shown to cover it",
            short(reviewed_sha)
        ),
        ApprovalHeadRefusal::Incomparable {
            reviewed_sha,
            head_sha,
        } => format!(
            "its approval sha {} cannot be compared with the PR head {} (too short to tell)",
            reviewed_sha, head_sha
        ),
        ApprovalHeadRefusal::Unauthorized { recorded_by } => format!(
            "its approval was recorded by an unauthorized seat: {recorded_by} (self-review is prohibited)"
        ),
    };
    format!(
        "PR-{pr} was not merged: {why}. Re-review the current head — e.g. \
         `aida queue work PR-{pr} --role reviewer`, or `aida review record <SPEC> --verdict approved \
         --sha <head>` after reviewing it — then merge again."
    )
}

/// TASK-1448: every recorded verdict a merge gate must weigh for `pr` —
/// the PR-keyed record plus the spec-keyed record for each id in `spec_ids`,
/// read under each of `roots` (the calling worktree and the main clone can
/// both hold `.aida/review-verdicts/`). A file seen under two roots is read
/// once. Unreadable/absent files contribute nothing.
///
/// TASK-1460: when the PR `head` is known, a key whose current file was
/// recorded at some OTHER commit also contributes its archived verdict for
/// `head` (BUG-1539's per-sha archive), so a later round at a different sha
/// cannot hide the review of the commit about to merge. The current file is
/// still read first and always counts — the archive only adds evidence.
// trace:TASK-1448 | ai:claude
// trace:TASK-1460 | ai:claude
pub(crate) fn merge_gate_verdict_candidates(
    roots: &[&std::path::Path],
    pr: u64,
    spec_ids: &[String],
    head: Option<&str>,
) -> Vec<crate::gate::review_verdict::RecordedVerdict> {
    let head = head.map(str::trim).filter(|h| !h.is_empty());
    let mut keys: Vec<String> = vec![format!("PR-{pr}")];
    for id in spec_ids {
        let id = id.trim().to_ascii_uppercase();
        if !id.is_empty() && !keys.contains(&id) {
            keys.push(id);
        }
    }
    let mut seen: Vec<std::path::PathBuf> = Vec::new();
    let mut out = Vec::new();
    for root in roots {
        for key in &keys {
            let path = crate::gate::review_verdict::verdict_path(root, key);
            let canon = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            if seen.contains(&canon) {
                continue;
            }
            seen.push(canon);
            // TASK-1801: a current artefact that exists but the anchored
            // reader refuses (linked file or directory) is not evidence, and
            // it must not let an archived round answer for the key either.
            // trace:TASK-1801 | ai:claude
            let body = crate::gate::review_verdict::read_verdict_body(&path);
            if body.is_err() && std::fs::symlink_metadata(&path).is_ok() {
                continue;
            }
            let current = body
                .ok()
                .and_then(|body| crate::gate::review_verdict::parse_recorded_verdict(&body));
            let current_at_head = match (&current, head) {
                (Some(v), Some(h)) => v
                    .reviewed_sha
                    .as_deref()
                    .is_some_and(|r| crate::gate::review_verdict::same_reviewed_sha(r, h)),
                _ => false,
            };
            if let Some(v) = current {
                out.push(v);
            }
            if let (false, Some(h)) = (current_at_head, head) {
                if let Some(archived) =
                    crate::gate::review_verdict::read_verdict_for_sha(root, key, h)
                {
                    out.push(archived);
                }
            }
        }
    }
    out
}

/// BUG-2164: An open PR with an APPROVED review decision.
// trace:BUG-2164 | ai:antigravity
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenApprovedPr {
    pub(crate) number: u64,
    pub(crate) title: String,
}

/// BUG-2164: Parse open PRs with an APPROVED review decision from `gh pr list --json ...` stdout.
// trace:BUG-2164 | ai:antigravity
pub(crate) fn parse_open_approved_prs_json(json: &str) -> Vec<OpenApprovedPr> {
    let Ok(val) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let Some(arr) = val.as_array() else {
        return Vec::new();
    };
    arr.iter()
        .filter(|item| {
            item.get("reviewDecision")
                .and_then(|v| v.as_str())
                .map(|s| s.eq_ignore_ascii_case("APPROVED"))
                .unwrap_or(false)
        })
        .filter_map(|item| {
            let number = item.get("number").and_then(|v| v.as_u64())?;
            let title = item
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            Some(OpenApprovedPr { number, title })
        })
        .collect()
}

// trace:TASK-1795 trace:TASK-1803 | ai:antigravity
fn append_open_approved_prs(base: &str, approved: &[OpenApprovedPr]) -> String {
    if approved.is_empty() {
        base.to_string()
    } else {
        let mut msg = format!("{base}\n\nOpen approved PRs:");
        // trace:TASK-1795 | ai:codex
        for pr in approved {
            if pr.title.is_empty() {
                msg.push_str(&format!("\n  aida pr ship {}", pr.number));
            } else {
                msg.push_str(&format!("\n  aida pr ship {} — {}", pr.number, pr.title));
            }
        }
        msg
    }
}

/// BUG-2164: Format the refusal message when `aida pr ship` is invoked on the default branch without a PR number.
// trace:BUG-2164 trace:TASK-1795 | ai:antigravity
pub(crate) fn format_default_branch_ship_refusal(approved: &[OpenApprovedPr]) -> String {
    let base = "aida pr ship: on the default branch; pass a PR number (e.g. aida pr ship 8)";
    append_open_approved_prs(base, approved)
}

/// TASK-1803: Format the refusal message when `aida pr ship` is invoked without a PR number
/// and the default branch could not be determined.
// trace:TASK-1803 | ai:antigravity
pub(crate) fn format_unknown_default_branch_ship_refusal(approved: &[OpenApprovedPr]) -> String {
    let base = "aida pr ship: the default branch could not be determined; run `git remote set-head origin --auto` or pass a PR number (e.g. aida pr ship 8)";
    append_open_approved_prs(base, approved)
}

#[cfg(test)]
#[path = "tests/pr_ship_gate_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/bug_1918_approval_authority_tests.rs"]
mod bug_1918_approval_authority_tests;
