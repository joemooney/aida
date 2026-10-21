//! TASK-2053 (BUG-2169 slice S3): the host-safe lifecycle of a contained
//! seat's private clone. `probe` reads a clone only through git run in a
//! read-only, network-less bwrap; `destroy` salvages, removes the clone and
//! its cargo target, and releases the base pin; session end, reap and
//! dismiss use them; reap enumerates orphan pins (signoff condition 4).
//!
//! The `task_2053_bwrap_` tests need bubblewrap and an unprivileged user
//! namespace. Without them each prints `SKIP (bwrap unavailable)` and passes,
//! unless `AIDA_REQUIRE_BWRAP_LIVE=1` is set (the CI gate), where a skip
//! fails the test.
//! trace:TASK-2053 | ai:claude

use super::tests::{clone_invocations, fixture, git_ok, ref_exists, Fixture};
use super::*;
use std::path::{Path, PathBuf};

/// Whether the live tests can run here; fails instead of skipping under
/// `AIDA_REQUIRE_BWRAP_LIVE`.
fn bwrap_live() -> bool {
    let ok = std::process::Command::new("bwrap")
        .args(["--unshare-all", "--ro-bind", "/", "/", "--dev", "/dev"])
        .args(["--proc", "/proc", "true"])
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        assert!(
            std::env::var_os("AIDA_REQUIRE_BWRAP_LIVE").is_none(),
            "AIDA_REQUIRE_BWRAP_LIVE=1 but bwrap cannot create an unprivileged user namespace \
             here, so the TASK-2053 probe gate could not run"
        );
        eprintln!("SKIP (bwrap unavailable)");
    }
    ok
}

/// A seat clone of `fx.main` at its real location (a sibling of main, the
/// default worktree parent), plus a `clone` lease naming it.
fn seat(fx: &Fixture, name: &str, lease_id: &str) -> PathBuf {
    let clone = fx.root.join(format!("{CLONE_DIR_PREFIX}aida-{name}"));
    create(&fx.main, &clone, "feat", "origin/main").unwrap();
    write_clone_lease(&fx.main, &clone, lease_id);
    clone.canonicalize().unwrap()
}

fn write_clone_lease(main: &Path, clone: &Path, id: &str) {
    let path = crate::lease_path(main, id);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        path,
        format!(
            "id = \"{id}\"\nscope = \"TASK-2053\"\nslug = \"task-2053\"\nowner = \"test\"\n\
             worktree_path = {}\nbranch = \"feat\"\nstarted_at = \"2026-10-21T00:00:00Z\"\n\
             hostname = \"h\"\ntree_kind = \"clone\"\n",
            aida_core::toml_quote::toml_string(&clone.display().to_string())
        ),
    )
    .unwrap();
}

/// Commit a file in the clone (test-side git: the fixture plays the seat).
fn seat_commit(clone: &Path, file: &str) {
    std::fs::write(clone.join(file), "seat work\n").unwrap();
    git_ok(clone, &["add", file]);
    git_ok(
        clone,
        &[
            "-c",
            "user.email=seat@example.com",
            "-c",
            "user.name=Seat",
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "commit",
            "-qm",
            "seat work",
        ],
    );
}

fn known(probe: Probe) -> ProbeFacts {
    match probe {
        Probe::Known(facts) => facts,
        Probe::Unknown(why) => panic!("probe had no answer: {why}"),
    }
}

fn target_dir(fx: &Fixture, clone: &Path) -> PathBuf {
    fx.main
        .join("target")
        .join("worktrees")
        .join(clone.file_name().unwrap())
}

#[test]
fn status_v2_parsing_reads_head_branch_and_entries() {
    let sha = "a".repeat(40);
    let facts = parse_status_v2(&format!(
        "# branch.oid {sha}\n# branch.head feat\n# branch.upstream origin/feat\n\
         1 .M N... 100644 100644 100644 {sha} {sha} README.md\n? new.txt\n"
    ));
    assert_eq!(facts.head.as_deref(), Some(sha.as_str()));
    assert_eq!(facts.branch.as_deref(), Some("feat"));
    assert_eq!(facts.dirty.len(), 2);
    assert!(facts.unshipped());

    let detached = parse_status_v2("# branch.oid (initial)\n# branch.head (detached)\n");
    assert_eq!(detached.head, None);
    assert_eq!(detached.branch, None);
    assert!(!detached.unshipped());
}

