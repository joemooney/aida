//! TASK-218: pin the smart status-transition table for `aida queue
//! rework`. The handler itself does I/O against the store, so we
//! test the pure decision function exhaustively here and rely on
//! integration-test smoke (built-binary + temp store) for the
//! side-effecting glue.
//! trace:TASK-218 | ai:claude
use super::*;

fn queue_entry_for_test(
    user_id: &str,
    requirement_id: uuid::Uuid,
    position: i64,
    for_role: Option<&str>,
) -> aida_core::QueueEntry {
    aida_core::QueueEntry {
        user_id: user_id.to_string(),
        requirement_id,
        position,
        added_by: user_id.to_string(),
        note: None,
        added_at: chrono::Utc::now(),
        for_role: for_role.map(|role| role.to_string()),
        for_scope: None,
        for_session: None,
        added_by_machine: None,
    }
}

fn req_for_test(spec_id: &str, status: RequirementStatus) -> aida_core::Requirement {
    let mut req = aida_core::Requirement::new(spec_id.to_string(), String::new());
    req.spec_id = Some(spec_id.to_string());
    req.status = status;
    req
}

/// Approved → no flip. The spec is ready to be queued as-is, so
/// rework just queues it.
#[test]
fn approved_does_not_flip() {
    assert_eq!(rework_smart_target(&RequirementStatus::Approved), None);
}

/// Planned → InProgress. Rework on a Planned spec means "start
/// working it now," so the queue add is paired with the status flip.
#[test]
fn planned_flips_to_in_progress() {
    assert_eq!(
        rework_smart_target(&RequirementStatus::Planned),
        Some(RequirementStatus::InProgress)
    );
}

/// InProgress → no flip. Already at the right status; caller surfaces
/// the "already in progress" warning and re-queues without --force.
#[test]
fn in_progress_does_not_flip() {
    assert_eq!(rework_smart_target(&RequirementStatus::InProgress), None);
}

/// NeedsAttention → InProgress. A findings-led rework is the triage decision:
/// the item must leave the parked state before unified pickability filters run.
// trace:BUG-1056 | ai:codex
#[test]
fn needs_attention_flips_to_in_progress() {
    assert_eq!(
        rework_smart_target(&RequirementStatus::NeedsAttention),
        Some(RequirementStatus::InProgress)
    );
}

/// Done → InProgress. The canonical PR-review-found-issues case —
/// implementer marked it done on a branch, reviewer sent it back.
#[test]
fn done_flips_to_in_progress() {
    assert_eq!(
        rework_smart_target(&RequirementStatus::Done),
        Some(RequirementStatus::InProgress)
    );
}

/// Completed → InProgress (with --force at the caller). The handler
/// itself adds the --force guard; the smart table just records the
/// target.
#[test]
fn completed_flips_to_in_progress() {
    assert_eq!(
        rework_smart_target(&RequirementStatus::Completed),
        Some(RequirementStatus::InProgress)
    );
}

/// Rejected → Approved (with --force). The spec is being reconsidered,
/// not re-implemented yet — Approved is the natural landing.
#[test]
fn rejected_flips_to_approved() {
    assert_eq!(
        rework_smart_target(&RequirementStatus::Rejected),
        Some(RequirementStatus::Approved)
    );
}

/// Draft → no flip. Rework on a Draft is unusual; preserve the
/// status and let the queue add proceed.
#[test]
fn draft_does_not_flip() {
    assert_eq!(rework_smart_target(&RequirementStatus::Draft), None);
}

/// Sanity check: smart_target is idempotent on its own output. After
/// flipping (e.g. Done → InProgress) re-running smart_target on
/// InProgress is a no-op, so chained reworks don't oscillate.
#[test]
fn smart_target_is_idempotent_on_its_own_output() {
    let after_done = rework_smart_target(&RequirementStatus::Done).unwrap();
    assert_eq!(after_done, RequirementStatus::InProgress);
    assert_eq!(rework_smart_target(&after_done), None);

    let after_rejected = rework_smart_target(&RequirementStatus::Rejected).unwrap();
    assert_eq!(after_rejected, RequirementStatus::Approved);
    assert_eq!(rework_smart_target(&after_rejected), None);
}

/// BUG-1470: the metadata-only form must not create an unleased InProgress
/// spec that the later drain refuses to claim.
// trace:BUG-1470 | ai:codex
#[test]
fn metadata_only_rework_targets_claimable_approved() {
    for status in [
        RequirementStatus::Draft,
        RequirementStatus::Planned,
        RequirementStatus::NeedsAttention,
        RequirementStatus::Done,
        RequirementStatus::Completed,
    ] {
        assert_eq!(
            rework_target_for_mode(&status, false),
            Some(RequirementStatus::Approved)
        );
    }
    assert_eq!(
        rework_target_for_mode(&RequirementStatus::InProgress, false),
        Some(RequirementStatus::Approved)
    );
    for status in [
        RequirementStatus::Planned,
        RequirementStatus::NeedsAttention,
        RequirementStatus::Done,
        RequirementStatus::Completed,
    ] {
        assert_eq!(
            rework_target_for_mode(&status, true),
            Some(RequirementStatus::InProgress)
        );
    }

    // F3: THE PASS-THROUGH ARM. Everything above is the constant half of the
    // function — replace the whole `!launches_work` block with
    // `Some(Approved)` and every assertion above still passes, so none of them
    // can tell this function from a constant. These three are the only inputs
    // where the metadata-only branch defers to `rework_smart_target` instead,
    // and they are what makes the claim "keeps the spec claimable" different
    // from "promotes everything".
    // trace:BUG-1470 | ai:claude
    assert_eq!(
        rework_target_for_mode(&RequirementStatus::Approved, false),
        None,
        "an already-claimable spec must not be flipped at all"
    );
    assert_eq!(
        rework_target_for_mode(&RequirementStatus::Superseded, false),
        None,
        "a superseded spec was handed to a successor; reworking this record \
         must not guess a status for it"
    );
    assert_eq!(
        rework_target_for_mode(&RequirementStatus::Rejected, false),
        Some(RequirementStatus::Approved),
        "rejected reworks to Approved through the pass-through arm, not the \
         metadata-only constant — the two agree here, and the assertion pins \
         WHICH path produced it via the two cases above"
    );

    // And the same three under --work, where no metadata-only mapping applies.
    assert_eq!(
        rework_target_for_mode(&RequirementStatus::Approved, true),
        None
    );
    assert_eq!(
        rework_target_for_mode(&RequirementStatus::Superseded, true),
        None
    );
    assert_eq!(
        rework_target_for_mode(&RequirementStatus::Rejected, true),
        Some(RequirementStatus::Approved)
    );
    assert_eq!(
        rework_target_for_mode(&RequirementStatus::Draft, true),
        None,
        "with --work, Draft defers to the smart target, which declines to guess"
    );
    assert_eq!(
        rework_target_for_mode(&RequirementStatus::InProgress, true),
        None,
        "a spec already In Progress with --work needs no flip"
    );
}

