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
    // Published under exactly one name: no staging file and no second link.
    assert_eq!(
        std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
        1
    );
    assert_eq!(std::fs::metadata(&path).unwrap().nlink(), 1);
    assert_eq!(receipt.version, 1);
    assert!(
        !automatic_recovery_eligible(&receipt),
        "a version-1 receipt predates supersession and is never auto-recoverable"
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

    // Sealing or closing a launch that was never registered fails, and
    // leaves no lock file behind for it.
    let unknown = format!("draft-{}", uuid::Uuid::new_v4());
    assert!(seal(&unknown, DIGEST, None).is_err());
    assert!(mark_consumed(&unknown).is_err());
    assert!(mark_unused(&unknown).is_err());
    assert!(!lock_path(&unknown).unwrap().exists());
}

/// A verdict collected with no draft to read closes the receipt as unused,
/// never as consumed; a sealed receipt (a draft was read) cannot be unused.
#[test]
fn bug_2669_a_launch_that_read_no_draft_closes_its_receipt_as_unused() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let f = Fixture::new(&["reviewer"]);
    let (_draft, receipt) = f.registered();
    let unused = mark_unused(&receipt.launch).unwrap();
    assert_eq!(unused.state, State::Unused);
    assert!(unused.seal.is_none());
    assert_eq!(mark_unused(&receipt.launch).unwrap(), unused);
    assert_eq!(load(&receipt.launch).unwrap(), unused);
    assert!(mark_consumed(&receipt.launch).is_err());
    assert!(seal(&receipt.launch, DIGEST, None).is_err());
    assert_eq!(load(&receipt.launch).unwrap(), unused);

    let (_d2, sealed) = f.registered();
    let sealed = seal(&sealed.launch, DIGEST, None).unwrap();
    let refused = mark_unused(&sealed.launch).unwrap_err();
    assert!(format!("{refused:#}").contains("sealed"), "{refused:#}");
    assert_eq!(load(&sealed.launch).unwrap(), sealed);

    let (_d3, consumed) = f.registered();
    mark_consumed(&consumed.launch).unwrap();
    assert!(mark_unused(&consumed.launch).is_err());
    assert_eq!(load(&consumed.launch).unwrap().state, State::Consumed);
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
    // Every field a reader relies on is checked in full.
    let field = |name: &str, value: serde_json::Value| {
        let mut bad = good.clone();
        bad[name] = value;
        bad
    };
    let mut bad_seal = good.clone();
    bad_seal["seal"] = serde_json::json!({"sha256": "abc", "sealed_at": receipt.created_at});
    for bad in [
        unknown,
        version,
        state,
        renamed,
        field("head", "0123".into()),
        field("head", "".into()),
        field("head", format!("{HEAD}0").into()),
        field("head", HEAD.replace('0', "g").into()),
        field("head", HEAD.replace('0', "é").into()),
        field("pr", 0.into()),
        field("reviewer_session", " ".into()),
        field("launcher_grant", "".into()),
        field("principal", "".into()),
        field("worktree", "relative/checkout".into()),
        field("project_root", "".into()),
        bad_seal,
    ] {
        std::fs::write(&path, serde_json::to_vec(&bad).unwrap()).unwrap();
        assert!(load(&receipt.launch).is_err(), "{bad}");
        assert!(seal(&receipt.launch, DIGEST, None).is_err(), "{bad}");
        assert!(mark_consumed(&receipt.launch).is_err(), "{bad}");
        assert!(mark_unused(&receipt.launch).is_err(), "{bad}");
        // The row is reported as unreadable in both renderings, never
        // sliced, shown in part or swept.
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let human = list_report(None, false, Utc::now()).unwrap();
        assert!(
            human.contains(&format!("unreadable receipt {name}: ")),
            "{human}"
        );
        assert!(!human.contains(&receipt.launch), "{human}");
        let json: serde_json::Value =
            serde_json::from_str(&list_report(None, true, Utc::now()).unwrap()).unwrap();
        assert_eq!(json["receipts"], serde_json::json!([]), "{json}");
        assert_eq!(json["unreadable"][0]["file"], name.as_str(), "{json}");
        assert!(json["unreadable"][0]["error"].as_str().is_some());
        assert_eq!(
            sweep_from(Utc::now(), "", unbounded()),
            0,
            "an unreadable receipt is left alone"
        );
        assert!(path.exists());
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
    assert_eq!(
        list_report(Some(&f.root), false, Utc::now()).unwrap(),
        "No reviewer draft handoffs are registered for this project.\n"
    );

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

    assert_eq!(draft_files(&first), DraftFiles::Absent);
    std::fs::write(first_draft.path(), "{}").unwrap();
    assert_eq!(draft_files(&first), DraftFiles::Present);

    // The listing states file facts: an open, unsealed version-1 receipt
    // whose directory holds draft.json, not eligible for automatic recovery.
    let now = Utc::now();
    let human = list_report(Some(&f.root), false, now).unwrap();
    let row = human
        .lines()
        .find(|line| line.starts_with(&first.launch))
        .unwrap();
    assert!(
        row.contains(&format!(
            "PR-59 @ {}  open  unsealed  draft: present",
            &HEAD[..12]
        )),
        "{row}"
    );
    assert!(
        row.ends_with("(not eligible for automatic recovery)"),
        "{row}"
    );
    assert!(human.contains("unreadable receipt not-a-uuid.json: "));
    assert!(!human.contains("    project "));
    let all = list_report(None, false, now).unwrap();
    assert!(all.contains(&format!("    project {}", first.project_root.display())));
    let json: serde_json::Value =
        serde_json::from_str(&list_report(Some(&f.root), true, now).unwrap()).unwrap();
    let rows = json["receipts"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    let row = rows.iter().find(|r| r["launch"] == first.launch).unwrap();
    assert_eq!(row["draft"], "present");
    assert_eq!(row["state"], "open");
    assert_eq!(row["sealed"], false);
    assert_eq!(row["version"], 1);
    assert_eq!(row["automatic_recovery_eligible"], false);
    assert_eq!(row["within_automatic_window"], true);
    assert_eq!(row["head"], HEAD);
    assert_eq!(json["unreadable"][0]["file"], "not-a-uuid.json");

    first_draft.consumed().unwrap();
    assert_eq!(draft_files(&first), DraftFiles::MarkedRecorded);

    // A directory recreated under the same name is not the registered one.
    let slot = second_draft.path().parent().unwrap().to_path_buf();
    let moved = slot.with_file_name("moved-aside");
    std::fs::rename(&slot, &moved).unwrap();
    assert_eq!(draft_files(&second), DraftFiles::DirectoryMissing);
    std::fs::create_dir(&slot).unwrap();
    std::fs::write(slot.join("draft.json"), "{}").unwrap();
    assert_eq!(draft_files(&second), DraftFiles::DirectoryReplaced);
    std::fs::remove_dir_all(&slot).unwrap();
    std::os::unix::fs::symlink(&moved, &slot).unwrap();
    assert_eq!(draft_files(&second), DraftFiles::DirectoryReplaced);

    for (all, json) in [(false, false), (true, false), (false, true), (true, true)] {
        handle_list(all, json).unwrap();
    }
    assert_eq!(age_label(chrono::Duration::minutes(-5)), "0m");
    assert_eq!(age_label(chrono::Duration::minutes(59)), "59m");
    assert_eq!(age_label(chrono::Duration::minutes(61)), "1h01m");
    assert_eq!(age_label(chrono::Duration::hours(72)), "3d");
}

/// Only "no such entry" means a draft is absent. A lookup that fails for
/// any other reason is unreadable, and an unreadable draft keeps its
/// receipt however old it is.
#[test]
fn bug_2669_a_failed_draft_lookup_is_unreadable_and_never_swept() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let f = Fixture::new(&["reviewer"]);
    let aged = |receipt: &Receipt| {
        let mut aged = receipt.clone();
        aged.created_at = Utc::now() - chrono::Duration::days(60);
        std::fs::write(f.file(receipt), serde_json::to_vec(&aged).unwrap()).unwrap();
        aged
    };

    // The checkout path cannot be resolved (a symlink loop): not "missing".
    let (_d1, looped) = f.registered();
    let loop_path = f.home.join("loop");
    std::os::unix::fs::symlink(&loop_path, &loop_path).unwrap();
    let mut looped = looped;
    looped.worktree = loop_path;
    assert_eq!(draft_files(&looped), DraftFiles::Unreadable);
    let looped = aged(&looped);
    assert!(!expired(&looped, Utc::now()));

    // The directory is the registered one but cannot be searched.
    let (draft, denied) = f.registered();
    std::fs::write(draft.path(), "{}").unwrap();
    let slot = draft.path().parent().unwrap().to_path_buf();
    let searchable_mode = std::fs::metadata(&slot).unwrap().permissions();
    std::fs::set_permissions(&slot, std::fs::Permissions::from_mode(0o000)).unwrap();
    // A privileged test process can still search the directory.
    let searchable = std::fs::symlink_metadata(draft.path()).is_ok();
    if !searchable {
        assert_eq!(draft_files(&denied), DraftFiles::Unreadable);
    }
    let denied = aged(&denied);
    assert!(!expired(&denied, Utc::now()));

    assert_eq!(sweep_from(Utc::now(), "", unbounded()), 0);
    std::fs::set_permissions(&slot, searchable_mode).unwrap();
    for receipt in [&looped, &denied] {
        assert!(f.file(receipt).exists(), "{}", receipt.launch);
    }
}

