//! STORY-1656: `aida init DIR --github --gitlab HOST/GROUP` creates the repo
//! on both forges and publishes both legs to each; GitLab projects are
//! created through the API only when push-to-create is refused.
//!
//! Nothing here talks to a real forge: `gh` and `glab` are fake scripts on
//! PATH / `AIDA_TEST_GLAB_BINARY`, and the GitLab SSH transport is a fake
//! `ssh` (GIT_SSH_COMMAND) serving bare repos from a temp "forge root" that
//! answers an unknown project with GitLab's refusal text.
// trace:STORY-1656 | ai:claude

use super::*;

// ───────────────────────────── pure pieces ─────────────────────────────

#[test]
fn refusal_classifier_only_matches_project_not_found() {
    assert!(is_push_to_create_refusal(
        "ERROR: The project you were looking for could not be found or you don't have permission to view it.\n\
         fatal: Could not read from remote repository."
    ));
    assert!(is_push_to_create_refusal(
        "remote: project could not be found"
    ));
    for other in [
        "git@gitlab.example.test: Permission denied (publickey).",
        "ssh: connect to host gitlab.example.test port 22: Connection refused",
        " ! [rejected]        main -> main (non-fast-forward)",
        "fatal: repository 'https://gitlab.example.test/ai/x.git/' not found",
        "",
    ] {
        assert!(
            !is_push_to_create_refusal(other),
            "must not treat as a push-to-create refusal: {other}"
        );
    }
}

#[test]
fn create_project_args_private_by_default_with_namespace() {
    assert_eq!(
        create_project_args("gitlab.example.test", Some(42), "proj", false),
        vec![
            "api",
            "--hostname",
            "gitlab.example.test",
            "-X",
            "POST",
            "projects",
            "-f",
            "name=proj",
            "-f",
            "path=proj",
            "-f",
            "namespace_id=42",
            "-f",
            "visibility=private",
        ]
    );
    let public = create_project_args("gitlab.example.test", None, "proj", true);
    assert!(public.contains(&"visibility=public".to_string()));
    assert!(
        !public.iter().any(|a| a.starts_with("namespace_id=")),
        "no namespace means the caller's personal namespace: {public:?}"
    );
    assert_eq!(
        namespace_lookup_args("gitlab.example.test", "ai/tools"),
        vec![
            "api",
            "--hostname",
            "gitlab.example.test",
            "namespaces/ai%2Ftools"
        ]
    );
    assert_eq!(
        parse_namespace_id(r#"{"id":42,"full_path":"ai"}"#),
        Some(42)
    );
    assert_eq!(parse_namespace_id(r#"{"message":"404 Not found"}"#), None);
}

#[test]
fn gitlab_hub_parses_host_and_group_and_builds_urls() {
    let hub = GitLabHub::parse("gitlab.joemooney.com/ai").unwrap();
    assert_eq!(hub.host, "gitlab.joemooney.com");
    assert_eq!(hub.namespace, "ai");
    assert_eq!(
        hub.remote_url("proj"),
        "git@gitlab.joemooney.com:ai/proj.git"
    );

    let bare = GitLabHub::parse("https://GitLab.example.test/").unwrap();
    assert_eq!(bare.host, "gitlab.example.test");
    assert_eq!(bare.namespace, "");
    assert_eq!(bare.remote_url("proj"), "git@gitlab.example.test:proj.git");

    let https = GitLabHub {
        https: true,
        ..GitLabHub::parse("gitlab.example.test/ai/tools").unwrap()
    };
    assert_eq!(
        https.remote_url("proj"),
        "https://gitlab.example.test/ai/tools/proj.git"
    );
    assert!(GitLabHub::parse("").is_err());
    assert!(GitLabHub::parse("git@host").is_err());
}

#[test]
fn gitlab_target_of_url_splits_namespace_and_name() {
    assert_eq!(
        gitlab_target_of_url("git@gitlab.example.test:ai/tools/proj.git"),
        Some((
            "gitlab.example.test".to_string(),
            "ai/tools".to_string(),
            "proj".to_string()
        ))
    );
    assert_eq!(
        gitlab_target_of_url("ssh://git@gitlab.example.test:2222/ai/proj.git"),
        Some((
            "gitlab.example.test".to_string(),
            "ai".to_string(),
            "proj".to_string()
        ))
    );
}

#[test]
fn init_gitlab_flag_parses_alongside_github_and_conflicts_with_remote() {
    use clap::Parser;
    assert!(crate::cli::Cli::try_parse_from([
        "aida",
        "init",
        "proj",
        "--github",
        "--gitlab",
        "gitlab.example.test/ai",
        "--public"
    ])
    .is_ok());
    assert!(crate::cli::Cli::try_parse_from([
        "aida",
        "init",
        "proj",
        "--gitlab",
        "gitlab.example.test/ai",
        "--public"
    ])
    .is_ok());
    assert!(crate::cli::Cli::try_parse_from([
        "aida",
        "init",
        "proj",
        "--gitlab",
        "gitlab.example.test/ai",
        "--remote",
        "git@x:y.git"
    ])
    .is_err());
    assert!(
        crate::cli::Cli::try_parse_from(["aida", "init", "--gitlab", "gitlab.example.test"])
            .is_err(),
        "--gitlab needs the positional DIR"
    );
}

#[test]
fn forge_profile_mirrors_select_the_gitlab_hub() {
    // trace:TASK-1748 | ai:antigravity
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".aida")).unwrap();
    std::fs::write(
        home.path().join(".aida/forges.toml"),
        r#"
        [[profile]]
        name = "home-github"
        kind = "github"
        namespace = "joe"
        mirrors = ["home-gitlab"]

        [[profile]]
        name = "home-gitlab"
        kind = "gitlab"
        host = "gitlab.example.test"
        namespace = "ai"
        "#,
    )
    .unwrap();
    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("HOME", home.path().to_str().unwrap()),
        ("AIDA_TEST_HOME", home.path().to_str().unwrap()),
    ]);
    let mut plan = bare_plan(std::path::PathBuf::from("myproj"));
    plan.forge = Some("home-github".to_string());
    crate::init_bootstrap::resolve_forge_profile(&mut plan).unwrap();
    assert!(plan.github);
    assert_eq!(plan.github_repo.as_deref(), Some("joe/myproj"));
    assert_eq!(
        plan.gitlab,
        Some(GitLabHub {
            host: "gitlab.example.test".to_string(),
            namespace: "ai".to_string(),
            https: false,
        })
    );
}

