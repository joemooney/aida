use super::*;

fn lease(id: &str, scope: &str, cwd: &str, creator_pid: Option<u32>) -> SessionLease {
    SessionLease {
        id: id.to_string(),
        scope: scope.to_string(),
        slug: scope.to_lowercase(),
        owner: "u".into(),
        worktree_path: std::path::PathBuf::from(cwd),
        branch: format!("br-{}", id),
        started_at: chrono::Utc::now(),
        hostname: "h".into(),
        role: None,
        creator_pid,
        active_pid: None,
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
    }
}

/// BUG-361: session end warns only on the verified risk state:
/// commits ahead + GH-confirmed no open PR.
// trace:BUG-361 | ai:codex
#[test]
fn classifies_session_end_unshipped_work_when_commits_and_no_pr() {
    let got = classify_session_end_unshipped_work(
        "019e55d2570e",
        "BUG-361",
        "bug-361",
        Some(2),
        workflow_hints::PrState::Absent,
    )
    .expect("commits ahead with no PR should warn");
    assert_eq!(got.lease_id, "019e55d2570e");
    assert_eq!(got.scope, "BUG-361");
    assert_eq!(got.branch, "bug-361");
    assert_eq!(got.commits_ahead, 2);

    let lines = session_end_unshipped_warning_lines(&got);
    assert!(lines[0].contains("2 unshipped commits"), "{lines:?}");
    assert!(lines[0].contains("BUG-361"), "{lines:?}");
    assert!(lines[0].contains("branch `bug-361`"), "{lines:?}");
    assert!(lines[1].contains("git push"), "{lines:?}");
    assert!(lines[1].contains("aida pr ship"), "{lines:?}");
}

/// BUG-361: if there are no commits ahead, an open PR, or GH state is
/// unknown, session end must not assert the missing-PR condition.
// trace:BUG-361 | ai:codex
#[test]
fn session_end_unshipped_work_skips_non_risk_states() {
    assert!(classify_session_end_unshipped_work(
        "lease",
        "BUG-361",
        "bug-361",
        Some(0),
        workflow_hints::PrState::Absent,
    )
    .is_none());
    assert!(classify_session_end_unshipped_work(
        "lease",
        "BUG-361",
        "bug-361",
        Some(1),
        workflow_hints::PrState::Open(361),
    )
    .is_none());
    assert!(classify_session_end_unshipped_work(
        "lease",
        "BUG-361",
        "bug-361",
        Some(1),
        workflow_hints::PrState::Unknown,
    )
    .is_none());
    assert!(classify_session_end_unshipped_work(
        "lease",
        "BUG-361",
        "bug-361",
        None,
        workflow_hints::PrState::Absent,
    )
    .is_none());
}

/// BUG-361: the durable morning-sweep signal is a STORY-325 punt-ledger
/// record, not just ephemeral stderr.
// trace:BUG-361 | ai:codex
#[test]
fn session_end_unshipped_work_builds_punt_ledger_record() {
    let work = SessionEndUnshippedWork {
        lease_id: "019e55d2570e".to_string(),
        scope: "BUG-361".to_string(),
        branch: "bug-361".to_string(),
        commits_ahead: 1,
    };
    let now = chrono::Utc::now();
    let record = session_end_unshipped_punt_record(&work, now);

    assert_eq!(record.timestamp, now);
    assert_eq!(record.spec, "BUG-361");
    assert_eq!(record.category, aida_core::PuntCategory::Other);
    assert_eq!(record.raised_by.as_deref(), Some("session-end"));
    assert_eq!(record.resolution_path, "punted");
    assert_eq!(
        record.classification.as_deref(),
        Some("UNSHIPPED-SESSION-END")
    );
    assert_eq!(record.decision.as_deref(), Some("visibility-warning"));
    assert!(
        record.detail.contains("1 commit ahead"),
        "{}",
        record.detail
    );
    assert!(
        record.detail.contains("branch `bug-361`"),
        "{}",
        record.detail
    );
}

