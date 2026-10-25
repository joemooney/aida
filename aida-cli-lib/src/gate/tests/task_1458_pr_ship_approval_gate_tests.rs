use super::pr_ship_approval_gate;
use crate::gate::review_verdict::{RecordedVerdict, RecorderAttestation, VerdictKind};
use crate::gate::ship::ShipApprovalPolicy;

const HEAD: &str = "1aca4e3e9251aaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OLD: &str = "08834c6045a9bbbbbbbbbbbbbbbbbbbbbbbbbbbb";
/// When HEAD was pushed in the stale cases: AFTER the approval of OLD.
const HEAD_COMMITTED_AT: &str = "2026-09-24T00:00:00Z";

fn approval(sha: &str) -> RecordedVerdict {
    RecordedVerdict {
        kind: VerdictKind::Approved,
        raw: "approved".into(),
        reviewed_sha: Some(sha.into()),
        recorded_at: Some("2026-09-23T00:00:00Z".into()),
        // BUG-1918: an independent human-at-TTY recorder, so these tests
        // exercise the head-coverage gate behind the authority gate.
        // trace:BUG-1918 | ai:claude
        attestation: Some(RecorderAttestation {
            sha: sha.into(),
            human_at_tty: true,
            receipt_id: Some("rcpt".into()),
            identity: vec!["session:independent-reviewer".into()],
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// BUG-1918: a policy under which `verdicts`' attestations re-validated.
/// Realistic dates: the REVIEWED commit was committed (09-22) before the
/// approval (09-23); the head of a stale case was pushed after it
/// (09-24, see [`HEAD_COMMITTED_AT`]) — which is exactly why the
/// backdating check compares against the reviewed commit, not the head.
// trace:BUG-1918 | ai:claude
fn policy(verdicts: &[RecordedVerdict]) -> ShipApprovalPolicy {
    let mut p = ShipApprovalPolicy {
        reviewed_committed_at: Some("2026-09-22T00:00:00Z".parse().unwrap()),
        // An independent operator runs the merge.
        shipper: vec!["session:independent-shipper".into()],
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

fn activity(root: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(root.join(".aida").join("advisor-activity.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn covering_approval_pins_the_merge_to_the_approved_head() {
    let tmp = tempfile::tempdir().unwrap();
    let pin = pr_ship_approval_gate(
        tmp.path(),
        5,
        &[approval(HEAD)],
        Some(HEAD),
        &policy(&[approval(HEAD)]),
        false,
        true,
        None,
    )
    .expect("an approval at the head merges");
    assert_eq!(pin.as_deref(), Some(HEAD));
    assert!(activity(tmp.path()).is_empty(), "nothing to audit");
}

/// BUG-1918: no approval always refuses — there is no review opt-out —
/// and `--override-stale-approval` never excuses it.
// trace:BUG-1918 | ai:claude
#[test]
fn no_approval_always_refuses() {
    let tmp = tempfile::tempdir().unwrap();
    let err = pr_ship_approval_gate(
        tmp.path(),
        5,
        &[],
        Some(HEAD),
        &policy(&[]),
        false,
        true,
        None,
    )
    .expect_err("no approval refuses");
    assert!(err.to_string().contains("no approval recorded"), "{err}");
    assert!(pr_ship_approval_gate(
        tmp.path(),
        5,
        &[],
        Some(HEAD),
        &policy(&[]),
        true,
        true,
        None
    )
    .is_err());
}

/// BUG-1918 (round-2 regression): an honest approval of an older commit,
/// recorded BEFORE the current head was pushed, passes the authority gate
/// and reaches the TASK-1458 head gate and its override — it is not
/// mistaken for a backdated approval.
// trace:BUG-1918 | ai:claude
#[test]
fn honest_stale_approval_older_than_the_head_reaches_the_override() {
    let approved_at: chrono::DateTime<chrono::Utc> =
        approval(OLD).recorded_at.unwrap().parse().unwrap();
    let head_at: chrono::DateTime<chrono::Utc> = HEAD_COMMITTED_AT.parse().unwrap();
    assert!(
        head_at > approved_at,
        "the head was pushed after the review"
    );
    let p = policy(&[approval(OLD)]);
    assert_eq!(
        crate::gate::ship::approval_authority_refusal(&[approval(OLD)], &p),
        None,
        "authority holds: the approval post-dates the commit it reviewed"
    );
    let tmp = tempfile::tempdir().unwrap();
    let err = pr_ship_approval_gate(
        tmp.path(),
        5,
        &[approval(OLD)],
        Some(HEAD),
        &p,
        false,
        true,
        None,
    )
    .expect_err("stale without the override");
    assert!(
        err.to_string().contains("--override-stale-approval"),
        "{err}"
    );
    assert!(
        pr_ship_approval_gate(
            tmp.path(),
            5,
            &[approval(OLD)],
            Some(HEAD),
            &p,
            true,
            true,
            None
        )
        .is_ok(),
        "the override applies"
    );
}

/// BUG-1918 (round 3): the author cannot ship its own unreviewed commits
/// on top of an older independent approval via `--override-stale-approval`,
/// and the refusal does not advertise the override to it. An unanchored
/// shipper is refused the override too.
// trace:BUG-1918 | ai:claude
#[test]
fn stale_approval_override_is_refused_to_the_author() {
    let tmp = tempfile::tempdir().unwrap();
    let mut by_author = policy(&[approval(OLD)]);
    by_author.author_sessions = vec![crate::gate::ship::AuthorSession {
        label: "lease aaaabbbbcccc".into(),
        tokens: vec!["session:the-author".into()],
    }];
    by_author.shipper = vec!["session:the-author".into(), "proc:42@t".into()];
    assert_eq!(
        crate::gate::ship::approval_authority_refusal(&[approval(OLD)], &by_author),
        None,
        "the older approval itself is independent"
    );
    for override_stale in [false, true] {
        let err = pr_ship_approval_gate(
            tmp.path(),
            5,
            &[approval(OLD)],
            Some(HEAD),
            &by_author,
            override_stale,
            true,
            None,
        )
        .expect_err("the author never ships unreviewed commits");
        let msg = err.to_string();
        assert!(
            !msg.contains("re-run with `--override-stale-approval`"),
            "the override is not advertised to the author: {msg}"
        );
        if override_stale {
            assert!(msg.contains("authored the work"), "{msg}");
        }
    }
    let mut unanchored = policy(&[approval(OLD)]);
    unanchored.shipper = vec!["lease:aaaabbbbcccc".into()];
    let err = pr_ship_approval_gate(
        tmp.path(),
        5,
        &[approval(OLD)],
        Some(HEAD),
        &unanchored,
        true,
        true,
        None,
    )
    .expect_err("an unanchored shipper cannot override");
    assert!(err.to_string().contains("no identity"), "{err}");
    // Nothing was overridden: no override record.
    assert!(activity(tmp.path())
        .iter()
        .all(|e| e["step"] != "pr-merge-override-stale-approval"));
}

#[test]
fn stale_approval_refuses_and_logs_a_failed_merge_step() {
    let tmp = tempfile::tempdir().unwrap();
    let err = pr_ship_approval_gate(
        tmp.path(),
        5,
        &[approval(OLD)],
        Some(HEAD),
        &policy(&[approval(OLD)]),
        false,
        true,
        None,
    )
    .expect_err("a stale approval refuses");
    assert!(
        err.to_string().contains("--override-stale-approval"),
        "{err}"
    );
    let log = activity(tmp.path());
    assert_eq!(log.len(), 1);
    assert_eq!(log[0]["step"], "pr-merge");
    assert_eq!(log[0]["status"], "failed");
}

// STORY-1436: the held gate leaves a counted record; an override does not.
// trace:STORY-1436 | ai:claude
#[test]
fn stale_approval_refusal_records_a_gate_held_event() {
    let tmp = tempfile::tempdir().unwrap();
    let _on = crate::test_env::EnvVarGuard::unset(crate::events::EVENTS_DISABLE_ENV);
    let _ = pr_ship_approval_gate(
        tmp.path(),
        5,
        &[approval(OLD)],
        Some(HEAD),
        &policy(&[approval(OLD)]),
        false,
        true,
        None,
    );
    let evs = crate::events::read_all(tmp.path());
    assert_eq!(evs.len(), 1, "{evs:?}");
    assert!(matches!(
        &evs[0].kind,
        crate::events::EventKind::GateHeld { gate, pr: Some(5), .. }
            if gate == crate::events::GATE_STALE_APPROVAL
    ));
    let other = tempfile::tempdir().unwrap();
    let _ = pr_ship_approval_gate(
        other.path(),
        5,
        &[approval(OLD)],
        Some(HEAD),
        &policy(&[approval(OLD)]),
        true,
        true,
        None,
    );
    assert!(crate::events::read_all(other.path()).is_empty());
}

#[test]
fn override_is_recorded_durably_with_both_shas_and_pins_the_overridden_head() {
    let tmp = tempfile::tempdir().unwrap();
    let pin = pr_ship_approval_gate(
        tmp.path(),
        5,
        &[approval(OLD)],
        Some(HEAD),
        &policy(&[approval(OLD)]),
        true,
        true,
        None,
    )
    .expect("the override ships");
    assert_eq!(
        pin.as_deref(),
        Some(HEAD),
        "the override is granted for THIS head only"
    );
    let log = activity(tmp.path());
    assert_eq!(log.len(), 1, "{log:?}");
    assert_eq!(log[0]["step"], "pr-merge-override-stale-approval");
    assert_eq!(log[0]["pr"], 5);
    assert_eq!(log[0]["reviewed_sha"], OLD);
    assert_eq!(log[0]["head_sha"], HEAD);
}

#[test]
fn pr_ship_handler_pins_its_merge_through_the_approval_gate() {
    let src = include_str!("../../pr_cmd.rs");
    let start = src
        .find(concat!("pub(crate) fn pr_ship_", "handler("))
        .expect("handler");
    let body = &src[start..];
    let gate = body
        .find(concat!("let match_head = pr_ship_", "approval_gate("))
        .expect("the handler runs the approval gate");
    let opts = body
        .find(concat!("match_head, // trace:", "TASK-1458"))
        .expect("the gate's pin reaches MergeOptions");
    let merge = body
        .find(concat!(".merge_", "change("))
        .expect("the handler merges");
    assert!(gate < opts && opts < merge, "gate → pin → merge order");
}
