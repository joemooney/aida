#![cfg(target_os = "linux")]
//! Black-box acceptance for `aida lint constants` (TASK-1785): the built CLI
//! runs against an isolated project whose spec mentions 15 while the traced
//! code uses 15, 40 and an `// arbitrary`-marked 7. Only 40 is reported, the
//! default run exits 0, and only `--fail-on unexplained` turns the finding
//! into a non-zero exit. trace:TASK-1785 | ai:claude

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod support;

const USER: &str = "task1785user";

struct Fixture {
    _tmp: tempfile::TempDir,
    repo: PathBuf,
    home: PathBuf,
    spec: String,
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
    let out = Command::new("git")
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
    git(&repo, &["config", "user.email", "task1785@example.test"]);
    git(&repo, &["config", "user.name", "TASK-1785 Test"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);

    let mut f = Fixture {
        _tmp: tmp,
        repo,
        home,
        spec: String::new(),
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
        &[
            "add",
            "--title",
            "Throttle on io pressure",
            "--type",
            "story",
            "--description",
            "Pause the worker when io-full reaches 15.",
        ],
    );
    f.spec = added
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .find(|t| t.starts_with("STORY-") && t[6..].chars().all(|c| c.is_ascii_digit()))
        .unwrap_or_else(|| panic!("missing story id in {added}"))
        .to_string();

    let src = f.repo.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("lib.rs"),
        format!(
            "// trace:{} | ai:claude\n\
             pub fn throttle(io: u32, cpu: u32) -> bool {{\n\
             \x20   let retries = 7; // arbitrary: retry budget\n\
             \x20   io > 15 || cpu > 40 || retries == 0\n\
             }}\n\
             \n\
             pub fn untraced() -> u32 {{ 99 }}\n",
            f.spec
        ),
    )
    .unwrap();
    f
}

#[test]
fn only_the_unexplained_literal_is_reported() {
    let f = fixture();
    let out = ok(&f, "implementer", &["lint", "constants", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&out).expect("json report");
    let findings = v["findings"].as_array().expect("findings array");
    assert_eq!(findings.len(), 1, "{out}");
    assert_eq!(findings[0]["file"], "src/lib.rs");
    assert_eq!(findings[0]["line"], 4);
    assert_eq!(findings[0]["value"], "40");
    assert_eq!(findings[0]["specs"][0], f.spec.as_str());
    assert_eq!(v["explained"], 1);

    let human = ok(&f, "implementer", &["lint", "constants"]);
    assert!(human.contains("src/lib.rs:4"), "{human}");
    assert!(human.contains(&f.spec), "{human}");
    assert!(
        !human.contains("99"),
        "untraced code must not be scanned: {human}"
    );
}

#[test]
fn nonzero_exit_only_with_fail_on() {
    let f = fixture();
    let out = run(
        &f,
        "implementer",
        &["lint", "constants", "--fail-on", "unexplained"],
    );
    assert_eq!(out.status.code(), Some(1));
    let src = f.repo.join("src/lib.rs");
    let marked = std::fs::read_to_string(&src)
        .unwrap()
        .replace("cpu > 40 ||", "cpu > 40 || // arbitrary\n   ");
    std::fs::write(&src, marked).unwrap();
    ok(
        &f,
        "implementer",
        &["lint", "constants", "--fail-on", "unexplained"],
    );
}

#[test]
fn spec_text_lint_is_unchanged() {
    let f = fixture();
    let out = run(&f, "implementer", &["lint", &f.spec, "--json"]);
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("EARS lint still emits its JSON");
    assert_eq!(v["lens"], "ears");
    assert_eq!(v["specs"][0]["id"], f.spec.as_str());
}
