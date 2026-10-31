use super::*;
use crate::test_env::EnvVarsGuard;
use std::ffi::OsString;

/// BUG-2125: the fixed identity every grant in this module is bound to.
/// These tests used to read `current_user_id(None)` and the grant store
/// (`$AIDA_HOME`) outside the env lock, between short-lived env guards, so
/// a sibling test setting `AIDA_USER` could make the minted subject differ
/// from the caller the later validation read, and the grant failed its
/// subject check. Each grant-store test now holds ONE env guard for its
/// whole body with `AIDA_USER` pinned here; per-step env changes go
/// through [`Scoped`] under that same acquisition.
// trace:BUG-2125 | ai:claude
const SUBJECT: &str = "seat-authority-fixture-user";

/// Hold the env lock for the whole test with `AIDA_USER` = [`SUBJECT`].
// trace:BUG-2125 | ai:claude
fn pinned_env() -> EnvVarsGuard {
    let mut env = EnvVarsGuard::snapshot(&["AIDA_USER"]);
    env.set_key("AIDA_USER", SUBJECT);
    env
}

/// Apply `pairs` for this value's lifetime under the test's already-held
/// [`pinned_env`] guard (the env lock is not reentrant), restoring each
/// key's prior value on drop.
// trace:BUG-2125 | ai:claude
struct Scoped<'a> {
    env: &'a mut EnvVarsGuard,
    prev: Vec<(&'static str, Option<OsString>)>,
}

impl<'a> Scoped<'a> {
    fn apply(env: &'a mut EnvVarsGuard, pairs: &[(&'static str, Option<&str>)]) -> Self {
        let mut prev = Vec::with_capacity(pairs.len());
        for (key, value) in pairs {
            prev.push((*key, std::env::var_os(key)));
            match value {
                Some(v) => env.set_key(key, v),
                None => env.unset_key(key),
            }
        }
        Self { env, prev }
    }
}

impl Drop for Scoped<'_> {
    fn drop(&mut self) {
        for (key, prev) in self.prev.iter().rev() {
            match prev {
                Some(v) => self.env.set_key(key, v),
                None => self.env.unset_key(key),
            }
        }
    }
}

// A record can change between the validator and informational metadata read.
// Reject either inconsistency; matching metadata still emits an expired notice.
// trace:BUG-2014 | ai:codex
#[test]
fn expired_notice_rejects_changed_timestamp_or_seat() {
    let at = Utc::now();
    let record = SeatGrant {
        id: "fixture".into(),
        principal: "notice-user".into(),
        subject: "notice-user".into(),
        session_id: "fixture-session".into(),
        seat: "advisor".into(),
        tty_issued_at: at,
        delegable_seats: Vec::new(),
        parent_grant_id: None,
        issued_at: at,
        expires_at: at,
        revoked_at: None,
        binding: Vec::new(),
    };
    for (expires_at, seat, emits) in [
        (at, "advisor", true),
        (at + Duration::minutes(1), "advisor", false),
        (at, "reviewer", false),
    ] {
        let mut reread = record.clone();
        reread.expires_at = expires_at;
        reread.seat = seat.into();
        let notice = notice_grant_from_check(
            GrantCheck::Rejected {
                id: record.id.clone(),
                seat: Some(record.seat.clone()),
                reason: GrantRejection::Expired { at },
            },
            |_| Ok(reread.clone()),
        )
        .unwrap();
        assert_eq!(
            notice.is_some(),
            emits,
            "timestamp={expires_at}, seat={seat}"
        );
    }
}

