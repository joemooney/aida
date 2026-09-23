use super::{
    agent_gate_matches_req, branch_commits_ahead_main, build_auto_punt_args,
    build_integrate_rebase_args, build_phase3_auto_rebase_args, decide_shelve_attribution,
    dispatched_branch_head_sha, ensure_implementer_branch_pushed, find_orchestrated_lease,
    head_commit_message, headless_log_is_zero_bytes, lease_path, list_leases,
    orchestrated_lease_receipt_path, orchestrator_phase_child_env, orchestrator_pr_title_and_body,
    parse_agent_gates_from_config, prepare_orchestrated_lease_receipt,
    publish_orchestrated_lease_receipt_from_env, pushed_branch_commits_ahead_default,
    read_commits_in_range, resolve_shelve_gate_range, try_open_orchestrator_pr_for_no_pr_worktree,
    watchdog_failure_with_committed_work, AgentGateOnFail, RealPhaseDriver, SessionLease,
    ShelveAttribution, ORCHESTRATED_LEASE_RECEIPT_ENV,
};
use crate::auto_complete::{FailureKind, Phase, PhaseDriver, PhaseFailure, PhaseReconcile};
use aida_core::{
    DatabaseBackend, FieldChange, HistoryEntry, Requirement, RequirementStatus, RequirementType,
    RequirementsStore,
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

fn fixture_lease(root: &std::path::Path, lease_id: &str) -> SessionLease {
    toml::from_str(&std::fs::read_to_string(lease_path(root, lease_id)).unwrap()).unwrap()
}

fn publish_fixture_receipt(
    root: &std::path::Path,
    claude_id: &str,
    lease: &SessionLease,
) -> std::path::PathBuf {
    let _guard = crate::test_env::env_lock();
    let mut child = Command::new("true");
    let receipt = prepare_orchestrated_lease_receipt(&mut child, root, claude_id);
    assert!(receipt.parent().unwrap().is_dir());
    assert!(!receipt.exists(), "preparation must remove a stale receipt");
    let inherited = child
        .get_envs()
        .find_map(|(key, value)| {
            (key == ORCHESTRATED_LEASE_RECEIPT_ENV).then(|| value.unwrap().to_os_string())
        })
        .expect("phase child inherits the receipt path");
    assert_eq!(std::path::PathBuf::from(&inherited), receipt);

    let previous = std::env::var_os(ORCHESTRATED_LEASE_RECEIPT_ENV);
    std::env::set_var(ORCHESTRATED_LEASE_RECEIPT_ENV, &inherited);
    let result = publish_orchestrated_lease_receipt_from_env(Some(claude_id), lease);
    match previous {
        Some(value) => std::env::set_var(ORCHESTRATED_LEASE_RECEIPT_ENV, value),
        None => std::env::remove_var(ORCHESTRATED_LEASE_RECEIPT_ENV),
    }
    result.unwrap();
    assert!(
        receipt.is_file(),
        "queue-work hook must publish the receipt"
    );
    receipt
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
    write_executable(&path, body);
    path
}

fn write_executable(path: &std::path::Path, body: &str) {
    std::fs::write(&path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
    }
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
fn agent_gate_config_parses_dotted_pipeline_gate_tables() {
    let cfg: toml::Value = r#"
[pipeline.gate.security-review]
kind = "agent"
role = "security-reviewer"
applies_to = "tag:security"
on_fail = "warn"

[pipeline.gate.docs-review]
kind = "agent"
role = "docs-reviewer"
applies_to = "type:doc"
"#
    .parse()
    .unwrap();

    let gates = parse_agent_gates_from_config(Some(&cfg));

    assert_eq!(gates.len(), 2);
    let docs = gates
        .iter()
        .find(|gate| gate.name == "docs-review")
        .expect("docs gate");
    assert_eq!(docs.role, "docs-reviewer");
    assert_eq!(docs.on_fail, AgentGateOnFail::Shelve);
    let security = gates
        .iter()
        .find(|gate| gate.name == "security-review")
        .expect("security gate");
    assert_eq!(security.role, "security-reviewer");
    assert_eq!(security.on_fail, AgentGateOnFail::Warn);
}

#[test]
fn agent_gate_selector_matches_type_or_tag_tokens() {
    let cfg: toml::Value = r#"
[pipeline.gate.security-review]
kind = "agent"
role = "security-reviewer"
applies_to = "type:bug, tag:security"
"#
    .parse()
    .unwrap();
    let gate = parse_agent_gates_from_config(Some(&cfg))
        .pop()
        .expect("gate parses");
    let mut req = Requirement::new("secure story".to_string(), String::new());
    req.req_type = RequirementType::Story;

    assert!(!agent_gate_matches_req(&gate, &req));
    req.tags.insert("security".to_string());
    assert!(agent_gate_matches_req(&gate, &req));
    req.tags.clear();
    req.req_type = RequirementType::Bug;
    assert!(agent_gate_matches_req(&gate, &req));
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
fn watchdog_shelve_names_reviewable_branch_and_commit_count() {
    let (_tmp, worktree, _remote) = git_repo_with_origin();
    git(&worktree, &["branch", "-m", "bug-1450"]);
    write_commit(&worktree, "file.txt", "base\n", "base");
    git(&worktree, &["branch", "main"]);
    git(&worktree, &["push", "-q", "origin", "main"]);
    write_commit(
        &worktree,
        "file.txt",
        "base\nreviewable\n",
        "[AI:codex] fix(orchestrator): preserve watchdog work (BUG-1450)",
    );

    let failure = PhaseFailure::of(
        FailureKind::Watchdog,
        "the implementer phase watchdog stopped the session — ceiling exceeded",
    );
    let enriched = watchdog_failure_with_committed_work(failure, &worktree, "bug-1450");

    assert!(enriched.reason.contains("left 1 committed commit(s)"));
    assert!(enriched.reason.contains("reviewable branch `bug-1450`"));
    assert!(enriched
        .hint_override
        .as_deref()
        .unwrap()
        .contains("open or recover its PR"));
}

#[test]
fn watchdog_shelve_explicitly_distinguishes_no_committed_work() {
    let (_tmp, worktree, _remote) = git_repo_with_origin();
    git(&worktree, &["branch", "-m", "bug-1450"]);
    write_commit(&worktree, "file.txt", "base\n", "base");
    git(&worktree, &["branch", "main"]);
    git(&worktree, &["push", "-q", "origin", "main"]);

    let failure = PhaseFailure::of(
        FailureKind::Watchdog,
        "the implementer phase watchdog stopped the session — ceiling exceeded",
    );
    let enriched = watchdog_failure_with_committed_work(failure, &worktree, "bug-1450");

    assert!(enriched.reason.ends_with("left no committed work"));
}

#[cfg_attr(
    windows,
    ignore = "fake aida/gh harness is a bash script; Windows cannot execute shebang fixtures"
)]
#[test]
fn phase1_nonzero_implementer_exit_with_open_pr_still_proceeds() {
    let tmp = tempfile::tempdir().unwrap();
    let remote = tmp.path().join("origin.git");
    let root = tmp.path().join("root");
    let branch = "task-1224";

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
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida").join("config.toml"),
        "[forge]\nprovider = \"github\"\n",
    )
    .unwrap();
    write_commit(&root, "README.md", "fixture\n", "chore: init");
    git(&root, &["push", "-q", "-u", "origin", "main"]);
    git(&root, &["checkout", "-q", "-b", branch]);

    let fake_aida = tmp.path().join("aida");
    write_executable(
        &fake_aida,
        r#"#!/usr/bin/env bash
set -euo pipefail
session_id=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --session-id)
      session_id="$2"
      shift 2
      ;;
    *)
      shift
      ;;
  esac
done
if [[ -z "$session_id" ]]; then
  echo "missing --session-id" >&2
  exit 64
