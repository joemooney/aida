#![cfg(unix)]

//! Black-box regression coverage for label-only supervised merge holds.
//! The fake forge changes `merge-hold-gate` from red to green only after the
//! label is removed, so a successful merge proves one `pr ship` invocation
//! traversed classification, release, gate re-run, and merge in that order.
// trace:TASK-1287 | ai:codex

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// BUG-1918: the fake forge's PR head. A real-length hex sha, so the
/// fixture's approval can be checked against it.
// trace:BUG-1918 | ai:claude
const HEAD_SHA: &str = "ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12";

fn git(repo: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

struct Fixture {
    _temp: tempfile::TempDir,
    repo: PathBuf,
    home: PathBuf,
    bin: PathBuf,
    state: PathBuf,
}

impl Fixture {
    fn new(label_held: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let repo = root.join("repo");
        let home = root.join("home");
        let bin = root.join("bin");
        let state = root.join("forge-state");
        std::fs::create_dir_all(repo.join(".github/workflows")).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(repo.join(".github/workflows/ci.yml"), "name: CI\n").unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        git(&repo, &["config", "user.name", "AIDA Test"]);
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "base"]);
        git(&repo, &["checkout", "-q", "-b", "task-1287-fixture"]);
        git(
            &repo,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/acme/aida-fixture.git",
            ],
        );

        std::fs::create_dir_all(&state).unwrap();
        if label_held {
            std::fs::write(state.join("label"), "held\n").unwrap();
        }

        let gh = bin.join("gh");
        std::fs::write(
            &gh,
            r###"#!/bin/sh
set -eu
state=${AIDA_FIXTURE_STATE:?}
printf '%s\n' "$*" >> "$state/calls"

if [ "$1" = "--version" ]; then
  echo "gh version fixture"
elif [ "$1 $2" = "pr list" ]; then
  printf '%s\n' '[{"number":1287,"url":"https://github.test/pull/1287","headRefName":"task-1287-fixture","baseRefName":"main","title":"fixture"}]'
elif [ "$1 $2" = "pr view" ] && printf '%s' "$*" | grep -q 'state,title.*mergedAt'; then
  if [ -f "$state/merged" ]; then
    printf '%s\n' '{"state":"MERGED","title":"fixture","mergedAt":"2026-10-06T13:42:00Z","baseRefName":"main","headRefName":"task-1287-fixture","headRefOid":"ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12","isCrossRepository":false,"headRepository":{"nameWithOwner":"acme/aida-fixture"},"isDraft":false}'
    exit 0
  fi
  printf '%s\n' '{"state":"OPEN","title":"fixture","mergedAt":null,"baseRefName":"main","headRefName":"task-1287-fixture","headRefOid":"ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12","isCrossRepository":false,"headRepository":{"nameWithOwner":"acme/aida-fixture"},"isDraft":false}'
elif [ "$1 $2" = "pr view" ] && printf '%s' "$*" | grep -q 'state,mergeable,reviewDecision,headRefOid'; then
  # trace:TASK-1606 | ai:codex — preflight now reads forge mergeability.
  printf 'OPEN\tMERGEABLE\t\tab12cd34ef56ab12cd34ef56ab12cd34ef56ab12\n'
elif [ "$1 $2" = "pr view" ] && printf '%s' "$*" | grep -q -- '--json labels'; then
  test -f "$state/label" && printf '%s\n' true || printf '%s\n' false
elif [ "$1 $2" = "pr view" ] && printf '%s' "$*" | grep -q 'title,body'; then
  printf '%s\n' '{"title":"[AI:codex] fix(pr): fixture (TASK-1287)","body":""}'
elif [ "$1 $2" = "pr checks" ] && printf '%s' "$*" | grep -q -- '--json'; then
  if [ -f "$state/rows" ]; then
    if printf '%s' "$*" | grep -q -- '--required'; then
      cat "$state/required"
    else
      cat "$state/rows"
    fi
    exit 0
  fi
  if [ -f "$state/label" ]; then
    bucket=fail
  else
    bucket=pass
    printf '%s\n' gate-green >> "$state/events"
  fi
  printf '[{"name":"merge-hold-gate","workflow":"merge-hold-gate","bucket":"%s"}]\n' "$bucket"
  test "$bucket" = pass
elif [ "$1 $2" = "pr checks" ] && printf '%s' "$*" | grep -q -- '--watch'; then
  test ! -f "$state/label"
elif [ "$1 $2" = "pr checks" ]; then
  printf '%s\n' 'merge-hold-gate fail 1 https://github.test/check/1'
elif [ "$1 $2" = "pr edit" ] && printf '%s' "$*" | grep -q -- '--remove-label'; then
  rm -f "$state/label"
  printf '%s\n' release >> "$state/events"
elif [ "$1 $2" = "pr merge" ]; then
  test ! -f "$state/label"
  printf '%s\n' merge >> "$state/events"
else
  printf 'unexpected fake gh call: %s\n' "$*" >&2
  exit 2
fi
"###,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&gh).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&gh, permissions).unwrap();

        let fixture = Self {
            _temp: temp,
            repo,
            home,
            bin,
            state,
        };
        fixture.approve_at_head();
        fixture
    }

    /// BUG-1918: `aida pr ship` refuses a PR with no approval from an
    /// independent, re-validated recorder. These tests exercise the hold and
    /// CI paths behind that gate, so the fixture carries what
    /// `aida review record` leaves after a human approved at a terminal: a
    /// human-review receipt in the (fixture) AIDA home and an attested
    /// verdict at the head.
    // trace:BUG-1918 | ai:claude
    fn approve_at_head(&self) {
        let receipt_id = "0b0e1d2c-3f4a-4b5c-8d6e-7f8091a2b3c4";
        let receipts = self.home.join(".aida/review-receipts");
        std::fs::create_dir_all(&receipts).unwrap();
        std::fs::write(
            receipts.join(format!("{receipt_id}.json")),
            format!(
                r#"{{"id":"{receipt_id}","sha":"{HEAD_SHA}","subject":"PR-1287","recorded_at":"2026-10-06T13:00:00Z"}}"#
            ),
        )
        .unwrap();
        let verdicts = self.repo.join(".aida/review-verdicts");
        std::fs::create_dir_all(&verdicts).unwrap();
        std::fs::write(
            verdicts.join("PR-1287.json"),
            format!(
                r#"{{"verdict":"approved","reviewed_sha":"{HEAD_SHA}","recorded_at":"2026-10-06T13:00:00Z","recorded_by":"aida review record (operator)","recorder_attestation":{{"sha":"{HEAD_SHA}","human_at_tty":true,"receipt_id":"{receipt_id}","identity":["session:fixture-reviewer"],"author_check_passed":true}}}}"#
            ),
        )
        .unwrap();
    }

    fn ship_command(&self) -> Command {
        let inherited = std::env::var_os("PATH").unwrap_or_default();
        let path = std::env::join_paths(
            std::iter::once(self.bin.clone()).chain(std::env::split_paths(&inherited)),
        )
        .unwrap();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
        cmd.current_dir(&self.repo)
            .args([
                "pr",
                "ship",
                "1287",
                "--no-pull",
                "--no-cleanup",
                "--no-trailer-check",
            ])
            .env("HOME", &self.home)
            .env("PATH", path)
            .env("AIDA_FIXTURE_STATE", &self.state)
            .env("AIDA_TEST_GH_BINARY", self.bin.join("gh"))
            .env("AIDA_TELEMETRY", "0")
            .env("NO_COLOR", "1")
            .env("AIDA_PR_SHIP_ALLOW_IN_DRIVE", "1");
        cmd
    }

    fn ship(&self) -> Output {
        self.ship_command().output().unwrap()
    }

    // trace:TASK-1331 | ai:codex
    fn use_gitlab(&self, ci_config: &str) {
        git(
            &self.repo,
            &[
                "remote",
                "set-url",
                "origin",
                "https://gitlab.com/acme/aida-fixture.git",
            ],
        );
        std::fs::write(self.repo.join(".gitlab-ci.yml"), "fixture: {}\n").unwrap();
        std::fs::create_dir_all(self.repo.join(".aida")).unwrap();
        std::fs::write(
            self.repo.join(".aida/config.toml"),
            format!("[forge]\nprovider = \"gitlab\"\n[ci]\n{ci_config}\n"),
        )
        .unwrap();
        let glab = self.bin.join("glab");
        std::fs::write(
            &glab,
            r###"#!/bin/sh
set -eu
state=${AIDA_FIXTURE_STATE:?}
printf '%s\n' "$*" >> "$state/glab-calls"
case "$*" in
  "--version") echo "glab version 1.50.0" ;;
  "ci status "*) exit 1 ;;
  *"pipelines/1/jobs"*)
    test ! -f "$state/jobs-unavailable" || exit 1
    printf '%s\n' '[{"name":"Build","status":"success"},{"name":"Optional","status":"failed"}]' ;;
  *"projects/:id/pipelines"*)
    printf '%s\n' '[{"id":1,"status":"failed","name":"gitlab-ci"}]' ;;
  *"merge_requests/1287/merge"*)
    printf '%s\n' merge >> "$state/events"
    printf '%s\n' '{"state":"merged"}' ;;
  *"merge_requests/1287"*)
    printf '%s\n' '{"iid":1287,"state":"opened","detailed_merge_status":"mergeable","title":"fixture","source_branch":"task-1287-fixture","target_branch":"main","sha":"ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12","web_url":"https://gitlab.test/merge_requests/1287"}' ;;
  *"merge_requests"*) printf '%s\n' '[]' ;;
  *) printf 'unexpected fake glab call: %s\n' "$*" >&2; exit 2 ;;
