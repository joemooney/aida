//! BUG-HOME-038: a seat clone's host-git refusal is not uncommitted work; a
//! seat proven contained runs git in its own clone with no hook; and
//! `aida ship --from-seat` accepts only a recorded clone and a one-ref bundle.
//! trace:BUG-HOME-038 | ai:claude

use super::s4_host_git_tests::leased_clone;
use super::tests::{fixture, ref_exists, Fixture};
use super::*;
use std::path::{Path, PathBuf};

/// `git -C dir args…` through the test helper (auto-maintenance off); its
/// trimmed stdout. Panics on failure.
fn git_ok(dir: &Path, args: &[&str]) -> String {
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} in {} failed: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// `aida ship` in a seat clone it cannot prove containment in fails with the
/// refusal naming the clone, never with "commit uncommitted work".
#[test]
fn bug_home_038_host_git_refusal_is_not_pending_work() {
    let fx = fixture();
    let clone = leased_clone(&fx, "refusal", "c0ffee038001");
    let err = crate::ship::tracked_changes_pending(&clone)
        .expect_err("a refused clone is an error, not pending work")
        .to_string();
    assert!(err.contains(&clone.display().to_string()), "{err}");
    assert!(err.contains("not uncommitted work"), "{err}");
    assert!(err.contains("aida ship --from-seat"), "{err}");
    // A plain repository still answers normally.
    assert!(!crate::ship::tracked_changes_pending(&fx.main).unwrap());
}

/// Run `f` with the containment probe reporting `EROFS`, as it does under
/// the seat sandbox's read-only bind of main (no real mount needed).
#[cfg(unix)]
fn read_only_main<R>(f: impl FnOnce() -> R) -> R {
    aida_core::seat_clone_detect::with_containment_probe(Some(libc::EROFS), f)
}

/// Main's `.git` chmod 0555 (`EACCES`, which a seat can arrange for any
/// directory it owns) does not prove containment.
#[cfg(unix)]
#[test]
fn bug_home_038_eacces_on_main_git_is_not_containment() {
    let fx = fixture();
    let clone = leased_clone(&fx, "eacces", "c0ffee038010");
    let _session = session("c0ffee038010");
    let refused = aida_core::seat_clone_detect::with_containment_probe(Some(libc::EACCES), || {
        aida_core::seat_clone_detect::contained_in_clone(&clone)
    });
    assert!(!refused);
}

/// Plant a hook in the clone's `.git/hooks` and in a `core.hooksPath`
/// directory; each one records that it ran.
fn plant_hooks(clone: &Path, sentinel: &Path, root: &Path) {
    let body = format!("#!/bin/sh\necho \"$0\" >> '{}'\n", sentinel.display());
    let planted = root.join("planted-hooks");
    std::fs::create_dir_all(&planted).unwrap();
    for name in ["pre-commit", "commit-msg", "pre-push", "post-commit"] {
        crate::test_exec::write_executable(&clone.join(".git/hooks").join(name), &body);
        crate::test_exec::write_executable(&planted.join(name), &body);
    }
    git_ok(
        clone,
        &["config", "core.hooksPath", planted.to_str().unwrap()],
    );
}

/// This process's session is `id` (and no Claude Code session id leaks in).
fn session(id: &str) -> crate::test_env::EnvVarsGuard {
    crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_SESSION_ID", Some(id)),
        ("CLAUDE_CODE_SESSION_ID", None),
    ])
}

