#![cfg(target_os = "linux")]
//! Black-box acceptance coverage for BUG-1771: `aida list --status shelved`
//! and `aida list --status needs-decision` reach the STORY-1023 parked lenses
//! on the git-backend (distributed) path instead of refusing the token.
//!
//! These tests drive the SHIPPED binary through `aida list`. The pre-existing
//! unit coverage for the same feature calls `requirement_matches_status_filter`
//! directly, so it kept passing while both lenses were unreachable from every
//! real invocation — a test that never touches the command surface cannot
//! witness a command-surface regression.
//!
//! The two lenses are distinguished by WHICH parked field the spec carries, not
//! by its status (both sit at the stored `NeedsAttention`): a `failure_reason`
//! reads as Shelved, an `attention_reason` as Needs Decision. The fixture makes
//! each one the way the CLI really makes it — `aida review record --verdict
//! request-changes` shelves, `aida punt` asks for a decision — so a change to
//! either writer shows up here.
// trace:BUG-1771 | ai:claude

use std::path::Path;
use std::process::{Command, Output};

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
    Command::new(env!("CARGO_BIN_EXE_aida"))
        .current_dir(&fixture.repo)
        .env("HOME", &fixture.home)
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_SESSION_ROLE", "advisor")
        .env("NO_COLOR", "1")
        .args(args)
        .output()
        .expect("run aida")
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
    assert!(git(&repo, &["config", "user.name", "BUG-1771 Test"])
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

fn spec_id_from(stdout: &str) -> String {
    stdout
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .find(|token| token.starts_with("TASK-") && token[5..].chars().all(|c| c.is_ascii_digit()))
        .unwrap_or_else(|| panic!("missing task id in {stdout}"))
        .to_string()
}

/// Add an Approved task and move it to In Progress (the status both parking
/// writers below require).
fn add_in_progress(fixture: &Fixture, title: &str) -> String {
    let out = ok(
        "aida add",
        aida(
            fixture,
            &[
                "add", "--title", title, "--type", "task", "--status", "approved",
            ],
        ),
    );
    let id = spec_id_from(&out);
    ok(
        "aida edit --status in-progress",
        aida(fixture, &["edit", &id, "--status", "in-progress"]),
    );
    id
}

/// The `spec_id`s `aida list` returns for a filter, read out of the machine
/// JSON projection so the assertion cannot be satisfied by a rendered label.
fn list_ids(fixture: &Fixture, args: &[&str]) -> Vec<String> {
    let mut argv = vec!["list", "--format", "json"];
    argv.extend_from_slice(args);
    let stdout = ok("aida list", aida(fixture, &argv));
    let rows: serde_json::Value =
        serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("list json: {e}\n{stdout}"));
    let mut ids: Vec<String> = rows
        .as_array()
        .unwrap_or_else(|| panic!("list json is not an array: {stdout}"))
        .iter()
        .map(|r| {
            r.get("spec_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    ids.sort();
    ids
}

/// The `status_lens` value `aida list --format json` reports for one row — the
/// label channel the filter must agree with.
fn list_lens(fixture: &Fixture, args: &[&str], spec_id: &str) -> Option<String> {
    let mut argv = vec!["list", "--format", "json"];
    argv.extend_from_slice(args);
    let stdout = ok("aida list", aida(fixture, &argv));
    let rows: serde_json::Value = serde_json::from_str(&stdout).expect("list json");
    rows.as_array()?
        .iter()
        .find(|r| r.get("spec_id").and_then(|v| v.as_str()) == Some(spec_id))
        .and_then(|r| r.get("status_lens"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

struct Parked {
    shelved: String,
    needs_decision: String,
    approved: String,
}

/// One spec parked each way, plus an Approved control that must never be
/// selected by a lens token.
fn parked_fixture(fixture: &Fixture) -> Parked {
    let shelved = add_in_progress(fixture, "BUG-1771 shelved fixture");
    // A refusing verdict insists on `--pr`, and resolving that PR's head needs
    // a forge — so the fixture pins the sha to its own HEAD instead.
    let head = String::from_utf8_lossy(&git(&fixture.repo, &["rev-parse", "HEAD"]).stdout)
        .trim()
        .to_string();
    ok(
        "aida review record",
        aida(
            fixture,
            &[
                "review",
                "record",
                &shelved,
                "--verdict",
                "request-changes",
                "--pr",
                "1",
                "--sha",
                &head,
                "--summary",
                "fixture refusal",
            ],
        ),
    );
    let needs_decision = add_in_progress(fixture, "BUG-1771 needs-decision fixture");
    ok(
        "aida punt",
        aida(
            fixture,
            &[
                "punt",
                &needs_decision,
                "--category",
                "design-fork",
                "--reason",
                "fixture fork",
            ],
        ),
    );
    let approved = spec_id_from(&ok(
        "aida add control",
        aida(
            fixture,
            &[
                "add",
                "--title",
                "BUG-1771 approved control",
                "--type",
                "task",
                "--status",
                "approved",
            ],
        ),
    ));
    Parked {
        shelved,
        needs_decision,
        approved,
    }
}

/// AC1 + AC2: both lens tokens select their own parked set on the shipped
/// git-backend list path. Before the fix each of these four `aida list` calls
/// exited non-zero with "Unknown status filter", so `ok()` panics without it.
#[test]
fn lens_status_tokens_select_their_own_parked_set() {
    let fixture = init_fixture();
    let p = parked_fixture(&fixture);

    // The fixture really did park them two different ways — asserted through
    // the stored status first, so a lens assertion below cannot pass because
    // neither spec was parked at all.
    let parked = list_ids(&fixture, &["--status", "needs-attention"]);
    assert_eq!(
        parked,
        sorted(&[&p.shelved, &p.needs_decision]),
        "both fixtures must sit at the stored NeedsAttention status"
    );

    assert_eq!(
        list_ids(&fixture, &["--status", "shelved"]),
        vec![p.shelved.clone()],
        "`--status shelved` must select the failure-reason spec ONLY"
    );
    assert_eq!(
        list_ids(&fixture, &["--status", "needs-decision"]),
        vec![p.needs_decision.clone()],
        "`--status needs-decision` must select the attention-reason spec ONLY"
    );

    // The filter and the rendered label are one computation (BUG-1771's
    // `row_parked_lens`): a row the filter selects reports the lens asked for.
    assert_eq!(
        list_lens(&fixture, &["--status", "shelved"], &p.shelved).as_deref(),
        Some("Shelved")
    );
    assert_eq!(
        list_lens(&fixture, &["--status", "needs-decision"], &p.needs_decision).as_deref(),
        Some("NeedsDecision")
    );
}

/// AC4: a lens token mixed with a stored status honours BOTH halves. The
/// widening to `needs-attention` must not leak the other lens back in, and the
/// stored half must not be dropped.
#[test]
fn lens_token_mixed_with_a_stored_status_honours_both() {
    let fixture = init_fixture();
    let p = parked_fixture(&fixture);

    assert_eq!(
        list_ids(&fixture, &["--status", "shelved,approved"]),
        sorted(&[&p.shelved, &p.approved]),
        "the Approved row must survive the lens narrowing, and the \
         needs-decision row must not ride in on the widening"
    );

    // Naming the stored status the lens widens to means every parked row is in
    // scope by its own status — the narrowing stands down rather than hiding
    // rows the caller explicitly asked for.
    assert_eq!(
        list_ids(&fixture, &["--status", "shelved,needs-attention"]),
        sorted(&[&p.shelved, &p.needs_decision]),
        "an explicit needs-attention keeps both parked rows visible"
    );

    // Both lenses at once is the whole parked set, reached without naming the
    // stored status.
    assert_eq!(
        list_ids(&fixture, &["--status", "shelved,needs-decision"]),
        sorted(&[&p.shelved, &p.needs_decision])
    );
}

/// AC3: the refusal enumerates the lens tokens it accepts. Built from the same
/// table the parser reads, so the accepted set and the message cannot drift
/// apart the way they did for the nine months this bug was shipped.
#[test]
fn unknown_status_refusal_names_the_lens_tokens() {
    let fixture = init_fixture();
    let out = aida(&fixture, &["list", "--status", "definitely-not-a-status"]);
    assert!(
        !out.status.success(),
        "an unknown status token must still be refused"
    );
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        text.contains("definitely-not-a-status"),
        "the refusal must name the offending token: {text}"
    );
    for token in ["shelved", "needs-decision"] {
        assert!(
            text.contains(token),
            "the refusal must enumerate the `{token}` lens: {text}"
        );
    }
}

fn sorted(ids: &[&String]) -> Vec<String> {
    let mut v: Vec<String> = ids.iter().map(|s| (*s).clone()).collect();
    v.sort();
    v
}

#[test]
fn epic_inherits_child_lens() {
    let fixture = init_fixture();
    let p = parked_fixture(&fixture);

    // Create an epic and add it as parent to the shelved task
    let out = ok(
        "aida add epic",
        aida(
            &fixture,
            &[
                "add",
                "--title",
                "epic test",
                "--type",
                "epic",
                "--status",
                "approved",
            ],
        ),
    );
    let epic_id = out.split_whitespace().nth(1).unwrap().to_string();
    ok(
        "aida rel add",
        aida(
            &fixture,
            &[
                "rel", "add", "--from", &epic_id, "--to", &p.shelved, "--type", "Parent",
            ],
        ),
    );

    // Epic should have inherited NeedsAttention status and the Shelved lens.
    let list = list_ids(&fixture, &["--status", "shelved"]);
    assert!(
        list.contains(&epic_id),
        "epic must inherit shelved lens from child"
    );

    assert_eq!(
        list_lens(&fixture, &["--status", "shelved"], &epic_id).as_deref(),
        Some("Shelved")
    );
}
