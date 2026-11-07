//! `aida status --awaiting` — the top-priority "Awaiting you" section that
//! surfaces every item where the operator is the gate. Aggregates open PRs
//! that are mergeable with no pending CI / requested-changes, unacked
//! agent briefs, findings awaiting triage, queued reviewer-verdict items,
//! and NeedsAttention escalations into one scannable list. Hidden on
//! quiet days so the section appearing is itself the signal.
//!
//! The classifier is pure: callers gather facts (gh PR rollup, brief
//! directory walk, findings view, queue snapshot, requirement summaries)
//! and hand them in as inputs. This module owns the shape + the renderer
//! so the layout invariants stay unit-testable.
//!
//! trace:STORY-465 | ai:claude

use crate::gate::review_verdict;
use crate::status_cleanup::OpenPrItem;
use colored::Colorize;
use std::collections::HashSet;
use std::io::Write;

/// Default cap on rendered lines before `--verbose` lifts it.
pub(crate) const DEFAULT_CAP: usize = 5;

/// One snapshot of every actionable category. An empty report renders
/// nothing — the absence of the section IS the "nothing awaits you"
/// signal, so a quiet `aida status` stays quiet.
#[derive(Debug, Default, Clone)]
pub(crate) struct AwaitingReport {
    // trace:STORY-2336 | ai:codex
    pub idle_eligible: Vec<crate::idle_eligible::IdleEligibleItem>,
    /// Open changes `aida review watch` is skipping without reviewing them,
    /// each with the watcher's own reason: a stale overlapping head, a
    /// refused branch update routed here, an exited review, the round cap.
    /// A local state-file read, so it rides the per-turn notice too.
    // trace:BUG-2426 | ai:claude
    pub review_watch_skipped: Vec<WatchSkippedItem>,
    /// Open PRs that pass the awaiting-you gate: `MERGEABLE` + no failing
    /// or pending CI + reviewer verdict is not `CHANGES_REQUESTED`. The
    /// aida-chat motivating case: 5 PRs sat open for hours because the
    /// system was waiting on the human's merge button and nothing said so.
    pub mergeable_prs: Vec<MergeablePrItem>,
    /// Typed recusal holds, projected for the current principal. They never
    /// masquerade as ordinary ready-to-merge rows.
    // trace:STORY-1397 | ai:codex
    pub recusal_holds: Vec<RecusalHoldItem>,
    /// Open PRs under a non-recusal merge hold (supervision / rework /
    /// decision / unreadable marker). A held PR is never a ready-to-merge row,
    /// but it must never vanish either: each one is listed with its typed
    /// reason and whose action it is. Full report only (needs the PR snapshot).
    // trace:STORY-1397 | ai:claude
    pub held_prs: Vec<HeldPrItem>,
    /// Open PRs whose CI is failing and which have no verdict, merge hold,
    /// reviewer route, or live branch owner. These are repair work, not review
    /// work, so the green-only orphan sweep deliberately does not claim them.
    // trace:TASK-192 | ai:codex
    pub unowned_failing_prs: Vec<UnownedFailingPrItem>,
    /// Unacked briefs filed for the running agent (or every agent when
    /// the caller can't narrow). Each one is a hand-off the operator
    /// hasn't picked up yet.
    pub pending_briefs: Vec<PendingBriefItem>,
    /// Total findings awaiting triage (`aida findings list` count). The
    /// renderer collapses this to a single line because the triage view
    /// is the real surface — we just want one breadcrumb at the top.
    pub findings_total: usize,
    /// Queue items routed to the current role where the role is a
    /// reviewer-class seat (the human-verdict gate). Empty when the
    /// caller is not acting as a reviewer.
    pub reviewer_queue_items: Vec<ReviewerQueueItem>,
    /// Specs parked in `NeedsAttention` — the implementer-→-advisor-→-
    /// human escalation cascade landed here.
    pub escalations: Vec<EscalationItem>,
    /// Specs parked mechanically with a structured FailureReason. These still
    /// need rework/retry, but they are not human-decision escalations.
    // trace:STORY-1023 | ai:codex
    pub shelved_total: usize,
    /// Unread inter-agent mail for the OPERATOR's own handle, plus a separate
    /// count for the shared role / agent-type inboxes (BUG-767). Folded in from
    /// the mailbox core so the coordination inbox is ONE surface, not split
    /// between mail (per-turn hook) and everything-else (`aida status`). Cheap:
    /// derived from the local + canonical mailbox files, never a network call,
    /// so it can ride the per-turn notice. Zero when the inbox is caught up.
    // trace:STORY-741 | ai:claude
    // trace:BUG-767 | ai:claude
    pub mail: MailChannel,
    /// Pending worker directives from the local directive file. The enqueue
    /// path (e.g. a human-audit request) lands here, and today it only
    /// surfaces via the worker poll view — folding it in makes the unified
    /// inbox the one place the advisor has to look. Cheap: a single local
    /// file read, never a network call, so it rides the per-turn notice.
    /// Renders as ONE collapsed line (like findings) because the worker
    /// directives view is the real surface — this is the breadcrumb.
    // trace:TASK-1146 | ai:claude
    pub worker_directives: DirectivesChannel,
    /// Due SEAT jobs from the `[schedule]` registry (STORY-1226) for the
    /// session's seat (every seat when none is known). The scheduler never
    /// runs a seat job — this line IS its delivery on Codex/Antigravity, and
    /// the nudge on Claude. Cheap: config parse + ledger/state file reads,
    /// never git or the network, so it rides the per-turn notice. Substrate
    /// jobs never appear here (the tick runs them). Renders as ONE collapsed
    /// line with the head job; `aida schedule due` is the real surface.
    // trace:STORY-1226 | ai:claude
    pub cron: CronChannel,
    /// Spec-linked branches ahead of main, with no open PR and no live lease.
    /// These are recoverable pushed/local commits that can otherwise disappear
    /// from the operator's field of view after a drain dies before PR creation.
    // trace:STORY-1043 | ai:codex
    pub unshipped_work: Vec<UnshippedWorkItem>,
    /// BUG-1288: whether the `unshipped_work` scan above ran to completion or
    /// was time-boxed. See [`UnshippedScanStatus`].
    pub unshipped_work_scan: Option<UnshippedScanStatus>,
    /// Latest scheduled cross-platform run is red. Full report only; the
    /// per-turn notice path skips the network-backed workflow probe.
    // trace:STORY-1043 | ai:codex
    pub nightly_red: Option<NightlyRedItem>,
    /// STORY-1419: PRs whose rework has landed on a refusal this seat recorded
    /// — `head != reviewed_sha` on a blocking verdict. Three times in one night
    /// a reviewer found this by sweeping heads by hand or was told by another
    /// agent; the signal existed and no surface reported it. Full report only:
    /// it needs the PR snapshot, which the per-turn notice path skips.
    // trace:STORY-1419 | ai:claude
    pub rework_ready: Vec<ReworkReadyItem>,
    /// BUG-1549: PRs whose recorded APPROVAL no longer covers the current
    /// head — the merge-safety direction `rework_ready` cannot see, because
    /// that row filters to blocking verdicts first. A stale approval can be
    /// MERGED (unlike a stale refusal, which only wastes a round), so this
    /// is rendered ahead of `rework_ready` despite arriving second in code.
    /// Full report only: same PR-snapshot dependency as `rework_ready`.
    // trace:BUG-1549 | ai:claude
    pub stale_approvals: Vec<StaleApprovalItem>,
    /// BUG-1549: PRs a LIVE refusal (RequestChanges/Rejected at the head, or
    /// unverifiable against it, and not superseded by a later approval at the
    /// head) keeps out of the mergeable set. Every PR `classify_pr_review`
    /// suppresses has exactly one row here or in `stale_approvals`.
    // trace:BUG-1549 | ai:claude
    pub blocked_reviews: Vec<BlockedReviewItem>,
    /// TASK-1445 (containment for BUG-1510 AC5): a live drain's lease-based PR
    /// attribution disagrees with what the PR's own commits credit. Drain
    /// status attributes a PR by the lease it ran under; commit trailers are
    /// independent evidence of what the PR is actually about. The incident:
    /// STORY-1391's drain opened a PR trailered BUG-1420 and the split sat
    /// unreported for 52 seconds before a verdict landed on the wrong spec.
    /// Full report only — resolving the lease's branch needs a local git log,
    /// which the per-turn notice path skips for latency, same as
    /// `unshipped_work`.
    // trace:TASK-1445 | ai:claude
    pub pr_attribution_disagreements: Vec<PrAttributionDisagreementItem>,
    /// BUG-1564: In-Progress specs with no live session, lease or process
    /// backing the flag — the anomaly `aida ps` already detects (the
    /// flag-only column + the orphan pass) but that, until now, only a
    /// human who read that column and knew what it meant could see. Reuses
    /// `gather_running_work`'s orphan verdict verbatim (no second
    /// implementation); a spec a live fan-out is plausibly building
    /// (`likely_fanout`) is excluded here exactly as it is on `aida ps` —
    /// informational, not a genuine anomaly. Full-report only: it walks
    /// leases + a live-process probe via `gather_running_work`, the same
    /// "needs a heavier local probe" tier as `unshipped_work` /
    /// `pr_attribution_disagreements`, so the per-turn `--notice` path
    /// (which must stay local and fast, no full-store load) leaves it empty.
    // trace:BUG-1564 | ai:claude
    pub orphaned_in_progress: Vec<OrphanedInProgressItem>,
    /// STORY-1680: In Progress specs whose derived driver is `idle` and whose
    /// idle age (newest branch commit, else last status change) exceeds
    /// `[awaiting] idle_in_progress_hours` — "in progress, but nothing is
    /// working it". A spec listed here is dropped from `orphaned_in_progress`
    /// so it is reported once. Full report only, like the orphan channel.
    // trace:STORY-1680 | ai:claude
    pub idle_in_progress: Vec<crate::idle_in_progress::IdleInProgressItem>,
    /// TASK-1454: LIVE seats confirmed blocked on an unanswered permission
    /// prompt (BUG-1553's "the blocked state is the only one a human can
    /// fix"). Sourced from the `Notification(permission_prompt)` hook's
    /// on-disk marker, not a heuristic — so unlike most of the other
    /// heavier-probe channels above, this stays CHEAP on the notice-fast
    /// path: the marker directory is empty on the overwhelming majority of
    /// turns, and only the rare non-empty case pays the lease-resolution
    /// cost (`aida ps`'s own `gather_running_work`, reused verbatim so
    /// there is exactly one Blocked classifier).
    // trace:TASK-1454 | ai:claude
    pub blocked_seats: Vec<BlockedSeatItem>,
    /// Failed `aida deputy` jobs the requester has not acknowledged
    /// (`aida deputy ack`). Scoped to the reading session's own jobs when it
    /// has a session id; every one in the operator's shell. File reads only,
    /// so it rides the per-turn notice.
    // trace:STORY-B6B1278890774EF993D2EBC327862470-155 | ai:claude
    pub deputy_failed: Vec<crate::deputy::FailedJob>,
    /// BUG-1530: the reading seat's session role (`AIDA_SESSION_ROLE` / the
    /// role file), as the caller already resolves it for every other
    /// role-scoped surface. Drives which channels the headline (`render`)
    /// and the per-turn `compact_line` lead with — a reviewer's headline
    /// must not lead with advisor-owned findings or implementer-owned
    /// rework, and vice versa. `None` (no role known) keeps every channel
    /// unscoped, exactly like before this field existed.
    // trace:BUG-1530 | ai:claude
    pub role: Option<String>,
}

/// BUG-1530: coarse seat classification for headline scoping. Only three
/// channels have a single, specific-seat owner today (findings triage is
/// advisor authority; a shelved/rework item is implementer work; a
/// reviewer-queue row is routed to the reviewer seat specifically) — every
/// other channel (a PR to merge, a brief filed for you, mail, an
/// escalation, …) is "your own gate" regardless of which seat you sit in,
/// so it stays universal and is never hidden by this classification.
// trace:BUG-1530 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AwaitingSeat {
    Reviewer,
    Advisor,
    Implementer,
    Other,
}

/// Resolve the reading seat from a raw role string, normalizing aliases
/// (`dialog` -> `advisor`) the same way every other role-scoped surface
/// does. `None` when no role is known at all.
// trace:BUG-1530 | ai:claude
fn classify_seat(role: Option<&str>) -> Option<AwaitingSeat> {
    let role = role?;
    let role = role.trim();
    if role.is_empty() {
        return None;
    }
    // Review fix: role names are matched case-insensitively (`Reviewer` must
    // not fall through to `Other` and hide the seat's own rows).
    let canonical = crate::canonical_role_name(&role.to_ascii_lowercase());
    Some(match canonical.as_str() {
        "reviewer" => AwaitingSeat::Reviewer,
        "advisor" => AwaitingSeat::Advisor,
        "implementer" => AwaitingSeat::Implementer,
        _ => AwaitingSeat::Other,
    })
}

/// True when a channel owned by `owner` should be shown to `seat` — either
/// because `seat` IS that owner, or because no seat is known at all (today's
/// unscoped behavior, preserved exactly when `AIDA_SESSION_ROLE` is unset).
// trace:BUG-1530 | ai:claude
fn owned_channel_visible(seat: Option<AwaitingSeat>, owner: AwaitingSeat) -> bool {
    match seat {
        None => true,
        // Review fix: an unrecognised seat (integrator, product, human, …) is
        // not scoped at all — it sees every channel, never nothing (PRIN-5:
        // an unknown seat must not hide actionable work). trace:BUG-1530
        Some(AwaitingSeat::Other) => true,
        Some(s) => s == owner,
    }
}

#[cfg(test)]
mod bug_1530_review_fix_tests {
    include!("tests/awaiting_you_bug_1530_review_fix_tests.rs");
}

/// BUG-1564: one In-Progress spec with no live session/lease/process behind
/// it. `abandoned = true` means a spec-scoped lease existed but its holder
/// process is dead (work started, then the session died — ABANDONED);
/// `abandoned = false` means no spec-scoped lease ever existed for this spec
/// (nothing has picked it up — NOT-YET-STARTED). `since_label` is a
/// best-effort "how long" signal derived from the spec's last-modified
/// timestamp (a proxy, not a confirmed transition time — the exact
/// InProgress-since moment lives in the orphan-branch git log via `aida
/// history events`, which this fast surface does not walk) and says so
/// plainly when it can't be resolved at all.
// trace:BUG-1564 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct OrphanedInProgressItem {
    pub spec_id: String,
    pub title: String,
    pub abandoned: bool,
    pub since_label: String,
    /// BUG-1656: the lease's recorded pid is dead but a live harness lease is
    /// in this repo — an Agent-tool subagent may be working the spec.
    // trace:BUG-1656 | ai:claude
    #[serde(default)]
    pub possibly_subagent: bool,
}

/// TASK-1454: one LIVE seat confirmed blocked on a human approval gate — the
/// `aida awaiting` projection of a `Blocked`-activity `aida ps` row. `spec`
/// is the lease's scope when it resolves to one (`None` for a generic
/// harness-worktree fan-out lease, mirroring `PsRow.spec`'s own honesty
/// about scope-unknown rows); `session_id` is always present as the
/// fallback identifier.
// trace:TASK-1454 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct BlockedSeatItem {
    pub session_id: String,
    pub spec: Option<String>,
    pub tool: Option<String>,
    pub since_label: String,
}

/// BUG-1549: how one recorded verdict relates to a PR's current head.
///
/// `Unverifiable` covers every "cannot tell" shape — no `reviewed_sha`, a sha
/// too short to compare (`ShaRelation::Incomparable`), or a PR with no known
/// head sha. Only `Same` and `Moved` are positive evidence.
// trace:BUG-1549 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReviewRelation {
    Same,
    Moved,
    Unverifiable,
}

/// Relation of a recorded `reviewed` sha to `head`, via [`compare_shas`].
// trace:BUG-1549 | ai:claude
pub(crate) fn review_relation(reviewed: Option<&str>, head: Option<&str>) -> ReviewRelation {
    fn clean(s: Option<&str>) -> Option<&str> {
        s.map(str::trim).filter(|s| !s.is_empty())
    }
    let (Some(reviewed), Some(head)) = (clean(reviewed), clean(head)) else {
        return ReviewRelation::Unverifiable;
    };
    match compare_shas(head, reviewed) {
        ShaRelation::Same => ReviewRelation::Same,
        ShaRelation::Moved => ReviewRelation::Moved,
        ShaRelation::Incomparable => ReviewRelation::Unverifiable,
    }
}

/// Why a live refusal blocks the PR.
// trace:BUG-1549 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BlockedReason {
    /// The refusal was recorded against the current head.
    AtHead,
    /// The refusal cannot be pinned to a commit (no sha, a sha too short to
    /// compare, or no known head), so a push cannot be shown to clear it.
    Unverifiable,
}