/// Condition 2: with main's `.git` read-only (EROFS) and this process's
/// session leasing the clone, host git runs in the seat's own clone, and
/// neither a planted pre-push hook nor a `core.hooksPath` hook ever runs.
/// Without the session's lease, the refusal is back.
#[cfg(unix)]
#[test]
fn bug_home_038_contained_seat_git_never_runs_planted_hooks() {
    let fx = fixture();
    let clone = leased_clone(&fx, "hooks", "c0ffee038002");
    let sentinel = fx.root.join("hook-ran");
    plant_hooks(&clone, &sentinel, &fx.root);
    git_ok(&clone, &["config", "user.name", "Seat"]);
    git_ok(&clone, &["config", "user.email", "seat@example.com"]);
    git_ok(&clone, &["config", "maintenance.auto", "false"]);
    git_ok(&clone, &["config", "gc.auto", "0"]);

    let own = session("c0ffee038002");
    read_only_main(|| {
        assert!(aida_core::seat_clone_detect::contained_in_clone(&clone));
        assert_eq!(aida_core::git_ops::refused_seat_clone(&clone), None);
        std::fs::write(clone.join("README.md"), "seat\n").unwrap();
        assert!(crate::ship::tracked_changes_pending(&clone).unwrap());
        crate::ship::commit_tracked_changes(&clone, "fix: seat (BUG-HOME-038)").unwrap();
        let push = aida_core::git_ops::host_git_command(&clone)
            .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0"])
            .args(["push", "-q", "origin", "HEAD:refs/heads/seat-pushed"])
            .output()
            .unwrap();
        assert!(
            push.status.success(),
            "{}",
            String::from_utf8_lossy(&push.stderr)
        );
    });
    assert!(
        !sentinel.exists(),
        "a seat hook ran: {}",
        std::fs::read_to_string(&sentinel).unwrap_or_default()
    );
    assert!(ref_exists(&fx.origin, "refs/heads/seat-pushed"));
    // Main writable again (the host's view): git is refused.
    assert!(!aida_core::seat_clone_detect::contained_in_clone(&clone));
    // Not this process's lease any more: git is refused.
    drop(own);
    let _other = session("c0ffee0380ff");
    read_only_main(|| {
        assert!(!aida_core::seat_clone_detect::contained_in_clone(&clone));
        assert_eq!(
            aida_core::git_ops::refused_seat_clone(&clone),
            Some(clone.clone())
        );
    });
}

/// Ruling item 1: a contained process holding seat A's lease is refused in
/// seat B's clone (readable to it, but B's config is not A's to run).
#[cfg(unix)]
#[test]
fn bug_home_038_contained_seat_is_refused_in_another_seats_clone() {
    let fx = fixture();
    let seat_a = super::s4_host_git_tests::leased_clone_on(&fx, "seat-a", "c0ffee038005", "a");
    let seat_b = super::s4_host_git_tests::leased_clone_on(&fx, "seat-b", "c0ffee038006", "b");
    let _session = session("c0ffee038005");
    read_only_main(|| {
        assert!(aida_core::seat_clone_detect::contained_in_clone(&seat_a));
        assert!(!aida_core::seat_clone_detect::contained_in_clone(&seat_b));
        assert_eq!(
            aida_core::git_ops::refused_seat_clone(&seat_b),
            Some(seat_b.clone())
        );
        let err = crate::ship::tracked_changes_pending(&seat_b)
            .expect_err("seat B's clone is refused to seat A")
            .to_string();
        assert!(err.contains(&seat_b.display().to_string()), "{err}");
        let status = aida_core::git_ops::host_git_command(&seat_b)
            .args(["status", "--porcelain"])
            .output()
            .unwrap();
        assert!(!status.status.success(), "git ran in seat B's clone");
    });
}

/// A commit on `branch` in main, then the branch deleted: the objects stay
/// (as they would after a seat import), the ref does not.
fn orphan_commit(main: &Path, branch: &str, file: &str) -> String {
    git_ok(main, &["checkout", "-q", "-b", branch, "main"]);
    std::fs::write(main.join(file), format!("{branch}\n")).unwrap();
    git_ok(main, &["add", file]);
    git_ok(main, &["commit", "-qm", &format!("feat: {branch}")]);
    git_ok(main, &["checkout", "-q", "main"]);
    git_ok(main, &["rev-parse", branch])
}