/// The probe sandbox binds nothing writable, shares no namespace (network
/// included), clears the environment and overrides hooks and fsmonitor at
/// command scope.
#[test]
fn the_probe_sandbox_is_read_only_networkless_and_hookless() {
    let args: Vec<String> = probe_sandbox_args(Path::new("/w/aida-seat-x"), Path::new("/m"))
        .into_iter()
        .map(|a| a.into_string().unwrap())
        .collect();
    let has = |seq: &[&str]| args.windows(seq.len()).any(|w| w == seq);
    assert!(has(&["--unshare-all"]), "{args:?}");
    assert!(!args.iter().any(|a| a == "--share-net"), "{args:?}");
    assert!(has(&["--ro-bind", "/", "/"]), "{args:?}");
    assert!(has(&["--ro-bind", "/w/aida-seat-x", "/w/aida-seat-x"]));
    assert!(has(&["--ro-bind", "/m/.git", "/m/.git"]));
    assert!(
        !args.iter().any(|a| a.starts_with("--bind")),
        "no writable bind: {args:?}"
    );
    assert!(has(&["--clearenv"]) && has(&["--die-with-parent"]));
    assert!(has(&["--setenv", "GIT_CONFIG_GLOBAL", "/dev/null"]));
    assert!(has(&["--setenv", "GIT_CONFIG_NOSYSTEM", "1"]));
    assert!(has(&["--setenv", "GIT_CONFIG_KEY_0", "core.hooksPath"]));
    assert!(has(&["--setenv", "GIT_CONFIG_VALUE_0", "/dev/null"]));
    assert!(has(&["--setenv", "GIT_CONFIG_KEY_1", "core.fsmonitor"]));
    assert!(has(&["--setenv", "GIT_CONFIG_VALUE_1", "false"]));
    assert_eq!(args.last().map(String::as_str), Some("--"));
}

#[test]
fn clone_reap_verdicts_keep_anything_unreadable_unshipped_or_unmerged() {
    use crate::session_reap::{classify_clone_reap, ReapVerdict};
    let clean = ProbeFacts {
        head: Some("a".repeat(40)),
        branch: Some("feat".into()),
        dirty: vec![],
        unpushed: 0,
    };
    let verdict = |probe: &Probe, exited, finished, merged: bool| {
        classify_clone_reap(probe, "feat", exited, finished, || merged)
    };
    let ok = Probe::Known(clean.clone());
    assert!(matches!(
        verdict(&ok, true, true, true),
        ReapVerdict::Reap(_)
    ));

    let skip = |v: ReapVerdict, needle: &str| match v {
        ReapVerdict::Skip(reason) => assert!(reason.contains(needle), "{reason}"),
        ReapVerdict::Reap(reason) => panic!("reaped: {reason}"),
    };
    skip(
        verdict(&Probe::Unknown("no bwrap".into()), true, true, true),
        "could not be read",
    );
    let dirty = ProbeFacts {
        dirty: vec!["? x".into()],
        ..clean.clone()
    };
    skip(
        verdict(&Probe::Known(dirty), true, true, true),
        "uncommitted",
    );
    let other = ProbeFacts {
        branch: Some("other".into()),
        ..clean.clone()
    };
    skip(
        verdict(&Probe::Known(other), true, true, true),
        "session branch",
    );
    let ahead = ProbeFacts {
        unpushed: 2,
        ..clean.clone()
    };
    skip(
        verdict(&Probe::Known(ahead), true, true, true),
        "2 commit(s)",
    );
    skip(verdict(&ok, false, true, true), "still running");
    skip(verdict(&ok, true, false, true), "not finished");
    skip(verdict(&ok, true, true, false), "no merged PR");
}

/// The lease is seat-writable through the store, so `destroy` refuses any
/// path that is not a seat clone of this main, and deletes nothing.
#[test]
fn destroy_refuses_a_path_that_is_not_a_seat_clone_location() {
    let fx = fixture();
    let elsewhere = fx.root.join("nested").join(format!("{CLONE_DIR_PREFIX}x"));
    let unnamed = fx.root.join("victim");
    for dir in [&elsewhere, &unnamed] {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("keep.txt"), "keep\n").unwrap();
    }
    for dir in [&elsewhere, &unnamed, &fx.main] {
        let err = destroy(&fx.main, dir, DestroyMode::SalvageUnshipped, None).unwrap_err();
        assert!(
            format!("{err:#}").contains("is not a seat clone"),
            "{err:#}"
        );
    }
    assert!(elsewhere.join("keep.txt").exists());
    assert!(unnamed.join("keep.txt").exists());
    assert!(fx.main.join("README.md").exists());
    assert!(is_seat_clone_location(
        &fx.main,
        &fx.root.join(format!("{CLONE_DIR_PREFIX}aida-y"))
    ));
}

