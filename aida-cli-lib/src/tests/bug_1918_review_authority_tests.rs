//! BUG-1918: approval authority is bound to session identity, never to the
//! free-text `recorded_by` label.
//!
//! Covers the advisor's acceptance points: an implementer cannot self-approve
//! even with forged labels or a reviewer seat; the author identity persists
//! across processes, working directories and lease release; the human-at-TTY
//! receipt path; ship-time grant re-validation; and the fasttrack opt-out.
// trace:BUG-1918 | ai:claude

use super::*;
use crate::pr_ship::{
    approval_authority_refusal, ApprovalAuthorityRefusal, AuthorSession, ShipApprovalPolicy,
};
use crate::review_verdict::{RecordedVerdict, RecorderAttestation, VerdictKind};

const SPEC: &str = "BUG-91801";
const HEAD: &str = "1aca4e3e9251aaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const AUTHOR_SESSION_ID: &str = "cb4777a7-0000-4000-8000-000000000001";
const OPERATOR: &str = "aida review record (operator)";

fn lease(id: &str, scope: &str, worktree: PathBuf) -> SessionLease {
    SessionLease {
        id: id.to_string(),
        scope: scope.to_string(),
        slug: scope.to_ascii_lowercase(),
        owner: "tester".into(),
        worktree_path: worktree,
        branch: format!("{}-work", scope.to_ascii_lowercase()),
        started_at: chrono::Utc::now(),
        hostname: "h".into(),
        role: Some("implementer".into()),
        creator_pid: Some(4242),
        creator_pid_start_time: Some("t0".into()),
        active_pid: None,
        active_pid_start_time: None,
        cargo_target_dir: None,
        parent_project_root: None,
        pr_head_sha: None,
        pr_base_sha: None,
        pr_base_ref: None,
        zen_intent_token: None,
        escalated_to_human: None,
        parent_branch: None,
        parent_branch_sha: None,
        review_verb: false,
        claim_verb: false,
        manual_enter_at: None,
        interrupted_at: None,
        interrupted_reason: None,
    }
}

fn write_lease(root: &Path, l: &SessionLease) {
    let path = crate::lease_path(root, &l.id);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, toml::to_string_pretty(l).unwrap()).unwrap();
}

fn approval(attestation: Option<RecorderAttestation>, at: &str) -> RecordedVerdict {
    RecordedVerdict {
        kind: VerdictKind::Approved,
        raw: "approved".into(),
        reviewed_sha: Some(HEAD.into()),
        recorded_at: Some(at.into()),
        recorded_by: Some(OPERATOR.into()),
        attestation,
        ..Default::default()
    }
}

fn subjects() -> Vec<String> {
    vec![SPEC.to_string()]
}

// ── Identity tokens ─────────────────────────────────────────────────────────

#[test]
fn tokens_match_on_identity_and_respect_process_start_identity() {
    assert!(tokens_match("session:abc", "session:abc"));
    assert!(!tokens_match("session:abc", "session:abd"));
    assert!(!tokens_match("session:abc", "lease:abc"), "kinds differ");
    assert!(tokens_match("proc:7@t0", "proc:7@t0"));
    assert!(!tokens_match("proc:7@t0", "proc:7@t1"), "recycled pid");
    assert!(
        tokens_match("proc:7@", "proc:7@t1"),
        "legacy pid-only record"
    );
    assert!(!tokens_match("proc:7@t0", "proc:8@t0"));
    assert!(!tokens_match("session:", "session:"), "empty never matches");
    assert!(!tokens_match("garbage", "garbage"), "untyped never matches");
    assert!(identities_overlap(
        &["lease:a".into(), "session:x".into()],
        &["session:x".into()]
    ));
    assert!(!identities_overlap(&[], &["session:x".into()]));
}

