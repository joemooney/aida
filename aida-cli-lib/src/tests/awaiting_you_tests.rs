// trace:TASK-3424 | ai:codex
use super::*;

fn pr(number: u64, mergeable: Option<&str>, ci: Option<&str>, verdict: Option<&str>) -> OpenPrItem {
    OpenPrItem {
        is_draft: false,
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

fn recusal(
    state: crate::gate::merge_hold_core::HoldRoutingState,
) -> crate::gate::merge_hold_core::MergeHoldRecord {
    crate::gate::merge_hold_core::MergeHoldRecord {
        schema_version: 2,
        pr: 2023,
        reason_kind: crate::gate::merge_hold_core::HoldReasonKind::Recusal,
        detail: "author recusal".into(),
        recused_principals: vec!["agent:author".into()],
        routed_to: vec!["agent:reader".into()],
        routing_state: state,
        target_head_sha: Some("abcdef0123456789".into()),
        label_state: Some("synced".into()),
        legacy: false,
        verdict_ref: None,
        release_condition: None,
        spec: None,
        spec_key: None,
        placed_by: None,
        absorbed: Vec::new(),
    }
}

// trace:STORY-1397 | ai:codex
#[test]
fn recusal_projection_never_offers_the_author_the_merge() {
    let author = project_recusal_hold(
        &recusal(crate::gate::merge_hold_core::HoldRoutingState::Routed),
        "agent:author",
    )
    .unwrap();
    assert_eq!(author.relationship, "recused");
    assert!(author.action.contains("cannot review, clear, or merge"));
    let author_report = AwaitingReport {
        recusal_holds: vec![author],
        ..Default::default()
    };
    assert!(
        author_report.is_empty(),
        "recusal is not the author's action"
    );

    let reader = project_recusal_hold(
        &recusal(crate::gate::merge_hold_core::HoldRoutingState::Routed),
        "agent:reader",
    )
    .unwrap();
    assert_eq!(reader.relationship, "routed-reader");
    assert!(reader.action.contains("exact head"));
}

#[test]
fn no_reader_is_an_explicit_operator_state_in_all_report_shapes() {
    let mut record = recusal(crate::gate::merge_hold_core::HoldRoutingState::NoIndependentReader);
    record.routed_to.clear();
    let item = project_recusal_hold(&record, "operator").unwrap();
    assert_eq!(item.relationship, "operator-attention");
    assert!(item.action.contains("no independent reader"));
    let report = AwaitingReport {
        recusal_holds: vec![item],
        ..Default::default()
    };
    let mut rendered = Vec::new();
    report.render(false, &mut rendered).unwrap();
    let rendered = String::from_utf8(rendered).unwrap();
    assert!(rendered.contains("no independent reader"));
    assert_eq!(
        report.to_json()["recusal_holds"][0]["state"],
        "no-independent-reader"
    );
    assert!(report.compact_line().unwrap().contains("recusal-hold"));
}

// trace:STORY-1397 | ai:claude — review blocker: a supervision-held PR
// must stay visible (with its reason) rather than vanish from awaiting.
#[test]
fn supervision_held_pr_stays_visible_with_its_reason() {
    let record = crate::gate::merge_hold_core::typed_hold(
        2103,
        crate::gate::merge_hold_core::HoldReasonKind::Supervision,
        "STORY-1397 is marked guided — merge requires review",
        None,
    );
    let item = project_held_pr(&record, "route typed recusals", None, false);
    assert_eq!(item.reason_kind, "supervision");
    assert!(item.action.contains("aida merge-hold clear 2103"));
    let report = AwaitingReport {
        held_prs: vec![item],
        ..Default::default()
    };
    assert!(!report.is_empty(), "a held PR is not silence");
    assert_eq!(report.total(), 1);
    let mut rendered = Vec::new();
    report.render(false, &mut rendered).unwrap();
    let rendered = String::from_utf8(rendered).unwrap();
    assert!(rendered.contains("2103"));
    assert!(rendered.contains("[supervision]"));
    assert!(rendered.contains("marked guided"));
    assert!(!rendered.contains("ready to merge"));
    assert_eq!(
        report.to_json()["held_prs"][0]["reason_kind"],
        "supervision"
    );
    assert!(report.compact_line().unwrap().contains("1 held"));
}

#[test]
fn each_non_recusal_hold_kind_names_its_own_action() {
    use crate::gate::merge_hold_core::HoldReasonKind;
    let action = |kind| {
        project_held_pr(
            &crate::gate::merge_hold_core::typed_hold(7, kind, "d", None),
            "t",
            None,
            false,
        )
        .action
    };
    assert!(action(HoldReasonKind::Rework).contains("implementer"));
    assert!(action(HoldReasonKind::Decision).contains("human decision"));
    assert!(action(HoldReasonKind::Unknown).contains("unreadable"));
}

// trace:BUG-1788 | ai:antigravity
#[test]
fn rework_hold_with_approval_flips_action() {
    use crate::gate::merge_hold_core::HoldReasonKind;
    let action_unapproved = project_held_pr(
        &crate::gate::merge_hold_core::typed_hold(7, HoldReasonKind::Rework, "d", None),
        "t",
        None,
        false,
    )
    .action;
    assert!(action_unapproved.contains("implementer"));

    let action_approved = project_held_pr(
        &crate::gate::merge_hold_core::typed_hold(7, HoldReasonKind::Rework, "d", None),
        "t",
        Some("48433c6b00000000000000000000000000000000"),
        false,
    )
    .action;
    assert!(action_approved.contains("rework approved at 48433c6"));
    assert!(action_approved.contains("awaiting human: aida merge-hold clear"));
}

#[test]
fn stale_head_recusal_is_operator_attention_not_a_phantom_route() {
    let mut record = recusal(crate::gate::merge_hold_core::HoldRoutingState::Routed);
    record = crate::gate::merge_hold_core::project_live_head(&record, Some("newhead"));
    let item = project_recusal_hold(&record, "").unwrap();
    assert_eq!(item.state, "stale-head");
    assert_eq!(item.relationship, "operator-attention");
    assert!(item.action.contains("merge-hold list --fix"));
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
        unowned_failing_prs: classify_unowned_failing_prs(&[failing_candidate(2035)], fixed_now()),
        ..Default::default()
    };
    let mut rendered = Vec::new();
    assert!(report.render(false, &mut rendered).unwrap());
    let rendered = strip_ansi(&String::from_utf8(rendered).unwrap());
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
        relayed_from: None,
    }
}

