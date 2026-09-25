//! BUG-1610: before `aida review` offers to open a change from a branch
//! with no open PR/MR, it must verify the intended base exists on the
//! remote and differs from the source — and an explicit `--target-branch`
//! must always win over whatever a previous offer saved. trace:BUG-1610 | ai:claude

use super::*;

fn git(repo: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("git {:?} failed to spawn: {e}", args));
    assert!(
        out.status.success(),
        "git {:?} failed\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Build the exact repro shape from BUG-1610: an empty GitLab project
/// pushed to with ONLY the feature branch — `main` never existed on
/// `origin`, so (in real GitLab) the feature branch became the project
/// default. Returns the working-copy repo with `origin` pointed at a real
/// bare remote carrying just `story-52-work`.
fn empty_project_first_push_is_feature_branch() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let origin = tmp.path().join("origin.git");
    let work = tmp.path().join("work");
    std::fs::create_dir_all(&origin).unwrap();
    git(&origin, &["init", "-q", "--bare"]);
    std::fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "-q", "-b", "story-52-work"]);
    git(&work, &["config", "user.email", "t@example.com"]);
    git(&work, &["config", "user.name", "Test"]);
    std::fs::write(work.join("a.txt"), "hello\n").unwrap();
    git(&work, &["add", "a.txt"]);
    git(&work, &["commit", "-q", "-m", "first commit"]);
    git(
        &work,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&work, &["push", "-q", "-u", "origin", "story-52-work"]);
    tmp
}

// --- preflight_mr_base ------------------------------------------------

#[test]
fn preflight_mr_base_source_equals_base_short_circuits_without_network() {
    // No `origin` remote configured at all — if this needed to talk to the
    // network it would fail with a spawn/lookup error, not this outcome.
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    git(repo, &["init", "-q"]);
    assert_eq!(
        preflight_mr_base(repo, "story-52-work", "story-52-work"),
        MrBasePreflight::SourceEqualsBase
    );
}

#[test]
fn preflight_mr_base_missing_remote_base_when_first_pushed_ref_is_feature_branch() {
    // trace:BUG-1610 | ai:claude — the exact repro: an empty GitLab project
    // whose first pushed ref is the feature branch, `main` never pushed.
    let project = empty_project_first_push_is_feature_branch();
    let work = project.path().join("work");
    assert_eq!(
        preflight_mr_base(&work, "story-52-work", "main"),
        MrBasePreflight::MissingRemoteBase
    );
}

#[test]
fn preflight_mr_base_ok_when_base_exists_on_remote_and_differs() {
    let tmp = tempfile::tempdir().unwrap();
    let origin = tmp.path().join("origin.git");
    let work = tmp.path().join("work");
    std::fs::create_dir_all(&origin).unwrap();
    git(&origin, &["init", "-q", "--bare"]);
    std::fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "-q", "-b", "main"]);
    git(&work, &["config", "user.email", "t@example.com"]);
    git(&work, &["config", "user.name", "Test"]);
    std::fs::write(work.join("a.txt"), "hello\n").unwrap();
    git(&work, &["add", "a.txt"]);
    git(&work, &["commit", "-q", "-m", "first commit"]);
    git(
        &work,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&work, &["push", "-q", "-u", "origin", "main"]);
    git(&work, &["checkout", "-q", "-b", "story-52-work"]);
    git(&work, &["push", "-q", "-u", "origin", "story-52-work"]);

    assert_eq!(
        preflight_mr_base(&work, "story-52-work", "main"),
        MrBasePreflight::Ok
    );
}

// --- mr_base_diagnosis_message -----------------------------------------

#[test]
fn diagnosis_message_names_both_branches_and_gives_gitlab_recovery_sequence() {
    let msg = mr_base_diagnosis_message(
        crate::forge::ForgeKind::GitLab,
        MrBasePreflight::MissingRemoteBase,
        "story-52-work",
        "main",
    );
    assert!(msg.contains("story-52-work"), "names the source: {msg}");
    assert!(msg.contains("main"), "names the base: {msg}");
    assert!(
        msg.contains("git push -u origin main"),
        "must give the push recovery step: {msg}"
    );
    assert!(
        msg.contains("glab repo update --defaultBranch main"),
        "must give the provider-correct default-branch fix: {msg}"
    );
    assert!(
        msg.contains("glab mr create --source-branch story-52-work --target-branch main"),
        "must give a fresh, explicit-target MR create: {msg}"
    );
}

