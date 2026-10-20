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
    const SECRET: &str = "glpat-failure-secret-998877";
    // The fake curl echoes its stdin (the PRIVATE-TOKEN header) to stderr,
    // so inheriting curl's stderr or logging the header on failure puts the
    // token in this child's output. trace:BUG-2119 | ai:claude
    if let Some(out) = crate::test_env::run_path_isolated_test_output() {
        for (stream, bytes) in [("stdout", &out.stdout), ("stderr", &out.stderr)] {
            assert!(
                !String::from_utf8_lossy(bytes).contains(SECRET),
                "the token reached the process {stream} on a curl failure"
            );
        }
        return;
    }
    let tmp = TempDir::new().unwrap();
    let bin = init_bin_dir(&tmp);
    let curl_bin = bin.join("curl");
    let rec_argv = tmp.path().join("rec_argv.txt");

    let old_path = std::env::var("PATH").unwrap_or_default();
    let new_path = format!("{}:{}", bin.display(), old_path);
    let secret = SECRET;

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
         cat >&2\n\
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
         cat >&2\n\
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
    // When .git/config holds the credentialed URL from an earlier setup,
    // configure_git_remote_credentials rewrites it to the clean URL for this
    // invocation only and supplies the token through a host-scoped helper,
    // leaving the stored URL (the operator's only credential) untouched.
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
    let other_host_file = tmp.path().join("git_credential_fill_other.txt");
    let inherited_file = tmp.path().join("inherited_config.txt");
    let fake_remote_https = exec_dir.join("git-remote-https");
    let helper_script = format!(
        "#!/bin/sh\n\
         for a in \"$@\"; do\n\
             printf '%s\\n' \"$a\" >> '{}'\n\
         done\n\
         printf 'protocol=https\\nhost=gitlab.example.test\\n\\n' | git credential fill > '{}'\n\
         printf 'protocol=https\\nhost=other.example.test\\n\\n' | GIT_TERMINAL_PROMPT=0 git credential fill > '{}' 2>/dev/null\n\
         git config --get aida.inherited > '{}'\n\
         while IFS= read -r line; do\n\
             case \"$line\" in\n\
                 capabilities*) printf 'list\\n\\n' ;;\n\
                 list*) printf '\\n' ; break ;;\n\
                 '') break ;;\n\
             esac\n\
         done\n\
         exit 0\n",
        helper_argv_file.display(),
        cred_out_file.display(),
        other_host_file.display(),
        inherited_file.display()
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
        // A parent-supplied GIT_CONFIG_* pair (CI's safe.directory, say)
        // must survive the pairs aida appends. trace:BUG-2119 | ai:claude
        ("GIT_CONFIG_KEY_0", Some("aida.inherited")),
        ("GIT_CONFIG_VALUE_0", Some("kept")),
        ("GIT_CONFIG_COUNT", Some("1")),
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

    // trace:BUG-2119 | ai:claude
    let other_host = fs::read_to_string(&other_host_file).unwrap_or_default();
    assert!(
        !other_host.contains(secret),
        "the helper must answer only for the remote's host:\n{other_host}"
    );
    assert_eq!(
        fs::read_to_string(&inherited_file)
            .unwrap_or_default()
            .trim(),
        "kept",
        "an inherited GIT_CONFIG_* pair must not be overwritten"
    );
    let stored = std::process::Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["config", "--get", "remote.my-gitlab-remote.url"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&stored.stdout).trim(),
        secret_url,
        "a probe must never rewrite the stored remote URL"
    );
}

#[cfg(unix)]
#[test]
fn run_remote_mirror_fresh_add_and_rerun_keep_stored_credentials_out_of_probe_argv() {
    // trace:BUG-1994 | ai:claude
    // trace:BUG-2119 | ai:antigravity
    // trace:BUG-2119 | ai:claude
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
        fresh_url,
        "fresh-add must keep the operator's credential, not discard it"
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
        dirty_existing_url,
        "a re-run must never rewrite the stored URL"
    );

    let all_git_argv = fs::read_to_string(&rec_git_argv).unwrap_or_default();
    // Only `git remote add` — which carries no more than aida's own `--url`
    // argv — sees the fresh secret; no probe, fetch or push does.
    for line in all_git_argv.lines().filter(|l| l.contains(fresh_secret)) {
        assert!(
            line.contains(" remote add gitlab "),
            "fresh-add secret must reach only `git remote add`:\n{all_git_argv}"
        );
    }
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

