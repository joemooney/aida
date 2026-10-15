#![cfg(unix)]

//! BUG-1953: `aida pr ship <N>` run from `main` must look up step 5's lease on
//! the PR head branch, not on the cwd branch. Before the fix, step 5 printed
//! "no AIDA lease for branch main — nothing to clean up" and the head branch's
//! lease, worktree and claim survived the merge.
//!
//! The lease's worktree is the shell's cwd, so step 5 reports the BUG-61
//! inside-worktree skip instead of shelling out to `aida session end` — which
//! proves the lease was matched without depending on which `aida` binary the
//! post-merge subprocess would resolve.
// trace:BUG-1953 | ai:claude

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

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

/// BUG-1918: placeholder for the fake forge's PR head in the script below;
/// replaced with the fixture repo's real head commit, whose committer date the
/// ship gate checks the approval against.
// trace:BUG-1918 | ai:claude
const HEAD_SHA: &str = "ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12";
/// The fixture head's committer date: before the fixture approval.
const HEAD_COMMITTED_AT: &str = "2026-10-13T00:00:00Z";

/// BUG-1918: `aida pr ship` refuses a PR with no approval from an independent,
/// re-validated recorder. This test exercises step 5 behind that gate, so the
/// fixture carries what `aida review record` leaves after a human approved at a
/// terminal: a human-review receipt in the (fixture) AIDA home and an attested
/// verdict at the head, recorded by a reviewer session distinct from the
/// implementer lease `01bug1953abc`.
// trace:BUG-1918 | ai:claude
fn approve_at_head(home: &Path, repo: &Path, head: &str) {
    let receipt_id = "1c2d3e4f-5a6b-4c7d-8e9f-0a1b2c3d4e5f";
    let receipts = home.join(".aida/review-receipts");
    std::fs::create_dir_all(&receipts).unwrap();
    std::fs::write(
        receipts.join(format!("{receipt_id}.json")),
        format!(
            r#"{{"id":"{receipt_id}","sha":"{head}","subject":"PR-42","recorded_at":"2026-10-13T01:00:00Z","identity":["session:fixture-reviewer"]}}"#
        ),
    )
    .unwrap();
    let verdicts = repo.join(".aida/review-verdicts");
    std::fs::create_dir_all(&verdicts).unwrap();
    std::fs::write(
        verdicts.join("PR-42.json"),
        format!(
            r#"{{"verdict":"approved","reviewed_sha":"{head}","recorded_at":"2026-10-13T01:00:00Z","recorded_by":"aida review record (operator)","recorder_attestation":{{"sha":"{head}","human_at_tty":true,"receipt_id":"{receipt_id}","identity":["session:fixture-reviewer"]}}}}"#
        ),
    )
    .unwrap();
}

const FAKE_GH: &str = r###"#!/bin/sh
set -eu
state=${AIDA_FIXTURE_STATE:?}
printf '%s\n' "$*" >> "$state/calls"
head='"baseRefName":"main","headRefName":"task-123-work","headRefOid":"ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12","isCrossRepository":false,"headRepository":{"nameWithOwner":"acme/aida-fixture"},"isDraft":false'
if [ "$1" = "--version" ]; then
  echo "gh version fixture"
elif [ "$1 $2" = "pr list" ] && printf '%s' "$*" | grep -q -- '--base'; then
  :
elif [ "$1 $2" = "pr list" ]; then
  printf '42\thttps://github.test/pull/42\ttask-123-work\tmain\n'
elif [ "$1 $2" = "pr view" ] && printf '%s' "$*" | grep -q 'state,title.*mergedAt'; then
  if [ -f "$state/merged" ]; then
    printf '{"state":"MERGED","title":"fixture","mergedAt":"2026-10-13T00:00:00Z",%s}\n' "$head"
  else
    printf '{"state":"OPEN","title":"fixture","mergedAt":null,%s}\n' "$head"
  fi
elif [ "$1 $2" = "pr view" ] && printf '%s' "$*" | grep -q 'state,mergeable,reviewDecision,headRefOid'; then
  printf 'OPEN\tMERGEABLE\t\tab12cd34ef56ab12cd34ef56ab12cd34ef56ab12\n'