fn to(agent: &str) -> aida_core::mailbox::Recipient {
    aida_core::mailbox::Recipient::Agent(agent.to_string())
}

// ── BUG-HOME-040: role mail is direct, named, and not buried ───────────
// trace:BUG-B6B1278890774EF993D2EBC327862470-040 | ai:claude

const NOW: i64 = 100 * 86_400;

fn intent_mail(
    id: &str,
    from: &str,
    recipient: &str,
    intent: aida_core::mailbox::Intent,
    age: i64,
) -> aida_core::mailbox::Message {
    // Message timestamps are epoch millis; `NOW` and `age` are seconds.
    let mut m = mail(id, from, to(recipient), (NOW - age) * 1000);
    m.intent = intent;
    m.subject = Some(format!("subject {id}"));
    m
}

fn role_split(msgs: &[aida_core::mailbox::Message]) -> MailChannel {
    split_mail_scopes_for_role(
        "joe",
        Some("advisor"),
        &["advisor".to_string(), "claude".to_string()],
        msgs,
        &std::collections::HashMap::new(),
        NOW,
    )
}

// trace:BUG-b6b1278890774ef993d2ebc327862470-040.ac637a5f | ai:claude
#[test]
fn role_mail_is_direct_not_shared() {
    use aida_core::mailbox::Intent;
    let msgs = vec![
        intent_mail("r1", "codex", "advisor", Intent::Handoff, 60),
        intent_mail("v1", "codex", "claude", Intent::Fyi, 60),
        intent_mail("o1", "codex", "joe", Intent::Fyi, 60),
    ];
    let c = role_split(&msgs);
    assert_eq!(c.unread, 1, "operator's own number is unchanged");
    assert_eq!(c.role_unread, 1);
    assert_eq!(c.role.as_deref(), Some("advisor"));
    assert_eq!(c.shared_unread, 1, "only the vendor inbox is shared");
    assert_eq!(c.shared_scope.as_deref(), Some("claude"));
    let line = AwaitingReport {
        mail: c,
        ..Default::default()
    }
    .compact_line()
    .unwrap();
    assert!(line.contains("1 mail to advisor"), "{line}");
    assert!(line.contains("1 shared mail"), "{line}");
}

// trace:BUG-b6b1278890774ef993d2ebc327862470-040.ac8c3b1b | ai:claude
#[test]
fn previews_put_requests_first_then_newest_capped_at_three() {
    use aida_core::mailbox::Intent;
    let msgs = vec![
        intent_mail("fyi-new", "a", "advisor", Intent::Fyi, 10),
        intent_mail("handoff", "b", "advisor", Intent::Handoff, 100),
        intent_mail("req-old", "c", "advisor", Intent::Request, 900),
        intent_mail("req-new", "d", "joe", Intent::Request, 200),
        intent_mail("fyi-old", "e", "claude", Intent::Fyi, 5000),
    ];
    let c = role_split(&msgs);
    let order: Vec<&str> = c.previews.iter().map(|p| p.from.as_str()).collect();
    assert_eq!(order, vec!["d", "c", "b"], "{:?}", c.previews);
    assert_eq!(
        c.previews[0].line(),
        "[request] from d · 3m ago · subject req-new"
    );
}