fn bare_plan(dir: std::path::PathBuf) -> crate::init_bootstrap::BootstrapPlan {
    crate::init_bootstrap::BootstrapPlan {
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

// ───────────────────────── fake forge harness ─────────────────────────

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "user.name=aida-test",
            "-c",
            "user.email=aida-test@example.invalid",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A project with a commit on `main` and an orphan `aida-store` branch, as
/// the standard init leaves it right before the remote steps.
fn seed_project(dir: &std::path::Path) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("README.md"), "proj\n").unwrap();
    git(dir, &["add", "README.md"]);
    git(dir, &["commit", "-q", "-m", "initial"]);
    git(dir, &["checkout", "-q", "--orphan", "aida-store"]);
    git(dir, &["rm", "-rq", "--cached", "."]);
    std::fs::remove_file(dir.join("README.md")).unwrap();
    std::fs::write(dir.join("store.yaml"), "ids: []\n").unwrap();
    git(dir, &["add", "store.yaml"]);
    git(dir, &["commit", "-q", "-m", "store"]);
    git(dir, &["checkout", "-q", "-f", "main"]);
}

#[cfg(unix)]
struct FakeForges {
    _tmp: tempfile::TempDir,
    root: std::path::PathBuf,
    project: std::path::PathBuf,
    record: std::path::PathBuf,
    github_bare: std::path::PathBuf,
    gitlab_root: std::path::PathBuf,
    bin: std::path::PathBuf,
    ssh: std::path::PathBuf,
}

#[cfg(unix)]
impl FakeForges {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let project = root.join("proj");
        let record = root.join("calls.log");
        let bin = root.join("bin");
        let gitlab_root = root.join("gitlab");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&gitlab_root).unwrap();
        std::fs::write(&record, "").unwrap();

        let github_bare = root.join("github.git");
        git(&root, &["init", "-q", "--bare", "github.git"]);
        crate::test_exec::write_executable(
            &github_bare.join("hooks").join("post-receive"),
            format!(
                "#!/bin/sh\nwhile read o n r; do echo \"push github $r\" >> '{}'; done\n",
                record.display()
            ),
        );

        crate::test_exec::write_executable(
            &bin.join("gh"),
            format!(
                "#!/bin/sh\n\
                 printf 'gh %s\\n' \"$*\" >> '{rec}'\n\
                 if [ \"$1\" = repo ] && [ \"$2\" = create ]; then\n\
                   git remote add origin '{bare}' && git push --quiet -u origin HEAD\n\
                   exit $?\n\
                 fi\n\
                 exit 0\n",
                rec = record.display(),
                bare = github_bare.display()
            ),
        );

        crate::test_exec::write_executable(
            &bin.join("glab"),
            format!(
                "#!/bin/sh\n\
                 printf 'glab %s\\n' \"$*\" >> '{rec}'\n\
                 printf '%s\\n' \"$GITLAB_TOKEN\" >> '{root}/../glab-tokens.log'\n\
                 case \"$*\" in\n\
                   *\"-X POST projects\"*)\n\
                     name=''\n\
                     for a; do case \"$a\" in path=*) name=\"${{a#path=}}\";; esac; done\n\
                     repo='{root}/ai/'\"$name\"'.git'\n\
                     mkdir -p '{root}/ai' && git init --quiet --bare \"$repo\" >/dev/null 2>&1 || exit 1\n\
                     printf '#!/bin/sh\\nwhile read o n r; do echo \"push gitlab $r\" >> %s; done\\n' \"'{rec}'\" > \"$repo/hooks/post-receive\"\n\
                     chmod +x \"$repo/hooks/post-receive\"\n\
                     echo '{{\"id\":7,\"path_with_namespace\":\"ai/'\"$name\"'\"}}'\n\
                     exit 0;;\n\
                   *namespaces/*)\n\
                     echo '{{\"id\":42,\"full_path\":\"ai\",\"kind\":\"group\"}}'\n\
                     exit 0;;\n\
                 esac\n\
                 echo \"unexpected glab: $*\" >&2\n\
                 exit 1\n",
                rec = record.display(),
                root = gitlab_root.display()
            ),
        );

        // Run through `sh` (never exec'd directly) so no ETXTBSY window.
        let ssh = root.join("fake-ssh");
        std::fs::write(
            &ssh,
            r#"#!/bin/sh
cmd="$2"
printf '%s\n' "$cmd" >> 'ROOT/../ssh-commands.log'
prog="${cmd%% *}"
path=$(printf '%s' "$cmd" | sed -e "s/^[^']*'//" -e "s/'\$//")
cd 'ROOT' || exit 1
if [ -f .deny ]; then
  echo "git@gitlab.example.test: Permission denied (publickey)." >&2
  exit 255
fi
if [ ! -d "$path" ]; then
  echo "ERROR: The project you were looking for could not be found or you don't have permission to view it." >&2
  exit 1
fi
if [ -f .fail_push_stderr ]; then
  cat .fail_push_stderr >&2
  exit 1
fi
case "$prog" in
  git-receive-pack) exec git receive-pack "$path" ;;
  git-upload-pack) exec git upload-pack "$path" ;;
