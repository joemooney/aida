//! TASK-1543 — Reap polish:
//! 1. Where reap/teardown prunes a missing worktree, use 'git worktree remove --force -- <lease path>'
//!    (scoped to that entry) instead of a repo-wide 'git worktree prune', so another session's
//!    temporarily unavailable worktree keeps its registration.
//! 2. worktree_is_locked parses 'git worktree list --porcelain -z' so paths with newlines are detected.
//! 3. Fixture tests for both.
//
// trace:TASK-1543 | ai:antigravity

use super::*;
use std::path::{Path, PathBuf};

fn git(root: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit_file(root: &Path, file: &str, content: &str, message: &str) {
    std::fs::write(root.join(file), content).unwrap();
    git(root, &["add", file]);
    git(root, &["commit", "-q", "-m", message]);
}

fn fixture_lease(worktree: &Path, branch: &str, scope: &str) -> SessionLease {
    serde_json::from_value(serde_json::json!({
        "id": "t1543fixture",
        "scope": scope,
        "slug": scope.to_ascii_lowercase(),
        "owner": "test",
        "worktree_path": worktree,
        "branch": branch,
        "started_at": "2026-01-01T00:00:00Z",
        "hostname": "fixture",
    }))
    .expect("fixture lease parses")
}

fn create_fixture_repo() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.name", "Test"]);
    git(&root, &["config", "user.email", "test@example.com"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    commit_file(&root, "README.md", "base\n", "init");
    git(&root, &["branch", "-M", "main"]);
    (tmp, root)
}

/// Canonicalize a fixture path into the spelling git itself uses.
///
/// `canonicalize` is what resolves a Windows 8.3 short component (the
/// `C:\Users\RUNNER~1\...` temp root a GitHub Actions runner hands out) to the
/// long name git prints, so it stays. What it adds on Windows is the `\\?\`
/// verbatim prefix, which is not a spelling git emits and not one every git
/// subcommand accepts, so it is stripped again. Pure string work — no `cfg`
/// branch — and a no-op on Unix, where no fixture path starts with `\\?\`.
// trace:TASK-1543 | ai:claude
fn canonical_for_git(path: &Path) -> PathBuf {
    let canon = path.canonicalize().expect("fixture path canonicalizes");
    let text = canon.to_string_lossy().to_string();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    canon
}

/// Stable comparison key for a worktree path across the two spellings the
/// fixture and `git worktree list --porcelain` can each produce for the same
/// directory.
///
/// They genuinely differ on Windows: the fixture path descends from
/// `std::env::temp_dir()`, which on a GitHub Actions runner is the 8.3 short
/// form `C:\Users\RUNNER~1\AppData\Local\Temp`, and `std::fs::canonicalize`
/// rewrites that as the `\\?\`-prefixed verbatim form with `\` separators,
/// while git prints the resolved long name with `/` separators
/// (`C:/Users/runneradmin/AppData/Local/Temp/...`). macOS differs the same way
/// (`/var` vs `/private/var`). What is identical in every spelling is the tail
/// below the temp root — the tempdir name and the worktree directory name, both
/// created by this fixture and echoed back by git verbatim — so the key is those
/// last two components, separator- and case-normalised.
// trace:TASK-1543 | ai:claude
fn worktree_key(path: &Path) -> String {
    let parts: Vec<String> = path
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => Some(s.to_string_lossy().to_lowercase()),
            _ => None,
        })
        .collect();
    let skip = parts.len().saturating_sub(2);
    parts.into_iter().skip(skip).collect::<Vec<_>>().join("/")
}

/// Is `wt` registered in this `git worktree list --porcelain` output? Parses the
/// listing into records and compares by [`worktree_key`] rather than
/// substring-matching the raw text. Still exact about presence: a removed or
/// pruned worktree has no record at all, so its key is absent and the negative
/// assertions below keep their teeth.
// trace:TASK-1543 | ai:claude
fn listing_registers(listing: &str, wt: &Path) -> bool {
    let want = worktree_key(wt);
    aida_core::git_ops::parse_worktree_list_porcelain(listing)
        .iter()
        .any(|p| worktree_key(p) == want)
}

