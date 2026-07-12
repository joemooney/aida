//! Blocked/waiting board — the TUI flow-cockpit home view (STORY-686).
//!
//! Re-orients the `aida tui` home from a status browser to a flow cockpit:
//! one board that answers "why isn't this moving, who must act, how do I
//! unblock it." Each open spec is grouped by the **reason** it is not
//! progressing; selecting a reason fills the list pane with its items;
//! Enter dispatches that reason's lightest unblock action.
//!
//! The board is a *read-cockpit-that-dispatches*: it composes only
//! cache-fast `aida` reads (never `aida status`, ~3.75s — BUG-616), and
//! when the operator acts it launches the relevant `aida` subcommand
//! rather than reimplementing approve / questions / triage inside the TUI.
//!
//! Precedence (so each spec lands in exactly ONE group): in_flight >
//! blocked > needs-attention > awaiting-review > needs-answer >
//! needs-approval > deferred.
//!
//! trace:STORY-686 | ai:claude

use crate::dashboard::{parse_list_json, ListRow, RowKind};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// One of the seven reason-groups the cockpit surfaces. Ordered by the
/// precedence used to assign a spec to exactly one group (highest first):
/// a spec matching several flags belongs to the first reason it matches.
/// trace:STORY-686 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// A live lease holds the spec — work is moving, shown for awareness.
    InFlight,
    /// A `BlockedBy` edge points at an incomplete spec — waiting on a dep.
    Blocked,
    /// Parked in `NeedsAttention` (a punt) — an implementer must triage.
    NeedsAttention,
    /// Done on a branch with an open PR — a reviewer must act.
    AwaitingReview,
    /// A pending `DecisionRequest` — a human must answer.
    NeedsAnswer,
    /// A `Draft` spec — you / the advisor must approve (or reject).
    NeedsApproval,
    /// Parked on the deferred shelf — returns on its revisit trigger.
    Deferred,
    /// Unread mail in your inbox (STORY-701). Unlike every other variant,
    /// this is NOT produced by [`classify`] — mail isn't a spec, so it never
    /// enters the precedence walk over [`BoardInputs`]. It exists in the
    /// `Reason` taxonomy purely so the mailbox group shares the same
    /// `label`/`owner`/`row_kind` machinery (and the same Nav-section
    /// enumeration via [`Reason::all`]) every other cockpit group uses. Its
    /// rows come from [`mail_rows`], not [`rows_for`].
    // trace:STORY-701 | ai:claude
    Mail,
}

impl Reason {
    /// Precedence-ordered list, highest priority first, PLUS the mail group
    /// last. Only the first seven participate in the spec-classification
    /// precedence [`classify`] walks — [`Reason::Mail`] rides along so the
    /// Nav enumerates it too ([`crate::nav::NavSection::all`]), but it is
    /// never assigned to a spec.
    // trace:STORY-701 | ai:claude
    pub fn all() -> [Reason; 8] {
        [
            Reason::InFlight,
            Reason::Blocked,
            Reason::NeedsAttention,
            Reason::AwaitingReview,
            Reason::NeedsAnswer,
            Reason::NeedsApproval,
            Reason::Deferred,
            Reason::Mail,
        ]
    }

    /// Short label shown in the Nav pane (before the `(count) · owner`).
    pub fn label(self) -> &'static str {
        match self {
            Reason::InFlight => "in flight",
            Reason::Blocked => "blocked by dep",
            Reason::NeedsAttention => "needs attention",
            Reason::AwaitingReview => "awaiting review",
            Reason::NeedsAnswer => "needs an answer",
            Reason::NeedsApproval => "needs approval",
            Reason::Deferred => "deferred",
            Reason::Mail => "mail",
        }
    }

    /// Who must act on this reason — shown after the count in the Nav. A thin
    /// display shim over [`Reason::owner_class`], preserving the original
    /// `&'static str` so the existing nav row-label formatting is unchanged.
    pub fn owner(self) -> &'static str {
        self.owner_class().label()
    }

    /// The typed default owner of this reason-group — the full owner set
    /// STORY-702 classifies. `NeedsApproval` / `NeedsAnswer` are yours; the
    /// in-motion and handed-off reasons belong to their role (implementer /
    /// reviewer); blocked / deferred have no actor (dependency / trigger). The
    /// advisor-backlog SUB-class of needs-approval is refined to
    /// [`Owner::Advisor`] at the item level by [`item_owner`] — a reason alone
    /// can't see the sub-class flag.
    // trace:STORY-702 | ai:claude
    pub fn owner_class(self) -> Owner {
        match self {
            Reason::InFlight => Owner::Implementer,
            Reason::Blocked => Owner::Dependency,
            Reason::NeedsAttention => Owner::Implementer,
            Reason::AwaitingReview => Owner::Reviewer,
            Reason::NeedsAnswer => Owner::You,
            Reason::NeedsApproval => Owner::You,
            Reason::Deferred => Owner::Trigger,
            // Your unread mail is yours to read/reply to. trace:STORY-701
            Reason::Mail => Owner::You,
        }
    }

    /// The [`RowKind`] every row in this group carries, so the launcher's
    /// Enter handler can pick the right dispatch Intent.
    pub fn row_kind(self) -> RowKind {
        match self {
            Reason::InFlight => RowKind::ReasonInFlight,
            Reason::Blocked => RowKind::ReasonBlocked,
            Reason::NeedsAttention => RowKind::ReasonNeedsAttention,
            Reason::AwaitingReview => RowKind::ReasonAwaitingReview,
            Reason::NeedsAnswer => RowKind::ReasonNeedsAnswer,
            Reason::NeedsApproval => RowKind::ReasonNeedsApproval,
            Reason::Deferred => RowKind::ReasonDeferred,
            Reason::Mail => RowKind::ReasonMail,
        }
    }
}

/// The full set of actors the cockpit can name as owning a reason-group —
/// the "who must act" axis STORY-702 classifies. Every open spec's reason maps
/// to exactly one default owner via [`Reason::owner_class`]; a needs-approval
/// row is further refined by [`item_owner`] (a blessed-but-unrouted
/// advisor-backlog row is the advisor's to route, not yours).
///
/// This is CLASSIFICATION only. The lens/toggle GESTURE that re-groups the
/// board *by* this owner axis is EPIC-54's action->target verb and is
/// deliberately NOT built here.
// trace:STORY-702 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    /// You (the operator) must personally act: approve/reject a draft, answer
    /// a pending decision, or read/reply to mail.
    You,
    /// The advisor seat must act: a blessed-but-unrouted (advisor-backlog)
    /// spec awaiting routing onto the implementer queue. Constructed only by
    /// [`item_owner`], whose consumer is EPIC-54's owner-lens shell.
    #[allow(dead_code)] // consumed by the EPIC-54 owner-lens shell. trace:STORY-702
    Advisor,
    /// An implementer must act: work is in flight, or a punt needs triage.
    Implementer,
    /// A reviewer must act: Done-on-branch with an open PR awaiting review.
    Reviewer,
    /// No actor — waiting on an incomplete dependency to clear.
    Dependency,
    /// No actor — parked until a revisit trigger fires.
    Trigger,
}

impl Owner {
    /// The short label shown after a reason-group's count in the Nav pane.
    /// Preserves the exact strings the pre-STORY-702 `Reason::owner()` emitted
    /// (`you` / `impl` / `reviewer` / `wait` / `trigger`) so the nav rendering
    /// is unchanged; `advisor` is new (only reachable via [`item_owner`]).
    // trace:STORY-702 | ai:claude
    pub fn label(self) -> &'static str {
        match self {
            Owner::You => "you",
            Owner::Advisor => "advisor",
            Owner::Implementer => "impl",
            Owner::Reviewer => "reviewer",
            Owner::Dependency => "wait",
            Owner::Trigger => "trigger",
        }
    }
}

/// A spec the classifier has assigned to exactly one reason-group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifiedItem {
    pub spec_id: String,
    pub title: String,
    pub status: String,
    pub reason: Reason,
    /// True for the **advisor-backlog** sub-class of the needs-approval group
    /// (TASK-901): an `Approved`-but-not-yet-queued spec that the advisor has
    /// blessed but not routed. These ride the same `NeedsApproval` reason as
    /// drafts but want a different unblock action (queue it, not approve it)
    /// and a backlog label, so the advisor's pending queue stops being a black
    /// box. Drafts (the approve-or-reject sub-class) have this `false`.
    /// trace:TASK-901 | ai:claude
    pub advisor_backlog: bool,
    /// True for a **live intake-proposal** row (TASK-904): a spec the headless
    /// `aida intake` candidate fence weighs — the actual proposal set the
    /// cold-boot advisor pass considers, as opposed to TASK-901's cache-fast
    /// `Approved`-but-not-queued proxy (which an `aida intake` candidate set
    /// can also surface DRAFTS the proxy misses). These ride the same
    /// `NeedsApproval` reason but carry their own label + dispatch (Enter fires
    /// `aida intake` on that scope). The fence is a heavyweight `claude -p`-
    /// backed read (seconds-to-minutes), so it is filled async / on demand,
    /// never on the paint path. Cache-fast rows (drafts, advisor-backlog) have
    /// this `false`.
    // trace:TASK-904 | ai:claude
    pub intake_proposal: bool,
    /// The one-line reason this spec is PARKED — surfaced inline by the cockpit's
    /// advisor-backlog panel so a parked item explains itself (STORY-703): the
    /// deferred shelf's revisit trigger, a punt/needs-attention note, or the
    /// advisor-backlog "blessed but not routed" status. `None` for a row that is
    /// actively moving (in-flight) or already handed off (awaiting review) — those
    /// aren't parked. Computed by [`park_reason`] during [`classify`].
    // trace:STORY-703 | ai:claude
    pub park_reason: Option<String>,
}

