// trace:TASK-3428 | ai:codex

/// Does the project's working tree have uncommitted changes? Conservative: any
/// `git status --porcelain` output (excluding the gitignored store/cache noise
/// git already omits) counts as dirty. trace:STORY-659
pub(crate) fn working_tree_is_dirty(project_root: &std::path::Path) -> bool {
    // F4: a refused or failed status (a seat clone refuses host git) is
    // dirty, never "no uncommitted work". trace:BUG-1985 | ai:claude
    aida_core::git_ops::host_git_command(project_root)
        .args(["status", "--porcelain"])
        .output()
        .map(|o| !o.status.success() || !o.stdout.is_empty())
        .unwrap_or(true)
}
