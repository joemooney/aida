//! BUG-1944: live-work evidence and the shared start-of-work bump.
// trace:BUG-1944 | ai:claude

use super::*;
use aida_core::{Requirement, RequirementStatus, Storage};

fn git(repo: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn commit_file(repo: &std::path::Path, name: &str) {
    std::fs::write(repo.join(name), name).unwrap();
    git(repo, &["add", name]);
    git(repo, &["commit", "-q", "-m", name]);
}

/// A git project on `main` with one commit and a distributed-style store
/// holding `spec_id` at `status`.
fn project(spec_id: &str, status: RequirementStatus) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-q", "-b", "main"]);
    std::fs::write(root.join(".gitignore"), ".aida/\n.aida-store/\n").unwrap();
    commit_file(root, "base.txt");
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::create_dir_all(root.join(".aida-store")).unwrap();
    let storage = Storage::new(root.join(".aida-store"));
    let mut req = Requirement::new("live work spec".into(), String::new());
    req.spec_id = Some(spec_id.to_string());
    req.status = status;
    let mut store = aida_core::models::RequirementsStore::new();
    store.requirements = vec![req];
    storage.save(&store).unwrap();
    dir
}

fn status_of(root: &std::path::Path, spec_id: &str) -> RequirementStatus {
    Storage::new(root.join(".aida-store"))
        .load()
        .unwrap()
        .get_requirement_by_spec_id(spec_id)
        .unwrap()
        .status
        .clone()
}

fn no_pr(_: &str) -> crate::PrLookup {
    crate::PrLookup::NoOpenPr
}

fn forge_down(_: &str) -> crate::PrLookup {
    crate::PrLookup::GhUnreachable("network down".to_string())
}

fn pr_open(branch: &str) -> crate::PrLookup {
    crate::PrLookup::Found(crate::OpenPrInfo {
        number: 77,
        title: "t".to_string(),
        url: "u".to_string(),
        head_branch: Some(branch.to_string()),
    })
}

// ── (a) pure decision ───────────────────────────────────────────────────────

#[test]
fn decide_keeps_on_open_pr_unmerged_or_unknown_and_reverts_only_when_idle() {
    let b = |ev| vec![("bug-1-work".to_string(), ev)];
    assert!(matches!(
        decide(&b(BranchEvidence::OpenPr(5))),
        LiveWorkVerdict::Keep(why) if why.contains("open PR #5")
    ));
    assert!(matches!(
        decide(&b(BranchEvidence::Unmerged(2))),
        LiveWorkVerdict::Keep(why) if why.contains("2 unmerged commits")
    ));
    assert!(matches!(
        decide(&b(BranchEvidence::Unknown("forge down".into()))),
        LiveWorkVerdict::Keep(_)
    ));
    assert_eq!(
        decide(&b(BranchEvidence::Idle)),
        LiveWorkVerdict::NoLiveWork
    );
    assert_eq!(decide(&[]), LiveWorkVerdict::NoLiveWork);
}

// ── (a) git + forge evidence ────────────────────────────────────────────────

#[test]
fn branch_ahead_of_main_is_live_work_without_asking_the_forge() {
    let dir = project("BUG-9944", RequirementStatus::InProgress);
    let root = dir.path();
    git(root, &["checkout", "-q", "-b", "bug-9944-work"]);
    commit_file(root, "fix.txt");
    git(root, &["checkout", "-q", "main"]);

    let verdict = spec_live_work_with(root, "BUG-9944", &|_| panic!("forge not needed"));
    assert!(
        matches!(&verdict, LiveWorkVerdict::Keep(why) if why.contains("1 unmerged commit")),
        "{verdict:?}"
    );
}

#[test]
fn merged_branch_consults_the_forge_and_fails_safe_when_it_is_down() {
    let dir = project("BUG-9945", RequirementStatus::InProgress);
    let root = dir.path();
    // Branch exists but carries nothing beyond main.
    git(root, &["branch", "bug-9945-work"]);

    assert_eq!(
        spec_live_work_with(root, "BUG-9945", &no_pr),
        LiveWorkVerdict::NoLiveWork
    );
    assert!(matches!(
        spec_live_work_with(root, "BUG-9945", &pr_open),
        LiveWorkVerdict::Keep(why) if why.contains("open PR #77")
    ));
    assert!(matches!(
        spec_live_work_with(root, "BUG-9945", &forge_down),
        LiveWorkVerdict::Keep(_)
    ));
}

#[test]
fn spec_without_any_branch_has_no_live_work() {
    let dir = project("BUG-9946", RequirementStatus::InProgress);
    git(dir.path(), &["branch", "bug-99460-work"]);
    git(dir.path(), &["branch", "story-9946-work"]);
    assert_eq!(
        spec_live_work_with(dir.path(), "BUG-9946", &|_| panic!("no candidate branch")),
        LiveWorkVerdict::NoLiveWork
    );
}