impl BlockedReason {
    /// The wire label shared by `--json` and the agent-mode TOON table.
    // trace:BUG-1739 | ai:claude
    pub(crate) fn as_wire_str(self) -> &'static str {
        match self {
            Self::AtHead => "at_head",
            Self::Unverifiable => "unverifiable",
        }
    }
}

/// The ONE row a PR's local review verdicts produce, if any.
// trace:BUG-1549 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PrReviewRow {
    /// A live refusal — the PR must not merge.
    Blocked {
        reason: BlockedReason,
        reviewed_sha: String,
        /// RFC-3339 timestamp the refusal was recorded at, when known —
        /// carried through so the age since the refusal (no rework since)
        /// can be reported. `None` when the verdict never recorded one.
        // trace:TASK-1310 | ai:claude
        recorded_at: Option<String>,
    },
    /// The newest approval was recorded against a sha the head moved past.
    Stale { reviewed_sha: String },
    /// The newest approval cannot be verified against the head.
    Unverifiable { reviewed_sha: String },
    /// Only refusals the head has moved past: rework landed, re-review.
    /// Does NOT suppress (STORY-1419's row).
    ReworkReady {
        reviewed_sha: String,
        recorded_by: Option<String>,
    },
}

impl PrReviewRow {
    /// True for the rows that explain a suppression. The invariant
    /// `classify_pr_review` guarantees: `suppressed` iff this is true.
    pub(crate) fn explains_suppression(&self) -> bool {
        !matches!(self, PrReviewRow::ReworkReady { .. })
    }
}

/// What a PR's local verdicts mean for the mergeable set and the report.
// trace:BUG-1549 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct PrReviewDecision {
    pub suppressed: bool,
    pub row: Option<PrReviewRow>,
}

fn recorded_instant(
    v: &crate::gate::review_verdict::RecordedVerdict,
) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    v.recorded_at
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s.trim()).ok())
}

/// TASK-1310: how long a `Blocked` review row has stood with no rework —
/// the duration `aida awaiting`/`aida status` report so a refusal can't sit
/// unworked indefinitely unnoticed. Past this many seconds since
/// `recorded_at`, the row is flagged as long-standing (`REFUSAL_OVERDUE_SECS`
/// below). Chosen as a plain, generous default (a week) rather than a config
/// knob — the smallest thing that meets the acceptance; widen to a
/// `.aida/config.toml` setting if a real project needs a different cadence.
// trace:TASK-1310 | ai:claude
const REFUSAL_OVERDUE_SECS: i64 = 7 * 24 * 3600;

/// One `Blocked` row's age since its verdict was recorded, against `now`.
/// `None` means the verdict carries no parseable `recorded_at` — reported
/// as "age unknown", never as "no rework" (PRIN-5: unknown is not the same
/// claim as zero).
// trace:TASK-1310 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BlockedAge {
    pub secs: i64,
    pub overdue: bool,
}

/// Parses `recorded_at` (RFC-3339) and measures its age against `now`. A
/// negative age (clock skew, or a stamp in the future) is clamped to zero
/// rather than reported as overdue or negative.
// trace:TASK-1310 | ai:claude
pub(crate) fn blocked_age(
    recorded_at: Option<&str>,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<BlockedAge> {
    let at = recorded_at
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())?;
    let secs = (now - at.with_timezone(&chrono::Utc)).num_seconds().max(0);
    Some(BlockedAge {
        secs,
        overdue: secs >= REFUSAL_OVERDUE_SECS,
    })
}

/// Renders a `Blocked` row's age suffix: `"refused 3d ago, no rework since"`,
/// `"refused 9d ago, no rework since — overdue"` past the threshold, or
/// `"age unknown"` when `recorded_at` could not be parsed. Reused by both
/// the text renderer and the JSON shape (via [`blocked_age`]) so the two
/// surfaces cannot disagree — the query that produced the number is exactly
/// this function plus the `recorded_at` field both surfaces also print.
// trace:TASK-1310 | ai:claude
pub(crate) fn blocked_age_label(
    recorded_at: Option<&str>,
    now: chrono::DateTime<chrono::Utc>,
) -> String {
    match blocked_age(recorded_at, now) {
        Some(age) if age.overdue => format!(
            "refused {}, no rework since — overdue",
            crate::last_drain::format_age(age.secs)
        ),
        Some(age) => format!(
            "refused {}, no rework since",
            crate::last_drain::format_age(age.secs)
        ),
        None => "age unknown".to_string(),
    }
}

/// Age suffix for an `Unverifiable` Blocked row: the refusal cannot be placed
/// against the head, so whether rework landed since is UNKNOWN — never claim
/// "no rework since" (PRIN-5), and never flag it overdue.
// trace:TASK-1310 | ai:claude
pub(crate) fn blocked_age_label_unverifiable(
    recorded_at: Option<&str>,
    now: chrono::DateTime<chrono::Utc>,
) -> String {
    match blocked_age(recorded_at, now) {
        Some(age) => format!(
            "refused {}, rework since then unknown",
            crate::last_drain::format_age(age.secs)
        ),
        None => "age unknown".to_string(),
    }
}

/// BUG-1549: the single PR review classifier. BOTH the mergeable suppression
/// set (`local_suppressed_prs`, lib.rs) and every review row in the report
/// (`PrReviewRows::add`) derive from this one pure function, so they cannot
/// disagree. `candidates` is the spec-keyed verdict for every spec id in the
/// PR title plus the PR-keyed verdict; `head` is the PR's head sha.
///
/// The rule (operator proxy decision, 2026-09-23):
/// - A refusal (RequestChanges/Rejected) is LIVE unless its relation is
///   `Moved`, or an approval with a strictly LATER `recorded_at` (both
///   timestamps known) has relation `Same`. Unknown recency keeps it live.
/// - Any live refusal: suppressed, `Blocked` row (`AtHead` when that
///   refusal's relation is `Same`, else `Unverifiable`). With several, the
///   newest live refusal (undated counts as oldest) names the reason.
/// - Otherwise the newest approval (undated counts as oldest; a tie is broken
///   fail-closed, toward a non-`Same` relation) decides: `Same` = mergeable,
///   no row; `Moved` = suppressed, `Stale` row; `Unverifiable` = suppressed,
///   `Unverifiable` row.
/// - No approval and no live refusal: not suppressed; a `ReworkReady` row
///   when a `Moved` refusal exists (the newest one), else no row.
///
/// Invariant: `suppressed` iff `row` explains a suppression.
// trace:BUG-1549 | ai:claude
pub(crate) fn classify_pr_review(
    candidates: &[crate::gate::review_verdict::RecordedVerdict],
    head: Option<&str>,
) -> PrReviewDecision {
    let rel = |v: &crate::gate::review_verdict::RecordedVerdict| {
        review_relation(v.reviewed_sha.as_deref(), head)
    };
    let sha = |v: &crate::gate::review_verdict::RecordedVerdict| {
        v.reviewed_sha.as_deref().unwrap_or("").trim().to_string()
    };
    // BUG-1529: a refusal closed by its PR's merge is history, not a live
    // obstruction for a later PR on the same spec. trace:BUG-1529 | ai:claude
    let refusals: Vec<_> = candidates
        .iter()
        .filter(|v| v.kind.blocks_done() && !v.is_closed())
        .collect();
    let approvals: Vec<_> = candidates.iter().filter(|v| v.kind.approves()).collect();

    let superseded = |r: &crate::gate::review_verdict::RecordedVerdict| -> bool {
        let Some(rt) = recorded_instant(r) else {
            return false;
        };
        approvals.iter().any(|a| {
            rel(a) == ReviewRelation::Same && recorded_instant(a).is_some_and(|at| at > rt)
        })
    };
    let live = refusals
        .iter()
        .copied()
        .filter(|r| rel(r) != ReviewRelation::Moved && !superseded(r))
        .max_by_key(|r| recorded_instant(r));
    if let Some(r) = live {
        let reason = if rel(r) == ReviewRelation::Same {
            BlockedReason::AtHead
        } else {
            BlockedReason::Unverifiable
        };
        return PrReviewDecision {
            suppressed: true,
            row: Some(PrReviewRow::Blocked {
                reason,
                reviewed_sha: sha(r),
                recorded_at: r.recorded_at.clone(),
            }),
        };
    }

    let newest_approval = approvals
        .iter()
        .copied()
        .max_by_key(|a| (recorded_instant(a), rel(a) != ReviewRelation::Same));
    if let Some(a) = newest_approval {
        let row = match rel(a) {
            ReviewRelation::Same => None,
            ReviewRelation::Moved => Some(PrReviewRow::Stale {
                reviewed_sha: sha(a),
            }),
            ReviewRelation::Unverifiable => Some(PrReviewRow::Unverifiable {
                reviewed_sha: sha(a),
            }),
        };
        return PrReviewDecision {
            suppressed: row.as_ref().is_some_and(PrReviewRow::explains_suppression),
            row,
        };
    }

    // Only Moved refusals remain (any other refusal would have been live).
    let row = refusals
        .iter()
        .copied()
        .max_by_key(|r| recorded_instant(r))
        .map(|r| PrReviewRow::ReworkReady {
            reviewed_sha: sha(r),
            recorded_by: r.recorded_by.clone(),
        });
    PrReviewDecision {
        suppressed: false,
        row,
    }
}

/// BUG-1549: one PR a live refusal blocks — the row explaining why it is
/// absent from the mergeable set. Not seat-scoped: whoever is about to merge
/// needs it.
// trace:BUG-1549 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BlockedReviewItem {
    pub pr: u64,
    pub spec: Option<String>,
    /// The sha the refusal was recorded against — empty when none was.
    pub reviewed_sha: String,
    /// Where the PR is now — empty when unknown.
    pub head_sha: String,
    pub reason: BlockedReason,
    /// RFC-3339 timestamp the refusal was recorded at, when known. `None`
    /// means the age is unknown — never rendered as "no rework" (PRIN-5).
    // trace:TASK-1310 | ai:claude
    pub recorded_at: Option<String>,
}

/// STORY-1419: one PR whose rework has landed on a refusal you recorded —
/// every refusal's sha has been moved past and nothing newer governs. Both
/// shas are reported; whether the move was a rebase or a real rework is the
/// reviewer's call.
// trace:STORY-1419 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReworkReadyItem {
    pub pr: u64,
    pub spec: Option<String>,
    /// The sha the refusal was recorded against.
    pub reviewed_sha: String,
    /// Where the PR is now.
    pub head_sha: String,
    /// STORY-1420: set when the refusal's recorder has EXITED (a one-shot
    /// drain reviewer, or a registered seat whose pid is gone). Such a row is
    /// shown to EVERY seat instead of routed to a recorder that can never
    /// receive it; this names the recorder so the reader knows whose refusal
    /// it is.
    // trace:STORY-1420 | ai:claude
    pub inherited_from: Option<String>,
}

/// STORY-1420: is the seat that recorded a verdict still around to receive
/// the follow-up? Resolved cheaply and locally (agent registry + pid probe,
/// never the network) by [`classify_recorder`].
// trace:STORY-1420 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecorderLiveness {
    /// A registered seat with this name has a live process.
    Live,
    /// The recorder cannot receive anything any more.
    Exited,
    /// No evidence either way — PRIN-5: surfaced to everyone, never hidden.
    Unknown,
}

/// STORY-1420: classify a verdict's `recorded_by` against the recorder's
/// liveness. `named_live(name)` answers from the local agent registry:
/// `Some(true)` = a live entry with that name, `Some(false)` = entries exist
/// but every one has exited, `None` = no entry at all.
///
/// - `aida drain reviewer` is the orchestrator's headless phase-3 writer: a
///   one-shot `claude -p` that exits once its verdict is written, with no
///   registry entry and no seat that outlives it — always `Exited`.
/// - `<name> (… reviewer seat)` (the `aida review record` identity) is looked
///   up by `<name>`.
/// - Anything else (the operator writer, a hand-written identity) is
///   `Unknown`.
// trace:STORY-1420 | ai:claude
pub(crate) fn classify_recorder(
    recorded_by: &str,
    named_live: impl Fn(&str) -> Option<bool>,
) -> RecorderLiveness {
    let who = recorded_by.trim();
    if who.eq_ignore_ascii_case("aida drain reviewer") {
        return RecorderLiveness::Exited;
    }
    let Some((name, rest)) = who.split_once(" (") else {
        return RecorderLiveness::Unknown;
    };
    let name = name.trim();
    if name.is_empty() || !rest.to_ascii_lowercase().contains("reviewer seat") {
        return RecorderLiveness::Unknown;
    }
    match named_live(name) {
        Some(true) => RecorderLiveness::Live,
        Some(false) => RecorderLiveness::Exited,
        None => RecorderLiveness::Unknown,
    }
}

/// STORY-1420: who is reading, for routing the rework-ready row. `identity`
/// is the seat identity (`AIDA_USER`) the STORY-1419 scoping already matched
/// against `recorded_by`.
// trace:STORY-1420 | ai:claude
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ReworkReader<'a> {
    pub identity: Option<&'a str>,
}

/// Why a PR's newest APPROVED verdict does not cover its current head:
/// re-review (Stale — the head demonstrably moved past what was approved)
/// vs. can't-tell-from-here (Unverifiable — no sha, one too short to
/// compare, or no known head).
// trace:BUG-1549 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StaleApprovalReason {
    Stale,
    Unverifiable,
}

impl StaleApprovalReason {
    /// The wire label shared by `--json` and the agent-mode TOON table.
    // trace:BUG-1739 | ai:claude
    pub(crate) fn as_wire_str(self) -> &'static str {
        match self {
            Self::Stale => "stale",
            Self::Unverifiable => "unverifiable",
        }
    }
}

/// BUG-1549: one PR whose newest recorded APPROVAL does not provably cover
/// its head. NOT seat-scoped: the reader who needs the warning is whoever is
/// about to merge, not necessarily the approver.
// trace:BUG-1549 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StaleApprovalItem {
    pub pr: u64,
    pub spec: Option<String>,
    /// The sha the approval was recorded against — empty when none was.
    pub reviewed_sha: String,
    /// Where the PR is now — empty when unknown.
    pub head_sha: String,
    pub reason: StaleApprovalReason,
}

/// The review rows of the report, built ONLY from [`classify_pr_review`]
/// decisions.
// trace:BUG-1549 | ai:claude
#[derive(Debug, Default, Clone)]
pub(crate) struct PrReviewRows {
    pub blocked_reviews: Vec<BlockedReviewItem>,
    pub stale_approvals: Vec<StaleApprovalItem>,
    pub rework_ready: Vec<ReworkReadyItem>,
}

impl PrReviewRows {
    /// Render one PR's decision into its row, if any. The spec id is derived
    /// from the head branch (STORY-1419). Only the non-suppressing
    /// `ReworkReady` row is scoped to `seat` (the refusing reviewer; unknown
    /// on either side surfaces) — a row that explains a suppression is never
    /// filtered, so suppressed-iff-row survives into the report.
    // trace:STORY-1419 trace:BUG-1549 | ai:claude
    #[cfg(test)]
    pub(crate) fn add(
        &mut self,
        pr: u64,
        head_sha: Option<&str>,
        head_branch: &str,
        decision: &PrReviewDecision,
        seat: Option<&str>,
    ) {
        // Every recorder is treated as live here — the STORY-1419 contract.
        // Production routes through `add_routed`, which resolves liveness.
        self.add_routed(
            pr,
            head_sha,
            head_branch,
            decision,
            ReworkReader { identity: seat },
            |_| RecorderLiveness::Live,
        )
    }

