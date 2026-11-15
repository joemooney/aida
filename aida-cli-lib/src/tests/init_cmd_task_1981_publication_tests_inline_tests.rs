use super::*;
fn capture_snapshot(store: &std::path::Path) -> Result<(String, String)> {
    commit_initial_store(store)
}
fn publish_snapshot(store: &std::path::Path, branch: &str, sha: &str) -> Result<bool> {
    match publish_initial_store_snapshot(store, branch, sha)? {
        InitialStorePublication::Pushed => Ok(false),
        InitialStorePublication::Rebased => Ok(true),
        InitialStorePublication::Deferred(e) => Err(e),
    }
}
fn retry_snapshot(store: &std::path::Path, branch: &str) -> Result<String> {
    rebase_initial_store_snapshot(store, branch)
}
include!("task_1981_publication_tests.rs");

// trace:TASK-1981 | ai:codex
#[test]
fn task_1981_init_initial_outage_retains_offline_deferred_result() {
    let fixture = Fixture::new();
    let (branch, captured) = commit_initial_store(&fixture.store).unwrap();
    git_at(
        &fixture.store,
        &[
            "remote",
            "set-url",
            "origin",
            "/nonexistent/task-1981-origin",
        ],
    );
    match publish_initial_store_snapshot(&fixture.store, &branch, &captured).unwrap() {
        InitialStorePublication::Deferred(error) => {
            assert!(
                format!("{error:#}").contains("initial store push failed"),
                "{error:#}"
            );
        }
        _ => panic!("unreachable initial remote must remain deferred"),
    }
    assert_eq!(git_ops::head_sha(&fixture.store).unwrap(), captured);
}
