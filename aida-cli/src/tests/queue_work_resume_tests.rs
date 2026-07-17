use super::*;

#[test]
fn resolve_resume_id_unique_prefix() {
    let recorded = vec!["abc123def456".to_string(), "xyz789ghi012".to_string()];
    assert_eq!(resolve_resume_id(&recorded, "abc").unwrap(), "abc123def456");
}

#[test]
fn resolve_resume_id_ambiguous_prefix_errs() {
    let recorded = vec!["abc1".to_string(), "abc2".to_string()];
    assert!(resolve_resume_id(&recorded, "abc").is_err());
}

#[test]
fn resolve_resume_id_passthrough_full_id() {
    // No recorded match, but a UUID-ish-length id → passed through.
    let recorded: Vec<String> = vec![];
    let full = "0123456789abcdef0123";
    assert_eq!(resolve_resume_id(&recorded, full).unwrap(), full);
}

#[test]
fn resolve_resume_id_short_no_match_errs() {
    let recorded: Vec<String> = vec![];
    assert!(resolve_resume_id(&recorded, "abc").is_err());
}

// TASK-402 (friction #1): a pasted AIDA lease id is recognised by shape
// and the error names the right id form instead of the generic miss.
// trace:TASK-402 | ai:claude
#[test]
fn looks_like_lease_id_distinguishes_lease_from_uuid() {
    // AIDA lease ids: hyphenless hex, 8-16 chars.
    assert!(looks_like_lease_id("019e45cfc559"));
    assert!(looks_like_lease_id("019e45cf"));
    // Claude session UUIDs carry hyphens — not a lease id.
    assert!(!looks_like_lease_id("019e45cf-acea-73c1-9476-d044fe19d8e4"));
    // A bare short prefix the user typed is too short / not full hex run.
    assert!(!looks_like_lease_id("abc"));
    assert!(!looks_like_lease_id(""));
    // Non-hex characters disqualify it.
    assert!(!looks_like_lease_id("zzzzzzzz"));
}

#[test]
fn resolve_resume_id_lease_shaped_miss_points_at_uuid() {
    // The user pasted the lease id from the kickoff banner. No recorded
    // session matches, but the message must call out the lease-vs-UUID
    // confusion, not the generic "no recorded session" miss.
    let recorded: Vec<String> = vec!["019e45cf-acea-73c1-9476-d044fe19d8e4".to_string()];
    let err = resolve_resume_id(&recorded, "019e45cfc559").unwrap_err();
    let msg = err.to_string().to_lowercase();
    assert!(msg.contains("lease id"), "msg: {msg}");
    assert!(msg.contains("uuid"), "msg: {msg}");
}

// TASK-402 (friction #5): the resume command is paste-ready — it prepends
// the `cd <worktree>` step only when the recorded worktree differs from
// the current cwd. trace:TASK-402 | ai:claude
#[test]
fn resume_command_prepends_cd_when_worktree_differs() {
    let base = "aida queue work TASK-358 --resume 019e45cf-acea-73c1";
    let cmd = resume_command_with_cwd(
        base,
        Some("/home/joe/ai/aida-task-358"),
        Some("/home/joe/ai/aida"),
    );
    assert_eq!(cmd, format!("cd /home/joe/ai/aida-task-358 && {base}"));
}

#[test]
fn resume_command_omits_cd_when_worktree_matches_cwd() {
    let base = "aida queue work TASK-358 --resume 019e45cf-acea-73c1";
    let cmd = resume_command_with_cwd(base, Some("/home/joe/ai/aida"), Some("/home/joe/ai/aida"));
    assert_eq!(cmd, base);
}

#[test]
fn resume_command_omits_cd_when_worktree_unknown() {
    let base = "aida queue work TASK-358 --resume 019e45cf-acea-73c1";
    let cmd = resume_command_with_cwd(base, None, Some("/home/joe/ai/aida"));
    assert_eq!(cmd, base);
    let cmd_empty = resume_command_with_cwd(base, Some(""), Some("/home/joe/ai/aida"));
    assert_eq!(cmd_empty, base);
}

#[test]
fn queue_work_launch_session_id_accessor() {
    assert_eq!(QueueWorkLaunch::Fresh("f".into()).session_id(), "f");
    assert_eq!(QueueWorkLaunch::Resume("r".into()).session_id(), "r");
}

