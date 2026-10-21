//! BUG-2201: a contained session in a project other than aida gets the same
//! `aida`/git Bash rules and store reads as one in the aida repo.
//!
//! Contained launches pass `--setting-sources project`, so the operator's
//! user-level allows never apply, and a downstream project's
//! `.claude/settings.json` never receives the template's `permissions` block
//! (it is not a managed-merge slot). In aida-relay that left a headless
//! reviewer unable to run `AIDA_AGENT_TYPE=claude aida review record …` or read
//! `<main>/.aida-store`, so it never recorded a verdict. The fixture here is a
//! project shaped like aida-relay: a main checkout whose own settings carry no
//! `permissions`, and a review worktree in a sibling `<repo>-worktrees/`.
//!
//! trace:BUG-2201 | ai:claude

use super::evaluator::allowed;
use super::*;
use std::path::{Path, PathBuf};

/// `<tmp>/relay` (main checkout with a store and a hooks-only
/// `.claude/settings.json`) and `<tmp>/relay-worktrees/relay-pr-96`, a linked
/// worktree of it.
fn outside_project_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(tmp.path()).unwrap();
    let main = base.join("relay");
    std::fs::create_dir_all(main.join(".aida-store/objects/TASK/000")).unwrap();
    std::fs::create_dir_all(main.join(".aida/review-verdicts")).unwrap();
    std::fs::create_dir_all(main.join(".claude")).unwrap();
    std::fs::write(
        main.join(".claude/settings.json"),
        r#"{"hooks":{"PreToolUse":[]}}"#,
    )
    .unwrap();
    std::fs::write(main.join(".aida/cache.db"), b"").unwrap();
    let gitdir = main.join(".git/worktrees/relay-pr-96");
    std::fs::create_dir_all(&gitdir).unwrap();
    std::fs::write(gitdir.join("commondir"), "../..\n").unwrap();
    let worktree = base.join("relay-worktrees/relay-pr-96");
    std::fs::create_dir_all(worktree.join("bin")).unwrap();
    std::fs::write(
        worktree.join(".git"),
        format!("gitdir: {}\n", gitdir.display()),
    )
    .unwrap();
    (tmp, main, worktree)
}

/// The settings a contained launch in `worktree` gets: the same composition
/// as `claude_contained_settings_json`, without its store side effects.
fn launch_settings(worktree: &Path, main: &Path) -> serde_json::Value {
    let json = crate::session::contained_settings_json_with_boundary(
        &[],
        false,
        &contained_write_roots(worktree),
        &contained_store_write_paths(main),
        &crate::session::main_git_deny_paths(main),
    );
    serde_json::from_str(&json).unwrap()
}

fn rules(settings: &serde_json::Value, key: &str) -> Vec<String> {
    settings["permissions"][key]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r.as_str().map(str::to_string))
        .collect()
}

#[test]
fn outside_project_reviewer_gets_aida_and_git_bash_rules() {
    let (_tmp, main, worktree) = outside_project_fixture();
    let own: serde_json::Value =
        serde_json::from_slice(&std::fs::read(main.join(".claude/settings.json")).unwrap())
            .unwrap();
    assert!(
        own.get("permissions").is_none(),
        "the fixture project grants nothing itself"
    );
    let allow = rules(&launch_settings(&worktree, &main), "allow");
    for rule in ["Bash(aida)", "Bash(aida *)", "Bash(git)", "Bash(git *)"] {
        assert!(allow.contains(&rule.to_string()), "{rule}: {allow:?}");
    }
}

#[test]
fn outside_project_reviewer_reads_its_worktree_and_the_main_store() {
    let (_tmp, main, worktree) = outside_project_fixture();
    let settings = launch_settings(&worktree, &main);
    let elsewhere = Path::new("/unrelated");
    for path in [
        worktree.join("bin/relay.sh"),
        main.join(".aida-store/objects/TASK/000/TASK-119.yaml"),
        main.join(".aida/cache.db"),
    ] {
        assert!(
            allowed(&settings, "Read", &path, elsewhere, elsewhere),
            "{} must be readable: {settings}",
            path.display()
        );
    }
    // Reads stop at the store: the rest of the main checkout is not granted.
    assert!(!allowed(
        &settings,
        "Read",
        &main.join("src/lib.rs"),
        elsewhere,
        elsewhere
    ));
}

#[test]
fn baseline_rules_never_widen_the_write_boundary() {
    let (_tmp, main, worktree) = outside_project_fixture();
    let settings = launch_settings(&worktree, &main);
    let elsewhere = Path::new("/unrelated");
    for rule in contained_baseline_allow_rules_for_test(&worktree, &main) {
        assert!(
            !WRITE_TOOLS
                .iter()
                .any(|tool| rule.starts_with(&format!("{tool}("))),
            "a baseline rule is never a write rule: {rule}"
        );
    }
    assert!(allowed(
        &settings,
        "Write",
        &worktree.join(".aida/review-verdicts/PR-96.json"),
        elsewhere,
        elsewhere
    ));
    for outside in [
        main.join(".aida/review-verdicts/PR-96.json"),
        main.join(".aida-store/objects/TASK/000/TASK-119.yaml"),
    ] {
        assert!(
            !allowed(&settings, "Write", &outside, elsewhere, elsewhere),
            "{} stays outside the tool write boundary",
            outside.display()
        );
    }
    // The destructive git denies still win over `Bash(git *)`.
    assert!(rules(&settings, "deny").contains(&"Bash(git reset --hard *)".to_string()));
}

#[test]
fn fail_closed_launch_grants_no_baseline_rules() {
    let json = crate::session::contained_settings_json(&[], false, &[]);
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["permissions"]["allow"], serde_json::json!([]));
}

fn contained_baseline_allow_rules_for_test(worktree: &Path, main: &Path) -> Vec<String> {
    crate::session::contained_baseline_allow_rules(
        &contained_write_roots(worktree),
        &contained_store_write_paths(main),
    )
}
