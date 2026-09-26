//! BUG-1629: auto-complete phase 1 when the child loses its session lease and
//! handoff receipt, plus the single-resolution workspace pin. Every fixture is
//! a tempdir with a stub `aida` binary; nothing touches `~/.aida` or a live
//! store.
// trace:BUG-1629 | ai:claude

use super::{list_leases, orchestrated_lease_receipt_path, RealPhaseDriver};
use crate::auto_complete::{FailureKind, ImplementerOutcome, PhaseDriver};

fn project(temp: &tempfile::TempDir) -> std::path::PathBuf {
    let root = temp.path().join("proj");
    std::fs::create_dir_all(root.join(".aida").join("sessions")).unwrap();
    root
}

fn git(root: &std::path::Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

fn git_project(temp: &tempfile::TempDir) -> std::path::PathBuf {
    let root = project(temp);
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["commit", "-q", "--allow-empty", "-m", "init"]);
    root
}

/// Write a lease file with an explicit scope (the wiring-test fixture uses
/// the branch as scope, which cannot model a same-scope lease).
fn write_lease(root: &std::path::Path, id: &str, scope: &str, branch: &str, worktree: &str) {
    std::fs::write(
        root.join(".aida")
            .join("sessions")
            .join(format!("{id}.toml")),
        format!(
            "id = \"{id}\"\n\
             scope = \"{scope}\"\n\
             slug = \"{branch}\"\n\
             owner = \"test\"\n\
             worktree_path = \"{worktree}\"\n\
             branch = \"{branch}\"\n\
             started_at = \"2026-09-24T00:00:00Z\"\n\
             hostname = \"test\"\n"
        ),
    )
    .unwrap();
}

fn driver(root: &std::path::Path, spec: &str, stub: std::path::PathBuf) -> RealPhaseDriver {
    let mut d = RealPhaseDriver::new(
        root.to_path_buf(),
        spec.to_string(),
        "test-queue".to_string(),
        None,
        true,
        None,
        crate::AutonomyMode::Default,
        "run-token".to_string(),
        false,
        false,
        false,
        false,
        crate::auto_complete::LifecycleSkip::default(),
        crate::auto_complete::AutoCompleteVariant::Full,
    );
    d.aida_exe = stub;
    d.no_human = Some(crate::auto_complete::NoHumanMode::Both);
    d.drain_tuning.no_progress = std::time::Duration::ZERO;
    d.drain_tuning.ceiling = std::time::Duration::ZERO;
    d
}