// trace:BUG-2119 | ai:claude
#[test]
fn split_http_url_userinfo_takes_only_a_password_as_a_credential() {
    use crate::gitlab_project_create::split_http_url_userinfo as split;
    for url in [
        "https://joe@gitlab.com/x.git",
        "https://joe:@gitlab.com/x.git",
        "https://gitlab.com/x.git",
        "ssh://git:pw@gitlab.com/x.git",
    ] {
        assert_eq!(split(url), (url.to_string(), None), "{url}");
    }
    assert_eq!(
        split("https://oauth2:p%40ss%2Fw%3Ard@gitlab.com/x.git"),
        (
            "https://gitlab.com/x.git".to_string(),
            Some(("oauth2".to_string(), "p@ss/w:rd".to_string()))
        )
    );
    assert_eq!(
        split("https://j%2Boe:tok@gitlab.com:8443/x.git"),
        (
            "https://gitlab.com:8443/x.git".to_string(),
            Some(("j+oe".to_string(), "tok".to_string()))
        )
    );
    assert_eq!(
        split("https://:tok@gitlab.com/x.git").1,
        Some(("oauth2".to_string(), "tok".to_string()))
    );
}

fn command_envs(cmd: &std::process::Command) -> Vec<(String, String)> {
    cmd.get_envs()
        .filter_map(|(k, v)| Some((k.to_str()?.to_string(), v?.to_str()?.to_string())))
        .collect()
}

// trace:BUG-2119 | ai:claude
#[test]
fn credentials_scope_to_the_host_and_skip_the_url_override_for_a_clean_remote() {
    // Serialise against the GIT_CONFIG_COUNT swap below: the git runs here
    // read it.
    let _guard = crate::test_env::EnvVarsGuard::apply(&[("GIT_CONFIG_COUNT", None)]);
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    for args in [
        &["init", "-q"][..],
        &[
            "remote",
            "add",
            "gitlab",
            "https://gitlab.example.test:8443/ai/x.git",
        ],
    ] {
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .status()
            .unwrap()
            .success());
    }
    let mut cmd = std::process::Command::new("git");
    crate::gitlab_project_create::configure_git_remote_credentials(
        &mut cmd,
        repo,
        "gitlab",
        Some("https://oauth2:hint-tok@gitlab.example.test:8443/ai/x.git"),
    );
    let envs = command_envs(&cmd);
    let values: Vec<&str> = envs
        .iter()
        .filter(|(k, _)| k.starts_with("GIT_CONFIG_KEY_"))
        .map(|(_, v)| v.as_str())
        .collect();
    assert!(
        values
            .iter()
            .all(|k| *k == "credential.https://gitlab.example.test:8443.helper"),
        "a clean stored URL needs only the host-scoped helper: {values:?}"
    );
    assert_eq!(values.len(), 2, "{values:?}");
    assert!(envs.contains(&("AIDA_GIT_CREDENTIAL_TOKEN".into(), "hint-tok".into())));
}

// trace:BUG-2119 | ai:claude
#[test]
fn a_username_only_url_falls_back_to_the_gitlab_token() {
    // No operator credential helper may answer for the host here.
    let _guard = crate::test_env::EnvVarsGuard::apply(&[
        ("GITLAB_TOKEN", Some("env-tok")),
        ("AIDA_GITLAB_TOKEN", None),
        ("GIT_CONFIG_GLOBAL", Some("/dev/null")),
        ("GIT_CONFIG_NOSYSTEM", Some("1")),
    ]);
    let mut cmd = std::process::Command::new("git");
    crate::gitlab_project_create::configure_git_remote_credentials(
        &mut cmd,
        std::path::Path::new("/nonexistent"),
        "gitlab",
        Some("https://joe@gitlab.com/ai/x.git"),
    );
    let envs = command_envs(&cmd);
    assert!(
        envs.contains(&("AIDA_GIT_CREDENTIAL_TOKEN".into(), "env-tok".into())),
        "a username is not a token: {envs:?}"
    );
}

