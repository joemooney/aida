//! TASK-3820 (STORY-2828 S2b): `aida mandate renew` extends the operator's
//! covered grants in place. The relay double-extension regression lives
//! beside the relay fixture in `gate/relay_parent.rs`.
// trace:TASK-3820 | ai:claude
use super::*;

const PRINCIPAL: &str = crate::gate::test_support::HERMETIC_TEST_USER;

fn child_of(root: &Path, parent: &str, seat: &str, delegable: &[&str]) -> String {
    let id = test_support::mint_grant_for(root, PRINCIPAL, seat, delegable);
    let parent = read_grant(parent).unwrap();
    test_support::rewrite_grant(&id, |g| {
        g.parent_grant_id = Some(parent.id.clone());
        g.expires_at = parent.expires_at;
    });
    id
}

fn row<'a>(report: &'a GrantExtensionReport, id: &str) -> &'a GrantExtension {
    report
        .grants
        .iter()
        .find(|g| g.id == id)
        .unwrap_or_else(|| panic!("{id} missing from {}", report.text()))
}

fn bytes(id: &str) -> Vec<u8> {
    std::fs::read(grant_path(id).unwrap()).unwrap()
}

#[test]
fn task_3820_extends_chain_to_root_deadline_within_cap_and_keeps_identity() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _env = crate::gate::test_support::AmbientGuard::hermetic_with_seat(root, "advisor", &[]);
    let now = Utc::now();
    let advisor = test_support::mint_grant_for(root, PRINCIPAL, "advisor", &["reviewer"]);
    test_support::rewrite_grant(&advisor, |g| g.expires_at = now + Duration::hours(1));
    let reviewer = child_of(root, &advisor, "reviewer", &["implementer"]);
    let worker = child_of(root, &reviewer, "implementer", &[]);
    // A launcher capped this child below its parent (BUG-2486).
    let capped = child_of(root, &advisor, "reviewer", &[]);
    test_support::rewrite_grant(&capped, |g| g.expires_at = now + Duration::minutes(20));
    // The seat cap `aida role enter` enforces here: 12h.
    std::fs::write(
        root.join(".aida/config.toml"),
        "store_path = \".aida-store\"\n[seat]\nmax_grant_hours = 12\n",
    )
    .unwrap();
    let before: Vec<SeatGrant> = [&advisor, &reviewer, &worker]
        .iter()
        .map(|id| read_grant(id).unwrap())
        .collect();
    let report = extend_grants(root, root, PRINCIPAL, now + Duration::hours(24), now).unwrap();
    let deadline = now + Duration::hours(12);
    for old in &before {
        let r = row(&report, &old.id);
        assert_eq!(r.outcome, ExtensionOutcome::Extended, "{}", report.text());
        assert_eq!(r.previous_expires_at, old.expires_at);
        assert_eq!(r.expires_at, deadline);
        let new = read_grant(&old.id).unwrap();
        assert_eq!(new.expires_at, deadline);
        // Only the expiry moves: id, binding, subject, delegation, parent.
        let mut same = old.clone();
        same.expires_at = deadline;
        assert_eq!(
            serde_json::to_value(&same).unwrap(),
            serde_json::to_value(&new).unwrap()
        );
        assert!(matches!(
            grant_check_identity_at(root, &old.id, PRINCIPAL, now, true),
            GrantCheck::Valid(_)
        ));
    }
    let r = row(&report, &capped);
    assert_eq!(r.outcome, ExtensionOutcome::Kept);
    assert_eq!(
        read_grant(&capped).unwrap().expires_at,
        now + Duration::minutes(20)
    );
    // The hermetic grant store is shared with sibling tests: judge only ours.
    let ours = [&advisor, &reviewer, &worker, &capped];
    assert!(
        !report
            .refused()
            .iter()
            .any(|g| ours.iter().any(|id| **id == g.id)),
        "{}",
        report.text()
    );
    assert_eq!(report.effective_cap_hours, 12);
    // Text and JSON carry the same rows.
    let json = serde_json::to_value(&report).unwrap();
    let text = report.text();
    for (g, j) in report.grants.iter().zip(json["grants"].as_array().unwrap()) {
        assert_eq!(j["id"], g.id);
        assert_eq!(j["outcome"], g.outcome.as_str());
        assert_eq!(j["expires_at"], serde_json::json!(g.expires_at));
        let line = text
            .lines()
            .find(|l| l.split_whitespace().nth(1) == Some(g.id.as_str()))
            .unwrap();
        assert!(line.trim_start().starts_with(g.outcome.as_str()), "{line}");
        assert!(line.contains(&g.expires_at.to_rfc3339()), "{line}");
        assert!(line.contains(&format!("role={}", g.role)), "{line}");
    }
    // A shorter renewal never shortens anything.
    let snapshot: Vec<_> = [&advisor, &reviewer, &worker]
        .iter()
        .map(|id| bytes(id))
        .collect();
    let report = extend_grants(root, root, PRINCIPAL, now + Duration::hours(2), now).unwrap();
    for (id, was) in [&advisor, &reviewer, &worker].iter().zip(snapshot) {
        assert_eq!(row(&report, id).outcome, ExtensionOutcome::Kept);
        assert_eq!(bytes(id), was);
    }
}

