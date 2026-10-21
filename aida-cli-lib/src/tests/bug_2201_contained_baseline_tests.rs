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
    for rule in ["Bash(aida)", "Bash(aida *)"] {
        assert!(allow.contains(&rule.to_string()), "{rule}: {allow:?}");
    }
    // TASK-2058: no git rule at all; plain reads run under sandbox auto-allow.
    assert!(
        !allow.iter().any(|r| r.starts_with("Bash(git")),
        "{allow:?}"
    );
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
    // The destructive git denies are still in place.
    assert!(rules(&settings, "deny").contains(&"Bash(git reset --hard *)".to_string()));
}

/// Whether a Claude Code `Bash(...)` rule matches `command`: `Bash(p *)`
/// matches `p` followed by a space and anything; `Bash(p)` matches `p` only.
fn bash_rule_matches(rule: &str, command: &str) -> bool {
    let Some(spec) = rule.strip_prefix("Bash(").and_then(|r| r.strip_suffix(')')) else {
        return false;
    };
    match spec.strip_suffix(" *") {
        Some(prefix) => command
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with(' ')),
        None => command == spec,
    }
}

// TASK-2058: no baseline rule allows a git command that writes a ref or a
// remote or runs another program, in any spelling: a refspec force, the
// lease/includes forces, global options before the subcommand, mirror and
// delete pushes, a bare prefix, or the exec/write options of read-only
// subcommands (`grep -O`, `fetch --upload-pack`, `diff --output`, `-c k=v`).
// The baseline carries no git rule at all, so every git form here, read-only
// ones included, is left to the sandbox's own auto-allow.
#[test]
fn baseline_allows_no_ref_or_remote_writing_git() {
    let (_tmp, main, worktree) = outside_project_fixture();
    let allow = rules(&launch_settings(&worktree, &main), "allow");
    for command in [
        "git push origin main",
        "git push origin +main",
        "git push origin +refs/heads/main:refs/heads/main",
        "git push --force origin main",
        "git push -f origin main",
        "git push --force-with-lease origin main",
        "git push --force-if-includes origin main",
        "git push --mirror origin",
        "git push --delete origin main",
        "git push origin :main",
        "git -c core.hooksPath=/x push --force",
        "git -C ../other push -f",
        "git reset --hard HEAD~1",
        "git update-ref refs/heads/main HEAD",
        "git branch -D main",
        "git tag -d v1",
        "git commit -m x",
        "git remote set-url origin x",
        "git config core.hooksPath /x",
        "git worktree remove --force /x",
        // Option-borne exec and writes in otherwise read-only subcommands.
        "git grep -Ogit foo",
        "git grep -O\"git push --force origin main\" foo",
        "git grep --open-files-in-pager=sh foo",
        "git fetch . +abc123:refs/heads/main",
        "git fetch origin +refs/heads/*:refs/heads/*",
        "git fetch --upload-pack=sh origin",
        "git fetch --prune origin",
        "git fetch origin",
        "git diff --output=/x/evil HEAD",
        "git diff HEAD --output=/x/evil",
        "git diff --ext-diff HEAD",
        "git log --output=/x/evil",
        "git show --textconv HEAD:file",
        "git -c diff.external=sh diff",
        "git -c core.pager=sh log",
        "git -c core.sshCommand=sh fetch origin",
        "git -c core.fsmonitor=sh status",
        "git status",
        "git diff main...HEAD",
    ] {
        let hits: Vec<&String> = allow
            .iter()
            .filter(|rule| bash_rule_matches(rule, command))
            .collect();
        assert!(hits.is_empty(), "`{command}` is allowed by {hits:?}");
    }
    // Mutation control: the matcher does match what the baseline grants, and
    // would catch a broad git rule.
    assert!(bash_rule_matches("Bash(git *)", "git grep -Ogit foo"));
    assert!(bash_rule_matches(
        "Bash(git fetch *)",
        "git fetch --prune origin"
    ));
    for command in ["aida review record X", "aida show TASK-1"] {
        assert!(
            allow.iter().any(|rule| bash_rule_matches(rule, command)),
            "`{command}` must be allowed: {allow:?}"
        );
    }
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
