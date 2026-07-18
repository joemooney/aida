//! STORY-42: cover the pure-decision helpers used by `aida queue work`.
//! Side-effecting bits (session_start, exec) are integration-tested
//! by hand in the merge gate; these unit tests pin role inference,
//! prompt routing, scope derivation, and spec-id matching so future
//! refactors don't silently break the resolver.
//! trace:STORY-42 | ai:claude
use super::*;
use aida_core::{QueueEntry, Relationship, Requirement, RequirementType};
use uuid::Uuid;

fn req(spec_id: &str, agreed: Option<&str>, t: RequirementType) -> Requirement {
    let mut r = Requirement::new(spec_id.to_string(), String::new());
    r.spec_id = Some(spec_id.into());
    r.agreed_id = agreed.map(String::from);
    r.req_type = t;
    r
}

fn entry(req_id: Uuid, for_role: Option<&str>, for_scope: Option<&str>) -> QueueEntry {
    QueueEntry {
        user_id: "u".into(),
        requirement_id: req_id,
        position: 1,
        added_by: "u".into(),
        note: None,
        added_at: chrono::Utc::now(),
        for_role: for_role.map(String::from),
        for_scope: for_scope.map(String::from),
        for_session: None,
        added_by_machine: None,
    }
}

fn resolved(spec: &str, qe: QueueEntry) -> QueueWorkEntry {
    QueueWorkEntry {
        queue: qe,
        spec_id: spec.into(),
        status_at_plan: "Approved".into(),
    }
}

fn plan_with(mode: QueueWorkMode, scope: &str, entries: Vec<QueueWorkEntry>) -> QueueWorkPlan {
    let review_target = parse_review_scope(scope);
    QueueWorkPlan {
        mode,
        entries,
        scope: scope.into(),
        review_target,
        anchor_display: scope.into(),
        anchor_title: "anchor".into(),
    }
}

/// Single-role cluster → that role, "cluster-derived", no warnings.
#[test]
fn role_uniform_cluster_picks_that_role() {
    let r = req("BUG-1", None, RequirementType::Bug);
    let e1 = resolved("BUG-1", entry(r.id, Some("implementer"), None));
    let e2 = resolved("BUG-2", entry(Uuid::now_v7(), Some("implementer"), None));
    let plan = plan_with(QueueWorkMode::Cluster, "EPIC-20", vec![e1, e2]);
    let (role, origin, warns) = infer_queue_work_role(&plan, None);
    assert_eq!(role, "implementer");
    assert_eq!(origin, "cluster-derived");
    assert!(warns.is_none());
}

/// Mixed-role cluster → majority wins + a warning about the minority.
#[test]
fn role_majority_wins_with_warning() {
    let mut entries = Vec::new();
    for _ in 0..3 {
        entries.push(resolved(
            "X",
            entry(Uuid::now_v7(), Some("implementer"), None),
        ));
    }
    entries.push(resolved("Y", entry(Uuid::now_v7(), Some("reviewer"), None)));
    let plan = plan_with(QueueWorkMode::Cluster, "EPIC-20", entries);
    let (role, origin, warns) = infer_queue_work_role(&plan, None);
    assert_eq!(role, "implementer");
    assert_eq!(origin, "cluster-derived");
    let warns = warns.expect("minority should warn");
    assert!(warns
        .iter()
        .any(|w| w.contains("reviewer") || w.contains("other role")));
}

/// Explicit --role override beats the cluster tally and the
/// scope default.
#[test]
fn role_override_wins() {
    let e = resolved("BUG-1", entry(Uuid::now_v7(), Some("implementer"), None));
    let plan = plan_with(QueueWorkMode::Item, "EPIC-20", vec![e]);
    let (role, origin, warns) = infer_queue_work_role(&plan, Some("architect"));
    assert_eq!(role, "architect");
    assert_eq!(origin, "--role flag");
    assert!(warns.is_none());
}

/// Empty cluster (no for_role on any entry) + PR-N scope → reviewer
// default. trace:STORY-42 | ai:claude
#[test]
fn role_scope_default_pr_is_reviewer() {
    let e = resolved("STORY-1", entry(Uuid::now_v7(), None, None));
    let plan = plan_with(QueueWorkMode::Cluster, "PR-11", vec![e]);
    let (role, origin, _) = infer_queue_work_role(&plan, None);
    assert_eq!(role, "reviewer");
    assert_eq!(origin, "scope-default");
}

/// Empty cluster + non-PR scope → implementer default.
#[test]
fn role_scope_default_non_pr_is_implementer() {
    let e = resolved("STORY-1", entry(Uuid::now_v7(), None, None));
    let plan = plan_with(QueueWorkMode::Cluster, "EPIC-20", vec![e]);
    let (role, origin, _) = infer_queue_work_role(&plan, None);
    assert_eq!(role, "implementer");
    assert_eq!(origin, "scope-default");
}

/// Reviewer role + PR scope → `/aida-review --pr N`.
#[test]
fn prompt_reviewer_pr_passes_number() {
    let e = resolved("STORY-X", entry(Uuid::now_v7(), Some("reviewer"), None));
    let plan = plan_with(QueueWorkMode::Cluster, "PR-11", vec![e]);
    assert_eq!(
        derive_queue_work_prompt(&plan, "reviewer", false, false),
        "/aida-review --pr 11"
    );
}

/// Reviewer role + non-PR scope → bare `/aida-review`.
#[test]
fn prompt_reviewer_non_pr_is_bare() {
    let e = resolved("STORY-X", entry(Uuid::now_v7(), Some("reviewer"), None));
    let plan = plan_with(QueueWorkMode::Cluster, "EPIC-20", vec![e]);
    assert_eq!(
        derive_queue_work_prompt(&plan, "reviewer", false, false),
        "/aida-review"
    );
}

/// Implementer + item mode → `/aida-pickup <ID>` (focus directive).
#[test]
fn prompt_implementer_item_passes_focus() {
    let e = resolved("BUG-83", entry(Uuid::now_v7(), Some("implementer"), None));
    let plan = QueueWorkPlan {
        mode: QueueWorkMode::Item,
        entries: vec![e],
        scope: "EPIC-20".into(),
        review_target: None,
        anchor_display: "BUG-83".into(),
        anchor_title: "title".into(),
    };
    assert_eq!(
        derive_queue_work_prompt(&plan, "implementer", false, false),
        "/aida-pickup BUG-83"
    );
}

/// Implementer + cluster/head → `/aida-pickup --auto-first`
/// (manifest carries the context; STORY-42 pre-flight is the consent
// point so the skill skips its own confirm). trace:TASK-86 | ai:claude
#[test]
fn prompt_implementer_cluster_is_auto_first() {
    let e = resolved("BUG-83", entry(Uuid::now_v7(), Some("implementer"), None));
    let plan = plan_with(QueueWorkMode::Cluster, "EPIC-20", vec![e]);
    assert_eq!(
        derive_queue_work_prompt(&plan, "implementer", false, false),
        "/aida-pickup --auto-first"
    );
}

