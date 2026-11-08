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

/// Run `f` with the containment probe answering as the seat sandbox of the
/// seat whose tree is `own` does: everything outside `own` (main, sibling
/// clones) read-only (`EROFS`), `own` writable (no real mount needed).
#[cfg(unix)]
fn seat_view<R>(own: &Path, f: impl FnOnce() -> R) -> R {
    let own = own.to_path_buf();
    aida_core::seat_clone_detect::with_containment_probe(
        move |git_dir| (!git_dir.starts_with(&own)).then_some(libc::EROFS),
        f,
    )
}

/// Main's `.git` chmod 0555 (`EACCES`, which a seat can arrange for any
/// directory it owns) does not prove containment.
#[cfg(unix)]
#[test]
fn bug_home_038_eacces_on_main_git_is_not_containment() {
    let fx = fixture();
    let clone = leased_clone(&fx, "eacces", "c0ffee038010");
    let own = clone.clone();
    let refused = aida_core::seat_clone_detect::with_containment_probe(
        move |git_dir| (!git_dir.starts_with(&own)).then_some(libc::EACCES),
        || aida_core::seat_clone_detect::contained_in_clone(&clone),
    );
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

/// This process claims session `id` (both session variables, as a seat that
/// sets its own environment can).
#[cfg(unix)]
fn session(id: Option<&str>) -> crate::test_env::EnvVarsGuard {
    crate::test_env::EnvVarsGuard::apply(&[("AIDA_SESSION_ID", id), ("CLAUDE_CODE_SESSION_ID", id)])
}

/// Condition 2: with main's `.git` read-only (EROFS), the clone leased in
/// main and writable to this process, host git runs in the seat's own clone,
/// and neither a planted pre-push hook nor a `core.hooksPath` hook ever runs.
/// With the clone read-only too (another seat's view of it), the refusal is
/// back.
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

    seat_view(&clone, || {
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
    // Not this process's clone (read-only to it): git is refused.
    seat_view(&fx.root.join("aida-seat-aida-elsewhere"), || {
        assert!(!aida_core::seat_clone_detect::contained_in_clone(&clone));
        assert_eq!(
            aida_core::git_ops::refused_seat_clone(&clone),
            Some(clone.clone())
        );
    });
}

/// Ruling item 1: a contained seat A is refused in seat B's clone (readable
/// to it, but B's config is not A's to run). TASK-139: that holds whatever
/// session id A exports — none, its own, or B's.
// trace:TASK-B6B1278890774EF993D2EBC327862470-139 | ai:claude
#[cfg(unix)]
#[test]
fn bug_home_038_contained_seat_is_refused_in_another_seats_clone() {
    let fx = fixture();
    let seat_a = super::s4_host_git_tests::leased_clone_on(&fx, "seat-a", "c0ffee038005", "a");
    let seat_b = super::s4_host_git_tests::leased_clone_on(&fx, "seat-b", "c0ffee038006", "b");
    for claimed in [None, Some("c0ffee038005"), Some("c0ffee038006")] {
        let _session = session(claimed);
        seat_view(&seat_a, || {
            assert!(
                aida_core::seat_clone_detect::contained_in_clone(&seat_a),
                "{claimed:?}"
            );
            assert!(
                !aida_core::seat_clone_detect::contained_in_clone(&seat_b),
                "seat A claiming {claimed:?} passed the own-clone check in seat B's clone"
            );
        });
    }
    let _session = session(Some("c0ffee038006"));
    seat_view(&seat_a, || {
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

/// The handler's commit step for a run without `--commit-pending`: never
/// reached.
// trace:TASK-2712 | ai:claude
fn no_commit(_: &Path, _: &Path, _: &str, _: &str) -> anyhow::Result<Option<String>> {
    panic!("the commit step ran without --commit-pending")
}

fn opts(clone: &Path, dry_run: bool, no_pr: bool) -> crate::ship_from_seat::FromSeatOptions {
    crate::ship_from_seat::FromSeatOptions {
        clone: clone.to_path_buf(),
        spec: Some("BUG-1".into()),
        no_pr,
        dry_run,
        commit_pending: None,
    }
}

/// Whether bwrap can create an unprivileged sandbox here. When it cannot,
/// the caller skips, unless `AIDA_REQUIRE_BWRAP_LIVE` is set.
// trace:TASK-2712 | ai:claude
#[cfg(target_os = "linux")]
fn bwrap_live_or_skip() -> bool {
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
    }
    live
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
        commit: &no_commit,
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

/// TASK-2712 (the advisor's N3 fallback): a native Codex lane cannot commit
/// in its own sandbox, so `aida ship` inside its seat refuses with the
/// host-commit recovery, and `--commit-pending` in a dry run commits nothing.
// trace:TASK-2712 | ai:claude
#[test]
fn task_2712_native_seat_ship_refuses_with_host_commit_recovery() {
    let fx = fixture();
    let (clone, tip) = seat_with_commit(
        &fx,
        "nativedry",
        "c0ffee271202",
        "native-dry",
        "fix: native codex seat (BUG-1)",
    );
    let err = crate::codex_native_sandbox::refuse_native_publication(Some("native"), &clone)
        .unwrap_err()
        .to_string();
    assert!(err.contains("--commit-pending"), "{err}");
    assert!(err.contains("--from-seat"), "{err}");
    assert!(err.contains("untouched"), "{err}");
    crate::codex_native_sandbox::refuse_native_publication(None, &clone).unwrap();

    let deps = crate::ship_from_seat::Deps {
        bundle: &test_bundle,
        commit: &no_commit,
        forge: &|_| panic!("forge reached in a dry run"),
        after_open: &|_, _| {},
    };
    let mut dry = opts(&clone, true, false);
    dry.commit_pending = Some("fix: more (BUG-1)".into());
    crate::ship_from_seat::run_from(&fx.main, dry, &deps).unwrap();
    assert_eq!(git_ok(&clone, &["rev-parse", "HEAD"]), tip);
}

/// TASK-2712 N2/N4 with the advisor's N3 fallback, end to end where bwrap
/// runs: a native Codex seat leaves its work uncommitted; `ship --from-seat
/// --commit-pending` commits it host-side in AIDA's own sandbox (hooks off,
/// the host's identity, the lease's branch only), then the unchanged
/// publication pushes exactly that commit to a fake origin and opens the PR
/// on a fake forge. Hooks the seat planted (in `.git/hooks` and through
/// `core.hooksPath`) never run. A seat on another branch is refused with
/// nothing committed.
// trace:TASK-2712 | ai:claude
#[cfg(target_os = "linux")]
#[test]
fn task_2712_native_codex_seat_commit_ships_from_seat_without_seat_hooks() {
    if !bwrap_live_or_skip() {
        return;
    }
    let fx = fixture();
    let clone =
        super::s4_host_git_tests::leased_clone_on(&fx, "native", "c0ffee271201", "native-feat");
    git_ok(&fx.main, &["config", "user.name", "Host Operator"]);
    git_ok(&fx.main, &["config", "user.email", "host@example.com"]);
    git_ok(&clone, &["config", "user.name", "Seat Forged"]);
    git_ok(&clone, &["config", "user.email", "forged@example.com"]);
    let sentinel = fx.root.join("hook-ran");
    plant_hooks(&clone, &sentinel, &fx.root);
    std::fs::write(clone.join("native.txt"), "native seat\n").unwrap();
    let base = git_ok(&clone, &["rev-parse", "HEAD"]);
    // trace:BUG-2493 | ai:codex
    let message =
        "fix: native codex seat (BUG-1)\n\n# Body stays verbatim  \n\nHelp-Golden: unchanged\n\n";

    // A seat that moved off its lease's branch is refused; nothing commits.
    git_ok(&clone, &["checkout", "-q", "-b", "elsewhere"]);
    let err = crate::seat_clone::commit_pending(&fx.main, &clone, "native-feat", message)
        .unwrap_err()
        .to_string();
    assert!(err.contains("not its lease's branch"), "{err}");
    assert_eq!(git_ok(&clone, &["rev-parse", "HEAD"]), base);
    git_ok(&clone, &["checkout", "-q", "native-feat"]);

    let forge = crate::forge::fake::RecordingForge {
        opens_as: Some(88),
        ..crate::forge::fake::RecordingForge::new()
    };
    let deps = crate::ship_from_seat::Deps {
        bundle: &test_bundle,
        commit: &crate::seat_clone::commit_pending,
        forge: &|_| Box::new(forge.clone()) as Box<dyn crate::forge::Forge>,
        after_open: &|_, _| {},
    };
    let mut run = opts(&clone, false, false);
    run.commit_pending = Some(message.into());
    crate::ship_from_seat::run_from(&fx.main, run, &deps).unwrap();

    let tip = git_ok(&clone, &["rev-parse", "HEAD"]);
    assert_ne!(tip, base, "the host committed nothing");
    assert_eq!(
        git_ok(&fx.origin, &["rev-parse", "refs/heads/native-feat"]),
        tip,
        "the pushed commit is the one the host made in the seat, byte for byte"
    );
    assert_eq!(
        git_ok(&clone, &["log", "-1", "--format=%an <%ae>|%cn <%ce>|%s"]),
        "Host Operator <host@example.com>|Host Operator <host@example.com>|fix: native codex seat (BUG-1)",
        "the host's identity, never the seat's"
    );
    assert_eq!(
        git_ok(&clone, &["show", "--format=", "--name-only", "HEAD"]),
        "native.txt"
    );
    assert_eq!(git_ok(&clone, &["status", "--porcelain"]), "");
    let opened = forge.opened.lock().unwrap().clone();
    assert_eq!(opened.len(), 1);
    assert_eq!(opened[0].title, message.lines().next().unwrap());
    // Read the object directly: git_ok trims whitespace and would conceal
    // cleanup of the body or its final blank line.
    // trace:BUG-2493 | ai:codex
    let object = std::process::Command::new("git")
        .args(["cat-file", "commit", "HEAD"])
        .current_dir(&clone)
        .output()
        .unwrap();
    assert!(object.status.success());
    let object = String::from_utf8(object.stdout).unwrap();
    assert_eq!(object.split_once("\n\n").unwrap().1, message);
    assert!(
        !sentinel.exists(),
        "a seat hook ran: {}",
        std::fs::read_to_string(&sentinel).unwrap_or_default()
    );

    // Nothing pending: the commit step is a no-op.
    assert_eq!(
        crate::seat_clone::commit_pending(&fx.main, &clone, "native-feat", message).unwrap(),
        None
    );
}

/// A leased seat clone on `branch` with one uncommitted file (the shape a
/// native Codex lane leaves behind).
// trace:TASK-2712 | ai:claude
fn seat_with_pending(fx: &Fixture, name: &str, id: &str, branch: &str) -> PathBuf {
    let clone = super::s4_host_git_tests::leased_clone_on(fx, name, id, branch);
    std::fs::write(clone.join(format!("{name}.txt")), "pending\n").unwrap();
    clone
}

// trace:TASK-2902 | ai:codex
fn seat_with_publication_pending(fx: &Fixture, name: &str, id: &str, branch: &str) -> PathBuf {
    let clone = seat_with_pending(fx, name, id, branch);
    // trace:TASK-2902 | ai:codex — use a live spec belonging to this lease.
    let lease_path = crate::lease_path(&fx.main, id);
    let lease = std::fs::read_to_string(&lease_path)
        .unwrap()
        .replace("scope = \"TASK-2053\"", "scope = \"BUG-1\"");
    std::fs::write(lease_path, lease).unwrap();
    let mut store = aida_core::RequirementsStore::new();
    store
        .requirements
        .push(title_spec("Repair seat publication"));
    aida_core::Storage::new(fx.main.join("requirements.yaml"))
        .save(&store)
        .unwrap();
    clone
}

/// A commit step that must never run.
// trace:TASK-2712 | ai:claude
fn commit_never(_: &Path, _: &Path, _: &str, _: &str) -> anyhow::Result<Option<String>> {
    panic!("the commit step ran although a refusal was due")
}

/// A commit step without bwrap: test git commits the clone's pending work
/// (hooks off) with the host's message, as the sandboxed commit would.
// trace:TASK-2712 | ai:claude
fn test_commit(
    _: &Path,
    clone: &Path,
    branch: &str,
    message: &str,
) -> anyhow::Result<Option<String>> {
    assert_eq!(
        git_ok(clone, &["symbolic-ref", "--short", "HEAD"]),
        branch,
        "committed on the lease's branch"
    );
    if git_ok(clone, &["status", "--porcelain"]).is_empty() {
        return Ok(None);
    }
    git_ok(clone, &["add", "-A"]);
    git_ok(
        clone,
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "user.name=Host",
            "-c",
            "user.email=host@example.com",
            "commit",
            "--cleanup=verbatim",
            "-qm",
            message,
        ],
    );
    Ok(Some(git_ok(clone, &["rev-parse", "HEAD"])))
}

/// Advisor F3: `--commit-pending` validates the message's trailer and the
/// explicit spec BEFORE it commits, in a real run and in a dry run alike, so
/// a refused message leaves nothing committed and the seat is not stuck.
// trace:TASK-2712 | ai:claude
#[test]
fn task_2712_commit_pending_refuses_a_bad_message_before_committing() {
    let fx = fixture();
    let clone = seat_with_pending(&fx, "badmsg", "c0ffee271210", "bad-msg");
    let base = git_ok(&clone, &["rev-parse", "HEAD"]);
    let deps = crate::ship_from_seat::Deps {
        bundle: &|_, _, _| panic!("bundled after a refused message"),
        commit: &commit_never,
        forge: &|_| panic!("forge reached after a refused message"),
        after_open: &|_, _| {},
    };
    for dry_run in [false, true] {
        // No trailer at all.
        let mut run = opts(&clone, dry_run, false);
        run.commit_pending = Some("fix: no trailer".into());
        let err = format!(
            "{:#}",
            crate::ship_from_seat::run_from(&fx.main, run, &deps).unwrap_err()
        );
        assert!(err.contains("trailer"), "{err}");
        assert!(err.contains("Nothing was committed"), "{err}");
        // A trailer naming another spec than the explicit one.
        let mut run = opts(&clone, dry_run, false);
        run.commit_pending = Some("fix: other (BUG-2)".into());
        let err = format!(
            "{:#}",
            crate::ship_from_seat::run_from(&fx.main, run, &deps).unwrap_err()
        );
        assert!(err.contains("BUG-1"), "{err}");
        assert!(err.contains("Nothing was committed"), "{err}");
    }
    assert_eq!(git_ok(&clone, &["rev-parse", "HEAD"]), base);
    assert!(!git_ok(&clone, &["status", "--porcelain"]).is_empty());
    // A good message passes the checks; the dry run still commits nothing.
    let mut run = opts(&clone, true, false);
    run.commit_pending = Some("fix: pending (BUG-1)".into());
    let dry_deps = crate::ship_from_seat::Deps {
        bundle: &test_bundle,
        commit: &commit_never,
        forge: &|_| panic!("forge reached in a dry run"),
        after_open: &|_, _| {},
    };
    // The bundle of a branch with no new commit is refused after the
    // checks; the point is that the commit step never ran.
    let _ = crate::ship_from_seat::run_from(&fx.main, run, &dry_deps);
    assert_eq!(git_ok(&clone, &["rev-parse", "HEAD"]), base);
}

/// Advisor F6: every refusal of the host commit channel fires before git
/// touches the clone: a path with no `clone` lease, another checkout's
/// directory, a symbolic link to a recorded clone, an ended lease, and a
/// lease whose worker is another live process. The commit step never runs.
// trace:TASK-2712 | ai:claude
#[cfg(unix)]
#[test]
fn task_2712_commit_pending_refusals_commit_nothing() {
    let fx = fixture();
    let message = "fix: pending (BUG-1)";
    let refused = |clone: &Path, want: &str| {
        let err = format!(
            "{:#}",
            crate::ship_from_seat::commit_seat_pending(
                &fx.main,
                clone,
                Some("BUG-1"),
                message,
                &commit_never
            )
            .unwrap_err()
        );
        assert!(err.contains(want), "{}: {err}", clone.display());
    };

    // Not host-recorded: an `aida-seat-*` clone without a lease, and a
    // foreign directory.
    let forged = fx.root.join(format!("{CLONE_DIR_PREFIX}aida-forged2712"));
    create(&fx.main, &forged, "forged", "origin/main").unwrap();
    std::fs::write(forged.join("x.txt"), "x\n").unwrap();
    refused(&forged, "not a seat clone this checkout recorded");
    refused(&fx.origin, "not a seat clone this checkout recorded");

    // A symbolic link to a recorded clone.
    let clone = seat_with_pending(&fx, "linked", "c0ffee271211", "linked");
    let link = fx.root.join("seat-link");
    std::os::unix::fs::symlink(&clone, &link).unwrap();
    refused(&link, "symbolic link");

    // A lease held by another live process (a competing owner).
    let lease = crate::lease_path(&fx.main, "c0ffee271211");
    let text = std::fs::read_to_string(&lease).unwrap();
    let mut owner = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let start = aida_core::liveness::process_start_identity(owner.id()).unwrap();
    std::fs::write(
        &lease,
        format!(
            "{text}active_pid = {}\nactive_pid_start_time = {}\n",
            owner.id(),
            aida_core::toml_quote::toml_string(&start)
        ),
    )
    .unwrap();
    refused(&clone, "held by live process");
    let _ = owner.kill();
    let _ = owner.wait();
    // Once that owner is gone, the same lease no longer refuses.
    assert!(crate::ship_from_seat::pending_commit_preflight(
        &fx.main,
        &clone,
        Some("BUG-1"),
        message
    )
    .is_ok());

    // An ended lease no longer vouches for the clone.
    std::fs::remove_file(&lease).unwrap();
    refused(&clone, "not a seat clone this checkout recorded");
    assert!(!git_ok(&clone, &["status", "--porcelain"]).is_empty());
}

/// Advisor F6: a clone path replaced (or swapped for a symbolic link) after
/// it was checked is refused by the identity pin the commit re-checks.
// trace:TASK-2712 | ai:claude
#[cfg(unix)]
#[test]
fn task_2712_replaced_clone_path_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("aida-seat-x");
    std::fs::create_dir(&dir).unwrap();
    let pinned = crate::seat_clone::dir_identity(&dir).unwrap();
    crate::seat_clone::ensure_dir_identity(&dir, pinned).unwrap();
    std::fs::rename(&dir, tmp.path().join("moved")).unwrap();
    std::fs::create_dir(&dir).unwrap();
    let err = crate::seat_clone::ensure_dir_identity(&dir, pinned)
        .unwrap_err()
        .to_string();
    assert!(err.contains("replaced"), "{err}");
    std::fs::remove_dir(&dir).unwrap();
    std::os::unix::fs::symlink(tmp.path().join("moved"), &dir).unwrap();
    let err = crate::seat_clone::ensure_dir_identity(&dir, pinned)
        .unwrap_err()
        .to_string();
    assert!(err.contains("not a directory"), "{err}");
}

/// Advisor F6: the commit sandbox fails LOUDLY when it cannot find exactly
/// one read-only bind of the clone to make writable, instead of running
/// with the clone silently read-only.
// trace:TASK-2712 | ai:claude
#[test]
fn task_2712_commit_sandbox_bind_miss_fails_loudly() {
    let clone = Path::new("/seats/aida-seat-x");
    let os = |v: &[&str]| -> Vec<std::ffi::OsString> {
        v.iter().map(|s| std::ffi::OsString::from(*s)).collect()
    };
    let ok = crate::seat_clone::writable_clone_bind(
        os(&[
            "--ro-bind",
            "/",
            "/",
            "--ro-bind",
            "/seats/aida-seat-x",
            "/seats/aida-seat-x",
        ]),
        clone,
    )
    .unwrap();
    assert_eq!(ok[3], "--bind");
    assert_eq!(ok[0], "--ro-bind");
    for args in [
        os(&["--ro-bind", "/", "/"]),
        os(&[
            "--ro-bind",
            "/seats/aida-seat-x",
            "/seats/aida-seat-x",
            "--ro-bind",
            "/seats/aida-seat-x",
            "/seats/aida-seat-x",
        ]),
    ] {
        let err = crate::seat_clone::writable_clone_bind(args, clone)
            .unwrap_err()
            .to_string();
        assert!(err.contains("cannot make the clone writable"), "{err}");
    }
    // The production commit sandbox does find its bind.
    let tmp = tempfile::tempdir().unwrap();
    let real = tmp.path().join("aida-seat-y");
    let main = tmp.path().join("main");
    std::fs::create_dir_all(real.join(".git")).unwrap();
    std::fs::create_dir_all(main.join(".git")).unwrap();
    let args =
        crate::seat_clone::commit_sandbox_args(&real, &main, &[], &tmp.path().join("cfg"), &[])
            .unwrap();
    let real_os = real.as_os_str();
    assert!(args
        .windows(3)
        .any(|w| w[0] == "--bind" && w[1] == *real_os && w[2] == *real_os));
}

/// Advisor F6: a main checkout without the host's `user.name`/`user.email`
/// is refused before any sandbox runs; nothing is committed.
// trace:TASK-2712 | ai:claude
#[test]
fn task_2712_commit_pending_without_host_identity_commits_nothing() {
    let fx = fixture();
    let clone = seat_with_pending(&fx, "noident", "c0ffee271212", "no-ident");
    let base = git_ok(&clone, &["rev-parse", "HEAD"]);
    let _global = crate::test_env::EnvVarsGuard::set(&[
        ("GIT_CONFIG_GLOBAL", "/dev/null"),
        ("GIT_CONFIG_NOSYSTEM", "1"),
    ]);
    for key in ["user.name", "user.email"] {
        let _ = crate::test_git::git_command()
            .arg("-C")
            .arg(&fx.main)
            .args(["config", "--unset-all", key])
            .output();
    }
    let err = crate::seat_clone::commit_pending(&fx.main, &clone, "no-ident", "fix: x (BUG-1)")
        .unwrap_err()
        .to_string();
    assert!(err.contains("user.name"), "{err}");
    assert!(err.contains("nothing was committed"), "{err}");
    assert_eq!(git_ok(&clone, &["rev-parse", "HEAD"]), base);
}

/// Advisor F6, where bwrap runs: `git add` succeeds, then the commit fails
/// (the branch's ref cannot be written); the index is restored to its prior
/// tree, so nothing is left staged, and the working tree keeps the work.
// trace:TASK-2712 | ai:claude
#[cfg(target_os = "linux")]
#[test]
fn task_2712_failed_commit_after_add_restores_the_index() {
    use std::os::unix::fs::PermissionsExt as _;
    if !bwrap_live_or_skip() {
        return;
    }
    let fx = fixture();
    let clone = seat_with_pending(&fx, "restore", "c0ffee271213", "restore");
    git_ok(&fx.main, &["config", "user.name", "Host Operator"]);
    git_ok(&fx.main, &["config", "user.email", "host@example.com"]);
    let base = git_ok(&clone, &["rev-parse", "HEAD"]);
    let heads = clone.join(".git/refs/heads");
    std::fs::set_permissions(&heads, std::fs::Permissions::from_mode(0o555)).unwrap();
    let logs = clone.join(".git/logs/refs/heads");
    if logs.is_dir() {
        std::fs::set_permissions(&logs, std::fs::Permissions::from_mode(0o555)).unwrap();
    }
    let result =
        crate::seat_clone::commit_pending(&fx.main, &clone, "restore", "fix: restore (BUG-1)");
    for locked_dir in [&heads, &logs].into_iter().filter(|dir| dir.is_dir()) {
        crate::test_exec::mark_executable(&locked_dir);
    }
    let err = format!("{:#}", result.unwrap_err());
    assert!(err.contains("could not commit"), "{err}");
    assert_eq!(git_ok(&clone, &["rev-parse", "HEAD"]), base);
    assert_eq!(
        git_ok(&clone, &["diff", "--cached", "--name-only"]),
        "",
        "the index was left staged"
    );
    assert!(clone.join("restore.txt").exists(), "the work is kept");
}

/// Advisor F1: the lane's own post-exit step commits what a native child
/// left pending in its seat clone, with the spec's trailer, through the
/// commit step; a tree that is not a recorded seat clone is left alone, and
/// a clean seat commits nothing.
// trace:TASK-2712 | ai:claude
#[test]
fn task_2712_lane_post_exit_commits_pending_seat_work() {
    use crate::ship_from_seat::{post_exit_commit, post_exit_message, PostExitCommit};
    let fx = fixture();
    let clone = seat_with_publication_pending(&fx, "postexit", "c0ffee271214", "post-exit");
    let base = git_ok(&clone, &["rev-parse", "HEAD"]);
    let outcome = post_exit_commit(&fx.main, &clone, "BUG-1", "lane-7", true, &test_commit);
    let tip = git_ok(&clone, &["rev-parse", "HEAD"]);
    assert_ne!(tip, base, "the host made no commit");
    assert_eq!(outcome, PostExitCommit::Committed(tip));
    let subject = git_ok(&clone, &["log", "-1", "--format=%s"]);
    assert_eq!(
        subject,
        post_exit_message(
            crate::load_store_for_lookup(&fx.main)
                .as_ref()
                .and_then(|store| store.get_requirement_by_spec_id("BUG-1")),
            "BUG-1",
            "lane-7",
            true
        )
        .unwrap()
    );
    assert!(subject.ends_with("(BUG-1)"), "{subject}");
    assert_eq!(git_ok(&clone, &["status", "--porcelain"]), "");
    // Nothing pending now.
    assert_eq!(
        post_exit_commit(&fx.main, &clone, "BUG-1", "lane-7", true, &test_commit),
        PostExitCommit::Clean
    );
    // Not a seat clone: untouched, the commit step never runs.
    assert_eq!(
        post_exit_commit(&fx.main, &fx.main, "BUG-1", "lane-7", true, &commit_never),
        PostExitCommit::NotASeat
    );
}

/// Advisor F4: the leftovers of a lane whose child did NOT exit cleanly are
/// salvage: committed with `(Refs SPEC)`, never a completing trailer.
// trace:TASK-2712 | ai:claude
#[test]
fn task_2712_post_exit_salvage_after_failed_exit_uses_refs_trailer() {
    use crate::ship_from_seat::{post_exit_commit, post_exit_message, PostExitCommit};
    let fx = fixture();
    let clone = seat_with_publication_pending(&fx, "salvage", "c0ffee271216", "salvage");
    let outcome = post_exit_commit(&fx.main, &clone, "BUG-1", "lane-9", false, &test_commit);
    let tip = git_ok(&clone, &["rev-parse", "HEAD"]);
    assert_eq!(outcome, PostExitCommit::Committed(tip));
    let subject = git_ok(&clone, &["log", "-1", "--format=%s"]);
    assert_eq!(
        subject,
        post_exit_message(None, "BUG-1", "lane-9", false).unwrap()
    );
    assert!(subject.ends_with("(Refs BUG-1)"), "{subject}");
    assert!(!subject.contains(" (BUG-1)"), "{subject}");
}

/// Origin-scoped native lanes must publish canonical trailers even when HOME
/// aliases are installed. Exercise both completion and failed-exit salvage,
/// with and without an agreed display ID, through the real trailer guard/push.
// trace:BUG-2459 | ai:codex
#[test]
fn bug_2459_post_exit_origin_scope_publishes_without_home_alias_trailers() {
    use crate::ship_from_seat::{post_exit_commit, PostExitCommit};
    use aida_core::node_alias::{install_for_thread, NodeAliasBinding, NodeAliasMap};
    let node = "b6b1278890774ef993d2ebc327862470";
    let origin = format!("BUG-{node}-225");
    let _aliases = install_for_thread(NodeAliasMap {
        bindings: vec![NodeAliasBinding {
            label: "HOME".into(),
            node: node.into(),
            retired: false,
        }],
        ..Default::default()
    });
    assert_eq!(
        aida_core::node_alias::display_spec_id(&origin),
        "BUG-HOME-225"
    );
    for agreed in [Some("BUG-2459"), None] {
        for clean_exit in [true, false] {
            let fx = fixture();
            let clone = seat_with_pending(&fx, "alias", "c0ffee245900", "alias-publish");
            let lease = crate::lease_path(&fx.main, "c0ffee245900");
            let text = std::fs::read_to_string(&lease).unwrap();
            std::fs::write(&lease, text.replace("TASK-2053", &origin)).unwrap();
            let mut req = title_spec("Publish native lane work");
            req.spec_id = Some(origin.clone());
            req.agreed_id = agreed.map(str::to_string);
            let mut store = aida_core::RequirementsStore::new();
            store.requirements.push(req);
            aida_core::Storage::new(fx.main.join("requirements.yaml"))
                .save(&store)
                .unwrap();
            let outcome = post_exit_commit(
                &fx.main,
                &clone,
                &origin,
                "lane-alias",
                clean_exit,
                &test_commit,
            );
            let tip = git_ok(&clone, &["rev-parse", "HEAD"]);
            assert_eq!(outcome, PostExitCommit::Committed(tip.clone()));
            let subject = git_ok(&clone, &["log", "-1", "--format=%s"]);
            let id = agreed.unwrap_or(&origin);
            let trailer = if clean_exit {
                format!("({id})")
            } else {
                format!("(Refs {id})")
            };
            assert!(subject.ends_with(&trailer), "{subject}");
            // trace:BUG-2461 | ai:codex
            if clean_exit {
                assert!(subject.starts_with("[AI:codex] fix("), "{subject}");
                assert!(
                    subject.ends_with(&format!(": Publish native lane work {trailer}")),
                    "{subject}"
                );
            }
            assert!(!subject.contains("HOME"), "{subject}");
            let mut options = opts(&clone, false, false);
            options.spec = Some(origin.clone());
            let forge = crate::forge::fake::RecordingForge {
                opens_as: Some(77),
                ..crate::forge::fake::RecordingForge::new()
            };
            crate::ship_from_seat::run_from(
                &fx.main,
                options,
                &crate::ship_from_seat::Deps {
                    bundle: &test_bundle,
                    commit: &no_commit,
                    forge: &|_| Box::new(forge.clone()),
                    after_open: &|_, _| {},
                },
            )
            .unwrap();
            assert_eq!(
                git_ok(&fx.origin, &["rev-parse", "refs/heads/alias-publish"]),
                tip
            );
            // trace:BUG-2461 | ai:codex
            assert_eq!(forge.opened.lock().unwrap()[0].title, subject);
            assert!(import_refs(&fx.main).is_empty());
        }
    }
}

/// Advisor F5: the index is rolled back only while HEAD has not moved.
// trace:TASK-2712 | ai:claude
#[test]
fn task_2712_index_restore_only_while_head_unmoved() {
    use crate::seat_clone::index_restore_allowed;
    assert!(index_restore_allowed(Some("abc"), Some("abc")));
    assert!(
        !index_restore_allowed(Some("abc"), Some("def")),
        "commit landed"
    );
    assert!(
        !index_restore_allowed(Some("abc"), None),
        "HEAD unreadable now"
    );
    assert!(
        !index_restore_allowed(None, Some("abc")),
        "HEAD unknown before"
    );
    assert!(!index_restore_allowed(None, None));
}

/// Advisor F1 end to end where bwrap runs: the post-exit step with the
/// production commit (AIDA's own sandbox, hooks off, the host's identity).
// trace:TASK-2712 | ai:claude
#[cfg(target_os = "linux")]
#[test]
fn task_2712_lane_post_exit_commit_runs_the_sandboxed_host_commit() {
    if !bwrap_live_or_skip() {
        return;
    }
    let fx = fixture();
    let clone = seat_with_publication_pending(&fx, "postlive", "c0ffee271215", "post-live");
    git_ok(&fx.main, &["config", "user.name", "Host Operator"]);
    git_ok(&fx.main, &["config", "user.email", "host@example.com"]);
    let sentinel = fx.root.join("hook-ran");
    plant_hooks(&clone, &sentinel, &fx.root);
    let base = git_ok(&clone, &["rev-parse", "HEAD"]);
    let outcome = crate::ship_from_seat::post_exit_commit(
        &fx.main,
        &clone,
        "BUG-1",
        "lane-8",
        true,
        &crate::seat_clone::commit_pending,
    );
    let tip = git_ok(&clone, &["rev-parse", "HEAD"]);
    assert_ne!(tip, base, "{outcome:?}");
    assert_eq!(
        outcome,
        crate::ship_from_seat::PostExitCommit::Committed(tip)
    );
    assert_eq!(
        git_ok(&clone, &["log", "-1", "--format=%an <%ae>"]),
        "Host Operator <host@example.com>"
    );
    assert!(!sentinel.exists(), "a seat hook ran");
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
        commit: &no_commit,
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
        commit: &no_commit,
        forge: &|_| panic!("forge reached despite a spec mismatch"),
        after_open: &|_, _| {},
    };
    let err = crate::ship_from_seat::run_from(&fx.main, opts(&clone, false, true), &deps)
        .unwrap_err()
        .to_string();
    assert!(err.contains("you named BUG-1"), "{err}");
    assert!(err.contains("BUG-2"), "{err}");
    assert!(!ref_exists(&fx.origin, "refs/heads/seat-mismatch"));
    // TASK-138: the refusal after import leaves no quarantine ref.
    assert_eq!(import_refs(&fx.main), "", "the refused import kept its ref");

    let ids = vec!["BUG-2".to_string(), "TASK-7".to_string()];
    crate::ship_from_seat::explicit_spec_check(None, "bug-2", &ids).unwrap();
    crate::ship_from_seat::explicit_spec_check(None, "TASK-7", &ids).unwrap();
    assert!(crate::ship_from_seat::explicit_spec_check(None, "BUG-3", &ids).is_err());
}

/// `--from-seat` run from inside a seat clone is refused before anything.
#[test]
fn bug_home_038_from_seat_inside_a_clone_is_refused() {
    let fx = fixture();
    let (clone, _) = seat_with_commit(&fx, "inside", "c0ffee038008", "inside", "fix: x (BUG-1)");
    let deps = crate::ship_from_seat::Deps {
        bundle: &|_, _, _| panic!("bundled from inside a clone"),
        commit: &no_commit,
        forge: &|_| panic!("forge reached from inside a clone"),
        after_open: &|_, _| {},
    };
    let err = crate::ship_from_seat::run_from(&clone, opts(&clone, false, true), &deps)
        .unwrap_err()
        .to_string();
    assert!(err.contains("runs from the main checkout"), "{err}");
}

/// Ruling item 2: a seat commit with no `(SPEC-ID)` trailer is refused
/// before the push, and (TASK-138) its quarantine ref is deleted; a dead
/// trailer is refused by the STORY-469 check.
#[test]
fn bug_home_038_from_seat_trailer_check_runs_before_push() {
    let fx = fixture();
    let (clone, _) = seat_with_commit(&fx, "bare", "c0ffee038009", "bare", "fix: no trailer");
    let deps = crate::ship_from_seat::Deps {
        bundle: &test_bundle,
        commit: &no_commit,
        forge: &|_| panic!("forge reached despite a refused trailer"),
        after_open: &|_, _| {},
    };
    let err = crate::ship_from_seat::run_from(&fx.main, opts(&clone, false, true), &deps)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("carries a `(SPEC-ID)` or `(Refs SPEC-ID)` trailer"),
        "{err}"
    );
    assert!(!ref_exists(&fx.origin, "refs/heads/bare"));
    assert_eq!(import_refs(&fx.main), "", "the refused import kept its ref");

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

/// BUG-2431: a seat commit whose only spec reference is `(Refs SPEC-ID)` —
/// a partial slice of a multi-PR spec — is published under the named spec,
/// its PR titled with the `Refs` subject (which merge-time completion skips),
/// and a dead `Refs` ID is still refused.
// trace:BUG-2431 | ai:claude
#[test]
fn bug_2431_from_seat_accepts_refs_only_trailer() {
    let fx = fixture();
    let (clone, tip) = seat_with_commit(
        &fx,
        "slice",
        "c0ffee03800d",
        "seat-slice",
        "feat: step one of the spec (Refs BUG-1)",
    );
    let forge = crate::forge::fake::RecordingForge {
        opens_as: Some(78),
        ..crate::forge::fake::RecordingForge::new()
    };
    let deps = crate::ship_from_seat::Deps {
        bundle: &test_bundle,
        commit: &no_commit,
        forge: &|_| Box::new(forge.clone()) as Box<dyn crate::forge::Forge>,
        after_open: &|_, _| {},
    };
    crate::ship_from_seat::run_from(&fx.main, opts(&clone, false, false), &deps).unwrap();
    assert_eq!(
        git_ok(&fx.origin, &["rev-parse", "refs/heads/seat-slice"]),
        tip
    );
    let opened = forge.opened.lock().unwrap().clone();
    assert_eq!(opened.len(), 1);
    assert_eq!(opened[0].title, "feat: step one of the spec (Refs BUG-1)");
    assert!(
        crate::pr_ship::extract_trailing_spec_ids_from_subject(&opened[0].title).is_empty(),
        "a Refs-only PR title must not name a spec for completion"
    );

    let base = git_ok(&fx.main, &["rev-parse", "origin/main"]);
    let range = format!("{base}..HEAD");
    let ids = crate::ship_from_seat::seat_trailer_check(&clone, &range, |_| {
        crate::trace::SpecResolution::Live
    })
    .unwrap();
    assert_eq!(ids, vec!["BUG-1".to_string()]);
    crate::ship_from_seat::explicit_spec_check(None, "bug-1", &ids).unwrap();
    let err = crate::ship_from_seat::seat_trailer_check(&clone, &range, |_| {
        crate::trace::SpecResolution::Missing
    })
    .unwrap_err()
    .to_string();
    assert!(err.contains("BUG-1"), "{err}");
}

/// Write `reference` at `oid` in `repo` with a reflog entry dated `at`
/// (unix seconds), or with no reflog when `at` is `None`.
fn quarantine_ref_at(repo: &Path, reference: &str, oid: &str, at: Option<u64>) {
    let mut cmd = crate::test_git::git_command();
    cmd.arg("-C").arg(repo);
    if let Some(at) = at {
        cmd.env("GIT_COMMITTER_DATE", format!("@{at} +0000"))
            .args(["-c", "core.logAllRefUpdates=always"]);
    } else {
        cmd.args(["-c", "core.logAllRefUpdates=false"]);
    }
    let out = cmd.args(["update-ref", reference, oid]).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// TASK-138: quarantine refs imported more than a day ago are pruned (by
/// their reflog's import time, or, with no reflog, their commit date);
/// fresh ones and refs outside the quarantine prefix are kept.
// trace:TASK-B6B1278890774EF993D2EBC327862470-138 | ai:claude
#[test]
fn ship_from_seat_prunes_only_stale_quarantine_refs() {
    let fx = fixture();
    let fresh_commit = orphan_commit(&fx.main, "fresh-commit", "f.txt");
    let now = unix_now();
    let two_days = 2 * crate::ship_from_seat::QUARANTINE_MAX_AGE_SECS;
    let old_commit = {
        let out = crate::test_git::git_command()
            .arg("-C")
            .arg(&fx.main)
            .env("GIT_COMMITTER_DATE", format!("@{} +0000", now - two_days))
            .args([
                "commit-tree",
                "-m",
                "old",
                &format!("{fresh_commit}^{{tree}}"),
            ])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    let p = crate::ship_from_seat::QUARANTINE_PREFIX;
    quarantine_ref_at(
        &fx.main,
        &format!("{p}old"),
        &fresh_commit,
        Some(now - two_days),
    );
    quarantine_ref_at(&fx.main, &format!("{p}new"), &fresh_commit, Some(now - 60));
    quarantine_ref_at(&fx.main, &format!("{p}nolog-old"), &old_commit, None);
    quarantine_ref_at(&fx.main, &format!("{p}nolog-new"), &fresh_commit, None);
    quarantine_ref_at(
        &fx.main,
        "refs/aida/other/old",
        &fresh_commit,
        Some(now - two_days),
    );

    let mut pruned = crate::ship_from_seat::prune_stale_quarantine(
        &fx.main,
        now,
        crate::ship_from_seat::QUARANTINE_MAX_AGE_SECS,
    );
    pruned.sort();
    assert_eq!(pruned, vec![format!("{p}nolog-old"), format!("{p}old")]);
    assert_eq!(
        import_refs(&fx.main),
        format!("{p}new\n{p}nolog-new"),
        "a fresh quarantine ref was pruned"
    );
    assert!(ref_exists(&fx.main, "refs/aida/other/old"));
}

/// TASK-138: every `--from-seat` run (a dry run included) prunes stale
/// quarantine refs first, and a real import's ref carries a reflog, so it
/// ages by when the host imported it.
// trace:TASK-B6B1278890774EF993D2EBC327862470-138 | ai:claude
#[test]
fn ship_from_seat_run_prunes_stale_quarantine_and_reflogs_imports() {
    let fx = fixture();
    let (clone, tip) = seat_with_commit(
        &fx,
        "prune",
        "c0ffee03800d",
        "seat-prune",
        "fix: seat change (BUG-1)",
    );
    let stale = format!("{}left-behind", crate::ship_from_seat::QUARANTINE_PREFIX);
    let long_ago = unix_now() - 2 * crate::ship_from_seat::QUARANTINE_MAX_AGE_SECS;
    let base = git_ok(&fx.main, &["rev-parse", "origin/main"]);
    quarantine_ref_at(&fx.main, &stale, &base, Some(long_ago));
    let deps = crate::ship_from_seat::Deps {
        bundle: &test_bundle,
        commit: &no_commit,
        forge: &|_| panic!("a dry run reached the forge"),
        after_open: &|_, _| {},
    };
    crate::ship_from_seat::run_from(&fx.main, opts(&clone, true, false), &deps).unwrap();
    assert!(
        !ref_exists(&fx.main, &stale),
        "the stale ref survived the run"
    );

    let bytes = test_bundle(&fx.main, &clone, "seat-prune").unwrap();
    let staged =
        crate::ship_from_seat::stage_bundle(&fx.main, "seat-prune", &bytes, false).unwrap();
    assert_eq!(staged.tip, tip);
    let reflog = git_ok(
        &fx.main,
        &["reflog", "show", "--format=%H", &staged.reference],
    );
    assert_eq!(reflog, tip, "the import wrote no reflog entry");
    assert!(crate::ship_from_seat::prune_stale_quarantine(
        &fx.main,
        unix_now(),
        crate::ship_from_seat::QUARANTINE_MAX_AGE_SECS
    )
    .is_empty());
}

/// TASK-140: `--from-seat` never merges, so `--no-merge` with it is a parse
/// error instead of being accepted and ignored.
// trace:TASK-B6B1278890774EF993D2EBC327862470-140 | ai:claude
#[test]
fn ship_from_seat_conflicts_with_no_merge() {
    use clap::Parser as _;
    let err =
        crate::pr_cmd::ShipArgs::try_parse_from(["ship", "--from-seat", "seat", "--no-merge"])
            .unwrap_err();
    assert_eq!(
        err.kind(),
        clap::error::ErrorKind::ArgumentConflict,
        "{err}"
    );
    let ok = crate::pr_cmd::ShipArgs::try_parse_from(["ship", "--from-seat", "seat", "--no-pr"])
        .unwrap();
    assert!(ok.no_pr && !ok.no_merge);
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

// trace:TASK-2891 | ai:codex
fn title_spec(title: &str) -> aida_core::Requirement {
    let mut req = aida_core::Requirement::new(title.into(), String::new());
    req.spec_id = Some("BUG-1".into());
    req.req_type = aida_core::RequirementType::Bug;
    req.feature = "Regeneration Controls".into();
    req
}

// trace:TASK-2891 | ai:codex
#[test]
fn task_2891_spec_title_preserves_attribution_and_authority() {
    use crate::ship_from_seat::seat_pr_title;
    let req = title_spec("Validate committed fractional aggregates");
    for trailer in [
        "(BUG-1)",
        "(BUG-1 TASK-2)",
        "(Refs BUG-1)",
        "(Refs BUG-1) (TASK-2)",
    ] {
        let subject = format!("[AI:codex] fix: address CI failures and review findings {trailer}");
        let title = seat_pr_title(Some(&req), &subject).unwrap();
        assert_eq!(
            title,
            format!("[AI:codex] fix: Validate committed fractional aggregates {trailer}")
        );
        assert_eq!(
            crate::pr_ship::extract_trailing_spec_ids_from_subject(&title),
            crate::pr_ship::extract_trailing_spec_ids_from_subject(&subject)
        );
        assert_eq!(
            crate::pr_ship::refs_spec_ids_in_text(&title),
            crate::pr_ship::refs_spec_ids_in_text(&subject)
        );
    }
    let title = seat_pr_title(
        Some(&req),
        "fix: (Refs BUG-1) salvage leftovers (BUG-1 TASK-2)",
    )
    .unwrap();
    assert_eq!(crate::pr_ship::refs_spec_ids_in_text(&title), vec!["BUG-1"]);
    let req = title_spec("fix(regen): validate committed fractional aggregates");
    assert_eq!(
        seat_pr_title(
            Some(&req),
            "[AI:claude] chore(seat): address CI failures and review findings (BUG-1)"
        )
        .unwrap(),
        "[AI:claude] fix(regen): validate committed fractional aggregates (BUG-1)"
    );
    assert!(seat_pr_title(
        Some(&title_spec("Validate aggregates (TASK-99)")),
        "fix: address CI failures and review findings"
    )
    .is_err());
    assert!(seat_pr_title(
        Some(&title_spec("Validate (Refs TASK-99) aggregates")),
        "fix: address CI failures and review findings (BUG-1)"
    )
    .is_err());
    let req = title_spec("Validate aggregates");
    assert_eq!(
        seat_pr_title(Some(&req), "fix: address CI failures and review findings").unwrap(),
        "fix: Validate aggregates"
    );
    assert!(seat_pr_title(None, "fix: address CI failures and review findings (BUG-1)").is_err());
    assert_eq!(
        seat_pr_title(None, "fix(regen): validate aggregates (BUG-1)").unwrap(),
        "fix(regen): validate aggregates (BUG-1)"
    );
    assert!(seat_pr_title(
        Some(&title_spec("  ")),
        "fix: address CI failures and review findings (BUG-1)"
    )
    .is_err());
}

/// Drive real bundle publication with a spec-backed title, then the rework
/// path against an existing generic PR. Failed retitles must not report a
/// successful review handoff; descriptive author titles remain untouched.
// trace:TASK-2891 | ai:codex
#[test]
fn task_2891_publication_opens_and_repairs_spec_titles() {
    let fx = fixture();
    let (clone, _) = seat_with_commit(
        &fx,
        "titles",
        "c0ffee289100",
        "seat-titles",
        "[AI:codex] fix: address CI failures and review findings (BUG-1)",
    );
    let mut store = aida_core::RequirementsStore::new();
    store.requirements.push(title_spec(
        "fix(regen): validate committed fractional aggregates",
    ));
    aida_core::Storage::new(fx.main.join("requirements.yaml"))
        .save(&store)
        .unwrap();
    let mut forge = crate::forge::fake::RecordingForge {
        opens_as: Some(77),
        ..crate::forge::fake::RecordingForge::new()
    };
    let handoffs = std::cell::Cell::new(0);
    let run = |forge: &crate::forge::fake::RecordingForge| {
        crate::ship_from_seat::run_from(
            &fx.main,
            opts(&clone, false, false),
            &crate::ship_from_seat::Deps {
                bundle: &test_bundle,
                commit: &no_commit,
                forge: &|_| Box::new(forge.clone()),
                after_open: &|_, _| handoffs.set(handoffs.get() + 1),
            },
        )
    };
    let expected = "[AI:codex] fix(regen): validate committed fractional aggregates (BUG-1)";
    run(&forge).unwrap();
    assert_eq!(forge.opened.lock().unwrap()[0].title, expected);
    forge.open_for_branch = crate::forge::ChangeLookup::Found(crate::forge::ChangeRef {
        id: 77,
        url: "https://example.test/pull/77".into(),
        branch: "seat-titles".into(),
        base: "main".into(),
        title: Some(
            "[AI:codex] fix: Address CI failures and review findings   (BUG-1 TASK-2)".into(),
        ),
    });
    std::fs::remove_file(fx.main.join("requirements.yaml")).unwrap();
    let err = run(&forge).unwrap_err().to_string();
    assert!(
        err.contains("the owning spec is unavailable; restore store access and retry"),
        "{err}"
    );
    assert_eq!(
        handoffs.get(),
        1,
        "unavailable spec must prevent after_open"
    );
    assert!(forge.retitled.lock().unwrap().is_empty());
    aida_core::Storage::new(fx.main.join("requirements.yaml"))
        .save(&store)
        .unwrap();
    forge.retitle_error = Some("forge edit refused".into());
    assert!(run(&forge)
        .unwrap_err()
        .to_string()
        .contains("forge edit refused"));
    assert_eq!(handoffs.get(), 1);
    forge.retitle_error = None;
    run(&forge).unwrap();
    assert_eq!(
        *forge.retitled.lock().unwrap(),
        vec![(77, expected.replace(" (BUG-1)", "   (BUG-1 TASK-2)"))]
    );
    if let crate::forge::ChangeLookup::Found(change) = &mut forge.open_for_branch {
        change.title = Some("fix(regen): author-selected descriptive summary (BUG-1)".into());
    }
    run(&forge).unwrap();
    assert_eq!(forge.retitled.lock().unwrap().len(), 1);
    assert_eq!(forge.opened.lock().unwrap().len(), 1);
    assert_eq!(handoffs.get(), 3);
}

// trace:TASK-2891 | ai:codex
#[test]
fn task_2891_default_type_scope_and_single_line_summary() {
    use aida_core::RequirementType;
    for (kind, prefix) in [
        (RequirementType::Bug, "fix"),
        (RequirementType::Task, "chore"),
        (RequirementType::Story, "feat"),
        (RequirementType::Doc, "docs"),
    ] {
        let mut req = title_spec("Repair\n   publication titles");
        req.req_type = kind;
        req.feature = "Uncategorized".into();
        assert_eq!(
            crate::ship_from_seat::seat_pr_title(
                Some(&req),
                "[AI:codex] address CI failures and review findings (BUG-1)"
            )
            .unwrap(),
            format!("[AI:codex] {prefix}: Repair publication titles (BUG-1)")
        );
    }
}

// trace:TASK-2891 | ai:codex
#[test]
fn task_2891_descriptive_subject_survives_truncated_finding_title() {
    let mut req = title_spec(
        "cosmetic: squash title 'fix: address CI failures and review findings' is generi…",
    );
    req.req_type = aida_core::RequirementType::Task;
    for subject in [
        "[AI:codex] fix(ship): derive seat PR titles from owning specs (TASK-2891)",
        "fix: foo(BUG-1)",
    ] {
        assert_eq!(
            crate::ship_from_seat::seat_pr_title(Some(&req), subject).unwrap(),
            subject
        );
    }
}

// trace:TASK-2891 | ai:codex
#[test]
fn task_2891_generic_subject_normalization_and_adjacent_credit() {
    let mut req = title_spec("Repair publication titles");
    req.req_type = aida_core::RequirementType::Task;
    for (subject, suffix) in [
        (
            "[AI:codex] fix(ship): Address CI failures and review findings(BUG-1)",
            "(BUG-1)",
        ),
        (
            "[AI:codex] fix(ship): ADDRESS CI FAILURES AND REVIEW FINDINGS   (BUG-1)",
            "   (BUG-1)",
        ),
    ] {
        assert_eq!(
            crate::ship_from_seat::seat_pr_title(Some(&req), subject).unwrap(),
            format!("[AI:codex] fix(ship): Repair publication titles{suffix}")
        );
        let err = crate::ship_from_seat::seat_pr_title(None, subject)
            .unwrap_err()
            .to_string();
        assert!(err.contains("the owning spec is unavailable"), "{err}");
    }
}

/// The final lane-state reader uses clone facts and the recorded host's
/// origin, even though ordinary host Git is forbidden in the clone.
// trace:STORY-2475 | ai:codex
#[test]
fn story_2475_final_seat_state_reads_recorded_clone_through_its_host() {
    let fx = fixture();
    let clone = leased_clone(&fx, "final-state", "c0ffee247500");
    std::fs::write(clone.join("pending"), "unfinished edit").unwrap();
    let expected = crate::seat_clone::probe(&fx.main, &clone);
    let state = crate::headless_background::SeatState::read(&clone, "main");
    assert_eq!(
        state.origin,
        crate::headless_background::OriginHead::At(git_ok(&fx.main, &["rev-parse", "HEAD"]))
    );
    match expected {
        Probe::Known(facts) => {
            assert!(!facts.dirty.is_empty());
            assert_eq!(state.head, facts.head);
            assert_eq!(state.uncommitted, Some(facts.dirty.len()));
            // trace:BUG-2464 | ai:codex
            assert_eq!(state.head_has_unpushed, Some(false));
        }
        Probe::Unknown(_) | Probe::HasSubmodules(_) => {
            assert_eq!(state.head, None);
            assert_eq!(state.uncommitted, None);
        }
    }
}

// trace:TASK-2879 | ai:codex
#[test]
fn task_2879_native_queue_done_uses_lease_branch_before_diagnostics() {
    let fx = fixture();
    let clone = seat_with_pending(&fx, "native-done", "c0ffee287901", "pending");
    let branch = git_ok(&clone, &["branch", "--show-current"]);
    let why = crate::queue_misc::refuse_native_queue_done(&clone)
        .unwrap_err()
        .to_string();
    assert!(why.contains(&format!("branch `{branch}`")), "{why}");
    assert!(why.contains("no status or queue entry was changed"));
    assert!(!why.contains("current_branch_at returned None"));
    assert!(!why.contains("no test file"));
    assert!(clone.join("native-done.txt").exists());
    crate::queue_misc::refuse_native_queue_done(&fx.main).unwrap();
}

// trace:TASK-2879 | ai:codex
#[cfg(target_os = "linux")]
#[test]
fn task_2879_publication_merge_uses_host_identity_without_seat_hooks() {
    if !bwrap_live_or_skip() {
        return;
    }
    let fx = fixture();
    let clone = seat_with_pending(&fx, "publish-merge", "c0ffee287902", "publish-merge");
    test_commit(&fx.main, &clone, "publish-merge", "lane work (Refs BUG-1)").unwrap();
    let lane_head = git_ok(&clone, &["rev-parse", "HEAD"]);
    std::fs::write(fx.main.join("remote.txt"), "remote change\n").unwrap();
    git_ok(&fx.main, &["add", "remote.txt"]);
    git_ok(&fx.main, &["commit", "-qm", "remote work"]);
    let remote = git_ok(&fx.main, &["rev-parse", "HEAD"]);
    let sentinel = fx.root.join("merge-hook-ran");
    plant_hooks(&clone, &sentinel, &fx.root);
    let message = "merge remote PR head (Refs BUG-1)";
    let pinned =
        crate::ship_from_seat::pending_commit_preflight(&fx.main, &clone, Some("BUG-1"), message)
            .unwrap();
    crate::seat_clone::merge_publication_head(
        &fx.main,
        &clone,
        "publish-merge",
        &remote,
        message,
        pinned.pinned,
    )
    .unwrap();
    assert_eq!(git_ok(&clone, &["rev-parse", "HEAD^1"]), lane_head);
    assert_eq!(git_ok(&clone, &["rev-parse", "HEAD^2"]), remote);
    assert_eq!(
        git_ok(&clone, &["log", "-1", "--format=%an <%ae>"]),
        git_ok(&fx.main, &["log", "-1", "--format=%an <%ae>"])
    );
    assert!(
        !sentinel.exists(),
        "a seat hook ran during publication merge"
    );
}

// Exercise the production host/sandbox/bundle/push path, including an
// explicitly supplied spec on a branch and scope which name no spec.
// trace:TASK-2879 | ai:codex
#[cfg(target_os = "linux")]
#[test]
fn task_2879_run_at_reconciles_moved_origin_and_refuses_unsafe_merges() {
    if !bwrap_live_or_skip() {
        return;
    }
    for case in [
        "merge",
        "live",
        "conflict",
        "bad-trailer",
        "wrong-spec",
        "push-refused",
    ] {
        let fx = fixture();
        let branch = "publication";
        let id = "c0ffee287910";
        let subject = match case {
            "bad-trailer" => "local work without trailer",
            "wrong-spec" => "local work (Refs BUG-2)",
            _ => "local work (Refs BUG-1)",
        };
        let (clone, _) = seat_with_commit(&fx, "moved", id, branch, subject);
        let lease = crate::lease_path(&fx.main, id);
        let text = std::fs::read_to_string(&lease)
            .unwrap()
            .replace("TASK-2053", "publication");
        std::fs::write(&lease, &text).unwrap();
        if case == "conflict" {
            std::fs::write(clone.join("README.md"), "local conflict\n").unwrap();
            git_ok(&clone, &["add", "README.md"]);
            git_ok(
                &clone,
                &["commit", "-qm", "conflicting local work (Refs BUG-1)"],
            );
        }
        let local = git_ok(&clone, &["rev-parse", "HEAD"]);
        let remote_file = if case == "conflict" {
            "README.md"
        } else {
            "remote.txt"
        };
        std::fs::write(fx.main.join(remote_file), "remote change\n").unwrap();
        git_ok(&fx.main, &["add", remote_file]);
        git_ok(&fx.main, &["commit", "-qm", "remote work (Refs BUG-1)"]);
        let remote = git_ok(&fx.main, &["rev-parse", "HEAD"]);
        git_ok(
            &fx.main,
            &["push", "-q", "origin", &format!("HEAD:refs/heads/{branch}")],
        );
        std::fs::write(fx.main.join(".git/FETCH_HEAD"), "preserve fetch head\n").unwrap();
        let sentinel = fx.root.join("seat-hook-ran");
        plant_hooks(&clone, &sentinel, &fx.root);
        let mut owner = if case == "live" {
            let owner = std::process::Command::new("sleep")
                .arg("60")
                .spawn()
                .unwrap();
            let start = aida_core::liveness::process_start_identity(owner.id()).unwrap();
            std::fs::write(
                &lease,
                format!(
                    "{text}active_pid = {}\nactive_pid_start_time = {}\n",
                    owner.id(),
                    aida_core::toml_quote::toml_string(&start)
                ),
            )
            .unwrap();
            Some(owner)
        } else {
            None
        };
        if case == "push-refused" {
            let hook = fx.origin.join("hooks/pre-receive");
            crate::test_exec::write_executable(&hook, "#!/bin/sh\nexit 1\n");
        }
        let result = crate::ship_from_seat::run_at(&fx.main, opts(&clone, false, true));
        if let Some(owner) = &mut owner {
            let _ = owner.kill();
            let _ = owner.wait();
        }
        if case == "merge" {
            result.unwrap();
            let head = git_ok(&clone, &["rev-parse", "HEAD"]);
            assert_eq!(
                git_ok(&fx.origin, &["rev-parse", &format!("refs/heads/{branch}")]),
                head
            );
            assert_eq!(git_ok(&clone, &["rev-parse", "HEAD^1"]), local);
            assert_eq!(git_ok(&clone, &["rev-parse", "HEAD^2"]), remote);
            assert_eq!(
                git_ok(&clone, &["log", "-1", "--format=%s"]),
                "[host] chore(seat): merge remote PR head (Refs BUG-1)"
            );
        } else {
            let why = format!("{:#}", result.unwrap_err());
            assert_eq!(
                git_ok(&fx.origin, &["rev-parse", &format!("refs/heads/{branch}")]),
                remote
            );
            if case == "push-refused" {
                assert_ne!(git_ok(&clone, &["rev-parse", "HEAD"]), local);
                assert_eq!(git_ok(&clone, &["rev-parse", "HEAD^2"]), remote);
                assert!(
                    why.contains("git push")
                        && why.contains("reconciled commit remains in the seat"),
                    "{why}"
                );
            } else {
                assert_eq!(git_ok(&clone, &["rev-parse", "HEAD"]), local);
                assert!(import_refs(&fx.main).is_empty());
                let expected = match case {
                    "live" => "refusing to merge the remote PR head",
                    "conflict" => "could not merge the remote PR head",
                    "bad-trailer" => "no seat commit",
                    "wrong-spec" => "you named BUG-1",
                    _ => unreachable!(),
                };
                assert!(why.contains(expected), "{case}: {why}");
            }
            assert!(!clone.join(".git/MERGE_HEAD").exists());
        }
        assert_eq!(
            std::fs::read_to_string(fx.main.join(".git/FETCH_HEAD")).unwrap(),
            "preserve fetch head\n"
        );
        assert!(git_ok(
            &fx.main,
            &[
                "for-each-ref",
                "--format=%(refname)",
                "refs/aida/publication/"
            ]
        )
        .is_empty());
        assert!(!sentinel.exists(), "seat hook ran for {case}");
    }
}

// A lane can publish already-committed work while still running, including
// when it has unrelated untracked scratch files. No merge means no seat write.
// trace:TASK-2879 | ai:codex
#[cfg(target_os = "linux")]
#[test]
fn task_2879_run_at_publishes_live_dirty_seat_without_unneeded_merge() {
    if !bwrap_live_or_skip() {
        return;
    }
    for remote_exists in [false, true] {
        let fx = fixture();
        let branch = "already-committed";
        let (clone, tip) = seat_with_commit(
            &fx,
            "committed",
            "c0ffee287911",
            branch,
            "work (Refs BUG-1)",
        );
        if remote_exists {
            git_ok(
                &fx.main,
                &[
                    "push",
                    "-q",
                    "origin",
                    &format!("origin/main:refs/heads/{branch}"),
                ],
            );
        }
        std::fs::write(clone.join("scratch.txt"), "scratch\n").unwrap();
        let mut owner = std::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .unwrap();
        let lease = crate::lease_path(&fx.main, "c0ffee287911");
        let text = std::fs::read_to_string(&lease).unwrap();
        let start = aida_core::liveness::process_start_identity(owner.id()).unwrap();
        std::fs::write(
            &lease,
            format!(
                "{text}active_pid = {}\nactive_pid_start_time = {}\n",
                owner.id(),
                aida_core::toml_quote::toml_string(&start)
            ),
        )
        .unwrap();
        let result = crate::ship_from_seat::run_at(&fx.main, opts(&clone, false, true));
        let _ = owner.kill();
        let _ = owner.wait();
        result.unwrap();
        assert_eq!(
            git_ok(&fx.origin, &["rev-parse", &format!("refs/heads/{branch}")]),
            tip
        );
        assert_eq!(git_ok(&clone, &["rev-parse", "HEAD"]), tip);
        assert_eq!(
            std::fs::read_to_string(clone.join("scratch.txt")).unwrap(),
            "scratch\n"
        );
    }
}

// Test the actual forge constructor/lookup used by post_exit_commit, not just
// title parsing: fake gh sees the recorded branch and returns a Refs PR.
// trace:TASK-2879 | ai:codex
#[cfg(unix)]
#[test]
fn task_2879_post_exit_commit_looks_up_refs_pr() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    // trace:BUG-2459 | ai:codex
    let _aliases = aida_core::node_alias::install_for_thread(aida_core::node_alias::NodeAliasMap {
        bindings: vec![aida_core::node_alias::NodeAliasBinding {
            label: "HOME".into(),
            node: "b6b1278890774ef993d2ebc327862470".into(),
            retired: false,
        }],
        ..Default::default()
    });
    let origin = "BUG-b6b1278890774ef993d2ebc327862470-225";
    let fx = fixture();
    // trace:TASK-2902 | ai:codex — create the seat before installing its origin mapping.
    let clone = seat_with_publication_pending(&fx, "refs-pr", "c0ffee287912", "refs-pr");
    let mut req = title_spec("Preserve partial work");
    req.spec_id = Some(origin.into());
    req.agreed_id = Some("BUG-1".into());
    let mut store = aida_core::RequirementsStore::new();
    store.requirements.push(req);
    aida_core::Storage::new(fx.main.join("requirements.yaml"))
        .save(&store)
        .unwrap();
    let bin = fx.root.join("bin");
    std::fs::create_dir(&bin).unwrap();
    let gh = bin.join("gh");
    crate::test_exec::write_executable(
        &gh,
        r##"#!/bin/sh
printf '%s\n' "$*" >> "$(dirname "$0")/called"
case "$*" in
  --version) printf '%s\n' 'gh version 2.0.0' ;;
  *"--head refs-pr"*) /bin/cat "$(dirname "$0")/response" ;;
  *) exit 23 ;;
esac
"##,
    );
    std::fs::create_dir_all(fx.main.join(".aida")).unwrap();
    std::fs::write(
        fx.main.join(".aida/config.toml"),
        "[forge]\nprovider = \"github\"\n",
    )
    .unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let _env = crate::test_env::EnvVarGuard::set("PATH", &path);
    for (title, trailer) in [
        ("partial work (Refs BUG-1)", "(Refs BUG-1)"),
        ("complete work (BUG-1)", "(BUG-1)"),
        ("inconclusive title", "(Refs BUG-1)"),
        ("lookup failure", "(Refs BUG-1)"),
    ] {
        for declared in [false, true] {
            crate::native_publication::prepare(&fx.main, &clone).unwrap();
            if declared {
                crate::native_publication::declare(
                    &clone,
                    None,
                    "[AI:codex] fix: requested completion (BUG-1)",
                    false,
                )
                .unwrap();
            }
            std::fs::write(
                bin.join("response"),
                format!("77\t{title}\thttps://example.test/pr/77\trefs-pr\n"),
            )
            .unwrap();
            if title == "lookup failure" {
                std::fs::remove_file(bin.join("response")).unwrap();
            }
            std::fs::write(clone.join("more-work.txt"), format!("{title} {declared}")).unwrap();
            let outcome = crate::ship_from_seat::post_exit_commit(
                &fx.main,
                &clone,
                origin,
                "lane",
                true,
                &test_commit,
            );
            assert!(
                matches!(outcome, crate::ship_from_seat::PostExitCommit::Committed(_)),
                "{outcome:?}"
            );
            let subject = git_ok(&clone, &["log", "-1", "--format=%s"]);
            assert!(subject.ends_with(trailer), "{subject}");
            let calls = std::fs::read_to_string(bin.join("called")).unwrap();
            assert!(calls.contains("--head refs-pr"), "{calls}");
            if declared {
                assert!(
                    subject.starts_with("[AI:codex] fix: requested completion"),
                    "{subject}"
                );
            }
        }
    }
}

// trace:BUG-2456 | ai:codex
#[cfg(target_os = "linux")]
#[test]
fn bug_2456_clone_rework_publishes_through_host_and_never_runs_seat_hooks() {
    if !bwrap_live_or_skip() {
        return;
    }
    for conflicts in [false, true] {
        let fx = fixture();
        let clone = leased_clone(&fx, "rework-merge", "c0ffee245600");
        git_ok(&fx.main, &["branch", "feat"]);
        git_ok(&fx.main, &["push", "-q", "origin", "feat"]);
        std::fs::write(
            clone.join(if conflicts { "README.md" } else { "other" }),
            "rework\n",
        )
        .unwrap();
        let before = commit_pending(&fx.main, &clone, "feat", "fix: rework (BUG-2456)")
            .unwrap()
            .unwrap();
        publish_rework_merge(&fx.main, &clone, "feat", &before).unwrap();
        std::fs::write(fx.main.join("README.md"), "main moved\n").unwrap();
        git_ok(&fx.main, &["commit", "-qam", "main moved"]);
        git_ok(&fx.main, &["push", "-q", "origin", "main"]);
        let sentinel = fx.root.join("hook-ran");
        plant_hooks(&clone, &sentinel, &fx.root);
        // The seat's origin cannot choose the publication destination.
        git_ok(
            &clone,
            &["remote", "set-url", "origin", "/nonexistent/seat-origin"],
        );
        let files = prepare_rework_merge(&fx.main, &clone, "feat", false).unwrap();
        let remote = git_ok(&fx.origin, &["rev-parse", "refs/heads/feat"]);
        let head = git_ok(&clone, &["rev-parse", "HEAD"]);
        if conflicts {
            assert_eq!(files, ["README.md"]);
            assert_eq!(head, before);
            assert_eq!(remote, before);
        } else {
            assert!(files.is_empty());
            assert_ne!(head, before);
            assert_eq!(remote, head);
            assert!(import_refs(&fx.main).is_empty());
            // Tip mismatch must clean up the imported ref before refusing.
            let why = publish_rework_merge(&fx.main, &clone, "feat", &before)
                .unwrap_err()
                .to_string();
            assert!(why.contains("changed before publication"), "{why}");
            assert!(import_refs(&fx.main).is_empty());

            // A concurrent origin update rejects the actual host push. The
            // clone import must be removed on this failure too.
            git_ok(&fx.main, &["checkout", "-qb", "racer", &head]);
            git_ok(
                &fx.main,
                &["commit", "--allow-empty", "-qm", "remote advances"],
            );
            let raced = git_ok(&fx.main, &["rev-parse", "HEAD"]);
            git_ok(&fx.main, &["push", "-q", "origin", "racer:refs/heads/feat"]);
            git_ok(&fx.main, &["checkout", "main"]);
            let why = publish_rework_merge(&fx.main, &clone, "feat", &head)
                .unwrap_err()
                .to_string();
            assert!(why.contains("rejected"), "{why}");
            assert!(import_refs(&fx.main).is_empty());
            assert_eq!(git_ok(&fx.origin, &["rev-parse", "refs/heads/feat"]), raced);
        }
        assert!(!sentinel.exists(), "seat hooks must never execute");
        assert!(git_ok(&clone, &["status", "--porcelain"]).is_empty());
    }
}

// trace:TASK-2902 | ai:codex
#[test]
fn task_2902_declared_subject_survives_host_commit_and_pr_title() {
    use crate::ship_from_seat::{post_exit_commit, PostExitCommit};
    let fx = fixture();
    let clone = seat_with_publication_pending(&fx, "declaration", "c0ffee290201", "declared");
    for (clean, requested, expected) in [
        (
            true,
            "[AI:codex] feat(seat): partial slice (Refs BUG-1)",
            "[AI:codex] feat(seat): partial slice (Refs BUG-1)",
        ),
        (
            true,
            "[AI:codex] feat(seat): complete slice (BUG-1)",
            "[AI:codex] feat(seat): complete slice (BUG-1)",
        ),
        (
            false,
            "[AI:codex] feat(seat): interrupted slice (BUG-1)",
            "[AI:codex] feat(seat): interrupted slice (Refs BUG-1)",
        ),
    ] {
        crate::native_publication::prepare(&fx.main, &clone).unwrap();
        crate::native_publication::declare(&clone, Some("BUG-1"), requested, false).unwrap();
        std::fs::write(clone.join("slice.txt"), requested).unwrap();
        let result = post_exit_commit(&fx.main, &clone, "BUG-1", "test", clean, &test_commit);
        assert!(matches!(result, PostExitCommit::Committed(_)), "{result:?}");
        let subject = git_ok(&clone, &["log", "-1", "--format=%s"]);
        assert_eq!(subject, expected);
        assert_eq!(
            crate::ship_from_seat::seat_pr_title(None, &subject).unwrap(),
            expected
        );
        let forge = crate::forge::fake::RecordingForge {
            opens_as: Some(77),
            ..crate::forge::fake::RecordingForge::new()
        };
        crate::ship_from_seat::run_from(
            &fx.main,
            opts(&clone, false, false),
            &crate::ship_from_seat::Deps {
                bundle: &test_bundle,
                commit: &no_commit,
                forge: &|_| Box::new(forge.clone()),
                after_open: &|_, _| {},
            },
        )
        .unwrap();
        assert_eq!(forge.opened.lock().unwrap()[0].title, expected);
        assert_eq!(
            git_ok(
                &fx.origin,
                &["log", "-1", "--format=%s", "refs/heads/declared"]
            ),
            expected
        );
    }
}

// trace:BUG-2493 | ai:codex
#[test]
fn bug_2493_declared_body_survives_host_commit_and_refs_downgrade() {
    use crate::ship_from_seat::{post_exit_commit, PostExitCommit};
    let fx = fixture();
    let clone = seat_with_publication_pending(&fx, "body", "c0ffee249301", "body");
    let script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/check-help-golden-cochange.sh");
    for clean in [true, false] {
        let base = git_ok(&clone, &["rev-parse", "HEAD"]);
        crate::native_publication::prepare(&fx.main, &clone).unwrap();
        let body = "# Explanation with (BUG-2) in prose.  \n\nHelp-Golden: unchanged\n\n";
        let message = format!("[AI:codex] fix: body (BUG-1)\n\n{body}");
        crate::native_publication::declare(&clone, Some("BUG-1"), &message, false).unwrap();
        let surface = clone.join("aida-cli-lib/src/example_args.rs");
        std::fs::create_dir_all(surface.parent().unwrap()).unwrap();
        std::fs::write(surface, format!("// changed {clean}")).unwrap();
        let result = post_exit_commit(&fx.main, &clone, "BUG-1", "test", clean, &test_commit);
        assert!(matches!(result, PostExitCommit::Committed(_)), "{result:?}");
        // Inspect raw bytes so whitespace cleanup during declaration reading
        // or Refs downgrade cannot hide behind git_ok's trimmed output.
        // trace:BUG-2493 | ai:codex
        let object = crate::test_git::git_command()
            .args(["cat-file", "commit", "HEAD"])
            .current_dir(&clone)
            .output()
            .unwrap();
        assert!(object.status.success());
        let object = String::from_utf8(object.stdout).unwrap();
        let expected_subject = if clean {
            "[AI:codex] fix: body (BUG-1)"
        } else {
            "[AI:codex] fix: body (Refs BUG-1)"
        };
        assert_eq!(
            object.split_once("\n\n").unwrap().1,
            format!("{expected_subject}\n\n{body}")
        );
        if !clean {
            assert_eq!(
                git_ok(&clone, &["log", "-1", "--format=%s"]),
                "[AI:codex] fix: body (Refs BUG-1)"
            );
        }
        let output = std::process::Command::new("bash")
            .arg(&script)
            .arg(&base)
            .current_dir(&clone)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("allowed by"));
    }
}