/// TASK-2256 (advisor comment 35): the release `handle_queue_work` runs
/// after a refused reviewer child grant deletes the minted PR-N lease and
/// its companions, and keeps an implementer's lease, or one whose launch
/// failed for any other reason.
// trace:TASK-1955 | ai:claude (TASK-2256)
#[test]
fn a_refused_reviewer_child_grant_releases_only_the_review_lease() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let sessions = crate::leases::leases_dir(root);
    std::fs::create_dir_all(&sessions).unwrap();
    let mint = |id: &str| {
        let lease = crate::leases::lease_path(root, id);
        std::fs::write(&lease, format!("id = \"{id}\"\nscope = \"PR-42\"\n")).unwrap();
        std::fs::write(crate::leases::session_activity_path(root, id), "").unwrap();
        lease
    };
    let refused: anyhow::Result<()> = Err(anyhow::Error::new(ChildGrantRefused(anyhow::anyhow!(
        "seat `reviewer` is not delegable"
    )))
    .context("launching the reviewer"));

    let review = mint("leasereview01");
    assert!(crate::queue_cmd::release_refused_review_lease(
        root,
        true,
        Some("leasereview01"),
        &refused
    ));
    assert!(!review.exists(), "the PR-N lease is released");
    assert!(!crate::leases::session_activity_path(root, "leasereview01").exists());

    let implementer = mint("leaseimpl0001");
    assert!(!crate::queue_cmd::release_refused_review_lease(
        root,
        false,
        Some("leaseimpl0001"),
        &refused
    ));
    assert!(implementer.exists(), "an implementer lease stays");

    let other = mint("leaseother001");
    let failed: anyhow::Result<()> = Err(anyhow::anyhow!("reviewer exited 1"));
    assert!(!crate::queue_cmd::release_refused_review_lease(
        root,
        true,
        Some("leaseother001"),
        &failed
    ));
    assert!(other.exists(), "any other failure keeps the lease");
    assert!(!crate::queue_cmd::release_refused_review_lease(
        root,
        true,
        Some("leaseother001"),
        &Ok(())
    ));
    assert!(other.exists());
}

/// ADR-66 acceptance: an env-only `AIDA_SESSION_ROLE=advisor` confers no
/// seat; a validated grant does; revocation and a shrunk roster ceiling
/// fail closed.
#[test]
fn env_role_alone_confers_no_seat_and_grants_fail_closed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut env = pinned_env(); // trace:BUG-2125 | ai:claude

    // Env-only role, no grant: no seat.
    {
        let _env = Scoped::apply(
            &mut env,
            &[("AIDA_SESSION_ROLE", Some("advisor")), (GRANT_ENV, None)],
        );
        assert_eq!(current_seat(root), None, "env role alone must not seat");
    }

    // trace:TASK-1594 | ai:claude
    let id = test_support::mint_grant_for(root, SUBJECT, "advisor", &["implementer"]);

    // A validated grant seats the session.
    {
        let _env = Scoped::apply(&mut env, &[(GRANT_ENV, Some(id.as_str()))]);
        assert_eq!(current_seat(root).as_deref(), Some("advisor"));

        // trace:TASK-1337 | ai:codex
        let before = std::fs::read_dir(grant_dir().unwrap()).unwrap().count();
        validate_child_delegation(root, "implementer").unwrap();
        validate_child_delegation(root, "reviewer")
            .expect_err("preflight must preserve delegation gate");
        assert_eq!(
            std::fs::read_dir(grant_dir().unwrap()).unwrap().count(),
            before,
            "read-only validation must not issue a child grant"
        );
        // The grant delegates only its explicit subset.
        let child = issue_child(root, "implementer", "child-subject")
            .expect("delegable child seat is issuable");
        assert_eq!(child.seat, "implementer");
        assert_eq!(child.parent_grant_id.as_deref(), Some(id.as_str()));
        assert!(
            child.delegable_seats.is_empty(),
            "children default to an empty delegation set"
        );
        let refused = issue_child(root, "reviewer", "child-subject")
            .expect_err("a seat outside the delegation set must refuse");
        // Advisor comment 35 (TASK-1955): the refusal is typed, so a
        // launcher can release the lease it took, and reads unchanged.
        assert!(ChildGrantRefused::is_in(&refused), "{refused:?}");
        assert!(
            crate::queue_cmd::refused_review_launch_releases_lease(true, &refused),
            "a refused reviewer child releases its PR-N lease"
        );
        assert!(
            !crate::queue_cmd::refused_review_launch_releases_lease(false, &refused),
            "an implementer lease stays for the manual continuation"
        );
        assert!(
            !crate::queue_cmd::refused_review_launch_releases_lease(
                true,
                &anyhow::anyhow!("reviewer exited 1")
            ),
            "any other failure keeps the lease"
        );
        assert_eq!(
            format!("{refused:#}"),
            format!(
                "{:#}",
                validate_child_delegation(root, "reviewer").unwrap_err()
            )
        );

        // Revocation fails closed.
        assert!(revoke_current().unwrap());
        assert_eq!(current_seat(root), None, "revoked grant must not seat");
    }

    // A fresh grant dies when the roster ceiling no longer allows it.
    let id = test_support::mint_grant_for(root, SUBJECT, "reviewer", &[]);
    {
        let _env = Scoped::apply(&mut env, &[(GRANT_ENV, Some(id.as_str()))]);
        assert_eq!(current_seat(root).as_deref(), Some("reviewer"));
        let store = crate::detect_distributed_store_from(root).unwrap();
        std::fs::write(store.join("registry").join("team.toml"), "[members]\n").unwrap();
        assert_eq!(
            current_seat(root),
            None,
            "a shrunk roster ceiling invalidates the grant"
        );
    }
}

