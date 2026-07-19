// BUG-734: a worktree-less lease stores an empty `worktree_path`. Because
// `Path::starts_with` on an empty base matches EVERY path, feeding that empty
// path into the live-claude probes made every claude on the machine count as
// "inside" the blank worktree — so `aida session end` refused to end a lease
// that only needed its record deleted. The probes must treat an empty path as
// "no worktree ⇒ nothing inside it".
// trace:BUG-734 | ai:claude

use super::{probe_dangling_claudes_at_path, probe_live_claudes_in_worktree};
use std::path::Path;

// Documents the hazard the guard exists for: an empty base path is a prefix
// of every path, so an unguarded cwd scan matches everything.
// trace:BUG-734 | ai:claude
#[test]
fn empty_path_is_prefix_of_everything() {
    assert!(Path::new("/anywhere/at/all").starts_with(Path::new("")));
}

// With the guard, an empty worktree path yields no live matches regardless of
// how many claude processes are running on the host (pre-fix this returned
// every live claude on a dev machine). trace:BUG-734 | ai:claude
#[test]
fn live_probe_on_empty_worktree_path_matches_nothing() {
    assert!(probe_live_claudes_in_worktree(Path::new("")).is_empty());
}

// Same guard for the dangling-cwd probe — it shares the `starts_with` filter.
// trace:BUG-734 | ai:claude
#[test]
fn dangling_probe_on_empty_worktree_path_matches_nothing() {
    assert!(probe_dangling_claudes_at_path(Path::new("")).is_empty());
}