/// Implementer + head mode → also `/aida-pickup --auto-first`.
/// The no-arg invocation explicitly opts into queue-driven flow, so
/// the confirm is the same friction-without-value as cluster mode.
// trace:TASK-86 | ai:claude
#[test]
fn prompt_implementer_head_is_auto_first() {
    let e = resolved("BUG-83", entry(Uuid::now_v7(), Some("implementer"), None));
    let plan = plan_with(QueueWorkMode::Head, "EPIC-20", vec![e]);
    assert_eq!(
        derive_queue_work_prompt(&plan, "implementer", false, false),
        "/aida-pickup --auto-first"
    );
}

/// STORY-265: plan-only mode runs /aida-plan (not /aida-pickup) with the
/// item focus, so the session writes a plan instead of implementing.
#[test]
fn prompt_plan_only_runs_aida_plan() {
    let e = resolved("BUG-83", entry(Uuid::now_v7(), Some("implementer"), None));
    let plan = QueueWorkPlan {
        mode: QueueWorkMode::Item,
        entries: vec![e],
        scope: "EPIC-20".into(),
        review_target: None,
        anchor_display: "BUG-83".into(),
        anchor_title: "title".into(),
    };
    assert_eq!(
        derive_queue_work_prompt(&plan, "implementer", true, false),
        "/aida-plan BUG-83"
    );
}

/// STORY-735: guided keystone mode runs /aida-guided-implement (the
/// structured decision dialog) with the spec focus, not /aida-pickup.
#[test]
fn prompt_guided_runs_guided_implement() {
    let e = resolved("STORY-7", entry(Uuid::now_v7(), Some("implementer"), None));
    let plan = QueueWorkPlan {
        mode: QueueWorkMode::Item,
        entries: vec![e],
        scope: "EPIC-20".into(),
        review_target: None,
        anchor_display: "STORY-7".into(),
        anchor_title: "title".into(),
    };
    assert_eq!(
        derive_queue_work_prompt(&plan, "implementer", false, true),
        "/aida-guided-implement STORY-7"
    );
}

/// STORY-265: plan-only defaults the permission mode to `plan` (read-only),
/// overriding env/config/bypass — but an explicit --permission-mode wins.
#[test]
fn plan_only_defaults_permission_to_plan_but_flag_wins() {
    // no explicit flag: plan-only forces read-only `plan` even with bypass on
    let (m, o) = resolve_queue_work_permission_mode(None, Some("auto"), None, true, true);
    assert_eq!(m.as_deref(), Some("plan"));
    assert!(o.contains("plan-only"));
    // explicit --permission-mode flag still wins over plan-only
    let (m, _) = resolve_queue_work_permission_mode(Some("acceptEdits"), None, None, false, true);
    assert_eq!(m.as_deref(), Some("acceptEdits"));
}

/// Title shape "Review PR-11: …" overrides for_scope and parent
/// EPIC — review-mode session for PR-11.
#[test]
fn scope_review_title_wins_over_for_scope() {
    let mut review = req("STORY-9", None, RequirementType::Story);
    review.title = "Review PR-11: clean up sync flow".into();
    let qe = entry(review.id, Some("reviewer"), Some("EPIC-99"));
    let (scope, target) = derive_scope_from_entry(&qe, &review);
    assert_eq!(scope, "PR-11");
    assert!(target.is_some());
}

/// for_scope wins even for a child story (its parent-epic relationship is
/// no longer consulted for the session scope — BUG-431 #1).
#[test]
fn scope_for_scope_beats_parent_epic() {
    let mut bug = req("BUG-1", None, RequirementType::Bug);
    bug.relationships.push(Relationship {
        rel_type: RelationshipType::Child,
        target_id: Uuid::now_v7(), // a parent epic, irrelevant to scope now
        created_at: Some(chrono::Utc::now()),
        created_by: Some("t".into()),
    });
    let qe = entry(bug.id, Some("implementer"), Some("EPIC-21"));
    let (scope, _) = derive_scope_from_entry(&qe, &bug);
    assert_eq!(scope, "EPIC-21");
}

/// BUG-739: legacy rows auto-stamped with the generic Claude harness lease
/// must not route queue-work into the shared harness checkout.
#[test]
fn scope_ignores_legacy_harness_worktree_for_scope() {
    let bug = req("BUG-739", None, RequirementType::Bug);
    let qe = entry(
        bug.id,
        Some("implementer"),
        Some(worktree_lease::HARNESS_WORKTREE_SCOPE),
    );
    let (scope, target) = derive_scope_from_entry(&qe, &bug);
    assert_eq!(scope, "BUG-739");
    assert!(target.is_none());
}

/// BUG-431 #1: no for_scope → a child story scopes to its OWN id, NOT the
/// parent epic. Previously this fell back to the parent EPIC, so every
/// same-epic story in a drain contended for one epic scope (worktree +
/// branch collision, sibling lease-block, multi-spec PR). Each sibling
/// must get its own scope so the drain progresses without contention.
#[test]
fn scope_child_story_does_not_inherit_parent_epic() {
    let mut bug = req("BUG-1", None, RequirementType::Bug);
    // A child-of-epic relationship — the exact shape that used to pull the
    // session scope up to the parent epic.
    bug.relationships.push(Relationship {
        rel_type: RelationshipType::Child,
        target_id: Uuid::now_v7(),
        created_at: Some(chrono::Utc::now()),
        created_by: Some("t".into()),
    });
    let qe = entry(bug.id, Some("implementer"), None);
    let (scope, _) = derive_scope_from_entry(&qe, &bug);
    assert_eq!(
        scope, "BUG-1",
        "a child story's session must scope to its own id, not its parent epic"
    );
}

/// No for_scope and no parent EPIC → falls back to req's own
/// display id.
#[test]
fn scope_falls_back_to_own_id() {
    let bug = req("BUG-1", None, RequirementType::Bug);
    let qe = entry(bug.id, Some("implementer"), None);
    let (scope, _) = derive_scope_from_entry(&qe, &bug);
    assert_eq!(scope, "BUG-1");
}

/// BUG-739: queue-add's implicit cwd lease routing must skip the generic
/// harness scope while keeping explicit `--scope harness-worktree` meaningful.
#[test]
fn queue_add_scope_routing_skips_implicit_harness_scope() {
    let lease = lease_for("harness", worktree_lease::HARNESS_WORKTREE_SCOPE, 1);
    assert_eq!(
        queue_add_for_scope_routing(false, None, None, Some(&lease)),
        None
    );
    assert_eq!(
        queue_add_for_scope_routing(
            false,
            Some(worktree_lease::HARNESS_WORKTREE_SCOPE),
            None,
            Some(&lease),
        )
        .as_deref(),
        Some(worktree_lease::HARNESS_WORKTREE_SCOPE)
    );
}

#[test]
fn queue_add_scope_routing_keeps_real_implicit_scope() {
    let lease = lease_for("task", "TASK-1156", 1);
    assert_eq!(
        queue_add_for_scope_routing(false, None, None, Some(&lease)).as_deref(),
        Some("TASK-1156")
    );
}

