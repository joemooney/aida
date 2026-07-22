//! Unit tests for the worktree-scope guard's path-scoping check.
//!
//! The interesting cases are all path-resolution cases: symlinked worktree
//! roots, relative vs absolute inputs, and `..` traversal out of the worktree.
//! Each is exercised against a real temp filesystem so the symlink behaviour is
//! the real one, not a model of it.
//!
//! trace:TASK-1178 | ai:claude

use super::*;

/// `tempfile` hands back paths under `/tmp`, which on macOS is itself a symlink
/// (`/private/tmp`). Resolve once so the tests compare like with like.
fn canon(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

// -------------------------------------------------------------------------
// lexical_normalize
// -------------------------------------------------------------------------

#[test]
fn lexical_normalize_drops_cur_dir_and_resolves_parent() {
    assert_eq!(
        lexical_normalize(Path::new("a/./b/../c")),
        PathBuf::from("a/c")
    );
    assert_eq!(
        lexical_normalize(Path::new("/wt/sub/../other/file.rs")),
        PathBuf::from("/wt/other/file.rs")
    );
}

#[test]
fn lexical_normalize_preserves_escaping_leading_parent() {
    // A relative path that genuinely climbs out must stay climbing out —
    // silently swallowing the `..` would make a stray path look in-scope.
    assert_eq!(
        lexical_normalize(Path::new("../outside/file.rs")),
        PathBuf::from("../outside/file.rs")
    );
    assert_eq!(
        lexical_normalize(Path::new("a/../../up.rs")),
        PathBuf::from("../up.rs")
    );
    // `/..` is still the root, not an escape above it.
    assert_eq!(
        lexical_normalize(Path::new("/../etc")),
        PathBuf::from("/etc")
    );
}

// -------------------------------------------------------------------------
// stray_paths — the core check
// -------------------------------------------------------------------------

#[test]
fn in_worktree_commit_has_no_stray_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let wt = canon(tmp.path()).join("wt");
    std::fs::create_dir_all(wt.join("src")).unwrap();
    std::fs::write(wt.join("src/lib.rs"), "// x").unwrap();

    let staged = vec!["src/lib.rs".to_string(), "README.md".to_string()];
    // Committing FROM the session worktree: everything is in scope.
    assert!(stray_paths(&wt, &wt, &staged).is_empty());
}

#[test]
fn commit_in_the_shared_checkout_flags_every_staged_path() {
    // The exact failure this guard exists for: the session is scoped to `wt`,
    // but the edits (and therefore the commit) landed in the shared checkout.
    let tmp = tempfile::tempdir().unwrap();
    let root = canon(tmp.path());
    let wt = root.join("wt");
    let shared = root.join("main");
    std::fs::create_dir_all(wt.join("src")).unwrap();
    std::fs::create_dir_all(shared.join("src")).unwrap();
    std::fs::write(shared.join("src/lib.rs"), "// x").unwrap();

    let staged = vec!["src/lib.rs".to_string(), "src/other.rs".to_string()];
    let stray = stray_paths(&wt, &shared, &staged);
    assert_eq!(stray, staged, "every path in the shared checkout is stray");
}

#[test]
fn stray_paths_returns_the_original_staged_names_not_resolved_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let root = canon(tmp.path());
    let staged = vec!["aida-cli/src/main.rs".to_string()];
    let stray = stray_paths(&root.join("wt"), &root.join("main"), &staged);
    assert_eq!(stray, vec!["aida-cli/src/main.rs".to_string()]);
}

#[test]
fn deleted_staged_file_still_scopes_correctly() {
    // A staged DELETION names a path that no longer exists on disk — resolution
    // must fall back to the deepest existing ancestor rather than giving up.
    let tmp = tempfile::tempdir().unwrap();
    let root = canon(tmp.path());
    let wt = root.join("wt");
    let shared = root.join("main");
    std::fs::create_dir_all(wt.join("src")).unwrap();
    std::fs::create_dir_all(shared.join("src")).unwrap();

    let staged = vec!["src/gone.rs".to_string()];
    assert!(stray_paths(&wt, &wt, &staged).is_empty());
    assert_eq!(stray_paths(&wt, &shared, &staged).len(), 1);
}