// trace:TASK-2902 | ai:codex
#[test]
fn task_2902_stale_invalid_and_mismatched_declarations_refuse_before_commit() {
    let fx = fixture();
    let clone = seat_with_publication_pending(&fx, "stale", "c0ffee290202", "stale");
    let path = clone.join(".aida/native-publication.json");
    crate::native_publication::prepare(&fx.main, &clone).unwrap();
    crate::native_publication::declare(&clone, None, "[AI:codex] feat: slice (Refs BUG-1)", false)
        .unwrap();
    let old = std::fs::read(&path).unwrap();
    crate::native_publication::prepare(&fx.main, &clone).unwrap();
    assert!(!path.exists());
    for bytes in [old, b"invalid JSON".to_vec()] {
        std::fs::write(&path, bytes).unwrap();
        let result = crate::ship_from_seat::post_exit_commit(
            &fx.main,
            &clone,
            "BUG-1",
            "test",
            true,
            &commit_never,
        );
        assert!(
            matches!(result, crate::ship_from_seat::PostExitCommit::Failed(_)),
            "{result:?}"
        );
    }
    for subject in [
        "no trailer",
        "slice (Refs BUG-1 BUG-2)",
        "slice (BUG-2)",
        "slice (BUG-1)\nbody",
    ] {
        assert!(
            crate::native_publication::declare(&clone, None, subject, false).is_err(),
            "{subject}"
        );
    }
    for scope in ["EPIC-1", "BUG-1 BUG-2", "unknown"] {
        let result = crate::ship_from_seat::post_exit_commit(
            &fx.main,
            &clone,
            scope,
            "test",
            true,
            &commit_never,
        );
        assert!(
            matches!(result, crate::ship_from_seat::PostExitCommit::Failed(_)),
            "{result:?}"
        );
    }
}

