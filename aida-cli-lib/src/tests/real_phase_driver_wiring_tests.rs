use super::{
    branch_commits_ahead_main, build_auto_punt_args, build_integrate_rebase_args,
    build_phase3_auto_rebase_args, ensure_implementer_branch_pushed, find_orchestrated_lease,
    head_commit_message, headless_log_is_zero_bytes, list_leases, orchestrator_phase_child_env,
    orchestrator_pr_title_and_body, pushed_branch_commits_ahead_default, RealPhaseDriver,
};
use crate::auto_complete::{FailureKind, Phase, PhaseDriver, PhaseFailure, PhaseReconcile};
use aida_core::{
    DatabaseBackend, FieldChange, HistoryEntry, Requirement, RequirementStatus, RequirementsStore,
};
use chrono::{DateTime, Utc};
use std::process::Command;
use uuid::Uuid;

fn git(root: &std::path::Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("git {} failed to spawn: {e}", args.join(" ")));
    assert!(
        out.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn write_commit(root: &std::path::Path, file: &str, body: &str, msg: &str) {
    std::fs::write(root.join(file), body).unwrap();
    git(root, &["add", file]);
    git(root, &["commit", "-q", "-m", msg]);
}

fn dt(ts: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(ts)
        .unwrap()
        .with_timezone(&Utc)
}

fn hist_status(ts: &str, old_value: &str, new_value: &str) -> HistoryEntry {
    HistoryEntry {
        id: Uuid::now_v7(),
        author: "test".to_string(),
        timestamp: dt(ts),
        changes: vec![FieldChange {
            field_name: "status".to_string(),
            old_value: old_value.to_string(),
            new_value: new_value.to_string(),
        }],
    }
}

fn git_repo_with_origin() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let remote = tmp.path().join("origin.git");
    let worktree = tmp.path().join("work");
    std::fs::create_dir_all(&worktree).unwrap();
    git(tmp.path(), &["init", "--bare", "origin.git"]);
    git(&worktree, &["init", "-q"]);
    git(&worktree, &["config", "user.email", "aida@example.invalid"]);
    git(&worktree, &["config", "user.name", "AIDA Test"]);
    git(&worktree, &["checkout", "-q", "-b", "bug-878"]);
    git(
        &worktree,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    (tmp, worktree, remote)
}

/// Mint a session lease + its manifest under `<root>/.aida/sessions/`,
/// exactly as `aida queue work --session-id` would, so lease discovery has
// something real to resolve against. trace:TASK-262 | ai:claude
fn mint_lease(root: &std::path::Path, lease_id: &str, branch: &str, claude_id: Option<&str>) {
    let sessions = root.join(".aida").join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(
        sessions.join(format!("{lease_id}.toml")),
        format!(
            "id = \"{lease_id}\"\n\
             scope = \"{branch}\"\n\
             slug = \"{branch}\"\n\
             owner = \"test\"\n\
             worktree_path = \"/tmp/{lease_id}\"\n\
             branch = \"{branch}\"\n\
             started_at = \"2026-09-03T00:00:00Z\"\n\
             hostname = \"test\"\n"
        ),
    )
    .unwrap();
    let manifest = crate::session_manifest::SessionManifest {
        session_id: lease_id.to_string(),
        planned_at: chrono::Utc::now(),
        plan_source: "queue work".to_string(),
        claude_session_id: claude_id.map(str::to_string),
        batch_name: None,
        plan: None,
        items: vec![],
    };
    crate::session_manifest::save(
        &crate::session_manifest::manifest_path(root, lease_id),
        &manifest,
    )
    .unwrap();
}

/// Build a minimal interactive `RealPhaseDriver` rooted at an isolated
/// tempdir. Reads only that tempdir's (absent) `[drain]` config, so it
// never touches the real project. trace:TASK-262 | ai:claude
fn driver(root: &std::path::Path, spec: &str) -> RealPhaseDriver {
    RealPhaseDriver::new(
        root.to_path_buf(),
        spec.to_string(),
        "test-queue".to_string(),
        None,
        false,
        None,
        crate::AutonomyMode::Default,
        "run-token".to_string(),
        false,
        false,
        false,
        false,
        crate::auto_complete::LifecycleSkip::default(),
        crate::auto_complete::AutoCompleteVariant::Full,
    )
}

fn fake_gh(root: &std::path::Path, body: &str) -> std::path::PathBuf {
    let path = root.join("gh");
    std::fs::write(&path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
    }
    path
}

#[test]
fn driver_resolves_lifecycle_forge_once_from_target_origin() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q", "-b", "main"]);
    git(
        root,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/acme/repo.git",
        ],
    );
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida").join("config.toml"),
        "[forge]\nprovider = \"pure-git\"\n",
    )
    .unwrap();

    let driver = driver(root, "BUG-1037");

    // BUG-1109: a stored pure-git provider that contradicts a known-host
    // origin is auto-repaired at resolution, so even the plain resolver
    // reports the corrected kind (the resolve-once property below still holds).
    assert_eq!(
        crate::forge::resolve_forge_kind(root),
        crate::forge::ForgeKind::GitHub
    );
    assert_eq!(
        crate::forge::resolve_open_change_forge_kind(root),
        crate::forge::ForgeKind::GitHub
    );
    assert_eq!(driver.lifecycle_forge, crate::forge::ForgeKind::GitHub);
    assert_eq!(
        driver.lifecycle_forge().kind(),
        crate::forge::ForgeKind::GitHub
    );
}

