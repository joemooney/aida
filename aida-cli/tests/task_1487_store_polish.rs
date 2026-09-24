#![cfg(target_os = "linux")]
//! Black-box coverage for TASK-1487 (the TASK-1486 review follow-ups), all
//! under a fake HOME and temp directories:
//! - `init --refresh` prints the cause when auto-attaching the store
//!   worktree fails, instead of silently dropping it;
//! - `--file <dir>` combined with a tracker command or `mcp-serve` reaches
//!   the distributed-store dispatch instead of exiting "not yet supported".
// trace:TASK-1487 | ai:claude

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Fixture {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    root: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let root = tmp.path().to_path_buf();
    Fixture {
        _tmp: tmp,
        home,
        root,
    }
}

fn base_cmd(f: &Fixture, dir: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
    cmd.current_dir(dir)
        .env("HOME", &f.home)
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_OUTPUT_FORMAT", "human")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .env_remove("AIDA_STORE")
        .env_remove("REQ_DB_NAME")
        .env_remove("AIDA_REGISTRY_PATH")
        .env_remove("REQ_REGISTRY_PATH")
        .env_remove("AIDA_AGENT_OUTPUT")
        // Never let a tracker command reach this dev machine's own
        // ~/.config/aida/{jira,github,gitlab}.toml.
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("AIDA_JIRA_TOKEN")
        .env_remove("AIDA_GITHUB_TOKEN")
        .env_remove("AIDA_GITLAB_TOKEN");
    cmd
}

fn aida_in(f: &Fixture, dir: &Path, args: &[&str]) -> Output {
    base_cmd(f, dir)
        .stdin(Stdio::null())
        .args(args)
        .output()
        .expect("run aida")
}

fn text(out: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A distributed project (`.aida/config.toml` declares it) with a LOCAL
/// `aida-store` branch that exists but has no reachable `origin` remote —
/// `branch_exists_anywhere` sees the local branch (true), so auto-attach is
/// attempted, but `try_attach_store_worktree`'s `git fetch origin
/// aida-store:aida-store` fails immediately and deterministically (no
/// `origin` configured at all), with no network access required. This is the
/// offline-failure shape item 1 targets.
fn distributed_project_with_unreachable_store_branch(f: &Fixture) -> PathBuf {
    let proj = f.root.join("proj");
    std::fs::create_dir_all(proj.join(".aida")).unwrap();
    git(&proj, &["init", "-q"]);
    std::fs::write(proj.join(".aida/config.toml"), "mode = \"distributed\"\n").unwrap();
    std::fs::write(proj.join("README.md"), "hi\n").unwrap();
    git(&proj, &["add", "."]);
    git(&proj, &["commit", "-q", "-m", "init"]);
    git(&proj, &["branch", "aida-store"]);
    proj
}

/// Item 1: `init --refresh`'s auto-attach retry used to drop the underlying
/// error in its `_ =>` arm — the user saw "type protocols not seeded"
/// with no clue why. It must now name the cause, the same way the main
/// store resolver does for the identical auto-attach failure.
// trace:TASK-1487 | ai:claude
#[test]
fn init_refresh_names_the_auto_attach_failure_cause() {
    let f = fixture();
    let proj = distributed_project_with_unreachable_store_branch(&f);

    let out = aida_in(&f, &proj, &["init", "--refresh"]);
    assert!(out.status.success(), "{}", text(&out));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("couldn't auto-attach the store worktree"),
        "{}",
        text(&out)
    );
    // The cause must actually be printed, not swallowed — the underlying
    // `git fetch` failure names the remote it couldn't reach.
    assert!(
        err.contains("fetch") || err.contains("origin"),
        "cause was dropped: {}",
        text(&out)
    );
    assert!(!err.contains("TASK-"), "no internal ids: {}", text(&out));
}

/// A git-canonical store directory that doesn't exist yet — `--file <dir>`
/// with a nonexistent, extension-less path routes through the distributed
/// dispatch exactly like an existing directory does (`GitBackend::new`
/// creates it). Kept separate from any git repo so these tests need no
/// network and touch nothing outside the fixture's temp dir.
fn store_dir(f: &Fixture, name: &str) -> PathBuf {
    f.root.join(name)
}