#[test]
fn parent_traversal_out_of_the_worktree_is_stray() {
    let tmp = tempfile::tempdir().unwrap();
    let root = canon(tmp.path());
    let wt = root.join("wt");
    std::fs::create_dir_all(wt.join("src")).unwrap();
    std::fs::create_dir_all(root.join("outside")).unwrap();

    let staged = vec![
        "src/../src/in.rs".to_string(),       // traverses, stays inside
        "../outside/stray.rs".to_string(),    // climbs out
        "src/../../outside/b.rs".to_string(), // climbs out the long way
    ];
    let stray = stray_paths(&wt, &wt, &staged);
    assert_eq!(
        stray,
        vec![
            "../outside/stray.rs".to_string(),
            "src/../../outside/b.rs".to_string()
        ]
    );
}

#[test]
fn relative_and_absolute_repo_roots_agree() {
    let tmp = tempfile::tempdir().unwrap();
    let root = canon(tmp.path());
    let wt = root.join("wt");
    std::fs::create_dir_all(wt.join("src")).unwrap();

    // An absolute staged name (git never emits one, but a caller might) is used
    // as-is instead of being joined onto the repo root.
    let abs = wt.join("src/lib.rs").display().to_string();
    assert!(stray_paths(&wt, &wt, &[abs]).is_empty());

    let abs_outside = root.join("elsewhere/x.rs").display().to_string();
    assert_eq!(stray_paths(&wt, &wt, &[abs_outside]).len(), 1);

    // A repo root with redundant components resolves to the same scope.
    let noisy = root.join("wt/./src/..");
    assert!(stray_paths(&wt, &noisy, &["src/lib.rs".to_string()]).is_empty());
}

#[cfg(unix)]
#[test]
fn symlinked_worktree_path_is_recognized_as_in_scope() {
    // The session lease may record the worktree through a symlink (a symlinked
    // ~/ai tree, /tmp on macOS, a convenience alias). Files under the REAL path
    // must not be reported as stray just because the two spellings differ.
    let tmp = tempfile::tempdir().unwrap();
    let root = canon(tmp.path());
    let real = root.join("real-wt");
    std::fs::create_dir_all(real.join("src")).unwrap();
    std::fs::write(real.join("src/lib.rs"), "// x").unwrap();
    let link = root.join("linked-wt");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let staged = vec!["src/lib.rs".to_string()];
    // Lease says the symlink, commit happens in the real path…
    assert!(stray_paths(&link, &real, &staged).is_empty());
    // …and the other way around.
    assert!(stray_paths(&real, &link, &staged).is_empty());
}

#[cfg(unix)]
#[test]
fn symlink_out_of_the_worktree_is_still_stray() {
    // A path INSIDE the worktree that is a symlink to somewhere outside it
    // resolves outside — the guard reports the real destination, not the
    // in-worktree spelling.
    let tmp = tempfile::tempdir().unwrap();
    let root = canon(tmp.path());
    let wt = root.join("wt");
    let outside = root.join("outside");
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("x.rs"), "// x").unwrap();
    std::os::unix::fs::symlink(&outside, wt.join("escape")).unwrap();

    let stray = stray_paths(&wt, &wt, &["escape/x.rs".to_string()]);
    assert_eq!(stray, vec!["escape/x.rs".to_string()]);
}

#[test]
fn is_within_matches_the_worktree_root_itself() {
    let base = Path::new("/a/b");
    assert!(is_within(base, Path::new("/a/b")));
    assert!(is_within(base, Path::new("/a/b/c")));
    assert!(!is_within(base, Path::new("/a/bc")));
    assert!(!is_within(base, Path::new("/a")));
}

// -------------------------------------------------------------------------
// Warning rendering
// -------------------------------------------------------------------------