/// Signoff condition 4: a pin with a lease is not an orphan; an unleased pin
/// whose clone is gone is released; one whose clone exists is kept.
#[test]
fn reap_enumerates_orphan_pins_and_releases_only_those_whose_clone_is_gone() {
    let fx = fixture();
    let leased = seat(&fx, "leased", "c0ffee000001");
    let gone = fx.root.join(format!("{CLONE_DIR_PREFIX}aida-gone"));
    create(&fx.main, &gone, "feat", "origin/main").unwrap();
    std::fs::remove_dir_all(&gone).unwrap();
    let stray = fx.root.join(format!("{CLONE_DIR_PREFIX}aida-stray"));
    create(&fx.main, &stray, "feat", "origin/main").unwrap();

    let orphans = orphan_pins(&fx.main);
    let pins: Vec<&str> = orphans.iter().map(|o| o.pin.as_str()).collect();
    assert!(
        !pins.contains(&pin_ref(&leased).unwrap().as_str()),
        "{pins:?}"
    );
    assert_eq!(orphans.len(), 2, "{orphans:?}");
    let report = crate::session_reap::release_orphan_pins(&fx.main, &orphans);
    assert_eq!(report.len(), 2);
    assert!(!ref_exists(&fx.main, &pin_ref(&gone).unwrap()));
    assert!(ref_exists(&fx.main, &pin_ref(&stray).unwrap()));
    assert!(ref_exists(&fx.main, &pin_ref(&leased).unwrap()));
    assert!(stray.exists());
    let scan = crate::session_reap::scan_reapable(&fx.main);
    assert_eq!(scan.orphan_pins.len(), 1, "{:?}", scan.orphan_pins);
    assert!(scan.orphan_pins[0].clone_exists);
}

/// T6, stale-lock variant: a leftover lock on the base pin makes clone
/// creation refuse loudly, before any directory exists. There is never a
/// worktree in its place.
#[test]
fn a_stale_pin_lock_refuses_the_clone_with_no_fallback() {
    let fx = fixture();
    let dest = fx.root.join(format!("{CLONE_DIR_PREFIX}aida-locked"));
    let lock = fx
        .main
        .join(".git")
        .join(format!("{}.lock", pin_ref(&dest).unwrap()));
    std::fs::create_dir_all(lock.parent().unwrap()).unwrap();
    std::fs::write(&lock, "").unwrap();
    let err = create(&fx.main, &dest, "feat", "origin/main").unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("private clone") && msg.contains("never falls back"),
        "{msg}"
    );
    assert!(!dest.exists());
    let worktrees = git_ok(&fx.main, &["worktree", "list", "--porcelain"]);
    assert_eq!(worktrees.matches("worktree ").count(), 1, "{worktrees}");
}

#[cfg(target_os = "linux")]
#[test]
fn task_2053_bwrap_probe_reads_clean_dirty_and_unpushed() {
    if !bwrap_live() {
        return;
    }
    let fx = fixture();
    let clone = seat(&fx, "probe", "c0ffee000002");
    let facts = known(probe(&fx.main, &clone));
    assert_eq!(facts.branch.as_deref(), Some("feat"));
    assert!(facts.head.is_some());
    assert_eq!(facts.unpushed, 0, "{facts:?}");
    assert!(facts.dirty.is_empty(), "{facts:?}");

    seat_commit(&clone, "a.txt");
    std::fs::write(clone.join("untracked.txt"), "u\n").unwrap();
    let facts = known(probe(&fx.main, &clone));
    assert_eq!(facts.unpushed, 1, "{facts:?}");
    assert_eq!(facts.dirty.len(), 1, "{facts:?}");
    assert!(facts.dirty[0].contains("untracked.txt"));

    // T4 (probe part): the seat points its own alternates at `/`. The probe
    // still runs read-only and either answers or says it cannot; main's
    // objects are untouched either way.
    let packs_before = std::fs::read_dir(fx.main.join(".git/objects"))
        .unwrap()
        .count();
    std::fs::write(clone.join(".git/objects/info/alternates"), "/\n").unwrap();
    let _ = probe(&fx.main, &clone);
    let packs_after = std::fs::read_dir(fx.main.join(".git/objects"))
        .unwrap()
        .count();
    assert_eq!(packs_before, packs_after);

    // Fail closed: a clone that is not a git directory has no answer.
    assert!(matches!(
        probe(&fx.main, &fx.root.join("nothing-here")),
        Probe::Unknown(_)
    ));
}

