//! BUG-HOME-042: shipping a stacked PR's parent moves its open children onto
//! the branch it merged into, re-runs their CI, and then deletes the merged
//! branch; `aida pr ship` refuses a PR whose base is not the default branch.
// trace:BUG-B6B1278890774EF993D2EBC327862470-042 | ai:claude

use crate::pr_ship::{
    retarget_stacked_children, stacked_base_refusal, StackRetargetPorts, StackRetargetReport,
};

/// A mock forge: records every call in order and fails the ones it is told to.
#[derive(Default)]
struct MockForge {
    calls: Vec<String>,
    retarget_fails: Vec<u64>,
    retrigger_fails: Vec<u64>,
    delete_fails: bool,
    said: Vec<String>,
}

impl StackRetargetPorts for MockForge {
    fn retarget(&mut self, pr: u64, base: &str) -> anyhow::Result<()> {
        self.calls.push(format!("retarget {pr} {base}"));
        if self.retarget_fails.contains(&pr) {
            anyhow::bail!("forge refused");
        }
        Ok(())
    }
    fn retrigger_ci(&mut self, pr: u64) -> anyhow::Result<()> {
        self.calls.push(format!("retrigger {pr}"));
        if self.retrigger_fails.contains(&pr) {
            anyhow::bail!("branch moved");
        }
        Ok(())
    }
    fn delete_remote_branch(&mut self, branch: &str) -> anyhow::Result<()> {
        self.calls.push(format!("delete {branch}"));
        if self.delete_fails {
            anyhow::bail!("protected");
        }
        Ok(())
    }
    fn say(&mut self, line: &str) {
        self.said.push(line.to_string());
    }
}

#[test]
fn children_are_retargeted_and_rerun_before_the_branch_is_deleted() {
    let mut forge = MockForge::default();
    let report =
        retarget_stacked_children(&mut forge, "task-106-work", "main", &[3097, 3103], true);
    assert_eq!(
        forge.calls,
        [
            "retarget 3097 main",
            "retrigger 3097",
            "retarget 3103 main",
            "retrigger 3103",
            "delete task-106-work",
        ]
    );
    assert_eq!(
        report,
        StackRetargetReport {
            retargeted: vec![3097, 3103],
            retarget_failed: vec![],
            ci_retriggered: vec![3097, 3103],
            ci_not_retriggered: vec![],
            branch_deleted: true,
        }
    );
    let said = forge.said.join("\n");
    assert!(
        said.contains("retargeted PR-3097 from `task-106-work` to `main`"),
        "{said}"
    );
    assert!(said.contains("re-triggered CI on PR-3103"), "{said}");
    assert!(
        said.contains("deleted merged branch `task-106-work`"),
        "{said}"
    );
}

#[test]
fn a_child_that_cannot_be_retargeted_keeps_the_branch() {
    let mut forge = MockForge {
        retarget_fails: vec![3097],
        ..MockForge::default()
    };
    let report =
        retarget_stacked_children(&mut forge, "task-106-work", "main", &[3097, 3103], true);
    assert_eq!(report.retarget_failed, vec![3097]);
    assert_eq!(report.retargeted, vec![3103]);
    assert!(!report.branch_deleted);
    assert!(
        !forge.calls.iter().any(|c| c.starts_with("delete")),
        "{:?}",
        forge.calls
    );
    assert!(!forge.calls.contains(&"retrigger 3097".to_string()));
    let said = forge.said.join("\n");
    assert!(said.contains("gh pr edit 3097 --base main"), "{said}");
    assert!(said.contains("keeping `task-106-work`"), "{said}");
}

#[test]
fn a_failed_ci_rerun_is_reported_and_does_not_keep_the_branch() {
    let mut forge = MockForge {
        retrigger_fails: vec![3103],
        ..MockForge::default()
    };
    let report = retarget_stacked_children(&mut forge, "task-106-work", "main", &[3103], true);
    assert_eq!(report.ci_not_retriggered, vec![3103]);
    assert!(report.branch_deleted);
    let said = forge.said.join("\n");
    assert!(
        said.contains("could not re-trigger CI on PR-3103"),
        "{said}"
    );
    assert!(said.contains("push a commit to its branch"), "{said}");
}

#[test]
fn a_branch_still_needed_locally_is_kept_after_the_retarget() {
    let mut forge = MockForge::default();
    let report = retarget_stacked_children(&mut forge, "task-106-work", "main", &[3097], false);
    assert_eq!(report.retargeted, vec![3097]);
    assert!(!report.branch_deleted);
    assert!(!forge.calls.iter().any(|c| c.starts_with("delete")));
}

#[test]
fn a_failed_delete_is_reported() {
    let mut forge = MockForge {
        delete_fails: true,
        ..MockForge::default()
    };
    let report = retarget_stacked_children(&mut forge, "task-106-work", "main", &[3097], true);
    assert!(!report.branch_deleted);
    assert!(forge
        .said
        .join("\n")
        .contains("could not delete merged branch"));
}

#[test]
fn ship_refuses_a_pr_based_on_another_branch() {
    let why = stacked_base_refusal(3097, "task-106-work", Some("main"), false).expect("refused");
    assert!(why.contains("PR-3097 targets `task-106-work`"), "{why}");
    assert!(why.contains("gh pr edit 3097 --base main"), "{why}");
    assert!(why.contains("--allow-stacked-base"), "{why}");
}

#[test]
fn ship_allows_the_default_base_the_override_and_an_unknown_default() {
    assert_eq!(stacked_base_refusal(1, "main", Some("main"), false), None);
    assert_eq!(
        stacked_base_refusal(1, "task-106-work", Some("main"), true),
        None
    );
    assert_eq!(stacked_base_refusal(1, "task-106-work", None, false), None);
    assert_eq!(stacked_base_refusal(1, "", Some("main"), false), None);
}

#[test]
fn github_argv_for_retarget_and_empty_commit() {
    assert_eq!(
        crate::forge::github_retarget_argv(3103, "main"),
        ["pr", "edit", "3103", "--base", "main"]
    );
    let commit = crate::forge::github_empty_commit_argv("tree1", "head1");
    assert!(commit.contains(&"repos/{owner}/{repo}/git/commits".to_string()));
    assert!(commit.contains(&"tree=tree1".to_string()));
    assert!(commit.contains(&"parents[]=head1".to_string()));
    let update = crate::forge::github_fast_forward_ref_argv("task-107-work", "new1");
    assert!(update.contains(&"repos/{owner}/{repo}/git/refs/heads/task-107-work".to_string()));
    assert!(
        update.contains(&"force=false".to_string()),
        "never force-moves the head"
    );
}

#[test]
fn ship_cli_parses_allow_stacked_base() {
    let cli = crate::command_registry::try_parse_from([
        "aida",
        "pr",
        "ship",
        "3097",
        "--allow-stacked-base",
    ])
    .expect("parses --allow-stacked-base");
    match cli.command.registered::<crate::pr_cmd::PrArgs>() {
        crate::pr_cmd::PrArgs {
            inner:
                crate::cli::PrCommand::Ship {
                    allow_stacked_base, ..
                },
        } => assert!(allow_stacked_base),
        _ => panic!("expected pr ship"),
    }
}
