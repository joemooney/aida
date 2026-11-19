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
    let cap = (now + Duration::minutes(20)).timestamp();
    cap_launch_child_expiry(&capped, cap).unwrap();
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
    assert_eq!(read_grant(&capped).unwrap().expires_at.timestamp(), cap);
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

/// Run `hook` just before the locked re-check of grant `target` (every
/// other grant passes straight through), until the guard drops.
// trace:TASK-3820 | ai:claude
pub(crate) struct LockedRenewalHook;

impl LockedRenewalHook {
    pub(crate) fn on(target: &str, mut hook: impl FnMut() -> Result<()> + 'static) -> Self {
        let target = target.to_string();
        BEFORE_LOCKED_RENEWAL.with(|h| {
            *h.borrow_mut() = Some(Box::new(
                move |id: &str| {
                    if id == target {
                        hook()
                    } else {
                        Ok(())
                    }
                },
            ))
        });
        Self
    }
}

impl Drop for LockedRenewalHook {
    fn drop(&mut self) {
        BEFORE_LOCKED_RENEWAL.with(|h| *h.borrow_mut() = None);
    }
}

#[test]
fn task_3820_child_missed_by_a_partial_run_catches_up_but_a_launcher_cap_holds() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _env = crate::gate::test_support::AmbientGuard::hermetic_with_seat(root, "advisor", &[]);
    let now = Utc::now();
    let advisor = test_support::mint_grant_for(root, PRINCIPAL, "advisor", &["reviewer"]);
    test_support::rewrite_grant(&advisor, |g| g.expires_at = now + Duration::hours(1));
    let child = child_of(root, &advisor, "reviewer", &[]);
    let capped = child_of(root, &advisor, "reviewer", &[]);
    let cap = (now + Duration::minutes(20)).timestamp();
    cap_launch_child_expiry(&capped, cap).unwrap();
    let target = now + Duration::hours(6);
    // Pass 1: the parent is extended, the child's write fails transiently.
    let report = {
        let _fail = LockedRenewalHook::on(&child, || {
            anyhow::bail!("injected: the record lock could not be taken")
        });
        extend_grants(root, root, PRINCIPAL, target, now).unwrap()
    };
    assert_eq!(row(&report, &advisor).outcome, ExtensionOutcome::Extended);
    assert_eq!(row(&report, &child).outcome, ExtensionOutcome::Refused);
    assert_eq!(read_grant(&advisor).unwrap().expires_at, target);
    assert_eq!(
        read_grant(&child).unwrap().expires_at,
        now + Duration::hours(1)
    );
    // Pass 2, the failure cleared: the child, now below its parent without
    // a launcher cap, catches up; the capped child keeps its cap.
    let report = extend_grants(root, root, PRINCIPAL, target, now).unwrap();
    let r = row(&report, &child);
    assert_eq!(r.outcome, ExtensionOutcome::Extended, "{}", report.text());
    assert_eq!(read_grant(&child).unwrap().expires_at, target);
    let r = row(&report, &capped);
    assert_eq!(r.outcome, ExtensionOutcome::Kept, "{}", report.text());
    assert!(r
        .reason
        .as_deref()
        .unwrap()
        .contains("capped by its launcher"));
    let after = read_grant(&capped).unwrap();
    assert_eq!(after.expires_at.timestamp(), cap);
    assert_eq!(after.launch_expiry_cap.map(|c| c.timestamp()), Some(cap));
}

#[test]
fn task_3820_launcher_cap_landing_before_the_locked_write_survives() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _env = crate::gate::test_support::AmbientGuard::hermetic_with_seat(root, "advisor", &[]);
    let now = Utc::now();
    let advisor = test_support::mint_grant_for(root, PRINCIPAL, "advisor", &["reviewer"]);
    test_support::rewrite_grant(&advisor, |g| g.expires_at = now + Duration::hours(1));
    let child = child_of(root, &advisor, "reviewer", &[]);
    let cap = (now + Duration::minutes(20)).timestamp();
    // The scan saw an ordinary child; the ChatGPT credential cap lands
    // after it, before the renewal takes the child's record lock.
    let report = {
        let id = child.clone();
        let _cap = LockedRenewalHook::on(&child, move || cap_launch_child_expiry(&id, cap));
        extend_grants(root, root, PRINCIPAL, now + Duration::hours(6), now).unwrap()
    };
    assert_eq!(row(&report, &advisor).outcome, ExtensionOutcome::Extended);
    let r = row(&report, &child);
    assert_eq!(r.outcome, ExtensionOutcome::Kept, "{}", report.text());
    assert_eq!(r.expires_at.timestamp(), cap);
    assert_eq!(read_grant(&child).unwrap().expires_at.timestamp(), cap);
}

