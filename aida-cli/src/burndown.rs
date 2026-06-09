//! STORY-527 slice 1: the pure pickability gate for `aida burndown plan`.
//!
//! `/aida-burndown` fans out worktree-isolated implementer subagents over a
//! ready set, with the main session integrating (see
//! `docs/aida/discipline/autonomous-burndown.md`). The non-negotiable safety
//! property is that only **bounded, unblocked, decision-free** specs are fanned
//! out — anything needing a human decision is parked, never dragged in. That is
//! what makes "never stop to ask" safe.
//!
//! This module is the side-effect-free heart of that gate: given a candidate
//! spec's already-probed facts, decide READY vs PARKED(reason). The selector
//! resolution and the graph/blocker probing live in `main.rs`; keeping the
//! verdict pure makes it exhaustively unit-testable. trace:STORY-527 | ai:claude

/// The gate's verdict for one candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Pickability {
    /// Bounded + unblocked + decision-free — safe to fan out autonomously.
    Ready,
    /// Held back, with a human-readable reason.
    Parked(String),
}

/// Already-probed facts about one candidate spec. Built in `main.rs` from the
/// store + graph; consumed by the pure [`classify`]. trace:STORY-527
#[derive(Debug, Clone)]
pub(crate) struct BurndownCandidate {
    /// Display SPEC-ID (e.g. `TASK-702`).
    pub id: String,
    /// Lowercased requirement type (`epic`, `task`, `story`, …).
    pub req_type: String,
    /// The spec's tags (used for the parking-tag check).
    pub tags: Vec<String>,
    /// True when any `BlockedBy` edge points at a not-yet-Completed spec.
    pub has_unsatisfied_blocker: bool,
    /// True when the spec carries a pending `DecisionRequest` (an open
    /// human-decision question via `aida questions`).
    pub has_pending_decision: bool,
}

/// A tag that marks a spec as not-autonomously-pickable — a human decision, a
/// deferral, or a draft-review gate. Matched case-insensitively; `deferred:` is
/// a prefix. Returns the matched tag (for the parked reason). trace:STORY-527
fn parking_tag(tags: &[String]) -> Option<String> {
    for t in tags {
        let lo = t.trim().to_ascii_lowercase();
        let parks = lo == "blocked"
            || lo == "needs-human-input"
            || lo == "needs-human"
            // A spec awaiting a human design/architecture decision or an
            // explicit operator action is NOT autonomously pickable, even
            // though it's bounded + unblocked. (Found dogfooding /aida-burndown:
            // STORY-493 needs-design-signoff + STORY-497 operator-action slipped
            // the gate.) trace:STORY-527 | ai:claude
            || lo == "needs-design-signoff"
            || lo == "needs-design"
            || lo == "operator-action"
            || lo == "review:draft-only"
            || lo.starts_with("deferred:");
        if parks {
            return Some(t.trim().to_string());
        }
    }
    None
}

/// The pickability gate. READY iff the spec is bounded (not an epic),
/// decision-free, unblocked, and not parking-tagged. Exclusions are ordered
/// cheapest/broadest first so the parked reason names the most fundamental
/// blocker. trace:STORY-527
pub(crate) fn classify(c: &BurndownCandidate) -> Pickability {
    if c.req_type.eq_ignore_ascii_case("epic") {
        return Pickability::Parked("epic — decompose into bounded specs first".to_string());
    }
    if c.has_pending_decision {
        return Pickability::Parked(
            "pending decision request — answer via `aida questions`".to_string(),
        );
    }
    if c.has_unsatisfied_blocker {
        return Pickability::Parked("blocked by an unsatisfied dependency (BlockedBy)".to_string());
    }
    if let Some(tag) = parking_tag(&c.tags) {
        return Pickability::Parked(format!("tagged `{tag}`"));
    }
    Pickability::Ready
}

