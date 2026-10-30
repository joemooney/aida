//! BUG-HOME-038: a seat clone's host-git refusal is not uncommitted work; a
//! seat proven contained runs git in its own clone with no hook; and
//! `aida ship --from-seat` accepts only a recorded clone and a one-ref bundle.
//! trace:BUG-HOME-038 | ai:claude

use super::s4_host_git_tests::leased_clone;
use super::tests::{fixture, ref_exists};
use super::*;
use std::path::Path;

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

/// Makes main's `.git` unwritable for the guard's lifetime, the way the
/// seat sandbox does, so the write probe proves containment.
#[cfg(unix)]
struct ReadOnlyDir<'a>(&'a Path);

#[cfg(unix)]
impl<'a> ReadOnlyDir<'a> {
    fn new(dir: &'a Path) -> Self {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        ReadOnlyDir(dir)
    }
}

#[cfg(unix)]
impl Drop for ReadOnlyDir<'_> {
    fn drop(&mut self) {
        // Back to 0o755 through the shared helper (the fixture ratchet).
        let ro = self.0;
        crate::test_exec::mark_executable(&ro);
    }
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

/// Condition 2: with main's `.git` unwritable, host git runs in the seat's
/// own clone, and neither a planted pre-push hook nor a `core.hooksPath` hook
/// ever runs. With it writable again, the refusal is back.
#[cfg(unix)]
#[test]
fn bug_home_038_contained_seat_git_never_runs_planted_hooks() {
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("SKIP (root ignores directory permissions)");
        return;
    }
    let fx = fixture();
    let clone = leased_clone(&fx, "hooks", "c0ffee038002");
    let sentinel = fx.root.join("hook-ran");
    plant_hooks(&clone, &sentinel, &fx.root);
    git_ok(&clone, &["config", "user.name", "Seat"]);
    git_ok(&clone, &["config", "user.email", "seat@example.com"]);
    git_ok(&clone, &["config", "maintenance.auto", "false"]);
    git_ok(&clone, &["config", "gc.auto", "0"]);

    let main_git = fx.main.join(".git");
    {
        let _ro = ReadOnlyDir::new(&main_git);
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
    }
    assert!(
        !sentinel.exists(),
        "a seat hook ran: {}",
        std::fs::read_to_string(&sentinel).unwrap_or_default()
    );
    assert!(ref_exists(&fx.origin, "refs/heads/seat-pushed"));
    // Main writable again: this process is the host, and git is refused.
    assert!(!aida_core::seat_clone_detect::contained_in_clone(&clone));
    assert_eq!(
        aida_core::git_ops::refused_seat_clone(&clone),
        Some(clone.clone())
    );
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
