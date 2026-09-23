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
}

/// STORY-1419: one PR whose rework has landed on a refusal you recorded.
///
/// The signal is exactly `head != reviewed_sha` on a PR carrying a blocking
/// verdict. Both shas are reported and NOTHING is classified: whether the move
/// was a rebase or a real rework is the reviewer's call, and inferring it is
/// precisely the judgement that proved unreliable when attempted elsewhere.
// trace:STORY-1419 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReworkReadyItem {
    pub pr: u64,
    pub spec: Option<String>,
    /// The sha the refusal was recorded against.
    pub reviewed_sha: String,
    /// Where the PR is now.
    pub head_sha: String,
}

/// One PR's inputs to the rework-ready test, assembled by the caller so the
/// decision itself stays pure and testable without a forge or a store.
// trace:STORY-1419 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct ReworkCandidate {
    pub pr: u64,
    pub head_sha: String,
    pub spec: Option<String>,
    /// True when the recorded verdict BLOCKS (RequestChanges / Rejected).
    pub verdict_blocks: bool,
    /// True when the recorded verdict is specifically APPROVED (not merely
    /// non-blocking — `Other`/unrecognised words are deliberately excluded so
    /// this stays scoped to the exact merge-safety question BUG-1549 names).
    // trace:BUG-1549 | ai:claude
    pub verdict_approved: bool,
    /// The verdict's `reviewed_sha`, absent when the writer recorded none.
    pub reviewed_sha: Option<String>,
    /// The seat that recorded the verdict, absent when the writer recorded none.
    pub recorded_by: Option<String>,
}

/// Build one candidate from the parts the caller has, deriving the spec id from
/// the head branch.
///
/// STORY-1419 review: `spec` was hardcoded `None` at the single production call
/// site, so the advertised row could never show a spec — and the test that
/// asserted the spec renders HAND-BUILT the item and bypassed this mapping
/// entirely. The mapping is a function now precisely so a test can reach it;
/// inline construction at the call site is what made it untestable.
// trace:STORY-1419 | ai:claude
pub(crate) fn rework_candidate_from_parts(
    pr: u64,
    head_sha: &str,
    head_branch: &str,
    verdict_blocks: bool,
    verdict_approved: bool,
    reviewed_sha: Option<&str>,
    recorded_by: Option<&str>,
) -> ReworkCandidate {
    ReworkCandidate {
        pr,
        head_sha: head_sha.to_string(),
        // the branch is the only place the spec reliably appears; the verdict
        // is PR-keyed and carries no spec id of its own
        spec: crate::pr_ship::extract_spec_ids_from_text(head_branch)
            .into_iter()
            .next(),
        verdict_blocks,
        verdict_approved,
        reviewed_sha: reviewed_sha.map(str::to_string),
        recorded_by: recorded_by.map(str::to_string),
    }
}

