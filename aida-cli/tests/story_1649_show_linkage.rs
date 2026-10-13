#![cfg(target_os = "linux")]
//! Black-box acceptance coverage for STORY-1649 against the real binary:
//! the TOON `aida show` surface carries one `linkage:` line naming the
//! shipped PR and merge commit (off under `--no-git`), and
//! `aida edit <ID> --status completed` warns — never refuses — when a code
//! spec reaches Completed with nothing shipped on the default branch, checking
//! both the local default branch and `origin/<default>`. BUG-1958 adds the
//! same warning on `aida done`. BUG-1996 adds the same warning on `aida queue done`.
// trace:STORY-1649 | ai:claude

use std::path::Path;
use std::process::{Command, Output};

mod support;

struct Fixture {
    _tmp: tempfile::TempDir,
    repo: std::path::PathBuf,
    home: std::path::PathBuf,
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn aida_with(fixture: &Fixture, envs: &[(&str, &str)], args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
    cmd.current_dir(&fixture.repo)
        .env("HOME", &fixture.home)
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_SESSION_ROLE", "advisor")
        .env("NO_COLOR", "1")
        .env("AIDA_AGENT_OUTPUT", "0")
        .env_remove("AIDA_OUTPUT_FORMAT")
        .env_remove("AIDA_STORE")
        .env_remove("AIDA_PROJECT_ROOT")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .args(args);
    if let Some(grant) = support::ensure_seat(&fixture.home, &fixture.repo, "advisor", &[]) {
        cmd.env("AIDA_SESSION_GRANT", grant);
    }
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.output().expect("run aida")
}

fn aida(fixture: &Fixture, args: &[&str]) -> Output {
    aida_with(fixture, &[], args)
}

fn ok(label: &str, out: &Output) -> (String, String) {
    assert!(
        out.status.success(),
        "{label} failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn init_fixture() -> Fixture {
    let tmp = tempfile::tempdir().expect("tempdir");
    let base = tmp.path().canonicalize().expect("canonical tempdir");
    let repo = base.join("repo");
    let home = base.join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Linkage Test"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let fixture = Fixture {
        _tmp: tmp,
        repo,
        home,
    };
    ok(
        "aida init",
        &aida(
            &fixture,
            &[
                "init",
                "--force",
                "--no-skills",
                "--no-hooks",
                "--no-agent-config",
                "--no-roles",
            ],
        ),
    );
    fixture
}

/// First `PREFIX-N` token in `aida add` output (`Added: TASK-1 - title`).
fn spec_id_from(stdout: &str) -> String {
    stdout
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .find(|t| {
            t.split_once('-').is_some_and(|(prefix, n)| {
                prefix.len() >= 2
                    && prefix.chars().all(|c| c.is_ascii_uppercase())
                    && !n.is_empty()
                    && n.chars().all(|c| c.is_ascii_digit())
            })
        })
        .unwrap_or_else(|| panic!("missing spec id in {stdout}"))
        .to_string()
}

fn add_spec(fixture: &Fixture, req_type: &str) -> String {
    let (stdout, _) = ok(
        "aida add",
        &aida(
            fixture,
            &[
                "add",
                "--title",
                "linkage fixture",
                "--type",
                req_type,
                "--status",
                "approved",
            ],
        ),
    );
    spec_id_from(&stdout)
}

fn commit_on_main(fixture: &Fixture, file: &str, subject: &str) -> String {
    std::fs::write(fixture.repo.join(file), "work\n").unwrap();
    git(&fixture.repo, &["add", file]);
    git(&fixture.repo, &["commit", "-q", "-m", subject]);
    git(&fixture.repo, &["rev-parse", "--short", "HEAD"])
}

fn toon_show(fixture: &Fixture, args: &[&str]) -> String {
    let mut full = vec!["show"];
    full.extend_from_slice(args);
    let (stdout, _) = ok(
        "aida show (toon)",
        &aida_with(fixture, &[("AIDA_AGENT_OUTPUT", "toon")], &full),
    );
    stdout
}

/// The phrase every completion warning carries; asserting on it (not on a
/// bare `Warning:`) keeps unrelated stderr hints from passing or failing
/// these tests.
const NO_SHIPPED_COMMIT: &str = "no commit referencing it was found";

fn linkage_line(stdout: &str) -> Option<&str> {
    stdout.lines().find(|l| l.starts_with("linkage:"))
}

// ---- AC1 / AC2: the TOON `linkage:` line -----------------------------------

#[test]
fn toon_show_includes_linkage_line() {
    let fx = init_fixture();
    let id = add_spec(&fx, "task");
    let short = commit_on_main(&fx, "work.txt", &format!("feat: ship it ({id}) (#53)"));

    let stdout = toon_show(&fx, &[&id]);
    let line = linkage_line(&stdout).unwrap_or_else(|| panic!("no linkage line in:\n{stdout}"));
    assert!(line.contains("shipped=true"), "{line}");
    assert!(line.contains("shipped_pr=53"), "{line}");
    assert!(line.contains(&format!("commit={short}")), "{line}");
    assert!(line.contains("on=main"), "{line}");
}

#[test]
fn toon_show_unshipped_spec_says_so() {
    let fx = init_fixture();
    let id = add_spec(&fx, "task");
    let stdout = toon_show(&fx, &[&id]);
    let line = linkage_line(&stdout).unwrap_or_else(|| panic!("no linkage line in:\n{stdout}"));
    assert!(line.contains("shipped=false"), "{line}");
    assert!(line.contains("commits=0"), "{line}");
    assert!(!line.contains("shipped_pr"), "{line}");
}

#[test]
fn toon_show_no_git_prints_no_linkage_line() {
    let fx = init_fixture();
    let id = add_spec(&fx, "task");
    commit_on_main(&fx, "work.txt", &format!("feat: ship it ({id}) (#53)"));
    let stdout = toon_show(&fx, &[&id, "--no-git"]);
    assert!(
        linkage_line(&stdout).is_none(),
        "--no-git must suppress the linkage line:\n{stdout}"
    );
    assert!(stdout.contains(&id), "{stdout}");
}

// ---- AC3 / AC4 / A1: `aida edit --status completed` -------------------------

fn complete(fixture: &Fixture, id: &str) -> (String, String) {
    let out = aida(fixture, &["edit", id, "--status", "completed"]);
    ok("aida edit --status completed", &out)
}

#[test]
fn completed_without_shipped_commit_warns() {
    let fx = init_fixture();
    let id = add_spec(&fx, "task");
    let (stdout, stderr) = complete(&fx, &id);
    assert!(
        stdout.contains(&id) || stdout.contains("Updated"),
        "{stdout}"
    );
    assert!(
        stderr.contains(NO_SHIPPED_COMMIT) && stderr.contains(&id),
        "expected a warning naming {id}:\n{stderr}"
    );
    assert!(stderr.contains("doctor:no-code"), "{stderr}");
    assert!(stderr.contains("git fetch"), "{stderr}");
}

#[test]
fn completed_with_shipped_commit_does_not_warn() {
    let fx = init_fixture();
    let id = add_spec(&fx, "task");
    commit_on_main(&fx, "work.txt", &format!("feat: ship it ({id}) (#53)"));
    let (_, stderr) = complete(&fx, &id);
    assert!(
        !stderr.contains(NO_SHIPPED_COMMIT),
        "shipped spec must not warn:\n{stderr}"
    );
}

/// A1: the merge landed on the remote; the local `main` is stale. No warning.
#[test]
fn completed_with_commit_on_origin_only_does_not_warn() {
    let fx = init_fixture();
    let id = add_spec(&fx, "task");
    let origin = fx._tmp.path().join("origin.git");
    git(
        origin.parent().unwrap(),
        &["init", "--bare", "-q", "-b", "main", "origin.git"],
    );
    git(
        &fx.repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&fx.repo, &["push", "-q", "-u", "origin", "main"]);
    commit_on_main(
        &fx,
        "work.txt",
        &format!("feat: merged remotely ({id}) (#12)"),
    );
    git(&fx.repo, &["push", "-q", "origin", "main"]);
    git(&fx.repo, &["reset", "-q", "--hard", "HEAD~1"]);
    assert_eq!(
        git(
            &fx.repo,
            &["log", "main", "--oneline", &format!("--grep={id}")]
        ),
        "",
        "fixture: local main must not carry the commit"
    );

    let (_, stderr) = complete(&fx, &id);
    assert!(
        !stderr.contains(NO_SHIPPED_COMMIT),
        "commit on origin/main only must not warn:\n{stderr}"
    );
    let stdout = toon_show(&fx, &[&id]);
    let line = linkage_line(&stdout).unwrap_or_else(|| panic!("no linkage line in:\n{stdout}"));
    assert!(line.contains("on=origin/main"), "{line}");
}

#[test]
fn completed_doc_spec_does_not_warn() {
    let fx = init_fixture();
    let id = add_spec(&fx, "decision");
    let (_, stderr) = complete(&fx, &id);
    assert!(
        !stderr.contains(NO_SHIPPED_COMMIT),
        "non-code spec must not warn:\n{stderr}"
    );
}

#[test]
fn completed_opted_out_spec_does_not_warn() {
    let fx = init_fixture();
    let id = add_spec(&fx, "task");
    ok(
        "aida edit --add-tag",
        &aida(&fx, &["edit", &id, "--add-tag", "doctor:no-code"]),
    );
    let (_, stderr) = complete(&fx, &id);
    assert!(
        !stderr.contains(NO_SHIPPED_COMMIT),
        "doctor:no-code opt-out must silence the warning:\n{stderr}"
    );
}

// ---- BUG-1958: `aida done` gets the same completion warning -----------------

fn done(fixture: &Fixture, id: &str) -> (String, String) {
    ok("aida done", &aida(fixture, &["done", id]))
}

// trace:BUG-1958 | ai:claude
#[test]
fn done_without_shipped_commit_warns() {
    let fx = init_fixture();
    let id = add_spec(&fx, "task");
    let (_, stderr) = done(&fx, &id);
    assert!(
        stderr.contains(NO_SHIPPED_COMMIT) && stderr.contains(&id),
        "expected `aida done` to warn naming {id}:\n{stderr}"
    );
    assert!(stderr.contains("doctor:no-code"), "{stderr}");
}

// trace:BUG-1958 | ai:claude
#[test]
fn done_opted_out_spec_does_not_warn() {
    let fx = init_fixture();
    let id = add_spec(&fx, "task");
    ok(
        "aida edit --add-tag",
        &aida(&fx, &["edit", &id, "--add-tag", "doctor:no-code"]),
    );
    let (_, stderr) = done(&fx, &id);
    assert!(
        !stderr.contains(NO_SHIPPED_COMMIT),
        "doctor:no-code opt-out must silence the `aida done` warning:\n{stderr}"
    );
}

// trace:BUG-1958 | ai:claude
#[test]
fn done_with_shipped_commit_does_not_warn() {
    let fx = init_fixture();
    let id = add_spec(&fx, "task");
    commit_on_main(&fx, "work.txt", &format!("feat: ship it ({id}) (#53)"));
    let (_, stderr) = done(&fx, &id);
    assert!(
        !stderr.contains(NO_SHIPPED_COMMIT),
        "shipped spec must not warn on `aida done`:\n{stderr}"
    );
}

// ---- BUG-1996: `aida queue done` gets the same completion warning -----------

fn queue_done(fixture: &Fixture, id: &str) -> (String, String) {
    ok(
        "aida queue done",
        &aida(fixture, &["queue", "done", id, "--skip-pr-check", "--yes"]),
    )
}

// trace:BUG-1996 | ai:antigravity
#[test]
fn queue_done_without_shipped_commit_warns() {
    let fx = init_fixture();
    let id = add_spec(&fx, "task");
    let (_, stderr) = queue_done(&fx, &id);
    assert!(
        stderr.contains(NO_SHIPPED_COMMIT) && stderr.contains(&id),
        "expected `aida queue done` to warn naming {id}:\n{stderr}"
    );
    assert!(stderr.contains("doctor:no-code"), "{stderr}");
}

// trace:BUG-1996 | ai:antigravity
#[test]
fn queue_done_opted_out_spec_does_not_warn() {
    let fx = init_fixture();
    let id = add_spec(&fx, "task");
    ok(
        "aida edit --add-tag",
        &aida(&fx, &["edit", &id, "--add-tag", "doctor:no-code"]),
    );
    let (_, stderr) = queue_done(&fx, &id);
    assert!(
        !stderr.contains(NO_SHIPPED_COMMIT),
        "doctor:no-code opt-out must silence the `aida queue done` warning:\n{stderr}"
    );
}

// trace:BUG-1996 | ai:antigravity
#[test]
fn queue_done_with_shipped_commit_does_not_warn() {
    let fx = init_fixture();
    let id = add_spec(&fx, "task");
    commit_on_main(&fx, "work.txt", &format!("feat: ship it ({id}) (#53)"));
    let (_, stderr) = queue_done(&fx, &id);
    assert!(
        !stderr.contains(NO_SHIPPED_COMMIT),
        "shipped spec must not warn on `aida queue done`:\n{stderr}"
    );
}
