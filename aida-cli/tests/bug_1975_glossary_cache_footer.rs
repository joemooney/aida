//! BUG-1975: piped `aida docs glossary` (TOON default) must put the `cache:`
//! footer on its own line — the glossary body ends without a newline, and the
//! footer used to be glued onto its last line.
// trace:BUG-1975 | ai:claude

use std::path::Path;
use std::process::{Command, Output};

fn run(cmd: &mut Command) -> Output {
    cmd.output().expect("spawn")
}

fn aida(repo: &Path, home: &Path, args: &[&str]) -> Output {
    run(Command::new(env!("CARGO_BIN_EXE_aida"))
        .args(args)
        .current_dir(repo)
        .env("HOME", home)
        .env("AIDA_TELEMETRY", "0")
        .env("NO_COLOR", "1")
        .env("AIDA_AGENT_OUTPUT", "1")
        .env_remove("AIDA_OUTPUT_FORMAT"))
}

fn git(repo: &Path, args: &[&str]) {
    let out = run(Command::new("git").args(args).current_dir(repo));
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

#[test]
fn piped_glossary_footer_starts_on_its_own_line() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let base = tmp.path().canonicalize().expect("canonical tempdir");
    let (repo, home) = (base.join("repo"), base.join("home"));
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Bug 1975 Test"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let init = aida(
        &repo,
        &home,
        &[
            "init",
            "--force",
            "--no-skills",
            "--no-hooks",
            "--no-agent-config",
            "--no-roles",
        ],
    );
    assert!(init.status.success(), "aida init: {init:?}");

    let out = aida(&repo, &home, &["docs", "glossary"]);
    assert!(out.status.success(), "aida docs glossary: {out:?}");
    let stdout = String::from_utf8(out.stdout).expect("utf8 stdout");
    assert!(stdout.ends_with('\n'), "{stdout:?}");

    // The body is exactly the human rendering; whatever follows it must be the
    // footer, starting on its own line with no blank line in between.
    let human = aida(&repo, &home, &["docs", "glossary", "--format", "human"]);
    let body = String::from_utf8(human.stdout).expect("utf8 human stdout");
    assert!(!body.is_empty() && stdout.starts_with(&body), "{stdout:?}");
    let rest = &stdout[body.len()..];
    if body.ends_with('\n') {
        assert!(rest.is_empty() || rest.starts_with("cache:\n"), "{rest:?}");
    } else {
        assert!(rest == "\n" || rest.starts_with("\ncache:\n"), "{rest:?}");
    }
    assert!(
        stdout.contains("\ncache:\n"),
        "footer must start on its own line: {rest:?}"
    );
    assert!(
        !stdout.contains("\n\ncache:"),
        "no blank line before footer"
    );
}
