//! `aida queue add <SPEC> --for <role>` wrote into the CALLING user's queue
//! file, and every reader resolved only its own file — so a routing written by
//! an agent advisor seat was invisible to the human who actually wore that
//! role, and `queue work` then blamed a lost lease for it.
//!
//! These cover the read-side fallback: a role-routed entry written under user
//! A is visible to (and workable by) user B holding that role, the entry is
//! attributed to the routing user, the stored keys are untouched, and the
//! not-in-your-queue diagnostic names the real holder instead of guessing at a
//! lease.
// trace:BUG-774 | ai:claude

use crate::queue_role_fallback;
use aida_core::{QueueEntry, Storage};

/// Build a store with one plain spec and return its uuid.
fn seed_spec(root: &std::path::Path, spec_id: &str) -> uuid::Uuid {
    use aida_core::db::{DatabaseBackend, GitBackend};
    let backend = GitBackend::new(root).unwrap();
    let mut store = aida_core::RequirementsStore::default();
    let mut req = aida_core::Requirement::new(format!("{spec_id} title"), String::new());
    req.req_type = aida_core::RequirementType::Task;
    req.spec_id = Some(spec_id.to_string());
    req.status = aida_core::RequirementStatus::Approved;
    let id = req.id;
    store.requirements.push(req);
    backend.save(&store).unwrap();
    id
}

fn entry_for(user: &str, req: uuid::Uuid, role: Option<&str>, position: i64) -> QueueEntry {
    QueueEntry {
        user_id: user.to_string(),
        requirement_id: req,
        position,
        added_by: user.to_string(),
        note: None,
        added_at: chrono::Utc::now(),
        for_role: role.map(str::to_string),
        for_scope: None,
        for_session: None,
        added_by_machine: None,
    }
}

/// The headline repro: user A (an agent advisor seat) routes a spec `--for
/// implementer`; user B, who wears the implementer role, must SEE it — and the
/// routing user's queue file must be left exactly as written.
#[test]
fn role_routed_entry_written_by_one_user_is_visible_to_the_role_holder() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("aida-store");
    let routed = seed_spec(&root, "TASK-7740");
    let mine = seed_spec(&root, "TASK-7741");
    let storage = Storage::new(&root);

    // The advisor seat routes work to whoever wears `implementer`.
    storage
        .queue_add(entry_for(
            "claude-advisor-1",
            routed,
            Some("implementer"),
            1000,
        ))
        .unwrap();
    // The human has their own, unrelated, entry.
    storage
        .queue_add(entry_for("joe", mine, Some("implementer"), 1000))
        .unwrap();

    // Own-file-only read: the routed spec is nowhere to be seen (the bug).
    let own = storage.queue_list("joe", false).unwrap();
    assert_eq!(own.len(), 1, "own file holds only the human's own entry");
    assert!(!own.iter().any(|e| e.requirement_id == routed));

    // Role-fallback read: the routed spec surfaces for the implementer.
    let seen = queue_role_fallback::queue_list_with_role_fallback(
        &storage,
        "joe",
        Some("implementer"),
        false,
    )
    .unwrap();
    assert_eq!(seen.len(), 2, "own entry plus the peer's routing: {seen:?}");
    let surfaced = seen
        .iter()
        .find(|e| e.requirement_id == routed)
        .expect("the peer-routed entry must be visible to the role holder");

    // Attribution: it is visibly the routing user's, not ours.
    assert_eq!(
        queue_role_fallback::routed_by_other_user(surfaced, "joe"),
        Some("claude-advisor-1"),
        "a surfaced foreign entry must name the user who routed it"
    );
    // Our own entry is NOT attributed elsewhere.
    let own_entry = seen.iter().find(|e| e.requirement_id == mine).unwrap();
    assert_eq!(
        queue_role_fallback::routed_by_other_user(own_entry, "joe"),
        None
    );

    // The storage invariant: the routing user's file is untouched — the key,
    // the stored user_id, and added_by all stay as written.
    let routing_file = storage.queue_list("claude-advisor-1", false).unwrap();
    assert_eq!(routing_file.len(), 1);
    assert_eq!(routing_file[0].user_id, "claude-advisor-1");
    assert_eq!(routing_file[0].added_by, "claude-advisor-1");
    assert!(root.join("registry/queues/claude-advisor-1.yaml").exists());
    assert_eq!(
        storage.queue_list("joe", false).unwrap().len(),
        1,
        "the reader must not have copied the foreign entry into our file"
    );
}

