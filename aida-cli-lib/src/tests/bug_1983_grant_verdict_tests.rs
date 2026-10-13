//! BUG-1983: an exported grant that fails validation is reported as WHICH
//! check failed, not "No active role"; `review record` refuses it; and the
//! verdict accepts exactly the grants `current_grant` always accepted.
// trace:BUG-1983 | ai:claude

use crate::seat_authority::{
    current_grant, grant_check, test_support::mint_grant_for, GrantCheck, GrantRejection,
    SeatGrant, GRANT_ENV,
};
use chrono::{Duration, Utc};
use std::path::Path;

/// `current_grant` exactly as it was before BUG-1983, kept verbatim as the
/// reference the new verdict must agree with (C1).
fn legacy_current_grant(project_root: &Path) -> Option<SeatGrant> {
    use crate::seat_authority::roster_allows;
    let id = std::env::var(GRANT_ENV).ok()?;
    let grant = read(&id)?;
    let now = Utc::now();
    if grant.revoked_at.is_some()
        || now >= grant.expires_at
        || grant.subject != crate::current_user_id(None)
        || !roster_allows(project_root, &grant.principal, &grant.seat)
    {
        return None;
    }
    if let Some(parent_id) = &grant.parent_grant_id {
        let parent = read(parent_id)?;
        if parent.revoked_at.is_some()
            || now >= parent.expires_at
            || parent.principal != grant.principal
            || !roster_allows(project_root, &parent.principal, &parent.seat)
            || !parent
                .delegable_seats
                .iter()
                .any(|seat| seat == &grant.seat)
        {
            return None;
        }
    }
    Some(grant)
}

fn grant_file(id: &str) -> std::path::PathBuf {
    crate::aida_home_dir()
        .unwrap()
        .join(".aida")
        .join("session-grants")
        .join(format!("{id}.json"))
}

fn read(id: &str) -> Option<SeatGrant> {
    uuid::Uuid::parse_str(id).ok()?;
    let grant: SeatGrant = serde_json::from_slice(&std::fs::read(grant_file(id)).ok()?).ok()?;
    (grant.id == id).then_some(grant)
}

fn edit(id: &str, change: impl FnOnce(&mut SeatGrant)) {
    let mut grant = read(id).unwrap();
    change(&mut grant);
    std::fs::write(grant_file(id), serde_json::to_vec(&grant).unwrap()).unwrap();
}

fn set_roster(root: &Path, subject: &str, seats: &[&str]) {
    let store = crate::detect_distributed_store_from(root).unwrap();
    let seats = seats
        .iter()
        .map(|s| format!("\"{s}\""))
        .collect::<Vec<_>>()
        .join(", ");
    std::fs::write(
        store.join("registry").join("team.toml"),
        format!("[members]\n\"{subject}\" = [{seats}]\n"),
    )
    .unwrap();
}

/// Mint a child `implementer` grant for this process, delegated from an
/// `advisor` parent, and return `(parent, child)`.
fn delegated(root: &Path, me: &str) -> (String, String) {
    let parent = mint_grant_for(root, me, "advisor", &["implementer"]);
    let child = mint_grant_for(root, me, "implementer", &[]);
    edit(&child, |g| g.parent_grant_id = Some(parent.clone()));
    (parent, child)
}

/// Run one fixture: `setup` returns the handle to export. Asserts the new
/// verdict and the legacy check agree, and returns the verdict.
fn verdict(setup: impl FnOnce(&Path, &str) -> String) -> (String, GrantCheck) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let me = crate::current_user_id(None);
    let id = setup(root, &me);
    let _env = crate::test_env::EnvVarsGuard::apply(&[(GRANT_ENV, Some(id.as_str()))]);
    let check = grant_check(root);
    assert_eq!(
        legacy_current_grant(root).map(|g| g.id),
        current_grant(root).map(|g| g.id),
        "current_grant must accept exactly what it always accepted ({check:?})"
    );
    assert_eq!(
        matches!(check, GrantCheck::Valid(_)),
        legacy_current_grant(root).is_some(),
        "{check:?}"
    );
    (id, check)
}

fn rejection(setup: impl FnOnce(&Path, &str) -> String) -> (String, GrantRejection) {
    match verdict(setup) {
        (id, GrantCheck::Rejected { reason, .. }) => (id, reason),
        (_, other) => panic!("expected a rejection, got {other:?}"),
    }
}

fn past() -> chrono::DateTime<Utc> {
    Utc::now() - Duration::hours(1)
}