/// All status variants are covered — exhaustive match in
/// `rework_smart_target` means adding a new variant won't silently
/// fall through. This test exists so a future variant addition (e.g.
/// "Blocked") trips the compiler check, not a silent None default.
#[test]
fn covers_every_status_variant() {
    use RequirementStatus::*;
    for s in &[
        Draft,
        Approved,
        Planned,
        InProgress,
        NeedsAttention,
        Done,
        Completed,
        Rejected,
        Superseded,
    ] {
        // Just confirm the function doesn't panic on any variant.
        let _ = rework_smart_target(s);
    }
}

/// BUG-814: rework must persist the blocking review findings onto the spec,
/// not merely requeue it. That durable comment is what prevents the next
/// implementer from seeing only already-satisfied acceptance and producing a
/// no-change loop.
// trace:BUG-814 | ai:codex
#[test]
fn rework_writes_blocking_review_findings_comment() {
    let _guard = crate::test_env::env_lock();
    let prev_cwd = std::env::current_dir().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir(root.join(".git")).unwrap();
    std::fs::create_dir_all(root.join(".aida").join("review-verdicts")).unwrap();
    std::fs::write(
        root.join(".aida")
            .join("review-verdicts")
            .join("PR-1637.json"),
        r#"{
            "verdict":"RequestChanges",
            "summary":"BUG-814 hides the review findings",
            "findings":["BUG-814 pickup prompt omits the RequestChanges detail"]
        }"#,
    )
    .unwrap();

    let store_root = root.join(".aida-store");
    let backend = aida_core::GitBackend::new(&store_root).unwrap();
    let storage = Storage::new(&store_root);
    let mut req = aida_core::Requirement::new("queue rework loop".to_string(), String::new());
    req.spec_id = Some("BUG-814".to_string());
    req.status = RequirementStatus::Done;
    let mut store = aida_core::RequirementsStore::default();
    store.requirements.push(req);
    backend.save(&store).unwrap();

    std::env::set_current_dir(root).unwrap();
    let result = handle_queue_rework(
        &storage,
        "BUG-814",
        false,
        Some("implementer"),
        false,
        None,
        None,
        false,
        false,
        false,
        None,
        true,
        Some("codex"),
    );
    std::env::set_current_dir(prev_cwd).unwrap();
    result.unwrap();

    let updated = storage.load().unwrap();
    let req = updated.get_requirement_by_spec_id("BUG-814").unwrap();
    let comment = req
        .comments
        .iter()
        .find(|c| c.content.contains("REVIEW FINDINGS TO ADDRESS (PR #1637)"))
        .expect("rework should persist review findings");
    assert!(comment
        .content
        .contains("1. BUG-814 pickup prompt omits the RequestChanges detail"));
}

/// BUG-851: reviewer-driven rework must stay routed to the role that owned the
/// original queue entry, and it should jump to the queue head by default.
// trace:BUG-851 | ai:codex
#[test]
fn rework_inherits_existing_route_and_requeues_at_head() {
    let _guard = crate::test_env::env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let store_root = tmp.path().join(".aida-store");
    // BUG-1598: anchor cache-path resolution to this tempdir so
    // `Storage::resolve_queued_requirement`'s `default_cache_path` walk-up
    // stops here instead of continuing into the shared system temp dir.
    // trace:BUG-1598 | ai:claude
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    let backend = aida_core::GitBackend::new(&store_root).unwrap();
    let storage = Storage::new(&store_root);

    let rework_req = req_for_test("BUG-851", RequirementStatus::Done);
    let other_req = req_for_test("TASK-852", RequirementStatus::Approved);
    let rework_id = rework_req.id;
    let other_id = other_req.id;
    let mut store = aida_core::RequirementsStore::default();
    store.requirements.push(rework_req);
    store.requirements.push(other_req);
    backend.save(&store).unwrap();
    storage
        .queue_add(queue_entry_for_test(
            "codex",
            other_id,
            1000,
            Some("implementer"),
        ))
        .unwrap();
    storage
        .queue_add(queue_entry_for_test(
            "codex",
            rework_id,
            2000,
            Some("implementer"),
        ))
        .unwrap();

    handle_queue_rework(
        &storage,
        "BUG-851",
        false,
        None,
        false,
        None,
        None,
        false,
        false,
        false,
        None,
        true,
        Some("codex"),
    )
    .unwrap();

    let entries = storage.queue_list("codex", true).unwrap();
    assert_eq!(entries[0].requirement_id, rework_id);
    assert_eq!(entries[0].for_role.as_deref(), Some("implementer"));
    assert_eq!(entries[0].position, 0);
    assert_eq!(entries[1].requirement_id, other_id);
}

/// BUG-851: an explicit --for remains authoritative over inherited routing.
// trace:BUG-851 | ai:codex
#[test]
fn rework_for_override_wins_over_existing_route() {
    let _guard = crate::test_env::env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let store_root = tmp.path().join(".aida-store");
    // BUG-1598: anchor cache-path resolution to this tempdir so
    // `Storage::resolve_queued_requirement`'s `default_cache_path` walk-up
    // stops here instead of continuing into the shared system temp dir.
    // trace:BUG-1598 | ai:claude
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    let backend = aida_core::GitBackend::new(&store_root).unwrap();
    let storage = Storage::new(&store_root);

    let req = req_for_test("BUG-851", RequirementStatus::Done);
    let req_id = req.id;
    let mut store = aida_core::RequirementsStore::default();
    store.requirements.push(req);
    backend.save(&store).unwrap();
    storage
        .queue_add(queue_entry_for_test(
            "codex",
            req_id,
            1000,
            Some("implementer"),
        ))
        .unwrap();

    handle_queue_rework(
        &storage,
        "BUG-851",
        false,
        Some("reviewer"),
        false,
        None,
        None,
        false,
        false,
        false,
        None,
        true,
        Some("codex"),
    )
    .unwrap();

    let entries = storage.queue_list("codex", true).unwrap();
    assert_eq!(entries[0].for_role.as_deref(), Some("reviewer"));
}

/// BUG-851: --tail opts out of the urgent-head default.
// trace:BUG-851 | ai:codex
#[test]
fn rework_tail_keeps_append_semantics() {
    let _guard = crate::test_env::env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let store_root = tmp.path().join(".aida-store");
    // BUG-1598: anchor cache-path resolution to this tempdir so
    // `Storage::resolve_queued_requirement`'s `default_cache_path` walk-up
    // stops here instead of continuing into the shared system temp dir.
    // trace:BUG-1598 | ai:claude
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    let backend = aida_core::GitBackend::new(&store_root).unwrap();
    let storage = Storage::new(&store_root);

    let rework_req = req_for_test("BUG-851", RequirementStatus::Done);
    let other_req = req_for_test("TASK-852", RequirementStatus::Approved);
    let rework_id = rework_req.id;
    let other_id = other_req.id;
    let mut store = aida_core::RequirementsStore::default();
    store.requirements.push(rework_req);
    store.requirements.push(other_req);
    backend.save(&store).unwrap();
    storage
        .queue_add(queue_entry_for_test("codex", rework_id, 1000, Some("")))
        .unwrap();
    storage
        .queue_add(queue_entry_for_test(
            "codex",
            other_id,
            2000,
            Some("implementer"),
        ))
        .unwrap();

    handle_queue_rework(
        &storage,
        "BUG-851",
        false,
        None,
        true,
        None,
        None,
        false,
        false,
        false,
        None,
        true,
        Some("codex"),
    )
    .unwrap();

    let entries = storage.queue_list("codex", true).unwrap();
    assert_eq!(entries[0].requirement_id, other_id);
    assert_eq!(entries[1].requirement_id, rework_id);
    assert_eq!(entries[1].position, 3000);
    assert_eq!(entries[1].for_role.as_deref(), Some("implementer"));
}

