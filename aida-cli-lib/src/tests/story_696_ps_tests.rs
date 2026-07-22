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
        manual_enter_at: None,
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
    assert_eq!(ps_orphan_verdict(None, false), Some(false));
    // Live lease → genuinely running, not orphaned.
    assert_eq!(ps_orphan_verdict(Some(LeaseState::Live), false), None);
    // Dead lease → orphaned, crashed session.
    assert_eq!(
        ps_orphan_verdict(Some(LeaseState::Stale), false),
        Some(true)
    );
    // Dormant (no live process) → also orphaned: the flag is not
    // liveness-backed.
    assert_eq!(
        ps_orphan_verdict(Some(LeaseState::Dormant), false),
        Some(true)
    );
    // BUG-778: a hand-entered worktree awaiting its agent launch is NOT an
    // orphan at any lease state — the operator is the missing worker.
    // trace:BUG-778 | ai:claude
    assert_eq!(ps_orphan_verdict(Some(LeaseState::Dormant), true), None);
    assert_eq!(ps_orphan_verdict(Some(LeaseState::Stale), true), None);
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
        origin: None,
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
        |_| None,
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
        // BUG-763: the backing pid's own start time — null when unresolvable.
        "pid_started_at": Option::<String>::None,
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
        "pid_started_at",
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

/// BUG-763: the started cell — time-of-day for a lease started today,
/// date-qualified for anything older, so a June birth can't read as this
/// morning next to a 500h elapsed. Pure over already-localized values (fixed
/// dates are fine: no clock or TTL is involved).
// trace:BUG-763 | ai:claude
#[test]
fn ps_started_cell_dates_non_today_rows() {
    let today = chrono::NaiveDate::from_ymd_opt(2026, 7, 19).unwrap();

    // Started today → time-of-day only, as before.
    let this_morning = today.and_hms_opt(11, 55, 0).unwrap();
    assert_eq!(ps_started_cell(this_morning, today), "11:55");

    // A lease older than 24h is never "today" — its date must show.
    let june = chrono::NaiveDate::from_ymd_opt(2026, 6, 26)
        .unwrap()
        .and_hms_opt(11, 55, 0)
        .unwrap();
    assert_eq!(ps_started_cell(june, today), "Jun-26 11:55");

    // Even yesterday (possibly <24h ago) is date-qualified — a bare
    // "23:50" next to today's rows would read as the future.
    let yesterday = chrono::NaiveDate::from_ymd_opt(2026, 7, 18)
        .unwrap()
        .and_hms_opt(23, 50, 0)
        .unwrap();
    assert_eq!(ps_started_cell(yesterday, today), "Jul-18 23:50");

    // Year boundary: last year's date still renders month-day (the elapsed
    // column carries the magnitude).
    let last_year = chrono::NaiveDate::from_ymd_opt(2025, 12, 31)
        .unwrap()
        .and_hms_opt(9, 0, 0)
        .unwrap();
    assert_eq!(ps_started_cell(last_year, today), "Dec-31 09:00");
}

/// BUG-763: the adopted marker — `Some` (both ages named) when the live pid
/// postdates the lease by more than the slack; `None` for the ordinary
/// same-session case, a missing pid start time, or start-up jitter within
/// slack. Times computed relative to now, never hardcoded.
// trace:BUG-763 | ai:claude
#[test]
fn ps_adopted_note_names_both_ages() {
    let now = chrono::Utc::now();
    let lease_born = now - chrono::Duration::hours(559);
    let pid_up = now - chrono::Duration::hours(2);

    let note = ps_adopted_note(lease_born, Some(pid_up), now)
        .expect("pid postdating the lease by weeks must be marked adopted");
    assert!(note.contains("adopted"), "{note}");
    assert!(note.contains("559h"), "lease age must be named: {note}");
    assert!(note.contains("2h"), "pid age must be named: {note}");

    // Same-session start-up ordering (pid moments after — or before — the
    // lease) is NOT adoption.
    let jitter_after = lease_born + chrono::Duration::seconds(30);
    assert_eq!(ps_adopted_note(lease_born, Some(jitter_after), now), None);
    let before = lease_born - chrono::Duration::seconds(30);
    assert_eq!(ps_adopted_note(lease_born, Some(before), now), None);

    // No pid start time resolvable → no marker (never guess).
    assert_eq!(ps_adopted_note(lease_born, None, now), None);
}

