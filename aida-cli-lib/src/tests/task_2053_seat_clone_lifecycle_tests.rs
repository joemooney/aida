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
    let sandbox = |allowlist: &[String]| -> Vec<String> {
        probe_sandbox_args(
            Path::new("/w/aida-seat-x"),
            Path::new("/m"),
            allowlist,
            Path::new("/h/probe-config"),
        )
        .into_iter()
        .map(|a| a.into_string().unwrap())
        .collect()
    };
    let args = sandbox(&[]);
    let has = |seq: &[&str]| args.windows(seq.len()).any(|w| w == seq);
    // The clone's own config is replaced, and git cannot follow a planted
    // `.git/commondir` to another one.
    assert!(has(&[
        "--ro-bind",
        "/h/probe-config",
        "/w/aida-seat-x/.git/config"
    ]));
    assert!(has(&["--setenv", "GIT_DIR", "/w/aida-seat-x/.git"]));
    assert!(has(&["--setenv", "GIT_COMMON_DIR", "/w/aida-seat-x/.git"]));
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

    // With `[contained] read_allowlist`, the probe sees what the seat sees:
    // the essential system paths and the allowlist, never the whole root.
    let narrowed = sandbox(&["/data/a".to_string()]);
    let has = |seq: &[&str]| narrowed.windows(seq.len()).any(|w| w == seq);
    assert!(!has(&["--ro-bind", "/", "/"]), "{narrowed:?}");
    assert!(has(&["--ro-bind-try", "/usr", "/usr"]), "{narrowed:?}");
    assert!(
        has(&["--ro-bind-try", "/data/a", "/data/a"]),
        "{narrowed:?}"
    );
    assert!(has(&["--ro-bind", "/w/aida-seat-x", "/w/aida-seat-x"]));
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
        classify_clone_reap(|| probe.clone(), "feat", exited, finished, || merged)
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
    // The cheap gates come first: a live or unfinished seat is never probed.
    for (exited, finished) in [(false, true), (true, false)] {
        let verdict = classify_clone_reap(
            || panic!("probed a seat the cheap gates already kept"),
            "feat",
            exited,
            finished,
            || true,
        );
        assert!(matches!(verdict, ReapVerdict::Skip(_)));
    }
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
    let patches: Vec<_> = std::fs::read_dir(&salvage_dir)
        .unwrap()
        .filter(|e| e.as_ref().unwrap().path().extension() == Some("patch".as_ref()))
        .collect();
    assert_eq!(patches.len(), 1);
    assert!(salvage_dir
        .read_dir()
        .unwrap()
        .any(|e| { e.unwrap().path().extension() == Some("bundle".as_ref()) }));
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

/// A host file outside `/tmp` and outside the essential system paths: the
/// crate's own manifest. A probe that can read it would also read `~/.ssh`.
fn outside_file() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")
}

fn write_read_allowlist(fx: &Fixture, dirs: &[&Path]) {
    let list: Vec<String> = dirs
        .iter()
        .map(|d| aida_core::toml_quote::toml_string(&d.display().to_string()))
        .collect();
    std::fs::create_dir_all(fx.main.join(".aida")).unwrap();
    std::fs::write(
        fx.main.join(".aida/config.toml"),
        format!("[contained]\nread_allowlist = [{}]\n", list.join(", ")),
    )
    .unwrap();
}

/// Review round 1, item 1: a seat-planted clean filter never runs in the
/// probe or salvage (the probe's own repo config defines no driver), and with
/// `[contained] read_allowlist` the probe sandbox cannot read a host file
/// outside it, while it can without the allowlist (control).
#[cfg(target_os = "linux")]
#[test]
fn task_2053_bwrap_probe_runs_no_seat_filter_and_reads_only_the_seat_view() {
    if !bwrap_live() {
        return;
    }
    let fx = fixture();
    let clone = seat(&fx, "filter", "c0ffee000011");
    let outside = outside_file();
    let marker = "name = \"aida-cli-lib\"";
    git_ok(
        &clone,
        &[
            "config",
            "filter.leak.clean",
            &format!("cat '{}'", outside.display()),
        ],
    );
    std::fs::write(clone.join(".gitattributes"), "* filter=leak\n").unwrap();
    std::fs::write(clone.join("README.md"), "seat edit\n").unwrap();

    assert!(known(probe(&fx.main, &clone)).unshipped());
    let patch = std::fs::read_to_string(salvage(&fx.main, &clone).unwrap()).unwrap();
    assert!(patch.contains("+seat edit"), "{patch}");
    assert!(
        !patch.contains(marker),
        "a seat-planted filter ran in the salvage:\n{patch}"
    );

    let read = |fx: &Fixture| {
        run_sandboxed(
            &clone,
            &fx.main,
            "cat",
            &[outside.as_os_str()],
            PROBE_OUTPUT_CAP,
        )
    };
    let unconfined = read(&fx).expect("control: readable without an allowlist");
    assert!(String::from_utf8_lossy(&unconfined).contains(marker));
    let visible = fx.root.join("visible");
    std::fs::create_dir_all(&visible).unwrap();
    write_read_allowlist(&fx, &[&visible]);
    assert!(
        matches!(read(&fx), Err(SandboxFailure::Failed(_))),
        "a host file outside read_allowlist was readable from the probe"
    );
    // The probe itself still works in the narrowed view.
    assert!(known(probe(&fx.main, &clone)).unshipped());
}

