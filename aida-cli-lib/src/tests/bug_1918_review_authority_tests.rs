//! BUG-1918: approval authority is bound to session identity, never to the
//! free-text `recorded_by` label.
//!
//! Covers the advisor's acceptance points: an implementer cannot self-approve
//! even with forged labels or a reviewer seat; the author identity persists
//! across processes, working directories and lease release; the human-at-TTY
//! receipt path; ship-time grant re-validation; that no label opts a PR out
//! of review; and that every merge path (ship, drain, release, burndown) goes
//! through the authority gate.
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
        driver: None,
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
    let author_identity = vec![format!("session:{AUTHOR_SESSION_ID}")];
    let receipt = write_human_receipt_in(receipts.path(), HEAD, SPEC, &author_identity).unwrap();
    let att = RecorderAttestation {
        sha: HEAD.into(),
        human_at_tty: true,
        receipt_id: Some(receipt),
        identity: vec![format!("session:{AUTHOR_SESSION_ID}")],
        ..Default::default()
    };
    let v = [approval(Some(att.clone()), "2026-10-10T05:30:00Z")];
    let key = att.authority_key(v[0].recorded_at.as_deref()).unwrap();
    let mut policy = ShipApprovalPolicy {
        author_sessions: author_sessions_at(root.path(), &subjects(), Some(&l.branch)),
        reviewed_committed_at: Some("2026-10-10T05:00:00Z".parse().unwrap()),
        ..Default::default()
    };
    // Grant the receipt full credit so only identity can refuse.
    policy
        .verified_authority
        .insert(key.clone(), author_identity.clone());
    assert!(matches!(
        approval_authority_refusal(&v, &policy),
        Some(ApprovalAuthorityRefusal::AuthorSession { .. })
    ));
    // Editing the verdict's identity does not help: the receipt's own copy
    // still names the author.
    let mut relabelled = att.clone();
    relabelled.identity = vec!["session:independent-reviewer".into()];
    let v = [approval(Some(relabelled.clone()), "2026-10-10T05:30:00Z")];
    assert!(matches!(
        approval_authority_refusal(&v, &policy),
        Some(ApprovalAuthorityRefusal::AuthorSession { .. })
    ));
    // A receipt genuinely written by a different session ships.
    policy
        .verified_authority
        .insert(key, vec!["session:independent-reviewer".into()]);
    assert_eq!(approval_authority_refusal(&v, &policy), None);
}

// ── Human-at-TTY receipts ───────────────────────────────────────────────────