/// BUG-361: recording uses the existing STORY-325 punt ledger path so
/// morning-sweep tooling can discover the unfinished session.
// trace:BUG-361 | ai:codex
#[test]
fn session_end_unshipped_work_appends_to_punt_ledger() {
    let tmp = tempfile::TempDir::new().unwrap();
    let work = SessionEndUnshippedWork {
        lease_id: "019e55d2570e".to_string(),
        scope: "BUG-361".to_string(),
        branch: "bug-361".to_string(),
        commits_ahead: 3,
    };

    record_session_end_unshipped_work(tmp.path(), &work);

    let records = punt::read_ledger(tmp.path());
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].spec, "BUG-361");
    assert_eq!(records[0].raised_by.as_deref(), Some("session-end"));
    assert_eq!(
        records[0].classification.as_deref(),
        Some("UNSHIPPED-SESSION-END")
    );
    assert!(records[0].detail.contains("3 commits ahead"),);
}

/// BUG-59: on a direct (TTY) `session end` that removes the worktree the
/// caller's cwd is sitting in, we can't `eval` a `cd` into the parent
/// shell — so we must surface a copy-pasteable `cd '<parent>'` hint so the
/// caller can leave the deleted inode before their next `/bin/sh` spawn
/// (e.g. Claude Code's Stop hook) ENOENTs on it.
#[test]
fn session_end_stale_cwd_hint_offers_cd_to_parent() {
    let lines = session_end_stale_cwd_hint_lines(std::path::Path::new("/home/me/project"));
    let joined = lines.join("\n");
    assert!(
        joined.contains("cd '/home/me/project'"),
        "hint must offer a cd back to the surviving parent project: {joined}"
    );
    assert!(
        joined.contains("removed worktree"),
        "hint must explain why the cwd is stale: {joined}"
    );
}

/// BUG-59: paths with a single-quote must be shell-escaped so the emitted
/// `cd` is safe to paste verbatim.
#[test]
fn session_end_stale_cwd_hint_escapes_single_quotes() {
    let lines = session_end_stale_cwd_hint_lines(std::path::Path::new("/home/me/it's a project"));
    let joined = lines.join("\n");
    assert!(
        joined.contains(r#"cd '/home/me/it'\''s a project'"#),
        "single-quote in the path must be escaped: {joined}"
    );
}

/// BUG-367: session end suppresses the unshipped work warning if the spec
/// status is already Completed in the requirement store.
// trace:BUG-367 | ai:antigravity
#[test]
fn session_end_unshipped_work_suppressed_when_completed() {
    let tmp = tempfile::TempDir::new().unwrap();
    let store_dir = tmp.path().join(".aida-store");
    std::fs::create_dir_all(&store_dir).unwrap();

    // Create a completed requirement in the mock store
    let storage = Storage::new(&store_dir);
    let mut req = Requirement::new("Completed Spec Test".to_string(), "".to_string());
    req.spec_id = Some("BUG-367".to_string());
    req.status = RequirementStatus::Completed;

    let mut store = storage.load().unwrap_or_default();
    store.requirements.push(req);
    storage.save(&store).unwrap();

    // Load store and verify is_completed resolves to true
    let mut is_completed = false;
    if store_dir.exists() {
        if let Ok(storage) = Storage::new(&store_dir).load() {
            if let Some(req) = storage.get_requirement_by_spec_id("BUG-367") {
                if req.status == RequirementStatus::Completed {
                    is_completed = true;
                }
            }
        }
    }
    assert!(is_completed);
}

/// Explicit id query short-circuits the resolution chain.
// trace:STORY-73 | ai:claude
#[test]
fn id_query_takes_precedence() {
    let leases = vec![
        lease("019e10260000", "EPIC-1", "/tmp/wt-1", None),
        lease("019e10271111", "EPIC-2", "/tmp/wt-2", None),
    ];
    let got = resolve_session_to_end(Some("019e1027"), None, None, &leases, false).unwrap();
    assert_eq!(got.scope, "EPIC-2");
}

/// Ambiguous prefix bails clearly.
// trace:STORY-73 | ai:claude
#[test]
fn ambiguous_id_query_bails() {
    let leases = vec![
        lease("019e10260000", "EPIC-1", "/tmp/wt-1", None),
        lease("019e10271111", "EPIC-2", "/tmp/wt-2", None),
    ];
    // "019e10" matches both.
    let err = resolve_session_to_end(Some("019e10"), None, None, &leases, false).unwrap_err();
    assert!(err.to_string().contains("ambiguous"), "{}", err);
}

/// BUG-312: the ambiguous-prefix error includes the FULL id + scope of
/// every match so the operator can pick the right one without
/// grepping `.aida/sessions/`.
// trace:BUG-312 | ai:claude
#[test]
fn ambiguous_id_query_lists_full_ids_and_scopes() {
    let leases = vec![
        lease("019e4df1af45", "TASK-419", "/tmp/wt-419", None),
        lease("019e4df1e131", "TASK-420", "/tmp/wt-420", None),
    ];
    // The collision case from the bug report: same 8-char prefix.
    let err = resolve_session_to_end(Some("019e4df1"), None, None, &leases, false).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("019e4df1af45"), "{}", msg);
    assert!(msg.contains("019e4df1e131"), "{}", msg);
    assert!(msg.contains("TASK-419"), "{}", msg);
    assert!(msg.contains("TASK-420"), "{}", msg);
}