/// Review round 1, item 2: reap re-checks for live processes right before
/// removal, so a seat resumed after the scan keeps its clone.
#[cfg(target_os = "linux")]
#[test]
fn task_2053_bwrap_reap_keeps_a_clone_a_live_process_is_using() {
    if !bwrap_live() {
        return;
    }
    let fx = fixture();
    let clone = seat(&fx, "busy", "c0ffee000012");
    let head = known(probe(&fx.main, &clone)).head.unwrap();
    let mut holder = std::process::Command::new("sleep")
        .arg("60")
        .current_dir(&clone)
        .spawn()
        .unwrap();
    let kept = destroy(&fx.main, &clone, DestroyMode::KeepUnshipped, Some(&head)).unwrap();
    let _ = holder.kill();
    let _ = holder.wait();
    assert!(
        matches!(&kept, Destroyed::Kept { reason, .. } if reason.contains("in use")),
        "{kept:?}"
    );
    assert!(clone.exists());
    assert!(ref_exists(&fx.main, &pin_ref(&clone).unwrap()));
    let removed = destroy(&fx.main, &clone, DestroyMode::KeepUnshipped, Some(&head)).unwrap();
    assert!(matches!(removed, Destroyed::Removed { .. }), "{removed:?}");
    assert!(!clone.exists());
}

/// Review round 1, item 3: `create` claims the clone directory before it
/// writes the base pin, so an orphan-pin pass racing a launch always sees the
/// directory and only reports the pin.
#[cfg(unix)]
#[test]
fn create_claims_the_clone_directory_before_writing_its_pin() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let fx = fixture();
    let dest = fx.root.join(format!("{CLONE_DIR_PREFIX}aida-race"));
    let real_git = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join("git"))
        .find(|candidate| candidate.is_file())
        .expect("git on PATH")
        .display()
        .to_string();
    let shim_dir = fx.root.join("shim");
    std::fs::create_dir_all(&shim_dir).unwrap();
    let log = fx.root.join("pin.log");
    crate::test_exec::write_executable(
        &shim_dir.join("git"),
        format!(
            "#!/bin/sh\ncase \"$*\" in *update-ref\\ refs/aida/seats/*)\n\
             if [ -d '{dest}' ]; then echo present; else echo absent; fi >> '{log}';;\n\
             esac\nexec '{real_git}' \"$@\"\n",
            dest = dest.display(),
            log = log.display()
        ),
    );
    {
        let _path = crate::test_env::EnvVarGuard::prepend_path(&shim_dir);
        create(&fx.main, &dest, "feat", "origin/main").unwrap();
    }
    assert_eq!(std::fs::read_to_string(&log).unwrap().trim(), "present");
    // The racing pass: a pin, its directory, no lease yet. Reported, kept.
    let orphans = orphan_pins(&fx.main);
    assert!(orphans.iter().all(|o| o.clone_exists), "{orphans:?}");
    crate::session_reap::release_orphan_pins(&fx.main, &orphans);
    assert!(ref_exists(&fx.main, &pin_ref(&dest).unwrap()));
}

/// Review round 1, item 5: a reader stops after the cap and kills the
/// writer, so an endless stream ends at once instead of at the timeout.
#[cfg(unix)]
#[test]
fn the_capped_runner_cuts_off_an_endless_stream() {
    let mut cmd = std::process::Command::new("sh");
    cmd.args(["-c", "yes"]);
    let started = std::time::Instant::now();
    let out =
        crate::command_output_with_timeout_capped(cmd, std::time::Duration::from_secs(60), 4096);
    let crate::BoundedCommandOutput::Completed(out) = out else {
        panic!("the capped run must complete, not time out");
    };
    assert_eq!(out.stdout.len(), 4097);
    assert!(started.elapsed() < std::time::Duration::from_secs(30));
}

