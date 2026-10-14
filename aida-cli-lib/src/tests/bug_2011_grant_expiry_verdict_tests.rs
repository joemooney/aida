//! The expiry launcher must reject exactly the shared verdict's rejection
//! fixtures even when their expiry is well outside the refusal window.
// trace:BUG-2011 | ai:codex
use super::*;
use clap::Parser;

const CHILD_ROOT: &str = "AIDA_TEST_ROLE_EXPIRY_ROOT";
const CHILD_NAME: &str =
    "seat_authority::bug_2011_grant_expiry_verdict_tests::role_expiry_cli_child";

// This driver is confined to this CLI probe. Parent requires an entry artifact
// and the actual handler's exit code; a missing filter cannot count as success.
// trace:BUG-2011 | ai:codex
#[test]
fn role_expiry_cli_child() {
    let Some(root) = std::env::var_os(CHILD_ROOT) else {
        return;
    };
    let root = PathBuf::from(root);
    crate::test_ambient::replace(Some(crate::test_ambient::Ambient {
        project_root: root.clone(),
        stdin_is_terminal: false,
        stdout_is_terminal: false,
    }));
    let cli =
        crate::Cli::try_parse_from(["aida", "role", "expiry", "--refuse-within", "1h"]).unwrap();
    let crate::Command::Role(command) = cli.command else {
        panic!("role command parsed");
    };
    std::fs::write(root.join("entered-expiry-handler"), "entered").unwrap();
    crate::role_cmd::handle_role_command(&command).unwrap();
    std::fs::write(root.join("allowed-expiry-handler"), "allowed").unwrap();
}

#[derive(Debug, Clone, Copy)]
enum Case {
    Valid,
    ValidBaseline,
    SubjectMismatch,
    RosterDenied,
    ParentPrincipalMismatch,
    ParentNotDelegating,
    PaddedHandle,
    Revoked,
    Expired,
    Missing,
    Malformed,
    ParentRevoked,
    ParentExpired,
}

fn edit_record(id: &str, change: impl FnOnce(&mut SeatGrant)) {
    let mut grant = read_grant(id).unwrap();
    change(&mut grant);
    write_grant(&grant).unwrap();
}

// trace:BUG-2011 | ai:codex
fn fixture(root: &Path, me: &str, case: Case) -> String {
    let delegated = matches!(
        case,
        Case::ParentPrincipalMismatch
            | Case::ParentNotDelegating
            | Case::ParentRevoked
            | Case::ParentExpired
    );
    let seat = if delegated {
        "reviewer"
    } else if matches!(case, Case::ValidBaseline) {
        "implementer"
    } else {
        "advisor"
    };
    let child = test_support::mint_grant_for(root, me, seat, &[]);
    edit_record(&child, |grant| {
        grant.expires_at = Utc::now() + Duration::hours(4)
    });
    if delegated {
        let principal = if matches!(case, Case::ParentPrincipalMismatch) {
            "another-principal"
        } else {
            me
        };
        let parent = test_support::mint_grant_for(root, principal, "advisor", &["reviewer"]);
        edit_record(&child, |grant| grant.parent_grant_id = Some(parent.clone()));
        edit_record(&parent, |grant| match case {
            Case::ParentNotDelegating => grant.delegable_seats.clear(),
            Case::ParentRevoked => grant.revoked_at = Some(Utc::now()),
            Case::ParentExpired => grant.expires_at = Utc::now() - Duration::hours(1),
            _ => {}
        });
    }
    match case {
        Case::SubjectMismatch => edit_record(&child, |g| g.subject = "another-subject".into()),
        Case::RosterDenied => {
            let store = crate::detect_distributed_store_from(root).unwrap();
            std::fs::write(
                store.join("registry/team.toml"),
                format!("[members]\n\"{me}\" = []\n"),
            )
            .unwrap();
        }
        Case::PaddedHandle => return format!(" {child} "),
        Case::Revoked => edit_record(&child, |g| g.revoked_at = Some(Utc::now())),
        Case::Expired => edit_record(&child, |g| g.expires_at = Utc::now() - Duration::hours(1)),
        Case::Missing => return Uuid::new_v4().to_string(),
        Case::Malformed => std::fs::write(grant_path(&child).unwrap(), "not json").unwrap(),
        _ => {}
    }
    child
}