    /// STORY-1420: [`Self::add`] with the rework-ready row routed by the
    /// recorder's liveness. A LIVE recorder keeps the STORY-1419 routing (the
    /// refusing seat). An EXITED recorder's row is shown to EVERY seat (the
    /// implementer, advisor and reviewer alike — a moved-past refusal needs a
    /// re-review and whoever can act should see it) and names the recorder.
    /// UNKNOWN liveness is shown to everyone too (PRIN-5). The row is never
    /// silently dropped for want of a recipient.
    // trace:STORY-1420 | ai:claude
    pub(crate) fn add_routed(
        &mut self,
        pr: u64,
        head_sha: Option<&str>,
        head_branch: &str,
        decision: &PrReviewDecision,
        reader: ReworkReader<'_>,
        liveness: impl Fn(&str) -> RecorderLiveness,
    ) {
        let seat = reader.identity;
        let Some(row) = decision.row.as_ref() else {
            return;
        };
        let spec = crate::pr_ship::extract_spec_ids_from_text(head_branch)
            .into_iter()
            .next();
        let head_sha = head_sha.unwrap_or("").trim().to_string();
        match row {
            PrReviewRow::Blocked {
                reason,
                reviewed_sha,
                recorded_at,
            } => self.blocked_reviews.push(BlockedReviewItem {
                pr,
                spec,
                reviewed_sha: reviewed_sha.clone(),
                head_sha,
                reason: *reason,
                recorded_at: recorded_at.clone(),
            }),
            PrReviewRow::Stale { reviewed_sha } => self.stale_approvals.push(StaleApprovalItem {
                pr,
                spec,
                reviewed_sha: reviewed_sha.clone(),
                head_sha,
                reason: StaleApprovalReason::Stale,
            }),
            PrReviewRow::Unverifiable { reviewed_sha } => {
                self.stale_approvals.push(StaleApprovalItem {
                    pr,
                    spec,
                    reviewed_sha: reviewed_sha.clone(),
                    head_sha,
                    reason: StaleApprovalReason::Unverifiable,
                })
            }
            PrReviewRow::ReworkReady {
                reviewed_sha,
                recorded_by,
            } => {
                let mut inherited_from = None;
                if let Some(who) = recorded_by.as_deref() {
                    match liveness(who) {
                        RecorderLiveness::Live => {
                            if let Some(me) = seat {
                                if !who.contains(me) {
                                    return;
                                }
                            }
                        }
                        // PRIN-5: nobody can say whether the recorder can
                        // still receive this — show it to everyone.
                        RecorderLiveness::Unknown => {}
                        // The recorder can never receive it: every seat sees it.
                        RecorderLiveness::Exited => {
                            inherited_from = Some(who.to_string());
                        }
                    }
                }
                self.rework_ready.push(ReworkReadyItem {
                    pr,
                    spec,
                    reviewed_sha: reviewed_sha.clone(),
                    head_sha,
                    inherited_from,
                })
            }
        }
    }
}

/// How a recorded sha relates to the current head.
///
/// THREE states, not two. Verdict writers record full or abbreviated shas
/// depending on the path, so a short-vs-long PAIR IS NOT A MOVED HEAD and must
/// be compared on the shared prefix.
///
/// THE POPULATION THIS FUNCTION CAN RECEIVE is only those records carrying a
/// `reviewed_sha`; a head-only legacy record is dropped upstream and never
/// reaches here. Of those, 37 of 107 were abbreviated when swept 2026-09-21.
/// The predicate and the date are stated because the same corpus answers 41 of
/// 149 under a wider predicate that includes records this code cannot see, and
/// a bare count outlives the question it was measured to answer.
///
/// `Incomparable` is the state a boolean could not express, and its absence was
/// a real defect: below the prefix floor the old predicate returned "not equal",
/// which emitted a row that NOTHING COULD EVER CLEAR, because no push will make
/// a 3-character string equal a 40-character one. "Too short to tell" is not
/// "different"; it degrades to silence, exactly like absent provenance.
// trace:BUG-1546 | ai:claude
// BUG-1549: `review_relation` maps this onto `ReviewRelation` for
// `classify_pr_review`, reusing the same prefix-length floor.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ShaRelation {
    Same,
    Moved,
    Incomparable,
}

/// Shortest prefix worth comparing. Below this a match is coincidence rather
/// than evidence.
const MIN_COMPARABLE_SHA: usize = 7;

pub(crate) fn compare_shas(head: &str, reviewed: &str) -> ShaRelation {
    let n = head.len().min(reviewed.len()).min(40);
    if n < MIN_COMPARABLE_SHA {
        return ShaRelation::Incomparable;
    }
    if head[..n].eq_ignore_ascii_case(&reviewed[..n]) {
        ShaRelation::Same
    } else {
        ShaRelation::Moved
    }
}

/// Pending-worker-directives summary for the awaiting-you report: how many
/// directives sit in the local directive file plus the FIFO head's one-line
/// summary (what the worker/advisor will pick up next). `pending == 0` →
/// the channel renders nothing.
// trace:TASK-1146 | ai:claude
#[derive(Debug, Default, Clone)]
pub(crate) struct DirectivesChannel {
    pub pending: usize,
    /// One-line summary of the next (FIFO-head) directive, e.g.
    /// `human-audit /aida-human-audit` or `drain batch:x --zen`.
    pub next: Option<String>,
}

/// Due-seat-jobs summary for the awaiting-you report: how many seat jobs are
/// due for the session's seat plus the head job's due-line. `due == 0` → the
/// channel renders nothing.
// trace:STORY-1226 | ai:claude
#[derive(Debug, Default, Clone)]
pub(crate) struct CronChannel {
    pub due: usize,
    /// One-line summary of the first due job, e.g.
    /// `mailbox-triage (every 30m, last 47m ago) → triage the mailbox`.
    pub next: Option<String>,
}

/// Unread-mail summary for the awaiting-you report: the full unread count
/// plus how many of those are flagged urgent. Both zero → the inbox is
/// caught up and the mail channel renders nothing.
///
/// BUG-767: `unread`/`urgent` are the OPERATOR's OWN inbox (the handle this
/// session sends and receives as) — deterministic and monotone under deletion:
/// clearing your inbox drives it to 0, never to some wider number. Mail sitting
/// in the SHARED inboxes this session also reads (its session role, its agent
/// type — where agent-to-agent traffic lands) is counted separately in
/// `shared_unread` and rendered on its own labelled line, so the fleet-wide
/// view is kept but can never leak into the operator-gated number.
// trace:STORY-741 | ai:claude
#[derive(Debug, Default, Clone)]
pub(crate) struct MailChannel {
    pub unread: usize,
    /// Urgent unread mail to this session: its own inbox plus its role's
    /// (`role_urgent` of these), since role mail is direct mail.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-040 | ai:claude
    pub urgent: usize,
    /// Unread mail in the shared role / agent-type inboxes this session also
    /// reads, EXCLUDING anything already counted in `unread` (a broadcast is
    /// in both, and must count once, in the operator's own number).
    // trace:BUG-767 | ai:claude
    pub shared_unread: usize,
    /// Which shared inboxes `shared_unread` spans, e.g. `advisor + claude` —
    /// the label that keeps this surface distinct from the operator's own.
    // trace:BUG-767 | ai:claude
    pub shared_scope: Option<String>,
    /// The mail view could not read every source: the counts may be low,
    /// so the report must not read as caught up.
    // trace:STORY-1843 | ai:claude
    pub partial: bool,
    /// Unread mail addressed to this session's ROLE (e.g. `advisor`). It is
    /// direct mail for the seat, so it never reads as `shared` noise.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-040 | ai:claude
    pub role_unread: usize,
    /// The role `role_unread` counts, e.g. `advisor`.
    pub role: Option<String>,
    /// How many of `urgent` are role mail.
    pub role_urgent: usize,
    /// The newest unread items (own, role and shared), `[request]` first,
    /// capped at [`MAIL_PREVIEW_LINES`], so the notice names who wrote what.
    pub previews: Vec<MailPreview>,
    /// Unread `[request]` mail to the operator or role left unread longer
    /// than [`OVERDUE_REQUEST_SECS`] — an item for the addressed seat.
    pub overdue_requests: Vec<MailPreview>,
    /// Unread automated notices older than [`STALE_AUTOMATED_MAIL_SECS`],
    /// dropped from every count above and summarized as one line.
    pub stale_automated: usize,
}

impl MailChannel {
    /// The urgent mail in the session's own inbox, apart from its role's.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-040 | ai:claude
    pub(crate) fn own_urgent(&self) -> usize {
        self.urgent.saturating_sub(self.role_urgent)
    }
}

/// ` (N urgent)`, or nothing when none is.
fn urgent_suffix(n: usize) -> String {
    if n > 0 {
        format!(" ({n} urgent)")
    } else {
        String::new()
    }
}

/// One unread message, as the per-turn notice names it.
// trace:BUG-B6B1278890774EF993D2EBC327862470-040 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MailPreview {
    pub from: String,
    pub intent: aida_core::mailbox::Intent,
    pub age_secs: i64,
    pub subject: String,
}

impl MailPreview {
    fn from_message(m: &aida_core::mailbox::Message, now: i64) -> Self {
        Self {
            from: m.from.clone(),
            intent: m.intent,
            age_secs: mail_age_secs(m, now).unwrap_or(0).max(0),
            subject: aida_core::mailbox::subject_line(m, 72),
        }
    }

    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "from": self.from,
            "intent": self.intent.as_str(),
            "age_secs": self.age_secs,
            "subject": self.subject,
        })
    }

    /// `[request] from codex · 9h ago · PR-3122 needs review routing`.
    pub fn line(&self) -> String {
        format!(
            "[{}] from {} · {} ago · {}",
            self.intent.as_str(),
            self.from,
            mail_age_label(self.age_secs),
            self.subject
        )
    }
}

/// How many unread items the per-turn notice names.
// trace:BUG-B6B1278890774EF993D2EBC327862470-040 | ai:claude
pub(crate) const MAIL_PREVIEW_LINES: usize = 3;
/// An unread `[request]` older than this escalates in `aida awaiting`.
pub(crate) const OVERDUE_REQUEST_SECS: i64 = 30 * 60;
/// Unread automated notices older than this stop counting as new mail.
pub(crate) const STALE_AUTOMATED_MAIL_SECS: i64 = 7 * 86_400;

/// Senders that are AIDA's own machinery, not a seat: `review-watch`,
/// `aida-session-reap`, and other `aida-*` / `*-watch` notifiers.
// trace:BUG-B6B1278890774EF993D2EBC327862470-040 | ai:claude
pub(crate) fn is_automated_sender(from: &str) -> bool {
    let from = from.trim().to_ascii_lowercase();
    from.starts_with("aida-") || from.ends_with("-watch") || from.contains("-watch-")
}

/// An automated notice left unread past [`STALE_AUTOMATED_MAIL_SECS`].
// trace:BUG-B6B1278890774EF993D2EBC327862470-040 | ai:claude
pub(crate) fn is_stale_automated(m: &aida_core::mailbox::Message, now: i64) -> bool {
    is_automated_sender(&m.from)
        && mail_age_secs(m, now).is_some_and(|age| age > STALE_AUTOMATED_MAIL_SECS)
}

/// Seconds since a message was sent, given `now` in unix SECONDS. Message
/// timestamps are epoch MILLIS (BUG-032); `None` when the time is unknown, so
/// an unstamped message is neither stale nor overdue.
// trace:BUG-B6B1278890774EF993D2EBC327862470-040 | ai:claude
pub(crate) fn mail_age_secs(m: &aida_core::mailbox::Message, now: i64) -> Option<i64> {
    aida_core::mailbox::timestamp_is_known(m.timestamp)
        .then(|| now.saturating_sub(m.timestamp / 1000))
}

fn mail_age_label(secs: i64) -> String {
    if secs < 60 {
        "<1m".to_string()
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        crate::leases::humanize_age_secs(secs.unsigned_abs())
    }
}

/// BUG-767: split unread mail into the operator's OWN inbox and the SHARED
/// (session-role / agent-type) inboxes the same session reads.
///
/// The `awaiting` contract is "channels where the OPERATOR is the gate", so the
/// headline number must be a function of the operator handle's inbox alone:
/// empty inbox → 0, always. Before this split the count spanned the whole
/// ambient identity set, so an operator whose own inbox was empty still saw the
/// role inbox's agent-to-agent backlog reported as *their* unread mail — and
/// because that set is env-derived (`AIDA_SESSION_ROLE` / `AIDA_AGENT_TYPE`),
/// the number could GROW as you cleaned your inbox.
///
/// PURE: takes the already-read messages + watermarks, so the counting scope is
/// unit-testable without touching the filesystem or env.
// trace:BUG-767 | ai:claude
pub(crate) fn split_mail_scopes(
    operator: &str,
    shared: &[String],
    messages: &[aida_core::mailbox::Message],
    watermarks: &std::collections::HashMap<String, i64>,
) -> MailChannel {
    split_mail_scopes_for_role(
        operator,
        None,
        shared,
        messages,
        watermarks,
        chrono::Utc::now().timestamp(),
    )
}

/// [`split_mail_scopes`] with the session's ROLE pulled out of the shared
/// scope: mail addressed to the role is the seat's direct mail
/// (`role_unread`), not fleet-wide noise. Also builds the per-turn previews,
/// the overdue-request items, and drops automated notices older than
/// [`STALE_AUTOMATED_MAIL_SECS`] from every count into `stale_automated`.
/// PURE: `now` is supplied (unix seconds).
// trace:BUG-B6B1278890774EF993D2EBC327862470-040 | ai:claude
pub(crate) fn split_mail_scopes_for_role(
    operator: &str,
    role: Option<&str>,
    shared: &[String],
    messages: &[aida_core::mailbox::Message],
    watermarks: &std::collections::HashMap<String, i64>,
    now: i64,
) -> MailChannel {
    use aida_core::mailbox::{unread_for_identities, Intent, Message, Recipient};
    let operator = operator.trim();
    let stale = |m: &Message| is_stale_automated(m, now);

    let own_all = unread_for_identities([operator], messages, watermarks);
    let own_ids: std::collections::HashSet<&str> = own_all.iter().map(|m| m.id.as_str()).collect();
    let role = role
        .map(str::trim)
        .filter(|r| !r.is_empty() && *r != operator);
    let role_all: Vec<&Message> = role
        .map(|r| {
            unread_for_identities([r], messages, watermarks)
                .into_iter()
                .filter(|m| !own_ids.contains(m.id.as_str()) && m.to != Recipient::Broadcast)
                .collect()
        })
        .unwrap_or_default();
    let role_ids: std::collections::HashSet<&str> =
        role_all.iter().map(|m| m.id.as_str()).collect();
    let shared_ids: Vec<&str> = shared
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty() && *s != operator && Some(*s) != role)
        .collect();
    let shared_all: Vec<&Message> =
        unread_for_identities(shared_ids.iter().copied(), messages, watermarks)
            .into_iter()
            .filter(|m| !own_ids.contains(m.id.as_str()) && !role_ids.contains(m.id.as_str()))
            .collect();

    let stale_automated = own_all
        .iter()
        .chain(&role_all)
        .chain(&shared_all)
        .filter(|m| stale(m))
        .count();
    let own: Vec<&Message> = own_all.into_iter().filter(|m| !stale(m)).collect();
    let role_mail: Vec<&Message> = role_all.into_iter().filter(|m| !stale(m)).collect();
    let shared_mail: Vec<&Message> = shared_all.into_iter().filter(|m| !stale(m)).collect();

    let rank = |i: Intent| match i {
        Intent::Request => 0,
        Intent::Handoff => 1,
        Intent::Fyi => 2,
    };
    let mut newest: Vec<&Message> = own
        .iter()
        .chain(&role_mail)
        .chain(&shared_mail)
        .copied()
        .collect();
    newest.sort_by(|a, b| {
        rank(a.intent)
            .cmp(&rank(b.intent))
            .then_with(|| b.timestamp.cmp(&a.timestamp))
            .then_with(|| a.id.cmp(&b.id))
    });
    let previews = newest
        .into_iter()
        .take(MAIL_PREVIEW_LINES)
        .map(|m| MailPreview::from_message(m, now))
        .collect();
    let mut overdue: Vec<&Message> = own
        .iter()
        .chain(&role_mail)
        .copied()
        .filter(|m| {
            m.intent == Intent::Request
                && mail_age_secs(m, now).is_some_and(|age| age > OVERDUE_REQUEST_SECS)
        })
        .collect();
    overdue.sort_by(|a, b| a.timestamp.cmp(&b.timestamp).then_with(|| a.id.cmp(&b.id)));

    MailChannel {
        unread: own.len(),
        urgent: own.iter().chain(&role_mail).filter(|m| m.urgent).count(),
        shared_unread: shared_mail.len(),
        shared_scope: (!shared_mail.is_empty()).then(|| shared_ids.join(" + ")),
        partial: false,
        role_unread: role_mail.len(),
        role_urgent: role_mail.iter().filter(|m| m.urgent).count(),
        role: role.filter(|_| !role_mail.is_empty()).map(str::to_string),
        previews,
        overdue_requests: overdue
            .into_iter()
            .map(|m| MailPreview::from_message(m, now))
            .collect(),
        stale_automated,
    }
}

#[derive(Debug, Clone)]
pub(crate) struct MergeablePrItem {
    pub number: u64,
    pub title: String,
    pub head_branch: String,
    /// `None` when the PR has no CI checks set up; `Some("pass")` when
    /// every check is green. The classifier already excluded `fail` /
    /// `pending`, so this is informational.
    pub ci_rollup: Option<String>,
    /// STORY-1405: `Some(description)` when a live review-in-progress marker
    /// covers this PR's current head — ready by CI, but a verdict is pending.
    // trace:STORY-1405 | ai:claude
    pub under_review: Option<String>,
}

/// One change the review watcher is parking, with its reason.
// trace:BUG-2426 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct WatchSkippedItem {
    pub change: String,
    pub head: String,
    pub reason: String,
    pub since: chrono::DateTime<chrono::Utc>,
}