// trace:TASK-2902 | ai:codex
#[test]
fn task_2902_lease_scope_and_veto_precede_declaration_or_commit() {
    if let Some(output) = crate::test_env::run_path_isolated_test_output() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("Native publication vetoed for BUG-1 by punt/hold"),
            "{stderr}"
        );
        assert!(
            stderr.contains("pending work remains uncommitted"),
            "{stderr}"
        );
        return;
    }
    let fx = fixture();
    let clone = seat_with_publication_pending(&fx, "bound", "c0ffee290203", "bound");
    crate::native_publication::prepare(&fx.main, &clone).unwrap();
    let path = clone.join(".aida/native-publication.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "malformed declaration").unwrap();
    let mut store = aida_core::RequirementsStore::new();
    store.requirements.push(title_spec("Implement bug"));
    let mut other = title_spec("Other work");
    other.spec_id = Some("BUG-2".into());
    store.requirements.push(other);
    let mut epic = title_spec("Container");
    epic.spec_id = Some("EPIC-1".into());
    epic.req_type = aida_core::RequirementType::Epic;
    store.requirements.push(epic);
    aida_core::Storage::new(fx.main.join("requirements.yaml"))
        .save(&store)
        .unwrap();
    let lease_path = crate::lease_path(&fx.main, "c0ffee290203");
    let original = std::fs::read_to_string(&lease_path).unwrap();
    for scope in ["BUG-2", "EPIC-1", "BUG-1 BUG-2", "missing"] {
        std::fs::write(
            &lease_path,
            original.replace("scope = \"BUG-1\"", &format!("scope = \"{scope}\"")),
        )
        .unwrap();
        let result = crate::ship_from_seat::post_exit_commit(
            &fx.main,
            &clone,
            "BUG-1",
            "test",
            true,
            &commit_never,
        );
        assert!(
            matches!(result, crate::ship_from_seat::PostExitCommit::Failed(_)),
            "{scope}: {result:?}"
        );
    }
    std::fs::write(lease_path, original).unwrap();
    crate::punt::write_hold_signal(
        &crate::punt::hold_signal_path(&fx.main, "BUG-1"),
        &crate::punt::HoldSignal {
            spec: "BUG-1".into(),
            spec_key: None,
            branch: "bound".into(),
            reason: Some("manual gate".into()),
        },
    )
    .unwrap();
    assert_eq!(
        crate::ship_from_seat::post_exit_commit(
            &fx.main,
            &clone,
            "BUG-1",
            "test",
            true,
            &commit_never
        ),
        crate::ship_from_seat::PostExitCommit::Clean
    );
}

