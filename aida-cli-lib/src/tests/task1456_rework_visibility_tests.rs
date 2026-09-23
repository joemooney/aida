//! TASK-1456 (follow-up to BUG-1515): the two remaining rework-visibility
//! criteria BUG-1515 didn't cover —
//!
//! 1. `aida list`'s default open lens must include a Done spec with an
//!    outstanding blocking refusal at its tip, noted as needing rework
//!    (`queue_cmd::select_done_rework_rows`).
//! 2. A `next N` / bare `--auto-complete` drain whose queued candidates are
//!    all refused must name them as rework instead of reading as a plain
//!    empty queue (`resolve_queue_rework_needed`).
//!
//! trace:TASK-1456 | ai:claude
use super::*;
use aida_core::{QueueEntry, Requirement, RequirementStatus};
use uuid::Uuid;

/// Minimal real-git helper, mirrored from `queue_work_tests.rs`'s BUG-1515
/// helper — `done_spec_outstanding_refusal` shells out to `git` to place the
/// verdict's reviewed sha against the branch tip.
// trace:TASK-1456 | ai:claude
fn git(repo: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn git_head(repo: &std::path::Path) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("git runs");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn summary_row(id: &str, status: &str) -> aida_core::RequirementSummary {
    aida_core::RequirementSummary {
        id: Uuid::new_v4(),
        spec_id: Some(id.into()),
        agreed_id: Some(id.into()),
        title: format!("{id} title"),
        description: String::new(),
        status: status.into(),
        priority: "medium".into(),
        owner: String::new(),
        assignee: None,
        feature: "Uncategorized".into(),
        req_type: "Task".into(),
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
        has_pending_decision: false,
        execution_mode: None,
        weight: None,
        origin: None,
        yaml_path: String::new(),
    }
}

// TASK-1456: no project root to check against degrades to "nothing folded
// in" — PRIN-5, never guess a Done row is rework when the verdict state
// can't be read.
#[test]
fn select_done_rework_rows_without_project_root_is_empty() {
    let rows = vec![summary_row("TASK-1", "Done")];
    let (rows, ids) = queue_cmd::select_done_rework_rows(rows, None);
    assert!(rows.is_empty());
    assert!(ids.is_empty());
}

// TASK-1456: a Done row with NO recorded verdict at all is plain
// awaiting-merge — not folded in as rework.
#[test]
fn select_done_rework_rows_excludes_done_with_no_verdict() {
    let dir = tempfile::tempdir().unwrap();
    let rows = vec![summary_row("TASK-1", "Done")];
    let (rows, ids) = queue_cmd::select_done_rework_rows(rows, Some(dir.path()));
    assert!(rows.is_empty());
    assert!(ids.is_empty());
}

// TASK-1456: a Done row whose recorded verdict is a still-live refusal AT
// the branch tip IS folded in — this is the exact state BUG-1515 taught
// `queue_fresh_pickup_policy` to read as rework. The returned id set must
// agree with the returned rows (the render-site reuse contract).
#[test]
fn select_done_rework_rows_includes_outstanding_refusal_at_tip() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(
        root,
        &["init", "--initial-branch=claude/task-1456", "--quiet"],
    );
    git(root, &["commit", "--allow-empty", "-m", "root", "--quiet"]);
    let head = git_head(root);
    crate::review_verdict::record_verdict(
        root,
        "TASK-1",
        Some("request-changes"),
        Some(&head),
        Some("claude/task-1456"),
        Some("needs another round"),
        &["fix the thing".to_string()],
        "reviewer",
    )
    .unwrap();

    let refused = summary_row("TASK-1", "Done");
    let refused_id = refused.id;
    let rows = vec![refused, summary_row("TASK-2", "Done")];
    let (selected, ids) = queue_cmd::select_done_rework_rows(rows, Some(root));
    assert_eq!(selected.len(), 1, "only the refused row is folded in");
    assert_eq!(selected[0].spec_id.as_deref(), Some("TASK-1"));
    assert_eq!(ids.len(), 1, "the id set must agree with the rows: {ids:?}");
    assert!(ids.contains(&refused_id));
}

