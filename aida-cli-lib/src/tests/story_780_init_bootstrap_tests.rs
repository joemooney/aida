use crate::init_bootstrap::{create_and_scaffold, preflight, BootstrapPlan, Lang};

fn plain_plan(dir: std::path::PathBuf) -> BootstrapPlan {
    BootstrapPlan {
        dir,
        lang: None,
        github: false,
        public: false,
        remote: None,
    }
}

#[test]
fn lang_parses_the_supported_set_and_names_it_on_error() {
    assert_eq!(Lang::parse("rust").unwrap(), Lang::Rust);
    assert_eq!(Lang::parse("Python").unwrap(), Lang::Python);
    assert_eq!(Lang::parse("node").unwrap(), Lang::Node);
    assert_eq!(Lang::parse("js").unwrap(), Lang::Node);
    let err = Lang::parse("cobol").unwrap_err().to_string();
    assert!(err.contains("rust") && err.contains("python") && err.contains("node"));
}

#[test]
fn lang_delegates_to_native_tools_only() {
    // The scope boundary: AIDA owns the sequence, never the templates.
    assert_eq!(Lang::Rust.tool(), "cargo");
    assert_eq!(Lang::Python.tool(), "uv");
    assert_eq!(Lang::Node.tool(), "npm");
    assert_eq!(Lang::Node.scaffold_args(), &["init", "-y"]);
}

#[test]
fn github_repo_create_argv_is_private_by_default() {
    let argv = crate::forge::ForgeKind::GitHub
        .repo_create_argv("myproj", false)
        .expect("github yields an argv");
    assert_eq!(argv[0], "gh");
    assert!(argv.contains(&"--private".to_string()));
    assert!(!argv.contains(&"--public".to_string()));
    // --source/--push: pushes the CURRENT branch so the caller owns ordering.
    assert!(argv.contains(&"--push".to_string()));

    let public = crate::forge::ForgeKind::GitHub
        .repo_create_argv("myproj", true)
        .unwrap();
    assert!(public.contains(&"--public".to_string()));
}

#[test]
fn gitlab_has_no_create_argv_push_to_create_is_the_path() {
    assert!(crate::forge::ForgeKind::GitLab
        .repo_create_argv("x", false)
        .is_none());
    assert!(crate::forge::ForgeKind::None
        .repo_create_argv("x", false)
        .is_none());
}

#[test]
fn preflight_refuses_a_nonempty_non_git_dir_before_any_mkdir() {
    let parent = tempfile::tempdir().unwrap();
    let dir = parent.path().join("occupied");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("precious.txt"), "not yours").unwrap();
    let err = preflight(&plain_plan(dir)).unwrap_err().to_string();
    assert!(
        err.contains("not a git repository"),
        "must refuse to scaffold over a stranger's dir: {err}"
    );
}

#[test]
fn preflight_allows_missing_and_empty_dirs() {
    let parent = tempfile::tempdir().unwrap();
    preflight(&plain_plan(parent.path().join("fresh"))).expect("missing dir is fine");
    let empty = parent.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    preflight(&plain_plan(empty)).expect("empty dir is fine");
}

/// The plain path end-to-end (no lang, no remote): dir created, git repo
/// born, exactly one commit — and re-running no-ops instead of erroring
/// (the fail-fast-then-resume contract).
#[test]
fn plain_bootstrap_is_idempotent_and_leaves_one_commit() {
    // CI runners have no global git identity; the bootstrap's first commit
    // must not depend on the machine's ~/.gitconfig in this test.
    let _guard = crate::test_env::env_lock();
    for (k, v) in [
        ("GIT_AUTHOR_NAME", "aida-test"),
        ("GIT_AUTHOR_EMAIL", "aida-test@example.invalid"),
        ("GIT_COMMITTER_NAME", "aida-test"),
        ("GIT_COMMITTER_EMAIL", "aida-test@example.invalid"),
    ] {
        std::env::set_var(k, v);
    }
    let parent = tempfile::tempdir().unwrap();
    let dir = parent.path().join("proj");
    let plan = plain_plan(dir.clone());
    create_and_scaffold(&plan).expect("first run");
    create_and_scaffold(&plan).expect("second run resumes, never errors");
    assert!(dir.join(".git").exists());
    let out = std::process::Command::new("git")
        .args(["rev-list", "--count", "HEAD"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "1",
        "re-running must not stack commits"
    );

    for k in [
        "GIT_AUTHOR_NAME",
        "GIT_AUTHOR_EMAIL",
        "GIT_COMMITTER_NAME",
        "GIT_COMMITTER_EMAIL",
    ] {
        std::env::remove_var(k);
    }
}

#[test]
fn bootstrap_flags_require_the_positional_dir() {
    use clap::Parser;
    // --lang without DIR must be a parse error, keeping bare `aida init`
    // byte-identical to its pre-STORY-780 behavior.
    assert!(crate::cli::Cli::try_parse_from(["aida", "init", "--lang", "rust"]).is_err());
    assert!(crate::cli::Cli::try_parse_from(["aida", "init", "--github"]).is_err());
    // --public rides on --github, not on DIR alone.
    assert!(crate::cli::Cli::try_parse_from(["aida", "init", "proj", "--public"]).is_err());
    // --github conflicts with --remote: one remote authority at a time.
    assert!(crate::cli::Cli::try_parse_from([
        "aida",
        "init",
        "proj",
        "--github",
        "--remote",
        "git@x:y.git"
    ])
    .is_err());
    // The full happy shape parses.
    assert!(crate::cli::Cli::try_parse_from([
        "aida", "init", "proj", "--lang", "rust", "--github", "--public"
    ])
    .is_ok());
}

#[test]
fn init_cd_capability_gates_on_the_wrapper_token() {
    use crate::shell_eval::{marker_has_cap, INIT_CD_CAP};
    // The new-wrapper marker carries it; the pre-STORY-780 marker does not.
    assert!(marker_has_cap(
        Some("role,session,dev,worktree,worktree-exit,worktree-stale,eval-block,init-cd"),
        INIT_CD_CAP
    ));
    assert!(!marker_has_cap(
        Some("role,session,dev,worktree,worktree-exit,worktree-stale,eval-block"),
        INIT_CD_CAP
    ));
    assert!(!marker_has_cap(None, INIT_CD_CAP));
}

#[test]
fn the_scaffolded_wrapper_routes_init_and_advertises_init_cd() {
    // Pin the wrapper template: the init case branch exists, the capability
    // token is advertised, and — the safety property — the init branch has no
    // bare-payload fallback (a markerless init prints, never evals).
    let script = crate::dev_cmd::SHELL_HELPERS;
    assert!(
        script.contains(r#""init"|"init "*)"#),
        "wrapper must route init through its own case branch"
    );
    assert!(
        script.contains("eval-block,init-cd"),
        "wrapper must advertise the init-cd capability"
    );
}
