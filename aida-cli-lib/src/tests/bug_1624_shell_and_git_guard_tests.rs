//! BUG-1624: the guard tests the BUG-1622 review asked for, plus the
//! shell-quoting of values that reach an eval'd or pasted shell line.
//!
//! Each test runs against scratch repos; marker files that an injected
//! command would create live inside those repos, and nothing touches
//! `~/.aida` or a live store.
// trace:BUG-1624 | ai:claude

use super::*;

fn git(root: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["-c", "user.email=t@example.com", "-c", "user.name=Test"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A scratch repo on `main` with one commit, a `feat` branch with a second
/// one, and `origin` pointing at itself.
fn scratch_repo() -> tempfile::TempDir {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q", "-b", "main"]);
    std::fs::write(root.join("a.txt"), "a").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "one"]);
    git(root, &["checkout", "-q", "-b", "feat"]);
    std::fs::write(root.join("b.txt"), "b").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "two"]);
    git(root, &["checkout", "-q", "main"]);
    git(root, &["remote", "add", "origin", "."]);
    tmp
}

/// `--upload-pack` / `--exec` payloads that create `pwned` in the repo if
/// git ever reads them as options.
const UPLOAD_PACK: &str = "--upload-pack=touch pwned;false";
const REBASE_EXEC: &str = "--exec=touch pwned";

fn assert_not_pwned(root: &std::path::Path, label: &str) {
    assert!(
        !root.join("pwned").exists(),
        "{label}: git ran an injected command"
    );
}

#[test]
fn bug_1624_stacks_json_cascade_refuses_a_malformed_record() {
    let tmp = scratch_repo();
    let root = tmp.path();
    let head = git(root, &["rev-parse", "main"]);
    let feat_before = git(root, &["rev-parse", "feat"]);
    // Two hand-edited records whose parent has vanished (so the cascade
    // picks them up): an option-like parent sha, and an option-like branch.
    let graph = serde_json::json!({
        "entries": {
            "feat": {
                "branch": "feat",
                "parent_branch": "gone-parent",
                "parent_branch_sha": REBASE_EXEC,
                "created_at": "2026-01-01T00:00:00Z"
            },
            REBASE_EXEC: {
                "branch": REBASE_EXEC,
                "parent_branch": "gone-parent",
                "parent_branch_sha": head,
                "created_at": "2026-01-02T00:00:00Z"
            }
        }
    });
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        crate::stacks::path(root),
        serde_json::to_string_pretty(&graph).unwrap(),
    )
    .unwrap();
    // A lease for each branch so the cascade reaches the rebase step.
    let dir = leases_dir(root);
    std::fs::create_dir_all(&dir).unwrap();
    for (id, branch) in [("stk001", "feat"), ("stk002", REBASE_EXEC)] {
        let lease = SessionLease {
            id: id.into(),
            scope: "BUG-1".into(),
            slug: "bug-1".into(),
            owner: "tester".into(),
            worktree_path: root.to_path_buf(),
            branch: branch.into(),
            started_at: chrono::Utc::now(),
            hostname: "h".into(),
            role: None,
            creator_pid: None,
            creator_pid_start_time: None,
            active_pid: None,
            active_pid_start_time: None,
            cargo_target_dir: None,
            parent_project_root: None,
            pr_head_sha: None,
            pr_base_sha: None,
            pr_base_ref: None,
            zen_intent_token: None,
            escalated_to_human: None,
            parent_branch: None,
            parent_branch_sha: None,
            review_verb: false,
            claim_verb: false,
            manual_enter_at: None,
        };
        std::fs::write(
            dir.join(format!("{id}.toml")),
            toml::to_string_pretty(&lease).unwrap(),
        )
        .unwrap();
    }

    cascade_rebase_stacked_branches(root, true).unwrap();

    assert_not_pwned(root, "stack cascade");
    assert_eq!(git(root, &["rev-parse", "feat"]), feat_before);
    assert_eq!(git(root, &["rev-parse", "--abbrev-ref", "HEAD"]), "main");
    // Both malformed records stay in the graph for a human to inspect.
    assert_eq!(crate::stacks::load(root).entries.len(), 2);
}