/// `aida review draft list` names the project the way the launcher does:
/// by its main checkout, from a linked checkout too.
#[test]
fn bug_2669_list_from_a_linked_checkout_shows_the_projects_receipts() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let f = Fixture::new(&["reviewer"]);
    let (_draft, receipt) = f.registered();
    let linked = f._tmp.path().join("linked");
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(&f.root)
        .args(["worktree", "add", "-q", "-b", "linked"])
        .arg(&linked)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert!(linked.join(".git").is_file());

    let project = crate::main_worktree_root_from(&linked);
    assert_eq!(
        project.canonicalize().unwrap(),
        f.root.canonicalize().unwrap()
    );
    let rows = |project: &Path| {
        let json: serde_json::Value =
            serde_json::from_str(&list_report(Some(project), true, Utc::now()).unwrap()).unwrap();
        json["receipts"].as_array().unwrap().clone()
    };
    let listed = rows(&project);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["launch"], receipt.launch.as_str());
    // The linked checkout's own path is not the project a receipt names.
    assert!(rows(&linked).is_empty());
}

/// Limits no test store reaches.
fn unbounded() -> SweepLimits {
    SweepLimits {
        batch: usize::MAX,
        scan: usize::MAX,
        budget: Duration::from_secs(60),
    }
}

