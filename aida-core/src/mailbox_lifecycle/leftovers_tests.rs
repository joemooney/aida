//! Fixtures for mailbox tombs, creator staging and stale nonce-era markers.
//! trace:TASK-b6b1278890774ef993d2ebc327862470-052 | ai:claude

use super::leftovers::{reap_stale_staging, scan_at, Leftovers, STALE_AFTER};
use super::tests::{repo, source};
use super::*;
use std::time::SystemTime;

fn later() -> SystemTime {
    SystemTime::now() + STALE_AFTER + Duration::from_secs(60)
}

/// A directory holding a creation nonce and an empty subdirectory, plus
/// `extra` files.
fn tomb_like(path: &Path, extra: &[&str]) {
    std::fs::create_dir_all(path.join("inbox")).unwrap();
    std::fs::write(path.join(SOURCE_ID_FILE), "0".repeat(32)).unwrap();
    for name in extra {
        std::fs::write(path.join(name), "{}").unwrap();
    }
}

#[test]
fn stale_creator_staging_is_reaped_and_young_staging_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let aida = dir.path().join(".aida");
    let staging = aida.join(".mailbox.creating.0123");
    std::fs::create_dir_all(&staging).unwrap();
    std::fs::write(staging.join(SOURCE_ID_FILE), "0".repeat(32)).unwrap();
    reap_stale_staging(&aida, "mailbox", SystemTime::now());
    assert!(staging.is_dir(), "a creator may still be placing it");
    reap_stale_staging(&aida, "mailbox", later());
    assert!(!staging.exists());
}

#[test]
fn staging_holding_anything_but_its_nonce_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let aida = dir.path().join(".aida");
    let staging = aida.join(".mailbox.creating.0123");
    tomb_like(&staging, &["m.json"]);
    reap_stale_staging(&aida, "mailbox", later());
    assert!(staging.join("m.json").is_file());
}

#[test]
fn empty_tombs_are_reaped_and_tombs_holding_mail_only_reported() {
    let dir = repo();
    let root = dir.path();
    let aida = root.join(".aida");
    let empty = aida.join(".mailbox-tomb-aaaa");
    let full = aida.join(".mailbox-tomb-bbbb");
    tomb_like(&empty, &[]);
    tomb_like(&full, &["late.json"]);

    let report = scan_at(root, false, later());
    assert_eq!(
        report,
        Leftovers {
            stranded_tombs: vec![full.clone()],
            reapable: vec![empty.clone()],
        }
    );
    assert!(empty.is_dir(), "a report removes nothing");

    let young = scan_at(root, true, SystemTime::now());
    assert!(young.reapable.is_empty() && empty.is_dir(), "{young:?}");

    let reaped = scan_at(root, true, later());
    assert_eq!(reaped.reapable, [empty.clone()]);
    assert!(!empty.exists());
    assert!(full.join("late.json").is_file(), "mail is never reaped");
    assert_eq!(reaped.stranded_tombs, [full]);
}

#[test]
fn a_marker_whose_admin_entry_was_pruned_is_reaped() {
    let dir = repo();
    let anchor = source(dir.path()).anchor;
    let era = anchor.join(NONCE_ERA_DIR);
    ensure_dir(&era, true).unwrap();
    let pruned = era.join("pruned");
    let record = EraMarker {
        admin: "gone".into(),
        admin_dir: dir.path().join(".git").join("worktrees").join("gone"),
        dev: 1,
        ino: 2,
    };
    std::fs::write(&pruned, serde_json::to_vec(&record).unwrap()).unwrap();
    // A pre-record marker cannot be judged without its checkout: kept.
    let bare = era.join("bare");
    std::fs::write(&bare, "gone").unwrap();
    for path in [&pruned, &bare] {
        std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o600))
            .unwrap();
    }

    assert_eq!(
        scan_at(dir.path(), false, later()).reapable,
        [pruned.clone()]
    );
    assert!(pruned.is_file());
    scan_at(dir.path(), true, SystemTime::now());
    assert!(!pruned.exists());
    assert!(bare.is_file());
}

/// The retention dry run carries the leftovers scan.
// trace:TASK-b6b1278890774ef993d2ebc327862470-059 | ai:claude
#[test]
fn the_retention_dry_run_reports_stranded_tombs() {
    let dir = repo();
    let full = dir.path().join(".aida").join(".mailbox-tomb-cccc");
    tomb_like(&full, &["late.json"]);
    let report = super::activation::dry_run_report(dir.path());
    assert_eq!(report.leftovers.stranded_tombs, [full]);
}

/// A drop aside an earlier build left is touched only once old: a stale one
/// is removed, a fresh one is linked back when its marker is gone, and is
/// removed beside a marker that stands.
// trace:TASK-b6b1278890774ef993d2ebc327862470-067 | ai:claude
#[test]
fn old_drop_asides_are_settled_and_young_ones_left() {
    let dir = repo();
    let anchor = source(dir.path()).anchor;
    let era = anchor.join(NONCE_ERA_DIR);
    ensure_dir(&era, true).unwrap();
    let git = std::fs::canonicalize(dir.path().join(".git")).unwrap();
    let meta = std::fs::metadata(&git).unwrap();
    let fresh = serde_json::to_vec(&EraMarker {
        admin: String::new(),
        admin_dir: git.clone(),
        dev: meta.dev(),
        ino: meta.ino(),
    })
    .unwrap();
    let stale = serde_json::to_vec(&EraMarker {
        admin: "gone".into(),
        admin_dir: git.join("worktrees").join("gone"),
        dev: 1,
        ino: 2,
    })
    .unwrap();
    let put = |name: &str, bytes: &[u8]| {
        let path = era.join(name);
        std::fs::write(&path, bytes).unwrap();
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600))
            .unwrap();
        path
    };
    let stale_aside = put(".s.drop.1", &stale);
    let lost_aside = put(".l.drop.2", &fresh);
    let kept_aside = put(".k.drop.3", &fresh);
    let kept = put("k", b"{}");

    let young = scan_at(dir.path(), true, SystemTime::now());
    assert!(young.reapable.is_empty(), "{young:?}");
    assert!(stale_aside.is_file() && lost_aside.is_file() && kept_aside.is_file());

    let mut reaped = scan_at(dir.path(), true, later()).reapable;
    reaped.sort();
    let mut want = vec![kept_aside.clone(), lost_aside.clone(), stale_aside.clone()];
    want.sort();
    assert_eq!(reaped, want);
    assert!(!stale_aside.exists() && !lost_aside.exists() && !kept_aside.exists());
    assert!(!era.join("s").exists(), "a stale aside is never restored");
    assert_eq!(
        std::fs::read(era.join("l")).unwrap(),
        fresh,
        "a fresh aside whose marker was lost is linked back"
    );
    assert_eq!(
        std::fs::read(&kept).unwrap(),
        b"{}",
        "a standing marker stays"
    );
}
