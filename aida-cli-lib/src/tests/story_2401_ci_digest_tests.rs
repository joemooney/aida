//! Real-Git fixtures for workflow content identity and CI freshness.
// trace:STORY-2401 | ai:codex
use super::*;
use crate::gate::{ci_freshness::behind_base_refusal, ship::ShipRefusal};

fn git(repo: &Path, args: &[&str]) -> String {
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

fn commit(repo: &Path, path: &str, body: &str) {
    let path = repo.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
    git(repo, &["add", "."]);
    git(repo, &["commit", "-qm", "fixture"]);
}

fn fixture(forge: ForgeKind) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    git(tmp.path(), &["init", "-q", "--initial-branch=main"]);
    commit(
        tmp.path(),
        ".aida/config.toml",
        &format!("[forge]\nprovider = \"{}\"\n", forge.config_token()),
    );
    tmp
}

fn definition(forge: ForgeKind) -> &'static str {
    match forge {
        ForgeKind::GitLab => ".gitlab-ci.yml",
        _ => ".github/workflows/ci.yml",
    }
}

#[test]
fn touch_then_revert_keeps_digest_and_avoids_stale_definition_refusal() {
    for forge in [ForgeKind::GitHub, ForgeKind::GitLab] {
        let tmp = fixture(forge);
        let repo = tmp.path();
        let path = definition(forge);
        commit(repo, path, "on: pull_request\n# original\n");
        git(repo, &["branch", "pr"]);
        commit(repo, path, "on: pull_request\n# changed\n");
        commit(repo, path, "on: pull_request\n# original\n");
        let warning = crate::pr_stale_check_warning(repo, "pr", "main").unwrap();
        assert_eq!(warning.head_digest, warning.base_digest);
        assert!(warning.definition_files.is_empty());
        let refusal = behind_base_refusal(7, "main", Ok(Some(warning)), false).unwrap_err();
        assert_eq!(refusal.0, ShipRefusal::BehindBase);
        assert!(!refusal.1.contains("OLDER definition"));
    }
}

#[test]
fn real_workflow_edit_refuses_as_stale_definition() {
    for forge in [ForgeKind::GitHub, ForgeKind::GitLab] {
        let tmp = fixture(forge);
        let repo = tmp.path();
        let path = definition(forge);
        commit(repo, path, "on: pull_request\n# original\n");
        git(repo, &["branch", "pr"]);
        commit(repo, path, "on: pull_request\n# changed\n");
        let warning = crate::pr_stale_check(repo, "pr", "main").unwrap().unwrap();
        assert_ne!(warning.head_digest, warning.base_digest);
        assert_eq!(
            behind_base_refusal(7, "main", Ok(Some(warning)), false)
                .unwrap_err()
                .0,
            ShipRefusal::StaleDefinition
        );
    }
}

#[test]
fn matching_endpoint_definitions_override_merge_base_path_history() {
    let tmp = fixture(ForgeKind::GitHub);
    let repo = tmp.path();
    let path = definition(ForgeKind::GitHub);
    commit(repo, path, "on: pull_request\n# original\n");
    git(repo, &["checkout", "-qb", "pr"]);
    // Distinct parent/tree avoids identical fixture commits within one second.
    commit(repo, "feature.txt", "branch-only source\n");
    commit(repo, path, "on: pull_request\n# same edit\n");
    git(repo, &["checkout", "-q", "main"]);
    commit(repo, path, "on: pull_request\n# same edit\n");
    let warning = crate::pr_stale_check(repo, "pr", "main").unwrap().unwrap();
    assert_eq!(warning.head_digest, warning.base_digest);
    assert!(warning.definition_files.is_empty());
}