/// Partition candidates into `(ready_ids, parked)` preserving input order —
/// the fan-out set and the skipped set with reasons. trace:STORY-527
pub(crate) fn partition(candidates: &[BurndownCandidate]) -> (Vec<String>, Vec<(String, String)>) {
    let mut ready = Vec::new();
    let mut parked = Vec::new();
    for c in candidates {
        match classify(c) {
            Pickability::Ready => ready.push(c.id.clone()),
            Pickability::Parked(reason) => parked.push((c.id.clone(), reason)),
        }
    }
    (ready, parked)
}

/// STORY-547: the broader "why is this open spec *still open*?" classifier.
/// Where [`classify`] answers the narrow pickability question for the candidate
/// set (the approved+queued specs a burndown would fan out), `explain_open`
/// answers it for **every** open spec, deriving the reason purely from store
/// signals (type, status, tags, BlockedBy edges, pending decisions, live
/// leases). No new stored field, no hand-written status, no findings — the
/// reason a spec stays open is already latent in the substrate; this just reads
/// it back. trace:STORY-547 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenBucket {
    /// Built; a draft PR is held for human review (`review:draft-only`).
    HeldForReview,
    /// A live session lease is working it right now.
    InFlight,
    /// Parked on a human decision (pending DecisionRequest, design-signoff,
    /// operator-action, needs-human, or NeedsAttention triage).
    AwaitingDecision,
    /// Deliberately postponed (`deferred:<why>`).
    Deferred,
    /// Blocked by an unsatisfied dependency.
    Blocked,
    /// An umbrella epic — driven by its children, not directly pickable.
    Umbrella,
    /// A vision/principle — no terminal state by design.
    LongLived,
    /// Draft, not yet advisor-approved.
    Ungroomed,
    /// Done on a branch — awaiting merge to the default branch.
    AwaitingMerge,
    /// Work in progress.
    InProgress,
    /// Approved & unblocked — ready to pick up (the burndown ready set).
    Actionable,
}

impl OpenBucket {
    /// Stable kebab-case key for JSON / grouping.
    pub(crate) fn key(self) -> &'static str {
        match self {
            OpenBucket::HeldForReview => "held-for-review",
            OpenBucket::InFlight => "in-flight",
            OpenBucket::AwaitingDecision => "awaiting-decision",
            OpenBucket::Deferred => "deferred",
            OpenBucket::Blocked => "blocked",
            OpenBucket::Umbrella => "umbrella",
            OpenBucket::LongLived => "long-lived",
            OpenBucket::Ungroomed => "ungroomed",
            OpenBucket::AwaitingMerge => "awaiting-merge",
            OpenBucket::InProgress => "in-progress",
            OpenBucket::Actionable => "actionable",
        }
    }

    /// True for the buckets that genuinely need a human nudge (vs. those that
    /// will resolve themselves through normal flow). Drives the explainer's
    /// "needs you" grouping. trace:STORY-547
    pub(crate) fn needs_human(self) -> bool {
        matches!(
            self,
            OpenBucket::HeldForReview
                | OpenBucket::AwaitingDecision
                | OpenBucket::Ungroomed
                | OpenBucket::Umbrella
        )
    }
}

/// Already-probed facts about one OPEN spec. `status` is normalized to
/// alphanumeric-lowercase (`inprogress`, `needsattention`, …) by the caller so
/// this stays pure + exhaustively testable. trace:STORY-547
#[derive(Debug, Clone)]
pub(crate) struct OpenFacts {
    /// Display SPEC-ID.
    pub id: String,
    /// Lowercased requirement type (`epic`, `vision`, `task`, …).
    pub req_type: String,
    /// Normalized status key (`draft`, `approved`, `inprogress`, `done`, …).
    pub status: String,
    /// The spec's tags.
    pub tags: Vec<String>,
    /// A `BlockedBy` edge points at a not-yet-Completed spec.
    pub has_unsatisfied_blocker: bool,
    /// Carries a pending `DecisionRequest`.
    pub has_pending_decision: bool,
    /// A live session lease's scope matches this spec.
    pub in_flight: bool,
}