impl WatchSkippedItem {
    pub(crate) fn from_watcher(root: &std::path::Path) -> Vec<Self> {
        crate::review_watch::skipped_changes(root)
            .into_iter()
            .map(|(change, head, reason, since)| Self {
                change,
                head,
                reason,
                since,
            })
            .collect()
    }
    fn short_head(&self) -> &str {
        self.head.get(..12).unwrap_or(&self.head)
    }
    fn since_label(&self) -> String {
        self.since.format("%Y-%m-%d %H:%M UTC").to_string()
    }
}

/// A broken PR which has fallen between the review and repair lanes.
// trace:TASK-192 | ai:codex
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnownedFailingPrItem {
    pub number: u64,
    pub title: String,
    pub head_branch: String,
    /// BUG-1514: `Some(spec_id)` when this row is the two-way inconsistency —
    /// a spec already marked Done whose PR (named in the title) is failing —
    /// rather than the ordinary no-owner/no-route case. `None` for the
    /// latter.
    pub done_spec: Option<String>,
}

/// Routing facts layered over the forge snapshot. Kept separate from
/// `OpenPrItem` because holds, queues, and live leases are local substrate
/// state rather than forge properties.
// trace:TASK-192 | ai:codex
#[derive(Debug, Clone)]
pub(crate) struct UnownedFailingPrCandidate {
    pub pr: OpenPrItem,
    pub has_local_verdict: bool,
    pub held: bool,
    pub route: ReviewerRoute,
    pub actively_owned: bool,
    /// BUG-1514: `Some(spec_id)` when the PR title names a spec whose status
    /// is currently Done. The caller resolves this so the classifier stays
    /// pure — no store lookup here.
    pub done_spec: Option<String>,
}

/// Fail-closed knowledge of the reviewer queue. Queue read or parse failures
/// are not evidence that a PR is unrouted.
// trace:TASK-192 | ai:codex
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReviewerRoute {
    Routed,
    Unrouted,
    Unknown,
}

/// BUG-1514: minimum time a PR must have been red before this channel
/// surfaces it. A PR is routinely red for several minutes mid push-fix-
/// repush; #2035, the motivating instance, was red and unowned for 3.8
/// hours. 30 minutes sits comfortably above normal rework-cycle latency
/// (CI run + notice + fix + repush) and far below the hours of neglect the
/// bug describes, so it debounces routine churn without hiding real
/// staleness for long. Named here rather than left implicit per acceptance
/// criterion #3.
// trace:BUG-1514 | ai:claude
pub(crate) const UNOWNED_FAILING_PR_MIN_AGE_MINUTES: i64 = 30;

/// BUG-1514: age-gate a PR against [`UNOWNED_FAILING_PR_MIN_AGE_MINUTES`].
/// Unknown creation time (missing/malformed `createdAt`) fails OPEN — this
/// bug is precisely about evidence gaps turning into silent invisibility,
/// so "we don't know how old it is" must not mean "never surface it."
// trace:BUG-1514 | ai:claude
fn pr_meets_min_age(
    created_at: Option<chrono::DateTime<chrono::Utc>>,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    match created_at {
        None => true,
        Some(created) => {
            now.signed_duration_since(created)
                >= chrono::Duration::minutes(UNOWNED_FAILING_PR_MIN_AGE_MINUTES)
        }
    }
}

/// The complement of the green orphan-review lane, PLUS the BUG-1514
/// two-way inconsistency: a spec already marked Done whose PR (named in the
/// title) is red. The ordinary no-owner/no-route case still requires every
/// ownership signal to be absent; the Done-spec case bypasses verdict/route/
/// ownership (the spec's own status already contradicts red CI, independent
/// of who is reviewing or holding the branch) but still respects an active
/// merge hold, so it doesn't duplicate a row another surface already owns.
/// Pending is not red; green/no-CI belongs to the reviewer sweep. Both paths
/// share the same minimum-age debounce.
///
/// NOT COVERED: a Done spec whose PR is closed, missing, or conflicting
/// rather than open-and-red. That needs a spec-driven forge lookup (one
/// `gh` call per Done spec) rather than this PR-driven pass over the
/// already-fetched open-PR snapshot, and is left as a follow-on — the same
/// cost/generality tradeoff TASK-192 made for the green sweep's complement.
// trace:TASK-192 trace:BUG-1514 | ai:claude ai:codex
pub(crate) fn classify_unowned_failing_prs(
    candidates: &[UnownedFailingPrCandidate],
    now: chrono::DateTime<chrono::Utc>,
) -> Vec<UnownedFailingPrItem> {
    candidates
        .iter()
        .filter(|c| c.pr.ci_rollup.as_deref() == Some("fail"))
        .filter(|c| {
            if c.done_spec.is_some() {
                return true;
            }
            let forge_verdict =
                c.pr.review_decision
                    .as_deref()
                    .unwrap_or("")
                    .to_ascii_uppercase();
            !matches!(forge_verdict.as_str(), "APPROVED" | "CHANGES_REQUESTED")
                && !c.has_local_verdict
        })
        .filter(|c| {
            !c.held
                && (c.done_spec.is_some()
                    || (c.route == ReviewerRoute::Unrouted && !c.actively_owned))
        })
        .filter(|c| pr_meets_min_age(c.pr.created_at, now))
        .map(|c| UnownedFailingPrItem {
            number: c.pr.number,
            title: c.pr.title.clone(),
            head_branch: c.pr.head_branch.clone(),
            done_spec: c.done_spec.clone(),
        })
        .collect()
}

// Shared row projection keeps TOON aligned with human/JSON action text.
// trace:TASK-192 | ai:codex
pub(crate) fn unowned_failing_pr_toon_rows(items: &[UnownedFailingPrItem]) -> Vec<Vec<String>> {
    items
        .iter()
        .map(|p| {
            vec![
                p.number.to_string(),
                p.title.clone(),
                p.head_branch.clone(),
                format!("gh pr checks {}", p.number),
            ]
        })
        .collect()
}

/// A spec id as the human/TOON awaiting views show it: friendly node label
/// applied (JSON keeps the canonical id).
// trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
pub(crate) fn shown_spec(id: &str) -> String {
    aida_core::node_alias::display_spec_id(id)
}

/// [`shown_spec`] for an optional id, `-` when absent.
pub(crate) fn shown_opt_spec(id: Option<&str>) -> String {
    id.map(shown_spec).unwrap_or_else(|| "-".into())
}

/// BUG-1739: the three-way orphan verdict as one label. The human render and
/// the agent-mode table read it from here; before this there was exactly one
/// copy because there was exactly one renderer.
// trace:BUG-1739 | ai:claude
pub(crate) fn orphaned_state_label(item: &OrphanedInProgressItem) -> &'static str {
    if item.possibly_subagent {
        // BUG-1681: the old wording said only "live harness lease in this repo",
        // which is what the old predicate actually tested — and why every stale
        // spec claimed a subagent the moment any fan-out was alive anywhere.
        // The predicate now requires the subagent to hold THIS worktree; say so.
        // trace:BUG-1656 | ai:claude
        // trace:BUG-1681 | ai:claude
        "possibly worked by a subagent — lease pid dead, a live subagent holds this worktree"
    } else if item.abandoned {
        "abandoned — lease died"
    } else {
        "not yet started — no lease"
    }
}

/// BUG-1739: TOON rows for agent-mode `orphaned_in_progress`. Carries the same
/// verdict and age the human line gives, plus the `aida ps` next step — an
/// agent reading this must learn *why* the spec is orphaned, not merely that
/// the count was non-zero.
// trace:BUG-1739 | ai:claude
pub(crate) fn orphaned_in_progress_toon_rows(items: &[OrphanedInProgressItem]) -> Vec<Vec<String>> {
    items
        .iter()
        .map(|o| {
            vec![
                shown_spec(&o.spec_id),
                orphaned_state_label(o).to_string(),
                o.since_label.clone(),
                "aida ps".to_string(),
            ]
        })
        .collect()
}

/// BUG-1739: TOON rows for agent-mode `blocked_seats`.
// trace:BUG-1739 | ai:claude
pub(crate) fn blocked_seat_toon_rows(items: &[BlockedSeatItem]) -> Vec<Vec<String>> {
    items
        .iter()
        .map(|b| {
            vec![
                b.session_id.clone(),
                shown_opt_spec(b.spec.as_deref()),
                b.tool.clone().unwrap_or_else(|| "-".into()),
                b.since_label.clone(),
            ]
        })
        .collect()
}

/// TOON rows for agent-mode `deputy_failed`.
// trace:STORY-B6B1278890774EF993D2EBC327862470-155 | ai:claude
pub(crate) fn deputy_failed_toon_rows(items: &[crate::deputy::FailedJob]) -> Vec<Vec<String>> {
    items
        .iter()
        .map(|j| {
            vec![
                j.id.clone(),
                j.deputy.clone(),
                j.kind_label().to_string(),
                j.requester_session.clone().unwrap_or_else(|| "-".into()),
                j.rc.to_string(),
                j.reason.clone(),
                j.output.display().to_string(),
                j.ack_command(),
            ]
        })
        .collect()
}

/// BUG-1739: TOON rows for agent-mode `pr_attribution_disagreements`.
// trace:BUG-1739 | ai:claude
pub(crate) fn pr_attribution_toon_rows(
    items: &[PrAttributionDisagreementItem],
) -> Vec<Vec<String>> {
    items
        .iter()
        .map(|d| {
            vec![
                d.pr.to_string(),
                shown_spec(&d.lease_spec),
                shown_spec(&d.trailer_spec),
            ]
        })
        .collect()
}

/// BUG-1739: TOON rows for agent-mode `rework_ready`.
// trace:BUG-1739 | ai:claude
pub(crate) fn rework_ready_toon_rows(items: &[ReworkReadyItem]) -> Vec<Vec<String>> {
    items
        .iter()
        .map(|r| {
            vec![
                r.pr.to_string(),
                shown_opt_spec(r.spec.as_deref()),
                r.reviewed_sha.clone(),
                r.head_sha.clone(),
            ]
        })
        .collect()
}

/// BUG-1739: TOON rows for agent-mode `stale_approvals`. `reason` uses the same
/// wire strings `to_json` emits, so the two machine surfaces agree.
// trace:BUG-1739 | ai:claude
pub(crate) fn stale_approval_toon_rows(items: &[StaleApprovalItem]) -> Vec<Vec<String>> {
    items
        .iter()
        .map(|i| {
            vec![
                i.pr.to_string(),
                shown_opt_spec(i.spec.as_deref()),
                i.reviewed_sha.clone(),
                i.head_sha.clone(),
                i.reason.as_wire_str().to_string(),
            ]
        })
        .collect()
}

/// BUG-1739: TOON rows for agent-mode `blocked_reviews`.
// trace:BUG-1739 | ai:claude
pub(crate) fn blocked_review_toon_rows(items: &[BlockedReviewItem]) -> Vec<Vec<String>> {
    items
        .iter()
        .map(|i| {
            vec![
                i.pr.to_string(),
                shown_opt_spec(i.spec.as_deref()),
                i.reviewed_sha.clone(),
                i.head_sha.clone(),
                i.reason.as_wire_str().to_string(),
            ]
        })
        .collect()
}

#[derive(Debug, Clone)]
pub(crate) struct PendingBriefItem {
    pub agent: String,
    pub spec_id: String,
    pub path: std::path::PathBuf,
}

// trace:STORY-1397 | ai:codex
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct RecusalHoldItem {
    pub pr: u64,
    pub head_sha: Option<String>,
    pub recused_principals: Vec<String>,
    pub routed_to: Vec<String>,
    pub state: String,
    pub relationship: String,
    pub action: String,
}

impl RecusalHoldItem {
    pub(crate) fn is_actionable_for_current_principal(&self) -> bool {
        self.relationship != "recused" && self.relationship != "other-reader"
    }
}

/// One open PR held by a non-recusal merge hold.
// trace:STORY-1397 | ai:claude
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct HeldPrItem {
    pub pr: u64,
    pub title: String,
    pub reason_kind: String,
    pub detail: String,
    pub action: String,
}

/// Project a non-recusal hold. Rework, decision and supervision need opposite
/// actions from different people, so each kind names its own.
// trace:STORY-1397 | ai:claude
pub(crate) fn project_held_pr(
    record: &crate::gate::merge_hold_core::MergeHoldRecord,
    title: &str,
    approved_at_head: Option<&str>,
    is_draft: bool,
) -> HeldPrItem {
    use crate::gate::merge_hold_core::HoldReasonKind;
    let pr = record.pr;
    let action = match record.reason_kind {
        HoldReasonKind::Rework => {
            if let Some(head) = approved_at_head {
                let short = crate::gate::review_verdict::short_sha(head);
                if is_draft {
                    format!("rework checkpoint approved at {short}; PR is Draft — finish implementation and mark ready")
                } else {
                    format!(
                        "rework approved at {short}; awaiting human: aida merge-hold clear {pr}"
                    )
                }
            } else {
                "implementer: address the review findings; the hold lifts after approval"
                    .to_string()
            }
        }
        HoldReasonKind::Decision => {
            format!("human decision needed; once decided, `aida merge-hold clear {pr}`")
        }
        HoldReasonKind::Unknown => {
            format!("hold marker unreadable — inspect `.aida/merge-holds/PR-{pr}`")
        }
        HoldReasonKind::Supervision | HoldReasonKind::Recusal => {
            format!("review, then `aida merge-hold clear {pr}` at a terminal")
        }
    };
    HeldPrItem {
        pr,
        title: title.to_string(),
        reason_kind: record.reason_kind.as_str().to_string(),
        detail: record.detail.clone(),
        action,
    }
}

/// Project a corpus-derived rework hold (BUG-1773). Presents as `rework` so it
/// is indistinguishable in shape from a marker-backed rework hold — the reader
/// should not have to know which producer armed it — but the detail names the
/// head and the finding count, which is what makes a corpus-derived row
/// actionable without a marker to read.
// trace:BUG-1773 | ai:claude
pub(crate) fn project_corpus_held_pr(
    pr: u64,
    title: &str,
    hold: &crate::gate::review_verdict::CorpusHold,
) -> HeldPrItem {
    let short: String = hold.sha.chars().take(12).collect();
    let detail = match &hold.integrity_error {
        Some(message) => format!("review verdicts cannot be reconciled at {short}: {message}"),
        None => {
            let findings = match hold.findings {
                0 => "no itemised findings".to_string(),
                1 => "1 finding".to_string(),
                n => format!("{n} findings"),
            };
            format!(
                "`{}` recorded at {short} ({findings}); no hold marker — derived from the verdict corpus",
                hold.verdict_raw
            )
        }
    };
    HeldPrItem {
        pr,
        title: title.to_string(),
        reason_kind: crate::gate::merge_hold_core::HoldReasonKind::Rework
            .as_str()
            .to_string(),
        detail,
        action: "implementer: address the review findings; the hold lifts after approval"
            .to_string(),
    }
}

/// One open PR's resolved inputs for the corpus-derived hold decision
/// (BUG-1773). The caller does the filesystem and store reads; everything the
/// decision needs is on this struct, so the decision itself stays pure and the
/// "no double entry" rule has a test that does not need a repository.
// trace:BUG-1773 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct CorpusHoldCandidate {
    pub pr: u64,
    pub title: String,
    /// The PR's current head. `None` when the snapshot omitted it — this view
    /// then holds nothing (it fails OPEN; the merge gate fails closed).
    pub head_sha: Option<String>,
    /// Raw verdict artifacts keyed to this PR (`PR-<n>` and the spec).
    pub bodies: Vec<String>,
    /// The owning spec is `Completed` — BUG-1529's criterion-4 escape.
    pub spec_completed: bool,
    /// A `.aida/merge-holds/PR-<n>` marker already speaks for this PR.
    pub has_marker_hold: bool,
    pub is_draft: bool,
}

/// Derive the rework holds the verdict corpus implies, for open PRs that have no
/// marker of their own. Pure: no filesystem, no network, no clock.
// trace:BUG-1773 | ai:claude
pub(crate) fn corpus_held_prs(candidates: &[CorpusHoldCandidate]) -> Vec<HeldPrItem> {
    candidates
        .iter()
        // A marker-backed hold is already projected by `project_held_pr`, and it
        // carries the richer typed reason. One row per PR, never two.
        .filter(|candidate| !candidate.has_marker_hold)
        .filter_map(|candidate| {
            let hold = crate::gate::review_verdict::corpus_hold_at_head(
                &candidate.bodies,
                candidate.head_sha.as_deref(),
                candidate.spec_completed,
            )?;
            Some(project_corpus_held_pr(
                candidate.pr,
                &candidate.title,
                &hold,
            ))
        })
        .collect()
}