// TASK-1456 (review follow-up): a refusal recorded against a STALE head —
// new commits landed on the branch since the review, the normal shape after
// a rework round — must be EXCLUDED, not folded in. Mirrors
// `queue_work_tests::stale_refusal_on_an_old_head_is_not_classified_awaiting_rework`
// for the batch (`select_done_rework_rows`/`resolve_done_rework_ids`) path,
// so `aida list`'s fold-in can never disagree with `queue_fresh_pickup_policy`
// on this case either.
#[test]
fn select_done_rework_rows_excludes_refusal_at_a_stale_tip() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(
        root,
        &["init", "--initial-branch=claude/task-1456", "--quiet"],
    );
    git(root, &["commit", "--allow-empty", "-m", "root", "--quiet"]);
    let old_head = git_head(root);
    crate::review_verdict::record_verdict(
        root,
        "TASK-1",
        Some("request-changes"),
        Some(&old_head),
        Some("claude/task-1456"),
        Some("needs another round"),
        &["fix the thing".to_string()],
        "reviewer",
    )
    .unwrap();
    // Rework happened: a new commit landed past the reviewed sha, but the
    // refusal file was never explicitly closed.
    git(root, &["commit", "--allow-empty", "-m", "fix", "--quiet"]);

    let rows = vec![summary_row("TASK-1", "Done")];
    let (selected, ids) = queue_cmd::select_done_rework_rows(rows, Some(root));
    assert!(
        selected.is_empty(),
        "a refusal pinned to an OLD head needs re-review, not another rework \
         round: {selected:?}"
    );
    assert!(ids.is_empty());
}

fn queue_entry(req_id: Uuid, for_role: Option<&str>) -> QueueEntry {
    QueueEntry {
        user_id: "u".into(),
        requirement_id: req_id,
        position: 1,
        added_by: "u".into(),
        note: None,
        added_at: chrono::Utc::now(),
        for_role: for_role.map(String::from),
        for_scope: None,
        for_session: None,
        added_by_machine: None,
    }
}

/// TASK-1456: the `next N` / bare `--auto-complete` drain's candidate scan
/// never considered `Done` at all (only Approved/Planned are "drivable
/// heads"), so a queue that is ENTIRELY Done-with-refusal specs used to
/// read as a plain empty queue. `resolve_queue_rework_needed` is the
/// resolver that lets `handle_auto_complete_next_n`'s idle branch name them
/// instead.
///
/// Fixture note: the store's `GitBackend::save`/`queue_add` auto-commit
/// fires whenever `is_git_repo(store_root)` is true — and `git rev-parse
/// --git-dir` ASCENDS to find an enclosing repo, so if `store_root` (a plain
/// subdirectory, no `.git` of its own) sits under an ALREADY-`git init`'d
/// project root, every store write lands a commit on the project's OWN
/// branch and moves its HEAD out from under a `reviewed_sha` captured
/// earlier. Real projects avoid this because the orphan `aida-store` branch
/// lives in its own worktree, isolated from the main branch's HEAD — this
/// fixture reproduces that isolation by ORDERING: every store/queue write
/// happens BEFORE `git init`, so `is_git_repo` is false throughout and no
/// stray commit ever lands.
#[test]
fn resolve_queue_rework_needed_finds_done_specs_with_outstanding_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let project_root = dir.path();

    let store_root = project_root.join("aida-store");
    let backend = aida_core::GitBackend::new(&store_root).unwrap();
    let storage = Storage::new(&store_root);
    let mut store = aida_core::RequirementsStore::default();

    let mut refused = Requirement::new("refused round".to_string(), String::new());
    refused.spec_id = Some("TASK-9001".to_string());
    refused.agreed_id = Some("TASK-9001".to_string());
    refused.status = RequirementStatus::Done;
    let refused_id = refused.id;
    store.requirements.push(refused);

    let mut untouched = Requirement::new("no verdict yet".to_string(), String::new());
    untouched.spec_id = Some("TASK-9002".to_string());
    untouched.agreed_id = Some("TASK-9002".to_string());
    untouched.status = RequirementStatus::Done;
    let untouched_id = untouched.id;
    store.requirements.push(untouched);

    backend.save(&store).unwrap();
    storage
        .queue_add(queue_entry(refused_id, Some("implementer")))
        .unwrap();
    storage
        .queue_add(queue_entry(untouched_id, Some("implementer")))
        .unwrap();

    // Only NOW does `project_root` become a git repo — after every store
    // write, so none of them could have landed a commit on it.
    git(
        project_root,
        &["init", "--initial-branch=claude/task-1456", "--quiet"],
    );
    git(
        project_root,
        &["commit", "--allow-empty", "-m", "root", "--quiet"],
    );
    let head = git_head(project_root);

    crate::review_verdict::record_verdict(
        project_root,
        "TASK-9001",
        Some("request-changes"),
        Some(&head),
        Some("claude/task-1456"),
        Some("needs another round"),
        &["fix the thing".to_string()],
        "reviewer",
    )
    .unwrap();

    let rework = resolve_queue_rework_needed(&storage, "u", Some("implementer")).unwrap();
    assert_eq!(
        rework.len(),
        1,
        "only the spec with a still-live refusal is reported as rework: {rework:?}"
    );
    assert_eq!(rework[0].spec, "TASK-9001");
    assert!(
        rework[0].reason.contains("REWORK"),
        "the reason must say REWORK: {}",
        rework[0].reason
    );
    assert!(
        rework[0].reason.contains("aida queue rework"),
        "the reason must name the working recovery route: {}",
        rework[0].reason
    );
}