/// Classify one open spec into its `(bucket, human-readable reason)`. Precedence
/// runs live/human signals first (most actionable + most current), then
/// structural facts (epic / vision), then a status fallback — so the reason
/// names the *most specific* thing keeping the spec open. trace:STORY-547
pub(crate) fn explain_open(f: &OpenFacts) -> (OpenBucket, String) {
    let has_tag =
        |name: &str| -> bool { f.tags.iter().any(|t| t.trim().eq_ignore_ascii_case(name)) };

    if f.in_flight {
        return (
            OpenBucket::InFlight,
            "in flight — a live session lease is working this now".to_string(),
        );
    }
    if has_tag("review:draft-only") {
        return (
            OpenBucket::HeldForReview,
            "built — held as a draft PR for human review (`review:draft-only`)".to_string(),
        );
    }
    if f.has_pending_decision {
        return (
            OpenBucket::AwaitingDecision,
            "awaiting a human decision — answer via `aida questions`".to_string(),
        );
    }
    for t in &f.tags {
        let lo = t.trim().to_ascii_lowercase();
        if lo == "needs-design-signoff"
            || lo == "needs-design"
            || lo == "operator-action"
            || lo == "needs-human"
            || lo == "needs-human-input"
        {
            return (
                OpenBucket::AwaitingDecision,
                format!("awaiting a human decision (tagged `{}`)", t.trim()),
            );
        }
    }
    for t in &f.tags {
        if t.trim().to_ascii_lowercase().starts_with("deferred:") {
            return (
                OpenBucket::Deferred,
                format!("deliberately deferred (tagged `{}`)", t.trim()),
            );
        }
    }
    if f.has_unsatisfied_blocker || has_tag("blocked") {
        return (
            OpenBucket::Blocked,
            "blocked by an unsatisfied dependency (BlockedBy → a not-yet-Completed spec)"
                .to_string(),
        );
    }
    if f.req_type.eq_ignore_ascii_case("epic") {
        return (
            OpenBucket::Umbrella,
            "umbrella epic — driven by its children; decompose or complete them".to_string(),
        );
    }
    if f.req_type.eq_ignore_ascii_case("vision") || f.req_type.eq_ignore_ascii_case("principle") {
        return (
            OpenBucket::LongLived,
            format!("long-lived {} — no terminal state by design", f.req_type),
        );
    }
    match f.status.as_str() {
        "needsattention" => (
            OpenBucket::AwaitingDecision,
            "parked for triage (NeedsAttention) — see `aida findings list`".to_string(),
        ),
        "draft" => (
            OpenBucket::Ungroomed,
            "draft — awaiting advisor grooming/approval before it can be picked up".to_string(),
        ),
        "done" => (
            OpenBucket::AwaitingMerge,
            "done on a branch — awaiting merge to the default branch (auto-completes on merge)"
                .to_string(),
        ),
        "inprogress" => (OpenBucket::InProgress, "work in progress".to_string()),
        _ => (
            OpenBucket::Actionable,
            "ready to pick up — approved & unblocked (appears in the `burndown plan` ready set)"
                .to_string(),
        ),
    }
}

/// Plain-language description of the active selector for the human-facing
/// header — glosses the bare word "selector" so a new user understands what is
/// being shown and how to narrow it. Pure (no color), so it's unit-testable;
/// the caller colorizes. trace:STORY-544 | ai:claude
pub(crate) fn selector_summary(status: &str, tag: Option<&str>, batch: Option<&str>) -> String {
    let mut filters: Vec<String> = Vec::new();
    if let Some(t) = tag {
        filters.push(format!("tag {t}"));
    }
    if let Some(b) = batch {
        filters.push(format!("batch {b}"));
    }
    let scope = if filters.is_empty() {
        format!("Showing {status} specs (default).")
    } else {
        format!(
            "Showing {status} specs filtered to {}.",
            filters.join(" + ")
        )
    };
    format!("{scope} Narrow with --batch NAME, --tag X, or --status <s>.")
}