#[test]
fn bug_2669_sweep_removes_only_expired_receipts_with_no_draft_within_its_budget() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let f = Fixture::new(&["reviewer"]);
    let age = |receipt: &Receipt, days: i64| {
        let mut aged = load(&receipt.launch).unwrap();
        aged.created_at = Utc::now() - chrono::Duration::days(days);
        std::fs::write(f.file(receipt), serde_json::to_vec(&aged).unwrap()).unwrap();
    };
    let (_d1, done) = f.registered();
    mark_consumed(&done.launch).unwrap();
    let (_d1b, done_too) = f.registered();
    mark_unused(&done_too.launch).unwrap();
    let (waiting_draft, waiting) = f.registered();
    std::fs::write(waiting_draft.path(), "{}").unwrap();
    let (_d3, recent) = f.registered();
    let (_d4, edge) = f.registered();
    let (_d5, busy) = f.registered();
    mark_consumed(&waiting.launch).unwrap();
    mark_consumed(&busy.launch).unwrap();
    for receipt in [&done, &done_too, &waiting, &busy] {
        age(receipt, 31);
    }
    age(&recent, 1);
    age(&edge, 29);
    let garbage = f.home.join(".aida/review-handoffs/not-a-uuid.json");
    std::fs::write(&garbage, "{}").unwrap();
    assert!(lock_path(&done.launch).unwrap().exists());

    // Registering does no maintenance: the launch path never sweeps.
    let (_d6, new) = f.registered();
    assert!(f.file(&done).exists() && f.file(&done_too).exists());

    let now = Utc::now();
    // A zero work budget, or a zero listing bound, removes nothing.
    let batch = |batch| SweepLimits {
        batch,
        ..unbounded()
    };
    assert_eq!(sweep_from(now, "", batch(0)), 0);
    assert_eq!(
        sweep_from(
            now,
            "",
            SweepLimits {
                scan: 0,
                ..unbounded()
            }
        ),
        0
    );
    assert!(f.file(&done).exists() && f.file(&done_too).exists());

    // A receipt whose lock is held is skipped at once, not waited for.
    let held = lock(&busy.launch, Duration::ZERO).unwrap();
    // The work budget bounds each run; successive runs continue from where
    // the pivot points, wrapping round the store.
    let id = |receipt: &Receipt| receipt.launch.trim_start_matches("draft-").to_string();
    let started = Instant::now();
    assert_eq!(sweep_from(now, &id(&done), batch(1)), 1);
    assert!(!f.file(&done).exists() && f.file(&done_too).exists());
    assert_eq!(sweep_from(now, "", unbounded()), 1);
    assert!(
        started.elapsed() < LOCK_WAIT,
        "maintenance never waits for a receipt lock"
    );
    assert!(!f.file(&done_too).exists());
    assert!(!lock_path(&done.launch).unwrap().exists());
    assert!(
        f.file(&busy).exists(),
        "a busy receipt is left for a later run"
    );
    drop(held);

    let kept: Vec<_> = list().unwrap().0.into_iter().map(|r| r.launch).collect();
    // A directory that still holds draft.json keeps its receipt however old.
    for receipt in [&waiting, &recent, &edge, &new, &busy] {
        assert!(kept.contains(&receipt.launch), "{}", receipt.launch);
    }
    assert_eq!(kept.len(), 5);
    assert!(garbage.exists());

    // Once free, the busy one goes; the public entry point is bounded too.
    assert_eq!(sweep_from(now, "ffffffff", unbounded()), 1);
    assert!(!f.file(&busy).exists());
    assert_eq!(sweep(), 0);
    assert_eq!(list().unwrap().0.len(), 4);
}

