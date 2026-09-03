use super::{
    build_auto_punt_args, build_integrate_rebase_args, build_phase3_auto_rebase_args,
    find_orchestrated_lease, headless_log_is_zero_bytes, list_leases, orchestrator_phase_child_env,
    RealPhaseDriver,
};
use crate::auto_complete::PhaseDriver;

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
