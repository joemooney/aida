//! BUG-2418: CARGO_HOME is read-only inside a contained lane.
//! TASK-2772: the lane's generated sandbox settings let the LIGHT gate write
//! the cargo slot pair, never the shared sccache cache or CARGO_HOME.
// trace:BUG-2418 TASK-2772 | ai:claude
use super::*;
use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path, args: &[&str]) {
    let out = crate::test_git::git_command()
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A main checkout with a store directory, as `bug_1929`'s fixture lays out.
fn main_checkout() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let main = std::fs::canonicalize(tmp.path()).unwrap().join("main");
    std::fs::create_dir_all(main.join(".aida-store/objects")).unwrap();
    std::fs::create_dir_all(main.join(".aida")).unwrap();
    git(&main, &["init", "-q", "-b", "main"]);
    (tmp, main)
}

fn allow_write(settings: &str) -> Vec<String> {
    let value: serde_json::Value = serde_json::from_str(settings).unwrap();
    value["sandbox"]["filesystem"]["allowWrite"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn under(entry: &str, dir: &Path) -> bool {
    // Sandbox entries spell an absolute path with a leading `//`.
    let path = Path::new(entry.trim_start_matches('/'));
    let dir = dir.strip_prefix("/").unwrap_or(dir);
    path.starts_with(dir) || dir.starts_with(path)
}

/// BUG-2418 test 1: the contained profiles (the os_wrap bwrap binds and the
/// Claude sandbox settings) never list CARGO_HOME or `~/.cargo` read-write.
#[test]
fn bug_2418_contained_render_has_no_cargo_home_in_rw_roots() {
    let (_tmp, main) = main_checkout();
    // trace:BUG-2418 | ai:claude
    let _lock = crate::test_env::env_lock();
    let mut cargo_homes: Vec<PathBuf> = crate::session::cargo_home_dir().into_iter().collect();
    if let Some(home) = crate::home_dir() {
        cargo_homes.push(home.join(".cargo"));
    }
    assert!(
        !cargo_homes.is_empty(),
        "no home directory to check against"
    );

    let rw = crate::session::os_wrap_rw_paths(crate::seat_clone::launcher_main().as_deref(), &main)
        .unwrap();
    for cargo_home in &cargo_homes {
        assert!(
            rw.iter()
                .all(|p| !p.starts_with(cargo_home) && !cargo_home.starts_with(p)),
            "os_wrap binds {} read-write: {rw:?}",
            cargo_home.display()
        );
    }
    let home = Path::new("/home/someone");
    assert!(crate::session::os_wrap_home_rw_paths(home)
        .iter()
        .all(|p| !p.starts_with(home.join(".cargo"))));

    let settings = crate::session::claude_contained_settings_json(
        crate::seat_clone::launcher_main().as_deref(),
        &contained_write_roots(&main),
    )
    .unwrap();
    let allowed = allow_write(&settings);
    for cargo_home in &cargo_homes {
        assert!(
            allowed.iter().all(|e| !under(e, cargo_home)),
            "the contained settings let Bash write {}: {allowed:?}",
            cargo_home.display()
        );
    }

    // The bwrap profile binds CARGO_HOME read-only, as a `--ro-bind-try`.
    let ro = crate::session::os_wrap_cargo_home_ro_args(Path::new("/h/.cargo"));
    assert_eq!(ro, ["--ro-bind-try", "/h/.cargo", "/h/.cargo"]);
}

fn bwrap_usable() -> bool {
    Command::new("bwrap")
        .args([
            "--ro-bind",
            "/",
            "/",
            "--dev",
            "/dev",
            "--proc",
            "/proc",
            "true",
        ])
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Run `sh -c script` inside the lane profile: `rw` read-write binds, then
/// CARGO_HOME read-only, exactly as the os_wrap launch orders them.
fn in_lane(wt: &Path, rw: &[PathBuf], cargo_home: &Path, script: &str) -> bool {
    let mut args = crate::session::bwrap_confinement_args_inner(wt, rw, &[]);
    args.extend(crate::session::os_wrap_cargo_home_ro_args(cargo_home));
    Command::new("bwrap")
        .args(&args)
        .args(["sh", "-c", script])
        .output()
        .unwrap()
        .status
        .success()
}

/// BUG-2418 test 2 (live): inside the lane, a write to `<CARGO_HOME>/bin/x`
/// and to the registry sources is refused, even when a broader read-write
/// root covers CARGO_HOME; the worktree and the npm cache stay writable.
#[test]
fn bug_2418_live_bwrap_refuses_cargo_home_writes() {
    if !bwrap_usable() {
        eprintln!("SKIP (bwrap cannot create a user namespace here)");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let wt = root.join("wt");
    let home = root.join("home");
    let cargo_home = home.join(".cargo");
    let src = cargo_home.join("registry/src/index.crates.io-0/serde-1.0.0");
    for dir in [&wt, &home.join(".npm"), &cargo_home.join("bin"), &src] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let mut rw = crate::session::os_wrap_home_rw_paths(&home);
    rw.push(home.clone()); // a covering read-write root must not win

    let bin_x = cargo_home.join("bin/x");
    let planted = src.join("lib.rs");
    for target in [&bin_x, &planted] {
        let script = format!("echo planted > '{}'", target.display());
        assert!(
            !in_lane(&wt, &rw, &cargo_home, &script),
            "the lane wrote {}",
            target.display()
        );
        assert!(!target.exists(), "{} reached the host", target.display());
    }

    // Control: the same sandbox writes where it should, so the refusals above
    // are the read-only bind, not a broken sandbox.
    let ok = format!(
        "echo ok > '{}' && echo ok > '{}'",
        wt.join("f").display(),
        home.join(".npm/f").display()
    );
    assert!(in_lane(&wt, &rw, &cargo_home, &ok));
    assert!(wt.join("f").exists() && home.join(".npm/f").exists());
}

/// BUG-2418: a cargo home the lane cannot write maps to the one-line note.
#[test]
fn bug_2418_read_only_cargo_home_maps_to_one_line() {
    let tmp = tempfile::tempdir().unwrap();
    assert_eq!(crate::session::cargo_home_read_only_note(tmp.path()), None);
    assert_eq!(
        crate::session::cargo_home_read_only_note(&tmp.path().join("absent")),
        None
    );
    // SAFETY: geteuid never fails and has no preconditions.
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("SKIP (root ignores directory write bits)");
        return;
    }
    use std::os::unix::fs::PermissionsExt;
    let ro = tmp.path().to_path_buf();
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
    let note = crate::session::cargo_home_read_only_note(&ro);
    // Restore write access so the temporary directory can be removed.
    crate::test_exec::mark_executable(&ro);
    assert_eq!(
        note,
        Some("new dependencies are fetched on the host: run cargo fetch outside the lane")
    );
}

// trace:STORY-2695 | ai:codex
fn lane_env(root: &Path) -> LaneBuildEnv {
    for dir in ["runtime", "home/.cargo"] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    LaneBuildEnv {
        runtime_dir: Some(root.join("runtime")),
        uid: Some(1000),
        home: Some(root.join("home")),
        cargo_home: Some(root.join("home/.cargo")),
    }
}

/// TASK-2772: exactly the slot pair the cargo slot wrapper writes; never the
/// shared sccache cache (advisor correction, ruling O2).
#[test]
fn bug_2418_task_2772_lane_build_paths_are_only_the_slot_pair() {
    let (_tmp, root) = real_tmp();
    let paths = lane_build_write_paths_from(&lane_env(&root));
    assert_eq!(
        paths,
        [
            root.join("runtime/cargo-slots-1000"),
            root.join("home/.local/state/cargo-slots"),
        ]
    );
    assert!(paths
        .iter()
        .all(|p| !p.to_string_lossy().contains("sccache")));
}

/// TASK-2772: no lane build path is ever CARGO_HOME, inside it, or above it;
/// relative paths are dropped.
#[test]
fn bug_2418_task_2772_lane_build_paths_never_reach_cargo_home() {
    let (_tmp, root) = real_tmp();
    let env = LaneBuildEnv {
        runtime_dir: Some(root.join("home/.cargo")),
        ..lane_env(&root)
    };
    assert_eq!(
        lane_build_write_paths_from(&env),
        [root.join("home/.local/state/cargo-slots")]
    );

    let env = LaneBuildEnv {
        cargo_home: Some(root.join("home")),
        ..lane_env(&root)
    };
    assert_eq!(
        lane_build_write_paths_from(&env),
        [root.join("runtime/cargo-slots-1000")]
    );

    let env = LaneBuildEnv {
        runtime_dir: Some(PathBuf::from("run")),
        home: None,
        ..lane_env(&root)
    };
    assert!(lane_build_write_paths_from(&env).is_empty());
}

/// TASK-2772: the generated contained settings grant the lane build paths
/// to sandboxed Bash, beside the store grant.
#[test]
fn bug_2418_task_2772_contained_settings_allow_lane_build_paths() {
    let (_tmp, main) = main_checkout();
    // Siblings pin XDG_RUNTIME_DIR/CARGO_HOME under the env lock; read both
    // sides under it too. trace:BUG-2418 | ai:claude
    let (settings, expected) = {
        let _lock = crate::test_env::env_lock();
        (
            crate::session::claude_contained_settings_json(
                crate::seat_clone::launcher_main().as_deref(),
                &contained_write_roots(&main),
            )
            .unwrap(),
            sandbox_allow_write_entries(&lane_build_write_paths()),
        )
    };
    let allowed = allow_write(&settings);
    assert!(!expected.is_empty(), "no lane build path resolved here");
    for entry in &expected {
        assert!(allowed.contains(entry), "{entry} missing from {allowed:?}");
    }
    // Never the shared sccache cache, under any spelling (ruling O2).
    assert!(
        allowed.iter().all(|e| !e.contains("sccache")),
        "sccache granted: {allowed:?}"
    );
    // The tool write boundary is unchanged: no Edit/Write rule for them.
    let value: serde_json::Value = serde_json::from_str(&settings).unwrap();
    let rules = value["permissions"]["allow"].as_array().unwrap();
    for entry in &expected {
        assert!(
            rules
                .iter()
                .filter_map(|r| r.as_str())
                .all(|r| !(r.starts_with("Edit(") && r.contains(entry.as_str()))),
            "an Edit rule grants {entry}"
        );
    }
}

fn uid() -> u32 {
    // SAFETY: getuid never fails and has no preconditions.
    unsafe { libc::getuid() }
}

/// A tempdir resolved through any symlinked `/tmp`, so expected paths match
/// what [`resolve_through_existing`] returns.
fn real_tmp() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    (tmp, root)
}

/// BUG-2418 (review F1): a CARGO_HOME that is a symlink to the real slot
/// directory, or a slot path reached through a symlink into CARGO_HOME, is
/// never granted, though the spellings differ.
#[test]
fn bug_2418_lane_build_paths_resolve_cargo_home_aliases() {
    let (_tmp, root) = real_tmp();
    let run = root.join("run");
    let slot = run.join("cargo-slots-1000");
    std::fs::create_dir_all(&slot).unwrap();
    let link = root.join("cargo-link");
    std::os::unix::fs::symlink(&slot, &link).unwrap();
    let env = LaneBuildEnv {
        runtime_dir: Some(run.clone()),
        uid: Some(1000),
        home: None,
        cargo_home: Some(link.clone()),
    };
    let (paths, notes) = lane_build_write_grants_from(&env);
    assert!(paths.is_empty(), "granted CARGO_HOME's target: {paths:?}");
    assert!(
        notes.is_empty(),
        "an overlap is not an ambiguity: {notes:?}"
    );

    // CARGO_HOME inside the slot's real target, reached through an alias.
    let cargo_home = root.join("real-cargo");
    std::fs::create_dir_all(&cargo_home).unwrap();
    let home = root.join("home");
    std::fs::create_dir_all(home.join(".local")).unwrap();
    std::os::unix::fs::symlink(&cargo_home, home.join(".local/state")).unwrap();
    let env = LaneBuildEnv {
        runtime_dir: None,
        uid: None,
        home: Some(home.clone()),
        cargo_home: Some(cargo_home.clone()),
    };
    assert!(lane_build_write_paths_from(&env).is_empty());

    // A slot that does not exist yet resolves through its existing parent.
    let env = LaneBuildEnv {
        runtime_dir: Some(run.join("missing")),
        uid: Some(7),
        home: None,
        cargo_home: Some(cargo_home),
    };
    assert_eq!(
        lane_build_write_paths_from(&env),
        [run.join("missing/cargo-slots-7")]
    );
}

/// BUG-2418 (review F1): when CARGO_HOME or the slot cannot be resolved (a
/// dangling symlink on the way), the slot is refused with one note each.
#[test]
fn bug_2418_lane_build_paths_refuse_unresolvable_sides() {
    let (_tmp, root) = real_tmp();
    let run = root.join("run");
    std::fs::create_dir_all(&run).unwrap();
    let dangling = root.join("dangling");
    std::os::unix::fs::symlink(root.join("nowhere"), &dangling).unwrap();

    let env = LaneBuildEnv {
        runtime_dir: Some(run.clone()),
        uid: Some(1000),
        home: None,
        cargo_home: Some(dangling.join("cargo")),
    };
    let (paths, notes) = lane_build_write_grants_from(&env);
    assert!(paths.is_empty());
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains("CARGO_HOME"), "{notes:?}");

    let env = LaneBuildEnv {
        runtime_dir: Some(dangling.clone()),
        uid: Some(1000),
        home: None,
        cargo_home: Some(root.join("cargo")),
    };
    let (paths, notes) = lane_build_write_grants_from(&env);
    assert!(paths.is_empty());
    assert_eq!(notes.len(), 1, "{notes:?}");

    let env = LaneBuildEnv {
        runtime_dir: Some(run),
        uid: Some(1000),
        home: None,
        cargo_home: None,
    };
    assert!(lane_build_write_paths_from(&env).is_empty());
}

/// BUG-2418 (review F1 regression): with CARGO_HOME a symlink to the real
/// slot directory, the generated contained settings carry no allowWrite entry
/// for it.
#[test]
fn bug_2418_contained_settings_never_grant_aliased_cargo_home() {
    let (_tmp, main) = main_checkout();
    let (_tmp2, root) = real_tmp();
    let run = root.join("run");
    let slot = run.join(format!("cargo-slots-{}", uid()));
    std::fs::create_dir_all(&slot).unwrap();
    let link = root.join("cargo-link");
    std::os::unix::fs::symlink(&slot, &link).unwrap();
    let run_s = run.to_string_lossy().into_owned();
    let link_s = link.to_string_lossy().into_owned();
    let settings = {
        let _env = crate::test_env::EnvVarsGuard::set(&[
            ("XDG_RUNTIME_DIR", run_s.as_str()),
            ("CARGO_HOME", link_s.as_str()),
        ]);
        crate::session::claude_contained_settings_json(
            crate::seat_clone::launcher_main().as_deref(),
            &contained_write_roots(&main),
        )
        .unwrap()
    };
    let allowed = allow_write(&settings);
    for dir in [&slot, &link] {
        assert!(
            allowed.iter().all(|e| !under(e, dir)),
            "the settings let Bash write CARGO_HOME ({}): {allowed:?}",
            dir.display()
        );
    }
}

/// TASK-2772 (review F2): the production os_wrap launch rw-binds the slot
/// pair and binds CARGO_HOME read-only only; live, the wrapped program writes
/// a slot and cannot write CARGO_HOME.
#[test]
fn task_2772_os_wrapped_launch_binds_slots_rw_and_cargo_home_ro() {
    if !bwrap_usable() {
        eprintln!("SKIP (bwrap cannot create a user namespace here)");
        return;
    }
    let (_tmp, main) = main_checkout();
    let (_tmp2, root) = real_tmp();
    let run = root.join("run");
    let slot = run.join(format!("cargo-slots-{}", uid()));
    let cargo_home = root.join("cargo");
    std::fs::create_dir_all(cargo_home.join("bin")).unwrap();
    std::fs::create_dir_all(&run).unwrap();
    let run_s = run.to_string_lossy().into_owned();
    let cargo_s = cargo_home.to_string_lossy().into_owned();
    let script = format!(
        "echo ok > '{slot}/lock' && ! (echo x > '{cargo}/bin/x') 2>/dev/null",
        slot = slot.display(),
        cargo = cargo_home.display()
    );
    let (program, args) = {
        let _env = crate::test_env::EnvVarsGuard::set(&[
            ("AIDA_OS_WRAP", "1"),
            ("XDG_RUNTIME_DIR", run_s.as_str()),
            ("CARGO_HOME", cargo_s.as_str()),
        ]);
        crate::session::os_wrapped_program_and_args(
            crate::seat_clone::launcher_main().as_deref(),
            &main,
            "sh",
            vec!["-c".to_string(), script],
        )
        .unwrap()
    };
    assert_eq!(program, "bwrap");
    let slot_s = slot.to_string_lossy().into_owned();
    let rw_bound = |path: &str| {
        args.windows(3)
            .any(|w| (w[0] == "--bind" || w[0] == "--bind-try") && w[1] == path && w[2] == path)
    };
    assert!(rw_bound(&slot_s), "slot not rw-bound: {args:?}");
    assert!(
        args.windows(2)
            .all(|w| !((w[0] == "--bind" || w[0] == "--bind-try")
                && Path::new(&w[1]).starts_with(&cargo_home))),
        "CARGO_HOME rw-bound: {args:?}"
    );
    assert!(
        args.windows(3)
            .any(|w| w[0] == "--ro-bind-try" && w[1] == cargo_s),
        "CARGO_HOME not bound read-only: {args:?}"
    );

    let out = Command::new(&program).args(&args).output().unwrap();
    assert!(
        out.status.success(),
        "wrapped launch: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(slot.join("lock").exists(), "the slot write did not land");
    assert!(!cargo_home.join("bin/x").exists(), "CARGO_HOME was written");
}