/// A routing for a DIFFERENT role stays invisible — the fallback widens by
/// role, it does not merge every peer's queue into yours.
#[test]
fn fallback_only_surfaces_entries_routed_to_the_callers_role() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("aida-store");
    let for_reviewer = seed_spec(&root, "TASK-7742");
    let storage = Storage::new(&root);
    storage
        .queue_add(entry_for("alice", for_reviewer, Some("reviewer"), 1000))
        .unwrap();

    let as_implementer = queue_role_fallback::queue_list_with_role_fallback(
        &storage,
        "joe",
        Some("implementer"),
        false,
    )
    .unwrap();
    assert!(as_implementer.is_empty(), "{as_implementer:?}");

    let as_reviewer = queue_role_fallback::queue_list_with_role_fallback(
        &storage,
        "joe",
        Some("reviewer"),
        false,
    )
    .unwrap();
    assert_eq!(as_reviewer.len(), 1);
}

/// Identity comparison folds case through the shared canonical helper: a shell
/// reporting `Joe` must not see its OWN queue file as a foreign one.
#[test]
fn own_queue_file_is_never_treated_as_foreign_across_case() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("aida-store");
    let spec = seed_spec(&root, "TASK-7743");
    let storage = Storage::new(&root);
    storage
        .queue_add(entry_for("joe", spec, Some("implementer"), 1000))
        .unwrap();

    // `Joe` resolves to the existing `joe.yaml`; the entry is ours, once.
    let seen = queue_role_fallback::queue_list_with_role_fallback(
        &storage,
        "Joe",
        Some("implementer"),
        false,
    )
    .unwrap();
    assert_eq!(seen.len(), 1, "no duplicate from the case-variant path");
    assert_eq!(
        queue_role_fallback::routed_by_other_user(&seen[0], "Joe"),
        None,
        "a case-only identity difference is the SAME person"
    );
}

/// A spec already in your own queue never doubles up because a peer routed it
/// too.
#[test]
fn own_entry_wins_over_a_duplicate_peer_routing() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("aida-store");
    let spec = seed_spec(&root, "TASK-7744");
    let storage = Storage::new(&root);
    storage
        .queue_add(entry_for("joe", spec, Some("implementer"), 1000))
        .unwrap();
    storage
        .queue_add(entry_for("alice", spec, Some("implementer"), 500))
        .unwrap();

    let seen = queue_role_fallback::queue_list_with_role_fallback(
        &storage,
        "joe",
        Some("implementer"),
        false,
    )
    .unwrap();
    assert_eq!(seen.len(), 1, "deduped by requirement id: {seen:?}");
    assert_eq!(seen[0].user_id, "joe");
}

/// With no role context the reader keeps the historical own-file-only view.
#[test]
fn no_role_means_no_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("aida-store");
    let spec = seed_spec(&root, "TASK-7745");
    let storage = Storage::new(&root);
    storage
        .queue_add(entry_for("alice", spec, Some("implementer"), 1000))
        .unwrap();

    let seen =
        queue_role_fallback::queue_list_with_role_fallback(&storage, "joe", None, false).unwrap();
    assert!(seen.is_empty(), "{seen:?}");
}