#[test]
fn bug_1624_prepare_graded_review_refuses_option_like_inputs() {
    use aida_core::db::DatabaseBackend;

    let tmp = scratch_repo();
    let root = tmp.path();
    // A git-canonical store in the scratch repo holding one spec with an
    // executable verification check, so the refusal path is reached.
    let store_dir = root.join(".aida-store");
    std::fs::create_dir_all(&store_dir).unwrap();
    git(&store_dir, &["init", "-q", "-b", "aida-store"]);
    git(&store_dir, &["config", "user.email", "t@example.com"]);
    git(&store_dir, &["config", "user.name", "Test"]);
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        "store_path = \".aida-store\"\n",
    )
    .unwrap();
    let backend = aida_core::GitBackend::new(&store_dir)
        .unwrap()
        .with_dispenser(load_dispenser(&store_dir).unwrap());
    let mut req = aida_core::Requirement::new(
        "graded fixture".to_string(),
        "## Verify\n\n```\ntouch graded-check-ran\n```\n".to_string(),
    );
    req.req_type = aida_core::RequirementType::Task;
    let spec = backend.add_requirement(req).unwrap().spec_id.unwrap();
    let head = git(root, &["rev-parse", "main"]);

    // A reviewed head that is not a commit ID.
    let reason = match prepare_graded_review(root, &spec, 1, "--output=pwned", None) {
        Err(failure) => failure.reason,
        Ok(_) => panic!("an option-like reviewed head must be refused"),
    };
    assert!(reason.contains("not a commit ID"), "{reason}");

    // A forge-reported branch that would be a fetch option.
    let reason = match prepare_graded_review(root, &spec, 1, &head, Some(UPLOAD_PACK)) {
        Err(failure) => failure.reason,
        Ok(_) => panic!("an option-like branch must be refused"),
    };
    assert!(reason.contains("starts with `-`"), "{reason}");

    assert_not_pwned(root, "graded review");
    assert!(!root.join("pwned").exists() && !root.join("graded-check-ran").exists());
}

#[test]
fn bug_1624_queue_recover_push_refuses_an_option_like_branch() {
    let tmp = scratch_repo();
    let root = tmp.path();
    let err = crate::queue_cmd::recover_push_branch(root, UPLOAD_PACK)
        .unwrap_err()
        .to_string();
    assert!(err.contains("starts with `-`"), "{err}");
    let err = crate::queue_cmd::recover_push_branch(root, "  --receive-pack=touch pwned")
        .unwrap_err()
        .to_string();
    assert!(err.contains("starts with `-`"), "{err}");
    assert_not_pwned(root, "queue recover push");
}

#[test]
fn bug_1624_pure_git_forge_refuses_option_like_branch_and_base() {
    use crate::forge::Forge;

    let tmp = scratch_repo();
    let root = tmp.path();
    let main_before = git(root, &["rev-parse", "main"]);
    let forge = crate::forge::PureGitForge::new(root);
    let change = |branch: &str, base: &str| crate::forge::ChangeRef {
        id: 1,
        url: String::new(),
        branch: branch.to_string(),
        base: base.to_string(),
        title: None,
    };
    for (branch, base) in [
        ("--orphan=pwned", "main"),
        ("feat", "--orphan=pwned"),
        (REBASE_EXEC, "main"),
    ] {
        let err = forge
            .merge_change(
                &change(branch, base),
                &crate::forge::MergeOptions::squash(),
                &mut crate::network_retry::NoopSink,
            )
            .map(|_| ())
            .unwrap_err()
            .to_string();
        assert!(err.contains("starts with `-`"), "{branch}/{base}: {err}");
    }
    let err = forge
        .checkout_change(&change("--orphan=pwned", "main"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("starts with `-`"), "{err}");

    assert_not_pwned(root, "pure-git forge");
    assert_eq!(git(root, &["rev-parse", "--abbrev-ref", "HEAD"]), "main");
    assert_eq!(git(root, &["rev-parse", "main"]), main_before);
    // Ordinary names still check out.
    forge.checkout_change(&change("feat", "main")).unwrap();
    assert_eq!(git(root, &["rev-parse", "--abbrev-ref", "HEAD"]), "feat");
}

#[test]
fn bug_1624_doctor_since_with_whitespace_is_trimmed_then_guarded() {
    let tmp = scratch_repo();
    let root = tmp.path();
    let now = chrono::Utc::now();
    // Whitespace-only is empty.
    for blank in ["   ", "\t", " \n "] {
        let err = resolve_completed_since_cutoff_at(root, blank, now, &chrono::Utc)
            .unwrap_err()
            .to_string();
        assert!(err.contains("--since") && err.contains("empty"), "{err}");
    }
    // Leading whitespace does not smuggle a dash-led value past the guard.
    for padded in [" --output=pwned", "\t--output=pwned  ", "  -o  "] {
        let err = resolve_completed_since_cutoff_at(root, padded, now, &chrono::Utc)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("--since") && err.contains("starts with `-`"),
            "{padded:?}: {err}"
        );
    }
    assert_not_pwned(root, "doctor --since");
    // A padded ref still resolves.
    assert!(resolve_completed_since_cutoff_at(root, "  feat  ", now, &chrono::Utc).is_ok());
}

/// A `.aida/session-env.sh` a branch committed, carrying everything the
/// strict review showed surviving the first filter.
const HOSTILE_SESSION_ENV: &str = "touch pwned\n\
export PROMPT_COMMAND='touch pwned'\n\
export BASH_ENV='./evil.sh'\n\
export LD_PRELOAD='./x.so'\n\
export PATH='.evil:/usr/bin'\n\
export AIDA_BIN='.evil/aida'\n\
export CARGO_TARGET_DIR='rel/target'\n\
export AIDA_AGENT_TYPE='cl'\\''aude $(touch pwned)'\n\
export BAD-NAME='x'\n";