fi
branch="${AIDA_FAKE_BRANCH:?}"
worktree="${AIDA_FAKE_WORKTREE:?}"
mkdir -p .aida/sessions .aida/headless-logs
printf '{"type":"assistant","message":{"content":[{"type":"text","text":"fake work done"}]}}\n' > ".aida/headless-logs/${branch}-${session_id}.jsonl"
printf 'implemented\n' > implemented.txt
git add implemented.txt
git commit -q -m "[AI:codex] test(orchestrator): fake implementation (TASK-1224)"
git push -q -u origin HEAD:"$branch"
lease_id="lease-task-1224"
cat > ".aida/sessions/${lease_id}.toml" <<EOF
id = "${lease_id}"
scope = "TASK-1224"
slug = "task-1224"
owner = "codex@example.test"
worktree_path = "${worktree}"
branch = "${branch}"
started_at = "2026-09-13T00:00:00Z"
hostname = "test"
role = "implementer"
EOF
cat > ".aida/sessions/${lease_id}.manifest.toml" <<EOF
session_id = "${lease_id}"
planned_at = "2026-09-13T00:00:00Z"
plan_source = "queue work"
claude_session_id = "${session_id}"
items = []
EOF
exit 1
"#,
    );
    let fake_gh = fake_gh(
        tmp.path(),
        r#"#!/usr/bin/env bash
if [[ "$*" == *"pr list"* && "$*" == *"--head task-1224"* ]]; then
  printf '1813\t[AI:codex] test(orchestrator): fake implementation (TASK-1224)\thttps://github.example.invalid/acme/aida/pull/1813\ttask-1224\n'
  exit 0
fi
if [[ "$*" == *"pr list"* ]]; then
  exit 0
fi
exit 1
"#,
    );
    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("AIDA_TEST_GH_BINARY", fake_gh.to_str().unwrap()),
        ("AIDA_FAKE_BRANCH", branch),
        ("AIDA_FAKE_WORKTREE", root.to_str().unwrap()),
        ("AIDA_EXIT_POLL_MS", "1"),
        ("AIDA_GH_VERIFY_RETRIES", "0"),
    ]);

    let mut driver = driver(&root, "TASK-1224");
    driver.aida_exe = fake_aida;
    driver.no_human = Some(crate::auto_complete::NoHumanMode::Both);

    // The fake implementer commits, pushes, "opens" a PR according to gh,
    // then exits 1. The BUG-1140/TASK-1224 contract is that the non-zero
    // status does not win over the substrate: open PR discovery proceeds to
    // the CI/review path instead of returning a tool-exit PhaseFailure.
    // trace:TASK-1224 | ai:codex
    match driver.run_implementer() {
        Ok(crate::auto_complete::ImplementerOutcome::PrOpened) => {}
        other => panic!("expected PrOpened after non-zero implementer exit, got {other:?}"),
    }
    assert_eq!(driver.pr_number, Some(1813));
    assert_eq!(driver.branch.as_deref(), Some(branch));
    assert_eq!(
        git(&remote, &["rev-parse", branch]),
        git(&root, &["rev-parse", "HEAD"])
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
fn discover_lease_n_candidates_unmatched_id_ignores_unrelated_leases() {
    // Multiplicity N: several concurrent leases on disk, but the
    // orchestrator's claude id matches none of them. They are not actionable
    // and must not be presented as resume candidates. trace:BUG-1485 | ai:codex
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
        !err.reason.contains("019e1111-aaaa") && !err.reason.contains("019e2222-bbbb"),
        "N-candidate failure should ignore unrelated lease ids; got {:?}",
        err.reason
    );
}

#[test]
fn discover_lease_recovers_start_position_after_live_lease_disappears() {
    // BUG-1445 shape: the child gets only far enough to establish its lease,
    // then its lifecycle removes the live files before waitpid returns.
    // trace:BUG-1485 | ai:codex
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let claude_id = "cccccccc-3333-7000-8000-000000000000";
    mint_lease(root, "019e3333-start", "bug-1485-start", Some(claude_id));
    let lease = fixture_lease(root, "019e3333-start");
    let receipt_path = orchestrated_lease_receipt_path(root, claude_id);
    std::fs::create_dir_all(receipt_path.parent().unwrap()).unwrap();
    std::fs::write(&receipt_path, "stale").unwrap();
    publish_fixture_receipt(root, claude_id, &lease);
    std::fs::remove_file(lease_path(root, &lease.id)).unwrap();
    std::fs::remove_file(crate::session_manifest::manifest_path(root, &lease.id)).unwrap();

    let recovered = driver(root, "BUG-1485")
        .discover_orchestrated_lease(claude_id)
        .unwrap();
    assert_eq!(recovered.0, "019e3333-start");
    assert_eq!(recovered.1, "bug-1485-start");
}