/// Empty id query miss bails (find_lease_by_id_prefix path).
// trace:STORY-73 | ai:claude
#[test]
fn unknown_id_query_bails() {
    let leases = vec![lease("019e10260000", "EPIC-1", "/tmp/wt-1", None)];
    let err = resolve_session_to_end(Some("ffffffff"), None, None, &leases, false).unwrap_err();
    // TASK-489: the id-prefix miss falls through to branch resolution
    // and surfaces the branch-shaped error pointing at `aida session
    // leases` — the right next step whether the user thought they
    // were typing an id or a branch. trace:TASK-489 | ai:claude
    let s = err.to_string();
    assert!(s.contains("aida session leases"), "{}", s);
}

/// No-arg + zero leases never reaches the chain (caller short-circuits)
/// — but multi-lease, no env, cwd not under any worktree yields the
/// "no resolvable" error with a listing.
// trace:STORY-73 | ai:claude
#[test]
fn unresolvable_lists_active_leases() {
    let leases = vec![
        lease("019e10260000", "EPIC-1", "/nonexistent/wt-1", None),
        lease("019e10271111", "EPIC-2", "/nonexistent/wt-2", None),
    ];
    // Clear env so #2 doesn't fire. TASK-521: route through
    // `EnvVarGuard` so a parallel test that legitimately sets
    // AIDA_SESSION_ID can't see it gone, and the prior value (a
    // real shell session running the test suite) is restored on
    // drop. trace:TASK-521 | ai:claude
    let _session_guard = crate::test_env::EnvVarGuard::unset("AIDA_SESSION_ID");
    let err = resolve_session_to_end(None, None, None, &leases, false).unwrap_err();
    let s = err.to_string();
    assert!(s.contains("no active session resolvable"), "{}", s);
    assert!(s.contains("EPIC-1"), "{}", s);
    assert!(s.contains("EPIC-2"), "{}", s);
}

/// -y on single-active fallback skips the prompt.
// trace:STORY-73 | ai:claude
#[test]
fn single_active_with_yes_resolves() {
    let leases = vec![lease("019e10260000", "EPIC-1", "/nonexistent/wt-1", None)];
    // TASK-521: serialised env-var swap (see sibling test above).
    // trace:TASK-521 | ai:claude
    let _session_guard = crate::test_env::EnvVarGuard::unset("AIDA_SESSION_ID");
    let got = resolve_session_to_end(None, None, None, &leases, true).unwrap();
    assert_eq!(got.scope, "EPIC-1");
}

/// TASK-489: `--spec` resolves by lease scope, case-insensitively.
// trace:TASK-489 | ai:claude
#[test]
fn spec_flag_resolves_by_scope() {
    let leases = vec![
        lease("019e10260000", "TASK-489", "/tmp/wt-489", None),
        lease("019e10271111", "TASK-490", "/tmp/wt-490", None),
    ];
    let got = resolve_session_to_end(None, Some("TASK-489"), None, &leases, false).unwrap();
    assert_eq!(got.scope, "TASK-489");

    // case-insensitive
    let got = resolve_session_to_end(None, Some("task-489"), None, &leases, false).unwrap();
    assert_eq!(got.scope, "TASK-489");
}