/// The cheap (non-network) inputs the classifier needs. Each Vec is the
/// raw `aida list … --json` row set for its status filter; the `--blocked`
/// set carries the `blocked` / `in_flight` flags. Kept as a struct so the
/// pure classifier is unit-testable from fixtures without shelling out.
/// trace:STORY-686 | ai:claude
#[derive(Debug, Clone, Default)]
pub struct BoardInputs {
    /// `aida list --json --blocked`: every open spec, carrying the
    /// `blocked` / `in_flight` / `queued` routing flags. Source for the
    /// in-flight and blocked reasons.
    pub all_rows: Vec<crate::dashboard::ListJsonRow>,
    /// `aida list --status draft --json`: drafts awaiting approval.
    pub draft_rows: Vec<crate::dashboard::ListJsonRow>,
    /// `aida list --status needsattention --json`: punted specs.
    pub needs_attention_rows: Vec<crate::dashboard::ListJsonRow>,
    /// `aida list --status done --json`: Done-on-branch, awaiting merge.
    pub done_rows: Vec<crate::dashboard::ListJsonRow>,
    /// `aida list --deferred --json`: the primed shelf.
    pub deferred_rows: Vec<crate::dashboard::ListJsonRow>,
    /// Spec ids with a pending `DecisionRequest` (parsed from
    /// `aida questions list`).
    pub pending_question_ids: Vec<String>,
}

/// Classify every input row into exactly one reason-group, honoring the
/// precedence in [`Reason::all`]. A spec id is claimed by the highest
/// reason it matches and never double-counted. Returns the items grouped
/// per reason, preserving each source's row order.
///
/// Precedence is enforced by tracking which ids are already claimed: the
/// in-flight pass runs first, then blocked, …, then deferred, and each
/// later pass skips ids an earlier (higher-priority) pass took.
/// trace:STORY-686 | ai:claude
pub fn classify(inputs: &BoardInputs) -> Vec<ClassifiedItem> {
    use std::collections::HashSet;
    let mut claimed: HashSet<String> = HashSet::new();
    let mut out: Vec<ClassifiedItem> = Vec::new();

    let mut take = |id: &str,
                    title: &str,
                    status: &str,
                    reason: Reason,
                    advisor_backlog: bool,
                    claimed: &mut HashSet<String>| {
        if claimed.insert(id.to_string()) {
            out.push(ClassifiedItem {
                spec_id: id.to_string(),
                title: title.to_string(),
                status: status.to_string(),
                reason,
                advisor_backlog,
                // The cache-fast classify pass never produces intake-proposal
                // rows; those merge in async via `merge_intake_proposals` after
                // the heavyweight `aida intake` fence lands. trace:TASK-904
                intake_proposal: false,
                // STORY-703: the structural park reason (deferred shelf rows get
                // their real revisit trigger patched in below, once the deferred
                // pass has the row in hand). trace:STORY-703
                park_reason: park_reason(reason, advisor_backlog, None, None, None),
            });
        }
    };

    // 1. in flight — from the --blocked enrichment's in_flight flag.
    for r in inputs.all_rows.iter().filter(|r| r.in_flight) {
        take(
            &r.spec_id,
            &r.title,
            &r.status,
            Reason::InFlight,
            false,
            &mut claimed,
        );
    }
    // 2. blocked by dependency — the --blocked enrichment's blocked flag.
    for r in inputs.all_rows.iter().filter(|r| r.blocked) {
        take(
            &r.spec_id,
            &r.title,
            &r.status,
            Reason::Blocked,
            false,
            &mut claimed,
        );
    }
    // 3. needs attention — the NeedsAttention status query.
    for r in &inputs.needs_attention_rows {
        take(
            &r.spec_id,
            &r.title,
            &r.status,
            Reason::NeedsAttention,
            false,
            &mut claimed,
        );
    }
    // 4. awaiting review — Done-on-branch (PR rows lazy-fill on top later).
    for r in &inputs.done_rows {
        take(
            &r.spec_id,
            &r.title,
            &r.status,
            Reason::AwaitingReview,
            false,
            &mut claimed,
        );
    }
    // 5. needs an answer — specs with a pending DecisionRequest. Title is
    //    looked up from the all-rows set when present; falls back to the id.
    for id in &inputs.pending_question_ids {
        let (title, status) = inputs
            .all_rows
            .iter()
            .find(|r| &r.spec_id == id)
            .map(|r| (r.title.clone(), r.status.clone()))
            .unwrap_or_else(|| (String::new(), "Needs input".to_string()));
        take(
            id,
            &title,
            &status,
            Reason::NeedsAnswer,
            false,
            &mut claimed,
        );
    }
    // 6a. needs approval — drafts (the approve-or-reject sub-class).
    for r in &inputs.draft_rows {
        take(
            &r.spec_id,
            &r.title,
            &r.status,
            Reason::NeedsApproval,
            false,
            &mut claimed,
        );
    }
    // 6b. advisor backlog — Approved-but-not-queued specs (TASK-901). The
    //     advisor has blessed these but not routed them; they're the
    //     cache-fast proxy for "what the advisor/intake pass would dispose of"
    //     and ride the needs-approval group so the advisor's queue stops being
    //     a black box. Derived from the same `--blocked` all-rows set (zero
    //     extra shell-out): Approved, not queued, not in-flight, not blocked —
    //     the higher-precedence passes above already claimed in-flight/blocked
    //     specs, so the `claimed` guard keeps each spec in exactly one group.
    //     trace:TASK-901 | ai:claude
    for r in inputs.all_rows.iter().filter(|r| is_advisor_backlog(r)) {
        take(
            &r.spec_id,
            &r.title,
            &r.status,
            Reason::NeedsApproval,
            true,
            &mut claimed,
        );
    }
    // 7. deferred — the primed shelf.
    for r in &inputs.deferred_rows {
        take(
            &r.spec_id,
            &r.title,
            &r.status,
            Reason::Deferred,
            false,
            &mut claimed,
        );
    }

    // STORY-703: patch each deferred item's park reason with its REAL revisit
    // trigger (`aida defer --until "<cond>"`, carried on the JSON row as
    // `deferred_until`). The `take` pass above stamped a generic deferred reason;
    // here we overwrite it with the trigger so the cockpit shows "returns when:
    // <cond>" inline rather than a placeholder. Only the deferred rows carry a
    // trigger, so this leaves every other tier's structural reason intact.
    // trace:STORY-703 | ai:claude
    for r in &inputs.deferred_rows {
        if let Some(item) = out
            .iter_mut()
            .find(|it| it.spec_id == r.spec_id && it.reason == Reason::Deferred)
        {
            item.park_reason = park_reason(
                Reason::Deferred,
                false,
                r.deferred_until.as_deref(),
                None,
                None,
            );
        }
    }

    out
}

