use super::*;

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

/// A lease whose worktree IS a live claude's cwd classifies Live — that is
/// the row `aida ps` paints as live. We simulate "a live process inside the
/// worktree" with a LiveSession whose cwd is the lease worktree.
#[test]
fn ps_live_lease_classifies_live() {
    let tmp = tempfile::tempdir().unwrap();
    let l = ps_lease("l-live", "STORY-1", tmp.path().to_path_buf());
    let live = vec![process_probe::LiveSession {
        pid: std::process::id(),
        cwd: tmp.path().to_path_buf(),
        jsonl: None,
        stale_cwd: false,
    }];
    assert_eq!(
        lease_state_for(&l, &live, chrono::Utc::now()),
        LeaseState::Live,
        "a lease backed by a live claude in its worktree is Live"
    );
}

/// A lease whose worktree is GONE (the dead-pid / crashed signature)
/// classifies Stale — the row `aida ps` paints STALE and folds behind
/// the footer count unless --all.
#[test]
fn ps_dead_pid_lease_classifies_stale() {
    let l = ps_lease(
        "l-dead",
        "STORY-2",
        std::path::PathBuf::from("/nonexistent/aida-ps-dead"),
    );
    assert_eq!(
        lease_state_for(&l, &[], chrono::Utc::now()),
        LeaseState::Stale,
        "a lease with no live process + missing worktree is STALE"
    );
}

#[test]
fn ps_process_backed_lease_uses_active_pid() {
    let mut l = ps_lease("l-codex", "BUG-741", std::path::PathBuf::from("."));
    l.active_pid = Some(std::process::id());

    let specs = vec![RunningWorkSpec {
        disp: "BUG-741".into(),
        agreed_id: Some("BUG-741".into()),
        spec_id: Some("BUG-741".into()),
        title: "codex liveness".into(),
        in_progress: true,
        orphan_excluded_type: false,
    }];

    let (rows, orphans) = build_running_work(
        &specs,
        &[l],
        &[],
        chrono::Utc::now(),
        |_| dispatch_health_ps::WorktreeGitProbe::default(),
        |_| None,
    );

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].state, LeaseState::Live);
    assert_eq!(rows[0].pid, Some(std::process::id()));
    assert!(orphans.is_empty());
}

/// BUG-752: an Agent-tool (harness) subagent lease with the parent claude
/// harness pid stamped as `active_pid` classifies Live while that pid is
/// alive — the agent-tool analog of the BUG-741 codex child-pid fix. The
/// worktree probe can never see these workers (they run inside the parent
/// claude process, cwd = project root), so the stamped pid is the liveness
/// signal.
// trace:BUG-752 | ai:claude
#[test]
fn ps_harness_lease_with_stamped_harness_pid_is_live() {
    let tmp = tempfile::tempdir().unwrap();
    let wt = tmp.path().join(".claude/worktrees/agent-abc123");
    std::fs::create_dir_all(&wt).unwrap();
    let mut l = ps_lease("l-harness-pid", "TASK-1117", wt);
    l.branch = "task-1117-edit-editor".into();
    l.active_pid = Some(std::process::id());

    let (rows, _) = build_running_work(
        &[],
        &[l],
        &[],
        chrono::Utc::now(),
        |_| dispatch_health_ps::WorktreeGitProbe {
            dirty: true,
            ahead_of_main: 0,
            last_commit_subject: Some("wip".into()),
        },
        |_| None,
    );

    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].state,
        LeaseState::Live,
        "a harness lease with a live stamped pid must read live, not dormant"
    );
    assert_eq!(rows[0].pid, Some(std::process::id()));
    let d = rows[0].dispatch.as_ref().unwrap();
    assert_eq!(
        d.state,
        dispatch_health_ps::DispatchState::Moving,
        "alive + dirty is Moving — no salvage hint for a working agent"
    );
    assert!(d.hint.is_none());
}