/// BUG-2139: a fresh headless child never inherits the launcher's grant.
/// A delegated seat mints a distinct child grant (routed seat, no
/// delegable seats, parent = launcher); a baseline seat the launcher
/// cannot delegate falls back to an ungranted child; every other seat
/// fails closed. The child grant is revocable by id.
// trace:BUG-2139 | ai:claude
#[test]
fn headless_child_gets_its_own_grant_or_baseline_or_refusal() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut env = pinned_env();
    // Use a short parent lifetime so a child defaulting to MAX_GRANT_AGE
    // cannot accidentally satisfy the expiry assertion.
    // trace:TASK-1789 | ai:codex
    let advisor = test_support::mint_grant_for_with_ttl(
        root,
        SUBJECT,
        "advisor",
        &["implementer", "reviewer"],
        Some(Duration::minutes(7)),
    );
    let parent_expiry = read_grant(&advisor).unwrap().expires_at;
    {
        let _env = Scoped::apply(&mut env, &[(GRANT_ENV, Some(advisor.as_str()))]);
        for seat in ["implementer", "reviewer"] {
            assert_eq!(
                preflight_headless_child(root, seat).unwrap(),
                HeadlessChildAuthority::Delegated
            );
            let HeadlessChildGrant::Issued(child) =
                issue_headless_child(root, seat, SUBJECT, false).unwrap()
            else {
                panic!("a delegated {seat} seat must mint a child grant");
            };
            assert_ne!(
                child.id, advisor,
                "the child never reuses the launcher grant"
            );
            assert_eq!(child.seat, seat);
            assert!(
                child.delegable_seats.is_empty(),
                "the child delegates nothing"
            );
            assert_eq!(child.parent_grant_id.as_deref(), Some(advisor.as_str()));
            // trace:TASK-1789 | ai:codex
            assert_eq!(child.expires_at, parent_expiry, "{seat} child expiry");
            assert_eq!(read_grant(&child.id).unwrap().expires_at, parent_expiry);
            // Revocation by id ends exactly the child, never the launcher.
            assert!(revoke_by_id(&child.id).unwrap());
            assert!(!revoke_by_id(&child.id).unwrap(), "idempotent");
            assert!(read_grant(&child.id).unwrap().revoked_at.is_some());
            assert!(read_grant(&advisor).unwrap().revoked_at.is_none());
        }
    }
    // A launcher whose grant does not delegate the seat.
    let narrow = test_support::mint_grant_for(root, SUBJECT, "advisor", &["product"]);
    for grant in [Some(narrow.as_str()), None] {
        let _env = Scoped::apply(&mut env, &[(GRANT_ENV, grant)]);
        // Baseline seats fall back to an ungranted child...
        for seat in BASELINE_SEATS {
            let got = issue_headless_child(root, seat, SUBJECT, false).unwrap();
            assert!(
                matches!(got, HeadlessChildGrant::Baseline { .. }),
                "{seat} with grant {grant:?}: {got:?}"
            );
            assert_eq!(got.env_value(), "", "a baseline child's grant env is empty");
            assert_eq!(got.id(), None);
        }
        // ...every other seat is refused.
        for seat in ["reviewer", "advisor", "integrator", "product"] {
            if grant.is_some() && seat == "product" {
                continue; // `product` IS delegated by the narrow grant.
            }
            issue_headless_child(root, seat, SUBJECT, false)
                .expect_err("a non-baseline seat must fail closed");
            preflight_headless_child(root, seat).expect_err("the preflight agrees with issuance");
        }
    }
}