/// BUG-763: the pid-start seam end-to-end over the pure `build_running_work`
/// core — a row whose live pid resolves a start time carries it, so the
/// renderer can compare lease-age vs process-age; a row with no live pid
/// carries none.
// trace:BUG-763 | ai:claude
#[test]
fn build_running_work_carries_pid_start_time() {
    let now = chrono::Utc::now();
    let pid_started = now - chrono::Duration::hours(2);

    // A process-backed lease born long before its current pid (the adopted
    // persistent-lease shape).
    let mut adopted = ps_lease("l-adopted", "BUG-763", std::path::PathBuf::from("."));
    adopted.started_at = now - chrono::Duration::hours(559);
    adopted.active_pid = Some(std::process::id());

    let (rows, _) = build_running_work(
        &[],
        &[adopted],
        &[],
        now,
        |_| dispatch_health_ps::WorktreeGitProbe::default(),
        |_| None,
        |_| Some(pid_started),
    );

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].pid_started_at, Some(pid_started));
    assert!(
        ps_adopted_note(rows[0].lease.started_at, rows[0].pid_started_at, now).is_some(),
        "a 559h-old lease with a 2h-old pid must carry the adopted note"
    );

    // No live pid → the probe is never consulted, no start time attached.
    let dead = ps_lease(
        "l-dead",
        "STORY-1",
        std::path::PathBuf::from("/nonexistent/aida-ps-pid-start"),
    );
    let (rows, _) = build_running_work(
        &[],
        &[dead],
        &[],
        now,
        |_| dispatch_health_ps::WorktreeGitProbe::default(),
        |_| None,
        |_| Some(pid_started),
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].pid, None);
    assert_eq!(rows[0].pid_started_at, None);
}

/// BUG-769: the elapsed column value. An ADOPTED lease (live pid younger than
/// the lease record) reports the PROCESS's uptime — the 583h-lease/3m-process
/// row must read 3m, not 583h, so the headline number stops contradicting the
/// BUG-763 annotation under it. Every non-adopted shape (pid born with the
/// lease, start-up jitter either way, no resolvable pid start) still reports
/// lease age. Times are relative to now, never hardcoded.
// trace:BUG-769 | ai:claude
#[test]
fn ps_elapsed_secs_reports_process_uptime_for_adopted_lease() {
    let now = chrono::Utc::now();
    let lease_born = now - chrono::Duration::hours(583);
    let lease_age = 583 * 3600;

    // Adopted: the pid is 3m old, the lease 583h → the column shows 3m.
    let pid_up = now - chrono::Duration::minutes(3);
    let elapsed = ps_elapsed_secs(lease_born, Some(pid_up), now);
    assert_eq!(
        elapsed, 180,
        "an adopted row's elapsed must be the process uptime, not the {lease_age}s lease age"
    );
    assert_eq!(humanize_duration_secs(elapsed), "3m 0s");

    // ...and the lease age is still on screen, in the annotation.
    let note = ps_adopted_note(lease_born, Some(pid_up), now).expect("adopted note present");
    assert!(note.contains("583h"), "lease age must survive in {note}");

    // Non-adopted, same session: pid born with the lease → lease age, unchanged.
    let same_session = lease_born + chrono::Duration::seconds(2);
    assert_eq!(
        ps_elapsed_secs(lease_born, Some(same_session), now),
        lease_age,
        "a pid born with its lease must not change the elapsed column"
    );

    // Start-up jitter inside the slack (either direction) is not adoption.
    let jitter = lease_born + chrono::Duration::seconds(PS_ADOPTED_SLACK_SECS - 1);
    assert_eq!(ps_elapsed_secs(lease_born, Some(jitter), now), lease_age);
    let before = lease_born - chrono::Duration::minutes(10);
    assert_eq!(ps_elapsed_secs(lease_born, Some(before), now), lease_age);

    // No resolvable pid start time → lease age (never guess).
    assert_eq!(ps_elapsed_secs(lease_born, None, now), lease_age);
}

/// BUG-769 end-to-end over the pure `build_running_work` core: the adopted row
/// carries process uptime in `elapsed_secs` (the value both the table column
/// and `--json` print), while a same-session row is untouched.
// trace:BUG-769 | ai:claude
#[test]
fn build_running_work_elapsed_is_process_uptime_when_adopted() {
    let now = chrono::Utc::now();
    let lease_born = now - chrono::Duration::hours(583);

    // Adopted: a 583h-old persistent lease re-stamped with a 3m-old pid.
    let mut adopted = ps_lease("l-adopted", "BUG-769", std::path::PathBuf::from("."));
    adopted.started_at = lease_born;
    adopted.active_pid = Some(std::process::id());

    let (rows, _) = build_running_work(
        &[],
        &[adopted.clone()],
        &[],
        now,
        |_| dispatch_health_ps::WorktreeGitProbe::default(),
        |_| None,
        |_| Some(now - chrono::Duration::minutes(3)),
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].elapsed_secs, 180,
        "adopted row must report the pid's 3m uptime, not the 583h lease age"
    );

    // Non-adopted: the SAME lease whose pid started with it → lease age.
    let (rows, _) = build_running_work(
        &[],
        &[adopted],
        &[],
        now,
        |_| dispatch_health_ps::WorktreeGitProbe::default(),
        |_| None,
        |_| Some(lease_born),
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].elapsed_secs,
        583 * 3600,
        "a non-adopted row's elapsed column is unchanged (lease age)"
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

