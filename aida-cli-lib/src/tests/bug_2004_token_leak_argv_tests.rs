//! BUG-2004 / BUG-2119: Ensure tokens avoid command argv exposure across
//! `curl`, `git`, and `git-remote-http` / `git-remote-https`:
//! (1) GitLab release probe (`gitlab_release_exists`) streams `PRIVATE-TOKEN`
//!     via curl stdin (`-H @-`), avoiding the curl command line, and returns
//!     `None` on curl/HTTP failures without leaking the token.
//! (2) Mirror probes and pushes (`ensure_gitlab_project_exists`,
//!     `run_remote_mirror`, `run_mirror_push`, `mirror_sync_hubs`,
//!     `remote_tag_head`) pass the remote NAME to `git`, strip any embedded
//!     `oauth2:<token>@` userinfo from `.git/config` and `GIT_CONFIG_*`, and
//!     feed credentials through an env-backed `credential.helper` so neither
//!     `git` nor `git-remote-http(s)` exposes the token in `ps` argv.
// trace:BUG-2004 | ai:antigravity
// trace:BUG-2119 | ai:antigravity

use super::*;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

#[cfg(unix)]
fn init_bin_dir(tmp: &TempDir) -> PathBuf {
    let bin = tmp.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    bin
}

#[cfg(unix)]
#[test]
fn gitlab_release_exists_passes_token_via_stdin_not_argv() {
    // trace:BUG-1994 | ai:claude
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = TempDir::new().unwrap();
    let bin = init_bin_dir(&tmp);
    let curl_bin = bin.join("curl");
    let rec_argv = tmp.path().join("rec_argv.txt");
    let rec_stdin = tmp.path().join("rec_stdin.txt");

    let script = format!(
        "#!/bin/sh\n\
         for a in \"$@\"; do\n\
             printf '%s\\n' \"$a\" >> '{}'\n\
         done\n\
         cat >> '{}'\n\
         printf '{{\"tag_name\":\"v1.2.3\"}}\\n200\\n'\n\
         exit 0\n",
        rec_argv.display(),
        rec_stdin.display()
    );
    crate::test_exec::write_executable(&curl_bin, script);

    let old_path = std::env::var("PATH").unwrap_or_default();
    let new_path = format!("{}:{}", bin.display(), old_path);
    let secret = "glpat-supersecret-token-12345";

    let _guard = crate::test_env::EnvVarsGuard::apply(&[
        ("PATH", Some(&new_path)),
        ("GITLAB_TOKEN", Some(secret)),
        ("AIDA_GITLAB_TOKEN", None),
        ("HOME", Some(tmp.path().to_str().unwrap())),
    ]);

    let res = crate::remote_create::gitlab_release_exists(
        "https://gitlab.example.com/api/v4/projects/foo%2Fbar/releases/v1.2.3",
        "v1.2.3",
    );
    assert_eq!(
        res,
        Some(true),
        "expected release to be recognized as existing"
    );

    let argv_content = fs::read_to_string(&rec_argv).unwrap_or_default();
    let stdin_content = fs::read_to_string(&rec_stdin).unwrap_or_default();

    assert!(
        !argv_content.contains(secret),
        "secret token must NOT appear in curl argv:\n{argv_content}"
    );
    assert!(
        argv_content.contains("-H\n@-")
            || (argv_content.contains("-H") && argv_content.contains("@-")),
        "curl must be passed -H @- to read header from stdin:\n{argv_content}"
    );

    assert!(
        stdin_content.contains(&format!("PRIVATE-TOKEN: {secret}")),
        "stdin must contain the PRIVATE-TOKEN header:\n{stdin_content}"
    );
}

#[cfg(unix)]
#[test]
fn gitlab_release_exists_without_token_does_not_pass_stdin_or_header() {
    // trace:BUG-1994 | ai:claude
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = TempDir::new().unwrap();
    let bin = init_bin_dir(&tmp);
    let curl_bin = bin.join("curl");
    let rec_argv = tmp.path().join("rec_argv.txt");
    let rec_stdin = tmp.path().join("rec_stdin.txt");

    let script = format!(
        "#!/bin/sh\n\
         for a in \"$@\"; do\n\
             printf '%s\\n' \"$a\" >> '{}'\n\
         done\n\
         cat >> '{}'\n\
         printf '{{\"tag_name\":\"v1.2.3\"}}\\n200\\n'\n\
         exit 0\n",
        rec_argv.display(),
        rec_stdin.display()
    );
    crate::test_exec::write_executable(&curl_bin, script);

    let old_path = std::env::var("PATH").unwrap_or_default();
    let new_path = format!("{}:{}", bin.display(), old_path);

    let _guard = crate::test_env::EnvVarsGuard::apply(&[
        ("PATH", Some(&new_path)),
        ("GITLAB_TOKEN", None),
        ("AIDA_GITLAB_TOKEN", None),
        ("HOME", Some(tmp.path().to_str().unwrap())),
    ]);

    let res = crate::remote_create::gitlab_release_exists(
        "https://gitlab.example.com/api/v4/projects/foo%2Fbar/releases/v1.2.3",
        "v1.2.3",
    );
    assert_eq!(
        res,
        Some(true),
        "expected release to be recognized as existing"
    );

    let argv_content = fs::read_to_string(&rec_argv).unwrap_or_default();
    let stdin_content = fs::read_to_string(&rec_stdin).unwrap_or_default();

    assert!(
        !argv_content.contains("-H") && !argv_content.contains("@-"),
        "curl must not be passed -H @- when token is absent:\n{argv_content}"
    );
    assert!(
        stdin_content.is_empty(),
        "no stdin should be streamed when token is absent:\n{stdin_content}"
    );
}