/// BUG-1056: a parked findings-led rework must not stay NeedsAttention, because
/// the unified pickability policy refuses NeedsAttention specs at queue head.
// trace:BUG-1056 | ai:codex
#[test]
fn metadata_rework_needs_attention_spec_becomes_pickable_queue_head() {
    // BUG-1470 / STORY-1353: lifting a punted NeedsAttention spec into
    // Approved is a DISPOSITION, so dispatch authority alone must not do it.
    // This test pins both directions of that gate: refused without advisor
    // authority, and successful (BUG-1056's original pickability behaviour)
    // with it. NOTE: no outer `env_lock()` here — `EnvVarGuard` holds
    // ENV_LOCK for its own lifetime and the lock is NOT reentrant, so
    // acquiring it above a guard deadlocks. Each phase below scopes its own
    // guard instead. trace:BUG-1470 trace:STORY-1353 | ai:claude
    let tmp = tempfile::tempdir().unwrap();
    let store_root = tmp.path().join(".aida-store");
    // BUG-1598: anchor cache-path resolution to this tempdir so
    // `Storage::resolve_queued_requirement`'s `default_cache_path` walk-up
    // stops here instead of continuing into the shared system temp dir.
    // trace:BUG-1598 | ai:claude
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    let backend = aida_core::GitBackend::new(&store_root).unwrap();
    let storage = Storage::new(&store_root);

    let rework_req = req_for_test("BUG-1056", RequirementStatus::NeedsAttention);
    let rework_id = rework_req.id;
    let mut store = aida_core::RequirementsStore::default();
    store.requirements.push(rework_req);
    backend.save(&store).unwrap();

    let rework = |storage: &Storage| {
        handle_queue_rework(
            storage,
            "BUG-1056",
            false,
            Some("implementer"),
            false,
            None,
            Some("review findings are the triage resolution"),
            false,
            false,
            false,
            None,
            true,
            Some("codex"),
        )
    };

    // STORY-1353 half: Needs Attention -> Approved is a DISPOSITION, so
    // dispatch authority alone must not perform it. This is the hole BUG-1494
    // recorded -- queue rework was the way to move a shelved spec without the
    // advisor ever ruling on it. The BUG-1470 F1 gate (queue_cmd.rs, at the
    // status flip) refuses this the SAME way it refuses an un-triaged Draft
    // (see `metadata_rework_of_a_draft_is_refused_without_advisor_authority`):
    // Ok(()), a printed hint, no status write, no queue entry -- NOT an Err.
    // An earlier version of this test asserted an Err here (STORY-1353's own
    // gate, since removed as a duplicate of this one -- see the NOTE at the
    // removed call site in handle_queue_rework); that duplicate silently
    // upgraded the pinned no-op contract to a hard failure.
    {
        let _role = crate::test_env::EnvVarGuard::unset("AIDA_SESSION_ROLE");
        rework(&storage).expect("a refused rework is a no-op, not an error");
        let parked = storage.load().unwrap();
        assert_eq!(
            parked
                .get_requirement_by_spec_id("BUG-1056")
                .unwrap()
                .status,
            RequirementStatus::NeedsAttention,
            "a refused rework must leave the spec parked, not half-moved"
        );
        assert!(
            storage
                .queue_list("codex", true)
                .unwrap_or_default()
                .is_empty(),
            "a refused rework must not leave a queue entry behind"
        );
    }

    // BUG-1056 half, preserved: WITH advisor authority the punted spec resumes
    // and becomes the pickable queue head. STORY-1353 gates this transition; it
    // does not remove it.
    let _role = crate::test_env::EnvVarGuard::set("AIDA_SESSION_ROLE", "advisor");
    rework(&storage).unwrap();

    let updated = storage.load().unwrap();
    let req = updated.get_requirement_by_spec_id("BUG-1056").unwrap();
    assert_eq!(req.status, RequirementStatus::Approved);
    assert_eq!(
        aida_core::pickability::pickability(req, &updated),
        aida_core::pickability::Pickability::Pickable
    );

    let entries = storage.queue_list("codex", true).unwrap();
    assert_eq!(entries[0].requirement_id, rework_id);
    assert_eq!(entries[0].for_role.as_deref(), Some("implementer"));
}

// BUG-1494: the literal incident scenario -- `aida queue rework --work` (or
// `--resume`) on a NeedsAttention spec resolves its smart target to
// InProgress (`rework_smart_target`), not Approved. This is the hop the
// original report walked through before a since-unrelated `edit --status
// approved` (InProgress -> Approved is an intentionally ungated execution
// flip) landed it Approved with no advisor ever ruling on it. The BUG-1470
// gate at the status flip in `handle_queue_rework` is unconditional on
// `work`, so this hop must refuse identically to the metadata-only
// (`work: false`) case pinned above -- but until now nothing drove the
// `work: true` branch through this gate, so a regression here would have
// gone unnoticed. Refusal must happen BEFORE the queue-add/session-launch
// side effects further down `handle_queue_rework`, so this is safe to run
// without spawning a session.
// trace:BUG-1494 | ai:claude
#[test]
fn work_rework_of_needs_attention_spec_is_refused_without_advisor_authority() {
    let tmp = tempfile::tempdir().unwrap();
    let store_root = tmp.path().join(".aida-store");
    // BUG-1598: anchor cache-path resolution to this tempdir so
    // `Storage::resolve_queued_requirement`'s `default_cache_path` walk-up
    // stops here instead of continuing into the shared system temp dir.
    // trace:BUG-1598 | ai:claude
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    let backend = aida_core::GitBackend::new(&store_root).unwrap();
    let storage = Storage::new(&store_root);

    let req = req_for_test("BUG-1494", RequirementStatus::NeedsAttention);
    let mut store = aida_core::RequirementsStore::default();
    store.requirements.push(req);
    backend.save(&store).unwrap();

    // Sanity: confirm this test actually exercises the InProgress-target
    // branch the incident hit, not the Approved-target metadata-only branch
    // pinned by `metadata_rework_needs_attention_spec_becomes_pickable_queue_head`.
    assert_eq!(
        rework_target_for_mode(&RequirementStatus::NeedsAttention, true),
        Some(RequirementStatus::InProgress)
    );

    let _role = crate::test_env::EnvVarGuard::unset("AIDA_SESSION_ROLE");
    handle_queue_rework(
        &storage,
        "BUG-1494",
        true, // work: true — the --work / --resume chain the incident used
        Some("implementer"),
        false,
        None,
        None,
        false,
        false,
        false,
        None,
        true,
        Some("codex"),
    )
    .expect("a refused rework is a no-op, not an error");

    let after = storage.load().unwrap();
    assert_eq!(
        after.get_requirement_by_spec_id("BUG-1494").unwrap().status,
        RequirementStatus::NeedsAttention,
        "a refused rework must leave the spec parked, not laundered into InProgress"
    );
    assert!(
        storage
            .queue_list("codex", true)
            .unwrap_or_default()
            .is_empty(),
        "a refused rework must not leave a queue entry (or launch a session) behind"
    );
}

