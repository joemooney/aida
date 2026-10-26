// trace:BUG-1918 | ai:claude
use super::*;
use crate::gate::review_verdict::{RecordedVerdict, RecorderAttestation, VerdictKind};

const HEAD: &str = "1aca4e3e9251aaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OLD: &str = "08834c6045a9bbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const OPERATOR: &str = "aida review record (operator)";
const AT: &str = "2026-10-10T05:30:00Z";
const AUTHOR_SESSION: &str = "session:cb4777a7-author";
const AUTHOR_PROC: &str = "proc:4242@t0";

fn approval(sha: &str, by: &str, attestation: Option<RecorderAttestation>) -> RecordedVerdict {
    RecordedVerdict {
        kind: VerdictKind::Approved,
        raw: "approved".into(),
        reviewed_sha: Some(sha.into()),
        recorded_at: Some(AT.into()),
        recorded_by: Some(by.into()),
        attestation,
        ..Default::default()
    }
}

fn human(sha: &str, identity: &[&str]) -> RecorderAttestation {
    RecorderAttestation {
        sha: sha.into(),
        human_at_tty: true,
        receipt_id: Some("rcpt-1".into()),
        identity: identity.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}

/// Committer date of the reviewed commit: before every approval's `AT`.
fn reviewed_at() -> Option<chrono::DateTime<chrono::Utc>> {
    Some("2026-10-10T05:00:00Z".parse().unwrap())
}

/// `policy` with every attestation in `verdicts` marked as re-validated
/// (its durable store vouching for the attestation's own identity) and a
/// readable reviewed-commit date — so a test isolates the identity/authorization
/// logic from the stores.
fn verified(mut policy: ShipApprovalPolicy, verdicts: &[RecordedVerdict]) -> ShipApprovalPolicy {
    for v in verdicts {
        if let Some(a) = v.attestation.as_ref() {
            if let Some(k) = a.authority_key(v.recorded_at.as_deref()) {
                policy.verified_authority.insert(k, a.identity.clone());
            }
        }
    }
    policy.reviewed_committed_at = policy.reviewed_committed_at.or_else(reviewed_at);
    policy
}

fn author_policy() -> ShipApprovalPolicy {
    ShipApprovalPolicy {
        author_sessions: vec![AuthorSession {
            label: "lease lease-author on TASK-1538".into(),
            tokens: vec![
                "lease:lease-author".into(),
                AUTHOR_SESSION.into(),
                AUTHOR_PROC.into(),
            ],
        }],
        ..Default::default()
    }
}

/// The BUG-1918 regression: the implementing session records its own
/// approval at the PR head under the operator label, then ships.
#[test]
fn self_recorded_operator_approval_at_head_from_the_author_session_refuses() {
    // As the 2026-10-10 incident recorded it: no bound identity at all.
    let v = [approval(HEAD, OPERATOR, None)];
    assert_eq!(approval_head_refusal(&v, Some(HEAD)), None, "covers head");
    assert!(matches!(
        approval_authority_refusal(&v, &author_policy()),
        Some(ApprovalAuthorityRefusal::Unattested { recorded_by, .. }) if recorded_by == OPERATOR
    ));
    // Even carrying a verified human-at-TTY attestation, the author
    // session's own approval refuses: identity, not the label, decides.
    for token in [AUTHOR_SESSION, AUTHOR_PROC, "lease:lease-author"] {
        let v = [approval(HEAD, OPERATOR, Some(human(HEAD, &[token])))];
        let refusal =
            approval_authority_refusal(&v, &verified(author_policy(), &v)).expect("must refuse");
        assert_eq!(
            refusal,
            ApprovalAuthorityRefusal::AuthorSession {
                recorded_by: OPERATOR.into()
            },
            "{token}"
        );
        let msg = approval_authority_refusal_message(2477, &refusal);
        assert!(msg.contains("PR-2477"), "{msg}");
        assert!(msg.contains("cannot approve its own work"), "{msg}");
    }
}

/// A forged reviewer label does not launder the author's identity.
#[test]
fn forged_reviewer_label_from_the_author_session_still_refuses() {
    let seat = RecorderAttestation {
        sha: HEAD.into(),
        seat: Some("reviewer".into()),
        grant_id: Some("g".into()),
        identity: vec![AUTHOR_SESSION.into()],
        ..Default::default()
    };
    let v = [approval(
        HEAD,
        "review-pr-9 (claude reviewer seat)",
        Some(seat),
    )];
    assert!(matches!(
        approval_authority_refusal(&v, &verified(author_policy(), &v)),
        Some(ApprovalAuthorityRefusal::AuthorSession { .. })
    ));
}

/// BUG-1918 (review round 2): there is no opt-out — a missing approval
/// refuses whatever the specs' tags say.
#[test]
fn no_approval_always_refuses() {
    assert_eq!(
        approval_authority_refusal(&[], &ShipApprovalPolicy::default()),
        Some(ApprovalAuthorityRefusal::NoApproval)
    );
    let msg = approval_authority_refusal_message(9, &ApprovalAuthorityRefusal::NoApproval);
    assert!(!msg.contains("lifecycle:no-review"), "{msg}");
    // A refusal is not an approval: it does not count as one.
    let refusal = RecordedVerdict {
        kind: VerdictKind::RequestChanges,
        raw: "request-changes".into(),
        reviewed_sha: Some(HEAD.into()),
        ..Default::default()
    };
    assert_eq!(
        approval_authority_refusal(&[refusal], &ShipApprovalPolicy::default()),
        Some(ApprovalAuthorityRefusal::NoApproval)
    );
}

/// BUG-1918: an identity with no session/process anchor (empty, or only
/// a lease token) is refused even when its grant/receipt re-validated.
#[test]
fn identity_without_an_anchor_refuses_even_with_a_valid_receipt() {
    for identity in [&[][..], &["lease:some-lease"][..]] {
        let v = [approval(HEAD, OPERATOR, Some(human(HEAD, identity)))];
        assert!(
            matches!(
                approval_authority_refusal(&v, &verified(ShipApprovalPolicy::default(), &v)),
                Some(ApprovalAuthorityRefusal::Unattested { reason, .. }) if reason.contains("identity")
            ),
            "{identity:?}"
        );
    }
}

/// BUG-1918: the newest approval is chosen by parsed time, not by string
/// order — a `-07:00` stamp that sorts lower as text can be the later one.
#[test]
fn newest_approval_compares_parsed_timestamps() {
    let mut older = approval(HEAD, "older", None);
    older.recorded_at = Some("2026-10-10T05:00:00Z".into());
    let mut newer = approval(HEAD, "newer", None);
    newer.recorded_at = Some("2026-10-10T01:00:00-07:00".into());
    let v = [newer.clone(), older.clone()];
    assert_eq!(
        newest_approval(&v).and_then(|a| a.recorded_by.as_deref()),
        Some("newer")
    );
    let v = [older, newer];
    assert_eq!(
        newest_approval(&v).and_then(|a| a.recorded_by.as_deref()),
        Some("newer")
    );
}

#[test]
fn independent_human_or_reviewer_seat_approval_proceeds() {
    let v = [approval(
        HEAD,
        OPERATOR,
        Some(human(HEAD, &["session:other"])),
    )];
    assert_eq!(
        approval_authority_refusal(&v, &verified(author_policy(), &v)),
        None
    );
    let seat = RecorderAttestation {
        sha: HEAD.into(),
        seat: Some("reviewer".into()),
        grant_id: Some("g".into()),
        identity: vec!["proc:8@t9".into()],
        ..Default::default()
    };
    let v = [approval(
        HEAD,
        "review-pr-9 (claude reviewer seat)",
        Some(seat),
    )];
    assert_eq!(
        approval_authority_refusal(&v, &verified(author_policy(), &v)),
        None
    );
}

/// The stored booleans alone are not authority: an attestation whose
/// grant / receipt did not re-validate refuses.
#[test]
fn unverified_authority_refuses_whatever_the_stored_booleans_say() {
    let v = [approval(
        HEAD,
        OPERATOR,
        Some(human(HEAD, &["session:other"])),
    )];
    assert!(matches!(
        approval_authority_refusal(&v, &author_policy()),
        Some(ApprovalAuthorityRefusal::Unattested { .. })
    ));
    // A human-at-TTY claim with no receipt has no authority key at all.
    let mut no_receipt = human(HEAD, &["session:other"]);
    no_receipt.receipt_id = None;
    let v = [approval(HEAD, OPERATOR, Some(no_receipt))];
    assert!(matches!(
        approval_authority_refusal(&v, &verified(author_policy(), &v)),
        Some(ApprovalAuthorityRefusal::Unattested { .. })
    ));
}

#[test]
fn attestation_that_does_not_authorize_refuses() {
    let base = human(HEAD, &["session:other"]);
    let cases = [
        // An implementer seat is never authority, even without the label.
        RecorderAttestation {
            human_at_tty: false,
            seat: Some("implementer".into()),
            grant_id: Some("g".into()),
            ..base.clone()
        },
        // Neither a seat nor a human at a TTY.
        RecorderAttestation {
            human_at_tty: false,
            ..base.clone()
        },
        // No recorder identity at all: it can never be checked against
        // the authors, so it never authorizes.
        RecorderAttestation {
            identity: Vec::new(),
            ..base.clone()
        },
        // Attested for a different commit than the verdict now names.
        RecorderAttestation {
            sha: OLD.into(),
            ..base.clone()
        },
    ];
    for attestation in cases {
        let v = [approval(
            HEAD,
            "advisor (advisor seat)",
            Some(attestation.clone()),
        )];
        assert!(
            matches!(
                approval_authority_refusal(&v, &verified(ShipApprovalPolicy::default(), &v)),
                Some(ApprovalAuthorityRefusal::Unattested { .. })
            ),
            "{attestation:?}"
        );
    }
}

#[test]
fn reused_pid_of_a_different_process_is_not_the_author() {
    let v = [approval(
        HEAD,
        OPERATOR,
        Some(human(HEAD, &["proc:4242@t1"])),
    )];
    assert_eq!(
        approval_authority_refusal(&v, &verified(author_policy(), &v)),
        None
    );
}

/// The durable store's copy of the recorder identity decides: a human
/// receipt naming the author session refuses even when the verdict's
/// own copy was edited to look independent.
#[test]
fn receipt_identity_not_the_verdict_copy_decides_authorship() {
    let v = [approval(
        HEAD,
        OPERATOR,
        Some(human(HEAD, &["session:looks-independent"])),
    )];
    let mut policy = verified(author_policy(), &v);
    for identity in policy.verified_authority.values_mut() {
        *identity = vec![AUTHOR_PROC.into()];
    }
    assert!(matches!(
        approval_authority_refusal(&v, &policy),
        Some(ApprovalAuthorityRefusal::AuthorSession { .. })
    ));
    // An empty receipt identity is unattested, not independent.
    for identity in policy.verified_authority.values_mut() {
        identity.clear();
    }
    assert!(matches!(
        approval_authority_refusal(&v, &policy),
        Some(ApprovalAuthorityRefusal::Unattested { reason, .. }) if reason.contains("identity")
    ));
}

/// `recorded_at` cannot be backdated into an old grant window: an
/// approval dated before the commit it reviewed, in the future, or
/// against an unreadable reviewed commit refuses.
#[test]
fn approval_dated_outside_the_heads_lifetime_refuses() {
    let v = [approval(
        HEAD,
        OPERATOR,
        Some(human(HEAD, &["session:other"])),
    )];
    let ok = verified(author_policy(), &v);
    assert_eq!(approval_authority_refusal(&v, &ok), None);
    let after_review = ShipApprovalPolicy {
        reviewed_committed_at: Some("2026-10-10T06:00:00Z".parse().unwrap()),
        ..ok.clone()
    };
    assert!(matches!(
        approval_authority_refusal(&v, &after_review),
        Some(ApprovalAuthorityRefusal::Unattested { reason, .. }) if reason.contains("before the commit")
    ));
    let unreadable = ShipApprovalPolicy {
        reviewed_committed_at: None,
        ..ok.clone()
    };
    assert!(matches!(
        approval_authority_refusal(&v, &unreadable),
        Some(ApprovalAuthorityRefusal::Unattested { .. })
    ));
    let mut future = v.clone();
    future[0].recorded_at = Some("2999-01-01T00:00:00Z".into());
    let future_policy = verified(author_policy(), &future);
    assert!(matches!(
        approval_authority_refusal(&future, &future_policy),
        Some(ApprovalAuthorityRefusal::Unattested { reason, .. }) if reason.contains("future")
    ));
}

/// A shell token recorded at claim time names the same process as a
/// recorder's `proc` ancestor token.
#[test]
fn claiming_shell_still_alive_above_the_recorder_is_the_author() {
    let policy = ShipApprovalPolicy {
        author_sessions: vec![AuthorSession {
            label: "claim record".into(),
            tokens: vec!["shell:777@s0".into()],
        }],
        ..Default::default()
    };
    let v = [approval(
        HEAD,
        OPERATOR,
        Some(human(HEAD, &["proc:777@s0"])),
    )];
    assert!(matches!(
        approval_authority_refusal(&v, &verified(policy, &v)),
        Some(ApprovalAuthorityRefusal::AuthorSession { .. })
    ));
}