#[test]
fn queue_work_session_id_override_threads_through() {
    // STORY-132: a caller-minted UUID becomes the Fresh launch id
    // verbatim, instead of the auto-generated `Uuid::now_v7()`.
    let uuid = "019e2cb7-a2cd-7782-a503-f0daf2b8df82";
    let launch = resolve_queue_work_launch("EPIC-26", None, false, Some(uuid)).unwrap();
    assert!(matches!(launch, QueueWorkLaunch::Fresh(_)));
    assert_eq!(launch.session_id(), uuid);
}

#[test]
fn queue_work_rejects_non_uuid_session_id() {
    // STORY-132: a malformed `--session-id` fails clean, before any
    // worktree side effects, with a UUID-flavored message.
    let err = resolve_queue_work_launch("EPIC-26", None, false, Some("not-a-uuid")).unwrap_err();
    assert!(err.to_string().to_lowercase().contains("uuid"));
}

#[test]
fn lease_ids_in_excludes_companion_files() {
    // BUG-114: `.manifest.toml` (always written when `--session-id` is
    // passed — which `--auto-complete` always does) and `.activity.toml`
    // are companions, not leases. Counting them was the phase-1
    // disambiguation failure.
    let dir = tempfile::tempdir().unwrap();
    let s = dir.path();
    let lease = "019e0000-0000-7000-8000-000000000001";
    std::fs::write(s.join(format!("{lease}.toml")), "id = \"x\"\n").unwrap();
    std::fs::write(s.join(format!("{lease}.activity.toml")), "").unwrap();
    std::fs::write(s.join(format!("{lease}.manifest.toml")), "").unwrap();
    std::fs::write(s.join("not-a-lease.txt"), "").unwrap();
    assert_eq!(lease_ids_in(s), vec![lease.to_string()]);
}

#[test]
fn find_orchestrated_lease_pins_session_by_claude_id() {
    // BUG-114 regression: with several concurrent leases on disk — plus
    // the orchestrated session's own `.manifest.toml` companion — the
    // orchestrator must still resolve its session by the claude id it
    // minted, not by diffing the lease set.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let sessions = root.join(".aida").join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();

    let mint = |lease_id: &str, branch: &str, claude_id: Option<&str>| {
        std::fs::write(
            sessions.join(format!("{lease_id}.toml")),
            format!(
                "id = \"{lease_id}\"\nbranch = \"{branch}\"\nworktree_path = \"/tmp/{lease_id}\"\n"
            ),
        )
        .unwrap();
        let manifest = session_manifest::SessionManifest {
            session_id: lease_id.to_string(),
            planned_at: chrono::Utc::now(),
            plan_source: "queue work".to_string(),
            claude_session_id: claude_id.map(str::to_string),
            batch_name: None,
            plan: None,
            items: vec![],
        };
        session_manifest::save(&session_manifest::manifest_path(root, lease_id), &manifest)
            .unwrap();
    };

    // Two unrelated concurrent sessions + the orchestrated one.
    mint(
        "019e1111-aaaa",
        "feature-a",
        Some("aaaaaaaa-1111-7000-8000-000000000000"),
    );
    mint("019e2222-bbbb", "feature-b", None);
    let orchestrated = "019e3333-7000-8000-9000-000000000099";
    mint("019e9999-cccc", "task-259", Some(orchestrated));

    assert_eq!(
        find_orchestrated_lease(root, orchestrated),
        Some((
            "019e9999-cccc".to_string(),
            "task-259".to_string(),
            std::path::PathBuf::from("/tmp/019e9999-cccc"),
        )),
    );
    // An unknown claude id resolves to nothing — the caller turns that
    // into a candidate-naming failure message.
    assert!(find_orchestrated_lease(root, "no-such-id").is_none());
}

/// BUG-223: the pure swap-detection rule. A worktree HEAD that differs
/// from the lease's session-start snapshot is a swap; an unchanged,
/// detached, or undetectable HEAD leaves the recorded branch standing.
#[test]
fn swapped_branch_detects_a_genuine_swap() {
    // /aida-pr's BUG-88 guard moved the commits to a fresh branch.
    assert_eq!(
        swapped_branch(Ok("epic-23-10".to_string()), "epic-23-9"),
        Some("epic-23-10".to_string()),
    );
    // No swap — worktree HEAD still matches the lease snapshot.
    assert_eq!(
        swapped_branch(Ok("epic-23-9".to_string()), "epic-23-9"),
        None
    );
    // Detached HEAD — `rev-parse --abbrev-ref` yields the literal "HEAD".
    assert_eq!(swapped_branch(Ok("HEAD".to_string()), "epic-23-9"), None);
    // Empty / undetectable — trust the recorded branch.
    assert_eq!(swapped_branch(Ok(String::new()), "epic-23-9"), None);
    // git failed (worktree already gone) — trust the recorded branch.
    assert_eq!(
        swapped_branch(Err(anyhow::anyhow!("no worktree")), "epic-23-9"),
        None,
    );
}

