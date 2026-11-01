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
        Probe::HasSubmodules(why) => panic!("probe found submodules: {why}"),
    }
}

/// The probe found submodule evidence; returns the reason.
fn has_submodules(probe: Probe) -> String {
    match probe {
        Probe::HasSubmodules(reason) => reason,
        other => panic!("expected submodule evidence, got {other:?}"),
    }
}

/// A one-commit repository to use as a submodule's upstream.
fn submodule_upstream(fx: &Fixture) -> PathBuf {
    let upstream = fx.root.join("sub-upstream");
    std::fs::create_dir_all(&upstream).unwrap();
    seat_git(&upstream, &["init", "-q"]);
    std::fs::write(upstream.join("lib.txt"), "v1\n").unwrap();
    seat_git(&upstream, &["add", "lib.txt"]);
    seat_git(&upstream, &["commit", "-qm", "v1"]);
    upstream
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
        unpushed_refs: vec![],
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
    skip(
        verdict(
            &Probe::HasSubmodules("the clone has submodules (x)".into()),
            true,
            true,
            true,
        ),
        "has submodules",
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

/// Under the live gate, an isolated child must inherit the gate, or its
/// `bwrap_live()` skips silently (TASK-2161).
// trace:TASK-2161 | ai:claude
#[test]
fn task_2053_bwrap_isolated_child_inherits_the_live_gate() {
    let gate = crate::test_env::REQUIRE_BWRAP_LIVE_ENV;
    if !crate::test_env::is_path_isolated_process() && std::env::var_os(gate).is_none() {
        eprintln!("SKIP (not under the live-bwrap gate)");
        return;
    }
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    assert!(
        std::env::var_os(gate).is_some(),
        "the isolated child lost {gate}"
    );
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

/// Test-side git with an identity and no background maintenance.
fn seat_git(dir: &Path, args: &[&str]) -> String {
    let mut full = vec![
        "-c",
        "user.email=seat@example.com",
        "-c",
        "user.name=Seat",
        "-c",
        "maintenance.auto=false",
        "-c",
        "gc.auto=0",
        "-c",
        "protocol.file.allow=always",
    ];
    full.extend_from_slice(args);
    git_ok(dir, &full)
}

/// Review round 6, item 1: work inside a tracked submodule (an uncommitted
/// change plus a commit its remote lacks) is outside what the patch and the
/// bundle hold, so the salvage fails and destroy keeps the clone.
#[cfg(target_os = "linux")]
#[test]
fn task_2053_bwrap_salvage_keeps_clone_with_submodule_work() {
    if !bwrap_live() {
        return;
    }
    let fx = fixture();
    let upstream = submodule_upstream(&fx);

    let clone = seat(&fx, "submodule", "c0ffee000025");
    let upstream_url = upstream.display().to_string();
    seat_git(
        &clone,
        &["submodule", "add", "-q", upstream_url.as_str(), "sm"],
    );
    seat_git(&clone, &["commit", "-qm", "add submodule"]);
    let sm = clone.join("sm");
    std::fs::write(sm.join("lib.txt"), "v2\n").unwrap();
    seat_git(&sm, &["commit", "-qam", "v2"]);
    std::fs::write(sm.join("lib.txt"), "v3 uncommitted\n").unwrap();

    let reason = has_submodules(probe(&fx.main, &clone));
    assert!(reason.contains("inspect"), "{reason}");
    // The salvage's own guard still refuses, should a submodule appear
    // after the probe.
    let err = format!(
        "{:#}",
        salvage(&fx.main, &clone).expect_err("submodule work")
    );
    assert!(err.contains("submodule"), "{err}");
    let kept = destroy(&fx.main, &clone, DestroyMode::SalvageUnshipped, None);
    assert!(matches!(kept, Ok(Destroyed::Kept { .. })), "{kept:?}");
    assert_eq!(
        std::fs::read_to_string(sm.join("lib.txt")).unwrap(),
        "v3 uncommitted\n"
    );
}

/// Review round 6 follow-up: a submodule commit on a side branch, with the
/// submodule's HEAD unchanged and the superproject clean and pushed, keeps
/// the clone: any submodule does (review round 7).
#[cfg(target_os = "linux")]
#[test]
fn task_2053_bwrap_submodule_side_branch_keeps_clean_clone() {
    if !bwrap_live() {
        return;
    }
    let fx = fixture();
    let upstream = submodule_upstream(&fx);

    let clone = seat(&fx, "subside", "c0ffee000027");
    let upstream_url = upstream.display().to_string();
    seat_git(
        &clone,
        &["submodule", "add", "-q", upstream_url.as_str(), "sm"],
    );
    seat_git(&clone, &["commit", "-qm", "add submodule"]);
    // Stand in for a push of the superproject: origin now has its HEAD.
    seat_git(&clone, &["update-ref", "refs/remotes/origin/feat", "HEAD"]);
    let sm = clone.join("sm");
    seat_git(&sm, &["checkout", "-q", "-b", "side"]);
    std::fs::write(sm.join("side.txt"), "side work\n").unwrap();
    seat_git(&sm, &["add", "side.txt"]);
    seat_git(&sm, &["commit", "-qm", "side"]);
    seat_git(&sm, &["checkout", "-q", "-"]);

    has_submodules(probe(&fx.main, &clone));
    let kept = destroy(&fx.main, &clone, DestroyMode::SalvageUnshipped, None);
    assert!(matches!(kept, Ok(Destroyed::Kept { .. })), "{kept:?}");
    let side = seat_git(&sm, &["log", "-1", "--format=%s", "side"]);
    assert_eq!(side.trim(), "side");
}

/// Review round 7, item 1: `.gitmodules` `ignore = dirty` hides submodule
/// work from a plain `git status`, but the clone still has a submodule, so
/// the probe says so and destroy, session end (with or without `--force`),
/// dismiss and reap all keep it, with its lease and base pin.
#[cfg(target_os = "linux")]
#[test]
fn task_2053_bwrap_submodule_ignore_dirty_keeps_the_clone() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    if !bwrap_live() {
        return;
    }
    let fx = fixture();
    let upstream = submodule_upstream(&fx);
    let id = "c0ffee000028";
    let clone = seat(&fx, "subignore", id);
    let pin = pin_ref(&clone).unwrap();
    let upstream_url = upstream.display().to_string();
    seat_git(
        &clone,
        &["submodule", "add", "-q", upstream_url.as_str(), "sm"],
    );
    seat_git(
        &clone,
        &[
            "config",
            "-f",
            ".gitmodules",
            "submodule.sm.ignore",
            "dirty",
        ],
    );
    seat_git(&clone, &["add", ".gitmodules"]);
    seat_git(&clone, &["commit", "-qm", "add submodule, ignore dirty"]);
    seat_git(&clone, &["update-ref", "refs/remotes/origin/feat", "HEAD"]);
    let sm = clone.join("sm");
    std::fs::write(sm.join("lib.txt"), "edited\n").unwrap();
    std::fs::write(sm.join("untracked.txt"), "new\n").unwrap();
    assert_eq!(seat_git(&clone, &["status", "--porcelain"]).trim(), "");

    let reason = has_submodules(probe(&fx.main, &clone));
    assert!(reason.contains("submodules"), "{reason}");
    for mode in [DestroyMode::SalvageUnshipped, DestroyMode::KeepUnshipped] {
        let kept = destroy(&fx.main, &clone, mode, None);
        assert!(
            matches!(&kept, Ok(Destroyed::Kept { reason, .. }) if reason.contains("submodules")),
            "{kept:?}"
        );
    }
    let still_held =
        || clone.exists() && ref_exists(&fx.main, &pin) && crate::lease_path(&fx.main, id).exists();
    let _ambient = crate::test_env::AmbientGuard::hermetic(&fx.main, None);
    for force in [false, true] {
        let refused = format!("{:#}", end_session(id, force).unwrap_err());
        assert!(refused.contains("submodules"), "{refused}");
        assert!(still_held());
    }
    crate::worktree_dismiss::handle_dismiss(Some(clone.to_str().unwrap()), None).unwrap();
    assert!(still_held());
    let lease = crate::list_leases(&fx.main)
        .into_iter()
        .find(|l| l.id == id)
        .expect("lease");
    let head = seat_git(&clone, &["rev-parse", "HEAD"]);
    let row = crate::session_reap::reap_one_clone(&fx.main, &lease, Some(head.trim()));
    assert!(row.contains("submodules"), "{row}");
    assert!(still_held());
    assert_eq!(
        std::fs::read_to_string(sm.join("lib.txt")).unwrap(),
        "edited\n"
    );
    assert!(sm.join("untracked.txt").exists());
}

/// Review round 7, item 2: a deinit'd and removed submodule keeps the seat's
/// commits in `.git/modules/<name>` only; no status, ref or `foreach` shows
/// them, so the probe keys on `.git/modules` and the clone is kept.
#[cfg(target_os = "linux")]
#[test]
fn task_2053_bwrap_deinit_submodule_commits_keep_the_clone() {
    if !bwrap_live() {
        return;
    }
    let fx = fixture();
    let upstream = submodule_upstream(&fx);
    let clone = seat(&fx, "subdeinit", "c0ffee000029");
    let upstream_url = upstream.display().to_string();
    seat_git(
        &clone,
        &["submodule", "add", "-q", upstream_url.as_str(), "sm"],
    );
    seat_git(&clone, &["commit", "-qm", "add submodule"]);
    let sm = clone.join("sm");
    std::fs::write(sm.join("work.txt"), "seat work\n").unwrap();
    seat_git(&sm, &["add", "work.txt"]);
    seat_git(&sm, &["commit", "-qm", "seat work"]);
    let work = seat_git(&sm, &["rev-parse", "HEAD"]);
    seat_git(&clone, &["submodule", "--quiet", "deinit", "-f", "sm"]);
    seat_git(&clone, &["rm", "-q", "sm"]);
    seat_git(&clone, &["rm", "-qf", ".gitmodules"]);
    seat_git(&clone, &["commit", "-qm", "drop submodule"]);
    seat_git(&clone, &["update-ref", "refs/remotes/origin/feat", "HEAD"]);
    assert_eq!(seat_git(&clone, &["status", "--porcelain"]).trim(), "");

    let reason = has_submodules(probe(&fx.main, &clone));
    assert!(reason.contains(".git/modules"), "{reason}");
    let kept = destroy(&fx.main, &clone, DestroyMode::SalvageUnshipped, None);
    assert!(matches!(kept, Ok(Destroyed::Kept { .. })), "{kept:?}");
    let modules = clone.join(".git").join("modules").join("sm");
    let held = git_ok(&modules, &["--git-dir", ".", "cat-file", "-t", work.trim()]);
    assert_eq!(held.trim(), "commit");
}

/// Review round 6, item 2: an untracked symlink to a directory cannot go
/// into a patch (diff follows it), so the salvage fails and destroy keeps
/// the clone; a symlink to a file is recorded as a symlink.
#[cfg(target_os = "linux")]
#[test]
fn task_2053_bwrap_salvage_keeps_clone_with_untracked_dir_symlink() {
    if !bwrap_live() {
        return;
    }
    let fx = fixture();
    let clone = seat(&fx, "dirlink", "c0ffee000026");
    std::fs::write(clone.join("target.txt"), "linked\n").unwrap();
    std::os::unix::fs::symlink("target.txt", clone.join("file-link")).unwrap();
    let patch = std::fs::read_to_string(salvage(&fx.main, &clone).unwrap()).unwrap();
    assert!(patch.contains("new file mode 120000"), "{patch}");

    std::fs::create_dir_all(clone.join("dir")).unwrap();
    std::fs::write(clone.join("dir").join("inner.txt"), "inner\n").unwrap();
    std::os::unix::fs::symlink("dir", clone.join("dir-link")).unwrap();
    let err = format!("{:#}", salvage(&fx.main, &clone).expect_err("dir symlink"));
    assert!(err.contains("symlink to a directory"), "{err}");
    let kept = destroy(&fx.main, &clone, DestroyMode::SalvageUnshipped, None);
    assert!(matches!(kept, Ok(Destroyed::Kept { .. })), "{kept:?}");
    assert!(std::fs::symlink_metadata(clone.join("dir-link")).is_ok());
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

/// TASK-2162: session end tells the operator to remove a clone with
/// submodules by hand, then `aida worktree dismiss` it. Dismiss on the
/// removed clone, whose lease, pin and cargo target remain, releases all
/// three and runs no host git there.
// trace:TASK-2162 | ai:claude
#[test]
fn task_2162_dismiss_releases_a_hand_removed_seat_clone() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let fx = fixture();
    let id = "c0ffee000041";
    let clone = seat(&fx, "removed", id);
    let pin = pin_ref(&clone).unwrap();
    let target = target_dir(&fx, &clone);
    std::fs::create_dir_all(target.join("debug")).unwrap();
    std::fs::remove_dir_all(&clone).unwrap();
    assert!(ref_exists(&fx.main, &pin));
    assert!(crate::lease_path(&fx.main, id).exists());

    let (shim_dir, log) = logging_git_shim(&fx);
    let path = format!(
        "{}:{}",
        shim_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let _ambient =
        crate::test_env::AmbientGuard::hermetic_and(&fx.main, None, &[("PATH", Some(&path))]);
    crate::worktree_dismiss::handle_dismiss(Some(clone.to_str().unwrap()), None).unwrap();

    assert!(!ref_exists(&fx.main, &pin), "the base pin outlived dismiss");
    assert!(
        !target.exists(),
        "the clone's cargo target outlived dismiss"
    );
    assert!(
        !crate::lease_path(&fx.main, id).exists(),
        "the clone lease outlived dismiss"
    );
    let ledger = std::fs::read_to_string(
        fx.main
            .join(".aida/discipline/worktree_dismiss_ledger.jsonl"),
    )
    .unwrap();
    assert!(ledger.contains("seat_clone_dismiss"), "{ledger}");
    let untagged = untagged_git_in(&log, &clone);
    assert!(
        untagged.is_empty(),
        "host git ran in the clone: {untagged:#?}"
    );
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

/// BUG-2224: the count parser names each ref, escapes seat-written names and
/// reads anything malformed as no answer.
#[test]
fn bug_2224_unpushed_count_output_names_every_ref() {
    let (total, refs) = parse_unpushed_count("3\n2 refs/heads/side\n1 refs/stash\n").unwrap();
    assert_eq!(total, 3);
    assert_eq!(
        refs,
        vec![
            "2 commit(s) on refs/heads/side",
            "1 commit(s) on refs/stash"
        ]
    );
    assert_eq!(parse_unpushed_count("0\n").unwrap(), (0, vec![]));
    let (_, refs) = parse_unpushed_count("1\n1 refs/tags/a\u{1b}b\n").unwrap();
    assert!(!refs[0].contains('\u{1b}'), "{refs:?}");
    assert!(parse_unpushed_count("").is_none());
    assert!(parse_unpushed_count("x\n").is_none());
    assert!(parse_unpushed_count("1\nrefs/heads/side\n").is_none());
}

/// BUG-2224: refs a seat clone inherited from the main checkout (local tags,
/// annotated or not, and `wip/*` branches origin never had) are not work
/// origin lacks, so a clean seat ends without `--force`. A seat-only commit
/// on a side branch off an inherited tag, and a stash entry, still refuse,
/// and the refusal names the ref.
#[cfg(target_os = "linux")]
#[test]
fn bug_2224_bwrap_inherited_refs_end_cleanly_but_seat_only_work_refuses() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    if !bwrap_live() {
        return;
    }
    let fx = fixture();
    git_ok(&fx.main, &["checkout", "-q", "-b", "wip/local"]);
    std::fs::write(fx.main.join("local.txt"), "main only\n").unwrap();
    git_ok(&fx.main, &["add", "local.txt"]);
    git_ok(
        &fx.main,
        &[
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "commit",
            "-qm",
            "main only",
        ],
    );
    git_ok(&fx.main, &["tag", "gl-v-check-1"]);
    git_ok(&fx.main, &["tag", "-a", "slice-1-r1", "-m", "slice"]);
    git_ok(&fx.main, &["checkout", "-q", "main"]);

    let id = "b2224c0ffee1";
    let clone = seat(&fx, "inherit", id);
    let main_url = fx.main.to_str().unwrap();
    git_ok(
        &clone,
        &[
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "fetch",
            "-q",
            main_url,
            "refs/tags/*:refs/tags/*",
            "refs/heads/wip/*:refs/heads/wip/*",
        ],
    );
    let facts = known(probe(&fx.main, &clone));
    assert_eq!(facts.unpushed, 0, "{facts:?}");
    assert!(facts.unpushed_refs.is_empty(), "{facts:?}");
    let _ambient = crate::test_env::AmbientGuard::hermetic(&fx.main, None);

    git_ok(&clone, &["checkout", "-q", "-b", "side", "slice-1-r1"]);
    seat_commit(&clone, "side.txt");
    git_ok(&clone, &["checkout", "-q", "feat"]);
    let facts = known(probe(&fx.main, &clone));
    assert_eq!(facts.unpushed, 1, "{facts:?}");
    assert_eq!(facts.unpushed_refs, vec!["1 commit(s) on refs/heads/side"]);
    let refused = format!("{:#}", end_session(id, false).unwrap_err());
    assert!(refused.contains("refs/heads/side"), "{refused}");
    git_ok(&clone, &["branch", "-q", "-D", "side"]);

    std::fs::write(clone.join("README.md"), "stashed\n").unwrap();
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
    let facts = known(probe(&fx.main, &clone));
    assert!(facts.unpushed >= 1, "{facts:?}");
    let refused = format!("{:#}", end_session(id, false).unwrap_err());
    assert!(refused.contains("refs/stash"), "{refused}");
    assert!(clone.exists() && crate::lease_path(&fx.main, id).exists());

    git_ok(&clone, &["stash", "clear"]);
    end_session(id, false).unwrap();
    assert!(!clone.exists());
    assert!(!crate::lease_path(&fx.main, id).exists());
}

/// BUG-B6B1278890774EF993D2EBC327862470-023: an orphaned clone lease whose
/// tree holds only ignored seat state (its `.aida/seat-clone.toml` marker)
/// reads clean to the lease classifiers. Host git never runs in a clone, so
/// the host `git status` reader counts the refusal as one uncommitted change;
/// the lease reader probes the clone in its sandbox instead, and still counts
/// real work there: untracked files and commits origin does not have.
// trace:BUG-B6B1278890774EF993D2EBC327862470-023 | ai:claude
#[cfg(target_os = "linux")]
#[test]
fn task_2053_bwrap_bug_023_an_orphaned_clone_lease_with_only_ignored_state_reads_clean() {
    if !bwrap_live() {
        return;
    }
    let fx = fixture();
    aida_core::seat_clone_detect::note_host_main(&fx.main);
    let clone = seat(&fx, "orphan", "c0ffee000023");
    assert!(clone.join(MARKER_RELATIVE).exists());
    let lease = crate::list_leases(&fx.main)
        .into_iter()
        .find(|lease| lease.id == "c0ffee000023")
        .expect("clone lease");

    // The host reader refuses the clone: the "1 uncommitted change(s)".
    assert_eq!(
        crate::status_sections::worktree_dirty_entries(&clone).len(),
        1
    );
    let entries = crate::leases::lease_tree_dirty_entries(&lease);
    assert!(entries.is_empty(), "{entries:?}");
    let report = crate::leases::stale_lease_recovery_for_lease(&lease);
    assert!(report.dirty.is_empty(), "{:?}", report.dirty);
    assert!(
        report.worktree_state_phrase().contains("CLEAN"),
        "{}",
        report.worktree_state_phrase()
    );

    std::fs::write(clone.join("untracked.txt"), "u\n").unwrap();
    let entries = crate::leases::lease_tree_dirty_entries(&lease);
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert!(entries[0].contains("untracked.txt"), "{entries:?}");

    std::fs::remove_file(clone.join("untracked.txt")).unwrap();
    seat_commit(&clone, "a.txt");
    let entries = crate::leases::lease_tree_dirty_entries(&lease);
    assert_eq!(
        entries,
        vec!["1 commit(s) that origin does not have".to_string()]
    );
}

/// BUG-051: every probe `git status` entry is named by its path, and a set
/// of findings is never rendered as an empty list.
// trace:BUG-B6B1278890774EF993D2EBC327862470-051.ac225260 | ai:claude
// trace:BUG-B6B1278890774EF993D2EBC327862470-051 | ai:claude
#[test]
fn bug_051_refusal_items_name_every_path_and_are_never_empty() {
    assert_eq!(describe_status_entry("? notes.txt"), "untracked: notes.txt");
    assert_eq!(
        describe_status_entry("1 .M N... 100644 100644 100644 0123abc 0123abc src/a b.rs"),
        "changed (.M): src/a b.rs"
    );
    assert_eq!(
        describe_status_entry("2 R. N... 100644 100644 100644 aaa aaa R100 new.rs\told.rs"),
        "renamed (R.): old.rs -> new.rs"
    );
    assert_eq!(
        describe_status_entry("u UU N... 100644 100644 100644 100644 a b c conflict.rs"),
        "unmerged (UU): conflict.rs"
    );
    assert_eq!(describe_status_entry("garbage"), "garbage");

    let clean = ProbeFacts::default();
    assert!(!clean.unshipped());
    assert!(unshipped_items(&clean).is_empty());

    // Commits counted on no listed ref still produce a named line.
    let orphaned = ProbeFacts {
        unpushed: 3,
        ..ProbeFacts::default()
    };
    let items = unshipped_items(&orphaned);
    assert!(items.len() >= 2, "{items:?}");
    assert!(items[0].contains("3 commit(s)"), "{items:?}");
    assert!(unshipped_reason(&orphaned).contains("3 commit(s) on no listed ref"));

    let dirty = ProbeFacts {
        dirty: vec!["? scratch.txt".into()],
        unpushed: 1,
        unpushed_refs: vec!["1 commit(s) on refs/heads/feat".into()],
        ..ProbeFacts::default()
    };
    let items = unshipped_items(&dirty);
    assert_eq!(items[0], "untracked: scratch.txt");
    assert!(items.iter().any(|i| i.contains("refs/heads/feat")));
    let reason = unshipped_reason(&dirty);
    assert!(reason.contains("untracked: scratch.txt"), "{reason}");
    assert!(reason.contains("refs/heads/feat"), "{reason}");
}

/// BUG-051: the session's runtime links and build/cache output are not work.
// trace:BUG-B6B1278890774EF993D2EBC327862470-051.acbe987b | ai:claude
// trace:BUG-B6B1278890774EF993D2EBC327862470-051 | ai:claude
#[cfg(unix)]
#[test]
fn bug_051_runtime_links_and_build_output_are_not_work() {
    let tmp = tempfile::tempdir().unwrap();
    let clone = tmp.path();
    std::os::unix::fs::symlink(tmp.path(), clone.join(".aida-store")).unwrap();
    for noise in [
        "? .aida-store",
        "? .aida/cache.db",
        "? .aida/cache.db-wal",
        "? .aida/agents/claude.json",
        "? target/debug/aida",
    ] {
        assert!(is_runtime_noise_entry(clone, noise), "{noise}");
    }
    for work in [
        "? scratch.txt",
        "? .aida/config.toml",
        "? src/target/x.rs",
        "1 .M N... 100644 100644 100644 a a target/tracked.rs",
    ] {
        assert!(!is_runtime_noise_entry(clone, work), "{work}");
    }
    // A real `.aida-store` directory, not the link, still counts.
    let other = tempfile::tempdir().unwrap();
    std::fs::create_dir(other.path().join(".aida-store")).unwrap();
    assert!(!is_runtime_noise_entry(other.path(), "? .aida-store"));
}

/// A seat clone (lease `id`) whose one commit on `feat` was pushed to origin
/// by URL, after which origin's `feat` gained a commit (an update-branch
/// merge). Neither the clone nor main tracks origin's `feat`, so the plain
/// probe counts the seat commit as one origin lacks.
fn ancestor_of_origin_seat(fx: &Fixture, name: &str, id: &str) -> PathBuf {
    let clone = seat(fx, name, id);
    seat_commit(&clone, "work.txt");
    let origin = fx.origin.to_str().unwrap();
    seat_git(&clone, &["push", "-q", origin, "feat:refs/heads/feat"]);
    let other = fx.root.join(format!("{name}-other"));
    seat_git(
        &fx.root,
        &["clone", "-q", "-b", "feat", origin, other.to_str().unwrap()],
    );
    seat_git(
        &other,
        &["commit", "-q", "--allow-empty", "-m", "merge main"],
    );
    seat_git(&other, &["push", "-q", "origin", "feat"]);
    // The precondition is that neither the clone nor main tracks origin's
    // branch: with a tracking ref at the seat's tip the probe would count
    // nothing whatever the ancestor rule did. Whether the push leaves one
    // depends on the git version (CI's did), so remove it rather than assume.
    for dir in [&clone, &fx.main] {
        if ref_exists(dir, "refs/remotes/origin/feat") {
            git_ok(dir, &["update-ref", "-d", "refs/remotes/origin/feat"]);
        }
        assert!(!ref_exists(dir, "refs/remotes/origin/feat"));
    }
    assert_eq!(known(probe(&fx.main, &clone)).unpushed, 1);
    clone
}

/// BUG-051: the lease classifier (reap, status and the dormant-lease checks
/// read clones the same way) applies session end's ancestor-of-origin rule:
/// the clone reads clean, while a seat commit origin lacks still counts.
// trace:BUG-B6B1278890774EF993D2EBC327862470-051.ac79099f | ai:claude
// trace:BUG-B6B1278890774EF993D2EBC327862470-051 | ai:claude
#[cfg(target_os = "linux")]
#[test]
fn bug_051_bwrap_lease_classifier_counts_an_ancestor_of_origin_as_pushed() {
    if !bwrap_live() {
        return;
    }
    let fx = fixture();
    aida_core::seat_clone_detect::note_host_main(&fx.main);
    let id = "c0ffee000053";
    let clone = ancestor_of_origin_seat(&fx, "bug051c", id);
    let lease = crate::list_leases(&fx.main)
        .into_iter()
        .find(|lease| lease.id == id)
        .expect("clone lease");
    let entries = crate::leases::lease_tree_dirty_entries(&lease);
    assert!(entries.is_empty(), "{entries:?}");
    assert!(ref_exists(&fx.main, "refs/remotes/origin/feat"));

    seat_commit(&clone, "more.txt");
    let entries = crate::leases::lease_tree_dirty_entries(&lease);
    assert_eq!(
        entries,
        vec!["1 commit(s) that origin does not have".to_string()]
    );
}

/// BUG-051: the ref-tips probe output splits into ref tips and the candidate
/// commits origin lacks, so a seat commit on top of one main reaches leaves
/// that one excused; output without the separator is all ref tips.
// trace:BUG-B6B1278890774EF993D2EBC327862470-051 | ai:claude
#[test]
fn bug_051_split_tips_separates_ref_tips_from_candidates() {
    let split = crate::seat_clone::split_tips;
    assert_eq!(split("a\nb\n--\nc\nd\n"), (vec!["a", "b"], vec!["c", "d"]));
    assert_eq!(split("a\n--\n"), (vec!["a"], vec![]));
    assert_eq!(split("a\nb\n"), (vec!["a", "b"], vec![]));
    assert_eq!(split(""), (vec![], vec![]));
}

/// BUG-051: the refresh fetch never runs without budget, or for a branch name
/// git would not accept.
// trace:BUG-B6B1278890774EF993D2EBC327862470-051.ac79099f | ai:claude
// trace:BUG-B6B1278890774EF993D2EBC327862470-051 | ai:claude
#[test]
fn bug_051_refresh_origin_branch_needs_budget_and_a_valid_branch() {
    let fx = fixture();
    let refresh = crate::seat_clone::refresh_origin_branch;
    let budget = std::time::Duration::from_secs(5);
    assert!(!refresh(&fx.main, "main", std::time::Duration::ZERO));
    assert!(!refresh(&fx.main, "", budget));
    assert!(!refresh(&fx.main, "-x", budget));
    assert!(!refresh(&fx.main, "a..b", budget));
    // origin's `main` is already tracked at the same commit: nothing moved.
    assert!(!refresh(&fx.main, "main", budget));
    git_ok(&fx.main, &["update-ref", "-d", "refs/remotes/origin/main"]);
    assert!(refresh(&fx.main, "main", budget));
    assert!(ref_exists(&fx.main, "refs/remotes/origin/main"));
}

/// BUG-051: a seat branch published by URL (no tracking ref in the clone or
/// main), whose origin copy then gained a commit (an update-branch merge), is
/// pushed: session end, without `--force`, removes the clone. The session's
/// runtime links and build output do not hold it either.
// trace:BUG-B6B1278890774EF993D2EBC327862470-051.ac79099f | ai:claude
// trace:BUG-B6B1278890774EF993D2EBC327862470-051.Tests | ai:claude
// trace:BUG-B6B1278890774EF993D2EBC327862470-051 | ai:claude
#[cfg(target_os = "linux")]
#[test]
fn bug_051_bwrap_session_end_ends_a_clone_whose_branch_is_an_ancestor_of_origin() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    if !bwrap_live() {
        return;
    }
    let fx = fixture();
    let id = "c0ffee000051";
    let clone = ancestor_of_origin_seat(&fx, "bug051a", id);

    std::os::unix::fs::symlink(&fx.main, clone.join(".aida-store")).unwrap();
    std::fs::create_dir_all(clone.join(".aida/agents")).unwrap();
    std::fs::write(clone.join(".aida/agents/claude.json"), "{}").unwrap();
    std::fs::write(clone.join(".aida/cache.db"), "db").unwrap();
    std::fs::create_dir_all(clone.join("target/debug")).unwrap();
    std::fs::write(clone.join("target/debug/aida"), "bin").unwrap();

    let _ambient = crate::test_env::AmbientGuard::hermetic(&fx.main, None);
    end_session(id, false).unwrap();
    assert!(!clone.exists());
    assert!(!crate::lease_path(&fx.main, id).exists());
    assert!(!fx
        .main
        .join(".aida/salvage")
        .join(clone.file_name().unwrap())
        .exists());
}

/// BUG-051: an untracked file refuses session end, and the refusal names it.
// trace:BUG-B6B1278890774EF993D2EBC327862470-051.ac225260 | ai:claude
// trace:BUG-B6B1278890774EF993D2EBC327862470-051.Tests | ai:claude
// trace:BUG-B6B1278890774EF993D2EBC327862470-051 | ai:claude
#[cfg(target_os = "linux")]
#[test]
fn bug_051_bwrap_session_end_refusal_names_the_untracked_file() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    if !bwrap_live() {
        return;
    }
    let fx = fixture();
    let id = "c0ffee000052";
    let clone = seat(&fx, "bug051b", id);
    std::fs::write(clone.join("scratch-notes.txt"), "draft\n").unwrap();
    let _ambient = crate::test_env::AmbientGuard::hermetic(&fx.main, None);
    let refused = format!("{:#}", end_session(id, false).unwrap_err());
    assert!(
        refused.contains("untracked: scratch-notes.txt"),
        "{refused}"
    );
    assert!(clone.exists());
    assert!(crate::lease_path(&fx.main, id).exists());
}

/// BUG-049 (ruling 3): the real seat-clone teardown removes the clone's
/// Claude Code trust entry when pre-trust is on, and (ruling 1) leaves
/// `~/.claude.json` byte-identical when it is off.
// trace:BUG-B6B1278890774EF993D2EBC327862470-049.ac15c23c | ai:claude
#[cfg(target_os = "linux")]
#[test]
fn bug_b6b_049_bwrap_destroy_drops_the_clone_trust_entry_only_when_on() {
    if !bwrap_live() {
        return;
    }
    use crate::claude_trust::{ensure_seat_trusted_in, TrustWrite, ENV_OVERRIDE};
    let fx = fixture();
    let home = crate::home_dir().expect("hermetic test home");
    let read_projects = || -> serde_json::Value {
        std::fs::read_to_string(home.join(".claude.json"))
            .ok()
            .and_then(|b| serde_json::from_str::<serde_json::Value>(&b).ok())
            .map(|v| v["projects"].clone())
            .unwrap_or(serde_json::Value::Null)
    };

    // On: the entry written at launch goes with the clone.
    let on = seat(&fx, "trust-on", "c0ffee000491");
    let key = on.to_string_lossy().into_owned();
    {
        let _env = crate::test_env::EnvVarsGuard::apply(&[(ENV_OVERRIDE, Some("1"))]);
        assert!(matches!(
            ensure_seat_trusted_in(&home, &fx.main, &on).unwrap(),
            TrustWrite::Written(_) | TrustWrite::AlreadyTrusted(_)
        ));
        assert!(read_projects().get(&key).is_some());
        let removed = destroy(&fx.main, &on, DestroyMode::SalvageUnshipped, None).unwrap();
        assert!(matches!(removed, Destroyed::Removed { .. }), "{removed:?}");
    }
    assert!(!on.exists());
    assert!(
        read_projects().get(&key).is_none(),
        "entry outlived the clone"
    );

    // Off: teardown never touches the file, even for a seat with an entry.
    let off = seat(&fx, "trust-off", "c0ffee000492");
    let off_key = off.to_string_lossy().into_owned();
    {
        let _env = crate::test_env::EnvVarsGuard::apply(&[(ENV_OVERRIDE, Some("1"))]);
        ensure_seat_trusted_in(&home, &fx.main, &off).unwrap();
    }
    let before = std::fs::read(home.join(".claude.json")).unwrap();
    {
        let _env = crate::test_env::EnvVarsGuard::apply(&[(ENV_OVERRIDE, Some("0"))]);
        let removed = destroy(&fx.main, &off, DestroyMode::SalvageUnshipped, None).unwrap();
        assert!(matches!(removed, Destroyed::Removed { .. }), "{removed:?}");
        assert_eq!(std::fs::read(home.join(".claude.json")).unwrap(), before);
    }
    assert!(read_projects().get(&off_key).is_some());
}

// Exercise the actual sandbox script with real git, logging child invocations
// without mutating this test process's PATH. Includes missing objects and a blob
// so the batch must not turn either into evidence that work is already shipped.
// trace:BUG-2384 | ai:codex
#[test]
fn bug_2384_large_shipped_set_uses_one_batch_per_clone() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let seed = temp.path().join("seed");
    std::fs::create_dir(&seed).unwrap();
    seat_git(&seed, &["init", "-q", "-b", "main"]);
    seat_git(&seed, &["config", "user.name", "Test"]);
    seat_git(&seed, &["config", "user.email", "test@example.com"]);
    std::fs::write(seed.join("file"), "base\n").unwrap();
    seat_git(&seed, &["add", "."]);
    seat_git(&seed, &["commit", "-qm", "base"]);
    let base = seat_git(&seed, &["rev-parse", "HEAD"]);
    std::fs::write(seed.join("file"), "unshipped\n").unwrap();
    seat_git(&seed, &["commit", "-qam", "unshipped"]);
    let head = seat_git(&seed, &["rev-parse", "HEAD"]);
    let blob = seat_git(&seed, &["rev-parse", "HEAD:file"]);
    let bin = temp.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let real_git = std::process::Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    assert!(real_git.status.success());
    let wrapper = bin.join("git");
    std::fs::write(
        &wrapper,
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$GIT_TEST_CALLS\"\nexec \"$GIT_TEST_REAL\" \"$@\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let path = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(&inherited)))
        .unwrap();
    let calls = temp.path().join("calls");
    let mut shipped: Vec<String> = (1..=6400).map(|i| format!("{i:040x}")).collect();
    shipped.extend([base.trim().to_string(), blob.trim().to_string()]);
    for index in 0..25 {
        let clone = temp.path().join(format!("clone-{index}"));
        seat_git(
            temp.path(),
            &[
                "clone",
                "-q",
                seed.to_str().unwrap(),
                clone.to_str().unwrap(),
            ],
        );
        // Its origin has only the pinned base: the last commit must be kept.
        seat_git(
            &clone,
            &["update-ref", "refs/remotes/origin/main", base.trim()],
        );
        let run = |ids: &[String]| {
            let out = std::process::Command::new("sh")
                .current_dir(&clone)
                .args(["-c", UNPUSHED_COUNT_SCRIPT, "aida-seat-unpushed", ""])
                .args(ids)
                .env("PATH", &path)
                .env("GIT_TEST_CALLS", &calls)
                .env(
                    "GIT_TEST_REAL",
                    String::from_utf8_lossy(&real_git.stdout).trim(),
                )
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            parse_unpushed_count(&String::from_utf8(out.stdout).unwrap()).unwrap()
        };
        let (count, refs) = run(&shipped);
        assert_eq!(count, 1);
        assert!(refs.iter().any(|r| r.contains("refs/heads/main")));
        // A real shipped commit excuses its ancestors, without losing the
        // diagnostic on a later genuinely unshipped commit above it.
        let mut all_shipped = shipped.clone();
        all_shipped.push(head.trim().to_string());
        assert_eq!(run(&all_shipped), (0, vec![]));
    }
    let calls = std::fs::read_to_string(calls).unwrap();
    assert_eq!(
        calls.lines().filter(|l| l.starts_with("cat-file ")).count(),
        50
    );
    assert!(calls.lines().all(|l| !l.starts_with("cat-file -e")));
    assert!(
        calls.lines().count() < 25 * 20,
        "git invocation count must scale with refs, not shipped SHAs"
    );
}

// An elapsed budget is unknown evidence, never an empty/zero commit result.
// This exercises refusal before bwrap needs to be available on the host.
// trace:BUG-2384 | ai:codex
#[test]
fn bug_2384_expired_clone_scan_keeps_unknown_diagnostic() {
    let fx = fixture();
    let clone = seat(&fx, "expired-budget", "01bug2384expired");
    match probe_until(&fx.main, &clone, &[], Some(std::time::Instant::now())) {
        Probe::Unknown(why) => assert!(why.contains("deadline elapsed"), "{why}"),
        other => panic!("an elapsed budget must retain the clone: {other:?}"),
    }
}