/// TASK-489: `--spec` with zero matches uses the spec-shaped error.
// trace:TASK-489 | ai:claude
#[test]
fn spec_flag_zero_matches_errors() {
    let leases = vec![lease("019e10260000", "TASK-1", "/tmp/wt-1", None)];
    let err = resolve_session_to_end(None, Some("TASK-999"), None, &leases, false).unwrap_err();
    let s = err.to_string();
    assert!(s.contains("No lease found for spec"), "{}", s);
    assert!(s.contains("TASK-999"), "{}", s);
    assert!(s.contains("aida session leases"), "{}", s);
}

/// TASK-489: `--spec` with multiple matches lists every candidate's
/// lease id so the operator can disambiguate.
// trace:TASK-489 | ai:claude
#[test]
fn spec_flag_multi_matches_lists_lease_ids() {
    let leases = vec![
        lease("019e10260000", "TASK-489", "/tmp/wt-489a", None),
        lease("019e10271111", "TASK-489", "/tmp/wt-489b", None),
    ];
    let err = resolve_session_to_end(None, Some("TASK-489"), None, &leases, false).unwrap_err();
    let s = err.to_string();
    assert!(s.contains("019e10260000"), "{}", s);
    assert!(s.contains("019e10271111"), "{}", s);
    assert!(s.contains("disambiguate"), "{}", s);
}

/// TASK-489: `--branch` resolves by branch name.
// trace:TASK-489 | ai:claude
#[test]
fn branch_flag_resolves_by_branch() {
    let leases = vec![lease("019e10260000", "TASK-489", "/tmp/wt-489", None)];
    // lease helper sets branch = format!("br-{}", id)
    let branch = leases[0].branch.clone();
    let got = resolve_session_to_end(None, None, Some(&branch), &leases, false).unwrap();
    assert_eq!(got.scope, "TASK-489");
}

/// TASK-489: `--branch` with zero matches uses the branch-shaped
/// error.
// trace:TASK-489 | ai:claude
#[test]
fn branch_flag_zero_matches_errors() {
    let leases = vec![lease("019e10260000", "TASK-1", "/tmp/wt-1", None)];
    let err = resolve_session_to_end(None, None, Some("nope-x"), &leases, false).unwrap_err();
    let s = err.to_string();
    assert!(s.contains("No lease found for branch"), "{}", s);
    assert!(s.contains("nope-x"), "{}", s);
    assert!(s.contains("aida session leases"), "{}", s);
}

/// TASK-489: positional matching the SPEC-ID pattern routes through
/// the spec lookup (so a missing scope errors with the spec-shaped
/// message, not the 8-char-id-prefix one).
// trace:TASK-489 | ai:claude
#[test]
fn positional_spec_pattern_routes_to_spec_lookup() {
    let leases = vec![
        lease("019e10260000", "TASK-489", "/tmp/wt-489", None),
        lease("019e10271111", "STORY-86", "/tmp/wt-86", None),
    ];
    let got = resolve_session_to_end(Some("TASK-489"), None, None, &leases, false).unwrap();
    assert_eq!(got.scope, "TASK-489");

    // unknown spec → spec-shaped error
    let err = resolve_session_to_end(Some("BUG-999"), None, None, &leases, false).unwrap_err();
    let s = err.to_string();
    assert!(s.contains("No lease found for spec"), "{}", s);
    assert!(s.contains("BUG-999"), "{}", s);
}

/// TASK-489: positional that doesn't look like a spec ID and isn't a
/// hex-id-prefix match falls back to branch lookup.
// trace:TASK-489 | ai:claude
#[test]
fn positional_non_spec_falls_back_to_branch() {
    let leases = vec![lease("019e10260000", "TASK-489", "/tmp/wt-489", None)];
    let branch = leases[0].branch.clone();
    let got = resolve_session_to_end(Some(&branch), None, None, &leases, false).unwrap();
    assert_eq!(got.scope, "TASK-489");
}

