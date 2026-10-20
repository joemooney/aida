//! Test fixtures spawn git through [`git_command`], which turns off automatic
//! maintenance. Since git 2.54 every commit, merge or fetch starts a detached
//! geometric repack that can prune an `.git/objects/<xx>` dir while the
//! fixture's next git call writes into it ("unable to create temporary file").
//! `tests/test_git_maintenance_ratchet.py` keeps new fixtures on this helper.
// trace:BUG-2191 | ai:claude

/// `git` with `-c maintenance.auto=false -c gc.auto=0` already applied; add
/// the subcommand and its arguments as usual.
///
/// The `-c` pair does not reach the far side of a push: local transport
/// clears `GIT_CONFIG_PARAMETERS`, and a bare remote's `receive-pack` runs
/// auto-maintenance under its own config. So the command also sets
/// `init.templateDir` to [`template_dir`], whose `config` turns maintenance
/// off; every repository this helper initializes or clones (bare remotes
/// included) carries it.
pub(crate) fn git_command() -> std::process::Command {
    let mut command = std::process::Command::new("git");
    command.args(aida_core::git_ops::NO_AUTO_MAINTENANCE);
    command
        .arg("-c")
        .arg(format!("init.templateDir={}", template_dir().display()));
    command
}

/// A git template directory: the default layout (`hooks/`, `info/exclude`,
/// `description`) plus a `config` with automatic maintenance off. Created
/// once per test process.
fn template_dir() -> &'static std::path::Path {
    static DIR: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let dir = tempfile::Builder::new()
            .prefix("aida-test-git-template-")
            .tempdir()
            .expect("create git template dir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("hooks")).unwrap();
        std::fs::create_dir_all(root.join("info")).unwrap();
        std::fs::write(root.join("info/exclude"), "").unwrap();
        std::fs::write(root.join("description"), "test fixture\n").unwrap();
        std::fs::write(
            root.join("config"),
            "[receive]\n\tautogc = false\n[gc]\n\tauto = 0\n[maintenance]\n\tauto = false\n",
        )
        .unwrap();
        dir
    })
    .path()
}

// trace:BUG-2191.aca71745 | ai:claude
#[test]
fn git_command_turns_off_auto_maintenance() {
    let command = git_command();
    assert_eq!(command.get_program(), "git");
    let args: Vec<_> = command.get_args().collect();
    assert_eq!(args[..4], aida_core::git_ops::NO_AUTO_MAINTENANCE);
    // A bare remote made through the helper keeps maintenance off for the
    // receive-pack side of a push, which the -c pair cannot reach.
    let tmp = tempfile::tempdir().unwrap();
    let remote = tmp.path().join("remote.git");
    assert!(git_command()
        .args(["init", "-q", "--bare"])
        .arg(&remote)
        .status()
        .unwrap()
        .success());
    for (key, value) in [
        ("receive.autogc", "false"),
        ("gc.auto", "0"),
        ("maintenance.auto", "false"),
    ] {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&remote)
            .args(["config", "--get", key])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), value, "{key}");
    }
    assert!(remote.join("info/exclude").is_file());
}
