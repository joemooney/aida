//! BUG-2381: tests never discover, lock or write the operator's live store.
//! libtest starts in the crate directory; in a seat clone or linked worktree
//! that checkout's `.aida-store` is the LIVE store, so process-cwd store
//! discovery handed a fixture `session_start` the shared store write lock of
//! the operator's main checkout. Also covers the hermetic checkout the
//! unpinned containment tests resolve: no operator git config, an explicit
//! `main` branch, and removed when the test process exits.
//! trace:BUG-2381 | ai:claude

use std::path::{Path, PathBuf};

use crate::session::contained_cwd_seam::{hermetic_checkout, hermetic_git_init};

fn workspace() -> PathBuf {
    std::fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()).unwrap()
}

/// A main checkout under a tempdir whose `.aida/config.toml` names a store
/// directory that exists: the shape `session_start`'s fixtures pin.
fn store_fixture() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let main = tmp.path().canonicalize().unwrap().join("aida");
    std::fs::create_dir_all(main.join(".aida")).unwrap();
    let status = hermetic_git_init(&main).status().unwrap();
    assert!(status.success());
    std::fs::write(
        main.join(".aida/config.toml"),
        "[deployment]\nstore_path = \".aida-store\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(main.join(".aida-store/objects")).unwrap();
    (tmp, main)
}

/// The blocking review item: a test that pinned its fixture root resolves
/// the store, and so the store write lock, under its own tempdir. Before the
/// fix the walk started at the process cwd, which from a seat clone reached
/// the live store (and in CI found no store at all), so both asserts failed.
#[test]
fn bug_2381_a_pinned_fixture_resolves_its_own_store_and_store_lock() {
    let (tmp, main) = store_fixture();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        &main,
        "implementer",
        &[],
        &[("AIDA_STORE", None)],
    );

    let store = crate::store_sync::detect_distributed_store().expect("the fixture's store");

    assert_eq!(store, main.join(".aida-store"));
    let lock = aida_core::db::store_write_lock_path(&store);
    let fixture_root = tmp.path().canonicalize().unwrap();
    assert!(lock.starts_with(&fixture_root), "{}", lock.display());
    assert!(!lock.starts_with(workspace()), "{}", lock.display());
}

/// With nothing pinned, the walk never starts in this workspace's checkout
/// (whose store, in a seat clone, is the live one).
#[test]
fn bug_2381_unpinned_store_discovery_never_starts_in_the_workspace_checkout() {
    let start = crate::store_sync::store_discovery_start().expect("a start dir");
    assert!(!start.starts_with(workspace()), "{}", start.display());
}

/// Review item 2: the hermetic checkout's `git init` reads no system or
/// global git config (no operator `init.templateDir` hooks or
/// `init.defaultBranch`), and its branch is `main`.
#[test]
fn bug_2381_hermetic_checkout_ignores_operator_git_config_and_is_on_main() {
    let init = hermetic_git_init(Path::new("/nonexistent"));
    let envs: Vec<_> = init
        .get_envs()
        .map(|(k, v)| (k.to_os_string(), v.map(|v| v.to_os_string())))
        .collect();
    for (key, value) in [
        ("GIT_CONFIG_GLOBAL", "/dev/null"),
        ("GIT_CONFIG_NOSYSTEM", "1"),
    ] {
        assert!(
            envs.contains(&(key.into(), Some(value.into()))),
            "{key}={value} missing: {envs:?}"
        );
    }
    let head = std::fs::read_to_string(hermetic_checkout().join(".git/HEAD")).unwrap();
    assert_eq!(head.trim(), "ref: refs/heads/main");
}

/// Review item 2: the hermetic checkout does not outlive its test process.
/// The child creates it and prints its path; once the child has exited the
/// path is gone.
#[cfg(unix)]
#[test]
fn bug_2381_hermetic_checkout_is_removed_when_the_test_process_exits() {
    const TAG: &str = "BUG-2381-HERMETIC-CHECKOUT=";
    let Some(output) = crate::test_env::run_path_isolated_test_output() else {
        let dir = hermetic_checkout();
        assert!(dir.join(".git").is_dir());
        println!("{TAG}{}", dir.display());
        return;
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let dir = stdout
        .lines()
        // libtest may print `test <name> ... ` on the same line first.
        .find_map(|line| line.split_once(TAG).map(|(_, dir)| dir.trim()))
        .unwrap_or_else(|| panic!("the child printed no checkout path:\n{stdout}"));
    assert!(
        !Path::new(dir).exists(),
        "the hermetic checkout outlived its test process: {dir}"
    );
}