elif [ "$1 $2" = "pr view" ] && printf '%s' "$*" | grep -q -- '--json labels'; then
  printf '%s\n' false
elif [ "$1 $2" = "pr view" ] && printf '%s' "$*" | grep -q 'title,body'; then
  printf '%s\n' '{"title":"[AI:claude] fix(pr): fixture (BUG-1953)","body":""}'
elif [ "$1 $2" = "pr checks" ] && printf '%s' "$*" | grep -q -- '--json'; then
  printf '%s\n' '[{"name":"Build","workflow":"CI","bucket":"pass"}]'
elif [ "$1 $2" = "pr checks" ]; then
  printf 'Build\tpass\t1m\thttps://github.test/check/1\n'
elif [ "$1 $2" = "pr merge" ]; then
  touch "$state/merged"
else
  printf 'unexpected fake gh call: %s\n' "$*" >&2
  exit 2
fi
"###;

// trace:BUG-1953.ac87d12b | ai:claude
#[test]
fn ship_by_number_from_main_matches_the_lease_on_the_pr_head_branch() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let repo = root.join("repo");
    let home = root.join("home");
    let bin = root.join("bin");
    let state = root.join("forge-state");
    for dir in [
        repo.join(".github/workflows"),
        home.clone(),
        bin.clone(),
        state.clone(),
    ] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(repo.join(".github/workflows/ci.yml"), "name: CI\n").unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "AIDA Test"]);
    git(&repo, &["add", "."]);
    let out = Command::new("git")
        .current_dir(&repo)
        .args(["commit", "-q", "-m", "base"])
        .env("GIT_COMMITTER_DATE", HEAD_COMMITTED_AT)
        .env("GIT_AUTHOR_DATE", HEAD_COMMITTED_AT)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let head_sha = String::from_utf8(
        Command::new("git")
            .current_dir(&repo)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    git(&repo, &["branch", "task-123-work"]);
    git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/acme/aida-fixture.git",
        ],
    );

    // A live lease on the PR head branch — the shell itself stays on `main`.
    let sessions = repo.join(".aida/sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(
        sessions.join("01bug1953abc.toml"),
        format!(
            "id = \"01bug1953abc\"\nscope = \"TASK-123\"\nslug = \"task-123\"\n\
             owner = \"test@example.com\"\nworktree_path = '{}'\n\
             branch = \"task-123-work\"\nstarted_at = \"2026-10-13T00:00:00Z\"\n\
             hostname = \"fixture\"\nrole = \"implementer\"\n",
            repo.display()
        ),
    )
    .unwrap();

    let gh = bin.join("gh");
    std::fs::write(&gh, FAKE_GH.replace(HEAD_SHA, &head_sha)).unwrap();
    approve_at_head(&home, &repo, &head_sha);
    let mut permissions = std::fs::metadata(&gh).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&gh, permissions).unwrap();

    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let path =
        std::env::join_paths(std::iter::once(bin.clone()).chain(std::env::split_paths(&inherited)))
            .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_aida"))
        .current_dir(&repo)
        .args(["pr", "ship", "42", "--no-pull", "--no-trailer-check"])
        .env("HOME", &home)
        .env("PATH", path)
        .env("AIDA_FIXTURE_STATE", &state)
        .env("AIDA_TEST_GH_BINARY", &gh)
        .env("AIDA_TELEMETRY", "0")
        .env("NO_COLOR", "1")
        .env("AIDA_PR_SHIP_ALLOW_IN_DRIVE", "1")
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(state.join("merged").exists(), "the PR must merge: {text}");
    assert!(
        !text.contains("no AIDA lease for branch main"),
        "step 5 must not look up the cwd branch: {text}"
    );
    assert!(
        text.contains("step 5: lease 01bug1953abc")
            && text.contains("holds the shipped branch task-123-work"),
        "step 5 must match the head branch's lease (BUG-61 inside-worktree skip): {text}"
    );
}