// trace:TASK-2902 | ai:codex
#[test]
fn task_2902_native_ship_message_records_only_and_dry_run_does_not_write() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let fx = fixture();
    let clone = seat_with_publication_pending(&fx, "command", "c0ffee290204", "command");
    crate::native_publication::prepare(&fx.main, &clone).unwrap();
    let _env = crate::test_env::EnvVarGuard::set("AIDA_CONTAINMENT_POSTURE", "native");
    std::env::set_current_dir(&clone).unwrap();
    let base = git_ok(&clone, &["rev-parse", "HEAD"]);
    let pending = git_ok(&clone, &["status", "--porcelain"]);
    let crate::command_registry::Handler::Early(run) = crate::pr_cmd::SHIP.run else {
        panic!("ship is an early command");
    };
    let ctx = crate::command_registry::DispatchCtx {
        globals: None,
        store: None,
    };
    let subject = "[AI:codex] feat(seat): partial slice (Refs BUG-1)";
    let command = || (crate::pr_cmd::SHIP.command)();
    let path = clone.join(".aida/native-publication.json");
    let preview = command()
        .try_get_matches_from(["ship", "-m", subject, "--dry-run"])
        .unwrap();
    run(&preview, &ctx).unwrap();
    assert!(!path.exists());
    let declare = command()
        .try_get_matches_from(["ship", "BUG-1", "-m", subject])
        .unwrap();
    run(&declare, &ctx).unwrap();
    assert_eq!(
        crate::native_publication::read(&fx.main, &clone, "BUG-1")
            .unwrap()
            .as_deref(),
        Some(subject)
    );
    assert_eq!(git_ok(&clone, &["rev-parse", "HEAD"]), base);
    assert_eq!(git_ok(&clone, &["status", "--porcelain"]), pending);
    // trace:BUG-2493 | ai:codex
    let body_file = fx.main.join("body.txt");
    std::fs::write(&body_file, "Help-Golden: unchanged\n").unwrap();
    let declare = command()
        .try_get_matches_from([
            "ship",
            "-m",
            subject,
            "--commit-body-file",
            body_file.to_str().unwrap(),
        ])
        .unwrap();
    run(&declare, &ctx).unwrap();
    assert_eq!(
        crate::native_publication::read(&fx.main, &clone, "BUG-1")
            .unwrap()
            .unwrap(),
        format!("{subject}\n\nHelp-Golden: unchanged\n")
    );
    let missing = command()
        .try_get_matches_from(["ship", "--commit-body-file", body_file.to_str().unwrap()])
        .unwrap();
    assert!(run(&missing, &ctx).is_err());
    let host = command()
        .try_get_matches_from([
            "ship",
            "--from-seat",
            clone.to_str().unwrap(),
            "--commit-pending",
            subject,
            "--commit-body-file",
            body_file.to_str().unwrap(),
        ])
        .unwrap();
    use clap::FromArgMatches;
    assert_eq!(
        crate::pr_cmd::ShipArgs::from_arg_matches(&host)
            .unwrap()
            .commit_body_file,
        Some(body_file.clone())
    );
    // Test the same assembly used to populate FromSeatOptions in run_ship.
    let host_args = crate::pr_cmd::ShipArgs::from_arg_matches(&host).unwrap();
    assert_eq!(
        crate::pr_cmd::ship_messages(&host_args, false).unwrap(),
        (None, Some(format!("{subject}\n\nHelp-Golden: unchanged\n")))
    );
    for argv in [
        vec![
            "ship",
            "--from-seat",
            clone.to_str().unwrap(),
            "--commit-body-file",
            body_file.to_str().unwrap(),
        ],
        vec![
            "ship",
            "-m",
            subject,
            "--commit-body-file",
            body_file.to_str().unwrap(),
        ],
    ] {
        let parsed = command().try_get_matches_from(argv).unwrap();
        let args = crate::pr_cmd::ShipArgs::from_arg_matches(&parsed).unwrap();
        assert!(crate::pr_cmd::ship_messages(&args, false)
            .unwrap_err()
            .to_string()
            .contains("requires native -m or --from-seat --commit-pending"));
    }
    let no_message = command().try_get_matches_from(["ship"]).unwrap();
    assert!(run(&no_message, &ctx).is_err());
}