#[test]
fn authoring_lease_excludes_review_and_pr_scoped_leases() {
    let tmp = tempfile::tempdir().unwrap();
    let ids = subjects();
    let author = lease("aaaaaaaaaaaa", SPEC, tmp.path().into());
    assert!(is_authoring_lease_for(&author, &ids));
    assert!(is_authoring_lease_for(
        &lease(
            "bbbbbbbbbbbb",
            &SPEC.to_ascii_lowercase(),
            tmp.path().into()
        ),
        &ids
    ));
    let mut review = author.clone();
    review.review_verb = true;
    assert!(!is_authoring_lease_for(&review, &ids));
    assert!(!is_authoring_lease_for(
        &lease("cccccccccccc", "PR-2477", tmp.path().into()),
        &["PR-2477".into()]
    ));
    assert!(!is_authoring_lease_for(
        &lease("dddddddddddd", "  ", tmp.path().into()),
        &ids
    ));
    assert!(!is_authoring_lease_for(
        &lease("eeeeeeeeeeee", "TASK-1", tmp.path().into()),
        &ids
    ));
}

#[test]
fn recorder_context_binds_leases_by_worktree_or_session_id_only() {
    let tmp = tempfile::tempdir().unwrap();
    let wt = tmp.path().join("wt");
    std::fs::create_dir_all(wt.join("sub")).unwrap();
    let l = lease("0123456789ab", SPEC, wt.clone());
    let leases = [l];
    let inside = RecorderIdentity::with_context(vec![], &leases, &[wt.join("sub")], None);
    assert!(inside.tokens.contains(&"lease:0123456789ab".to_string()));
    let carries =
        RecorderIdentity::with_context(vec![], &leases, &[tmp.path().into()], Some("0123456789ab"));
    assert!(carries.tokens.contains(&"lease:0123456789ab".to_string()));
    // A too-short id prefix binds nothing (no accidental prefix match).
    let short = RecorderIdentity::with_context(vec![], &leases, &[tmp.path().into()], Some("0123"));
    assert!(short.tokens.is_empty());
    let outside = RecorderIdentity::with_context(vec![], &leases, &[tmp.path().into()], None);
    assert!(outside.tokens.is_empty());
}

// ── Persistent identity: survives cwd change, new process, lease release ──

/// Advisor blockers 2 and 3: the implementer `cd`s to the main checkout in a
/// fresh shell and its lease has been released. The durable authorship
/// ledger still names its agent session, so the approval refuses.
#[test]
fn author_identity_survives_cwd_change_new_process_and_lease_release() {
    let root = tempfile::tempdir().unwrap();
    let wt = root.path().join("worktree");
    std::fs::create_dir_all(&wt).unwrap();
    let l = lease("aaaabbbbcccc", SPEC, wt);

    // Process 1: the implementer takes the lease under its agent session.
    {
        let _env =
            crate::test_env::EnvVarsGuard::apply(&[("AIDA_SESSION_ID", Some(AUTHOR_SESSION_ID))]);
        write_lease(root.path(), &l);
        record_authoring_lease(root.path(), &l);
    }
    let ledger = load_ledger(root.path());
    assert_eq!(ledger.len(), 1, "{ledger:?}");
    assert_eq!(ledger[0].scope, SPEC);
    assert!(ledger[0]
        .tokens
        .contains(&format!("session:{AUTHOR_SESSION_ID}")));
    // A review lease never writes a claim record.
    let mut review = lease("rrrrrrrrrrrr", SPEC, root.path().into());
    review.review_verb = true;
    record_authoring_lease(root.path(), &review);
    assert_eq!(load_ledger(root.path()).len(), 1);

    // The lease is released.
    std::fs::remove_file(crate::lease_path(root.path(), &l.id)).unwrap();
    assert!(visible_leases(root.path()).is_empty());

    // Process 2: same agent session, acting from the main checkout.
    let recorder = RecorderIdentity::with_context(
        vec![format!("session:{AUTHOR_SESSION_ID}")],
        &[],
        &[root.path().into()],
        Some(AUTHOR_SESSION_ID),
    );
    let authors = author_sessions_at(root.path(), &subjects(), None);
    assert_eq!(authors.len(), 1, "the claim record outlives the lease");
    let refusal = crate::review_author_refusal(&authors, &recorder, true).expect("must refuse");
    assert!(refusal.contains("cannot approve its own work"), "{refusal}");

    // An independent session from the same directory is not the author.
    let other = RecorderIdentity::with_context(
        vec!["session:independent-reviewer".into()],
        &[],
        &[root.path().into()],
        None,
    );
    assert_eq!(crate::review_author_refusal(&authors, &other, true), None);

    // The ledger is also consulted by branch, for a PR whose specs differ.
    assert_eq!(
        author_sessions_at(root.path(), &[], Some(&l.branch)).len(),
        1
    );
}

