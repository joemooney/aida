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
            corrupt: Vec::new(),
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

/// An aside of a pre-record marker (its body the bare admin name) is judged
/// against that admin name's entry: a lost fresh one is linked back, a
/// stale one removed.
// trace:TASK-b6b1278890774ef993d2ebc327862470-072 | ai:claude
#[test]
fn pre_record_drop_asides_are_judged_by_admin_name() {
    let dir = repo();
    let anchor = source(dir.path()).anchor;
    let era = anchor.join(NONCE_ERA_DIR);
    ensure_dir(&era, true).unwrap();
    let put = |name: &str, bytes: &[u8]| {
        let path = era.join(name);
        std::fs::write(&path, bytes).unwrap();
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600))
            .unwrap();
        path
    };
    // "wt" names a worktree admin entry that exists; "gone" names none.
    std::fs::create_dir_all(dir.path().join(".git").join("worktrees").join("wt")).unwrap();
    let lost = put(".m.drop.1", b"wt");
    let stale = put(".g.drop.2", b"gone");

    assert_eq!(
        scan_at(dir.path(), false, later()).reapable,
        [stale.clone(), lost.clone()]
    );
    scan_at(dir.path(), true, later());
    assert!(!lost.exists() && !stale.exists());
    assert_eq!(
        std::fs::read(era.join("m")).unwrap(),
        b"wt",
        "the lost pre-record marker is linked back"
    );
    assert!(!era.join("g").exists(), "a stale one is never restored");
}

/// A torn record aside (a partial JSON body) or any body that cannot be a
/// bare admin name is undecidable: reported as corrupt, never removed or
/// restored, however old.
// trace:TASK-b6b1278890774ef993d2ebc327862470-075 | ai:claude
#[test]
fn a_torn_era_marker_aside_is_reported_and_kept() {
    let dir = repo();
    let anchor = source(dir.path()).anchor;
    let era = anchor.join(NONCE_ERA_DIR);
    ensure_dir(&era, true).unwrap();
    let mut asides = Vec::new();
    for (i, body) in [&b"{\"admin\":\"wt\",\"admin_d"[..], b"", b"a/b", b"x\0y"]
        .into_iter()
        .enumerate()
    {
        let path = era.join(format!(".t{i}.drop.{i}"));
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600))
            .unwrap();
        asides.push(path);
    }
    let reaped = scan_at(dir.path(), true, later());
    assert_eq!(reaped.corrupt, asides);
    assert!(reaped.reapable.is_empty(), "{reaped:?}");
    for (i, aside) in asides.iter().enumerate() {
        assert!(aside.is_file(), "kept: {}", aside.display());
        assert!(!era.join(format!("t{i}")).exists(), "never restored");
    }
}

/// An era marker is written whole or not at all, create-new; a crashed
/// writer's temp file is reaped once old.
// trace:TASK-b6b1278890774ef993d2ebc327862470-075 | ai:claude
#[test]
fn era_marker_writes_are_atomic_and_temps_reaped() {
    let dir = repo();
    let anchor = source(dir.path()).anchor;
    let era = anchor.join(NONCE_ERA_DIR);
    ensure_dir(&era, true).unwrap();
    let marker = era.join("m");
    write_source_id(&marker, b"first").unwrap();
    let again = write_source_id(&marker, b"second").unwrap_err();
    assert_eq!(again.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(std::fs::read(&marker).unwrap(), b"first");
    let names = |d: &Path| -> Vec<String> {
        std::fs::read_dir(d)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(ID_TEMP_PREFIX))
            .collect()
    };
    assert!(names(&anchor).is_empty(), "no temp left behind");
    // Without a no-replace rename it is linked in instead.
    NOREPLACE_UNSUPPORTED.with(|f| f.set(true));
    let linked = write_source_id(&era.join("n"), b"linked");
    let refused = write_source_id(&marker, b"third");
    NOREPLACE_UNSUPPORTED.with(|f| f.set(false));
    linked.unwrap();
    assert_eq!(
        refused.unwrap_err().kind(),
        std::io::ErrorKind::AlreadyExists
    );
    assert_eq!(std::fs::read(era.join("n")).unwrap(), b"linked");
    assert_eq!(std::fs::metadata(era.join("n")).unwrap().nlink(), 1);
    assert!(names(&anchor).is_empty(), "no temp left behind");
    // A crashed writer's temp: kept while young, reaped once old.
    let temp = anchor.join(format!("{ID_TEMP_PREFIX}dead.tmp"));
    std::fs::write(&temp, b"{").unwrap();
    scan_at(dir.path(), true, SystemTime::now());
    assert!(temp.is_file());
    assert!(scan_at(dir.path(), true, later()).reapable.contains(&temp));
    assert!(!temp.exists());
}