/// A bundle carrying two refs is refused; a one-ref bundle of the right
/// branch is accepted and lands only in the quarantine ref.
#[test]
fn bug_home_038_two_ref_bundle_is_refused_and_one_ref_is_quarantined() {
    let fx = fixture();
    let tip = orphan_commit(&fx.main, "feat", "a.txt");
    orphan_commit(&fx.main, "other", "b.txt");
    let two = fx.root.join("two.bundle");
    let one = fx.root.join("one.bundle");
    git_ok(
        &fx.main,
        &[
            "bundle",
            "create",
            "-q",
            two.to_str().unwrap(),
            "refs/heads/feat",
            "refs/heads/other",
            "--not",
            "main",
        ],
    );
    git_ok(
        &fx.main,
        &[
            "bundle",
            "create",
            "-q",
            one.to_str().unwrap(),
            "refs/heads/feat",
            "--not",
            "main",
        ],
    );
    git_ok(&fx.main, &["branch", "-q", "-D", "feat", "other"]);

    let err = crate::ship_from_seat::verify_bundle(&fx.main, &two, "feat")
        .unwrap_err()
        .to_string();
    assert!(err.contains("carries 2 refs"), "{err}");
    let err = crate::ship_from_seat::verify_bundle(&fx.main, &one, "other")
        .unwrap_err()
        .to_string();
    assert!(err.contains("only `refs/heads/other`"), "{err}");

    let got = crate::ship_from_seat::verify_bundle(&fx.main, &one, "feat").unwrap();
    assert_eq!(got, tip);
    let quarantine =
        crate::ship_from_seat::quarantine_bundle(&fx.main, &one, "feat", &got).unwrap();
    assert_eq!(quarantine, "refs/aida/seat-import/feat");
    assert_eq!(git_ok(&fx.main, &["rev-parse", &quarantine]), tip);
    assert!(!ref_exists(&fx.main, "refs/heads/feat"));
}

/// A bundle whose prerequisite the host lacks fails `git bundle verify`.
#[test]
fn bug_home_038_bundle_with_missing_prerequisite_is_refused() {
    let fx = fixture();
    let elsewhere = fx.root.join("elsewhere");
    git_ok(
        &fx.root,
        &[
            "clone",
            "-q",
            fx.origin.to_str().unwrap(),
            elsewhere.to_str().unwrap(),
        ],
    );
    git_ok(&elsewhere, &["config", "user.email", "e@example.com"]);
    git_ok(&elsewhere, &["config", "user.name", "E"]);
    std::fs::write(elsewhere.join("x.txt"), "x\n").unwrap();
    git_ok(&elsewhere, &["add", "x.txt"]);
    git_ok(&elsewhere, &["commit", "-qm", "base the host lacks"]);
    git_ok(&elsewhere, &["checkout", "-q", "-b", "feat"]);
    std::fs::write(elsewhere.join("y.txt"), "y\n").unwrap();
    git_ok(&elsewhere, &["add", "y.txt"]);
    git_ok(&elsewhere, &["commit", "-qm", "feat"]);
    let bundle = fx.root.join("prereq.bundle");
    git_ok(
        &elsewhere,
        &[
            "bundle",
            "create",
            "-q",
            bundle.to_str().unwrap(),
            "refs/heads/feat",
            "--not",
            "main",
        ],
    );
    let err = crate::ship_from_seat::verify_bundle(&fx.main, &bundle, "feat")
        .unwrap_err()
        .to_string();
    assert!(err.contains("git bundle verify"), "{err}");
}

/// Only a clone this checkout leased is published from: an `aida-seat-*`
/// clone without a lease, and any other path, are refused.
#[test]
fn bug_home_038_unrecorded_clone_path_is_refused() {
    let fx = fixture();
    let recorded = leased_clone(&fx, "recorded", "c0ffee038003");
    assert_eq!(
        crate::ship_from_seat::recorded_seat(&fx.main, &recorded)
            .unwrap()
            .root,
        recorded
    );

    let forged = fx.root.join(format!("{CLONE_DIR_PREFIX}aida-forged"));
    create(&fx.main, &forged, "feat", "origin/main").unwrap();
    let err = crate::ship_from_seat::recorded_seat(&fx.main, &forged)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("not a seat clone this checkout recorded"),
        "{err}"
    );

    let err = crate::ship_from_seat::recorded_seat(&fx.main, &fx.origin)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("not a seat clone this checkout recorded"),
        "{err}"
    );
}