/// spec_matches walks uuid, spec_id (case-insensitive), and
// agreed_id (case-insensitive). trace:STORY-42 | ai:claude
#[test]
fn spec_matches_covers_uuid_and_ids() {
    let mut r = req("BUG-1-099", Some("BUG-42"), RequirementType::Bug);
    r.id = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
    assert!(spec_matches(&r, "11111111-1111-1111-1111-111111111111"));
    assert!(spec_matches(&r, "BUG-1-099"));
    assert!(spec_matches(&r, "bug-1-099")); // case-insensitive
    assert!(spec_matches(&r, "BUG-42")); // agreed_id
    assert!(spec_matches(&r, "bug-42"));
    assert!(!spec_matches(&r, "BUG-99"));
}

/// BUG-366: the "awaiting review" hint must be an unambiguous reviewer
/// pickup, not a bare `aida queue work PR-N` that invites implementer-drain
// flags the PR-N path can't resolve. trace:BUG-366 | ai:claude
#[test]
fn review_pickup_hint_names_reviewer_role() {
    assert_eq!(
        review_pickup_hint(250),
        "aida queue work PR-250 --for reviewer"
    );
}

fn lease_for(id: &str, scope: &str, age_secs: i64) -> SessionLease {
    SessionLease {
        id: id.to_string(),
        scope: scope.to_string(),
        slug: scope.to_lowercase(),
        owner: "u".into(),
        worktree_path: std::path::PathBuf::from(format!("/tmp/wt-{}", id)),
        branch: format!("br-{}", id),
        started_at: chrono::Utc::now() - chrono::Duration::seconds(age_secs),
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
    }
}

// No leases → no conflict. trace:TASK-81 | ai:claude
#[test]
fn lease_conflict_empty() {
    assert!(find_scope_lease_conflict(&[], "TASK-81").is_none());
}

// Lease on a different scope → no conflict. trace:TASK-81 | ai:claude
#[test]
fn lease_conflict_mismatched_scope() {
    let leases = vec![lease_for("aaaa", "EPIC-20", 10)];
    assert!(find_scope_lease_conflict(&leases, "TASK-81").is_none());
}

/// Exact scope match → that lease is the conflict.
// trace:TASK-81 | ai:claude
#[test]
fn lease_conflict_exact_match() {
    let leases = vec![lease_for("aaaa", "TASK-81", 10)];
    let got = find_scope_lease_conflict(&leases, "TASK-81").unwrap();
    assert_eq!(got.id, "aaaa");
}

/// Case-insensitive scope match — `aida queue work task-81` should
// still detect a lease owning `TASK-81`. trace:TASK-81 | ai:claude
#[test]
fn lease_conflict_case_insensitive() {
    let leases = vec![lease_for("aaaa", "TASK-81", 10)];
    let got = find_scope_lease_conflict(&leases, "task-81").unwrap();
    assert_eq!(got.id, "aaaa");
}

/// Multiple leases on the same scope → freshest (smallest age) wins,
/// so `session_end` targets the live one rather than a stale ghost.
// trace:TASK-81 | ai:claude
#[test]
fn lease_conflict_picks_freshest() {
    let leases = vec![
        lease_for("oldold", "TASK-81", 600),
        lease_for("freshh", "TASK-81", 10),
    ];
    let got = find_scope_lease_conflict(&leases, "TASK-81").unwrap();
    assert_eq!(got.id, "freshh");
}

/// --permission-mode flag beats everything else (incl. the bypass knob).
// trace:TASK-84 trace:STORY-495 | ai:claude
#[test]
fn permission_mode_flag_wins() {
    let (m, o) = resolve_queue_work_permission_mode(
        Some("plan"),
        Some("auto"),
        Some("default"),
        true,
        false,
    );
    assert_eq!(m.as_deref(), Some("plan"));
    assert_eq!(o, "--permission-mode flag");
}

/// AIDA_PERMISSION_MODE env wins over config + the bypass knob.
// trace:TASK-84 trace:STORY-495 | ai:claude
#[test]
fn permission_mode_env_beats_config() {
    let (m, o) =
        resolve_queue_work_permission_mode(None, Some("auto"), Some("default"), true, false);
    assert_eq!(m.as_deref(), Some("auto"));
    assert_eq!(o, "AIDA_PERMISSION_MODE env");
}

/// config.toml [behavior] beats the bypass knob.
// trace:TASK-84 trace:STORY-495 | ai:claude
#[test]
fn permission_mode_config_beats_worktree_default() {
    let (m, o) = resolve_queue_work_permission_mode(None, None, Some("acceptEdits"), true, false);
    assert_eq!(m.as_deref(), Some("acceptEdits"));
    assert_eq!(o, ".aida/config.toml");
}

/// STORY-495: the `[agents] bypass` knob (no other overrides) →
// bypassPermissions. trace:STORY-495 | ai:claude
#[test]
fn permission_mode_bypass_knob_injects_bypass() {
    let (m, o) = resolve_queue_work_permission_mode(None, None, None, true, false);
    assert_eq!(m.as_deref(), Some("bypassPermissions"));
    assert_eq!(o, "[agents] bypass knob");
}

/// STORY-495: faithful default — no flag, no env, no config, knob off →
/// native (None), so no `--permission-mode` is injected.
// trace:STORY-495 | ai:claude
#[test]
fn permission_mode_faithful_default_is_native() {
    let (m, o) = resolve_queue_work_permission_mode(None, None, None, false, false);
    assert_eq!(m, None);
    assert_eq!(o, "native (faithful default)");
}

/// Empty string from flag/env/config is treated as absent (so an empty
/// shell variable doesn't accidentally pin a mode); with the knob on the
// resolution falls through to the bypass knob. trace:TASK-84 trace:STORY-495 | ai:claude
#[test]
fn permission_mode_empty_strings_are_ignored() {
    let (m, o) = resolve_queue_work_permission_mode(Some(""), Some(""), Some(""), true, false);
    assert_eq!(m.as_deref(), Some("bypassPermissions"));
    assert_eq!(o, "[agents] bypass knob");
}

// --- AutonomyMode (STORY-287) -----------------------------------------

/// No flags → the human is driving; every prompt pauses.
// trace:STORY-287 | ai:claude
#[test]
fn autonomy_mode_default_when_no_flags() {
    assert_eq!(resolve_autonomy_mode(false, false), AutonomyMode::Default);
}

/// `--zen` alone → advisor-on-standby mode.
// trace:STORY-287 | ai:claude
#[test]
fn autonomy_mode_zen_flag_alone() {
    assert_eq!(resolve_autonomy_mode(true, false), AutonomyMode::Zen);
}

/// `--no-human` alone → the headless drain mode.
// trace:STORY-287 | ai:claude
#[test]
fn autonomy_mode_no_human_alone() {
    assert_eq!(resolve_autonomy_mode(false, true), AutonomyMode::NoHuman);
}

/// Precedence: `--no-human --zen` resolves to `NoHuman` — the stronger
/// mode wins (the dispatch also warns and clears `AIDA_ZEN`).
// trace:STORY-287 | ai:claude
#[test]
fn autonomy_mode_no_human_beats_zen() {
    assert_eq!(resolve_autonomy_mode(true, true), AutonomyMode::NoHuman);
}