/// Listing the store and examining receipts share one budget, and neither
/// starves the other: a run whose time is already spent, or whose listing is
/// cut short, still examines one receipt, so repeated runs always drain an
/// expired store.
#[test]
fn bug_2669_every_sweep_examines_a_receipt_however_little_budget_is_left() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let f = Fixture::new(&["reviewer"]);
    let expired_receipt = || {
        let (_draft, receipt) = f.registered();
        let mut aged = mark_consumed(&receipt.launch).unwrap();
        aged.created_at = Utc::now() - chrono::Duration::days(31);
        std::fs::write(f.file(&receipt), serde_json::to_vec(&aged).unwrap()).unwrap();
    };
    let remaining = || list().unwrap().0.len();
    let now = Utc::now();

    // No time at all: listing stops at the first receipt it finds, and that
    // receipt is still examined (and, being expired, removed).
    for _ in 0..3 {
        expired_receipt();
    }
    let spent = SweepLimits {
        budget: Duration::ZERO,
        ..unbounded()
    };
    for left in [2, 1, 0] {
        assert_eq!(sweep_from(now, "", spent), 1);
        assert_eq!(remaining(), left);
    }
    assert_eq!(sweep_from(now, "", spent), 0, "an empty store");

    // A listing cut short by its entry bound leaves time to examine what it
    // found: one entry listed, one receipt examined, per run.
    for _ in 0..2 {
        expired_receipt();
    }
    let one_entry = SweepLimits {
        scan: 1,
        ..unbounded()
    };
    for left in [1, 0] {
        assert_eq!(sweep_from(now, "ffffffff", one_entry), 1);
        assert_eq!(remaining(), left);
    }
}

