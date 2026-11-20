// trace:BUG-2669 | ai:claude
use super::*;
use std::os::unix::fs::PermissionsExt;

const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
const DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

struct Fixture {
    _tmp: tempfile::TempDir,
    env: crate::test_env::EnvVarsGuard,
    root: PathBuf,
    home: PathBuf,
    grant: String,
}

impl Fixture {
    /// A project and a hermetic home, with a launcher grant delegating
    /// `delegable`.
    fn new(delegable: &[&str]) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("project");
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let mut env = crate::test_env::EnvVarsGuard::apply(&[
            ("HOME", Some(home.to_str().unwrap())),
            ("AIDA_HOME", Some(home.to_str().unwrap())),
            ("AIDA_PROJECT_ROOT", Some(root.to_str().unwrap())),
            ("AIDA_DRIVE_ROOT", Some(root.to_str().unwrap())),
            ("AIDA_SESSION_GRANT", None),
            ("AIDA_CONTAINED", None),
            ("AIDA_STANDALONE_IMPLEMENTER", None),
            ("AIDA_SESSION_ID", None),
        ]);
        crate::test_git::commit_fixture(&root);
        let grant = sa::test_support::mint_grant_for(
            &root,
            &crate::current_user_id(None),
            "advisor",
            delegable,
        );
        env.set_key(sa::GRANT_ENV, &grant);
        Self {
            _tmp: tmp,
            env,
            root,
            home,
            grant,
        }
    }

    fn draft(&self) -> LaunchDraft {
        LaunchDraft::prepare(&self.root, 59, HEAD.into()).unwrap()
    }

    fn registered(&self) -> (LaunchDraft, Receipt) {
        let draft = self.draft();
        let receipt = register(&self.root, &self.root, &draft, "review-session")
            .unwrap()
            .expect("a launcher delegating reviewer registers a receipt");
        (draft, receipt)
    }

    fn file(&self, receipt: &Receipt) -> PathBuf {
        self.home
            .join(".aida/review-handoffs")
            .join(file_name(&receipt.launch).unwrap())
    }
}

#[test]
fn bug_2669_register_writes_a_private_exclusive_receipt_beside_the_grant_store() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let f = Fixture::new(&["reviewer"]);
    let (draft, receipt) = f.registered();
    let path = f.file(&receipt);
    assert_eq!(
        path.parent().unwrap().parent(),
        sa::grant_dir().unwrap().parent()
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(receipt.launch, draft.launch_id().unwrap());
    assert_eq!(
        (receipt.dir_dev, receipt.dir_ino),
        draft.dir_identity().unwrap()
    );
    assert_eq!(
        (receipt.pr, receipt.head.as_str(), receipt.state),
        (59, HEAD, State::Open)
    );
    assert_eq!(receipt.project_root, f.root.canonicalize().unwrap());
    assert_eq!(receipt.worktree, f.root.canonicalize().unwrap());
    assert_eq!(receipt.reviewer_session, "review-session");
    assert_eq!(receipt.launcher_grant, f.grant);
    assert_eq!(
        receipt.principal,
        sa::read_grant(&f.grant).unwrap().principal
    );
    assert!(receipt.seal.is_none() && receipt.child_grant.is_none());
    assert_eq!(load(&receipt.launch).unwrap(), receipt);
    // No staging file is left behind.
    assert_eq!(
        std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
        1
    );

    // The same slot cannot be registered twice, and the first receipt stays.
    let again = register(&f.root, &f.root, &draft, "another-session").unwrap_err();
    assert!(format!("{again:#}").contains("cannot create"), "{again:#}");
    assert_eq!(load(&receipt.launch).unwrap(), receipt);
    assert_eq!(
        std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
        1
    );
}

#[test]
fn bug_2669_only_a_launcher_delegating_reviewer_registers_a_receipt() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    {
        let mut f = Fixture::new(&["implementer"]);
        let draft = f.draft();
        assert!(register(&f.root, &f.root, &draft, "review-session")
            .unwrap()
            .is_none());
        f.env.unset_key(sa::GRANT_ENV);
        assert!(register(&f.root, &f.root, &draft, "review-session")
            .unwrap()
            .is_none());
        assert!(!f.home.join(".aida/review-handoffs").exists());
        assert_eq!(list().unwrap(), (vec![], vec![]));
    }

    // With the delegation, an empty reviewer session is still refused.
    let f = Fixture::new(&["reviewer"]);
    assert!(register(&f.root, &f.root, &f.draft(), " ").is_err());
    assert!(list().unwrap().0.is_empty());
}

