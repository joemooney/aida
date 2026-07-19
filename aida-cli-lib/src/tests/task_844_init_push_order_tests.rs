use super::*;
use crate::forge::{project_path_of, ForgeKind};

/// Empty origin + main has commits → push main FIRST so the forge adopts
/// main (not the orphan store) as default.
#[test]
fn pushes_code_branch_first_when_main_has_commits() {
    let plan = decide_init_push_plan(Some("main"), true);
    assert_eq!(plan, InitPushPlan::CodeBranchFirst("main".to_string()));
}

/// master is honored the same as main (the code branch name flows through).
#[test]
fn pushes_code_branch_first_honors_master() {
    let plan = decide_init_push_plan(Some("master"), true);
    assert_eq!(plan, InitPushPlan::CodeBranchFirst("master".to_string()));
}

/// Brand-new repo: code branch has no commits → fall back to orphan-only
/// (do NOT push uncommitted code — blast-radius guardrail).
#[test]
fn falls_back_to_orphan_only_when_no_commits() {
    let plan = decide_init_push_plan(Some("main"), false);
    assert_eq!(plan, InitPushPlan::OrphanOnly);
}

/// No code branch at all → orphan-only fallback.
#[test]
fn falls_back_to_orphan_only_when_no_code_branch() {
    let plan = decide_init_push_plan(None, true);
    assert_eq!(plan, InitPushPlan::OrphanOnly);
}

/// GitHub set-default-branch command shape: `gh repo edit [proj]
/// --default-branch main`. Project ref is optional (gh resolves from cwd).
#[test]
fn github_set_default_branch_cmd_with_and_without_project() {
    let with = ForgeKind::GitHub.set_default_branch_cmd("main", Some("owner/repo"));
    assert_eq!(
        with,
        Some(vec![
            "gh".into(),
            "repo".into(),
            "edit".into(),
            "owner/repo".into(),
            "--default-branch".into(),
            "main".into(),
        ])
    );
    let without = ForgeKind::GitHub.set_default_branch_cmd("main", None);
    assert_eq!(
        without,
        Some(vec![
            "gh".into(),
            "repo".into(),
            "edit".into(),
            "--default-branch".into(),
            "main".into(),
        ])
    );
}

/// GitLab set-default-branch command shape: `glab api -X PUT
/// projects/<url-encoded-path> -f default_branch=main`. The project path is
/// required and `/`-encoded to `%2F`.
#[test]
fn gitlab_set_default_branch_cmd_encodes_project_path() {
    let cmd = ForgeKind::GitLab.set_default_branch_cmd("main", Some("group/sub/proj"));
    assert_eq!(
        cmd,
        Some(vec![
            "glab".into(),
            "api".into(),
            "-X".into(),
            "PUT".into(),
            "projects/group%2Fsub%2Fproj".into(),
            "-f".into(),
            "default_branch=main".into(),
        ])
    );
}

/// GitLab requires a project ref — without one there's no command to build.
#[test]
fn gitlab_set_default_branch_cmd_none_without_project() {
    assert_eq!(ForgeKind::GitLab.set_default_branch_cmd("main", None), None);
}

/// Pure-git has no forge API → no command at all.
#[test]
fn pure_git_has_no_set_default_branch_cmd() {
    assert_eq!(
        ForgeKind::None.set_default_branch_cmd("main", Some("owner/repo")),
        None
    );
}

/// project_path_of parses both SSH and HTTPS origins and strips `.git`.
#[test]
fn project_path_extraction() {
    assert_eq!(
        project_path_of("git@github.com:owner/repo.git").as_deref(),
        Some("owner/repo")
    );
    assert_eq!(
        project_path_of("https://gitlab.example.com/group/sub/proj.git").as_deref(),
        Some("group/sub/proj")
    );
    assert_eq!(
        project_path_of("git@gitlab.joemooney.com:grp/proj").as_deref(),
        Some("grp/proj")
    );
    assert_eq!(project_path_of(""), None);
}