/// Review round 1, item 5: a salvage past its cap is written cut off with a
/// closing marker and reported as an error, and `destroy` keeps the clone.
#[cfg(target_os = "linux")]
#[test]
fn task_2053_bwrap_a_salvage_past_its_cap_is_marked_and_keeps_the_clone() {
    if !bwrap_live() {
        return;
    }
    let fx = fixture();
    let clone = seat(&fx, "big", "c0ffee000013");
    std::fs::write(clone.join("big.txt"), "x\n".repeat(64 * 1024)).unwrap();
    let err = salvage_capped(&fx.main, &clone, 1024).unwrap_err();
    assert!(format!("{err:#}").contains("cut off"), "{err:#}");
    let dir = fx
        .main
        .join(".aida/salvage")
        .join(clone.file_name().unwrap());
    let patch = std::fs::read_dir(&dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let body = std::fs::read_to_string(patch).unwrap();
    assert!(body.trim_end().ends_with("the clone was kept"), "{body}");
    assert!(clone.exists());
}

/// A `bwrap` first on PATH that fails every run whose arguments contain
/// `fail_on` (all runs when empty) and execs the real one otherwise.
fn bwrap_shim_path(fx: &Fixture, fail_on: &str) -> String {
    let real = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join("bwrap"))
        .find(|candidate| candidate.is_file())
        .expect("bwrap on PATH")
        .display()
        .to_string();
    let dir = fx.root.join("bwrap-shim");
    std::fs::create_dir_all(&dir).unwrap();
    let body = if fail_on.is_empty() {
        "#!/bin/sh\nexit 1\n".to_string()
    } else {
        format!("#!/bin/sh\ncase \"$*\" in *{fail_on}*) exit 1;; esac\nexec '{real}' \"$@\"\n")
    };
    crate::test_exec::write_executable(&dir.join("bwrap"), body);
    format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    )
}

fn end_session(id: &str, force: bool) -> anyhow::Result<()> {
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
}

/// Review round 2, item 1: untracked names that git would quote (non-ASCII,
/// a double quote, a backslash, a newline) are salvaged with their content, and an
/// untracked file that cannot be read fails the salvage, which keeps the clone.
#[cfg(target_os = "linux")]
#[test]
fn task_2053_bwrap_salvage_keeps_quoted_untracked_names_and_fails_on_unreadable() {
    if !bwrap_live() {
        return;
    }
    let fx = fixture();
    let clone = seat(&fx, "names", "c0ffee000021");
    let names = [
        "naïve.txt",
        "q\"uote.txt",
        "back\\slash.txt",
        "new\nline.txt",
    ];
    for (i, name) in names.iter().enumerate() {
        std::fs::write(clone.join(name), format!("content-{i}\n")).unwrap();
    }
    let patch = std::fs::read_to_string(salvage(&fx.main, &clone).unwrap()).unwrap();
    for i in 0..names.len() {
        assert!(patch.contains(&format!("+content-{i}")), "{patch}");
    }

    use std::os::unix::fs::PermissionsExt;
    let locked = clone.join("locked.txt");
    std::fs::write(&locked, "secret work\n").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let readable = std::fs::read(&locked).is_ok(); // true when the test runs as root
    let failed = salvage(&fx.main, &clone);
    let kept = destroy(&fx.main, &clone, DestroyMode::SalvageUnshipped, None);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o644)).unwrap();
    if !readable {
        assert!(
            failed.is_err(),
            "an unreadable untracked file must fail the salvage"
        );
        assert!(matches!(kept, Ok(Destroyed::Kept { .. })), "{kept:?}");
        assert!(clone.exists());
    }
}