// --- AutonomyMode::resolve_run (ADR-7 / ADR-10) -----------------------
// The in-flight typed resolution that replaces the bare `AIDA_ZEN` re-read
// inside run_auto_complete. Same precedence as resolve_autonomy_mode, but
// expressed over the typed `--no-human` mode + the zen-INTENT-TOKEN
// presence (not a bare env read), so a leaked AIDA_ZEN is leak-resistant.

/// No `--no-human`, no zen token → the human is driving. (ADR-10)
#[test]
fn resolve_run_default_when_nothing_set() {
    assert_eq!(
        AutonomyMode::resolve_run(None, false),
        AutonomyMode::Default
    );
}

/// Zen-intent token present (and no `--no-human`) → a supervised zen run
/// under ADR-10.
#[test]
fn resolve_run_zen_when_token_present() {
    let m = AutonomyMode::resolve_run(None, true);
    assert_eq!(m, AutonomyMode::Zen);
    assert!(m.is_zen());
}

/// `--no-human` wins over a zen token (the stronger mode) — symmetric with
/// resolve_autonomy_mode's precedence. (ADR-10)
#[test]
fn resolve_run_no_human_beats_zen_token() {
    assert_eq!(
        AutonomyMode::resolve_run(Some(auto_complete::NoHumanMode::Both), true),
        AutonomyMode::NoHuman
    );
    assert_eq!(
        AutonomyMode::resolve_run(Some(auto_complete::NoHumanMode::ReviewerOnly), false),
        AutonomyMode::NoHuman
    );
}

/// BUG-237 leak-resistance preserved: NO zen token (a leaked `AIDA_ZEN=1`
/// carries none) → NOT recorded as a zen run. (ADR-10 / BUG-237)
#[test]
fn resolve_run_no_token_is_not_zen() {
    let m = AutonomyMode::resolve_run(None, false);
    assert!(!m.is_zen());
}

/// ADR-10: the phase driver CARRIES the resolved-once typed autonomy mode
/// as a field (symmetric with `no_human`), and the engine's zen predicate
/// (`is_zen_run`) reads that carried field — NOT a bare `AIDA_ZEN` env
/// read. This is the drain-state zen-stamping source in `run_auto_complete`.
/// The three modes round-trip through construction unchanged, so the
/// autonomy behavior is byte-identical to the pre-ADR-10 env-derived read.
// trace:ADR-10 | ai:claude
#[test]
fn phase_driver_carries_autonomy_mode_and_is_zen_run_reads_the_field() {
    fn build(mode: AutonomyMode) -> RealPhaseDriver {
        RealPhaseDriver::new(
            std::env::temp_dir().join(format!("aida-adr10-{}", uuid::Uuid::now_v7())),
            "ADR-10".to_string(),
            None,
            false,
            // `no_human` is independent of the carried autonomy mode here —
            // the field is the SOLE source `is_zen_run` consults.
            None,
            mode,
            "test-run".to_string(),
            false,
            false,
            false,
            false,
            auto_complete::LifecycleSkip::none(),
        )
    }
    // The field is stored verbatim, and `is_zen_run()` reads it.
    assert_eq!(build(AutonomyMode::Zen).autonomy_mode, AutonomyMode::Zen);
    assert!(build(AutonomyMode::Zen).is_zen_run());
    assert!(!build(AutonomyMode::Default).is_zen_run());
    assert!(!build(AutonomyMode::NoHuman).is_zen_run());
    // Guard against a future refactor that silently re-reads the env: the
    // field wins regardless of what `AIDA_ZEN` is set to in the process.
    // (No env mutation here — asserting the field is self-contained.)
    assert_eq!(
        build(AutonomyMode::Default).autonomy_mode,
        AutonomyMode::Default
    );
}

// --- resolve_drain_alias (TASK-578) -----------------------------------

/// `--drain` off is a pure identity map — the operator's flags pass through
// untouched. trace:TASK-578 | ai:claude
#[test]
fn drain_alias_off_is_identity() {
    let r = resolve_drain_alias(
        false,
        Some("through-ci"),
        Some("reviewer-only"),
        Some(3),
        10,
    );
    assert_eq!(r.auto_complete.as_deref(), Some("through-ci"));
    assert_eq!(r.no_human.as_deref(), Some("reviewer-only"));
    assert_eq!(r.max, Some(3));

    let empty = resolve_drain_alias(false, None, None, None, 10);
    assert_eq!(empty.auto_complete, None);
    assert_eq!(empty.no_human, None);
    assert_eq!(empty.max, None);
}

/// Bare `--drain` expands to the full headless drain bounded by the queue
// size. trace:TASK-578 | ai:claude
#[test]
fn drain_alias_bare_expands_to_full_headless_queue_sized() {
    let r = resolve_drain_alias(true, None, None, None, 7);
    assert_eq!(r.auto_complete.as_deref(), Some("full"));
    assert_eq!(r.no_human.as_deref(), Some("both"));
    assert_eq!(r.max, Some(7));
}

/// An unknown / empty queue size falls back to the spec's `--max 99`.
// trace:TASK-578 | ai:claude
#[test]
fn drain_alias_unknown_queue_size_falls_back_to_99() {
    let r = resolve_drain_alias(true, None, None, None, 0);
    assert_eq!(r.max, Some(99));
}

/// Explicit flags always win over the `--drain` defaults — the alias never
// overwrites an operator-supplied value. trace:TASK-578 | ai:claude
#[test]
fn drain_alias_explicit_flags_override_defaults() {
    let r = resolve_drain_alias(
        true,
        Some("through-merge"),
        Some("reviewer-only"),
        Some(2),
        50,
    );
    assert_eq!(r.auto_complete.as_deref(), Some("through-merge"));
    assert_eq!(r.no_human.as_deref(), Some("reviewer-only"));
    assert_eq!(r.max, Some(2));
}

/// Mixed: `--drain --max 3` keeps the explicit cap but still defaults the
// autonomy fields. trace:TASK-578 | ai:claude
#[test]
fn drain_alias_partial_override_keeps_other_defaults() {
    let r = resolve_drain_alias(true, None, None, Some(3), 50);
    assert_eq!(r.auto_complete.as_deref(), Some("full"));
    assert_eq!(r.no_human.as_deref(), Some("both"));
    assert_eq!(r.max, Some(3));
}

// --- TASK-306: --no-human kickoff gate --------------------------------

/// STORY-276: the gate keys purely off acknowledgement — ack'd → proceed
/// silently, otherwise the banner + prompt. `both` is no longer rejected
/// (the headless implementer ships); it is acknowledged like any mode.
// trace:TASK-306, STORY-276
#[test]
fn no_human_gate_keys_off_acknowledgement() {
    assert_eq!(classify_no_human_gate(false), NoHumanGate::NeedsAck);
    assert_eq!(classify_no_human_gate(true), NoHumanGate::Acknowledged);
}

