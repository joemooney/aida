//! TASK-1564: a drain phase failure is recorded as a comment on the parent
//! spec, not as its own draft spec.
// trace:TASK-1564 | ai:claude

use super::{auto_complete, auto_complete_telemetry, drain_failure_note};
use aida_core::DatabaseBackend;
use aida_core::Storage;

fn run_git(repo: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A git repo with a distributed store holding one parent spec. The shape
/// matters: `detect_distributed_store_from` resolves through
/// `.aida/config.toml`, so a fixture that only drops a `.aida-store/`
/// directory would exercise the no-store fallback instead of the real path.
fn project_with_parent_spec(spec_id: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();

    run_git(&root, &["init", "--initial-branch=drain-lane", "--quiet"]);
    run_git(&root, &["config", "user.email", "test@example.com"]);
    run_git(&root, &["config", "user.name", "Test"]);
    std::fs::write(root.join("README.md"), "init\n").unwrap();
    run_git(&root, &["add", "README.md"]);
    run_git(&root, &["commit", "-q", "-m", "chore: init"]);

    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida").join("config.toml"),
        "[deployment]\nstore_path = \".aida-store\"\n",
    )
    .unwrap();

    let store_root = root.join(".aida-store");
    let backend = aida_core::GitBackend::new(&store_root).unwrap();
    let mut req = aida_core::Requirement::new(format!("parent of {spec_id}"), String::new());
    req.spec_id = Some(spec_id.to_string());
    let mut store = aida_core::RequirementsStore::default();
    store.requirements.push(req);
    backend.save(&store).unwrap();

    (tmp, root)
}

fn failure_event(spec: &str, completed_at: &str) -> auto_complete_telemetry::AutoCompleteEvent {
    auto_complete_telemetry::AutoCompleteEvent {
        spec_id: spec.to_string(),
        started_at: "2026-10-02T00:00:00Z".to_string(),
        completed_at: completed_at.to_string(),
        outcome: "failed".to_string(),
        variant: "full".to_string(),
        failed_phase: Some(2),
        failure_kind: Some("ci-red".to_string()),
        failure_message: Some("CI was red on 3 checks".to_string()),
        phase_durations: vec![auto_complete_telemetry::PhaseDuration {
            phase: 1,
            slug: "implementer".to_string(),
            elapsed_ms: 1234,
        }],
        total_ms: 4321,
        drafted_bug: None,
        failure_comment: None,
        binary_sha: Some("abc1234".to_string()),
        auto_rebase: Vec::new(),
        lifecycle_skips: Vec::new(),
    }
}

fn record(
    root: &std::path::Path,
    spec: &str,
    completed_at: &str,
) -> (Option<String>, aida_core::RequirementsStore) {
    let failure = auto_complete::PhaseFailure::of(
        auto_complete::FailureKind::CiRed,
        "CI was red on 3 checks",
    );
    let event = failure_event(spec, completed_at);
    let id = super::note_auto_complete_failure_on_parent(
        root,
        spec,
        auto_complete::Phase::Ci,
        &failure,
        "re-run CI after fixing the red checks",
        &event,
    );
    let store = Storage::new(root.join(".aida-store")).load().unwrap();
    (id, store)
}

fn note_body(store: &aida_core::RequirementsStore, spec: &str) -> String {
    let req = store
        .get_requirement_by_spec_id(spec)
        .unwrap_or_else(|| panic!("{spec} is in the store"));
    let notes: Vec<&aida_core::Comment> = req
        .comments
        .iter()
        .filter(|c| c.content.contains(drain_failure_note::MARKER))
        .collect();
    assert_eq!(
        notes.len(),
        1,
        "exactly one drain-failure note, found {}",
        notes.len()
    );
    notes[0].content.clone()
}

/// AC1: the failure lands as a comment on the parent and creates NO new spec
/// object. The count is the assertion — a note that also filed a draft would
/// pass a comment-only check.
#[test]
fn phase_failure_comments_on_the_parent_and_files_no_spec() {
    let (_tmp, root) = project_with_parent_spec("TASK-9001");
    let before = Storage::new(root.join(".aida-store"))
        .load()
        .unwrap()
        .requirements
        .len();

    let (id, store) = record(&root, "TASK-9001", "2026-10-02T01:00:00Z");

    assert!(id.is_some(), "the note's comment id is returned");
    assert_eq!(
        store.requirements.len(),
        before,
        "no spec object may be created — before {before}, after {}",
        store.requirements.len()
    );
    let req = store.get_requirement_by_spec_id("TASK-9001").unwrap();
    assert_eq!(req.comments.len(), 1, "the note is the parent's comment");
    assert_eq!(req.comments[0].id.to_string(), id.unwrap());
}