#[test]
fn ci_evidence_key_binds_head_and_content_and_ignores_uncommitted_edits() {
    let tmp = fixture(ForgeKind::GitHub);
    let repo = tmp.path();
    let path = definition(ForgeKind::GitHub);
    commit(repo, path, "on: pull_request\n");
    let first = CiEvidenceKey::read(repo, "HEAD", ForgeKind::GitHub).unwrap();
    std::fs::write(repo.join(path), "dirty workflow\n").unwrap();
    assert_eq!(
        first,
        CiEvidenceKey::read(repo, "HEAD", ForgeKind::GitHub).unwrap()
    );
    git(repo, &["checkout", "--", path]);
    commit(repo, "README.md", "ordinary source\n");
    let second = CiEvidenceKey::read(repo, "HEAD", ForgeKind::GitHub).unwrap();
    assert_eq!(first.workflow_digest, second.workflow_digest);
    assert_ne!(first, second);
    commit(repo, path, "on: pull_request\n# edit\n");
    assert_ne!(
        second.workflow_digest,
        CiEvidenceKey::read(repo, "HEAD", ForgeKind::GitHub)
            .unwrap()
            .workflow_digest
    );
}

#[test]
fn absent_definitions_are_known_but_bad_refs_are_errors() {
    for forge in [ForgeKind::GitHub, ForgeKind::GitLab] {
        let tmp = fixture(forge);
        let repo = tmp.path();
        let absent = CiEvidenceKey::read(repo, "HEAD", forge).unwrap();
        assert!(absent.workflow_digest.ends_with(":absent"));
        assert!(CiEvidenceKey::read(repo, "missing-ref", forge).is_err());
        assert!(crate::pr_stale_check(repo, "missing-ref", "main").is_err());
        commit(repo, definition(forge), "on: pull_request\n");
        let present = CiEvidenceKey::read(repo, "HEAD", forge).unwrap();
        assert_ne!(present.workflow_digest, absent.workflow_digest);
        git(repo, &["rm", definition(forge)]);
        git(repo, &["commit", "-qm", "remove definition"]);
        assert_eq!(
            CiEvidenceKey::read(repo, "HEAD", forge)
                .unwrap()
                .workflow_digest,
            absent.workflow_digest
        );
    }
}

#[test]
fn non_workflow_script_edits_do_not_invalidate_the_workflow_digest() {
    let tmp = fixture(ForgeKind::GitHub);
    let repo = tmp.path();
    commit(
        repo,
        definition(ForgeKind::GitHub),
        "on: pull_request\n# scripts/check.sh\n",
    );
    commit(repo, "scripts/check.sh", "echo original\n");
    git(repo, &["branch", "pr"]);
    commit(repo, "scripts/check.sh", "echo changed\n");
    let warning = crate::pr_stale_check(repo, "pr", "main").unwrap().unwrap();
    assert_eq!(warning.head_digest, warning.base_digest);
    assert!(warning.definition_files.is_empty());
}

#[test]
fn invalid_definition_object_types_are_not_empty_definitions() {
    for forge in [ForgeKind::GitHub, ForgeKind::GitLab] {
        let tmp = fixture(forge);
        let repo = tmp.path();
        let path = match forge {
            ForgeKind::GitLab => ".gitlab-ci.yml",
            _ => ".github/workflows",
        };
        std::fs::create_dir_all(repo.join(path).parent().unwrap()).unwrap();
        std::os::unix::fs::symlink("README.md", repo.join(path)).unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-qm", "linked definition"]);
        assert!(CiEvidenceKey::read(repo, "HEAD", forge).is_err());
    }
}

#[test]
fn stale_definition_classification_uses_digests_even_without_path_hints() {
    let warning = |head: &str, base: &str, paths: Vec<String>| crate::StaleCheckWarning {
        behind_commits: 1,
        head_digest: head.into(),
        base_digest: base.into(),
        definition_files: paths,
        test_files: vec![],
    };
    assert_eq!(
        behind_base_refusal(7, "main", Ok(Some(warning("a", "b", vec![]))), false)
            .unwrap_err()
            .0,
        ShipRefusal::StaleDefinition
    );
    assert_eq!(
        behind_base_refusal(
            7,
            "main",
            Ok(Some(warning("a", "a", vec!["historical-path".into()]))),
            false
        )
        .unwrap_err()
        .0,
        ShipRefusal::BehindBase
    );
}
