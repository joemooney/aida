//! BUG-2004: Ensure tokens never appear in child process argv:
//! (1) GitLab release probe (`gitlab_release_exists`) streams `PRIVATE-TOKEN`
//!     via curl stdin (`-H @-`), never on the command line.
//! (2) Mirror re-run probe (`ensure_gitlab_project_exists`) passes the remote
//!     NAME to `git ls-remote`, so raw URLs with embedded credentials are not passed in git's argv.
// trace:BUG-2004 | ai:antigravity

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

#[test]
fn ensure_gitlab_project_exists_passes_remote_name_to_ls_remote() {
    let dir = std::path::Path::new("/test/project");
    let cmd = crate::gitlab_project_create::ls_remote_probe(dir, "my-gitlab-remote");
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
        "ls_remote_probe must pass remote name, not URL"
    );
}
