//! Test fixtures spawn git through [`git_command`], which turns off automatic
//! maintenance. Since git 2.54 every commit, merge or fetch starts a detached
//! geometric repack that can prune an `.git/objects/<xx>` dir while the
//! fixture's next git call writes into it ("unable to create temporary file").
//! `tests/test_git_maintenance_ratchet.py` keeps new fixtures on this helper.
// trace:BUG-2191 | ai:claude

/// `git` with `-c maintenance.auto=false -c gc.auto=0` already applied; add
/// the subcommand and its arguments as usual.
pub(crate) fn git_command() -> std::process::Command {
    let mut command = std::process::Command::new("git");
    command.args(aida_core::git_ops::NO_AUTO_MAINTENANCE);
    command
}

#[test]
fn git_command_turns_off_auto_maintenance() {
    let command = git_command();
    assert_eq!(command.get_program(), "git");
    let args: Vec<_> = command.get_args().collect();
    assert_eq!(args, aida_core::git_ops::NO_AUTO_MAINTENANCE);
}