/// The live lease binds a recorder acting inside its worktree even with no
/// session id in the environment; the label it would write is irrelevant.
#[test]
fn recorder_inside_the_authoring_worktree_is_the_author() {
    let root = tempfile::tempdir().unwrap();
    let wt = root.path().join("worktree");
    std::fs::create_dir_all(&wt).unwrap();
    let l = lease("aaaabbbbdddd", SPEC, wt.canonicalize().unwrap());
    write_lease(root.path(), &l);
    let leases = visible_leases(root.path());
    let recorder =
        RecorderIdentity::with_context(vec![], &leases, &[l.worktree_path.clone()], None);
    let authors = author_sessions_at(root.path(), &subjects(), None);
    assert!(crate::review_author_refusal(&authors, &recorder, true).is_some());
    // Unresolvable subject: every authoring lease counts (fails closed).
    let authors = crate::review_recorder_authors(root.path(), &[], None);
    let refusal = crate::review_author_refusal(&authors, &recorder, false).expect("fails closed");
    assert!(refusal.contains("could not be resolved"), "{refusal}");
}

/// End-to-end through `aida review record`: the implementer session, holding
/// a valid REVIEWER seat grant and writing under the operator label, is
/// refused because its session id matches the claim record — and nothing is
/// written.
#[test]
fn review_record_refuses_the_author_session_even_with_a_reviewer_seat() {
    let root = tempfile::tempdir().unwrap();
    let l = lease("aaaabbbbeeee", SPEC, root.path().join("elsewhere"));
    {
        let _env =
            crate::test_env::EnvVarsGuard::apply(&[("AIDA_SESSION_ID", Some(AUTHOR_SESSION_ID))]);
        record_authoring_lease(root.path(), &l);
    }
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        root.path(),
        "reviewer",
        &[],
        &[("AIDA_SESSION_ID", Some(AUTHOR_SESSION_ID))],
    );
    let err = crate::handle_review_record_at(
        root.path().to_path_buf(),
        SPEC,
        "approved",
        Some(HEAD),
        Some("main"),
        Some("lgtm"),
        &[],
        &[],
        None,
    )
    .expect_err("the author session must not approve its own work");
    assert!(
        err.to_string().contains("cannot approve its own work"),
        "{err}"
    );
    assert!(
        crate::review_verdict::read_recorded_verdict(root.path(), SPEC).is_none(),
        "a refused approval writes no verdict"
    );
}