/// BUG-752: a harness lease with NO pid signal at all (legacy lease from a
/// pre-fix binary, or no claude ancestor found at register time) has
/// UNDETERMINABLE liveness — it must classify Unknown, never "dead process,
/// salvageable", even with a dirty worktree. The salvage-commit hint is
/// dangerous mid-drain: it would commit half-done work out from under a live
/// agent and double-dispatch.
// trace:BUG-752 | ai:claude
#[test]
fn ps_harness_lease_without_pid_is_unknown_not_salvageable() {
    let tmp = tempfile::tempdir().unwrap();
    let wt = tmp.path().join(".claude/worktrees/agent-def456");
    std::fs::create_dir_all(&wt).unwrap();
    let mut l = ps_lease("l-harness-nopid", "harness-worktree", wt);
    l.branch = "worktree-agent-def456".into();

    let (rows, _) = build_running_work(
        &[],
        &[l],
        &[],
        chrono::Utc::now(),
        |_| dispatch_health_ps::WorktreeGitProbe {
            dirty: true,
            ahead_of_main: 0,
            last_commit_subject: Some("wip: half-done".into()),
        },
        |_| None,
    );

    assert_eq!(rows.len(), 1);
    let d = rows[0].dispatch.as_ref().unwrap();
    assert_eq!(
        d.state,
        dispatch_health_ps::DispatchState::Unknown,
        "no pid evidence on a harness lease is unknown, not dead"
    );
    let hint = d
        .hint
        .as_deref()
        .expect("Unknown carries an explanatory hint");
    assert!(hint.contains("liveness unknown"), "{hint}");
    assert!(
        !hint.contains("add -A") && !hint.contains("salvage-commit"),
        "the salvage-commit command must never appear for unknown liveness: {hint}"
    );
}

/// BUG-752 regression guard: a NORMAL (non-harness) session lease with a dead
/// process and a dirty worktree still classifies Salvageable — the unknown
/// carve-out is scoped to harness worktrees only, where the cwd probe is
/// structurally blind.
// trace:BUG-752 | ai:claude
#[test]
fn ps_non_harness_dead_dirty_lease_still_salvageable() {
    let tmp = tempfile::tempdir().unwrap();
    let mut l = ps_lease("l-dead-dirty", "STORY-9", tmp.path().to_path_buf());
    l.branch = "story-9-widget".into();

    let (rows, _) = build_running_work(
        &[],
        &[l],
        &[],
        chrono::Utc::now(),
        |_| dispatch_health_ps::WorktreeGitProbe {
            dirty: true,
            ahead_of_main: 0,
            last_commit_subject: None,
        },
        |_| None,
    );

    assert_eq!(rows.len(), 1);
    let d = rows[0].dispatch.as_ref().unwrap();
    assert_eq!(
        d.state,
        dispatch_health_ps::DispatchState::Salvageable,
        "genuinely determined dead + dirty keeps the salvage urgency"
    );
}

/// BUG-752: the pure tri-state pid-liveness matrix and the
/// no-pid-signal-harness predicate.
// trace:BUG-752 | ai:claude
#[test]
fn ps_pid_liveness_tri_state_matrix() {
    // Live lease → demonstrably alive, regardless of the harness flag.
    assert_eq!(ps_pid_liveness(LeaseState::Live, false), Some(true));
    assert_eq!(ps_pid_liveness(LeaseState::Live, true), Some(true));
    // Non-live harness lease with no pid signal → undeterminable.
    assert_eq!(ps_pid_liveness(LeaseState::Dormant, true), None);
    assert_eq!(ps_pid_liveness(LeaseState::Stale, true), None);
    // Non-live ordinary lease → the probe looked and found nothing: dead.
    assert_eq!(ps_pid_liveness(LeaseState::Dormant, false), Some(false));
    assert_eq!(ps_pid_liveness(LeaseState::Stale, false), Some(false));
}