/// BUG-223: `update_lease_branch` rewrites the `branch` field and only
/// that field — every other lease field survives the round-trip.
#[test]
fn update_lease_branch_rewrites_only_the_branch() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let sessions = leases_dir(root);
    std::fs::create_dir_all(&sessions).unwrap();
    let lease = SessionLease {
        id: "019eabcd-1234".to_string(),
        scope: "EPIC-23".to_string(),
        slug: "epic-23".to_string(),
        owner: "tester".into(),
        worktree_path: root.to_path_buf(),
        branch: "epic-23-9".to_string(),
        started_at: chrono::Utc::now(),
        hostname: "h".into(),
        role: Some("implementer".into()),
        creator_pid: Some(4242),
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
    };
    std::fs::write(
        sessions.join("019eabcd-1234.toml"),
        toml::to_string_pretty(&lease).unwrap(),
    )
    .unwrap();

    update_lease_branch(root, "019eabcd-1234", "epic-23-10").unwrap();

    let body = std::fs::read_to_string(sessions.join("019eabcd-1234.toml")).unwrap();
    let reloaded: SessionLease = toml::from_str(&body).unwrap();
    assert_eq!(reloaded.branch, "epic-23-10");
    // Every other field survives the rewrite untouched.
    assert_eq!(reloaded.scope, "EPIC-23");
    assert_eq!(reloaded.creator_pid, Some(4242));
    assert_eq!(reloaded.role.as_deref(), Some("implementer"));
}

/// BUG-223 regression: simulate `/aida-pr`'s BUG-88 guard swapping the
/// implementer branch, and verify the orchestrator's reconciliation both
/// returns the live branch and rewrites the stale lease.
#[test]
fn reconcile_orchestrated_branch_follows_a_pr_branch_swap() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let git = |args: &[&str]| {
        let ok = std::process::Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?} failed");
    };
    // A git worktree on `epic-23-9` with one commit.
    git(&["init", "-q", "-b", "epic-23-9"]);
    git(&["config", "user.email", "t@t"]);
    git(&["config", "user.name", "t"]);
    git(&["config", "commit.gpgsign", "false"]);
    git(&["commit", "-q", "--allow-empty", "-m", "seed"]);

    // Lease recorded `epic-23-9` at session-start.
    let sessions = leases_dir(root);
    std::fs::create_dir_all(&sessions).unwrap();
    let lease = SessionLease {
        id: "019eaaaa-bbbb".to_string(),
        scope: "EPIC-23".into(),
        slug: "epic-23".into(),
        owner: "t".into(),
        worktree_path: root.to_path_buf(),
        branch: "epic-23-9".into(),
        started_at: chrono::Utc::now(),
        hostname: "h".into(),
        role: Some("implementer".into()),
        creator_pid: None,
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
    };
    std::fs::write(
        sessions.join("019eaaaa-bbbb.toml"),
        toml::to_string_pretty(&lease).unwrap(),
    )
    .unwrap();

    // No swap yet: reconcile is a no-op, returns the recorded branch.
    assert_eq!(
        reconcile_orchestrated_branch(root, "019eaaaa-bbbb", root, "epic-23-9"),
        "epic-23-9",
    );

    // `/aida-pr`'s BUG-88 guard moves the commits to a fresh branch.
    git(&["checkout", "-q", "-b", "epic-23-10"]);

    // Reconcile now follows the swap and rewrites the lease.
    assert_eq!(
        reconcile_orchestrated_branch(root, "019eaaaa-bbbb", root, "epic-23-9"),
        "epic-23-10",
    );
    let body = std::fs::read_to_string(sessions.join("019eaaaa-bbbb.toml")).unwrap();
    let reloaded: SessionLease = toml::from_str(&body).unwrap();
    assert_eq!(reloaded.branch, "epic-23-10");
}
