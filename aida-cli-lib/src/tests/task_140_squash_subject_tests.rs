use super::*;

fn git(root: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

/// TASK-140: the squash subject must come from the PR BRANCH's head, not the
/// local cwd HEAD. Here cwd HEAD is `main` (an unrelated subject) while the
/// PR branch carries the real one — the function must return the branch's.
#[test]
fn branch_head_commit_message_reads_branch_not_cwd_head() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["config", "user.email", "t@t.t"]);
    git(root, &["config", "user.name", "t"]);
    std::fs::write(root.join("a"), "1").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "unrelated main-tip subject"]);
    // Feature branch with the REAL subject.
    git(root, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(root.join("b"), "2").unwrap();
    git(root, &["add", "."]);
    git(
        root,
        &["commit", "-q", "-m", "the real PR subject (TASK-575)"],
    );
    // Back on main: cwd HEAD is now the unrelated tip — the bug condition.
    git(root, &["checkout", "-q", "main"]);

    let msg = branch_head_commit_message(root, "feature").expect("branch head readable");
    assert!(
        msg.contains("the real PR subject"),
        "must read the branch's commit, got: {msg}"
    );
    assert!(
        !msg.contains("unrelated main-tip"),
        "must NOT read cwd HEAD (main), got: {msg}"
    );
    // Unknown branch (no local, no origin) → None.
    assert!(branch_head_commit_message(root, "no-such-branch").is_none());
}

/// BUG-1957: `derive_pr_ship_squash_subject` returns the completion trailer
/// it derived (ids + provenance), store-qualified: a branch-name id completes
/// a plain spec, and is withheld (with a note) when the spec is closure-held.
// trace:BUG-1957 | ai:claude
#[test]
fn bug_1957_pr_ship_squash_subject_returns_completion_trailer() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["config", "user.email", "t@t.t"]);
    git(root, &["config", "user.name", "t"]);
    std::fs::write(root.join("a"), "1").unwrap();
    git(root, &["add", "a"]);
    git(root, &["commit", "-q", "-m", "wip"]);
    git(root, &["checkout", "-q", "-b", "task-1538-x"]);

    let bin = tempfile::tempdir().unwrap();
    crate::test_exec::write_executable(
        &bin.path().join("gh"),
        "#!/bin/sh\necho '{\"title\":\"fix: prose\",\"body\":\"\"}'\n",
    );
    let _path = crate::test_env::EnvVarGuard::prepend_path(bin.path());

    let save = |tags: &[&str]| {
        let mut req = aida_core::Requirement::new("t".to_string(), String::new());
        req.spec_id = Some("TASK-1538".to_string());
        req.req_type = aida_core::RequirementType::Task;
        req.status = aida_core::RequirementStatus::InProgress;
        for tag in tags {
            req.tags.insert(tag.to_string());
        }
        let store = aida_core::RequirementsStore {
            requirements: vec![req],
            ..Default::default()
        };
        let backend = aida_core::GitBackend::new(&root.join(".aida-store")).unwrap();
        aida_core::DatabaseBackend::save(&backend, &store).unwrap();
    };

    save(&[]);
    let (subject, trailer) =
        derive_pr_ship_squash_subject(root, 1957, "task-1538-x", false).unwrap();
    assert_eq!(trailer.ids, vec!["TASK-1538".to_string()]);
    assert_eq!(trailer.source, pr_ship::TrailerSource::Branch);
    assert_eq!(subject.as_deref(), Some("fix: prose (TASK-1538)"));

    save(&[aida_core::pickability::CLOSURE_PENDING_TAG]);
    let (subject, trailer) =
        derive_pr_ship_squash_subject(root, 1957, "task-1538-x", false).unwrap();
    assert!(trailer.ids.is_empty(), "{trailer:?}");
    assert_eq!(trailer.source, pr_ship::TrailerSource::None);
    assert!(trailer.notes[0].contains("TASK-1538"), "{trailer:?}");
    // The PR title differs from the head commit subject, so a subject is
    // always produced here; the withheld id must not reach it.
    assert_eq!(subject.as_deref(), Some("fix: prose"));

    let (_, trailer) = derive_pr_ship_squash_subject(root, 1957, "task-1538-x", true).unwrap();
    assert!(trailer.ids.is_empty(), "{trailer:?}");

    // The dry-run output carries the provenance line after the plan.
    save(&[]);
    let out = pr_ship_dry_run_output("PLAN\n".to_string(), root, 1957, "task-1538-x", false);
    assert_eq!(
        out,
        "PLAN\n  · completion trailer (TASK-1538) — from the branch name\n"
    );
    let out = pr_ship_dry_run_output("PLAN\n".to_string(), root, 0, "task-1538-x", false);
    assert!(
        out.starts_with("PLAN\n  · completion trailer: not resolved — "),
        "{out}"
    );
    assert!(
        out.contains("PR title or branch name at merge time"),
        "{out}"
    );
}

