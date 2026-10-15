use crate::init_bootstrap::{create_and_scaffold, preflight, BootstrapPlan, Lang};

fn plain_plan(dir: std::path::PathBuf) -> BootstrapPlan {
    BootstrapPlan {
        dir,
        lang: None,
        github: false,
        public: false,
        remote: None,
        forge: None,
        github_repo: None,
        gitlab: None,
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
fn forge_profile_github_uses_existing_repo_create_shape() {
    let profiles = crate::forge_profiles::parse_profiles(
        r#"
        [[profile]]
        name = "home-github"
        kind = "github"
        namespace = "joe"
        visibility = "public"
        "#,
    )
    .unwrap();
    let profile = profiles.require("home-github").unwrap();
    let argv = crate::forge::ForgeKind::GitHub
        .repo_create_argv(&profile.github_repo_name("myproj"), profile.is_public())
        .unwrap();

    assert_eq!(argv[0], "gh");
    assert_eq!(argv[1], "repo");
    assert_eq!(argv[2], "create");
    assert_eq!(argv[3], "joe/myproj");
    assert!(argv.contains(&"--public".to_string()));
    assert!(argv.contains(&"--source".to_string()));
    assert!(argv.contains(&"--push".to_string()));
}

// trace:TASK-1748 | ai:antigravity
#[test]
fn headless_bootstrap_uses_default_forge_profile() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".aida")).unwrap();
    std::fs::write(
        home.path().join(".aida/forges.toml"),
        r#"
        [[profile]]
        name = "home-gitlab"
        kind = "gitlab"
        host = "gitlab.example.test"
        namespace = "joe"
        protocol = "https"

        [defaults]
        profile = "home-gitlab"
        "#,
    )
    .unwrap();
    // trace:BUG-1021 | ai:claude — AIDA_TEST_HOME: `$HOME` is not honored on Windows.
    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("HOME", home.path().to_str().unwrap()),
        ("AIDA_TEST_HOME", home.path().to_str().unwrap()),
        ("AIDA_HEADLESS", "1"),
    ]);

    let mut plan = plain_plan(std::path::PathBuf::from("myproj"));
    crate::init_bootstrap::resolve_forge_profile(&mut plan).unwrap();

    assert_eq!(
        plan.remote.as_deref(),
        Some("https://gitlab.example.test/joe/myproj.git")
    );
}