esac
"###,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&glab).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&glab, permissions).unwrap();
    }
}

fn output_text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

// BUG-1566: `aida pr ship` no longer releases a supervised merge-hold
// without a human at an interactive terminal — the same integrity floor as
// `aida merge-hold clear`. Under `cargo test` stdin is not a TTY, so a
// label-only hold must be REFUSED before any release or merge happens.
// trace:BUG-1566 | ai:claude
#[test]
fn label_only_hold_is_refused_without_a_human_at_a_terminal() {
    let fixture = Fixture::new(true);
    let output = fixture.ship();
    let text = output_text(&output);
    assert!(
        !output.status.success(),
        "headless ship must refuse: {text}"
    );
    assert!(
        text.contains("aida merge-hold clear 1287"),
        "refusal must point at the human clear path: {text}"
    );
    let events = std::fs::read_to_string(fixture.state.join("events")).unwrap_or_default();
    assert!(
        !events.contains("release") && !events.contains("merge"),
        "nothing may be released or merged: {events:?}"
    );
    assert!(
        fixture.state.join("label").exists(),
        "the hold label must still be in place"
    );
}

#[test]
fn no_label_and_no_marker_ships_without_release() {
    let fixture = Fixture::new(false);
    let output = fixture.ship();
    let text = output_text(&output);
    assert!(output.status.success(), "{text}");
    assert!(!text.contains("releasing supervised merge-hold"), "{text}");
    let events = std::fs::read_to_string(fixture.state.join("events")).unwrap();
    assert!(events.ends_with("merge\n"), "{events}");
    assert!(!events.contains("release"), "{events}");
}