/// The next-step footer printed after a non-empty ready set. Tells the user the
/// run itself is the `/aida-burndown` Claude Code skill — there is deliberately
/// no `aida burndown run`/`start` CLI verb — so they stop hunting for one.
/// Pure text (no color); the caller colorizes. trace:STORY-544 | ai:claude
pub(crate) fn next_step_footer() -> String {
    "Next step: invoke /aida-burndown in Claude Code to fan out the ready set above.\n\
     There is no `aida burndown run`/`start` — the runner is the /aida-burndown skill, \
     not a CLI subcommand."
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(
        id: &str,
        req_type: &str,
        tags: &[&str],
        blocked: bool,
        decision: bool,
    ) -> BurndownCandidate {
        BurndownCandidate {
            id: id.to_string(),
            req_type: req_type.to_string(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            has_unsatisfied_blocker: blocked,
            has_pending_decision: decision,
        }
    }

    #[test]
    fn bounded_unblocked_decision_free_spec_is_ready() {
        assert_eq!(
            classify(&cand("TASK-1", "task", &["papercut"], false, false)),
            Pickability::Ready
        );
    }

    #[test]
    fn epic_is_parked_for_decomposition() {
        match classify(&cand("EPIC-1", "epic", &[], false, false)) {
            Pickability::Parked(r) => assert!(r.contains("decompose")),
            other => panic!("expected Parked, got {other:?}"),
        }
    }

    #[test]
    fn pending_decision_parks() {
        assert!(matches!(
            classify(&cand("STORY-1", "story", &[], false, true)),
            Pickability::Parked(_)
        ));
    }

    #[test]
    fn unsatisfied_blocker_parks() {
        assert!(matches!(
            classify(&cand("TASK-2", "task", &[], true, false)),
            Pickability::Parked(_)
        ));
    }

    #[test]
    fn parking_tags_park_case_insensitively_with_deferred_prefix() {
        for tag in [
            "blocked",
            "needs-human-input",
            "Needs-Human",
            "needs-design-signoff",
            "operator-action",
            "review:draft-only",
            "deferred:post-stability",
        ] {
            match classify(&cand("X-1", "task", &[tag], false, false)) {
                Pickability::Parked(r) => assert!(r.to_lowercase().contains(&tag.to_lowercase())),
                other => panic!("tag {tag} should park, got {other:?}"),
            }
        }
        // A benign tag does not park.
        assert_eq!(
            classify(&cand(
                "X-2",
                "task",
                &["batch:foo", "papercut"],
                false,
                false
            )),
            Pickability::Ready
        );
    }

    #[test]
    fn partition_preserves_order_and_separates() {
        let cands = vec![
            cand("A", "task", &[], false, false),             // ready
            cand("B", "epic", &[], false, false),             // parked (epic)
            cand("C", "task", &["deferred:x"], false, false), // parked (tag)
            cand("D", "story", &[], false, false),            // ready
        ];
        let (ready, parked) = partition(&cands);
        assert_eq!(ready, vec!["A".to_string(), "D".to_string()]);
        assert_eq!(
            parked.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>(),
            vec!["B".to_string(), "C".to_string()]
        );
    }

    // STORY-544: the human-facing presentation helpers — plain-language
    // selector gloss + next-step footer pointing at the /aida-burndown skill.
    #[test]
    fn selector_summary_default_is_plain_language() {
        let s = selector_summary("approved", None, None);
        assert!(s.contains("Showing approved specs (default)."));
        // The narrowing hint names all three knobs and drops bare "selector".
        assert!(s.contains("--batch NAME"));
        assert!(s.contains("--tag X"));
        assert!(s.contains("--status <s>"));
        assert!(!s.to_lowercase().contains("selector:"));
    }

    #[test]
    fn selector_summary_reflects_filters() {
        let s = selector_summary("draft", Some("papercut"), Some("scaffolding"));
        assert!(s.contains("Showing draft specs filtered to tag papercut + batch scaffolding."));
    }

    // STORY-547: the broader "why still open" explainer.
    fn open(
        req_type: &str,
        status: &str,
        tags: &[&str],
        blocked: bool,
        decision: bool,
        in_flight: bool,
    ) -> OpenFacts {
        OpenFacts {
            id: "X-1".to_string(),
            req_type: req_type.to_string(),
            status: status.to_string(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            has_unsatisfied_blocker: blocked,
            has_pending_decision: decision,
            in_flight,
        }
    }

    #[test]
    fn explain_open_buckets_every_status_and_signal() {
        // Live + held + decision signals win over everything else.
        assert_eq!(
            explain_open(&open("task", "approved", &[], false, false, true)).0,
            OpenBucket::InFlight
        );
        assert_eq!(
            explain_open(&open(
                "task",
                "approved",
                &["review:draft-only"],
                false,
                false,
                false
            ))
            .0,
            OpenBucket::HeldForReview
        );
        assert_eq!(
            explain_open(&open("task", "approved", &[], false, true, false)).0,
            OpenBucket::AwaitingDecision
        );
        assert_eq!(
            explain_open(&open(
                "bug",
                "approved",
                &["needs-design-signoff"],
                false,
                false,
                false
            ))
            .0,
            OpenBucket::AwaitingDecision
        );
        assert_eq!(
            explain_open(&open(
                "task",
                "draft",
                &["deferred:post-stability"],
                false,
                false,
                false
            ))
            .0,
            OpenBucket::Deferred
        );
        assert_eq!(
            explain_open(&open("story", "approved", &[], true, false, false)).0,
            OpenBucket::Blocked
        );
        // Structural facts.
        assert_eq!(
            explain_open(&open("epic", "planned", &[], false, false, false)).0,
            OpenBucket::Umbrella
        );
        assert_eq!(
            explain_open(&open("vision", "inprogress", &[], false, false, false)).0,
            OpenBucket::LongLived
        );
        // Status fallbacks.
        assert_eq!(
            explain_open(&open("task", "needsattention", &[], false, false, false)).0,
            OpenBucket::AwaitingDecision
        );
        assert_eq!(
            explain_open(&open("task", "draft", &[], false, false, false)).0,
            OpenBucket::Ungroomed
        );
        assert_eq!(
            explain_open(&open("task", "done", &[], false, false, false)).0,
            OpenBucket::AwaitingMerge
        );
        assert_eq!(
            explain_open(&open("task", "inprogress", &[], false, false, false)).0,
            OpenBucket::InProgress
        );
        // Approved + unblocked + ungated = the burndown-ready case.
        assert_eq!(
            explain_open(&open(
                "task",
                "approved",
                &["papercut"],
                false,
                false,
                false
            ))
            .0,
            OpenBucket::Actionable
        );
    }

    #[test]
    fn explain_open_precedence_decision_beats_structural() {
        // An epic that is ALSO tagged for a human decision reports the decision
        // (the more actionable, human-facing reason) rather than "umbrella".
        let (bucket, _) = explain_open(&open(
            "epic",
            "draft",
            &["needs-design-signoff"],
            false,
            false,
            false,
        ));
        assert_eq!(bucket, OpenBucket::AwaitingDecision);
    }

    #[test]
    fn open_bucket_keys_and_needs_human_are_stable() {
        assert_eq!(OpenBucket::HeldForReview.key(), "held-for-review");
        assert_eq!(OpenBucket::Deferred.key(), "deferred");
        assert!(OpenBucket::HeldForReview.needs_human());
        assert!(OpenBucket::Ungroomed.needs_human());
        assert!(OpenBucket::Umbrella.needs_human());
        // Self-resolving / flow buckets don't demand a human nudge.
        assert!(!OpenBucket::InProgress.needs_human());
        assert!(!OpenBucket::AwaitingMerge.needs_human());
        assert!(!OpenBucket::Deferred.needs_human());
    }

    #[test]
    fn next_step_footer_points_at_skill_and_denies_cli_verb() {
        let f = next_step_footer();
        // (a) tells the user to invoke the skill.
        assert!(f.contains("/aida-burndown"));
        // (b) explicitly states there is no `aida burndown run`/`start`.
        assert!(f.contains("no `aida burndown run`/`start`"));
        assert!(f.to_lowercase().contains("skill"));
        // No internal trace SPEC-IDs leak into user-facing text.
        assert!(!f.contains("STORY-"));
    }
}