/// Project a parked spec's metadata into the one-line reason it sits in the
/// advisor's backlog instead of moving — the content STORY-703 surfaces inline.
/// A PURE function over the classified facts plus the optional content slots
/// (revisit trigger / punt note / finding), so it is unit-testable from a fixture
/// of park states without the TUI shell.
///
/// Returns `None` for a row that is actively MOVING ([`Reason::InFlight`]) or
/// already handed off ([`Reason::AwaitingReview`]) — those are not "parked", so
/// they carry no park reason. Every other reason maps to a short explanation:
///   * `Deferred` -> "returns when: <trigger>" (the revisit condition)
///   * `NeedsAttention` -> "parked: <punt note>" (a punt the advisor must triage)
///   * `NeedsApproval` -> the advisor-backlog "blessed, awaiting routing" line, or
///     the draft "awaiting an approve/reject verdict" line
///   * `NeedsAnswer` -> "waiting on a decision[: <finding>]"
///   * `Blocked` -> "blocked by an incomplete dependency"
// trace:STORY-703 | ai:claude
pub fn park_reason(
    reason: Reason,
    advisor_backlog: bool,
    revisit_trigger: Option<&str>,
    punt_note: Option<&str>,
    finding: Option<&str>,
) -> Option<String> {
    fn clean(s: Option<&str>) -> Option<&str> {
        s.map(str::trim).filter(|t| !t.is_empty())
    }
    match reason {
        Reason::Deferred => Some(match clean(revisit_trigger) {
            Some(t) => format!("returns when: {t}"),
            None => "deferred — no revisit trigger recorded".to_string(),
        }),
        Reason::NeedsAttention => Some(match clean(punt_note).or_else(|| clean(finding)) {
            Some(n) => format!("parked: {n}"),
            None => "needs attention — parked for triage".to_string(),
        }),
        Reason::NeedsApproval => Some(if advisor_backlog {
            "blessed by the advisor — awaiting routing to the implementer queue".to_string()
        } else {
            "awaiting an approve/reject verdict".to_string()
        }),
        Reason::NeedsAnswer => Some(match clean(finding) {
            Some(f) => format!("waiting on a decision: {f}"),
            None => "waiting on a human decision".to_string(),
        }),
        Reason::Blocked => Some("blocked by an incomplete dependency".to_string()),
        // Mail is live communication, not a parked/blocked spec — never
        // produced by `classify`, so this arm is unreachable in practice, but
        // kept for exhaustiveness. trace:STORY-701
        Reason::InFlight | Reason::AwaitingReview | Reason::Mail => None,
    }
}

/// The total advisor-queue depth: how many classified items the advisor still
/// owns a disposition on — the drafts awaiting an approve/reject verdict PLUS the
/// advisor-backlog (Approved-but-not-queued) specs the advisor has blessed but
/// not routed. Both ride the [`Reason::NeedsApproval`] group. Surfacing this
/// count (STORY-703) stops the advisor's pending queue being a black box. Pure
/// over the item set.
// trace:STORY-703 | ai:claude
pub fn advisor_queue_depth(items: &[ClassifiedItem]) -> usize {
    items
        .iter()
        .filter(|it| it.reason == Reason::NeedsApproval)
        .count()
}

/// Refine a classified item to its actual owner (STORY-702) — the same as
/// `item.reason.owner_class()` EXCEPT an advisor-backlog needs-approval row (an
/// Approved-but-not-queued spec the advisor has blessed but not routed) is
/// owned by the ADVISOR, who must route it — not you. A reason alone can't see
/// the sub-class flag, so this item-level refinement is where [`Owner::Advisor`]
/// is produced. Pure over the item.
// trace:STORY-702 | ai:claude
// The owner-lens shell that consumes this is EPIC-54's; the classifier lands
// ahead of it as a pure, unit-tested function (per the EPIC-53↔54 seam plan).
#[allow(dead_code)]
pub fn item_owner(item: &ClassifiedItem) -> Owner {
    if item.reason == Reason::NeedsApproval && item.advisor_backlog {
        Owner::Advisor
    } else {
        item.reason.owner_class()
    }
}

/// The "you-plate": everything YOU (the operator) must personally clear,
/// aggregated into ONE view regardless of which reason-group each item fell
/// into — the needs-approval drafts + the needs-answer decisions + your unread
/// mail. Pure over the classified item set plus an `unread_mail` count.
///
/// Classification + aggregation ONLY (STORY-702). The lens/toggle GESTURE that
/// swaps the board into this owner-grouped view is EPIC-54's action->target
/// verb and is deliberately NOT built here.
// trace:STORY-702 | ai:claude
// Lands ahead of its EPIC-54 owner-lens consumer as a pure aggregation.
#[allow(dead_code)]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct YouPlate {
    /// Draft specs awaiting your approve/reject verdict — the needs-approval
    /// group MINUS the advisor-backlog sub-class (which the advisor routes,
    /// per [`item_owner`]).
    pub needs_approval: Vec<String>,
    /// Specs with a pending decision only you can answer (needs-answer).
    pub needs_answer: Vec<String>,
    /// Count of unread messages in your mailbox. The mail SOURCE
    /// (`unread_inbox` -> rows) lands with STORY-701's mailbox group; until
    /// then a caller passes its unread count (0 folds cleanly to "no mail").
    pub unread_mail: usize,
}

#[allow(dead_code)] // methods consumed by the EPIC-54 owner-lens shell.
impl YouPlate {
    /// Total number of things on your plate: approvals + answers + unread mail.
    // trace:STORY-702 | ai:claude
    pub fn total(&self) -> usize {
        self.needs_approval.len() + self.needs_answer.len() + self.unread_mail
    }

    /// True when nothing is awaiting you — the plate is clear.
    // trace:STORY-702 | ai:claude
    pub fn is_clear(&self) -> bool {
        self.total() == 0
    }
}

/// Aggregate the classified board into the operator's [`YouPlate`] (STORY-702):
/// the single "what must I personally clear" view — needs-approval (your
/// drafts) + needs-answer + unread mail. Items the advisor owns (advisor-backlog
/// rows, resolved by [`item_owner`]) are excluded — the plate is what YOU owe,
/// not what the advisor owes. Pure over the items plus the mail count.
// trace:STORY-702 | ai:claude
// Consumer is EPIC-54's owner-lens shell; the aggregation lands ahead of it.
#[allow(dead_code)]
pub fn you_plate(items: &[ClassifiedItem], unread_mail: usize) -> YouPlate {
    let mut plate = YouPlate {
        unread_mail,
        ..YouPlate::default()
    };
    for it in items.iter().filter(|it| item_owner(it) == Owner::You) {
        match it.reason {
            Reason::NeedsApproval => plate.needs_approval.push(it.spec_id.clone()),
            Reason::NeedsAnswer => plate.needs_answer.push(it.spec_id.clone()),
            // owner_class maps only NeedsApproval / NeedsAnswer to You, so no
            // other reason reaches here; keep the arm exhaustive.
            _ => {}
        }
    }
    plate
}

/// True for the advisor-backlog sub-class of needs-approval (TASK-901): an
/// `Approved` spec that is not yet queued and not otherwise in motion
/// (in-flight / blocked). These are the specs the advisor has blessed but not
/// routed — the cache-fast stand-in for the headless `aida intake` proposal
/// set (which is a heavyweight cold-boot `claude -p` advisor pass, far too
/// slow for the board's paint budget; live-proposal surfacing is filed as a
/// follow-up). Status compare is case-insensitive to tolerate `Approved` vs
/// `approved` across the cache projection. trace:TASK-901 | ai:claude
fn is_advisor_backlog(r: &crate::dashboard::ListJsonRow) -> bool {
    r.status.eq_ignore_ascii_case("approved") && !r.queued && !r.in_flight && !r.blocked
}

/// Count the items per reason in a classified set — drives the Nav-label
/// `(count)` and the empty-reason hide. trace:STORY-686 | ai:claude
pub fn counts(items: &[ClassifiedItem]) -> std::collections::HashMap<&'static str, usize> {
    let mut m = std::collections::HashMap::new();
    for it in items {
        *m.entry(it.reason.label()).or_insert(0) += 1;
    }
    m
}