/// The doctor's lease-gone heal is the automated InProgress → Approved
/// writer: with unmerged commits on the spec branch it must not revert.
#[test]
fn doctor_lease_gone_heal_keeps_in_progress_while_branch_has_unmerged_commits() {
    let dir = project("BUG-9947", RequirementStatus::InProgress);
    let root = dir.path();
    git(root, &["checkout", "-q", "-b", "bug-9947-work"]);
    commit_file(root, "fix.txt");
    git(root, &["checkout", "-q", "main"]);

    let finding = crate::DoctorFinding {
        category: "spec-status-drift".to_string(),
        id: "BUG-9947".to_string(),
        summary: "BUG-9947 is In Progress but no active lease holds it".to_string(),
        action: "revert spec to Approved (no active lease found)".to_string(),
        safe_heal: true,
    };
    let result = crate::doctor_cmd::heal_doctor_spec_status(root, &finding).unwrap();
    assert_eq!(result.status, "skipped", "{result:?}");
    assert!(
        result.action.contains("kept BUG-9947 In Progress"),
        "{result:?}"
    );
    assert_eq!(status_of(root, "BUG-9947"), RequirementStatus::InProgress);

    // Once the branch is gone (merged and deleted), the heal may revert.
    git(root, &["branch", "-D", "bug-9947-work"]);
    let result = crate::doctor_cmd::heal_doctor_spec_status(root, &finding).unwrap();
    assert_eq!(result.status, "healed", "{result:?}");
    assert_eq!(status_of(root, "BUG-9947"), RequirementStatus::Approved);
}

// ── (b) the shared start-of-work bump ───────────────────────────────────────

#[test]
fn start_of_work_bump_moves_only_approved_specs() {
    let dir = project("TASK-9948", RequirementStatus::Approved);
    let root = dir.path();
    assert!(bump_spec_in_progress(
        root,
        "TASK-9948",
        aida_core::conflict::LEASE_TAKE_AUTHOR
    ));
    assert_eq!(status_of(root, "TASK-9948"), RequirementStatus::InProgress);
    // Idempotent.
    assert!(!bump_spec_in_progress(
        root,
        "TASK-9948",
        aida_core::conflict::LEASE_TAKE_AUTHOR
    ));

    for status in [
        RequirementStatus::Draft,
        RequirementStatus::Done,
        RequirementStatus::Completed,
        RequirementStatus::Rejected,
        RequirementStatus::Superseded,
    ] {
        let dir = project("TASK-9949", status.clone());
        assert!(!bump_spec_in_progress(
            dir.path(),
            "TASK-9949",
            aida_core::conflict::PR_OPEN_AUTHOR
        ));
        assert_eq!(status_of(dir.path(), "TASK-9949"), status);
    }
}

#[test]
fn pr_open_bump_targets_cover_scope_branch_and_covered_specs() {
    let targets = crate::pr_cmd::pr_open_bump_targets(
        Some("BUG-1944"),
        "bug-1944-work",
        &["TASK-7".to_string(), "bug-1944".to_string()],
    );
    assert_eq!(targets, vec!["BUG-1944".to_string(), "TASK-7".to_string()]);
    assert!(crate::pr_cmd::pr_open_bump_targets(
        Some("harness-worktree"),
        "worktree-agent-abc",
        &[]
    )
    .is_empty());
}

// ── review fixes: bounded forge, git failure ───────────────────────────────

#[test]
fn forge_lookup_runs_under_the_ceiling_and_a_spent_budget_skips_it() {
    // Each lookup runs with the scoped per-call ceiling set, so a hung `gh`
    // is killed and reads GhUnreachable (Keep) instead of stalling the doctor.
    let started = std::time::Instant::now();
    let seen =
        crate::live_work::bounded_pr_lookup(started, std::time::Duration::from_secs(60), || {
            assert_eq!(
                crate::FORGE_LOOKUP_TIMEOUT.with(|c| c.get()),
                Some(crate::FORGE_CLI_CALL_TIMEOUT)
            );
            crate::PrLookup::NoOpenPr
        });
    assert!(matches!(seen, crate::PrLookup::NoOpenPr));
    assert_eq!(crate::FORGE_LOOKUP_TIMEOUT.with(|c| c.get()), None);

    // Once the total budget is spent no lookup runs, and the answer is unknown.
    let spent = crate::live_work::bounded_pr_lookup(started, std::time::Duration::ZERO, || {
        panic!("a spent budget must not call the forge")
    });
    assert!(matches!(spent, crate::PrLookup::GhUnreachable(_)));
    let ev = vec![(
        "bug-9944-work".to_string(),
        crate::live_work::BranchEvidence::Unknown("forge lookup budget spent".to_string()),
    )];
    assert!(matches!(
        crate::live_work::decide(&ev),
        crate::live_work::LiveWorkVerdict::Keep(_)
    ));
}

#[test]
fn git_failing_to_list_branches_keeps_the_spec() {
    // A checkout whose refs git cannot read must not read as "no branches".
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".git"), "gitdir: /nonexistent/bug-1944\n").unwrap();
    assert!(crate::live_work::spec_branch_refs(dir.path(), "BUG-9944").is_err());
    let verdict = crate::live_work::spec_live_work_with(dir.path(), "BUG-9944", &no_pr);
    assert!(
        matches!(verdict, crate::live_work::LiveWorkVerdict::Keep(_)),
        "{verdict:?}"
    );
}

#[test]
fn every_pr_open_path_bumps_the_branch_spec() {
    // `try_auto_queue_pr_review` (used by the PR skill, the session-end
    // backup and the orphan sweep) bumps through `bump_pr_specs`.
    let dir = project("BUG-9945", RequirementStatus::Approved);
    crate::live_work::bump_pr_specs(dir.path(), "bug-9945-work", &[]);
    assert_eq!(
        status_of(dir.path(), "BUG-9945"),
        RequirementStatus::InProgress
    );
}
