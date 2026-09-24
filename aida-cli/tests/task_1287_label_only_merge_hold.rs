#![cfg(unix)]

//! Black-box regression coverage for label-only supervised merge holds.
//! The fake forge changes `merge-hold-gate` from red to green only after the
//! label is removed, so a successful merge proves one `pr ship` invocation
//! traversed classification, release, gate re-run, and merge in that order.
// trace:TASK-1287 | ai:codex

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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

if [ "$1 $2" = "pr list" ]; then
  printf '%s\n' '[{"number":1287,"url":"https://github.test/pull/1287","headRefName":"task-1287-fixture","baseRefName":"main","title":"fixture"}]'
elif [ "$1 $2" = "pr view" ] && printf '%s' "$*" | grep -q 'state,title,mergedAt'; then
  printf '%s\n' '{"state":"OPEN","title":"fixture","mergedAt":null,"baseRefName":"main","headRefName":"task-1287-fixture","headRefOid":"abc","isCrossRepository":false,"headRepository":{"nameWithOwner":"acme/aida-fixture"},"isDraft":false}'
elif [ "$1 $2" = "pr view" ] && printf '%s' "$*" | grep -q -- '--json labels'; then
  test -f "$state/label" && printf '%s\n' true || printf '%s\n' false
elif [ "$1 $2" = "pr view" ] && printf '%s' "$*" | grep -q 'title,body'; then
  printf '%s\n' '{"title":"[AI:codex] fix(pr): fixture (TASK-1287)","body":""}'
elif [ "$1 $2" = "pr checks" ] && printf '%s' "$*" | grep -q -- '--json'; then
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

        Self {
            _temp: temp,
            repo,
            home,
            bin,
            state,
        }
    }

    fn ship(&self) -> Output {
        let inherited = std::env::var_os("PATH").unwrap_or_default();
        let path = std::env::join_paths(
            std::iter::once(self.bin.clone()).chain(std::env::split_paths(&inherited)),
        )
        .unwrap();
        Command::new(env!("CARGO_BIN_EXE_aida"))
            .current_dir(&self.repo)
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
            .env("AIDA_TELEMETRY", "0")
            .env("NO_COLOR", "1")
            .env("AIDA_PR_SHIP_ALLOW_IN_DRIVE", "1")
            .output()
            .unwrap()
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
    assert_eq!(
        std::fs::read_to_string(fixture.state.join("events")).unwrap(),
        "merge\n"
    );
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
