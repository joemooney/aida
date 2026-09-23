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

use crate::review_verdict;
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
    /// Open PRs that pass the awaiting-you gate: `MERGEABLE` + no failing
    /// or pending CI + reviewer verdict is not `CHANGES_REQUESTED`. The
    /// aida-chat motivating case: 5 PRs sat open for hours because the
    /// system was waiting on the human's merge button and nothing said so.
    pub mergeable_prs: Vec<MergeablePrItem>,
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
    use super::*;

    // trace:BUG-1530 | ai:claude
    #[test]
    fn unrecognised_seat_sees_every_owned_channel() {
        for role in ["integrator", "product", "human", "some-new-seat"] {
            let seat = classify_seat(Some(role));
            for owner in [
                AwaitingSeat::Reviewer,
                AwaitingSeat::Advisor,
                AwaitingSeat::Implementer,
            ] {
                assert!(
                    owned_channel_visible(seat, owner),
                    "role {role} must see {owner:?}"
                );
            }
        }
    }

    // trace:BUG-1530 | ai:claude
    #[test]
    fn role_matching_is_case_insensitive() {
        assert_eq!(
            classify_seat(Some("Reviewer")),
            Some(AwaitingSeat::Reviewer)
        );
        assert_eq!(classify_seat(Some("ADVISOR")), Some(AwaitingSeat::Advisor));
        assert!(owned_channel_visible(
            classify_seat(Some("Reviewer")),
            AwaitingSeat::Reviewer
        ));
    }
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
    v: &review_verdict::RecordedVerdict,
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
    candidates: &[review_verdict::RecordedVerdict],
    head: Option<&str>,
) -> PrReviewDecision {
    let rel =
        |v: &review_verdict::RecordedVerdict| review_relation(v.reviewed_sha.as_deref(), head);
    let sha = |v: &review_verdict::RecordedVerdict| {
        v.reviewed_sha.as_deref().unwrap_or("").trim().to_string()
    };
    // BUG-1529: a refusal closed by its PR's merge is history, not a live
    // obstruction for a later PR on the same spec. trace:BUG-1529 | ai:claude
    let refusals: Vec<_> = candidates
        .iter()
        .filter(|v| v.kind.blocks_done() && !v.is_closed())
        .collect();
    let approvals: Vec<_> = candidates
        .iter()
        .filter(|v| v.kind == review_verdict::VerdictKind::Approved)
        .collect();

    let superseded = |r: &review_verdict::RecordedVerdict| -> bool {
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
    /// drain reviewer, or a registered seat whose pid is gone), so the row was
    /// inherited by the spec's owner/implementer — or the advisor/human
    /// bucket when nobody owns it — instead of routed to a seat that can never
    /// receive it. Names the recorder so the reader knows whose refusal it is.
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
/// against `recorded_by`; `role` is the session role the BUG-1530 headline
/// scoping reads.
// trace:STORY-1420 | ai:claude
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ReworkReader<'a> {
    pub identity: Option<&'a str>,
    pub role: Option<&'a str>,
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
            ReworkReader {
                identity: seat,
                role: None,
            },
            |_| RecorderLiveness::Live,
            |_| None,
        )
    }

    /// STORY-1420: [`Self::add`] with the rework-ready row routed by the
    /// recorder's liveness. A LIVE recorder keeps the STORY-1419 routing (the
    /// refusing seat). An EXITED recorder's follow-up is attributed to the
    /// spec's owner/implementer (`spec_owner`), or — when nobody owns it — to
    /// the unowned advisor/human bucket every seat sees. UNKNOWN liveness is
    /// shown to everyone (PRIN-5). The row is never silently dropped for want
    /// of a recipient.
    // trace:STORY-1420 | ai:claude
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn add_routed(
        &mut self,
        pr: u64,
        head_sha: Option<&str>,
        head_branch: &str,
        decision: &PrReviewDecision,
        reader: ReworkReader<'_>,
        liveness: impl Fn(&str) -> RecorderLiveness,
        spec_owner: impl Fn(&str) -> Option<String>,
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
                        RecorderLiveness::Exited => {
                            let owner = spec.as_deref().and_then(&spec_owner);
                            if !exited_rework_visible(reader, owner.as_deref()) {
                                return;
                            }
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

/// STORY-1420: who sees a rework-ready row whose recorder has exited. With an
/// owner/implementer on the spec, it is theirs — plus the advisor (the
/// standing disposition gate) and any seat not scoped at all (no role, or an
/// unrecognised one: BUG-1530's PRIN-5 rule). A reviewer seat that is not the
/// owner does not inherit another reviewer's dead refusal. With NO owner, the
/// row lands in the unowned advisor/human bucket shown to everyone.
// trace:STORY-1420 | ai:claude
fn exited_rework_visible(reader: ReworkReader<'_>, owner: Option<&str>) -> bool {
    let Some(owner) = owner.map(str::trim).filter(|o| !o.is_empty()) else {
        return true;
    };
    if reader
        .identity
        .is_some_and(|me| me.trim().eq_ignore_ascii_case(owner))
    {
        return true;
    }
    let seat = classify_seat(reader.role);
    owned_channel_visible(seat, AwaitingSeat::Implementer)
        || owned_channel_visible(seat, AwaitingSeat::Advisor)
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
    use aida_core::mailbox::unread_for_identities;
    let own = unread_for_identities([operator], messages, watermarks);
    let own_ids: std::collections::HashSet<&str> = own.iter().map(|m| m.id.as_str()).collect();
    let shared_ids: Vec<&str> = shared
        .iter()
        .map(String::as_str)
        .filter(|s| !s.trim().is_empty() && s.trim() != operator.trim())
        .collect();
    let shared_unread = unread_for_identities(shared_ids.iter().copied(), messages, watermarks)
        .into_iter()
        .filter(|m| !own_ids.contains(m.id.as_str()))
        .count();
    MailChannel {
        unread: own.len(),
        urgent: own.iter().filter(|m| m.urgent).count(),
        shared_unread,
        shared_scope: if shared_unread > 0 {
            Some(shared_ids.join(" + "))
        } else {
            None
        },
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

#[derive(Debug, Clone)]
pub(crate) struct PendingBriefItem {
    pub agent: String,
    pub spec_id: String,
    pub path: std::path::PathBuf,
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
    pub state: review_verdict::ReviewActionability,
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
    pub fn total(&self) -> usize {
        self.mergeable_prs.len()
            + self.unowned_failing_prs.len()
            + self.pending_briefs.len()
            + (if self.findings_total > 0 { 1 } else { 0 })
            + (if self.mail.unread > 0 { 1 } else { 0 })
            // trace:BUG-767 | ai:claude — the shared-inbox line is its own row.
            + (if self.mail.shared_unread > 0 { 1 } else { 0 })
            + (if self.worker_directives.pending > 0 {
                1
            } else {
                0
            })
            // trace:STORY-1226 | ai:claude — due seat jobs are one row.
            + (if self.cron.due > 0 { 1 } else { 0 })
            + self.unshipped_work.len()
            + self.rework_ready.len()
            + self.stale_approvals.len()
            + self.blocked_reviews.len()
            + (if self.nightly_red.is_some() { 1 } else { 0 })
            + self.reviewer_queue_items.len()
            // BUG-1508 AC4/AC7: the "actionable N of M routed" summary line.
            + (if !self.reviewer_queue_items.is_empty() { 1 } else { 0 })
            + (if self.shelved_total > 0 { 1 } else { 0 })
            + self.escalations.len()
            + self.pr_attribution_disagreements.len()
            + self.orphaned_in_progress.len()
            // BUG-1288: a truncated unshipped-work scan is its own line
            // (below) — it must count toward `total()` too, or a quiet-
            // looking report (0 items found, scan incomplete) would hit the
            // is_empty() fast path and hide the one honesty signal that
            // says "not actually verified clean." PRIN-5.
            + (if self
                .unshipped_work_scan
                .is_some_and(|scan| !scan.complete)
            {
                1
            } else {
                0
            })
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
                .map(|s| format!(" ({s})"))
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
                .map(|s| format!(" ({s})"))
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
                .map(|s| format!(" ({s})"))
                .unwrap_or_default();
            // trace:STORY-1420 | ai:claude — an inherited row names the
            // exited recorder instead of claiming the refusal was yours.
            let whose = match item.inherited_from.as_deref() {
                Some(who) => format!("a refusal by {who} (recorder exited, now yours)"),
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
                item.lease_spec.bold(),
                item.trailer_spec.bold(),
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
        for pr in &self.unowned_failing_prs {
            if budget == 0 {
                overflow += 1;
                continue;
            }
            // trace:BUG-1514 | ai:claude — name WHY it's stuck (acceptance #5).
            let reason = match &pr.done_spec {
                Some(spec) => format!("{spec} is marked Done but its CI is failing"),
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
                format!(" {}", b.spec_id.bold())
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
                let urgent = if self.mail.urgent > 0 {
                    format!(" ({} urgent)", self.mail.urgent)
                } else {
                    String::new()
                };
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
            writeln!(
                w,
                "  🧭 unshipped work{}: {} on `{}` — {} commit{} ahead, age {}, PR {} — `{}`",
                scope_note,
                item.spec_id.bold(),
                item.branch,
                item.commits_ahead,
                if item.commits_ahead == 1 { "" } else { "s" },
                item.age,
                item.pr_state,
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
                let actionable = self
                    .reviewer_queue_items
                    .iter()
                    .filter(|q| q.state == review_verdict::ReviewActionability::NeedsReview)
                    .count();
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
                    review_verdict::ReviewActionability::NeedsReview => ("👀", "needs-review"),
                    review_verdict::ReviewActionability::AwaitingRework => {
                        ("🔧", "awaiting-rework")
                    }
                    review_verdict::ReviewActionability::Resolved => ("✅", "resolved"),
                };
                writeln!(
                    w,
                    "  {} {}: {} — {}",
                    glyph,
                    label,
                    q.spec_id.bold(),
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
                writeln!(
                    w,
                    "  {} {} shelved item{} in rework — `{}`",
                    crate::glyph(crate::glyphs::Glyph::Pause).blue(),
                    self.shelved_total,
                    if self.shelved_total == 1 { "" } else { "s" },
                    "aida findings list".cyan(),
                )?;
                budget -= 1;
            }
        }
        for e in &self.escalations {
            if budget == 0 {
                overflow += 1;
                continue;
            }
            writeln!(w, "  🗣️ escalation: {} — {}", e.spec_id.bold(), e.title,)?;
            budget -= 1;
        }
        // trace:BUG-1564 | ai:claude
        for o in &self.orphaned_in_progress {
            if budget == 0 {
                overflow += 1;
                continue;
            }
            let state = if o.abandoned {
                "abandoned — lease died"
            } else {
                "not yet started — no lease"
            };
            writeln!(
                w,
                "  {} orphaned In-Progress: {} — {}, {} — `{}`",
                "⚠️".yellow(),
                o.spec_id.bold(),
                state,
                o.since_label,
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
            "mergeable_prs": self.mergeable_prs.iter().map(|p| serde_json::json!({
                "number": p.number,
                "title": p.title,
                "head_branch": p.head_branch,
                "ci_rollup": p.ci_rollup,
                "under_review": p.under_review,
            })).collect::<Vec<_>>(),
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
                    "reason": match i.reason {
                        BlockedReason::AtHead => "at_head",
                        BlockedReason::Unverifiable => "unverifiable",
                    },
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
                "reason": match i.reason {
                    StaleApprovalReason::Stale => "stale",
                    StaleApprovalReason::Unverifiable => "unverifiable",
                },
            })).collect::<Vec<_>>(),
            "reviewer_queue_items": self.reviewer_queue_items.iter().map(|q| serde_json::json!({
                "spec_id": q.spec_id,
                "title": q.title,
                "state": q.state.as_str(),
            })).collect::<Vec<_>>(),
            // BUG-1508 AC4/AC7: the depth figure that decides reviewer
            // capacity is actionable-of-routed, not a bare routed count.
            "reviewer_actionable_of_routed": {
                "actionable": self.reviewer_queue_items.iter().filter(|q| q.state == review_verdict::ReviewActionability::NeedsReview).count(),
                "routed": self.reviewer_queue_items.len(),
            },
            "shelved_total": self.shelved_total,
            "escalations": self.escalations.iter().map(|e| serde_json::json!({
                "spec_id": e.spec_id,
                "title": e.title,
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
            })).collect::<Vec<_>>(),
        })
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
        if !self.mergeable_prs.is_empty() {
            parts.push(pluralize(self.mergeable_prs.len(), "PR", "PRs"));
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
            let urgent = if self.mail.urgent > 0 {
                format!(" ({} urgent)", self.mail.urgent)
            } else {
                String::new()
            };
            parts.push(format!(
                "{}{}",
                pluralize(self.mail.unread, "mail", "mail"),
                urgent
            ));
        }
        // trace:BUG-767 | ai:claude
        if self.mail.shared_unread > 0 {
            parts.push(format!("{} shared mail", self.mail.shared_unread));
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
                let actionable = self
                    .reviewer_queue_items
                    .iter()
                    .filter(|q| q.state == review_verdict::ReviewActionability::NeedsReview)
                    .count();
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
    use super::*;

    fn pr(
        number: u64,
        mergeable: Option<&str>,
        ci: Option<&str>,
        verdict: Option<&str>,
    ) -> OpenPrItem {
        OpenPrItem {
            number,
            title: format!("PR {number}"),
            head_branch: format!("branch-{number}"),
            ci_rollup: ci.map(String::from),
            mergeable: mergeable.map(String::from),
            review_decision: verdict.map(String::from),
            head_sha: None,
            labels: Vec::new(),
            created_at: None,
        }
    }

    fn failing_candidate(number: u64) -> UnownedFailingPrCandidate {
        UnownedFailingPrCandidate {
            pr: pr(number, Some("MERGEABLE"), Some("fail"), None),
            has_local_verdict: false,
            held: false,
            route: ReviewerRoute::Unrouted,
            actively_owned: false,
            done_spec: None,
        }
    }

    fn fixed_now() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-09-22T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    // TASK-192: the historical PR-2035 shape is red and has no route at all.
    // It must be visible even though the green orphan-review sweep correctly
    // refuses to claim it.
    #[test]
    fn red_unrouted_unheld_unowned_pr_is_actionable() {
        let mut review_required = failing_candidate(2036);
        review_required.pr.review_decision = Some("REVIEW_REQUIRED".into());
        let rows =
            classify_unowned_failing_prs(&[failing_candidate(2035), review_required], fixed_now());
        assert_eq!(
            rows.iter().map(|row| row.number).collect::<Vec<_>>(),
            vec![2035, 2036],
            "REVIEW_REQUIRED describes missing review, not an existing verdict"
        );
    }

    #[test]
    fn green_pending_and_every_existing_route_are_excluded() {
        let mut green = failing_candidate(1);
        green.pr.ci_rollup = Some("pass".into());
        let mut pending = failing_candidate(2);
        pending.pr.ci_rollup = Some("pending".into());
        let mut held = failing_candidate(3);
        held.held = true;
        let mut routed = failing_candidate(4);
        routed.route = ReviewerRoute::Routed;
        let mut owned = failing_candidate(5);
        owned.actively_owned = true;
        let mut local_verdict = failing_candidate(6);
        local_verdict.has_local_verdict = true;
        let mut forge_verdict = failing_candidate(7);
        forge_verdict.pr.review_decision = Some("CHANGES_REQUESTED".into());
        let mut approved = failing_candidate(8);
        approved.pr.review_decision = Some("APPROVED".into());

        assert!(classify_unowned_failing_prs(
            &[
                green,
                pending,
                held,
                routed,
                owned,
                local_verdict,
                forge_verdict,
                approved,
            ],
            fixed_now(),
        )
        .is_empty());
    }

    #[test]
    fn unknown_reviewer_queue_fails_closed() {
        let mut unavailable = failing_candidate(9);
        unavailable.route = ReviewerRoute::Unknown;
        assert!(classify_unowned_failing_prs(&[unavailable], fixed_now()).is_empty());
    }

    // BUG-1514: a Done spec whose PR is red is an inconsistency even though a
    // merge hold, in this specific test, is deliberately NOT set — the row
    // must still surface despite full ownership (routed + actively owned +
    // has_local_verdict), because none of those facts make "Done" true.
    #[test]
    fn done_spec_with_failing_pr_surfaces_despite_full_ownership() {
        let mut owned_and_routed = failing_candidate(2050);
        owned_and_routed.route = ReviewerRoute::Routed;
        owned_and_routed.actively_owned = true;
        owned_and_routed.has_local_verdict = true;
        owned_and_routed.pr.review_decision = Some("APPROVED".into());
        owned_and_routed.done_spec = Some("BUG-1470".into());

        let rows = classify_unowned_failing_prs(&[owned_and_routed], fixed_now());
        assert_eq!(
            rows.len(),
            1,
            "Done+red must surface regardless of ownership"
        );
        assert_eq!(rows[0].done_spec.as_deref(), Some("BUG-1470"));
    }

    // BUG-1514: a merge hold is an existing, independent "something is
    // already stopping this" signal — the Done-spec bypass must not
    // duplicate a row that surface already owns.
    #[test]
    fn done_spec_with_failing_pr_still_respects_a_merge_hold() {
        let mut held = failing_candidate(2051);
        held.held = true;
        held.done_spec = Some("BUG-1470".into());
        assert!(classify_unowned_failing_prs(&[held], fixed_now()).is_empty());
    }

    // BUG-1514 acceptance #3: freshly-red PRs must not alarm.
    #[test]
    fn freshly_red_pr_is_debounced_below_the_min_age() {
        let mut fresh = failing_candidate(2052);
        fresh.pr.created_at = Some(fixed_now() - chrono::Duration::minutes(5));
        assert!(
            classify_unowned_failing_prs(&[fresh], fixed_now()).is_empty(),
            "5 minutes red is routine rework-cycle churn, not neglect"
        );
    }

    #[test]
    fn red_pr_past_the_min_age_surfaces() {
        let mut stale = failing_candidate(2053);
        stale.pr.created_at = Some(fixed_now() - chrono::Duration::hours(4));
        let rows = classify_unowned_failing_prs(&[stale], fixed_now());
        assert_eq!(
            rows.len(),
            1,
            "3.8h+ red-and-unowned is exactly #2035's shape"
        );
    }

    #[test]
    fn broken_unowned_row_reaches_human_and_json_surfaces() {
        let report = AwaitingReport {
            unowned_failing_prs: classify_unowned_failing_prs(
                &[failing_candidate(2035)],
                fixed_now(),
            ),
            ..Default::default()
        };
        let mut rendered = Vec::new();
        assert!(report.render(false, &mut rendered).unwrap());
        let rendered = String::from_utf8(rendered).unwrap();
        assert!(rendered.contains("PR-2035 CI failing with no owner or route"));
        assert!(rendered.contains("gh pr checks 2035"));

        let json = report.to_json();
        assert_eq!(json["total"], 1);
        assert_eq!(json["unowned_failing_prs"][0]["number"], 2035);
        assert_eq!(
            json["unowned_failing_prs"][0]["action"],
            "gh pr checks 2035"
        );
        let compact = report.compact_line().unwrap();
        assert!(compact.contains("1 broken-unowned"), "{compact}");
        assert_eq!(
            unowned_failing_pr_toon_rows(&report.unowned_failing_prs),
            vec![vec![
                "2035".to_string(),
                "PR 2035".to_string(),
                "branch-2035".to_string(),
                "gh pr checks 2035".to_string(),
            ]]
        );
    }

    // ── BUG-767: the mail counting SCOPE ────────────────────────────────────
    // trace:BUG-767 | ai:claude

    fn mail(
        id: &str,
        from: &str,
        to: aida_core::mailbox::Recipient,
        ts: i64,
    ) -> aida_core::mailbox::Message {
        aida_core::mailbox::Message {
            subject: None,
            id: id.to_string(),
            thread_id: "t".to_string(),
            from: from.to_string(),
            to,
            timestamp: ts,
            in_reply_to: None,
            body: "b".to_string(),
            urgent: false,
            intent: aida_core::mailbox::Intent::default(),
            retracted: false,
            deleted: false,
            archived: false,
            from_source: aida_core::mailbox::SenderSource::Explicit,
            from_role: None,
        }
    }

    fn to(agent: &str) -> aida_core::mailbox::Recipient {
        aida_core::mailbox::Recipient::Agent(agent.to_string())
    }

    // ── BUG-1549: the one PR review classifier, table-driven ────────────────

    const HEAD: &str = "cafef00d1234567890";
    const OLD: &str = "deadbeef9999999999";
    const T1: &str = "2026-09-01T00:00:00+00:00";
    const T2: &str = "2026-09-02T00:00:00+00:00";
    const T3: &str = "2026-09-03T00:00:00+00:00";

    fn verdict(kind: &str, sha: Option<&str>, at: Option<&str>) -> review_verdict::RecordedVerdict {
        review_verdict::RecordedVerdict {
            kind: review_verdict::VerdictKind::parse(kind),
            raw: kind.to_string(),
            reviewed_sha: sha.map(str::to_string),
            recorded_at: at.map(str::to_string),
            ..Default::default()
        }
    }
    fn refusal(sha: Option<&str>, at: Option<&str>) -> review_verdict::RecordedVerdict {
        verdict("request-changes", sha, at)
    }
    fn approval(sha: Option<&str>, at: Option<&str>) -> review_verdict::RecordedVerdict {
        verdict("approved", sha, at)
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Kind {
        NoRow,
        Blocked(BlockedReason),
        Stale,
        Unverifiable,
        ReworkReady,
    }

    fn kind_of(row: Option<&PrReviewRow>) -> Kind {
        match row {
            None => Kind::NoRow,
            Some(PrReviewRow::Blocked { reason, .. }) => Kind::Blocked(*reason),
            Some(PrReviewRow::Stale { .. }) => Kind::Stale,
            Some(PrReviewRow::Unverifiable { .. }) => Kind::Unverifiable,
            Some(PrReviewRow::ReworkReady { .. }) => Kind::ReworkReady,
        }
    }

    // BUG-1549: every case is (candidates, head) -> (suppressed, row kind),
    // and EVERY case also asserts the invariant — suppressed iff the PR has a
    // Blocked, Stale or Unverifiable row — both on the decision and after it
    // is rendered into the report's row vectors (where seat scoping applies).
    // trace:BUG-1549 | ai:claude
    #[test]
    fn classify_pr_review_table() {
        use BlockedReason::{AtHead, Unverifiable as BUnv};
        let head = Some(HEAD);
        let short = Some("cafef0");
        #[allow(clippy::type_complexity)]
        let cases: Vec<(
            &str,
            Vec<review_verdict::RecordedVerdict>,
            Option<&str>,
            bool,
            Kind,
        )> = vec![
            ("nothing", vec![], head, false, Kind::NoRow),
            (
                "refusal at head",
                vec![refusal(head, Some(T1))],
                head,
                true,
                Kind::Blocked(AtHead),
            ),
            (
                "rejected at head",
                vec![verdict("rejected", head, Some(T1))],
                head,
                true,
                Kind::Blocked(AtHead),
            ),
            (
                "refusal moved",
                vec![refusal(Some(OLD), Some(T1))],
                head,
                false,
                Kind::ReworkReady,
            ),
            (
                "sha-less refusal",
                vec![refusal(None, Some(T1))],
                head,
                true,
                Kind::Blocked(BUnv),
            ),
            (
                "blank-sha refusal",
                vec![refusal(Some("   "), Some(T1))],
                head,
                true,
                Kind::Blocked(BUnv),
            ),
            (
                "short-sha refusal",
                vec![refusal(short, Some(T1))],
                head,
                true,
                Kind::Blocked(BUnv),
            ),
            (
                "head-less PR, sha'd refusal",
                vec![refusal(head, Some(T1))],
                None,
                true,
                Kind::Blocked(BUnv),
            ),
            (
                "refusal superseded by later approval at head",
                vec![refusal(head, Some(T1)), approval(head, Some(T2))],
                head,
                false,
                Kind::NoRow,
            ),
            (
                "sha-less refusal superseded by later approval at head",
                vec![refusal(None, Some(T1)), approval(head, Some(T2))],
                head,
                false,
                Kind::NoRow,
            ),
            (
                "refusal then later sha-less approval: not superseded",
                vec![refusal(head, Some(T1)), approval(None, Some(T2))],
                head,
                true,
                Kind::Blocked(AtHead),
            ),
            (
                "refusal then later stale approval: not superseded",
                vec![refusal(head, Some(T1)), approval(Some(OLD), Some(T2))],
                head,
                true,
                Kind::Blocked(AtHead),
            ),
            (
                "undated refusal plus approval at head: recency unknown",
                vec![refusal(head, None), approval(head, Some(T2))],
                head,
                true,
                Kind::Blocked(AtHead),
            ),
            (
                "dated refusal plus undated approval at head: recency unknown",
                vec![refusal(head, Some(T1)), approval(head, None)],
                head,
                true,
                Kind::Blocked(AtHead),
            ),
            (
                "approval at head OLDER than the refusal",
                vec![approval(head, Some(T1)), refusal(head, Some(T2))],
                head,
                true,
                Kind::Blocked(AtHead),
            ),
            (
                "two title specs: spec1 refusal moved, spec2 refusal at head",
                vec![refusal(Some(OLD), Some(T2)), refusal(head, Some(T1))],
                head,
                true,
                Kind::Blocked(AtHead),
            ),
            (
                "two title specs: spec1 refusal moved, spec2 refusal sha-less",
                vec![refusal(Some(OLD), Some(T1)), refusal(None, Some(T2))],
                head,
                true,
                Kind::Blocked(BUnv),
            ),
            (
                "refusal t1, approval at head t2, refusal at head t3",
                vec![
                    refusal(head, Some(T1)),
                    approval(head, Some(T2)),
                    refusal(head, Some(T3)),
                ],
                head,
                true,
                Kind::Blocked(AtHead),
            ),
            (
                "approval at head only",
                vec![approval(head, Some(T1))],
                head,
                false,
                Kind::NoRow,
            ),
            (
                "undated approval at head only",
                vec![approval(head, None)],
                head,
                false,
                Kind::NoRow,
            ),
            (
                "approval with a truncated but matching sha",
                vec![approval(Some("cafef00d"), Some(T1))],
                head,
                false,
                Kind::NoRow,
            ),
            (
                "stale approval only",
                vec![approval(Some(OLD), Some(T1))],
                head,
                true,
                Kind::Stale,
            ),
            (
                "sha-less approval only",
                vec![approval(None, Some(T1))],
                head,
                true,
                Kind::Unverifiable,
            ),
            (
                "short-sha approval only",
                vec![approval(short, Some(T1))],
                head,
                true,
                Kind::Unverifiable,
            ),
            (
                "head-less PR, sha'd approval",
                vec![approval(head, Some(T1))],
                None,
                true,
                Kind::Unverifiable,
            ),
            (
                "spec approval at head t1 + PR sha-less approval t2: newest decides",
                vec![approval(head, Some(T1)), approval(None, Some(T2))],
                head,
                true,
                Kind::Unverifiable,
            ),
            (
                "spec approval at head t2 + PR sha-less approval t1: newest decides",
                vec![approval(head, Some(T2)), approval(None, Some(T1))],
                head,
                false,
                Kind::NoRow,
            ),
            (
                "spec approval at head t1 + PR stale approval t2: newest decides",
                vec![approval(head, Some(T1)), approval(Some(OLD), Some(T2))],
                head,
                true,
                Kind::Stale,
            ),
            (
                "undated approval at head + dated sha-less approval: undated is oldest",
                vec![approval(head, None), approval(None, Some(T1))],
                head,
                true,
                Kind::Unverifiable,
            ),
            (
                "dated approval at head + undated sha-less approval: undated is oldest",
                vec![approval(head, Some(T1)), approval(None, None)],
                head,
                false,
                Kind::NoRow,
            ),
            (
                "moved refusal + later approval at head",
                vec![refusal(Some(OLD), Some(T1)), approval(head, Some(T2))],
                head,
                false,
                Kind::NoRow,
            ),
            (
                "moved refusal + stale approval",
                vec![refusal(Some(OLD), Some(T1)), approval(Some(OLD), Some(T2))],
                head,
                true,
                Kind::Stale,
            ),
            (
                "unrecognised verdict word only",
                vec![verdict("pondering", head, Some(T1))],
                head,
                false,
                Kind::NoRow,
            ),
        ];

        for (name, candidates, head, want_suppressed, want_kind) in cases {
            let d = classify_pr_review(&candidates, head);
            assert_eq!(d.suppressed, want_suppressed, "{name}: suppressed");
            assert_eq!(kind_of(d.row.as_ref()), want_kind, "{name}: row kind");

            // The invariant, on the decision…
            let explained = d
                .row
                .as_ref()
                .is_some_and(PrReviewRow::explains_suppression);
            assert_eq!(d.suppressed, explained, "{name}: suppressed iff explained");

            // …and after rendering into the report rows, under a seat that
            // did NOT record anything (seat scoping must never hide a row that
            // explains a suppression).
            let mut rows = PrReviewRows::default();
            rows.add(2100, head, "claude/bug-2100", &d, Some("some-other-seat"));
            let explaining = rows.blocked_reviews.len() + rows.stale_approvals.len();
            assert_eq!(
                explaining,
                usize::from(d.suppressed),
                "{name}: exactly one explaining row iff suppressed"
            );
            assert_eq!(
                rows.rework_ready.len(),
                usize::from(want_kind == Kind::ReworkReady),
                "{name}: rework_ready row"
            );
        }
    }

    // STORY-1419: the rework-ready row (and only it — it does not suppress) is
    // scoped to the seat that refused; unknown on either side surfaces.
    // trace:STORY-1419 trace:BUG-1549 | ai:claude
    #[test]
    fn rework_ready_rows_are_scoped_to_the_refusing_seat_but_unknown_surfaces() {
        let moved = |by: Option<&str>| {
            let mut r = refusal(Some(OLD), Some(T1));
            r.recorded_by = by.map(str::to_string);
            classify_pr_review(&[r], Some(HEAD))
        };
        let mine = moved(Some("claude-reviewer-1 (claude reviewer seat)"));
        let theirs = moved(Some("aida drain reviewer"));
        let anon = moved(None);

        let mut scoped = PrReviewRows::default();
        scoped.add(2014, Some(HEAD), "b", &mine, Some("claude-reviewer-1"));
        scoped.add(2030, Some(HEAD), "b", &theirs, Some("claude-reviewer-1"));
        scoped.add(2040, Some(HEAD), "b", &anon, Some("claude-reviewer-1"));
        let prs: Vec<u64> = scoped.rework_ready.iter().map(|r| r.pr).collect();
        assert_eq!(prs, vec![2014, 2040], "mine plus the unattributable one");

        let mut unscoped = PrReviewRows::default();
        for (n, d) in [(2014, &mine), (2030, &theirs), (2040, &anon)] {
            unscoped.add(n, Some(HEAD), "b", d, None);
        }
        assert_eq!(unscoped.rework_ready.len(), 3, "no seat known: surface all");
    }

    // STORY-1420: a refusal recorded by a seat that has EXITED reaches the
    // implementer (the spec owner) and the advisor instead of nobody; a live
    // seat's refusal still routes only to it; unknown liveness shows to all.
    // trace:STORY-1420 | ai:claude
    #[test]
    fn exited_recorder_rework_is_inherited_by_implementer_and_advisor() {
        let moved = |by: &str| {
            let mut r = refusal(Some(OLD), Some(T1));
            r.recorded_by = Some(by.to_string());
            classify_pr_review(&[r], Some(HEAD))
        };
        let drain = moved("aida drain reviewer");
        let live = moved("claude-reviewer-1 (claude reviewer seat)");
        let gone = moved("claude-reviewer-9 (claude reviewer seat)");
        let stranger = moved("claude-reviewer-5 (claude reviewer seat)");
        let registry = |name: &str| match name {
            "claude-reviewer-1" => Some(true),
            "claude-reviewer-9" => Some(false),
            _ => None,
        };
        let liveness = |who: &str| classify_recorder(who, registry);
        let owner = |spec: &str| (spec == "BUG-2200").then(|| "impl-alice".to_string());
        let route = |identity: &str, role: &str| {
            let mut rows = PrReviewRows::default();
            for (n, d) in [
                (2200, &drain),
                (2201, &live),
                (2202, &gone),
                (2203, &stranger),
            ] {
                rows.add_routed(
                    n,
                    Some(HEAD),
                    &format!("claude/bug-{n}"),
                    d,
                    ReworkReader {
                        identity: Some(identity),
                        role: Some(role),
                    },
                    liveness,
                    owner,
                );
            }
            rows.rework_ready
        };
        let prs = |rows: &[ReworkReadyItem]| rows.iter().map(|r| r.pr).collect::<Vec<_>>();

        // The implementer who owns BUG-2200 inherits the drain reviewer's
        // refusal; the unowned exited one (2202) is everyone's; the live seat's
        // refusal is not theirs; unknown liveness (2203) shows to all.
        let implementer = route("impl-alice", "implementer");
        assert_eq!(prs(&implementer), vec![2200, 2202, 2203]);
        assert_eq!(
            implementer[0].inherited_from.as_deref(),
            Some("aida drain reviewer"),
            "an inherited row names the exited recorder"
        );
        assert_eq!(implementer[2].inherited_from, None);

        // The advisor (standing disposition gate) sees the exited rows too.
        assert_eq!(prs(&route("advisor-1", "advisor")), vec![2200, 2202, 2203]);

        // The live refusing seat keeps its own row; a reviewer that is not
        // the owner does not inherit the owned exited row, but still sees the
        // unowned bucket and the unknown-liveness row.
        assert_eq!(
            prs(&route("claude-reviewer-1", "reviewer")),
            vec![2201, 2202, 2203]
        );

        let mut buf = Vec::new();
        AwaitingReport {
            rework_ready: implementer,
            ..Default::default()
        }
        .render(false, &mut buf)
        .unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(
            out.contains("a refusal by aida drain reviewer (recorder exited"),
            "{out}"
        );
    }

    // trace:STORY-1420 | ai:claude
    #[test]
    fn classify_recorder_reads_the_registry_and_fails_open() {
        let reg = |name: &str| match name {
            "live-1" => Some(true),
            "dead-1" => Some(false),
            _ => None,
        };
        assert_eq!(
            classify_recorder("aida drain reviewer", reg),
            RecorderLiveness::Exited
        );
        assert_eq!(
            classify_recorder("live-1 (claude reviewer seat)", reg),
            RecorderLiveness::Live
        );
        assert_eq!(
            classify_recorder("dead-1 (reviewer seat)", reg),
            RecorderLiveness::Exited
        );
        assert_eq!(
            classify_recorder("never-seen (claude reviewer seat)", reg),
            RecorderLiveness::Unknown
        );
        assert_eq!(
            classify_recorder("aida review record (operator)", reg),
            RecorderLiveness::Unknown
        );
    }

    // BUG-1549: the rows carry both shas and the branch-derived spec, and the
    // blocked row reaches the header count, the render and the JSON.
    // trace:BUG-1549 | ai:claude
    #[test]
    fn review_rows_carry_shas_spec_and_reach_the_render() {
        let mut rows = PrReviewRows::default();
        let stale = classify_pr_review(&[approval(Some(OLD), Some(T1))], Some(HEAD));
        rows.add(2060, Some(HEAD), "task-1298-work", &stale, None);
        let blocked = classify_pr_review(&[refusal(Some(HEAD), Some(T1))], Some(HEAD));
        rows.add(2061, Some(HEAD), "some-unlabelled-branch", &blocked, None);
        assert_eq!(rows.stale_approvals[0].reviewed_sha, OLD);
        assert_eq!(rows.stale_approvals[0].head_sha, HEAD);
        assert_eq!(rows.stale_approvals[0].spec.as_deref(), Some("TASK-1298"));
        assert_eq!(
            rows.blocked_reviews[0].spec, None,
            "no spec id: still a row"
        );

        let r = AwaitingReport {
            blocked_reviews: rows.blocked_reviews,
            stale_approvals: rows.stale_approvals,
            ..Default::default()
        };
        assert_eq!(r.total(), 2);
        let mut buf = Vec::new();
        assert!(r.render(false, &mut buf).unwrap());
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("PR-2061") && out.contains("blocked"), "{out}");
        assert!(
            out.contains("PR-2060") && out.contains("TASK-1298"),
            "{out}"
        );
        assert_eq!(r.to_json()["blocked_reviews"][0]["reason"], "at_head");
        assert!(r.compact_line().unwrap().contains("1 blocked-review"));
    }

    // TASK-1310: `blocked_age` is the pure function both the text render and
    // the JSON shape read — testing it directly is testing what both
    // surfaces will say, without depending on wall-clock timing in a render
    // assertion.
    // trace:TASK-1310 | ai:claude
    #[test]
    fn blocked_age_computes_days_since_recorded_at() {
        let now: chrono::DateTime<chrono::Utc> = "2026-09-23T00:00:00Z".parse().unwrap();
        let age = blocked_age(Some("2026-09-20T00:00:00+00:00"), now).expect("parseable");
        assert_eq!(age.secs, 3 * 24 * 3600);
        assert!(!age.overdue, "3 days is under the overdue threshold");
        assert_eq!(
            blocked_age_label(Some("2026-09-20T00:00:00+00:00"), now),
            "refused 3d ago, no rework since"
        );
    }

    // trace:TASK-1310 | ai:claude
    // trace:TASK-1310 | ai:claude
    #[test]
    fn unverifiable_blocked_age_never_claims_no_rework() {
        let now: chrono::DateTime<chrono::Utc> = "2026-09-23T00:00:00Z".parse().unwrap();
        let label = blocked_age_label_unverifiable(Some("2026-09-01T00:00:00+00:00"), now);
        assert!(label.contains("rework since then unknown"), "{label}");
        assert!(!label.contains("no rework"), "{label}");
        assert!(!label.contains("overdue"), "{label}");
    }

    #[test]
    fn blocked_age_flags_long_standing_refusals_as_overdue() {
        let now: chrono::DateTime<chrono::Utc> = "2026-09-23T00:00:00Z".parse().unwrap();
        let age = blocked_age(Some("2026-09-01T00:00:00+00:00"), now).expect("parseable");
        assert!(age.overdue, "22 days must clear the overdue threshold");
        assert!(
            blocked_age_label(Some("2026-09-01T00:00:00+00:00"), now).contains("overdue"),
            "the label must name the long-standing refusal, not just its age"
        );
    }

    // PRIN-5: a refusal with no recorded_at (or an unparseable one) is
    // reported as "age unknown" — never silently read as "no rework" or as
    // zero elapsed time. Mirrors PR-1784.json's real shape: the sha KEY is
    // present but its value is empty, which is exactly the
    // "blank-sha refusal" case `classify_pr_review_table` already covers for
    // the reason; this asserts the age reads unknown independently of that.
    // trace:TASK-1310 | ai:claude
    #[test]
    fn blocked_age_is_unknown_without_a_parseable_recorded_at() {
        let now: chrono::DateTime<chrono::Utc> = "2026-09-23T00:00:00Z".parse().unwrap();
        assert_eq!(blocked_age(None, now), None);
        assert_eq!(blocked_age(Some(""), now), None);
        assert_eq!(blocked_age(Some("   "), now), None);
        assert_eq!(blocked_age(Some("not-a-timestamp"), now), None);
        assert_eq!(blocked_age_label(None, now), "age unknown");
    }

    // trace:TASK-1310 | ai:claude
    #[test]
    fn blocked_review_render_and_json_carry_the_age() {
        // Recorded 2h ago, relative to the actual wall clock — this must
        // read as recent (not overdue) no matter what day the suite runs.
        let recent = (chrono::Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
        let decision = classify_pr_review(&[refusal(Some(HEAD), Some(&recent))], Some(HEAD));
        let mut rows = PrReviewRows::default();
        rows.add(2200, Some(HEAD), "some-branch", &decision, None);
        assert_eq!(
            rows.blocked_reviews[0].recorded_at.as_deref(),
            Some(recent.as_str()),
            "the raw recorded_at must ride along so a second reader can recompute the age"
        );

        let r = AwaitingReport {
            blocked_reviews: rows.blocked_reviews,
            ..Default::default()
        };
        let mut buf = Vec::new();
        assert!(r.render(false, &mut buf).unwrap());
        let out = String::from_utf8(buf).unwrap();
        assert!(
            out.contains("no rework since"),
            "the blocked row must say how long it has stood: {out}"
        );
        assert!(
            !out.contains("overdue"),
            "2h ago must not read as long-standing: {out}"
        );

        let json = r.to_json();
        assert_eq!(json["blocked_reviews"][0]["recorded_at"], recent);
        assert!(json["blocked_reviews"][0]["age_secs"].is_number());
        assert_eq!(json["blocked_reviews"][0]["overdue"], false);
    }

    // trace:TASK-1310 | ai:claude
    #[test]
    fn blocked_review_json_reports_unknown_age_never_no_rework() {
        // The blank-sha-key shape (PR-1784.json): the refusal has NO
        // recorded_at at all, so age must be null/unknown, not zero.
        let decision = classify_pr_review(&[refusal(Some(HEAD), None)], Some(HEAD));
        let mut rows = PrReviewRows::default();
        rows.add(2201, Some(HEAD), "some-branch", &decision, None);
        assert_eq!(rows.blocked_reviews[0].recorded_at, None);

        let r = AwaitingReport {
            blocked_reviews: rows.blocked_reviews,
            ..Default::default()
        };
        let json = r.to_json();
        assert!(json["blocked_reviews"][0]["age_secs"].is_null());
        assert!(json["blocked_reviews"][0]["overdue"].is_null());

        let mut buf = Vec::new();
        r.render(false, &mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("age unknown"), "{out}");
        assert!(!out.contains("no rework since"), "{out}");
    }

    /// The regression: three messages in the operator's own inbox, a big
    /// backlog in the shared role inbox. Deleting the operator's three drives
    /// the operator-gated count to 0 — it must NEVER shift to the role
    /// backlog's size (the observed 3 → 18 jump).
    #[test]
    fn operator_mail_count_goes_to_zero_when_own_inbox_is_emptied() {
        let wm = std::collections::HashMap::new();
        let mut msgs: Vec<_> = (0..3)
            .map(|i| mail(&format!("own-{i}"), "codex", to("joe"), 100 + i))
            .collect();
        // The shared `advisor` inbox carries a fat agent-to-agent backlog.
        msgs.extend((0..18).map(|i| mail(&format!("role-{i}"), "codex", to("advisor"), 200 + i)));

        let shared = vec!["advisor".to_string()];
        let before = split_mail_scopes("joe", &shared, &msgs, &wm);
        assert_eq!(before.unread, 3, "operator's own three must be the count");
        assert_eq!(before.shared_unread, 18, "role backlog is counted apart");

        // Delete the operator's three (the mailbox delete marker).
        for m in msgs.iter_mut().filter(|m| m.id.starts_with("own-")) {
            m.deleted = true;
        }
        let after = split_mail_scopes("joe", &shared, &msgs, &wm);
        assert_eq!(
            after.unread, 0,
            "emptying your own inbox must read 0, never the shared backlog"
        );
        assert_eq!(after.shared_unread, 18, "the shared surface is unchanged");
        assert_eq!(after.shared_scope.as_deref(), Some("advisor"));
    }

    /// A broadcast lands in every inbox; it must count ONCE, in the operator's
    /// own number, and never be double-counted as shared mail.
    #[test]
    fn broadcast_counts_once_in_the_operator_scope() {
        let wm = std::collections::HashMap::new();
        let msgs = vec![mail(
            "bcast",
            "codex",
            aida_core::mailbox::Recipient::Broadcast,
            10,
        )];
        let c = split_mail_scopes("joe", &["advisor".to_string()], &msgs, &wm);
        assert_eq!(c.unread, 1);
        assert_eq!(c.shared_unread, 0, "already counted as the operator's own");
    }

    /// No session role / agent type → nothing shared, and the operator's own
    /// inbox is the whole story.
    #[test]
    fn no_shared_identities_means_no_shared_channel() {
        let wm = std::collections::HashMap::new();
        let msgs = vec![mail("m1", "codex", to("advisor"), 10)];
        let c = split_mail_scopes("joe", &[], &msgs, &wm);
        assert_eq!(c.unread, 0);
        assert_eq!(c.shared_unread, 0);
        assert!(c.shared_scope.is_none());
    }

    /// The shared line is its own row in the report — the fleet-wide capability
    /// is kept and labelled, just not folded into `mail.unread`.
    #[test]
    fn shared_mail_renders_on_its_own_labelled_line() {
        let r = AwaitingReport {
            mail: MailChannel {
                unread: 0,
                urgent: 0,
                shared_unread: 18,
                shared_scope: Some("advisor".to_string()),
            },
            ..Default::default()
        };
        assert!(!r.is_empty());
        assert_eq!(r.total(), 1);
        let mut buf = Vec::new();
        assert!(r.render(false, &mut buf).unwrap());
        let s = strip_ansi(&String::from_utf8(buf).unwrap());
        assert!(
            s.contains("18 unread mail in the shared advisor inbox"),
            "shared mail must be labelled distinctly:\n{s}"
        );
        let line = r
            .compact_line()
            .expect("shared mail yields a per-turn line");
        assert!(line.contains("18 shared mail"), "compact line: {line}");
    }

    #[test]
    fn unshipped_work_renders_json_and_notice_count() {
        let r = AwaitingReport {
            unshipped_work: vec![UnshippedWorkItem {
                spec_id: "STORY-1043".to_string(),
                branch: "story-1043".to_string(),
                commits_ahead: 2,
                age: "3h".to_string(),
                recovery: "aida pr ship story-1043".to_string(),
                pr_state: "absent".to_string(),
            }],
            ..Default::default()
        };
        assert_eq!(r.total(), 1);
        let line = r
            .compact_line()
            .expect("unshipped work yields a per-turn line");
        assert!(line.contains("unshipped:1"), "compact line: {line}");

        let mut buf = Vec::new();
        r.render(false, &mut buf).unwrap();
        let s = strip_ansi(&String::from_utf8(buf).unwrap());
        assert!(s.contains("unshipped work: STORY-1043"), "{s}");
        assert!(s.contains("aida pr ship story-1043"), "{s}");

        let json = r.to_json();
        assert_eq!(json["unshipped_work"][0]["spec_id"], "STORY-1043");
        assert_eq!(json["unshipped_work"][0]["commits_ahead"], 2);
    }

    // BUG-1288: a truncated scan with ZERO items found must not render as a
    // quiet, all-clear report — the human surface, not just `--json`, has to
    // say the scan didn't finish. This is the exact PRIN-5 failure mode: an
    // empty `unshipped_work` list reads as "checked, found nothing" unless
    // the incomplete-scan line says otherwise. trace:BUG-1288 | ai:claude
    #[test]
    fn incomplete_unshipped_scan_renders_honestly_even_with_no_items_found() {
        let r = AwaitingReport {
            unshipped_work: Vec::new(),
            unshipped_work_scan: Some(UnshippedScanStatus {
                complete: false,
                scanned: 3,
                candidates: 9,
            }),
            ..Default::default()
        };
        // A truncated scan is itself something to report — the section must
        // not hit the is_empty() fast path and disappear.
        assert!(!r.is_empty(), "an incomplete scan must not read as empty");
        assert_eq!(r.total(), 1);

        let mut buf = Vec::new();
        let printed = r.render(false, &mut buf).unwrap();
        assert!(printed, "an incomplete scan must render something");
        let s = strip_ansi(&String::from_utf8(buf).unwrap());
        assert!(
            s.contains("unshipped scan incomplete (3/9"),
            "expected an honest incomplete-scan line:\n{s}"
        );

        let json = r.to_json();
        assert_eq!(json["unshipped_work_scan"]["complete"], false);
        assert_eq!(json["unshipped_work_scan"]["scanned"], 3);
        assert_eq!(json["unshipped_work_scan"]["candidates"], 9);
    }

    // A COMPLETE scan must stay silent — this line exists only for the
    // truncated case, never as noise on every quiet report.
    // trace:BUG-1288 | ai:claude
    #[test]
    fn complete_unshipped_scan_prints_no_incomplete_line() {
        let r = AwaitingReport {
            unshipped_work: vec![UnshippedWorkItem {
                spec_id: "STORY-1043".to_string(),
                branch: "story-1043".to_string(),
                commits_ahead: 2,
                age: "3h".to_string(),
                recovery: "aida pr ship story-1043".to_string(),
                pr_state: "absent".to_string(),
            }],
            unshipped_work_scan: Some(UnshippedScanStatus {
                complete: true,
                scanned: 9,
                candidates: 9,
            }),
            ..Default::default()
        };
        let mut buf = Vec::new();
        r.render(false, &mut buf).unwrap();
        let s = strip_ansi(&String::from_utf8(buf).unwrap());
        assert!(
            !s.contains("scan incomplete"),
            "a complete scan must not print the incomplete-scan line:\n{s}"
        );
    }

    // trace:BUG-1564 | ai:claude
    #[test]
    fn orphaned_in_progress_renders_and_distinguishes_abandoned_from_not_yet_started() {
        let r = AwaitingReport {
            orphaned_in_progress: vec![
                OrphanedInProgressItem {
                    spec_id: "BUG-9001".to_string(),
                    title: "crashed session".to_string(),
                    abandoned: true,
                    since_label: "last touched 3h ago".to_string(),
                },
                OrphanedInProgressItem {
                    spec_id: "TASK-9002".to_string(),
                    title: "never picked up".to_string(),
                    abandoned: false,
                    since_label: "last-touched time unknown".to_string(),
                },
            ],
            ..Default::default()
        };
        assert_eq!(r.total(), 2);
        let line = r
            .compact_line()
            .expect("orphaned in-progress yields a per-turn line");
        assert!(
            line.contains("2 orphaned in-progress"),
            "compact line: {line}"
        );

        let mut buf = Vec::new();
        r.render(false, &mut buf).unwrap();
        let s = strip_ansi(&String::from_utf8(buf).unwrap());
        assert!(
            s.contains("BUG-9001") && s.contains("abandoned — lease died"),
            "{s}"
        );
        assert!(
            s.contains("TASK-9002") && s.contains("not yet started — no lease"),
            "{s}"
        );
        assert!(s.contains("last-touched time unknown"), "{s}");

        let json = r.to_json();
        assert_eq!(json["orphaned_in_progress"][0]["spec_id"], "BUG-9001");
        assert_eq!(json["orphaned_in_progress"][0]["abandoned"], true);
        assert_eq!(json["orphaned_in_progress"][1]["abandoned"], false);
    }

    // trace:TASK-1305 | ai:claude
    #[test]
    fn unshipped_work_no_pr_and_open_pr_render_differently() {
        let no_pr = UnshippedWorkItem {
            spec_id: "STORY-2001".to_string(),
            branch: "story-2001-no-pr".to_string(),
            commits_ahead: 3,
            age: "2h".to_string(),
            recovery: "aida pr ship story-2001-no-pr".to_string(),
            pr_state: "absent".to_string(),
        };
        let open_pr = UnshippedWorkItem {
            spec_id: "STORY-2002".to_string(),
            branch: "story-2002-open-pr".to_string(),
            commits_ahead: 3,
            age: "2h".to_string(),
            // The collector still records the generic "ship it" hint on the
            // item — the render layer is what must not repeat it verbatim
            // for an already-open PR.
            recovery: "aida pr ship story-2002-open-pr".to_string(),
            pr_state: "open".to_string(),
        };

        let r = AwaitingReport {
            unshipped_work: vec![no_pr, open_pr],
            ..Default::default()
        };
        let mut buf = Vec::new();
        r.render(false, &mut buf).unwrap();
        let s = strip_ansi(&String::from_utf8(buf).unwrap());
        let lines: Vec<&str> = s
            .lines()
            .filter(|l| l.contains("unshipped work:"))
            .collect();
        assert_eq!(lines.len(), 2, "expected one row per branch:\n{s}");

        let no_pr_line = lines
            .iter()
            .find(|l| l.contains("STORY-2001"))
            .expect("no-PR row present");
        let open_pr_line = lines
            .iter()
            .find(|l| l.contains("STORY-2002"))
            .expect("open-PR row present");

        // Criterion 1: the two states are distinguishable without opening the PR.
        assert_ne!(no_pr_line, open_pr_line);
        assert!(no_pr_line.contains("PR absent"), "{no_pr_line}");
        assert!(open_pr_line.contains("PR open"), "{open_pr_line}");

        // Criterion 2: a branch with an open PR is not told to open one.
        assert!(
            no_pr_line.contains("aida pr ship story-2001-no-pr"),
            "{no_pr_line}"
        );
        assert!(
            !open_pr_line.contains("aida pr ship"),
            "an open-PR row must not repeat the 'open a PR' hint: {open_pr_line}"
        );
        assert!(open_pr_line.contains("merge decision"), "{open_pr_line}");
    }

    // TASK-1445 (containment for BUG-1510 AC5): the disagreeing case must
    // surface on the surface an operator/advisor already looks — render,
    // JSON, and the per-turn compact line — naming both claimed owners.
    // trace:TASK-1445 | ai:claude
    #[test]
    fn pr_attribution_disagreement_renders_json_and_notice_count() {
        let r = AwaitingReport {
            pr_attribution_disagreements: vec![PrAttributionDisagreementItem {
                pr: 2043,
                lease_spec: "STORY-1391".to_string(),
                trailer_spec: "BUG-1420".to_string(),
            }],
            ..Default::default()
        };
        assert_eq!(r.total(), 1);
        let line = r
            .compact_line()
            .expect("attribution disagreement yields a per-turn line");
        assert!(line.contains("1 attribution split"), "compact line: {line}");

        let mut buf = Vec::new();
        r.render(false, &mut buf).unwrap();
        let s = strip_ansi(&String::from_utf8(buf).unwrap());
        assert!(s.contains("PR-2043 attribution split"), "{s}");
        assert!(s.contains("lease says STORY-1391"), "{s}");
        assert!(s.contains("commit trailers say BUG-1420"), "{s}");

        let json = r.to_json();
        assert_eq!(json["pr_attribution_disagreements"][0]["pr"], 2043);
        assert_eq!(
            json["pr_attribution_disagreements"][0]["lease_spec"],
            "STORY-1391"
        );
        assert_eq!(
            json["pr_attribution_disagreements"][0]["trailer_spec"],
            "BUG-1420"
        );
    }

    // STORY-1419: the CLASSIFIER tests above do not pin the PLUMBING. Dropping
    // `rework_ready` from `total()` left all of them green — the row existed and
    // the report did not count it. This asserts the wiring: the row reaches the
    // header count AND the rendered output, and names both shas so the reviewer
    // can see what moved.
    // trace:STORY-1419 | ai:claude
    #[test]
    fn a_rework_ready_row_reaches_the_header_count_and_the_render() {
        let r = AwaitingReport {
            rework_ready: vec![ReworkReadyItem {
                pr: 2014,
                spec: Some("TASK-1298".into()),
                reviewed_sha: "3310400503".into(),
                head_sha: "f95b30853e".into(),
                inherited_from: None,
            }],
            ..Default::default()
        };
        assert_eq!(r.total(), 1, "the row must be counted in the header total");
        assert!(!r.is_empty());

        let mut buf = Vec::new();
        assert!(r.render(false, &mut buf).unwrap());
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("PR-2014"), "{out}");
        assert!(out.contains("TASK-1298"), "{out}");
        assert!(
            out.contains("3310400503"),
            "the refused sha must be shown: {out}"
        );
        assert!(
            out.contains("f95b30853e"),
            "the current head must be shown: {out}"
        );
        assert!(
            !out.to_lowercase().contains("rebase"),
            "must NOT classify the move as rebase-vs-rework: {out}"
        );
    }

    #[test]
    fn empty_report_renders_nothing_and_returns_false() {
        let r = AwaitingReport::default();
        assert!(r.is_empty());
        let mut buf = Vec::new();
        let wrote = r.render(false, &mut buf).unwrap();
        assert!(!wrote);
        assert!(buf.is_empty(), "section must be hidden when count is 0");
    }

    #[test]
    fn five_mergeable_prs_render_under_cap_with_count_in_header() {
        let prs = (1..=5)
            .map(|n| pr(n, Some("MERGEABLE"), Some("pass"), None))
            .collect::<Vec<_>>();
        let classified = classify_open_prs(&prs, &HashSet::new());
        assert_eq!(classified.len(), 5);
        let r = AwaitingReport {
            mergeable_prs: classified,
            ..Default::default()
        };
        assert_eq!(r.total(), 5);

        let mut buf = Vec::new();
        let wrote = r.render(false, &mut buf).unwrap();
        assert!(wrote);
        let s = strip_ansi(&String::from_utf8(buf).unwrap());
        assert!(
            s.contains("Awaiting you (5)"),
            "header must show the total count, got:\n{s}"
        );
        for n in 1..=5 {
            assert!(
                s.contains(&format!("PR-{n}")),
                "PR-{n} missing from output:\n{s}"
            );
        }
        assert!(
            !s.contains("more"),
            "no overflow line at exactly the cap:\n{s}"
        );
    }

    #[test]
    fn mixed_gate_states_only_surface_the_mergeable_pass_no_changes_requested() {
        let prs = vec![
            // Awaiting you — mergeable, CI pass, no review verdict.
            pr(1, Some("MERGEABLE"), Some("pass"), None),
            // Awaiting CI (different gate) — excluded.
            pr(2, Some("MERGEABLE"), Some("pending"), None),
            // CI fail (broken) — excluded.
            pr(3, Some("MERGEABLE"), Some("fail"), None),
            // RequestChanges (needs rebase/fix) — excluded.
            pr(
                4,
                Some("MERGEABLE"),
                Some("pass"),
                Some("CHANGES_REQUESTED"),
            ),
            // Awaiting you — APPROVED + green is the cleanest case.
            pr(5, Some("MERGEABLE"), Some("pass"), Some("APPROVED")),
            // Conflicting — excluded.
            pr(6, Some("CONFLICTING"), Some("pass"), None),
            // Awaiting you — no CI checks set up at all (aida-chat case).
            pr(7, Some("MERGEABLE"), None, None),
        ];
        let classified = classify_open_prs(&prs, &HashSet::new());
        let nums: Vec<_> = classified.iter().map(|p| p.number).collect();
        assert_eq!(nums, vec![1, 5, 7], "only the awaiting-you PRs surface");
    }

    // BUG-1490: the exact shape observed live on PR #2030 — GitHub's
    // review_decision is empty (the refusal was recorded straight to the
    // AIDA substrate via `aida review record`, never posted as a GitHub
    // review), so `review_decision` alone cannot suppress it. The caller
    // resolves the AIDA-recorded verdict and hands the PR number in via
    // `local_blocking`; classify_open_prs must honour it independently of
    // GitHub's (empty) verdict.
    // trace:BUG-1490 | ai:claude
    #[test]
    fn aida_recorded_verdict_suppresses_pr_with_empty_github_review_decision() {
        let prs = vec![pr(2030, Some("MERGEABLE"), Some("pass"), None)];
        let mut local_blocking = HashSet::new();
        local_blocking.insert(2030);
        let classified = classify_open_prs(&prs, &local_blocking);
        assert!(
            classified.is_empty(),
            "a PR with a recorded AIDA RequestChanges verdict at head must not be awaiting-you: {classified:?}"
        );
    }

    // The sibling case: the same PR with no locally recorded verdict stays
    // awaiting-you, so the new parameter only ever narrows, never widens.
    // trace:BUG-1490 | ai:claude
    #[test]
    fn pr_without_local_verdict_still_surfaces() {
        let prs = vec![pr(2030, Some("MERGEABLE"), Some("pass"), None)];
        let classified = classify_open_prs(&prs, &HashSet::new());
        assert_eq!(
            classified.iter().map(|p| p.number).collect::<Vec<_>>(),
            vec![2030]
        );
    }

    #[test]
    fn header_count_collapses_findings_to_one_line_regardless_of_total() {
        let r = AwaitingReport {
            mergeable_prs: classify_open_prs(
                &[pr(1, Some("MERGEABLE"), Some("pass"), None)],
                &HashSet::new(),
            ),
            findings_total: 17,
            ..Default::default()
        };
        // Header (N) = 1 PR + 1 findings line = 2 (NOT 18).
        assert_eq!(r.total(), 2);
        let mut buf = Vec::new();
        r.render(false, &mut buf).unwrap();
        let s = strip_ansi(&String::from_utf8(buf).unwrap());
        assert!(s.contains("Awaiting you (2)"), "{s}");
        assert!(s.contains("17 findings awaiting triage"), "{s}");
    }

    #[test]
    fn overflow_caps_at_five_unless_verbose() {
        // 7 PRs all classify as awaiting; default cap shows 5 + overflow line.
        let prs = (1..=7)
            .map(|n| pr(n, Some("MERGEABLE"), Some("pass"), None))
            .collect::<Vec<_>>();
        let r = AwaitingReport {
            mergeable_prs: classify_open_prs(&prs, &HashSet::new()),
            ..Default::default()
        };
        let mut buf = Vec::new();
        r.render(false, &mut buf).unwrap();
        let s = strip_ansi(&String::from_utf8(buf).unwrap());
        let pr_lines = s.lines().filter(|l| l.contains("PR-")).count();
        assert_eq!(pr_lines, 5, "default cap = 5 PR lines, got:\n{s}");
        assert!(
            s.contains("2 more"),
            "overflow line must show remainder, got:\n{s}"
        );

        // --verbose lifts the cap.
        let mut buf = Vec::new();
        r.render(true, &mut buf).unwrap();
        let s = strip_ansi(&String::from_utf8(buf).unwrap());
        let pr_lines = s.lines().filter(|l| l.contains("PR-")).count();
        assert_eq!(pr_lines, 7, "verbose lifts the cap, got:\n{s}");
        assert!(!s.contains(" more"), "no overflow line under verbose:\n{s}");
    }

    #[test]
    fn json_shape_is_stable_and_includes_total() {
        let r = AwaitingReport {
            mergeable_prs: vec![MergeablePrItem {
                number: 7,
                title: "demo".into(),
                head_branch: "feat".into(),
                ci_rollup: Some("pass".into()),
                under_review: None,
            }],
            findings_total: 4,
            escalations: vec![EscalationItem {
                spec_id: "SPIKE-12".into(),
                title: "advisor punt".into(),
            }],
            ..Default::default()
        };
        let v = r.to_json();
        // 1 PR + 1 findings line + 1 escalation = 3 actionable lines.
        assert_eq!(v["total"], 3);
        assert_eq!(v["mergeable_prs"][0]["number"], 7);
        assert_eq!(v["findings_total"], 4);
        assert_eq!(v["escalations"][0]["spec_id"], "SPIKE-12");
    }

    // STORY-741: unread mail is now a first-class channel in the report — the
    // count folds into the header total and renders its own line, so the
    // coordination inbox is ONE surface instead of mail-here / everything-
    // else-there. trace:STORY-741
    #[test]
    fn unread_mail_folds_into_report_and_renders_with_urgent() {
        let r = AwaitingReport {
            mail: MailChannel {
                unread: 3,
                urgent: 1,
                ..Default::default()
            },
            ..Default::default()
        };
        // A report with ONLY mail is non-empty and counts as one line.
        assert!(!r.is_empty());
        assert_eq!(r.total(), 1);
        let mut buf = Vec::new();
        let wrote = r.render(false, &mut buf).unwrap();
        assert!(wrote);
        let s = strip_ansi(&String::from_utf8(buf).unwrap());
        assert!(s.contains("3 unread mail"), "mail line missing:\n{s}");
        assert!(s.contains("(1 urgent)"), "urgent flag missing:\n{s}");
        assert!(
            s.contains("aida mailbox inbox"),
            "mail line must point at the inbox:\n{s}"
        );
    }

    // STORY-741: the header total counts mail as ONE line regardless of how
    // many messages are unread — the notice is a breadcrumb, not the inbox.
    #[test]
    fn mail_counts_as_one_line_in_the_header_total() {
        let r = AwaitingReport {
            mail: MailChannel {
                unread: 42,
                urgent: 0,
                ..Default::default()
            },
            findings_total: 5,
            ..Default::default()
        };
        // 1 findings line + 1 mail line = 2 (NOT 47).
        assert_eq!(r.total(), 2);
    }

    // STORY-741: the compact per-turn line spans EVERY populated channel and is
    // `None` when nothing awaits.
    //
    // STORY-769 note: the *command* `aida awaiting --notice` is no longer silent
    // when nothing awaits — it ALWAYS leads with a "Current date/time + Timing"
    // line (emitted separately by `emit_notice_time_line`, before this report is
    // even built). This `compact_line()` is the AWAITING-CHANNELS half of that
    // output and legitimately stays `None` when those channels are empty; the
    // time line carries the always-on signal. So the two halves compose: time
    // line every turn, awaiting line only when something awaits. trace:STORY-769
    #[test]
    fn compact_line_spans_all_channels_and_is_empty_when_nothing_awaits() {
        assert!(
            AwaitingReport::default().compact_line().is_none(),
            "an empty report must produce no awaiting-channels line (the time line \
             is emitted separately and is always present — STORY-769)"
        );

        let r = AwaitingReport {
            pending_briefs: vec![
                PendingBriefItem {
                    agent: "claude".into(),
                    spec_id: "TASK-1".into(),
                    path: std::path::PathBuf::from("/b/1"),
                },
                PendingBriefItem {
                    agent: "claude".into(),
                    spec_id: "TASK-2".into(),
                    path: std::path::PathBuf::from("/b/2"),
                },
            ],
            findings_total: 1,
            mail: MailChannel {
                unread: 3,
                urgent: 1,
                ..Default::default()
            },
            escalations: vec![EscalationItem {
                spec_id: "SPIKE-9".into(),
                title: "punt".into(),
            }],
            ..Default::default()
        };
        let line = r.compact_line().expect("populated report must have a line");
        // One line, spanning every channel, with counts + the urgent flag.
        assert_eq!(line.lines().count(), 1, "must be exactly one line: {line}");
        assert!(line.contains("2 briefs"), "briefs channel missing: {line}");
        assert!(
            line.contains("1 finding"),
            "findings channel missing: {line}"
        );
        assert!(line.contains("3 mail"), "mail channel missing: {line}");
        assert!(line.contains("(1 urgent)"), "urgent flag missing: {line}");
        assert!(
            line.contains("1 escalation"),
            "escalation channel missing: {line}"
        );
        assert!(
            line.contains("aida awaiting"),
            "line must point at the full view: {line}"
        );
    }

    // STORY-741: PRs are the one network-backed channel, so the per-turn line
    // omits them (the caller builds the report with `no_ci`). This asserts the
    // compact line renders cleanly from the cheap channels alone.
    #[test]
    fn compact_line_renders_from_cheap_channels_without_prs() {
        let r = AwaitingReport {
            mail: MailChannel {
                unread: 1,
                urgent: 0,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(r.mergeable_prs.is_empty());
        let line = r.compact_line().unwrap();
        assert!(line.contains("1 mail"), "{line}");
        assert!(!line.contains(" PR"), "no PR channel expected: {line}");
    }

    // STORY-741: the JSON contract gains a stable `mail` object.
    #[test]
    fn json_shape_includes_mail_channel() {
        let r = AwaitingReport {
            mail: MailChannel {
                unread: 4,
                urgent: 2,
                ..Default::default()
            },
            ..Default::default()
        };
        let v = r.to_json();
        assert_eq!(v["mail"]["unread"], 4);
        assert_eq!(v["mail"]["urgent"], 2);
        assert_eq!(v["total"], 1);
    }

    // TASK-1146: pending worker directives are now a channel in the report —
    // the count folds into the header total as ONE collapsed line (like
    // findings), names the FIFO-head directive, and points at the worker
    // directives view. trace:TASK-1146
    #[test]
    fn worker_directives_fold_into_report_as_one_line() {
        let r = AwaitingReport {
            worker_directives: DirectivesChannel {
                pending: 3,
                next: Some("human-audit /aida-human-audit".into()),
            },
            ..Default::default()
        };
        // A report with ONLY directives is non-empty and counts as one line
        // regardless of how many directives are queued.
        assert!(!r.is_empty());
        assert_eq!(r.total(), 1);
        let mut buf = Vec::new();
        let wrote = r.render(false, &mut buf).unwrap();
        assert!(wrote);
        let s = strip_ansi(&String::from_utf8(buf).unwrap());
        assert!(
            s.contains("3 pending worker directives"),
            "directives line missing:\n{s}"
        );
        assert!(
            s.contains("(next: human-audit /aida-human-audit)"),
            "FIFO-head summary missing:\n{s}"
        );
        assert!(
            s.contains("aida worker directives"),
            "line must point at the worker directives view:\n{s}"
        );
    }

    // TASK-1146: the compact per-turn notice line names the directives channel,
    // and the channel is local-file-backed so it legitimately rides the
    // network-free `--notice` path.
    #[test]
    fn compact_line_includes_worker_directives_channel() {
        let r = AwaitingReport {
            worker_directives: DirectivesChannel {
                pending: 1,
                next: Some("drain batch:x --zen".into()),
            },
            ..Default::default()
        };
        let line = r.compact_line().expect("directives alone must fire a line");
        assert!(
            line.contains("1 directive"),
            "directives channel missing: {line}"
        );
        assert!(
            line.contains("aida awaiting"),
            "line must point at the full view: {line}"
        );
    }

    // STORY-1226: due seat jobs are a channel — counted in the total, on the
    // compact per-turn line, in the rendered report, and in the JSON shape.
    // trace:STORY-1226 | ai:claude
    #[test]
    fn cron_channel_counts_in_total_and_compact_line() {
        let r = AwaitingReport {
            cron: CronChannel {
                due: 2,
                next: Some("mailbox-triage (every 30m, last 47m ago) → triage the mailbox".into()),
            },
            ..Default::default()
        };
        assert_eq!(r.total(), 1, "due jobs collapse to one row");
        assert!(!r.is_empty());
        let line = r.compact_line().expect("due jobs alone must fire a line");
        assert!(line.contains("2 due jobs"), "cron channel missing: {line}");
        let mut buf = Vec::new();
        assert!(r.render(false, &mut buf).unwrap());
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("2 due seat jobs"), "{s}");
        assert!(s.contains("mailbox-triage (every 30m"), "{s}");
        assert!(s.contains("aida schedule due"), "{s}");
        let v = r.to_json();
        assert_eq!(v["cron"]["due"], 2);
        assert!(v["cron"]["next"]
            .as_str()
            .unwrap()
            .starts_with("mailbox-triage"));
        // Singular form, and absent when nothing is due.
        let one = AwaitingReport {
            cron: CronChannel { due: 1, next: None },
            ..Default::default()
        };
        assert!(one.compact_line().unwrap().contains("1 due job"));
        assert!(AwaitingReport::default().to_json()["cron"]["next"].is_null());
        assert_eq!(AwaitingReport::default().to_json()["cron"]["due"], 0);
    }

    // TASK-1146: the JSON contract gains a stable `worker_directives` object.
    #[test]
    fn json_shape_includes_worker_directives_channel() {
        let r = AwaitingReport {
            worker_directives: DirectivesChannel {
                pending: 2,
                next: Some("human-audit /aida-human-audit".into()),
            },
            ..Default::default()
        };
        let v = r.to_json();
        assert_eq!(v["worker_directives"]["pending"], 2);
        assert_eq!(
            v["worker_directives"]["next"],
            "human-audit /aida-human-audit"
        );
        assert_eq!(v["total"], 1);

        // Empty channel serializes as pending 0 / next null (stable shape).
        let v = AwaitingReport::default().to_json();
        assert_eq!(v["worker_directives"]["pending"], 0);
        assert!(v["worker_directives"]["next"].is_null());
    }

    /// Mirrors `status_cleanup::tests::strip_ansi` so assertions can check
    /// plain text without coupling to the colour codes.
    fn strip_ansi(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut in_esc = false;
        for c in s.chars() {
            if in_esc {
                if c == 'm' {
                    in_esc = false;
                }
                continue;
            }
            if c == '\u{1b}' {
                in_esc = true;
                continue;
            }
            out.push(c);
        }
        out
    }

    fn reviewer_item(
        spec_id: &str,
        state: review_verdict::ReviewActionability,
    ) -> ReviewerQueueItem {
        ReviewerQueueItem {
            spec_id: spec_id.to_string(),
            title: format!("title for {spec_id}"),
            state,
        }
    }

    // BUG-1508 AC2/AC4/AC7: routed rows never vanish (all render, each
    // annotated), and the depth figure everywhere it's printed is
    // "actionable N of M routed" -- both numbers, because the gap between
    // them is the signal.
    #[test]
    fn reviewer_rows_all_render_and_report_actionable_of_routed() {
        let r = AwaitingReport {
            reviewer_queue_items: vec![
                reviewer_item("BUG-1", review_verdict::ReviewActionability::NeedsReview),
                reviewer_item("BUG-2", review_verdict::ReviewActionability::AwaitingRework),
                reviewer_item("BUG-3", review_verdict::ReviewActionability::Resolved),
            ],
            ..Default::default()
        };

        // The depth figure used for capacity decisions: actionable (1) of
        // routed (3) -- not a bare routed count of 3.
        let compact = r.compact_line().expect("populated report must have a line");
        assert!(compact.contains("actionable 1 of 3 routed"), "{compact}");

        let mut buf = Vec::new();
        assert!(r.render(true, &mut buf).unwrap());
        let out = strip_ansi(&String::from_utf8(buf).unwrap());
        assert!(out.contains("reviewer: actionable 1 of 3 routed"), "{out}");
        // AC2: none of the three rows vanish -- each is present, annotated
        // by its own state.
        assert!(out.contains("needs-review: BUG-1"), "{out}");
        assert!(out.contains("awaiting-rework: BUG-2"), "{out}");
        assert!(out.contains("resolved: BUG-3"), "{out}");
    }

    #[test]
    fn reviewer_state_reaches_json() {
        let r = AwaitingReport {
            reviewer_queue_items: vec![reviewer_item(
                "BUG-7",
                review_verdict::ReviewActionability::AwaitingRework,
            )],
            ..Default::default()
        };
        let v = r.to_json();
        assert_eq!(v["reviewer_queue_items"][0]["state"], "awaiting-rework");
        assert_eq!(v["reviewer_actionable_of_routed"]["actionable"], 0);
        assert_eq!(v["reviewer_actionable_of_routed"]["routed"], 1);
    }

    // BUG-1530: the reproduction from the bug report — a reviewer seat with
    // advisor-owned findings, implementer-owned shelved/rework, and its own
    // routed reviewer-queue row all populated at once. The reviewer's
    // headline must lead with its own routed section and fold the other two
    // away into a named "for other seats" count rather than leading with
    // them or silently dropping them.
    fn mixed_seat_report(role: &str) -> AwaitingReport {
        AwaitingReport {
            findings_total: 11,
            shelved_total: 8,
            reviewer_queue_items: vec![reviewer_item(
                "BUG-9",
                review_verdict::ReviewActionability::NeedsReview,
            )],
            role: Some(role.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn bug_1530_reviewer_headline_leads_with_own_routed_work_not_findings_or_rework() {
        let r = mixed_seat_report("reviewer");

        let compact = r.compact_line().expect("populated report must have a line");
        assert!(compact.contains("actionable 1 of 1 routed"), "{compact}");
        assert!(!compact.contains("finding"), "{compact}");
        assert!(!compact.contains("in rework"), "{compact}");
        // 11 findings + 8 shelved = 19 items folded away for other seats.
        assert!(compact.contains("19 for other seats"), "{compact}");

        let mut buf = Vec::new();
        assert!(r.render(true, &mut buf).unwrap());
        let out = strip_ansi(&String::from_utf8(buf).unwrap());
        assert!(out.contains("reviewer: actionable 1 of 1 routed"), "{out}");
        assert!(out.contains("needs-review: BUG-9"), "{out}");
        assert!(!out.contains("finding"), "{out}");
        assert!(!out.contains("shelved item"), "{out}");
        assert!(out.contains("19 items routed to other seats"), "{out}");
    }

    #[test]
    fn bug_1530_advisor_headline_leads_with_findings_not_reviewer_or_rework() {
        let r = mixed_seat_report("advisor");

        let compact = r.compact_line().expect("populated report must have a line");
        assert!(compact.contains("11 findings"), "{compact}");
        assert!(!compact.contains("actionable"), "{compact}");
        assert!(!compact.contains("in rework"), "{compact}");
        // 8 shelved + 1 reviewer row = 9 items folded away for other seats.
        assert!(compact.contains("9 for other seats"), "{compact}");

        let mut buf = Vec::new();
        assert!(r.render(true, &mut buf).unwrap());
        let out = strip_ansi(&String::from_utf8(buf).unwrap());
        assert!(out.contains("finding"), "{out}");
        assert!(!out.contains("needs-review: BUG-9"), "{out}");
        assert!(!out.contains("shelved item"), "{out}");
        assert!(out.contains("9 items routed to other seats"), "{out}");
    }

    #[test]
    fn bug_1530_implementer_headline_leads_with_rework_not_findings_or_reviewer() {
        let r = mixed_seat_report("implementer");

        let compact = r.compact_line().expect("populated report must have a line");
        assert!(compact.contains("8 in rework"), "{compact}");
        assert!(!compact.contains("finding"), "{compact}");
        assert!(!compact.contains("actionable"), "{compact}");
        // 11 findings + 1 reviewer row = 12 items folded away for other seats.
        assert!(compact.contains("12 for other seats"), "{compact}");
    }

    // Acceptance #5: the reviewer and advisor headlines differ, and each
    // names its own routed section — a test that only checked one seat would
    // pass vacuously.
    #[test]
    fn bug_1530_reviewer_and_advisor_headlines_differ_and_each_names_its_own_section() {
        let reviewer_line = mixed_seat_report("reviewer").compact_line().unwrap();
        let advisor_line = mixed_seat_report("advisor").compact_line().unwrap();
        assert_ne!(reviewer_line, advisor_line);
        assert!(reviewer_line.contains("routed"), "{reviewer_line}");
        assert!(advisor_line.contains("findings"), "{advisor_line}");
    }

    // Acceptance: with no role set, today's unscoped behavior is unchanged —
    // every channel appears, nothing is folded into a hidden count.
    #[test]
    fn bug_1530_no_role_keeps_todays_unscoped_behavior() {
        let mut r = mixed_seat_report("reviewer");
        r.role = None;

        let compact = r.compact_line().expect("populated report must have a line");
        assert!(compact.contains("11 findings"), "{compact}");
        assert!(compact.contains("8 in rework"), "{compact}");
        assert!(compact.contains("actionable 1 of 1 routed"), "{compact}");
        assert!(!compact.contains("for other seats"), "{compact}");

        let mut buf = Vec::new();
        assert!(r.render(true, &mut buf).unwrap());
        let out = strip_ansi(&String::from_utf8(buf).unwrap());
        assert!(out.contains("finding"), "{out}");
        assert!(out.contains("shelved item"), "{out}");
        assert!(out.contains("needs-review: BUG-9"), "{out}");
        assert!(!out.contains("routed to other seats"), "{out}");
    }

    // Acceptance #4: unshipped work isn't scoped to any seat — once a seat
    // is known it is labelled project-wide rather than implied to be this
    // seat's own gate; with no role it renders exactly as before.
    #[test]
    fn bug_1530_unshipped_work_labelled_project_wide_once_a_seat_is_known() {
        let item = UnshippedWorkItem {
            spec_id: "STORY-1".to_string(),
            branch: "story-1".to_string(),
            commits_ahead: 1,
            age: "1h".to_string(),
            recovery: "aida pr ship story-1".to_string(),
            pr_state: "absent".to_string(),
        };
        let scoped = AwaitingReport {
            unshipped_work: vec![item.clone()],
            role: Some("reviewer".to_string()),
            ..Default::default()
        };
        let unscoped = AwaitingReport {
            unshipped_work: vec![item],
            ..Default::default()
        };
        assert!(
            scoped
                .compact_line()
                .unwrap()
                .contains("unshipped (project-wide):1"),
            "{}",
            scoped.compact_line().unwrap()
        );
        assert!(
            unscoped.compact_line().unwrap().contains("unshipped:1"),
            "{}",
            unscoped.compact_line().unwrap()
        );
    }

    // The `dialog` alias normalizes to `advisor` at this boundary too, same
    // as every other role-scoped surface.
    #[test]
    fn bug_1530_dialog_alias_normalizes_to_advisor_seat() {
        assert_eq!(classify_seat(Some("dialog")), Some(AwaitingSeat::Advisor));
        assert_eq!(classify_seat(Some("advisor")), Some(AwaitingSeat::Advisor));
        assert_eq!(classify_seat(Some("")), None);
        assert_eq!(classify_seat(None), None);
    }

    /// BUG-1481: the PR-2009 shape — `ci_rollup` already carries the
    /// upstream-computed "missing" state (a required check's row never
    /// showed up on the head) rather than "pass". A PR in that state must
    /// not be classified as awaiting-you / mergeable, the same as a `fail`
    /// or `pending` rollup.
    // trace:BUG-1481 | ai:claude
    #[test]
    fn missing_required_check_is_not_awaiting_you() {
        let missing = pr(2009, Some("MERGEABLE"), Some("missing"), None);
        assert!(!is_awaiting_you(&missing, false));
        let classified = classify_open_prs(&[missing], &HashSet::new());
        assert!(
            classified.is_empty(),
            "a head missing a required check must not appear in mergeable_prs: {classified:?}"
        );
    }

    /// Same shape but the required-check set itself couldn't be read at
    /// all — must also not be classified as green.
    // trace:BUG-1481 | ai:claude
    #[test]
    fn unknown_required_check_set_is_not_awaiting_you() {
        let unknown = pr(2009, Some("MERGEABLE"), Some("unknown"), None);
        assert!(!is_awaiting_you(&unknown, false));
    }
}