/// A stub `aida`. Every `queue work` invocation appends its argv to
/// `.aida/attempts`. `first` runs on the first invocation, `rest` on later
/// ones; both see `$sid` (the `--session-id`) and `$spec`.
#[cfg(unix)]
fn stub(root: &std::path::Path, first: &str, rest: &str) -> std::path::PathBuf {
    let path = root.join("aida-stub");
    std::fs::write(
        &path,
        format!(
            r#"#!/usr/bin/env bash
set -eu
if [ "${{1:-}}" = "session" ] && [ "${{2:-}}" = "end" ]; then
  rm -f ".aida/sessions/${{3:-}}.toml" ".aida/sessions/${{3:-}}.manifest.toml"
  exit 0
fi
spec=""; sid=""; prev=""
for arg in "$@"; do
  if [ "$prev" = "work" ]; then spec="$arg"; fi
  if [ "$prev" = "--session-id" ]; then sid="$arg"; fi
  prev="$arg"
done
first=1
[ -f .aida/attempts ] && first=0
printf '%s\n' "$*" >> .aida/attempts
if [ "$first" = 1 ]; then
{first}
else
{rest}
fi
"#
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&path, permissions).unwrap();
    path
}

fn attempts(root: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(root.join(".aida").join("attempts"))
        .map(|body| body.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

fn session_id_of(argv: &str) -> String {
    let mut words = argv.split_whitespace();
    while let Some(word) = words.next() {
        if word == "--session-id" {
            return words.next().unwrap().to_string();
        }
    }
    panic!("no --session-id in {argv}");
}

/// The QCI NFR-56 shape: the child exits in milliseconds with neither its
/// lease nor its handoff receipt while an unrelated lease is live. Phase 1
/// reports the precise state (nothing to resume), launches exactly one clean
/// replacement on the same pinned workspace, and leaves no lease behind.
#[cfg(unix)]
#[test]
fn bug_1629_lost_lease_and_receipt_retries_once_and_leaves_no_orphan() {
    let temp = tempfile::tempdir().unwrap();
    let root = project(&temp);
    let unrelated = temp.path().join("story-52");
    write_lease(
        &root,
        "019e0000-unrelated",
        "STORY-52",
        "story-52",
        &unrelated.display().to_string(),
    );
    let stub = stub(&root, "exit 1", "exit 1");
    let mut d = driver(&root, "NFR-56", stub);

    let err = d.run_implementer().unwrap_err();
    assert_eq!(err.kind, FailureKind::LaunchRefused);
    assert!(
        !err.reason.contains("--resume"),
        "a child that never started has nothing to resume: {}",
        err.reason
    );
    assert!(err.reason.contains("nothing to resume"), "{}", err.reason);
    assert!(
        err.reason
            .contains("single clean replacement launch already ran"),
        "{}",
        err.reason
    );

    let runs = attempts(&root);
    assert_eq!(runs.len(), 2, "exactly one replacement launch: {runs:?}");
    assert_ne!(session_id_of(&runs[0]), session_id_of(&runs[1]));
    let expected_path = temp.path().join("proj-nfr-56");
    for run in &runs {
        assert!(run.contains("--branch nfr-56 "), "{run}");
        assert!(
            run.contains(&format!("--path {}", expected_path.display())),
            "{run}"
        );
        assert!(!run.contains("--force-claim"), "{run}");
    }

    let leases: Vec<String> = list_leases(&root).into_iter().map(|l| l.id).collect();
    assert_eq!(leases, vec!["019e0000-unrelated".to_string()]);
    for run in &runs {
        assert!(!orchestrated_lease_receipt_path(&root, &session_id_of(run)).exists());
    }
}

/// When the replacement launch starts properly, phase 1 continues with it —
/// here the replacement punts, which the orchestrator routes normally.
#[cfg(unix)]
#[test]
fn bug_1629_replacement_launch_that_keeps_its_lease_is_used() {
    let temp = tempfile::tempdir().unwrap();
    let root = project(&temp);
    let rest = r#"lease="lease-${sid//-/}"
cat > ".aida/sessions/${lease}.toml" <<EOF
id = "$lease"
scope = "$spec"
slug = "nfr-56"
owner = "test"
worktree_path = "$PWD/../proj-nfr-56"
branch = "nfr-56"
started_at = "2026-09-24T00:00:00Z"
hostname = "test"
EOF
cat > ".aida/sessions/${lease}.manifest.toml" <<EOF
session_id = "$lease"
planned_at = "2026-09-24T00:00:00Z"
plan_source = "queue work"
claude_session_id = "$sid"
items = []
EOF
printf '{"spec":"%s","category":"design-fork","detail":"pick one"}' "$spec" > "$AIDA_PUNT_SIGNAL_FILE"
exit 0"#;
    let stub = stub(&root, "exit 1", rest);
    let mut d = driver(&root, "NFR-56", stub);

    let outcome = d.run_implementer().expect("the replacement launch is used");
    assert!(
        matches!(outcome, ImplementerOutcome::Punted { .. }),
        "{outcome:?}"
    );
    assert_eq!(attempts(&root).len(), 2);
}

/// The launch uses the workspace the orchestrator checked and pinned, not a
/// second resolution.
#[cfg(unix)]
#[test]
fn bug_1629_launch_uses_the_pinned_workspace_without_re_resolving() {
    let temp = tempfile::tempdir().unwrap();
    let root = project(&temp);
    let stub = stub(&root, "exit 1", "exit 1");
    let mut d = driver(&root, "SPEC-016", stub);
    let pinned = temp.path().join("pinned-spec-016");
    d.pin_implementer_workspace(&pinned.display().to_string(), "spec-016-pinned");

    let _ = d.run_implementer();
    let runs = attempts(&root);
    assert_eq!(runs.len(), 2);
    for run in &runs {
        assert!(run.contains("--branch spec-016-pinned "), "{run}");
        assert!(
            run.contains(&format!("--path {}", pinned.display())),
            "{run}"
        );
    }
}

/// A resolver failure refuses the launch instead of launching an implementer
/// without a pinned worktree (the BUG-1244 hazard of `.ok().flatten()`).
#[cfg(unix)]
#[test]
fn bug_1629_resolver_failure_never_launches_an_unpinned_implementer() {
    let temp = tempfile::tempdir().unwrap();
    let root = project(&temp);
    let stub = stub(&root, "exit 0", "exit 0");
    // Slugifies to empty, so the pickup resolver fails.
    let mut d = driver(&root, "---", stub);

    let err = d.run_implementer().unwrap_err();
    assert_eq!(err.kind, FailureKind::LaunchRefused);
    assert!(err.reason.contains("resolver failed"), "{}", err.reason);
    assert!(attempts(&root).is_empty(), "no child may launch");
}

/// The explicit `--branch` reuse race: a branch that was free when phase 1
/// resolved it but exists at launch is refused, never adopted.
#[cfg(unix)]
#[test]
fn bug_1629_fresh_branch_created_after_resolution_is_refused_not_adopted() {
    let temp = tempfile::tempdir().unwrap();
    let root = git_project(&temp);
    let stub = stub(&root, "exit 0", "exit 0");
    let mut d = driver(&root, "SPEC-016", stub);
    let (worktree, branch) = d.implementer_workspace().unwrap().unwrap();
    assert_eq!(branch, "spec-016");
    d.pin_implementer_workspace(&worktree, &branch);
    git(&root, &["branch", "spec-016"]);

    let err = d.run_implementer().unwrap_err();
    assert_eq!(err.kind, FailureKind::LaunchRefused);
    assert!(err.reason.contains("workspace race"), "{}", err.reason);
    assert!(attempts(&root).is_empty(), "no child may launch");
}

/// A live lease on the spec's scope wins over the pickup resolver, and a
/// workspace taken from that lease is not treated as a fresh branch.
#[test]
fn bug_1629_live_lease_wins_over_the_resolver() {
    let temp = tempfile::tempdir().unwrap();
    let root = git_project(&temp);
    let live = temp.path().join("live-spec-016");
    write_lease(
        &root,
        "019e1629-live",
        "SPEC-016",
        "spec-016-live",
        &live.display().to_string(),
    );
    let mut d = driver(&root, "SPEC-016", root.join("unused-stub"));

    let (worktree, branch) = d.implementer_workspace().unwrap().unwrap();
    assert_eq!(worktree, live.display().to_string());
    assert_eq!(branch, "spec-016-live");
    let (_, resolver_branch) = super::resolve_pickup_workspace(&root, "SPEC-016", "auto").unwrap();
    assert_ne!(branch, resolver_branch, "the lease, not the resolver, wins");

    d.pin_implementer_workspace(&worktree, &branch);
    assert!(
        !d.phase1_workspace.as_ref().unwrap().fresh,
        "a lease-owned branch keeps the TASK-245 reuse"
    );
}

#[test]
fn bug_1629_retry_and_race_decisions() {
    assert!(super::phase1_lost_child_retry_allowed(0));
    assert!(!super::phase1_lost_child_retry_allowed(1));
    assert!(super::phase1_fresh_branch_raced(true, true));
    assert!(!super::phase1_fresh_branch_raced(true, false));
    assert!(!super::phase1_fresh_branch_raced(false, true));
}