#[test]
fn human_receipt_vouches_only_for_its_own_commit() {
    let dir = tempfile::tempdir().unwrap();
    let id = write_human_receipt_in(dir.path(), HEAD, SPEC, &["session:human".into()]).unwrap();
    assert!(human_receipt_valid_in(dir.path(), &id, HEAD));
    assert_eq!(
        human_receipt_in(dir.path(), &id, HEAD).unwrap().identity,
        vec!["session:human".to_string()],
        "the receipt carries the recorder identity"
    );
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
                ..Default::default()
            }),
            recorded_at,
        )
    };
    let good = at("reviewer", &grant.id, &in_window);
    let keys = verified_authority_keys(root.path(), std::slice::from_ref(&good));
    assert_eq!(keys.len(), 1, "a live grant for that seat re-validates");
    let head_at = Some(now - chrono::Duration::days(1));
    let policy = ShipApprovalPolicy {
        verified_authority: keys,
        reviewed_committed_at: head_at,
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
            reviewed_committed_at: head_at,
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

// ── No opt-out ──────────────────────────────────────────────────────────────

#[test]
fn ship_policy_without_a_store_requires_review() {
    let tmp = tempfile::tempdir().unwrap();
    let policy = crate::pr_cmd::ship_approval_policy(tmp.path(), &subjects(), None, &[]);
    assert_eq!(
        approval_authority_refusal(&[], &policy),
        Some(ApprovalAuthorityRefusal::NoApproval)
    );
    let author = AuthorSession::default();
    assert!(!author.matches(&[]));
    assert_eq!(policy.reviewed_committed_at, None);
}

// ── Vendor-neutral agent identity (injected ancestry) ───────────────────────

fn proc(pid: u32, name: &str, cmd: &[&str], start: &str) -> crate::review_authority::ProcEntry {
    crate::review_authority::ProcEntry {
        pid,
        name: name.into(),
        cmd: cmd.iter().map(|s| s.to_string()).collect(),
        start: Some(start.into()),
    }
}

/// `aida` (pid `aida`) run by shell `shell` under `harness`, then the chain
/// above it.
fn source_under(
    aida: u32,
    shell: u32,
    harness: crate::review_authority::ProcEntry,
    leader: Option<crate::review_authority::ProcEntry>,
) -> IdentitySource {
    IdentitySource {
        session_ids: vec![],
        chain: vec![
            proc(aida, "aida", &["aida"], "a"),
            proc(shell, "bash", &["/bin/bash", "-c", "aida …"], "s"),
            harness,
            proc(10, "systemd", &["/lib/systemd/systemd", "--user"], "u"),
        ],
        session_leader: leader,
    }
}

#[test]
fn agent_harness_detection_is_vendor_neutral_and_ignores_arguments() {
    let h = |name: &str, cmd: &[&str]| {
        is_agent_harness(name, &cmd.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    };
    assert!(h("claude", &["claude"]));
    assert!(h("codex", &["/usr/local/bin/codex", "exec"]));
    assert!(h("antigravity", &["/opt/Antigravity/antigravity"]));
    assert!(h(
        "node",
        &["node", "/usr/lib/node_modules/@google/gemini-cli/gemini.js"]
    ));
    assert!(h("node", &["/usr/bin/node", "/home/u/.npm/bin/gemini"]));
    assert!(h("MainThread", &["/home/u/.local/bin/claude", "--resume"]));
    assert!(
        !h("bash", &["/bin/bash", "-c", "claude"]),
        "an argument is not a harness"
    );
    assert!(!h("aida", &["aida", "dispatch", "--vendor", "claude"]));
    assert!(!h("node", &["node", "--inspect", "gemini"]));
    assert!(!h("node", &["node", "/srv/app/server.js"]));
    assert!(!h("claudette", &["claudette"]));
}

/// Advisor blockers 2/3 for a non-Claude vendor: an antigravity agent claims
/// in shell A, its lease is released, and it records an approval from the
/// main checkout in shell B. The harness process is the common identity, so
/// `aida review record` refuses — and nothing is written.
#[test]
fn harness_agent_claim_in_one_shell_then_approve_in_another_is_refused() {
    for harness in [
        proc(50, "antigravity", &["/opt/Antigravity/antigravity"], "h0"),
        proc(50, "codex", &["/usr/bin/codex"], "h0"),
        proc(50, "node", &["node", "/opt/gemini/gemini.js"], "h0"),
        proc(50, "claude", &["claude"], "h0"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let l = lease("aaaabbbb1111", SPEC, root.path().join("wt"));
        with_identity_source(source_under(100, 200, harness.clone(), None), || {
            record_authoring_lease(root.path(), &l)
        });
        let ledger = load_ledger(root.path());
        assert!(
            ledger[0].tokens.contains(&"proc:50@h0".to_string()),
            "{ledger:?}"
        );
        // The lease was released (never written to disk).
        assert!(visible_leases(root.path()).is_empty());
        let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
            root.path(),
            "reviewer",
            &[],
            &[("AIDA_SESSION_ID", None)],
        );
        let err = with_identity_source(source_under(101, 201, harness.clone(), None), || {
            crate::handle_review_record_at(
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
        })
        .expect_err("the claiming harness must not approve its own work");
        assert!(
            err.to_string().contains("cannot approve its own work"),
            "{}: {err}",
            harness.name
        );
        assert!(crate::review_verdict::read_recorded_verdict(root.path(), SPEC).is_none());

        // A different harness instance (another agent) is not the author.
        let authors = author_sessions_at(root.path(), &subjects(), None);
        let other = with_identity_source(
            source_under(
                102,
                202,
                proc(
                    51,
                    &harness.name,
                    &harness.cmd.iter().map(String::as_str).collect::<Vec<_>>(),
                    "h1",
                ),
                None,
            ),
            || RecorderIdentity::current(root.path(), &[]),
        );
        assert_eq!(crate::review_author_refusal(&authors, &other, true), None);
    }
}

/// An agent under no recognised harness is identified by its POSIX session
/// leader, shared by all of its shell calls.
#[test]
fn unrecognised_agent_is_bound_by_its_session_leader() {
    let root = tempfile::tempdir().unwrap();
    let l = lease("aaaabbbb2222", SPEC, root.path().join("wt"));
    let tool = proc(60, "my-agent", &["/usr/bin/my-agent"], "t0");
    let leader = Some(proc(60, "", &[], "t0"));
    with_identity_source(source_under(100, 200, tool.clone(), leader.clone()), || {
        record_authoring_lease(root.path(), &l)
    });
    assert!(load_ledger(root.path())[0]
        .tokens
        .contains(&"sid:60@t0".to_string()));
    let authors = author_sessions_at(root.path(), &subjects(), None);

    let same = with_identity_source(source_under(101, 201, tool.clone(), leader), || {
        RecorderIdentity::current(root.path(), &[])
    });
    assert!(crate::review_author_refusal(&authors, &same, true).is_some());

    let other_leader = Some(proc(61, "", &[], "t1"));
    let other = with_identity_source(
        source_under(
            102,
            202,
            proc(61, "my-agent", &["my-agent"], "t1"),
            other_leader,
        ),
        || RecorderIdentity::current(root.path(), &[]),
    );
    assert_eq!(crate::review_author_refusal(&authors, &other, true), None);
}

/// A harness's own session id (e.g. `CLAUDE_CODE_SESSION_ID`) binds claim
/// and record even across harness restarts.
#[test]
fn harness_session_id_binds_claim_and_record() {
    let root = tempfile::tempdir().unwrap();
    let l = lease("aaaabbbb3333", SPEC, root.path().join("wt"));
    let src = |pid| IdentitySource {
        session_ids: vec!["conv-1".into()],
        ..source_under(
            100 + pid,
            200 + pid,
            proc(pid, "claude", &["claude"], "x"),
            None,
        )
    };
    with_identity_source(src(70), || record_authoring_lease(root.path(), &l));
    let authors = author_sessions_at(root.path(), &subjects(), None);
    let later = with_identity_source(src(71), || RecorderIdentity::current(root.path(), &[]));
    assert!(crate::review_author_refusal(&authors, &later, true).is_some());
}

/// MAJOR 6: the ship policy, built from an on-disk ledger, refuses the
/// harness agent's own approval; and no approval at all always refuses.
#[test]
fn ship_policy_from_on_disk_ledger_refuses_the_harness_authors_approval() {
    let root = tempfile::tempdir().unwrap();
    let l = lease("aaaabbbb4444", SPEC, root.path().join("wt"));
    let harness = proc(80, "antigravity", &["antigravity"], "h");
    with_identity_source(source_under(100, 200, harness.clone(), None), || {
        record_authoring_lease(root.path(), &l)
    });
    let recorder = with_identity_source(source_under(101, 201, harness.clone(), None), || {
        RecorderIdentity::current(root.path(), &[])
    });
    let att = RecorderAttestation {
        sha: HEAD.into(),
        seat: Some("reviewer".into()),
        grant_id: Some(uuid::Uuid::new_v4().to_string()),
        identity: recorder.tokens.clone(),
        ..Default::default()
    };
    let v = [approval(Some(att), "2026-10-10T05:30:00Z")];
    let policy = with_identity_source(source_under(102, 202, harness, None), || {
        crate::pr_cmd::ship_approval_policy(root.path(), &subjects(), Some(&l.branch), &v)
    });
    assert_eq!(policy.author_sessions.len(), 1);
    assert!(matches!(
        approval_authority_refusal(&v, &policy),
        Some(ApprovalAuthorityRefusal::AuthorSession { .. })
    ));
    // With no approval, nothing ships — there is no opt-out.
    assert_eq!(
        approval_authority_refusal(&[], &policy),
        Some(ApprovalAuthorityRefusal::NoApproval)
    );
}

#[test]
fn head_commit_date_is_read_from_the_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(tmp.path())
            .args(args)
            .env("GIT_COMMITTER_DATE", "2026-10-10T05:00:00Z")
            .env("GIT_AUTHOR_DATE", "2026-10-10T05:00:00Z")
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    git(&["init", "-q"]);
    git(&[
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@t",
        "commit",
        "-q",
        "--allow-empty",
        "-m",
        "c",
    ]);
    let sha = git(&["rev-parse", "HEAD"]);
    assert_eq!(
        crate::pr_cmd::commit_committed_at(tmp.path(), &sha),
        Some("2026-10-10T05:00:00Z".parse().unwrap())
    );
    assert_eq!(crate::pr_cmd::commit_committed_at(tmp.path(), HEAD), None);
    assert_eq!(
        crate::pr_cmd::commit_committed_at(tmp.path(), "--all"),
        None
    );
    assert_eq!(crate::pr_cmd::commit_committed_at(tmp.path(), ""), None);
}

// ── PR-keyed approvals resolve their subject from the forge ─────────────────

#[test]
fn pr_keyed_review_resolves_head_branch_and_trailer_specs() {
    let subject = crate::ReviewPrSubject {
        head_ref: "bug-91801-work".into(),
        commit_subjects: vec!["[AI:claude] fix(x): thing (TASK-91802, BUG-91801)".into()],
    };
    let (ids, branch) = crate::review_subject("PR-2485", None, None, |n| {
        assert_eq!(n, 2485);
        Ok(subject.clone())
    })
    .unwrap();
    assert_eq!(branch.as_deref(), Some("bug-91801-work"));
    assert!(ids.contains(&"BUG-91801".to_string()), "{ids:?}");
    assert!(ids.contains(&"TASK-91802".to_string()), "{ids:?}");

    // `--pr` with a spec key resolves too, and keeps the key.
    let (ids, _) = crate::review_subject("TASK-7", Some(9), None, |_| Ok(subject.clone())).unwrap();
    assert!(ids.contains(&"TASK-7".to_string()));

    // An unresolvable PR, or one with no head branch, refuses.
    let err =
        crate::review_subject("PR-2485", None, None, |_| anyhow::bail!("offline")).unwrap_err();
    assert!(
        err.to_string().contains("could not resolve PR-2485"),
        "{err}"
    );
    let err = crate::review_subject("PR-2485", None, None, |_| {
        Ok(crate::ReviewPrSubject::default())
    })
    .unwrap_err();
    assert!(err.to_string().contains("no head branch"), "{err}");

    // A spec-keyed review with no PR never consults the forge.
    let (ids, branch) =
        crate::review_subject("BUG-91801", None, Some("b"), |_| unreachable!()).unwrap();
    assert_eq!(ids, subjects());
    assert_eq!(branch.as_deref(), Some("b"));
}

/// MAJOR 5: an approval keyed `PR-N` from the main checkout is checked
/// against the PR's authors, resolved through the forge.
#[test]
fn pr_keyed_approval_from_main_is_refused_for_the_author() {
    let root = tempfile::tempdir().unwrap();
    let l = lease("aaaabbbb5555", SPEC, root.path().join("wt"));
    let harness = proc(90, "codex", &["codex"], "h");
    with_identity_source(source_under(100, 200, harness.clone(), None), || {
        record_authoring_lease(root.path(), &l)
    });
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        root.path(),
        "reviewer",
        &[],
        &[("AIDA_SESSION_ID", None)],
    );
    let record = || {
        crate::handle_review_record_at(
            root.path().to_path_buf(),
            "PR-2485",
            "approved",
            Some(HEAD),
            None,
            Some("lgtm"),
            &[],
            &[],
            None,
        )
    };
    // No forge: refuses rather than approving an unresolved subject.
    let err = with_identity_source(source_under(101, 201, harness.clone(), None), record)
        .expect_err("unresolvable PR refuses");
    assert!(
        err.to_string().contains("could not resolve PR-2485"),
        "{err}"
    );
    // Forge resolves the PR to the author's branch: refused as the author.
    let subject = crate::ReviewPrSubject {
        head_ref: l.branch.clone(),
        commit_subjects: vec![],
    };
    let err = crate::with_review_pr_subject(subject, || {
        with_identity_source(source_under(101, 201, harness, None), record)
    })
    .expect_err("the author must not approve its PR");
    assert!(
        err.to_string().contains("cannot approve its own work"),
        "{err}"
    );
}

/// A ledger that cannot be written is reported (stderr warning and a gate
/// event) and never panics or aborts the claim.
#[test]
fn unwritable_ledger_is_reported_not_swallowed() {
    let root = tempfile::tempdir().unwrap();
    // `.aida/review-authorship.jsonl` is a directory: the append fails.
    std::fs::create_dir_all(root.path().join(".aida/review-authorship.jsonl")).unwrap();
    let l = lease("aaaabbbb6666", SPEC, root.path().join("wt"));
    record_authoring_lease(root.path(), &l);
    assert!(load_ledger(root.path()).is_empty());
}

// ── Round-2 advisor acceptance ──────────────────────────────────────────────

/// Shipping from the main checkout, with the PR's branch renamed away from
/// the authoring lease's branch: the forge's head ref and commit trailers
/// still resolve the PR's specs, so the author is found and its approval is
/// refused. With no resolvable spec at all, every author counts (fail closed).
#[test]
fn ship_from_main_with_a_renamed_branch_still_finds_the_author() {
    let root = tempfile::tempdir().unwrap();
    let l = lease("aaaabbbb8888", SPEC, root.path().join("wt"));
    assert_eq!(l.branch, "bug-91801-work");
    let harness = proc(85, "codex", &["codex"], "h");
    with_identity_source(source_under(100, 200, harness.clone(), None), || {
        record_authoring_lease(root.path(), &l)
    });
    let recorder = with_identity_source(source_under(101, 201, harness, None), || {
        RecorderIdentity::current(root.path(), &[])
    });
    let att = RecorderAttestation {
        sha: HEAD.into(),
        seat: Some("reviewer".into()),
        grant_id: Some(uuid::Uuid::new_v4().to_string()),
        identity: recorder.tokens.clone(),
        ..Default::default()
    };
    let v = [approval(Some(att), "2026-10-10T05:30:00Z")];

    // The forge knows the PR by its renamed head; its commits carry the spec.
    let subject = crate::ReviewPrSubject {
        head_ref: "renamed-branch".into(),
        commit_subjects: vec!["[AI:claude] fix(x): y (BUG-91801)".into()],
    };
    let (ids, branch, resolved) = crate::with_review_pr_subject(subject, || {
        crate::pr_cmd::pr_gate_subject(root.path(), 2485, &[], Some("renamed-local"))
    });
    assert!(resolved);
    assert_eq!(branch.as_deref(), Some("renamed-branch"));
    assert_eq!(ids, subjects());
    let policy = crate::pr_cmd::ship_approval_policy(root.path(), &ids, branch.as_deref(), &v);
    assert_eq!(policy.author_sessions.len(), 1, "{policy:?}");
    assert!(matches!(
        approval_authority_refusal(&v, &policy),
        Some(ApprovalAuthorityRefusal::AuthorSession { .. })
    ));

    // Forge offline: the local title/branch claim an unrelated spec. That
    // claim is not trusted for authorship — every recorded author counts.
    let claimed = vec!["TASK-99901".to_string()];
    let (ids, _, resolved) =
        crate::pr_cmd::pr_gate_subject(root.path(), 2485, &claimed, Some("task-99901-work"));
    assert!(!resolved);
    assert_eq!(ids, claimed);
    let (_, policy) = crate::pr_cmd::merge_approval_inputs(
        &[root.path()],
        root.path(),
        2485,
        &claimed,
        Some("task-99901-work"),
        Some(HEAD),
    );
    assert_eq!(policy.author_sessions.len(), 1, "fails closed: {policy:?}");
    assert!(matches!(
        approval_authority_refusal(&v, &policy),
        Some(ApprovalAuthorityRefusal::AuthorSession { .. })
    ));
}

/// A launcher claims under ITS `AIDA_SESSION_ID`; the worker it starts runs
/// under a different one (the lease id) and a different process tree. The
/// ledger records the worker (its session id and stamped process), so the
/// worker's own approval is refused after the lease is gone.
#[test]
fn worker_with_a_different_session_id_than_the_launcher_is_the_author() {
    let root = tempfile::tempdir().unwrap();
    let l = lease("aaaabbbb9999", SPEC, root.path().join("wt"));
    write_lease(root.path(), &l);
    {
        let _env = crate::test_env::EnvVarsGuard::apply(&[(
            "AIDA_SESSION_ID",
            Some("launcher-session-0001"),
        )]);
        with_identity_source(
            source_under(100, 200, proc(90, "claude", &["claude"], "L"), None),
            || record_authoring_lease(root.path(), &l),
        );
    }
    // The launcher stamps the worker's process onto the lease.
    let worker = proc(95, "codex", &["codex"], "W");
    crate::review_authority::record_lease_identity(
        root.path(),
        &l.id,
        &[crate::review_authority::process_token(
            "proc",
            worker.pid,
            worker.start.as_deref(),
        )],
    );
    let ledger = load_ledger(root.path());
    assert_eq!(ledger.len(), 2, "{ledger:?}");
    assert!(ledger
        .iter()
        .all(|r| r.tokens.contains(&format!("session:{}", l.id))));
    std::fs::remove_file(crate::lease_path(root.path(), &l.id)).unwrap();
    let authors = author_sessions_at(root.path(), &subjects(), None);

    // The worker, carrying the lease id as its session id.
    let with_id = {
        let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_SESSION_ID", Some(&l.id))]);
        with_identity_source(source_under(101, 201, worker.clone(), None), || {
            RecorderIdentity::current(root.path(), &[])
        })
    };
    assert!(crate::review_author_refusal(&authors, &with_id, true).is_some());
    // The worker with no session id at all: its stamped process binds it.
    let bare = {
        let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_SESSION_ID", None)]);
        with_identity_source(source_under(102, 202, worker, None), || {
            RecorderIdentity::current(root.path(), &[])
        })
    };
    assert!(crate::review_author_refusal(&authors, &bare, true).is_some());
    // An unrelated reviewer is not the author.
    let other = {
        let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_SESSION_ID", None)]);
        with_identity_source(
            source_under(103, 203, proc(97, "codex", &["codex"], "R"), None),
            || RecorderIdentity::current(root.path(), &[]),
        )
    };
    assert_eq!(crate::review_author_refusal(&authors, &other, true), None);
}

/// `aida agent new` / `aida session start --launch`: the launcher creates a
/// lease for a worker it then hands off to. Only the worker's identity is
/// recorded as the author, so the launcher (e.g. a coordinator or advisor)
/// can review and approve the worker's PR, while the worker cannot approve
/// its own.
// trace:BUG-1918 | ai:claude
#[test]
fn launcher_may_approve_the_worker_it_launched_but_the_worker_may_not() {
    let root = tempfile::tempdir().unwrap();
    let l = lease("aaaabbbb7777", SPEC, root.path().join("wt"));
    write_lease(root.path(), &l);
    let launcher_harness = proc(90, "claude", &["claude"], "L");
    {
        let _env =
            crate::test_env::EnvVarsGuard::apply(&[("AIDA_SESSION_ID", Some("advisor-session"))]);
        with_identity_source(
            source_under(100, 200, launcher_harness.clone(), None),
            || crate::review_authority::as_launcher(|| record_authoring_lease(root.path(), &l)),
        );
    }
    // The launcher stamps the worker's process onto the lease.
    let worker = proc(95, "codex", &["codex"], "W");
    crate::review_authority::record_lease_identity(
        root.path(),
        &l.id,
        &[crate::review_authority::process_token(
            "proc",
            worker.pid,
            worker.start.as_deref(),
        )],
    );
    let ledger = load_ledger(root.path());
    assert!(!ledger.is_empty());
    for r in &ledger {
        assert!(
            !r.tokens
                .iter()
                .any(|t| t.starts_with("shell:") || t == "session:advisor-session"),
            "the launcher's own identity is not recorded as the author: {r:?}"
        );
    }
    let authors = author_sessions_at(root.path(), &subjects(), None);
    assert!(!authors.is_empty());

    // The launcher, back in its own shell later: allowed.
    let launcher = {
        let _env =
            crate::test_env::EnvVarsGuard::apply(&[("AIDA_SESSION_ID", Some("advisor-session"))]);
        with_identity_source(source_under(101, 201, launcher_harness, None), || {
            RecorderIdentity::current(root.path(), &[])
        })
    };
    assert_eq!(
        crate::review_author_refusal(&authors, &launcher, true),
        None,
        "the launcher did not author the worker's commits"
    );
    // Even from the very shell that created the lease (a human launcher at
    // one terminal): the live lease's creating shell is the launcher's.
    let same_shell = {
        let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_SESSION_ID", None)]);
        let mut src = source_under(104, 4242, proc(91, "bash", &["-bash"], "H"), None);
        src.chain[1].start = Some("t0".into());
        with_identity_source(src, || RecorderIdentity::current(root.path(), &[]))
    };
    assert!(same_shell.tokens.contains(&"shell:4242@t0".to_string()));
    assert_eq!(
        crate::review_author_refusal(&authors, &same_shell, true),
        None
    );
    // The worker, carrying the lease id as its session id: refused.
    let with_id = {
        let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_SESSION_ID", Some(&l.id))]);
        with_identity_source(source_under(102, 202, worker.clone(), None), || {
            RecorderIdentity::current(root.path(), &[])
        })
    };
    assert!(crate::review_author_refusal(&authors, &with_id, true).is_some());
    // The worker with no session id: its stamped process binds it.
    let bare = {
        let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_SESSION_ID", None)]);
        with_identity_source(source_under(103, 203, worker, None), || {
            RecorderIdentity::current(root.path(), &[])
        })
    };
    assert!(crate::review_author_refusal(&authors, &bare, true).is_some());
}

/// `aida worktree add` / `enter` / `do-guided` are the implementer claiming
/// for ITSELF: its own harness identity is the author. Claim the way
/// `ensure_spec_worktree` does (a plain `session_start`, never `as_launcher`),
/// then approve from main, from a new shell under the same harness, after
/// the lease is gone: refused, and the stale-approval override is refused too.
// trace:BUG-1918 | ai:claude
#[test]
fn implementer_claim_via_worktree_add_cannot_self_approve_from_main() {
    // Source guard: the worktree add/enter/do-guided claim is not a hand-off.
    let part4 = include_str!("../worktree_gc.rs");
    let start = part4
        .find("pub(crate) fn ensure_spec_worktree(")
        .expect("ensure_spec_worktree exists");
    let end = start
        + part4[start..]
            .find("\n}\n")
            .expect("ensure_spec_worktree ends");
    let body = &part4[start..end];
    assert!(body.contains("session_start("), "claims via session_start");
    assert!(
        !body.contains("as_launcher("),
        "worktree add/enter must record the implementer's own identity"
    );

    let root = tempfile::tempdir().unwrap();
    let l = lease("aaaabbbb6666", SPEC, root.path().join("wt"));
    write_lease(root.path(), &l);
    let harness = proc(90, "claude", &["claude"], "H");
    {
        let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_SESSION_ID", None)]);
        with_identity_source(source_under(100, 200, harness.clone(), None), || {
            record_authoring_lease(root.path(), &l)
        });
    }
    std::fs::remove_file(crate::lease_path(root.path(), &l.id)).unwrap();
    let authors = author_sessions_at(root.path(), &subjects(), None);
    let from_main = {
        let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_SESSION_ID", None)]);
        with_identity_source(source_under(101, 201, harness, None), || {
            RecorderIdentity::current(root.path(), &[])
        })
    };
    assert!(
        crate::review_author_refusal(&authors, &from_main, true).is_some(),
        "the implementer approving its own work from main is refused"
    );
    let policy = ShipApprovalPolicy {
        author_sessions: authors,
        shipper: from_main.tokens,
        ..Default::default()
    };
    assert!(
        crate::pr_ship::stale_override_refusal(&policy).is_some(),
        "the implementer cannot override a stale approval of its own work"
    );
}

/// The production launch binding: `agent new` spawns its worker with
/// `bind_worker_env` and records it with `record_launched_worker`; `queue
/// work` and `session start --launch` export the lease id before `exec`.
/// A REAL child process is spawned and bound here: once the lease is
/// released, the worker is refused by the session id it actually received
/// and, without it, by its real process identity; the launcher is not.
// trace:BUG-1918 | ai:claude
#[cfg(unix)]
#[test]
fn launched_worker_is_bound_by_the_real_launch_binding() {
    use std::io::BufRead;
    // Source guards: each launch path applies the binding.
    let part2 = include_str!("../agent_launch.rs");
    let tracked = &part2[part2
        .find("pub(crate) fn run_tracked_agent(")
        .expect("run_tracked_agent")..];
    let bind = tracked
        .find("review_authority::bind_worker_env(&mut command")
        .expect("foreground agent new binds the worker env");
    let spawn = tracked.find(".spawn_retrying_etxtbsy()").unwrap();
    let record = tracked
        .find("review_authority::record_launched_worker(")
        .expect("foreground agent new records the spawned worker");
    assert!(bind < spawn && spawn < record);
    let session_start = include_str!("../session_start.rs");
    let launch = &session_start[session_start.find("    if launch_claude {").unwrap()..];
    assert!(
        launch.find("export_worker_session_env(&id)").unwrap()
            < launch.find("session::new_session(").unwrap(),
        "session start --launch exports the lease id before exec"
    );
    let queue = include_str!("../queue_cmd.rs");
    assert!(
        queue.find("export_worker_session_env(&lease.id)").unwrap()
            < queue.find("TASK-112: exec").unwrap(),
        "queue work exports the lease id before launching"
    );

    let root = tempfile::tempdir().unwrap();
    let l = lease("aaaabbbb5555", SPEC, root.path().join("wt"));
    write_lease(root.path(), &l);
    let launcher_harness = proc(90, "claude", &["claude"], "L");
    {
        let _env =
            crate::test_env::EnvVarsGuard::apply(&[("AIDA_SESSION_ID", Some("advisor-session"))]);
        with_identity_source(
            source_under(100, 200, launcher_harness.clone(), None),
            || crate::review_authority::as_launcher(|| record_authoring_lease(root.path(), &l)),
        );
    }
    // Spawn a real worker the way `run_tracked_agent` does.
    let mut cmd = std::process::Command::new("sh");
    cmd.args(["-c", "printf '%s\\n' \"$AIDA_SESSION_ID\"; exec sleep 30"])
        .env("AIDA_SESSION_ID", "advisor-session")
        .stdout(std::process::Stdio::piped());
    crate::review_authority::bind_worker_env(&mut cmd, &l.id);
    let mut child = cmd.spawn().expect("spawn worker");
    crate::review_authority::record_launched_worker(root.path(), &l.id, child.id());
    let mut seen = String::new();
    std::io::BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut seen)
        .unwrap();
    let worker = crate::review_authority::ProcEntry {
        pid: child.id(),
        name: "claude".into(),
        cmd: vec!["claude".into()],
        start: crate::process_probe::process_start_identity(child.id()),
    };
    std::fs::remove_file(crate::lease_path(root.path(), &l.id)).unwrap();
    let authors = author_sessions_at(root.path(), &subjects(), None);

    let worker_seen = seen.trim().to_string();
    assert_eq!(worker_seen, l.id, "the worker received its lease id");
    let with_env = {
        let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_SESSION_ID", Some(&worker_seen))]);
        with_identity_source(source_under(102, 202, worker.clone(), None), || {
            RecorderIdentity::current(root.path(), &[])
        })
    };
    let bare = {
        let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_SESSION_ID", None)]);
        with_identity_source(source_under(103, 203, worker, None), || {
            RecorderIdentity::current(root.path(), &[])
        })
    };
    let launcher = {
        let _env =
            crate::test_env::EnvVarsGuard::apply(&[("AIDA_SESSION_ID", Some("advisor-session"))]);
        with_identity_source(source_under(104, 204, launcher_harness, None), || {
            RecorderIdentity::current(root.path(), &[])
        })
    };
    let _ = child.kill();
    let _ = child.wait();
    assert!(crate::review_author_refusal(&authors, &with_env, true).is_some());
    assert!(
        crate::review_author_refusal(&authors, &bare, true).is_some(),
        "the worker's real process identity binds it without the env"
    );
    assert_eq!(
        crate::review_author_refusal(&authors, &launcher, true),
        None
    );
}