#[test]
fn a_valid_grant_and_a_valid_delegation_stay_valid() {
    let (_, check) = verdict(|root, me| mint_grant_for(root, me, "advisor", &[]));
    assert!(matches!(check, GrantCheck::Valid(ref g) if g.seat == "advisor"));
    let (_, check) = verdict(|root, me| delegated(root, me).1);
    assert!(matches!(check, GrantCheck::Valid(ref g) if g.seat == "implementer"));
}

#[test]
fn no_exported_grant_is_absent_not_rejected() {
    let _env = crate::test_env::EnvVarsGuard::apply(&[(GRANT_ENV, None)]);
    let tmp = tempfile::tempdir().unwrap();
    assert!(matches!(grant_check(tmp.path()), GrantCheck::Absent));
    drop(_env);
    let _env = crate::test_env::EnvVarsGuard::apply(&[(GRANT_ENV, Some("  "))]);
    assert!(matches!(grant_check(tmp.path()), GrantCheck::Absent));
}

/// C3 regression pin: an env-only advisor label with no grant has no
/// advisor authority.
#[test]
fn env_only_advisor_label_has_no_authority() {
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_SESSION_ROLE", Some("advisor")),
        (GRANT_ENV, None),
    ]);
    assert!(!crate::role::has_advisor_authority());
}

/// Every rejection variant, each reached on its own, plus fixtures where two
/// checks fail at once so the reported reason pins the check order (C1).
#[test]
fn every_rejection_names_its_check_in_the_legacy_order() {
    let unknown = uuid::Uuid::new_v4().to_string();
    let cases: Vec<(&str, Box<dyn FnOnce(&Path, &str) -> String>, GrantRejection)> = vec![
        (
            "bad handle",
            Box::new(|_, _| "not-a-grant".into()),
            GrantRejection::BadHandle,
        ),
        (
            "unknown id",
            Box::new(move |_, _| unknown),
            GrantRejection::Unreadable,
        ),
        (
            "malformed",
            Box::new(|root, me| {
                let id = mint_grant_for(root, me, "advisor", &[]);
                std::fs::write(grant_file(&id), b"{").unwrap();
                id
            }),
            GrantRejection::Malformed,
        ),
        (
            "handle mismatch",
            Box::new(|root, me| {
                let id = mint_grant_for(root, me, "advisor", &[]);
                edit(&id, |g| g.id = uuid::Uuid::new_v4().to_string());
                id
            }),
            GrantRejection::Malformed,
        ),
        (
            "revoked and expired: revoked wins",
            Box::new(|root, me| {
                let id = mint_grant_for(root, me, "advisor", &[]);
                edit(&id, |g| {
                    g.revoked_at = Some(past());
                    g.expires_at = past();
                });
                id
            }),
            GrantRejection::Revoked { at: Utc::now() },
        ),
        (
            "expired and foreign: expired wins",
            Box::new(|root, me| {
                let id = mint_grant_for(root, me, "advisor", &[]);
                edit(&id, |g| {
                    g.expires_at = past();
                    g.subject = "someone-else".into();
                });
                id
            }),
            GrantRejection::Expired { at: Utc::now() },
        ),
        (
            "foreign and off-roster: subject wins",
            Box::new(|root, me| {
                let id = mint_grant_for(root, me, "advisor", &[]);
                edit(&id, |g| g.subject = "someone-else".into());
                set_roster(root, me, &[]);
                id
            }),
            GrantRejection::SubjectMismatch {
                grant_subject: "someone-else".into(),
                caller: String::new(),
            },
        ),
        (
            "off-roster",
            Box::new(|root, me| {
                let id = mint_grant_for(root, me, "advisor", &[]);
                set_roster(root, me, &["reviewer"]);
                id
            }),
            GrantRejection::RosterDenied {
                principal: String::new(),
                seat: "advisor".into(),
            },
        ),
        (
            "parent missing",
            Box::new(|root, me| {
                let (parent, child) = delegated(root, me);
                std::fs::remove_file(grant_file(&parent)).unwrap();
                child
            }),
            GrantRejection::Parent(Box::new(GrantRejection::Unreadable)),
        ),
        (
            "parent revoked and expired: revoked wins",
            Box::new(|root, me| {
                let (parent, child) = delegated(root, me);
                edit(&parent, |g| {
                    g.revoked_at = Some(past());
                    g.expires_at = past();
                });
                child
            }),
            GrantRejection::Parent(Box::new(GrantRejection::Revoked { at: Utc::now() })),
        ),
        (
            "parent expired and foreign: expired wins",
            Box::new(|root, me| {
                let (parent, child) = delegated(root, me);
                edit(&parent, |g| {
                    g.expires_at = past();
                    g.principal = "someone-else".into();
                });
                child
            }),
            GrantRejection::Parent(Box::new(GrantRejection::Expired { at: Utc::now() })),
        ),
        (
            "parent foreign and off-roster: principal wins",
            Box::new(|root, me| {
                let (parent, child) = delegated(root, me);
                edit(&parent, |g| g.principal = "someone-else".into());
                set_roster(root, me, &["implementer"]);
                child
            }),
            GrantRejection::ParentPrincipalMismatch,
        ),
        (
            "parent off-roster and not delegating: roster wins",
            Box::new(|root, me| {
                let (parent, child) = delegated(root, me);
                edit(&parent, |g| g.delegable_seats.clear());
                set_roster(root, me, &["implementer"]);
                child
            }),
            GrantRejection::Parent(Box::new(GrantRejection::RosterDenied {
                principal: String::new(),
                seat: "advisor".into(),
            })),
        ),
        (
            "parent not delegating",
            Box::new(|root, me| {
                let (parent, child) = delegated(root, me);
                edit(&parent, |g| g.delegable_seats.clear());
                child
            }),
            GrantRejection::ParentNotDelegating {
                parent_seat: "advisor".into(),
            },
        ),
    ];
    let mut seen = std::collections::BTreeSet::new();
    for (name, setup, expected) in cases {
        let (_, got) = rejection(setup);
        assert_eq!(
            same_check(&got),
            same_check(&expected),
            "{name}: got {got:?}"
        );
        seen.insert(same_check(&got));
    }
    // Every distinct check is reached once, so none can be dropped silently.
    assert_eq!(seen.len(), 13, "{seen:?}");
}