#[test]
fn bug_2669_seal_pins_one_digest_and_consumption_closes_the_receipt() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let f = Fixture::new(&["reviewer"]);
    let (_draft, receipt) = f.registered();
    for bad in ["", "abc", &DIGEST.to_uppercase(), &format!("{DIGEST}0")] {
        assert!(seal(&receipt.launch, bad, None).is_err(), "{bad}");
    }
    assert!(load(&receipt.launch).unwrap().seal.is_none());

    let sealed = seal(&receipt.launch, DIGEST, Some("child-grant")).unwrap();
    let pinned = sealed.seal.clone().unwrap();
    assert_eq!(pinned.sha256, DIGEST);
    assert_eq!(sealed.child_grant.as_deref(), Some("child-grant"));
    assert_eq!(load(&receipt.launch).unwrap(), sealed);
    // Everything but the seal and the child grant is unchanged.
    assert_eq!(
        Receipt {
            seal: None,
            child_grant: None,
            ..sealed.clone()
        },
        receipt
    );

    // The same digest again changes nothing; another digest is refused.
    assert_eq!(
        seal(&receipt.launch, DIGEST, Some("other")).unwrap(),
        sealed
    );
    let other = DIGEST.replace('a', "b");
    let refused = seal(&receipt.launch, &other, None).unwrap_err();
    assert!(format!("{refused:#}").contains("different draft"));
    assert_eq!(load(&receipt.launch).unwrap(), sealed);

    let consumed = mark_consumed(&receipt.launch).unwrap();
    assert_eq!(consumed.state, State::Consumed);
    assert_eq!(consumed.seal, sealed.seal);
    assert_eq!(mark_consumed(&receipt.launch).unwrap(), consumed);
    // A closed receipt is never resealed.
    assert!(seal(&receipt.launch, DIGEST, None).is_err());
    assert!(seal(&receipt.launch, &other, None).is_err());
    assert_eq!(load(&receipt.launch).unwrap(), consumed);

    // A receipt in a terminal state other than consumed is not consumable.
    let (_d2, second) = f.registered();
    let mut superseded = second.clone();
    superseded.state = State::Superseded;
    std::fs::write(f.file(&second), serde_json::to_vec(&superseded).unwrap()).unwrap();
    assert!(mark_consumed(&second.launch).is_err());
    assert!(seal(&second.launch, DIGEST, None).is_err());
    assert_eq!(load(&second.launch).unwrap(), superseded);

    // Sealing or closing a launch that was never registered fails.
    let unknown = format!("draft-{}", uuid::Uuid::new_v4());
    assert!(seal(&unknown, DIGEST, None).is_err());
    assert!(mark_consumed(&unknown).is_err());
}

#[test]
fn bug_2669_load_refuses_tampered_misnamed_and_linked_receipts() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let f = Fixture::new(&["reviewer"]);
    let (_draft, receipt) = f.registered();
    let path = f.file(&receipt);
    let good = serde_json::to_value(&receipt).unwrap();

    for launch in [
        "",
        "draft-",
        "draft-../../session-grants/x",
        "0123",
        &receipt.launch.to_uppercase(),
        &receipt.launch.replace('-', ""),
        &format!("{}/", receipt.launch),
    ] {
        assert!(load(launch).is_err(), "{launch}");
        assert!(seal(launch, DIGEST, None).is_err(), "{launch}");
    }

    let mut unknown = good.clone();
    unknown["recorded_by"] = "forged".into();
    let mut version = good.clone();
    version["version"] = 2.into();
    let mut state = good.clone();
    state["state"] = "recovered".into();
    let mut renamed = good.clone();
    renamed["launch"] = format!("draft-{}", uuid::Uuid::new_v4()).into();
    for bad in [unknown, version, state, renamed] {
        std::fs::write(&path, serde_json::to_vec(&bad).unwrap()).unwrap();
        assert!(load(&receipt.launch).is_err(), "{bad}");
        assert!(seal(&receipt.launch, DIGEST, None).is_err(), "{bad}");
        assert!(mark_consumed(&receipt.launch).is_err(), "{bad}");
    }
    std::fs::write(&path, vec![b' '; RECEIPT_LIMIT as usize + 1]).unwrap();
    assert!(load(&receipt.launch).is_err());

    // A symlink or a second hard link to a valid receipt is not a receipt.
    let elsewhere = f.home.join("elsewhere.json");
    std::fs::write(&elsewhere, serde_json::to_vec(&good).unwrap()).unwrap();
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &path).unwrap();
    assert!(load(&receipt.launch).is_err());
    std::fs::remove_file(&path).unwrap();
    std::fs::hard_link(&elsewhere, &path).unwrap();
    assert!(load(&receipt.launch).is_err());
    std::fs::remove_file(&elsewhere).unwrap();
    assert_eq!(load(&receipt.launch).unwrap(), receipt);
}