fn matches_case(check: &GrantCheck, case: Case) -> bool {
    match (case, check) {
        (Case::Valid | Case::ValidBaseline, GrantCheck::Valid(_)) => true,
        (
            Case::SubjectMismatch,
            GrantCheck::Rejected {
                reason: GrantRejection::SubjectMismatch { .. },
                ..
            },
        ) => true,
        (
            Case::RosterDenied,
            GrantCheck::Rejected {
                reason: GrantRejection::RosterDenied { .. },
                ..
            },
        ) => true,
        (
            Case::ParentPrincipalMismatch,
            GrantCheck::Rejected {
                reason: GrantRejection::ParentPrincipalMismatch,
                ..
            },
        ) => true,
        (
            Case::ParentNotDelegating,
            GrantCheck::Rejected {
                reason: GrantRejection::ParentNotDelegating { .. },
                ..
            },
        ) => true,
        (
            Case::PaddedHandle,
            GrantCheck::Rejected {
                reason: GrantRejection::BadHandle,
                ..
            },
        ) => true,
        (
            Case::Revoked,
            GrantCheck::Rejected {
                reason: GrantRejection::Revoked { .. },
                ..
            },
        ) => true,
        (
            Case::Expired,
            GrantCheck::Rejected {
                reason: GrantRejection::Expired { .. },
                ..
            },
        ) => true,
        (
            Case::Missing,
            GrantCheck::Rejected {
                reason: GrantRejection::Unreadable,
                ..
            },
        ) => true,
        (
            Case::Malformed,
            GrantCheck::Rejected {
                reason: GrantRejection::Malformed,
                ..
            },
        ) => true,
        (
            Case::ParentRevoked,
            GrantCheck::Rejected {
                reason: GrantRejection::Parent(inner),
                ..
            },
        ) => matches!(inner.as_ref(), GrantRejection::Revoked { .. }),
        (
            Case::ParentExpired,
            GrantCheck::Rejected {
                reason: GrantRejection::Parent(inner),
                ..
            },
        ) => matches!(inner.as_ref(), GrantRejection::Expired { .. }),
        _ => false,
    }
}

// trace:BUG-2011 | ai:codex
#[test]
fn role_expiry_refuse_within_1h_matches_each_grant_rejection_fixture() {
    for case in [
        Case::Valid,
        Case::ValidBaseline,
        Case::SubjectMismatch,
        Case::RosterDenied,
        Case::ParentPrincipalMismatch,
        Case::ParentNotDelegating,
        Case::PaddedHandle,
        Case::Revoked,
        Case::Expired,
        Case::Missing,
        Case::Malformed,
        Case::ParentRevoked,
        Case::ParentExpired,
    ] {
        let root = tempfile::tempdir().unwrap();
        let mut env = crate::test_env::EnvVarsGuard::snapshot(&[GRANT_ENV, "AIDA_USER"]);
        env.set_key("AIDA_USER", "expiry-fixture-user");
        let id = fixture(root.path(), "expiry-fixture-user", case);
        env.set_key(GRANT_ENV, &id);
        let check = grant_check(root.path());
        assert!(
            matches_case(&check, case),
            "fixture {case:?} was not live: {check:?}"
        );
        let rejected = matches!(check, GrantCheck::Rejected { .. });
        let reason = match &check {
            GrantCheck::Rejected {
                reason: GrantRejection::Revoked { .. },
                ..
            } => "session grant was revoked".to_string(),
            GrantCheck::Rejected { reason, .. } => reason.to_string(),
            _ => String::new(),
        };
        // Invalid canonical YAML makes a requirements-store load observable.
        let poisoned = crate::detect_distributed_store_from(root.path())
            .unwrap()
            .join(aida_core::object_store::relative_object_path("TASK-9999").unwrap());
        std::fs::create_dir_all(poisoned.parent().unwrap()).unwrap();
        std::fs::write(poisoned, "spec_id: [").unwrap();
        let output = std::process::Command::new(crate::aida_bin::running_executable_fallback())
            .args(["--exact", "--nocapture", "--test-threads=1", CHILD_NAME])
            .env(CHILD_ROOT, root.path())
            .env(GRANT_ENV, &id)
            .env("AIDA_USER", "expiry-fixture-user")
            .env("AIDA_SESSION_ROLE", "advisor")
            .output()
            .unwrap();
        assert!(
            root.path().join("entered-expiry-handler").exists(),
            "child did not execute the handler for {case:?}"
        );
        assert_eq!(
            output.status.code(),
            Some(if rejected { 3 } else { 0 }),
            "{case:?}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            root.path().join("allowed-expiry-handler").exists(),
            !rejected
        );
        if rejected {
            let error = String::from_utf8_lossy(&output.stderr);
            assert!(error.contains("Not launching"), "{case:?}: {error}");
            assert!(
                error.contains(&reason),
                "{case:?}: expected {reason}, got {error}"
            );
            assert!(error.contains("aida role enter"), "{case:?}: {error}");
        } else {
            assert!(String::from_utf8_lossy(&output.stdout)
                .contains("test result: ok. 1 passed; 0 failed;"));
        }
        assert!(
            !root.path().join(".aida/cache.db").exists(),
            "expiry must not load the requirements cache"
        );
    }
}