/// Review round 5: an untracked nested git repository (listed as `sub/`)
/// holding a committed and an uncommitted file cannot go into a patch, so
/// the salvage fails and destroy keeps the clone with both files.
#[cfg(target_os = "linux")]
#[test]
fn task_2053_bwrap_salvage_keeps_clone_with_untracked_nested_repo() {
    if !bwrap_live() {
        return;
    }
    let fx = fixture();
    let clone = seat(&fx, "nested", "c0ffee000023");
    let sub = clone.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    git_ok(&sub, &["init", "-q"]);
    std::fs::write(sub.join("committed.txt"), "nested commit\n").unwrap();
    git_ok(&sub, &["add", "committed.txt"]);
    git_ok(
        &sub,
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
            "-q",
            "-m",
            "nested",
        ],
    );
    std::fs::write(sub.join("uncommitted.txt"), "nested work\n").unwrap();

    let facts = known(probe(&fx.main, &clone));
    assert!(facts.unshipped(), "the nested repo reads dirty: {facts:?}");
    let failed = salvage(&fx.main, &clone);
    let err = format!("{:#}", failed.expect_err("a nested repo fails the salvage"));
    assert!(err.contains("nested git repository"), "{err}");
    let kept = destroy(&fx.main, &clone, DestroyMode::SalvageUnshipped, None);
    assert!(matches!(kept, Ok(Destroyed::Kept { .. })), "{kept:?}");
    assert!(sub.join("committed.txt").exists());
    assert!(sub.join("uncommitted.txt").exists());
}

/// Review round 2, item 2: commits on a side branch the seat switched away
/// from, a tag, and every stash entry count as unpushed, and the salvage
/// bundles them all.
#[cfg(target_os = "linux")]
#[test]
fn task_2053_bwrap_side_branches_tags_and_stashes_are_unpushed_and_bundled() {
    if !bwrap_live() {
        return;
    }
    let fx = fixture();
    let clone = seat(&fx, "refs", "c0ffee000022");
    git_ok(&clone, &["checkout", "-q", "-b", "side"]);
    seat_commit(&clone, "side.txt");
    git_ok(&clone, &["tag", "kept-tag"]);
    git_ok(&clone, &["checkout", "-q", "feat"]);
    let facts = known(probe(&fx.main, &clone));
    assert_eq!(facts.unpushed, 1, "{facts:?}");
    assert!(facts.unshipped() && facts.dirty.is_empty());

    let stash = |text: &str| {
        std::fs::write(clone.join("README.md"), text).unwrap();
        git_ok(
            &clone,
            &[
                "-c",
                "user.email=seat@example.com",
                "-c",
                "user.name=Seat",
                "stash",
                "-q",
            ],
        );
    };
    stash("first stash\n");
    stash("second stash\n");
    let facts = known(probe(&fx.main, &clone));
    assert!(
        facts.unpushed >= 3,
        "two stash entries count too: {facts:?}"
    );

    let patch_path = salvage(&fx.main, &clone).unwrap();
    let patch = std::fs::read_to_string(&patch_path).unwrap();
    let stashes = git_ok(&clone, &["log", "-g", "--format=%H", "refs/stash"]);
    for sha in stashes.lines() {
        assert!(patch.contains(&format!("# stash entry: {sha}")), "{patch}");
    }
    let bundle = patch_path.with_extension("bundle");
    let heads = git_ok(
        &fx.main,
        &["bundle", "list-heads", bundle.to_str().unwrap()],
    );
    for want in ["refs/heads/side", "refs/tags/kept-tag", "refs/stash"] {
        assert!(
            heads.contains(want),
            "{want} missing from the bundle:\n{heads}"
        );
    }
    // The older stash entry's commit is in the bundle's pack too.
    // The bundle is thin: it restores on top of a clone of the main checkout.
    let restore = fx.root.join("restore");
    git_ok(
        &fx.root,
        &[
            "clone",
            "-q",
            fx.main.to_str().unwrap(),
            restore.to_str().unwrap(),
        ],
    );
    git_ok(&restore, &["bundle", "unbundle", bundle.to_str().unwrap()]);
    for sha in stashes.lines() {
        git_ok(&restore, &["cat-file", "-e", &format!("{sha}^{{commit}}")]);
    }
}

/// Review round 2, item 3: session end on a clone whose probe answers
/// "unknown" refuses, with or without `--force`, and changes nothing.
#[cfg(target_os = "linux")]
#[test]
fn task_2053_bwrap_session_end_refuses_an_unreadable_seat_clone() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    if !bwrap_live() {
        return;
    }
    let fx = fixture();
    let id = "c0ffee000023";
    let clone = seat(&fx, "unknown", id);
    let pin = pin_ref(&clone).unwrap();
    let path = bwrap_shim_path(&fx, "");
    let _ambient =
        crate::test_env::AmbientGuard::hermetic_and(&fx.main, None, &[("PATH", Some(&path))]);
    for force in [false, true] {
        assert!(end_session(id, force).is_err());
        assert!(clone.exists() && ref_exists(&fx.main, &pin));
        assert!(crate::lease_path(&fx.main, id).exists());
    }
}