/// TASK-1456: an empty/ordinary queue (nothing Done, nothing refused) must
/// not spuriously report rework — the idle branch's generic "nothing to
/// drive" message stays correct for the genuinely-empty case.
#[test]
fn resolve_queue_rework_needed_is_empty_for_an_ordinary_queue() {
    let dir = tempfile::tempdir().unwrap();
    let project_root = dir.path();
    let store_root = project_root.join("aida-store");
    let backend = aida_core::GitBackend::new(&store_root).unwrap();
    let storage = Storage::new(&store_root);
    let mut store = aida_core::RequirementsStore::default();

    let mut approved = Requirement::new("ready to drive".to_string(), String::new());
    approved.spec_id = Some("TASK-9003".to_string());
    approved.agreed_id = Some("TASK-9003".to_string());
    approved.status = RequirementStatus::Approved;
    let approved_id = approved.id;
    store.requirements.push(approved);
    backend.save(&store).unwrap();
    storage
        .queue_add(queue_entry(approved_id, Some("implementer")))
        .unwrap();

    let rework = resolve_queue_rework_needed(&storage, "u", Some("implementer")).unwrap();
    assert!(
        rework.is_empty(),
        "an ordinary queue has no rework: {rework:?}"
    );
}

// TASK-1456 (review follow-up): `aida list --format json`'s
// status_label/status_lens value for a folded-in rework row — "Rework
// Needed" / "ReworkNeeded", taking priority over any NeedsAttention parked
// lens (the two never actually coincide — a row is Done XOR NeedsAttention
// — but priority is asserted anyway so it never silently flips).
#[test]
fn list_json_row_carries_rework_needed_status_label() {
    let (label, lens) = git_backend_cmd::list_json_status_label_lens(true, None);
    assert_eq!(label.as_deref(), Some("Rework Needed"));
    assert_eq!(lens, Some("ReworkNeeded"));

    // Not rework: falls through to the (absent, here) parked lens.
    let (label, lens) = git_backend_cmd::list_json_status_label_lens(false, None);
    assert_eq!(label, None);
    assert_eq!(lens, None);
}

// TASK-1456 (review follow-up): the `next N` / bare `--auto-complete`
// drain's idle-branch message names every all-refused candidate, its
// reason, and reads as rework — not the plain "nothing to drive" a
// genuinely empty queue gets.
#[test]
fn next_n_idle_message_names_rework_candidates() {
    let rework = vec![
        events::IneligibleBatchMember {
            spec: "TASK-9001".to_string(),
            reason: "REWORK NEEDED — reviewer requested changes; still outstanding; route via \
                     `aida queue rework TASK-9001` then `aida queue work TASK-9001 \
                     --auto-complete=through-ci --no-human=both`"
                .to_string(),
        },
        events::IneligibleBatchMember {
            spec: "TASK-9002".to_string(),
            reason: "REWORK NEEDED — reviewer requested changes; still outstanding".to_string(),
        },
    ];
    let message = next_n_rework_idle_message(&rework);
    assert!(
        message.contains("2 items"),
        "must count both candidates: {message}"
    );
    assert!(
        message.contains("every candidate needs rework"),
        "must read as rework, not a plain empty queue: {message}"
    );
    assert!(message.contains("TASK-9001"), "{message}");
    assert!(message.contains("TASK-9002"), "{message}");
    assert!(
        message.contains("aida queue rework TASK-9001"),
        "must carry the recovery route: {message}"
    );

    let single = next_n_rework_idle_message(&rework[..1]);
    assert!(
        single.contains("1 item "),
        "singular item, no trailing s: {single}"
    );
}