#[test]
fn queue_destination_contract_names_local_role_queue() {
    // trace:STORY-1002 | ai:codex
    let details = queue_destination_details(
        "STORY-1002",
        "role:implementer",
        "local",
        Some("implementer"),
        true,
    );

    assert_eq!(details.identity, "role:implementer");
    assert_eq!(details.queue, "local");
    assert_eq!(details.routed_role, "implementer");
    assert_eq!(
        details.observe_command,
        "aida queue list --user role:implementer --for implementer"
    );
    assert_eq!(
        details.pickup_command,
        "aida queue work STORY-1002 --user role:implementer --role implementer"
    );
}

#[test]
fn queue_destination_contract_names_removed_entry() {
    // trace:STORY-1002 | ai:codex
    let details = queue_destination_details("TASK-1", "joe", "local", Some("reviewer"), false);

    assert_eq!(details.identity, "joe");
    assert_eq!(details.queue, "local");
    assert_eq!(details.routed_role, "reviewer");
    assert_eq!(
        details.observe_command,
        "aida queue list --user joe --for reviewer"
    );
    assert_eq!(details.pickup_command, "n/a (entry removed from queue)");
}

#[test]
fn queue_destination_contract_names_all_role_destination() {
    // trace:STORY-1002 | ai:codex
    let details = queue_destination_details("TASK-1", "joe", "local", Some("all"), false);

    assert_eq!(details.routed_role, "all");
    assert_eq!(details.observe_command, "aida queue list --user joe --all");
    assert_eq!(details.pickup_command, "n/a (entry removed from queue)");
}

#[test]
fn queue_destination_contract_names_global_role_queue() {
    // trace:STORY-1002 | ai:codex
    let details = queue_destination_details(
        "BUG-915",
        "role:implementer",
        "global-role",
        Some("implementer"),
        true,
    );

    assert_eq!(details.identity, "role:implementer");
    assert_eq!(details.queue, "global-role");
    assert_eq!(details.routed_role, "implementer");
    assert_eq!(
        details.observe_command,
        "aida queue list --global --for implementer"
    );
    assert_eq!(
        details.pickup_command,
        "aida queue work BUG-915 --role implementer"
    );
}

#[test]
fn queue_destination_contract_renders_human_block() {
    // trace:STORY-1002 | ai:codex
    let details = queue_destination_details(
        "STORY-1002",
        "role:implementer",
        "local",
        Some("implementer"),
        true,
    );
    let rendered = queue_mutation_destination_human("Added STORY-1002", &details);

    assert!(rendered.contains("Destination:"));
    assert!(rendered.contains("identity: role:implementer"));
    assert!(rendered.contains("queue: local"));
    assert!(rendered.contains("routed role: implementer"));
    assert!(rendered.contains("observe: aida queue list --user role:implementer --for implementer"));
    assert!(rendered
        .contains("pickup: aida queue work STORY-1002 --user role:implementer --role implementer"));
}

#[test]
fn queue_destination_contract_renders_json_fields() {
    // trace:STORY-1002 | ai:codex
    let details = queue_destination_details("STORY-1002", "joe", "local", Some("reviewer"), true);
    let rendered =
        queue_mutation_destination_json("move", "STORY-1002", Some("Queue output"), &details);

    assert_eq!(rendered["action"], "move");
    assert_eq!(rendered["spec_id"], "STORY-1002");
    assert_eq!(rendered["title"], "Queue output");
    assert_eq!(rendered["destination"]["identity"], "joe");
    assert_eq!(rendered["destination"]["queue"], "local");
    assert_eq!(rendered["destination"]["routed_role"], "reviewer");
    assert_eq!(
        rendered["destination"]["observe_command"],
        "aida queue list --user joe --for reviewer"
    );
    assert_eq!(
        rendered["destination"]["pickup_command"],
        "aida queue work STORY-1002 --user joe --role reviewer"
    );
}

/// BUG-1277: `aida queue rework SPEC --for implementer` (no explicit
/// `--user`) must land the requeued entry on the role's shared queue — the
/// exact identity a batch drain reads (`--user role:implementer`) — not the
/// invoking user's personal queue. Assert visibility via that SAME read
/// path, not merely that the command exits zero: a zero exit is what the
/// pre-fix bug produced too, while still landing the entry somewhere a
/// batch drain never looks.
// trace:BUG-1277 | ai:claude
#[test]
fn rework_for_implementer_lands_on_role_queue_visible_to_batch_drain() {
    let _guard = crate::test_env::env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let store_root = tmp.path().join(".aida-store");
    // BUG-1598: anchor cache-path resolution to this tempdir so
    // `Storage::resolve_queued_requirement`'s `default_cache_path` walk-up
    // stops here instead of continuing into the shared system temp dir.
    // trace:BUG-1598 | ai:claude
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    let backend = aida_core::GitBackend::new(&store_root).unwrap();
    let storage = Storage::new(&store_root);

    let req = req_for_test("BUG-1277", RequirementStatus::Done);
    let req_id = req.id;
    let mut store = aida_core::RequirementsStore::default();
    store.requirements.push(req);
    backend.save(&store).unwrap();

    handle_queue_rework(
        &storage,
        "BUG-1277",
        false,
        Some("implementer"),
        false,
        None,
        None,
        false,
        false,
        false,
        None,
        true,
        // No explicit --user: this is the default path a reviewer-findings
        // rework actually takes.
        None,
    )
    .unwrap();

    // This is exactly the read a batch drain performs to pick work up.
    let role_queue = storage.queue_list("role:implementer", true).unwrap();
    assert_eq!(
        role_queue
            .iter()
            .filter(|e| e.requirement_id == req_id)
            .count(),
        1,
        "rework --for implementer should leave exactly one entry on the \
         role:implementer queue"
    );
    assert_eq!(role_queue[0].for_role.as_deref(), Some("implementer"));
}

