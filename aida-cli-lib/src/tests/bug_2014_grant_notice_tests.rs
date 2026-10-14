// trace:BUG-2014 | ai:codex
use super::*;
use crate::seat_authority::{self, SeatGrant, GRANT_ENV};

fn record(id: &str) -> SeatGrant {
    serde_json::from_slice(
        &std::fs::read(
            seat_authority::grant_dir()
                .unwrap()
                .join(format!("{id}.json")),
        )
        .unwrap(),
    )
    .unwrap()
}
fn change(id: &str, f: impl FnOnce(&mut SeatGrant)) {
    let mut grant = record(id);
    f(&mut grant);
    std::fs::write(
        seat_authority::grant_dir()
            .unwrap()
            .join(format!("{id}.json")),
        serde_json::to_vec(&grant).unwrap(),
    )
    .unwrap();
}

#[test]
fn deterministic_crossings_renewal_and_poisoned_store() {
    let root = tempfile::tempdir().unwrap();
    let mut env =
        crate::test_env::EnvVarsGuard::snapshot(&[GRANT_ENV, "AIDA_USER", "AIDA_SESSION_ROLE"]);
    env.set_key("AIDA_USER", "notice-user");
    env.set_key("AIDA_SESSION_ROLE", "product");
    let id =
        seat_authority::test_support::mint_grant_for(root.path(), "notice-user", "advisor", &[]);
    env.set_key(GRANT_ENV, &id);
    let now = Utc::now();
    let at = now + Duration::hours(3);
    change(&id, |grant| grant.expires_at = at);
    // Any graph load fails; no cache is allowed to appear.
    let store = crate::detect_distributed_store_from(root.path()).unwrap();
    let poison = store.join(aida_core::object_store::relative_object_path("TASK-9999").unwrap());
    std::fs::create_dir_all(poison.parent().unwrap()).unwrap();
    std::fs::write(&poison, "spec_id: [").unwrap();
    assert!(notice(root.path(), now).unwrap().is_none());
    let mut seen = Vec::new();
    for left in [90, 90, 20, 20, 0, -1] {
        if let Some(message) = notice(root.path(), at - Duration::minutes(left)).unwrap() {
            seen.push(message);
        }
    }
    assert_eq!(seen.len(), 3);
    assert!(seen[0].contains("advisor seat grant expires"));
    assert!(seen[1].contains("20m"));
    assert!(seen[2].contains("advisor seat grant expired"));
    assert!(seen[2].contains("aida role enter advisor"));
    assert!(!seen.iter().any(|message| message.contains("product")));
    assert!(!root.path().join(".aida/cache.db").exists());
    assert_eq!(std::fs::read_to_string(&poison).unwrap(), "spec_id: [");
    // Same-id renewal starts a new epoch; a newly minted id also starts fresh.
    change(&id, |grant| {
        grant.issued_at = at;
        grant.expires_at = at + Duration::minutes(90);
    });
    assert!(notice(root.path(), at).unwrap().is_some());
    assert!(notice(root.path(), at).unwrap().is_none());
    let renewed =
        seat_authority::test_support::mint_grant_for(root.path(), "notice-user", "advisor", &[]);
    change(&renewed, |grant| {
        grant.expires_at = at + Duration::minutes(90)
    });
    env.set_key(GRANT_ENV, &renewed);
    assert!(notice(root.path(), at).unwrap().is_some());
}

#[test]
fn child_effective_expiry_and_typed_rejections() {
    let root = tempfile::tempdir().unwrap();
    let mut env = crate::test_env::EnvVarsGuard::snapshot(&[GRANT_ENV, "AIDA_USER"]);
    env.set_key("AIDA_USER", "notice-child");
    let parent = seat_authority::test_support::mint_grant_for(
        root.path(),
        "notice-child",
        "advisor",
        &["reviewer"],
    );
    let child =
        seat_authority::test_support::mint_grant_for(root.path(), "notice-child", "reviewer", &[]);
    let now = Utc::now();
    change(&parent, |grant| {
        grant.expires_at = now + Duration::minutes(20)
    });
    change(&child, |grant| {
        grant.parent_grant_id = Some(parent.clone());
        grant.expires_at = now + Duration::hours(4);
    });
    env.set_key(GRANT_ENV, &child);
    let message = notice(root.path(), now).unwrap().unwrap();
    assert!(message.contains("reviewer seat grant expires"));
    assert!(message.contains("20m"));
    assert!(notice(root.path(), now + Duration::minutes(21))
        .unwrap()
        .unwrap()
        .contains("reviewer seat grant expired"));
    // Both records expired: retain the parent's earlier effective timestamp.
    let late = now + Duration::hours(5);
    let metadata = seat_authority::notice_grant(root.path(), late)
        .unwrap()
        .unwrap();
    assert_eq!(metadata.expiry.expires_at, now + Duration::minutes(20));
    assert!(notice(root.path(), late).unwrap().is_none());
    change(&child, |grant| grant.revoked_at = Some(now));
    assert!(notice(root.path(), now).unwrap().is_none());
    change(&child, |grant| {
        grant.revoked_at = None;
        grant.subject = "other".into();
    });
    assert!(notice(root.path(), now).unwrap().is_none());
    env.set_key(GRANT_ENV, "invalid");
    assert!(notice(root.path(), now).unwrap().is_none());
}

