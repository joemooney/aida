//! STORY-2215: the one-time parent grant `aida agent new` mints at a human
//! terminal. Its record is the audit trail; a child derived from it validates
//! by the parent's id, expiry and revocation only (never its process binding),
//! so a child outlives its launcher (`--bg`); and no later process may derive
//! a NEW child from it.
// trace:STORY-2215 | ai:claude

use super::*;
use crate::test_env::EnvVarsGuard;

const SUBJECT: &str = "story-2215-fixture-user";
/// A launcher process that has exited: no live process carries this token.
const GONE_LAUNCHER: &str = "proc:999998@linux-starttime:1";

fn pinned_env() -> EnvVarsGuard {
    let mut env = EnvVarsGuard::snapshot(&["AIDA_USER", GRANT_ENV, "AIDA_SESSION_ID"]);
    env.set_key("AIDA_USER", SUBJECT);
    env.unset_key("AIDA_SESSION_ID");
    env.unset_key(GRANT_ENV);
    env
}

/// The roster ceiling (advisor + implementer for [`SUBJECT`]) the inline mint
/// checks, seeded through the shared fixture.
fn seed_roster(root: &Path) {
    test_support::mint_grant_for(root, SUBJECT, "advisor", &["implementer"]);
}

fn inline_parent(root: &Path, binding: Vec<String>) -> SeatGrant {
    mint_direct(
        root,
        "advisor",
        vec!["implementer".into()],
        INLINE_PARENT_MAX,
        binding,
        Some(INLINE_ORIGIN),
    )
    .unwrap()
}

#[test]
fn inline_parent_record_carries_principal_seat_and_origin() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = pinned_env();
    seed_roster(tmp.path());
    let parent = inline_parent(tmp.path(), vec![GONE_LAUNCHER.into()]);
    let record = read_grant(&parent.id).unwrap();
    assert_eq!(record.principal, SUBJECT);
    assert_eq!(record.subject, SUBJECT);
    assert_eq!(record.seat, "advisor");
    assert_eq!(record.delegable_seats, vec!["implementer".to_string()]);
    assert_eq!(record.origin.as_deref(), Some(INLINE_ORIGIN));
    assert!(record.parent_grant_id.is_none());
    assert!(record.expires_at - record.issued_at <= INLINE_PARENT_MAX);
    // A role-enter-shaped record (no origin) serializes without the field.
    let mut plain = record.clone();
    plain.origin = None;
    assert!(!serde_json::to_string(&plain).unwrap().contains("origin"));
}

/// Advisor ruling Q2 (REQUIRED): the launcher exits (the `--bg` case) and the
/// child grant still validates; the parent's binding is never checked.
#[test]
fn child_of_inline_parent_validates_after_launcher_exits() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = pinned_env();
    seed_roster(tmp.path());
    let parent = inline_parent(tmp.path(), vec![GONE_LAUNCHER.into()]);
    let child = issue_child_from(parent.clone(), "implementer", SUBJECT, false).unwrap();
    assert_eq!(child.parent_grant_id.as_deref(), Some(parent.id.as_str()));
    assert!(child.expires_at <= parent.expires_at);
    match grant_check_for(tmp.path(), &child.id, SUBJECT) {
        GrantCheck::Valid(grant) => assert_eq!(grant.seat, "implementer"),
        other => panic!("child of an exited launcher's inline parent must validate: {other:?}"),
    }
    // Revoking the parent still ends the child, as for any parent.
    revoke_by_id(&parent.id).unwrap();
    assert!(matches!(
        grant_check_for(tmp.path(), &child.id, SUBJECT),
        GrantCheck::Rejected { .. }
    ));
}

/// Ruling Q2: a NEW child from an inline parent needs a fresh human prompt.
/// A process that presents the parent (here: bound to this very process, as
/// an exec'd worker would be) but did not mint it is refused.
#[test]
fn inline_parent_is_not_re_derivable_by_another_process() {
    let tmp = tempfile::tempdir().unwrap();
    let mut env = pinned_env();
    seed_roster(tmp.path());
    let parent = inline_parent(tmp.path(), own_process_token().into_iter().collect());
    env.set_key(GRANT_ENV, &parent.id);
    assert_eq!(current_seat(tmp.path()).as_deref(), Some("advisor"));
    let refusal = validate_child_delegation(tmp.path(), "implementer")
        .expect_err("an inline parent this process did not mint is not re-derivable");
    assert!(
        format!("{refusal:#}").contains("one-time grant"),
        "{refusal:#}"
    );
}

#[test]
fn derivable_only_for_ordinary_grants_or_the_minting_process() {
    let base = SeatGrant {
        id: "parent-id".into(),
        principal: SUBJECT.into(),
        subject: SUBJECT.into(),
        session_id: String::new(),
        seat: "advisor".into(),
        tty_issued_at: Utc::now(),
        delegable_seats: vec!["implementer".into()],
        parent_grant_id: None,
        issued_at: Utc::now(),
        expires_at: Utc::now(),
        revoked_at: None,
        binding: Vec::new(),
        origin: None,
    };
    assert!(inline_parent_derivable(&base, None));
    let inline = SeatGrant {
        origin: Some(INLINE_ORIGIN.into()),
        ..base
    };
    assert!(!inline_parent_derivable(&inline, None));
    assert!(!inline_parent_derivable(&inline, Some("other-id")));
    assert!(inline_parent_derivable(&inline, Some("parent-id")));
}