/// BUG-1277: an entry that already exists under a different (e.g. the
/// invoking human's own) identity must be MOVED to the role queue, not
/// duplicated. Forgetting the removal half of the old three-command dance
/// is exactly the failure mode the bug report named.
// trace:BUG-1277 | ai:claude
#[test]
fn rework_moves_existing_entry_from_other_user_instead_of_duplicating() {
    let _guard = crate::test_env::env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let store_root = tmp.path().join(".aida-store");
    // BUG-1598: anchor cache-path resolution to this tempdir so
    // `Storage::resolve_queued_requirement`'s `default_cache_path` walk-up
    // stops here instead of continuing into the shared system temp dir.
    // trace:BUG-1598 | ai:claude
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    let backend = aida_core::GitBackend::new(&store_root).unwrap();
    let storage = Storage::new(&store_root);

    let req = req_for_test("BUG-1277", RequirementStatus::Done);
    let req_id = req.id;
    let mut store = aida_core::RequirementsStore::default();
    store.requirements.push(req);
    backend.save(&store).unwrap();

    // Pre-existing entry under the invoking (human/advisor) identity —
    // the state a prior `queue add`/`rework` left it in.
    storage
        .queue_add(queue_entry_for_test(
            "claude-product-1",
            req_id,
            1000,
            Some("implementer"),
        ))
        .unwrap();

    handle_queue_rework(
        &storage,
        "BUG-1277",
        false,
        Some("implementer"),
        false,
        None,
        None,
        false,
        false,
        false,
        None,
        true,
        None,
    )
    .unwrap();

    let old_identity_queue = storage.queue_list("claude-product-1", true).unwrap();
    assert!(
        !old_identity_queue
            .iter()
            .any(|e| e.requirement_id == req_id),
        "the stale entry under the old identity must be removed, not left \
         behind as a duplicate"
    );
    let role_queue = storage.queue_list("role:implementer", true).unwrap();
    assert_eq!(
        role_queue
            .iter()
            .filter(|e| e.requirement_id == req_id)
            .count(),
        1
    );
}

/// BUG-1427: the move guarantee covers every queue identity, not only the
/// first match returned by the backend. A rework must sweep all stale copies
/// before leaving the single destination entry.
// trace:BUG-1427 | ai:codex
#[test]
fn rework_moves_existing_entries_from_every_other_identity() {
    let _guard = crate::test_env::env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let store_root = tmp.path().join(".aida-store");
    // BUG-1598: anchor cache-path resolution to this tempdir so
    // `Storage::resolve_queued_requirement`'s `default_cache_path` walk-up
    // stops here instead of continuing into the shared system temp dir.
    // trace:BUG-1598 | ai:claude
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    let backend = aida_core::GitBackend::new(&store_root).unwrap();
    let storage = Storage::new(&store_root);

    let req = req_for_test("BUG-1427", RequirementStatus::Done);
    let req_id = req.id;
    let mut store = aida_core::RequirementsStore::default();
    store.requirements.push(req);
    backend.save(&store).unwrap();

    for identity in ["advisor-one", "advisor-two"] {
        storage
            .queue_add(queue_entry_for_test(
                identity,
                req_id,
                1000,
                Some("implementer"),
            ))
            .unwrap();
    }

    handle_queue_rework(
        &storage,
        "BUG-1427",
        false,
        Some("implementer"),
        false,
        None,
        None,
        false,
        false,
        false,
        None,
        true,
        None,
    )
    .unwrap();

    for identity in ["advisor-one", "advisor-two"] {
        assert!(
            storage
                .queue_list(identity, true)
                .unwrap()
                .iter()
                .all(|entry| entry.requirement_id != req_id),
            "stale entry under {identity} must be removed"
        );
    }
    let destination = storage.queue_list("role:implementer", true).unwrap();
    assert_eq!(
        destination
            .iter()
            .filter(|entry| entry.requirement_id == req_id)
            .count(),
        1,
        "rework must leave one destination entry after sweeping every source"
    );
}

/// BUG-1277: an explicit `--user` is still honoured verbatim — the fix only
/// changes the DEFAULT destination, never overrides an explicit choice.
// trace:BUG-1277 | ai:claude
#[test]
fn rework_explicit_user_still_overrides_role_default() {
    let _guard = crate::test_env::env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let store_root = tmp.path().join(".aida-store");
    // BUG-1598: anchor cache-path resolution to this tempdir so
    // `Storage::resolve_queued_requirement`'s `default_cache_path` walk-up
    // stops here instead of continuing into the shared system temp dir.
    // trace:BUG-1598 | ai:claude
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    let backend = aida_core::GitBackend::new(&store_root).unwrap();
    let storage = Storage::new(&store_root);

    let req = req_for_test("BUG-1277", RequirementStatus::Done);
    let req_id = req.id;
    let mut store = aida_core::RequirementsStore::default();
    store.requirements.push(req);
    backend.save(&store).unwrap();

    handle_queue_rework(
        &storage,
        "BUG-1277",
        false,
        Some("implementer"),
        false,
        None,
        None,
        false,
        false,
        false,
        None,
        true,
        Some("codex"),
    )
    .unwrap();

    let entries = storage.queue_list("codex", true).unwrap();
    assert_eq!(entries[0].requirement_id, req_id);
    let role_queue = storage.queue_list("role:implementer", true).unwrap();
    assert!(
        !role_queue.iter().any(|e| e.requirement_id == req_id),
        "an explicit --user must not also land on the role queue"
    );
}

/// BUG-1470 F1: `queue rework` reached the Draft -> Approved promotion through
/// a different door than the three sibling sites in the same file, so it
/// applied no advisor-authority gate. The promotion only became reachable when
/// metadata-only rework started mapping Draft to Approved, which is why the
/// gate and that mapping belong in the same change.
///
/// The paired test above proves the act SUCCEEDS with authority; this one
/// proves it is REFUSED without. Either alone is satisfied by a gate that is
/// always open or always shut.
// trace:BUG-1470 | ai:claude
#[test]
fn metadata_rework_of_a_draft_is_refused_without_advisor_authority() {
    let _role = crate::test_env::EnvVarGuard::set("AIDA_SESSION_ROLE", "implementer");
    let tmp = tempfile::tempdir().unwrap();
    let store_root = tmp.path().join(".aida-store");
    // BUG-1598: anchor cache-path resolution to this tempdir so
    // `Storage::resolve_queued_requirement`'s `default_cache_path` walk-up
    // stops here instead of continuing into the shared system temp dir.
    // trace:BUG-1598 | ai:claude
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    let backend = aida_core::GitBackend::new(&store_root).unwrap();
    let storage = Storage::new(&store_root);

    let req = req_for_test("BUG-14700", RequirementStatus::Draft);
    let mut store = aida_core::RequirementsStore::default();
    store.requirements.push(req);
    backend.save(&store).unwrap();

    handle_queue_rework(
        &storage,
        "BUG-14700",
        false,
        Some("implementer"),
        false,
        None,
        None,
        false,
        false,
        false,
        None,
        true,
        Some("codex"),
    )
    .unwrap();

    let updated = storage.load().unwrap();
    let after = updated.get_requirement_by_spec_id("BUG-14700").unwrap();
    assert_eq!(
        after.status,
        RequirementStatus::Draft,
        "an un-triaged draft must not be promoted by a non-advisor session"
    );

    // The refusal must be total, not partial: no queue entry either. A spec
    // left Draft but queued is a worse state than an untouched one, because
    // the queue head then refuses it on every later pickup.
    let entries = storage.queue_list("codex", true).unwrap_or_default();
    assert!(
        entries.is_empty(),
        "a refused rework must not leave a queue entry behind; got {} entries",
        entries.len()
    );
}

