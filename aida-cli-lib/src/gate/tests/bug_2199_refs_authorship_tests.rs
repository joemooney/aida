//! BUG-2199: a `(Refs X)`-only PR binds authorship to X, and a generic
//! `harness-worktree` lease (which names no spec) never authors a PR it is
//! not working on — so stale main-checkout harness leases cannot make every
//! Refs-only PR unshippable.
// trace:BUG-2199 | ai:claude

use super::*;
use crate::gate::review_verdict::{RecordedVerdict, RecorderAttestation, VerdictKind};
use crate::gate::ship::{approval_authority_refusal, ApprovalAuthorityRefusal};
use crate::worktree_lease::HARNESS_WORKTREE_SCOPE;

const SPEC: &str = "BUG-91901";
const HEAD: &str = "2bdb5f4f9251aaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PR: u64 = 2791;

fn lease(id: &str, scope: &str, worktree: PathBuf, branch: &str) -> SessionLease {
    SessionLease {
        tree_kind: Default::default(),
        id: id.to_string(),
        scope: scope.to_string(),
        slug: scope.to_ascii_lowercase(),
        owner: "tester".into(),
        worktree_path: worktree,
        branch: branch.to_string(),
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
        child_grant_id: None,
        interrupted_reason: None,
    }
}

fn write_lease(root: &Path, l: &SessionLease) {
    let path = crate::lease_path(root, &l.id);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, toml::to_string_pretty(l).unwrap()).unwrap();
}

fn proc(pid: u32, name: &str) -> ProcEntry {
    ProcEntry {
        pid,
        name: name.into(),
        cmd: vec![name.into()],
        start: Some("h".into()),
    }
}

fn source(aida: u32, shell: u32, harness: u32) -> IdentitySource {
    IdentitySource {
        session_ids: vec![],
        chain: vec![
            proc(aida, "aida"),
            proc(shell, "bash"),
            proc(harness, "claude"),
            proc(10, "systemd"),
        ],
        session_leader: None,
    }
}