/// BUG-2139: the baseline seat list is the one `classify_role_display`
/// reads, so the statusline and the headless fallback cannot drift.
// trace:BUG-2139 | ai:claude
#[test]
fn baseline_seats_match_role_display() {
    for seat in BASELINE_SEATS {
        assert!(is_baseline_seat(seat));
        assert_eq!(
            classify_role_display(Some(seat), None),
            RoleDisplay::Held((*seat).to_string())
        );
    }
    for seat in ["reviewer", "advisor", "integrator", "product"] {
        assert!(!is_baseline_seat(seat));
    }
}

/// A role label with no valid grant reads as ungranted; a validated grant
/// reads as held; the implementer baseline needs no grant.
// trace:BUG-1924 | ai:claude
#[test]
fn role_display_follows_the_grant_not_the_label() {
    assert_eq!(
        classify_role_display(Some("advisor"), None),
        RoleDisplay::Ungranted("advisor".into())
    );
    assert_eq!(
        classify_role_display(Some("dialog"), None),
        RoleDisplay::Ungranted("advisor".into()),
        "the deprecated alias canonicalizes before display"
    );
    assert_eq!(
        classify_role_display(Some("advisor"), Some("advisor")),
        RoleDisplay::Held("advisor".into())
    );
    assert_eq!(
        classify_role_display(Some("advisor"), Some("reviewer")),
        RoleDisplay::Held("reviewer".into()),
        "the grant, not the label, names the displayed seat"
    );
    assert_eq!(
        classify_role_display(None, Some("reviewer")),
        RoleDisplay::Held("reviewer".into())
    );
    assert_eq!(
        classify_role_display(Some("implementer"), None),
        RoleDisplay::Held("implementer".into())
    );
    assert_eq!(classify_role_display(None, None), RoleDisplay::Default);
    assert_eq!(
        classify_role_display(Some("  "), None),
        RoleDisplay::Default
    );
}

/// End to end through the grant store: an env-only advisor label is
/// ungranted, a minted advisor grant is held, and revocation drops it
/// back to ungranted.
// trace:BUG-1924 | ai:claude
#[test]
fn role_display_validates_the_grant_store() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut env = pinned_env(); // trace:BUG-2125 | ai:claude
    {
        let _env = Scoped::apply(
            &mut env,
            &[("AIDA_SESSION_ROLE", Some("advisor")), (GRANT_ENV, None)],
        );
        assert_eq!(
            role_display(root, Some("advisor")),
            RoleDisplay::Ungranted("advisor".into())
        );
    }
    let id = test_support::mint_grant_for(root, SUBJECT, "advisor", &[]);
    {
        let _env = Scoped::apply(
            &mut env,
            &[
                ("AIDA_SESSION_ROLE", Some("advisor")),
                (GRANT_ENV, Some(id.as_str())),
            ],
        );
        assert_eq!(
            role_display(root, Some("advisor")),
            RoleDisplay::Held("advisor".into())
        );
        assert!(revoke_current().unwrap());
        assert_eq!(
            role_display(root, Some("advisor")),
            RoleDisplay::Ungranted("advisor".into()),
            "a revoked grant reads as ungranted"
        );
    }
}

fn expiry_in(seat: &str, left: Duration) -> (GrantExpiry, DateTime<Utc>) {
    let now = Utc::now();
    (
        GrantExpiry {
            seat: seat.to_string(),
            expires_at: now + left,
        },
        now,
    )
}