#[test]
fn discover_lease_recovers_post_push_position_after_live_lease_disappears() {
    // BUG-1442 shape: completed work is pushed, then the live lease disappears
    // before PR recovery. The durable receipt retains the exact branch and
    // worktree needed by the existing open-PR recovery path.
    // trace:BUG-1485 | ai:codex
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let claude_id = "dddddddd-4444-7000-8000-000000000000";
    let (_tmp, worktree, remote) = git_repo_with_origin();
    write_commit(
        &worktree,
        "finished.txt",
        "done\n",
        "fix: completed implementation",
    );
    ensure_implementer_branch_pushed(&worktree, "bug-878", true).unwrap();
    assert!(
        git(
            remote.parent().unwrap(),
            &[
                "--git-dir",
                remote.to_str().unwrap(),
                "rev-parse",
                "refs/heads/bug-878"
            ]
        )
        .len()
            == 40,
        "fixture must reach the post-push position before lease release"
    );

    mint_lease(root, "019e4444-pushed", "bug-878", Some(claude_id));
    let mut lease = fixture_lease(root, "019e4444-pushed");
    lease.worktree_path = worktree.clone();
    publish_fixture_receipt(root, claude_id, &lease);
    std::fs::remove_file(lease_path(root, &lease.id)).unwrap();
    std::fs::remove_file(crate::session_manifest::manifest_path(root, &lease.id)).unwrap();

    let recovered = driver(root, "BUG-1485")
        .discover_orchestrated_lease(claude_id)
        .unwrap();
    assert_eq!(recovered.1, "bug-878");
    assert_eq!(recovered.2, worktree);
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

/// Build a repo whose `origin` carries BOTH a default branch and a pushed
/// feature branch one commit ahead of it — the exact post-push state phase 2
/// leaves behind before it tears the implementer worktree down.
///
/// `git_repo_with_origin` is not reusable here: it starts directly on
/// `bug-878` with nothing pushed, so there is no origin default ref to
/// measure "ahead" against.
// trace:BUG-1485 | ai:claude
#[cfg(unix)]
fn repo_with_pushed_branch_ahead_of_origin_default(
) -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let remote = tmp.path().join("origin.git");
    let work = tmp.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    git(tmp.path(), &["init", "--bare", "-b", "main", "origin.git"]);
    git(&work, &["init", "-q", "-b", "main"]);
    git(&work, &["config", "user.email", "aida@example.invalid"]);
    git(&work, &["config", "user.name", "AIDA Test"]);
    git(
        &work,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    write_commit(&work, "base.txt", "base\n", "chore: base");
    git(&work, &["push", "-q", "-u", "origin", "main"]);
    // `default_branch_of` shells out to a hard-coded `gh` first and only then
    // probes origin/HEAD, so the probe has to be able to answer.
    git(&work, &["remote", "set-head", "origin", "main"]);
    git(&work, &["checkout", "-q", "-b", "bug-878"]);
    write_commit(
        &work,
        "finished.txt",
        "done\n",
        "fix: completed implementation (BUG-878)",
    );
    git(&work, &["push", "-q", "-u", "origin", "bug-878"]);
    (tmp, work, remote)
}

/// A `gh` stub that answers `pr create` with a real-shaped URL, so PR recovery
/// can run to COMPLETION instead of stopping at the forge boundary.
#[cfg(unix)]
fn fake_gh_that_opens_pr(dir: &std::path::Path, pr_number: u64) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("fake-gh");
    std::fs::write(
        &path,
        format!(
            "#!/usr/bin/env bash\n\
             if [ \"$1\" = \"--version\" ]; then echo 'gh version test'; exit 0; fi\n\
             case \"$*\" in\n\
             \t*\"pr create\"*) echo 'https://github.com/example/aida/pull/{pr_number}'; exit 0 ;;\n\
             esac\n\
             exit 1\n"
        ),
    )
    .unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

/// BUG-1485 F1: post-push PR recovery must not require the recorded worktree
/// to still exist.
///
/// The previous round recovered the BRANCH from the durable receipt and
/// stopped there. That half was never in dispute. THIS is the disputed half —
/// what recovery does next, when it proceeds against a worktree that is gone.
/// Before the fix, `branch_commits_ahead_main` ran `git -C <missing>`, failed,
/// and `unwrap_or(0)` flattened "could not look" into "looked and found
/// nothing", so recovery returned None and the drain fell through to a generic
/// "run /aida-pr inside the session" failure — against a session that no
/// longer existed.
// trace:BUG-1485 | ai:claude
#[cfg(unix)]
#[test]
fn post_push_pr_recovery_completes_when_the_recorded_worktree_is_gone() {
    let (_tmp, work, _remote) = repo_with_pushed_branch_ahead_of_origin_default();

    // The worktree the lease recorded no longer exists — phase 2 removed it.
    let gone = work.parent().unwrap().join("torn-down-worktree");
    assert!(!gone.exists(), "fixture must model a MISSING worktree");

    // The work itself is safe on origin, which is the whole point: there is
    // something to recover, and the old code could not see it.
    assert_eq!(
        pushed_branch_commits_ahead_default(&work, "bug-878").unwrap(),
        1,
        "fixture must leave origin/bug-878 one commit ahead of origin default"
    );
    assert_eq!(
        branch_commits_ahead_main(&gone, "bug-878"),
        None,
        "the old instrument must be blind here — that blindness IS the defect"
    );

    let fake_gh = fake_gh_that_opens_pr(work.parent().unwrap(), 4242);
    let _env =
        crate::test_env::EnvVarsGuard::set(&[("AIDA_TEST_GH_BINARY", fake_gh.to_str().unwrap())]);

    let recovered = try_open_orchestrator_pr_for_no_pr_worktree(
        &work,
        &gone,
        "bug-878",
        crate::forge::ForgeKind::GitHub,
        "BUG-878",
    );

    let (ahead, pr) = recovered.expect(
        "recovery must complete from the pushed branch when the worktree is gone; \
         returning None here is the BUG-1485 failure",
    );
    assert_eq!(ahead, 1, "the recovered ahead-count must come from origin");
    assert_eq!(pr, 4242, "the PR number must come from the forge call");
}

/// The complement, without which the test above is satisfied by a fix that
/// always reports success: when the branch was never pushed there is genuinely
/// nothing to recover, and recovery must still decline rather than invent a PR.
// trace:BUG-1485 | ai:claude
#[cfg(unix)]
#[test]
fn post_push_pr_recovery_declines_when_the_branch_was_never_pushed() {
    let (_tmp, work, _remote) = repo_with_pushed_branch_ahead_of_origin_default();
    let gone = work.parent().unwrap().join("torn-down-worktree");

    let fake_gh = fake_gh_that_opens_pr(work.parent().unwrap(), 4242);
    let _env =
        crate::test_env::EnvVarsGuard::set(&[("AIDA_TEST_GH_BINARY", fake_gh.to_str().unwrap())]);

    let recovered = try_open_orchestrator_pr_for_no_pr_worktree(
        &work,
        &gone,
        "branch-that-was-never-pushed",
        crate::forge::ForgeKind::GitHub,
        "BUG-878",
    );
    assert!(
        recovered.is_none(),
        "no pushed branch means nothing to recover; got {recovered:?}"
    );
}

// A `gh` stub that answers `pr list` (the branch-lookup `aida pr ship`
// reuses) with ONE already-open PR for the head, and `pr create` with a
// DIFFERENT PR number. A test asserting the returned PR is the `pr list`
// number, never the `pr create` one, proves adoption happened instead of a
// second PR being opened. trace:TASK-1443 | ai:claude
#[cfg(unix)]
fn fake_gh_with_existing_open_pr(
    dir: &std::path::Path,
    existing_pr: u64,
    branch: &str,
) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("fake-gh-existing-pr");
    std::fs::write(
        &path,
        format!(
            "#!/usr/bin/env bash\n\
             if [ \"$1\" = \"--version\" ]; then echo 'gh version test'; exit 0; fi\n\
             case \"$*\" in\n\
             \t*\"pr list\"*) echo -e '{existing_pr}\\ttitle\\thttps://github.com/example/aida/pull/{existing_pr}\\t{branch}'; exit 0 ;;\n\
             \t*\"pr create\"*) echo 'https://github.com/example/aida/pull/9999'; exit 0 ;;\n\
             esac\n\
             exit 1\n"
        ),
    )
    .unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

/// TASK-1443: the orchestrator must not open a second PR for a head that
/// already has an open one (#2042 and #2043 shared a head, 106 seconds
/// apart — two drives raced the same branch). Reuses the BUG-1485
/// pushed-branch-worktree-gone fixture; the `gh` stub answers `pr list`
/// with an existing open PR, and `pr create` with a different number so a
/// regression that skips the adopt-check is caught opening PR #9999
/// instead of returning the existing #2042.
// trace:TASK-1443 | ai:claude
#[cfg(unix)]
#[test]
fn post_push_pr_recovery_adopts_existing_open_pr_instead_of_creating_another() {
    let (_tmp, work, _remote) = repo_with_pushed_branch_ahead_of_origin_default();
    let gone = work.parent().unwrap().join("torn-down-worktree");

    let fake_gh = fake_gh_with_existing_open_pr(work.parent().unwrap(), 2042, "bug-878");
    let _env =
        crate::test_env::EnvVarsGuard::set(&[("AIDA_TEST_GH_BINARY", fake_gh.to_str().unwrap())]);

    let recovered = try_open_orchestrator_pr_for_no_pr_worktree(
        &work,
        &gone,
        "bug-878",
        crate::forge::ForgeKind::GitHub,
        "BUG-878",
    );

    let (_ahead, pr) = recovered.expect("an already-open PR must be adopted, not treated as none");
    assert_eq!(
        pr, 2042,
        "must adopt the existing open PR (2042), never call `pr create` for a second one (would be 9999)"
    );
}

/// TASK-1442 follow-up (containment for BUG-1510): the drain's own PR-open
/// recovery path must refuse to open a PR when the branch's commits are
/// trailered for a DIFFERENT spec than the one this drive is for — the exact
/// shape of the BUG-1510 incident (STORY-1391's drain opened PR #2043 whose
/// commits were all trailered BUG-1420). Reuses the same pushed-branch
/// fixture as the BUG-1485 tests above (worktree gone, work safe on origin,
/// commit trailered `(BUG-878)`); only the expected spec passed to recovery
/// differs. No real `gh` involvement is needed because the guard runs BEFORE
/// the forge is ever called — the fake `gh` stub is still wired so a
/// regression that skips the guard would be caught opening PR #4242 instead
/// of refusing.
// trace:TASK-1442 | ai:claude
#[cfg(unix)]
#[test]
fn post_push_pr_recovery_refuses_when_commit_trailer_names_a_different_spec() {
    let (_tmp, work, _remote) = repo_with_pushed_branch_ahead_of_origin_default();
    let gone = work.parent().unwrap().join("torn-down-worktree");

    let fake_gh = fake_gh_that_opens_pr(work.parent().unwrap(), 4242);
    let _env =
        crate::test_env::EnvVarsGuard::set(&[("AIDA_TEST_GH_BINARY", fake_gh.to_str().unwrap())]);

    // The pushed commit is trailered `(BUG-878)` (see the fixture), but this
    // drive claims to be working a DIFFERENT spec — the misattribution.
    let recovered = try_open_orchestrator_pr_for_no_pr_worktree(
        &work,
        &gone,
        "bug-878",
        crate::forge::ForgeKind::GitHub,
        "STORY-1391",
    );
    assert!(
        recovered.is_none(),
        "a branch whose only commit is trailered for a different spec must not get a PR \
         opened under the wrong spec's identity; got {recovered:?}"
    );
}

/// TASK-1444 follow-up: `resolve_shelve_gate_range` must scan the PR's OWN
/// branch, not the caller's current `HEAD`. Reuses the pushed-branch fixture
/// (origin/HEAD set to `main`, branch `bug-878` pushed one commit ahead,
/// trailered `(BUG-878)`), but checks the worktree back out to `main` first —
/// so `HEAD` carries none of that commit. `resolve_gate_range(.., None)`
/// (the pre-TASK-1444 range, `<default>..HEAD`) must come back empty; the
/// branch-aware range must still find the commit on `origin/bug-878`.
// trace:TASK-1444 | ai:claude
#[test]
fn resolve_shelve_gate_range_uses_the_branch_not_current_head() {
    let (_tmp, work, _remote) = repo_with_pushed_branch_ahead_of_origin_default();
    // The fixture leaves the worktree checked out on `bug-878`; move HEAD
    // back to `main` so it no longer carries the fixture's commit.
    git(&work, &["checkout", "-q", "main"]);

    let head_range = super::resolve_gate_range(&work, None);
    let head_commits = read_commits_in_range(&work, &head_range).unwrap();
    assert!(
        head_commits.is_empty(),
        "HEAD is back on main; the HEAD-based range must not see the branch's commit, got {head_commits:?}"
    );

    let branch_range = resolve_shelve_gate_range(&work, Some("bug-878"))
        .expect("origin/bug-878 was pushed by the fixture and must resolve");
    let branch_commits = read_commits_in_range(&work, &branch_range).unwrap();
    assert!(
        branch_commits
            .iter()
            .any(|(_, subject)| subject.contains("BUG-878")),
        "the branch-aware range must find the pushed branch's own commit, got {branch_commits:?}"
    );

    // No branch to resolve at all → Uncertain territory, never a guess.
    assert!(resolve_shelve_gate_range(&work, None).is_none());
    assert!(resolve_shelve_gate_range(&work, Some("no-such-branch")).is_none());
}

/// TASK-1444 / BUG-1510 end-to-end: when a reviewer verdict's commits are
/// confidently trailered for a DIFFERENT spec than the lease, the shelve
/// must still land on the LEASE spec (never silently flip the other one's
/// status) — with a note on the lease's own `FailureReason` explaining the
/// mismatch. Exercises the real wiring (`RealPhaseDriver::shelve_on_failure`
/// → `resolve_shelve_gate_range` → `decide_shelve_attribution` →
/// `shelve_spec_on_failure`), not just the pure decision function.
// trace:TASK-1444 | ai:claude
#[test]
fn shelve_on_failure_always_shelves_the_lease_spec_with_an_attribution_note() {
    let tmp = tempfile::tempdir().unwrap();
    let remote = tmp.path().join("origin.git");
    let work = tmp.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    git(tmp.path(), &["init", "--bare", "-b", "main", "origin.git"]);
    git(&work, &["init", "-q", "-b", "main"]);
    git(&work, &["config", "user.email", "aida@example.invalid"]);
    git(&work, &["config", "user.name", "AIDA Test"]);
    git(
        &work,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    write_commit(&work, "base.txt", "base\n", "chore: base");
    git(&work, &["push", "-q", "-u", "origin", "main"]);
    git(&work, &["remote", "set-head", "origin", "main"]);
    git(&work, &["checkout", "-q", "-b", "story-1391"]);
    // The PR's own commits are ALL trailered for a different spec — the
    // BUG-1510 incident shape.
    write_commit(
        &work,
        "fix.txt",
        "fix\n",
        "fix(orchestrator): address review findings (BUG-1420)",
    );
    git(&work, &["push", "-q", "-u", "origin", "story-1391"]);
    // Move HEAD off the PR branch so a HEAD-based range (the pre-TASK-1444
    // bug) could not possibly see the right commits by accident.
    git(&work, &["checkout", "-q", "main"]);

    std::fs::create_dir_all(work.join(".aida")).unwrap();
    std::fs::write(
        work.join(".aida").join("config.toml"),
        "store_path = \".aida-store\"\n",
    )
    .unwrap();

    let mut req = Requirement::new("uses reviewer feedback".to_string(), String::new());
    req.spec_id = Some("STORY-1391".to_string());
    req.status = RequirementStatus::InProgress;
    let mut store = RequirementsStore::default();
    store.requirements.push(req);
    aida_core::GitBackend::new(&work.join(".aida-store"))
        .unwrap()
        .save(&store)
        .unwrap();

    let mut phase_driver = driver(&work, "STORY-1391");
    phase_driver.branch = Some("story-1391".to_string());

    let failure = PhaseFailure::of(
        FailureKind::VerdictRequestChanges,
        "reviewer requested changes",
    );
    let fr = phase_driver
        .shelve_on_failure(
            "STORY-1391",
            Phase::Reviewer,
            &failure,
            "resolve the reviewer's findings",
        )
        .unwrap()
        .expect("a shelvable InProgress spec must produce a FailureReason");

    assert!(
        fr.detail.contains("BUG-1420") && fr.detail.contains("attribution"),
        "expected the lease's own FailureReason to note the mismatched attribution, got: {}",
        fr.detail
    );

    let reloaded = aida_core::GitBackend::new(&work.join(".aida-store"))
        .unwrap()
        .load()
        .unwrap();
    let lease_req = reloaded
        .requirements
        .iter()
        .find(|r| r.spec_id.as_deref() == Some("STORY-1391"))
        .expect("lease spec must still be in the store");
    assert_eq!(
        lease_req.status,
        RequirementStatus::NeedsAttention,
        "the shelve must always land on the LEASE spec, never silently skip it"
    );
    assert!(lease_req
        .failure_reason
        .as_ref()
        .unwrap()
        .detail
        .contains("BUG-1420"));

    // Sanity check against the pure decision function directly, confirming
    // the wiring and the pure core agree on this shape.
    let range = resolve_shelve_gate_range(&work, Some("story-1391")).unwrap();
    let commits = read_commits_in_range(&work, &range).unwrap();
    assert_eq!(
        decide_shelve_attribution(&commits, "STORY-1391"),
        ShelveAttribution::Reattributed("BUG-1420".to_string())
    );
}

/// BUG-1527: a drain dispatched for one spec must not accept an implementer
/// that swapped to a DIFFERENT spec's branch mid-phase and write Done for
/// the dispatched spec on that other spec's work. The fake `aida` launcher
/// plays the implementer: it records the lease with the ORIGINALLY-dispatched
/// branch (the session-start snapshot `reconcile_orchestrated_branch` reads),
/// but checks the worktree out onto a totally different branch and commits
/// there trailered for a different spec — exactly BUG-1527's incident shape
/// (`bug-1442-work` -> `bug-1485-work`). `run_implementer()` must refuse
/// BEFORE any PR lookup or Done write: no `gh` stub is even wired, so a
/// regression that fell through to the PR-lookup/Done-write path would fail
/// this test by trying (and failing) to spawn `gh`, not just by writing the
/// wrong status.
// trace:BUG-1527 | ai:claude
#[cfg(unix)]
#[test]
fn phase1_refuses_when_implementer_ends_on_another_specs_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let remote = tmp.path().join("origin.git");
    let root = tmp.path().join("root");
    let dispatched_branch = "story-9001-work";

    git(tmp.path(), &["init", "--bare", "-b", "main", "origin.git"]);
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "aida@example.invalid"]);
    git(&root, &["config", "user.name", "AIDA Test"]);
    git(&root, &["checkout", "-q", "-b", "main"]);
    git(
        &root,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    write_commit(&root, "README.md", "fixture\n", "chore: init");
    git(&root, &["push", "-q", "-u", "origin", "main"]);
    git(&root, &["remote", "set-head", "origin", "main"]);
    git(&root, &["checkout", "-q", "-b", dispatched_branch]);

    let fake_aida = tmp.path().join("aida");
    write_executable(
        &fake_aida,
        r#"#!/usr/bin/env bash
set -euo pipefail
session_id=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --session-id)
      session_id="$2"
      shift 2
      ;;
    *)
      shift
      ;;
  esac