/// Which PRs have moved past the refusal recorded against them.
///
/// Scoped to `seat` when it is known, so the row reaches the reviewer who
/// refused rather than everyone. When the seat is unknown every row is
/// returned — the same choice `pending_briefs` makes for an unidentifiable
/// agent, because a missed handoff costs more than a surplus line.
///
/// DEGRADES TO SILENCE, NEVER TO A WRONG ROW. A verdict carrying no
/// `reviewed_sha` yields nothing: there is no sha to compare, so the honest
/// output is the same silence as before rather than a guess. That is BUG-1538's
/// blast radius showing through here, and it is why this cannot claim full
/// coverage until provenance is always written.
// trace:STORY-1419 | ai:claude
pub(crate) fn rework_ready_rows(
    candidates: &[ReworkCandidate],
    seat: Option<&str>,
) -> Vec<ReworkReadyItem> {
    candidates
        .iter()
        .filter(|c| c.verdict_blocks)
        .filter(|c| match (seat, c.recorded_by.as_deref()) {
            (Some(me), Some(who)) => who.contains(me),
            // unknown on either side: surface it rather than hide it
            _ => true,
        })
        .filter_map(|c| {
            let reviewed = c
                .reviewed_sha
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())?;
            let head = c.head_sha.trim();
            // DECISION, recorded rather than inherited: only a POSITIVE Moved
            // emits a row. Same and Incomparable are both silence here, and
            // that is deliberate even though they are different states.
            //
            // A ROW SURFACE CANNOT CARRY A DISTINCTION IN THE ABSENCE OF A ROW.
            // "No row" is one state however many reasons produce it, so asking
            // Incomparable to look different from Same HERE would be asking
            // silence to have two flavours. The governing principle — that
            // absent evidence must be distinguishable from good evidence — is
            // satisfied by the distinction existing somewhere a consumer can
            // REACH, not by every surface rendering it.
            //
            // WHERE THE DISTINCTION LIVES: in `ShaRelation` itself. It is
            // three-state precisely so a caller that CAN express the
            // difference is able to. Binding on anything built later: a
            // diagnostic or verbose view over verdict staleness MUST report
            // Incomparable distinctly from Same. Collapsing it back to a
            // boolean at such a surface would be the failure this shape exists
            // to avoid — the row surface is the one place where it is correct.
            //
            // The immediate consequence is that an unusably short recorded sha
            // cannot pin a row open
            if head.is_empty() || compare_shas(head, reviewed) != ShaRelation::Moved {
                return None;
            }
            Some(ReworkReadyItem {
                pr: c.pr,
                spec: c.spec.clone(),
                reviewed_sha: reviewed.to_string(),
                head_sha: head.to_string(),
            })
        })
        .collect()
}

/// BUG-1549: one PR whose recorded APPROVAL no longer covers its head.
///
/// STORY-1419's `ReworkReadyItem`/`rework_ready_rows` answers "did rework
/// land on a REFUSAL I recorded" — its filter to `verdict_blocks` is correct
/// for that question and is deliberately left alone (BUG-1549's own "not in
/// scope"). This is a DIFFERENT question with a different audience (the
/// approver, not the reviewer-of-a-refusal) and a different remedy (do not
/// merge, not "go re-review"): did a PR's head move past an APPROVAL, so a
/// stale approval could be merged as if it still covered the current code.
// trace:BUG-1549 | ai:claude
/// Why a PR's recorded APPROVED verdict does not read as covering its
/// current head. Two distinct causes, two distinct operator remedies:
/// re-review (Stale — the head demonstrably moved past what was approved)
/// vs. can't-tell-from-here (Unverifiable — the writer recorded no sha, or
/// one too short to compare, so coverage can neither be confirmed nor
/// denied). Collapsing the two into one boolean would lose exactly the
/// distinction BUG-1549 AC1 asks the row to carry.
// trace:BUG-1549 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StaleApprovalReason {
    /// The approval was recorded against a sha the head has since moved
    /// past (`ShaRelation::Moved`).
    Stale,
    /// No `reviewed_sha` was recorded, or one too short to compare
    /// (`ShaRelation::Incomparable`) — coverage cannot be confirmed.
    Unverifiable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StaleApprovalItem {
    pub pr: u64,
    pub spec: Option<String>,
    /// The sha the approval was recorded against, when one was recorded at
    /// all — empty when `reason` is `Unverifiable` because the writer
    /// recorded none.
    pub reviewed_sha: String,
    /// Where the PR is now.
    pub head_sha: String,
    /// Why this approval doesn't read as covering `head_sha`.
    pub reason: StaleApprovalReason,
}