// trace:BUG-752 | ai:claude
#[test]
fn harness_lease_without_pid_signal_predicate() {
    let harness_wt = std::path::PathBuf::from("/x/.claude/worktrees/agent-abc");
    let mut l = ps_lease("l-h", "harness-worktree", harness_wt);
    l.branch = "worktree-agent-abc".into();
    assert!(harness_lease_without_pid_signal(&l));

    // Any recorded pid signal disqualifies — liveness IS determinable.
    let mut with_active = l.clone();
    with_active.active_pid = Some(1234);
    assert!(!harness_lease_without_pid_signal(&with_active));
    let mut with_creator = l.clone();
    with_creator.creator_pid = Some(1234);
    assert!(!harness_lease_without_pid_signal(&with_creator));

    // A non-harness worktree/branch is never in the carve-out.
    let mut ordinary = ps_lease(
        "l-o",
        "STORY-9",
        std::path::PathBuf::from("/x/worktrees/story-9"),
    );
    ordinary.branch = "story-9-widget".into();
    assert!(!harness_lease_without_pid_signal(&ordinary));
}

/// An In-Progress spec with NO spec-scoped lease is orphaned (flag-only):
/// `ps_orphan_verdict(None) == Some(false)`. A live lease means genuinely
/// running (not orphaned → None). A dead/dormant lease is orphaned with a
/// crashed-session marker (`Some(true)`).
#[test]
fn ps_orphan_verdict_matrix() {
    // No lease backing an In-Progress flag → orphaned, flag-only.
    assert_eq!(ps_orphan_verdict(None), Some(false));
    // Live lease → genuinely running, not orphaned.
    assert_eq!(ps_orphan_verdict(Some(LeaseState::Live)), None);
    // Dead lease → orphaned, crashed session.
    assert_eq!(ps_orphan_verdict(Some(LeaseState::Stale)), Some(true));
    // Dormant (no live process) → also orphaned: the flag is not
    // liveness-backed.
    assert_eq!(ps_orphan_verdict(Some(LeaseState::Dormant)), Some(true));
}

// Rollup / stateless types are excluded from the orphan pass — an
// In-Progress epic (BUG-626 child-rollup) never holds a session, so it
// must NOT read as orphaned; real work-item types still flow through.
// trace:TASK-940 | ai:claude
#[test]
fn ps_orphan_excludes_rollup_types() {
    use aida_core::RequirementType;
    // Rollup / stateless → excluded (would otherwise be all-noise).
    assert!(ps_orphan_excluded_type(&RequirementType::Epic));
    assert!(ps_orphan_excluded_type(&RequirementType::Folder));
    assert!(ps_orphan_excluded_type(&RequirementType::Meta));
    // Real work items → still flagged when orphaned.
    assert!(!ps_orphan_excluded_type(&RequirementType::Task));
    assert!(!ps_orphan_excluded_type(&RequirementType::Story));
    assert!(!ps_orphan_excluded_type(&RequirementType::Bug));
    assert!(!ps_orphan_excluded_type(&RequirementType::Functional));
    assert!(!ps_orphan_excluded_type(&RequirementType::Spike));
}

/// TASK-1072: the cache-fast path decodes the excluded-type set from the
/// cache's Debug-form string ("Epic", …) rather than a parsed enum. This
/// asserts the string form and the enum form agree for EVERY
/// `RequirementType` variant, so switching `aida ps` to the cache summaries
/// can't drift the orphan pass's type exclusion.
// trace:TASK-1072 | ai:claude
#[test]
fn ps_orphan_excluded_type_str_matches_enum_for_all_types() {
    use aida_core::RequirementType::*;
    for t in [
        Functional,
        NonFunctional,
        System,
        User,
        ChangeRequest,
        Bug,
        Epic,
        Story,
        Task,
        Spike,
        Sprint,
        Folder,
        Meta,
        Principle,
        Vision,
        Constraint,
        Decision,
        Term,
        Doc,
    ] {
        // The cache stores `format!("{:?}", req_type)` (STORY-707 projection),
        // so that's the exact string the cache-fast path feeds the _str form.
        let cache_form = format!("{t:?}");
        assert_eq!(
            ps_orphan_excluded_type_str(&cache_form),
            ps_orphan_excluded_type(&t),
            "string / enum orphan-exclusion disagree for {t:?}"
        );
    }
}

