// BUG-1520: `aida list --fields` behaved three different ways across the three
// output formats — applied+validated on human and TOON, silently ignored with
// exit code 0 on json, which is the one surface machine consumers read. And the
// json projection spelled the type key `req_type` while `--fields`, the human
// header and the TOON header all spell it `type`, so a consumer asking for the
// documented field and reading the documented key missed twice over.
//
// The test drives all THREE formats for both a bogus field and a valid
// narrowing, so a future format cannot be added that skips the check —
// criterion 4 exists because the defect was a format branch that forgot one.
// Binary-driving e2e, sharing the Linux-only gate the sibling real-CLI suites
// use (see bug_1289_format_json.rs, the same family on different verbs).
// trace:BUG-1520 | ai:claude
#![cfg(target_os = "linux")]

use std::path::Path;
use std::process::{Command, Output};

const FORMATS: [&str; 3] = ["human", "toon", "json"];

fn aida(repo: &Path, home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aida"))
        .current_dir(repo)
        .env("HOME", home)
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_SESSION_ROLE", "advisor")
        .env_remove("AIDA_AGENT_OUTPUT")
        .env_remove("AIDA_OUTPUT_FORMAT")
        .args(args)
        .output()
        .expect("run aida")
}

fn git(repo: &Path, args: &[&str]) {
    assert!(Command::new("git")
        .current_dir(repo)
        .args(args)
        .status()
        .expect("run git")
        .success());
}

fn init_repo() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let base = tmp.path().canonicalize().expect("canonical tempdir");
    let repo = base.join("repo");
    let home = base.join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "BUG-1520 Test"]);
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
        "init: {}",
        String::from_utf8_lossy(&init.stderr)
    );
    let add = aida(
        &repo,
        &home,
        &[
            "add",
            "--title",
            "BUG-1520 fixture",
            "--type",
            "task",
            "--status",
            "approved",
        ],
    );
    assert!(
        add.status.success(),
        "add: {}",
        String::from_utf8_lossy(&add.stderr)
    );
    (tmp, repo, home)
}

/// Criterion 1: a bogus `--fields` entry is refused on EVERY format. Before the
/// fix, json returned full rows with exit code 0 — the silent case.
// trace:BUG-1520 | ai:claude
#[test]
fn bogus_field_is_refused_on_every_format() {
    let (_tmp, repo, home) = init_repo();
    for format in FORMATS {
        let out = aida(
            &repo,
            &home,
            &["list", "--fields", "id,zzz", "--format", format],
        );
        assert!(
            !out.status.success(),
            "`--format {format}` accepted a bogus --fields entry (exit {:?})\n\
             --- stdout ---\n{}\n--- stderr ---\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
        // The message must NAME the offending entry and the valid set, on
        // whichever channel that format uses (human: stderr, agent: stdout).
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            combined.contains("zzz") && combined.contains("valid fields"),
            "`--format {format}` refused without naming the entry and the valid set: {combined}"
        );
    }
}

/// Criterion 2: a valid `--fields` NARROWS every format, json included.
// trace:BUG-1520 | ai:claude
#[test]
fn valid_fields_narrow_every_format() {
    let (_tmp, repo, home) = init_repo();
    for format in FORMATS {
        let out = aida(
            &repo,
            &home,
            &["list", "--fields", "id,status", "--format", format],
        );
        assert!(
            out.status.success(),
            "`--format {format}` rejected a valid --fields: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        // `title` is in every format's default projection and in none of the
        // narrowed ones, so its ABSENCE is what proves narrowing happened.
        // Asserting only that id/status are present would pass on an ignored
        // flag, which is exactly the bug.
        assert!(
            !stdout.contains("BUG-1520 fixture"),
            "`--format {format}` ignored --fields — the title is still present:\n{stdout}"
        );
    }
}

/// Criterion 3: the json rows carry BOTH `req_type` and `type`, with the same
/// value. A rename would silently break any consumer written against
/// `req_type`, which is the failure STORY-1352's monitor contract exists to
/// prevent — so this asserts retention, not replacement.
// trace:BUG-1520 | ai:claude
#[test]
fn json_rows_carry_both_type_spellings() {
    let (_tmp, repo, home) = init_repo();

    let full = aida(&repo, &home, &["list", "--format", "json"]);
    assert!(full.status.success());
    let rows: serde_json::Value = serde_json::from_slice(&full.stdout).expect("parseable json");
    let row = rows
        .as_array()
        .and_then(|a| a.first())
        .expect("at least one row");
    let full_req_type = row
        .get("req_type")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("req_type must be RETAINED, not renamed: {row}"))
        .to_string();
    assert_eq!(
        row.get("type").and_then(|v| v.as_str()),
        Some(full_req_type.as_str()),
        "`type` — the name --fields and both other headers use — must be present \
         and carry the SAME value as req_type: {row}"
    );

    // And the narrowed projection keeps both, so a narrowed row's keys stay a
    // subset of the full row's rather than a different vocabulary.
    let narrowed = aida(
        &repo,
        &home,
        &["list", "--fields", "id,type", "--format", "json"],
    );
    assert!(narrowed.status.success());
    let rows: serde_json::Value =
        serde_json::from_slice(&narrowed.stdout).expect("parseable narrowed json");
    let row = rows
        .as_array()
        .and_then(|a| a.first())
        .expect("at least one narrowed row");
    // The load-bearing assertion: a narrowed row reuses the FULL row's values.
    // Rendering through TOON's display tokens would lowercase this only when
    // narrowed, so `--fields` would silently change data — a sharper version of
    // the defect being fixed.
    assert_eq!(
        row.get("req_type").and_then(|v| v.as_str()),
        Some(full_req_type.as_str()),
        "narrowing must not change the value: {row}"
    );
    assert_eq!(
        row.get("type").and_then(|v| v.as_str()),
        Some(full_req_type.as_str()),
        "both spellings survive narrowing: {row}"
    );
    assert!(
        row.get("title").is_none(),
        "the narrowed row must not carry unrequested keys: {row}"
    );
}