#[test]
fn concurrent_claims_emit_at_most_once_per_crossing() {
    let dir = tempfile::tempdir().unwrap();
    for crossing in 1..=3 {
        let outputs = std::thread::scope(|scope| {
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(16));
            let handles: Vec<_> = (0..16)
                .map(|_| {
                    let barrier = barrier.clone();
                    let dir = dir.path();
                    scope.spawn(move || {
                        barrier.wait();
                        claim(dir, "fixture", "epoch", crossing).unwrap()
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .filter(|emitted| *emitted)
                .count()
        });
        assert_eq!(outputs, 1);
        assert!(dir.path().join("fixture.notice.lock").exists());
    }
}

#[test]
fn corrupt_unreadable_and_unwritable_ledgers_emit_nothing() {
    // Forked suite children may retain the lock: contention and ledger errors
    // must both suppress a claim. Only Ok(true) violates the no-notice contract.
    // trace:BUG-2014 | ai:codex
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("fixture.notice.json"), b"{").unwrap();
    assert!(!matches!(
        claim(dir.path(), "fixture", "epoch", 1),
        Ok(true)
    ));
    std::fs::remove_file(dir.path().join("fixture.notice.json")).unwrap();
    std::fs::create_dir(dir.path().join("fixture.notice.json")).unwrap();
    assert!(!matches!(
        claim(dir.path(), "fixture", "epoch", 1),
        Ok(true)
    ));
    // ENOTDIR remains reliable even when CI runs as root.
    let blocked = dir.path().join("not-a-directory");
    std::fs::write(&blocked, "blocked").unwrap();
    assert!(!matches!(claim(&blocked, "fixture", "epoch", 1), Ok(true)));
}

// Exact boundary assertions kill <= to < mutations at each crossing.
// trace:BUG-2014 | ai:codex
#[test]
fn threshold_boundaries_are_inclusive() {
    for (left, expected) in [
        (Duration::hours(2) + Duration::milliseconds(1), 0),
        (Duration::hours(2), 1),
        (Duration::minutes(30) + Duration::milliseconds(1), 1),
        (Duration::minutes(30), 2),
        (Duration::milliseconds(1), 2),
        (Duration::zero(), 3),
        (Duration::milliseconds(-1), 3),
    ] {
        assert_eq!(threshold(left), expected, "remaining: {left}");
    }
}

// trace:BUG-2014 | ai:codex
#[test]
fn baseline_grants_at_twenty_minutes_emit_nothing() {
    let root = tempfile::tempdir().unwrap();
    let mut env = crate::test_env::EnvVarsGuard::snapshot(&[GRANT_ENV, "AIDA_USER"]);
    env.set_key("AIDA_USER", "notice-baseline");
    let now = Utc::now();
    for seat in ["implementer", "guest", "requester"] {
        let id =
            seat_authority::test_support::mint_grant_for(root.path(), "notice-baseline", seat, &[]);
        change(&id, |grant| grant.expires_at = now + Duration::minutes(20));
        env.set_key(GRANT_ENV, &id);
        assert!(matches!(
            seat_authority::grant_check_at(root.path(), now),
            seat_authority::GrantCheck::Valid(_)
        ));
        assert!(
            seat_authority::notice_grant(root.path(), now)
                .unwrap()
                .is_none(),
            "{seat}"
        );
        assert!(notice(root.path(), now).unwrap().is_none(), "{seat}");
    }
}