/// TASK-1311: a requeue by a NON-TTY advisor (the test process has no
/// terminal) flips the status and clears the shelve metadata, but must KEEP
/// the `needs-human` escalation tag: an escalation to a human is undone only
/// by a human at a terminal. The spec stays parked and the return is recorded.
/// The TTY-human half is the pure `requeue::may_clear_escalation` test.
// trace:TASK-1311 | ai:claude
#[test]
fn requeue_by_non_tty_advisor_keeps_the_escalation_tag() {
    let tmp = tempfile::tempdir().unwrap();
    let store_root = tmp.path().join(".aida-store");
    // BUG-1598: anchor cache-path resolution to this tempdir so
    // `Storage::resolve_queued_requirement`'s `default_cache_path` walk-up
    // stops here instead of continuing into the shared system temp dir.
    // trace:BUG-1598 | ai:claude
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    let backend = aida_core::GitBackend::new(&store_root).unwrap();
    let storage = Storage::new(&store_root);

    let mut req = req_for_test("BUG-13110", RequirementStatus::NeedsAttention);
    req.tags.insert("needs-human".to_string());
    req.tags.insert("batch:keep".to_string());
    req.failure_reason = Some(aida_core::FailureReason {
        phase: "ci".into(),
        phase_index: 2,
        kind: "ci-red".into(),
        detail: "clippy failed".into(),
        recovery_hint: None,
        shelved_by: None,
        shelved_at: chrono::Utc::now(),
    });
    let mut store = aida_core::RequirementsStore::default();
    store.requirements.push(req);
    backend.save(&store).unwrap();

    let _role = crate::test_env::EnvVarGuard::set("AIDA_SESSION_ROLE", "advisor");
    handle_queue_rework(
        &storage,
        "BUG-13110",
        false,
        Some("implementer"),
        false,
        None,
        Some("CI fixed on main"),
        false,
        false,
        false,
        None,
        true,
        Some("codex"),
    )
    .unwrap();

    let updated = storage.load().unwrap();
    let after = updated.get_requirement_by_spec_id("BUG-13110").unwrap();
    assert_eq!(after.status, RequirementStatus::Approved);
    assert!(
        after.tags.contains("needs-human"),
        "a non-TTY advisor must not undo an escalation: {:?}",
        after.tags
    );
    assert!(after.tags.contains("batch:keep"), "{:?}", after.tags);
    assert!(
        after.failure_reason.is_none(),
        "the shelve's FailureReason must not survive the requeue"
    );
    let tags: Vec<String> = after.tags.iter().cloned().collect();
    assert!(
        crate::burndown::parking_tag(&tags).is_some(),
        "still parked for a human"
    );
    assert!(
        after.comments.iter().any(|c| c
            .content
            .contains("Returned from NeedsAttention to Approved via `aida queue rework`")
            && c.content.contains("ci/ci-red: clippy failed")
            && c.content.contains("Kept escalation tag(s) needs-human")
            && c.content.contains("Triage reason: CI fixed on main")),
        "re-entry must record why the spec came back"
    );
    let entries = storage.queue_list("codex", true).unwrap();
    assert_eq!(entries.len(), 1, "requeued onto the queue");
}

// ── STORY-1429: requeue races, lease gate, single reason, triage loop ──
// trace:STORY-1429 | ai:claude

/// A git-canonical store under a temp project root (`<tmp>/.aida-store`), so
/// the leases, drain lock and event log a requeue reads live in `<tmp>/.aida`.
fn story_1429_fixture(reqs: Vec<aida_core::Requirement>) -> (tempfile::TempDir, Storage) {
    let tmp = tempfile::tempdir().unwrap();
    let store_root = tmp.path().join(".aida-store");
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    let backend = aida_core::GitBackend::new(&store_root).unwrap();
    let storage = Storage::new(&store_root);
    let mut store = aida_core::RequirementsStore::default();
    store.requirements = reqs;
    backend.save(&store).unwrap();
    (tmp, storage)
}

fn parked(spec_id: &str) -> aida_core::Requirement {
    let mut r = req_for_test(spec_id, RequirementStatus::NeedsAttention);
    r.failure_reason = Some(aida_core::FailureReason {
        phase: "ci".into(),
        phase_index: 2,
        kind: "ci-red".into(),
        detail: "clippy failed".into(),
        recovery_hint: None,
        shelved_by: None,
        shelved_at: chrono::Utc::now(),
    });
    r
}

fn lease_for(scope: &str, active_pid: Option<u32>) -> crate::SessionLease {
    crate::SessionLease {
        id: format!("lease-{}-0000", scope.to_ascii_lowercase()),
        scope: scope.to_string(),
        slug: scope.to_ascii_lowercase(),
        owner: "tester".into(),
        worktree_path: std::path::PathBuf::from(format!(
            "/nonexistent/aida-{}",
            scope.to_ascii_lowercase()
        )),
        branch: scope.to_ascii_lowercase(),
        started_at: chrono::Utc::now(),
        hostname: "h".into(),
        role: Some("implementer".into()),
        creator_pid: None,
        creator_pid_start_time: None,
        active_pid,
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

fn write_lease(root: &std::path::Path, lease: &crate::SessionLease) {
    let dir = root.join(".aida").join("sessions");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("{}.toml", lease.id)),
        toml::to_string(lease).unwrap(),
    )
    .unwrap();
}

/// A pid above the kernel's pid_max: never alive.
const DEAD_PID: u32 = 99_999_999;

#[allow(clippy::too_many_arguments)]
fn rework_as(storage: &Storage, id: &str, reason: Option<&str>, force: bool) -> anyhow::Result<()> {
    handle_queue_rework(
        storage,
        id,
        false,
        Some("implementer"),
        false,
        None,
        reason,
        false,
        force,
        false,
        None,
        true,
        Some("codex"),
    )
}

fn status_of(storage: &Storage, id: &str) -> RequirementStatus {
    storage
        .load()
        .unwrap()
        .get_requirement_by_spec_id(id)
        .unwrap()
        .status
        .clone()
}

#[test]
fn rework_refuses_while_other_session_lease_is_live() {
    let _env = crate::test_env::EnvVarsGuard::set(&[("AIDA_SESSION_ROLE", "advisor")]);
    let (tmp, storage) = story_1429_fixture(vec![parked("BUG-9101")]);
    // The test process itself is the live holder.
    write_lease(tmp.path(), &lease_for("BUG-9101", Some(std::process::id())));
    for force in [false, true] {
        let err = rework_as(&storage, "BUG-9101", None, force)
            .expect_err("a live claim by another session must refuse");
        let msg = err.to_string();
        assert!(
            msg.contains("aida session end lease-bu") && msg.contains("--force does not"),
            "{msg}"
        );
        assert_eq!(
            status_of(&storage, "BUG-9101"),
            RequirementStatus::NeedsAttention,
            "refused before any write (force={force})"
        );
        assert!(storage
            .queue_list("codex", true)
            .unwrap_or_default()
            .is_empty());
    }
}

#[test]
fn rework_proceeds_over_dead_lease() {
    let _env = crate::test_env::EnvVarsGuard::set(&[("AIDA_SESSION_ROLE", "advisor")]);
    let (tmp, storage) = story_1429_fixture(vec![parked("BUG-9102")]);
    write_lease(tmp.path(), &lease_for("BUG-9102", Some(DEAD_PID)));
    rework_as(&storage, "BUG-9102", None, false).expect("a dead claim does not block");
    assert_eq!(status_of(&storage, "BUG-9102"), RequirementStatus::Approved);
    assert_eq!(storage.queue_list("codex", true).unwrap().len(), 1);
}

