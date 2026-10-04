#![cfg(target_os = "linux")]
//! Black-box acceptance coverage for BUG-1687: a deferred spec must stop
//! reporting the status it was deferred OUT of, and `aida list --status
//! deferred` must return the deferred shelf instead of refusing the token.
//!
//! The filed harm was two agent sessions in one afternoon disagreeing about the
//! same spec: the one that ran `aida defer` reported it deferred, and the one
//! that read `aida show` reported `status: needs-attention` and called the first
//! one wrong. Both read their own surface correctly — the substrate gave two
//! answers. So every assertion here drives the SHIPPED binary and reads the
//! surface an agent actually reads.
//!
//! Deferral is a VIEW-FLAG (STORY-584), orthogonal to status, which is what
//! makes it different from BUG-1771's parked lenses: there is no stored status
//! to widen a cache query to, and the stored status must SURVIVE the round trip
//! (AC3). The tests below therefore check both halves — the display override,
//! and that nothing was rewritten underneath it.
// trace:BUG-1687 | ai:claude

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

fn aida_with(fixture: &Fixture, envs: &[(&str, &str)], args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
    cmd.current_dir(&fixture.repo)
        .env("HOME", &fixture.home)
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_SESSION_ROLE", "advisor")
        .env("NO_COLOR", "1")
        .args(args);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.output().expect("run aida")
}

fn aida(fixture: &Fixture, args: &[&str]) -> Output {
    aida_with(fixture, &[], args)
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
    assert!(git(&repo, &["config", "user.name", "BUG-1687 Test"])
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

fn add_task(fixture: &Fixture, title: &str, status: &str) -> String {
    let out = ok(
        "aida add",
        aida(
            fixture,
            &[
                "add", "--title", title, "--type", "task", "--status", status,
            ],
        ),
    );
    spec_id_from(&out)
}

/// The `spec_id`s `aida list` returns for a filter, read out of the machine
/// JSON projection so an assertion cannot be satisfied by a rendered label.
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

/// One TOON scalar out of an `aida show` body, matched on the WHOLE line.
///
/// A substring test is not good enough here: `stored_status: needs-attention`
/// contains `status: needs-attention`, so "the status line does not say
/// needs-attention" has to be asked of the `status` key itself.
fn toon_scalar<'a>(out: &'a str, key: &str) -> Option<&'a str> {
    out.lines()
        .find_map(|line| line.strip_prefix(&format!("{key}: ")))
}

/// One field out of `aida show --json`.
fn show_json(fixture: &Fixture, spec: &str) -> serde_json::Value {
    let stdout = ok("aida show --json", aida(fixture, &["show", spec, "--json"]));
    serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("show json: {e}\n{stdout}"))
}

fn stored_status(fixture: &Fixture, spec: &str) -> String {
    show_json(fixture, spec)
        .get("stored_status")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("show json has no stored_status for {spec}"))
        .to_string()
}

fn defer(fixture: &Fixture, spec: &str, until: &str) {
    ok(
        "aida defer",
        aida(fixture, &["defer", spec, "--until", until]),
    );
}

fn sorted(ids: &[&String]) -> Vec<String> {
    let mut v: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
    v.sort();
    v
}