// BUG-1532: a merge-hold MARKER binds `aida pr ship` with no drive running
// and whatever its reason text says. Headless (no TTY) the ship is refused
// before CI is watched — the marker survives, and nothing is released or
// merged. Before the fix the client guard only honoured a marker whose
// reason named a live drive member's spec.
// trace:BUG-1532 | ai:claude
#[test]
fn marker_hold_binds_pr_ship_without_a_live_drive() {
    let fixture = Fixture::new(false);
    let holds = fixture.repo.join(".aida/merge-holds");
    std::fs::create_dir_all(&holds).unwrap();
    let marker = holds.join("PR-1287");
    let body = r#"{"schema_version":2,"pr":1287,"reason_kind":"rework","detail":"CHANGES REQUESTED for TASK-1287 at 3acf3671fd7a","routing_state":"pending"}"#;
    std::fs::write(&marker, body).unwrap();

    let output = fixture.ship();
    let text = output_text(&output);
    assert!(!output.status.success(), "a held PR must not ship: {text}");
    assert!(
        text.contains("aida merge-hold clear 1287"),
        "refusal must point at the human clear path: {text}"
    );
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap(),
        body,
        "the marker must survive the refused ship"
    );
    let events = std::fs::read_to_string(fixture.state.join("events")).unwrap_or_default();
    assert!(
        !events.contains("release") && !events.contains("merge"),
        "nothing may be released or merged: {events:?}"
    );
    let calls = std::fs::read_to_string(fixture.state.join("calls")).unwrap_or_default();
    assert!(
        !calls.contains("pr checks") && !calls.contains("pr merge"),
        "refused before the CI watch: {calls:?}"
    );
}