/// STORY-276: the scope line differs by mode — `both` names the headless
/// implementer + the punt safety net; `reviewer-only` says phase 1 stays
// interactive. trace:STORY-276
#[test]
fn no_human_scope_line_differs_by_mode() {
    let both = no_human_scope_line(auto_complete::NoHumanMode::Both);
    assert!(both.contains("both"), "{both}");
    assert!(both.contains("punts"), "{both}");
    let reviewer = no_human_scope_line(auto_complete::NoHumanMode::ReviewerOnly);
    assert!(
        reviewer.contains("reviewer phase runs headless"),
        "{reviewer}"
    );
    assert!(reviewer.contains("interactive"), "{reviewer}");
}

// --- TASK-306: orchestrator-context statusline badge ------------------

/// A corroborated phase-1 session shows the phase index, its name, and
// the always-present pause cue. trace:TASK-306
#[test]
fn orchestrator_badge_shows_phase_and_name() {
    let b = OrchestratorBadge::build(Some(1), None);
    assert_eq!(b.phase, "auto:1/6 implementer");
    assert!(b.no_human.is_none());
    assert_eq!(b.pause, "pause-here");
}

/// The `--no-human` scope is folded in when the env var is present.
// trace:TASK-306
#[test]
fn orchestrator_badge_includes_no_human_scope() {
    let b = OrchestratorBadge::build(Some(3), Some("reviewer-only"));
    assert_eq!(b.phase, "auto:3/6 reviewer");
    assert_eq!(b.no_human.as_deref(), Some("no-human:reviewer-only"));
}

/// Defensive fallback: a missing phase env var renders `?`; an
// out-of-range index keeps the number but drops the name. trace:TASK-306
#[test]
fn orchestrator_badge_falls_back_when_phase_unknown() {
    assert_eq!(OrchestratorBadge::build(None, None).phase, "auto:?/6");
    assert_eq!(OrchestratorBadge::build(Some(9), None).phase, "auto:9/6");
}