#[test]
fn task_3820_ancestor_revoked_before_the_locked_write_refuses_the_grandchild() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _env = crate::gate::test_support::AmbientGuard::hermetic_with_seat(root, "advisor", &[]);
    let now = Utc::now();
    let advisor = test_support::mint_grant_for(root, PRINCIPAL, "advisor", &["reviewer"]);
    test_support::rewrite_grant(&advisor, |g| g.expires_at = now + Duration::hours(1));
    let reviewer = child_of(root, &advisor, "reviewer", &["implementer"]);
    let worker = child_of(root, &reviewer, "implementer", &[]);
    // The grandchild passed every check on the scan; the root is revoked
    // before its locked write. Its immediate parent is still live.
    let report = {
        let id = advisor.clone();
        let _revoke = LockedRenewalHook::on(&worker, move || {
            anyhow::ensure!(revoke_by_id(&id)?, "root was not revoked");
            Ok(())
        });
        extend_grants(root, root, PRINCIPAL, now + Duration::hours(6), now).unwrap()
    };
    assert_eq!(row(&report, &reviewer).outcome, ExtensionOutcome::Extended);
    let r = row(&report, &worker);
    assert_eq!(r.outcome, ExtensionOutcome::Refused, "{}", report.text());
    assert!(
        r.reason.as_deref().unwrap().contains("while renewing"),
        "{:?}",
        r.reason
    );
    assert_eq!(
        read_grant(&worker).unwrap().expires_at,
        now + Duration::hours(1)
    );
}

#[test]
fn task_3820_ample_credential_deadline_is_recorded_and_bounds_renewal() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _env = crate::gate::test_support::AmbientGuard::hermetic_with_seat(root, "advisor", &[]);
    let now = Utc::now();
    let advisor = test_support::mint_grant_for(root, PRINCIPAL, "advisor", &["reviewer"]);
    test_support::rewrite_grant(&advisor, |g| g.expires_at = now + Duration::hours(1));
    let child = child_of(root, &advisor, "reviewer", &[]);
    // The credential outlives the child: nothing to shorten, but the
    // deadline is still recorded.
    let deadline = (now + Duration::hours(2)).timestamp();
    cap_launch_child_expiry(&child, deadline).unwrap();
    let capped = read_grant(&child).unwrap();
    assert_eq!(capped.expires_at, now + Duration::hours(1));
    assert_eq!(
        capped.launch_expiry_cap.map(|c| c.timestamp()),
        Some(deadline)
    );
    let report = extend_grants(root, root, PRINCIPAL, now + Duration::hours(8), now).unwrap();
    assert_eq!(row(&report, &advisor).outcome, ExtensionOutcome::Extended);
    let r = row(&report, &child);
    assert_eq!(r.outcome, ExtensionOutcome::Extended, "{}", report.text());
    assert_eq!(r.expires_at.timestamp(), deadline);
    assert_eq!(read_grant(&child).unwrap().expires_at.timestamp(), deadline);
    // A second renewal keeps it at the deadline.
    let report = extend_grants(root, root, PRINCIPAL, now + Duration::hours(8), now).unwrap();
    let r = row(&report, &child);
    assert_eq!(r.outcome, ExtensionOutcome::Kept, "{}", report.text());
    assert!(r
        .reason
        .as_deref()
        .unwrap()
        .contains("capped by its launcher"));
    assert_eq!(read_grant(&child).unwrap().expires_at.timestamp(), deadline);
}

#[test]
fn task_3820_child_expiring_behind_its_parent_lock_is_never_revived() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _env = crate::gate::test_support::AmbientGuard::hermetic_with_seat(root, "advisor", &[]);
    let now = Utc::now();
    let advisor = test_support::mint_grant_for(root, PRINCIPAL, "advisor", &["reviewer"]);
    test_support::rewrite_grant(&advisor, |g| g.expires_at = now + Duration::hours(1));
    let child = child_of(root, &advisor, "reviewer", &[]);
    let expiry = Utc::now() + Duration::milliseconds(1500);
    test_support::rewrite_grant(&child, |g| g.expires_at = expiry);
    let was = bytes(&child);
    // Just before the child's locked re-check, another writer takes the
    // parent's record lock and holds it until the child has expired (well
    // inside the bounded wait), after the parent was already extended.
    let holder = std::sync::Arc::new(std::sync::Mutex::new(None));
    let report = {
        let (parent, holder) = (advisor.clone(), holder.clone());
        let _hold = LockedRenewalHook::on(&child, move || {
            let (tx, rx) = std::sync::mpsc::channel();
            let parent = parent.clone();
            *holder.lock().unwrap() = Some(std::thread::spawn(move || {
                let lock = lock_grant_record_bounded(&parent).unwrap();
                tx.send(()).unwrap();
                let wait = (expiry - Utc::now()).num_milliseconds().max(0) as u64 + 200;
                std::thread::sleep(std::time::Duration::from_millis(wait));
                drop(lock);
            }));
            rx.recv().unwrap();
            Ok(())
        });
        extend_grants(root, root, PRINCIPAL, now + Duration::hours(6), now).unwrap()
    };
    holder.lock().unwrap().take().unwrap().join().unwrap();
    assert_eq!(row(&report, &advisor).outcome, ExtensionOutcome::Extended);
    let r = row(&report, &child);
    assert_eq!(r.outcome, ExtensionOutcome::Refused, "{}", report.text());
    let why = r.reason.as_deref().unwrap();
    assert!(
        why.contains("expired") && why.contains("while renewing"),
        "{why}"
    );
    assert_eq!(bytes(&child), was);
}