/// End to end where bwrap runs: the seat's committed branch is bundled inside
/// the sandbox with planted hooks never running, verified, and quarantined.
#[cfg(target_os = "linux")]
#[test]
fn bug_home_038_bwrap_import_quarantines_the_seat_branch() {
    let live = std::process::Command::new("bwrap")
        .args(["--unshare-all", "--ro-bind", "/", "/", "--dev", "/dev"])
        .args(["--proc", "/proc", "true"])
        .output()
        .is_ok_and(|o| o.status.success());
    if !live {
        assert!(
            std::env::var_os("AIDA_REQUIRE_BWRAP_LIVE").is_none(),
            "AIDA_REQUIRE_BWRAP_LIVE=1 but bwrap cannot run here"
        );
        eprintln!("SKIP (bwrap unavailable)");
        return;
    }
    let fx = fixture();
    let clone = leased_clone(&fx, "import", "c0ffee038004");
    git_ok(&clone, &["config", "user.name", "Seat"]);
    git_ok(&clone, &["config", "user.email", "seat@example.com"]);
    std::fs::write(clone.join("seat.txt"), "seat\n").unwrap();
    git_ok(&clone, &["add", "seat.txt"]);
    git_ok(
        &clone,
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "commit",
            "-qm",
            "fix: seat",
        ],
    );
    let tip = git_ok(&clone, &["rev-parse", "HEAD"]);
    let sentinel = fx.root.join("hook-ran");
    plant_hooks(&clone, &sentinel, &fx.root);

    let seat = crate::ship_from_seat::recorded_seat(&fx.main, &clone).unwrap();
    let (quarantine, got) = crate::ship_from_seat::import_seat_branch(&seat, "feat").unwrap();
    assert_eq!(got, tip);
    assert_eq!(git_ok(&fx.main, &["rev-parse", &quarantine]), tip);
    assert!(!ref_exists(&fx.main, "refs/heads/feat"));
    assert!(!sentinel.exists(), "a seat hook ran");
    let report = config_hooks_report(&fx.main, &clone).join("\n");
    assert!(report.contains("core.hookspath"), "{report}");
    assert!(report.contains("pre-push"), "{report}");
}

/// A leased seat clone on `branch` with one committed change titled
/// `subject` (hooks off); returns the clone and its tip.
fn seat_with_commit(
    fx: &Fixture,
    name: &str,
    id: &str,
    branch: &str,
    subject: &str,
) -> (PathBuf, String) {
    let clone = super::s4_host_git_tests::leased_clone_on(fx, name, id, branch);
    git_ok(&clone, &["config", "user.name", "Seat"]);
    git_ok(&clone, &["config", "user.email", "seat@example.com"]);
    std::fs::write(clone.join(format!("{name}.txt")), "seat\n").unwrap();
    git_ok(&clone, &["add", "."]);
    git_ok(
        &clone,
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "commit",
            "-qm",
            subject,
            "-m",
            "The seat's change.",
        ],
    );
    let tip = git_ok(&clone, &["rev-parse", "HEAD"]);
    (clone, tip)
}

/// The handler's bundle step without bwrap: test git bundles the branch in
/// the clone against main's `origin/main`.
fn test_bundle(main: &Path, clone: &Path, branch: &str) -> anyhow::Result<Vec<u8>> {
    let base = git_ok(main, &["rev-parse", "origin/main"]);
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(clone)
        .args(["bundle", "create", "-", &format!("refs/heads/{branch}")])
        .args(["--not", &base])
        .output()?;
    anyhow::ensure!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(out.stdout)
}

fn opts(clone: &Path, dry_run: bool, no_pr: bool) -> crate::ship_from_seat::FromSeatOptions {
    crate::ship_from_seat::FromSeatOptions {
        clone: clone.to_path_buf(),
        spec: Some("BUG-1".into()),
        no_pr,
        dry_run,
    }
}

