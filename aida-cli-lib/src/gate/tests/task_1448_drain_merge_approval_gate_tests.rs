// trace:TASK-1448 | ai:claude
use super::*;

const HEAD: &str = "1aca4e3e9251aaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OLD: &str = "08834c6045a9bbbbbbbbbbbbbbbbbbbbbbbbbbbb";

/// An attested, independent human-at-TTY approval (BUG-1918), so these
/// tests reach the head-coverage gate behind the authority gate.
fn approval(sha: Option<&str>) -> crate::gate::review_verdict::RecordedVerdict {
    crate::gate::review_verdict::RecordedVerdict {
        kind: crate::gate::review_verdict::VerdictKind::Approved,
        raw: "approved".into(),
        reviewed_sha: sha.map(str::to_string),
        recorded_at: Some("2026-09-23T00:00:00Z".into()),
        attestation: Some(crate::gate::review_verdict::RecorderAttestation {
            sha: sha.unwrap_or(HEAD).into(),
            human_at_tty: true,
            receipt_id: Some("rcpt".into()),
            identity: vec!["session:independent-reviewer".into()],
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn policy(
    verdicts: &[crate::gate::review_verdict::RecordedVerdict],
) -> crate::gate::ship::ShipApprovalPolicy {
    let mut p = crate::gate::ship::ShipApprovalPolicy {
        reviewed_committed_at: Some("2026-09-22T00:00:00Z".parse().unwrap()),
        ..Default::default()
    };
    for v in verdicts {
        if let Some(a) = v.attestation.as_ref() {
            if let Some(k) = a.authority_key(v.recorded_at.as_deref()) {
                p.verified_authority.insert(k, a.identity.clone());
            }
        }
    }
    p
}

fn gate(
    v: &[crate::gate::review_verdict::RecordedVerdict],
    head: Option<&str>,
) -> Result<(), auto_complete::PhaseFailure> {
    drain_merge_approval_gate(v, &policy(v), head, 5)
}

fn assert_shelves(result: Result<(), auto_complete::PhaseFailure>) -> String {
    let failure = result.expect_err("the merge phase must refuse");
    assert_eq!(failure.kind, auto_complete::FailureKind::StaleApproval);
    // Shelve-and-continue, never stop the drain; never a transient retry.
    assert!(failure.kind.is_shelvable());
    assert!(!auto_complete::should_retry_transient_failure(
        failure.kind,
        0,
        3
    ));
    failure.reason
}

fn assert_shelves_stale(result: Result<(), auto_complete::PhaseFailure>) -> String {
    let reason = assert_shelves(result);
    assert!(reason.contains("Re-review the current head"), "{reason}");
    reason
}

#[test]
fn approval_at_head_lets_the_drain_merge() {
    assert!(gate(&[approval(Some(HEAD))], Some(HEAD)).is_ok());
}

#[test]
fn approval_behind_head_shelves() {
    let reason = assert_shelves_stale(gate(&[approval(Some(OLD))], Some(HEAD)));
    assert!(
        reason.contains(&OLD[..12]) && reason.contains(&HEAD[..12]),
        "{reason}"
    );
}

#[test]
fn approval_without_sha_shelves() {
    // No reviewed sha: the attestation cannot authorize it either.
    assert_shelves(gate(&[approval(None)], Some(HEAD)));
}

#[test]
fn unreadable_head_shelves() {
    assert_shelves_stale(gate(&[approval(Some(HEAD))], None));
}

/// BUG-1918: the drain routes through the same authority gate as `aida pr
/// ship` — no approval, the drain's own unattested provenance stamp, or
/// an author session's approval all shelve.
// trace:BUG-1918 | ai:claude
#[test]
fn drain_merge_refuses_without_an_attested_independent_approval() {
    let reason = assert_shelves(gate(&[], Some(HEAD)));
    assert!(reason.contains("no approval recorded"), "{reason}");
    let stamped = crate::gate::review_verdict::RecordedVerdict {
        kind: crate::gate::review_verdict::VerdictKind::Approved,
        raw: "approved".into(),
        reviewed_sha: Some(HEAD.into()),
        recorded_by: Some("aida drain reviewer".into()),
        ..Default::default()
    };
    let reason = assert_shelves(gate(&[stamped], Some(HEAD)));
    assert!(reason.contains("no verified reviewer identity"), "{reason}");
    let v = [approval(Some(HEAD))];
    let mut by_author = policy(&v);
    by_author.author_sessions = vec![crate::gate::ship::AuthorSession {
        label: "lease l on TASK-1".into(),
        tokens: vec!["session:independent-reviewer".into()],
    }];
    let reason = assert_shelves(drain_merge_approval_gate(&v, &by_author, Some(HEAD), 5));
    assert!(reason.contains("cannot approve its own work"), "{reason}");
}

#[test]
fn stale_approval_recovery_hint_names_the_re_review() {
    let ctx = auto_complete::HintContext {
        spec: "TASK-1448".into(),
        pr_number: Some(5),
        ..Default::default()
    };
    let hint = auto_complete::recovery_hint(
        auto_complete::Phase::Merge,
        auto_complete::FailureKind::StaleApproval,
        &ctx,
    );
    assert!(hint.contains("Re-review the"), "{hint}");
    assert!(hint.contains("PR-5"), "{hint}");
    assert_eq!(
        auto_complete::FailureKind::StaleApproval.cause_slug(),
        "stale-approval"
    );
}