done
mkdir -p .aida/sessions .aida/headless-logs
printf '{"type":"assistant","message":{"content":[{"type":"text","text":"fake work done"}]}}\n' > ".aida/headless-logs/${AIDA_FAKE_BRANCH}-${session_id}.jsonl"
# The swap: check out a DIFFERENT branch than the one this phase was
# dispatched for, and commit work trailered for a DIFFERENT spec.
git checkout -q -b "${AIDA_SWAP_BRANCH}"
printf 'swapped\n' > swapped.txt
git add swapped.txt
git commit -q -m "[AI:codex] fix(other): unrelated fix (${AIDA_SWAP_SPEC})"
lease_id="lease-bug-1527"
cat > ".aida/sessions/${lease_id}.toml" <<EOF
id = "${lease_id}"
scope = "STORY-9001"
slug = "story-9001"
owner = "codex@example.test"
worktree_path = "${AIDA_FAKE_WORKTREE}"
branch = "${AIDA_FAKE_BRANCH}"
started_at = "2026-09-23T00:00:00Z"
hostname = "test"
role = "implementer"
EOF
cat > ".aida/sessions/${lease_id}.manifest.toml" <<EOF
session_id = "${lease_id}"
planned_at = "2026-09-23T00:00:00Z"
plan_source = "queue work"
claude_session_id = "${session_id}"
items = []
EOF
exit 0
"#,
    );
    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("AIDA_FAKE_BRANCH", dispatched_branch),
        ("AIDA_SWAP_BRANCH", "bug-9002-work"),
        ("AIDA_SWAP_SPEC", "BUG-9002"),
        ("AIDA_FAKE_WORKTREE", root.to_str().unwrap()),
        ("AIDA_EXIT_POLL_MS", "1"),
        ("AIDA_GH_VERIFY_RETRIES", "0"),
    ]);

    let mut driver = driver(&root, "STORY-9001");
    driver.aida_exe = fake_aida;
    driver.no_human = Some(crate::auto_complete::NoHumanMode::Both);

    let outcome = driver.run_implementer();
    let failure = match outcome {
        Err(f) => f,
        Ok(ok) => {
            panic!("expected the swap to refuse phase 1, got a success outcome instead: {ok:?}")
        }
    };
    assert_eq!(failure.kind, FailureKind::ShippedMismatch);
    assert!(
        failure.reason.contains(dispatched_branch) && failure.reason.contains("bug-9002-work"),
        "expected the failure to name both the dispatched and the swapped-to branch, got: {}",
        failure.reason
    );
    assert!(
        failure.reason.contains("BUG-9002"),
        "expected the failure to name which spec the swapped branch actually credits, got: {}",
        failure.reason
    );

    // The dispatched spec's own branch never moved, and the swapped-to
    // branch's PR was never looked up or touched — `driver.pr_number` stays
    // unset, proving no Done write (which requires a PR) was ever attempted.
    assert_eq!(driver.pr_number, None);
}