/// TASK-1168: the `aida ps` spec + role columns auto-size to their widest cell
/// instead of pre-truncating to a fixed ~13 visible chars. The regression this
/// pins: `harness-worktree` (16) and `general-purpose` (15) are SHORT, bounded
/// identifiers — they must render whole, not as `harness-workt…` /
/// `general-p…`.
// trace:TASK-1168 | ai:claude
#[test]
fn ps_columns_auto_size_short_identifiers_untruncated() {
    // The old fixed role width (10) ellipsized every one of these.
    let roles = vec![
        "implementer".to_string(),
        "general-purpose".to_string(),
        "harness-worktree".to_string(),
    ];
    let role_w = ps_column_width(&roles, PS_ROLE_MIN_WIDTH, PS_ROLE_MAX_WIDTH);
    assert_eq!(role_w, "harness-worktree".len());
    for role in &roles {
        let (cell, rest) = ps_wrap_cell(role, role_w);
        assert_eq!(&cell, role, "role must render whole, not ellipsized");
        assert!(rest.is_none(), "a closed-set role never needs a wrap line");
        assert!(!cell.contains('…'), "no ellipsis on a bounded identifier");
    }

    // Spec cells: ordinary SPEC-IDs keep the historical floor width, and a
    // `harness-worktree` scope in the spec column renders whole too.
    let specs = vec!["TASK-1168".to_string(), "harness-worktree".to_string()];
    let spec_w = ps_column_width(&specs, PS_SPEC_MIN_WIDTH, PS_SPEC_MAX_WIDTH);
    assert_eq!(spec_w, "harness-worktree".len());
    let (cell, rest) = ps_wrap_cell("harness-worktree", spec_w);
    assert_eq!(cell, "harness-worktree");
    assert!(rest.is_none());

    // A table of only short ids never narrows below the historical width, so
    // the common case looks exactly as before.
    let short = vec!["BUG-7".to_string()];
    assert_eq!(
        ps_column_width(&short, PS_SPEC_MIN_WIDTH, PS_SPEC_MAX_WIDTH),
        PS_SPEC_MIN_WIDTH
    );
    // …and an empty table falls back to the floor rather than collapsing.
    let none: Vec<String> = Vec::new();
    assert_eq!(
        ps_column_width(&none, PS_ROLE_MIN_WIDTH, PS_ROLE_MAX_WIDTH),
        PS_ROLE_MIN_WIDTH
    );
}

/// TASK-1168: a pathologically long scope is bounded by the column ceiling and
/// WRAPS onto a continuation line — every character survives, and the break
/// prefers a separator so a hyphenated identifier doesn't split mid-word.
// trace:TASK-1168 | ai:claude
#[test]
fn ps_over_wide_cell_wraps_instead_of_ellipsizing() {
    let long = "harness-worktree-fanout-subagent-0042";
    let cells = vec![long.to_string()];
    // The ceiling bounds the column so one row can't blow out the table.
    let w = ps_column_width(&cells, PS_SPEC_MIN_WIDTH, PS_SPEC_MAX_WIDTH);
    assert_eq!(w, PS_SPEC_MAX_WIDTH);

    let (head, rest) = ps_wrap_cell(long, w);
    let rest = rest.expect("an over-wide cell wraps onto a continuation line");
    assert!(!head.contains('…'), "wrap, not ellipsis");
    assert_eq!(format!("{head}{rest}"), long, "no characters are lost");
    assert!(head.chars().count() <= w, "the head fits the column");
    // Broke after a separator, not mid-word.
    assert!(
        head.ends_with('-'),
        "expected a segment-boundary break: {head}"
    );

    // No separator inside the column → hard split, still lossless.
    let unbroken = "aaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let (head, rest) = ps_wrap_cell(unbroken, 10);
    let rest = rest.expect("still wraps");
    assert_eq!(head.chars().count(), 10);
    assert_eq!(format!("{head}{rest}"), unbroken);

    // Multi-byte content wraps on CHAR boundaries (never panics mid-codepoint).
    let wide = "スペック-アイディー-とても長い";
    let (head, rest) = ps_wrap_cell(wide, 6);
    let rest = rest.expect("wraps");
    assert_eq!(format!("{head}{rest}"), wide);
}