#[test]
fn no_warning_when_nothing_is_stray() {
    assert!(format_warning(Path::new("/wt"), Path::new("/wt"), &[]).is_none());
}

#[test]
fn warning_names_the_stray_paths_and_both_roots() {
    let body = format_warning(
        Path::new("/home/u/wt"),
        Path::new("/home/u/main"),
        &["src/a.rs".to_string(), "src/b.rs".to_string()],
    )
    .expect("stray paths must produce a warning");
    assert!(body.contains("/home/u/wt"), "{body}");
    assert!(body.contains("/home/u/main"), "{body}");
    assert!(body.contains("src/a.rs"), "{body}");
    assert!(body.contains("src/b.rs"), "{body}");
    assert!(body.contains("2 path(s)"), "{body}");
    // Warn-only is part of the message, so nobody reads it as a block.
    assert!(body.contains("Warn-only"), "{body}");
}

#[test]
fn warning_truncates_a_very_large_stray_set() {
    let staged: Vec<String> = (0..25).map(|i| format!("src/f{i}.rs")).collect();
    let body = format_warning(Path::new("/wt"), Path::new("/main"), &staged).unwrap();
    assert!(body.contains("25 path(s)"), "{body}");
    assert!(body.contains("src/f0.rs"), "{body}");
    assert!(!body.contains("src/f20.rs"), "{body}");
    assert!(body.contains("and 15 more"), "{body}");
}

#[test]
fn warning_text_carries_no_spec_id() {
    // User-facing stderr: a SPEC-ID here is opaque noise to a first user.
    let body = format_warning(Path::new("/wt"), Path::new("/main"), &["a.rs".to_string()]).unwrap();
    let re = regex::Regex::new(r"\b(STORY|TASK|BUG|EPIC|SPIKE|FR|CR|ADR|PRIN)-[0-9]+\b").unwrap();
    assert!(!re.is_match(&body), "{body}");
}

// -------------------------------------------------------------------------
// Lease resolution
// -------------------------------------------------------------------------

#[test]
fn lease_stem_matches_on_either_prefix_direction() {
    assert!(lease_stem_matches("019f76831234", "019f7683"));
    assert!(lease_stem_matches("019f7683", "019f76831234"));
    assert!(lease_stem_matches("019f7683", "019f7683"));
    assert!(!lease_stem_matches("019f7683", "abcd1234"));
    assert!(!lease_stem_matches("019f7683", ""));
    assert!(!lease_stem_matches("", "019f7683"));
}

#[test]
fn lease_file_yields_its_worktree_path() {
    let tmp = tempfile::tempdir().unwrap();
    let lease = tmp.path().join("019f7683.toml");
    std::fs::write(
        &lease,
        "id = \"019f7683\"\nscope = \"TASK-1\"\nworktree_path = \"/home/u/wt\"\n",
    )
    .unwrap();
    assert_eq!(
        worktree_from_lease_file(&lease),
        Some(PathBuf::from("/home/u/wt"))
    );
}

#[test]
fn worktree_less_advisory_lease_yields_nothing() {
    // A review/claim lease writes an empty worktree_path by convention — it
    // scopes no filesystem region, so the gate must stay out of the way.
    let tmp = tempfile::tempdir().unwrap();
    let lease = tmp.path().join("019f7683.toml");
    std::fs::write(
        &lease,
        "id = \"019f7683\"\nscope = \"PR-12\"\nworktree_path = \"\"\nreview_verb = true\n",
    )
    .unwrap();
    assert_eq!(worktree_from_lease_file(&lease), None);

    // …and so must a lease with no worktree_path key at all.
    let bare = tmp.path().join("019f0000.toml");
    std::fs::write(&bare, "id = \"019f0000\"\nscope = \"PR-12\"\n").unwrap();
    assert_eq!(worktree_from_lease_file(&bare), None);
}

