use super::*;

// trace:BUG-2501 | ai:codex
#[test]
fn pull_merges_concurrent_spec_edits_with_upstream_scalars() {
    let dir = tempfile::tempdir().unwrap();
    let hub = dir.path().join("hub.git");
    std::fs::create_dir_all(&hub).unwrap();
    git(&hub, &["init", "--bare"]).unwrap();

    let first = dir.path().join("first");
    home_fixture_init(&first).unwrap();
    configure_user(&first, "Test", "test@example.com").unwrap();
    git(&first, &["checkout", "-b", "aida-store"]).unwrap();
    use crate::models::{Comment, Requirement};
    let relative = "objects/BUG/000/BUG-test.yaml";
    std::fs::create_dir_all(first.join("objects/BUG/000")).unwrap();
    let seed = Requirement::new("seed".into(), "description".into());
    std::fs::write(first.join(relative), serde_yaml::to_string(&seed).unwrap()).unwrap();
    add(&first, &[relative]).unwrap();
    commit(&first, "seed").unwrap();
    git(&first, &["remote", "add", "origin", hub.to_str().unwrap()]).unwrap();
    assert!(push(&first, "origin", "aida-store").unwrap());
    git(&hub, &["symbolic-ref", "HEAD", "refs/heads/aida-store"]).unwrap();

    git(dir.path(), &["clone", hub.to_str().unwrap(), "second"]).unwrap();
    let second = dir.path().join("second");
    configure_user(&second, "Test", "test@example.com").unwrap();

    let mut remote = seed.clone();
    remote.title = "upstream".into();
    remote
        .comments
        .push(Comment::new("remote".into(), "remote comment".into()));
    std::fs::write(
        first.join(relative),
        serde_yaml::to_string(&remote).unwrap(),
    )
    .unwrap();
    add(&first, &[relative]).unwrap();
    commit(&first, "remote edit").unwrap();
    assert!(push(&first, "origin", "aida-store").unwrap());

    let mut local = seed.clone();
    local.title = "local".into();
    local.tags.insert("local-tag".into());
    let target = uuid::Uuid::new_v4();
    local.dependencies.push(target);
    local.relationships.push(crate::models::Relationship {
        rel_type: crate::models::RelationshipType::BlockedBy,
        target_id: target,
        created_at: Some(chrono::Utc::now()),
        created_by: Some("local".into()),
    });
    local.modified_at = remote.modified_at + chrono::Duration::days(1);
    local
        .comments
        .push(Comment::new("local".into(), "local comment".into()));
    std::fs::write(
        second.join(relative),
        serde_yaml::to_string(&local).unwrap(),
    )
    .unwrap();
    add(&second, &[relative]).unwrap();
    commit(&second, "local edit").unwrap();
    // A failing commit hook leaves a resolved rebase paused without conflicts.
    // The hook permits the third attempt so the old retry loop fails this
    // regression without hanging the test process.
    // trace:BUG-2501 | ai:codex
    let before = head_sha(&second).unwrap();
    let hook = second.join(".git/hooks/prepare-commit-msg");
    std::fs::write(&hook, "#!/bin/sh\nn=0\n[ ! -f .git/hook-count ] || n=$(cat .git/hook-count)\nn=$((n + 1))\necho $n > .git/hook-count\n[ $n -gt 2 ]\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let error = pull_rebase(&second, "origin", "aida-store").unwrap_err();
    assert!(error.to_string().contains("continue failed"), "{error:#}");
    assert!(!rebase_in_progress(&second));
    assert_eq!(head_sha(&second).unwrap(), before);
    assert_eq!(current_branch(&second).unwrap(), "aida-store");
    assert_eq!(
        std::fs::read_to_string(second.join(".git/hook-count"))
            .unwrap()
            .trim(),
        "1"
    );
    std::fs::remove_file(hook).unwrap();
    pull_rebase(&second, "origin", "aida-store").unwrap();
    assert!(!rebase_in_progress(&second));
    assert_eq!(current_branch(&second).unwrap(), "aida-store");
    let merged: Requirement =
        serde_yaml::from_str(&std::fs::read_to_string(second.join(relative)).unwrap()).unwrap();
    assert_eq!(merged.title, "upstream");
    assert_eq!(merged.tags, local.tags);
    assert_eq!(merged.relationships, local.relationships);
    assert_eq!(merged.dependencies, local.dependencies);
    assert_eq!(merged.comments.len(), 2);
    assert!(merged
        .comments
        .iter()
        .any(|c| c.id == remote.comments[0].id));
    assert!(merged.comments.iter().any(|c| c.id == local.comments[0].id));
    assert!(!has_changes_checked(&second).unwrap());
    // Untracked host metadata must not disable coordination refreshes.
    std::fs::create_dir_all(second.join(".aida")).unwrap();
    std::fs::write(second.join(".aida/node.toml"), "local metadata").unwrap();
    pull_rebase(&second, "origin", "aida-store").unwrap();
    pull_rebase_auto_merge(&second, "origin", "aida-store").unwrap();
    assert!(second.join(".aida/node.toml").exists());
    // Both pull entry points refuse staged and unstaged pending writes,
    // even when the operator enables Git autostashing.
    assert!(
        git(&second, &["config", "rebase.autoStash", "true"])
            .unwrap()
            .success
    );
    for staged in [false, true] {
        let before = head_sha(&second).unwrap();
        std::fs::write(second.join(relative), "pending content\n").unwrap();
        if staged {
            add(&second, &[relative]).unwrap();
        }
        let error = pull_rebase(&second, "origin", "aida-store").unwrap_err();
        assert!(
            error.to_string().contains("uncommitted changes"),
            "{error:#}"
        );
        let error = pull_rebase_auto_merge(&second, "origin", "aida-store").unwrap_err();
        assert!(
            error.to_string().contains("uncommitted changes"),
            "{error:#}"
        );
        assert_eq!(head_sha(&second).unwrap(), before);
        assert_eq!(
            std::fs::read_to_string(second.join(relative)).unwrap(),
            "pending content\n"
        );
        assert!(git(&second, &["stash", "list"]).unwrap().stdout.is_empty());
        assert!(!rebase_in_progress(&second));
        assert!(git(&second, &["reset", "--hard", "HEAD"]).unwrap().success);
    }
}