/// Acceptance 1: Where reap prunes a missing worktree, use
/// `git worktree remove --force -- <lease path>` scoped to that entry instead of
/// a repo-wide `git worktree prune`, so another session's temporarily
/// unavailable worktree keeps its registration.
// trace:TASK-1543 | ai:antigravity
#[test]
fn task_1543_reap_missing_worktree_scopes_removal_preserving_sibling_missing_worktree() {
    let (tmp, root) = create_fixture_repo();

    let wt_a = tmp.path().join("wt-spec-a");
    let wt_b = tmp.path().join("wt-spec-b");

    git(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "spec-a",
            wt_a.to_str().unwrap(),
        ],
    );
    git(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "spec-b",
            wt_b.to_str().unwrap(),
        ],
    );

    let wt_a = canonical_for_git(&wt_a);
    let wt_b = canonical_for_git(&wt_b);

    commit_file(&wt_a, "a.txt", "content-a\n", "feat(a): task a (TASK-1543)");
    commit_file(&wt_b, "b.txt", "content-b\n", "feat(b): task b (TASK-9999)");

    // Land spec-a on main via batched integration commit so reap can classify it landed.
    git(&root, &["checkout", "-q", "main"]);
    std::fs::write(root.join("a.txt"), "content-a\n").unwrap();
    git(&root, &["add", "a.txt"]);
    git(
        &root,
        &[
            "commit",
            "-q",
            "-m",
            "[AI:antigravity] chore(integrate): batch 1 (TASK-1543)",
            "-m",
            "Commit for spec a (TASK-1543)",
        ],
    );

    // Both worktree directories are removed by hand (e.g. unmounted / wiped).
    std::fs::remove_dir_all(&wt_a).unwrap();
    std::fs::remove_dir_all(&wt_b).unwrap();

    let initial_listing = git(&root, &["worktree", "list", "--porcelain"]);
    assert!(
        listing_registers(&initial_listing, &wt_a),
        "wt_a must start out registered:\n{initial_listing}"
    );
    assert!(
        listing_registers(&initial_listing, &wt_b),
        "wt_b must start out registered:\n{initial_listing}"
    );
    assert!(initial_listing.contains("prunable"));

    let (_facts, tip) = gather_merge_facts_pinned(
        &root,
        Some("main"),
        "spec-a",
        "TASK-1543",
        false,
        true,
        |_| false,
    );
    let lease = fixture_lease(&wt_a, "spec-a", "TASK-1543");
    let outcome = reap_one(&root, &lease, tip.as_deref());
    assert!(
        outcome.starts_with("reaped"),
        "expected reaped, got: {outcome}"
    );
    assert!(
        outcome.contains("deleted"),
        "expected branch deleted, got: {outcome}"
    );
    assert!(resolve_local_branch_tip(&root, "spec-a").is_none());

    // After reap of wt_a:
    // 1. wt_a registration is GONE.
    // 2. wt_b registration is PRESERVED (still in git worktree listing as prunable).
    let listing = git(&root, &["worktree", "list", "--porcelain"]);
    assert!(
        !listing_registers(&listing, &wt_a),
        "wt_a should have been removed from worktree list:\n{listing}"
    );
    assert!(
        listing_registers(&listing, &wt_b),
        "wt_b should have been preserved in worktree list:\n{listing}"
    );
    assert!(
        listing.contains("prunable"),
        "wt_b should still be marked prunable in worktree list:\n{listing}"
    );
}

