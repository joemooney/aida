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
    let mut cargo_homes: Vec<PathBuf> = crate::session::cargo_home_dir().into_iter().collect();
    if let Some(home) = crate::home_dir() {
        cargo_homes.push(home.join(".cargo"));
    }
    assert!(
        !cargo_homes.is_empty(),
        "no home directory to check against"
    );

    let rw = crate::session::os_wrap_rw_paths(&main).unwrap();
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

    let settings =
        crate::session::claude_contained_settings_json(&contained_write_roots(&main)).unwrap();
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

fn lane_env() -> LaneBuildEnv {
    LaneBuildEnv {
        runtime_dir: Some(PathBuf::from("/run/user/1000")),
        uid: Some(1000),
        home: Some(PathBuf::from("/home/u")),
        cargo_home: Some(PathBuf::from("/home/u/.cargo")),
    }
}

/// TASK-2772: exactly the slot pair the cargo slot wrapper writes; never the
/// shared sccache cache (advisor correction, ruling O2).
#[test]
fn bug_2418_task_2772_lane_build_paths_are_only_the_slot_pair() {
    let paths = lane_build_write_paths_from(&lane_env());
    assert_eq!(
        paths,
        [
            PathBuf::from("/run/user/1000/cargo-slots-1000"),
            PathBuf::from("/home/u/.local/state/cargo-slots"),
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
    let env = LaneBuildEnv {
        runtime_dir: Some(PathBuf::from("/home/u/.cargo")),
        ..lane_env()
    };
    assert_eq!(
        lane_build_write_paths_from(&env),
        [PathBuf::from("/home/u/.local/state/cargo-slots")]
    );

    let env = LaneBuildEnv {
        cargo_home: Some(PathBuf::from("/home/u")),
        ..lane_env()
    };
    assert_eq!(
        lane_build_write_paths_from(&env),
        [PathBuf::from("/run/user/1000/cargo-slots-1000")]
    );

    let env = LaneBuildEnv {
        runtime_dir: Some(PathBuf::from("run")),
        home: None,
        ..lane_env()
    };
    assert!(lane_build_write_paths_from(&env).is_empty());
}

/// TASK-2772: the generated contained settings grant the lane build paths
/// to sandboxed Bash, beside the store grant.
#[test]
fn bug_2418_task_2772_contained_settings_allow_lane_build_paths() {
    let (_tmp, main) = main_checkout();
    let settings =
        crate::session::claude_contained_settings_json(&contained_write_roots(&main)).unwrap();
    let allowed = allow_write(&settings);
    let expected = sandbox_allow_write_entries(&lane_build_write_paths());
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