/// Seat-aware projection. Exact stable principal equality is deliberately used:
/// sharing a role or vendor does not make two agents the same reviewer.
// trace:STORY-1397 | ai:codex
pub(crate) fn project_recusal_hold(
    record: &crate::gate::merge_hold_core::MergeHoldRecord,
    current_principal: &str,
) -> Option<RecusalHoldItem> {
    if record.reason_kind != crate::gate::merge_hold_core::HoldReasonKind::Recusal {
        return None;
    }
    let me = crate::gate::merge_hold_core::PrincipalIdentity::parse(current_principal);
    let verified = me.identity_status == crate::gate::merge_hold_core::IdentityStatus::Verified;
    let recused = verified && crate::gate::merge_hold_core::principal_is_recused(record, &me);
    let routed = verified && record.routed_to.iter().any(|p| p == &me);
    let (relationship, action) = if recused {
        (
            "recused".to_string(),
            "await another independent reader; you cannot review, clear, or merge this hold"
                .to_string(),
        )
    } else if routed {
        (
            "routed-reader".to_string(),
            format!("independently review PR-{} at exact head", record.pr),
        )
    } else if record.routing_state
        == crate::gate::merge_hold_core::HoldRoutingState::NoIndependentReader
    {
        (
            "operator-attention".to_string(),
            format!(
                "no independent reader is recorded; if independent, review PR-{pr} at exact head and `aida merge-hold clear {pr}` at a terminal, otherwise start or assign a reader",
                pr = record.pr
            ),
        )
    } else if record.routing_state == crate::gate::merge_hold_core::HoldRoutingState::StaleHead {
        // trace:STORY-1397 | ai:claude
        (
            "operator-attention".to_string(),
            "PR head moved since routing; re-route with `aida merge-hold list --fix`".to_string(),
        )
    } else {
        (
            "other-reader".to_string(),
            "await the routed independent reader".to_string(),
        )
    };
    Some(RecusalHoldItem {
        pr: record.pr,
        head_sha: record.target_head_sha.clone(),
        recused_principals: record.recused_principals.iter().map(|p| p.key()).collect(),
        routed_to: record.routed_to.iter().map(|p| p.key()).collect(),
        state: record.routing_state.as_str().to_string(),
        relationship,
        action,
    })
}

#[derive(Debug, Clone)]
pub(crate) struct ReviewerQueueItem {
    pub spec_id: String,
    pub title: String,
    /// Does a verdict already cover the current head? Resolved locally by
    /// `crate::reviewer_row_actionability` -- verdict file + git refs, no
    /// forge call. The row is kept and rendered regardless of state (routed
    /// rows never vanish); this only changes how it reads.
    // trace:BUG-1508 | ai:claude
    pub state: crate::gate::review_verdict::ReviewActionability,
}

#[derive(Debug, Clone)]
pub(crate) struct EscalationItem {
    pub spec_id: String,
    pub title: String,
}

// trace:STORY-1043 | ai:codex
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct UnshippedWorkItem {
    pub spec_id: String,
    pub branch: String,
    pub commits_ahead: u32,
    pub age: String,
    pub recovery: String,
    pub pr_state: String,
    /// BUG-1756: whether this branch has a remote counterpart (`origin/<name>`
    /// exists, or the row itself is a remote-only ref). `false` is the single
    /// most strandable state work can be in — the commits exist on exactly one
    /// machine — so the render and the recovery hint must distinguish it from
    /// a pushed-but-no-PR branch (push first vs just open a PR); TASK-1305
    /// already established that collapsing distinct unshipped states into one
    /// rendering is a defect.
    // trace:BUG-1756 | ai:claude
    pub pushed: bool,
}

/// BUG-1288: honesty flag for the `unshipped_work` scan itself, distinct from
/// the items it found. `aida awaiting --json` / `aida status --full` bound
/// the underlying branch probe to a wall-clock budget (see
/// `collect_unshipped_work_items_bounded` in `lib.rs`) so a repository with a
/// large candidate-branch population cannot block a machine-readable poll for
/// tens of seconds. `None` on the report means the scan never ran at all (the
/// `--notice` fast path, which has always skipped this channel). `Some` means
/// it ran; `complete: false` means the wall-clock budget was exhausted before
/// every candidate branch was probed, so `unshipped_work` is a LOWER BOUND —
/// real unshipped work cannot be missing from what IS reported, but more may
/// exist among the unscanned candidates. Never collapse an incomplete scan
/// into an empty/zero result (PRIN-5).
// trace:BUG-1288 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct UnshippedScanStatus {
    /// Every eligible candidate branch was probed before the budget expired.
    pub complete: bool,
    /// Candidate branches actually probed before finishing or timing out.
    pub scanned: usize,
    /// Total eligible candidate branches identified for this run — the same
    /// width PR #1999 (STORY-1368) established (every branch belonging to a
    /// non-terminal-status spec or a recorded lease); this scan never narrows
    /// that set, only how much of it it had time to finish probing.
    pub candidates: usize,
}

// TASK-1305: `recovery` is recorded once by the collector as the "no PR yet"
// hint (`aida pr ship <branch>`), which is correct for `pr_state` "absent"
// (and still reasonable for "merged"/"unknown", where shipping the remaining
// commits is the right move either way). It is actively wrong for "open" —
// the branch already has a PR in the review path, so the ask is a merge
// decision, not another PR. Every render surface for this item must route
// its action hint through here rather than reading `.recovery` directly, so
// the "open" case can never regress back to the identical-row bug this spec
// exists to fix. trace:TASK-1305 | ai:claude
pub(crate) fn unshipped_work_recovery_hint(item: &UnshippedWorkItem) -> String {
    if item.pr_state == "open" {
        format!(
            "gh pr view {} — PR already open, needs a merge decision",
            item.branch
        )
    } else {
        item.recovery.clone()
    }
}

/// TASK-1445: one live-drain PR whose lease-based owner and commit-trailer
/// owner disagree. Both claimed owners are reported, named by the evidence
/// that produced them — never a guess at which one is "right."
// trace:TASK-1445 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct PrAttributionDisagreementItem {
    pub pr: u64,
    /// The spec drain status attributes the PR to (the lease it ran under).
    pub lease_spec: String,
    /// The spec the PR's own commit trailers confidently name instead.
    pub trailer_spec: String,
}

// trace:STORY-1043 | ai:codex
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct NightlyRedItem {
    pub summary: String,
    pub run_id: Option<u64>,
    pub nights: usize,
}

impl AwaitingReport {
    /// Count of *lines* this report will render (PRs + briefs + 1 line
    /// for findings if any + 1 for unread mail + 1 for pending worker
    /// directives + reviewer items + escalations). Drives the `(N)` in
    /// the section header and the empty-report short-circuit.
    /// BUG-1739: every contributor `total()` counts, paired with the key the
    /// agent-mode view prints for it. This is the ONE list; `total()` is its
    /// sum and [`Self::render_agent_view`] must emit something for each key.
    ///
    /// The bug this exists to prevent: `total()` and the agent render were
    /// edited independently for nine contributors, so `awaiting: 6` could be
    /// printed above eight empty tables with no way to learn what the other
    /// three were. Three of them were specs stranded In-Progress for 64 hours.
    ///
    /// A source-scanning guard was the alternative and is weaker: it proves
    /// two files mention the same tokens, not that the renderer emits
    /// anything. Summing this list makes a new contributor *impossible* to add
    /// without a key, and the correspondence test renders each key's report for
    /// real and fails when the output does not change.
    // trace:BUG-1739 | ai:claude
    pub(crate) fn contributions(&self) -> Vec<(&'static str, usize)> {
        vec![
            ("prs", self.mergeable_prs.len()),
            ("idle_eligible", self.idle_eligible.len()),
            // trace:BUG-2426 | ai:claude
            ("review_watch_skipped", self.review_watch_skipped.len()),
            (
                "recusal_holds",
                self.recusal_holds
                    .iter()
                    .filter(|h| h.is_actionable_for_current_principal())
                    .count(),
            ),
            ("held_prs", self.held_prs.len()),
            ("unowned_failing_prs", self.unowned_failing_prs.len()),
            ("briefs", self.pending_briefs.len()),
            ("findings", usize::from(self.findings_total > 0)),
            ("mail_unread", usize::from(self.mail.unread > 0)),
            // trace:BUG-B6B1278890774EF993D2EBC327862470-040 | ai:claude —
            // role mail is the seat's direct mail; overdue requests are items.
            ("mail_role_unread", usize::from(self.mail.role_unread > 0)),
            ("mail_overdue_requests", self.mail.overdue_requests.len()),
            // trace:BUG-767 | ai:claude — the shared-inbox line is its own row.
            (
                "mail_shared_unread",
                usize::from(self.mail.shared_unread > 0),
            ),
            // trace:STORY-1843 | ai:claude — a partial view is never caught up.
            ("mail_partial", usize::from(self.mail.partial)),
            (
                "directives_pending",
                usize::from(self.worker_directives.pending > 0),
            ),
            // trace:STORY-1226 | ai:claude — due seat jobs are one row.
            ("cron_due", usize::from(self.cron.due > 0)),
            ("unshipped_work", self.unshipped_work.len()),
            ("rework_ready", self.rework_ready.len()),
            ("stale_approvals", self.stale_approvals.len()),
            ("blocked_reviews", self.blocked_reviews.len()),
            ("nightly_red", usize::from(self.nightly_red.is_some())),
            ("reviewer", self.reviewer_queue_items.len()),
            // BUG-1508 AC4/AC7: the "actionable N of M routed" summary line is
            // its own row. It is a SCALAR, not a table: agent mode already
            // printed `reviewer[N]`, so before BUG-1739 this row made the
            // total exceed the rendered rows by one whenever review was routed.
            (
                "reviewer_actionable",
                usize::from(!self.reviewer_queue_items.is_empty()),
            ),
            ("shelved", usize::from(self.shelved_total > 0)),
            ("escalations", self.escalations.len()),
            (
                "pr_attribution_disagreements",
                self.pr_attribution_disagreements.len(),
            ),
            ("orphaned_in_progress", self.orphaned_in_progress.len()),
            // trace:STORY-1680 | ai:claude
            ("idle_in_progress", self.idle_in_progress.len()),
            // BUG-1288: a truncated unshipped-work scan is its own line — it
            // must count toward `total()` too, or a quiet-looking report (0
            // items found, scan incomplete) would hit the is_empty() fast path
            // and hide the one honesty signal that says "not actually verified
            // clean." PRIN-5. Rendered as the `+` suffix on `unshipped:`.
            (
                "unshipped_scan_incomplete",
                usize::from(self.unshipped_work_scan.is_some_and(|scan| !scan.complete)),
            ),
            // trace:TASK-1454 | ai:claude
            ("blocked_seats", self.blocked_seats.len()),
            // trace:STORY-B6B1278890774EF993D2EBC327862470-155 | ai:claude
            ("deputy_failed", self.deputy_failed.len()),
        ]
    }

    /// Count of *lines* this report will render. Drives the `(N)` in the
    /// section header and the empty-report short-circuit. The sum of
    /// [`Self::contributions`] — do not reintroduce a second hand-written
    /// arithmetic here (BUG-1739).
    pub fn total(&self) -> usize {
        self.contributions().iter().map(|(_, n)| n).sum()
    }

    /// BUG-1739: the agent-mode (TOON) view, as a string so a test can assert
    /// what it actually emits. It lived inline in the `aida awaiting` command
    /// arm and could therefore only be tested by reading source as text, which
    /// is why nine contributors drifted out of it unnoticed.
    ///
    /// INVARIANT: every key in [`Self::contributions`] must be observable here.
    /// `agent_view_renders_every_contribution` proves it by rendering each
    /// contributor for real. Adding a contributor without an emission fails it.
    // trace:BUG-1739 | ai:claude
    pub(crate) fn render_agent_view(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        // BUG-695: honor AIDA_AGENT_OUTPUT like `aida integrate`/`ps`/`status` —
        // emit token-efficient TOON (flat scalars + uniform tables) instead of the
        // human box-drawing header + status emoji, which waste agent tokens and
        // ignore the `[ui] glyphs` profile. `--json` above still wins for
        // structured consumers; the `--notice` compact path returned earlier.
        // trace:BUG-695 | ai:claude
        let _ = writeln!(out, "view: awaiting");
        let _ = writeln!(out, "awaiting: {}", self.total());
        let _ = writeln!(out, "findings: {}", self.findings_total);
        let _ = writeln!(out, "mail_unread: {}", self.mail.unread);
        let _ = writeln!(out, "mail_urgent: {}", self.mail.urgent);
        // BUG-767: shared role/agent-type inboxes on their own labelled keys —
        // `mail_unread` stays the operator's own, deterministic count.
        // trace:BUG-767 | ai:claude
        // trace:BUG-B6B1278890774EF993D2EBC327862470-040 | ai:claude
        let _ = writeln!(out, "mail_role_unread: {}", self.mail.role_unread);
        let _ = writeln!(
            out,
            "mail_role: {}",
            self.mail.role.as_deref().unwrap_or("-")
        );
        let _ = writeln!(
            out,
            "mail_overdue_requests: {}",
            self.mail.overdue_requests.len()
        );
        let _ = writeln!(out, "mail_stale_automated: {}", self.mail.stale_automated);
        let _ = writeln!(out, "mail_shared_unread: {}", self.mail.shared_unread);
        let _ = writeln!(out, "mail_partial: {}", self.mail.partial);
        let _ = writeln!(
            out,
            "mail_shared_scope: {}",
            self.mail.shared_scope.as_deref().unwrap_or("-")
        );
        // trace:TASK-1146 | ai:claude
        let _ = writeln!(
            out,
            "directives_pending: {}",
            self.worker_directives.pending
        );
        let _ = writeln!(
            out,
            "directives_next: {}",
            self.worker_directives.next.as_deref().unwrap_or("-")
        );
        // BUG-1288: a truncated scan means the count below is a LOWER
        // BOUND, not the whole answer — the `+` suffix says so instead of
        // letting an agent read a bare number as exhaustive. PRIN-5.
        // trace:BUG-1288 | ai:claude
        let unshipped_suffix = self
            .unshipped_work_scan
            .as_ref()
            .filter(|scan| !scan.complete)
            .map(|_| "+")
            .unwrap_or("");
        let _ = writeln!(
            out,
            "unshipped: {}{}",
            self.unshipped_work.len(),
            unshipped_suffix
        );
        let _ = writeln!(
            out,
            "nightly_red: {}",
            self.nightly_red
                .as_ref()
                .map(|n| n.summary.as_str())
                .unwrap_or("-")
        );
        // BUG-1739: three SCALAR contributors `total()` counts that agent mode
        // printed nothing for. `cron_due` and `shelved` get their own keys in
        // the `mail_unread`/`directives_pending` idiom; `reviewer_actionable`
        // is the BUG-1508 AC4/AC7 summary row, which `total()` adds on top of
        // the `reviewer[N]` table and which therefore made the headline exceed
        // the rendered rows by one whenever review was routed here.
        // trace:BUG-1739 | ai:claude
        let _ = writeln!(out, "cron_due: {}", self.cron.due);
        let _ = writeln!(out, "shelved: {}", self.shelved_total);
        let _ = writeln!(
            out,
            "reviewer_actionable: {} of {} routed",
            self.reviewer_actionable_count(),
            self.reviewer_queue_items.len()
        );
        // trace:BUG-2426 | ai:claude
        let skipped: Vec<Vec<String>> = self
            .review_watch_skipped
            .iter()
            .map(|p| {
                vec![
                    p.change.clone(),
                    p.short_head().to_string(),
                    p.reason.clone(),
                    p.since_label(),
                ]
            })
            .collect();
        let _ = writeln!(
            out,
            "{}",
            crate::toon::table_raw(
                "review_watch_skipped",
                &["change", "head", "reason", "since"],
                &skipped
            )
        );
        let idle: Vec<Vec<String>> = self
            .idle_eligible
            .iter()
            .map(|p| {
                vec![
                    p.pr.to_string(),
                    p.state.clone(),
                    p.age_label(),
                    p.next_command.clone(),
                ]
            })
            .collect();
        let _ = writeln!(
            out,
            "{}",
            crate::toon::table_raw(
                "idle_eligible",
                &["pr", "state", "age", "next_command"],
                &idle
            )
        );
        let prs: Vec<Vec<String>> = self
            .mergeable_prs
            .iter()
            .map(|p| {
                vec![
                    p.number.to_string(),
                    p.title.clone(),
                    // "?" = CI unknown/not-set, matching the codebase convention
                    // (status_cleanup.rs renders the same column with unwrap_or("?")).
                    p.ci_rollup.clone().unwrap_or_else(|| "?".to_string()),
                ]
            })
            .collect();
        let _ = writeln!(
            out,
            "{}",
            crate::toon::table_raw("prs", &["number", "title", "ci"], &prs)
        );
        // trace:STORY-1397 | ai:codex
        let recusal_holds: Vec<Vec<String>> = self
            .recusal_holds
            .iter()
            .map(|h| {
                vec![
                    h.pr.to_string(),
                    h.head_sha.clone().unwrap_or_else(|| "?".into()),
                    h.state.clone(),
                    h.relationship.clone(),
                    h.action.clone(),
                ]
            })
            .collect();
        let _ = writeln!(
            out,
            "{}",
            crate::toon::table_raw(
                "recusal_holds",
                &["pr", "head", "state", "relationship", "action"],
                &recusal_holds,
            )
        );
        // trace:STORY-1397 | ai:claude
        let held_prs: Vec<Vec<String>> = self
            .held_prs
            .iter()
            .map(|h| {
                vec![
                    h.pr.to_string(),
                    h.reason_kind.clone(),
                    h.detail.clone(),
                    h.action.clone(),
                ]
            })
            .collect();
        let _ = writeln!(
            out,
            "{}",
            crate::toon::table_raw(
                "held_prs",
                &["pr", "reason_kind", "detail", "action"],
                &held_prs,
            )
        );
        // trace:TASK-192 | ai:codex
        let broken_prs = unowned_failing_pr_toon_rows(&self.unowned_failing_prs);
        let _ = writeln!(
            out,
            "{}",
            crate::toon::table_raw(
                "unowned_failing_prs",
                &["number", "title", "branch", "action"],
                &broken_prs,
            )
        );
        let briefs: Vec<Vec<String>> = self
            .pending_briefs
            .iter()
            .map(|b| vec![b.agent.clone(), b.spec_id.clone()])
            .collect();
        let _ = writeln!(
            out,
            "{}",
            crate::toon::table_raw("briefs", &["agent", "spec"], &briefs)
        );
        // trace:STORY-1043 | ai:codex
        let unshipped: Vec<Vec<String>> = self
            .unshipped_work
            .iter()
            .map(|u| {
                vec![
                    u.spec_id.clone(),
                    u.branch.clone(),
                    u.commits_ahead.to_string(),
                    u.age.clone(),
                    u.recovery.clone(),
                ]
            })
            .collect();
        let _ = writeln!(
            out,
            "{}",
            crate::toon::table_raw(
                "unshipped_work",
                &["spec", "branch", "ahead", "age", "recovery"],
                &unshipped
            )
        );
        let reviewer: Vec<Vec<String>> = self
            .reviewer_queue_items
            .iter()
            .map(|r| vec![r.spec_id.clone(), r.title.clone()])
            .collect();
        let _ = writeln!(
            out,
            "{}",
            crate::toon::table_raw("reviewer", &["spec", "title"], &reviewer)
        );
        let escalations: Vec<Vec<String>> = self
            .escalations
            .iter()
            .map(|e| vec![e.spec_id.clone(), e.title.clone()])
            .collect();
        let _ = writeln!(
            out,
            "{}",
            crate::toon::table_raw("escalations", &["spec", "title"], &escalations)
        );
        // BUG-1739: the six LIST-shaped contributors `total()` counts that
        // agent mode never rendered. `orphaned_in_progress` is the one that was
        // actively hiding work: three specs stranded In-Progress for 45-64
        // hours in this repository were inside `awaiting:` and printed nowhere,
        // and three relay sessions in a row failed to diagnose the gap from
        // this output. Empty tables still print, exactly like `prs[0]` above --
        // an agent must be able to SEE that a class is empty rather than infer
        // it from a total that does not add up. PRIN-5.
        // trace:BUG-1739 | ai:claude
        let _ = writeln!(
            out,
            "{}",
            crate::toon::table_raw(
                "orphaned_in_progress",
                &["spec", "state", "since", "action"],
                &orphaned_in_progress_toon_rows(&self.orphaned_in_progress),
            )
        );
        // trace:STORY-1680 | ai:claude
        let _ = writeln!(
            out,
            "{}",
            crate::toon::table_raw(
                "idle_in_progress",
                &["spec", "idle", "age_source", "branch", "threshold_hours"],
                &crate::idle_in_progress::toon_rows(&self.idle_in_progress),
            )
        );
        let _ = writeln!(
            out,
            "{}",
            crate::toon::table_raw(
                "blocked_seats",
                &["session", "spec", "tool", "since"],
                &blocked_seat_toon_rows(&self.blocked_seats),
            )
        );
        // trace:STORY-B6B1278890774EF993D2EBC327862470-155 | ai:claude
        let _ = writeln!(
            out,
            "{}",
            crate::toon::table_raw(
                "deputy_failed",
                &[
                    "job",
                    "deputy",
                    "kind",
                    "requester",
                    "rc",
                    "reason",
                    "log",
                    "ack"
                ],
                &deputy_failed_toon_rows(&self.deputy_failed),
            )
        );
        let _ = writeln!(
            out,
            "{}",
            crate::toon::table_raw(
                "stale_approvals",
                &["pr", "spec", "reviewed", "head", "reason"],
                &stale_approval_toon_rows(&self.stale_approvals),
            )
        );
        let _ = writeln!(
            out,
            "{}",
            crate::toon::table_raw(
                "blocked_reviews",
                &["pr", "spec", "reviewed", "head", "reason"],
                &blocked_review_toon_rows(&self.blocked_reviews),
            )
        );
        let _ = writeln!(
            out,
            "{}",
            crate::toon::table_raw(
                "rework_ready",
                &["pr", "spec", "reviewed", "head"],
                &rework_ready_toon_rows(&self.rework_ready),
            )
        );
        let _ = writeln!(
            out,
            "{}",
            crate::toon::table_raw(
                "pr_attribution_disagreements",
                &["pr", "lease_spec", "trailer_spec"],
                &pr_attribution_toon_rows(&self.pr_attribution_disagreements),
            )
        );
        out
    }

