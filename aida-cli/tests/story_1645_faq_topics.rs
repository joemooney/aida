#![cfg(target_os = "linux")]
//! Black-box acceptance coverage for STORY-1645: `aida faq topics` lists the
//! tags in use on live FAQs with a count per tag.
//!
//! Seeds tagged FAQs plus one superseded and one archived FAQ, then drives the
//! shipped binary: counts cover live FAQs only, sorted by count then name; the
//! agent output is an id-less table; an empty store prints a one-line notice;
//! and every printed topic round-trips through `aida list faq --tag <topic>`.
// trace:STORY-1645 | ai:claude

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
    assert!(git(&repo, &["config", "user.name", "Faq Topics Test"])
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

fn add_faq(fixture: &Fixture, title: &str, tags: &str) -> String {
    let out = ok(
        "aida add faq",
        aida(
            fixture,
            &["add", "--type", "faq", "--title", title, "--tags", tags],
        ),
    );
    id_from(&out, "FAQ-")
}

fn topics_json(fixture: &Fixture) -> Vec<(String, u64)> {
    let stdout = ok(
        "aida faq topics --format json",
        aida(fixture, &["faq", "topics", "--format", "json"]),
    );
    let rows: serde_json::Value =
        serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("topics json: {e}\n{stdout}"));
    rows.as_array()
        .unwrap_or_else(|| panic!("not an array: {stdout}"))
        .iter()
        .map(|r| {
            (
                r["tag"].as_str().unwrap_or_default().to_string(),
                r["count"].as_u64().unwrap_or_default(),
            )
        })
        .collect()
}

#[test]
fn faq_topics_counts_live_faqs_only() {
    let fx = init_fixture();

    // Acceptance 3: no FAQ topics yet — a one-line notice, exit 0.
    let empty = ok("aida faq topics (empty)", aida(&fx, &["faq", "topics"]));
    // Agent (TOON) output appends the standard `cache:` footer, so pin the
    // notice line and that nothing but the footer follows it.
    let mut lines = empty.trim().lines();
    assert_eq!(lines.next(), Some("no FAQ topics"), "{empty}");
    assert!(
        lines.all(|l| l == "cache:" || l.trim_start().starts_with("stale:")),
        "{empty}"
    );

    let live_a = add_faq(&fx, "How do I set up?", "setup,cli");
    let live_b = add_faq(&fx, "How do I list work?", "cli");
    let superseded = add_faq(&fx, "How did we do it before?", "legacy,cli");
    let archived = add_faq(&fx, "An archived question?", "attic");
    ok(
        "aida edit --status superseded",
        aida(&fx, &["edit", &superseded, "--status", "superseded"]),
    );
    ok("aida archive", aida(&fx, &["archive", &archived]));

    // Acceptance 1: live FAQs only, count desc then name.
    let topics = topics_json(&fx);
    assert_eq!(
        topics,
        vec![("cli".to_string(), 2), ("setup".to_string(), 1)],
        "superseded/archived FAQ tags must not appear"
    );

    // Acceptance 2: the agent output is a compact table with no spec ids.
    let agent = ok("aida faq topics", aida(&fx, &["faq", "topics"]));
    assert!(agent.contains("topics[2]{tag,count}:"), "{agent}");
    assert!(agent.contains("cli,2"), "{agent}");
    assert!(agent.contains("setup,1"), "{agent}");
    assert!(!agent.contains("FAQ-"), "{agent}");
    for id in [&live_a, &live_b, &superseded, &archived] {
        assert!(!agent.contains(id.as_str()), "{agent}");
    }
    assert!(!agent.contains("legacy"), "{agent}");
    assert!(!agent.contains("attic"), "{agent}");
    // The long spelling is the same surface.
    let long = ok(
        "aida list --type faq --topics",
        aida(&fx, &["list", "--type", "faq", "--topics"]),
    );
    assert_eq!(long, agent);

    // Acceptance 4: every printed topic is accepted verbatim as a filter and
    // returns exactly `count` live FAQs.
    for (tag, count) in &topics {
        let ids = list_ids(&fx, &["list", "faq", "--tag", tag]);
        assert_eq!(ids.len() as u64, *count, "{tag}: {ids:?}");
        assert!(!ids.contains(&superseded), "{tag}: {ids:?}");
    }
}

#[test]
fn topics_flag_requires_the_faq_type() {
    let fx = init_fixture();
    let out = aida(&fx, &["list", "--topics"]);
    assert!(
        !out.status.success(),
        "`aida list --topics` without the FAQ type should fail:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    // Agent mode reports errors on stdout, human mode on stderr: check both.
    let both = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(both.contains("aida faq topics"), "{both}");
}