/// AC2: `aida list --status deferred` returns every deferred spec on the
/// shipped (git-backend) path. Before the fix the command exited non-zero with
/// "Unknown status filter 'deferred'", so `ok()` panics without it.
///
/// Each positive assertion is paired with the control that stops it passing for
/// the wrong reason: a deferred row is hidden by the DEFAULT defer filter, so
/// "the row appears" is only meaningful next to "the row is absent without the
/// filter".
#[test]
fn deferred_status_token_returns_the_deferred_shelf() {
    let fixture = init_fixture();
    let deferred = add_task(&fixture, "BUG-1687 deferred fixture", "approved");
    let active = add_task(&fixture, "BUG-1687 active control", "approved");
    defer(&fixture, &deferred, "after the stability push");

    // Control: the default view hides it, so the assertion below is about the
    // filter and not about the spec merely existing.
    let default_view = list_ids(&fixture, &[]);
    assert!(
        !default_view.contains(&deferred),
        "the default view must still hide a deferred spec: {default_view:?}"
    );
    assert!(
        default_view.contains(&active),
        "the non-deferred control must be in the default view: {default_view:?}"
    );

    assert_eq!(
        list_ids(&fixture, &["--status", "deferred"]),
        vec![deferred.clone()],
        "`--status deferred` must return the deferred spec and ONLY it"
    );

    // The token is a spelling of the existing flag, so the two must agree
    // exactly — a second, divergent defer lens would be a new bug.
    assert_eq!(
        list_ids(&fixture, &["--status", "deferred"]),
        list_ids(&fixture, &["--deferred"]),
        "`--status deferred` and `--deferred` must select the same set"
    );

    // A deferred spec at a CLOSED status must still come back: a bare
    // `--status deferred` names no stored status, so the STORY-723 default open
    // lens would otherwise re-hide it.
    let closed = add_task(&fixture, "BUG-1687 deferred and completed", "approved");
    ok(
        "aida edit --status completed",
        aida(&fixture, &["edit", &closed, "--status", "completed"]),
    );
    defer(&fixture, &closed, "next major");
    assert_eq!(
        list_ids(&fixture, &["--status", "deferred"]),
        sorted(&[&deferred, &closed]),
        "the defer shelf spans statuses — the open-lens default must not \
         re-hide a deferred-and-Completed spec"
    );
}

/// AC2 + AC4, mixed set: a view token beside a stored status honours BOTH, the
/// way BUG-1771 made a lens token do. The axes compose as an AND — "deferred
/// work that is also Approved" — and neither token is silently dropped.
#[test]
fn deferred_token_mixed_with_a_stored_status_honours_both() {
    let fixture = init_fixture();
    let deferred_approved = add_task(&fixture, "BUG-1687 deferred approved", "approved");
    let deferred_draft = add_task(&fixture, "BUG-1687 deferred draft", "draft");
    let active_approved = add_task(&fixture, "BUG-1687 active approved", "approved");
    defer(&fixture, &deferred_approved, "q3");
    defer(&fixture, &deferred_draft, "q4");

    assert_eq!(
        list_ids(&fixture, &["--status", "deferred,approved"]),
        vec![deferred_approved.clone()],
        "the stored half must narrow the shelf (not be dropped), and the defer \
         half must exclude the live Approved row"
    );
    let shelf = list_ids(&fixture, &["--status", "deferred"]);
    assert!(
        shelf.contains(&deferred_draft) && !shelf.contains(&active_approved),
        "sanity: the unqualified shelf holds both deferred rows and no live one: {shelf:?}"
    );
}