    /// BUG-1508 AC4/AC7: how many routed reviewer items actually need this
    /// seat's verdict. One classifier, three readers (the human render, the
    /// agent render, `to_json`) — it was inlined at each before BUG-1739.
    // trace:BUG-1739 | ai:claude
    pub(crate) fn reviewer_actionable_count(&self) -> usize {
        self.reviewer_queue_items
            .iter()
            .filter(|q| q.state == crate::gate::review_verdict::ReviewActionability::NeedsReview)
            .count()
    }

    pub fn is_empty(&self) -> bool {
        self.total() == 0
    }

    /// Render the section. Returns `Ok(true)` when something was written
    /// (caller can blank-line spacing accordingly), `Ok(false)` on the
    /// hide-when-empty fast path. `verbose` lifts the cap from
    /// [`DEFAULT_CAP`] to "every item."
    pub fn render(&self, verbose: bool, mut w: impl Write) -> std::io::Result<bool> {
        if self.is_empty() {
            return Ok(false);
        }
        let cap = if verbose { usize::MAX } else { DEFAULT_CAP };

        writeln!(
            w,
            "{}",
            format!("─── Awaiting you ({}) ───", self.total())
                .bold()
                .yellow()
        )?;

        let mut budget = cap;
        let mut overflow = 0usize;
        // BUG-1530: scope the seat-owned channels (findings/shelved/reviewer)
        // to the reading seat. `hidden_other_seat` accumulates the count of
        // items folded away this way so the report says what it hid instead
        // of silently dropping it (PRIN-5).
        let seat = classify_seat(self.role.as_deref());
        let show_findings = owned_channel_visible(seat, AwaitingSeat::Advisor);
        let show_shelved = owned_channel_visible(seat, AwaitingSeat::Implementer);
        let show_reviewer = owned_channel_visible(seat, AwaitingSeat::Reviewer);
        let mut hidden_other_seat = 0usize;

        // Order: PRs first (most actionable — the unblocked-merge case),
        // then briefs (handoffs you owe), then findings line (a triage
        // pointer rather than a per-finding list), then the mail line,
        // then the worker-directives line (another collapsed breadcrumb),
        // then reviewer-queue items, then escalations.
        // STORY-1419: rework-ready first — a reviewer who refused is the gate,
        // and the round cannot move until they look. Deliberately reports BOTH
        // shas and does not say whether the move was a rebase or a rework; that
        // judgement is the reviewer's and inferring it is unreliable.
        // BUG-1549: a stale APPROVAL is rendered first — it is the one that
        // can be MERGED (a stale refusal just wastes a re-review round), so
        // it outranks even the STORY-1419 rework-ready row below it.
        // trace:BUG-1549 | ai:claude
        // TASK-1454: a seat confirmed blocked on YOUR approval, right now,
        // renders first of everything — this is BUG-1553's whole point: the
        // work is stopped and the fix is one keystroke by a person who may
        // not know they're the blocker. Ground truth (a Notification-hook
        // marker), not a heuristic, so it earns top billing ahead of even
        // the PR-review channels below.
        // trace:TASK-1454 | ai:claude
        for item in &self.blocked_seats {
            if budget == 0 {
                overflow += 1;
                continue;
            }
            // trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
            let scope = item
                .spec
                .as_deref()
                .map(shown_spec)
                .unwrap_or_else(|| item.session_id.clone());
            let tool = item.tool.as_deref().unwrap_or("a tool call");
            writeln!(
                w,
                "  {} {} blocked on your approval: {} — waiting {} — `{}`",
                crate::glyph(crate::glyphs::Glyph::Blocked).red(),
                scope.bold(),
                tool,
                item.since_label,
                "aida ps".cyan(),
            )?;
            budget -= 1;
        }
        // A deputy job submitted on your behalf failed: rc, who asked, and
        // where its log is, until it is acknowledged.
        // trace:STORY-B6B1278890774EF993D2EBC327862470-155 | ai:claude
        for job in &self.deputy_failed {
            if budget == 0 {
                overflow += 1;
                continue;
            }
            let short: String = job.id.chars().take(8).collect();
            // trace:STORY-2338 | ai:claude
            writeln!(
                w,
                "  {} deputy {} {} failed: rc {} ({}) — requested by {} — log {} — `{}`",
                crate::glyph(crate::glyphs::Glyph::Blocked).red(),
                job.kind_label(),
                short.bold(),
                job.rc,
                job.reason,
                job.requester_session
                    .as_deref()
                    .unwrap_or("an unknown session"),
                job.output.display(),
                job.ack_command().cyan(),
            )?;
            budget -= 1;
        }

        // trace:BUG-1549 | ai:claude — a live refusal is why the PR is not in
        // the mergeable list; say so rather than letting it silently vanish.
        for item in &self.blocked_reviews {
            if budget == 0 {
                overflow += 1;
                continue;
            }
            let spec = item
                .spec
                .as_deref()
                .map(|s| format!(" ({})", shown_spec(s)))
                .unwrap_or_default();
            // trace:TASK-1310 | ai:claude — how long the refusal has stood
            // with no subsequent rework, reused from the same verdict data
            // classify_pr_review already read; "age unknown" (PRIN-5) when
            // recorded_at was never captured.
            let age = match item.reason {
                BlockedReason::AtHead => {
                    blocked_age_label(item.recorded_at.as_deref(), chrono::Utc::now())
                }
                BlockedReason::Unverifiable => {
                    blocked_age_label_unverifiable(item.recorded_at.as_deref(), chrono::Utc::now())
                }
            };
            match item.reason {
                BlockedReason::AtHead => writeln!(
                    w,
                    "  {} PR-{}{} blocked — changes requested at the current head {} — {}",
                    "⛔".red(),
                    item.pr.to_string().bold(),
                    spec,
                    short_sha_for_row(&item.head_sha).bold(),
                    age,
                )?,
                BlockedReason::Unverifiable => writeln!(
                    w,
                    "  {} PR-{}{} blocked — a refusal cannot be checked against the head ({}) — {}",
                    "⛔".red(),
                    item.pr.to_string().bold(),
                    spec,
                    if item.reviewed_sha.is_empty() {
                        "no reviewed sha was recorded".to_string()
                    } else if item.head_sha.is_empty() {
                        "the PR head is unknown".to_string()
                    } else {
                        format!(
                            "recorded sha {} is too short to compare",
                            short_sha_for_row(&item.reviewed_sha).dimmed()
                        )
                    },
                    age,
                )?,
            }
            budget -= 1;
        }

        for item in &self.stale_approvals {
            if budget == 0 {
                overflow += 1;
                continue;
            }
            let spec = item
                .spec
                .as_deref()
                .map(|s| format!(" ({})", shown_spec(s)))
                .unwrap_or_default();
            match item.reason {
                StaleApprovalReason::Stale => writeln!(
                    w,
                    "  {} PR-{}{} moved past your approval — reviewed {}, now {} — do not merge on the old verdict",
                    "🛑".red(),
                    item.pr.to_string().bold(),
                    spec,
                    short_sha_for_row(&item.reviewed_sha).dimmed(),
                    short_sha_for_row(&item.head_sha).bold(),
                )?,
                StaleApprovalReason::Unverifiable => writeln!(
                    w,
                    "  {} PR-{}{} approval cannot be verified — {} — do not merge on the old verdict",
                    "🛑".red(),
                    item.pr.to_string().bold(),
                    spec,
                    if item.reviewed_sha.is_empty() {
                        "no reviewed sha was recorded".to_string()
                    } else if item.head_sha.is_empty() {
                        "the PR head is unknown".to_string()
                    } else {
                        format!(
                            "recorded sha {} is too short to compare against {}",
                            short_sha_for_row(&item.reviewed_sha).dimmed(),
                            short_sha_for_row(&item.head_sha).bold(),
                        )
                    },
                )?,
            }
            budget -= 1;
        }

        // trace:STORY-1419 | ai:claude
        for item in &self.rework_ready {
            if budget == 0 {
                overflow += 1;
                continue;
            }
            let spec = item
                .spec
                .as_deref()
                .map(|s| format!(" ({})", shown_spec(s)))
                .unwrap_or_default();
            // trace:STORY-1420 | ai:claude — an inherited row names the
            // exited recorder instead of claiming the refusal was yours.
            let whose = match item.inherited_from.as_deref() {
                Some(who) => format!("a refusal by {who} (recorder exited)"),
                None => "your refusal".to_string(),
            };
            writeln!(
                w,
                "  {} PR-{}{} moved past {} — reviewed {}, now {}",
                "🔄".cyan(),
                item.pr.to_string().bold(),
                spec,
                whose,
                short_sha_for_row(&item.reviewed_sha).dimmed(),
                short_sha_for_row(&item.head_sha).bold(),
            )?;
            budget -= 1;
        }

        // trace:TASK-1445 | ai:claude
        for item in &self.pr_attribution_disagreements {
            if budget == 0 {
                overflow += 1;
                continue;
            }
            writeln!(
                w,
                "  {} PR-{} attribution split — lease says {}, commit trailers say {}",
                "⚠️".yellow(),
                item.pr.to_string().bold(),
                shown_spec(&item.lease_spec).bold(),
                shown_spec(&item.trailer_spec).bold(),
            )?;
            budget -= 1;
        }

        // trace:BUG-2426 | ai:claude
        for item in &self.review_watch_skipped {
            if budget == 0 {
                overflow += 1;
                continue;
            }
            writeln!(
                w,
                "  review watch skips {} at {} — {} (since {})",
                item.change.bold(),
                item.short_head(),
                item.reason,
                item.since_label()
            )?;
            budget -= 1;
        }
        // trace:STORY-2336 | ai:codex
        for item in &self.idle_eligible {
            if budget == 0 {
                overflow += 1;
                continue;
            }
            writeln!(
                w,
                "  idle_eligible PR-{} {} — {} → {}",
                item.pr,
                item.state,
                item.age_label(),
                item.next_command
            )?;
            budget -= 1;
        }
        for pr in &self.mergeable_prs {
            if budget == 0 {
                overflow += 1;
                continue;
            }
            let ci = match pr.ci_rollup.as_deref() {
                Some("pass") => "CI green".green().to_string(),
                None | Some("?") => "no CI".dimmed().to_string(),
                Some(other) => other.dimmed().to_string(),
            };
            // trace:STORY-1405 | ai:claude
            let review = pr
                .under_review
                .as_deref()
                .map(|d| format!(" · {}", format!("under review — {d}").yellow()))
                .unwrap_or_default();
            writeln!(
                w,
                "  {} PR-{} ready to merge — {} · {}{}",
                "🟢".green(),
                pr.number.to_string().bold(),
                pr.title,
                ci,
                review,
            )?;
            budget -= 1;
        }
        for hold in &self.recusal_holds {
            if budget == 0 {
                overflow += 1;
                continue;
            }
            writeln!(
                w,
                "  🛑 PR-{} recusal hold [{}] — {}",
                hold.pr.to_string().bold(),
                hold.relationship,
                hold.action
            )?;
            budget -= 1;
        }
        // trace:STORY-1397 | ai:claude
        for hold in &self.held_prs {
            if budget == 0 {
                overflow += 1;
                continue;
            }
            writeln!(
                w,
                "  ⏸ PR-{} held [{}] — {} · {} · {}",
                hold.pr.to_string().bold(),
                hold.reason_kind,
                hold.title,
                hold.detail,
                hold.action
            )?;
            budget -= 1;
        }
        for pr in &self.unowned_failing_prs {
            if budget == 0 {
                overflow += 1;
                continue;
            }
            // trace:BUG-1514 | ai:claude — name WHY it's stuck (acceptance #5).
            let reason = match &pr.done_spec {
                Some(spec) => format!("{} is marked Done but its CI is failing", shown_spec(spec)),
                None => "CI failing with no owner or route".to_string(),
            };
            writeln!(
                w,
                "  {} PR-{} {} — {} · inspect `{}`",
                "🔴".red(),
                pr.number.to_string().bold(),
                reason,
                pr.title,
                format!("gh pr checks {}", pr.number).cyan(),
            )?;
            budget -= 1;
        }
        for b in &self.pending_briefs {
            if budget == 0 {
                overflow += 1;
                continue;
            }
            let id = if b.spec_id.is_empty() {
                String::new()
            } else {
                format!(" {}", shown_spec(&b.spec_id).bold())
            };
            writeln!(
                w,
                "  📬 brief filed for {}:{} {}",
                b.agent.cyan(),
                id,
                b.path.display().to_string().dimmed(),
            )?;
            budget -= 1;
        }
        // trace:BUG-1530 | ai:claude — findings triage is advisor authority.
        if self.findings_total > 0 && !show_findings {
            hidden_other_seat += self.findings_total;
        } else if self.findings_total > 0 {
            if budget == 0 {
                overflow += 1;
            } else {
                writeln!(
                    w,
                    "  🔍 {} finding{} awaiting triage — `{}`",
                    self.findings_total,
                    if self.findings_total == 1 { "" } else { "s" },
                    "aida findings list".cyan(),
                )?;
                budget -= 1;
            }
        }
        if self.mail.unread > 0 {
            if budget == 0 {
                overflow += 1;
            } else {
                let urgent = urgent_suffix(self.mail.own_urgent());
                writeln!(
                    w,
                    "  {} {} unread mail{}{} — `{}`",
                    crate::glyph(crate::glyphs::Glyph::IncomingMail),
                    self.mail.unread,
                    if self.mail.unread == 1 { "" } else { "s" },
                    urgent,
                    "aida mailbox inbox".cyan(),
                )?;
                budget -= 1;
            }
        }
        // BUG-HOME-040: a [request] left unread past the threshold is an item
        // for the addressed seat, named by sender and subject.
        // trace:BUG-B6B1278890774EF993D2EBC327862470-040 | ai:claude
        for req in &self.mail.overdue_requests {
            if budget == 0 {
                overflow += 1;
            } else {
                writeln!(
                    w,
                    "  {} unanswered {} — `{}`",
                    crate::glyph(crate::glyphs::Glyph::Warning),
                    req.line(),
                    "aida mailbox inbox".cyan(),
                )?;
                budget -= 1;
            }
        }
        // trace:BUG-B6B1278890774EF993D2EBC327862470-040 | ai:claude
        if self.mail.role_unread > 0 {
            if budget == 0 {
                overflow += 1;
            } else {
                let role = self.mail.role.as_deref().unwrap_or("your role");
                writeln!(
                    w,
                    "  {} {} unread mail to {}{} — `{}`",
                    crate::glyph(crate::glyphs::Glyph::IncomingMail),
                    self.mail.role_unread,
                    role,
                    urgent_suffix(self.mail.role_urgent),
                    format!("aida mailbox inbox {role}").cyan(),
                )?;
                budget -= 1;
            }
        }
        // BUG-767: the SHARED role / agent-type inboxes get their own labelled
        // line — kept visible (an advisor still sees its role hand-offs) but
        // never folded into the operator's own unread number.
        // trace:BUG-767 | ai:claude
        if self.mail.shared_unread > 0 {
            if budget == 0 {
                overflow += 1;
            } else {
                let scope = self.mail.shared_scope.as_deref().unwrap_or("shared");
                let first = scope.split(" + ").next().unwrap_or(scope);
                writeln!(
                    w,
                    "  {} {} unread mail in the shared {} inbox — `{}`",
                    crate::glyph(crate::glyphs::Glyph::IncomingMail),
                    self.mail.shared_unread,
                    scope,
                    format!("aida mailbox inbox {first}").cyan(),
                )?;
                budget -= 1;
            }
        }
        // Old automated notices: one line, never counted as new mail.
        // trace:BUG-B6B1278890774EF993D2EBC327862470-040 | ai:claude
        if self.mail.stale_automated > 0 {
            if budget == 0 {
                overflow += 1;
            } else {
                writeln!(w, "  {}", self.stale_automated_line())?;
                budget -= 1;
            }
        }
        // trace:STORY-1843 | ai:claude
        if self.mail.partial {
            if budget == 0 {
                overflow += 1;
            } else {
                writeln!(
                    w,
                    "  {} mail view partial: some mail may be missing — `{}`",
                    crate::glyph(crate::glyphs::Glyph::Warning),
                    "aida mailbox inbox".cyan(),
                )?;
                budget -= 1;
            }
        }
        // trace:TASK-1146 | ai:claude
        if self.worker_directives.pending > 0 {
            if budget == 0 {
                overflow += 1;
            } else {
                let next = match self.worker_directives.next.as_deref() {
                    Some(n) if !n.is_empty() => format!(" (next: {n})"),
                    _ => String::new(),
                };
                writeln!(
                    w,
                    "  ⚙️ {} pending worker directive{}{} — `{}`",
                    self.worker_directives.pending,
                    if self.worker_directives.pending == 1 {
                        ""
                    } else {
                        "s"
                    },
                    next,
                    "aida worker directives".cyan(),
                )?;
                budget -= 1;
            }
        }
        // trace:STORY-1226 | ai:claude
        if self.cron.due > 0 {
            if budget == 0 {
                overflow += 1;
            } else {
                let next = match self.cron.next.as_deref() {
                    Some(n) if !n.is_empty() => format!(" — {n}"),
                    _ => String::new(),
                };
                writeln!(
                    w,
                    "  ⏰ {} due seat job{}{} — `{}`",
                    self.cron.due,
                    if self.cron.due == 1 { "" } else { "s" },
                    next,
                    "aida schedule due".cyan(),
                )?;
                budget -= 1;
            }
        }
        // trace:STORY-1043 | ai:codex
        for item in &self.unshipped_work {
            if budget == 0 {
                overflow += 1;
                continue;
            }
            // trace:TASK-1305 | ai:claude — route through the shared hint so a
            // branch with an open PR is never told to open one.
            // BUG-1530: unshipped work isn't scoped to any one seat — label
            // it as such once a seat is known, rather than implying it's
            // this seat's own gate.
            let scope_note = if seat.is_some() {
                " (project-wide)"
            } else {
                ""
            };
            // BUG-1756: a never-pushed branch exists on exactly one machine;
            // say so instead of rendering it identically to a pushed branch
            // that merely lacks a PR. trace:BUG-1756 | ai:claude
            let push_note = if item.pushed {
                ""
            } else {
                ", local-only (never pushed — this machine holds the only copy)"
            };
            writeln!(
                w,
                "  🧭 unshipped work{}: {} on `{}` — {} commit{} ahead, age {}, PR {}{} — `{}`",
                scope_note,
                shown_spec(&item.spec_id).bold(),
                item.branch,
                item.commits_ahead,
                if item.commits_ahead == 1 { "" } else { "s" },
                item.age,
                item.pr_state,
                push_note,
                unshipped_work_recovery_hint(item).cyan(),
            )?;
            budget -= 1;
        }
        // BUG-1288: an incomplete scan means "nothing reported" is not the
        // same as "nothing exists" — the wall-clock budget ran out before
        // every candidate branch was checked. Unconditional (not
        // budget-gated like the list above): a quiet-looking report during a
        // truncated scan must never read as "verified clean" when it
        // wasn't. PRIN-5.
        // trace:BUG-1288 | ai:claude
        if let Some(scan) = &self.unshipped_work_scan {
            if !scan.complete {
                writeln!(
                    w,
                    "  ⚠️ unshipped scan incomplete ({}/{} candidate branches checked) — re-run for the full picture",
                    scan.scanned, scan.candidates,
                )?;
            }
        }
        // trace:STORY-1043 | ai:codex
        if let Some(item) = &self.nightly_red {
            if budget == 0 {
                overflow += 1;
            } else {
                let inspect = item
                    .run_id
                    .map(|id| format!("gh run view {id}"))
                    .unwrap_or_else(|| "gh run list --workflow cross-platform.yml".to_string());
                writeln!(w, "  🔴 {} — `{}`", item.summary, inspect.cyan())?;
                budget -= 1;
            }
        }
        // BUG-1508 AC4/AC7: the summary line carries BOTH numbers --
        // actionable and routed -- because the gap between them is itself
        // the signal (a queue that reads five-deep on review when four are
        // really awaiting rework misdirects capacity at the wrong seat).
        // trace:BUG-1508 | ai:claude
        // trace:BUG-1530 | ai:claude — reviewer-queue rows are routed to the
        // reviewer seat specifically; fold them into the hidden count for
        // every other seat instead of itemizing.
        if !self.reviewer_queue_items.is_empty() && !show_reviewer {
            hidden_other_seat += self.reviewer_queue_items.len();
        } else if !self.reviewer_queue_items.is_empty() {
            if budget == 0 {
                overflow += 1;
            } else {
                let actionable = self.reviewer_actionable_count();
                writeln!(
                    w,
                    "  🔎 reviewer: actionable {} of {} routed",
                    actionable,
                    self.reviewer_queue_items.len()
                )?;
                budget -= 1;
            }
            // BUG-1508 AC2: every routed row still renders -- an
            // already-reviewed row is never dropped, only annotated, so it
            // reads as rework/resolved rather than silently disappearing.
            for q in &self.reviewer_queue_items {
                if budget == 0 {
                    overflow += 1;
                    continue;
                }
                let (glyph, label) = match q.state {
                    crate::gate::review_verdict::ReviewActionability::NeedsReview => {
                        ("👀", "needs-review")
                    }
                    crate::gate::review_verdict::ReviewActionability::AwaitingRework => {
                        ("🔧", "awaiting-rework")
                    }
                    crate::gate::review_verdict::ReviewActionability::Resolved => {
                        ("✅", "resolved")
                    }
                };
                writeln!(
                    w,
                    "  {} {}: {} — {}",
                    glyph,
                    label,
                    shown_spec(&q.spec_id).bold(),
                    q.title,
                )?;
                budget -= 1;
            }
        }
        // trace:BUG-1530 | ai:claude — a shelved/rework item is implementer
        // work ("queue rework --work"); fold it away for other seats.
        if self.shelved_total > 0 && !show_shelved {
            hidden_other_seat += self.shelved_total;
        } else if self.shelved_total > 0 {
            if budget == 0 {
                overflow += 1;
            } else {
                // TASK-1311: name the one-keystroke requeue next to the
                // shelved count, with the status it lands in. STORY-1429:
                // point at the triage loop, which previews each requeue.
                // trace:TASK-1311 trace:STORY-1429 | ai:claude
                writeln!(
                    w,
                    "  {} {} shelved item{} in rework — triage one keystroke each with `{}` (to Approved) · or `{}`",
                    crate::glyph(crate::glyphs::Glyph::Pause).blue(),
                    self.shelved_total,
                    if self.shelved_total == 1 { "" } else { "s" },
                    "aida rework".cyan(),
                    crate::requeue::requeue_command("<ID>").cyan(),
                )?;
                budget -= 1;
            }
        }
        for e in &self.escalations {
            if budget == 0 {
                overflow += 1;
                continue;
            }
            // trace:TASK-1311 | ai:claude
            writeln!(
                w,
                "  🗣️ escalation: {} — {} · once decided: `{}` (to Approved)",
                shown_spec(&e.spec_id).bold(),
                e.title,
                crate::requeue::requeue_command(&e.spec_id).cyan(),
            )?;
            budget -= 1;
        }
        // trace:BUG-1564 | ai:claude
        for o in &self.orphaned_in_progress {
            if budget == 0 {
                overflow += 1;
                continue;
            }
            // trace:BUG-1739 | ai:claude — one classifier, shared with agent mode.
            let state = orphaned_state_label(o);
            writeln!(
                w,
                "  {} orphaned In-Progress: {} — {}, {} — `{}`",
                "⚠️".yellow(),
                shown_spec(&o.spec_id).bold(),
                state,
                o.since_label,
                "aida ps".cyan(),
            )?;
            budget -= 1;
        }
        // trace:STORY-1680 | ai:claude
        for i in &self.idle_in_progress {
            if budget == 0 {
                overflow += 1;
                continue;
            }
            writeln!(
                w,
                "  {} in-progress but nothing is working it: {} — {} — `{}`",
                "⚠️".yellow(),
                shown_spec(&i.spec_id).bold(),
                i.age_label(),
                "aida ps".cyan(),
            )?;
            budget -= 1;
        }

        if overflow > 0 {
            writeln!(
                w,
                "  {} {} more — `{}`",
                "…".dimmed(),
                overflow,
                "aida status --awaiting --verbose".cyan(),
            )?;
        }
        // trace:BUG-1530 | ai:claude — PRIN-5: say what is hidden. Never
        // silently drop the advisor/implementer/reviewer-owned items folded
        // out of this seat's headline; name the count instead.
        if hidden_other_seat > 0 {
            writeln!(
                w,
                "  {} {} item{} routed to other seats (not shown here)",
                "·".dimmed(),
                hidden_other_seat,
                if hidden_other_seat == 1 { "" } else { "s" },
            )?;
        }
        writeln!(w)?;
        Ok(true)
    }