// trace:TASK-2902 trace:BUG-2459 | ai:codex
#[test]
fn task_2902_post_exit_uses_canonical_display_id_for_origin_scope() {
    let fx = fixture();
    let clone = seat_with_publication_pending(&fx, "origin", "c0ffee290205", "origin");
    let origin = "BUG-b6b1278890774ef993d2ebc327862470-021";
    let mut map = aida_core::node_alias::NodeAliasMap::default();
    map.bindings.push(aida_core::node_alias::NodeAliasBinding {
        label: "HOME".into(),
        node: "b6b1278890774ef993d2ebc327862470".into(),
        retired: false,
    });
    let _map = aida_core::node_alias::install_for_thread(map);
    let mut store = aida_core::RequirementsStore::new();
    let mut req = title_spec("Canonical publication");
    req.spec_id = Some(origin.into());
    req.agreed_id = Some("BUG-1".into());
    store.requirements.push(req);
    aida_core::Storage::new(fx.main.join("requirements.yaml"))
        .save(&store)
        .unwrap();
    let lease_path = crate::lease_path(&fx.main, "c0ffee290205");
    let lease = std::fs::read_to_string(&lease_path)
        .unwrap()
        .replace("scope = \"BUG-1\"", &format!("scope = \"{origin}\""));
    std::fs::write(lease_path, lease).unwrap();
    for (clean, suffix) in [(true, "(BUG-1)"), (false, "(Refs BUG-1)")] {
        std::fs::write(clone.join("canonical.txt"), suffix).unwrap();
        let result = crate::ship_from_seat::post_exit_commit(
            &fx.main,
            &clone,
            origin,
            "canonical",
            clean,
            &test_commit,
        );
        assert!(
            matches!(result, crate::ship_from_seat::PostExitCommit::Committed(_)),
            "{result:?}"
        );
        let subject = git_ok(&clone, &["log", "-1", "--format=%s"]);
        assert!(subject.ends_with(suffix), "{subject}");
        crate::ship_from_seat::pending_message_check(&fx.main, &subject, Some("BUG-1")).unwrap();
        assert!(!subject.contains("HOME"));
    }
}