/// AC2: the comment body — not a log line — carries the failed phase, the
/// seat, the commit and branch under test, and the captured failure text.
#[test]
fn the_note_carries_phase_seat_branch_commit_and_failure_text() {
    let (_tmp, root) = project_with_parent_spec("TASK-9002");
    let head = run_git(&root, &["rev-parse", "HEAD"]);
    // `Seat: ci` is the NO-role spelling, so clear the key under the env lock
    // rather than trusting the ambient environment.
    let _no_role = crate::test_env::EnvVarsGuard::apply(&[("AIDA_SESSION_ROLE", None)]);

    let (_, store) = record(&root, "TASK-9002", "2026-10-02T01:00:00Z");
    let body = note_body(&store, "TASK-9002");

    assert!(
        body.contains("phase 2 (ci)"),
        "names the failed phase: {body}"
    );
    assert!(body.contains("Seat: ci"), "names the seat: {body}");
    assert!(
        body.contains("Branch under test: drain-lane"),
        "names the branch under test: {body}"
    );
    assert!(
        body.contains(&head[..12]),
        "names the commit under test ({head}): {body}"
    );
    assert!(
        body.contains("CI was red on 3 checks"),
        "carries the captured failure text: {body}"
    );
    assert!(
        body.contains("Failure kind: `ci-red`"),
        "carries the failure kind: {body}"
    );
    assert!(
        body.contains("re-run CI after fixing the red checks"),
        "carries the recovery hint: {body}"
    );
    assert!(
        body.contains("Recorded at: 2026-10-02T01:00:00Z"),
        "carries the timestamp: {body}"
    );
    assert!(
        body.contains("phase 1 (implementer): 1234 ms"),
        "carries the phase durations: {body}"
    );
}

/// AC3: promotion stays available and explicit. The recipe is in the note
/// itself, so the reader who judges it a distinct defect never has to go
/// looking for the command.
#[test]
fn the_note_carries_an_explicit_promotion_recipe() {
    let (_tmp, root) = project_with_parent_spec("TASK-9003");
    let (_, store) = record(&root, "TASK-9003", "2026-10-02T01:00:00Z");
    let body = note_body(&store, "TASK-9003");

    assert!(
        body.contains("NOT promoted automatically"),
        "promotion is explicit, not implied: {body}"
    );
    assert!(
        body.contains("aida add --type bug")
            && body.contains("--parent TASK-9003")
            && body.contains("--description-from-file"),
        "carries a runnable promotion recipe: {body}"
    );
    assert!(
        body.contains("aida edit <NEW-ID> --status approved"),
        "the recipe lands an approvable spec, not a second draft: {body}"
    );
}

/// The BUG-864 anti-spam rule, carried across the container change: a
/// re-drive of a still-broken spec bumps the existing note instead of
/// stacking a second one.
#[test]
fn a_recurrence_bumps_the_existing_note_rather_than_adding_a_second() {
    let (_tmp, root) = project_with_parent_spec("TASK-9004");
    let (first, _) = record(&root, "TASK-9004", "2026-10-02T01:00:00Z");
    let (second, store) = record(&root, "TASK-9004", "2026-10-02T02:00:00Z");

    assert_eq!(first, second, "the same note is bumped, not replaced");
    let req = store.get_requirement_by_spec_id("TASK-9004").unwrap();
    assert_eq!(req.comments.len(), 1, "still exactly one note");
    let body = &req.comments[0].content;
    assert!(body.contains("Attempts: 2"), "attempts bumped: {body}");
    assert!(
        body.contains("Latest recurrence: 2026-10-02T02:00:00Z"),
        "latest recurrence restamped: {body}"
    );
}

/// A failure in a DIFFERENT phase is a different signature and gets its own
/// note — the control for the recurrence test above, which would also pass if
/// every failure collapsed into one note.
#[test]
fn a_different_phase_gets_its_own_note() {
    let (_tmp, root) = project_with_parent_spec("TASK-9005");
    record(&root, "TASK-9005", "2026-10-02T01:00:00Z");

    let failure = auto_complete::PhaseFailure::of(
        auto_complete::FailureKind::NoPr,
        "the implementer opened no PR",
    );
    let mut event = failure_event("TASK-9005", "2026-10-02T02:00:00Z");
    event.failed_phase = Some(1);
    event.failure_kind = Some("no-pr".to_string());
    super::note_auto_complete_failure_on_parent(
        &root,
        "TASK-9005",
        auto_complete::Phase::Implementer,
        &failure,
        "run the implementer again",
        &event,
    );

    let store = Storage::new(root.join(".aida-store")).load().unwrap();
    let req = store.get_requirement_by_spec_id("TASK-9005").unwrap();
    assert_eq!(
        req.comments.len(),
        2,
        "a distinct (phase, kind) signature gets its own note"
    );
}