#[test]
fn rework_fails_closed_on_unknown_lease_state() {
    let _env = crate::test_env::EnvVarsGuard::set(&[("AIDA_SESSION_ROLE", "advisor")]);
    // (1) A lease that does not parse.
    let (tmp, storage) = story_1429_fixture(vec![parked("BUG-9103")]);
    let dir = tmp.path().join(".aida").join("sessions");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("broken.toml"), "scope = [unterminated").unwrap();
    let err = rework_as(&storage, "BUG-9103", None, true).expect_err("unparseable lease");
    assert!(err.to_string().contains("does not parse"), "{err}");
    assert_eq!(
        status_of(&storage, "BUG-9103"),
        RequirementStatus::NeedsAttention
    );

    // (2) A lease directory that cannot be read.
    let (tmp2, storage2) = story_1429_fixture(vec![parked("BUG-9104")]);
    std::fs::write(tmp2.path().join(".aida").join("sessions"), "not a dir").unwrap();
    let err = rework_as(&storage2, "BUG-9104", None, false).expect_err("unreadable lease dir");
    assert!(err.to_string().contains("unreadable"), "{err}");
    assert_eq!(
        status_of(&storage2, "BUG-9104"),
        RequirementStatus::NeedsAttention
    );

    // (3) A liveness probe that errors, on a lease that claims the spec.
    let check = crate::requeue_lease_gate_from(
        Ok(vec![lease_for("BUG-9105", None)]),
        None,
        &["BUG-9105"],
        |_| Err("probe failed".to_string()),
    );
    assert!(
        matches!(check, crate::RequeueLeaseCheck::Unknown(_)),
        "{check:?}"
    );
    assert!(check.refusal("BUG-9105").unwrap().contains("cannot tell"));
    // A probe error on a lease for a DIFFERENT spec is irrelevant.
    let other = crate::requeue_lease_gate_from(
        Ok(vec![lease_for("BUG-OTHER", None)]),
        None,
        &["BUG-9105"],
        |_| Err("probe failed".to_string()),
    );
    assert_eq!(other, crate::RequeueLeaseCheck::Clear);
    // The caller's own lease never blocks.
    let own = lease_for("BUG-9105", None);
    let selfcheck =
        crate::requeue_lease_gate_from(Ok(vec![own.clone()]), Some(&own), &["BUG-9105"], |_| {
            Ok(true)
        });
    assert_eq!(selfcheck, crate::RequeueLeaseCheck::Clear);
}

#[test]
fn rework_in_progress_metadata_only_refuses_over_live_lease() {
    let _env = crate::test_env::EnvVarsGuard::set(&[("AIDA_SESSION_ROLE", "advisor")]);
    let (tmp, storage) = story_1429_fixture(vec![req_for_test(
        "BUG-9106",
        RequirementStatus::InProgress,
    )]);
    write_lease(tmp.path(), &lease_for("BUG-9106", Some(std::process::id())));
    let err = rework_as(&storage, "BUG-9106", None, true)
        .expect_err("metadata-only rework must not reset a live session's spec");
    assert!(err.to_string().contains("claimed by live session"), "{err}");
    assert_eq!(
        status_of(&storage, "BUG-9106"),
        RequirementStatus::InProgress
    );
}

/// The reason goes into the audit note once, not also as a second comment.
#[test]
fn rework_reason_is_recorded_once_and_emits_spec_requeued() {
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_SESSION_ROLE", Some("advisor")),
        (crate::events::EVENTS_DISABLE_ENV, None),
    ]);
    let (tmp, storage) = story_1429_fixture(vec![parked("BUG-9107")]);
    rework_as(&storage, "BUG-9107", Some("fixed on main"), false).unwrap();
    let store = storage.load().unwrap();
    let r = store.get_requirement_by_spec_id("BUG-9107").unwrap();
    assert_eq!(r.status, RequirementStatus::Approved);
    let with_reason = r
        .comments
        .iter()
        .filter(|c| c.content.contains("fixed on main"))
        .count();
    assert_eq!(with_reason, 1, "{:#?}", r.comments);
    assert!(r.failure_reason.is_none());

    // A second requeue of the now-Approved spec writes no second audit note.
    rework_as(&storage, "BUG-9107", None, false).unwrap();
    let store = storage.load().unwrap();
    let r = store.get_requirement_by_spec_id("BUG-9107").unwrap();
    let notes = r
        .comments
        .iter()
        .filter(|c| c.content.starts_with("Returned from NeedsAttention"))
        .count();
    assert_eq!(notes, 1);

    let evs = crate::events::read_all(tmp.path());
    let requeued: Vec<_> = evs
        .iter()
        .filter(|e| matches!(e.kind, crate::events::EventKind::SpecRequeued { .. }))
        .collect();
    assert_eq!(requeued.len(), 1, "{evs:?}");
    assert!(matches!(
        &requeued[0].kind,
        crate::events::EventKind::SpecRequeued { via, .. } if via == "queue-rework"
    ));
    assert!(
        !evs.iter()
            .any(|e| matches!(e.kind, crate::events::EventKind::SpecReDriven { .. })),
        "a human requeue is not a supervised re-drive"
    );
}

/// The Storage door (CLI and MCP) checks the status on the copy read inside
/// `update_atomically`, not on the caller's earlier read.
#[test]
fn storage_door_status_check_happens_under_the_write() {
    let (_tmp, storage) = story_1429_fixture(vec![req_for_test(
        "BUG-9108",
        RequirementStatus::InProgress,
    )]);
    let id = storage
        .load()
        .unwrap()
        .get_requirement_by_spec_id("BUG-9108")
        .unwrap()
        .id;
    let ctx = crate::requeue::ReturnCtx {
        via: "test".into(),
        via_slug: "queue-rework",
        author: "t".into(),
        clear_escalation: true,
        reason: None,
    };
    // The caller believed it was NeedsAttention; the store says InProgress.
    let (outcome, _) = crate::requeue::return_to_flight_in_storage(
        &storage,
        id,
        &RequirementStatus::NeedsAttention,
        &RequirementStatus::Approved,
        &ctx,
    )
    .unwrap();
    assert!(matches!(
        outcome,
        crate::requeue::ReturnOutcome::StatusMoved { .. }
    ));
    let r = storage.load().unwrap();
    let r = r.get_requirement_by_spec_id("BUG-9108").unwrap();
    assert_eq!(r.status, RequirementStatus::InProgress);
    assert!(r.comments.is_empty());
}

