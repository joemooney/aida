#![cfg(target_os = "linux")]
//! Black-box acceptance for whole-tree `aida trace coverage` (TASK-1783): a
//! fixture crate with one traced and one untraced function reports 1/2 and
//! names the untraced one, in human and JSON output; only `--fail-on
//! uncovered` exits non-zero; the diff-scoped mode keeps its report shape.
//! trace:TASK-1783 | ai:claude

#[path = "support/test_git.rs"]
mod test_git;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod support;

const USER: &str = "task1783user";

struct Fixture {
    _tmp: tempfile::TempDir,
    repo: PathBuf,
    home: PathBuf,
}

fn run(f: &Fixture, role: &str, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
    cmd.current_dir(&f.repo)
        .env("HOME", &f.home)
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_USER", USER)
        .env("AIDA_SESSION_ROLE", role)
        .env_remove("AIDA_AGENT_OUTPUT")
        .env_remove("AIDA_OUTPUT_FORMAT")
        .env_remove("AIDA_STORE");
    if let Some(grant) = support::ensure_seat_for(&f.home, &f.repo, USER, role, &[]) {
        cmd.env("AIDA_SESSION_GRANT", grant);
    }
    cmd.args(args).output().expect("run aida")
}

fn ok(f: &Fixture, role: &str, args: &[&str]) -> String {
    let out = run(f, role, args);
    assert!(
        out.status.success(),
        "aida {args:?} failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn git(repo: &Path, args: &[&str]) {
    let out = test_git::git_command()
        .current_dir(repo)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().expect("tempdir");
    let base = tmp.path().canonicalize().expect("canonical tempdir");
    let repo = base.join("repo");
    let home = base.join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    // trace:BUG-2017 | ai:codex — explicit positive global-ID fixture.
    test_git::enroll_synthetic_home(&repo);
    git(&repo, &["config", "user.email", "task1783@example.test"]);
    git(&repo, &["config", "user.name", "TASK-1783 Test"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);

    let f = Fixture {
        _tmp: tmp,
        repo,
        home,
    };
    ok(
        &f,
        "",
        &[
            "init",
            "--no-skills",
            "--no-hooks",
            "--no-agent-config",
            "--no-roles",
        ],
    );
    let added = ok(
        &f,
        "advisor",
        &["add", "--title", "Traced behaviour", "--type", "story"],
    );
    let spec = added
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .find(|t| t.starts_with("STORY-") && t[6..].chars().all(|c| c.is_ascii_digit()))
        .unwrap_or_else(|| panic!("missing story id in {added}"))
        .to_string();

    let src = f.repo.join("fixture/src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("lib.rs"),
        format!(
            "// trace:{spec} | ai:claude\n\
             pub fn traced() -> u32 {{\n\
             \x20   let x = 2;\n\
             \x20   x + 3\n\
             }}\n\
             \n\
             pub fn untraced() -> u32 {{\n\
             \x20   let y = 4;\n\
             \x20   y + 5\n\
             }}\n"
        ),
    )
    .unwrap();
    git(&f.repo, &["add", "fixture"]);
    git(&f.repo, &["commit", "-q", "-m", "fixture crate"]);
    f
}

// trace:TASK-1783.accb5051 | ai:claude
#[test]
fn fixture_crate_reports_one_of_two_and_names_the_untraced_fn() {
    let f = fixture();
    let out = ok(&f, "implementer", &["trace", "coverage", "--all", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&out).expect("json report");
    assert_eq!(v["mode"], "tree");
    assert_eq!(v["functions"], 2, "{out}");
    assert_eq!(v["traced"], 1, "{out}");
    let file = &v["per_file"][0];
    assert_eq!(file["file"], "fixture/src/lib.rs");
    assert_eq!(file["untraced"][0]["name"], "untraced");
    assert_eq!(file["untraced"][0]["line"], 7);

    let human = ok(
        &f,
        "implementer",
        &["trace", "coverage", "--path", "fixture"],
    );
    assert!(human.contains("fixture/src/lib.rs  1/2"), "{human}");
    assert!(human.contains("fixture/src/lib.rs:7  untraced"), "{human}");
}

// trace:TASK-1783.ac20f5be | ai:claude
#[test]
fn nonzero_exit_only_with_fail_on() {
    let f = fixture();
    let out = run(
        &f,
        "implementer",
        &["trace", "coverage", "--all", "--fail-on", "uncovered"],
    );
    assert_eq!(out.status.code(), Some(1));
    // --fail-on belongs to the whole-tree mode only.
    let out = run(
        &f,
        "implementer",
        &["trace", "coverage", "--fail-on", "uncovered"],
    );
    assert!(!out.status.success());
}

// trace:TASK-1783.ac20f5be | ai:claude
#[test]
fn diff_scoped_mode_is_unchanged() {
    let f = fixture();
    let out = ok(
        &f,
        "implementer",
        &["trace", "coverage", "--range", "HEAD~1..HEAD", "--json"],
    );
    let v: serde_json::Value = serde_json::from_str(&out).expect("diff json");
    assert_eq!(v["range"], "HEAD~1..HEAD");
    assert_eq!(v["posture"], "report");
    assert!(v.get("mode").is_none(), "{out}");
    // One whole-file hunk, covered by the in-hunk trace anchor.
    assert_eq!(v["coverable"], 1, "{out}");
    assert_eq!(v["covered"], 1, "{out}");
    assert_eq!(v["ok"], true);
}