/// The variant with its check-identifying fields, ignoring timestamps and
/// identities that vary per fixture.
fn same_check(reason: &GrantRejection) -> String {
    match reason {
        GrantRejection::BadHandle => "BadHandle".into(),
        GrantRejection::Unreadable => "Unreadable".into(),
        GrantRejection::Malformed => "Malformed".into(),
        GrantRejection::Revoked { .. } => "Revoked".into(),
        GrantRejection::Expired { .. } => "Expired".into(),
        GrantRejection::SubjectMismatch { grant_subject, .. } => {
            format!("SubjectMismatch({grant_subject})")
        }
        GrantRejection::RosterDenied { seat, .. } => format!("RosterDenied({seat})"),
        GrantRejection::Parent(inner) => format!("Parent({})", same_check(inner)),
        GrantRejection::ParentPrincipalMismatch => "ParentPrincipalMismatch".into(),
        GrantRejection::ParentNotDelegating { parent_seat } => {
            format!("ParentNotDelegating({parent_seat})")
        }
    }
}

/// AC4 + C2: an expired grant says so, by an 8-character prefix only.
#[test]
fn an_expired_grant_reads_as_expired_not_roleless() {
    let (id, check) = verdict(|root, me| {
        let id = mint_grant_for(root, me, "advisor", &[]);
        edit(&id, |g| g.expires_at = past());
        id
    });
    let sentence = check.rejection_sentence().expect("rejected grant");
    assert!(sentence.contains(&id[..8]), "{sentence}");
    assert!(!sentence.contains(&id), "full handle leaked: {sentence}");
    assert!(
        sentence.contains("(advisor)") && sentence.contains("expired at"),
        "{sentence}"
    );
    assert!(sentence.contains("aida role enter advisor"), "{sentence}");
    assert!(!sentence.contains("No active role"), "{sentence}");
    assert!(GrantCheck::Absent.rejection_sentence().is_none());
}

/// D3: `review record` refuses a rejected grant with the same sentence.
#[test]
fn review_record_refuses_a_rejected_grant() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let me = crate::current_user_id(None);
    let id = mint_grant_for(root, &me, "reviewer", &[]);
    edit(&id, |g| g.revoked_at = Some(past()));
    let _env = crate::test_env::EnvVarsGuard::apply(&[(GRANT_ENV, Some(id.as_str()))]);
    let err = crate::handle_review_record_at(
        root.to_path_buf(),
        "BUG-1",
        "approved",
        Some("0123456789abcdef0123456789abcdef01234567"),
        Some("some-branch"),
        None,
        &[],
        &[],
        None,
    )
    .expect_err("a rejected grant must not record a verdict");
    let message = format!("{err:#}");
    assert!(message.starts_with("refused: review record."), "{message}");
    assert!(
        message.contains(&id[..8]) && !message.contains(&id),
        "{message}"
    );
    assert!(message.contains("revoked at"), "{message}");
}