#[cfg(unix)]
#[test]
fn gitlab_release_exists_curl_failures_do_not_leak_token() {
    // trace:BUG-1994 | ai:claude
    // trace:BUG-2119 | ai:antigravity
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = TempDir::new().unwrap();
    let bin = init_bin_dir(&tmp);
    let curl_bin = bin.join("curl");
    let rec_argv = tmp.path().join("rec_argv.txt");

    let old_path = std::env::var("PATH").unwrap_or_default();
    let new_path = format!("{}:{}", bin.display(), old_path);
    let secret = "glpat-failure-secret-998877";

    let _guard = crate::test_env::EnvVarsGuard::apply(&[
        ("PATH", Some(&new_path)),
        ("GITLAB_TOKEN", Some(secret)),
        ("AIDA_GITLAB_TOKEN", None),
        ("HOME", Some(tmp.path().to_str().unwrap())),
    ]);

    // Case 1: curl exits non-zero (exit 22) after reading stdin.
    let script_exit_22 = format!(
        "#!/bin/sh\n\
         for a in \"$@\"; do printf '%s\\n' \"$a\" >> '{}'; done\n\
         cat >/dev/null\n\
         echo 'curl: (22) The requested URL returned error: 502' >&2\n\
         exit 22\n",
        rec_argv.display()
    );
    crate::test_exec::write_executable(&curl_bin, script_exit_22);
    assert_eq!(
        crate::remote_create::gitlab_release_exists(
            "https://gitlab.example.com/api/v4/projects/foo%2Fbar/releases/v1.2.3",
            "v1.2.3",
        ),
        None,
        "non-zero curl exit must return None"
    );

    // Case 2: curl returns HTTP 500 with body.
    let script_http_500 = format!(
        "#!/bin/sh\n\
         for a in \"$@\"; do printf '%s\\n' \"$a\" >> '{}'; done\n\
         cat >/dev/null\n\
         printf '{{\"message\":\"500 Internal Server Error\"}}\\n500\\n'\n\
         exit 0\n",
        rec_argv.display()
    );
    crate::test_exec::write_executable(&curl_bin, script_http_500);
    assert_eq!(
        crate::remote_create::gitlab_release_exists(
            "https://gitlab.example.com/api/v4/projects/foo%2Fbar/releases/v1.2.3",
            "v1.2.3",
        ),
        None,
        "HTTP 500 must return None"
    );

    let argv_content = fs::read_to_string(&rec_argv).unwrap_or_default();
    assert!(
        !argv_content.contains(secret),
        "secret token must never appear in curl argv across failure modes:\n{argv_content}"
    );
}

#[test]
fn mirror_probe_command_passes_remote_name_to_ls_remote() {
    let dir = std::path::Path::new("/test/project");
    let secret_url = "https://oauth2:SECRET_TOKEN_VALUE@gitlab.com/group/my-project.git";

    assert!(
        crate::gitlab_project_create::mirror_probe_command(
            dir,
            "github-remote",
            "https://github.com/group/my-project.git"
        )
        .is_none(),
        "mirror_probe_command must return None for non-GitLab URLs"
    );

    let cmd =
        crate::gitlab_project_create::mirror_probe_command(dir, "my-gitlab-remote", secret_url)
            .expect("mirror_probe_command must return Command for GitLab URL");

    let args: Vec<&str> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();
    assert_eq!(
        args,
        vec![
            "-C",
            "/test/project",
            "ls-remote",
            "--heads",
            "my-gitlab-remote"
        ],
        "mirror_probe_command must pass remote name, not URL"
    );
    assert!(
        !args.iter().any(|a| a.contains("SECRET")),
        "probe command must not contain secrets in argv: {args:?}"
    );
    assert!(
        !args.iter().any(|a| a.contains("gitlab.com")),
        "probe command must pass remote name instead of URL: {args:?}"
    );

    let envs: std::collections::HashMap<String, String> = cmd
        .get_envs()
        .filter_map(|(k, v)| Some((k.to_str()?.to_string(), v?.to_str()?.to_string())))
        .collect();
    assert_eq!(
        envs.get("AIDA_GIT_CREDENTIAL_TOKEN").map(String::as_str),
        Some("SECRET_TOKEN_VALUE"),
        "token must be passed in AIDA_GIT_CREDENTIAL_TOKEN env var"
    );
    assert!(
        envs.values()
            .filter(|v| *v != "SECRET_TOKEN_VALUE")
            .all(|v| !v.contains("SECRET_TOKEN_VALUE")),
        "GIT_CONFIG_VALUE_* must strip embedded secret from URL: {envs:?}"
    );
}