/// Every seat-import quarantine ref in `repo`.
fn import_refs(repo: &Path) -> String {
    git_ok(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname)",
            crate::ship_from_seat::QUARANTINE_PREFIX,
        ],
    )
}

/// Every ref in `repo` with its object.
fn all_refs(repo: &Path) -> String {
    git_ok(repo, &["for-each-ref", "--format=%(objectname) %(refname)"])
}

/// Ruling items 3–5: `--dry-run` imports nothing; a real run pushes the
/// quarantined commit to the LEASE's branch on origin, deletes the
/// quarantine ref, and opens the PR with the "published from seat" note.
#[test]
fn bug_home_038_from_seat_handler_pushes_lease_branch_and_opens_pr() {
    let fx = fixture();
    let (clone, tip) = seat_with_commit(
        &fx,
        "publish",
        "c0ffee038007",
        "seat-feat",
        "fix: seat change (BUG-1)",
    );
    let forge = crate::forge::fake::RecordingForge {
        opens_as: Some(77),
        ..crate::forge::fake::RecordingForge::new()
    };
    let opened_for = std::cell::RefCell::new(Vec::<String>::new());
    let deps = crate::ship_from_seat::Deps {
        bundle: &test_bundle,
        forge: &|_| Box::new(forge.clone()) as Box<dyn crate::forge::Forge>,
        after_open: &|_, branch| opened_for.borrow_mut().push(branch.to_string()),
    };

    let before = all_refs(&fx.main);
    crate::ship_from_seat::run_from(&fx.main, opts(&clone, true, false), &deps).unwrap();
    assert_eq!(all_refs(&fx.main), before, "a dry run changed main's refs");
    assert!(!ref_exists(&fx.origin, "refs/heads/seat-feat"));
    assert!(forge.opened.lock().unwrap().is_empty());

    crate::ship_from_seat::run_from(&fx.main, opts(&clone, false, false), &deps).unwrap();
    assert_eq!(
        git_ok(&fx.origin, &["rev-parse", "refs/heads/seat-feat"]),
        tip
    );
    assert_eq!(
        import_refs(&fx.main),
        "",
        "the quarantine ref outlived the push"
    );
    let opened = forge.opened.lock().unwrap().clone();
    assert_eq!(opened.len(), 1);
    assert_eq!(opened[0].branch, "seat-feat");
    assert_eq!(opened[0].title, "fix: seat change (BUG-1)");
    assert!(
        opened[0].body.contains("Published from seat"),
        "{}",
        opened[0].body
    );
    assert!(
        opened[0].body.contains("The seat's change."),
        "{}",
        opened[0].body
    );
    assert_eq!(*opened_for.borrow(), vec!["seat-feat".to_string()]);
}

/// Review finding 2: a failed open-PR lookup after the push is reported as
/// an error, and no (possibly duplicate) PR is opened.
#[test]
fn bug_home_038_from_seat_failed_pr_lookup_opens_no_pr() {
    let fx = fixture();
    let (clone, _) = seat_with_commit(
        &fx,
        "lookup",
        "c0ffee03800b",
        "seat-lookup",
        "fix: seat change (BUG-1)",
    );
    let forge = crate::forge::fake::RecordingForge {
        opens_as: Some(78),
        open_for_branch: crate::forge::ChangeLookup::Unreachable("connection reset".into()),
        ..crate::forge::fake::RecordingForge::new()
    };
    let deps = crate::ship_from_seat::Deps {
        bundle: &test_bundle,
        forge: &|_| Box::new(forge.clone()) as Box<dyn crate::forge::Forge>,
        after_open: &|_, _| panic!("after_open ran without an open PR"),
    };
    let err = crate::ship_from_seat::run_from(&fx.main, opts(&clone, false, false), &deps)
        .unwrap_err()
        .to_string();
    assert!(err.contains("connection reset"), "{err}");
    assert!(err.contains("no PR was opened"), "{err}");
    assert!(forge.opened.lock().unwrap().is_empty(), "a PR was opened");
}