/// Review round 2, item 3: when `destroy` keeps the clone during session end
/// (here its salvage fails), the clone and its pin stay, and reap reports
/// them for `aida worktree dismiss` instead of releasing the pin.
#[cfg(target_os = "linux")]
#[test]
fn task_2053_bwrap_session_end_keeps_a_clone_destroy_could_not_salvage() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    if !bwrap_live() {
        return;
    }
    let fx = fixture();
    let id = "c0ffee000024";
    let clone = seat(&fx, "kept", id);
    let pin = pin_ref(&clone).unwrap();
    seat_commit(&clone, "work.txt");
    let path = bwrap_shim_path(&fx, "aida-seat-salvage");
    let _ambient =
        crate::test_env::AmbientGuard::hermetic_and(&fx.main, None, &[("PATH", Some(&path))]);
    let _ = end_session(id, true);
    assert!(clone.exists(), "a clone whose salvage failed must be kept");
    assert!(ref_exists(&fx.main, &pin));
    if !crate::lease_path(&fx.main, id).exists() {
        let orphans = orphan_pins(&fx.main);
        let row = orphans.iter().find(|o| o.pin == pin).expect("reported");
        assert!(row.clone_exists);
        crate::session_reap::release_orphan_pins(&fx.main, &orphans);
        assert!(ref_exists(&fx.main, &pin));
    }
}

/// Review round 2, item 4: an orphan pin is released only while it still
/// names the listed commit and its clone and lease are still absent.
#[test]
fn release_orphan_pin_rechecks_and_compares_before_deleting() {
    let fx = fixture();
    let gone = fx.root.join(format!("{CLONE_DIR_PREFIX}aida-cas"));
    create(&fx.main, &gone, "feat", "origin/main").unwrap();
    std::fs::remove_dir_all(&gone).unwrap();
    let pin = pin_ref(&gone).unwrap();
    let orphan = orphan_pins(&fx.main)
        .into_iter()
        .find(|o| o.pin == pin)
        .unwrap();

    // The pin moved since the scan: compare-and-delete refuses.
    let moved = OrphanPin {
        oid: "1".repeat(orphan.oid.len()),
        ..orphan.clone()
    };
    assert!(release_orphan_pin(&fx.main, &moved).is_err());
    assert!(ref_exists(&fx.main, &pin));
    // A lease appeared since the scan: kept.
    write_clone_lease(&fx.main, &gone, "c0ffee000025");
    assert!(!release_orphan_pin(&fx.main, &orphan).unwrap());
    std::fs::remove_file(crate::lease_path(&fx.main, "c0ffee000025")).unwrap();
    // The directory reappeared since the scan: kept.
    std::fs::create_dir_all(&gone).unwrap();
    assert!(!release_orphan_pin(&fx.main, &orphan).unwrap());
    std::fs::remove_dir(&gone).unwrap();
    assert!(release_orphan_pin(&fx.main, &orphan).unwrap());
    assert!(!ref_exists(&fx.main, &pin));
}

/// A logging `git` first on PATH (tagged invocations are the sandboxed
/// accessor's own); returns (shim dir, log).
fn logging_git_shim(fx: &Fixture) -> (PathBuf, PathBuf) {
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
    (shim_dir, log)
}

fn untagged_git_in(log: &Path, clone: &Path) -> Vec<String> {
    clone_invocations(&std::fs::read_to_string(log).unwrap(), clone)
        .into_iter()
        .filter(|(tagged, _)| !tagged)
        .map(|(_, line)| line)
        .collect()
}