// trace:TASK-1331 | ai:codex
#[test]
fn ship_ignores_pending_informational_jobs_before_coarse_watch() {
    let fixture = Fixture::new(false);
    std::fs::create_dir_all(fixture.repo.join(".aida")).unwrap();
    std::fs::write(
        fixture.repo.join(".aida/config.toml"),
        "[ci]\ninformational_workflows = [\"Optional*\"]\n",
    )
    .unwrap();
    std::fs::write(fixture.state.join("required"), "[]").unwrap();
    std::fs::write(
        fixture.state.join("rows"),
        r#"[
        {"name":"Build (ubuntu-latest)","workflow":"CI","bucket":"pass"},
        {"name":"Build (windows-latest)","workflow":"Optional platforms","bucket":"pending"},
        {"name":"Build (macos-latest)","workflow":"Optional platforms","bucket":"fail"}
    ]"#,
    )
    .unwrap();
    let output = fixture.ship();
    assert!(output.status.success(), "{}", output_text(&output));
    let calls = std::fs::read_to_string(fixture.state.join("calls")).unwrap();
    assert!(
        !calls.contains("--watch"),
        "must never enter unfiltered watcher: {calls}"
    );
    assert!(calls.contains("pr merge"), "{calls}");
}

// trace:TASK-1331 | ai:codex
#[test]
fn ship_still_rejects_required_informational_and_unlisted_failures() {
    for required in [false, true] {
        let fixture = Fixture::new(false);
        let workflow = if required {
            "Cross-platform nightly"
        } else {
            "CI"
        };
        let rows = format!(
            r#"[{{"name":"Build (ubuntu-latest)","workflow":"{workflow}","bucket":"fail"}}]"#
        );
        std::fs::write(fixture.state.join("rows"), &rows).unwrap();
        std::fs::write(
            fixture.state.join("required"),
            if required { &rows } else { "[]" },
        )
        .unwrap();
        let output = fixture.ship();
        assert!(!output.status.success(), "{}", output_text(&output));
        assert!(
            output_text(&output).contains("CI is red"),
            "{}",
            output_text(&output)
        );
        let calls = std::fs::read_to_string(fixture.state.join("calls")).unwrap();
        assert!(!calls.contains("pr merge"), "{calls}");
    }
}

// trace:TASK-1331 | ai:codex
#[test]
fn gitlab_ship_refines_failed_watcher_for_informational_jobs() {
    for config in [
        "informational_checks = [\"Optional\"]",
        "informational_workflows = [\"gitlab-*\"]",
    ] {
        let fixture = Fixture::new(false);
        fixture.use_gitlab(config);
        let output = fixture.ship();
        assert!(output.status.success(), "{}", output_text(&output));
        let calls = std::fs::read_to_string(fixture.state.join("glab-calls")).unwrap();
        let rows = calls
            .find("pipelines/1/jobs")
            .expect("must classify job rows");
        let merge = calls
            .find("merge_requests/1287/merge")
            .expect("must merge after green classified jobs");
        assert!(rows < merge, "{calls}");
        assert!(
            !calls.contains("ci status"),
            "bounded wait must not invoke unbounded watcher: {calls}"
        );
        assert!(calls.contains("merge_requests/1287/merge"), "{calls}");
        assert_eq!(
            std::fs::read_to_string(fixture.state.join("events")).unwrap(),
            "merge\n"
        );
    }
}

// trace:TASK-1331 | ai:codex
#[test]
fn gitlab_ship_rejects_unlisted_failures_and_unavailable_job_rows() {
    for unavailable in [false, true] {
        let fixture = Fixture::new(false);
        fixture.use_gitlab(if unavailable {
            "informational_checks = [\"Optional\"]"
        } else {
            "informational_checks = [\"Other\"]"
        });
        if unavailable {
            std::fs::write(fixture.state.join("jobs-unavailable"), "").unwrap();
        }
        let output = fixture.ship();
        assert!(!output.status.success(), "{}", output_text(&output));
        let calls = std::fs::read_to_string(fixture.state.join("glab-calls")).unwrap();
        assert!(
            !calls.contains("ci status") && calls.contains("pipelines/1/jobs"),
            "{calls}"
        );
        assert!(!calls.contains("merge_requests/1287/merge"), "{calls}");
        let text = output_text(&output);
        if unavailable {
            assert_eq!(output.status.code(), Some(20), "{text}");
            assert!(text.contains("CI did not pass"), "{text}");
        } else {
            assert!(
                text.contains("CI is red") && text.contains("Optional"),
                "{text}"
            );
        }
    }
}

