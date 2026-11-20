//! TASK-1924: each stored credential is offered only to its own host.
// trace:TASK-1952 | trace:TASK-1924 | ai:claude

use super::*;

/// What `git credential fill` answers for `host` under the configuration
/// `configure_remote_credentials` put on a command for `dir`'s `origin`.
fn fill(dir: &Path, host: &str) -> String {
    let mut cmd = crate::test_git::git_command();
    cmd.current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "/bin/false")
        .env("SSH_ASKPASS", "/bin/false");
    configure_remote_credentials(&mut cmd, dir, "origin");
    let mut child = cmd
        .args(["credential", "fill"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("protocol=https\nhost={host}\n\n").as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn repo_with_origin(urls: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let out = crate::test_git::git_command()
            .current_dir(dir.path())
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    };
    git(&["init", "-q"]);
    for (key, url) in urls {
        git(&["config", &format!("remote.origin.{key}"), url]);
    }
    dir
}

#[cfg(unix)]
#[test]
fn task_1924_a_pushurl_token_on_another_host_is_scoped_to_that_host() {
    let dir = repo_with_origin(&[
        ("url", "https://host-a.example/group/p.git"),
        ("pushurl", "https://u:token-b@host-b.example/group/p.git"),
    ]);
    let b = fill(dir.path(), "host-b.example");
    assert!(
        b.contains("password=token-b"),
        "host B keeps its token: {b}"
    );
    let a = fill(dir.path(), "host-a.example");
    assert!(
        !a.contains("token-b"),
        "host A must never see B's token: {a}"
    );
}

#[cfg(unix)]
#[test]
fn task_1924_each_host_gets_its_own_token() {
    let dir = repo_with_origin(&[
        ("url", "https://ua:token-a@host-a.example/group/p.git"),
        ("pushurl", "https://ub:token-b@host-b.example/group/p.git"),
    ]);
    let a = fill(dir.path(), "host-a.example");
    assert!(
        a.contains("username=ua") && a.contains("password=token-a"),
        "{a}"
    );
    assert!(!a.contains("token-b"), "{a}");
    let b = fill(dir.path(), "host-b.example");
    assert!(
        b.contains("username=ub") && b.contains("password=token-b"),
        "{b}"
    );
    assert!(!b.contains("token-a"), "{b}");
}

/// The `GIT_*` environment `configure_remote_credentials` put on `cmd`.
fn git_env(cmd: &Command) -> Vec<(String, String)> {
    cmd.get_envs()
        .filter_map(|(k, v)| Some((k.to_str()?.to_string(), v?.to_str()?.to_string())))
        .filter(|(k, _)| k.starts_with("GIT_"))
        .collect()
}

// TASK-3792: under a read deadline the stored-URL read is bounded. A read
// the deadline cut off leaves the token's presence unknown, so the command
// may use no transport at all; a read in time rewrites as before.
// trace:TASK-3792 | ai:claude
#[cfg(all(unix, feature = "native"))]
#[test]
fn task_3792_an_unread_stored_url_under_a_deadline_allows_no_transport() {
    use std::time::{Duration, Instant};
    let dir = repo_with_origin(&[("url", "https://u:secret-token@host.example/p.git")]);
    let configure = |deadline: Option<Instant>| {
        let mut cmd = Command::new("git");
        with_config_read_deadline(deadline, || {
            configure_remote_credentials(&mut cmd, dir.path(), "origin")
        });
        git_env(&cmd)
    };
    let expired = configure(Some(Instant::now()));
    assert_eq!(
        expired,
        vec![("GIT_ALLOW_PROTOCOL".to_string(), String::new())],
        "an expired deadline must refuse every transport"
    );
    let in_time = configure(Some(Instant::now() + Duration::from_secs(30)));
    assert_eq!(in_time, configure(None));
    assert!(
        in_time
            .iter()
            .any(|(_, v)| v == "https://u:secret-token@host.example/p.git"),
        "{in_time:?}"
    );
    assert!(!in_time.iter().any(|(k, _)| k == "GIT_ALLOW_PROTOCOL"));
    // The scope restores the previous (absent) deadline.
    assert_eq!(configure(None), in_time);
}