/// Review round 3: `aida session end --force` and `aida worktree dismiss` on
/// a clone run no untagged git in it (signoff condition 1), and session end
/// never follows a seat-planted `<clone>/.aida` symlink: the decoy it points
/// at keeps its `sessions` link and its files.
#[cfg(target_os = "linux")]
#[test]
fn task_2053_bwrap_session_end_and_dismiss_run_no_host_git_and_never_follow_a_clone_aida_link() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    if !bwrap_live() {
        return;
    }
    let fx = fixture();
    let id = "c0ffee000031";
    let ended = seat(&fx, "end-shim", id);
    seat_commit(&ended, "work.txt");
    let decoy = fx.root.join("decoy-aida");
    std::fs::create_dir_all(&decoy).unwrap();
    std::fs::write(decoy.join("sentinel"), "keep\n").unwrap();
    std::fs::write(decoy.join("cache.db"), "keep\n").unwrap();
    std::os::unix::fs::symlink(fx.root.join("elsewhere"), decoy.join("sessions")).unwrap();
    std::fs::write(
        decoy.join("session-env.sh"),
        format!(
            "export CARGO_TARGET_DIR={}\n",
            decoy.join("target").display()
        ),
    )
    .unwrap();
    std::fs::create_dir_all(decoy.join("target")).unwrap();
    let aida = ended.join(".aida");
    if std::fs::symlink_metadata(&aida).is_ok() {
        std::fs::remove_dir_all(&aida).unwrap();
    }
    std::os::unix::fs::symlink(&decoy, &aida).unwrap();
    let dismissed = seat(&fx, "dismiss-shim", "c0ffee000032");
    std::fs::write(dismissed.join("loose.txt"), "loose\n").unwrap();

    let (shim_dir, log) = logging_git_shim(&fx);
    let path = format!(
        "{}:{}",
        shim_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let _ambient =
        crate::test_env::AmbientGuard::hermetic_and(&fx.main, None, &[("PATH", Some(&path))]);

    end_session(id, true).unwrap();
    assert!(!ended.exists());
    assert!(
        std::fs::symlink_metadata(decoy.join("sessions")).is_ok_and(|m| m.file_type().is_symlink())
    );
    for kept in ["sentinel", "cache.db", "session-env.sh", "target"] {
        assert!(
            decoy.join(kept).exists(),
            "session end touched decoy/{kept}"
        );
    }
    crate::worktree_dismiss::handle_dismiss(Some(dismissed.to_str().unwrap()), None).unwrap();
    assert!(!dismissed.exists());

    for clone in [&ended, &dismissed] {
        let untagged = untagged_git_in(&log, clone);
        assert!(
            untagged.is_empty(),
            "host git ran in {}: {untagged:#?}",
            clone.display()
        );
    }
}

/// Review round 3: a clone using the reftable ref backend stays readable by
/// the probe (its `refStorage` extension is carried into the probe config).
/// Skipped where git cannot migrate refs to reftable.
#[cfg(target_os = "linux")]
#[test]
fn task_2053_bwrap_probe_reads_a_reftable_clone() {
    if !bwrap_live() {
        return;
    }
    let fx = fixture();
    let clone = seat(&fx, "reftable", "c0ffee000033");
    let migrated = std::process::Command::new("git")
        .arg("-C")
        .arg(&clone)
        .args(["refs", "migrate", "--ref-format=reftable"])
        .output()
        .is_ok_and(|o| o.status.success());
    if !migrated {
        eprintln!("SKIP (git cannot migrate refs to reftable)");
        return;
    }
    seat_commit(&clone, "work.txt");
    let facts = known(probe(&fx.main, &clone));
    assert_eq!(facts.branch.as_deref(), Some("feat"));
    assert!(facts.unpushed >= 1, "{facts:?}");
}

/// Review round 5 (carried MINOR): reap's per-clone step skips a clone the
/// scan recorded no HEAD for, keeps an unshipped clone and its lease, and
/// releases the lease only once the clone is removed.
#[cfg(target_os = "linux")]
#[test]
fn task_2053_bwrap_reap_one_clone_releases_the_lease_only_on_removal() {
    if !bwrap_live() {
        return;
    }
    let fx = fixture();
    let clone = seat(&fx, "reap", "c0ffee000024");
    let lease = || {
        crate::list_leases(&fx.main)
            .into_iter()
            .find(|l| l.id == "c0ffee000024")
    };
    let reap = |tip: Option<&str>| {
        crate::session_reap::reap_one_clone(&fx.main, &lease().expect("lease"), tip)
    };

    let row = reap(None);
    assert!(
        row.starts_with("skipped — the scan recorded no HEAD"),
        "{row}"
    );
    assert!(clone.exists() && lease().is_some());

    std::fs::write(clone.join("work.txt"), "unshipped\n").unwrap();
    let tip = known(probe(&fx.main, &clone)).head.unwrap();
    let row = reap(Some(&tip));
    assert!(row.starts_with("skipped — seat clone kept"), "{row}");
    assert!(clone.exists() && lease().is_some());

    std::fs::remove_file(clone.join("work.txt")).unwrap();
    let row = reap(Some(&tip));
    assert!(row.starts_with("reaped"), "{row}");
    assert!(!clone.exists() && lease().is_none());
}