#[test]
fn diagnosis_message_source_equals_base_explains_self_merge() {
    let msg = mr_base_diagnosis_message(
        crate::forge::ForgeKind::GitLab,
        MrBasePreflight::SourceEqualsBase,
        "story-52-work",
        "story-52-work",
    );
    assert!(msg.contains("story-52-work"));
    assert!(msg.contains("cannot merge into itself"));
}

#[test]
fn diagnosis_message_ok_outcome_is_empty() {
    assert_eq!(
        mr_base_diagnosis_message(
            crate::forge::ForgeKind::GitLab,
            MrBasePreflight::Ok,
            "story-52-work",
            "main"
        ),
        ""
    );
}

// --- MrRecoveryState read/write -----------------------------------------

#[test]
fn write_mr_recovery_state_refuses_source_equals_target() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("STORY-52.json");
    let state = MrRecoveryState {
        spec: "STORY-52".to_string(),
        source_branch: "story-52-work".to_string(),
        target_branch: "story-52-work".to_string(),
    };
    let result = write_mr_recovery_state(&path, &state);
    assert!(result.is_err(), "must refuse to save source == target");
    assert!(!path.exists(), "must not write the file at all");
}

#[test]
fn write_then_read_mr_recovery_state_round_trips() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("nested").join("STORY-52.json");
    let state = MrRecoveryState {
        spec: "STORY-52".to_string(),
        source_branch: "story-52-work".to_string(),
        target_branch: "main".to_string(),
    };
    write_mr_recovery_state(&path, &state).expect("valid source != target must save");
    let read_back = read_mr_recovery_state(&path).expect("just-written state reads back");
    assert_eq!(read_back, state);
}

#[test]
fn read_mr_recovery_state_is_none_when_absent() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("does-not-exist.json");
    assert!(read_mr_recovery_state(&path).is_none());
}

// --- resolve_mr_target_branch: explicit flag always wins -----------------

#[test]
fn resolve_mr_target_branch_explicit_flag_overrides_saved_state() {
    // trace:BUG-1610 | ai:claude — the exact BUG-1610 failure: a saved
    // `story-52-work -> story-52-work` recovery pair must NOT survive a
    // corrected `--target-branch main` retry.
    let saved = MrRecoveryState {
        spec: "STORY-52".to_string(),
        source_branch: "story-52-work".to_string(),
        target_branch: "story-52-work".to_string(),
    };
    let resolved = resolve_mr_target_branch(Some("main"), Some(&saved), "main");
    assert_eq!(resolved, "main");
}

#[test]
fn resolve_mr_target_branch_falls_back_to_saved_when_no_explicit_flag() {
    let saved = MrRecoveryState {
        spec: "STORY-52".to_string(),
        source_branch: "story-52-work".to_string(),
        target_branch: "release".to_string(),
    };
    let resolved = resolve_mr_target_branch(None, Some(&saved), "main");
    assert_eq!(resolved, "release");
}

#[test]
fn resolve_mr_target_branch_falls_back_to_default_when_nothing_saved() {
    let resolved = resolve_mr_target_branch(None, None, "main");
    assert_eq!(resolved, "main");
}

// An origin that cannot be reached is not reported as "base missing": the
// preflight fails closed with its own outcome and a retry message instead of
// a misleading recovery recipe.
// trace:BUG-1610 | ai:claude
#[test]
fn unreachable_origin_is_distinguished_from_missing_base() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        let ok = std::process::Command::new("git")
            .current_dir(&repo)
            .args(args)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?} failed");
    };
    git(&["init", "-q"]);
    // A path that does not exist: git cannot reach it and exits 128, not 2.
    let missing = tmp.path().join("no-such-origin.git");
    git(&["remote", "add", "origin", missing.to_str().unwrap()]);
    let outcome = crate::pr_cmd::preflight_mr_base(&repo, "feature", "main");
    assert_eq!(outcome, crate::pr_cmd::MrBasePreflight::OriginUnreachable);
    let msg = crate::pr_cmd::mr_base_diagnosis_message(
        crate::forge::ForgeKind::GitLab,
        outcome,
        "feature",
        "main",
    );
    assert!(msg.contains("could not reach"), "{msg}");
    assert!(!msg.contains("glab repo update"), "{msg}");
}
