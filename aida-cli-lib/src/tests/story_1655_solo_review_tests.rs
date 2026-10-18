//! STORY-1655: audited solo-human self-approval. One named test per binding
//! amendment of the operator decision (S1 global-only key, S2 typed terminal
//! challenge, S3 never in the AIDA repository), plus the spec's acceptance
//! tests for the record-time gate and the ship-time gate.
// trace:STORY-1655 | ai:claude

use super::*;
use crate::pr_ship::{
    approval_authority_refusal, solo_approval_notice, ApprovalAuthorityRefusal, AuthorSession,
    ShipApprovalPolicy,
};
use crate::review_authority::RecorderIdentity;
use crate::review_verdict::{RecordedVerdict, RecorderAttestation, VerdictKind};
use crate::{review_author_gate, review_author_refusal, ReviewAuthorGate};

const SPEC: &str = "STORY-91655";
const PR: u64 = 4242;
const HEAD: &str = "1aca4e3e9251aaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HUMAN: &str = "session:solo-human";

fn authors() -> Vec<AuthorSession> {
    vec![AuthorSession {
        label: "claim by the solo human".into(),
        tokens: vec![HUMAN.into()],
    }]
}

fn the_author() -> RecorderIdentity {
    RecorderIdentity {
        tokens: vec![HUMAN.into()],
    }
}

fn global_config(home: &Path, body: &str) {
    std::fs::create_dir_all(home.join(".aida")).unwrap();
    std::fs::write(home.join(".aida").join("config.toml"), body).unwrap();
}

/// HOME may change only in an isolated child process; the child already has
/// a fresh temp HOME. `None` in the parent, which returns once the child
/// passed.
fn isolated_home() -> Option<std::path::PathBuf> {
    if crate::test_env::run_path_isolated_test() {
        return None;
    }
    Some(crate::home_dir().expect("the isolated child has a HOME"))
}

const SOLO_ON: &str = "[review]\nsolo_human_self_approval = true\n";

// ── Record-time gate (spec acceptance) ──────────────────────────────────────

#[test]
fn solo_human_allows_tty_human_to_approve_own_claim() {
    let gate = review_author_gate(&authors(), &the_author(), true, true, || true);
    assert_eq!(gate, ReviewAuthorGate::SoloSelfApproval);

    // The receipt and the attestation the verdict carries both say solo.
    let dir = tempfile::tempdir().unwrap();
    let id = crate::review_authority::write_human_receipt_as(
        dir.path(),
        HEAD,
        SPEC,
        &[HUMAN.into()],
        true,
    )
    .unwrap();
    let receipt = crate::review_authority::human_receipt_in(dir.path(), &id, HEAD).unwrap();
    assert!(receipt.solo_human);
    let att = RecorderAttestation {
        sha: HEAD.into(),
        human_at_tty: true,
        identity: vec![HUMAN.into()],
        receipt_id: Some(id),
        solo_human: true,
        ..Default::default()
    };
    let json = serde_json::to_value(&att).unwrap();
    assert_eq!(json["solo_human"], serde_json::json!(true), "{json}");
}

#[test]
fn solo_human_never_covers_agents() {
    // Solo mode on, but the recorder did not pass the human-at-TTY floor
    // (every agent, and every non-TTY session): the BUG-1918 refusal stands.
    let expected = review_author_refusal(&authors(), &the_author(), true).unwrap();
    let gate = review_author_gate(&authors(), &the_author(), true, false, || true);
    assert_eq!(gate, ReviewAuthorGate::Refused(expected));
}

#[test]
fn solo_human_off_keeps_bug_1918_refusal_byte_identical() {
    for resolved in [true, false] {
        let expected = review_author_refusal(&authors(), &the_author(), resolved).unwrap();
        let gate = review_author_gate(&authors(), &the_author(), resolved, true, || false);
        assert_eq!(gate, ReviewAuthorGate::Refused(expected));
    }
    // An independent recorder never consults the mode at all.
    let other = RecorderIdentity {
        tokens: vec!["session:independent".into()],
    };
    let gate = review_author_gate(&authors(), &other, true, true, || {
        panic!("solo mode must not be read for an independent approval")
    });
    assert_eq!(gate, ReviewAuthorGate::Independent);
}

#[test]
fn default_attestation_serializes_without_a_solo_field() {
    let att = RecorderAttestation {
        sha: HEAD.into(),
        human_at_tty: true,
        ..Default::default()
    };
    let json = serde_json::to_string(&att).unwrap();
    assert!(!json.contains("solo_human"), "{json}");
    let back: RecorderAttestation = serde_json::from_str(&json).unwrap();
    assert!(!back.solo_human);
}