// ============================================================================
// TASK-1449: `RealPhaseDriver::rework_no_op_failure` refuses on unknowns and
// compares against the blocking verdict's reviewed_sha on the DISPATCHED
// branch, rather than failing open. Tests set `rework_guard` /
// `phase_done_pr` directly — `begin_rework_guard`'s arming path is exercised
// elsewhere; these cover the guard's own judgment once armed.
//
// TASK-1449 (post-review hardening): `dispatched_branch_head_sha` reads
// ONLY `origin/<branch>` (fetched first, best effort) — no same-named local
// branch fallback — so the fixture below gives every dispatched branch a
// real bare-repo `origin` and PUSHES to it; a commit that never reaches
// origin is invisible to the guard, exactly as in production (a dispatched
// round's PR lives on origin by definition).
// ============================================================================

/// `main` with one commit, plus a DISPATCHED branch forked from it, both
/// pushed to a real bare-repo `origin` — the shape `rework_no_op_failure`
/// judges. Returns the worktree root.
fn rework_fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let bare = tmp.path().join("origin.git");
    let root = tmp.path().join("work");
    std::fs::create_dir_all(&root).unwrap();
    git(tmp.path(), &["init", "--bare", "-q", "origin.git"]);
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["config", "user.email", "aida@example.invalid"]);
    git(&root, &["remote", "add", "origin", bare.to_str().unwrap()]);
    git(&root, &["config", "user.name", "AIDA Test"]);
    write_commit(&root, "README.md", "root\n", "chore: seed (TASK-0)");
    git(&root, &["push", "-q", "origin", "main"]);
    git(&root, &["checkout", "-q", "-b", "task-1449-work"]);
    git(&root, &["push", "-q", "-u", "origin", "task-1449-work"]);
    (tmp, root)
}

/// Push the dispatched branch's current local HEAD to `origin`, exactly as
/// an implementer's round would — the guard reads only `origin/<branch>`.
fn push_dispatched(root: &std::path::Path) {
    git(root, &["push", "-q", "origin", "task-1449-work"]);
}

#[test]
fn rework_no_op_failure_is_none_when_guard_is_not_armed() {
    // A genuine first-round (non-rework) advance must never trip the guard —
    // it is armed only when a blocking verdict exists.
    let (_tmp, root) = rework_fixture();
    let mut d = driver(&root, "TASK-1449");
    d.rework_guard = None;
    d.phase_done_pr = Some(1);
    assert!(d.rework_no_op_failure().is_none());
}

#[test]
fn rework_no_op_fires_when_dispatched_branch_head_equals_reviewed_sha() {
    let (_tmp, root) = rework_fixture();
    write_commit(&root, "impl.rs", "v1\n", "fix: attempt one (TASK-1449)");
    push_dispatched(&root);
    let head = git(&root, &["rev-parse", "HEAD"]);

    let mut d = driver(&root, "TASK-1449");
    d.rework_guard = Some((
        77,
        "task-1449-work".to_string(),
        Some(head),
        None,
        "outstanding review findings".to_string(),
        3,
    ));
    d.phase_done_pr = Some(77);

    let failure = d
        .rework_no_op_failure()
        .expect("an unmoved dispatched branch must refuse to advance");
    assert_eq!(failure.kind, FailureKind::ReworkNoOp);
    assert!(failure.reason.contains("ROUND 3"), "{}", failure.reason);
}

#[test]
fn rework_no_op_fires_when_some_other_head_moved_but_not_the_dispatched_branch() {
    // BUG-1522 AC7 shape: a DIFFERENT branch gains a commit (simulating
    // another PR's head moving) while the DISPATCHED branch sits untouched
    // at the reviewed sha. The guard must still fire.
    let (_tmp, root) = rework_fixture();
    let reviewed_sha = git(&root, &["rev-parse", "HEAD"]);
    git(&root, &["checkout", "-q", "-b", "unrelated-other-pr"]);
    write_commit(&root, "other.rs", "v1\n", "fix: unrelated work (BUG-9998)");
    git(&root, &["checkout", "-q", "task-1449-work"]);

    let mut d = driver(&root, "TASK-1449");
    d.rework_guard = Some((
        77,
        "task-1449-work".to_string(),
        Some(reviewed_sha),
        None,
        "outstanding review findings".to_string(),
        2,
    ));
    d.phase_done_pr = Some(77);

    let failure = d
        .rework_no_op_failure()
        .expect("another branch moving must not excuse the dispatched branch's own no-op");
    assert_eq!(failure.kind, FailureKind::ReworkNoOp);
}

#[test]
fn rework_no_op_passes_on_genuine_new_content_on_the_dispatched_branch() {
    let (_tmp, root) = rework_fixture();
    let reviewed_sha = git(&root, &["rev-parse", "HEAD"]);
    write_commit(
        &root,
        "impl.rs",
        "v2 — real fix\n",
        "fix: address findings (TASK-1449)",
    );
    push_dispatched(&root);
    let after = git(&root, &["rev-parse", "HEAD"]);
    assert_ne!(reviewed_sha, after);

    let mut d = driver(&root, "TASK-1449");
    d.rework_guard = Some((
        77,
        "task-1449-work".to_string(),
        Some(reviewed_sha),
        None,
        "outstanding review findings".to_string(),
        2,
    ));
    d.phase_done_pr = Some(77);

    assert!(
        d.rework_no_op_failure().is_none(),
        "genuine new patch-unique content must be allowed to proceed"
    );
}

#[test]
fn rework_no_op_passes_on_sha_less_verdict_with_a_new_commit() {
    // TASK-1449 (rework, common-path regression): ~86% of verdicts carry no
    // reviewed_sha. A missing sha must NOT itself refuse — the guard falls
    // back to the dispatched branch's head captured at ARM TIME, and a real
    // commit pushed since then must be allowed to proceed.
    let (_tmp, root) = rework_fixture();
    let arm_time_head = git(&root, &["rev-parse", "HEAD"]);
    write_commit(
        &root,
        "impl.rs",
        "v2 — real fix\n",
        "fix: address findings (TASK-1449)",
    );
    push_dispatched(&root);

    let mut d = driver(&root, "TASK-1449");
    d.rework_guard = Some((
        77,
        "task-1449-work".to_string(),
        None, // no reviewed_sha on the blocking verdict
        Some(arm_time_head),
        "outstanding review findings".to_string(),
        2,
    ));
    d.phase_done_pr = Some(77);

    assert!(
        d.rework_no_op_failure().is_none(),
        "a sha-less verdict with a genuine new commit must be allowed to proceed"
    );
}