#[test]
fn reconcile_failure_does_not_credit_stale_merged_pr_after_reopen() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q", "-b", "main"]);
    git(
        root,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/acme/repo.git",
        ],
    );

    let mut req = Requirement::new("reopened bug".to_string(), String::new());
    req.spec_id = Some("BUG-1112".to_string());
    req.status = RequirementStatus::Approved;
    req.history = vec![
        hist_status("2026-09-10T12:00:00Z", "Done", "Completed"),
        hist_status("2026-09-12T12:00:00Z", "Completed", "Approved"),
    ];
    let mut store = RequirementsStore::default();
    store.requirements.push(req);
    aida_core::GitBackend::new(&root.join(".aida-store"))
        .unwrap()
        .save(&store)
        .unwrap();

    let gh = fake_gh(
        root,
        r#"#!/usr/bin/env bash
if [[ "${1:-}" == "pr" && "${2:-}" == "view" && "${3:-}" == "1739" ]]; then
  if [[ "$*" == *"commits"* ]]; then
    printf '[AI:codex] fix(orchestrator): original ship (BUG-1112)\n'
    exit 0
  fi
  cat <<'JSON'
{
  "state": "MERGED",
  "title": "[AI:codex] fix(orchestrator): original ship (BUG-1112)",
  "mergedAt": "2026-09-10T12:30:00Z",
  "baseRefName": "main",
  "headRefName": "bug-1112",
  "headRefOid": "abc123",
  "isCrossRepository": false,
  "headRepository": {"nameWithOwner": "acme/repo"},
  "isDraft": false
}
JSON
  exit 0
fi
exit 1
"#,
    );
    let _env = crate::test_env::EnvVarsGuard::set(&[("AIDA_TEST_GH_BINARY", gh.to_str().unwrap())]);

    let mut driver = driver(root, "BUG-1112");
    driver.pr_number = Some(1739);

    // BUG-1112 acceptance #2: the real phase-1 reconciliation path must not
    // phantom-ship reopened work by reusing the PR that completed the previous
    // lifecycle. The stale PR merged before the latest terminal->open status
    // transition, so phase 1 remains a genuine no-PR failure and the spec can
    // be driven again or shelved as new work.
    // trace:BUG-1112 | ai:codex
    assert_eq!(
        driver.reconcile_failure(
            Phase::Implementer,
            &PhaseFailure::of(FailureKind::NoPr, "phase 1 opened no PR"),
        ),
        PhaseReconcile::GenuineFailure
    );
}

#[test]
fn phase3_auto_rebase_argv_is_pr_rebase_no_smoke() {
    // The reviewer-phase auto-rebase subprocess must run exactly
    // `aida pr rebase <N> --no-smoke` — the `--no-smoke` flag is
    // load-bearing (the orchestrator drives the rebase non-interactively).
    assert_eq!(
        build_phase3_auto_rebase_args(193),
        vec!["pr", "rebase", "193", "--no-smoke"],
    );
}