// ── BUG-778: a hand-entered worktree is not a crashed agent ──────────────

/// The reported false positive, end to end. `aida worktree enter <SPEC>` takes
/// the implementer lease and launches NO agent; for the seconds before the
/// operator starts one, `aida ps` painted the row "stalled … process dead" AND
/// listed the spec under "Orphaned In-Progress specs" with the hint
/// `aida queue work <SPEC>` — which would have dispatched a second session onto
/// a spec being worked by hand. It must read as awaiting-agent, carry no
/// re-dispatch hint, and not appear as an orphan.
// trace:BUG-778 | ai:claude
#[test]
fn ps_freshly_hand_entered_spec_reads_awaiting_agent_not_orphaned() {
    let tmp = tempfile::tempdir().unwrap();
    let wt = tmp.path().join("task-1169");
    std::fs::create_dir_all(&wt).unwrap();
    let now = chrono::Utc::now();
    let mut l = ps_lease("l-entered", "TASK-1169", wt);
    l.branch = "task-1169-launcher".into();
    // 30 seconds ago — exactly the reported window.
    l.manual_enter_at = Some(now - chrono::Duration::seconds(30));

    let specs = vec![RunningWorkSpec {
        disp: "TASK-1169".into(),
        agreed_id: Some("TASK-1169".into()),
        spec_id: Some("TASK-1169".into()),
        title: "launcher-owned integration waits".into(),
        in_progress: true,
        orphan_excluded_type: false,
    }];

    let (rows, orphans) = build_running_work(
        &specs,
        &[l],
        &[],
        now,
        |_| dispatch_health_ps::WorktreeGitProbe::default(),
        |_| None,
        |_| None,
    );

    assert_eq!(rows.len(), 1);
    let d = rows[0].dispatch.as_ref().unwrap();
    assert_eq!(
        d.state,
        dispatch_health_ps::DispatchState::AwaitingAgent,
        "a worktree entered by hand 30s ago is awaiting its agent, not dead"
    );
    let hint = d.hint.as_deref().expect("awaiting-agent explains itself");
    assert!(hint.contains("entered by hand"), "{hint}");
    assert!(!hint.contains("process dead"), "{hint}");
    assert!(!hint.contains("dead process"), "{hint}");
    assert!(
        !hint.contains("aida queue work"),
        "the re-dispatch hint would race the operator: {hint}"
    );
    assert!(
        orphans.is_empty(),
        "a hand-entered spec must not be listed as orphaned: {:?}",
        orphans.iter().map(|o| &o.spec).collect::<Vec<_>>()
    );
}