/// End to end through `aida review record`: with the global key on, the
/// author session (no human-at-TTY floor under test, like any agent) is still
/// refused with the BUG-1918 text and nothing is written.
#[test]
fn solo_human_mode_on_still_refuses_the_author_agent_end_to_end() {
    let Some(home) = isolated_home() else {
        return;
    };
    let root = tempfile::tempdir().unwrap();
    global_config(&home, SOLO_ON);
    let session = "cb4777a7-0000-4000-8000-000000016550";
    let lease = crate::SessionLease {
        tree_kind: Default::default(),
        id: "aaaabbbb1655".into(),
        scope: SPEC.into(),
        slug: SPEC.to_ascii_lowercase(),
        owner: "tester".into(),
        worktree_path: root.path().join("elsewhere"),
        branch: "story-91655-work".into(),
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
    };
    {
        let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_SESSION_ID", Some(session))]);
        crate::review_authority::record_authoring_lease(root.path(), &lease);
    }
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        root.path(),
        "reviewer",
        &[],
        &[("AIDA_SESSION_ID", Some(session))],
    );
    assert_eq!(global_value_quiet(), Ok(true), "the global key is on");
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
    .expect_err("solo mode never covers a session without the human floor");
    assert!(
        err.to_string().contains("cannot approve its own work"),
        "{err}"
    );
    assert!(crate::review_verdict::read_recorded_verdict(root.path(), SPEC).is_none());
}

// ── S1: global user config only ─────────────────────────────────────────────

#[test]
fn s1_project_config_value_is_ignored_with_a_warning() {
    if isolated_home().is_none() {
        return;
    }
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(project.path().join(".aida")).unwrap();
    std::fs::write(project.path().join(".aida").join("config.toml"), SOLO_ON).unwrap();

    assert!(project_value_present(project.path()));
    assert!(
        warn_if_project_value(project.path()),
        "the project value is warned about"
    );
    assert!(PROJECT_VALUE_IGNORED.contains("ignored"));
    assert!(
        !effective(project.path()),
        "a project value never turns the mode on"
    );
    let row = crate::config_cmd::solo_human_policy_value(project.path()).0;
    assert!(row.starts_with("false"), "{row}");
    assert!(row.contains("ignored"), "{row}");
}

#[test]
fn s1_global_user_config_turns_the_mode_on() {
    let Some(home) = isolated_home() else {
        return;
    };
    let project = tempfile::tempdir().unwrap();
    assert!(!effective(project.path()), "default off");
    assert_eq!(
        crate::config_cmd::solo_human_policy_value(project.path()).0,
        "false"
    );
    global_config(&home, SOLO_ON);
    assert!(effective(project.path()));
    global_config(&home, "[review]\nsolo_human_self_approval = \"yes\"\n");
    assert!(global_value_quiet().is_err(), "a non-boolean is refused");
    assert!(!effective(project.path()), "and resolves to off");
}

// ── S2: typed challenge from the controlling terminal ───────────────────────

fn tty(answer: &str) -> Option<(std::io::Cursor<Vec<u8>>, Vec<u8>)> {
    Some((std::io::Cursor::new(answer.as_bytes().to_vec()), Vec::new()))
}

#[test]
fn s2_piped_answer_is_refused() {
    // A scripted run (stdin is not a terminal) is refused even when the
    // correct answer is available: nothing is read.
    let err = run_challenge_with(false, tty("STORY-91655 4242\n"), SPEC, PR).unwrap_err();
    assert!(err.to_string().contains("piped or scripted"), "{err}");

    // The production path agrees when stdin is piped.
    let root = tempfile::tempdir().unwrap();
    let _ambient = crate::test_env::AmbientGuard::hermetic(root.path(), None);
    assert!(!crate::authority_stdin_is_terminal());
    let err = run_challenge(SPEC, PR).unwrap_err();
    assert!(err.to_string().contains("piped or scripted"), "{err}");
}

#[test]
fn s2_challenge_needs_a_controlling_terminal_and_the_exact_answer() {
    let none: Option<(std::io::Cursor<Vec<u8>>, Vec<u8>)> = None;
    assert!(run_challenge_with(true, none, SPEC, PR).is_err());
    assert!(run_challenge_with(true, tty("STORY-91655 4243\n"), SPEC, PR).is_err());
    assert!(run_challenge_with(true, tty("\n"), SPEC, PR).is_err());
    assert!(run_challenge_with(true, tty(""), SPEC, PR).is_err());
    assert!(run_challenge_with(true, tty("story-91655  #4242\n"), SPEC, PR).is_ok());
}

#[test]
fn s2_challenge_never_reads_stdin() {
    let src = include_str!("../solo_review.rs");
    let body = src.split("#[cfg(test)]\n#[path").next().unwrap();
    assert!(
        !body.contains("io::stdin"),
        "the answer must come from /dev/tty"
    );
    assert!(body.contains("\"/dev/tty\""));
}