/// The lock is opened without blocking and only a regular file is locked: a
/// FIFO or a symlink planted at the lock name is refused at once, within the
/// caller's wait, instead of hanging the launcher or maintenance.
#[test]
fn bug_2669_a_lock_name_that_is_not_a_regular_file_is_refused_without_blocking() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let f = Fixture::new(&["reviewer"]);
    let (_draft, receipt) = f.registered();
    mark_consumed(&receipt.launch).unwrap();
    let path = lock_path(&receipt.launch).unwrap();
    let plant_fifo = || {
        let _ = std::fs::remove_file(&path);
        let name = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
        // SAFETY: `name` is a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    };

    // No reader: a blocking write-only open would wait for one for ever.
    plant_fifo();
    let started = Instant::now();
    assert!(lock(&receipt.launch, LOCK_WAIT).is_err());
    // Closing an already consumed receipt is otherwise a no-op that succeeds.
    assert!(mark_consumed(&receipt.launch).is_err());
    // With a reader the open succeeds, and the file type check refuses it.
    let reader = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&path)
        .unwrap();
    let refused = lock(&receipt.launch, LOCK_WAIT).unwrap_err();
    assert!(
        format!("{refused:#}").contains("not a regular file"),
        "{refused:#}"
    );
    drop(reader);
    assert!(
        started.elapsed() < LOCK_WAIT,
        "a planted FIFO is refused without waiting"
    );

    // A symlink to a regular file elsewhere is not followed.
    std::fs::remove_file(&path).unwrap();
    let elsewhere = f.home.join("elsewhere.lock");
    std::fs::write(&elsewhere, "").unwrap();
    std::os::unix::fs::symlink(&elsewhere, &path).unwrap();
    assert!(lock(&receipt.launch, Duration::ZERO).is_err());

    // The receipt itself was never touched, and an expired one behind a
    // planted lock name is left for a later run, not waited for.
    assert_eq!(load(&receipt.launch).unwrap().state, State::Consumed);
    let mut aged = load(&receipt.launch).unwrap();
    aged.created_at = Utc::now() - chrono::Duration::days(31);
    std::fs::write(f.file(&receipt), serde_json::to_vec(&aged).unwrap()).unwrap();
    plant_fifo();
    assert_eq!(sweep_from(Utc::now(), "", unbounded()), 0);
    assert!(f.file(&receipt).exists());
    std::fs::remove_file(&path).unwrap();
    assert_eq!(sweep_from(Utc::now(), "", unbounded()), 1);
}