#[cfg(unix)]
#[test]
fn git_remote_https_receives_stripped_url_and_credential_helper_supplies_token() {
    // trace:BUG-1994 | ai:claude
    // trace:BUG-2119 | ai:antigravity
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    assert!(std::process::Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["init", "-q"])
        .status()
        .unwrap()
        .success());
    let secret = "glpat-secret-in-remote-http-777";
    let secret_url = format!("https://oauth2:{secret}@gitlab.example.test/group/my-project.git");
    // Even if .git/config had the credentialed URL from an earlier setup,
    // configure_git_remote_credentials must override remote.<name>.url to the
    // clean URL and supply the token through credential.helper.
    assert!(std::process::Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["remote", "add", "my-gitlab-remote", &secret_url])
        .status()
        .unwrap()
        .success());

    // Build a custom GIT_EXEC_PATH directory with a fake `git-remote-https`
    // that records its own argv and invokes `git credential fill` to verify
    // the credential helper supplies the token via stdin/stdout.
    let exec_dir = tmp.path().join("git-exec");
    fs::create_dir_all(&exec_dir).unwrap();
    let helper_argv_file = tmp.path().join("git_remote_https_argv.txt");
    let cred_out_file = tmp.path().join("git_credential_fill.txt");
    let fake_remote_https = exec_dir.join("git-remote-https");
    let helper_script = format!(
        "#!/bin/sh\n\
         for a in \"$@\"; do\n\
             printf '%s\\n' \"$a\" >> '{}'\n\
         done\n\
         printf 'protocol=https\\nhost=gitlab.example.test\\n\\n' | git credential fill > '{}'\n\
         while IFS= read -r line; do\n\
             case \"$line\" in\n\
                 capabilities*) printf 'list\\n\\n' ;;\n\
                 list*) printf '\\n' ; break ;;\n\
                 '') break ;;\n\
             esac\n\
         done\n\
         exit 0\n",
        helper_argv_file.display(),
        cred_out_file.display()
    );
    crate::test_exec::write_executable(&fake_remote_https, helper_script);

    let real_exec_path = String::from_utf8(
        std::process::Command::new("git")
            .arg("--exec-path")
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let old_path = std::env::var("PATH").unwrap_or_default();
    let new_path = format!(
        "{}:{}:{}",
        exec_dir.display(),
        real_exec_path.trim(),
        old_path
    );
    let _guard = crate::test_env::EnvVarsGuard::apply(&[
        ("GIT_EXEC_PATH", Some(exec_dir.to_str().unwrap())),
        ("PATH", Some(&new_path)),
    ]);

    let mut cmd =
        crate::gitlab_project_create::mirror_probe_command(&repo, "my-gitlab-remote", &secret_url)
            .expect("GitLab probe command");
    let out = cmd
        .output()
        .expect("run git ls-remote with fake git-remote-https");
    assert!(
        out.status.success(),
        "git ls-remote should succeed with fake git-remote-https: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    let helper_argv = fs::read_to_string(&helper_argv_file).unwrap_or_default();
    assert!(
        !helper_argv.contains(secret),
        "git-remote-https argv must NEVER contain the token:\n{helper_argv}"
    );
    assert!(
        helper_argv.contains("https://gitlab.example.test/group/my-project.git"),
        "git-remote-https argv must receive the stripped clean URL:\n{helper_argv}"
    );

    let cred_out = fs::read_to_string(&cred_out_file).unwrap_or_default();
    assert!(
        cred_out.contains("username=oauth2"),
        "credential helper must supply username=oauth2:\n{cred_out}"
    );
    assert!(
        cred_out.contains(&format!("password={secret}")),
        "credential helper must supply password=<secret>:\n{cred_out}"
    );
}

#[cfg(unix)]
#[test]
fn run_remote_mirror_fresh_add_and_rerun_pass_remote_name_and_scrub_credentials() {
    // trace:BUG-1994 | ai:claude
    // trace:BUG-2119 | ai:antigravity
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(repo.join(".aida")).unwrap();
    fs::write(repo.join(".aida/config.toml"), "[store.sync]\n").unwrap();

    // Find the real git binary before prepending our wrapper to PATH.
    let real_git = std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .map(|dir| PathBuf::from(dir).join("git"))
        .find(|p| p.is_file())
        .expect("locate real git binary");

    assert!(std::process::Command::new(&real_git)
        .arg("-C")
        .arg(&repo)
        .args(["init", "-q"])
        .status()
        .unwrap()
        .success());

    let bin = init_bin_dir(&tmp);
    let git_wrapper = bin.join("git");
    let rec_git_argv = tmp.path().join("rec_git_argv.txt");
    let wrapper_script = format!(
        "#!/bin/sh\n\
         printf 'ARGV:' >> '{rec}'\n\
         for a in \"$@\"; do\n\
             printf ' %s' \"$a\" >> '{rec}'\n\
         done\n\
         printf '\\n' >> '{rec}'\n\
         for a in \"$@\"; do\n\
             if [ \"$a\" = \"ls-remote\" ]; then\n\
                 exit 0\n\
             fi\n\
         done\n\
         exec '{real_git}' \"$@\"\n",
        rec = rec_git_argv.display(),
        real_git = real_git.display(),
    );
    crate::test_exec::write_executable(&git_wrapper, wrapper_script);

    let old_path = std::env::var("PATH").unwrap_or_default();
    let new_path = format!("{}:{}", bin.display(), old_path);
    let _guard = crate::test_env::EnvVarsGuard::apply(&[
        ("PATH", Some(&new_path)),
        ("HOME", Some(tmp.path().to_str().unwrap())),
        ("GITLAB_TOKEN", None),
        ("AIDA_GITLAB_TOKEN", None),
    ]);

    // 1. Fresh-add branch (remote_create.rs:2174) with embedded oauth2:<secret>@ URL.
    let fresh_secret = "glpat-fresh-add-secret-111";
    let fresh_url = format!("https://oauth2:{fresh_secret}@gitlab.example.test/ai/fresh.git");
    crate::remote_create::run_remote_mirror(
        &repo,
        "gitlab",
        Some(&fresh_url),
        crate::config_commit::CommitMode::Never,
    )
    .expect("fresh-add run_remote_mirror should succeed");

    let stored_fresh_url = String::from_utf8(
        std::process::Command::new(&real_git)
            .arg("-C")
            .arg(&repo)
            .args(["remote", "get-url", "gitlab"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(
        stored_fresh_url.trim(),
        "https://gitlab.example.test/ai/fresh.git",
        "fresh-add must store clean URL without credentials in .git/config"
    );

    // 2. Re-run branch (remote_create.rs:2133) where .git/config previously had credentials.
    let rerun_secret = "glpat-rerun-existing-secret-222";
    let dirty_existing_url =
        format!("https://oauth2:{rerun_secret}@gitlab.example.test/ai/existing.git");
    assert!(std::process::Command::new(&real_git)
        .arg("-C")
        .arg(&repo)
        .args(["remote", "add", "gitlab-rerun", &dirty_existing_url])
        .status()
        .unwrap()
        .success());
    crate::remote_create::run_remote_mirror(
        &repo,
        "gitlab-rerun",
        Some("https://gitlab.example.test/ai/existing.git"),
        crate::config_commit::CommitMode::Never,
    )
    .expect("re-run run_remote_mirror should succeed");

    let stored_rerun_url = String::from_utf8(
        std::process::Command::new(&real_git)
            .arg("-C")
            .arg(&repo)
            .args(["remote", "get-url", "gitlab-rerun"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(
        stored_rerun_url.trim(),
        "https://gitlab.example.test/ai/existing.git",
        "re-run must scrub embedded credentials from .git/config"
    );

    let all_git_argv = fs::read_to_string(&rec_git_argv).unwrap_or_default();
    assert!(
        !all_git_argv.contains(fresh_secret),
        "fresh-add secret must never appear in any git argv:\n{all_git_argv}"
    );
    assert!(
        !all_git_argv.contains(rerun_secret),
        "re-run secret must never appear in any git argv:\n{all_git_argv}"
    );
    assert!(
        all_git_argv.contains("ls-remote --heads gitlab\n"),
        "fresh-add must invoke `git ls-remote --heads gitlab` (remote name, not URL):\n{all_git_argv}"
    );
    assert!(
        all_git_argv.contains("ls-remote --heads gitlab-rerun\n"),
        "re-run must invoke `git ls-remote --heads gitlab-rerun` (remote name, not URL):\n{all_git_argv}"
    );
}