/// AC1 + AC4: every `aida show` surface reports the deferral rather than the
/// status the spec was deferred out of — and the stored status is still
/// readable beside it, so nothing is hidden.
///
/// `needs-attention` is the status from the filed report: the orchestrator
/// deferred two punted specs and `aida show` kept calling them parked-for-a-
/// human.
#[test]
fn show_reports_the_deferral_not_the_status_it_was_deferred_out_of() {
    let fixture = init_fixture();
    let spec = add_task(&fixture, "BUG-1687 punted then deferred", "approved");
    ok(
        "aida edit --status in-progress",
        aida(&fixture, &["edit", &spec, "--status", "in-progress"]),
    );
    ok(
        "aida punt",
        aida(
            &fixture,
            &[
                "punt",
                &spec,
                "--category",
                "design-fork",
                "--reason",
                "fixture fork",
            ],
        ),
    );
    assert_eq!(
        stored_status(&fixture, &spec),
        "Needs Attention",
        "fixture precondition: the spec is parked for a human before deferral"
    );

    defer(&fixture, &spec, "after the stability push");

    // The agent surface (TOON) — the one that misled both sessions. stdout is a
    // pipe here, so agent mode is the default, exactly as it is for an agent.
    let toon = ok("aida show", aida(&fixture, &["show", &spec]));
    assert_eq!(
        toon_scalar(&toon, "status"),
        Some("deferred"),
        "the agent surface must report the deferral, and must NOT report a \
         status that asks for a human now: {toon}"
    );
    assert_eq!(
        toon_scalar(&toon, "stored_status"),
        Some("needs-attention"),
        "the stored status must stay readable beside the override: {toon}"
    );
    assert_eq!(
        toon_scalar(&toon, "deferred_until"),
        Some("after the stability push"),
        "the revisit trigger is what makes deferral legible: {toon}"
    );

    // The human surface.
    let human = ok(
        "aida show --format human",
        aida_with(&fixture, &[("AIDA_AGENT_OUTPUT", "0")], &["show", &spec]),
    );
    // Asserted per STATUS LINE, not over the whole body: `aida show` also
    // prints the punt RECORD ("Needs Attention — punted:" with its category and
    // reason), which is a factual account of an event that did happen and is
    // not a claim about the spec's current state. AC1 is about the status the
    // surface presents, and `aida show` presents it twice (TASK-269 reprints it
    // at the foot) — so both lines are checked, and a body-wide substring test
    // would pass or fail for the wrong reason either way.
    let status_lines: Vec<&str> = human
        .lines()
        .filter_map(|l| l.strip_prefix("Status: "))
        .collect();
    assert_eq!(
        status_lines.len(),
        2,
        "expected the Status line and its TASK-269 reprint: {human}"
    );
    for line in &status_lines {
        assert!(
            line.contains("Deferred (after the stability push)"),
            "a human Status line must read Deferred with its trigger: {line:?}"
        );
        assert!(
            !line.contains("Needs") && !line.contains("In Progress"),
            "a human Status line must not ask for a human now: {line:?}"
        );
    }

    // The machine surface, and AC4's "one answer": show's display status, the
    // defer fields, and the list filter all agree.
    let json = show_json(&fixture, &spec);
    assert_eq!(
        json.get("status").and_then(|v| v.as_str()),
        Some("Deferred")
    );
    assert_eq!(
        json.get("stored_status").and_then(|v| v.as_str()),
        Some("Needs Attention"),
        "the stored status is the data; only the display answer changed"
    );
    assert_eq!(json.get("deferred").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(
        json.get("deferred_until").and_then(|v| v.as_str()),
        Some("after the stability push")
    );
    assert_eq!(
        list_ids(&fixture, &["--status", "deferred"]),
        vec![spec.clone()],
        "the list surface must claim the same spec show calls deferred"
    );
    assert!(
        !list_ids(&fixture, &["--status", "needs-decision"]).contains(&spec),
        "a deferred spec must not also be offered as a decision waiting on a human"
    );
}

/// AC3: `aida undefer` restores the prior status rather than stranding the
/// spec. The stored status is never touched by either command, which is the
/// whole reason the display override is the right shape — so this asserts the
/// round trip end to end AND that the spec is back in the live view.
#[test]
fn undefer_restores_the_prior_status_round_trip() {
    let fixture = init_fixture();
    let spec = add_task(&fixture, "BUG-1687 round trip", "approved");
    ok(
        "aida edit --status in-progress",
        aida(&fixture, &["edit", &spec, "--status", "in-progress"]),
    );
    let before = stored_status(&fixture, &spec);
    assert_eq!(before, "In Progress", "fixture precondition");

    defer(&fixture, &spec, "after the stability push");
    assert_eq!(
        stored_status(&fixture, &spec),
        before,
        "deferring must not rewrite the stored status"
    );
    let toon = ok("aida show", aida(&fixture, &["show", &spec]));
    assert_eq!(
        toon_scalar(&toon, "status"),
        Some("deferred"),
        "while deferred, the display answer is the deferral: {toon}"
    );
    assert_eq!(
        toon_scalar(&toon, "stored_status"),
        Some("in-progress"),
        "and the status it was deferred out of is still readable: {toon}"
    );

    ok("aida undefer", aida(&fixture, &["undefer", &spec]));
    assert_eq!(
        stored_status(&fixture, &spec),
        before,
        "un-deferring must restore the prior status, not strand the spec"
    );
    let toon = ok("aida show", aida(&fixture, &["show", &spec]));
    assert_eq!(
        toon_scalar(&toon, "status"),
        Some("in-progress"),
        "the override must lift with the flag: {toon}"
    );
    assert_eq!(
        toon_scalar(&toon, "stored_status"),
        None,
        "and the override's companion field goes with it: {toon}"
    );
    assert!(
        list_ids(&fixture, &[]).contains(&spec),
        "an un-deferred spec belongs back in the default view"
    );
    assert!(
        list_ids(&fixture, &["--status", "deferred"]).is_empty(),
        "and must be off the shelf the filter returns"
    );
}