/// An earlier binary's launcher capped a delegated child by shortening its
/// expiry alone: no recorded cap, no provenance marker. Renewal must never
/// read that as "uncapped" and stretch it to the renewed root's deadline.
// trace:TASK-3820 | ai:claude
#[test]
fn task_3820_legacy_capped_child_without_cap_provenance_is_never_extended() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _env = crate::gate::test_support::AmbientGuard::hermetic_with_seat(root, "advisor", &[]);
    let now = Utc::now();
    let advisor = test_support::mint_grant_for(root, PRINCIPAL, "advisor", &["reviewer"]);
    let parent = test_support::rewrite_grant(&advisor, |g| g.expires_at = now + Duration::hours(4));
    // The record exactly as the previous binary wrote it: the old member set,
    // its expiry shortened to a 30m credential deadline by the old launcher.
    let legacy = Uuid::new_v4().to_string();
    let deadline = now + Duration::minutes(30);
    let old_format = serde_json::json!({
        "id": legacy,
        "principal": PRINCIPAL,
        "subject": PRINCIPAL,
        "session_id": Uuid::new_v4().to_string(),
        "seat": "reviewer",
        "tty_issued_at": parent.tty_issued_at,
        "delegable_seats": [],
        "parent_grant_id": advisor,
        "issued_at": now,
        "expires_at": deadline,
        "revoked_at": null,
        "binding": own_process_token().into_iter().collect::<Vec<_>>(),
    });
    let path = grant_path(&legacy).unwrap();
    std::fs::write(&path, serde_json::to_vec_pretty(&old_format).unwrap()).unwrap();
    let parsed = read_grant(&legacy).unwrap();
    assert!(parsed.launch_expiry_cap.is_none() && !parsed.launch_cap_recorded);
    let was = bytes(&legacy);
    let report = extend_grants(root, root, PRINCIPAL, now + Duration::hours(8), now).unwrap();
    assert_eq!(row(&report, &advisor).outcome, ExtensionOutcome::Extended);
    let r = row(&report, &legacy);
    assert_eq!(r.outcome, ExtensionOutcome::Refused, "{}", report.text());
    assert!(
        r.reason.as_deref().unwrap().contains("relaunch"),
        "{:?}",
        r.reason
    );
    assert_eq!(r.expires_at, deadline);
    assert_eq!(bytes(&legacy), was);
    assert_eq!(read_grant(&legacy).unwrap().expires_at, deadline);
}

/// The renewal re-checks provenance under the record lock: an earlier
/// binary rewriting the record after the scan drops the marker, and wins.
// trace:TASK-3820 | ai:claude
#[test]
fn task_3820_marker_dropped_before_the_locked_write_refuses() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _env = crate::gate::test_support::AmbientGuard::hermetic_with_seat(root, "advisor", &[]);
    let now = Utc::now();
    let advisor = test_support::mint_grant_for(root, PRINCIPAL, "advisor", &["reviewer"]);
    test_support::rewrite_grant(&advisor, |g| g.expires_at = now + Duration::hours(1));
    let child = child_of(root, &advisor, "reviewer", &[]);
    let id = child.clone();
    let report = {
        let _rewrite = LockedRenewalHook::on(&child, move || {
            test_support::rewrite_grant(&id, |g| g.launch_cap_recorded = false);
            Ok(())
        });
        extend_grants(root, root, PRINCIPAL, now + Duration::hours(6), now).unwrap()
    };
    let r = row(&report, &child);
    assert_eq!(r.outcome, ExtensionOutcome::Refused, "{}", report.text());
    assert_eq!(
        read_grant(&child).unwrap().expires_at,
        now + Duration::hours(1)
    );
}