// trace:BUG-b6b1278890774ef993d2ebc327862470-040.ac212b9a | ai:claude
#[test]
fn old_automated_notices_drop_out_of_counts_into_one_summary() {
    use aida_core::mailbox::Intent;
    let week = STALE_AUTOMATED_MAIL_SECS;
    let msgs = vec![
        intent_mail("w-old", "review-watch", "advisor", Intent::Fyi, week + 1),
        intent_mail(
            "reap-old",
            "aida-session-reap",
            "joe",
            Intent::Fyi,
            week + 1,
        ),
        intent_mail("w-new", "review-watch", "advisor", Intent::Fyi, week - 1),
        // An old message from a SEAT is still real mail.
        intent_mail("seat-old", "codex", "advisor", Intent::Fyi, week + 1),
    ];
    let c = role_split(&msgs);
    assert_eq!(c.stale_automated, 2);
    assert_eq!(c.unread, 0);
    assert_eq!(c.role_unread, 2, "fresh automated + old seat mail remain");
    assert!(c.previews.iter().all(|p| p.subject != "subject w-old"));
    let r = AwaitingReport {
        mail: MailChannel {
            stale_automated: 2,
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(
        r.compact_line().is_none() && r.mail_preview_lines().is_empty(),
        "old automated mail alone never wakes the notice"
    );
}

#[test]
fn automated_sender_classification() {
    for s in ["review-watch", "aida-session-reap", "advisor-watch-01"] {
        assert!(is_automated_sender(s), "{s}");
    }
    for s in ["codex-implementer-01", "advisor", "joe", "watchful"] {
        assert!(!is_automated_sender(s), "{s}");
    }
}

// trace:BUG-b6b1278890774ef993d2ebc327862470-040.ac206206 | ai:claude
#[test]
fn requests_unread_past_thirty_minutes_escalate_for_the_addressed_seat() {
    use aida_core::mailbox::Intent;
    let msgs = vec![
        intent_mail(
            "late",
            "codex",
            "advisor",
            Intent::Request,
            OVERDUE_REQUEST_SECS + 1,
        ),
        intent_mail(
            "fresh",
            "codex",
            "advisor",
            Intent::Request,
            OVERDUE_REQUEST_SECS - 1,
        ),
        intent_mail("handoff", "codex", "advisor", Intent::Handoff, 3 * 3600),
        // A request in a shared inbox is not addressed to this seat.
        intent_mail("shared", "codex", "claude", Intent::Request, 3 * 3600),
    ];
    let c = role_split(&msgs);
    assert_eq!(c.overdue_requests.len(), 1, "{:?}", c.overdue_requests);
    assert_eq!(c.overdue_requests[0].subject, "subject late");
    let r = AwaitingReport {
        mail: c,
        ..Default::default()
    };
    assert!(r.compact_line().unwrap().contains("1 overdue request"));
    let json = r.to_json();
    assert_eq!(json["mail"]["overdue_requests"][0]["from"], "codex");
    assert_eq!(json["mail"]["role_unread"], 3);
}

// ── BUG-1549: the one PR review classifier, table-driven ────────────────

const HEAD: &str = "cafef00d1234567890";
const OLD: &str = "deadbeef9999999999";
const T1: &str = "2026-09-01T00:00:00+00:00";
const T2: &str = "2026-09-02T00:00:00+00:00";
const T3: &str = "2026-09-03T00:00:00+00:00";

fn verdict(
    kind: &str,
    sha: Option<&str>,
    at: Option<&str>,
) -> crate::gate::review_verdict::RecordedVerdict {
    crate::gate::review_verdict::RecordedVerdict {
        kind: crate::gate::review_verdict::VerdictKind::parse(kind),
        raw: kind.to_string(),
        reviewed_sha: sha.map(str::to_string),
        recorded_at: at.map(str::to_string),
        ..Default::default()
    }
}
fn refusal(sha: Option<&str>, at: Option<&str>) -> crate::gate::review_verdict::RecordedVerdict {
    verdict("request-changes", sha, at)
}
fn approval(sha: Option<&str>, at: Option<&str>) -> crate::gate::review_verdict::RecordedVerdict {
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
        Vec<crate::gate::review_verdict::RecordedVerdict>,
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
// implementer, the advisor and the reviewer instead of nobody; a live
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
    let route = |identity: &str| {
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
                },
                liveness,
            );
        }
        rows.rework_ready
    };
    let prs = |rows: &[ReworkReadyItem]| rows.iter().map(|r| r.pr).collect::<Vec<_>>();

    // The implementer sees both exited rows; the live seat's refusal is
    // not theirs; unknown liveness (2203) shows to all.
    let implementer = route("impl-alice");
    assert_eq!(prs(&implementer), vec![2200, 2202, 2203]);
    assert_eq!(
        implementer[0].inherited_from.as_deref(),
        Some("aida drain reviewer"),
        "an inherited row names the exited recorder"
    );
    assert_eq!(implementer[2].inherited_from, None);

    // The advisor (standing disposition gate) sees the exited rows too.
    assert_eq!(prs(&route("advisor-1")), vec![2200, 2202, 2203]);

    // The live refusing seat keeps its own row, and — a moved-past
    // refusal being a re-review — also sees the exited rows.
    assert_eq!(
        prs(&route("claude-reviewer-1")),
        vec![2200, 2201, 2202, 2203]
    );
    // A different reviewer does not get the live seat's own row.
    assert_eq!(prs(&route("claude-reviewer-2")), vec![2200, 2202, 2203]);

    let mut buf = Vec::new();
    AwaitingReport {
        rework_ready: implementer,
        ..Default::default()
    }
    .render(false, &mut buf)
    .unwrap();
    let out = String::from_utf8(buf).unwrap();
    assert!(
        out.contains("a refusal by aida drain reviewer (recorder exited)"),
        "{out}"
    );
    assert!(!out.contains("now yours"), "{out}");
}