/// A stand-in for the running binary: an absolute, existing file.
fn fake_running_exe(dir: &std::path::Path) -> std::path::PathBuf {
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let exe = bin.join("aida");
    std::fs::write(&exe, "").unwrap();
    exe
}

/// Only CARGO_TARGET_DIR (absolute), AIDA_AGENT_TYPE and AIDA_BIN survive,
/// and AIDA_BIN plus the PATH prepend come from the running binary, never
/// from the file.
#[test]
fn bug_1624_session_env_allowlists_names_and_ignores_file_aida_bin() {
    let tree = tempfile::TempDir::new().unwrap();
    let exe = fake_running_exe(tree.path());
    let lines = session_env_eval_lines(HOSTILE_SESSION_ENV, &exe);
    for banned in [
        "PROMPT_COMMAND",
        "BASH_ENV",
        "LD_PRELOAD",
        "'.evil",
        ".evil/aida",
        "CARGO_TARGET_DIR",
        "BAD-NAME",
        "\ntouch pwned",
    ] {
        assert!(!lines.contains(banned), "{banned} leaked into:\n{lines}");
    }
    assert!(!lines.starts_with("touch"), "{lines}");
    let bin_dir = exe.parent().unwrap().display().to_string();
    assert_eq!(
        lines,
        format!(
            "export AIDA_BIN='{}'\nPATH='{bin_dir}':\"$PATH\"\nexport AIDA_AGENT_TYPE='cl'\\''aude $(touch pwned)'\n",
            exe.display()
        ),
        "order follows the file; got:\n{lines}"
    );

    // An absolute CARGO_TARGET_DIR is kept; a relative running binary never
    // yields an AIDA_BIN or a PATH entry.
    let ok = session_env_eval_lines(
        "export CARGO_TARGET_DIR='/w/target'\nexport AIDA_BIN='/w/bin/aida'\n",
        std::path::Path::new("rel/aida"),
    );
    assert_eq!(ok, "export CARGO_TARGET_DIR='/w/target'\n");

    // The worktree-enter payload uses the same filter.
    std::fs::create_dir_all(tree.path().join(".aida")).unwrap();
    std::fs::write(
        tree.path().join(".aida/session-env.sh"),
        HOSTILE_SESSION_ENV,
    )
    .unwrap();
    let payload = enter_shell_payload(tree.path(), "BUG-1624", None);
    for banned in [
        "PROMPT_COMMAND",
        "LD_PRELOAD",
        "BASH_ENV",
        "'.evil",
        "\ntouch pwned\n",
    ] {
        assert!(
            !payload.contains(banned),
            "{banned} leaked into:\n{payload}"
        );
    }
    assert!(!payload.contains("export PATH"), "{payload}");

    // `worktree exit` only unsets allowlisted names.
    let unset = session_env_unset_names(tree.path());
    assert!(
        unset
            .iter()
            .all(|n| n == "CARGO_TARGET_DIR" || n == "AIDA_AGENT_TYPE" || n == "AIDA_BIN"),
        "{unset:?}"
    );
    assert!(!unset.iter().any(|n| n == "PATH"));
}

/// `session start --launch` / `queue work` apply the session env to the
/// process before exec: the same allowlist holds there.
#[test]
fn bug_1624_apply_session_env_to_process_ignores_non_allowlisted_names() {
    const VAR: &str = "AIDA_TEST_BUG_1624_NOT_ALLOWLISTED";
    #[allow(unused_unsafe)]
    unsafe {
        std::env::remove_var(VAR);
    }
    let before_path = std::env::var_os("PATH");
    let before_preload = std::env::var_os("LD_PRELOAD");
    let applied = apply_session_env_to_process(&format!(
        "export {VAR}='x'\nexport PATH='.evil:/usr/bin'\nexport LD_PRELOAD='./x.so'\n\
         export PROMPT_COMMAND='touch pwned'\n"
    ));
    assert!(applied.is_empty(), "{applied:?}");
    assert!(std::env::var_os(VAR).is_none());
    assert_eq!(std::env::var_os("PATH"), before_path);
    assert_eq!(std::env::var_os("LD_PRELOAD"), before_preload);
}

/// A hostile recorded cwd stays a single `cd` argument in the paste-ready
/// resume line.
#[test]
fn bug_1624_resume_hint_quotes_the_recorded_cwd() {
    let base = "aida queue work BUG-1624 --resume x";
    let cmd = resume_command_with_cwd(base, Some("/w/a;touch pwned"), Some("/w/b"));
    assert_eq!(cmd, format!("cd '/w/a;touch pwned' && {base}"));
}

/// The filtered session env, run through a real shell, executes nothing.
#[cfg(unix)]
#[test]
fn bug_1624_session_env_eval_lines_run_nothing_in_a_shell() {
    let tree = tempfile::TempDir::new().unwrap();
    let exe = fake_running_exe(tree.path());
    let lines = session_env_eval_lines(HOSTILE_SESSION_ENV, &exe);
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("{lines}printf '%s' \"$AIDA_AGENT_TYPE\""))
        .current_dir(tree.path())
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "cl'aude $(touch pwned)"
    );
    assert!(!tree.path().join("pwned").exists());
}