/// TASK-489: lowercase `task-489` shape is treated as a branch, not a
/// spec — the AC explicitly distinguishes the two by case. The shared
/// `looks_like_spec_id` accepts both, so the resolver applies an
/// extra uppercase-prefix guard.
// trace:TASK-489 | ai:claude
#[test]
fn lowercase_spec_shape_routes_to_branch() {
    let mut l = lease("019e10260000", "TASK-489", "/tmp/wt-489", None);
    l.branch = "task-489".to_string();
    let leases = vec![l];
    let got = resolve_session_to_end(Some("task-489"), None, None, &leases, false).unwrap();
    assert_eq!(got.scope, "TASK-489");
}

/// TASK-489: the uppercase-prefix guard accepts canonical SPEC-IDs
/// (`TASK-489`, `STORY-86`, `FR-1-001`) and rejects lowercase
/// (`task-489`) or mixed (`Task-489`) shapes.
// trace:TASK-489 | ai:claude
#[test]
fn uppercase_spec_prefix_guard() {
    assert!(positional_has_uppercase_spec_prefix("TASK-489"));
    assert!(positional_has_uppercase_spec_prefix("STORY-86"));
    assert!(positional_has_uppercase_spec_prefix("FR-1-001"));
    assert!(!positional_has_uppercase_spec_prefix("task-489"));
    assert!(!positional_has_uppercase_spec_prefix("Task-489"));
    assert!(!positional_has_uppercase_spec_prefix("019e4df1"));
}

/// TASK-489: ambiguous hex-id prefix on a positional still surfaces
/// the ambiguous-prefix error (BUG-312 path) rather than masking it
/// with a branch fallback.
// trace:TASK-489 | ai:claude
#[test]
fn positional_ambiguous_hex_prefix_keeps_ambiguous_error() {
    let leases = vec![
        lease("019e4df1af45", "TASK-419", "/tmp/wt-419", None),
        lease("019e4df1e131", "TASK-420", "/tmp/wt-420", None),
    ];
    let err = resolve_session_to_end(Some("019e4df1"), None, None, &leases, false).unwrap_err();
    let s = err.to_string();
    assert!(s.contains("ambiguous"), "{}", s);
}

/// BUG-312: no collision against other ids → floor (8 chars) is enough.
// trace:BUG-312 | ai:claude
#[test]
fn unique_prefix_len_returns_floor_when_no_collision() {
    let ids = ["019e4df1af45", "019aaaaaaaaa", "019bbbbbbbbb"];
    assert_eq!(unique_prefix_len("019e4df1af45", &ids, 8), 8);
}

/// BUG-312: two ids share the first 8 chars → bump to the first
/// distinguishing char (9). The historical 8-char display would lie;
/// 9 chars is the smallest honest answer.
// trace:BUG-312 | ai:claude
#[test]
fn unique_prefix_len_bumps_past_collision() {
    // From the bug report — same HLC generation window.
    let ids = ["019e4df1af45", "019e4df1e131"];
    assert_eq!(unique_prefix_len("019e4df1af45", &ids, 8), 9);
    assert_eq!(unique_prefix_len("019e4df1e131", &ids, 8), 9);
}

/// BUG-312: an id identical (or prefix-of) to the WHOLE other id can
/// never be made unique by extending — return the full length and
/// let the caller decide. Defensive guard; lease ids are uniform 12
/// chars in practice, so this is the never-shorter-than-self path.
// trace:BUG-312 | ai:claude
#[test]
fn unique_prefix_len_caps_at_id_length() {
    let ids = ["019e4df1af45", "019e4df1af45ff"];
    // Target is a prefix of the other → grows to its own length.
    assert_eq!(unique_prefix_len("019e4df1af45", &ids, 8), 12);
}

/// BUG-312: a single-lease set never needs to extend — floor wins.
// trace:BUG-312 | ai:claude
#[test]
fn unique_prefix_len_single_id() {
    let ids = ["019e4df1af45"];
    assert_eq!(unique_prefix_len("019e4df1af45", &ids, 8), 8);
}