#[cfg(target_os = "linux")]
#[test]
fn task_2053_bwrap_destroy_salvages_then_removes_clone_target_and_pin() {
    if !bwrap_live() {
        return;
    }
    let fx = fixture();
    let clone = seat(&fx, "end", "c0ffee000003");
    let target = target_dir(&fx, &clone);
    std::fs::create_dir_all(target.join("debug")).unwrap();
    let pin = pin_ref(&clone).unwrap();
    seat_commit(&clone, "committed.txt");
    std::fs::write(clone.join("README.md"), "edited\n").unwrap();
    std::fs::write(clone.join("fresh.txt"), "fresh content\n").unwrap();

    // Reap mode keeps it, with a salvage patch.
    let kept = destroy(&fx.main, &clone, DestroyMode::KeepUnshipped, None).unwrap();
    let Destroyed::Kept {
        salvage: Some(patch),
        ..
    } = kept
    else {
        panic!("reap mode must keep unshipped work: {kept:?}");
    };
    assert!(clone.exists() && ref_exists(&fx.main, &pin));
    let body = std::fs::read_to_string(&patch).unwrap();
    assert!(body.contains("committed.txt"), "{body}");
    assert!(body.contains("+edited"), "{body}");
    assert!(body.contains("+fresh content"), "{body}");
    assert!(patch.starts_with(fx.main.join(".aida/salvage")));

    // A HEAD that moved since the scan keeps it.
    let moved = destroy(
        &fx.main,
        &clone,
        DestroyMode::KeepUnshipped,
        Some(&"0".repeat(40)),
    )
    .unwrap();
    assert!(matches!(moved, Destroyed::Kept { ref reason, .. } if reason.contains("moved")));

    // End mode salvages and removes everything.
    let removed = destroy(&fx.main, &clone, DestroyMode::SalvageUnshipped, None).unwrap();
    assert!(
        matches!(removed, Destroyed::Removed { salvage: Some(_) }),
        "{removed:?}"
    );
    assert!(!clone.exists());
    assert!(!target.exists());
    assert!(!ref_exists(&fx.main, &pin));
    assert!(fx.main.join("README.md").exists());
}

/// T7: the seat plants a `core.fsmonitor`, a clean filter and hooks in its
/// clone. The host's probe, reap scan, salvage and destroy never run them on
/// the host, and no host git invocation lands in the clone outside the
/// accessor module (condition 1, extended to the S3 paths).
#[cfg(target_os = "linux")]
#[test]
fn task_2053_bwrap_t7_planted_config_never_runs_on_the_host() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    if !bwrap_live() {
        return;
    }
    let fx = fixture();
    let clone = seat(&fx, "t7", "c0ffee000004");
    let canaries = fx.root.join("canaries");
    std::fs::create_dir_all(&canaries).unwrap();
    let fsmonitor = canaries.join("fsmonitor");
    let filter = canaries.join("filter");
    let hook = canaries.join("hook");
    git_ok(
        &clone,
        &[
            "config",
            "core.fsmonitor",
            &format!("touch {}; false", fsmonitor.display()),
        ],
    );
    git_ok(
        &clone,
        &[
            "config",
            "filter.evil.clean",
            &format!("touch {}; cat", filter.display()),
        ],
    );
    for name in ["post-checkout", "pre-commit", "reference-transaction"] {
        crate::test_exec::write_executable(
            &clone.join(".git/hooks").join(name),
            format!("#!/bin/sh\ntouch {}\n", hook.display()),
        );
    }
    std::fs::write(clone.join(".gitattributes"), "* filter=evil\n").unwrap();
    std::fs::write(clone.join("dirty.txt"), "x\n").unwrap();

    let real_git = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join("git"))
        .find(|candidate| candidate.is_file())
        .expect("git on PATH")
        .display()
        .to_string();
    let shim_dir = fx.root.join("shim");
    std::fs::create_dir_all(&shim_dir).unwrap();
    let log = fx.root.join("git.log");
    std::fs::write(&log, "").unwrap();
    crate::test_exec::write_executable(
        &shim_dir.join("git"),
        format!(
            "#!/bin/sh\n{{ printf '%s\\t%s\\t' \"${{{ACCESSOR_ENV}:-0}}\" \"$PWD\"; \
             for a in \"$@\"; do printf '%s\\037' \"$a\"; done; printf '\\n'; }} >> '{}'\n\
             exec '{real_git}' \"$@\"\n",
            log.display()
        ),
    );
    let _path = crate::test_env::EnvVarGuard::prepend_path(&shim_dir);

    assert!(known(probe(&fx.main, &clone)).unshipped());
    let scan = crate::session_reap::scan_reapable(&fx.main);
    assert!(
        scan.skipped
            .iter()
            .any(|row| row.worktree == clone.display().to_string()),
        "the clone lease is scanned: {scan:?}"
    );
    salvage(&fx.main, &clone).unwrap();
    let removed = destroy(&fx.main, &clone, DestroyMode::SalvageUnshipped, None).unwrap();
    assert!(matches!(removed, Destroyed::Removed { .. }), "{removed:?}");

    for canary in [&fsmonitor, &filter, &hook] {
        assert!(!canary.exists(), "{} ran on the host", canary.display());
    }
    let log_body = std::fs::read_to_string(&log).unwrap();
    let untagged: Vec<String> = clone_invocations(&log_body, &clone)
        .into_iter()
        .filter(|(tagged, _)| !tagged)
        .map(|(_, line)| line)
        .collect();
    assert!(
        untagged.is_empty(),
        "host git ran in the clone: {untagged:#?}"
    );

    // Negative control: the same plant in a fresh clone DOES fire under plain
    // host git, so the quiet canaries above mean the host never ran it.
    let control = seat(&fx, "t7-control", "c0ffee000007");
    git_ok(
        &control,
        &[
            "config",
            "core.fsmonitor",
            &format!("touch {}; false", fsmonitor.display()),
        ],
    );
    let _ = std::process::Command::new(&real_git)
        .arg("-C")
        .arg(&control)
        .args(["status", "--porcelain"])
        .output()
        .unwrap();
    assert!(
        fsmonitor.exists(),
        "the planted fsmonitor must fire under host git"
    );
}