fn ps_summary(
    spec_id: Option<&str>,
    agreed_id: Option<&str>,
    title: &str,
    status: &str,
    req_type: &str,
) -> aida_core::RequirementSummary {
    aida_core::RequirementSummary {
        id: uuid::Uuid::new_v4(),
        spec_id: spec_id.map(str::to_string),
        agreed_id: agreed_id.map(str::to_string),
        title: title.to_string(),
        description: String::new(),
        status: status.to_string(),
        priority: "Medium".into(),
        owner: String::new(),
        assignee: None,
        feature: "Uncategorized".into(),
        req_type: req_type.to_string(),
        tags: Vec::new(),
        created_at: String::new(),
        modified_at: String::new(),
        archived: false,
        archived_at: None,
        deferred: false,
        deferred_at: None,
        deferred_until: None,
        in_degree: 0,
        out_degree: 0,
        heft: 0,
        blocked: false,
        // trace:TASK-1065 | ai:claude
        has_pending_decision: false,
        execution_mode: None,
        weight: None,
        yaml_path: String::new(),
    }
}

/// TASK-1072: the pure summary→projection mapping. Disp id follows the
/// agreed_id → spec_id → uuid precedence, In-Progress candidacy is decoded
/// from the cache status string, and the excluded-type flag from the type
/// string.
// trace:TASK-1072 | ai:claude
#[test]
fn running_work_spec_from_summary_maps_fields() {
    // agreed_id wins the disp slot; InProgress → orphan candidate; Task is
    // not an excluded type.
    let a = running_work_spec_from_summary(ps_summary(
        Some("TASK-7-001"),
        Some("TASK-7"),
        "do the thing",
        "InProgress",
        "Task",
    ));
    assert_eq!(a.disp, "TASK-7");
    assert_eq!(a.agreed_id.as_deref(), Some("TASK-7"));
    assert_eq!(a.spec_id.as_deref(), Some("TASK-7-001"));
    assert_eq!(a.title, "do the thing");
    assert!(a.in_progress);
    assert!(!a.orphan_excluded_type);

    // No agreed_id → spec_id fills disp; Draft is not a candidate.
    let b = running_work_spec_from_summary(ps_summary(
        Some("STORY-9-002"),
        None,
        "later",
        "Draft",
        "Story",
    ));
    assert_eq!(b.disp, "STORY-9-002");
    assert!(!b.in_progress);
    assert!(!b.orphan_excluded_type);

    // An In-Progress epic is an excluded (rollup) type — must NOT surface as
    // an orphan even though its status flag is InProgress.
    let c = running_work_spec_from_summary(ps_summary(
        Some("EPIC-4-001"),
        Some("EPIC-4"),
        "big rollup",
        "InProgress",
        "Epic",
    ));
    assert!(c.in_progress);
    assert!(c.orphan_excluded_type);
}

/// TASK-1072: end-to-end fixture over the pure [`build_running_work`] core,
/// mocking the /proc live-slice + lease state. Asserts the row/orphan output
/// is exactly what the prior full-store path produced: a live lease resolves
/// its scope to a spec display id and is NOT orphaned; an In-Progress spec
/// with no lease surfaces as flag-only; an excluded (epic) type never
/// surfaces. The `live` slice is passed ONCE (the single-probe discipline).
// trace:TASK-1072 | ai:claude
#[test]
fn build_running_work_resolves_specs_and_orphans_on_fixture() {
    let now = chrono::Utc::now();
    let live_dir = tempfile::tempdir().unwrap();

    // Spec index (what the cache summaries would project).
    let specs = vec![
        RunningWorkSpec {
            disp: "TASK-1".into(),
            agreed_id: Some("TASK-1".into()),
            spec_id: Some("TASK-1-001".into()),
            title: "live one".into(),
            in_progress: true,
            orphan_excluded_type: false,
        },
        RunningWorkSpec {
            disp: "STORY-2".into(),
            agreed_id: Some("STORY-2".into()),
            spec_id: Some("STORY-2-001".into()),
            title: "flag-only".into(),
            in_progress: true,
            orphan_excluded_type: false,
        },
        RunningWorkSpec {
            disp: "EPIC-3".into(),
            agreed_id: Some("EPIC-3".into()),
            spec_id: Some("EPIC-3-001".into()),
            title: "rollup".into(),
            in_progress: true,
            orphan_excluded_type: true,
        },
    ];

    // ONE lease, scoped to TASK-1, backed by a live claude in its worktree.
    let leases = vec![ps_lease(
        "sess-live",
        "TASK-1",
        live_dir.path().to_path_buf(),
    )];
    // ONE /proc snapshot, reused for every liveness check (single probe).
    let live = vec![process_probe::LiveSession {
        pid: 4242,
        cwd: live_dir.path().to_path_buf(),
        jsonl: None,
        stale_cwd: false,
    }];

    // TASK-1090: no-op probe stub — this fixture asserts scope/orphan
    // resolution, not dispatch-health, and must stay filesystem-free.
    // TASK-1143: no-op lock stub — no worktree is locked here.
    let (rows, orphans) = build_running_work(
        &specs,
        &leases,
        &live,
        now,
        |_| dispatch_health_ps::WorktreeGitProbe::default(),
        |_| None,
    );

    // Row: TASK-1's scope resolved to its display id; live pid attached.
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].spec.as_deref(), Some("TASK-1"));
    assert_eq!(rows[0].state, LeaseState::Live);
    assert_eq!(rows[0].pid, Some(4242));
    // TASK-1143: no lock probe returned a value → the row is unlocked.
    assert_eq!(rows[0].locked_by, None);

    // Orphans: STORY-2 is In-Progress with no lease → flag-only orphan.
    // TASK-1 is live (not orphaned). EPIC-3 is an excluded rollup type.
    assert_eq!(orphans.len(), 1);
    assert_eq!(orphans[0].spec, "STORY-2");
    assert!(!orphans[0].stale_lease, "no lease → flag-only, not crashed");
    assert!(
        !orphans[0].likely_fanout,
        "no live harness lease → not a fan-out"
    );
}