// Exercise actual CLI exit status, continuation to merge, and the downstream
// hold gate using the same fake forge as the hold regressions above.
// trace:TASK-1602 | ai:codex
fn drive_state(fixture: &Fixture) -> PathBuf {
    let aida = fixture.repo.join(".aida");
    std::fs::create_dir_all(&aida).unwrap();
    std::fs::write(
        aida.join("drain.lock"),
        serde_json::json!({
            "pid": std::process::id(), "started_at_utc": "2026-10-06T13:42:00Z",
            "command": "test drive", "host": "test", "wave_id": "test-wave"
        })
        .to_string(),
    )
    .unwrap();
    let path = aida.join("drain-state.json");
    std::fs::write(
        &path,
        serde_json::json!({
            "command": "test drive", "mode": "single", "members": [{
                "spec": "TASK-1602", "state": "in-phase-3", "pr": 1287
            }], "current": "TASK-1602", "current_phase": "3 (reviewer)",
            "orchestrator_pid": std::process::id(), "started_at": "2026-10-06T13:42:00Z",
            "on_drain_complete": "escalates merge", "run_uuid": "test-run"
        })
        .to_string(),
    )
    .unwrap();
    path
}

fn operator_ship(fixture: &Fixture) -> Command {
    let mut cmd = fixture.ship_command();
    for key in [
        "AIDA_HEADLESS",
        "AIDA_AUTO_COMPLETE",
        "AIDA_AUTO_COMPLETE_TOKEN",
        "AIDA_AGENT_NAME",
        "AIDA_AGENT_TYPE",
        "AIDA_PR_SHIP_ALLOW_IN_DRIVE",
    ] {
        cmd.env_remove(key);
    }
    cmd
}

#[test]
fn drive_owned_cli_timeout_is_nonzero_and_never_watches_ci() {
    let fixture = Fixture::new(false);
    drive_state(&fixture);
    let output = operator_ship(&fixture)
        .args(["--wait", "0"])
        .output()
        .unwrap();
    let text = output_text(&output);
    assert!(!output.status.success(), "{text}");
    assert!(text.contains("timed out"), "{text}");
    let calls = std::fs::read_to_string(fixture.state.join("calls")).unwrap();
    assert!(
        !calls.contains("pr checks") && !calls.contains("pr merge"),
        "{calls}"
    );
    let output = operator_ship(&fixture)
        .env("AIDA_HEADLESS", "1")
        .args(["--wait", "300"])
        .output()
        .unwrap();
    assert!(!output.status.success(), "{}", output_text(&output));
    assert!(output_text(&output).contains("caller is a drive"));
    assert!(!output_text(&output).contains("waiting for drive release"));
}