    /// Machine-readable JSON shape for `--json` consumers. Stable contract:
    /// adding a field is non-breaking; renaming or removing is breaking.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "total": self.total(),
            "idle_eligible": self.idle_eligible,
            // trace:BUG-2426 | ai:claude
            "review_watch_skipped": self.review_watch_skipped,
            "mergeable_prs": self.mergeable_prs.iter().map(|p| serde_json::json!({
                "number": p.number,
                "title": p.title,
                "head_branch": p.head_branch,
                "ci_rollup": p.ci_rollup,
                "under_review": p.under_review,
            })).collect::<Vec<_>>(),
            "recusal_holds": self.recusal_holds,
            "held_prs": self.held_prs,
            "unowned_failing_prs": self.unowned_failing_prs.iter().map(|p| serde_json::json!({
                "number": p.number,
                "title": p.title,
                "head_branch": p.head_branch,
                "ci_rollup": "fail",
                "action": format!("gh pr checks {}", p.number),
                // trace:BUG-1514 | ai:claude
                "done_spec": p.done_spec,
            })).collect::<Vec<_>>(),
            "pending_briefs": self.pending_briefs.iter().map(|b| serde_json::json!({
                "agent": b.agent,
                "spec_id": b.spec_id,
                "path": b.path.display().to_string(),
            })).collect::<Vec<_>>(),
            "findings_total": self.findings_total,
            "mail": {
                "unread": self.mail.unread,
                "urgent": self.mail.urgent,
                // trace:BUG-767 | ai:claude — shared role/agent-type inboxes,
                // reported alongside (never inside) the operator's own count.
                "shared_unread": self.mail.shared_unread,
                "shared_scope": self.mail.shared_scope,
                "partial": self.mail.partial,
                // trace:BUG-B6B1278890774EF993D2EBC327862470-040 | ai:claude
                "role_unread": self.mail.role_unread,
                "role": self.mail.role,
                "previews": self.mail.previews.iter().map(MailPreview::to_json).collect::<Vec<_>>(),
                "overdue_requests": self.mail.overdue_requests.iter().map(MailPreview::to_json).collect::<Vec<_>>(),
                "stale_automated": self.mail.stale_automated,
            },
            "worker_directives": {
                "pending": self.worker_directives.pending,
                "next": self.worker_directives.next,
            },
            // trace:STORY-1226 | ai:claude
            "cron": {
                "due": self.cron.due,
                "next": self.cron.next,
            },
            "unshipped_work": self.unshipped_work.iter().map(|i| serde_json::json!({
                "spec_id": i.spec_id,
                "branch": i.branch,
                "commits_ahead": i.commits_ahead,
                "age": i.age,
                "pr_state": i.pr_state,
                // trace:BUG-1756 | ai:claude — false = local-only: the one
                // machine-durable copy; consumers must not collapse it into
                // the pushed-no-PR state.
                "pushed": i.pushed,
                "recovery": i.recovery,
            })).collect::<Vec<_>>(),
            // BUG-1288: `null` means the scan never ran (the `--notice` fast
            // path); otherwise `complete` says whether every eligible
            // candidate branch was probed before the wall-clock budget
            // expired, so a consumer can tell "no unshipped work" apart from
            // "ran out of time before finishing the check" instead of the
            // two being silently indistinguishable zeros. trace:BUG-1288 | ai:claude
            "unshipped_work_scan": self.unshipped_work_scan.map(|s| serde_json::json!({
                "complete": s.complete,
                "scanned": s.scanned,
                "candidates": s.candidates,
            })),
            "nightly_red": self.nightly_red.as_ref().map(|n| serde_json::json!({
                "summary": n.summary,
                "run_id": n.run_id,
                "nights": n.nights,
            })),
            // trace:BUG-1549 | ai:claude
            // trace:TASK-1310 | ai:claude — `recorded_at` is the raw input and
            // `age_secs`/`overdue` are its derived reading (via `blocked_age`),
            // so a second reader can recompute and falsify the number instead
            // of trusting the label alone.
            "blocked_reviews": self.blocked_reviews.iter().map(|i| {
                let age = blocked_age(i.recorded_at.as_deref(), chrono::Utc::now());
                serde_json::json!({
                    "pr": i.pr,
                    "spec": i.spec,
                    "reviewed_sha": i.reviewed_sha,
                    "head_sha": i.head_sha,
                    "reason": i.reason.as_wire_str(),
                    "recorded_at": i.recorded_at,
                    "age_secs": age.map(|a| a.secs),
                    // Unverifiable: rework-since is unknown, so no overdue claim.
                    "overdue": match i.reason {
                        BlockedReason::AtHead => age.map(|a| a.overdue),
                        BlockedReason::Unverifiable => None,
                    },
                })
            }).collect::<Vec<_>>(),
            "stale_approvals": self.stale_approvals.iter().map(|i| serde_json::json!({
                "pr": i.pr,
                "spec": i.spec,
                "reviewed_sha": i.reviewed_sha,
                "head_sha": i.head_sha,
                "reason": i.reason.as_wire_str(),
            })).collect::<Vec<_>>(),
            "reviewer_queue_items": self.reviewer_queue_items.iter().map(|q| serde_json::json!({
                "spec_id": q.spec_id,
                "title": q.title,
                "state": q.state.as_str(),
            })).collect::<Vec<_>>(),
            // BUG-1508 AC4/AC7: the depth figure that decides reviewer
            // capacity is actionable-of-routed, not a bare routed count.
            "reviewer_actionable_of_routed": {
                "actionable": self.reviewer_actionable_count(),
                "routed": self.reviewer_queue_items.len(),
            },
            "shelved_total": self.shelved_total,
            "escalations": self.escalations.iter().map(|e| serde_json::json!({
                "spec_id": e.spec_id,
                "title": e.title,
                // trace:TASK-1311 | ai:claude
                "requeue": crate::requeue::requeue_command(&e.spec_id),
            })).collect::<Vec<_>>(),
            // trace:TASK-1445 | ai:claude
            "pr_attribution_disagreements": self.pr_attribution_disagreements.iter().map(|d| serde_json::json!({
                "pr": d.pr,
                "lease_spec": d.lease_spec,
                "trailer_spec": d.trailer_spec,
            })).collect::<Vec<_>>(),
            // trace:BUG-1564 | ai:claude
            "orphaned_in_progress": self.orphaned_in_progress.iter().map(|o| serde_json::json!({
                "spec_id": o.spec_id,
                "title": o.title,
                "abandoned": o.abandoned,
                "since_label": o.since_label,
                // trace:BUG-1656 | ai:claude
                "possibly_subagent": o.possibly_subagent,
            })).collect::<Vec<_>>(),
            // trace:STORY-1680 | ai:claude
            "idle_in_progress": crate::idle_in_progress::json_rows(&self.idle_in_progress),
            // trace:TASK-1454 | ai:claude
            "blocked_seats": self.blocked_seats.iter().map(|b| serde_json::json!({
                "session_id": b.session_id,
                "spec": b.spec,
                "tool": b.tool,
                "since_label": b.since_label,
            })).collect::<Vec<_>>(),
            // trace:STORY-B6B1278890774EF993D2EBC327862470-155 | ai:claude
            "deputy_failed": self.deputy_failed.iter().map(|j| serde_json::json!({
                "job": j.id,
                "deputy": j.deputy,
                "kind": j.kind,
                "ack": j.ack_command(),
                "requester_session": j.requester_session,
                "rc": j.rc,
                "reason": j.reason,
                "log": j.output,
            })).collect::<Vec<_>>(),
        })
    }

    /// The lines the per-turn notice prints under [`Self::compact_line`]:
    /// the newest unread mail by kind, sender, age and subject (`[request]`
    /// first), then one summary line for old automated notices. Empty when
    /// the compact line is silent, so a quiet inbox stays quiet.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-040 | ai:claude
    pub fn mail_preview_lines(&self) -> Vec<String> {
        if self.compact_line().is_none() {
            return Vec::new();
        }
        let mut lines: Vec<String> = self
            .mail
            .previews
            .iter()
            .map(|p| {
                format!(
                    "  {} {}",
                    crate::glyph(crate::glyphs::Glyph::IncomingMail),
                    p.line()
                )
            })
            .collect();
        if self.mail.stale_automated > 0 {
            lines.push(format!("  {}", self.stale_automated_line()));
        }
        lines
    }

    fn stale_automated_line(&self) -> String {
        let n = self.mail.stale_automated;
        format!(
            "{} older than {}d from automated senders (not counted) — `aida mailbox inbox`",
            pluralize(n, "unread notice", "unread notices"),
            STALE_AUTOMATED_MAIL_SECS / 86_400
        )
    }

    /// One compact line spanning every populated channel, for the per-turn
    /// notice — the can't-miss signal that SOMETHING awaits across ANY
    /// coordination channel (not just mail). Returns `None` when nothing
    /// awaits so the caller stays silent. Pure: renders only the counts the
    /// caller already gathered — it does NO I/O, so cheapness is the caller's
    /// job (build the report with `no_ci` on the per-turn path so PRs, the one
    /// network-backed channel, are omitted; the full `aida awaiting` includes
    /// them). Example (awaiting-glyph prefix elided): `Awaiting you: 2 briefs ·
    /// 1 finding · 3 mail (1 urgent) · 1 escalation — run `aida awaiting``.
    // trace:STORY-741 | ai:claude
    pub fn compact_line(&self) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        // BUG-1530: scope the seat-owned channels (findings/shelved/reviewer)
        // to the reading seat, same as `render`. `hidden_other_seat` says
        // what was folded away instead of silently dropping it (PRIN-5).
        let seat = classify_seat(self.role.as_deref());
        let show_findings = owned_channel_visible(seat, AwaitingSeat::Advisor);
        let show_shelved = owned_channel_visible(seat, AwaitingSeat::Implementer);
        let show_reviewer = owned_channel_visible(seat, AwaitingSeat::Reviewer);
        let mut hidden_other_seat = 0usize;
        let mut parts: Vec<String> = Vec::new();
        // trace:TASK-1454 | ai:claude — leads the compact line too: a
        // blocked seat is the single most time-sensitive item this notice
        // can carry (work is stopped RIGHT NOW on your keystroke).
        if !self.blocked_seats.is_empty() {
            parts.push(pluralize(
                self.blocked_seats.len(),
                "BLOCKED seat",
                "BLOCKED seats",
            ));
        }
        // trace:STORY-B6B1278890774EF993D2EBC327862470-155 | ai:claude
        if !self.deputy_failed.is_empty() {
            parts.push(pluralize(
                self.deputy_failed.len(),
                "failed deputy job",
                "failed deputy jobs",
            ));
        }
        if !self.idle_eligible.is_empty() {
            parts.push(format!("{} idle eligible", self.idle_eligible.len()));
        }
        // trace:BUG-2426 | ai:claude
        if !self.review_watch_skipped.is_empty() {
            parts.push(format!(
                "{} skipped by review watch",
                self.review_watch_skipped.len()
            ));
        }
        if !self.mergeable_prs.is_empty() {
            parts.push(pluralize(self.mergeable_prs.len(), "PR", "PRs"));
        }
        let actionable_recusals = self
            .recusal_holds
            .iter()
            .filter(|h| h.is_actionable_for_current_principal())
            .count();
        if actionable_recusals > 0 {
            parts.push(format!(
                "{} recusal-hold{}",
                actionable_recusals,
                if actionable_recusals == 1 { "" } else { "s" }
            ));
        }
        // trace:STORY-1397 | ai:claude
        if !self.held_prs.is_empty() {
            parts.push(format!("{} held", self.held_prs.len()));
        }
        if !self.unowned_failing_prs.is_empty() {
            parts.push(format!("{} broken-unowned", self.unowned_failing_prs.len()));
        }
        // trace:BUG-1549 | ai:claude
        if !self.stale_approvals.is_empty() {
            parts.push(format!("{} stale-approval", self.stale_approvals.len()));
        }
        if !self.blocked_reviews.is_empty() {
            parts.push(format!("{} blocked-review", self.blocked_reviews.len()));
        }
        if !self.pending_briefs.is_empty() {
            parts.push(pluralize(self.pending_briefs.len(), "brief", "briefs"));
        }
        // trace:BUG-1530 | ai:claude — findings triage is advisor authority.
        if self.findings_total > 0 {
            if show_findings {
                parts.push(pluralize(self.findings_total, "finding", "findings"));
            } else {
                hidden_other_seat += self.findings_total;
            }
        }
        if self.mail.unread > 0 {
            let urgent = urgent_suffix(self.mail.own_urgent());
            parts.push(format!(
                "{}{}",
                pluralize(self.mail.unread, "mail", "mail"),
                urgent
            ));
        }
        // trace:BUG-B6B1278890774EF993D2EBC327862470-040 | ai:claude — role
        // mail is the seat's own, so it is named for the role, not "shared".
        if !self.mail.overdue_requests.is_empty() {
            parts.push(pluralize(
                self.mail.overdue_requests.len(),
                "overdue request",
                "overdue requests",
            ));
        }
        if self.mail.role_unread > 0 {
            parts.push(format!(
                "{} mail to {}{}",
                self.mail.role_unread,
                self.mail.role.as_deref().unwrap_or("your role"),
                urgent_suffix(self.mail.role_urgent)
            ));
        }
        // trace:BUG-767 | ai:claude
        if self.mail.shared_unread > 0 {
            parts.push(format!("{} shared mail", self.mail.shared_unread));
        }
        // trace:STORY-1843 | ai:claude
        if self.mail.partial {
            parts.push("mail view partial".to_string());
        }
        // trace:TASK-1146 | ai:claude
        if self.worker_directives.pending > 0 {
            parts.push(pluralize(
                self.worker_directives.pending,
                "directive",
                "directives",
            ));
        }
        // trace:STORY-1226 | ai:claude
        if self.cron.due > 0 {
            parts.push(format!(
                "{} due job{}",
                self.cron.due,
                if self.cron.due == 1 { "" } else { "s" }
            ));
        }
        // trace:STORY-1043 | ai:codex
        if !self.unshipped_work.is_empty() {
            // BUG-1530: unshipped work isn't scoped to any one seat — label
            // it as project-wide once a seat is known, rather than implying
            // it's this seat's own gate.
            let label = if seat.is_some() {
                "unshipped (project-wide)"
            } else {
                "unshipped"
            };
            parts.push(format!("{}:{}", label, self.unshipped_work.len()));
        }
        if self.nightly_red.is_some() {
            parts.push("nightly-red".to_string());
        }
        // trace:BUG-1530 | ai:claude — reviewer-queue rows are routed to the
        // reviewer seat specifically.
        if !self.reviewer_queue_items.is_empty() {
            if show_reviewer {
                // BUG-1508 AC4/AC7: "actionable N of M routed" everywhere
                // this depth figure is printed, including the compact
                // per-turn line.
                let actionable = self.reviewer_actionable_count();
                parts.push(format!(
                    "actionable {} of {} routed",
                    actionable,
                    self.reviewer_queue_items.len()
                ));
            } else {
                hidden_other_seat += self.reviewer_queue_items.len();
            }
        }
        // trace:BUG-1530 | ai:claude — a shelved/rework item is implementer
        // work ("queue rework --work").
        if self.shelved_total > 0 {
            if show_shelved {
                parts.push(format!("{} in rework", self.shelved_total));
            } else {
                hidden_other_seat += self.shelved_total;
            }
        }
        if !self.escalations.is_empty() {
            parts.push(pluralize(
                self.escalations.len(),
                "escalation",
                "escalations",
            ));
        }
        // trace:TASK-1445 | ai:claude
        if !self.pr_attribution_disagreements.is_empty() {
            parts.push(format!(
                "{} attribution split",
                self.pr_attribution_disagreements.len()
            ));
        }
        // trace:BUG-1564 | ai:claude
        if !self.orphaned_in_progress.is_empty() {
            parts.push(pluralize(
                self.orphaned_in_progress.len(),
                "orphaned in-progress",
                "orphaned in-progress",
            ));
        }
        // trace:STORY-1680 | ai:claude
        if !self.idle_in_progress.is_empty() {
            parts.push(pluralize(
                self.idle_in_progress.len(),
                "idle in-progress",
                "idle in-progress",
            ));
        }
        // trace:BUG-1530 | ai:claude — PRIN-5: say what is hidden. Never
        // silently drop the advisor/implementer/reviewer-owned items folded
        // out of this seat's headline; name the count instead.
        if hidden_other_seat > 0 {
            parts.push(format!("{hidden_other_seat} for other seats"));
        }
        if parts.is_empty() {
            return None;
        }
        Some(format!(
            "{} Awaiting you: {} — run `aida awaiting`",
            crate::glyph(crate::glyphs::Glyph::Awaiting),
            parts.join(" · ")
        ))
    }
}