/// The lane write-profile refusal covers the host's receipt store and lock
/// directory wherever they resolve, and nothing that merely shares a name.
#[test]
fn bug_2669_host_only_roots_are_the_resolved_receipt_and_lock_directories() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let f = Fixture::new(&["reviewer"]);
    let aida = f.home.join(".aida");
    for name in ["review-handoffs", "review-handoff-locks"] {
        // Refused before the directory exists, once it does, and beneath it.
        assert!(inside_host_only_root(&aida.join(name)), "{name}");
        std::fs::create_dir_all(aida.join(name).join("nested")).unwrap();
        assert!(inside_host_only_root(&aida.join(name)), "{name}");
        assert!(inside_host_only_root(&aida.join(name).join("nested")));
        assert!(inside_host_only_root(
            &aida.join(name).join("not-there/yet")
        ));
        // Reached through a symlinked parent, it is still the host's store.
        let alias = f.home.join(format!("alias-{name}"));
        std::os::unix::fs::symlink(&aida, &alias).unwrap();
        assert!(inside_host_only_root(&alias.join(name)), "{name}");

        // Same name, somewhere else: not the host's store.
        let unrelated = f.root.join("vendor").join(name);
        std::fs::create_dir_all(unrelated.join("nested")).unwrap();
        assert!(!inside_host_only_root(&unrelated), "{name}");
        assert!(!inside_host_only_root(&unrelated.join("nested")));
        assert!(!inside_host_only_root(&f.home.join(name)));
        assert!(!inside_host_only_root(&aida.join(format!("{name}-other"))));
    }
    assert!(!inside_host_only_root(&aida));
    assert!(!inside_host_only_root(&aida.join("grant-notices")));

    // The other direction: a mount above the store contains both roots, by
    // resolved location and through a symlinked path; a mount beside or
    // inside the store contains neither.
    let roots = |name: &str| aida.canonicalize().unwrap().join(name);
    let both = vec![roots("review-handoffs"), roots("review-handoff-locks")];
    assert_eq!(host_only_roots_beneath(&aida), both);
    assert_eq!(host_only_roots_beneath(&f.home), both);
    assert_eq!(host_only_roots_beneath(f._tmp.path()), both);
    assert_eq!(
        host_only_roots_beneath(&f.home.join("alias-review-handoffs")),
        both
    );
    for beside in [
        aida.join("grant-notices"),
        aida.join("review-handoffs"),
        aida.join("review-handoffs/nested"),
        f.root.clone(),
    ] {
        assert!(
            host_only_roots_beneath(&beside).is_empty(),
            "{}",
            beside.display()
        );
    }
}

/// `aida review draft` is registered from its own module through the
/// command registry's nested table, not as a `ReviewCommand` variant.
#[test]
fn bug_2669_review_draft_is_registered_from_its_own_module_under_review() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    use crate::cmdkit::{DispatchCtx, Flow, StakeholderAction};
    use crate::command_registry as registry;
    let _f = Fixture::new(&["reviewer"]);

    let keys: Vec<(&str, &str)> = registry::NESTED
        .iter()
        .map(|(parent, reg)| (*parent, reg.name))
        .collect();
    let mut sorted = keys.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(keys, sorted, "NESTED is sorted by parent then name");
    assert!(keys.contains(&("review", "draft")));
    for (parent, reg) in registry::NESTED {
        let parent_reg = registry::lookup(parent).expect("the parent is a registered command");
        assert_eq!((reg.command)().get_name(), reg.name);
        let attached = (parent_reg.command)()
            .get_subcommands()
            .filter(|sub| sub.get_name() == reg.name)
            .count();
        assert_eq!(attached, 1, "`aida {parent} {}`", reg.name);
    }
    // The parent's own clap types do not name it.
    assert!(
        <crate::review_cmd::ReviewArgs as clap::CommandFactory>::command()
            .get_subcommands()
            .all(|sub| sub.get_name() != "draft")
    );

    let command = registry::parse_command(["aida", "review", "draft", "list", "--all", "--json"]);
    assert_eq!(command.name(), "review");
    assert_eq!(command.subcommand_name(), Some("draft"));
    // Listing is a read; the parent's other verbs keep the parent's policy.
    assert_eq!(
        crate::stakeholder_cli_action(&command),
        StakeholderAction::Read
    );
    assert_eq!(
        crate::stakeholder_cli_action(&registry::parse_command(["aida", "review", "list"])),
        StakeholderAction::Write
    );

    let review = registry::lookup("review").unwrap();
    let ctx = DispatchCtx {
        globals: None,
        store: None,
    };
    let matches = (review.command)()
        .try_get_matches_from(["review", "draft", "list", "--all", "--json"])
        .unwrap();
    assert!(matches!(
        registry::run_nested("review", &matches, &ctx),
        Some(Ok(Flow::Return))
    ));
    let other = (review.command)()
        .try_get_matches_from(["review", "list"])
        .unwrap();
    assert!(registry::run_nested("review", &other, &ctx).is_none());
    assert!(registry::nested_policy("review", &other).is_none());
    assert!((review.command)()
        .try_get_matches_from(["review", "draft"])
        .is_err());
}
