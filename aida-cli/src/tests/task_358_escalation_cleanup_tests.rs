use super::*;

/// Build a minimal usable lease, write it to a temp project's
/// `.aida/sessions/<id>.toml`, return both lease + path.
fn write_test_lease(
    project_root: &std::path::Path,
    id: &str,
    scope: &str,
    escalated: bool,
) -> std::path::PathBuf {
    let sessions = leases_dir(project_root);
    std::fs::create_dir_all(&sessions).unwrap();
    let lease = SessionLease {
        id: id.to_string(),
        scope: scope.to_string(),
        slug: scope.to_lowercase(),
        owner: "tester".into(),
        // A non-existent path is fine — the unit tests do not invoke the
        // git-worktree-remove leg of force_cleanup_lease.
        worktree_path: project_root.join(format!("wt-{}", id)),
        branch: format!("br-{}", id),
        started_at: chrono::Utc::now(),
        hostname: "h".into(),
        role: Some("implementer".into()),
        creator_pid: None,
        active_pid: None,
        cargo_target_dir: None,
        parent_project_root: None,
        pr_head_sha: None,
        pr_base_sha: None,
        pr_base_ref: None,
        zen_intent_token: None,
        escalated_to_human: if escalated {
            Some(chrono::Utc::now())
        } else {
            None
        },
        parent_branch: None,
        parent_branch_sha: None,
        review_verb: false,
        claim_verb: false,
    };
    let path = sessions.join(format!("{id}.toml"));
    std::fs::write(&path, toml::to_string_pretty(&lease).unwrap()).unwrap();
    path
}

/// `mark_lease_escalated_to_human` stamps a None marker → Some, and the
/// stamped lease round-trips through TOML so list_leases sees it.
#[test]
fn mark_lease_escalated_to_human_stamps_marker() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let path = write_test_lease(root, "abc123", "TASK-358", false);

    // Sanity: starts unmarked.
    let before: SessionLease = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(before.escalated_to_human.is_none());

    mark_lease_escalated_to_human(root, "abc123").unwrap();

    let after: SessionLease = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(
        after.escalated_to_human.is_some(),
        "the stamp must persist on disk"
    );
}

/// An interactive user lease on the same spec (no marker) is NOT a
/// cleanup target. This is the load-bearing safety property — if the
/// gate dropped, an interactive `aida edit TASK-X --status approved`
/// could nuke a user's own session worktree on TASK-X.
#[test]
fn cleanup_skips_lease_without_marker() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let lease_path = write_test_lease(root, "user01", "TASK-358", /* escalated */ false);

    cleanup_escalated_leases_for_spec(root, "TASK-358");

    assert!(
        lease_path.exists(),
        "an unmarked lease on the same spec must survive the cleanup"
    );
}

/// A marked lease for a *different* spec is also untouched — the
/// per-spec cleanup hook must not touch escalations for other specs.
#[test]
fn cleanup_skips_escalated_lease_for_other_spec() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let lease_path = write_test_lease(root, "esc01", "TASK-999", /* escalated */ true);

    cleanup_escalated_leases_for_spec(root, "TASK-358");

    assert!(
        lease_path.exists(),
        "an escalated lease for an unrelated spec must survive"
    );
}

/// BUG-307: when the feature flag is off, the wrapper always reports
/// `Live` — restores pre-BUG-307 behaviour for an operator who wants
/// the explicit-`--steal` discipline back.
#[test]
fn auto_release_disabled_always_reports_live() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_test_lease(root, "off01", "TASK-307", false);
    let lease = list_leases(root).into_iter().next().unwrap();
    let cfg = orchestrator::OrchestratorConfig {
        auto_release_dormant_leases: false,
        stale_lease_threshold_minutes: 10,
    };
    assert_eq!(
        auto_release_decision_for_lease(root, &lease, &cfg),
        orchestrator::AutoReleaseDecision::Live
    );
}

/// BUG-307: a lease whose creator_pid points at a long-dead process,
/// whose mtime is past the threshold, and whose worktree is missing
/// (worktree_path was never created in the temp dir) auto-releases.
/// This is the canonical "lease leaked from a previous stall" case.
#[test]
fn auto_release_dormant_missing_worktree_is_safely_dormant() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let path = write_test_lease(root, "leak01", "TASK-307", false);
    // Backdate the lease file's mtime past the 10-minute threshold so
    // the freshness gate doesn't pin it. `File::set_modified` is the
    // stable-since-1.75 way to do this without pulling a new crate.
    let stale = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
    let f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    f.set_modified(stale).unwrap();
    drop(f);

    let lease = list_leases(root).into_iter().next().unwrap();
    let cfg = orchestrator::OrchestratorConfig::default();
    let decision = auto_release_decision_for_lease(root, &lease, &cfg);
    assert!(
        matches!(
            decision,
            orchestrator::AutoReleaseDecision::SafelyDormant {
                process_dead: true,
                worktree_missing: true,
                ..
            }
        ),
        "expected SafelyDormant with worktree_missing, got {:?}",
        decision
    );
}

/// BUG-307: a lease whose lease file was just written (mtime ~now)
/// short-circuits to `Live` even with a never-existed PID — the
/// freshness gate protects the brief window where a session_start is
/// still wiring up its shell.
#[test]
fn auto_release_fresh_lease_is_live() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_test_lease(root, "fresh1", "TASK-307", false);
    let lease = list_leases(root).into_iter().next().unwrap();
    let cfg = orchestrator::OrchestratorConfig::default();
    assert_eq!(
        auto_release_decision_for_lease(root, &lease, &cfg),
        orchestrator::AutoReleaseDecision::Live
    );
}

/// A legacy lease TOML written before this field existed deserializes
/// fine with `escalated_to_human` as `None` — backward compatibility.
#[test]
fn legacy_lease_without_field_deserializes_with_none() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let wt = root.join("wt");
    std::fs::create_dir_all(&wt).unwrap();
    let toml_text = format!(
        r#"
id = "legacy01"
scope = "TASK-358"
slug = "task-358"
owner = "u"
worktree_path = "{}"
branch = "task-358"
started_at = "2026-05-19T00:00:00Z"
hostname = "h"
"#,
        // Windows canonical paths contain backslashes, which are escape
        // characters in TOML basic strings. Escape them so this legacy
        // fixture tests optional-field compatibility, not TOML syntax.
        // trace:BUG-346 | ai:codex
        wt.canonicalize()
            .unwrap()
            .to_string_lossy()
            .replace('\\', "\\\\")
    );
    let sessions = leases_dir(&root);
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(sessions.join("legacy01.toml"), toml_text).unwrap();

    let leases = list_leases(&root);
    assert_eq!(leases.len(), 1, "the legacy lease loads");
    assert!(
        leases[0].escalated_to_human.is_none(),
        "field defaults to None when absent"
    );
}
