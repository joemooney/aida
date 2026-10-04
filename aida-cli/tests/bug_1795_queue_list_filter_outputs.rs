#![cfg(target_os = "linux")]
//! Black-box acceptance tests for BUG-1795. Each case runs the built AIDA CLI
//! against an isolated project so it exercises the actual human, JSON, and
//! TOON queue-list emitters. A mutation in any emitter's filter application
//! makes that format's empty-batch test fail. trace:BUG-1795 | ai:codex

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

// STORY-1473 / ADR-66: env roles confer no authority; commands ride a
// validated seat grant for their requested role.
mod support;

const USER: &str = "bug1795user";

struct Fixture {
    _tmp: tempfile::TempDir,
    repo: PathBuf,
    home: PathBuf,
    member: String,
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
    // ADR-66: the role env is only a hint; each command rides a validated
    // seat grant for its requested role. trace:STORY-1473 | ai:claude
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

fn mark_store_object_completed(repo: &Path, member: &str) {
    fn visit(dir: &Path, member: &str) -> Option<PathBuf> {
        for entry in std::fs::read_dir(dir).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(found) = visit(&path, member) {
                    return Some(found);
                }
            } else if path.extension().is_some_and(|ext| ext == "yaml") {
                let contents = std::fs::read_to_string(&path).ok()?;
                if contents.contains(member) {
                    return Some(path);
                }
            }
        }
        None
    }

    let object = visit(&repo.join(".aida-store/objects"), member)
        .unwrap_or_else(|| panic!("store object for {member} not found"));
    let contents = std::fs::read_to_string(&object).unwrap();
    let updated = contents.replace("status: Approved", "status: Completed");
    assert_ne!(
        updated,
        contents,
        "approved status not found in {}",
        object.display()
    );
    std::fs::write(object, updated).unwrap();
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().expect("tempdir");
    let base = tmp.path().canonicalize().expect("canonical tempdir");
    let repo = base.join("repo");
    let home = base.join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "bug1795@example.test"]);
    git(&repo, &["config", "user.name", "BUG-1795 Test"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);

    let mut f = Fixture {
        _tmp: tmp,
        repo,
        home,
        member: String::new(),
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
            "BUG-1795 queued fixture",
            "--type",
            "task",
            "--status",
            "approved",
            "--tags",
            "batch:wave-a",
        ],
    );
    f.member = added
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .find(|token| token.starts_with("TASK-") && token[5..].chars().all(|c| c.is_ascii_digit()))
        .unwrap_or_else(|| panic!("missing task id in {added}"))
        .to_string();
    ok(
        &f,
        "advisor",
        &[
            "queue",
            "add",
            &f.member,
            "--user",
            USER,
            "--for",
            "implementer",
            "--no-scope",
        ],
    );
    f
}

fn list_args(format: &str, batch: &str) -> Vec<String> {
    let mut args = vec![
        "queue".into(),
        "list".into(),
        "--format".into(),
        format.into(),
    ];
    if format == "json" {
        // Exercise the cache-fast branch rather than merely the global format
        // selector that shares the same projection.
        args.push("--json".into());
    }
    args.extend(
        ["--user", USER, "--for", "implementer", "--batch", batch]
            .into_iter()
            .map(str::to_string),
    );
    args
}

fn call(f: &Fixture, format: &str, batch: &str, extra: &[&str]) -> Output {
    let mut args = list_args(format, batch);
    args.extend(extra.iter().map(|s| (*s).to_string()));
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run(f, "implementer", &refs)
}

fn assert_empty_batch(f: &Fixture, format: &str) {
    let out = call(f, format, "does-not-exist", &[]);
    assert!(
        out.status.success(),
        "{format} queue list failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    match format {
        "json" => {
            let rows: serde_json::Value = serde_json::from_str(&stdout)
                .unwrap_or_else(|e| panic!("invalid JSON ({e}):\n{stdout}"));
            assert_eq!(rows.as_array().map(Vec::len), Some(0), "{stdout}");
        }
        "toon" => {
            assert!(
                stdout.lines().any(|line| line.starts_with("count: 0 ")),
                "{stdout}"
            );
            assert!(
                !stdout.contains(&f.member),
                "unexpected member row:\n{stdout}"
            );
        }
        "human" => {
            assert!(
                stdout.contains("Your queue")
                    && stdout.contains("no items routed to role implementer"),
                "{stdout}"
            );
            assert!(
                !stdout.contains(&f.member),
                "unexpected member row:\n{stdout}"
            );
        }
        _ => unreachable!(),
    }
}

#[test]
fn human_queue_list_outputs_zero_rows_for_unknown_batch() {
    let f = fixture();
    assert_empty_batch(&f, "human");
    let stdout = String::from_utf8_lossy(&call(&f, "human", "wave-a", &[]).stdout).into_owned();
    assert!(
        stdout.contains(&f.member),
        "expected batch member:\n{stdout}"
    );
}

#[test]
fn json_queue_list_outputs_zero_rows_for_unknown_batch() {
    let f = fixture();
    assert_empty_batch(&f, "json");
    let out = call(&f, "json", "wave-a", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let rows: serde_json::Value = serde_json::from_slice(&out.stdout).expect("queue JSON");
    assert_eq!(rows.as_array().map(Vec::len), Some(1));
    assert_eq!(rows[0]["spec_id"], f.member);
}

#[test]
fn toon_queue_list_outputs_zero_rows_for_unknown_batch() {
    let f = fixture();
    assert_empty_batch(&f, "toon");
    let stdout = String::from_utf8_lossy(&call(&f, "toon", "wave-a", &[]).stdout).into_owned();
    assert!(
        stdout.lines().any(|line| line.starts_with("count: 1 ")),
        "{stdout}"
    );
    assert!(
        stdout.contains(&f.member),
        "expected batch member:\n{stdout}"
    );
}

#[test]
fn json_and_toon_keep_completed_members_with_include_completed_and_filters() {
    let f = fixture();
    // Preserve the cache's Approved summary while changing the canonical YAML.
    // This models a stale terminal queue row: list's opportunistic GC consults
    // the cache, while the filter path reads the current store object.
    mark_store_object_completed(&f.repo, &f.member);

    for format in ["json", "toon"] {
        let out = call(&f, format, "wave-a", &["--include-completed"]);
        assert!(
            out.status.success(),
            "{format} queue list failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains(&f.member),
            "{format} dropped completed batch member:\n{stdout}"
        );
        if format == "json" {
            let rows: serde_json::Value = serde_json::from_str(&stdout).expect("queue JSON");
            assert_eq!(rows.as_array().map(Vec::len), Some(1));
        } else {
            assert!(
                stdout.lines().any(|line| line.starts_with("count: 1 ")),
                "{stdout}"
            );
        }
    }
}
