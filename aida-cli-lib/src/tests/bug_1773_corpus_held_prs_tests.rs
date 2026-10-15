//! BUG-1773: `held_prs` derived from the review-verdict corpus.
//!
//! BUG-1705 measured the leak on a real PR: #2242 carried `request-changes` at
//! its exact head, and `aida awaiting` listed it nowhere, because `held_prs` was
//! built from `.aida/merge-holds/PR-<n>` markers and only
//! `handle_review_record_at` writes one.
//!
//! Every case here drives `review_verdict::corpus_hold_at_head` directly: the
//! decision is a pure function of (verdict bodies, verdict shas, PR head sha,
//! closed/completed flags), with no network and no clock (AC5).
// trace:BUG-1773 | ai:claude

use crate::review_verdict::{self, VerdictKind};

const HEAD: &str = "9cd65761d7aa4f3b2c1e0d9a8b7c6d5e4f3a2b1c";
const OLD: &str = "1111111111111111111111111111111111111111";

/// One verdict artifact body. `findings` is the itemised list the row counts.
fn body(verdict: &str, sha: &str, findings: &[&str], closed: Option<&str>) -> String {
    let findings = findings
        .iter()
        .map(|f| format!("{:?}", f))
        .collect::<Vec<_>>()
        .join(",");
    let closed = match closed {
        Some(sha) => format!(r#","closed_by_merge":"{sha}""#),
        None => String::new(),
    };
    format!(
        r#"{{"verdict":"{verdict}","reviewed_sha":"{sha}","recorded_by":"reviewer-a","findings":[{findings}]{closed}}}"#
    )
}

// AC1 — the reported bug. A refusal at the PR's CURRENT head holds it, with no
// marker file and no label anywhere in the inputs.
#[test]
fn request_changes_at_the_current_head_holds_the_pr() {
    let hold = review_verdict::corpus_hold_at_head(
        &[body(
            "request-changes",
            HEAD,
            &["missing unwind test"],
            None,
        )],
        Some(HEAD),
        false,
    )
    .expect("a refusal at the current head must hold the PR");
    assert_eq!(hold.sha, HEAD);
    assert_eq!(hold.verdict_raw, "request-changes");
    assert_eq!(hold.findings, 1);
    assert!(hold.integrity_error.is_none());
}

// AC1 — `rejected` is a refusal too, not only `request-changes`.
#[test]
fn rejected_at_the_current_head_holds_the_pr() {
    assert!(review_verdict::corpus_hold_at_head(
        &[body("rejected", HEAD, &[], None)],
        Some(HEAD),
        false
    )
    .is_some());
}

// AC3a — the over-holding case that reusing `local_verdict_blocks_merge` would
// cause: a verdict left behind at a head the PR has moved past. That predicate
// returns `true` here (not cleanly approved); this view must hold nothing.
#[test]
fn a_verdict_only_at_an_older_head_holds_nothing() {
    assert!(review_verdict::corpus_hold_at_head(
        &[body("request-changes", OLD, &["stale"], None)],
        Some(HEAD),
        false
    )
    .is_none());
}

// AC3b — closed by a merge. `reconcile_artifacts_for_sha` never reads
// `closed_by_merge`, so this exclusion is the one `corpus_hold_at_head` has to
// apply itself; if it stops doing so, this is the test that notices.
#[test]
fn a_verdict_closed_by_a_merge_holds_nothing() {
    assert!(review_verdict::corpus_hold_at_head(
        &[body("request-changes", HEAD, &["addressed"], Some(OLD))],
        Some(HEAD),
        false
    )
    .is_none());
}

// AC3c — the BUG-1529 criterion-4 fallback: a Completed spec shipped by
// definition, so a still-refusing record on it is the measured false positive.
#[test]
fn a_blocking_verdict_on_a_completed_spec_holds_nothing() {
    assert!(review_verdict::corpus_hold_at_head(
        &[body("request-changes", HEAD, &["shipped anyway"], None)],
        Some(HEAD),
        /* spec_completed */ true
    )
    .is_none());
}

// AC3d — a VIEW fails OPEN on an unknown head. The merge gate
// (`local_verdict_blocks_merge`) fails CLOSED on the same input, deliberately;
// collapsing the two policies into one predicate is what would list every PR
// whose head could not be read.
#[test]
fn an_unknown_head_holds_nothing_even_with_a_live_refusal() {
    assert!(review_verdict::corpus_hold_at_head(
        &[body("request-changes", HEAD, &["real finding"], None)],
        None,
        false
    )
    .is_none());
}

// AC3e — an approval at the current head releases.
#[test]
fn an_approval_at_the_current_head_holds_nothing() {
    assert!(review_verdict::corpus_hold_at_head(
        &[body("approved", HEAD, &[], None)],
        Some(HEAD),
        false
    )
    .is_none());
}

// AC3e, the BUG-1705 AC3 prohibition: an `approved` recorded at the OLD
// rejected sha must NOT lift a refusal standing at the current head.
#[test]
fn an_approval_at_the_old_sha_does_not_lift_a_refusal_at_the_head() {
    let hold = review_verdict::corpus_hold_at_head(
        &[
            body("request-changes", HEAD, &["still open"], None),
            body("approved", OLD, &[], None),
        ],
        Some(HEAD),
        false,
    );
    assert!(
        hold.is_some(),
        "an approval at a sha the PR has moved past must not release the hold"
    );
}

// AC4 — no artifact at all is not an error and holds nothing.
#[test]
fn no_verdict_artifact_holds_nothing() {
    assert!(review_verdict::corpus_hold_at_head(&[], Some(HEAD), false).is_none());
}

// Two reviewers disagreeing at ONE commit is an integrity error, and the hold
// stands: that is a state a human must resolve, not one to render silent.
#[test]
fn conflicting_verdicts_at_one_head_hold_the_pr_and_name_the_conflict() {
    let other =
        format!(r#"{{"verdict":"approved","reviewed_sha":"{HEAD}","recorded_by":"reviewer-b"}}"#);
    let hold = review_verdict::corpus_hold_at_head(
        &[body("request-changes", HEAD, &["disputed"], None), other],
        Some(HEAD),
        false,
    )
    .expect("a conflict at the head must not read as released");
    let message = hold
        .integrity_error
        .expect("the conflict must be named, not swallowed");
    assert!(
        message.contains("conflicting review verdicts"),
        "unexpected message: {message}"
    );
}

// The findings count a row shows is the count recorded against THIS head, so
// the number the reader sees is the number of things they must still address.
#[test]
fn the_findings_count_comes_from_the_recording_at_this_head() {
    let hold = review_verdict::corpus_hold_at_head(
        &[body("request-changes", HEAD, &["a", "b", "c"], None)],
        Some(HEAD),
        false,
    )
    .unwrap();
    assert_eq!(hold.findings, 3);
}

// The projection presents as `rework` — a reader should not have to know which
// producer armed the hold — while the detail says it came from the corpus.
#[test]
fn a_corpus_hold_projects_as_rework_and_names_its_provenance() {
    let hold = review_verdict::corpus_hold_at_head(
        &[body("request-changes", HEAD, &["one"], None)],
        Some(HEAD),
        false,
    )
    .unwrap();
    let item = crate::awaiting_you::project_corpus_held_pr(2242, "a held PR", &hold);
    assert_eq!(item.pr, 2242);
    assert_eq!(
        item.reason_kind,
        crate::merge_hold::HoldReasonKind::Rework.as_str()
    );
    assert!(item.detail.contains("1 finding"), "detail: {}", item.detail);
    assert!(
        item.detail.contains("verdict corpus"),
        "the row must say no marker backed it: {}",
        item.detail
    );
    assert!(item.action.contains("address the review findings"));
}

// `VerdictKind::Unknown` — a QUALIFIED approval such as "approved, pending
// cross-platform green" (BUG-1505: the qualification is exactly what makes it
// not an approval). At the current head this is neither an approval nor an
// interpretable refusal, and `reconcile_artifacts_for_sha` calls it an integrity
// error. It is surfaced as a hold rather than silently released: PRIN-5 —
// unreadable evidence is not good evidence — and `held_prs` already has the
// precedent of showing an unreadable hold rather than hiding it. Contrast the
// unknown-HEAD case above, which holds nothing: a missing head is missing
// information about the PR, whereas this is positive evidence that someone
// recorded something nobody can interpret.
#[test]
fn a_qualified_approval_at_the_head_is_surfaced_not_released() {
    assert_eq!(
        VerdictKind::parse("approved pending cross-platform green"),
        VerdictKind::Unknown
    );
    let hold = review_verdict::corpus_hold_at_head(
        &[body(
            "approved pending cross-platform green",
            HEAD,
            &[],
            None,
        )],
        Some(HEAD),
        false,
    )
    .expect("a qualified approval is not an approval and must not read as released");
    assert!(
        hold.integrity_error
            .is_some_and(|m| m.contains("unrecognised verdict")),
        "the row must say the verdict word could not be interpreted"
    );
}

// ---------------------------------------------------------------------------
// The seam the assembled report actually calls. `corpus_hold_at_head` being
// correct proves nothing if nothing reaches it, so these drive
// `awaiting_you::corpus_held_prs` — the function `collect_awaiting_report_inner`
// extends `held_prs` with.
// ---------------------------------------------------------------------------

fn candidate(pr: u64, bodies: Vec<String>, has_marker_hold: bool) -> CorpusHoldCandidate {
    CorpusHoldCandidate {
        is_draft: false,
        pr,
        title: format!("PR {pr}"),
        head_sha: Some(HEAD.to_string()),
        bodies,
        spec_completed: false,
        has_marker_hold,
    }
}

use crate::awaiting_you::{corpus_held_prs, CorpusHoldCandidate};

// AC1 end-to-end at the seam the report calls.
#[test]
fn the_seam_surfaces_a_refusal_that_no_marker_backs() {
    let items = corpus_held_prs(&[candidate(
        2242,
        vec![body(
            "request-changes",
            HEAD,
            &["missing unwind test"],
            None,
        )],
        false,
    )]);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].pr, 2242);
    assert_eq!(
        items[0].reason_kind,
        crate::merge_hold::HoldReasonKind::Rework.as_str()
    );
}

