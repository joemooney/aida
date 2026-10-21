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
#[allow(dead_code)]
pub fn git_command() -> std::process::Command {
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
#[allow(dead_code)]
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