/// TASK-1143: the pure locked-by cell helper — a present lock owner renders
/// as its own value; an absent (or defensively-empty) lock renders blank so
/// the column stays quiet on the unlocked common case.
// trace:TASK-1143 | ai:claude
#[test]
fn ps_locked_by_cell_shows_owner_or_blank() {
    assert_eq!(ps_locked_by_cell(Some("advisor-a")), "advisor-a");
    assert_eq!(ps_locked_by_cell(None), "");
    // Defensive: an empty `authorized_by` is treated as unlocked, matching
    // `verify_worktree_lock`.
    assert_eq!(ps_locked_by_cell(Some("")), "");
}

/// TASK-1143: end-to-end over the pure `build_running_work` core — a locked
/// worktree's row carries the lock owner; an unlocked worktree's row is
/// blank. The lock seam is injected (a stub keyed on the worktree path), so
/// this proves the per-worktree resolution without a real lease dir.
// trace:TASK-1143 | ai:claude
#[test]
fn build_running_work_surfaces_the_worktree_lock_owner() {
    let now = chrono::Utc::now();
    let locked_dir = tempfile::tempdir().unwrap();
    let unlocked_dir = tempfile::tempdir().unwrap();

    let specs = vec![
        RunningWorkSpec {
            disp: "TASK-1".into(),
            agreed_id: Some("TASK-1".into()),
            spec_id: Some("TASK-1-001".into()),
            title: "locked".into(),
            in_progress: true,
            orphan_excluded_type: false,
        },
        RunningWorkSpec {
            disp: "TASK-2".into(),
            agreed_id: Some("TASK-2".into()),
            spec_id: Some("TASK-2-001".into()),
            title: "unlocked".into(),
            in_progress: true,
            orphan_excluded_type: false,
        },
    ];

    // Two live leases: TASK-1 in the locked worktree, TASK-2 in the
    // unlocked one. Both backed by a live claude so neither is stale.
    let leases = vec![
        ps_lease("sess-locked", "TASK-1", locked_dir.path().to_path_buf()),
        ps_lease("sess-unlocked", "TASK-2", unlocked_dir.path().to_path_buf()),
    ];
    let live = vec![
        process_probe::LiveSession {
            pid: 4242,
            cwd: locked_dir.path().to_path_buf(),
            jsonl: None,
            stale_cwd: false,
        },
        process_probe::LiveSession {
            pid: 4343,
            cwd: unlocked_dir.path().to_path_buf(),
            jsonl: None,
            stale_cwd: false,
        },
    ];

    // Lock seam: only the locked worktree resolves to an advisor.
    let locked_path = locked_dir.path().to_path_buf();
    let (rows, _orphans) = build_running_work(
        &specs,
        &leases,
        &live,
        now,
        |_| dispatch_health_ps::WorktreeGitProbe::default(),
        |wt| {
            if wt == locked_path {
                Some("advisor-a".to_string())
            } else {
                None
            }
        },
    );

    let locked_row = rows
        .iter()
        .find(|r| r.spec.as_deref() == Some("TASK-1"))
        .expect("locked row present");
    let unlocked_row = rows
        .iter()
        .find(|r| r.spec.as_deref() == Some("TASK-2"))
        .expect("unlocked row present");

    assert_eq!(locked_row.locked_by.as_deref(), Some("advisor-a"));
    assert_eq!(
        ps_locked_by_cell(locked_row.locked_by.as_deref()),
        "advisor-a"
    );
    assert_eq!(unlocked_row.locked_by, None);
    assert_eq!(ps_locked_by_cell(unlocked_row.locked_by.as_deref()), "");
}