// ── S3: never in the AIDA repository ────────────────────────────────────────

#[test]
fn s3_aida_repository_never_runs_solo_mode() {
    let Some(home) = isolated_home() else {
        return;
    };
    assert!(!effective_with(true, true));
    assert!(effective_with(true, false));
    assert!(!effective_with(false, false));

    let project = tempfile::tempdir().unwrap();
    std::fs::write(
        project.path().join("Cargo.toml"),
        "[workspace]\n[workspace.package]\nrepository = \"https://github.com/joemooney/aida\"\n",
    )
    .unwrap();
    global_config(&home, SOLO_ON);
    assert!(crate::is_aida_repo(project.path()));
    assert!(!effective(project.path()), "global on, still off in AIDA");
}

// ── Ship-time gate ──────────────────────────────────────────────────────────

fn solo_approval(receipt: &str) -> RecordedVerdict {
    RecordedVerdict {
        kind: VerdictKind::Approved,
        raw: "approved".into(),
        reviewed_sha: Some(HEAD.into()),
        recorded_at: Some("2026-10-10T05:30:00Z".into()),
        recorded_by: Some("aida review record (operator)".into()),
        attestation: Some(RecorderAttestation {
            sha: HEAD.into(),
            human_at_tty: true,
            identity: vec![HUMAN.into()],
            receipt_id: Some(receipt.into()),
            solo_human: true,
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn ship_policy(v: &RecordedVerdict, solo_on: bool, solo_verified: bool) -> ShipApprovalPolicy {
    let key = v
        .attestation
        .as_ref()
        .unwrap()
        .authority_key(v.recorded_at.as_deref())
        .unwrap();
    let mut p = ShipApprovalPolicy {
        author_sessions: authors(),
        reviewed_committed_at: Some("2026-10-10T05:00:00Z".parse().unwrap()),
        solo_human: solo_on,
        ..Default::default()
    };
    p.verified_authority.insert(key.clone(), vec![HUMAN.into()]);
    if solo_verified {
        p.verified_solo.insert(key);
    }
    p
}

#[test]
fn ship_gate_honours_a_verified_solo_self_approval() {
    let v = [solo_approval("2f1d7c2e-0000-4000-8000-000000016550")];
    let policy = ship_policy(&v[0], true, true);
    assert_eq!(approval_authority_refusal(&v, &policy), None);
    let notice = solo_approval_notice(&v, &policy).expect("names the mode");
    assert!(notice.contains("solo-human"), "{notice}");
}

#[test]
fn ship_gate_refuses_a_solo_approval_when_the_mode_is_off_at_ship_time() {
    let v = [solo_approval("2f1d7c2e-0000-4000-8000-000000016550")];
    let policy = ship_policy(&v[0], false, true);
    assert!(matches!(
        approval_authority_refusal(&v, &policy),
        Some(ApprovalAuthorityRefusal::AuthorSession { .. })
    ));
    assert_eq!(solo_approval_notice(&v, &policy), None);
}

#[test]
fn ship_gate_refuses_a_solo_flag_its_receipt_does_not_carry() {
    // Flipping `solo_human` in the verdict JSON verifies nothing: the
    // receipt in the user's AIDA home must say solo too.
    let aida_home = tempfile::tempdir().unwrap();
    let home_s = aida_home.path().to_string_lossy().to_string();
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home_s))]);
    let receipts = aida_home.path().join(".aida").join("review-receipts");
    let plain = crate::review_authority::write_human_receipt_as(
        &receipts,
        HEAD,
        SPEC,
        &[HUMAN.into()],
        false,
    )
    .unwrap();
    let solo = crate::review_authority::write_human_receipt_as(
        &receipts,
        HEAD,
        SPEC,
        &[HUMAN.into()],
        true,
    )
    .unwrap();
    let forged = solo_approval(&plain);
    let genuine = solo_approval(&solo);
    let verified = crate::review_authority::verified_solo_keys(&[forged.clone(), genuine.clone()]);
    let key = |v: &RecordedVerdict| {
        v.attestation
            .as_ref()
            .unwrap()
            .authority_key(v.recorded_at.as_deref())
            .unwrap()
    };
    assert!(!verified.contains(&key(&forged)), "{verified:?}");
    assert!(verified.contains(&key(&genuine)), "{verified:?}");

    let policy = ship_policy(&forged, true, false);
    assert!(matches!(
        approval_authority_refusal(&[forged], &policy),
        Some(ApprovalAuthorityRefusal::AuthorSession { .. })
    ));
}

#[test]
fn doctor_and_awaiting_name_the_mode() {
    assert_eq!(
        MODE_ON_LINE,
        "review: solo-human mode ON (self-approval permitted for TTY humans)"
    );
}