/// An approval must name who recorded it. An identity with no session id,
/// process, or POSIX session is refused at record time, and nothing is
/// written.
#[test]
fn an_unanchored_identity_cannot_record_an_approval() {
    use crate::review_authority::has_identity_anchor;
    assert!(!has_identity_anchor(&[]));
    assert!(!has_identity_anchor(&["lease:aaaabbbb1111".into()]));
    assert!(!has_identity_anchor(&["session: ".into(), "proc:".into()]));
    assert!(has_identity_anchor(&["session:s".into()]));
    assert!(has_identity_anchor(&["proc:9@t".into()]));
    assert!(has_identity_anchor(&["sid:60@t0".into()]));

    let root = tempfile::tempdir().unwrap();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        root.path(),
        "reviewer",
        &[],
        &[("AIDA_SESSION_ID", None)],
    );
    let err = with_identity_source(IdentitySource::default(), || {
        crate::handle_review_record_at(
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
    })
    .expect_err("an unanchored identity must not approve");
    assert!(
        err.to_string().contains("no identity AIDA can bind"),
        "{err}"
    );
    assert!(crate::review_verdict::read_recorded_verdict(root.path(), SPEC).is_none());
}

/// Source-shape guards: `aida release --after-pr`, burndown's wave merge and
/// the drain all judge approval authority before they merge, and pin the
/// merge to the approved head. Needles are split so they cannot match this
/// file's own literals.
#[test]
fn release_burndown_and_drain_merges_pass_the_authority_gate() {
    fn ordered(body: &str, needles: &[&str], what: &str) {
        let mut at = 0;
        for n in needles {
            let i = body[at..]
                .find(n)
                .unwrap_or_else(|| panic!("{what}: `{n}` missing or out of order"));
            at += i + n.len();
        }
    }
    let gate = concat!("merge_path_approval", "_gate(");
    let merge = concat!(".merge_", "change(");

    let release = include_str!("../upgrade_release.rs");
    let start = release
        .find(concat!("`--after-pr` is a ", "merge path"))
        .expect("release --after-pr gate");
    let body = &release[start..start + 4000];
    ordered(
        body,
        &[gate, concat!("match_head: after_pr", "_match_head"), merge],
        "release --after-pr",
    );

    let wave = include_str!("../burndown_run.rs");
    let start = wave
        .find(concat!("fn merge_wave", "_pr(project_root"))
        .expect("burndown wave merge");
    let body = &wave[start..start + 8000];
    ordered(
        body,
        &[gate, concat!("match_head: wave", "_match_head"), merge],
        "burndown wave merge",
    );

    let drain = include_str!("../phase_driver_trait.rs");
    let start = drain
        .find(concat!("merge_approval", "_inputs("))
        .expect("drain approval inputs");
    let body = &drain[start..start + 4000];
    ordered(
        body,
        &[concat!("drain_merge_approval", "_gate("), merge],
        "drain merge",
    );
}