fn reviewer_approval(identity: Vec<String>) -> RecordedVerdict {
    RecordedVerdict {
        kind: VerdictKind::Approved,
        raw: "approved".into(),
        reviewed_sha: Some(HEAD.into()),
        recorded_at: Some("2026-10-20T21:00:00Z".into()),
        recorded_by: Some("aida review record (reviewer)".into()),
        attestation: Some(RecorderAttestation {
            sha: HEAD.into(),
            seat: Some("reviewer".into()),
            grant_id: Some(uuid::Uuid::new_v4().to_string()),
            identity,
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// A non-author reviewer recording with the main checkout as its cwd: it
/// binds `lease` (pinned there), sharing no process with its creator.
fn main_checkout_recorder(root: &Path, lease: &SessionLease) -> RecorderIdentity {
    let recorder = RecorderIdentity::with_context(
        with_identity_source(source(101, 201, 301), || {
            IdentitySource::live().recorder_tokens()
        }),
        std::slice::from_ref(lease),
        &[root.to_path_buf()],
        None,
    );
    assert!(recorder.tokens.contains(&format!("lease:{}", lease.id)));
    recorder
}

fn spec_less_subject() -> crate::ReviewPrSubject {
    crate::ReviewPrSubject {
        head_ref: "claude/spec-less".into(),
        commit_subjects: vec!["[AI:claude] chore(x): tidy".into()],
    }
}

fn ship_policy(
    subject: crate::ReviewPrSubject,
    root: &Path,
) -> crate::gate::ship::ShipApprovalPolicy {
    let head = subject.head_ref.clone();
    crate::with_review_pr_subject(subject, || {
        crate::gate::approval::merge_approval_inputs(
            &[root],
            root,
            PR,
            &[],
            Some(&head),
            Some(HEAD),
        )
    })
    .1
}

fn refs_only_subject() -> crate::ReviewPrSubject {
    crate::ReviewPrSubject {
        head_ref: "claude/fixture-hygiene".into(),
        commit_subjects: vec![format!(
            "[AI:claude] test(git): fixture hygiene (Refs {SPEC})"
        )],
    }
}

#[test]
fn harness_worktree_lease_is_not_an_authoring_lease() {
    let root = tempfile::tempdir().unwrap();
    let harness = lease(
        "hhhh00001111",
        HARNESS_WORKTREE_SCOPE,
        root.path().into(),
        "main",
    );
    assert!(!is_authoring_lease(&harness));
    assert!(!is_authoring_lease_for(
        &harness,
        &[HARNESS_WORKTREE_SCOPE.to_string()]
    ));
    assert!(is_authoring_lease(&lease(
        "aaaa00001111",
        SPEC,
        root.path().join("wt"),
        "b"
    )));
    // A harness lease still authors the branch it works on.
    let on_branch = lease(
        "hhhh00002222",
        HARNESS_WORKTREE_SCOPE,
        root.path().join("wt"),
        "topic",
    );
    let authors = author_sessions(
        &[on_branch],
        &[],
        &[SPEC.to_string()],
        Some("topic"),
        |_| None,
    );
    assert_eq!(authors.len(), 1, "{authors:?}");
}

#[test]
fn refs_subject_names_authorship_ids_but_never_completion_ids() {
    let s = format!("[AI:claude] fix(x): y (Refs {SPEC}, TASK-91902) (#12)");
    assert!(crate::pr_ship::extract_trailing_spec_ids_from_subject(&s).is_empty());
    assert_eq!(
        crate::pr_ship::extract_authorship_spec_ids_from_subject(&s),
        vec![SPEC.to_string(), "TASK-91902".to_string()]
    );
    let both = format!("[AI:claude] fix(x): y (Refs: {SPEC}) (TASK-91903)");
    assert_eq!(
        crate::pr_ship::extract_authorship_spec_ids_from_subject(&both),
        vec!["TASK-91903".to_string(), SPEC.to_string()]
    );
    assert_eq!(
        crate::pr_ship::extract_authorship_spec_ids_from_subject("fix(x): y (Refs:BUG-1)"),
        vec!["BUG-1".to_string()]
    );
    assert!(
        crate::pr_ship::extract_authorship_spec_ids_from_subject("fix(x): (Refsy BUG-1)")
            .is_empty()
    );
    // The BUG-2203 Refs grammar: singular `Ref` and a spaced colon bind too.
    for subject in ["fix(x): y (Ref BUG-1)", "fix(x): y (Refs : BUG-1)"] {
        assert!(crate::pr_ship::extract_trailing_spec_ids_from_subject(subject).is_empty());
        assert_eq!(
            crate::pr_ship::extract_authorship_spec_ids_from_subject(subject),
            vec!["BUG-1".to_string()],
            "{subject}"
        );
        assert_eq!(
            crate::pr_ship::extract_trailing_refs_spec_ids_from_subject(subject),
            vec!["BUG-1".to_string()],
            "{subject}"
        );
    }
}

/// The PR-2791 shape: a Refs-only PR, a stale `harness-worktree` lease pinned
/// to the main checkout (live and in the ledger), and a non-author reviewer
/// whose cwd is the main checkout. Its cwd still binds that lease (the
/// remaining behaviour); the lease just no longer authors the PR.
#[test]
fn refs_only_pr_with_a_main_checkout_harness_lease_ships_a_non_author_approval() {
    let root = tempfile::tempdir().unwrap();
    let stale = lease(
        "hhhh00003333",
        HARNESS_WORKTREE_SCOPE,
        root.path().into(),
        "main",
    );
    write_lease(root.path(), &stale);
    with_identity_source(source(100, 200, 300), || {
        record_authoring_lease(root.path(), &stale)
    });
    let recorder = main_checkout_recorder(root.path(), &stale);
    let v = [reviewer_approval(recorder.tokens.clone())];

    let (_, policy) = crate::with_review_pr_subject(refs_only_subject(), || {
        crate::gate::approval::merge_approval_inputs(
            &[root.path()],
            root.path(),
            PR,
            &[],
            Some("claude/fixture-hygiene"),
            Some(HEAD),
        )
    });
    assert!(policy.author_sessions.is_empty(), "{policy:?}");
    assert!(!matches!(
        approval_authority_refusal(&v, &policy),
        Some(ApprovalAuthorityRefusal::AuthorSession { .. })
    ));
}

/// The narrowing is to the Refs spec, not away from authors: the real author
/// on the Refs spec is still refused, and an unrelated author is not counted.
#[test]
fn refs_only_pr_still_refuses_the_refs_specs_author() {
    let root = tempfile::tempdir().unwrap();
    let author = lease(
        "aaaa00004444",
        SPEC,
        root.path().join("wt"),
        "bug-91901-work",
    );
    let unrelated = lease(
        "aaaa00005555",
        "TASK-91904",
        root.path().join("wt2"),
        "task-91904-work",
    );
    write_lease(root.path(), &unrelated);
    with_identity_source(source(100, 200, 300), || {
        record_authoring_lease(root.path(), &author)
    });
    let recorder = with_identity_source(source(101, 201, 300), || RecorderIdentity::current(&[]));
    let v = [reviewer_approval(recorder.tokens.clone())];

    let (_, policy) = crate::with_review_pr_subject(refs_only_subject(), || {
        crate::gate::approval::merge_approval_inputs(
            &[root.path()],
            root.path(),
            PR,
            &[],
            Some("claude/fixture-hygiene"),
            Some(HEAD),
        )
    });
    assert_eq!(policy.author_sessions.len(), 1, "{policy:?}");
    assert!(matches!(
        approval_authority_refusal(&v, &policy),
        Some(ApprovalAuthorityRefusal::AuthorSession { .. })
    ));
}

/// Leases bind a recorder by its actual cwd, not by the project root it names:
/// a recorder outside the main checkout is not bound to a lease pinned there.
#[test]
fn recorder_binds_leases_by_cwd_not_project_root() {
    let root = tempfile::tempdir().unwrap();
    let claim = lease(
        "aaaa00006666",
        SPEC,
        root.path().canonicalize().unwrap(),
        "main",
    );
    let recorder = RecorderIdentity::current(std::slice::from_ref(&claim));
    assert!(
        !recorder.tokens.contains(&format!("lease:{}", claim.id)),
        "{recorder:?}"
    );
}

/// Fail-closed path (the PR maps to NO spec): a stale main-checkout harness
/// lease on another branch does not author it, so a non-author approval ships.
#[test]
fn spec_less_pr_ignores_a_harness_lease_on_another_branch() {
    let root = tempfile::tempdir().unwrap();
    let stale = lease(
        "hhhh00007777",
        HARNESS_WORKTREE_SCOPE,
        root.path().into(),
        "main",
    );
    write_lease(root.path(), &stale);
    with_identity_source(source(100, 200, 300), || {
        record_authoring_lease(root.path(), &stale)
    });
    let v = [reviewer_approval(
        main_checkout_recorder(root.path(), &stale).tokens,
    )];
    let policy = ship_policy(spec_less_subject(), root.path());
    assert!(policy.author_sessions.is_empty(), "{policy:?}");
    assert!(!matches!(
        approval_authority_refusal(&v, &policy),
        Some(ApprovalAuthorityRefusal::AuthorSession { .. })
    ));
}

/// Fail-closed path: a harness lease working ON the PR's head branch authored
/// it, live or only in the ledger, so its own approval is refused.
#[test]
fn spec_less_pr_refuses_the_harness_lease_on_its_head_branch() {
    for live in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let author = lease(
            "hhhh00008888",
            HARNESS_WORKTREE_SCOPE,
            root.path().join("wt"),
            "claude/spec-less",
        );
        if live {
            write_lease(root.path(), &author);
        }
        with_identity_source(source(100, 200, 300), || {
            record_authoring_lease(root.path(), &author)
        });
        let recorder =
            with_identity_source(source(101, 201, 300), || RecorderIdentity::current(&[]));
        let v = [reviewer_approval(recorder.tokens)];
        let policy = ship_policy(spec_less_subject(), root.path());
        assert!(!policy.author_sessions.is_empty(), "live={live}");
        assert!(
            matches!(
                approval_authority_refusal(&v, &policy),
                Some(ApprovalAuthorityRefusal::AuthorSession { .. })
            ),
            "live={live}: {policy:?}"
        );
    }
}