/// A directory that reappears at the final absence-check boundary is preserved.
// trace:TASK-1543 | ai:codex
#[test]
fn task_1543_reap_preserves_worktree_directory_that_reappears_before_registration_clear() {
    let (tmp, root) = create_fixture_repo();
    let wt = tmp.path().join("wt-reappeared");
    git(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "spec-reappeared",
            wt.to_str().unwrap(),
        ],
    );
    let wt = canonical_for_git(&wt);
    commit_file(&wt, "landed.txt", "landed\n", "work");
    let gitfile = std::fs::read_to_string(wt.join(".git")).unwrap();
    git(&root, &["checkout", "-q", "main"]);
    git(&root, &["merge", "-q", "--squash", "spec-reappeared"]);
    git(&root, &["commit", "-q", "-m", "land work"]);
    std::fs::remove_dir_all(&wt).unwrap();

    let (_, tip) = gather_merge_facts_pinned(
        &root,
        Some("main"),
        "spec-reappeared",
        "TASK-1543",
        false,
        true,
        |_| false,
    );
    let lease = fixture_lease(&wt, "spec-reappeared", "TASK-1543");
    let outcome = reap_one_with_clear_hooks(
        &root,
        &lease,
        tip.as_deref(),
        || {
            std::fs::create_dir_all(&wt).unwrap();
            std::fs::write(wt.join(".git"), &gitfile).unwrap();
            std::fs::write(wt.join("precious.txt"), "preserve me").unwrap();
        },
        || {},
    );
    assert!(
        wt.join("precious.txt").exists(),
        "reappeared worktree content must remain after reap_one"
    );
    assert!(
        outcome.contains("worktree path reappeared"),
        "expected safe skip, got: {outcome}"
    );
}

/// Acceptance 1: teardown_worktree_path does not run repo-wide prune, preserving
/// another session's temporarily unavailable worktree.
// trace:TASK-1543 | ai:antigravity
#[test]
fn task_1543_teardown_does_not_prune_sibling_missing_worktree() {
    let (tmp, root) = create_fixture_repo();

    let wt_1 = tmp.path().join("wt-1");
    let wt_2 = tmp.path().join("wt-2");

    git(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "branch-1",
            wt_1.to_str().unwrap(),
        ],
    );
    git(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "branch-2",
            wt_2.to_str().unwrap(),
        ],
    );

    let wt_1 = canonical_for_git(&wt_1);
    let wt_2 = canonical_for_git(&wt_2);

    // Directory of wt_2 is removed (temporarily unavailable).
    std::fs::remove_dir_all(&wt_2).unwrap();

    // Now teardown wt_1 (which still exists on disk).
    let ok = aida_core::worktree_pool_destroy::teardown_worktree_path(&root, &wt_1, &[]).unwrap();
    assert!(ok);
    assert!(!wt_1.exists());

    // wt_2 must NOT have been pruned by teardown of wt_1!
    let listing = git(&root, &["worktree", "list", "--porcelain"]);
    assert!(
        !listing_registers(&listing, &wt_1),
        "wt_1 must be removed:\n{listing}"
    );
    assert!(
        listing_registers(&listing, &wt_2),
        "wt_2 must be preserved:\n{listing}"
    );
    assert!(
        listing.contains("prunable"),
        "wt_2 must still be listed as prunable:\n{listing}"
    );
}

/// Acceptance 2, end-to-end against real git: worktree_is_locked parses
/// `git worktree list --porcelain -z` so paths with newlines are detected
/// properly when locked or unlocked.
///
/// Unix only, and not because the assertions are weaker elsewhere: a newline is
/// not a legal character in a Windows path, so `git worktree add` there cannot
/// even create the fixture ("could not create leading directories ... Invalid
/// argument"). The parser half of the behaviour is covered on every platform by
/// `task_1543_porcelain_z_lock_parser_keeps_newline_paths_in_one_field` below.
// trace:TASK-1543 | ai:antigravity
// trace:TASK-1543 | ai:claude
#[cfg(unix)]
#[test]
fn task_1543_worktree_is_locked_detects_newline_in_path() {
    let (tmp, root) = create_fixture_repo();

    // Worktree path containing newlines.
    let wt_nl = tmp.path().join("wt\nwith\nnewlines");
    let wt_other = tmp.path().join("wt_other");

    git(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "b_nl",
            wt_nl.to_str().unwrap(),
        ],
    );
    git(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "b_other",
            wt_other.to_str().unwrap(),
        ],
    );

    let wt_nl = canonical_for_git(&wt_nl);
    let wt_other = canonical_for_git(&wt_other);

    // Initially neither is locked.
    assert!(
        !crate::worktree_is_locked(&root, &wt_nl),
        "unlocked newline worktree should not report locked"
    );
    assert!(
        !crate::worktree_is_locked(&root, &wt_other),
        "unlocked regular worktree should not report locked"
    );

    // Lock wt_nl with a reason.
    git(
        &root,
        &[
            "worktree",
            "lock",
            "--reason",
            "testing newline locks",
            wt_nl.to_str().unwrap(),
        ],
    );
    assert!(
        crate::worktree_is_locked(&root, &wt_nl),
        "locked newline worktree should report locked"
    );
    assert!(
        !crate::worktree_is_locked(&root, &wt_other),
        "unlocked sibling should not report locked"
    );

    // Unlock wt_nl.
    git(&root, &["worktree", "unlock", wt_nl.to_str().unwrap()]);
    assert!(
        !crate::worktree_is_locked(&root, &wt_nl),
        "unlocked newline worktree should not report locked"
    );

    // Lock wt_other without reason (bare locked).
    git(&root, &["worktree", "lock", wt_other.to_str().unwrap()]);
    assert!(
        !crate::worktree_is_locked(&root, &wt_nl),
        "newline worktree should remain unlocked"
    );
    assert!(
        crate::worktree_is_locked(&root, &wt_other),
        "regular worktree should report locked"
    );

    // Lock wt_nl bare as well.
    git(&root, &["worktree", "lock", wt_nl.to_str().unwrap()]);
    assert!(
        crate::worktree_is_locked(&root, &wt_nl),
        "bare locked newline worktree should report locked"
    );
    assert!(
        crate::worktree_is_locked(&root, &wt_other),
        "regular worktree should still report locked"
    );

    // Non-existent repo returns false.
    let bogus = tmp.path().join("does_not_exist");
    assert!(!crate::worktree_is_locked(&bogus, &wt_nl));
}