#[test]
fn rework_no_op_fires_on_sha_less_verdict_with_an_unchanged_head() {
    // TASK-1449 (rework, common-path regression): with no reviewed_sha, the
    // arm-time dispatched-branch head is the fallback baseline. When the
    // round produces no commits at all, that baseline still catches the
    // no-op — falling back does not mean "always pass".
    let (_tmp, root) = rework_fixture();
    let arm_time_head = git(&root, &["rev-parse", "HEAD"]);

    let mut d = driver(&root, "TASK-1449");
    d.rework_guard = Some((
        77,
        "task-1449-work".to_string(),
        None, // no reviewed_sha on the blocking verdict
        Some(arm_time_head),
        "outstanding review findings".to_string(),
        2,
    ));
    d.phase_done_pr = Some(77);

    let failure = d
        .rework_no_op_failure()
        .expect("a sha-less verdict with an unchanged head must still fire as a no-op");
    assert_eq!(failure.kind, FailureKind::ReworkNoOp);
}

#[test]
fn rework_no_op_refuses_only_when_both_sha_and_arm_time_head_are_missing() {
    // TASK-1449 AC3: refuse (UNKNOWN) ONLY when neither the verdict's
    // reviewed_sha nor the arm-time head could be established — never
    // merely because the sha is missing.
    let (_tmp, root) = rework_fixture();
    let mut d = driver(&root, "TASK-1449");
    d.rework_guard = Some((
        77,
        "task-1449-work".to_string(),
        None,
        None,
        "outstanding review findings".to_string(),
        2,
    ));
    d.phase_done_pr = Some(77);

    let failure = d
        .rework_no_op_failure()
        .expect("with no baseline at all, the guard must refuse rather than silently pass");
    assert_eq!(failure.kind, FailureKind::ReworkNoOp);
    assert!(failure.reason.contains("neither"), "{}", failure.reason);
}

#[test]
fn rework_no_op_refuses_when_dispatched_branch_head_is_unreadable() {
    // TASK-1449 AC1: an unreadable head (here: the dispatched branch was
    // never created) is UNKNOWN — refuse, never advance.
    let (_tmp, root) = rework_fixture();
    let mut d = driver(&root, "TASK-1449");
    d.rework_guard = Some((
        77,
        "branch-that-does-not-exist".to_string(),
        Some("deadbeef".repeat(5)),
        None,
        "outstanding review findings".to_string(),
        2,
    ));
    d.phase_done_pr = Some(77);

    let failure = d
        .rework_no_op_failure()
        .expect("an unreadable dispatched-branch head must refuse rather than silently pass");
    assert_eq!(failure.kind, FailureKind::ReworkNoOp);
    assert!(
        failure.reason.contains("could not be read"),
        "{}",
        failure.reason
    );
}

#[test]
fn rework_no_op_fires_even_when_pr_number_is_none_this_round() {
    // TASK-1449 AC4: the Held/Inconclusive `ImplementerOutcome` arms capture
    // no PR (`phase_done_pr` stays `None`). The old `phase_done_pr !=
    // Some(pr)` gate made the guard unreachable there; the dispatched-branch
    // comparison must not depend on `phase_done_pr` at all.
    let (_tmp, root) = rework_fixture();
    let head = git(&root, &["rev-parse", "HEAD"]);

    let mut d = driver(&root, "TASK-1449");
    d.rework_guard = Some((
        77,
        "task-1449-work".to_string(),
        Some(head),
        None,
        "outstanding review findings".to_string(),
        2,
    ));
    d.phase_done_pr = None; // Held/Inconclusive: no PR captured this round.

    let failure = d
        .rework_no_op_failure()
        .expect("a None phase_done_pr must not disarm the guard");
    assert_eq!(failure.kind, FailureKind::ReworkNoOp);
}

#[test]
fn rework_no_op_catches_phase_done_pr_bound_to_another_specs_pr() {
    // TASK-1449 AC3 / the BUG-1527 shape: this round's own `phase_done_pr`
    // names a DIFFERENT, real PR whose commits credit another spec entirely.
    // The old `phase_done_pr != Some(pr) => return None` exit treated that
    // mismatch as license to advance. The attribution check must name it,
    // and the dispatched-branch comparison (unaffected by `phase_done_pr`)
    // must still fire regardless.
    let (_tmp, root) = rework_fixture();
    let reviewed_sha = git(&root, &["rev-parse", "HEAD"]);
    // The fixture's `origin` is a local bare repo (for the other tests'
    // real push/fetch); repoint it at a fake GitHub URL so the driver
    // resolves `ForgeKind::GitHub` below.
    git(
        &root,
        &[
            "remote",
            "set-url",
            "origin",
            "https://github.com/acme/repo.git",
        ],
    );
    git(&root, &["checkout", "-q", "-b", "other-spec-work"]);
    write_commit(
        &root,
        "other.rs",
        "v1\n",
        "fix(x): unrelated fix (BUG-9999)",
    );
    git(&root, &["checkout", "-q", "task-1449-work"]);

    let gh = fake_gh(
        &root,
        r#"#!/usr/bin/env bash
if [[ "${1:-}" == "pr" && "${2:-}" == "view" && "${3:-}" == "999" ]]; then
  cat <<'JSON'
{
  "state": "OPEN",
  "title": "unrelated fix",
  "mergedAt": null,
  "baseRefName": "main",
  "headRefName": "other-spec-work",
  "headRefOid": "deadbeefcafe",
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

    let mut d = driver(&root, "TASK-1449");
    // The driver already cached its `ForgeKind::GitHub` at construction from
    // the remote URL above (needed for `pr_head_ref_best_effort`'s
    // `gh`-mocked `change_metadata` call); drop the (fake, unreachable)
    // remote now so `rework_no_op_failure`'s best-effort `git fetch origin`
    // fails instantly ("no such remote") instead of touching the network.
    git(&root, &["remote", "remove", "origin"]);
    d.rework_guard = Some((
        77,
        "task-1449-work".to_string(),
        Some(reviewed_sha),
        None,
        "outstanding review findings".to_string(),
        2,
    ));
    d.phase_done_pr = Some(999);

    let failure = d
        .rework_no_op_failure()
        .expect("a mismatched, misattributed PR must refuse rather than pass silently");
    assert_eq!(failure.kind, FailureKind::ReworkNoOp);
    assert!(
        failure.reason.contains("PR-999") && failure.reason.contains("not attributed"),
        "expected the misattribution to be named explicitly, got: {}",
        failure.reason
    );
}

#[test]
fn rework_no_op_fires_when_arm_time_local_ref_was_stale_before_fetch() {
    // TASK-1449 (post-review hardening): reproduces the exact regression the
    // reviewer flagged. `root`'s local `origin/task-1449-work` tracking ref
    // is STALE at "arm time" — a SECOND clone of the same bare origin pushed
    // a commit `root` never fetched. Without fetching at arm time, the
    // baseline would be the stale local sha W; after the (genuinely no-op)
    // round, the after-read (which DID fetch) sees the real origin head X —
    // and X differs from W with real content, so a naive comparison reads
    // that as "content changed" and wrongly lets a no-op round through, even
    // though X was already on origin before this round ever started.
    //
    // This exercises the SAME low-level capture `begin_rework_guard` uses
    // (`dispatched_branch_head_sha`) directly for the arm-time read, rather
    // than driving `begin_rework_guard` end-to-end: that needs a real
    // forge-side open PR + a recorded blocking verdict, and `PureGitForge`
    // (the only forge a local bare-repo origin resolves to) never finds a
    // change (`change_for_spec` always returns `NoChange`), so a pure-git
    // fixture cannot arm the guard through the public entry point. The
    // capture helper below is the exact function `begin_rework_guard` calls.
    let (_tmp, root) = rework_fixture();

    // A second clone of the SAME origin, simulating a different process /
    // machine that pushed progress `root` hasn't fetched yet.
    let bare = root
        .parent()
        .unwrap()
        .join("origin.git")
        .to_str()
        .unwrap()
        .to_string();
    let other_clone = root.parent().unwrap().join("other-clone");
    git(
        root.parent().unwrap(),
        &["clone", "-q", &bare, "other-clone"],
    );
    git(
        &other_clone,
        &[
            "checkout",
            "-q",
            "-b",
            "task-1449-work",
            "origin/task-1449-work",
        ],
    );
    git(
        &other_clone,
        &["config", "user.email", "aida@example.invalid"],
    );
    git(&other_clone, &["config", "user.name", "AIDA Test"]);
    write_commit(
        &other_clone,
        "impl.rs",
        "v1 — already-reviewed content\n",
        "fix: prior round's real work (TASK-1449)",
    );
    git(&other_clone, &["push", "-q", "origin", "task-1449-work"]);
    let real_origin_head = git(&other_clone, &["rev-parse", "HEAD"]);

    // `root` never re-fetched — its local `origin/task-1449-work` tracking
    // ref is still the stale seed sha (confirm the staleness is real).
    let stale_local_ref = git(&root, &["rev-parse", "origin/task-1449-work"]);
    assert_ne!(
        stale_local_ref, real_origin_head,
        "the fixture must start genuinely stale for this test to mean anything"
    );

    // The fixed arm-time capture: fetches, so it reads the REAL origin head,
    // not the stale cached ref.
    let arm_time_head = dispatched_branch_head_sha(&root, "task-1449-work")
        .expect("origin/task-1449-work must be readable after a fetch");
    assert_eq!(
        arm_time_head, real_origin_head,
        "arm-time capture must fetch before reading, not trust a stale local ref"
    );

    // The round produces NO further commits — origin/task-1449-work stays at
    // `real_origin_head` for the rest of this test.
    let mut d = driver(&root, "TASK-1449");
    d.rework_guard = Some((
        77,
        "task-1449-work".to_string(),
        None, // sha-less verdict — exercises the arm-time-head fallback too
        Some(arm_time_head),
        "outstanding review findings".to_string(),
        2,
    ));
    d.phase_done_pr = Some(77);

    let failure = d.rework_no_op_failure().expect(
        "a stale arm-time local ref must not manufacture a false 'content changed' — \
             the round is a genuine no-op",
    );
    assert_eq!(failure.kind, FailureKind::ReworkNoOp);
}

/// Minimal repo with a local `main` the tests below branch off. No origin
/// remote needed — `resolve_default_branch_ref` falls back to a plain local
/// `main` branch when no `origin/HEAD` is configured.
// trace:TASK-1457 | ai:claude
fn repo_on_default_branch() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let work = tmp.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "-q", "-b", "main"]);
    git(&work, &["config", "user.email", "aida@example.invalid"]);
    git(&work, &["config", "user.name", "AIDA Test"]);
    write_commit(&work, "base.txt", "base\n", "chore: base");
    (tmp, work)
}