#[test]
fn task_3820_descendant_never_outlives_its_root_and_follows_its_refusal() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _env = crate::gate::test_support::AmbientGuard::hermetic_with_seat(root, "advisor", &[]);
    let now = Utc::now();
    let target = now + Duration::hours(6);
    // Root and child go to the renewed mandate's deadline, not to the seat cap.
    let advisor = test_support::mint_grant_for(root, PRINCIPAL, "advisor", &["reviewer"]);
    test_support::rewrite_grant(&advisor, |g| g.expires_at = now + Duration::hours(1));
    let reviewer = child_of(root, &advisor, "reviewer", &[]);
    // A root that already outlives the target is kept; its child is not
    // lengthened past the target either.
    let long = test_support::mint_grant_for(root, PRINCIPAL, "advisor", &["reviewer"]);
    test_support::rewrite_grant(&long, |g| g.expires_at = now + Duration::hours(20));
    let long_child = child_of(root, &long, "reviewer", &[]);
    // A root that is refused takes its descendants with it.
    let unbound = test_support::mint_grant_for(root, PRINCIPAL, "advisor", &["reviewer"]);
    test_support::rewrite_grant(&unbound, |g| {
        g.binding.clear();
        g.expires_at = now + Duration::hours(1);
    });
    let unbound_child = child_of(root, &unbound, "reviewer", &[]);
    let report = extend_grants(root, root, PRINCIPAL, target, now).unwrap();
    for id in [&advisor, &reviewer] {
        assert_eq!(row(&report, id).outcome, ExtensionOutcome::Extended);
        assert_eq!(read_grant(id).unwrap().expires_at, target);
    }
    for id in [&long, &long_child] {
        assert_eq!(row(&report, id).outcome, ExtensionOutcome::Kept);
        assert_eq!(
            read_grant(id).unwrap().expires_at,
            now + Duration::hours(20)
        );
    }
    let r = row(&report, &unbound_child);
    assert_eq!(r.outcome, ExtensionOutcome::Refused);
    assert!(
        r.reason.as_deref().unwrap().contains(&unbound),
        "{:?}",
        r.reason
    );
    assert_eq!(
        read_grant(&unbound_child).unwrap().expires_at,
        now + Duration::hours(1)
    );
}