/// The `--json` row shape: every session row carries the documented keys,
/// and an orphaned spec is emitted under `orphaned` with a `flag-only`
/// liveness. Builds the same JSON the handler emits from in-memory rows.
#[test]
fn ps_json_shape() {
    let tmp = tempfile::tempdir().unwrap();
    let l = ps_lease("session-abc", "STORY-3", tmp.path().to_path_buf());
    let row = serde_json::json!({
        "session_id": l.id,
        "scope": l.scope,
        "spec": Some("STORY-3"),
        "role": l.role,
        "worktree": l.worktree_path.display().to_string(),
        "branch": l.branch,
        "pid": Some(4242u32),
        "started_at": l.started_at.to_rfc3339(),
        "elapsed_secs": 90u64,
        "liveness": LeaseState::Live.label(),
        "live": true,
        // TASK-1143: the worktree lock owner — null when unlocked.
        "locked_by": Option::<String>::None,
    });
    for key in [
        "session_id",
        "scope",
        "spec",
        "role",
        "worktree",
        "branch",
        "pid",
        "started_at",
        "elapsed_secs",
        "liveness",
        "live",
        "locked_by",
    ] {
        assert!(row.get(key).is_some(), "session row missing key `{key}`");
    }
    assert_eq!(row["liveness"], "live");

    let orphan = serde_json::json!({
        "spec": "STORY-9",
        "title": "an in-progress spec nobody is working",
        "liveness": "flag-only",
        "live": false,
    });
    assert_eq!(orphan["liveness"], "flag-only");
    assert_eq!(orphan["live"], false);

    let envelope = serde_json::json!({ "sessions": [row], "orphaned": [orphan] });
    assert!(envelope["sessions"].is_array());
    assert!(envelope["orphaned"].is_array());
}

/// BUG-669: persist a session lease to `.aida/sessions/<id>.toml`.
fn persist_lease(root: &std::path::Path, lease: &SessionLease) {
    std::fs::create_dir_all(leases_dir(root)).unwrap();
    std::fs::write(
        lease_path(root, &lease.id),
        toml::to_string_pretty(lease).unwrap(),
    )
    .unwrap();
}

/// BUG-669: a recycled pool worktree must NOT inherit the prior occupant's
/// lease. `clear_worktree_session_leases` drops the stale lease pointing at
/// the reused tree so the new session's fresh lease (the NEW spec) is the
/// only one `aida ps` resolves for that path.
#[test]
fn pool_reuse_clears_prior_occupant_lease() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // The recycled worktree dir must exist so canonicalize() agrees on both
    // sides of the path comparison.
    let wt = root.join("pool-wt-0");
    std::fs::create_dir_all(&wt).unwrap();
    let wt_canon = wt.canonicalize().unwrap();

    // Prior occupant: a TASK-0439 lease pointing at the pooled tree.
    persist_lease(root, &ps_lease("sess-old", "TASK-0439", wt_canon.clone()));

    // Acquire-time reset removes exactly the one stale lease.
    assert_eq!(
        clear_worktree_session_leases(root, &wt),
        1,
        "the prior occupant's lease must be cleared on reuse"
    );
    assert!(
        list_leases(root).is_empty(),
        "no lease should survive the reset"
    );

    // The new session stamps its OWN lease for the same tree (ADR-7).
    persist_lease(root, &ps_lease("sess-new", "ADR-7", wt_canon));
    let scopes: Vec<String> = list_leases(root).into_iter().map(|l| l.scope).collect();
    assert_eq!(
        scopes,
        vec!["ADR-7".to_string()],
        "the reused tree must report the NEW spec, not the inherited one: {scopes:?}"
    );
}

