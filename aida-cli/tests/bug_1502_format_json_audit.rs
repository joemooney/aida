// End-to-end conformance for the global JSON pin and its checked-in command
// audit. These tests drive the real binary, because dispatch gaps are the
// defect class unit render tests repeatedly missed.
// trace:BUG-1502 | ai:codex
#![cfg(target_os = "linux")]

use std::collections::BTreeSet;
use std::path::Path;
use std::process::{Command, Output};

fn aida(repo: &Path, home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aida"))
        .current_dir(repo)
        .env("HOME", home)
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_SESSION_ROLE", "advisor")
        .args(args)
        .output()
        .expect("run aida")
}

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(repo)
        .args(args)
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?}");
}

fn fixture() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    std::path::PathBuf,
    String,
) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "BUG-1502 Test"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let init = aida(
        &repo,
        &home,
        &[
            "init",
            "--no-skills",
            "--no-hooks",
            "--no-agent-config",
            "--no-roles",
        ],
    );
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );

    let description =
        "quote: \"hello\"\nbackslash: C:\\\\tmp\\file\nblank follows\n\nUnicode: café 東京 🧭";
    let description_path = tmp.path().join("description.txt");
    std::fs::write(&description_path, description).unwrap();
    let add = aida(
        &repo,
        &home,
        &[
            "add",
            "--title",
            "format json fixture",
            "--type",
            "task",
            "--status",
            "approved",
            "--description-from-file",
            description_path.to_str().unwrap(),
        ],
    );
    assert!(
        add.status.success(),
        "{}",
        String::from_utf8_lossy(&add.stderr)
    );
    let stdout = String::from_utf8_lossy(&add.stdout);
    let spec = stdout
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .find(|token| token.starts_with("TASK-") && token[5..].chars().all(|c| c.is_ascii_digit()))
        .unwrap()
        .to_string();
    (tmp, repo, home, spec)
}

fn json(out: &Output, label: &str) -> serde_json::Value {
    assert!(
        out.status.success(),
        "{label}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|error| {
        panic!("{label}: {error}\n{}", String::from_utf8_lossy(&out.stdout))
    })
}

#[test]
fn show_format_json_round_trips_description_and_matches_json_flag() {
    let (_tmp, repo, home, spec) = fixture();
    let via_format = aida(
        &repo,
        &home,
        &["show", &spec, "--no-git", "--format", "json"],
    );
    let via_flag = aida(&repo, &home, &["show", &spec, "--no-git", "--json"]);
    let format_value = json(&via_format, "show --format json");
    let flag_value = json(&via_flag, "show --json");
    assert_eq!(format_value, flag_value);
    assert_eq!(
        format_value["description"],
        "quote: \"hello\"\nbackslash: C:\\\\tmp\\file\nblank follows\n\nUnicode: café 東京 🧭"
    );
    assert!(format_value["git_linkage"].is_null());
}

#[test]
fn show_json_includes_git_linkage_unless_suppressed() {
    let (_tmp, repo, home, spec) = fixture();
    std::fs::write(
        repo.join("trace.rs"),
        format!("// trace:{spec} | ai:test\nfn linked() {{}}\n"),
    )
    .unwrap();
    git(&repo, &["add", "trace.rs"]);
    git(
        &repo,
        &[
            "commit",
            "-q",
            "-m",
            &format!("test: link fixture ({spec})"),
        ],
    );
    let value = json(
        &aida(&repo, &home, &["show", &spec, "--format", "json"]),
        "show linkage",
    );
    assert!(value["git_linkage"]["commits"]
        .as_array()
        .is_some_and(|v| !v.is_empty()));
    assert!(value["git_linkage"]["files"]
        .as_array()
        .is_some_and(|v| !v.is_empty()));
    assert_eq!(value["git_linkage"]["shipped"], true);
}

#[test]
fn unsupported_global_json_pin_errors_before_human_fallback() {
    let (_tmp, repo, home, _spec) = fixture();
    let out = aida(&repo, &home, &["focus", "show", "--format", "json"]);
    assert!(!out.status.success());
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(combined.contains("has no JSON projection"), "{combined}");
    assert!(
        !combined.contains("No focus set"),
        "unsupported JSON fell through to the human renderer: {combined}"
    );
}

#[test]
fn checked_in_audit_is_exhaustive_and_every_honoured_probe_parses() {
    let (_tmp, repo, home, spec) = fixture();
    let catalog = json(
        &aida(&repo, &home, &["help", "commands", "--json", "--hidden"]),
        "command catalog",
    );
    let catalog_paths: BTreeSet<String> = catalog
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            entry["path"]
                .as_array()
                .unwrap()
                .iter()
                .map(|part| part.as_str().unwrap())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    let audit = include_str!("../../docs/cli-format-json-audit.md");
    let audit_paths: BTreeSet<String> = audit
        .lines()
        .filter_map(|line| line.strip_prefix("| `"))
        .filter_map(|line| line.split_once('`').map(|(path, _)| path.to_string()))
        .collect();
    assert_eq!(catalog_paths, audit_paths, "regenerate the BUG-1502 audit");

    let probes: &[(&str, &[&str])] = &[
        ("aida show", &["show", &spec, "--no-git"]),
        ("aida list", &["list", "--limit", "1"]),
        ("aida status", &["status", &spec]),
        ("aida ps", &["ps"]),
        ("aida queue list", &["queue", "list"]),
        ("aida queue progress", &["queue", "progress"]),
        ("aida findings list", &["findings", "list"]),
    ];
    for (path, args) in probes {
        let mut via_format = args.to_vec();
        via_format.extend(["--format", "json"]);
        let format_value = json(&aida(&repo, &home, &via_format), path);
        let mut via_flag = args.to_vec();
        via_flag.push("--json");
        let flag_value = json(&aida(&repo, &home, &via_flag), path);
        assert_eq!(format_value, flag_value, "{path} spellings diverged");
        assert!(audit.contains(&format!("| `{path}` | JSON honoured |")));
    }
}