#[test]
fn bug_2669_list_reports_receipts_unreadable_files_and_draft_state() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let f = Fixture::new(&["reviewer"]);
    assert_eq!(list().unwrap(), (vec![], vec![]));
    handle_list(false, false).unwrap();

    let (first_draft, first) = f.registered();
    let (second_draft, second) = f.registered();
    let dir = f.home.join(".aida/review-handoffs");
    std::fs::write(dir.join("not-a-uuid.json"), "{}").unwrap();
    std::fs::write(dir.join(".staged.tmp"), "{}").unwrap();
    std::fs::write(dir.join("notes.txt"), "ignored").unwrap();
    let (receipts, unreadable) = list().unwrap();
    assert_eq!(receipts.len(), 2);
    assert!(receipts.contains(&first) && receipts.contains(&second));
    assert!(receipts[0].created_at <= receipts[1].created_at);
    assert_eq!(unreadable.len(), 1);
    assert_eq!(unreadable[0].0, "not-a-uuid.json");

    assert_eq!(draft_status(&first), "not-written");
    std::fs::write(first_draft.path(), "{}").unwrap();
    assert_eq!(draft_status(&first), "unrecorded");
    first_draft.consumed().unwrap();
    assert_eq!(draft_status(&first), "recorded");

    // A directory recreated under the same name is not the registered one.
    let slot = second_draft.path().parent().unwrap().to_path_buf();
    let moved = slot.with_file_name("moved-aside");
    std::fs::rename(&slot, &moved).unwrap();
    assert_eq!(draft_status(&second), "directory-missing");
    std::fs::create_dir(&slot).unwrap();
    std::fs::write(slot.join("draft.json"), "{}").unwrap();
    assert_eq!(draft_status(&second), "directory-replaced");
    std::fs::remove_dir_all(&slot).unwrap();
    std::os::unix::fs::symlink(&moved, &slot).unwrap();
    assert_eq!(draft_status(&second), "directory-replaced");

    for (all, json) in [(false, false), (true, false), (false, true), (true, true)] {
        handle_list(all, json).unwrap();
    }
    assert_eq!(age_label(chrono::Duration::minutes(-5)), "0m");
    assert_eq!(age_label(chrono::Duration::minutes(59)), "59m");
    assert_eq!(age_label(chrono::Duration::minutes(61)), "1h01m");
    assert_eq!(age_label(chrono::Duration::hours(72)), "3d");
}

#[test]
fn bug_2669_registering_sweeps_only_expired_receipts_with_no_waiting_draft() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let f = Fixture::new(&["reviewer"]);
    let age = |receipt: &Receipt, days: i64| {
        let mut aged = receipt.clone();
        aged.created_at = Utc::now() - chrono::Duration::days(days);
        std::fs::write(f.file(receipt), serde_json::to_vec(&aged).unwrap()).unwrap();
    };
    let (_d1, done) = f.registered();
    mark_consumed(&done.launch).unwrap();
    let (waiting_draft, waiting) = f.registered();
    std::fs::write(waiting_draft.path(), "{}").unwrap();
    let (_d3, recent) = f.registered();
    let (_d4, edge) = f.registered();
    mark_consumed(&waiting.launch).unwrap();
    for receipt in [&done, &waiting] {
        age(receipt, 31);
    }
    age(&recent, 1);
    age(&edge, 29);
    let garbage = f.home.join(".aida/review-handoffs/not-a-uuid.json");
    std::fs::write(&garbage, "{}").unwrap();
    assert!(lock_path(&done.launch).unwrap().exists());

    let (_d5, new) = f.registered();
    let kept: Vec<_> = list().unwrap().0.into_iter().map(|r| r.launch).collect();
    assert!(!kept.contains(&done.launch), "expired and nothing waiting");
    assert!(!f.file(&done).exists() && !lock_path(&done.launch).unwrap().exists());
    // An unrecorded draft keeps its receipt however old it is.
    for receipt in [&waiting, &recent, &edge, &new] {
        assert!(kept.contains(&receipt.launch), "{}", receipt.launch);
    }
    assert!(garbage.exists());
}