#[test]
fn integrate_rebase_argv_is_pr_rebase_no_smoke() {
    // STORY-335: `aida queue integrate --rebase` rebases each member's PR
    // branch onto current main via exactly `aida pr rebase <N> --no-smoke`
    // before driving its --from-pr merge. --no-smoke is load-bearing — the
    // subsequent --from-pr drive runs CI, so a local smoke would be wasted.
    assert_eq!(
        build_integrate_rebase_args(641),
        vec!["pr", "rebase", "641", "--no-smoke"],
    );
}

#[test]
fn auto_punt_argv_carries_design_fork_reason_and_lean() {
    // The headless-implementer punt subprocess must run
    // `aida punt <spec> --category design-fork --reason <r> --lean <l>`.
    let args = build_auto_punt_args("TASK-262", "two viable schemas", "schema A");
    assert_eq!(
        args,
        vec![
            "punt",
            "TASK-262",
            "--category",
            "design-fork",
            "--reason",
            "two viable schemas",
            "--lean",
            "schema A",
        ],
    );
    // The reason/lean are passed as discrete argv elements (not shell-
    // joined), so spaces in them can never split into extra args.
    let i = args.iter().position(|a| a == "--reason").unwrap();
    assert_eq!(args[i + 1], "two viable schemas");
}

#[test]
fn phase_child_env_carries_auto_complete_variant() {
    let env = orchestrator_phase_child_env(
        "run-token",
        crate::auto_complete::Phase::Implementer,
        crate::auto_complete::AutoCompleteVariant::ThroughCi,
        "queue-owner",
    );
    assert!(env
        .iter()
        .any(|(k, v)| *k == crate::orchestrator::AUTO_COMPLETE_ENV && v == "1"));
    assert!(env
        .iter()
        .any(|(k, v)| *k == crate::orchestrator::TOKEN_ENV && v == "run-token"));
    assert!(env
        .iter()
        .any(|(k, v)| *k == crate::orchestrator::VARIANT_ENV && v == "through-ci"));
    assert!(env
        .iter()
        .any(|(k, v)| *k == crate::orchestrator::PHASE_ENV && v == "1"));
    assert!(env
        .iter()
        .any(|(k, v)| *k == "AIDA_SESSION_ROLE" && v == "implementer"));
    assert!(env
        .iter()
        .any(|(k, v)| *k == "AIDA_USER" && v == "queue-owner"));
}

#[test]
fn reviewer_phase_child_env_sets_reviewer_role() {
    let env = orchestrator_phase_child_env(
        "run-token",
        crate::auto_complete::Phase::Reviewer,
        crate::auto_complete::AutoCompleteVariant::Full,
        "pipeline-owner",
    );
    // BUG-901: phase children must not inherit the launcher/advisor shell role;
    // queue-work's strict role-routed lookup reads AIDA_SESSION_ROLE.
    assert!(env
        .iter()
        .any(|(k, v)| *k == "AIDA_SESSION_ROLE" && v == "reviewer"));
    // BUG-1038: the reviewer role must not replace the queue identity selected
    // by the parent drain; queue membership reads AIDA_USER.
    assert!(env
        .iter()
        .any(|(k, v)| *k == "AIDA_USER" && v == "pipeline-owner"));
}

#[test]
fn phase2_push_guard_pushes_branch_ahead_of_upstream() {
    let (_tmp, worktree, remote) = git_repo_with_origin();
    write_commit(&worktree, "file.txt", "base\n", "base");
    git(&worktree, &["push", "-q", "-u", "origin", "bug-878"]);
    write_commit(&worktree, "file.txt", "base\nlocal\n", "local work");

    // The implementer committed locally but did not push. Phase 2 must publish
    // that HEAD before it probes CI or tears down the lease/worktree.
    // trace:BUG-878 | ai:codex
    ensure_implementer_branch_pushed(&worktree, "bug-878", true).unwrap();

    let local_head = git(&worktree, &["rev-parse", "HEAD"]);
    let remote_head = git(&remote, &["rev-parse", "bug-878"]);
    assert_eq!(remote_head, local_head);
    assert_eq!(git(&worktree, &["rev-list", "--count", "@{u}..HEAD"]), "0");
}

