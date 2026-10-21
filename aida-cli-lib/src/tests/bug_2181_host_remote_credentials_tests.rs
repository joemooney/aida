//! Host remote calls (store sync, push/pull, doctor, PR, session and phase
//! probes) keep an operator-embedded `oauth2:<token>@` origin URL out of
//! `git-remote-https` argv, and still authenticate with that token.
//!
//! The fixture origin is a credentialed `https://` URL. A fake
//! `git-remote-https` on `GIT_EXEC_PATH` records its argv, refuses unless
//! `git credential fill` yields the token, then proxies a real transfer to a
//! local bare repository through `git remote-ext`.
// trace:BUG-2181 | ai:claude

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

const SECRET: &str = "glpat-host-remote-secret-2181";
const HOST: &str = "fixture.example.test";

struct CredentialedOrigin {
    _tmp: TempDir,
    repo: PathBuf,
    bare: PathBuf,
    argv_log: PathBuf,
    exec_dir: PathBuf,
    main_sha: String,
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

impl CredentialedOrigin {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let bare = tmp.path().join("bare.git");
        let seed = tmp.path().join("seed");
        let repo = tmp.path().join("repo");
        git(
            tmp.path(),
            &["init", "-q", "--bare", bare.to_str().unwrap()],
        );
        git(
            tmp.path(),
            &["init", "-q", "-b", "main", seed.to_str().unwrap()],
        );
        git(&seed, &["commit", "-q", "--allow-empty", "-m", "seed"]);
        git(&seed, &["push", "-q", bare.to_str().unwrap(), "main"]);
        let main_sha = git(&seed, &["rev-parse", "HEAD"]);
        git(
            tmp.path(),
            &["init", "-q", "-b", "work", repo.to_str().unwrap()],
        );
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "work"]);
        let url = format!("https://oauth2:{SECRET}@{HOST}/group/project.git");
        git(&repo, &["remote", "add", "origin", &url]);

        let exec_dir = tmp.path().join("git-exec");
        fs::create_dir_all(&exec_dir).unwrap();
        let argv_log = tmp.path().join("remote_https_argv.txt");
        crate::test_exec::write_executable(
            &exec_dir.join("git-remote-https"),
            format!(
                "#!/bin/sh\n\
                 for a in \"$@\"; do printf '%s\\n' \"$a\" >> '{log}'; done\n\
                 cred=$(printf 'protocol=https\\nhost={HOST}\\n\\n' | GIT_TERMINAL_PROMPT=0 git credential fill 2>/dev/null)\n\
                 case \"$cred\" in *'password={SECRET}'*) ;; *) echo 'fatal: Authentication failed' >&2; exit 128 ;; esac\n\
                 exec git -c protocol.ext.allow=always remote-ext \"$1\" \"git %s {bare}\"\n",
                log = argv_log.display(),
                bare = bare.display(),
            ),
        );
        CredentialedOrigin {
            _tmp: tmp,
            repo,
            bare,
            argv_log,
            exec_dir,
            main_sha,
        }
    }

    /// Route every `git` child's https transport through the fake helper.
    fn transport_env(&self) -> crate::test_env::EnvVarsGuard {
        let real_exec = git(&self.repo, &["--exec-path"]);
        let path = format!(
            "{}:{real_exec}:{}",
            self.exec_dir.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        crate::test_env::EnvVarsGuard::apply(&[
            ("GIT_EXEC_PATH", Some(self.exec_dir.to_str().unwrap())),
            ("PATH", Some(&path)),
            ("GIT_TERMINAL_PROMPT", Some("0")),
        ])
    }

    fn argv(&self) -> String {
        fs::read_to_string(&self.argv_log).unwrap_or_default()
    }
}

/// Each routed call reaches the credentialed origin, authenticates with the
/// stored token, and the transport helper's argv only ever names the clean
/// URL. Covers the core `git_ops` remote calls (store sync's
/// `remote_branch_head_sha`, fetch, push) and the cli probes.
#[cfg(unix)]
#[test]
fn routed_host_remote_calls_authenticate_without_the_token_in_argv() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let fx = CredentialedOrigin::new();
    let _env = fx.transport_env();
    let repo = fx.repo.as_path();

    assert_eq!(
        aida_core::git_ops::remote_branch_head_sha(repo, "origin", "main").as_deref(),
        Some(fx.main_sha.as_str()),
        "store sync's ls-remote must authenticate and read origin/main"
    );
    assert!(aida_core::git_ops::is_remote_reachable(repo, "origin"));
    aida_core::git_ops::fetch_branch_into_local(repo, "origin", "main").unwrap();
    assert!(
        aida_core::git_ops::push(repo, "origin", "work").unwrap(),
        "push must authenticate"
    );
    assert_eq!(
        git(&fx.bare, &["rev-parse", "refs/heads/work"]),
        git(repo, &["rev-parse", "work"]),
        "the push reached origin"
    );
    assert_eq!(
        crate::phase_driver::remote_head_sha(repo, "main").as_deref(),
        Some(fx.main_sha.as_str())
    );
    assert_eq!(
        crate::orchestrator_leases::remote_branch_gone(repo, "main"),
        Some(false)
    );
    assert_eq!(
        crate::orchestrator_leases::remote_branch_gone(repo, "no-such-branch"),
        Some(true)
    );
    assert!(matches!(
        crate::pr_lookup::probe_branch_on_origin(repo, "main"),
        crate::pr_lookup::BranchOriginProbe::Present
    ));
    let fetched = crate::store_push_pull::run_remote_git_with_timeout(
        repo,
        "origin",
        &["fetch", "--quiet", "--no-tags", "origin", "main"],
        std::time::Duration::from_secs(60),
    )
    .unwrap();
    assert!(fetched.success(), "timed fetch must authenticate");
    crate::store_push_pull::fetch_branch(repo, "main", true).unwrap();

    let argv = fx.argv();
    assert!(
        !argv.contains(SECRET),
        "the origin token must never reach git-remote-https argv:\n{argv}"
    );
    assert!(
        argv.contains(&format!("https://{HOST}/group/project.git")),
        "git-remote-https must receive the clean origin URL:\n{argv}"
    );
    assert_eq!(
        git(repo, &["config", "--get", "remote.origin.url"]),
        format!("https://oauth2:{SECRET}@{HOST}/group/project.git"),
        "the operator's stored credential must be left untouched"
    );
}

/// The seam is live: a bare `git ls-remote origin` that skips the helper
/// does put the token in the transport's argv, so the test above can fail.
#[cfg(unix)]
#[test]
fn an_unrouted_remote_call_leaks_the_token_into_transport_argv() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let fx = CredentialedOrigin::new();
    let _env = fx.transport_env();
    let _ = crate::test_git::git_command()
        .arg("-C")
        .arg(&fx.repo)
        .args(["ls-remote", "origin"])
        .output()
        .unwrap();
    assert!(
        fx.argv().contains(SECRET),
        "an unrouted call must expose the token, or the argv seam is vacuous:\n{}",
        fx.argv()
    );
}