/// BUG-669: the reset only touches leases pointing at the acquired tree — a
/// concurrent sibling session in a DIFFERENT worktree is untouched.
#[test]
fn pool_reuse_leaves_other_worktree_leases_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mine = root.join("pool-wt-0");
    let other = root.join("pool-wt-1");
    std::fs::create_dir_all(&mine).unwrap();
    std::fs::create_dir_all(&other).unwrap();

    persist_lease(
        root,
        &ps_lease("sess-old", "TASK-0439", mine.canonicalize().unwrap()),
    );
    persist_lease(
        root,
        &ps_lease("sess-sib", "STORY-1", other.canonicalize().unwrap()),
    );

    assert_eq!(clear_worktree_session_leases(root, &mine), 1);
    let scopes: Vec<String> = list_leases(root).into_iter().map(|l| l.scope).collect();
    assert_eq!(
        scopes,
        vec!["STORY-1".to_string()],
        "only the acquired tree's lease is reset; the sibling survives"
    );
}

/// TASK-1064: the three-way orphan framing. A flag-only spec (no spec-scoped
/// lease) while a fan-out is live → "likely worked by a fan-out". A crashed
/// (stale) spec-scoped lease is a genuine orphan regardless of any fan-out.
/// No fan-out → still a genuine orphan.
#[test]
fn ps_orphan_likely_fanout_matrix() {
    // flag-only (stale_lease=false) + fan-out live → reframe as fan-out work.
    assert!(ps_orphan_likely_fanout(false, true));
    // flag-only + NO fan-out → genuine orphan.
    assert!(!ps_orphan_likely_fanout(false, false));
    // crashed spec-scoped lease (stale) → genuine orphan even with a fan-out.
    assert!(!ps_orphan_likely_fanout(true, true));
    assert!(!ps_orphan_likely_fanout(true, false));
}

/// TASK-1064: a LIVE generic `harness-worktree` lease (an advisor Agent-tool
/// fan-out) is detected as an active fan-out; a spec-scoped lease (even live)
/// is NOT — only the generic non-spec-linked harness scope counts.
#[test]
fn live_fanout_harness_lease_detects_generic_harness_only() {
    let tmp = tempfile::tempdir().unwrap();
    let now = chrono::Utc::now();

    // A live generic harness lease → fan-out active.
    let harness = ps_lease(
        "sess-fanout",
        worktree_lease::HARNESS_WORKTREE_SCOPE,
        tmp.path().to_path_buf(),
    );
    let live = vec![process_probe::LiveSession {
        pid: std::process::id(),
        cwd: tmp.path().to_path_buf(),
        jsonl: None,
        stale_cwd: false,
    }];
    assert!(
        live_fanout_harness_lease(&[harness.clone()], &live, now),
        "a live harness-worktree lease must read as an active fan-out"
    );

    // A live SPEC-scoped lease is not a fan-out (it backs its own spec).
    let spec_lease = ps_lease("sess-spec", "STORY-7", tmp.path().to_path_buf());
    assert!(
        !live_fanout_harness_lease(&[spec_lease], &live, now),
        "a spec-scoped lease is not a generic fan-out lease"
    );

    // A DEAD harness lease (worktree gone, no live process) is not active.
    let dead_harness = ps_lease(
        "sess-dead",
        worktree_lease::HARNESS_WORKTREE_SCOPE,
        std::path::PathBuf::from("/nonexistent/aida-fanout-dead"),
    );
    assert!(
        !live_fanout_harness_lease(&[dead_harness], &[], now),
        "a dead harness lease is not an active fan-out"
    );
}