/// The grace window is bounded, and it is scoped to hand-entered leases only:
/// past the window the entered worktree ages into STALLED, and an
/// orchestrator-spawned lease whose agent genuinely died still flags
/// immediately with its unchanged resume hint.
// trace:BUG-778 | ai:claude
#[test]
fn ps_dead_agent_lease_still_flags_stalled_after_the_grace_window() {
    let tmp = tempfile::tempdir().unwrap();
    let now = chrono::Utc::now();
    let grace = dispatch_health_ps::DEFAULT_AWAITING_AGENT_GRACE_SECS as i64;

    // (a) hand-entered, but nothing ever launched — past the window.
    let wt_a = tmp.path().join("story-9");
    std::fs::create_dir_all(&wt_a).unwrap();
    let mut entered = ps_lease("l-entered-old", "STORY-9", wt_a);
    entered.branch = "story-9-widget".into();
    entered.started_at = now - chrono::Duration::seconds(grace + 60);
    entered.manual_enter_at = Some(now - chrono::Duration::seconds(grace + 60));

    // (b) orchestrator-spawned lease, agent dead, no hand-enter stamp.
    let wt_b = tmp.path().join("story-10");
    std::fs::create_dir_all(&wt_b).unwrap();
    let mut spawned = ps_lease("l-spawned", "STORY-10", wt_b);
    spawned.branch = "story-10-widget".into();
    spawned.started_at = now - chrono::Duration::seconds(grace + 60);

    let specs = vec![
        RunningWorkSpec {
            disp: "STORY-9".into(),
            agreed_id: Some("STORY-9".into()),
            spec_id: Some("STORY-9".into()),
            title: "hand-entered, never launched".into(),
            in_progress: true,
            orphan_excluded_type: false,
        },
        RunningWorkSpec {
            disp: "STORY-10".into(),
            agreed_id: Some("STORY-10".into()),
            spec_id: Some("STORY-10".into()),
            title: "agent died".into(),
            in_progress: true,
            orphan_excluded_type: false,
        },
    ];

    let (rows, orphans) = build_running_work(
        &specs,
        &[entered, spawned],
        &[],
        now,
        |_| dispatch_health_ps::WorktreeGitProbe::default(),
        |_| None,
        |_| None,
    );

    let entered_row = rows.iter().find(|r| r.lease.id == "l-entered-old").unwrap();
    let entered_dispatch = entered_row.dispatch.as_ref().unwrap();
    assert_eq!(
        entered_dispatch.state,
        dispatch_health_ps::DispatchState::Stalled,
        "the grace window expires — an entered worktree nobody worked still surfaces"
    );
    // …but even then it never gets the racing re-dispatch verb.
    let entered_hint = entered_dispatch.hint.as_deref().unwrap();
    assert!(!entered_hint.contains("aida queue work"), "{entered_hint}");

    let spawned_row = rows.iter().find(|r| r.lease.id == "l-spawned").unwrap();
    let spawned_dispatch = spawned_row.dispatch.as_ref().unwrap();
    assert_eq!(
        spawned_dispatch.state,
        dispatch_health_ps::DispatchState::Stalled,
        "a genuinely dead agent lease is unchanged by the hand-enter carve-out"
    );
    let spawned_hint = spawned_dispatch.hint.as_deref().unwrap();
    assert!(
        spawned_hint.contains("aida queue work STORY-10"),
        "the ordinary resume hint survives: {spawned_hint}"
    );

    // Both are orphans again once the hand-enter grace lapses.
    let orphaned: Vec<&str> = orphans.iter().map(|o| o.spec.as_str()).collect();
    assert!(orphaned.contains(&"STORY-9"), "{orphaned:?}");
    assert!(orphaned.contains(&"STORY-10"), "{orphaned:?}");
}

/// The pure provenance read: seconds since the hand-over, `None` for an
/// orchestrator-spawned lease, clamped at 0 under clock skew.
// trace:BUG-778 | ai:claude
#[test]
fn ps_manual_enter_secs_reads_the_handover_stamp() {
    let now = chrono::Utc::now();
    let l = ps_lease("l", "STORY-1", std::path::PathBuf::from("/x"));
    assert_eq!(ps_manual_enter_secs(&l, now), None);

    let mut entered = l.clone();
    entered.manual_enter_at = Some(now - chrono::Duration::seconds(45));
    assert_eq!(ps_manual_enter_secs(&entered, now), Some(45));

    // A future stamp (clock skew across hosts) clamps to "just handed over".
    let mut skewed = l.clone();
    skewed.manual_enter_at = Some(now + chrono::Duration::seconds(90));
    assert_eq!(ps_manual_enter_secs(&skewed, now), Some(0));
}

/// The writer `aida worktree enter|add` calls: the stamp must survive the TOML
/// round-trip back into `SessionLease`, and — because it patches a single key
/// rather than reserializing the struct — must preserve lease keys this crate's
/// model doesn't carry (STORY-711 writes `authorized_by` the same way).
// trace:BUG-778 | ai:claude
#[test]
fn mark_lease_manual_enter_round_trips_and_preserves_foreign_keys() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(leases_dir(root)).unwrap();
    let lease = ps_lease("l-stamp", "STORY-3", root.join("wt"));
    let path = lease_path(root, &lease.id);
    let mut body = toml::to_string_pretty(&lease).unwrap();
    // A key `SessionLease` does not model — the round-trip hazard.
    body.push_str("\nauthorized_by = \"advisor-a\"\n");
    std::fs::write(&path, body).unwrap();

    mark_lease_manual_enter(root, &lease.id).expect("stamping a real lease succeeds");

    let back: SessionLease = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let stamped = back
        .manual_enter_at
        .expect("the hand-over stamp survives the round-trip");
    assert!(
        ps_manual_enter_secs(&back, chrono::Utc::now()).unwrap() < 60,
        "the stamp reads as fresh: {stamped}"
    );
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("authorized_by"),
        "a foreign key must not be dropped by the patch"
    );

    // Re-entering refreshes rather than duplicating.
    mark_lease_manual_enter(root, &lease.id).expect("re-enter is idempotent");
    let again: SessionLease = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(again.manual_enter_at.unwrap() >= stamped);
}