/// A grant near expiry warns and refuses a launch; one with time to
/// spare does neither; an expired one says it expired.
// trace:TASK-1736 | ai:claude
#[test]
fn expiry_messages_follow_the_time_left() {
    let (fresh, now) = expiry_in("advisor", Duration::hours(3));
    assert_eq!(expiry_warning(&fresh, now, EXPIRY_WARN_WITHIN), None);
    assert_eq!(expiry_suffix(&fresh, now, EXPIRY_WARN_WITHIN), None);
    assert_eq!(launch_refusal(&fresh, now, Duration::hours(1)), None);

    let (soon, now) = expiry_in("advisor", Duration::minutes(90));
    let warning = expiry_warning(&soon, now, EXPIRY_WARN_WITHIN).expect("warns inside 2h");
    assert!(warning.contains("expires at"), "{warning}");
    assert!(warning.contains("(in 1h30m)"), "{warning}");
    assert!(
        warning.contains("eval \"$(aida role enter advisor)\""),
        "{warning}"
    );
    assert_eq!(
        expiry_suffix(&soon, now, EXPIRY_WARN_WITHIN).as_deref(),
        Some("expires in 1h30m")
    );
    assert_eq!(
        launch_refusal(&soon, now, Duration::hours(1)),
        None,
        "90m left is enough to launch"
    );

    let (close, now) = expiry_in("reviewer", Duration::minutes(30));
    let refusal = launch_refusal(&close, now, Duration::hours(1)).expect("refuses under 1h");
    assert!(refusal.starts_with("Not launching"), "{refusal}");
    assert!(refusal.contains("in 30m"), "{refusal}");
    assert!(refusal.contains("aida role enter reviewer"), "{refusal}");

    let (gone, now) = expiry_in("advisor", -Duration::minutes(5));
    let warning = expiry_warning(&gone, now, EXPIRY_WARN_WITHIN).unwrap();
    assert!(warning.contains("expired at"), "{warning}");
    assert_eq!(
        expiry_suffix(&gone, now, EXPIRY_WARN_WITHIN).as_deref(),
        Some("grant expired")
    );
    assert!(launch_refusal(&gone, now, Duration::hours(1))
        .unwrap()
        .contains("expired at"));
}

/// A session needs at least the requested window, including exact equality.
// trace:TASK-1372 | ai:codex
#[test]
fn expiry_launch_refusal_allows_exact_window_boundary() {
    let within = Duration::hours(1);
    let (expiry, now) = expiry_in("advisor", within);
    assert_eq!(launch_refusal(&expiry, now, within), None);
    assert_eq!(
        launch_refusal(&expiry, now - Duration::nanoseconds(1), within),
        None,
    );
    assert!(launch_refusal(&expiry, now + Duration::nanoseconds(1), within).is_some());
}

/// A set handle naming no usable record is `Unresolved`, never silently
/// `NoGrant`; the launch refusal names the renew command.
// trace:TASK-1736 | ai:claude
#[test]
fn unresolvable_grant_is_distinguished_from_no_grant() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut env = pinned_env(); // trace:BUG-2125 | ai:claude
    {
        let _env = Scoped::apply(&mut env, &[(GRANT_ENV, Some("  "))]);
        assert_eq!(session_grant_lookup(), GrantLookup::NoGrant);
    }
    let missing = Uuid::new_v4().to_string();
    for handle in [missing.as_str(), "not-a-uuid"] {
        let _env = Scoped::apply(&mut env, &[(GRANT_ENV, Some(handle))]);
        assert!(
            matches!(session_grant_lookup(), GrantLookup::Unresolved(_)),
            "{handle}"
        );
    }
    let id = test_support::mint_grant_for(root, SUBJECT, "advisor", &[]);
    let mut grant = read_grant(&id).unwrap();
    grant.revoked_at = Some(Utc::now());
    write_grant(&grant).unwrap();
    {
        let _env = Scoped::apply(&mut env, &[(GRANT_ENV, Some(id.as_str()))]);
        assert_eq!(
            session_grant_lookup(),
            GrantLookup::Unresolved("session grant was revoked".into())
        );
    }
    let refusal = unresolved_launch_refusal("session grant was revoked", Some("Advisor"));
    assert!(refusal.starts_with("Not launching"), "{refusal}");
    assert!(refusal.contains("aida role enter advisor"), "{refusal}");
    assert!(unresolved_launch_refusal("x", None).contains("aida role enter <seat>"));
}