/// Item 3: `--file <dir>` combined with a tracker command used to go
/// straight to `handle_git_backend_command`, which has no arms for
/// jira/github/gitlab and fell into its "not yet supported" catch-all. It
/// must now route through `run_on_distributed_store` and reach the
/// tracker's own (missing-config) error instead.
// trace:TASK-1487 | ai:claude
#[test]
fn explicit_file_dir_routes_tracker_commands_past_not_yet_supported() {
    let f = fixture();
    let cases: &[(&str, &[&str], &str)] = &[
        ("jira", &["list"], "aida jira config"),
        ("github", &["list"], "aida github config"),
        ("gitlab", &["list"], "aida gitlab config"),
    ];
    for (tracker, sub, config_hint) in cases {
        let dir = store_dir(&f, tracker);
        let mut args: Vec<&str> = vec!["--file", dir.to_str().unwrap(), tracker];
        args.extend_from_slice(sub);
        let out = aida_in(&f, &f.root, &args);
        let combined = text(&out);
        assert!(
            !combined.contains("not yet supported"),
            "{tracker}: {combined}"
        );
        assert!(
            combined.contains(config_hint),
            "{tracker} must reach its own tracker-config guidance: {combined}"
        );
    }
}

/// Item 3, mcp-serve half: `--file <dir> mcp-serve` must reach
/// `mcp::run_mcp_server` (which prints its startup banner to stderr and, on
/// stdin EOF, exits cleanly) instead of "not yet supported". Stdin is closed
/// so the server sees immediate EOF and returns right away — no protocol
/// exchange needed to prove the routing fix.
// trace:TASK-1487 | ai:claude
#[test]
fn explicit_file_dir_routes_mcp_serve_past_not_yet_supported() {
    let f = fixture();
    let dir = store_dir(&f, "mcpstore");
    let out = base_cmd(&f, &f.root)
        .stdin(Stdio::null())
        .args(["--file", dir.to_str().unwrap(), "mcp-serve"])
        .output()
        .expect("run aida mcp-serve");
    let combined = text(&out);
    assert!(!combined.contains("not yet supported"), "{combined}");
    assert!(
        combined.contains("AIDA MCP server started"),
        "must reach run_mcp_server: {combined}"
    );
}

/// The strict review's regression catch: `--file`/`AIDA_STORE` conventionally
/// point at the STORE directory (`<repo>/.aida-store`), not the project root.
/// `aida --file <repo>/.aida-store mcp-serve` must resolve its project root
/// to `<repo>` — observable through the `role_list` tool, whose response
/// text echoes `self.project_root.display()` verbatim (`"(no roles defined
/// for <project_root>) — …"` when none exist, which is the case here).
// trace:TASK-1487 | ai:claude
#[test]
fn explicit_file_aida_store_dir_resolves_mcp_serve_project_root_to_its_parent() {
    let f = fixture();
    let proj = f.root.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let store = proj.join(".aida-store");

    let mut child = base_cmd(&f, &f.root)
        .args(["--file", store.to_str().unwrap(), "mcp-serve"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn aida mcp-serve");

    {
        let stdin = child.stdin.as_mut().expect("stdin");
        writeln!(
            stdin,
            r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"role_list","arguments":{{}}}}}}"#
        )
        .unwrap();
    }
    // Drop stdin to send EOF right after the one request — the server's
    // read loop (`for line in stdin.lock().lines()`) exits cleanly on EOF.
    drop(child.stdin.take());

    let out = child.wait_with_output().expect("wait for aida mcp-serve");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let response: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!("bad JSON-RPC response: {e}\nstdout: {stdout}\nstderr: {stderr}")
    });
    let role_list_text = response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("no text content in response: {response}\nstderr: {stderr}"));

    let proj_str = proj.to_str().unwrap();
    let store_str = store.to_str().unwrap();
    assert!(
        role_list_text.contains(proj_str),
        "role_list must report the project root ({proj_str}): {role_list_text}"
    );
    assert!(
        !role_list_text.contains(store_str),
        "role_list must not report the store dir itself ({store_str}) as the \
         project root: {role_list_text}"
    );
}
