#![cfg(target_os = "linux")]
//! Black-box acceptance coverage for STORY-1642: knowledge types are
//! reference, not work.
//!
//! Seeds completed FAQs plus one superseded and one archived FAQ, then drives
//! the shipped binary: `aida faq list` (no flags) and `aida list faq` return
//! only the live FAQs with just the id + question columns; the default
//! `aida list` hides them; `--type faq,decision` is read as a knowledge
//! listing.
// trace:STORY-1642 | ai:claude

use std::path::Path;
use std::process::{Command, Output};

mod support;

struct Fixture {
    _tmp: tempfile::TempDir,
    repo: std::path::PathBuf,
    home: std::path::PathBuf,
}

fn git(dir: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run git")
}

fn aida(fixture: &Fixture, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
    cmd.current_dir(&fixture.repo)
        .env("HOME", &fixture.home)
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_SESSION_ROLE", "advisor")
        .env("NO_COLOR", "1")
        .args(args);
    if let Some(grant) = support::ensure_seat(&fixture.home, &fixture.repo, "advisor", &[]) {
        cmd.env("AIDA_SESSION_GRANT", grant);
    }
    cmd.output().expect("run aida")
}

fn ok(label: &str, out: Output) -> String {
    assert!(
        out.status.success(),
        "{label} failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn init_fixture() -> Fixture {
    let tmp = tempfile::tempdir().expect("tempdir");
    let base = tmp.path().canonicalize().expect("canonical tempdir");
    let repo = base.join("repo");
    let home = base.join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    assert!(git(&repo, &["init", "-q", "-b", "main"]).status.success());
    assert!(git(&repo, &["config", "user.email", "test@example.com"])
        .status
        .success());
    assert!(git(&repo, &["config", "user.name", "Knowledge Test"])
        .status
        .success());
    assert!(git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"])
        .status
        .success());
    let fixture = Fixture {
        _tmp: tmp,
        repo,
        home,
    };
    ok(
        "aida init",
        aida(
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

fn id_from(stdout: &str, prefix: &str) -> String {
    stdout
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .find(|t| {
            t.strip_prefix(prefix)
                .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        })
        .unwrap_or_else(|| panic!("missing {prefix} id in {stdout}"))
        .to_string()
}

fn add(fixture: &Fixture, ty: &str, title: &str, prefix: &str) -> String {
    let out = ok(
        "aida add",
        aida(fixture, &["add", "--type", ty, "--title", title]),
    );
    id_from(&out, prefix)
}

fn list_ids(fixture: &Fixture, args: &[&str]) -> Vec<String> {
    let mut argv: Vec<&str> = args.to_vec();
    argv.extend_from_slice(&["--format", "json"]);
    let stdout = ok("aida list --format json", aida(fixture, &argv));
    let rows: serde_json::Value =
        serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("list json: {e}\n{stdout}"));
    let mut ids: Vec<String> = rows
        .as_array()
        .unwrap_or_else(|| panic!("not an array: {stdout}"))
        .iter()
        .map(|r| r["spec_id"].as_str().unwrap_or_default().to_string())
        .collect();
    ids.sort();
    ids
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

#[test]
fn faq_listings_show_only_live_faqs_with_id_and_question() {
    let fx = init_fixture();
    let live_a = add(&fx, "faq", "How do I file a bug?", "FAQ-");
    let live_b = add(&fx, "faq", "How do I list work?", "FAQ-");
    let superseded = add(&fx, "faq", "How did we do it before?", "FAQ-");
    let archived = add(&fx, "faq", "An archived question?", "FAQ-");
    let task = add(&fx, "task", "Real work", "TASK-");

    ok(
        "aida edit --status superseded",
        aida(&fx, &["edit", &superseded, "--status", "superseded"]),
    );
    ok("aida archive", aida(&fx, &["archive", &archived]));

    let live = sorted(vec![live_a.clone(), live_b.clone()]);

    // `aida faq list` with no flags: only the live FAQs.
    assert_eq!(list_ids(&fx, &["faq", "list"]), live);
    // `aida list faq` reads the type word as `--type faq`: same result.
    assert_eq!(list_ids(&fx, &["list", "faq"]), live);

    // The FAQ table is just id + question.
    for argv in [&["faq", "list"][..], &["list", "faq"][..]] {
        let out = ok("faq table", aida(&fx, argv));
        assert!(
            out.contains("specs[2]{id,title}:"),
            "{argv:?} should render only id+title for the 2 live FAQs:\n{out}"
        );
        assert!(!out.contains(&superseded), "{out}");
        assert!(!out.contains(&archived), "{out}");
    }

    // The default list is work only: no FAQ rows, even under --all.
    assert_eq!(list_ids(&fx, &["list"]), vec![task.clone()]);
    assert!(list_ids(&fx, &["list", "--all"])
        .iter()
        .all(|id| !id.starts_with("FAQ-")));

    // A comma type list naming a knowledge type is a knowledge listing.
    let decision = add(&fx, "decision", "We use git as canonical store", "ADR-");
    let mixed = list_ids(&fx, &["list", "--type", "faq,decision"]);
    assert!(
        mixed.contains(&live_a) && mixed.contains(&live_b),
        "{mixed:?}"
    );
    assert!(mixed.contains(&decision), "{mixed:?}");
    assert!(!mixed.contains(&task), "{mixed:?}");
}