/// Acceptance 2, platform-independent half: the NUL framing of
/// `git worktree list --porcelain -z` keeps a worktree path containing newlines
/// in a single field and attributes `locked` to the right record. Runs on every
/// platform — including Windows, where such a path cannot exist on disk —
/// because it feeds the pure parser a recorded porcelain stream instead of
/// asking the filesystem for an illegal path.
// trace:TASK-1543 | ai:claude
#[test]
fn task_1543_porcelain_z_lock_parser_keeps_newline_paths_in_one_field() {
    // Byte-for-byte the shape git emits: NUL after every attribute, plus one
    // extra NUL closing each record.
    let stream = concat!(
        "worktree /fixture/repo\0",
        "HEAD 1111111111111111111111111111111111111111\0",
        "branch refs/heads/main\0",
        "\0",
        "worktree /fixture/wt\nwith\nnewlines\0",
        "HEAD 2222222222222222222222222222222222222222\0",
        "branch refs/heads/b_nl\0",
        "locked testing newline locks\0",
        "\0",
        "worktree /fixture/wt_other\0",
        "HEAD 3333333333333333333333333333333333333333\0",
        "branch refs/heads/b_other\0",
        "\0",
    );

    let states = crate::parse_worktree_lock_states_z(stream);
    // Three records, not five: the newline path was NOT split into extra
    // entries, which is exactly what a line-oriented parse got wrong.
    assert_eq!(
        states,
        vec![
            (PathBuf::from("/fixture/repo"), false),
            (PathBuf::from("/fixture/wt\nwith\nnewlines"), true),
            (PathBuf::from("/fixture/wt_other"), false),
        ],
        "only the newline-path record is locked, and it stays one record"
    );

    // A bare `locked` with no reason counts as locked.
    let bare = concat!(
        "worktree /fixture/wt_other\0",
        "HEAD 3333333333333333333333333333333333333333\0",
        "locked\0",
        "\0",
    );
    assert_eq!(
        crate::parse_worktree_lock_states_z(bare),
        vec![(PathBuf::from("/fixture/wt_other"), true)],
        "bare `locked` (no reason) must be detected"
    );

    // A `locked` field after the record terminator belongs to nobody.
    let stray = concat!("worktree /fixture/wt_other\0", "\0", "locked\0", "\0");
    assert_eq!(
        crate::parse_worktree_lock_states_z(stray),
        vec![(PathBuf::from("/fixture/wt_other"), false)],
        "a lock attribute outside a record must not leak onto the previous one"
    );

    assert!(crate::parse_worktree_lock_states_z("").is_empty());
}