/// Review finding 3: an explicit spec the seat's commits do not name is
/// refused before anything is pushed; the named one is accepted.
#[test]
fn bug_home_038_from_seat_explicit_spec_must_match_trailers() {
    let fx = fixture();
    let (clone, _) = seat_with_commit(
        &fx,
        "mismatch",
        "c0ffee03800c",
        "seat-mismatch",
        "fix: seat change (BUG-2)",
    );
    let deps = crate::ship_from_seat::Deps {
        bundle: &test_bundle,
        forge: &|_| panic!("forge reached despite a spec mismatch"),
        after_open: &|_, _| {},
    };
    let err = crate::ship_from_seat::run_from(&fx.main, opts(&clone, false, true), &deps)
        .unwrap_err()
        .to_string();
    assert!(err.contains("you named BUG-1"), "{err}");
    assert!(err.contains("BUG-2"), "{err}");
    assert!(!ref_exists(&fx.origin, "refs/heads/seat-mismatch"));

    let ids = vec!["BUG-2".to_string(), "TASK-7".to_string()];
    crate::ship_from_seat::explicit_spec_check("bug-2", &ids).unwrap();
    crate::ship_from_seat::explicit_spec_check("TASK-7", &ids).unwrap();
    assert!(crate::ship_from_seat::explicit_spec_check("BUG-3", &ids).is_err());
}

/// `--from-seat` run from inside a seat clone is refused before anything.
#[test]
fn bug_home_038_from_seat_inside_a_clone_is_refused() {
    let fx = fixture();
    let (clone, _) = seat_with_commit(&fx, "inside", "c0ffee038008", "inside", "fix: x (BUG-1)");
    let deps = crate::ship_from_seat::Deps {
        bundle: &|_, _, _| panic!("bundled from inside a clone"),
        forge: &|_| panic!("forge reached from inside a clone"),
        after_open: &|_, _| {},
    };
    let err = crate::ship_from_seat::run_from(&clone, opts(&clone, false, true), &deps)
        .unwrap_err()
        .to_string();
    assert!(err.contains("runs from the main checkout"), "{err}");
}

/// Ruling item 2: a seat commit with no `(SPEC-ID)` trailer is refused
/// before the push, its quarantine ref kept; a dead trailer is refused by
/// the STORY-469 check.
#[test]
fn bug_home_038_from_seat_trailer_check_runs_before_push() {
    let fx = fixture();
    let (clone, _) = seat_with_commit(&fx, "bare", "c0ffee038009", "bare", "fix: no trailer");
    let deps = crate::ship_from_seat::Deps {
        bundle: &test_bundle,
        forge: &|_| panic!("forge reached despite a refused trailer"),
        after_open: &|_, _| {},
    };
    let err = crate::ship_from_seat::run_from(&fx.main, opts(&clone, false, true), &deps)
        .unwrap_err()
        .to_string();
    assert!(err.contains("carries a `(SPEC-ID)` trailer"), "{err}");
    assert!(!ref_exists(&fx.origin, "refs/heads/bare"));
    assert_eq!(import_refs(&fx.main), "refs/aida/seat-import/bare");

    let base = git_ok(&fx.main, &["rev-parse", "origin/main"]);
    let (clone, _) = seat_with_commit(&fx, "dead", "c0ffee03800a", "dead", "fix: y (BUG-404)");
    let range = format!("{base}..HEAD");
    let err = crate::ship_from_seat::seat_trailer_check(&clone, &range, |_| {
        crate::trace::SpecResolution::Missing
    })
    .unwrap_err()
    .to_string();
    assert!(err.contains("BUG-404"), "{err}");
    crate::ship_from_seat::seat_trailer_check(&clone, &range, |_| {
        crate::trace::SpecResolution::Live
    })
    .unwrap();
}

#[test]
fn bug_home_038_seat_pr_body_notes_the_seat() {
    let body = crate::ship_from_seat::seat_pr_body(
        "fix: x (BUG-1)\n\nWhy it changed.\n",
        "aida-seat-aida-x",
    );
    assert!(body.contains("Why it changed."), "{body}");
    assert!(
        body.contains("Published from seat `aida-seat-aida-x`"),
        "{body}"
    );
}