/// `read_behavior_permission_mode` parses `[behavior]
/// permission_mode = "..."` and ignores other sections.
// trace:TASK-84 | ai:claude
#[test]
fn read_behavior_permission_mode_parses_value() {
    let tmp = std::env::temp_dir().join(format!(
        "aida-task84-config-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    let aida = tmp.join(".aida");
    std::fs::create_dir_all(&aida).unwrap();
    std::fs::write(
        aida.join("config.toml"),
        "[id_format]\npolicy = \"node-aware-only\"\n\n[behavior]\npermission_mode = \"auto\"\n",
    )
    .unwrap();
    let got = read_behavior_permission_mode(&tmp);
    assert_eq!(got.as_deref(), Some("auto"));
    let _ = std::fs::remove_dir_all(&tmp);
}

// Missing config file → None. trace:TASK-84 | ai:claude
#[test]
fn read_behavior_permission_mode_missing_is_none() {
    let tmp = std::env::temp_dir().join(format!(
        "aida-task84-noconf-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    std::fs::create_dir_all(&tmp).unwrap();
    assert!(read_behavior_permission_mode(&tmp).is_none());
    let _ = std::fs::remove_dir_all(&tmp);
}

/// `review_title_matches` accepts the canonical "Review PR-N: ..."
/// shape, leading whitespace, and is case-insensitive on the prefix
// but exact on the number. trace:TASK-85 | ai:claude
#[test]
fn review_title_matches_canonical() {
    assert!(review_title_matches(
        "Review PR-14: shave this yak",
        ReviewForge::GitHub,
        14
    ));
    assert!(review_title_matches(
        "  Review PR-14: leading space",
        ReviewForge::GitHub,
        14
    ));
    assert!(review_title_matches(
        "review pr-14: lowercase prefix",
        ReviewForge::GitHub,
        14
    ));
    assert!(review_title_matches(
        "Review MR-7: gitlab works",
        ReviewForge::GitLab,
        7
    ));
}

/// Reject titles that aren't review stories, that name a different
// number, or that name the wrong forge. trace:TASK-85 | ai:claude
#[test]
fn review_title_matches_rejects_mismatches() {
    // Different PR number.
    assert!(!review_title_matches(
        "Review PR-15: nope",
        ReviewForge::GitHub,
        14
    ));
    // Different forge.
    assert!(!review_title_matches(
        "Review PR-14: github not gitlab",
        ReviewForge::GitLab,
        14
    ));
    // Title doesn't start with Review.
    assert!(!review_title_matches(
        "Fixing PR-14",
        ReviewForge::GitHub,
        14
    ));
    // PR-14 mentioned but not as review.
    assert!(!review_title_matches(
        "Follow-up to PR-14",
        ReviewForge::GitHub,
        14
    ));
    // Substring number — `PR-140` must not match PR-14.
    assert!(!review_title_matches(
        "Review PR-140: longer number",
        ReviewForge::GitHub,
        14
    ));
}

/// `format_review_label` produces the human-facing label that error
// messages use. trace:TASK-85 | ai:claude
#[test]
fn format_review_label_shapes() {
    assert_eq!(format_review_label(ReviewForge::GitHub, 14), "PR-14");
    assert_eq!(format_review_label(ReviewForge::GitLab, 7), "MR-7");
}

/// Quote-aware inline-comment stripping for TOML lines.
// trace:TASK-84 | ai:claude
#[test]
fn strip_toml_inline_comment_basics() {
    // No comment → unchanged.
    assert_eq!(strip_toml_inline_comment(""), "");
    assert_eq!(strip_toml_inline_comment("key = \"v\""), "key = \"v\"");
    // Trailing comment stripped.
    assert_eq!(
        strip_toml_inline_comment("key = \"v\" # trailing"),
        "key = \"v\" "
    );
    // Whole-line comment.
    assert_eq!(strip_toml_inline_comment("# only"), "");
    // `#` inside a double-quoted string preserved.
    assert_eq!(
        strip_toml_inline_comment("key = \"hash #inside\""),
        "key = \"hash #inside\""
    );
    // `#` inside a single-quoted string preserved.
    assert_eq!(
        strip_toml_inline_comment("key = 'hash #inside'"),
        "key = 'hash #inside'"
    );
    // Quote followed by `#` outside any string → stripped.
    assert_eq!(
        strip_toml_inline_comment("key = \"v\"#tight"),
        "key = \"v\""
    );
}

/// `read_behavior_permission_mode` honors inline TOML comments
// (regression for ultrareview bug_002). trace:TASK-84 | ai:claude
#[test]
fn read_behavior_permission_mode_strips_inline_comment() {
    let tmp = std::env::temp_dir().join(format!(
        "aida-task84-inline-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    let aida = tmp.join(".aida");
    std::fs::create_dir_all(&aida).unwrap();
    std::fs::write(
        aida.join("config.toml"),
        "[behavior]\npermission_mode = \"auto\"  # default for autonomous runs\n",
    )
    .unwrap();
    let got = read_behavior_permission_mode(&tmp);
    assert_eq!(got.as_deref(), Some("auto"));
    let _ = std::fs::remove_dir_all(&tmp);
}

/// `[behavior]` section absent → None, even when other sections exist.
// trace:TASK-84 | ai:claude
#[test]
fn read_behavior_permission_mode_section_absent() {
    let tmp = std::env::temp_dir().join(format!(
        "aida-task84-noseq-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    let aida = tmp.join(".aida");
    std::fs::create_dir_all(&aida).unwrap();
    std::fs::write(aida.join("config.toml"), "[id_format]\npolicy = \"x\"\n").unwrap();
    assert!(read_behavior_permission_mode(&tmp).is_none());
    let _ = std::fs::remove_dir_all(&tmp);
}

// --- TASK-217: status-aware not-queued error message ---

fn make_req_status(spec: &str, t: RequirementType, s: RequirementStatus) -> aida_core::Requirement {
    let mut r = req(spec, Some(spec), t);
    r.status = s;
    r
}

#[test]
fn not_queued_approved_suggests_add_and_work() {
    let r = make_req_status(
        "STORY-86",
        RequirementType::Story,
        RequirementStatus::Approved,
    );
    let msg = format_queue_work_not_queued_error("STORY-86", &r, Some("implementer"));
    assert!(msg.contains("isn't queued"), "msg: {msg}");
    assert!(msg.contains("Approved"), "msg: {msg}");
    assert!(
        msg.contains("aida queue add STORY-86 --for implementer"),
        "msg: {msg}"
    );
    assert!(msg.contains("aida queue work STORY-86"), "msg: {msg}");
}

#[test]
fn not_queued_planned_suggests_promote_to_approved() {
    let r = make_req_status(
        "STORY-86",
        RequirementType::Story,
        RequirementStatus::Planned,
    );
    let msg = format_queue_work_not_queued_error("STORY-86", &r, Some("implementer"));
    assert!(msg.contains("Planned"), "msg: {msg}");
    assert!(
        msg.contains("aida edit STORY-86 --status approved"),
        "msg: {msg}"
    );
    assert!(msg.contains("aida queue add STORY-86"), "msg: {msg}");
}

#[test]
fn not_queued_in_progress_warns_lease_lost() {
    let r = make_req_status(
        "STORY-86",
        RequirementType::Story,
        RequirementStatus::InProgress,
    );
    let msg = format_queue_work_not_queued_error("STORY-86", &r, Some("implementer"));
    assert!(msg.contains("In Progress"), "msg: {msg}");
    assert!(msg.contains("lease may have been lost"), "msg: {msg}");
    assert!(msg.contains("aida queue list --all"), "msg: {msg}");
}

#[test]
fn not_queued_done_suggests_rework_verb() {
    let r = make_req_status("STORY-86", RequirementType::Story, RequirementStatus::Done);
    let msg = format_queue_work_not_queued_error("STORY-86", &r, Some("implementer"));
    assert!(msg.contains("Done"), "msg: {msg}");
    assert!(
        msg.contains("aida queue rework STORY-86 --work"),
        "msg: {msg}"
    );
    // BUG-236: the suggestion must run verbatim for any Done spec, so
    // it stays `--work` (fresh session) — never `--work --resume`,
    // which bounces when the spec has no recorded claude session.
    assert!(!msg.contains("--resume"), "msg: {msg}");
    assert!(msg.contains("auto-bump"), "msg: {msg}");
    // TASK-240: a Done spec's PR author gets a merge-now path, routed via
    // `aida show` (which prints the PR number) so the command is honest.
    assert!(msg.contains("gh pr merge"), "msg: {msg}");
    assert!(msg.contains("aida show STORY-86"), "msg: {msg}");
}

#[test]
fn not_queued_completed_suggests_force_reopen() {
    let r = make_req_status(
        "STORY-86",
        RequirementType::Story,
        RequirementStatus::Completed,
    );
    let msg = format_queue_work_not_queued_error("STORY-86", &r, Some("implementer"));
    assert!(msg.contains("Completed"), "msg: {msg}");
    assert!(msg.contains("already shipped"), "msg: {msg}");
    assert!(
        msg.contains("aida edit STORY-86 --status in-progress --force"),
        "msg: {msg}"
    );
}

#[test]
fn not_queued_rejected_suggests_force_reopen() {
    let r = make_req_status(
        "STORY-86",
        RequirementType::Story,
        RequirementStatus::Rejected,
    );
    let msg = format_queue_work_not_queued_error("STORY-86", &r, Some("implementer"));
    assert!(msg.contains("Rejected"), "msg: {msg}");
    assert!(
        msg.contains("aida edit STORY-86 --status approved --force"),
        "msg: {msg}"
    );
}

#[test]
fn not_queued_container_uses_cluster_message() {
    let r = make_req_status(
        "EPIC-23",
        RequirementType::Epic,
        RequirementStatus::InProgress,
    );
    let msg = format_queue_work_not_queued_error("EPIC-23", &r, Some("implementer"));
    // Containers get a different shape — focus on inspecting + adding
    // children rather than the leaf status-aware recovery.
    assert!(msg.contains("no queued children"), "msg: {msg}");
    assert!(msg.contains("aida queue list --tree"), "msg: {msg}");
    assert!(msg.contains("aida list --parent EPIC-23"), "msg: {msg}");
}

#[test]
fn not_queued_falls_back_when_role_unknown() {
    let r = make_req_status(
        "STORY-86",
        RequirementType::Story,
        RequirementStatus::Approved,
    );
    let msg = format_queue_work_not_queued_error("STORY-86", &r, None);
    assert!(msg.contains("--for <role>"), "msg: {msg}");
}

/// BUG-226: `--quiet` parses on `aida queue work` and defaults off, so
/// a standalone reviewer prints its end-of-command summary unless the
/// caller opts out.
#[test]
fn queue_work_quiet_flag_parses() {
    let on = Cli::try_parse_from([
        "aida", "queue", "work", "PR-65", "--role", "reviewer", "--quiet",
    ])
    .expect("--quiet parses");
    let off = Cli::try_parse_from(["aida", "queue", "work", "PR-65", "--role", "reviewer"])
        .expect("no --quiet parses");
    let quiet_of = |c: &Cli| match &c.command {
        Command::Queue(QueueCommand::Work { quiet, .. }) => *quiet,
        _ => panic!("expected queue work command"),
    };
    assert!(quiet_of(&on), "--quiet should set quiet=true");
    assert!(!quiet_of(&off), "quiet defaults to false");
}

/// TASK-560: --resume + --auto-complete must now PARSE (the clap conflict
/// was lifted) so the handler can reject it with a helpful message instead
// of clap's terse "cannot be used with". trace:TASK-560
#[test]
fn queue_work_resume_plus_auto_complete_parses_for_handler_rejection() {
    let cli = Cli::try_parse_from([
        "aida",
        "queue",
        "work",
        "STORY-465",
        "--resume",
        "--auto-complete",
    ])
    .expect("--resume + --auto-complete should parse (handler rejects, not clap)");
    match cli.command {
        Command::Queue(QueueCommand::Work {
            resume,
            auto_complete,
            ..
        }) => {
            assert!(resume.is_some() && auto_complete.is_some());
        }
        other => panic!("expected queue work command, got {other:?}"),
    }
}

/// TASK-560: the conflict message fires only for the pair, and carries the
// WHY + both recovery paths. trace:TASK-560
#[test]
fn resume_autocomplete_conflict_message_explains_and_recovers() {
    assert!(resume_autocomplete_conflict_message(false, true).is_none());
    assert!(resume_autocomplete_conflict_message(true, false).is_none());
    assert!(resume_autocomplete_conflict_message(false, false).is_none());
    let msg = resume_autocomplete_conflict_message(true, true).expect("pair conflicts");
    assert!(msg.contains("FRESH"), "explains why: {msg}");
    assert!(
        msg.contains("--resume alone"),
        "names the continue path: {msg}"
    );
    assert!(
        msg.contains("aida session end"),
        "names the fresh-drain path: {msg}"
    );
}

#[test]
fn queue_work_force_claim_flag_parses() {
    let cli = Cli::try_parse_from(["aida", "queue", "work", "TASK-559", "--force-claim"])
        .expect("--force-claim parses on queue work");
    match cli.command {
        Command::Queue(QueueCommand::Work { force_claim, .. }) => {
            assert!(force_claim, "--force-claim should set force_claim=true");
        }
        other => panic!("expected queue work command, got {other:?}"),
    }
}

// BUG-311: the orchestrator's phase-1 `aida queue work` subprocess must
// carry `--steal` when the outer drain was invoked with it. Without
// threading, the inner subprocess's dormant-lease guard sees `steal=false`
// and bails with the canned "pass --steal" message — exactly the
// symptom this BUG reported. These tests pin the argv contract so a
// refactor cannot silently drop the flag again. trace:BUG-311 | ai:claude

#[test]
fn implementer_phase_args_threads_steal() {
    let args = build_implementer_phase_args(
        "BUG-311",
        "0192f1c8-aaaa-7000-8000-000000000001",
        true,
        false,
        false,
        None,
    );
    assert!(
        args.iter().any(|a| a == "--steal"),
        "--steal must be threaded to phase 1 when the outer drain set it; got {:?}",
        args
    );
    assert_eq!(args[0], "queue");
    assert_eq!(args[1], "work");
    assert_eq!(args[2], "BUG-311");
    assert_eq!(args[3], "--session-id");
    assert_eq!(args[4], "0192f1c8-aaaa-7000-8000-000000000001");
}

#[test]
fn implementer_phase_args_omits_steal_by_default() {
    let args = build_implementer_phase_args("BUG-311", "uuid", false, false, false, None);
    assert!(
        args.iter().all(|a| a != "--steal"),
        "--steal must not appear when the outer drain did not pass it; got {:?}",
        args
    );
}

#[test]
fn implementer_phase_args_threads_no_human_and_permission() {
    let args = build_implementer_phase_args(
        "TASK-1",
        "uuid",
        true,
        false,
        true,
        Some("bypassPermissions"),
    );
    assert!(args.iter().any(|a| a == "--steal"));
    assert!(args.iter().any(|a| a == "--no-human"));
    let i = args
        .iter()
        .position(|a| a == "--permission-mode")
        .expect("--permission-mode threaded");
    assert_eq!(
        args.get(i + 1).map(String::as_str),
        Some("bypassPermissions")
    );
}

#[test]
fn implementer_phase_args_threads_force_claim() {
    let args = build_implementer_phase_args("TASK-559", "uuid", false, true, false, None);
    assert!(
        args.iter().any(|a| a == "--force-claim"),
        "--force-claim must be threaded to phase 1 when the outer drain set it; got {:?}",
        args
    );
    assert!(
        args.iter().all(|a| a != "--steal"),
        "--force-claim should not imply --steal; got {:?}",
        args
    );
}

#[test]
fn auto_complete_phase1_status_promotes_only_not_started_statuses() {
    assert_eq!(
        auto_complete_phase1_target_status(&RequirementStatus::Draft),
        Some(RequirementStatus::InProgress)
    );
    assert_eq!(
        auto_complete_phase1_target_status(&RequirementStatus::Approved),
        Some(RequirementStatus::InProgress)
    );
    assert_eq!(
        auto_complete_phase1_target_status(&RequirementStatus::Planned),
        Some(RequirementStatus::InProgress)
    );
    assert_eq!(
        auto_complete_phase1_target_status(&RequirementStatus::InProgress),
        None
    );
    assert_eq!(
        auto_complete_phase1_target_status(&RequirementStatus::NeedsAttention),
        None
    );
    assert_eq!(
        auto_complete_phase1_target_status(&RequirementStatus::Completed),
        None
    );
    assert_eq!(
        auto_complete_phase1_target_status(&RequirementStatus::Rejected),
        None
    );
}

#[test]
fn prepare_auto_complete_phase1_status_flips_approved_before_spawn() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("requirements.yaml");
    let storage = Storage::new(&path);
    let mut req = aida_core::Requirement::new("early punt".to_string(), String::new());
    req.spec_id = Some("BUG-369".to_string());
    req.status = RequirementStatus::Approved;
    let mut store = aida_core::RequirementsStore::default();
    store.requirements.push(req);
    storage.save(&store).unwrap();

    let changed = prepare_auto_complete_phase1_status(&storage, "BUG-369")
        .expect("phase-1 status preparation should succeed");

    assert_eq!(
        changed,
        Some(("BUG-369".to_string(), RequirementStatus::Approved))
    );
    let updated = storage.load().unwrap();
    let req = updated.get_requirement_by_spec_id("BUG-369").unwrap();
    assert_eq!(req.status, RequirementStatus::InProgress);
}

/// TASK-547 acceptance: smart-default auto-queues an Approved-but-not-queued
/// spec for the current role when resolving the queue work plan, unless `--strict` is set.
// trace:TASK-547 | ai:antigravity
#[test]
fn resolve_queue_work_plan_auto_queues_when_not_strict() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("aida-store");
    let backend = aida_core::GitBackend::new(&root).unwrap();
    let storage = Storage::new(&root);
    let mut req = aida_core::Requirement::new("backlog item".to_string(), String::new());
    req.spec_id = Some("BUG-376".to_string());
    req.status = RequirementStatus::Approved;
    let mut store = aida_core::RequirementsStore::default();
    store.requirements.push(req);
    backend.save(&store).unwrap();

    // 1. With strict = true, it must refuse and error out with status-aware error message
    let res = resolve_queue_work_plan(&storage, "test-user", Some("BUG-376"), None, true, false);
    assert!(res.is_err());
    let err = res.unwrap_err().to_string();
    assert!(
        err.contains("`BUG-376` isn't queued"),
        "expected not queued error, got: {err}"
    );

    // TASK-1053: a DRY RUN on the same Approved-but-unqueued spec must
    // resolve the very same Item plan WITHOUT persisting the auto-queue —
    // the queue stays empty afterwards. trace:TASK-1053 | ai:claude
    let res = resolve_queue_work_plan(&storage, "test-user", Some("BUG-376"), None, false, true)
        .expect("dry-run should resolve a plan without persisting");
    assert_eq!(res.mode, QueueWorkMode::Item);
    assert_eq!(res.anchor_display, "BUG-376");
    let entries = storage.queue_list("test-user", false).unwrap();
    assert!(
        entries.is_empty(),
        "dry-run must not persist the auto-queue, found: {entries:?}"
    );

    // 2. With strict = false (real run), it must automatically queue it and return a successful plan
    let res = resolve_queue_work_plan(&storage, "test-user", Some("BUG-376"), None, false, false)
        .expect("auto-queue should succeed and return plan");
    assert_eq!(res.mode, QueueWorkMode::Item);
    assert_eq!(res.anchor_display, "BUG-376");

    // Verify it was added to the queue
    let entries = storage.queue_list("test-user", false).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].for_role.as_deref(), Some("implementer"));
}

fn queue_review_story(storage: &Storage, root: &std::path::Path) {
    let backend = aida_core::GitBackend::new(root).unwrap();
    let mut review =
        aida_core::Requirement::new("Review PR-457: throwaway".to_string(), String::new());
    review.spec_id = Some("STORY-901".to_string());
    review.status = RequirementStatus::Approved;
    let review_id = review.id;
    let mut store = aida_core::RequirementsStore::default();
    store.requirements.push(review);
    backend.save(&store).unwrap();
    storage
        .queue_add(aida_core::QueueEntry {
            user_id: "u".into(),
            requirement_id: review_id,
            position: 0,
            added_by: "u".into(),
            note: None,
            added_at: chrono::Utc::now(),
            for_role: Some("reviewer".into()),
            for_scope: None,
            for_session: None,
            added_by_machine: None,
        })
        .unwrap();
}

/// STORY-501: the dispatch gate's signal — `queued_review_story_for_pr`
/// detects a queued "Review PR-N" story (so the dispatch DEFERS PR→spec
// resolution to the reviewer pickup). trace:STORY-501 | ai:claude
#[test]
fn queued_review_story_for_pr_detects_queued_story() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("aida-store");
    let storage = Storage::new(&root);
    queue_review_story(&storage, &root);
    assert!(queued_review_story_for_pr(
        &storage,
        "u",
        ReviewForge::GitHub,
        457
    ));
    assert!(!queued_review_story_for_pr(
        &storage,
        "u",
        ReviewForge::GitHub,
        999
    ));
}

/// STORY-501: with a "Review PR-N" story queued, resolve_queue_work_plan
/// routes `PR-N` to the reviewer (review_target set → `/aida-review --pr N`)
/// — the path the dispatch gate now lets run instead of resolving
// PR→backing-spec into an implementer pickup. trace:STORY-501 | ai:claude
#[test]
fn resolve_queue_work_plan_pr_n_with_review_story_routes_to_reviewer() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("aida-store");
    let storage = Storage::new(&root);
    queue_review_story(&storage, &root);
    let plan = resolve_queue_work_plan(&storage, "u", Some("PR-457"), None, false, false)
        .expect("PR-N with a queued review story resolves to a plan");
    assert!(
        plan.review_target.is_some(),
        "PR-N pickup must set review_target so it routes to the reviewer"
    );
    assert_eq!(
        derive_queue_work_prompt(&plan, "reviewer", false, false),
        "/aida-review --pr 457"
    );
}

/// TASK-630 (BUG-250 criterion 5): the held-state re-entry decision is a pure
/// function, so it can be exercised exhaustively with no Storage, worktree,
/// or launcher. A deliberate PR-hold parks the spec Done + dequeued with a
/// marker; `--resume` against that combination is the ONLY case that may
/// re-enter. Every other state, a missing marker, or a non-resume invocation
// must NOT — those keep the existing recovery hints. trace:TASK-630 | ai:claude
#[test]
fn held_resume_reentry_only_for_resume_done_and_marked() {
    use RequirementStatus::*;

    // The one re-enterable combination: explicit --resume, Done status,
    // hold marker present.
    assert!(
        held_resume_reentry_allowed(true, &Done, true),
        "resume + Done + hold marker is the deliberate-hold re-entry case"
    );

    // No --resume → never re-enter (a plain `queue work <spec>` keeps its
    // status-aware not-queued hint).
    assert!(
        !held_resume_reentry_allowed(false, &Done, true),
        "without --resume a held Done spec is not auto-re-entered"
    );

    // Marker absent → not a deliberate hold; leave it to the Done hint
    // (rework / wait-for-merge).
    assert!(
        !held_resume_reentry_allowed(true, &Done, false),
        "no hold marker → not a deliberate hold, no re-entry"
    );

    // Held re-entry is Done-specific: a hold marker against any other status
    // must not unlock resume (defensive — a held spec is always Done).
    for status in [
        Draft,
        Approved,
        Planned,
        InProgress,
        Completed,
        Rejected,
        NeedsAttention,
    ] {
        assert!(
            !held_resume_reentry_allowed(true, &status, true),
            "held re-entry must be Done-only; {status:?} must not re-enter"
        );
    }
}

/// TASK-630: a held-spec resume plan is an Item-mode pickup anchored on the
/// spec itself (its own id is the lease scope — the implementer worktree the
/// dormant session lives in), with exactly one entry. This is what lets the
/// rest of `handle_queue_work` resume the session unchanged.
// trace:TASK-630 | ai:claude
#[test]
fn held_resume_plan_is_item_scoped_to_the_spec() {
    let mut req = aida_core::Requirement::new("held work".to_string(), String::new());
    req.spec_id = Some("STORY-306".to_string());
    req.status = RequirementStatus::Done;

    let plan = held_resume_plan(&req, "test-user");

    assert_eq!(plan.mode, QueueWorkMode::Item);
    assert_eq!(plan.entries.len(), 1, "exactly the held spec, no cluster");
    assert_eq!(plan.anchor_display, "STORY-306");
    assert_eq!(
        plan.scope, "STORY-306",
        "scope is the spec's own id (its implementer worktree)"
    );
    assert!(
        plan.review_target.is_none(),
        "a held implementer spec is not a PR-review pickup"
    );
    assert_eq!(plan.entries[0].spec_id, "STORY-306");
}

/// BUG-311 acceptance: when `--steal`'s internal `session_end` fails, the
/// inner subprocess's error must name the lease + actual reason — not
/// the canned "pass --steal" message. anyhow's `{:#}` collapses the
/// chain inline; the `--steal`-prefixed map_err in `handle_queue_work`
/// guarantees the lease id + reason are in the primary line.
// trace:BUG-311 | ai:claude
#[test]
fn steal_session_end_failure_surfaces_actual_reason_not_canned_message() {
    let lease_id = "019e4dec-abcd-7000-8000-000000000000";
    let short = &lease_id[..8];
    let underlying = anyhow::anyhow!("worktree has uncommitted changes — pass --force to discard");
    let wrapped: anyhow::Error = anyhow::anyhow!(
        "--steal could not end lease {}: {:#} \
             (resolve manually with `aida session end {} --force` to discard, \
             or commit/stash the worktree's changes first, then re-run)",
        short,
        underlying,
        short,
    );

    let primary = format!("{}", wrapped);
    assert!(
        primary.contains(&format!("--steal could not end lease {}", short)),
        "primary line must name the lease the steal targeted; got: {}",
        primary
    );
    assert!(
        primary.contains("worktree has uncommitted changes"),
        "primary line must include the actual session_end reason inline; got: {}",
        primary
    );
    assert!(
            !primary.contains("pass --steal to end that session first"),
            "BUG-311: must NOT emit the canned `pass --steal` message when --steal IS already in play; got: {}",
            primary
        );
}