esac
echo "unexpected ssh command: $cmd" >&2
exit 1
"#
            .replace("ROOT", &gitlab_root.display().to_string()),
        )
        .unwrap();

        seed_project(&project);
        FakeForges {
            _tmp: tmp,
            root,
            project,
            record,
            github_bare,
            gitlab_root,
            bin,
            ssh,
        }
    }

    /// The env every fake-forge test runs under; `token` = None removes all
    /// GitLab tokens (and points glab's config dir at an empty temp dir).
    fn env(&self, token: Option<&str>) -> crate::test_env::EnvVarsGuard {
        let path = format!(
            "{}:{}",
            self.bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let glab = self.bin.join("glab").display().to_string();
        let ssh = format!("sh {}", self.ssh.display());
        let home = self.root.join("home").display().to_string();
        let xdg = self.root.join("home").join(".config").display().to_string();
        crate::test_env::EnvVarsGuard::apply(&[
            ("PATH", Some(path.as_str())),
            ("AIDA_TEST_GLAB_BINARY", Some(glab.as_str())),
            ("GIT_SSH_COMMAND", Some(ssh.as_str())),
            ("GIT_SSH_VARIANT", Some("simple")),
            ("AIDA_GITLAB_TOKEN", token),
            ("GITLAB_TOKEN", None),
            ("HOME", Some(home.as_str())),
            ("XDG_CONFIG_HOME", Some(xdg.as_str())),
            ("GIT_AUTHOR_NAME", Some("aida-test")),
            ("GIT_AUTHOR_EMAIL", Some("aida-test@example.invalid")),
            ("GIT_COMMITTER_NAME", Some("aida-test")),
            ("GIT_COMMITTER_EMAIL", Some("aida-test@example.invalid")),
        ])
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(&self.record)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn plan(&self) -> crate::init_bootstrap::BootstrapPlan {
        let mut plan = bare_plan(self.project.clone());
        plan.github = true;
        plan.gitlab = Some(GitLabHub::parse("gitlab.example.test/ai").unwrap());
        plan
    }
}

/// AC1 + AC2 + AC4: one run creates the GitHub repo, finds the GitLab group
/// refusing push-to-create, creates the project through the API (private,
/// namespace resolved from the group), then pushes main to every hub before
/// aida-store to every hub, and verifies all hubs agree.
#[cfg(unix)]
#[test]
fn init_github_plus_gitlab_creates_both_and_publishes_in_order() {
    // trace:BUG-1994 | ai:codex
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let forges = FakeForges::new();
    let _env = forges.env(Some("glpat-fake"));

    let hubs = crate::init_bootstrap::publish_remotes(&forges.plan(), &forges.project)
        .expect("publish to both forges")
        .expect("two hubs");
    assert_eq!(hubs, vec!["origin".to_string(), "gitlab".to_string()]);

    assert_eq!(
        forges.calls(),
        vec![
            "gh repo create proj --private --source . --remote origin --push",
            "push github refs/heads/main",
            "glab api --hostname gitlab.example.test namespaces/ai",
            "glab api --hostname gitlab.example.test -X POST projects -f name=proj -f path=proj -f namespace_id=42 -f visibility=private",
            "push gitlab refs/heads/main",
            "push github refs/heads/aida-store",
            "push gitlab refs/heads/aida-store",
        ]
    );

    // Both hubs hold the same tips as local, and the GitLab hub is wired as
    // a mirror (store fan-out config + code-leg hook).
    let gitlab_bare = forges.gitlab_root.join("ai").join("proj.git");
    for branch in ["main", "aida-store"] {
        let local = git(&forges.project, &["rev-parse", branch]);
        assert_eq!(git(&forges.github_bare, &["rev-parse", branch]), local);
        assert_eq!(git(&gitlab_bare, &["rev-parse", branch]), local);
    }
    assert_eq!(
        git(&forges.project, &["remote", "get-url", "gitlab"]),
        "git@gitlab.example.test:ai/proj.git"
    );
    let cfg = crate::read_store_sync_config(&forges.project).unwrap();
    assert_eq!(cfg.mirror_remotes, vec!["gitlab".to_string()]);
    assert!(forges.project.join(".git/hooks/pre-push").exists());
}

/// AC2: a push failure that is NOT the refusal must never create a project.
#[cfg(unix)]
#[test]
fn non_refusal_push_error_never_creates_project() {
    // trace:BUG-1994 | ai:codex
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let forges = FakeForges::new();
    std::fs::write(forges.gitlab_root.join(".deny"), "").unwrap();
    let _env = forges.env(Some("glpat-fake"));

    let err = crate::init_bootstrap::publish_remotes(&forges.plan(), &forges.project)
        .expect_err("a denied push fails the run")
        .to_string();
    assert!(err.contains("Permission denied"), "{err}");
    assert!(
        !forges.calls().iter().any(|c| c.starts_with("glab")),
        "glab must not run on a non-refusal error: {:?}",
        forges.calls()
    );
    assert!(!forges.gitlab_root.join("ai").exists());
}

/// AC5: no GitLab token → the manual recipe, a non-zero result, and the
/// already-created GitHub repo is left in place (not rolled back).
#[cfg(unix)]
#[test]
fn no_token_prints_manual_recipe_and_keeps_github_repo() {
    // trace:BUG-1994 | ai:codex
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let forges = FakeForges::new();
    let _env = forges.env(None);

    let err = crate::init_bootstrap::publish_remotes(&forges.plan(), &forges.project)
        .expect_err("no token must fail the run")
        .to_string();
    assert!(err.contains("no GitLab token"), "{err}");
    assert!(err.contains("aida remote attach"), "manual recipe: {err}");
    assert!(err.contains("gitlab.example.test/projects/new"), "{err}");
    let calls = forges.calls();
    assert_eq!(
        calls[0],
        "gh repo create proj --private --source . --remote origin --push"
    );
    assert!(!calls.iter().any(|c| c.starts_with("glab")), "{calls:?}");
    assert_eq!(
        git(&forges.github_bare, &["rev-parse", "main"]),
        git(&forges.project, &["rev-parse", "main"]),
        "the GitHub repo stays created and pushed"
    );
}

/// AC3: `aida remote mirror <name> --url` creates a missing GitLab project.
#[cfg(unix)]
#[test]
fn mirror_url_creates_missing_gitlab_project() {
    // trace:BUG-1994 | ai:codex
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let forges = FakeForges::new();
    let _env = forges.env(Some("glpat-fake"));

    crate::remote_create::run_remote_mirror(
        &forges.project,
        "gitlab",
        Some("git@gitlab.example.test:ai/proj.git"),
        crate::config_commit::CommitMode::Never,
    )
    .expect("mirror wiring with create-if-missing");

    let calls = forges.calls();
    assert!(
        calls.iter().any(|c| c
            == "glab api --hostname gitlab.example.test -X POST projects -f name=proj -f path=proj -f namespace_id=42 -f visibility=private"),
        "{calls:?}"
    );
    assert!(forges.gitlab_root.join("ai").join("proj.git").exists());
    // The project now exists: wiring a second remote at it creates nothing.
    crate::remote_create::run_remote_mirror(
        &forges.project,
        "gitlab2",
        Some("git@gitlab.example.test:ai/proj.git"),
        crate::config_commit::CommitMode::Never,
    )
    .unwrap();
    assert_eq!(
        forges
            .calls()
            .iter()
            .filter(|c| c.contains("POST projects"))
            .count(),
        1
    );
}

// ───────────────────────── BUG-1972 follow-ups ─────────────────────────

#[cfg(unix)]
impl FakeForges {
    /// Every token the fake `glab` was handed, one line per call.
    fn glab_tokens(&self) -> Vec<String> {
        std::fs::read_to_string(self.root.join("glab-tokens.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// Write a glab config with a `glab auth login` token for `host`, under
    /// the config dir [`FakeForges::env`] points at.
    fn glab_login(&self, host: &str, token: &str) {
        let dir = self.root.join("home").join(".config").join("glab-cli");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.yml"),
            format!("hosts:\n  {host}:\n    token: {token}\n"),
        )
        .unwrap();
    }

    fn gitlab_project_exists(&self) -> bool {
        self.gitlab_root.join("ai").join("proj.git").exists()
    }

    /// Every command the fake GitLab SSH endpoint was asked to run
    /// (`git-upload-pack` for an `ls-remote` probe).
    fn ssh_commands(&self) -> Vec<String> {
        std::fs::read_to_string(self.root.join("ssh-commands.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// Asserts nothing reached the GitLab endpoint: no probe, no `glab`.
    fn assert_gitlab_untouched(&self) {
        assert!(self.ssh_commands().is_empty(), "{:?}", self.ssh_commands());
        assert!(
            !self.calls().iter().any(|c| c.starts_with("glab")),
            "{:?}",
            self.calls()
        );
        assert!(!self.gitlab_project_exists());
    }

    // trace:TASK-1740 | ai:antigravity
    fn fail_second_push_with_stderr(&self, stderr: &str) {
        std::fs::write(self.gitlab_root.join(".fail_push_stderr"), stderr).unwrap();
    }
}

/// BUG-1972 rework: a re-run whose --url differs from the existing remote
/// keeps the remote and never probes it, so nothing is created at a URL the
/// operator did not name.
// trace:BUG-1972 | ai:claude
#[cfg(unix)]
#[test]
fn mirror_rerun_with_a_different_url_never_probes_the_existing_remote() {
    // trace:BUG-1994 | ai:codex
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let forges = FakeForges::new();
    let existing = "git@gitlab.example.test:ai/proj.git";
    git(&forges.project, &["remote", "add", "gitlab", existing]);
    let _env = forges.env(Some("glpat-fake"));

    crate::remote_create::run_remote_mirror(
        &forges.project,
        "gitlab",
        Some("git@gitlab.other.test:team/elsewhere.git"),
        crate::config_commit::CommitMode::Never,
    )
    .expect("the existing remote is kept and wired");

    forges.assert_gitlab_untouched();
    assert_eq!(
        git(&forges.project, &["remote", "get-url", "gitlab"]),
        existing
    );
    let cfg = crate::read_store_sync_config(&forges.project).unwrap();
    assert_eq!(cfg.mirror_remotes, vec!["gitlab".to_string()]);
}

/// BUG-1972 rework: a re-run without --url stays offline: no probe, no
/// `glab`, even with a token and a missing project.
// trace:BUG-1972 | ai:claude
#[cfg(unix)]
#[test]
fn mirror_rerun_without_url_stays_offline() {
    // trace:BUG-1994 | ai:codex
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let forges = FakeForges::new();
    git(
        &forges.project,
        &[
            "remote",
            "add",
            "gitlab",
            "git@gitlab.example.test:ai/proj.git",
        ],
    );
    let _env = forges.env(Some("glpat-fake"));

    crate::remote_create::run_remote_mirror(
        &forges.project,
        "gitlab",
        None,
        crate::config_commit::CommitMode::Never,
    )
    .expect("wiring an existing remote needs no network");

    forges.assert_gitlab_untouched();
    let cfg = crate::read_store_sync_config(&forges.project).unwrap();
    assert_eq!(cfg.mirror_remotes, vec!["gitlab".to_string()]);
}

/// BUG-1972 rework: the same project written another way (HTTPS vs SCP)
/// counts as the same URL, so the re-run still probes and creates it.
// trace:BUG-1972 | ai:claude
#[cfg(unix)]
#[test]
fn mirror_rerun_with_an_equivalent_url_still_probes() {
    // trace:BUG-1994 | ai:codex
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let forges = FakeForges::new();
    git(
        &forges.project,
        &[
            "remote",
            "add",
            "gitlab",
            "git@gitlab.example.test:ai/proj.git",
        ],
    );
    let _env = forges.env(Some("glpat-fake"));

    crate::remote_create::run_remote_mirror(
        &forges.project,
        "gitlab",
        Some("https://gitlab.example.test/ai/proj"),
        crate::config_commit::CommitMode::Never,
    )
    .expect("the re-run creates the missing project");

    assert!(!forges.ssh_commands().is_empty());
    assert!(forges.gitlab_project_exists());
}

/// BUG-1972 (1): with both an env token and a glab login for the host, the
/// project is created with the host-scoped glab token, never the env one.
// trace:BUG-1972 | ai:claude
#[cfg(unix)]
#[test]
fn create_uses_the_host_scoped_glab_token_over_the_env_token() {
    // trace:BUG-1994 | ai:codex
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let forges = FakeForges::new();
    let _env = forges.env(Some("glpat-env-for-gitlab-com"));
    forges.glab_login("gitlab.example.test", "glpat-scoped");

    crate::remote_create::run_remote_mirror(
        &forges.project,
        "gitlab",
        Some("git@gitlab.example.test:ai/proj.git"),
        crate::config_commit::CommitMode::Never,
    )
    .expect("mirror wiring with create-if-missing");

    assert!(forges.gitlab_project_exists());
    let tokens = forges.glab_tokens();
    assert!(!tokens.is_empty());
    assert!(
        tokens.iter().all(|t| t == "glpat-scoped"),
        "only the host-scoped token may reach glab: {tokens:?}"
    );
}

/// BUG-1972 (2): re-running the mirror after the first run could not create
/// the project (no token yet) still creates it, though the remote exists.
// trace:BUG-1972 | ai:claude
#[cfg(unix)]
#[test]
fn mirror_rerun_creates_the_project_when_the_remote_already_exists() {
    // trace:BUG-1994 | ai:codex
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let forges = FakeForges::new();
    let url = "git@gitlab.example.test:ai/proj.git";
    {
        let _env = forges.env(None);
        let err = crate::remote_create::run_remote_mirror(
            &forges.project,
            "gitlab",
            Some(url),
            crate::config_commit::CommitMode::Never,
        )
        .expect_err("no token: the first run cannot create the project")
        .to_string();
        assert!(err.contains("no GitLab token"), "{err}");
    }
    assert_eq!(git(&forges.project, &["remote", "get-url", "gitlab"]), url);
    assert!(!forges.gitlab_project_exists());

    // The operator logs in and re-runs the same command.
    let _env = forges.env(None);
    forges.glab_login("gitlab.example.test", "glpat-after-login");
    crate::remote_create::run_remote_mirror(
        &forges.project,
        "gitlab",
        Some(url),
        crate::config_commit::CommitMode::Never,
    )
    .expect("the re-run creates the missing project");
    assert!(forges.gitlab_project_exists());
    assert_eq!(forges.glab_tokens().last().unwrap(), "glpat-after-login");
}

/// BUG-1972 (3): a probe error that is NOT the refusal wires the mirror and
/// creates nothing.
// trace:BUG-1972 | ai:claude
#[cfg(unix)]
#[test]
fn mirror_non_refusal_probe_error_wires_without_creating() {
    // trace:BUG-1994 | ai:codex
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let forges = FakeForges::new();
    std::fs::write(forges.gitlab_root.join(".deny"), "").unwrap();
    let _env = forges.env(Some("glpat-fake"));

    crate::remote_create::run_remote_mirror(
        &forges.project,
        "gitlab",
        Some("git@gitlab.example.test:ai/proj.git"),
        crate::config_commit::CommitMode::Never,
    )
    .expect("an unreachable mirror is still wired");

    assert!(
        !forges.calls().iter().any(|c| c.starts_with("glab")),
        "{:?}",
        forges.calls()
    );
    assert!(!forges.gitlab_root.join("ai").exists());
    let cfg = crate::read_store_sync_config(&forges.project).unwrap();
    assert_eq!(cfg.mirror_remotes, vec!["gitlab".to_string()]);
}

/// BUG-1972 (3): `aida init --gitlab` alone makes GitLab `origin`, pushed
/// with `-u`, and wires no mirror hub.
// trace:BUG-1972 | ai:claude
#[cfg(unix)]
#[test]
fn gitlab_only_init_publishes_to_origin_with_upstream() {
    // trace:BUG-1994 | ai:codex
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let forges = FakeForges::new();
    let _env = forges.env(Some("glpat-fake"));
    let mut plan = forges.plan();
    plan.github = false;

    let hubs = crate::init_bootstrap::publish_remotes(&plan, &forges.project)
        .expect("publish to GitLab")
        .expect("one hub");
    assert_eq!(hubs, vec!["origin".to_string()]);
    assert_eq!(
        git(&forges.project, &["remote", "get-url", "origin"]),
        "git@gitlab.example.test:ai/proj.git"
    );
    assert_eq!(
        git(&forges.project, &["config", "branch.main.remote"]),
        "origin"
    );
    assert!(git(&forges.project, &["remote"])
        .lines()
        .all(|r| r != "gitlab"));
    let gitlab_bare = forges.gitlab_root.join("ai").join("proj.git");
    for branch in ["main", "aida-store"] {
        assert_eq!(
            git(&gitlab_bare, &["rev-parse", branch]),
            git(&forges.project, &["rev-parse", branch])
        );
    }
    assert!(!forges.calls().iter().any(|c| c.starts_with("gh ")));
    let cfg = crate::read_store_sync_config(&forges.project).unwrap();
    assert!(cfg.mirror_remotes.is_empty(), "{:?}", cfg.mirror_remotes);
}

/// BUG-1972 review: a URL's embedded token never reaches the terminal.
// trace:BUG-1972 | ai:claude
#[test]
fn probe_warning_redacts_url_userinfo() {
    assert_eq!(
        redact_url_userinfo(
            "fatal: unable to access 'https://oauth2:glpat-XYZ@gitlab.corp/ai/p.git/': 502"
        ),
        "fatal: unable to access 'https://***@gitlab.corp/ai/p.git/': 502"
    );
    assert_eq!(
        redact_url_userinfo("https://gitlab.corp/ai/p.git and git@gitlab.corp:ai/p.git"),
        "https://gitlab.corp/ai/p.git and git@gitlab.corp:ai/p.git"
    );
    assert_eq!(redact_url_userinfo("ssh://git@h:22/x"), "ssh://***@h:22/x");
}

/// BUG-1972 (3): the ls-remote probe never waits on a credential prompt.
// trace:BUG-1972 | ai:claude
#[test]
fn ls_remote_probe_disables_the_terminal_prompt() {
    let cmd = ls_remote_probe(
        std::path::Path::new("."),
        "https://gitlab.example.test/ai/proj.git",
    );
    assert!(cmd
        .get_envs()
        .any(|(k, v)| k == "GIT_TERMINAL_PROMPT" && v == Some(std::ffi::OsStr::new("0"))));
}

/// AC4: a hub missing a leg (or on another tip) is reported, so init exits
/// non-zero instead of claiming success.
#[test]
fn hub_disagreement_is_reported() {
    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("GIT_AUTHOR_NAME", "aida-test"),
        ("GIT_AUTHOR_EMAIL", "aida-test@example.invalid"),
        ("GIT_COMMITTER_NAME", "aida-test"),
        ("GIT_COMMITTER_EMAIL", "aida-test@example.invalid"),
    ]);
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("proj");
    seed_project(&project);
    git(tmp.path(), &["init", "-q", "--bare", "a.git"]);
    git(tmp.path(), &["init", "-q", "--bare", "b.git"]);
    let a = tmp.path().join("a.git").display().to_string();
    let b = tmp.path().join("b.git").display().to_string();
    git(&project, &["remote", "add", "origin", &a]);
    git(&project, &["remote", "add", "gitlab", &b]);
    git(&project, &["push", "-q", "origin", "main", "aida-store"]);
    git(&project, &["push", "-q", "gitlab", "main"]);

    let hubs = vec!["origin".to_string(), "gitlab".to_string()];
    let gaps = hub_disagreements(&project, &hubs, &["main", "aida-store"]);
    assert_eq!(gaps.len(), 1, "{gaps:?}");
    assert!(
        gaps[0].starts_with("gitlab/aida-store is absent"),
        "{gaps:?}"
    );

    git(&project, &["push", "-q", "gitlab", "aida-store"]);
    assert!(hub_disagreements(&project, &hubs, &["main", "aida-store"]).is_empty());
}

// ───────────────────────── TASK-1740 test gaps ─────────────────────────

/// Redirects the process-wide fd 1/2 into temp files so the notices printed
/// by the code under test can be asserted. Hazard: while a capture is open,
/// output from parallel tests and child processes also lands in it, so
/// asserts only check for presence/absence of the expected lines, and
/// `capture_output` serializes captures with a mutex so two never overlap.
// trace:TASK-1740 | ai:claude
#[cfg(unix)]
struct StdioCapture {
    old_stdout: libc::c_int,
    old_stderr: libc::c_int,
    stdout_path: std::path::PathBuf,
    stderr_path: std::path::PathBuf,
    _tempdir: tempfile::TempDir,
}

#[cfg(unix)]
impl StdioCapture {
    fn new() -> Self {
        use std::os::unix::io::AsRawFd;
        let tempdir = tempfile::tempdir().unwrap();
        let stdout_path = tempdir.path().join("stdout.log");
        let stderr_path = tempdir.path().join("stderr.log");
        let stdout_file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&stdout_path)
            .unwrap();
        let stderr_file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&stderr_path)
            .unwrap();

        let old_stdout = unsafe { libc::dup(libc::STDOUT_FILENO) };
        let old_stderr = unsafe { libc::dup(libc::STDERR_FILENO) };
        unsafe {
            libc::dup2(stdout_file.as_raw_fd(), libc::STDOUT_FILENO);
            libc::dup2(stderr_file.as_raw_fd(), libc::STDERR_FILENO);
        }

        Self {
            old_stdout,
            old_stderr,
            stdout_path,
            stderr_path,
            _tempdir: tempdir,
        }
    }

    fn finish(mut self) -> (String, String) {
        use std::io::Write;
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();

        unsafe {
            if self.old_stdout >= 0 {
                libc::dup2(self.old_stdout, libc::STDOUT_FILENO);
                libc::close(self.old_stdout);
                self.old_stdout = -1;
            }
            if self.old_stderr >= 0 {
                libc::dup2(self.old_stderr, libc::STDERR_FILENO);
                libc::close(self.old_stderr);
                self.old_stderr = -1;
            }
        }

        let out = std::fs::read_to_string(&self.stdout_path).unwrap_or_default();
        let err = std::fs::read_to_string(&self.stderr_path).unwrap_or_default();
        (out, err)
    }
}

#[cfg(unix)]
impl Drop for StdioCapture {
    fn drop(&mut self) {
        unsafe {
            if self.old_stdout >= 0 {
                libc::dup2(self.old_stdout, libc::STDOUT_FILENO);
                libc::close(self.old_stdout);
                self.old_stdout = -1;
            }
            if self.old_stderr >= 0 {
                libc::dup2(self.old_stderr, libc::STDERR_FILENO);
                libc::close(self.old_stderr);
                self.old_stderr = -1;
            }
        }
    }
}

// trace:TASK-1740 | ai:antigravity
#[cfg(unix)]
fn capture_output<F, R>(f: F) -> ((String, String), R)
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    static CAPTURE_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _lock = CAPTURE_MUTEX.lock().unwrap();

    let capture = StdioCapture::new();
    let (tx, rx) = std::sync::mpsc::channel();

    struct Payload {
        run: Box<dyn FnOnce() + Send>,
    }

    extern "C" fn pthread_runner(arg: *mut libc::c_void) -> *mut libc::c_void {
        let payload: Box<Payload> = unsafe { Box::from_raw(arg as *mut Payload) };
        (payload.run)();
        use std::io::Write;
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
        std::ptr::null_mut()
    }

    let payload = Box::new(Payload {
        run: Box::new(move || {
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
            let _ = tx.send(res);
        }),
    });
    let raw = Box::into_raw(payload);
    let mut thread: libc::pthread_t = 0;
    unsafe {
        let rc = libc::pthread_create(
            &mut thread,
            std::ptr::null(),
            pthread_runner,
            raw as *mut libc::c_void,
        );
        assert_eq!(rc, 0, "pthread_create failed: {rc}");
        let join_rc = libc::pthread_join(thread, std::ptr::null_mut());
        assert_eq!(join_rc, 0, "pthread_join failed: {join_rc}");
    }

    let (stdout, stderr) = capture.finish();
    let res = rx
        .recv()
        .expect("child pthread must send result")
        .unwrap_or_else(|e| std::panic::resume_unwind(e));
    ((stdout, stderr), res)
}

/// TASK-1740 (1): a re-run whose --url has the same host but a different path
/// (e.g. ai/proj vs team/elsewhere) keeps the existing remote and never probes
/// or creates the existing project.
// trace:TASK-1740 | ai:antigravity
#[cfg(unix)]
#[test]
fn mirror_rerun_with_same_host_different_path_never_probes_existing_remote() {
    // trace:TASK-1740 | ai:claude
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let forges = FakeForges::new();
    let existing = "git@gitlab.example.test:ai/proj.git";
    git(&forges.project, &["remote", "add", "gitlab", existing]);
    let _env = forges.env(Some("glpat-fake"));

    crate::remote_create::run_remote_mirror(
        &forges.project,
        "gitlab",
        Some("git@gitlab.example.test:team/elsewhere.git"),
        crate::config_commit::CommitMode::Never,
    )
    .expect("the existing remote is kept and wired");

    forges.assert_gitlab_untouched();
    assert_eq!(
        git(&forges.project, &["remote", "get-url", "gitlab"]),
        existing
    );
    let cfg = crate::read_store_sync_config(&forges.project).unwrap();
    assert_eq!(cfg.mirror_remotes, vec!["gitlab".to_string()]);
}

/// TASK-1740 (2): when creating a project using an env token rather than a host-scoped
/// glab login token, the spec-required notice is printed to stdout.
// trace:TASK-1740 | ai:antigravity
#[cfg(unix)]
#[test]
fn create_gitlab_project_prints_notice_when_using_env_token() {
    // trace:TASK-1740 | ai:claude
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let forges = FakeForges::new();
    let bin_path = forges.bin.display().to_string();
    let glab_path = forges.bin.join("glab").display().to_string();

    let ((stdout, _stderr), res) = capture_output(move || {
        let path = format!("{}:{}", bin_path, std::env::var("PATH").unwrap_or_default());
        let _env = crate::test_env::EnvVarsGuard::apply(&[
            ("PATH", Some(path.as_str())),
            ("AIDA_TEST_GLAB_BINARY", Some(glab_path.as_str())),
            ("GITLAB_TOKEN", Some("glpat-env-token")),
            ("AIDA_GITLAB_TOKEN", None),
        ]);
        crate::gitlab_project_create::create_gitlab_project(
            "gitlab.example.test",
            "ai",
            "proj",
            false,
        )
    });
    res.expect("create_gitlab_project should succeed with valid env token");
    assert!(
        stdout.contains("no `glab auth login` token for gitlab.example.test; using $GITLAB_TOKEN"),
        "expected env-token notice on stdout, got:\n{stdout}"
    );
    assert!(
        stdout.contains("run `glab auth login --hostname gitlab.example.test`"),
        "expected hint to scope token in notice, got:\n{stdout}"
    );
}

/// TASK-1740 (3): 'already points at' warning redacts embedded credentials from the existing remote URL.
// trace:TASK-1740 | ai:antigravity
#[cfg(unix)]
#[test]
fn mirror_rerun_redacts_credentials_in_already_points_at_warning() {
    // trace:TASK-1740 | ai:claude
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let forges = FakeForges::new();
    let secret = "glpat-supersecret-existing-token";
    let existing = format!("https://oauth2:{secret}@gitlab.example.test/ai/proj.git");
    git(&forges.project, &["remote", "add", "gitlab", &existing]);
    let project = forges.project.clone();

    let ((_stdout, stderr), res) = capture_output(move || {
        let _env = forges.env(Some("glpat-fake"));
        crate::remote_create::run_remote_mirror(
            &project,
            "gitlab",
            Some("https://gitlab.example.test/team/different.git"),
            crate::config_commit::CommitMode::Never,
        )
    });
    res.expect("remote mirror run should succeed");
    assert!(
        stderr.contains("already points at"),
        "expected 'already points at' warning on stderr, got:\n{stderr}"
    );
    assert!(
        !stderr.contains(secret),
        "embedded secret must NOT appear in stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("https://***@gitlab.example.test/ai/proj.git"),
        "expected redacted URL with *** in stderr, got:\n{stderr}"
    );
}

/// TASK-1740 (3): 'added remote' announcement redacts embedded credentials from the URL.
// trace:TASK-1740 | ai:antigravity
#[cfg(unix)]
#[test]
fn run_remote_mirror_redacts_credentials_in_added_remote_notice() {
    // trace:TASK-1740 | ai:claude
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let forges = FakeForges::new();
    let secret = "glpat-supersecret-add-token";
    let url = format!("https://oauth2:{secret}@gitlab.example.test/ai/newproj.git");
    let project = forges.project.clone();

    let ((stdout, _stderr), res) = capture_output(move || {
        let _env = forges.env(Some("glpat-fake"));
        crate::remote_create::run_remote_mirror(
            &project,
            "gitlab",
            Some(&url),
            crate::config_commit::CommitMode::Never,
        )
    });
    res.expect("adding new remote mirror should succeed");
    assert!(
        stdout.contains("added remote `gitlab`"),
        "expected 'added remote' announcement on stdout, got:\n{stdout}"
    );
    assert!(
        !stdout.contains(secret),
        "embedded secret must NOT appear in stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("https://***@gitlab.example.test/ai/newproj.git"),
        "expected redacted URL with *** in stdout, got:\n{stdout}"
    );
}

/// TASK-1740 (3): push fallback error redacts credentials in the 'still failed' error message.
// trace:TASK-1740 | ai:antigravity
#[cfg(unix)]
#[test]
fn push_fallback_redacts_credentials_in_still_failed_error() {
    // trace:TASK-1740 | ai:claude
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let forges = FakeForges::new();
    let url = "git@gitlab.example.test:ai/proj.git";
    git(&forges.project, &["remote", "add", "gitlab", url]);
    let secret = "glpat-supersecret-second-push";
    let credential_url = format!("https://oauth2:{secret}@gitlab.example.test/ai/proj.git");
    let fake_stderr = format!("fatal: access denied to '{credential_url}'");
    forges.fail_second_push_with_stderr(&fake_stderr);
    let _env = forges.env(Some("glpat-fake"));

    let err = push_with_create_fallback(&forges.project, "gitlab", url, "main", false, false)
        .unwrap_err()
        .to_string();

    assert!(
        err.contains("still failed after creating the project"),
        "expected 'still failed' error message, got: {err}"
    );
    assert!(
        !err.contains(secret),
        "embedded secret must NOT appear in still-failed error: {err}"
    );
    assert!(
        err.contains("https://***@gitlab.example.test/ai/proj.git"),
        "expected redacted URL with ***, got: {err}"
    );
}
