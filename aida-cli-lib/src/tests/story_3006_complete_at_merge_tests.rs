//! STORY-3006: the train completes its landed commits' trailer specs at
//! merge time, and the next `aida pull` finds nothing left to do.
// trace:STORY-3006 | ai:claude
use super::*;
use aida_core::RequirementStatus;

fn git(repo: &Path, args: &[&str]) -> String {
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git runs");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "git {args:?} failed: {err}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit(repo: &Path, file: &str, subject: &str) -> String {
    std::fs::write(repo.join(file), subject).unwrap();
    git(repo, &["add", file]);
    git(repo, &["commit", "-q", "-m", subject]);
    git(repo, &["rev-parse", "HEAD"])
}

fn seed(store_dir: &Path, spec: &str, status: RequirementStatus) {
    use aida_core::db::DatabaseBackend;
    let backend = crate::test_env::home_fixture_backend(store_dir).unwrap();
    let mut req = aida_core::Requirement::new(format!("test {spec}"), String::new());
    req.spec_id = Some(spec.to_string());
    req.status = status;
    backend.add_requirement(req).unwrap();
}

fn status_of(store_dir: &Path, spec: &str) -> (RequirementStatus, Option<String>) {
    let store = crate::Storage::new(store_dir).load().unwrap();
    let req = store.get_requirement_by_spec_id(spec).unwrap();
    let info = req.implementation_info.as_ref();
    (
        req.status.clone(),
        info.and_then(|i| i.completion_sha.clone()),
    )
}

/// A primary checkout on `main` that has NOT pulled: the train's landed
/// commits sit on another ref, so a HEAD-based scan cannot see them.
/// Returns (tempdir, root, store_dir, main_before, completing_sha, refs_sha).
fn landed_train() -> (tempfile::TempDir, PathBuf, PathBuf, String, String, String) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["config", "user.name", "Test"]);
    git(&root, &["config", "user.email", "test@example.com"]);
    let main_before = commit(&root, "README.md", "chore: initial");
    let store_dir = root.join(".aida-store");
    seed(&store_dir, "BUG-9100", RequirementStatus::Done);
    seed(&store_dir, "STORY-9101", RequirementStatus::InProgress);
    git(&root, &["checkout", "-q", "-b", "train-landed"]);
    let completing = commit(&root, "a", "[AI:claude] fix: a (BUG-9100) (#4377)");
    let refs = commit(&root, "b", "[AI:claude] feat: b (Refs STORY-9101) (#4385)");
    git(&root, &["checkout", "-q", "main"]);
    (tmp, root, store_dir, main_before, completing, refs)
}

fn completion_events(root: &Path, spec: &str) -> usize {
    crate::events::read_all(root)
        .into_iter()
        .filter(|e| {
            e.spec.as_deref() == Some(spec)
                && matches!(e.kind, crate::events::EventKind::SpecCompleted { .. })
        })
        .count()
}

/// AC1 + AC3: a train landing whose squash subject ends in `(SPEC-ID) (#N)`
/// completes that spec at merge time, with the checkout not pulled; a
/// Refs-only landing leaves its spec open.
#[test]
fn train_landing_completes_trailer_spec_at_merge() {
    let _bump = crate::test_env::EnvVarGuard::unset("AIDA_AUTO_BUMP");
    let (_tmp, root, store_dir, _, completing, refs) = landed_train();

    complete_landed_specs(&root, &[(4377, completing.clone()), (4385, refs)]);

    assert_eq!(
        status_of(&store_dir, "BUG-9100"),
        (RequirementStatus::Completed, Some(completing)),
        "the `(BUG-9100) (#4377)` landing completes BUG-9100 at merge"
    );
    assert_ne!(
        status_of(&store_dir, "STORY-9101").0,
        RequirementStatus::Completed,
        "a Refs-only landing never completes its spec"
    );
    assert_eq!(completion_events(&root, "BUG-9100"), 1);
}

/// AC2: once the train completed a spec, the next `aida pull`'s scan over the
/// same commits changes nothing.
#[test]
fn pull_after_merge_time_completion_is_a_no_op() {
    let _bump = crate::test_env::EnvVarGuard::unset("AIDA_AUTO_BUMP");
    let (_tmp, root, store_dir, main_before, completing, refs) = landed_train();
    complete_landed_specs(&root, &[(4377, completing.clone()), (4385, refs)]);
    let before = status_of(&store_dir, "BUG-9100");

    // The human pull: main merges the landed commits; the scanner walks them.
    git(&root, &["merge", "-q", "--no-edit", "train-landed"]);
    let storage = crate::Storage::new(&store_dir);
    let flips = crate::db_reconcile::auto_bump_done_to_completed(
        &root,
        &store_dir,
        Some(&main_before),
        &storage,
    )
    .unwrap();

    assert!(flips.is_empty(), "pull re-completed: {:?}", flips.len());
    assert_eq!(status_of(&store_dir, "BUG-9100"), before);
    assert_eq!(completion_events(&root, "BUG-9100"), 1);
}

/// A landing whose commit could not be identified (an empty or non-hex sha)
/// completes nothing and leaves the spec for the next pull.
#[test]
fn landing_without_a_known_commit_completes_nothing() {
    let _bump = crate::test_env::EnvVarGuard::unset("AIDA_AUTO_BUMP");
    let (_tmp, root, store_dir, ..) = landed_train();

    complete_landed_specs(&root, &[(4377, String::new()), (4385, "HEAD".into())]);

    assert_eq!(status_of(&store_dir, "BUG-9100").0, RequirementStatus::Done);
}