/// The backend door (`aida edit`, the supervisor) checks the same way.
#[test]
fn backend_door_status_check_happens_under_the_write() {
    let tmp = tempfile::tempdir().unwrap();
    let backend = aida_core::GitBackend::new(&tmp.path().join(".aida-store")).unwrap();
    let r = req_for_test("BUG-9109", RequirementStatus::Approved);
    let id = r.id;
    let mut store = aida_core::RequirementsStore::default();
    store.requirements.push(r);
    backend.save(&store).unwrap();
    let ctx = crate::requeue::ReturnCtx {
        via: "`aida edit --status`".into(),
        via_slug: "edit",
        author: "t".into(),
        clear_escalation: true,
        reason: None,
    };
    let (outcome, _) = crate::requeue::return_to_flight_in_backend(
        &backend,
        id,
        &RequirementStatus::NeedsAttention,
        &RequirementStatus::Approved,
        &ctx,
    )
    .unwrap();
    assert!(matches!(
        outcome,
        crate::requeue::ReturnOutcome::AlreadyInFlight { .. }
    ));
    let (outcome, _) = crate::requeue::return_to_flight_in_backend(
        &backend,
        id,
        &RequirementStatus::NeedsAttention,
        &RequirementStatus::Rejected,
        &ctx,
    )
    .unwrap();
    assert!(matches!(
        outcome,
        crate::requeue::ReturnOutcome::StatusMoved { .. }
    ));
    let after = backend.load().unwrap();
    assert!(after.requirements[0].comments.is_empty());

    // And a real exit to Rejected goes through the owner (the target is a
    // parameter, not always Approved).
    let parked_req = parked("BUG-9110");
    let pid = parked_req.id;
    let mut store = backend.load().unwrap();
    store.requirements.push(parked_req);
    backend.save(&store).unwrap();
    let (outcome, copy) = crate::requeue::return_to_flight_in_backend(
        &backend,
        pid,
        &RequirementStatus::NeedsAttention,
        &RequirementStatus::Rejected,
        &ctx,
    )
    .unwrap();
    assert!(outcome.applied());
    let copy = copy.unwrap();
    assert_eq!(copy.status, RequirementStatus::Rejected);
    assert!(copy.failure_reason.is_none(), "failure cleared on disk too");
}

#[test]
fn triage_loop_without_tty_prints_hints_and_never_prompts() {
    let (_tmp, storage) = story_1429_fixture(vec![parked("BUG-9111")]);
    let mut input = std::io::Cursor::new(b"r\n".to_vec());
    crate::queue_cmd::rework_triage_loop(
        &storage,
        false,
        false,
        Some("codex"),
        false,
        &mut input,
        &mut |_| panic!("no decide without a terminal"),
        &mut |_| panic!("no show without a terminal"),
    )
    .unwrap();
    assert_eq!(input.position(), 0, "the loop must not read input");
    assert_eq!(
        status_of(&storage, "BUG-9111"),
        RequirementStatus::NeedsAttention
    );
    assert!(storage
        .queue_list("codex", true)
        .unwrap_or_default()
        .is_empty());
}

#[test]
fn triage_loop_r_keystroke_lands_approved_queued_and_emits_spec_requeued() {
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_SESSION_ROLE", Some("advisor")),
        (crate::events::EVENTS_DISABLE_ENV, None),
    ]);
    let (tmp, storage) = story_1429_fixture(vec![parked("BUG-9112")]);
    let mut input = std::io::Cursor::new(b"r\n".to_vec());
    crate::queue_cmd::rework_triage_loop(
        &storage,
        true,
        false,
        Some("codex"),
        false,
        &mut input,
        &mut |_| panic!("no decision is pending"),
        &mut |_| panic!("show not pressed"),
    )
    .unwrap();
    assert_eq!(status_of(&storage, "BUG-9112"), RequirementStatus::Approved);
    let entries = storage.queue_list("codex", true).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].for_role.as_deref(), Some("implementer"));
    let evs = crate::events::read_all(tmp.path());
    assert!(
        evs.iter().any(|e| e.spec.as_deref() == Some("BUG-9112")
            && matches!(e.kind, crate::events::EventKind::SpecRequeued { .. })),
        "{evs:?}"
    );
}

/// `[d]` hands the one spec to the existing decision path, then the loop
/// comes back to the same spec with the requeue now offered.
#[test]
fn triage_loop_d_decides_then_returns_to_the_same_spec() {
    let _env = crate::test_env::EnvVarsGuard::set(&[("AIDA_SESSION_ROLE", "advisor")]);
    let mut r = parked("BUG-9113");
    r.decision_request = Some(aida_core::DecisionRequest {
        question: "which fork?".into(),
        choices: Vec::new(),
        recommended: None,
        rationale: None,
        answered: None,
        note: None,
        asked_at: None,
        answered_at: None,
    });
    let (_tmp, storage) = story_1429_fixture(vec![r]);
    // `r` first is not offered (decision pending) and is an unknown key; then
    // `d` answers; then `r` requeues.
    let mut input = std::io::Cursor::new(b"r\nd\nr\n".to_vec());
    let mut decided = Vec::new();
    crate::queue_cmd::rework_triage_loop(
        &storage,
        true,
        false,
        Some("codex"),
        false,
        &mut input,
        &mut |spec| {
            decided.push(spec.to_string());
            storage.update_atomically(|s| {
                if let Some(r) = s
                    .requirements
                    .iter_mut()
                    .find(|r| r.spec_id.as_deref() == Some(spec))
                {
                    if let Some(d) = r.decision_request.as_mut() {
                        d.answered = Some(0);
                    }
                }
            })?;
            Ok(())
        },
        &mut |_| Ok(()),
    )
    .unwrap();
    assert_eq!(decided, vec!["BUG-9113".to_string()]);
    assert_eq!(status_of(&storage, "BUG-9113"), RequirementStatus::Approved);
}

#[test]
fn rework_without_id_rejects_spec_scoped_flags() {
    let (_tmp, storage) = story_1429_fixture(vec![parked("BUG-9114")]);
    for flags in [
        crate::queue_cmd::ReworkFlags {
            work: true,
            ..Default::default()
        },
        crate::queue_cmd::ReworkFlags {
            status: Some("approved"),
            ..Default::default()
        },
        crate::queue_cmd::ReworkFlags {
            reason: Some("x"),
            ..Default::default()
        },
        crate::queue_cmd::ReworkFlags {
            resume: true,
            ..Default::default()
        },
        crate::queue_cmd::ReworkFlags {
            for_role: Some("reviewer"),
            ..Default::default()
        },
    ] {
        let err = crate::queue_cmd::handle_rework_entry(&storage, None, &flags)
            .expect_err("a spec-scoped flag needs an ID");
        assert!(err.to_string().contains("aida rework <ID>"), "{err}");
    }
    assert_eq!(
        status_of(&storage, "BUG-9114"),
        RequirementStatus::NeedsAttention
    );
}

#[test]
fn drain_running_answers_only_when_the_lock_is_definite() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    assert_eq!(
        crate::queue_cmd::drain_running(tmp.path()),
        crate::queue_cmd::DrainRunning::No
    );
    assert!(crate::queue_cmd::drain_running_notice(tmp.path()).contains("aida drain start"));
    std::fs::write(crate::drain_lock::drain_lock_path(tmp.path()), "{not json").unwrap();
    assert_eq!(
        crate::queue_cmd::drain_running(tmp.path()),
        crate::queue_cmd::DrainRunning::CannotTell
    );
    assert!(crate::queue_cmd::drain_running_notice(tmp.path()).contains("cannot tell"));
}

#[test]
fn findings_tip_points_at_the_loop() {
    assert_eq!(
        crate::findings_triage_tip(3),
        "3 parked · `aida rework` to triage them one keystroke each"
    );
    assert!(crate::findings_triage_tip(1).contains("triage it"));
}
