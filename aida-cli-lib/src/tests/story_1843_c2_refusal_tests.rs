//! STORY-1843 C2 (C SKETCH section 6, rework F5): every CLI destructive
//! entry that meets a refused mail retirement keeps the tree, lease, claim
//! and mail, takes the recovery-ownership ending (grant revoked, lease
//! stamped ending-retained) where it ends a session, and reopens a retirement
//! it could not consume.
//!
//! The refusal used throughout is QC1: the tree's mailbox holds mail and no
//! activation is recorded. The failure used for "the step did not happen" is
//! a held `index.lock`, which makes the reuse's `git checkout` / reset fail.
//! trace:STORY-1843 | ai:claude

use std::path::{Path, PathBuf};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A repository on `main` with one commit.
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    std::fs::write(dir.path().join("README.md"), "seed\n").unwrap();
    git(dir.path(), &["add", "README.md"]);
    git(dir.path(), &["commit", "-qm", "seed"]);
    dir
}

/// A linked worktree of `root` on a new `branch`, with an `.aida/` of its
/// own.
fn worktree(root: &Path, branch: &str) -> PathBuf {
    let wt = root.join(format!("wt-{branch}"));
    git(
        root,
        &["worktree", "add", "-q", "-b", branch, wt.to_str().unwrap()],
    );
    std::fs::create_dir_all(wt.join(".aida")).unwrap();
    aida_core::git_ops::ensure_aida_runtime_excluded(&wt).unwrap();
    wt.canonicalize().unwrap()
}

fn mailbox(tree: &Path) -> PathBuf {
    tree.join(".aida").join("mailbox")
}

fn put_mail(tree: &Path, name: &str) {
    std::fs::create_dir_all(mailbox(tree)).unwrap();
    std::fs::write(mailbox(tree).join(name), "{}").unwrap();
}