/// BUG-1997: `derive_pr_ship_squash_subject` never returns None when spec IDs
/// are withheld, even when the normalized subject equals the branch-head commit
/// subject. If it returned None, `gh pr merge --squash` would fall back to its
/// default (the PR title on multi-commit PRs), resurrecting the withheld ID.
// trace:BUG-1997 | ai:antigravity
#[test]
fn bug_1997_pr_ship_squash_subject_never_none_when_ids_withheld() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["config", "user.email", "t@t.t"]);
    git(root, &["config", "user.name", "t"]);
    std::fs::write(root.join("a"), "1").unwrap();
    git(root, &["add", "a"]);
    // Head commit has subject matching normalized clean title "fix: prose"
    git(root, &["commit", "-q", "-m", "fix: prose"]);
    git(root, &["checkout", "-q", "-b", "fix-branch"]);

    let bin = tempfile::tempdir().unwrap();
    crate::test_exec::write_executable(
        &bin.path().join("gh"),
        "#!/bin/sh\necho '{\"title\":\"fix: prose (TASK-1538)\",\"body\":\"\"}'\n",
    );
    let _path = crate::test_env::EnvVarGuard::prepend_path(bin.path());

    let mut req = aida_core::Requirement::new("t".to_string(), String::new());
    req.spec_id = Some("TASK-1538".to_string());
    req.req_type = aida_core::RequirementType::Task;
    req.status = aida_core::RequirementStatus::InProgress;
    let store = aida_core::RequirementsStore {
        requirements: vec![req],
        ..Default::default()
    };
    let backend = aida_core::GitBackend::new(&root.join(".aida-store")).unwrap();
    aida_core::DatabaseBackend::save(&backend, &store).unwrap();

    // With no_complete = true: normalized is "fix: prose", matching head commit "fix: prose".
    // It must return Some("fix: prose"), NOT None.
    let (subject, trailer) = derive_pr_ship_squash_subject(root, 1997, "fix-branch", true).unwrap();
    assert!(trailer.ids.is_empty(), "{trailer:?}");
    assert_eq!(
        subject.as_deref(),
        Some("fix: prose"),
        "must pass explicit subject when ids are withheld to prevent gh PR-title fallback"
    );

    drop(_path);

    // Also test PR-body non-completing marker with no_complete = false:
    let bin2 = tempfile::tempdir().unwrap();
    crate::test_exec::write_executable(
        &bin2.path().join("gh"),
        "#!/bin/sh\necho '{\"title\":\"fix: prose (TASK-1538)\",\"body\":\"aida: no-complete\"}'\n",
    );
    let _path2 = crate::test_env::EnvVarGuard::prepend_path(bin2.path());
    let (subject2, trailer2) =
        derive_pr_ship_squash_subject(root, 1997, "fix-branch", false).unwrap();
    assert!(trailer2.ids.is_empty(), "{trailer2:?}");
    assert_eq!(
        subject2.as_deref(),
        Some("fix: prose"),
        "must pass explicit subject when body withholds completion"
    );
}