#[test]
fn malformed_lease_file_is_ignored_not_fatal() {
    let tmp = tempfile::tempdir().unwrap();
    let lease = tmp.path().join("019f7683.toml");
    std::fs::write(&lease, "this is not toml = = =").unwrap();
    assert_eq!(worktree_from_lease_file(&lease), None);
    assert_eq!(
        worktree_from_lease_file(&tmp.path().join("nope.toml")),
        None
    );
}

#[test]
fn verdict_is_silent_for_a_non_scoped_session() {
    // The acceptance floor: a session with no worktree scope sees NO behaviour
    // change — even when it stages paths that would be stray for someone else.
    assert!(verdict(None, Path::new("/anywhere"), &["src/a.rs".to_string()]).is_none());
    // Nothing staged is likewise nothing to say.
    assert!(verdict(Some(Path::new("/wt")), Path::new("/main"), &[]).is_none());
}

#[test]
fn verdict_is_silent_in_worktree_and_speaks_out_of_worktree() {
    let tmp = tempfile::tempdir().unwrap();
    let root = canon(tmp.path());
    let wt = root.join("wt");
    let shared = root.join("main");
    std::fs::create_dir_all(wt.join("src")).unwrap();
    std::fs::create_dir_all(shared.join("src")).unwrap();
    let staged = vec!["src/lib.rs".to_string()];

    assert!(verdict(Some(&wt), &wt, &staged).is_none());
    let body = verdict(Some(&wt), &shared, &staged).expect("stray commit must warn");
    assert!(body.contains("src/lib.rs"), "{body}");
}

#[test]
fn session_worktree_resolves_from_the_lease_file_env_pointer() {
    let tmp = tempfile::tempdir().unwrap();
    let lease = tmp.path().join("019f7683.toml");
    std::fs::write(
        &lease,
        "id = \"019f7683\"\nscope = \"TASK-1\"\nworktree_path = \"/home/u/wt\"\n",
    )
    .unwrap();
    let _env =
        crate::test_env::EnvVarsGuard::set(&[("AIDA_WT_LEASE_FILE", lease.to_str().unwrap())]);
    assert_eq!(
        session_worktree(tmp.path()),
        Some(PathBuf::from("/home/u/wt"))
    );
}

#[test]
fn session_worktree_resolves_from_the_session_id_against_the_lease_store() {
    // The stray case: the commit is happening in a checkout whose own
    // `.aida/sessions/` holds the lease, matched by the id the session carries.
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join(".aida").join("sessions");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("019f7683abcd.toml"),
        "id = \"019f7683abcd\"\nscope = \"TASK-1\"\nworktree_path = \"/home/u/wt\"\n",
    )
    .unwrap();
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_WT_LEASE_FILE", None),
        ("AIDA_SESSION_ID", Some("019f7683")),
    ]);
    assert_eq!(
        session_worktree(tmp.path()),
        Some(PathBuf::from("/home/u/wt"))
    );
}

#[test]
fn session_worktree_is_none_without_a_session() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_WT_LEASE_FILE", None),
        ("AIDA_SESSION_ID", None),
    ]);
    assert_eq!(session_worktree(tmp.path()), None);
}

#[test]
fn leases_dir_lookup_picks_the_matching_session() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("sessions");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("aaaa1111.toml"),
        "id = \"aaaa1111\"\nscope = \"TASK-1\"\nworktree_path = \"/wt/one\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("bbbb2222.toml"),
        "id = \"bbbb2222\"\nscope = \"TASK-2\"\nworktree_path = \"/wt/two\"\n",
    )
    .unwrap();
    // A non-lease file in the same dir must be skipped.
    std::fs::write(dir.join("notes.txt"), "ignore me").unwrap();

    assert_eq!(
        worktree_from_leases_dir(&dir, "bbbb"),
        Some(PathBuf::from("/wt/two"))
    );
    assert_eq!(worktree_from_leases_dir(&dir, "cccc"), None);
    assert_eq!(
        worktree_from_leases_dir(&tmp.path().join("missing"), "aaaa"),
        None
    );
}