#[test]
fn drive_release_wait_continues_to_ship_and_still_enforces_holds() {
    for held in [false, true] {
        let fixture = Fixture::new(held);
        let path = drive_state(&fixture);
        let child = operator_ship(&fixture)
            .args(["--wait", "10"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        // Releasing state after the process starts allows either the first or
        // a later ownership probe to observe release; unit coverage verifies
        // the exact two-probe transition separately.
        std::thread::sleep(std::time::Duration::from_millis(250));
        std::fs::remove_file(path).unwrap();
        let output = child.wait_with_output().unwrap();
        let text = output_text(&output);
        assert_eq!(output.status.success(), !held, "{text}");
        let events = std::fs::read_to_string(fixture.state.join("events")).unwrap_or_default();
        assert_eq!(events.contains("merge"), !held, "{text}");
    }
}

#[test]
// trace:TASK-1602 | ai:codex
fn direct_tty_explains_ownership_but_managed_tty_gets_plain_refusal() {
    use portable_pty::{native_pty_system, CommandBuilder, PtySize};
    use std::io::Read;
    for managed in [false, true] {
        let fixture = Fixture::new(false);
        drive_state(&fixture);
        let source = operator_ship(&fixture);
        let mut cmd = CommandBuilder::new(source.get_program());
        cmd.args(source.get_args());
        cmd.cwd(&fixture.repo);
        for (key, value) in source.get_envs() {
            match value {
                Some(value) => cmd.env(key, value),
                None => cmd.env_remove(key),
            }
        }
        if managed {
            cmd.env("AIDA_AGENT_NAME", "codex");
        }
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut child = pair.slave.spawn_command(cmd).unwrap();
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().unwrap();
        let read = std::thread::spawn(move || {
            let mut text = String::new();
            let _ = reader.read_to_string(&mut text);
            text
        });
        let started = std::time::Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if started.elapsed() > std::time::Duration::from_secs(5) {
                child.kill().unwrap();
                panic!("ship must explain/refuse without waiting for terminal input");
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        assert!(!status.success());
        drop(pair.master);
        let text = read.join().unwrap();
        assert!(text.contains("owned by the live drive"), "{text}");
        assert_eq!(text.contains("test-wave"), !managed, "{text}");
        assert_eq!(text.contains("run unavailable"), !managed, "{text}");
        assert_eq!(text.contains("3/6 (reviewer)"), !managed, "{text}");
        assert_eq!(text.contains("activity unavailable"), !managed, "{text}");
        assert_eq!(
            text.contains("escalates the merge decision"),
            !managed,
            "{text}"
        );
        assert_eq!(text.contains("--wait 300"), !managed, "{text}");
    }
}

// trace:TASK-1602 | ai:codex
#[test]
fn drive_merge_during_wait_skips_ci_and_merge_without_claiming_credit() {
    use std::io::{BufRead, BufReader};
    for bucket in ["pass", "fail"] {
        let fixture = Fixture::new(false);
        let path = drive_state(&fixture);
        std::fs::write(
            fixture.state.join("rows"),
            format!(r#"[{{"name":"Build","workflow":"CI","bucket":"{bucket}"}}]"#,),
        )
        .unwrap();
        std::fs::write(fixture.state.join("required"), "[]").unwrap();
        let mut child = operator_ship(&fixture)
            .args(["--wait", "10"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        // Synchronize on the actual ownership wait, so metadata was OPEN
        // before the transition and the drive guard has observed its owner.
        let mut stderr = BufReader::new(child.stderr.take().unwrap());
        let mut prefix = String::new();
        loop {
            let mut line = String::new();
            assert!(stderr.read_line(&mut line).unwrap() > 0, "{prefix}");
            let waiting = line.contains("waiting for drive release");
            prefix.push_str(&line);
            if waiting {
                break;
            }
        }
        std::fs::write(fixture.state.join("merged"), "merged").unwrap();
        std::fs::remove_file(path).unwrap();
        child.stderr = Some(stderr.into_inner());
        let output = child.wait_with_output().unwrap();
        let text = format!("{prefix}{}", output_text(&output));
        let calls = std::fs::read_to_string(fixture.state.join("calls")).unwrap();
        assert!(output.status.success(), "{text}\n{calls}");
        assert!(
            text.contains("was already merged (not by this run)"),
            "{text}\n{calls}"
        );
        assert!(!text.contains("PR-1287 shipped"), "{text}");
        assert!(
            calls.matches("state,title,author,mergedAt").count() >= 2,
            "{calls}"
        );
        assert!(
            !calls.contains("pr checks") && !calls.contains("pr merge"),
            "{calls}"
        );
        let events =
            std::fs::read_to_string(fixture.repo.join(".aida/events.jsonl")).unwrap_or_default();
        assert!(!events.contains("PrMerged"), "{events}");
    }
}

// trace:TASK-1606 | ai:codex
#[test]
fn ownership_and_ci_share_one_wait_deadline() {
    use std::io::{BufRead, BufReader};
    let fixture = Fixture::new(false);
    let path = drive_state(&fixture);
    std::fs::write(
        fixture.state.join("rows"),
        r#"[{"name":"Build","workflow":"CI","bucket":"pending"}]"#,
    )
    .unwrap();
    std::fs::write(fixture.state.join("required"), "[]").unwrap();
    let started = std::time::Instant::now();
    let mut child = operator_ship(&fixture)
        .args(["--wait", "2"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut prefix = String::new();
    loop {
        let mut line = String::new();
        assert!(stderr.read_line(&mut line).unwrap() > 0, "{prefix}");
        let waiting = line.contains("waiting for drive release");
        prefix.push_str(&line);
        if waiting {
            break;
        }
    }
    // The ownership poll consumes the budget, then release leads into CI
    // with no new two-second allowance.
    std::fs::remove_file(path).unwrap();
    child.stderr = Some(stderr.into_inner());
    let output = child.wait_with_output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(21),
        "{prefix}{}",
        output_text(&output)
    );
    assert!(
        started.elapsed() < std::time::Duration::from_millis(3500),
        "a second CI budget was incorrectly granted"
    );
    let calls = std::fs::read_to_string(fixture.state.join("calls")).unwrap();
    assert!(!calls.contains("pr merge"), "{calls}");
}
