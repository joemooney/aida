//! BUG-2209: a sync commit never publishes a roster change that drops an
//! advisor. A test once wrote the hermetic user's roster over the operator's,
//! and the next `chore: sync pending changes` commit pushed it.
// trace:BUG-2209 | ai:claude

use super::*;

const OPERATOR_ROSTER: &str = "[members]\n\
     bug-1618-hermetic-test-user = [\"implementer\", \"advisor\"]\n\
     joe = [\"advisor\", \"implementer\", \"integrator\", \"product\", \"reviewer\"]\n\
     \"joe.mooney\" = [\"advisor\", \"implementer\"]\n";

/// A store worktree (`.aida-store`) with the operator's roster committed.
fn store_with_roster() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join(".aida-store");
    std::fs::create_dir_all(store.join("registry")).unwrap();
    std::fs::create_dir_all(store.join("objects")).unwrap();
    run_git(&store, &["init", "-q", "-b", "aida-store"]);
    run_git(&store, &["config", "user.email", "test@example.com"]);
    run_git(&store, &["config", "user.name", "Test"]);
    std::fs::write(store.join("registry/team.toml"), OPERATOR_ROSTER).unwrap();
    run_git(&store, &["add", "."]);
    run_git(&store, &["commit", "-qm", "seed"]);
    (tmp, store)
}

/// The exact da5ab2b581 shape: the pending roster holds only the hermetic
/// user. The sync commit goes ahead without it, the committed roster is back
/// in the worktree, and the rejected file is kept for the operator.
#[test]
fn bug_2209_sync_commit_refuses_a_roster_that_drops_advisors() {
    let (_tmp, store) = store_with_roster();
    std::fs::write(
        store.join("registry/team.toml"),
        "[members]\nbug-1618-hermetic-test-user = [\"implementer\"]\n",
    )
    .unwrap();
    std::fs::write(store.join("objects/spec.yaml"), "id: X\n").unwrap();

    assert!(commit_pending_store_changes_on_host(&store).unwrap());

    let committed = run_git(&store, &["show", "HEAD:registry/team.toml"]);
    assert!(committed.contains("joe = ["), "{committed}");
    assert!(committed.contains("\"joe.mooney\""), "{committed}");
    assert_eq!(
        std::fs::read_to_string(store.join("registry/team.toml")).unwrap(),
        OPERATOR_ROSTER
    );
    assert_eq!(
        run_git(&store, &["show", "--name-only", "--format=", "HEAD"]),
        "objects/spec.yaml",
        "the other pending change is still committed"
    );
    let rejected = run_git(
        &store,
        &["rev-parse", "--git-path", "aida/rejected-team.toml"],
    );
    let rejected = std::fs::read_to_string(store.join(rejected)).unwrap();
    assert!(
        rejected.contains("bug-1618-hermetic-test-user"),
        "{rejected}"
    );
}

/// Which members count as lost: an advisor removed, an advisor seat dropped,
/// or every advisor of a deleted or unparseable roster. Other changes pass.
#[test]
fn bug_2209_advisors_lost_names_only_removed_advisor_seats() {
    use crate::team::advisors_lost;
    let mut lost = advisors_lost(
        OPERATOR_ROSTER,
        Some("[members]\nbug-1618-hermetic-test-user = [\"implementer\"]\n"),
    );
    lost.sort();
    assert_eq!(
        lost,
        ["bug-1618-hermetic-test-user", "joe", "joe.mooney"].map(String::from)
    );
    assert_eq!(advisors_lost(OPERATOR_ROSTER, None).len(), 3);
    assert_eq!(advisors_lost(OPERATOR_ROSTER, Some("not = [toml")).len(), 3);
    // Adding a member, or changing a non-advisor seat, loses nobody.
    let grown = format!("{OPERATOR_ROSTER}alice = [\"implementer\"]\n");
    assert!(advisors_lost(OPERATOR_ROSTER, Some(&grown)).is_empty());
    assert!(advisors_lost("[members]\nbob = [\"implementer\"]\n", Some("[members]\n")).is_empty());
    // The legacy `dialog` token is an advisor seat.
    assert_eq!(
        advisors_lost("[members]\ncarol = \"dialog\"\n", Some("[members]\n")),
        ["carol".to_string()]
    );
}

/// A pending roster change that keeps every advisor is committed as usual.
#[test]
fn bug_2209_sync_commit_keeps_a_roster_change_that_keeps_advisors() {
    let (_tmp, store) = store_with_roster();
    let grown = format!("{OPERATOR_ROSTER}alice = [\"implementer\"]\n");
    std::fs::write(store.join("registry/team.toml"), &grown).unwrap();
    assert!(protect_roster_advisors(&store).unwrap().is_empty());
    assert!(commit_pending_store_changes_on_host(&store).unwrap());
    assert!(run_git(&store, &["show", "HEAD:registry/team.toml"]).contains("alice"));
}