/// The full ship-gate path for the 2026-10-10 incident: the author session's
/// approval at head, under the operator label, with a VALID receipt — refused
/// as a self-approval by the policy `aida pr ship` builds.
#[test]
fn ship_gate_refuses_self_approval_at_head_end_to_end() {
    let root = tempfile::tempdir().unwrap();
    let l = lease("aaaabbbbffff", SPEC, root.path().join("wt"));
    {
        let _env =
            crate::test_env::EnvVarsGuard::apply(&[("AIDA_SESSION_ID", Some(AUTHOR_SESSION_ID))]);
        record_authoring_lease(root.path(), &l);
    }
    let receipts = tempfile::tempdir().unwrap();
    let receipt = write_human_receipt_in(receipts.path(), HEAD, SPEC).unwrap();
    let att = RecorderAttestation {
        sha: HEAD.into(),
        human_at_tty: true,
        receipt_id: Some(receipt),
        identity: vec![format!("session:{AUTHOR_SESSION_ID}")],
        author_check_passed: true,
        ..Default::default()
    };
    let v = [approval(Some(att.clone()), "2026-10-10T05:30:00Z")];
    let mut policy = ShipApprovalPolicy {
        author_sessions: author_sessions_at(root.path(), &subjects(), Some(&l.branch)),
        ..Default::default()
    };
    // Grant the receipt full credit so only identity can refuse.
    policy
        .verified_authority
        .insert(att.authority_key(v[0].recorded_at.as_deref()).unwrap());
    assert!(matches!(
        approval_authority_refusal(&v, &policy),
        Some(ApprovalAuthorityRefusal::AuthorSession { .. })
    ));
    // The same approval from a different session ships.
    let mut independent = att;
    independent.identity = vec!["session:independent-reviewer".into()];
    let v = [approval(Some(independent), "2026-10-10T05:30:00Z")];
    assert_eq!(approval_authority_refusal(&v, &policy), None);
}

// ── Human-at-TTY receipts ───────────────────────────────────────────────────

#[test]
fn human_receipt_vouches_only_for_its_own_commit() {
    let dir = tempfile::tempdir().unwrap();
    let id = write_human_receipt_in(dir.path(), HEAD, SPEC).unwrap();
    assert!(human_receipt_valid_in(dir.path(), &id, HEAD));
    assert!(
        human_receipt_valid_in(dir.path(), &id, &HEAD[..12]),
        "short sha"
    );
    assert!(!human_receipt_valid_in(dir.path(), &id, "deadbeef"));
    assert!(!human_receipt_valid_in(dir.path(), &id, ""));
    // A non-uuid id never resolves to a path (no traversal).
    assert!(!human_receipt_valid_in(dir.path(), "../x", HEAD));
    // An unknown id fails.
    let unknown = uuid::Uuid::new_v4().to_string();
    assert!(!human_receipt_valid_in(dir.path(), &unknown, HEAD));
    // A receipt copied under another id does not vouch for that id.
    std::fs::copy(
        dir.path().join(format!("{id}.json")),
        dir.path().join(format!("{unknown}.json")),
    )
    .unwrap();
    assert!(!human_receipt_valid_in(dir.path(), &unknown, HEAD));
}

/// Under `cfg(test)` there is no TTY: an operator-labelled approval proves no
/// human and carries no receipt, so the ship gate treats it as unattested.
#[test]
fn operator_label_without_a_receipt_is_unattested_at_ship() {
    let att = RecorderAttestation {
        sha: HEAD.into(),
        human_at_tty: true,
        receipt_id: None,
        identity: vec!["session:someone".into()],
        author_check_passed: true,
        ..Default::default()
    };
    assert_eq!(att.authority_key(Some("2026-10-10T05:30:00Z")), None);
    let tmp = tempfile::tempdir().unwrap();
    let v = [approval(Some(att), "2026-10-10T05:30:00Z")];
    assert!(verified_authority_keys(tmp.path(), &v).is_empty());
    assert!(matches!(
        approval_authority_refusal(&v, &ShipApprovalPolicy::default()),
        Some(ApprovalAuthorityRefusal::Unattested { .. })
    ));
}

// ── Grant re-validation at ship time ────────────────────────────────────────