// trace:BUG-2119 | ai:claude
#[test]
fn git_config_pairs_append_after_inherited_and_explicit_counts() {
    // The count is process-wide: give it the pairs it promises, or a git that a
    // parallel test spawns meanwhile dies on "missing config key
    // GIT_CONFIG_KEY_0". trace:TASK-1979 | ai:claude
    let _guard = crate::test_env::EnvVarsGuard::apply(&[
        ("GIT_CONFIG_KEY_0", Some("aida.inherited0")),
        ("GIT_CONFIG_VALUE_0", Some("kept")),
        ("GIT_CONFIG_KEY_1", Some("aida.inherited1")),
        ("GIT_CONFIG_VALUE_1", Some("kept")),
        ("GIT_CONFIG_COUNT", Some("2")),
    ]);
    let mut inherited = std::process::Command::new("git");
    crate::gitlab_project_create::configure_git_remote_credentials(
        &mut inherited,
        std::path::Path::new("/nonexistent"),
        "gitlab",
        Some("https://oauth2:t@gitlab.com/x.git"),
    );
    let envs = command_envs(&inherited);
    assert!(
        envs.contains(&("GIT_CONFIG_COUNT".into(), "4".into())),
        "{envs:?}"
    );
    assert!(
        envs.iter().any(|(k, _)| k == "GIT_CONFIG_KEY_2"),
        "{envs:?}"
    );
    assert!(
        !envs.iter().any(|(k, _)| k == "GIT_CONFIG_KEY_0"),
        "{envs:?}"
    );

    let mut explicit = std::process::Command::new("git");
    explicit.env("GIT_CONFIG_COUNT", "5");
    crate::gitlab_project_create::configure_git_remote_credentials(
        &mut explicit,
        std::path::Path::new("/nonexistent"),
        "gitlab",
        Some("https://oauth2:t@gitlab.com/x.git"),
    );
    let envs = command_envs(&explicit);
    assert!(
        envs.contains(&("GIT_CONFIG_COUNT".into(), "7".into())),
        "{envs:?}"
    );
}

// trace:BUG-2119 | ai:claude
#[test]
fn percent_decode_keeps_an_escape_without_two_hex_digits() {
    use crate::gitlab_project_create::percent_decode;
    assert_eq!(percent_decode("a%+1b"), "a%+1b");
    assert_eq!(percent_decode("a%-1b%4"), "a%-1b%4");
    assert_eq!(percent_decode("%zz%41%2f"), "%zzA/");
}

/// With no token in the remote URL, an operator's credential helper for the
/// host must stay in charge; only a host with no helper gets the GitLab
/// fallback token.
// trace:BUG-2119 | ai:claude
#[cfg(unix)]
#[test]
fn gitlab_fallback_token_never_overrides_an_operator_credential_helper() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = TempDir::new().unwrap();
    let git = |dir: &std::path::Path, args: &[&str]| {
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .status()
            .unwrap()
            .success());
    };
    let clean_url = "https://gitlab.example.test/ai/x.git";
    let with_helper = tmp.path().join("with-helper");
    let without_helper = tmp.path().join("without-helper");
    for repo in [&with_helper, &without_helper] {
        fs::create_dir_all(repo).unwrap();
        git(repo, &["init", "-q"]);
        git(repo, &["remote", "add", "gitlab", clean_url]);
    }
    git(
        &with_helper,
        &[
            "config",
            "credential.helper",
            "!f() { echo username=operator; echo password=PAT-X; }; f",
        ],
    );

    // A fake git-remote-https that asks git for this host's credential.
    let exec_dir = tmp.path().join("git-exec");
    fs::create_dir_all(&exec_dir).unwrap();
    let fill_out = tmp.path().join("fill.txt");
    crate::test_exec::write_executable(
        &exec_dir.join("git-remote-https"),
        format!(
            "#!/bin/sh\n\
             printf 'protocol=https\\nhost=gitlab.example.test\\n\\n' | GIT_TERMINAL_PROMPT=0 git credential fill > '{}' 2>/dev/null\n\
             while IFS= read -r line; do\n\
                 case \"$line\" in\n\
                     capabilities*) printf 'list\\n\\n' ;;\n\
                     *) printf '\\n' ; break ;;\n\
                 esac\n\
             done\n\
             exit 0\n",
            fill_out.display()
        ),
    );
    let real_exec_path = String::from_utf8(
        std::process::Command::new("git")
            .arg("--exec-path")
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let new_path = format!(
        "{}:{}:{}",
        exec_dir.display(),
        real_exec_path.trim(),
        std::env::var("PATH").unwrap_or_default()
    );
    let _guard = crate::test_env::EnvVarsGuard::apply(&[
        ("GIT_EXEC_PATH", Some(exec_dir.to_str().unwrap())),
        ("PATH", Some(&new_path)),
        ("GITLAB_TOKEN", Some("FALLBACK-Y")),
        ("AIDA_GITLAB_TOKEN", None),
    ]);

    let fill_for = |repo: &std::path::Path| {
        let _ = fs::remove_file(&fill_out);
        let out = crate::gitlab_project_create::ls_remote_probe(repo, "gitlab")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        fs::read_to_string(&fill_out).unwrap_or_default()
    };
    let operator = fill_for(&with_helper);
    assert!(
        operator.contains("password=PAT-X") && !operator.contains("FALLBACK-Y"),
        "the operator's helper must answer, untouched:\n{operator}"
    );
    let fallback = fill_for(&without_helper);
    assert!(
        fallback.contains("password=FALLBACK-Y"),
        "with no operator helper, aida's helper supplies the GitLab token:\n{fallback}"
    );
}