// AC2 — a PR with BOTH a marker-backed hold and a corpus refusal appears once.
// The marker arm (`project_held_pr`) already projected it with its richer typed
// reason, so the corpus arm must stay silent rather than add a second row.
#[test]
fn a_marker_backed_hold_is_not_duplicated_by_the_corpus_arm() {
    let items = corpus_held_prs(&[candidate(
        2242,
        vec![body("request-changes", HEAD, &["one"], None)],
        /* has_marker_hold */ true,
    )]);
    assert!(
        items.is_empty(),
        "the marker arm already speaks for this PR: {items:?}"
    );
}

// An open PR with a clean corpus contributes nothing — the quiet day stays
// quiet, which is the signal the report depends on.
#[test]
fn the_seam_adds_nothing_for_prs_with_no_outstanding_refusal() {
    let items = corpus_held_prs(&[
        candidate(1, vec![], false),
        candidate(2, vec![body("approved", HEAD, &[], None)], false),
        candidate(
            3,
            vec![body("request-changes", OLD, &["stale"], None)],
            false,
        ),
    ]);
    assert!(items.is_empty(), "unexpected rows: {items:?}");
}

// Several PRs are judged independently — one refusal does not tar the rest.
#[test]
fn the_seam_judges_each_pr_independently() {
    let items = corpus_held_prs(&[
        candidate(1, vec![body("approved", HEAD, &[], None)], false),
        candidate(
            2,
            vec![body("request-changes", HEAD, &["real"], None)],
            false,
        ),
        candidate(3, vec![], false),
    ]);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].pr, 2);
}