#[test]
fn seat_grant_is_revalidated_against_the_grant_store() {
    let root = tempfile::tempdir().unwrap();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(root.path(), "reviewer", &[]);
    let grant = crate::seat_authority::current_grant(root.path()).expect("seated");
    let now = chrono::Utc::now();
    let in_window = (now + chrono::Duration::minutes(1)).to_rfc3339();
    let before_issue = (grant.issued_at - chrono::Duration::hours(1)).to_rfc3339();

    let at = |seat: &str, grant_id: &str, recorded_at: &str| {
        approval(
            Some(RecorderAttestation {
                sha: HEAD.into(),
                seat: Some(seat.into()),
                grant_id: Some(grant_id.into()),
                identity: vec!["session:reviewer".into()],
                author_check_passed: true,
                ..Default::default()
            }),
            recorded_at,
        )
    };
    let good = at("reviewer", &grant.id, &in_window);
    let keys = verified_authority_keys(root.path(), std::slice::from_ref(&good));
    assert_eq!(keys.len(), 1, "a live grant for that seat re-validates");
    let policy = ShipApprovalPolicy {
        verified_authority: keys,
        ..Default::default()
    };
    assert_eq!(approval_authority_refusal(&[good], &policy), None);

    let forged = [
        // A seat the grant does not carry.
        at("advisor", &grant.id, &in_window),
        // An implementer seat never authorizes an approval.
        at("implementer", &grant.id, &in_window),
        // Recorded before the grant existed.
        at("reviewer", &grant.id, &before_issue),
        // A grant id that is not in the store.
        at("reviewer", &uuid::Uuid::new_v4().to_string(), &in_window),
        // Not a grant handle at all.
        at("reviewer", "../../etc/passwd", &in_window),
    ];
    for v in forged {
        let keys = verified_authority_keys(root.path(), std::slice::from_ref(&v));
        assert!(keys.is_empty(), "{:?}", v.attestation);
        let policy = ShipApprovalPolicy {
            verified_authority: keys,
            ..Default::default()
        };
        assert!(matches!(
            approval_authority_refusal(&[v], &policy),
            Some(ApprovalAuthorityRefusal::Unattested { .. })
        ));
    }

    // Revocation before the verdict's timestamp invalidates it.
    assert!(crate::seat_authority::revoke_current().unwrap());
    let after_revoke = (chrono::Utc::now() + chrono::Duration::minutes(2)).to_rfc3339();
    let v = at("reviewer", &grant.id, &after_revoke);
    assert!(verified_authority_keys(root.path(), &[v]).is_empty());
}

// ── Fasttrack opt-out ───────────────────────────────────────────────────────

#[test]
fn only_an_explicit_no_review_tag_on_every_spec_opts_out() {
    let req = |id: &str, tags: &[&str]| {
        let mut r = aida_core::Requirement::new("t".into(), "d".into());
        r.spec_id = Some(id.into());
        r.tags = tags.iter().map(|t| t.to_string()).collect();
        r
    };
    let reqs = vec![
        req("TASK-1", &["lifecycle:no-review"]),
        req("TASK-2", &["review-gate"]),
        req("TASK-3", &["lifecycle:no-review"]),
    ];
    let ids = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert!(crate::pr_cmd::specs_opt_out_of_review(
        &reqs,
        &ids(&["TASK-1"])
    ));
    assert!(crate::pr_cmd::specs_opt_out_of_review(
        &reqs,
        &ids(&["TASK-1", "TASK-3"])
    ));
    assert!(!crate::pr_cmd::specs_opt_out_of_review(
        &reqs,
        &ids(&["TASK-1", "TASK-2"])
    ));
    assert!(!crate::pr_cmd::specs_opt_out_of_review(
        &reqs,
        &ids(&["TASK-2"])
    ));
    // Unknown spec, or no spec at all, keeps review required.
    assert!(!crate::pr_cmd::specs_opt_out_of_review(
        &reqs,
        &ids(&["TASK-9"])
    ));
    assert!(!crate::pr_cmd::specs_opt_out_of_review(&reqs, &[]));
}

#[test]
fn ship_policy_without_a_store_requires_review() {
    let tmp = tempfile::tempdir().unwrap();
    let policy = crate::pr_cmd::ship_approval_policy(tmp.path(), &subjects(), None, &[]);
    assert!(!policy.review_opt_out);
    assert_eq!(
        approval_authority_refusal(&[], &policy),
        Some(ApprovalAuthorityRefusal::NoApproval)
    );
    let author = AuthorSession::default();
    assert!(!author.recorded(&RecorderAttestation::default()));
}