/// Which PRs carry an APPROVED verdict that does not provably cover the
/// current head — the merge-safety direction nothing previously detected.
///
/// A candidate with no `reviewed_sha`, or one too short to compare
/// (`ShaRelation::Incomparable`, BUG-1546), now emits a row too (reason
/// `Unverifiable`) rather than being silently dropped: PRIN-5 fail-closed
/// means indeterminate coverage must not read as "still approved" on the
/// MERGE surface either, and a suppressed-but-invisible PR is worse than a
/// suppressed-and-explained one. Only `ShaRelation::Same` — approval
/// confirmed to still cover the head — emits nothing.
///
/// NOT scoped by seat (BUG-1549 AC2): a stale or unverifiable approval must
/// show regardless of which actor recorded it, because the reader who
/// needs the warning is whoever is about to merge, not necessarily the
/// approver.
// trace:BUG-1549 | ai:claude
pub(crate) fn stale_approval_rows(candidates: &[ReworkCandidate]) -> Vec<StaleApprovalItem> {
    candidates
        .iter()
        .filter(|c| c.verdict_approved)
        .filter_map(|c| {
            let reviewed = c
                .reviewed_sha
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let head = c.head_sha.trim();
            let (reason, reviewed_sha) = match reviewed {
                None => (StaleApprovalReason::Unverifiable, String::new()),
                Some(reviewed) if head.is_empty() => {
                    (StaleApprovalReason::Unverifiable, reviewed.to_string())
                }
                Some(reviewed) => match compare_shas(head, reviewed) {
                    ShaRelation::Same => return None,
                    ShaRelation::Moved => (StaleApprovalReason::Stale, reviewed.to_string()),
                    ShaRelation::Incomparable => {
                        (StaleApprovalReason::Unverifiable, reviewed.to_string())
                    }
                },
            };
            Some(StaleApprovalItem {
                pr: c.pr,
                spec: c.spec.clone(),
                reviewed_sha,
                head_sha: head.to_string(),
                reason,
            })
        })
        .collect()
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
#[derive(Debug, PartialEq, Eq)]
enum ShaRelation {
    Same,
    Moved,
    Incomparable,
}

/// Shortest prefix worth comparing. Below this a match is coincidence rather
/// than evidence.
const MIN_COMPARABLE_SHA: usize = 7;

fn compare_shas(head: &str, reviewed: &str) -> ShaRelation {
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
            + (if self.nightly_red.is_some() { 1 } else { 0 })
            + self.reviewer_queue_items.len()
            // BUG-1508 AC4/AC7: the "actionable N of M routed" summary line.
            + (if !self.reviewer_queue_items.is_empty() { 1 } else { 0 })
            + (if self.shelved_total > 0 { 1 } else { 0 })
            + self.escalations.len()
            + self.pr_attribution_disagreements.len()
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
            writeln!(
                w,
                "  {} PR-{}{} moved past your refusal — reviewed {}, now {}",
                "🔄".cyan(),
                item.pr.to_string().bold(),
                spec,
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
            writeln!(
                w,
                "  {} PR-{} ready to merge — {} · {}",
                "🟢".green(),
                pr.number.to_string().bold(),
                pr.title,
                ci,
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
        if self.findings_total > 0 {
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
            writeln!(
                w,
                "  🧭 unshipped work: {} on `{}` — {} commit{} ahead, age {}, PR {} — `{}`",
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
        if !self.reviewer_queue_items.is_empty() {
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
                review_verdict::ReviewActionability::AwaitingRework => ("🔧", "awaiting-rework"),
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
        if self.shelved_total > 0 {
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

        if overflow > 0 {
            writeln!(
                w,
                "  {} {} more — `{}`",
                "…".dimmed(),
                overflow,
                "aida status --awaiting --verbose".cyan(),
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
            "nightly_red": self.nightly_red.as_ref().map(|n| serde_json::json!({
                "summary": n.summary,
                "run_id": n.run_id,
                "nights": n.nights,
            })),
            // trace:BUG-1549 | ai:claude
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
        if !self.pending_briefs.is_empty() {
            parts.push(pluralize(self.pending_briefs.len(), "brief", "briefs"));
        }
        if self.findings_total > 0 {
            parts.push(pluralize(self.findings_total, "finding", "findings"));
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
            parts.push(format!("unshipped:{}", self.unshipped_work.len()));
        }
        if self.nightly_red.is_some() {
            parts.push("nightly-red".to_string());
        }
        if !self.reviewer_queue_items.is_empty() {
            // BUG-1508 AC4/AC7: "actionable N of M routed" everywhere this
            // depth figure is printed, including the compact per-turn line.
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
        }
        if self.shelved_total > 0 {
            parts.push(format!("{} in rework", self.shelved_total));
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
///   - CI is not failing or pending (pass / no-checks / `?` are fine)
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
        Some("fail") | Some("pending") => return false,
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
/// `local_blocking` names the PRs (by number) for which the caller already
/// resolved an AIDA-recorded RequestChanges/Rejected verdict at the PR's
/// CURRENT head (see `pr_has_local_blocking_verdict_at_head` in lib.rs) OR an
/// AIDA-recorded APPROVED verdict that does not provably cover that head —
/// stale, sha-less, or Incomparable (see
/// `pr_has_stale_or_unverifiable_local_approval` in lib.rs, BUG-1549) — the
/// caller unions both into this one set. The two sources are unioned with
/// GitHub's `review_decision`, never swapped.
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
        }
    }

    fn to(agent: &str) -> aida_core::mailbox::Recipient {
        aida_core::mailbox::Recipient::Agent(agent.to_string())
    }

    fn candidate(pr: u64, head: &str, reviewed: Option<&str>, by: Option<&str>) -> ReworkCandidate {
        ReworkCandidate {
            pr,
            head_sha: head.to_string(),
            spec: Some(format!("BUG-{pr}")),
            verdict_blocks: true,
            verdict_approved: false,
            reviewed_sha: reviewed.map(str::to_string),
            recorded_by: by.map(str::to_string),
        }
    }

    // trace:BUG-1549 | ai:claude
    fn approved_candidate(
        pr: u64,
        head: &str,
        reviewed: Option<&str>,
        by: Option<&str>,
    ) -> ReworkCandidate {
        ReworkCandidate {
            verdict_blocks: false,
            verdict_approved: true,
            ..candidate(pr, head, reviewed, by)
        }
    }

    // STORY-1419: the signal is head != reviewed_sha on a blocking verdict.
    // trace:STORY-1419 | ai:claude
    #[test]
    fn a_moved_head_on_a_blocking_verdict_is_rework_ready() {
        let rows = rework_ready_rows(
            &[candidate(
                2014,
                "f95b30853e",
                Some("3310400503"),
                Some("claude-reviewer-1"),
            )],
            None,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].pr, 2014);
        assert_eq!(rows[0].reviewed_sha, "3310400503");
        assert_eq!(rows[0].head_sha, "f95b30853e");
    }

    // The unmoved head is the common case and must stay silent, or the row
    // fires on every held PR and stops meaning anything.
    // trace:STORY-1419 | ai:claude
    #[test]
    fn an_unmoved_head_is_not_rework_ready() {
        assert!(rework_ready_rows(
            &[candidate(2001, "ffac563445", Some("ffac563445"), None)],
            None
        )
        .is_empty());
    }

    // Verdict writers record full or short shas depending on the path; a
    // short-vs-long pair is the SAME commit, not a moved head.
    // trace:STORY-1419 | ai:claude
    #[test]
    fn a_truncated_sha_is_not_a_moved_head() {
        assert!(rework_ready_rows(
            &[candidate(
                2030,
                "293da2d0cc9404f5226ad4deef89c0bc37e97c81",
                Some("293da2d0cc"),
                None
            )],
            None
        )
        .is_empty());
    }

    // The edge the abbreviated-sha corpus exposed: a sha SHORTER THAN THE
    // COMPARISON FLOOR is unusable evidence, and the failure is asymmetric.
    // Treating it as "different" emits a row that NO PUSH CAN EVER CLEAR — a
    // 3-character string never becomes equal to a 40-character one — so the
    // wrong answer here is permanent, not transient. Silence is the only
    // output consistent with the contract the absent-provenance case sets.
    // trace:BUG-1546 | ai:claude
    #[test]
    fn a_sha_too_short_to_compare_is_silence_not_a_permanent_row() {
        let head = "293da2d0cc9404f5226ad4deef89c0bc37e97c81";
        for short in ["2", "29", "293", "293d", "293da", "293da2"] {
            assert!(
                rework_ready_rows(&[candidate(2030, head, Some(short), None)], None).is_empty(),
                "a {}-char reviewed_sha is too short to be evidence, so it must \
                 not pin a row open forever (got one for {short:?})",
                short.len()
            );
        }
        // …and a sha that DISAGREES below the floor is equally unusable: the
        // floor is about comparability, not about which way the bytes fall.
        for short in ["f", "ff", "fff", "ffff", "fffff", "ffffff"] {
            assert!(
                rework_ready_rows(&[candidate(2030, head, Some(short), None)], None).is_empty(),
                "a sub-floor sha must be silence whichever way its bytes fall ({short:?})"
            );
        }
        // The floor is exactly 7: one more character and comparison resumes,
        // so this pins the boundary rather than merely "short is quiet".
        assert!(
            rework_ready_rows(&[candidate(2030, head, Some("293da2d"), None)], None).is_empty(),
            "7 matching chars is comparable and matching — silence"
        );
        assert_eq!(
            rework_ready_rows(&[candidate(2030, head, Some("fffffff"), None)], None).len(),
            1,
            "7 DIFFERING chars is comparable and moved — the row must fire"
        );
    }

    // BUG-1538's blast radius: no reviewed_sha means nothing to compare, so the
    // honest output is silence. Asserted explicitly so a later change that
    // starts GUESSING here fails loudly.
    // trace:STORY-1419 | ai:claude
    #[test]
    fn a_verdict_without_provenance_degrades_to_silence_not_a_guess() {
        assert!(rework_ready_rows(&[candidate(2009, "065f3df8aa", None, None)], None).is_empty());
        assert!(
            rework_ready_rows(&[candidate(2009, "065f3df8aa", Some("   "), None)], None).is_empty(),
            "a blank reviewed_sha is absent provenance, not a sha"
        );
    }

    // A non-blocking verdict is not a refusal, so its head moving is ordinary
    // progress rather than something the reviewer gates.
    // trace:STORY-1419 | ai:claude
    #[test]
    fn a_non_blocking_verdict_never_produces_a_row() {
        let mut approved = candidate(2046, "f56e089371", Some("be6a8eecb5"), None);
        approved.verdict_blocks = false;
        assert!(rework_ready_rows(&[approved], None).is_empty());
    }

    // Scoped to the seat that refused — and UNKNOWN on either side surfaces
    // rather than hides, because a missed handoff costs more than a spare line.
    // trace:STORY-1419 | ai:claude
    #[test]
    fn rows_are_scoped_to_the_refusing_seat_but_unknown_surfaces() {
        let mine = candidate(
            2014,
            "aaa1111",
            Some("bbb2222"),
            Some("claude-reviewer-1 (claude reviewer seat)"),
        );
        let theirs = candidate(
            2030,
            "ccc3333",
            Some("ddd4444"),
            Some("aida drain reviewer"),
        );
        let anon = candidate(2040, "eee5555", Some("fff6666"), None);

        let scoped = rework_ready_rows(
            &[mine.clone(), theirs.clone(), anon.clone()],
            Some("claude-reviewer-1"),
        );
        let prs: Vec<u64> = scoped.iter().map(|r| r.pr).collect();
        assert_eq!(prs, vec![2014, 2040], "mine plus the unattributable one");

        let unscoped = rework_ready_rows(&[mine, theirs, anon], None);
        assert_eq!(unscoped.len(), 3, "with no seat known, surface everything");
    }

    // ── BUG-1549: stale-approval detection ──────────────────────────────────
    // The merge-safety direction: an APPROVED verdict whose reviewed_sha is
    // not the PR's current head must surface as stale and must NOT read as
    // merge-ready.

    // Acceptance: approval AT the head is fine — no row.
    // trace:BUG-1549 | ai:claude
    #[test]
    fn an_approval_at_the_head_is_not_stale() {
        assert!(
            stale_approval_rows(&[approved_candidate(2060, "aaa1111", Some("aaa1111"), None)])
                .is_empty()
        );
    }

    // Acceptance: approval BEHIND the head — the head moved past the
    // approval — must be flagged. This is exactly the case nothing
    // previously detected because `rework_ready_rows` filters to blocking
    // verdicts first, structurally excluding every APPROVED verdict.
    // trace:BUG-1549 | ai:claude
    #[test]
    fn an_approval_behind_the_head_is_flagged_as_stale() {
        let rows = stale_approval_rows(&[approved_candidate(
            2060,
            "1aca4e3e9251",
            Some("08834c6045a9"),
            Some("claude-reviewer-1"),
        )]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].pr, 2060);
        assert_eq!(rows[0].reviewed_sha, "08834c6045a9");
        assert_eq!(rows[0].head_sha, "1aca4e3e9251");
        assert_eq!(rows[0].reason, StaleApprovalReason::Stale);
    }

    // Acceptance (BUG-1549 AC1): an approval recorded with no reviewed_sha is
    // indeterminate and must NOT read as a covering approval — and, unlike
    // before, must not vanish from the awaiting/mergeable surface either. It
    // now emits its own row (reason `Unverifiable`) so the operator sees WHY
    // the PR isn't reading as safe to merge, distinct from a genuinely stale
    // (head-moved) approval. This asserts on the surface this branch changes
    // (`stale_approval_rows`), not on `review_verdict::review_actionability`,
    // which is a different classifier answering a different question.
    // trace:BUG-1549 | ai:claude
    #[test]
    fn an_approval_without_a_sha_is_treated_as_absent_not_covering() {
        let rows = stale_approval_rows(&[approved_candidate(2060, "1aca4e3e9251", None, None)]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].pr, 2060);
        assert_eq!(rows[0].reviewed_sha, "");
        assert_eq!(rows[0].reason, StaleApprovalReason::Unverifiable);
    }

    // A blocking verdict is not an approval, so its head moving is
    // STORY-1419's row, not this one — the two rows are deliberately
    // disjoint rather than one row answering both questions.
    // trace:BUG-1549 | ai:claude
    #[test]
    fn a_blocking_verdict_never_produces_a_stale_approval_row() {
        assert!(stale_approval_rows(&[candidate(
            2061,
            "1aca4e3e9251",
            Some("08834c6045a9"),
            None
        )])
        .is_empty());
    }

    // BUG-1549 AC2: NOT scoped by seat — a stale or unverifiable approval
    // must show regardless of which actor recorded it.
    // trace:BUG-1549 | ai:claude
    #[test]
    fn stale_approvals_surface_regardless_of_the_recording_actor() {
        let mine = approved_candidate(
            2060,
            "aaa1111",
            Some("bbb2222"),
            Some("claude-reviewer-1 (claude reviewer seat)"),
        );
        let theirs = approved_candidate(2061, "ccc3333", Some("ddd4444"), Some("someone-else"));

        let rows = stale_approval_rows(&[mine, theirs]);
        assert_eq!(
            rows.iter().map(|r| r.pr).collect::<Vec<_>>(),
            vec![2060, 2061],
            "both rows must surface regardless of who recorded the approval"
        );
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

    // STORY-1419 review: the row advertises a spec id and the production call
    // site hardcoded `spec: None`, so it could never appear. My plumbing test
    // hand-built the item and bypassed the mapping — the same "a unit test
    // cannot see its own seam" failure, committed inside the test written to
    // prevent it.
    //
    // This goes through the PRODUCTION mapping: branch -> spec -> row -> render.
    // Nothing is hand-built except the inputs the forge would supply.
    // trace:STORY-1419 | ai:claude
    #[test]
    fn the_spec_reaches_the_row_through_the_production_mapping() {
        let candidate = rework_candidate_from_parts(
            2014,
            "f95b30853e",
            "task-1298-work",
            true,
            false,
            Some("3310400503"),
            None,
        );
        assert_eq!(
            candidate.spec.as_deref(),
            Some("TASK-1298"),
            "the spec must be derived from the branch, not left None"
        );

        let rows = rework_ready_rows(&[candidate], None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].spec.as_deref(), Some("TASK-1298"));

        let r = AwaitingReport {
            rework_ready: rows,
            ..Default::default()
        };
        let mut buf = Vec::new();
        r.render(false, &mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(
            out.contains("TASK-1298"),
            "the spec must survive all the way to the rendered row: {out}"
        );
    }

    // A branch carrying no spec id must still produce a row — the PR number is
    // the actionable part. Without this, "derive the spec" could be implemented
    // as "drop rows with no spec" and the suite would not notice.
    // trace:STORY-1419 | ai:claude
    #[test]
    fn a_branch_without_a_spec_id_still_produces_a_row() {
        let candidate = rework_candidate_from_parts(
            2047,
            "58fffe27b9",
            "some-unlabelled-branch",
            true,
            false,
            Some("aaaa1111"),
            None,
        );
        assert_eq!(candidate.spec, None);
        assert_eq!(rework_ready_rows(&[candidate], None).len(), 1);
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
}