/// TASK-1457 (BUG-1527 follow-up): `classify_branch_swap_attribution` is the
/// exact function `RealPhaseDriver::run_implementer`'s branch-swap gate
/// calls, so exercising it directly here is exercising the real seam without
/// the heavier fake-`aida`-launcher fixture. Three commit shapes that share
/// a swapped-to branch with the dispatched spec's own trailer must all
/// PROCEED (`Confirmed`) — never read as a swap:
///
///   - a same-spec rename (BUG-223): the only commit still trailers the
///     dispatched spec under its new branch name;
///   - a stacked branch: a predecessor spec's commit sits under this spec's
///     own commit, but the dispatched spec's own trailer is still present;
///   - a both-ids branch: a single commit trailers this spec AND another.
///
/// A fourth shape — a same-spec rename whose commits simply have not been
/// trailered yet — must NOT read as `Confirmed` (nothing credits the
/// dispatched spec) but must ALSO not read as the confident `Reattributed`
/// swap a real different-spec trailer produces: it is `Uncertain`, the
/// PRIN-5 "absent evidence" outcome, asserted here as a control alongside a
/// genuine swap.
// trace:BUG-1527 trace:TASK-1457 | ai:claude
#[test]
fn branch_swap_attribution_classifies_rename_stacked_both_ids_and_uncertain() {
    // Same-spec rename (BUG-223): proceeds.
    let (_tmp, work) = repo_on_default_branch();
    git(&work, &["checkout", "-q", "-b", "renamed-branch"]);
    write_commit(&work, "a.txt", "a\n", "fix(x): rename only (STORY-9001)");
    assert_eq!(
        super::classify_branch_swap_attribution(&work, "renamed-branch", "STORY-9001"),
        ShelveAttribution::Confirmed("STORY-9001".to_string()),
        "a same-spec rename must proceed"
    );

    // Stacked branch: a predecessor spec's commit plus this spec's own —
    // proceeds because this spec's own trailer is present somewhere on it.
    let (_tmp2, work2) = repo_on_default_branch();
    git(&work2, &["checkout", "-q", "-b", "stacked-branch"]);
    write_commit(
        &work2,
        "b.txt",
        "b\n",
        "fix(x): predecessor work (BUG-8999)",
    );
    write_commit(
        &work2,
        "c.txt",
        "c\n",
        "fix(x): this spec's work (STORY-9001)",
    );
    assert_eq!(
        super::classify_branch_swap_attribution(&work2, "stacked-branch", "STORY-9001"),
        ShelveAttribution::Confirmed("STORY-9001".to_string()),
        "a stacked branch carrying this spec's own trailer must proceed"
    );

    // Both-ids branch: a single commit trailers this spec AND another —
    // proceeds, same rule.
    let (_tmp3, work3) = repo_on_default_branch();
    git(&work3, &["checkout", "-q", "-b", "both-ids-branch"]);
    write_commit(
        &work3,
        "d.txt",
        "d\n",
        "fix(x): shared fix (STORY-9001) (BUG-8999)",
    );
    assert_eq!(
        super::classify_branch_swap_attribution(&work3, "both-ids-branch", "STORY-9001"),
        ShelveAttribution::Confirmed("STORY-9001".to_string()),
        "a commit trailering both this spec and another must proceed"
    );

    // Control: a genuine swap — the only commit confidently names a
    // DIFFERENT spec and nothing names this one.
    let (_tmp4, work4) = repo_on_default_branch();
    git(&work4, &["checkout", "-q", "-b", "genuine-swap-branch"]);
    write_commit(&work4, "e.txt", "e\n", "fix(other): unrelated (BUG-9002)");
    assert_eq!(
        super::classify_branch_swap_attribution(&work4, "genuine-swap-branch", "STORY-9001"),
        ShelveAttribution::Reattributed("BUG-9002".to_string()),
        "a trailer confidently naming a different spec is a genuine swap"
    );

    // TASK-1457 item 4: a same-spec rename whose commit carries NO trailer
    // at all — absent evidence, not contrary evidence. Must be `Uncertain`,
    // never `Reattributed` (there is nothing here to swap onto).
    let (_tmp5, work5) = repo_on_default_branch();
    git(
        &work5,
        &["checkout", "-q", "-b", "trailerless-rename-branch"],
    );
    write_commit(
        &work5,
        "f.txt",
        "f\n",
        "chore: continue work under the renamed branch",
    );
    assert_eq!(
        super::classify_branch_swap_attribution(&work5, "trailerless-rename-branch", "STORY-9001"),
        ShelveAttribution::Uncertain("no commit on this PR carries a spec-ID trailer".to_string()),
        "a trailer-less rename is absent evidence, not a confirmed swap"
    );
}

/// TASK-1457 item 4, end to end: exercise the exact same fake-`aida`-launcher
/// seam as `phase1_refuses_when_implementer_ends_on_another_specs_branch`,
/// but the swapped-to branch's commit carries NO spec-ID trailer at all —
/// the legitimate-rename-not-yet-trailered shape. Before this change this
/// reported the same confidently-worded "branch swapped mid-phase ... credits
/// BUG-9002" message a genuine swap gets, which is misleading when nothing
/// was ever established about another spec. The phase must still fail this
/// pass (a PR/Done write under unconfirmed attribution is unsafe either way
/// — PRIN-5), but the wording must say attribution is unknown, not assert a
/// swap that was never confirmed.
// trace:BUG-1527 trace:TASK-1457 | ai:claude
#[cfg(unix)]
#[test]
fn phase1_reports_attribution_unknown_for_a_trailerless_rename_not_a_swap() {
    let tmp = tempfile::tempdir().unwrap();
    let remote = tmp.path().join("origin.git");
    let root = tmp.path().join("root");
    let dispatched_branch = "story-9001-work";

    git(tmp.path(), &["init", "--bare", "-b", "main", "origin.git"]);
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "aida@example.invalid"]);
    git(&root, &["config", "user.name", "AIDA Test"]);
    git(&root, &["checkout", "-q", "-b", "main"]);
    git(
        &root,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    write_commit(&root, "README.md", "fixture\n", "chore: init");
    git(&root, &["push", "-q", "-u", "origin", "main"]);
    git(&root, &["remote", "set-head", "origin", "main"]);
    git(&root, &["checkout", "-q", "-b", dispatched_branch]);

    let fake_aida = tmp.path().join("aida");
    write_executable(
        &fake_aida,
        r#"#!/usr/bin/env bash
set -euo pipefail
session_id=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --session-id)
      session_id="$2"
      shift 2
      ;;
    *)
      shift
      ;;
  esac