/// Session end on a clone lease: refuses unshipped work without `--force`
/// (lease, clone and pin unchanged), then with `--force` salvages it, removes
/// the clone, its cargo target and its pin, and deletes the lease.
#[cfg(target_os = "linux")]
#[test]
fn task_2053_bwrap_session_end_tears_down_a_seat_clone() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    if !bwrap_live() {
        return;
    }
    let fx = fixture();
    let id = "c0ffee000005";
    let clone = seat(&fx, "sessend", id);
    let target = target_dir(&fx, &clone);
    std::fs::create_dir_all(&target).unwrap();
    let pin = pin_ref(&clone).unwrap();
    seat_commit(&clone, "work.txt");
    let _ambient = crate::test_env::AmbientGuard::hermetic(&fx.main, None);
    let end = |force: bool| {
        crate::session_end::session_end(
            Some(id),
            None,
            None,
            true,
            force,
            false,
            false,
            false,
            true,
            false,
            false,
            false,
        )
    };

    let refused = end(false).unwrap_err();
    assert!(
        format!("{refused:#}").contains("origin does not have"),
        "{refused:#}"
    );
    assert!(clone.exists() && target.exists() && ref_exists(&fx.main, &pin));
    assert!(crate::lease_path(&fx.main, id).exists());

    end(true).unwrap();
    assert!(!clone.exists());
    assert!(!target.exists());
    assert!(!ref_exists(&fx.main, &pin));
    assert!(!crate::lease_path(&fx.main, id).exists());
    let salvage_dir = fx
        .main
        .join(".aida/salvage")
        .join(clone.file_name().unwrap());
    let patches: Vec<_> = std::fs::read_dir(&salvage_dir).unwrap().collect();
    assert_eq!(patches.len(), 1);
}

/// `aida worktree dismiss <clone>`: salvages, removes the clone and its pin,
/// and releases the clone lease. Before S3 it skipped the clone as "not on a
/// local branch", since `git worktree list` does not know it.
#[cfg(target_os = "linux")]
#[test]
fn task_2053_bwrap_dismiss_removes_a_seat_clone_and_its_lease() {
    if !bwrap_live() {
        return;
    }
    let fx = fixture();
    let id = "c0ffee000006";
    let clone = seat(&fx, "dismiss", id);
    let pin = pin_ref(&clone).unwrap();
    std::fs::write(clone.join("loose.txt"), "loose\n").unwrap();
    let _ambient = crate::test_env::AmbientGuard::hermetic(&fx.main, None);
    crate::worktree_dismiss::handle_dismiss(Some(clone.to_str().unwrap()), None).unwrap();
    assert!(!clone.exists());
    assert!(!ref_exists(&fx.main, &pin));
    assert!(!crate::lease_path(&fx.main, id).exists());
    let ledger = std::fs::read_to_string(
        fx.main
            .join(".aida/discipline/worktree_dismiss_ledger.jsonl"),
    )
    .unwrap();
    assert!(ledger.contains("seat_clone_dismiss"), "{ledger}");
}
