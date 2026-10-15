// BUG-688: binary-driving e2e suites are Linux-gated like their siblings.
#![cfg(target_os = "linux")]
//! BUG-1993: the git-linkage scans `aida show` runs are bounded, and a scan
//! that times out renders as incomplete / shipped-unknown, never as "no
//! commits" or "not shipped". A `git` shim on PATH stalls one subprocess so
//! the budget is actually hit.
// trace:BUG-1993 | ai:claude

use std::path::{Path, PathBuf};
use std::process::Command;

mod support;

fn aida(repo: &Path, home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
    cmd.current_dir(repo);
    cmd.env("HOME", home);
    cmd.env("AIDA_HOME", home);
    cmd.env("AIDA_TELEMETRY", "0");
    for var in [
        "AIDA_SESSION_ROLE",
        "AIDA_SESSION_GRANT",
        "AIDA_PERMISSION_MODE",
        "AIDA_AGENT_OUTPUT",
        "AIDA_OUTPUT_FORMAT",
        "AIDA_HEADLESS_VENDOR",
    ] {
        cmd.env_remove(var);
    }
    cmd
}

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(repo)
        .args(args)
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

fn run_ok(cmd: &mut Command) -> String {
    let out = cmd.output().expect("run aida");
    assert!(
        out.status.success(),
        "aida failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Real `git`, resolved before any shim directory is put in front of PATH.
fn real_git() -> PathBuf {
    let path = std::env::var_os("PATH").expect("PATH set");
    std::env::split_paths(&path)
        .map(|dir| dir.join("git"))
        .find(|candidate| candidate.is_file())
        .expect("git on PATH")
}

/// A shim dir whose `git` sleeps when any argument equals `slow_arg`, and
/// otherwise execs the real git. The sleep far exceeds every linkage budget.
fn git_shim(dir: &Path, slow_arg: &str) -> PathBuf {
    let shim_dir = dir.join(format!("shim{slow_arg}"));
    std::fs::create_dir_all(&shim_dir).unwrap();
    let script = format!(
        "#!/bin/sh\nfor a in \"$@\"; do [ \"$a\" = \"{slow_arg}\" ] && sleep 20; done\nexec {} \"$@\"\n",
        real_git().display()
    );
    let path = shim_dir.join("git");
    std::fs::write(&path, script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    shim_dir
}

fn with_shim(cmd: &mut Command, shim_dir: &Path) {
    let path = std::env::var("PATH").unwrap_or_default();
    cmd.env("PATH", format!("{}:{path}", shim_dir.display()));
}

fn show_json(repo: &Path, home: &Path, spec: &str, shim: Option<&Path>) -> serde_json::Value {
    let mut cmd = aida(repo, home);
    cmd.args(["show", spec, "--format", "json"]);
    if let Some(dir) = shim {
        with_shim(&mut cmd, dir);
    }
    let out = run_ok(&mut cmd);
    let v: serde_json::Value = serde_json::from_str(&out).expect("show json");
    v["git_linkage"].clone()
}

fn show_human(repo: &Path, home: &Path, spec: &str, shim: &Path) -> String {
    let mut cmd = aida(repo, home);
    cmd.args(["show", spec, "--format", "human"]);
    with_shim(&mut cmd, shim);
    run_ok(&mut cmd)
}

/// A repo with one shipped commit naming a fresh spec. Returns the spec id.
fn fixture(repo: &Path, home: &Path) -> String {
    std::fs::create_dir_all(repo).unwrap();
    std::fs::create_dir_all(home).unwrap();
    git(repo, &["init", "-q", "-b", "main"]);
    git(repo, &["config", "user.email", "t@t.t"]);
    git(repo, &["config", "user.name", "t"]);
    git(repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    run_ok(aida(repo, home).args([
        "init",
        "--no-skills",
        "--no-hooks",
        "--no-agent-config",
        "--no-roles",
    ]));
    let grant = support::ensure_seat(home, repo, "advisor", &[]).expect("advisor grant");
    let add_out = run_ok(
        aida(repo, home)
            .env("AIDA_SESSION_ROLE", "advisor")
            .env("AIDA_SESSION_GRANT", grant)
            .args(["add", "--type", "bug", "--title", "linkage budget fixture"]),
    );
    let spec = add_out
        .split(|c: char| c.is_whitespace() || c == ',')
        .find(|t| t.starts_with("BUG-"))
        .unwrap_or_else(|| panic!("no spec id in add output:\n{add_out}"))
        .to_string();
    git(
        repo,
        &[
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            &format!("fix: the thing ({spec})"),
        ],
    );
    spec
}

fn notes(linkage: &serde_json::Value) -> String {
    linkage["incomplete"].to_string()
}

#[test]
fn linkage_timeouts_render_unknown_not_unshipped() {
    let base = tempfile::tempdir().expect("tempdir");
    let base_dir = base.path().canonicalize().expect("canonicalize tempdir");
    let repo = base_dir.join("repo");
    let home = base_dir.join("home");
    let spec = fixture(&repo, &home);

    // Baseline: unstalled, the commit is found and shipped, and complete.
    let fast = show_json(&repo, &home, &spec, None);
    assert_eq!(fast["shipped"], true, "baseline shipped: {fast}");
    assert_eq!(fast["complete"], true, "baseline complete: {fast}");

    // The referencing-commit `git log --all --date-order` stalls.
    let slow_log = git_shim(&base_dir, "--date-order");
    let started = std::time::Instant::now();
    let l = show_json(&repo, &home, &spec, Some(&slow_log));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(15),
        "the stalled log is bounded: {:?}",
        started.elapsed()
    );
    assert_eq!(l["complete"], false, "{l}");
    assert!(notes(&l).contains("commit scan incomplete"), "{l}");
    // A timed-out scan is `null`, never a false "not shipped".
    assert!(
        l["shipped"].is_null(),
        "shipped must be null on timeout: {l}"
    );
    let human = show_human(&repo, &home, &spec, &slow_log);
    assert!(human.contains("commit scan incomplete"), "{human}");
    assert!(
        !human.contains("no commits or trace comments reference"),
        "a timed-out scan must not read as no linkage:\n{human}"
    );

    // The `merge-base --is-ancestor` shipped check stalls.
    let slow_merge = git_shim(&base_dir, "--is-ancestor");
    let l = show_json(&repo, &home, &spec, Some(&slow_merge));
    assert_eq!(l["complete"], false, "{l}");
    assert!(notes(&l).contains("shipped state unknown"), "{l}");
    assert!(
        l["shipped"].is_null(),
        "shipped must be null on timeout: {l}"
    );
    assert_eq!(l["commits"].as_array().map(|c| c.len()), Some(1), "{l}");
    let human = show_human(&repo, &home, &spec, &slow_merge);
    assert!(human.contains("shipped state unknown"), "{human}");
    assert!(
        !human.contains("in flight") && !human.contains("not found"),
        "an unknown shipped state must not read as unshipped:\n{human}"
    );
}