done
mkdir -p .aida/sessions .aida/headless-logs
printf '{"type":"assistant","message":{"content":[{"type":"text","text":"fake work done"}]}}\n' > ".aida/headless-logs/${AIDA_FAKE_BRANCH}-${session_id}.jsonl"
# BUG-223 rename with no trailer yet — a legitimate rename, not a swap.
git checkout -q -b "${AIDA_SWAP_BRANCH}"
printf 'renamed\n' > renamed.txt
git add renamed.txt
git commit -q -m "chore: continue work under the renamed branch"
lease_id="lease-task-1457"
cat > ".aida/sessions/${lease_id}.toml" <<EOF
id = "${lease_id}"
scope = "STORY-9001"
slug = "story-9001"
owner = "codex@example.test"
worktree_path = "${AIDA_FAKE_WORKTREE}"
branch = "${AIDA_FAKE_BRANCH}"
started_at = "2026-09-23T00:00:00Z"
hostname = "test"
role = "implementer"
EOF
cat > ".aida/sessions/${lease_id}.manifest.toml" <<EOF
session_id = "${lease_id}"
planned_at = "2026-09-23T00:00:00Z"
plan_source = "queue work"
claude_session_id = "${session_id}"
items = []
EOF
exit 0
"#,
    );
    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("AIDA_FAKE_BRANCH", dispatched_branch),
        ("AIDA_SWAP_BRANCH", "story-9001-work-renamed"),
        ("AIDA_FAKE_WORKTREE", root.to_str().unwrap()),
        ("AIDA_EXIT_POLL_MS", "1"),
        ("AIDA_GH_VERIFY_RETRIES", "0"),
    ]);

    let mut driver = driver(&root, "STORY-9001");
    driver.aida_exe = fake_aida;
    driver.no_human = Some(crate::auto_complete::NoHumanMode::Both);

    let outcome = driver.run_implementer();
    let failure = match outcome {
        Err(f) => f,
        Ok(ok) => {
            panic!(
                "expected the trailer-less rename to still fail phase 1 pending attribution, \
                 got a success outcome instead: {ok:?}"
            )
        }
    };
    assert_eq!(failure.kind, FailureKind::ShippedMismatch);
    assert!(
        !failure.reason.contains("swapped"),
        "a trailer-less rename must not be worded as a confirmed swap, got: {}",
        failure.reason
    );
    assert!(
        failure.reason.contains("attribution unknown"),
        "expected the failure to say attribution is unknown, not assert a swap, got: {}",
        failure.reason
    );
    assert!(
        failure.reason.contains("BUG-223"),
        "expected the failure to name the same-spec-rename possibility, got: {}",
        failure.reason
    );

    // Still no PR lookup or Done write attempted — unconfirmed attribution
    // is unsafe either way (PRIN-5), exactly like the genuine-swap case.
    assert_eq!(driver.pr_number, None);
}

/// TASK-1457 item 3: `ensure_spec_done_after_pr`'s BUG-1527 gate — a PR whose
/// commits confidently credit a DIFFERENT spec must never flip the
/// dispatched spec to Done, even though a PR did open on the branch that was
/// handed to it.
// trace:BUG-1527 trace:TASK-1457 | ai:claude
#[test]
fn ensure_spec_done_after_pr_skips_the_write_when_the_pr_credits_another_spec() {
    let (_tmp, work, store_dir) = fixture_repo_and_store_with_inprogress_spec("STORY-9001");
    git(&work, &["checkout", "-q", "-b", "swap-branch"]);
    write_commit(
        &work,
        "fix.txt",
        "fix\n",
        "fix(other): unrelated (BUG-8888)",
    );

    super::ensure_spec_done_after_pr(&work, &work, "swap-branch", "STORY-9001", 42, true);

    let reloaded = aida_core::GitBackend::new(&store_dir)
        .unwrap()
        .load()
        .unwrap();
    let spec = reloaded
        .requirements
        .iter()
        .find(|r| r.spec_id.as_deref() == Some("STORY-9001"))
        .expect("spec must still be in the store");
    assert_eq!(
        spec.status,
        RequirementStatus::InProgress,
        "a PR that credits a different spec must never flip THIS spec to Done"
    );
}

/// TASK-1457 item 5: pin the decision that a PR carrying NO commit trailer
/// at all ALSO skips the Done write on this (non-swap) path. The BUG-1527
/// gate on `ensure_spec_done_after_pr` is deliberately coarse — see the doc
/// comment on `ensure_pr_open_spec_attribution` — because this function's
/// only job is a write it must never make on unconfirmed attribution;
/// PRIN-5 forbids treating "no evidence either way" as license to write.
/// The finer three-way split (distinct wording for "unknown" vs "swapped")
/// belongs to the branch-swap seam that reports to a human, not to this
/// silent internal gate.
// trace:BUG-1527 trace:TASK-1457 | ai:claude
#[test]
fn ensure_spec_done_after_pr_skips_the_write_when_the_pr_has_no_trailer_at_all() {
    let (_tmp, work, store_dir) = fixture_repo_and_store_with_inprogress_spec("STORY-9001");
    git(&work, &["checkout", "-q", "-b", "untrailered-branch"]);
    write_commit(&work, "fix.txt", "fix\n", "chore: work, no trailer yet");

    super::ensure_spec_done_after_pr(&work, &work, "untrailered-branch", "STORY-9001", 42, true);

    let reloaded = aida_core::GitBackend::new(&store_dir)
        .unwrap()
        .load()
        .unwrap();
    let spec = reloaded
        .requirements
        .iter()
        .find(|r| r.spec_id.as_deref() == Some("STORY-9001"))
        .expect("spec must still be in the store");
    assert_eq!(
        spec.status,
        RequirementStatus::InProgress,
        "a PR carrying no trailer at all must not be treated as confirming attribution — \
         the Done write must be skipped (pinned TASK-1457 decision)"
    );
}

/// Shared fixture for the `ensure_spec_done_after_pr` tests: a bare origin +
/// working repo on `main` with a `.aida-store` git-canonical store seeded
/// with one InProgress spec. Callers branch off `main` and add their own
/// commits before calling `ensure_spec_done_after_pr`.
// trace:TASK-1457 | ai:claude
fn fixture_repo_and_store_with_inprogress_spec(
    spec_id: &str,
) -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let remote = tmp.path().join("origin.git");
    let work = tmp.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    git(tmp.path(), &["init", "--bare", "-b", "main", "origin.git"]);
    git(&work, &["init", "-q", "-b", "main"]);
    git(&work, &["config", "user.email", "aida@example.invalid"]);
    git(&work, &["config", "user.name", "AIDA Test"]);
    git(
        &work,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    write_commit(&work, "base.txt", "base\n", "chore: base");
    git(&work, &["push", "-q", "-u", "origin", "main"]);
    git(&work, &["remote", "set-head", "origin", "main"]);

    std::fs::create_dir_all(work.join(".aida")).unwrap();
    std::fs::write(
        work.join(".aida").join("config.toml"),
        "store_path = \".aida-store\"\n",
    )
    .unwrap();

    let mut req = Requirement::new("dispatched spec".to_string(), String::new());
    req.spec_id = Some(spec_id.to_string());
    req.status = RequirementStatus::InProgress;
    let mut store = RequirementsStore::default();
    store.requirements.push(req);
    let store_dir = work.join(".aida-store");
    aida_core::GitBackend::new(&store_dir)
        .unwrap()
        .save(&store)
        .unwrap();

    (tmp, work, store_dir)
}