#[test]
fn phase2_push_guard_pushes_branch_with_no_upstream() {
    let (_tmp, worktree, remote) = git_repo_with_origin();
    write_commit(&worktree, "file.txt", "only local\n", "local root");

    // No upstream is also unpushed work: publish and establish tracking rather
    // than allowing session teardown to discard the only worktree copy.
    // trace:BUG-878 | ai:codex
    ensure_implementer_branch_pushed(&worktree, "bug-878", true).unwrap();

    let local_head = git(&worktree, &["rev-parse", "HEAD"]);
    let remote_head = git(&remote, &["rev-parse", "bug-878"]);
    assert_eq!(remote_head, local_head);
    assert_eq!(git(&worktree, &["rev-list", "--count", "@{u}..HEAD"]), "0");
}

#[test]
fn phase2_push_guard_failure_leaves_worktree_intact_and_ahead() {
    let (_tmp, worktree, _remote) = git_repo_with_origin();
    write_commit(&worktree, "file.txt", "base\n", "base");
    git(&worktree, &["push", "-q", "-u", "origin", "bug-878"]);
    write_commit(&worktree, "file.txt", "base\nlocal\n", "local work");
    git(
        &worktree,
        &[
            "remote",
            "set-url",
            "origin",
            "/definitely/missing/origin.git",
        ],
    );

    // A failed push is a hard phase-2 failure. Because this guard runs before
    // `aida session end`, the worktree and its local commit are still present.
    // trace:BUG-878 | ai:codex
    let err = ensure_implementer_branch_pushed(&worktree, "bug-878", true).unwrap_err();
    assert!(err.reason.contains("could not push implementer branch"));
    assert!(worktree.exists());
    assert_eq!(git(&worktree, &["rev-list", "--count", "@{u}..HEAD"]), "1");
}

#[test]
fn orchestrator_pr_body_names_orchestrator_opened_fallback() {
    let (title, body) = orchestrator_pr_title_and_body(
        "[AI:codex] fix(orchestrator): recover missing PR (BUG-893)\n\n\
         Test plan:\n\
         - cargo test -p aida-cli-lib real_phase_driver_wiring_tests",
    )
    .unwrap();

    assert_eq!(
        title,
        "[AI:codex] fix(orchestrator): recover missing PR (BUG-893)"
    );
    assert!(
        body.contains("Opened by the AIDA orchestrator"),
        "body should make orchestrator authorship explicit: {body}"
    );
    assert!(
        body.contains("Test plan:"),
        "body should preserve the commit body beneath the orchestrator note: {body}"
    );
}

#[test]
fn phase1_no_pr_recovery_uses_implementer_worktree_branch_state() {
    let tmp = tempfile::tempdir().unwrap();
    let remote = tmp.path().join("origin.git");
    let root = tmp.path().join("root");
    let implementer = tmp.path().join("implementer");

    git(tmp.path(), &["init", "--bare", "origin.git"]);
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "aida@example.invalid"]);
    git(&root, &["config", "user.name", "AIDA Test"]);
    git(&root, &["checkout", "-q", "-b", "main"]);
    git(
        &root,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    write_commit(&root, "file.txt", "base\n", "base");
    git(&root, &["push", "-q", "-u", "origin", "main"]);

    git(
        tmp.path(),
        &["clone", "-q", remote.to_str().unwrap(), "implementer"],
    );
    git(
        &implementer,
        &["config", "user.email", "aida@example.invalid"],
    );
    git(&implementer, &["config", "user.name", "AIDA Test"]);
    git(
        &implementer,
        &["checkout", "-q", "-b", "bug-893", "origin/main"],
    );
    write_commit(
        &implementer,
        "file.txt",
        "base\nfix\n",
        "[AI:codex] fix(orchestrator): recover missing PR (BUG-893)",
    );

    // The orchestrator's main checkout does not have the implementer branch,
    // so the old root-based BUG-459 fallback could not see recoverable work.
    // The BUG-893 path reads the implementer worktree instead, then pushes it
    // before opening the orchestrator-owned PR. trace:BUG-893 | ai:codex
    assert_eq!(branch_commits_ahead_main(&root, "bug-893"), None);
    assert_eq!(branch_commits_ahead_main(&implementer, "bug-893"), Some(1));
    ensure_implementer_branch_pushed(&implementer, "bug-893", true).unwrap();
    assert_eq!(
        git(&remote, &["rev-parse", "bug-893"]),
        git(&implementer, &["rev-parse", "HEAD"])
    );
}