// STORY-1420 review: an exited row reaches every reader — no identity,
// any identity, whatever its case.
// trace:STORY-1420 | ai:claude
#[test]
fn exited_row_reaches_every_reader() {
    let mut r = refusal(Some(OLD), Some(T1));
    r.recorded_by = Some("aida drain reviewer".to_string());
    let d = classify_pr_review(&[r], Some(HEAD));
    for identity in [
        None,
        Some("x"),
        Some("IMPL-alice"),
        Some("claude-reviewer-2"),
    ] {
        let mut rows = PrReviewRows::default();
        rows.add_routed(
            2300,
            Some(HEAD),
            "claude/bug-2300",
            &d,
            ReworkReader { identity },
            |who| classify_recorder(who, |_| None),
        );
        assert_eq!(rows.rework_ready.len(), 1, "{identity:?}");
        assert_eq!(
            rows.rework_ready[0].inherited_from.as_deref(),
            Some("aida drain reviewer")
        );
    }
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
    let out = strip_ansi(&String::from_utf8(buf).unwrap());
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
            partial: false,
            ..Default::default()
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
            pushed: true,
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
    // trace:BUG-1756 | ai:claude — the push axis reaches consumers.
    assert_eq!(json["unshipped_work"][0]["pushed"], true);
    assert!(
        !s.contains("local-only"),
        "a pushed branch must not carry the local-only marker:\n{s}"
    );
}