/// AC4: nothing silently swallows a phase failure, and a note that cannot be
/// written cannot change the drain's control flow. With no store to resolve,
/// the recorder returns `None` and does not panic — the caller's `if let Some`
/// then leaves the exit code and the ledger line exactly as they were.
#[test]
fn an_unresolvable_store_returns_none_without_panicking() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("no-aida-here");
    std::fs::create_dir_all(&root).unwrap();

    let failure = auto_complete::PhaseFailure::of(
        auto_complete::FailureKind::CiRed,
        "CI was red on 3 checks",
    );
    let event = failure_event("TASK-9006", "2026-10-02T01:00:00Z");
    let id = super::note_auto_complete_failure_on_parent(
        &root,
        "TASK-9006",
        auto_complete::Phase::Ci,
        &failure,
        "hint",
        &event,
    );
    assert!(id.is_none(), "no store, no note — and no panic");
}

/// AC4: the ledger line keeps every failure field it carried before. The
/// container for the narrative moved; the telemetry record did not.
#[test]
fn the_ledger_line_still_carries_every_failure_field() {
    let mut event = failure_event("TASK-9007", "2026-10-02T01:00:00Z");
    event.failure_comment = Some("0199ffff-0000-7000-8000-000000000001".to_string());
    let line = serde_json::to_string(&event).unwrap();

    for field in [
        "\"outcome\":\"failed\"",
        "\"failed_phase\":2",
        "\"failure_kind\":\"ci-red\"",
        "\"failure_message\":\"CI was red on 3 checks\"",
    ] {
        assert!(line.contains(field), "ledger line lost {field}: {line}");
    }
    assert!(
        line.contains("\"failure_comment\":\"0199ffff-0000-7000-8000-000000000001\""),
        "the ledger points at the note: {line}"
    );

    // Additive: a line written before the field existed still reads back.
    let legacy = line.replace(
        ",\"failure_comment\":\"0199ffff-0000-7000-8000-000000000001\"",
        "",
    );
    let parsed: auto_complete_telemetry::AutoCompleteEvent = serde_json::from_str(&legacy).unwrap();
    assert!(parsed.failure_comment.is_none());
    assert_eq!(parsed.failure_kind.as_deref(), Some("ci-red"));
}

/// The seat names the session role when one is set, so a note written from a
/// non-default seat says so rather than reading as the phase's default.
#[test]
fn the_seat_names_the_session_role_when_one_is_set() {
    let (_tmp, root) = project_with_parent_spec("TASK-9008");
    // The guard holds `ENV_LOCK` for its whole lifetime, so it both sets the
    // key and serialises against every sibling that reads it. A raw
    // `set_var` here would trip `test_env`'s source-scanning guard.
    let _role = crate::test_env::EnvVarGuard::set("AIDA_SESSION_ROLE", "implementer");
    let (_, store) = record(&root, "TASK-9008", "2026-10-02T01:00:00Z");

    let body = note_body(&store, "TASK-9008");
    assert!(
        body.contains("Seat: ci (session role: implementer)"),
        "the note names both the phase's seat and the session role: {body}"
    );
}

/// The signature predicate is what keeps a recurrence from stacking, so it
/// has to reject a near-miss as well as accept an exact match.
#[test]
fn signature_matching_requires_the_marker_and_the_exact_signature() {
    let sig = drain_failure_note::signature(2, "ci", "ci-red");
    let body = format!("{}\n- {sig}\n", drain_failure_note::MARKER);
    assert!(drain_failure_note::matches_signature(&body, &sig));

    assert!(
        !drain_failure_note::matches_signature(&format!("- {sig}\n"), &sig),
        "an operator's prose quoting the signature is not one of our notes"
    );
    assert!(
        !drain_failure_note::matches_signature(
            &body,
            &drain_failure_note::signature(2, "ci", "ci-timeout")
        ),
        "a different failure kind is a different signature"
    );
    assert!(
        !drain_failure_note::matches_signature(
            &body,
            &drain_failure_note::signature(3, "reviewer", "ci-red")
        ),
        "a different phase is a different signature"
    );
}