#[test]
fn phase3_no_pr_recovery_uses_pushed_branch_after_worktree_teardown() {
    let tmp = tempfile::tempdir().unwrap();
    let remote = tmp.path().join("origin.git");
    let root = tmp.path().join("root");
    let implementer = tmp.path().join("implementer");

    git(tmp.path(), &["init", "--bare", "origin.git"]);
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "aida@example.invalid"]);
    git(&root, &["config", "user.name", "AIDA Test"]);
    git(&root, &["checkout", "-q", "-b", "main"]);
    git(
        &root,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    write_commit(&root, "file.txt", "base\n", "base");
    git(&root, &["push", "-q", "-u", "origin", "main"]);

    git(
        tmp.path(),
        &["clone", "-q", remote.to_str().unwrap(), "implementer"],
    );
    git(
        &implementer,
        &["config", "user.email", "aida@example.invalid"],
    );
    git(&implementer, &["config", "user.name", "AIDA Test"]);
    git(
        &implementer,
        &["checkout", "-q", "-b", "bug-895", "origin/main"],
    );
    write_commit(
        &implementer,
        "file.txt",
        "base\nphase3\n",
        "[AI:codex] fix(orchestrator): recover pushed branch PR (BUG-895)",
    );
    ensure_implementer_branch_pushed(&implementer, "bug-895", true).unwrap();

    std::fs::remove_dir_all(&implementer).unwrap();
    git(
        &root,
        &[
            "fetch",
            "-q",
            "origin",
            "bug-895:refs/remotes/origin/bug-895",
        ],
    );

    // Phase 3 no longer has the implementer worktree, so recovery must prove
    // origin/<branch> is ahead from the orchestrator checkout and derive the
    // PR title/body from that pushed head. trace:BUG-895 | ai:codex
    assert_eq!(
        pushed_branch_commits_ahead_default(&root, "bug-895")
            .expect("pushed branch should be comparable"),
        1
    );
    let commit_msg = head_commit_message(&root, "origin/bug-895").unwrap();
    let (title, body) = orchestrator_pr_title_and_body(&commit_msg).unwrap();
    assert_eq!(
        title,
        "[AI:codex] fix(orchestrator): recover pushed branch PR (BUG-895)"
    );
    assert!(body.contains("Opened by the AIDA orchestrator"));
}

#[test]
fn discover_lease_zero_candidates_reports_no_session_started() {
    // Multiplicity 0: no lease ever appeared. The failure message tells the
    // operator the session never started — never lists phantom candidates.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join(".aida").join("sessions")).unwrap();
    let d = driver(root, "TASK-262");
    let err = d
        .discover_orchestrated_lease("aaaaaaaa-1111-7000-8000-000000000000")
        .unwrap_err();
    assert!(
        err.reason.contains("no session lease appeared"),
        "0-candidate failure should say the session never started; got {:?}",
        err.reason
    );
}

#[test]
fn discover_lease_one_match_resolves_branch_and_worktree() {
    // Multiplicity 1: the orchestrated session's claude id pins exactly one
    // lease; discovery returns its (id, branch, worktree).
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let claude_id = "bbbbbbbb-2222-7000-8000-000000000000";
    mint_lease(root, "019e9999-cccc", "task-262-branch", Some(claude_id));
    // Cross-check the underlying free fn agrees with the method seam.
    assert_eq!(
        find_orchestrated_lease(root, claude_id),
        Some((
            "019e9999-cccc".to_string(),
            "task-262-branch".to_string(),
            std::path::PathBuf::from("/tmp/019e9999-cccc"),
            None,
        )),
    );
    let d = driver(root, "TASK-262");
    let (lease_id, branch, worktree) = d.discover_orchestrated_lease(claude_id).unwrap();
    assert_eq!(lease_id, "019e9999-cccc");
    assert_eq!(branch, "task-262-branch");
    assert_eq!(worktree, std::path::PathBuf::from("/tmp/019e9999-cccc"));
}

#[test]
fn discover_lease_n_candidates_unmatched_id_lists_them_diagnostically() {
    // Multiplicity N: several concurrent leases on disk, but the
    // orchestrator's claude id matches none of them (e.g. the manifest
    // write raced). The failure must (a) suggest bare `--resume` and
    // (b) list the live lease ids for diagnosis only.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    mint_lease(
        root,
        "019e1111-aaaa",
        "feature-a",
        Some("11111111-aaaa-7000-8000-000000000000"),
    );
    mint_lease(root, "019e2222-bbbb", "feature-b", None);
    let d = driver(root, "TASK-262");
    let err = d
        .discover_orchestrated_lease("no-such-claude-id")
        .unwrap_err();
    assert!(
        err.reason.contains("--resume"),
        "N-candidate failure should suggest bare --resume; got {:?}",
        err.reason
    );
    assert!(
        err.reason.contains("019e1111-aaaa") && err.reason.contains("019e2222-bbbb"),
        "N-candidate failure should list the live lease ids; got {:?}",
        err.reason
    );
}