// trace:TASK-2902 | ai:codex
#[test]
fn task_2902_rotation_retains_intent_and_lease_release_cleans_binding() {
    for generation in [None, Some(1)] {
        let fx = fixture();
        let lease = "c0ffee290206";
        let clone = seat_with_publication_pending(&fx, "rotation", lease, "rotation");
        let requested = "[AI:codex] fix: partial rotation (Refs BUG-1)";
        crate::native_publication::prepare_generation(&fx.main, &clone, generation).unwrap();
        crate::native_publication::declare(&clone, None, requested, false).unwrap();
        let expected = fx
            .main
            .join(format!(".aida/native-publications/{lease}.json"));
        let binding = std::fs::read(&expected).unwrap();
        for generation in [2, 3] {
            crate::native_publication::prepare_generation(&fx.main, &clone, Some(generation))
                .unwrap();
            assert_eq!(std::fs::read(&expected).unwrap(), binding);
            assert_eq!(
                crate::native_publication::read(&fx.main, &clone, "BUG-1")
                    .unwrap()
                    .as_deref(),
                Some(requested)
            );
        }
        let result = crate::ship_from_seat::post_exit_commit(
            &fx.main,
            &clone,
            "BUG-1",
            "rotation",
            true,
            &test_commit,
        );
        assert!(
            matches!(result, crate::ship_from_seat::PostExitCommit::Committed(_)),
            "{result:?}"
        );
        assert_eq!(git_ok(&clone, &["log", "-1", "--format=%s"]), requested);
        crate::native_publication::prepare_generation(&fx.main, &clone, generation).unwrap();
        assert!(!clone.join(".aida/native-publication.json").exists());
        assert_ne!(std::fs::read(&expected).unwrap(), binding);
        if generation.is_some() {
            assert!(crate::remove_session_lease_record_only(&fx.main, lease));
        } else {
            crate::session_reap::release_lease_files(&fx.main, lease);
        }
        assert!(!expected.exists());
    }
}