/// `queue work` must be able to work a spec a peer routed to our role — the
/// plan resolves to that entry instead of erroring.
#[test]
fn queue_work_resolves_a_peer_routed_entry() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("aida-store");
    let spec = seed_spec(&root, "TASK-7746");
    let storage = Storage::new(&root);
    storage
        .queue_add(entry_for(
            "claude-advisor-1",
            spec,
            Some("implementer"),
            1000,
        ))
        .unwrap();

    let _env = crate::test_env::EnvVarGuard::set("AIDA_SESSION_ROLE", "implementer");
    let plan = crate::queue_cmd::resolve_queue_work_plan(
        &storage,
        "joe",
        Some("TASK-7746"),
        None,
        /* strict */ true,
        /* dry_run */ false,
    )
    .expect("a peer-routed entry must be workable by the role holder");
    assert_eq!(plan.entries.len(), 1);
    assert_eq!(plan.entries[0].spec_id, "TASK-7746");
}

/// The secondary bug: when a spec is queued by SOMEONE ELSE, say so — don't
/// blame a lost lease. Here the caller wears a different role, so the fallback
/// doesn't surface it and we fall through to the diagnostic.
#[test]
fn not_in_your_queue_diagnostic_names_the_holder_instead_of_a_lost_lease() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("aida-store");
    let spec = seed_spec(&root, "TASK-7747");
    let storage = Storage::new(&root);
    storage
        .queue_add(entry_for(
            "claude-advisor-1",
            spec,
            Some("implementer"),
            1000,
        ))
        .unwrap();

    let holders = queue_role_fallback::queued_by_other_users(&storage, "joe", &spec);
    assert_eq!(
        holders,
        vec![queue_role_fallback::ForeignQueueHolder {
            user: "claude-advisor-1".to_string(),
            for_role: Some("implementer".to_string()),
        }]
    );

    // The caller is wearing `advisor`, so the entry isn't routed to them.
    let _env = crate::test_env::EnvVarGuard::set("AIDA_SESSION_ROLE", "advisor");
    let err = crate::queue_cmd::resolve_queue_work_plan(
        &storage,
        "joe",
        Some("TASK-7747"),
        None,
        /* strict */ true,
        /* dry_run */ false,
    )
    .expect_err("a spec routed to another role must not resolve a plan")
    .to_string();
    assert!(
        err.contains("queued by @claude-advisor-1"),
        "must name the holder, got: {err}"
    );
    assert!(
        err.contains("for role implementer"),
        "must name the routing role, got: {err}"
    );
    assert!(
        !err.contains("lease may have been lost"),
        "must NOT blame a lost lease when the entry demonstrably exists: {err}"
    );
}

/// The genuinely-unqueued case keeps its own honest wording — no queue file
/// holds the spec, so a lost lease is the remaining explanation.
#[test]
fn genuinely_unqueued_in_progress_spec_still_reports_a_possible_lost_lease() {
    let mut req = aida_core::Requirement::new("orphan".to_string(), String::new());
    req.req_type = aida_core::RequirementType::Task;
    req.spec_id = Some("TASK-7748".to_string());
    req.status = aida_core::RequirementStatus::InProgress;
    let msg = crate::queue_cmd::format_queue_work_not_queued_error(
        "TASK-7748",
        &req,
        Some("implementer"),
    );
    assert!(msg.contains("genuinely unqueued"), "msg: {msg}");
    assert!(msg.contains("lease may have been lost"), "msg: {msg}");
}

/// The role of interest: an explicit `--for` wins, `any` (unrouted-only)
/// disables the fallback, else the active session role is used.
#[test]
fn fallback_role_resolution() {
    assert_eq!(
        queue_role_fallback::fallback_role(Some("reviewer"), Some("implementer")),
        Some("reviewer".to_string())
    );
    assert_eq!(
        queue_role_fallback::fallback_role(Some("any"), Some("implementer")),
        None
    );
    assert_eq!(
        queue_role_fallback::fallback_role(None, Some("implementer")),
        Some("implementer".to_string())
    );
    assert_eq!(queue_role_fallback::fallback_role(None, Some("")), None);
    assert_eq!(queue_role_fallback::fallback_role(None, None), None);
    // The deprecated `dialog` token normalizes to `advisor` at this boundary
    // like every other role-name boundary.
    assert_eq!(
        queue_role_fallback::fallback_role(None, Some("dialog")),
        Some("advisor".to_string())
    );
}