#[cfg(unix)]
#[test]
fn headless_implementer_empty_launch_retries_and_releases_leases() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    let stub = root.join("aida-stub");
    std::fs::write(
        &stub,
        r#"#!/usr/bin/env bash
set -eu
if [ "${1:-}" = "session" ] && [ "${2:-}" = "end" ]; then
  lease="${3:-}"
  rm -f ".aida/sessions/${lease}.toml" ".aida/sessions/${lease}.manifest.toml"
  exit 0
fi
spec=""
sid=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "work" ]; then
    spec="$arg"
    prev=""
    continue
  fi
  if [ "$prev" = "--session-id" ]; then
    sid="$arg"
    prev=""
    continue
  fi
  prev="$arg"
done
lease="lease-${sid//-/}"
worktree="$PWD/worktrees/$lease"
mkdir -p .aida/sessions .aida/headless-logs "$worktree"
: > ".aida/headless-logs/bug-826-${sid}.jsonl"
cat > ".aida/sessions/${lease}.toml" <<EOF
id = "$lease"
scope = "$spec"
slug = "bug-826"
owner = "test"
worktree_path = "$worktree"
branch = "bug-826-stub"
started_at = "2026-09-03T00:00:00Z"
hostname = "test"
EOF
cat > ".aida/sessions/${lease}.manifest.toml" <<EOF
session_id = "$lease"
planned_at = "2026-09-03T00:00:00Z"
plan_source = "queue work"
claude_session_id = "$sid"
items = []
EOF
printf '%s\n' "$sid" >> .aida/attempts
exit 1
"#,
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(&stub).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&stub, permissions).unwrap();

    let mut d = driver(root, "BUG-826");
    d.aida_exe = stub;
    d.no_human = Some(crate::auto_complete::NoHumanMode::Both);
    d.drain_tuning.no_progress = std::time::Duration::ZERO;
    d.drain_tuning.ceiling = std::time::Duration::ZERO;

    let err = d.run_implementer().unwrap_err();
    assert!(
        err.reason.contains("before emitting any output"),
        "final failure should name the empty-log launch death: {}",
        err.reason
    );
    let attempts = std::fs::read_to_string(root.join(".aida/attempts")).unwrap();
    assert_eq!(
        attempts.lines().count(),
        3,
        "the empty-log launch schedule should retry twice before failing"
    );
    assert!(
        list_leases(root).is_empty(),
        "empty launch cleanup must leave no surviving lease"
    );
}

#[cfg(unix)]
#[test]
fn empty_launch_release_removes_the_matching_lease() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let session_id = "bbbbbbbb-2222-7000-8000-000000000000";
    mint_lease(root, "019e3333-empty", "bug-826-branch", Some(session_id));
    std::fs::create_dir_all(root.join(".aida").join("headless-logs")).unwrap();
    std::fs::write(
        root.join(".aida")
            .join("headless-logs")
            .join(format!("bug-826-{session_id}.jsonl")),
        "",
    )
    .unwrap();

    assert!(headless_log_is_zero_bytes(root, session_id));
    let mut d = driver(root, "BUG-826");
    let stub = root.join("aida-session-end-stub");
    std::fs::write(
        &stub,
        r#"#!/usr/bin/env bash
set -eu
if [ "${1:-}" = "session" ] && [ "${2:-}" = "end" ]; then
  lease="${3:-}"
  rm -f ".aida/sessions/${lease}.toml" ".aida/sessions/${lease}.manifest.toml"
  exit 0
fi
exit 1
"#,
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(&stub).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&stub, permissions).unwrap();
    d.aida_exe = stub;
    d.release_empty_launch_lease(session_id);
    assert!(
        find_orchestrated_lease(root, session_id).is_none(),
        "released empty launch must not be discoverable as a live lease"
    );
    assert!(
        list_leases(root).is_empty(),
        "released empty launch must not leave a scope-blocking lease"
    );
}