/// Hit each real launch entry point with an unwritable binding destination.
/// Setup must clear stale seat intent, then refuse before a child can spawn.
// trace:TASK-2902 | ai:codex
#[test]
fn task_2902_launch_paths_clear_and_provision_before_spawn() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    use crate::agent_launch::{AgentLaunchConfig, AgentLaunchPlan, AgentPromptStyle, RotationEnv};
    use crate::session::HeadlessVendor;
    let fx = fixture();
    std::fs::create_dir_all(fx.main.join(".aida")).unwrap();
    std::fs::write(
        fx.main.join(".aida/config.toml"),
        "[contained.vendors]\ncodex = \"native\"\n",
    )
    .unwrap();
    git_ok(&fx.main, &["add", ".aida/config.toml"]);
    git_ok(&fx.main, &["commit", "-qm", "native policy"]);
    git_ok(
        &fx.main,
        &["update-ref", "refs/remotes/origin/main", "HEAD"],
    );
    let lease = "c0ffee290207";
    let clone = seat_with_publication_pending(&fx, "launches", lease, "launches");
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_OS_WRAP", Some("0")),
        ("AIDA_CONTAINED", None),
        ("AIDA_AGENT_CMD", Some("/bin/true")),
        ("AIDA_HOME", Some(fx.root.to_str().unwrap())),
        ("HOME", Some(fx.root.to_str().unwrap())),
        ("AIDA_SESSION_GRANT", None),
        ("AIDA_SESSION_ROLE", None),
    ]);
    let _cwd = crate::session::contained_cwd_seam::pin(&fx.main);
    let config = AgentLaunchConfig {
        delegate_seats: Vec::new(),
        agent_type: "codex",
        binary: "codex",
        default_args: crate::codex_native_sandbox::CODEX_NATIVE_POSTURE
            .iter()
            .map(|s| s.to_string())
            .collect(),
        prompt_style: AgentPromptStyle::Positional,
        contained: false,
    };
    let plan = AgentLaunchPlan {
        project_root: fx.main.clone(),
        launch_cwd: clone.clone(),
        role: Some("implementer".into()),
        role_instance: crate::role::RoleInstanceKind::Driver,
        current_spec: Some("BUG-1".into()),
        name: "publication-test".into(),
        lease_id: Some(lease.into()),
        native_session_id: None,
        resumed_from: None,
    };
    let binding = fx
        .main
        .join(format!(".aida/native-publications/{lease}.json"));
    std::fs::create_dir_all(&binding).unwrap();
    let declaration = clone.join(".aida/native-publication.json");
    std::fs::create_dir_all(declaration.parent().unwrap()).unwrap();
    let grant = crate::gate::seat_authority::HeadlessChildGrant::Baseline {
        cause: "test".into(),
    };
    let tee = crate::headless_tee::TeeOptions {
        enabled: false,
        label: None,
    };
    let binary = Path::new("/bin/true");
    for path in [
        "standalone",
        "drain",
        "resume",
        "bg",
        "worker",
        "generation",
        "nonrotating",
    ] {
        std::fs::write(&declaration, "stale declaration").unwrap();
        let error = match path {
            "standalone" => {
                crate::standalone_implementer::run(crate::standalone_implementer::Launch {
                    project_root: &fx.main,
                    vendor: HeadlessVendor::Codex,
                    prompt: "go",
                    session_id: "test",
                    spec: "BUG-1",
                    lease_id: lease,
                    branch: "launches",
                    contained: false,
                    child_grant: &grant,
                    host_main: Some(&fx.main),
                })
                .err()
            }
            "drain" => crate::session::run_vendor_headless_to_exit(
                Some(&fx.main),
                HeadlessVendor::Codex,
                "go",
                "test",
                &fx.root.join("log"),
                &tee,
                false,
                Some(lease),
                "implementer",
                &grant,
                Some((&clone, "launches")),
            )
            .err(),
            "resume" => crate::session::spawn_vendor_headless_resume_granted(
                Some(&fx.main),
                &crate::session::HeadlessResume {
                    vendor: HeadlessVendor::Codex,
                    session_id: "test",
                    resume_id: "test",
                },
                "go",
                &fx.root.join("log"),
                &clone,
                &tee,
                false,
                "implementer",
            )
            .err(),
            "bg" => {
                crate::agent_launch::build_agent_new_bg_command(binary, &config, &plan, &[], None)
                    .err()
            }
            "worker" => crate::agent_launch::agent_codex_background::publication_test_worker_exec(
                binary, &config, &plan,
            )
            .err(),
            _ => crate::agent_launch::run_agent_generation(
                binary,
                &config,
                &plan,
                None,
                &[],
                None,
                false,
                false,
                None,
                (path == "generation").then(|| RotationEnv::for_publication_test(1)),
                || panic!("child spawned before binding refusal"),
            )
            .err(),
        };
        let error = error.unwrap_or_else(|| panic!("{path} should refuse a binding write failure"));
        assert_eq!(
            error
                .downcast_ref::<std::io::Error>()
                .and_then(|e| e.raw_os_error()),
            Some(libc::EISDIR),
            "{path} must attempt to write its host binding: {error:#}",
        );
        assert!(
            !declaration.exists(),
            "{path} did not clear stale publication intent: {error:?}"
        );
    }
    // A later generation must reach the ordinary launch checks without
    // touching intent, even when binding setup would fail. Missing executable
    // keeps this a launch-boundary test with no real vendor child.
    std::fs::write(&declaration, "generation-one declaration").unwrap();
    let result = crate::agent_launch::run_agent_generation(
        Path::new("/aida-test-missing-codex"),
        &config,
        &plan,
        None,
        &[],
        None,
        false,
        false,
        None,
        Some(RotationEnv::for_publication_test(2)),
        || panic!("missing vendor must not spawn"),
    );
    assert!(result.is_err());
    assert_eq!(
        std::fs::read_to_string(&declaration).unwrap(),
        "generation-one declaration"
    );
}