// BUG-1756 AC1: a never-pushed branch renders DISTINGUISHED from a
// pushed-no-PR branch — TASK-1305 established that collapsing distinct
// unshipped states into one rendering is a defect; this is the same
// principle on the push axis. trace:BUG-1756 | ai:claude
#[test]
fn local_only_unshipped_work_renders_distinct_from_pushed() {
    let r = AwaitingReport {
        unshipped_work: vec![UnshippedWorkItem {
            spec_id: "BUG-1756".to_string(),
            branch: "bug-1756-local".to_string(),
            commits_ahead: 1,
            age: "26h".to_string(),
            recovery: "git push -u origin bug-1756-local && aida pr ship bug-1756-local"
                .to_string(),
            pr_state: "absent".to_string(),
            pushed: false,
        }],
        ..Default::default()
    };
    let mut buf = Vec::new();
    r.render(false, &mut buf).unwrap();
    let s = strip_ansi(&String::from_utf8(buf).unwrap());
    assert!(
        s.contains("local-only (never pushed — this machine holds the only copy)"),
        "the local-only state must be named, not collapsed into pushed-no-PR:\n{s}"
    );
    assert!(
        s.contains("git push -u origin bug-1756-local && aida pr ship bug-1756-local"),
        "the hint names the push first:\n{s}"
    );

    let json = r.to_json();
    assert_eq!(json["unshipped_work"][0]["pushed"], false);
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
            pushed: true,
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
                possibly_subagent: false,
            },
            OrphanedInProgressItem {
                spec_id: "TASK-9002".to_string(),
                title: "never picked up".to_string(),
                abandoned: false,
                since_label: "last-touched time unknown".to_string(),
                possibly_subagent: false,
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

// trace:TASK-1454 | ai:claude
#[test]
fn blocked_seats_render_first_json_and_compact_line() {
    let r = AwaitingReport {
        blocked_seats: vec![BlockedSeatItem {
            session_id: "sess-abc".to_string(),
            spec: Some("TASK-1454".to_string()),
            tool: Some("Bash".to_string()),
            since_label: "3m".to_string(),
        }],
        // A lower-priority channel alongside it, to prove ordering.
        mergeable_prs: vec![MergeablePrItem {
            number: 42,
            title: "some PR".to_string(),
            head_branch: "some-branch".to_string(),
            ci_rollup: Some("pass".to_string()),
            under_review: None,
        }],
        ..Default::default()
    };
    assert_eq!(r.total(), 2);
    let line = r
        .compact_line()
        .expect("blocked seat yields a per-turn line");
    assert!(line.contains("1 BLOCKED seat"), "compact line: {line}");
    // Ordering: the blocked seat leads the compact line, ahead of PRs.
    assert!(
        line.find("BLOCKED seat").unwrap() < line.find("PR").unwrap(),
        "compact line: {line}"
    );

    let mut buf = Vec::new();
    r.render(false, &mut buf).unwrap();
    let s = strip_ansi(&String::from_utf8(buf).unwrap());
    assert!(
        s.contains("TASK-1454") && s.contains("blocked on your approval: Bash"),
        "{s}"
    );
    assert!(s.contains("waiting 3m"), "{s}");
    // Ordering in the rendered text too: the blocked-seat line must
    // appear before the PR line.
    let blocked_pos = s.find("blocked on your approval").unwrap();
    let pr_pos = s.find("PR-42").unwrap();
    assert!(blocked_pos < pr_pos, "{s}");

    let json = r.to_json();
    assert_eq!(json["blocked_seats"][0]["session_id"], "sess-abc");
    assert_eq!(json["blocked_seats"][0]["spec"], "TASK-1454");
    assert_eq!(json["blocked_seats"][0]["tool"], "Bash");
}

// trace:TASK-1454 | ai:claude
#[test]
fn blocked_seat_with_no_resolved_spec_falls_back_to_session_id() {
    let r = AwaitingReport {
        blocked_seats: vec![BlockedSeatItem {
            session_id: "sess-xyz".to_string(),
            spec: None,
            tool: None,
            since_label: "45s".to_string(),
        }],
        ..Default::default()
    };
    let mut buf = Vec::new();
    r.render(false, &mut buf).unwrap();
    let s = strip_ansi(&String::from_utf8(buf).unwrap());
    assert!(s.contains("sess-xyz"), "{s}");
    assert!(s.contains("a tool call"), "{s}");
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
        pushed: true,
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
        pushed: true,
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
    let out = strip_ansi(&String::from_utf8(buf).unwrap());
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
    // trace:TASK-1311 | ai:claude
    assert_eq!(v["escalations"][0]["requeue"], "aida queue rework SPIKE-12");
}

/// TASK-1311: the escalation row names the one-keystroke requeue and the
/// status it lands in, so returning a triaged spec to flight is not left
/// to the operator remembering which flag to pass.
// trace:TASK-1311 | ai:claude
#[test]
fn escalation_row_offers_the_requeue_inline() {
    let r = AwaitingReport {
        escalations: vec![EscalationItem {
            spec_id: "BUG-77".into(),
            title: "punted".into(),
        }],
        ..Default::default()
    };
    let mut buf = Vec::new();
    r.render(true, &mut buf).unwrap();
    let s = strip_ansi(&String::from_utf8(buf).unwrap());
    assert!(
        s.contains("aida queue rework BUG-77") && s.contains("(to Approved)"),
        "{s}"
    );
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

// ---- BUG-1739: agent-mode render covers every `total()` contributor ----
//
// The defect: `total()` and the inline agent render were edited
// independently, so nine contributors counted toward `awaiting:` with no
// agent-mode output at all. Three specs stranded In-Progress for 64 hours
// were inside the headline and printed nowhere, and three relay sessions
// in a row could not diagnose it from the output.
//
// The pin is a TABLE of (contribution key, mutator). One test asserts the
// table's keys are exactly `contributions()`'s keys — so a tenth
// contributor cannot be added to `total()` without a row here — and the
// next renders each mutated report FOR REAL and requires the output to
// change. A source-scanning guard would only prove two files mention the
// same tokens; this proves the renderer emits something.
// trace:BUG-1739 | ai:claude

/// One populated example per contributor, keyed by its `contributions()`
/// key. Each mutator must make exactly its own contributor non-zero.
/// A contribution key paired with a mutator that populates exactly that
/// contributor.
type ContributionFixture = (&'static str, fn(&mut AwaitingReport));

fn contribution_fixtures() -> Vec<ContributionFixture> {
    vec![
        ("prs", |r| {
            r.mergeable_prs.push(MergeablePrItem {
                number: 1,
                title: "pr".into(),
                head_branch: "b".into(),
                ci_rollup: None,
                under_review: None,
            })
        }),
        ("idle_eligible", |r| {
            r.idle_eligible
                .push(crate::idle_eligible::IdleEligibleItem {
                    pr: 42,
                    head_sha: "abcdef1234".into(),
                    state: "review".into(),
                    kind: "unreviewed-push".into(),
                    idle_minutes: 20,
                    first_observed_at: chrono::Utc::now(),
                    observed_at: chrono::Utc::now(),
                    next_command: "aida queue work PR-42 --role reviewer".into(),
                });
        }),
        ("review_watch_skipped", |r| {
            r.review_watch_skipped.push(WatchSkippedItem {
                change: "PR-7".into(),
                head: "a".repeat(40),
                reason: "behind `main` with overlap; update-branch conflicts".into(),
                since: chrono::Utc::now(),
            });
        }),
        ("recusal_holds", |r| {
            r.recusal_holds.push(RecusalHoldItem {
                pr: 2,
                head_sha: Some("abc1234".into()),
                recused_principals: vec![],
                routed_to: vec![],
                state: "held".into(),
                // Anything but "recused"/"other-reader" is actionable.
                relationship: "author".into(),
                action: "route".into(),
            })
        }),
        ("held_prs", |r| {
            r.held_prs.push(HeldPrItem {
                pr: 3,
                title: "held".into(),
                reason_kind: "rework".into(),
                detail: "d".into(),
                action: "a".into(),
            })
        }),
        ("unowned_failing_prs", |r| {
            r.unowned_failing_prs.push(UnownedFailingPrItem {
                number: 4,
                title: "broken".into(),
                head_branch: "b4".into(),
                done_spec: None,
            })
        }),
        ("briefs", |r| {
            r.pending_briefs.push(PendingBriefItem {
                agent: "claude".into(),
                spec_id: "BUG-1".into(),
                path: std::env::temp_dir().join("brief"),
            })
        }),
        ("findings", |r| r.findings_total = 2),
        ("mail_unread", |r| r.mail.unread = 1),
        ("mail_role_unread", |r| r.mail.role_unread = 1),
        ("mail_overdue_requests", |r| {
            r.mail.overdue_requests.push(MailPreview {
                from: "codex".into(),
                intent: aida_core::mailbox::Intent::Request,
                age_secs: 3600,
                subject: "route PR".into(),
            })
        }),
        ("mail_shared_unread", |r| r.mail.shared_unread = 1),
        ("mail_partial", |r| r.mail.partial = true),
        ("directives_pending", |r| r.worker_directives.pending = 1),
        ("cron_due", |r| r.cron.due = 1),
        ("unshipped_work", |r| {
            r.unshipped_work.push(UnshippedWorkItem {
                spec_id: "BUG-2".into(),
                branch: "bug-2".into(),
                commits_ahead: 3,
                age: "2h".into(),
                recovery: "aida rebase".into(),
                pr_state: "none".into(),
                pushed: true,
            })
        }),
        ("rework_ready", |r| {
            r.rework_ready.push(ReworkReadyItem {
                pr: 5,
                spec: Some("BUG-3".into()),
                reviewed_sha: "aaaaaaa".into(),
                head_sha: "bbbbbbb".into(),
                inherited_from: None,
            })
        }),
        ("stale_approvals", |r| {
            r.stale_approvals.push(StaleApprovalItem {
                pr: 6,
                spec: Some("BUG-4".into()),
                reviewed_sha: "ccccccc".into(),
                head_sha: "ddddddd".into(),
                reason: StaleApprovalReason::Stale,
            })
        }),
        ("blocked_reviews", |r| {
            r.blocked_reviews.push(BlockedReviewItem {
                pr: 7,
                spec: Some("BUG-5".into()),
                reviewed_sha: "eeeeeee".into(),
                head_sha: "fffffff".into(),
                reason: BlockedReason::AtHead,
                recorded_at: None,
            })
        }),
        ("nightly_red", |r| {
            r.nightly_red = Some(NightlyRedItem {
                summary: "nightly red".into(),
                run_id: Some(1),
                nights: 2,
            })
        }),
        ("reviewer", |r| {
            r.reviewer_queue_items.push(reviewer_item(
                "BUG-6",
                review_verdict::ReviewActionability::NeedsReview,
            ))
        }),
        // Same population as "reviewer": `total()` counts the routed item
        // AND the BUG-1508 summary line, so this key's own observability is
        // proven by the dedicated reviewer_actionable test below.
        ("reviewer_actionable", |r| {
            r.reviewer_queue_items.push(reviewer_item(
                "BUG-7",
                crate::gate::review_verdict::ReviewActionability::NeedsReview,
            ))
        }),
        ("shelved", |r| r.shelved_total = 4),
        ("escalations", |r| {
            r.escalations.push(EscalationItem {
                spec_id: "BUG-8".into(),
                title: "escalated".into(),
            })
        }),
        ("pr_attribution_disagreements", |r| {
            r.pr_attribution_disagreements
                .push(PrAttributionDisagreementItem {
                    pr: 8,
                    lease_spec: "STORY-1".into(),
                    trailer_spec: "BUG-9".into(),
                })
        }),
        ("orphaned_in_progress", |r| {
            r.orphaned_in_progress.push(OrphanedInProgressItem {
                spec_id: "BUG-1688".into(),
                title: "stranded".into(),
                abandoned: true,
                since_label: "64h 15m ago".into(),
                possibly_subagent: false,
            })
        }),
        // trace:STORY-1680 | ai:claude
        ("idle_in_progress", |r| {
            r.idle_in_progress
                .push(crate::idle_in_progress::IdleInProgressItem {
                    spec_id: "TASK-1680".into(),
                    title: "nothing works it".into(),
                    idle_secs: 30 * 3600,
                    age_source: crate::idle_in_progress::IdleAgeSource::StatusChange,
                    branch: None,
                    threshold_hours: 24,
                })
        }),
        ("unshipped_scan_incomplete", |r| {
            r.unshipped_work_scan = Some(UnshippedScanStatus {
                complete: false,
                scanned: 1,
                candidates: 14,
            })
        }),
        ("blocked_seats", |r| {
            r.blocked_seats.push(BlockedSeatItem {
                session_id: "01a0f28c".into(),
                spec: Some("BUG-10".into()),
                tool: Some("Bash".into()),
                since_label: "5m".into(),
            })
        }),
        // trace:STORY-B6B1278890774EF993D2EBC327862470-155 | ai:claude
        ("deputy_failed", |r| {
            r.deputy_failed.push(crate::deputy::FailedJob {
                deputy: "default".into(),
                id: "0190f0aa-0000-7000-8000-000000000001".into(),
                requester_session: Some("sess-1".into()),
                rc: 3,
                reason: "exit".into(),
                output: "spool/out/x.out".into(),
                kind: crate::deputy::DeputyKind::Command,
            })
        }),
    ]
}

/// AC4's structural half: the fixture table and `contributions()` cannot
/// drift. Add a contributor to `total()` without a fixture and this fails,
/// which is what forces the author into the render test below.
#[test]
fn contribution_fixtures_cover_every_contribution_key() {
    let keys: Vec<&str> = AwaitingReport::default()
        .contributions()
        .iter()
        .map(|(k, _)| *k)
        .collect();
    let fixture_keys: Vec<&str> = contribution_fixtures().iter().map(|(k, _)| *k).collect();
    assert_eq!(
        keys, fixture_keys,
        "every `contributions()` key needs a fixture (and the same order); \
             a new contributor must be rendered in agent mode, not just counted"
    );
    // Keys are the agent-mode vocabulary — duplicates would make the
    // correspondence test below vacuous for the shadowed one.
    let mut sorted = keys.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), keys.len(), "contribution keys must be unique");
}

/// The agent view with the `awaiting:` headline removed. The headline is
/// `total()` itself, so ANY contributor changes it — comparing whole
/// renders would make the correspondence test below pass on a report whose
/// contributor is counted and printed nowhere, which is the exact bug.
/// Proven: dropping the `orphaned_in_progress` table left the unstripped
/// comparison green.
// trace:BUG-1739 | ai:claude
fn agent_view_body(report: &AwaitingReport) -> String {
    report
        .render_agent_view()
        .lines()
        .filter(|l| !l.starts_with("awaiting:"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// AC1/AC3/AC4: a non-zero `total()` is always attributable to something
/// agent mode printed. Renders each contributor for real and requires the
/// output BELOW the headline to change.
#[test]
fn agent_view_renders_every_contribution() {
    let baseline = agent_view_body(&AwaitingReport::default());
    for (key, populate) in contribution_fixtures() {
        let mut report = AwaitingReport::default();
        populate(&mut report);
        assert!(
            report.total() > 0,
            "fixture for `{key}` must make the report non-empty"
        );
        assert_ne!(
            agent_view_body(&report),
            baseline,
            "agent mode renders nothing for `{key}` beyond bumping the \
                 `awaiting:` headline, but `total()` counts it — the headline \
                 would exceed the visible rows with no way to learn why"
        );
    }
}

/// AC3: absence is SHOWN, not inferred — every list-shaped contributor
/// prints its `[0]` table on a quiet report, exactly like `prs[0]` did
/// before this bug. An agent must be able to learn the whole contributor
/// vocabulary from one invocation.
#[test]
fn agent_view_prints_every_key_even_when_quiet() {
    let rendered = AwaitingReport::default().render_agent_view();
    for (key, _) in AwaitingReport::default().contributions() {
        // The incomplete-scan row is the `+` suffix on `unshipped:`, and
        // `reviewer_actionable` is a scalar summary; both are asserted by
        // their own tests rather than by key presence.
        if key == "unshipped_scan_incomplete" {
            continue;
        }
        assert!(
            rendered.contains(key),
            "quiet agent view omits `{key}`:\n{rendered}"
        );
    }
}

/// AC2: the orphan table carries the spec id, the reason the human line
/// gives, and the `aida ps` next step — not merely a count.
#[test]
fn agent_view_orphan_table_carries_spec_reason_and_next_step() {
    let mut report = AwaitingReport::default();
    report.orphaned_in_progress.push(OrphanedInProgressItem {
        spec_id: "BUG-1688".into(),
        title: "stranded".into(),
        abandoned: true,
        since_label: "64h 15m ago".into(),
        possibly_subagent: false,
    });
    let rendered = report.render_agent_view();
    assert!(rendered.contains("orphaned_in_progress[1]"), "{rendered}");
    assert!(rendered.contains("BUG-1688"), "{rendered}");
    assert!(rendered.contains("abandoned — lease died"), "{rendered}");
    assert!(rendered.contains("64h 15m ago"), "{rendered}");
    assert!(rendered.contains("aida ps"), "{rendered}");
    // The headline must now reconcile with the rendered row.
    assert!(rendered.contains("awaiting: 1"), "{rendered}");
}

/// The observed incident, reproduced exactly: findings + nightly red +
/// an incomplete scan + three orphans rendered `awaiting: 6` above eight
/// empty tables. Every one of the six must now be visible.
#[test]
fn agent_view_reproduces_and_fixes_the_observed_awaiting_six() {
    let mut report = AwaitingReport {
        findings_total: 2,
        nightly_red: Some(NightlyRedItem {
            summary: "cross-platform nightly red since 2026-09-23".into(),
            run_id: Some(36_714_166_342),
            nights: 8,
        }),
        unshipped_work_scan: Some(UnshippedScanStatus {
            complete: false,
            scanned: 0,
            candidates: 14,
        }),
        ..Default::default()
    };
    for (spec, since) in [
        ("BUG-1688", "64h 15m ago"),
        ("BUG-1693", "64h 0m ago"),
        ("BUG-1695", "45h 45m ago"),
    ] {
        report.orphaned_in_progress.push(OrphanedInProgressItem {
            spec_id: spec.into(),
            title: "stranded".into(),
            abandoned: true,
            since_label: since.into(),
            possibly_subagent: false,
        });
    }
    assert_eq!(report.total(), 6, "the observed headline");
    let rendered = report.render_agent_view();
    assert!(rendered.contains("awaiting: 6"), "{rendered}");
    assert!(rendered.contains("unshipped: 0+"), "{rendered}");
    assert!(rendered.contains("orphaned_in_progress[3]"), "{rendered}");
    for spec in ["BUG-1688", "BUG-1693", "BUG-1695"] {
        assert!(
            rendered.contains(spec),
            "{spec} was counted in `awaiting: 6` and printed nowhere:\n{rendered}"
        );
    }
}

/// The reviewer contributor is an OFF-BY-ONE, not a missing table: the
/// `reviewer[N]` table always rendered, while `total()` counted N+1.
#[test]
fn agent_view_reviewer_summary_accounts_for_the_extra_counted_row() {
    let mut report = AwaitingReport::default();
    report.reviewer_queue_items.push(reviewer_item(
        "BUG-6",
        crate::gate::review_verdict::ReviewActionability::NeedsReview,
    ));
    assert_eq!(report.total(), 2, "one routed item plus the summary row");
    let rendered = report.render_agent_view();
    assert!(rendered.contains("reviewer[1]"), "{rendered}");
    assert!(
        rendered.contains("reviewer_actionable: 1 of 1 routed"),
        "the second counted row must be visible too:\n{rendered}"
    );
}

/// AC6's other half: the contributions refactor must not disturb the
/// hide-when-empty fast path `total()` drives.
#[test]
fn empty_report_still_totals_zero_and_hides() {
    let report = AwaitingReport::default();
    assert!(report.contributions().iter().all(|(_, n)| *n == 0));
    assert_eq!(report.total(), 0);
    assert!(report.is_empty());
}

/// `total()` is the sum of `contributions()` — pinned against a fully
/// populated report so the refactor cannot silently drop a term.
#[test]
fn total_equals_the_sum_of_every_populated_contribution() {
    let mut report = AwaitingReport::default();
    for (_, populate) in contribution_fixtures() {
        populate(&mut report);
    }
    let contributions = report.contributions();
    let summed: usize = contributions.iter().map(|(_, n)| n).sum();
    assert_eq!(report.total(), summed);
    // Every contributor populated; `reviewer` carries two items (its own
    // fixture and `reviewer_actionable`'s), so the total exceeds the
    // contributor count by exactly one.
    assert_eq!(
        report.total(),
        contributions.len() + 1,
        "each fixture contributes 1 except `reviewer`, which holds 2 items"
    );
}

/// The wire labels the agent table prints are the ones `--json` emits —
/// two machine surfaces, one vocabulary.
#[test]
fn reason_wire_labels_match_the_json_surface() {
    assert_eq!(StaleApprovalReason::Stale.as_wire_str(), "stale");
    assert_eq!(
        StaleApprovalReason::Unverifiable.as_wire_str(),
        "unverifiable"
    );
    assert_eq!(BlockedReason::AtHead.as_wire_str(), "at_head");
    assert_eq!(BlockedReason::Unverifiable.as_wire_str(), "unverifiable");
}

fn reviewer_item(
    spec_id: &str,
    state: crate::gate::review_verdict::ReviewActionability,
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
            reviewer_item(
                "BUG-1",
                crate::gate::review_verdict::ReviewActionability::NeedsReview,
            ),
            reviewer_item(
                "BUG-2",
                crate::gate::review_verdict::ReviewActionability::AwaitingRework,
            ),
            reviewer_item(
                "BUG-3",
                crate::gate::review_verdict::ReviewActionability::Resolved,
            ),
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
            crate::gate::review_verdict::ReviewActionability::AwaitingRework,
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
            crate::gate::review_verdict::ReviewActionability::NeedsReview,
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
        pushed: true,
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
