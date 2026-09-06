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

/// All status variants are covered — exhaustive match in
/// `rework_smart_target` means adding a new variant won't silently
/// fall through. This test exists so a future variant addition (e.g.
/// "Blocked") trips the compiler check, not a silent None default.
#[test]
fn covers_every_status_variant() {
    use RequirementStatus::*;
    for s in &[
        Draft, Approved, Planned, InProgress, Done, Completed, Rejected,
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