// trace:BUG-2011 | ai:codex
#[test]
fn validated_lookup_uses_the_earlier_parent_expiry() {
    let root = tempfile::tempdir().unwrap();
    let mut env = crate::test_env::EnvVarsGuard::snapshot(&[GRANT_ENV, "AIDA_USER"]);
    let subject = "parent-expiry-fixture";
    env.set_key("AIDA_USER", subject);
    let parent = test_support::mint_grant_for(root.path(), subject, "advisor", &["reviewer"]);
    let child = test_support::mint_grant_for(root.path(), subject, "reviewer", &[]);
    let expiry = Utc::now() + Duration::minutes(45);
    edit_record(&parent, |g| g.expires_at = expiry);
    edit_record(&child, |g| {
        g.parent_grant_id = Some(parent.clone());
        g.expires_at = expiry + Duration::hours(3);
    });
    env.set_key(GRANT_ENV, &child);
    let previous = crate::test_ambient::replace(Some(crate::test_ambient::Ambient {
        project_root: root.path().to_path_buf(),
        stdin_is_terminal: false,
        stdout_is_terminal: false,
    }));
    let lookup = session_grant_lookup();
    crate::test_ambient::replace(previous);
    assert_eq!(
        lookup,
        GrantLookup::Expiry(GrantExpiry {
            seat: "reviewer".into(),
            expires_at: expiry
        })
    );
}

// Every Valid verdict supplies expiry metadata, including an explicitly exported
// baseline grant. The display-only baseline behavior remains unchanged.
// trace:BUG-2011 | ai:codex
#[test]
fn valid_baseline_lookup_supplies_its_expiry() {
    let root = tempfile::tempdir().unwrap();
    let mut env = crate::test_env::EnvVarsGuard::snapshot(&[GRANT_ENV, "AIDA_USER"]);
    env.set_key("AIDA_USER", "baseline-expiry-fixture");
    let id =
        test_support::mint_grant_for(root.path(), "baseline-expiry-fixture", "implementer", &[]);
    let expiry = read_grant(&id).unwrap().expires_at;
    env.set_key(GRANT_ENV, &id);
    let previous = crate::test_ambient::replace(Some(crate::test_ambient::Ambient {
        project_root: root.path().to_path_buf(),
        stdin_is_terminal: false,
        stdout_is_terminal: false,
    }));
    let lookup = session_grant_lookup();
    crate::test_ambient::replace(previous);
    assert_eq!(
        lookup,
        GrantLookup::Expiry(GrantExpiry {
            seat: "implementer".into(),
            expires_at: expiry
        })
    );
    assert_eq!(
        session_grant_expiry(),
        None,
        "informational baseline remains silent"
    );
}