fn mail_in(tree: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(mailbox(tree))
        .map(|d| {
            d.flatten()
                .filter(|e| e.path().is_file())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// Record an activation under `root`'s anchor, as `aida mailbox retention
/// activate` does for an operator.
fn activate(root: &Path) {
    let common = PathBuf::from(git(
        root,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    ));
    let anchor = common.join("aida").join("mail-lifecycle");
    std::os::unix::fs::DirBuilderExt::mode(std::fs::DirBuilder::new().recursive(true), 0o700)
        .create(&anchor)
        .unwrap();
    // Only the source build that recorded an activation honours it: this
    // test binary's, named as `main_entry` names it.
    crate::upgrade_release::register_source_build();
    let build = aida_core::mailbox_lifecycle::source_build().expect("a git build");
    std::fs::write(
        anchor.join("activation.json"),
        format!(
            r#"{{"schema":1,"all_writer_hosts_admitting":true,"recorded_at":"2026-10-27T00:00:00Z","source_build":"{build}"}}"#
        ),
    )
    .unwrap();
}

/// Hold the worktree's `index.lock`, so `git checkout` / `reset` fail.
fn hold_index_lock(tree: &Path) -> PathBuf {
    let git_dir = PathBuf::from(git(tree, &["rev-parse", "--absolute-git-dir"]));
    let lock = git_dir.join("index.lock");
    std::fs::write(&lock, "").unwrap();
    lock
}

/// Write a worktree lease for `wt` and return its file.
fn write_lease(root: &Path, id: &str, wt: &Path, branch: &str, extra: &str) -> PathBuf {
    let file = crate::lease_path(root, id);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(
        &file,
        format!(
            "id = \"{id}\"\nscope = \"TASK-1843\"\nslug = \"task-1843\"\nowner = \"t\"\n\
             worktree_path = {}\nbranch = \"{branch}\"\nstarted_at = \"2026-10-27T00:00:00Z\"\n\
             hostname = \"h\"\n{extra}",
            aida_core::toml_quote::toml_string(&wt.display().to_string())
        ),
    )
    .unwrap();
    file
}

fn read_lease(file: &Path) -> crate::SessionLease {
    toml::from_str(&std::fs::read_to_string(file).unwrap()).unwrap()
}

fn assert_ending_retained(file: &Path) {
    assert!(file.exists(), "the lease is kept");
    let lease = read_lease(file);
    assert!(
        crate::leases::is_ending_retained(&lease),
        "the lease is stamped ending-retained"
    );
    assert!(lease
        .ending_retained_reason
        .as_deref()
        .is_some_and(|r| r.contains("write permit")));
}

#[test]
fn a_refused_force_cleanup_keeps_the_session_and_its_grant_stays_revoked() {
    let _trust = crate::test_env::EnvVarGuard::set("AIDA_TRUST_PID_NAMESPACE", "1");
    let repo = repo();
    let root = repo.path();
    let wt = worktree(root, "force");
    put_mail(&wt, "m.json");
    let grant = crate::gate::seat_authority::test_support::mint_grant_for(
        root,
        "test-subject",
        "advisor",
        &["implementer"],
    );
    let file = write_lease(
        root,
        "0019f1843a01",
        &wt,
        "force",
        &format!("child_grant_id = \"{grant}\"\n"),
    );
    let lease = read_lease(&file);

    assert!(!crate::leases::force_cleanup_lease(root, &lease));
    assert_ending_retained(&file);
    assert!(wt.exists(), "the tree is kept");
    assert_eq!(mail_in(&wt), ["m.json"]);
    assert!(
        !crate::gate::seat_authority::revoke_by_id(&grant).unwrap(),
        "the child grant was revoked"
    );
    assert_eq!(
        read_lease(&file).child_grant_id.as_deref(),
        Some(grant.as_str()),
        "the stamp keeps every other lease key"
    );

    // A retry refuses the same way, and the grant stays revoked.
    assert!(!crate::leases::force_cleanup_lease(
        root,
        &read_lease(&file)
    ));
    assert_ending_retained(&file);
    assert!(!crate::gate::seat_authority::revoke_by_id(&grant).unwrap());
}

#[test]
fn a_refused_reap_keeps_the_session_ending_retained() {
    let _trust = crate::test_env::EnvVarGuard::set("AIDA_TRUST_PID_NAMESPACE", "1");
    let repo = repo();
    let root = repo.path();
    let wt = worktree(root, "reaped");
    put_mail(&wt, "m.json");
    let file = write_lease(root, "0019f1843a02", &wt, "reaped", "");
    let tip = git(root, &["rev-parse", "reaped"]);

    let outcome = crate::session_reap::reap_one(root, &read_lease(&file), Some(&tip));
    assert!(
        outcome.starts_with("skipped") && outcome.contains("mail retention refused"),
        "{outcome}"
    );
    assert_ending_retained(&file);
    assert!(wt.exists());
    assert_eq!(mail_in(&wt), ["m.json"]);
    assert_eq!(
        git(root, &["rev-parse", "reaped"]),
        tip,
        "the branch is kept"
    );
}

#[test]
fn a_refused_doctor_heal_keeps_the_orphan_tree() {
    let _trust = crate::test_env::EnvVarGuard::set("AIDA_TRUST_PID_NAMESPACE", "1");
    let repo = repo();
    let root = repo.path();
    let wt = worktree(root, "orphan");
    put_mail(&wt, "m.json");
    let finding = crate::DoctorFinding {
        category: "orphan-worktrees".into(),
        id: wt.display().to_string(),
        summary: String::new(),
        action: String::new(),
        safe_heal: true,
    };
    let healed = crate::doctor_cmd::heal_doctor_orphan_worktree(root, &finding).unwrap();
    assert_ne!(healed.status, "healed");
    assert!(wt.exists(), "the tree is kept");
    assert_eq!(mail_in(&wt), ["m.json"]);
}

#[test]
fn a_lane_reuse_keeps_a_refused_tree_and_reopens_after_a_failed_checkout() {
    let repo = repo();
    let root = repo.path();
    let wt = worktree(root, "lane-old");
    put_mail(&wt, "m.json");

    // Refused: the tree is untouched and the lane branch was never made.
    let err =
        crate::dispatch::prepare_lane_worktree(root, &wt, "lane-new", "main", "main").unwrap_err();
    assert!(format!("{err:#}").contains("refusing to reuse"), "{err:#}");
    assert_eq!(mail_in(&wt), ["m.json"]);
    assert_eq!(git(&wt, &["rev-parse", "--abbrev-ref", "HEAD"]), "lane-old");

    // Retired, then the checkout fails: the mail is copied back.
    activate(root);
    let lock = hold_index_lock(&wt);
    let err =
        crate::dispatch::prepare_lane_worktree(root, &wt, "lane-new", "main", "main").unwrap_err();
    assert!(format!("{err:#}").contains("checkout"), "{err:#}");
    assert_eq!(
        mail_in(&wt),
        ["m.json"],
        "the failed reuse reopened the mail"
    );
    std::fs::remove_file(lock).unwrap();

    // The retry goes through and the next occupant starts with no mail.
    crate::dispatch::prepare_lane_worktree(root, &wt, "lane-new", "main", "main").unwrap();
    assert!(mail_in(&wt).is_empty());
}

#[test]
fn an_integrator_reuse_keeps_a_refused_tree_and_reopens_after_a_failed_reset() {
    let repo = repo();
    let root = repo.path();
    let tree = aida_core::worktree_pool::acquire(
        root,
        &aida_core::worktree_pool::AcquireOptions {
            lease_holder: Some(crate::integrate_checkout::INTEGRATOR_LEASE_SCOPE.to_string()),
            ..Default::default()
        },
    )
    .unwrap();
    std::fs::create_dir_all(tree.join(".aida")).unwrap();
    put_mail(&tree, "m.json");

    let err = crate::integrate_checkout::ensure_integrator_checkout(root).unwrap_err();
    assert!(
        format!("{err:#}").contains("refusing to reuse the integrator worktree"),
        "{err:#}"
    );
    assert_eq!(mail_in(&tree), ["m.json"]);

    activate(root);
    let lock = hold_index_lock(&tree);
    assert!(crate::integrate_checkout::ensure_integrator_checkout(root).is_err());
    assert_eq!(
        mail_in(&tree),
        ["m.json"],
        "the failed reset reopened the mail"
    );
    std::fs::remove_file(lock).unwrap();

    assert_eq!(
        crate::integrate_checkout::ensure_integrator_checkout(root).unwrap(),
        tree
    );
    assert!(mail_in(&tree).is_empty());
}

#[test]
fn a_refused_retirement_revokes_the_grant_and_a_second_stamp_keeps_it_revoked() {
    // The s6 fixture for session end's ending, at the seam session end
    // calls: the lease and its keys stay, the grant is revoked and stays so.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let grant = crate::gate::seat_authority::test_support::mint_grant_for(
        root,
        "test-subject",
        "advisor",
        &["implementer"],
    );
    let file = write_lease(
        root,
        "0019f1843a03",
        Path::new("/w"),
        "b",
        &format!("child_grant_id = \"{grant}\"\nunmodelled = \"kept\"\n"),
    );
    let refusal = aida_core::mailbox_lifecycle::guard::RetentionRefusal {
        checkout: "/w".into(),
        reason: "the mailbox holds mail, and not every mail writer takes a write permit".into(),
        needs_activation: true,
        retained_dir: None,
    };
    crate::mail_retention::keep_ending_retained(&file, &refusal);
    crate::mail_retention::keep_ending_retained(&file, &refusal);
    assert_ending_retained(&file);
    let table: toml::Table = std::fs::read_to_string(&file).unwrap().parse().unwrap();
    assert_eq!(table["unmodelled"].as_str(), Some("kept"));
    assert!(!crate::gate::seat_authority::revoke_by_id(&grant).unwrap());
}

/// The ending-retained stamp a refusal after the commit leaves: kept, and
/// naming the late write rather than a missing activation.
fn assert_late_retained(file: &Path) {
    assert!(file.exists(), "the lease is kept");
    let lease = read_lease(file);
    assert!(
        crate::leases::is_ending_retained(&lease),
        "the lease is stamped ending-retained"
    );
    assert!(lease
        .ending_retained_reason
        .as_deref()
        .is_some_and(|r| !r.is_empty()));
}

/// Land `name` in `tree`'s mailbox right before the next guarded removal:
/// a writer that took no permit, after the retirement committed.
fn late_mail_before_removal(tree: &Path, name: &'static str) {
    let tree = tree.to_path_buf();
    crate::mail_retention::on_before_guarded_removal(move || put_mail(&tree, name));
}

#[test]
fn r3_f1_late_mail_before_a_force_cleanup_removal_keeps_the_lease_and_manifest() {
    let _trust = crate::test_env::EnvVarGuard::set("AIDA_TRUST_PID_NAMESPACE", "1");
    let repo = repo();
    let root = repo.path();
    let wt = worktree(root, "late-cleanup");
    let id = "0019f1843a04";
    let file = write_lease(root, id, &wt, "late-cleanup", "");
    let manifest = crate::session_manifest::manifest_path(root, id);
    std::fs::write(&manifest, "kept\n").unwrap();
    late_mail_before_removal(&wt, "late.json");

    assert!(!crate::leases::force_cleanup_lease(
        root,
        &read_lease(&file)
    ));
    assert_late_retained(&file);
    assert!(manifest.exists(), "the manifest is kept");
    assert!(wt.exists(), "the tree is kept");
    assert_eq!(mail_in(&wt), ["late.json"], "the late mail is kept");
    // The runtime links the cleanup stripped are put back.
    assert!(wt.join(".aida").join("sessions").is_symlink());
}

#[test]
fn r3_f1_late_mail_before_a_session_end_removal_keeps_the_session() {
    let repo = repo();
    let root = repo.path();
    let wt = worktree(root, "late-end");
    let id = "0019f1843a05";
    let file = write_lease(root, id, &wt, "late-end", "");
    let manifest = crate::session_manifest::manifest_path(root, id);
    std::fs::write(&manifest, "kept\n").unwrap();
    // One guard for the ambient and the trust pin: the env lock is not
    // reentrant.
    let _ambient = crate::test_env::AmbientGuard::hermetic_and(
        root,
        None,
        &[("AIDA_TRUST_PID_NAMESPACE", Some("1"))],
    );
    late_mail_before_removal(&wt, "late.json");

    let refused = crate::session_end::session_end(
        Some(id),
        None,
        None,
        true,
        true,
        false,
        false,
        false,
        true,
        false,
        true,
        false,
    )
    .unwrap_err();
    let text = format!("{refused:#}");
    assert!(text.contains("NOT ended"), "{text}");
    // R3-F2: the refusal never suggests an unfenced raw removal.
    assert!(!text.contains("--force"), "{text}");
    assert_late_retained(&file);
    assert!(manifest.exists(), "the manifest is kept");
    assert!(wt.exists(), "the tree is kept");
    assert_eq!(mail_in(&wt), ["late.json"], "the late mail is kept");
}

#[test]
fn r3_f3_a_rebase_worktree_whose_removal_fails_is_kept_not_deleted_unfenced() {
    let repo = repo();
    let root = repo.path();
    // An empty mailbox and a removal git refuses (a locked worktree): the old
    // fallback deleted it with `remove_dir_all`, unfenced.
    let wt = worktree(root, "rebase-locked");
    std::fs::create_dir_all(mailbox(&wt)).unwrap();
    git(root, &["worktree", "lock", wt.to_str().unwrap()]);
    assert!(crate::pr_cmd::remove_rebase_worktree(root, &wt).is_err());
    assert!(wt.exists(), "a failed guarded removal keeps the tree");

    // A refusal keeps it too, with its mail.
    git(root, &["worktree", "unlock", wt.to_str().unwrap()]);
    put_mail(&wt, "m.json");
    let refused = crate::pr_cmd::remove_rebase_worktree(root, &wt).unwrap_err();
    assert!(crate::mail_retention::retention_refusal(&refused).is_some());
    assert!(wt.exists());
    assert_eq!(mail_in(&wt), ["m.json"]);
}

#[test]
fn r3_f4_the_report_says_an_activation_was_recorded_by_another_build() {
    use aida_core::mailbox_lifecycle::activation::ActivationReport;
    let report = |recorded, other: Option<&str>| ActivationReport {
        dry_run: true,
        recorded,
        recorded_by_other_build: other.map(str::to_string),
        this_build: Some("0.16.0+abc1234".into()),
        hosts: Vec::new(),
        scan_incomplete: None,
        would_record: true,
    };
    let other =
        crate::mailbox_cmd::retention_record_lines(&report(false, Some("0.16.0+0ld"))).join("\n");
    assert!(
        other.contains("by a different build: 0.16.0+0ld") && other.contains("0.16.0+abc1234"),
        "{other}"
    );
    assert!(
        other.contains("re-run `aida mailbox retention activate`"),
        "{other}"
    );
    assert_eq!(
        crate::mailbox_cmd::retention_record_lines(&report(false, None)),
        ["recorded activation: no"]
    );
    assert!(crate::mailbox_cmd::retention_record_lines(&report(true, None))[0].contains("yes"));
}