// The SHAPE of the artifact BUG-1705 was actually filed about. Measured in this
// repository on 2026-10-02: `.aida/review-verdicts/PR-2242.json` carries
// `verdict`, `findings`, `mode` and `summary` — and NO sha provenance of any
// kind. The spec's description says the refusal was recorded "against
// 9cd65761d7, its exact current head", but the artifact never recorded a sha, so
// no sha-matching predicate can bind it to a head.
//
// It therefore reaches the integrity branch rather than the sha-match branch,
// which is precisely why a refusal must HOLD on an unreconcilable corpus: had
// this returned `None`, the fix would not catch the one instance the parent bug
// was filed about. This test is the regression guard for that choice.
// trace:BUG-1773 | ai:claude
#[test]
fn the_bug_1705_witness_shape_is_held_although_it_records_no_sha() {
    let measured = r#"{
      "findings":["no test deliberately panics and verifies the warning sink is drained"],
      "mode":"orchestrator-phase-3",
      "summary":"Independent exact-head review requests a focused unwind test",
      "verdict":"CHANGES REQUESTED"
    }"#;
    let hold = review_verdict::corpus_hold_at_head(&[measured.to_string()], Some(HEAD), false)
        .expect("the refusal BUG-1705 reported must not read as released");
    let message = hold
        .integrity_error
        .expect("a verdict with no provenance must say so");
    assert!(
        message.contains("no reviewed_sha/head provenance"),
        "unexpected message: {message}"
    );
    // And the seam surfaces it as a row, not just the predicate.
    let items = corpus_held_prs(&[CorpusHoldCandidate {
        is_draft: false,
        pr: 2242,
        title: "BUG-1673".to_string(),
        head_sha: Some(HEAD.to_string()),
        bodies: vec![measured.to_string()],
        spec_completed: false,
        has_marker_hold: false,
    }]);
    assert_eq!(items.len(), 1, "the reported PR must appear in held_prs");
    assert_eq!(items[0].pr, 2242);
}
