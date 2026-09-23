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
