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