/// Project the classified items for one reason into [`ListRow`]s the
/// dashboard list pane renders. Each row carries the reason's [`RowKind`]
/// so the launcher dispatches the right Enter action.
///
/// The needs-approval group is split by sub-class (TASK-901): a draft carries
/// [`RowKind::ReasonNeedsApproval`] (Enter approves it), while an advisor-
/// backlog row (Approved-but-not-queued) carries
/// [`RowKind::ReasonAdvisorBacklog`] (Enter routes it to the queue) and its
/// status is prefixed `backlog · ` so the operator sees the advisor's pending
/// queue distinctly from the drafts awaiting a verdict.
/// trace:TASK-901 | ai:claude
/// An intake-proposal row (TASK-904) carries [`RowKind::ReasonIntakeProposal`]
/// (Enter fires `aida intake` scoped to that spec) and an `intake · ` status
/// prefix so the operator sees the live advisor's proposal fence distinctly
/// from both the drafts awaiting a verdict and the cache-fast advisor backlog.
// trace:TASK-904 | ai:claude
pub fn rows_for(items: &[ClassifiedItem], reason: Reason) -> Vec<ListRow> {
    items
        .iter()
        .filter(|it| it.reason == reason)
        .map(|it| ListRow {
            id: it.spec_id.clone(),
            title: it.title.clone(),
            // STORY-703: fold the park reason into the status column so a parked
            // item explains WHY inline ("backlog · Approved — blessed by the
            // advisor…", "Approved — returns when: <trigger>"). The prefix
            // (intake / backlog) is kept; the reason is appended with an em-dash.
            status: {
                let base = if it.intake_proposal {
                    format!("intake · {}", it.status)
                } else if it.advisor_backlog {
                    format!("backlog · {}", it.status)
                } else {
                    it.status.clone()
                };
                match &it.park_reason {
                    Some(reason) => format!("{base} — {reason}"),
                    None => base,
                }
            },
            kind: if it.intake_proposal {
                RowKind::ReasonIntakeProposal
            } else if it.advisor_backlog {
                RowKind::ReasonAdvisorBacklog
            } else {
                reason.row_kind()
            },
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Fast-source fetchers (shell-outs). Each composes a single cache-fast
// `aida` read; none ever calls `aida status`. trace:STORY-686 | ai:claude
// ---------------------------------------------------------------------------

/// Run a cache-fast `aida list …` variant and parse the JSON rows. Returns
/// an empty Vec on any failure so the board paints rather than crashing.
fn list_json(args: &[&str]) -> Vec<crate::dashboard::ListJsonRow> {
    let exe = crate::app::aida_exe();
    let mut cmd = Command::new(&exe);
    cmd.arg("list");
    cmd.args(args);
    cmd.arg("--json");
    if let Ok(cwd) = std::env::current_dir() {
        cmd.current_dir(cwd);
    }
    match cmd.output() {
        Ok(o) if o.status.success() => parse_list_json(&o.stdout),
        _ => Vec::new(),
    }
}

/// Gather the cheap (non-network) board inputs by composing the cache-fast
/// `aida list` reads plus the `aida questions list` decision-inbox parse.
/// The `gh pr list` network source is intentionally NOT touched here — it
/// lazy-fills the awaiting-review group separately.
///
/// The six legs are each a separate `aida` process; firing them
/// **concurrently** (one thread per leg) makes the home-view paint latency
/// the *max* of the legs (the `--blocked` graph walk dominates) rather than
/// their sum, keeping it well under the `aida status` scan the board exists
/// to avoid (BUG-616). trace:STORY-686 | ai:claude
pub fn fetch_inputs() -> BoardInputs {
    // Each leg owns its args so it can move into a thread.
    let all = std::thread::spawn(|| list_json(&["--blocked"]));
    let draft = std::thread::spawn(|| list_json(&["--status", "draft"]));
    let needs_attention = std::thread::spawn(|| list_json(&["--status", "needsattention"]));
    let done = std::thread::spawn(|| list_json(&["--status", "done"]));
    let deferred = std::thread::spawn(|| list_json(&["--deferred"]));
    let questions = std::thread::spawn(fetch_pending_question_ids);

    // A panicked leg (join Err) degrades to an empty set — the board still
    // paints its other reasons.
    BoardInputs {
        all_rows: all.join().unwrap_or_default(),
        draft_rows: draft.join().unwrap_or_default(),
        needs_attention_rows: needs_attention.join().unwrap_or_default(),
        done_rows: done.join().unwrap_or_default(),
        deferred_rows: deferred.join().unwrap_or_default(),
        pending_question_ids: questions.join().unwrap_or_default(),
    }
}

/// Parse the spec ids of pending decision requests from
/// `aida questions list`. There is no `--json` for this surface, so we
/// strip ANSI and pull the SPEC-ID out of each `Decision needed: <ID> …`
/// line (rendered by `print_decision_request`). Robust to colour codes and
/// to the empty / answered-only inboxes. trace:STORY-686 | ai:claude
pub fn fetch_pending_question_ids() -> Vec<String> {
    let exe = crate::app::aida_exe();
    let mut cmd = Command::new(&exe);
    cmd.args(["questions", "list"]);
    if let Ok(cwd) = std::env::current_dir() {
        cmd.current_dir(cwd);
    }
    let Ok(out) = cmd.output() else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    parse_pending_question_ids(&String::from_utf8_lossy(&out.stdout))
}

/// Pure parse of `aida questions list` stdout → pending spec ids. Splits on
/// the `Decision needed:` marker the pending block prints, then extracts
/// the first SPEC-ID-shaped token after it. Lines outside the Pending block
/// (the `Answered (N)` section uses a different `<ID> title → label` shape
/// with no `Decision needed:` marker) are ignored.
pub fn parse_pending_question_ids(stdout: &str) -> Vec<String> {
    let mut ids = Vec::new();
    for line in stdout.lines() {
        let clean = strip_ansi(line);
        if let Some(rest) = clean.split("Decision needed:").nth(1) {
            if let Some(id) = first_spec_id(rest) {
                ids.push(id);
            }
        }
    }
    ids
}

/// Strip ANSI SGR escape sequences (`\x1b[…m`) so plain-text parsing isn't
/// foiled by the colourised CLI output.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // Skip until the terminating letter of the CSI sequence.
            if chars.peek() == Some(&'[') {
                chars.next();
                for d in chars.by_ref() {
                    if d.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Extract the first `LETTERS-DIGITS[-DIGITS…]` SPEC-ID-shaped token from a
/// fragment. Returns the canonical id (e.g. `BUG-543`, `FR-1-042`).
fn first_spec_id(fragment: &str) -> Option<String> {
    for tok in fragment.split_whitespace() {
        let t = tok.trim();
        if is_spec_id(t) {
            return Some(t.to_string());
        }
    }
    None
}

/// True for a token shaped like a SPEC-ID: an uppercase-letter prefix, a
/// dash, then one or more dash-separated digit groups.
fn is_spec_id(tok: &str) -> bool {
    let Some((prefix, rest)) = tok.split_once('-') else {
        return false;
    };
    if prefix.is_empty() || !prefix.chars().all(|c| c.is_ascii_uppercase()) {
        return false;
    }
    !rest.is_empty()
        && rest
            .split('-')
            .all(|g| !g.is_empty() && g.chars().all(|c| c.is_ascii_digit()))
}

/// Lazy-fill the awaiting-review group with the open PRs from `gh pr list`
/// (the one ~1s network source). Returns the PR rows so the caller can
/// merge them on top of the Done-on-branch rows already painted. Best
/// effort: any `gh` failure (offline / unauth) yields an empty Vec and the
/// board just shows the Done rows. trace:STORY-686 | ai:claude
pub fn fetch_open_pr_rows() -> Vec<ListRow> {
    let Some(out) = run_gh_pr_list(Duration::from_secs(5)) else {
        return Vec::new();
    };
    crate::dashboard::parse_pr_json_rows(&out)
}

/// Shell `gh pr list --state open --json …` with a wall-clock timeout so an
/// offline / wedged `gh` never stalls the board. Returns the raw stdout
/// bytes on success.
fn run_gh_pr_list(timeout: Duration) -> Option<Vec<u8>> {
    use std::io::Read;
    use std::process::Stdio;
    let mut child = Command::new("gh")
        .args([
            "pr",
            "list",
            "--state",
            "open",
            "--json",
            "number,title,headRefName,statusCheckRollup",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()
        .ok()?;
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                let mut buf = Vec::new();
                if let Some(mut s) = child.stdout.take() {
                    let _ = s.read_to_end(&mut buf);
                }
                return Some(buf);
            }
            Ok(Some(_)) => return None,
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(_) => return None,
        }
    }
}

// ---------------------------------------------------------------------------
// Live intake-proposal source (TASK-904). The heavyweight `aida intake` pass
// spawns a cold-boot `claude -p` advisor (seconds-to-minutes) — far over the
// board's paint budget. We surface its candidate FENCE (the proposal set the
// advisor weighs) cheaply: `aida intake --dry-run` runs the same selection
// without launching `claude`, printing the eligible candidate ids. That
// dry-run is itself a store-load (~1s), so — exactly like the `gh pr list`
// awaiting-review source — it runs off the UI thread and on demand, never on
// paint. trace:TASK-904 | ai:claude
// ---------------------------------------------------------------------------

/// Shell `aida intake --dry-run` (the deprecated alias of `aida assess`;
/// `--dry-run` does the candidate selection WITHOUT the `claude -p` launch),
/// parse the eligible candidate spec ids, and return them. Best effort: any
/// failure (no store, binary error) yields an empty Vec so the board paints.
/// This is a heavier read than the cache-fast `aida list` legs — only ever
/// called off the UI thread.
// trace:TASK-904 | ai:claude
pub fn fetch_intake_proposal_ids() -> Vec<String> {
    let exe = crate::app::aida_exe();
    let mut cmd = Command::new(&exe);
    cmd.args(["intake", "--dry-run"]);
    if let Ok(cwd) = std::env::current_dir() {
        cmd.current_dir(cwd);
    }
    let Ok(out) = cmd.output() else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    parse_intake_proposal_ids(&String::from_utf8_lossy(&out.stdout))
}

/// Pure parse of `aida intake --dry-run` stdout → the eligible candidate spec
/// ids. The handler prints a single line shaped
/// `  → N spec(s) the advisor will weigh: ID, ID, …` (the fenced-out specs
/// follow on their own `✕`-prefixed lines, which we ignore). Robust to ANSI
/// colour codes. Returns an empty Vec for the "0 specs in the intake fence"
/// case.
// trace:TASK-904 | ai:claude
pub fn parse_intake_proposal_ids(stdout: &str) -> Vec<String> {
    for line in stdout.lines() {
        let clean = strip_ansi(line);
        if let Some(rest) = clean.split("the advisor will weigh:").nth(1) {
            return rest
                .split(',')
                .filter_map(|tok| {
                    let t = tok.trim();
                    if is_spec_id(t) {
                        Some(t.to_string())
                    } else {
                        None
                    }
                })
                .collect();
        }
    }
    Vec::new()
}

/// Merge the async-loaded intake-proposal candidate ids into the needs-approval
/// board group. Each candidate becomes a [`ClassifiedItem`] flagged
/// `intake_proposal`, with its title pulled from the already-classified board
/// (the candidates are a subset of the open specs the cache-fast pass loaded)
/// or left blank when not present. A candidate already surfaced as a draft or
/// advisor-backlog row is upgraded in place (its `intake_proposal` flag set)
/// rather than duplicated, so the operator sees one row per spec carrying the
/// strongest available label.
// trace:TASK-904 | ai:claude
pub fn merge_intake_proposals(items: &mut Vec<ClassifiedItem>, candidate_ids: &[String]) {
    use std::collections::HashSet;
    let candidates: HashSet<&str> = candidate_ids.iter().map(|s| s.as_str()).collect();
    // Upgrade existing rows in place, and note which candidates were absent.
    let mut present: HashSet<String> = HashSet::new();
    for it in items.iter_mut() {
        if candidates.contains(it.spec_id.as_str()) {
            it.intake_proposal = true;
            present.insert(it.spec_id.clone());
        }
    }
    // Append candidates the cache-fast pass didn't already surface (a draft the
    // proxy missed, say). Title is unknown here, so we leave it blank; Enter
    // still dispatches on the spec id. trace:TASK-904
    for id in candidate_ids {
        if !present.contains(id) {
            items.push(ClassifiedItem {
                spec_id: id.clone(),
                title: String::new(),
                status: "candidate".to_string(),
                reason: Reason::NeedsApproval,
                advisor_backlog: false,
                intake_proposal: true,
                // An intake proposal is a draft awaiting the advisor's verdict.
                // trace:STORY-703
                park_reason: park_reason(Reason::NeedsApproval, false, None, None, None),
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Mailbox group (STORY-701). The mail SOURCE (`unread_inbox` -> rows) and the
// SEND action are pure, registerable, unit-testable content — the EPIC-53
// half of the seam (docs/plans/2026-06-26-epic-53-cockpit-seam.md). The read
// side is wired into the legacy dashboard's Nav (mirroring `board`'s
// lazy-load cadence, see `DashboardModel::refresh_mail`); the compose/reply
// GESTURE (picking a message, typing a body, dispatching the send) is
// EPIC-54's — `send_mail_argv` is the action_fn a future shell registers.
// ---------------------------------------------------------------------------

/// Project unread messages into cockpit rows. Pure: no I/O, so it's testable
/// straight from `Message` fixtures. `id` is the message id (NOT a spec id —
/// a mail row's Enter dispatch and preview both special-case
/// `RowKind::ReasonMail` because of this).
// trace:STORY-701 | ai:claude
pub fn mail_rows(unread: &[&aida_core::mailbox::Message]) -> Vec<ListRow> {
    unread
        .iter()
        .map(|m| ListRow {
            id: m.id.clone(),
            title: mail_subject(&m.body, 60),
            status: mail_status(m),
            kind: RowKind::ReasonMail,
        })
        .collect()
}

/// First non-empty line of a message body, trimmed and truncated to `max`
/// chars (with an ellipsis when cut) — the row's display title. Mirrors
/// `aida_core::mailbox`'s private notice-subject projection (kept
/// independent since that one isn't `pub`).
// trace:STORY-701 | ai:claude
fn mail_subject(body: &str, max: usize) -> String {
    let first = body
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    if first.is_empty() {
        return "(empty message)".to_string();
    }
    let mut chars = first.chars();
    let head: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

/// The row's status column: sender, plus an urgent/actionable-intent flag —
/// the same signal `aida mailbox inbox` surfaces on its message lines.
// trace:STORY-701 | ai:claude
fn mail_status(m: &aida_core::mailbox::Message) -> String {
    let mut flags = Vec::new();
    if m.urgent {
        flags.push("urgent");
    }
    if m.intent.is_actionable() {
        flags.push(m.intent.as_str());
    }
    if flags.is_empty() {
        format!("from {}", m.from)
    } else {
        format!("from {} · {}", m.from, flags.join(", "))
    }
}

/// This shell's mail identity: the same precedence the CLI's queue/mailbox
/// user resolution uses (BUG-89) — `AIDA_USER`, then `USER`, then `USERNAME`
/// (Windows), then `"default"`.
// trace:STORY-701 | ai:claude
fn mail_identity() -> String {
    std::env::var("AIDA_USER")
        .or_else(|_| std::env::var("USER"))
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "default".to_string())
}

/// Resolve the AIDA project root from `cwd`: the nearest ancestor holding
/// `.git` or `.aida/config.toml`. Mirrors `aida_tui::ensure_project_context`
/// (kept independent: that one is `run()`'s startup gate and errors when no
/// project is found; this is a best-effort fetch-time lookup that degrades to
/// the raw cwd instead, matching the fetch's own "any failure yields an empty
/// Vec" grace).
// trace:STORY-701 | ai:claude
fn resolve_project_root(cwd: &Path) -> PathBuf {
    let mut dir = Some(cwd);
    while let Some(d) = dir {
        if d.join(".git").exists() || d.join(".aida").join("config.toml").is_file() {
            return d.to_path_buf();
        }
        dir = d.parent();
    }
    cwd.to_path_buf()
}

/// Read this operator's unread mail from the LOCAL mailbox layer (the fast,
/// live-exchange layer — matches the board's own cache-fast philosophy: never
/// a slow full-store scan on the paint path) and project it into cockpit
/// rows. Read-only: never advances the watermark, so painting the cockpit
/// never marks mail seen (mirrors `aida mailbox inbox --peek`). Best effort:
/// any read failure yields an empty Vec so the board paints.
// trace:STORY-701 | ai:claude
pub fn fetch_mail_rows() -> Vec<ListRow> {
    let Ok(cwd) = std::env::current_dir() else {
        return Vec::new();
    };
    let project_root = resolve_project_root(&cwd);
    let messages = aida_core::mailbox::read_local_messages(&project_root).unwrap_or_default();
    let agent = mail_identity();
    let watermark = aida_core::mailbox::read_local_watermark(&project_root, &agent);
    let unread = aida_core::mailbox::unread_inbox(&agent, &messages, watermark);
    mail_rows(&unread)
}

/// The mailbox "send" ACTION (STORY-701): the row action_fn a future
/// compose/reply gesture registers. Wraps `aida mailbox send`, returning the
/// exact argv (NOT a shell string) so a caller spawns it directly via
/// `Command::new(exe).args(...)` without threading an arbitrary message body
/// through the launcher's restricted `Intent::Launch` payload gate
/// (`intent::is_safe_payload` deliberately excludes quotes/punctuation a real
/// message body needs — see `launcher::act_on_row`'s `RowKind::ReasonMail`
/// arm). Pure: no I/O, fully unit-testable.
// trace:STORY-701 | ai:claude
#[allow(dead_code)] // consumed by the EPIC-54 shell's compose/reply gesture.
pub fn send_mail_argv(
    to: &str,
    body: &str,
    in_reply_to: Option<&str>,
    urgent: bool,
) -> Vec<String> {
    let mut argv = vec![
        "mailbox".to_string(),
        "send".to_string(),
        "--to".to_string(),
        to.to_string(),
    ];
    if let Some(id) = in_reply_to {
        argv.push("--in-reply-to".to_string());
        argv.push(id.to_string());
    }
    if urgent {
        argv.push("--urgent".to_string());
    }
    argv.push(body.to_string());
    argv
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dashboard::ListJsonRow;

    fn row(id: &str, status: &str, queued: bool, in_flight: bool, blocked: bool) -> ListJsonRow {
        ListJsonRow {
            spec_id: id.to_string(),
            title: format!("title of {id}"),
            req_type: "story".to_string(),
            status: status.to_string(),
            tags: vec![],
            queued,
            in_flight,
            blocked,
            deferred_until: None,
        }
    }

    /// A deferred row carrying its revisit trigger — for the STORY-703 park-reason
    /// projection tests.
    fn deferred_row(id: &str, trigger: &str) -> ListJsonRow {
        ListJsonRow {
            deferred_until: Some(trigger.to_string()),
            ..row(id, "Approved", false, false, false)
        }
    }

    #[test]
    fn flags_map_to_the_right_group() {
        let inputs = BoardInputs {
            all_rows: vec![
                row("STORY-1", "InProgress", false, true, false),
                row("STORY-2", "Approved", false, false, true),
            ],
            draft_rows: vec![row("STORY-3", "Draft", false, false, false)],
            needs_attention_rows: vec![row("STORY-4", "NeedsAttention", false, false, false)],
            done_rows: vec![row("STORY-5", "Done", false, false, false)],
            deferred_rows: vec![row("STORY-6", "Approved", false, false, false)],
            pending_question_ids: vec!["STORY-7".to_string()],
        };
        let items = classify(&inputs);
        let by_id = |id: &str| items.iter().find(|i| i.spec_id == id).map(|i| i.reason);
        assert_eq!(by_id("STORY-1"), Some(Reason::InFlight));
        assert_eq!(by_id("STORY-2"), Some(Reason::Blocked));
        assert_eq!(by_id("STORY-3"), Some(Reason::NeedsApproval));
        assert_eq!(by_id("STORY-4"), Some(Reason::NeedsAttention));
        assert_eq!(by_id("STORY-5"), Some(Reason::AwaitingReview));
        assert_eq!(by_id("STORY-6"), Some(Reason::Deferred));
        assert_eq!(by_id("STORY-7"), Some(Reason::NeedsAnswer));
    }

    #[test]
    fn approved_not_queued_surfaces_as_advisor_backlog() {
        // TASK-901: an Approved-but-not-queued spec rides the needs-approval
        // group flagged as advisor backlog, derived from the all-rows set with
        // no extra shell-out. A drafted spec stays a plain needs-approval row.
        let inputs = BoardInputs {
            all_rows: vec![
                row("STORY-10", "Approved", false, false, false), // backlog
                row("STORY-11", "Approved", true, false, false),  // queued → out
                row("STORY-12", "Planned", false, false, false),  // not Approved → out
            ],
            draft_rows: vec![row("STORY-13", "Draft", false, false, false)],
            ..BoardInputs::default()
        };
        let items = classify(&inputs);
        let find = |id: &str| items.iter().find(|i| i.spec_id == id);

        // STORY-10 is the advisor backlog: needs-approval reason, backlog flag.
        let backlog = find("STORY-10").expect("approved-not-queued surfaces");
        assert_eq!(backlog.reason, Reason::NeedsApproval);
        assert!(backlog.advisor_backlog);

        // The draft is needs-approval but NOT backlog (approve-or-reject).
        let draft = find("STORY-13").expect("draft surfaces");
        assert_eq!(draft.reason, Reason::NeedsApproval);
        assert!(!draft.advisor_backlog);

        // Queued / non-Approved rows are not advisor backlog.
        assert!(
            find("STORY-11").is_none(),
            "queued is in-motion, not backlog"
        );
        assert!(find("STORY-12").is_none(), "Planned is not advisor backlog");

        // Both drafts and backlog count toward the needs-approval group.
        assert_eq!(counts(&items).get(Reason::NeedsApproval.label()), Some(&2));
    }

    #[test]
    fn advisor_backlog_row_dispatches_to_queue_not_approve() {
        // The backlog row carries a distinct RowKind (so Enter routes it to the
        // queue, since it is already Approved) and a `backlog · ` status prefix.
        let inputs = BoardInputs {
            all_rows: vec![row("STORY-20", "Approved", false, false, false)],
            draft_rows: vec![row("STORY-21", "Draft", false, false, false)],
            ..BoardInputs::default()
        };
        let items = classify(&inputs);
        let rows = rows_for(&items, Reason::NeedsApproval);
        let backlog = rows.iter().find(|r| r.id == "STORY-20").unwrap();
        let draft = rows.iter().find(|r| r.id == "STORY-21").unwrap();
        assert_eq!(backlog.kind, RowKind::ReasonAdvisorBacklog);
        assert!(backlog.status.starts_with("backlog · "));
        // STORY-703: the advisor-backlog row now also carries its park reason
        // inline (blessed, awaiting routing).
        assert!(backlog.status.contains("awaiting routing"));
        assert_eq!(draft.kind, RowKind::ReasonNeedsApproval);
        // STORY-703: the draft row keeps its `Draft` status prefix but now also
        // explains why it's parked (awaiting a verdict).
        assert!(draft.status.starts_with("Draft"));
        assert!(draft.status.contains("approve/reject verdict"));
    }

    #[test]
    fn in_flight_approved_is_not_advisor_backlog() {
        // An Approved spec that a higher-precedence pass already claimed
        // (in-flight / blocked) must NOT also appear as advisor backlog.
        let inputs = BoardInputs {
            all_rows: vec![
                row("STORY-30", "Approved", false, true, false), // in flight
                row("STORY-31", "Approved", false, false, true), // blocked
            ],
            ..BoardInputs::default()
        };
        let items = classify(&inputs);
        assert_eq!(items.len(), 2, "no double-count into backlog");
        assert_eq!(
            items
                .iter()
                .find(|i| i.spec_id == "STORY-30")
                .unwrap()
                .reason,
            Reason::InFlight
        );
        assert_eq!(
            items
                .iter()
                .find(|i| i.spec_id == "STORY-31")
                .unwrap()
                .reason,
            Reason::Blocked
        );
        assert!(items.iter().all(|i| !i.advisor_backlog));
    }

    #[test]
    fn precedence_in_flight_beats_blocked() {
        // A spec that is both in_flight AND blocked lands in in_flight only.
        let inputs = BoardInputs {
            all_rows: vec![row("STORY-1", "InProgress", false, true, true)],
            ..BoardInputs::default()
        };
        let items = classify(&inputs);
        assert_eq!(items.len(), 1, "no double-count");
        assert_eq!(items[0].reason, Reason::InFlight);
    }

    #[test]
    fn precedence_needs_attention_beats_question_and_draft() {
        // Same spec id appears in NeedsAttention, pending-questions, and the
        // (hypothetical) draft set — NeedsAttention (higher) wins, once.
        let inputs = BoardInputs {
            needs_attention_rows: vec![row("BUG-9", "NeedsAttention", false, false, false)],
            pending_question_ids: vec!["BUG-9".to_string()],
            draft_rows: vec![row("BUG-9", "Draft", false, false, false)],
            ..BoardInputs::default()
        };
        let items = classify(&inputs);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].reason, Reason::NeedsAttention);
    }

    #[test]
    fn counts_match_grouping() {
        let inputs = BoardInputs {
            draft_rows: vec![
                row("A-1", "Draft", false, false, false),
                row("A-2", "Draft", false, false, false),
            ],
            deferred_rows: vec![row("D-1", "Approved", false, false, false)],
            ..BoardInputs::default()
        };
        let items = classify(&inputs);
        let c = counts(&items);
        assert_eq!(c.get(Reason::NeedsApproval.label()), Some(&2));
        assert_eq!(c.get(Reason::Deferred.label()), Some(&1));
        assert_eq!(c.get(Reason::Blocked.label()), None);
    }

    #[test]
    fn empty_reason_yields_no_rows() {
        let items = classify(&BoardInputs::default());
        assert!(items.is_empty());
        for reason in Reason::all() {
            assert!(rows_for(&items, reason).is_empty());
        }
    }

    #[test]
    fn rows_for_carries_reason_row_kind() {
        let inputs = BoardInputs {
            draft_rows: vec![row("STORY-3", "Draft", false, false, false)],
            ..BoardInputs::default()
        };
        let items = classify(&inputs);
        let rows = rows_for(&items, Reason::NeedsApproval);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "STORY-3");
        assert_eq!(rows[0].kind, RowKind::ReasonNeedsApproval);
    }

    #[test]
    fn parse_pending_questions_extracts_spec_ids() {
        // Mirrors the `print_decision_request` pending-block shape, with a
        // colour-coded `Decision needed:` marker; the Answered block (a
        // different shape, no marker) must be ignored.
        let stdout = "\
Pending decisions (2)

\u{1b}[1;33mDecision needed:\u{1b}[0m \u{1b}[36mBUG-543\u{1b}[0m  Epics don't auto-complete
  Some question text
    1. choice — consequence

Decision needed: FR-1-042  Another fork
  q2

Answered (1)
  TASK-798       Config coherence → Unify under [contained]
";
        let ids = parse_pending_question_ids(stdout);
        assert_eq!(ids, vec!["BUG-543".to_string(), "FR-1-042".to_string()]);
    }

    #[test]
    fn parse_pending_questions_empty_inbox() {
        assert!(
            parse_pending_question_ids("Decision inbox empty — no questions recorded.").is_empty()
        );
        assert!(parse_pending_question_ids("").is_empty());
        // Answered-only inbox: no `Decision needed:` markers.
        let answered_only = "Answered (1)\n  TASK-798  foo → bar\n";
        assert!(parse_pending_question_ids(answered_only).is_empty());
    }

    #[test]
    fn is_spec_id_classifies() {
        assert!(is_spec_id("BUG-543"));
        assert!(is_spec_id("FR-1-042"));
        assert!(is_spec_id("STORY-686"));
        assert!(!is_spec_id("lowercase-1"));
        assert!(!is_spec_id("NODASH"));
        assert!(!is_spec_id("BUG-"));
        assert!(!is_spec_id("BUG-x"));
        assert!(!is_spec_id("title"));
    }

    #[test]
    fn strip_ansi_removes_sgr() {
        assert_eq!(strip_ansi("\u{1b}[1;33mhi\u{1b}[0m"), "hi");
        assert_eq!(strip_ansi("plain"), "plain");
    }

    #[test]
    fn all_reasons_have_distinct_row_kinds() {
        let kinds: Vec<RowKind> = Reason::all().iter().map(|r| r.row_kind()).collect();
        for (i, a) in kinds.iter().enumerate() {
            for b in kinds.iter().skip(i + 1) {
                assert_ne!(a, b, "row kinds must be distinct per reason");
            }
        }
    }

    // --- Live intake-proposal source (TASK-904). ---

    #[test]
    fn parse_intake_proposals_extracts_candidate_ids() {
        // Mirrors the `aida intake --dry-run` weigh-line shape, with ANSI
        // colour codes, plus the fenced-out `✕` lines that must be ignored.
        let stdout = "\
\u{1b}[36m▸\u{1b}[0m intake pass
  bias=approve-eligible · risk≤medium
  \u{1b}[32m→\u{1b}[0m 2 spec(s) the advisor will weigh: \u{1b}[36mTASK-903\u{1b}[0m, \u{1b}[36mTASK-904\u{1b}[0m
  · 1 fenced out (do-not-approve class …):
    ✕ EPIC-42 — do-not-approve class `epic` (advisor-authored)
";
        let ids = parse_intake_proposal_ids(stdout);
        assert_eq!(ids, vec!["TASK-903".to_string(), "TASK-904".to_string()]);
    }

    #[test]
    fn parse_intake_proposals_empty_fence() {
        // The "0 specs in the intake fence" branch never prints a weigh-line.
        let stdout = "\
▸ intake pass
  → 0 specs in the intake fence (50 fenced out). Nothing for the advisor to weigh.
";
        assert!(parse_intake_proposal_ids(stdout).is_empty());
        assert!(parse_intake_proposal_ids("").is_empty());
    }

    #[test]
    fn merge_intake_upgrades_existing_and_appends_new() {
        // A candidate already classified (a draft) is upgraded in place — its
        // `intake_proposal` flag is set, no duplicate row. A candidate the
        // cache-fast pass never surfaced is appended as a fresh proposal row.
        // trace:TASK-904
        let inputs = BoardInputs {
            draft_rows: vec![row("STORY-1", "Draft", false, false, false)],
            ..BoardInputs::default()
        };
        let mut items = classify(&inputs);
        merge_intake_proposals(&mut items, &["STORY-1".to_string(), "STORY-99".to_string()]);

        let upgraded = items.iter().find(|i| i.spec_id == "STORY-1").unwrap();
        assert!(upgraded.intake_proposal, "existing draft upgraded in place");
        assert_eq!(
            items.iter().filter(|i| i.spec_id == "STORY-1").count(),
            1,
            "no duplicate row for an upgraded candidate"
        );

        let appended = items.iter().find(|i| i.spec_id == "STORY-99").unwrap();
        assert!(appended.intake_proposal);
        assert_eq!(appended.reason, Reason::NeedsApproval);
    }

    #[test]
    fn intake_proposal_row_has_distinct_kind_and_prefix() {
        // An intake-proposal row carries ReasonIntakeProposal (Enter shows the
        // candidate) and an `intake · ` status prefix — distinct from both the
        // plain draft and the `backlog · ` advisor-backlog row. trace:TASK-904
        let inputs = BoardInputs {
            draft_rows: vec![row("STORY-1", "Draft", false, false, false)],
            ..BoardInputs::default()
        };
        let mut items = classify(&inputs);
        merge_intake_proposals(&mut items, &["STORY-1".to_string()]);
        let rows = rows_for(&items, Reason::NeedsApproval);
        let proposal = rows.iter().find(|r| r.id == "STORY-1").unwrap();
        assert_eq!(proposal.kind, RowKind::ReasonIntakeProposal);
        assert!(proposal.status.starts_with("intake · "));
    }

    #[test]
    fn merge_intake_empty_is_noop() {
        let inputs = BoardInputs {
            draft_rows: vec![row("STORY-1", "Draft", false, false, false)],
            ..BoardInputs::default()
        };
        let mut items = classify(&inputs);
        let before = items.len();
        merge_intake_proposals(&mut items, &[]);
        assert_eq!(items.len(), before);
        assert!(items.iter().all(|i| !i.intake_proposal));
    }

    // --- Advisor park-reason projection (STORY-703). ---

    #[test]
    fn park_reason_maps_each_park_state() {
        // A fixture of park states → its inline reason string. Deferred surfaces
        // its trigger; needs-attention its punt note; the advisor-backlog vs
        // draft split of needs-approval; needs-answer; blocked. The two MOVING
        // states (in-flight / awaiting-review) are not parked → None.
        assert_eq!(
            park_reason(Reason::Deferred, false, Some("the shelf grows"), None, None).as_deref(),
            Some("returns when: the shelf grows")
        );
        // Deferred with no trigger falls back to a legible placeholder.
        assert_eq!(
            park_reason(Reason::Deferred, false, None, None, None).as_deref(),
            Some("deferred — no revisit trigger recorded")
        );
        // Blank/whitespace trigger is treated as absent.
        assert_eq!(
            park_reason(Reason::Deferred, false, Some("   "), None, None).as_deref(),
            Some("deferred — no revisit trigger recorded")
        );
        assert_eq!(
            park_reason(
                Reason::NeedsAttention,
                false,
                None,
                Some("needs a design call"),
                None
            )
            .as_deref(),
            Some("parked: needs a design call")
        );
        // A finding backfills the punt note when none was recorded.
        assert_eq!(
            park_reason(
                Reason::NeedsAttention,
                false,
                None,
                None,
                Some("flaky path")
            )
            .as_deref(),
            Some("parked: flaky path")
        );
        // Advisor-backlog vs draft split of needs-approval.
        assert_eq!(
            park_reason(Reason::NeedsApproval, true, None, None, None).as_deref(),
            Some("blessed by the advisor — awaiting routing to the implementer queue")
        );
        assert_eq!(
            park_reason(Reason::NeedsApproval, false, None, None, None).as_deref(),
            Some("awaiting an approve/reject verdict")
        );
        assert_eq!(
            park_reason(Reason::NeedsAnswer, false, None, None, None).as_deref(),
            Some("waiting on a human decision")
        );
        assert_eq!(
            park_reason(Reason::Blocked, false, None, None, None).as_deref(),
            Some("blocked by an incomplete dependency")
        );
        // Moving / handed-off states are not parked.
        assert_eq!(park_reason(Reason::InFlight, false, None, None, None), None);
        assert_eq!(
            park_reason(Reason::AwaitingReview, false, None, None, None),
            None
        );
    }

    #[test]
    fn classify_populates_park_reason_inline() {
        // End-to-end: a deferred spec carrying a revisit trigger, an
        // advisor-backlog spec, and a draft → each classified item carries its
        // park reason, and the trigger reaches the deferred row.
        let inputs = BoardInputs {
            all_rows: vec![row("STORY-1", "Approved", false, false, false)], // advisor backlog
            draft_rows: vec![row("STORY-2", "Draft", false, false, false)],
            deferred_rows: vec![deferred_row("STORY-3", "demand is proven")],
            ..BoardInputs::default()
        };
        let items = classify(&inputs);
        let find = |id: &str| items.iter().find(|i| i.spec_id == id).unwrap();

        assert_eq!(
            find("STORY-1").park_reason.as_deref(),
            Some("blessed by the advisor — awaiting routing to the implementer queue")
        );
        assert_eq!(
            find("STORY-2").park_reason.as_deref(),
            Some("awaiting an approve/reject verdict")
        );
        assert_eq!(
            find("STORY-3").park_reason.as_deref(),
            Some("returns when: demand is proven")
        );

        // The reason reaches the rendered row inline.
        let deferred_rows = rows_for(&items, Reason::Deferred);
        let r3 = deferred_rows.iter().find(|r| r.id == "STORY-3").unwrap();
        assert!(r3.status.contains("returns when: demand is proven"));
    }

    #[test]
    fn advisor_queue_depth_counts_drafts_and_backlog() {
        // The advisor-queue depth is the needs-approval group: drafts awaiting a
        // verdict PLUS advisor-backlog (approved-not-queued) specs. Deferred /
        // blocked / in-flight do not count.
        let inputs = BoardInputs {
            all_rows: vec![
                row("STORY-1", "Approved", false, false, false), // backlog → counts
                row("STORY-2", "Approved", true, false, false),  // queued → out
            ],
            draft_rows: vec![
                row("STORY-3", "Draft", false, false, false),
                row("STORY-4", "Draft", false, false, false),
            ],
            deferred_rows: vec![deferred_row("STORY-5", "later")],
            ..BoardInputs::default()
        };
        let items = classify(&inputs);
        // 1 backlog + 2 drafts = 3; the deferred + queued rows don't count.
        assert_eq!(advisor_queue_depth(&items), 3);
    }

    // --- Owner classification + you-plate aggregation (STORY-702). ---

    #[test]
    fn owner_class_covers_every_reason() {
        // The full owner set: each of the 7 reasons maps to exactly one typed
        // owner, and the display shim `Reason::owner()` still emits the exact
        // pre-STORY-702 strings so the nav rendering is unchanged.
        let expect = [
            (Reason::InFlight, Owner::Implementer, "impl"),
            (Reason::Blocked, Owner::Dependency, "wait"),
            (Reason::NeedsAttention, Owner::Implementer, "impl"),
            (Reason::AwaitingReview, Owner::Reviewer, "reviewer"),
            (Reason::NeedsAnswer, Owner::You, "you"),
            (Reason::NeedsApproval, Owner::You, "you"),
            (Reason::Deferred, Owner::Trigger, "trigger"),
        ];
        for (reason, owner, label) in expect {
            assert_eq!(reason.owner_class(), owner, "owner_class for {reason:?}");
            assert_eq!(reason.owner(), label, "owner() label for {reason:?}");
            assert_eq!(owner.label(), label, "Owner::label for {owner:?}");
        }
        // Every reason in the precedence list is covered (no panic, total map).
        for reason in Reason::all() {
            let _ = reason.owner_class();
        }
    }

    #[test]
    fn item_owner_refines_advisor_backlog_to_advisor() {
        // A plain draft is yours; the advisor-backlog sub-class of the SAME
        // needs-approval reason is the advisor's to route.
        let inputs = BoardInputs {
            all_rows: vec![row("STORY-1", "Approved", false, false, false)], // backlog
            draft_rows: vec![row("STORY-2", "Draft", false, false, false)],  // draft
            pending_question_ids: vec!["STORY-3".to_string()],               // needs-answer
            done_rows: vec![row("STORY-4", "Done", false, false, false)],    // reviewer
            ..BoardInputs::default()
        };
        let items = classify(&inputs);
        let find = |id: &str| items.iter().find(|i| i.spec_id == id).unwrap();
        assert_eq!(
            item_owner(find("STORY-1")),
            Owner::Advisor,
            "backlog → advisor"
        );
        assert_eq!(item_owner(find("STORY-2")), Owner::You, "draft → you");
        assert_eq!(
            item_owner(find("STORY-3")),
            Owner::You,
            "needs-answer → you"
        );
        assert_eq!(
            item_owner(find("STORY-4")),
            Owner::Reviewer,
            "done → reviewer"
        );
    }

    #[test]
    fn you_plate_groups_approval_answer_and_mail() {
        // The you-plate is the single "what must I clear" view: needs-approval
        // drafts + needs-answer + unread mail. The advisor-backlog row (STORY-1)
        // is the advisor's to route and is excluded.
        let inputs = BoardInputs {
            all_rows: vec![
                row("STORY-1", "Approved", false, false, false), // advisor backlog → excluded
                row("STORY-9", "InProgress", false, true, false), // in-flight → not yours
            ],
            draft_rows: vec![
                row("STORY-2", "Draft", false, false, false),
                row("STORY-3", "Draft", false, false, false),
            ],
            needs_attention_rows: vec![row("BUG-8", "NeedsAttention", false, false, false)],
            pending_question_ids: vec!["STORY-4".to_string()],
            deferred_rows: vec![deferred_row("STORY-5", "later")],
            ..BoardInputs::default()
        };
        let items = classify(&inputs);
        let plate = you_plate(&items, 2);

        assert_eq!(
            plate.needs_approval,
            vec!["STORY-2".to_string(), "STORY-3".to_string()],
            "drafts only; advisor-backlog excluded"
        );
        assert_eq!(plate.needs_answer, vec!["STORY-4".to_string()]);
        assert_eq!(plate.unread_mail, 2);
        // 2 approvals + 1 answer + 2 mail = 5.
        assert_eq!(plate.total(), 5);
        assert!(!plate.is_clear());

        // The advisor-backlog, in-flight, needs-attention, and deferred rows
        // are NOT on your plate.
        assert!(!plate.needs_approval.contains(&"STORY-1".to_string()));
    }

    #[test]
    fn you_plate_empty_and_mail_only() {
        // No items, no mail → a clear plate.
        let empty = you_plate(&[], 0);
        assert!(empty.is_clear());
        assert_eq!(empty.total(), 0);

        // Mail with no specs still lands you on the plate (nothing to approve or
        // answer, but you owe the inbox).
        let mail_only = you_plate(&[], 3);
        assert!(!mail_only.is_clear());
        assert_eq!(mail_only.total(), 3);
        assert!(mail_only.needs_approval.is_empty());
        assert!(mail_only.needs_answer.is_empty());
    }

    // --- Mailbox group (STORY-701). ---

    fn mail_msg(
        id: &str,
        from: &str,
        body: &str,
        urgent: bool,
        ts: i64,
    ) -> aida_core::mailbox::Message {
        aida_core::mailbox::Message {
            id: id.to_string(),
            thread_id: id.to_string(),
            from: from.to_string(),
            to: aida_core::mailbox::Recipient::Agent("you".to_string()),
            timestamp: ts,
            in_reply_to: None,
            body: body.to_string(),
            urgent,
            intent: aida_core::mailbox::Intent::Fyi,
            retracted: false,
            deleted: false,
        }
    }

    #[test]
    fn mail_rows_projects_unread_messages() {
        let m1 = mail_msg("m1", "codex", "PR ready for review\nsecond line", false, 10);
        let m2 = mail_msg("m2", "agy", "quick heads up", true, 20);
        let unread: Vec<&aida_core::mailbox::Message> = vec![&m1, &m2];
        let rows = mail_rows(&unread);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "m1");
        assert_eq!(rows[0].title, "PR ready for review");
        assert_eq!(rows[0].status, "from codex");
        assert_eq!(rows[0].kind, RowKind::ReasonMail);
        assert_eq!(rows[1].status, "from agy · urgent");
    }

    #[test]
    fn mail_rows_flags_actionable_intent() {
        let mut m = mail_msg("m1", "codex", "please review", false, 10);
        m.intent = aida_core::mailbox::Intent::Request;
        let unread = vec![&m];
        let rows = mail_rows(&unread);
        assert_eq!(rows[0].status, "from codex · request");
    }

    #[test]
    fn mail_rows_urgent_and_actionable_both_flag() {
        let mut m = mail_msg("m1", "codex", "drop everything", true, 10);
        m.intent = aida_core::mailbox::Intent::Handoff;
        let unread = vec![&m];
        let rows = mail_rows(&unread);
        assert_eq!(rows[0].status, "from codex · urgent, handoff");
    }

    #[test]
    fn mail_rows_truncates_long_subject_and_handles_empty_body() {
        let long_body = "x".repeat(80);
        let m1 = mail_msg("m1", "codex", &long_body, false, 10);
        let m2 = mail_msg("m2", "codex", "   \n  ", false, 20); // whitespace-only body
        let unread = vec![&m1, &m2];
        let rows = mail_rows(&unread);
        assert!(rows[0].title.ends_with('…'));
        assert_eq!(rows[0].title.chars().count(), 61); // 60 chars + ellipsis
        assert_eq!(rows[1].title, "(empty message)");
    }

    #[test]
    fn mail_rows_empty_input_is_empty() {
        assert!(mail_rows(&[]).is_empty());
    }

    #[test]
    fn reason_mail_taxonomy() {
        assert_eq!(Reason::Mail.label(), "mail");
        assert_eq!(Reason::Mail.owner_class(), Owner::You);
        assert_eq!(Reason::Mail.owner(), "you");
        assert_eq!(Reason::Mail.row_kind(), RowKind::ReasonMail);
        assert!(Reason::all().contains(&Reason::Mail));
    }

    #[test]
    fn send_mail_argv_builds_minimal_command() {
        let argv = send_mail_argv("codex", "hello there", None, false);
        assert_eq!(
            argv,
            vec!["mailbox", "send", "--to", "codex", "hello there"]
        );
    }

    #[test]
    fn send_mail_argv_includes_reply_and_urgent_flags() {
        let argv = send_mail_argv("codex", "on it", Some("m1"), true);
        assert_eq!(
            argv,
            vec![
                "mailbox",
                "send",
                "--to",
                "codex",
                "--in-reply-to",
                "m1",
                "--urgent",
                "on it",
            ]
        );
    }

    #[test]
    fn resolve_project_root_finds_nearest_git_ancestor() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let nested = root.join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(resolve_project_root(&nested), root);
    }

    #[test]
    fn resolve_project_root_finds_aida_config_when_no_git() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(root.join(".aida").join("config.toml"), "").unwrap();
        let nested = root.join("x");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(resolve_project_root(&nested), root);
    }
}