// trace:TASK-1748 | ai:antigravity
#[test]
fn explicit_unknown_forge_lists_configured_profiles() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".aida")).unwrap();
    std::fs::write(
        home.path().join(".aida/forges.toml"),
        r#"
        [[profile]]
        name = "home-gitlab"
        kind = "gitlab"
        host = "gitlab.example.test"
        namespace = "joe"
        "#,
    )
    .unwrap();
    // trace:BUG-1021 | ai:claude — AIDA_TEST_HOME: `$HOME` is not honored on Windows.
    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("HOME", home.path().to_str().unwrap()),
        ("AIDA_TEST_HOME", home.path().to_str().unwrap()),
    ]);

    let mut plan = plain_plan(std::path::PathBuf::from("myproj"));
    plan.forge = Some("work-gitlab".to_string());
    let err = crate::init_bootstrap::resolve_forge_profile(&mut plan)
        .unwrap_err()
        .to_string();

    assert!(err.contains("unknown --forge `work-gitlab`"));
    assert!(err.contains("home-gitlab"));
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
// trace:TASK-1532 | ai:agy
#[test]
fn plain_bootstrap_is_idempotent_and_leaves_one_commit() {
    // CI runners have no global git identity; the bootstrap's first commit
    // must not depend on the machine's ~/.gitconfig in this test.
    let _guard = crate::test_env::EnvVarsGuard::set(&[
        ("GIT_AUTHOR_NAME", "aida-test"),
        ("GIT_AUTHOR_EMAIL", "aida-test@example.invalid"),
        ("GIT_COMMITTER_NAME", "aida-test"),
        ("GIT_COMMITTER_EMAIL", "aida-test@example.invalid"),
    ]);
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
    assert!(crate::cli::Cli::try_parse_from([
        "aida",
        "init",
        "proj",
        "--remote",
        "git@x:y.git",
        "--public"
    ])
    .is_err());
    // --github conflicts with --forge too.
    assert!(crate::cli::Cli::try_parse_from([
        "aida", "init", "proj", "--github", "--forge", "home"
    ])
    .is_err());
    // The full happy shape parses.
    assert!(crate::cli::Cli::try_parse_from([
        "aida", "init", "proj", "--lang", "rust", "--github", "--public"
    ])
    .is_ok());
    assert!(crate::cli::Cli::try_parse_from([
        "aida",
        "init",
        "proj",
        "--forge",
        "home-gitlab",
        "--public"
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
    // Pin the wrapper template: the init case branch exists and the capability
    // the binary negotiates on is advertised. The channel is the side-channel
    // FILE (`init-cd-file`), never the stdout-capturing `init-cd` token: the
    // two are mutually exclusive, because a binary that sees `init-cd` prints
    // markers to a stdout this wrapper no longer reads.
    // trace:BUG-0445 | ai:claude
    use crate::shell_eval::{marker_has_cap, INIT_CD_CAP, INIT_CD_FILE_CAP, INIT_CD_FILE_ENV};
    let script = crate::dev_cmd::SHELL_HELPERS;
    assert!(
        script.contains(r#""init"|"init "*)"#),
        "wrapper must route init through its own case branch"
    );
    let advertised = script
        .lines()
        .find_map(|l| l.strip_prefix("export AIDA_SHELL_WRAPPER="))
        .expect("the wrapper exports its capability list")
        .trim_matches('\'');
    assert!(
        marker_has_cap(Some(advertised), INIT_CD_FILE_CAP),
        "wrapper must advertise the init-cd-file capability: {advertised}"
    );
    assert!(
        !marker_has_cap(Some(advertised), INIT_CD_CAP),
        "wrapper must NOT advertise stdout-capturing init-cd alongside the file channel: {advertised}"
    );
    assert!(
        script.contains(&format!(
            "{INIT_CD_FILE_ENV}=\"$_aida_cd_file\" command aida \"$@\""
        )),
        "the init arm must hand the binary the side-channel file by the env var the binary reads"
    );
}

/// The regression itself: the init arm must run the binary with stdout
/// ATTACHED. A `$(command aida ...)` capture is what made the child see a pipe
/// (auto-selecting the agent format) and buffered its prompts invisibly.
// trace:BUG-0445 | ai:claude
#[test]
fn the_init_arm_never_captures_the_childs_stdout() {
    let script = crate::dev_cmd::SHELL_HELPERS;
    let start = script
        .find(r#""init"|"init "*)"#)
        .expect("init arm present");
    let end = script[start..]
        .find(r#""tui"|"tui "*)"#)
        .map(|i| start + i)
        .expect("tui arm follows init");
    let init_arm = &script[start..end];
    assert!(
        !init_arm.contains("$(command aida"),
        "init arm must not wrap the binary in a command substitution:\n{init_arm}"
    );
    assert!(
        !init_arm.contains("_aida_out"),
        "init arm must not hold the child's stdout in a variable:\n{init_arm}"
    );
    assert!(
        init_arm.contains("rm -f \"$_aida_cd_file\""),
        "init arm must clean up its side-channel file:\n{init_arm}"
    );
    // trace:BUG-1960 | ai:claude
    // An interrupt must not strand the file: the handlers remove it on
    // INT/TERM and record the signal, scoped to the call (zsh: a helper under
    // localtraps; bash: save and restore), then the signal is re-raised at
    // this shell process. `$$` would target the top-level shell from inside a
    // subshell, so the re-raise must not use it.
    assert!(
        init_arm.contains("trap 'rm -f \"$_aida_cd_file\"; _aida_sig=INT' INT")
            && init_arm.contains("trap 'rm -f \"$_aida_cd_file\"; _aida_sig=TERM' TERM")
            && init_arm
                .contains("TRAPINT() { rm -f \"$_aida_cd_file\"; _aida_sig=INT; return 0; }")
            && init_arm
                .contains("TRAPTERM() { rm -f \"$_aida_cd_file\"; _aida_sig=TERM; return 0; }"),
        "init arm must remove its side-channel file on INT/TERM in bash and zsh:\n{init_arm}"
    );
    assert!(
        init_arm.contains("_aida_init_zsh() {")
            && init_arm.contains("setopt localoptions localtraps")
            && init_arm.contains("_aida_init_zsh \"$@\"")
            // trace:TASK-1344 | ai:claude
            && init_arm.contains("unfunction _aida_init_zsh 2>/dev/null")
            && init_arm.contains("_aida_traps=$(trap -p INT TERM)")
            && init_arm.contains("eval \"$_aida_traps\"")
            && init_arm.contains(
                "command sh -c 'kill -s \"$1\" \"$PPID\"; kill -s \"$1\" \"$$\"' aida \"$_aida_sig\""
            ),
        "the init arm's trap must be scoped to the call and the caller's traps restored:\n{init_arm}"
    );
    assert!(
        !init_arm.contains("kill -s \"$_aida_sig\" $$"),
        "the re-raise must target this shell process, not the top-level `$$`:\n{init_arm}"
    );
}

/// Pure channel choice on the binary's side of the negotiation.
// trace:BUG-0445 | ai:claude
#[test]
fn enter_dir_channel_prefers_the_side_channel_file() {
    use crate::init_bootstrap::{enter_dir_channel, EnterDirChannel};
    use std::ffi::OsStr;
    let file = OsStr::new("/run/user/1000/aida-init-cd.abc123");
    // A named file wins regardless of what the wrapper advertises.
    assert_eq!(
        enter_dir_channel(Some(file), Some("eval-block,init-cd-file")),
        EnterDirChannel::File(file.into())
    );
    assert_eq!(
        enter_dir_channel(Some(file), None),
        EnterDirChannel::File(file.into())
    );
    // An old wrapper that still captures stdout gets the stdout block.
    assert_eq!(
        enter_dir_channel(None, Some("eval-block,init-cd")),
        EnterDirChannel::StdoutMarkers
    );
    // An empty env var is "not set"; the new wrapper token alone is NOT a
    // request for stdout markers (nobody would read them).
    assert_eq!(
        enter_dir_channel(Some(OsStr::new("")), Some("eval-block,init-cd-file")),
        EnterDirChannel::Hint
    );
    assert_eq!(enter_dir_channel(None, None), EnterDirChannel::Hint);
}