#[test]
fn task_3820_never_revives_and_names_refused_grants() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _env = crate::gate::test_support::AmbientGuard::hermetic_with_seat(root, "advisor", &[]);
    let now = Utc::now();
    let expired = test_support::mint_grant_for(root, PRINCIPAL, "advisor", &["reviewer"]);
    test_support::rewrite_grant(&expired, |g| g.expires_at = now - Duration::seconds(1));
    let under_expired = child_of(root, &expired, "reviewer", &[]);
    test_support::rewrite_grant(&under_expired, |g| g.expires_at = now + Duration::hours(1));
    let revoked = test_support::mint_grant_for(root, PRINCIPAL, "advisor", &["reviewer"]);
    test_support::rewrite_grant(&revoked, |g| g.revoked_at = Some(now));
    let under_revoked = child_of(root, &revoked, "reviewer", &[]);
    let unbound = test_support::mint_grant_for(root, PRINCIPAL, "advisor", &[]);
    test_support::rewrite_grant(&unbound, |g| g.binding.clear());
    let orphan = test_support::mint_grant_for(root, PRINCIPAL, "reviewer", &[]);
    test_support::rewrite_grant(&orphan, |g| {
        g.parent_grant_id = Some(Uuid::new_v4().to_string())
    });
    let inline = test_support::mint_grant_for(root, PRINCIPAL, "advisor", &["reviewer"]);
    test_support::rewrite_grant(&inline, |g| g.origin = Some(INLINE_ORIGIN.into()));
    let under_inline = child_of(root, &inline, "reviewer", &[]);
    let foreign = test_support::mint_grant_for(root, "another-operator", "advisor", &[]);
    let untouched: Vec<_> = [
        &expired,
        &under_expired,
        &revoked,
        &under_revoked,
        &unbound,
        &orphan,
        &inline,
        &under_inline,
        &foreign,
    ]
    .iter()
    .map(|id| (id.to_string(), bytes(id)))
    .collect();
    let report = extend_grants(root, root, PRINCIPAL, now + Duration::hours(8), now).unwrap();
    for id in [&expired, &under_expired, &revoked, &under_revoked] {
        let r = row(&report, id);
        assert_eq!(r.outcome, ExtensionOutcome::Skipped, "{}", report.text());
        assert!(r.reason.as_deref().unwrap().contains("never revived"));
    }
    for id in [&unbound, &orphan] {
        assert_eq!(row(&report, id).outcome, ExtensionOutcome::Refused);
    }
    // The hermetic grant store is shared with sibling tests: judge only ours.
    let refused: Vec<&str> = report
        .refused()
        .iter()
        .map(|g| g.id.as_str())
        .filter(|id| untouched.iter().any(|(ours, _)| ours == id))
        .collect();
    assert_eq!(refused.len(), 2, "{}", report.text());
    assert!(refused.contains(&unbound.as_str()) && refused.contains(&orphan.as_str()));
    // Inline agent-new chains and other principals are outside the coverage.
    for id in [&inline, &under_inline, &foreign] {
        assert!(!report.grants.iter().any(|g| &g.id == id));
    }
    for (id, was) in untouched {
        assert_eq!(bytes(&id), was, "{id} was rewritten");
    }
}

#[test]
fn task_3820_concurrent_revoke_under_the_record_lock_wins() {
    use fs2::FileExt;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _env = crate::gate::test_support::AmbientGuard::hermetic_with_seat(root, "advisor", &[]);
    let now = Utc::now();
    let id = test_support::mint_grant_for(root, PRINCIPAL, "advisor", &[]);
    test_support::rewrite_grant(&id, |g| g.expires_at = now + Duration::hours(1));
    let locks = grant_lock_dir().unwrap();
    std::fs::create_dir_all(&locks).unwrap();
    let lock_path = locks.join(format!("{id}.lock"));
    let record_path = grant_path(&id).unwrap();
    let mut revoked = read_grant(&id).unwrap();
    revoked.revoked_at = Some(now);
    let revoked_bytes = serde_json::to_vec_pretty(&revoked).unwrap();
    let (held, wait) = std::sync::mpsc::channel();
    // A revoke that takes the record lock after the scan, before the write.
    let revoker = std::thread::spawn(move || {
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .unwrap();
        lock.lock_exclusive().unwrap();
        held.send(()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        std::fs::write(&record_path, revoked_bytes).unwrap();
        drop(lock);
    });
    wait.recv().unwrap();
    let report = extend_grants(root, root, PRINCIPAL, now + Duration::hours(8), now).unwrap();
    revoker.join().unwrap();
    let r = row(&report, &id);
    assert_eq!(r.outcome, ExtensionOutcome::Refused, "{}", report.text());
    assert!(
        r.reason.as_deref().unwrap().contains("revoked"),
        "{:?}",
        r.reason
    );
    let after = read_grant(&id).unwrap();
    assert!(after.revoked_at.is_some());
    assert_eq!(after.expires_at, now + Duration::hours(1));
}

#[test]
fn task_3820_production_entry_requires_a_direct_human() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Hermetic ambient: no interactive terminal.
    let _env = crate::gate::test_support::AmbientGuard::hermetic_with_seat(root, "advisor", &[]);
    let id = current_grant_id().unwrap();
    let was = bytes(&id);
    let refused =
        extend_operator_grants_as_human(root, root, Utc::now() + Duration::hours(8)).unwrap_err();
    assert!(refused.to_string().contains("TTY"), "{refused}");
    assert_eq!(bytes(&id), was);
}
