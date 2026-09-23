//! TASK-1451: `aida ps` flags a live seat whose resolved mail identity would
//! fall back to the shell user — visible BEFORE it sends unattributable
//! mail, using the SAME BUG-1533 `resolve_sender` precedence rather than a
//! second, drifting copy of it. Every test here uses an INJECTED environment
//! map / probe closure, never a real `/proc/<pid>/environ` read, so the
//! suite is portable and deterministic. trace:TASK-1451 | ai:claude

use super::*;
use std::collections::HashMap;

// Local copy of `story_696_ps_tests::ps_lease` — sibling test modules can't
// reach each other's private helpers, and duplicating this small fixture is
// cheaper than threading a shared `test_support` module through for one
// function. trace:TASK-1451 | ai:claude
fn ps_lease(id: &str, scope: &str, worktree: std::path::PathBuf) -> SessionLease {
    SessionLease {
        id: id.to_string(),
        scope: scope.to_string(),
        slug: scope.to_ascii_lowercase(),
        owner: "tester".into(),
        worktree_path: worktree,
        branch: scope.to_ascii_lowercase(),
        started_at: chrono::Utc::now(),
        hostname: "h".into(),
        role: Some("implementer".into()),
        creator_pid: None,
        creator_pid_start_time: None,
        active_pid: None,
        active_pid_start_time: None,
        cargo_target_dir: None,
        parent_project_root: None,
        pr_head_sha: None,
        pr_base_sha: None,
        pr_base_ref: None,
        zen_intent_token: None,
        escalated_to_human: None,
        parent_branch: None,
        parent_branch_sha: None,
        review_verb: false,
        claim_verb: false,
        manual_enter_at: None,
    }
}

fn env_map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// One session WITH an identity: `AIDA_AGENT_NAME` set resolves Attributed,
/// matching BUG-1533's precedence (agent_name beats the bare shell user).
#[test]
fn mail_identity_from_env_with_agent_name_is_attributed() {
    let env = env_map(&[("AIDA_AGENT_NAME", "claude-impl-1"), ("USER", "joe")]);
    assert_eq!(
        mail_identity_status_from_env(&env),
        MailIdentityStatus::Attributed
    );
}

/// `AIDA_SESSION_ROLE` alone (no agent name / queue user) still resolves
/// Attributed — the third tier of the BUG-1533 precedence.
#[test]
fn mail_identity_from_env_with_session_role_only_is_attributed() {
    let env = env_map(&[("AIDA_SESSION_ROLE", "advisor")]);
    assert_eq!(
        mail_identity_status_from_env(&env),
        MailIdentityStatus::Attributed
    );
}

/// One session WITHOUT an identity: no `AIDA_AGENT_NAME` / `AIDA_USER` /
/// `AIDA_SESSION_ROLE` in the process environment — this is the gap
/// TASK-1451 exists to surface. Present even though `USER` resolves a shell
/// identity: that's exactly the ambiguous collapse BUG-1533 named.
#[test]
fn mail_identity_from_env_with_no_identity_vars_is_unattributed() {
    let env = env_map(&[("USER", "joe"), ("HOME", "/home/joe")]);
    assert_eq!(
        mail_identity_status_from_env(&env),
        MailIdentityStatus::Unattributed
    );
}

/// An entirely empty environment (nothing at all, not even `USER`) is also
/// Unattributed, not a crash or a different classification.
#[test]
fn mail_identity_from_env_empty_is_unattributed() {
    let env = env_map(&[]);
    assert_eq!(
        mail_identity_status_from_env(&env),
        MailIdentityStatus::Unattributed
    );
}

/// Blank-but-present values (e.g. `AIDA_AGENT_NAME=""`) don't count as set —
/// `resolve_sender` trims and treats empty as absent, and this must fall
/// through the same way.
#[test]
fn mail_identity_from_env_blank_agent_name_is_unattributed() {
    let env = env_map(&[("AIDA_AGENT_NAME", "   "), ("USER", "joe")]);
    assert_eq!(
        mail_identity_status_from_env(&env),
        MailIdentityStatus::Unattributed
    );
}

/// End-to-end through `build_running_work`: a live-pid row whose injected
/// probe resolves Attributed carries that verdict on the `PsRow`.
#[test]
fn ps_row_carries_attributed_mail_identity_when_probe_resolves() {
    let tmp = tempfile::tempdir().unwrap();
    let mut l = ps_lease("l-attributed", "STORY-1", tmp.path().to_path_buf());
    l.active_pid = Some(std::process::id());

    let (rows, _) = build_running_work(
        &[],
        &[l],
        &[],
        chrono::Utc::now(),
        |_| dispatch_health_ps::WorktreeGitProbe::default(),
        |_| None,
        |_| None,
        |_| None,
        |_| None,
        |_| MailIdentityStatus::Attributed,
    );

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].pid, Some(std::process::id()));
    assert_eq!(rows[0].mail_identity, Some(MailIdentityStatus::Attributed));
}

/// End-to-end through `build_running_work`: a live-pid row whose injected
/// probe falls back to the shell user carries `Unattributed` — this is the
/// case `aida ps` must flag before that seat sends mail.
#[test]
fn ps_row_carries_unattributed_mail_identity_when_probe_falls_back() {
    let tmp = tempfile::tempdir().unwrap();
    let mut l = ps_lease("l-unattributed", "STORY-2", tmp.path().to_path_buf());
    l.active_pid = Some(std::process::id());

    let (rows, _) = build_running_work(
        &[],
        &[l],
        &[],
        chrono::Utc::now(),
        |_| dispatch_health_ps::WorktreeGitProbe::default(),
        |_| None,
        |_| None,
        |_| None,
        |_| None,
        |_| MailIdentityStatus::Unattributed,
    );

    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].mail_identity,
        Some(MailIdentityStatus::Unattributed)
    );
}

/// An unreadable environment (another user's process, process already gone,
/// or a non-Linux host) reports Unknown — PRIN-5: never silently "fine".
#[test]
fn ps_row_carries_unknown_mail_identity_when_probe_cannot_read() {
    let tmp = tempfile::tempdir().unwrap();
    let mut l = ps_lease("l-unknown", "STORY-3", tmp.path().to_path_buf());
    l.active_pid = Some(std::process::id());

    let (rows, _) = build_running_work(
        &[],
        &[l],
        &[],
        chrono::Utc::now(),
        |_| dispatch_health_ps::WorktreeGitProbe::default(),
        |_| None,
        |_| None,
        |_| None,
        |_| None,
        |_| MailIdentityStatus::Unknown,
    );

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].mail_identity, Some(MailIdentityStatus::Unknown));
}

/// A row with no live pid backing it has nothing to probe — `mail_identity`
/// stays `None` rather than a guessed verdict, and the probe is never
/// invoked for it.
#[test]
fn ps_row_mail_identity_is_none_without_live_pid() {
    let l = ps_lease(
        "l-no-pid",
        "STORY-4",
        std::path::PathBuf::from("/nonexistent/aida-ps-no-pid"),
    );

    let (rows, _) = build_running_work(
        &[],
        &[l],
        &[],
        chrono::Utc::now(),
        |_| dispatch_health_ps::WorktreeGitProbe::default(),
        |_| None,
        |_| None,
        |_| None,
        |_| None,
        |_| panic!("mail identity probe must not be called for a row with no live pid"),
    );

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].pid, None);
    assert_eq!(rows[0].mail_identity, None);
}