// trace:TASK-1736 | ai:claude
#[test]
fn format_remaining_is_compact() {
    assert_eq!(format_remaining(Duration::seconds(20)), "under 1m");
    assert_eq!(format_remaining(Duration::minutes(42)), "42m");
    assert_eq!(format_remaining(Duration::minutes(65)), "1h05m");
    assert_eq!(format_remaining(Duration::hours(2)), "2h00m");
}

/// The expiry comes from the raw record, so an expired grant still
/// reports; a child grant dies with its parent; a revoked grant and the
/// implementer baseline report nothing.
// trace:TASK-1736 | ai:claude
#[test]
fn session_grant_expiry_reads_the_record_and_its_parent() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut env = pinned_env(); // trace:BUG-2125 | ai:claude
    let rewrite = |id: &str, edit: &dyn Fn(&mut SeatGrant)| {
        let mut grant = read_grant(id).unwrap();
        edit(&mut grant);
        write_grant(&grant).unwrap();
        grant
    };

    {
        let _env = Scoped::apply(&mut env, &[(GRANT_ENV, None)]);
        assert_eq!(session_grant_expiry(), None, "no grant, nothing to report");
    }

    let id = test_support::mint_grant_for(root, SUBJECT, "advisor", &["reviewer"]);
    let soon = Utc::now() + Duration::minutes(30);
    rewrite(&id, &|g| g.expires_at = soon);
    {
        let _env = Scoped::apply(&mut env, &[(GRANT_ENV, Some(id.as_str()))]);
        assert_eq!(
            session_grant_expiry(),
            Some(GrantExpiry {
                seat: "advisor".into(),
                expires_at: soon,
            })
        );
    }

    // A child carrying a later expiry than its parent still dies with it.
    let parent = rewrite(&id, &|g| g.expires_at = soon);
    let mut kid = parent.clone();
    kid.id = Uuid::new_v4().to_string();
    kid.seat = "reviewer".into();
    kid.parent_grant_id = Some(parent.id.clone());
    kid.expires_at = soon + Duration::hours(10);
    write_grant(&kid).unwrap();
    {
        let _env = Scoped::apply(&mut env, &[(GRANT_ENV, Some(kid.id.as_str()))]);
        assert_eq!(session_grant_expiry().map(|e| e.expires_at), Some(soon));
    }

    // Already expired: current_grant drops it, but the expiry still reports.
    let past = Utc::now() - Duration::minutes(5);
    rewrite(&id, &|g| g.expires_at = past);
    {
        let _env = Scoped::apply(&mut env, &[(GRANT_ENV, Some(id.as_str()))]);
        assert!(current_grant(root).is_none());
        assert_eq!(session_grant_expiry().map(|e| e.expires_at), Some(past));
    }

    rewrite(&id, &|g| g.revoked_at = Some(Utc::now()));
    {
        let _env = Scoped::apply(&mut env, &[(GRANT_ENV, Some(id.as_str()))]);
        assert_eq!(
            session_grant_expiry(),
            None,
            "a revoked grant reports nothing"
        );
    }

    let implementer = test_support::mint_grant_for(root, SUBJECT, "implementer", &[]);
    rewrite(&implementer, &|g| g.expires_at = soon);
    {
        let _env = Scoped::apply(&mut env, &[(GRANT_ENV, Some(implementer.as_str()))]);
        assert_eq!(
            session_grant_expiry(),
            None,
            "the implementer baseline needs no grant, so its expiry costs nothing"
        );
    }
}