/// Row-length sha for the awaiting surface. Purely cosmetic — the comparison
/// that decides a row is done on the full values.
// trace:STORY-1419 | ai:claude
fn short_sha_for_row(sha: &str) -> String {
    sha.chars().take(10).collect()
}

/// `"{n} {singular|plural}"` — the tiny count formatter the compact line uses.
fn pluralize(n: usize, singular: &str, plural: &str) -> String {
    format!("{} {}", n, if n == 1 { singular } else { plural })
}

/// Classify a single open PR as "awaiting you." The aida-chat motivating
/// case (5 mergeable PRs OPEN for hours) lives or dies on this filter:
///   - `mergeable == "MERGEABLE"` (excludes CONFLICTING / UNKNOWN)
///   - CI is not failing, pending, missing a required check, or unknown
///     whether a required check is missing (pass / no-checks / `?` are
///     fine — BUG-1481)
///   - reviewer verdict is not `CHANGES_REQUESTED`
///   - no AIDA-recorded blocking verdict at the PR's current head, and no
///     AIDA-recorded APPROVED verdict that fails to provably cover it
///     (stale, sha-less, or Incomparable — BUG-1549)
///
/// A `REVIEW_REQUIRED` PR still qualifies: if the operator is the only
/// reviewer on a solo project, the human merge button is the only gate.
///
/// `local_verdict_blocks` is a caller-supplied fact (see
/// [`classify_open_prs`]) rather than a filesystem read here: this stays the
/// same pure, git/forge-free classifier the module doc promises. BUG-1490:
/// GitHub's `review_decision` is not the only place a refusal is recorded —
/// `aida review record` writes straight to the AIDA substrate with no forge
/// round trip, so a PR refused that way had `review_decision` empty and kept
/// showing up here as awaiting-you.
// trace:BUG-1490 | ai:claude
pub(crate) fn is_awaiting_you(pr: &OpenPrItem, local_verdict_blocks: bool) -> bool {
    let mergeable = pr.mergeable.as_deref().unwrap_or("");
    if !mergeable.eq_ignore_ascii_case("MERGEABLE") {
        return false;
    }
    match pr.ci_rollup.as_deref() {
        // BUG-1481: "missing" = a required check's row never showed up on
        // this head at all; "unknown" = the required-check set itself could
        // not be determined (branch protection unreadable). Neither is a
        // pass — absent evidence is not good evidence (PRIN-5).
        Some("fail") | Some("pending") | Some("missing") | Some("unknown") => return false,
        _ => {}
    }
    let verdict = pr
        .review_decision
        .as_deref()
        .unwrap_or("")
        .to_ascii_uppercase();
    if verdict == "CHANGES_REQUESTED" {
        return false;
    }
    if local_verdict_blocks {
        return false;
    }
    true
}

/// Filter a snapshot of open PRs down to the "Awaiting you" subset. Used
/// by the renderer and exercised directly in tests.
///
/// `local_blocking` names the PRs (by number) whose `classify_pr_review`
/// decision is suppressed (`local_suppressed_prs`, lib.rs, BUG-1549) — a live
/// refusal, or a newest approval that does not provably cover the head. Each
/// such PR has a blocked-review or stale-approval row explaining it. This set
/// is unioned with GitHub's `review_decision`, never swapped.
// trace:BUG-1490 | ai:claude
// trace:BUG-1549 | ai:claude
pub(crate) fn classify_open_prs(
    prs: &[OpenPrItem],
    local_blocking: &HashSet<u64>,
) -> Vec<MergeablePrItem> {
    prs.iter()
        .filter(|pr| is_awaiting_you(pr, local_blocking.contains(&pr.number)))
        .map(|pr| MergeablePrItem {
            number: pr.number,
            title: pr.title.clone(),
            head_branch: pr.head_branch.clone(),
            ci_rollup: pr.ci_rollup.clone(),
            under_review: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    include!("tests/awaiting_you_tests.rs");
}
