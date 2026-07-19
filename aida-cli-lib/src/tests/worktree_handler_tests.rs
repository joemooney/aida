use super::*;

fn git(path: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("git {:?} failed to spawn: {e}", args));
    assert!(
        out.status.success(),
        "git {:?} failed\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn init_main_repo(path: &std::path::Path) {
    git(path, &["init", "-b", "main", "--quiet"]);
    git(path, &["config", "user.email", "aida@example.test"]);
    git(path, &["config", "user.name", "AIDA Test"]);
    git(path, &["commit", "--allow-empty", "-m", "base", "--quiet"]);
}

// The single shell line `worktree enter` emits — `cd '<escaped>'` — is the
// contract the `aida()` wrapper auto-evals; assert its exact shape.
#[test]
fn enter_cd_line_is_single_quoted_cd() {
    let line = enter_cd_line(std::path::Path::new("/home/joe/ai/aida-epic54"));
    assert_eq!(line, "cd '/home/joe/ai/aida-epic54'");
}

#[test]
fn enter_cd_line_escapes_apostrophes() {
    let line = enter_cd_line(std::path::Path::new("/tmp/o'brien/aida-epic1"));
    // The path's apostrophe must be escaped so the eval'd `cd` is one word.
    assert_eq!(line, "cd '/tmp/o'\\''brien/aida-epic1'");
}

#[test]
fn add_creates_worktree_with_focus_then_is_idempotent() {
    let repo = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    init_main_repo(repo.path());

    // First add: creates the worktree, branch, and focus marker.
    let out = ensure_epic_worktree_core(repo.path(), home.path(), "EPIC-54", None, None)
        .expect("first add succeeds");
    assert!(out.created, "first add reports created");
    assert_eq!(out.branch, "epic-54-work");
    assert_eq!(out.focus, "EPIC-54");
    assert_eq!(out.path, home.path().join("ai").join("aida-epic54"));
    assert!(out.path.exists(), "worktree dir exists");

    // Focus marker written INSIDE the new worktree.
    let focus = crate::focus::read_focus_marker(&out.path);
    assert_eq!(focus.as_deref(), Some("EPIC-54"));

    // The new worktree is on the derived branch.
    let head = std::process::Command::new("git")
        .arg("-C")
        .arg(&out.path)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), "epic-54-work");

    // Second add: idempotent — reports already-exists, focus re-affirmed.
    let again = ensure_epic_worktree_core(repo.path(), home.path(), "EPIC-54", None, None)
        .expect("second add succeeds");
    assert!(!again.created, "re-run reports not-created");
    assert_eq!(again.path, out.path);
    assert_eq!(
        crate::focus::read_focus_marker(&again.path).as_deref(),
        Some("EPIC-54")
    );
}

#[test]
fn add_honors_path_and_branch_overrides() {
    let repo = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    init_main_repo(repo.path());
    let custom = home.path().join("custom-tree");

    let out = ensure_epic_worktree_core(
        repo.path(),
        home.path(),
        "epic-7",
        Some(custom.to_str().unwrap()),
        Some("my-branch"),
    )
    .expect("override add succeeds");
    assert!(out.created);
    assert_eq!(out.path, custom);
    assert_eq!(out.branch, "my-branch");
    assert_eq!(out.focus, "EPIC-7");
    assert_eq!(
        crate::focus::read_focus_marker(&out.path).as_deref(),
        Some("EPIC-7")
    );
}

#[test]
fn list_branches_parse_pairs_path_to_short_branch() {
    let porcelain = "\
worktree /home/joe/ai/aida
HEAD aaa
branch refs/heads/main

worktree /home/joe/ai/aida-epic54
HEAD bbb
branch refs/heads/epic-54-work

worktree /home/joe/ai/aida-detached
HEAD ccc
detached
";
    let map = parse_worktree_branches(porcelain);
    assert_eq!(
        map.get(std::path::Path::new("/home/joe/ai/aida"))
            .map(String::as_str),
        Some("main")
    );
    assert_eq!(
        map.get(std::path::Path::new("/home/joe/ai/aida-epic54"))
            .map(String::as_str),
        Some("epic-54-work")
    );
    // The detached worktree has no branch line → absent from the map.
    assert!(!map.contains_key(std::path::Path::new("/home/joe/ai/aida-detached")));
}

// ── STORY-742: `aida worktree add|enter <spec>` (single-spec worktree) ──

fn mk_req(spec_id: &str, kind: RequirementType) -> aida_core::Requirement {
    let mut r = aida_core::Requirement::new(format!("req {spec_id}"), String::new());
    r.spec_id = Some(spec_id.to_string());
    r.req_type = kind;
    r
}

// The epic-vs-spec split: an EPIC (or an unresolved arg) keeps the legacy
// epic path; a resolved non-epic spec routes to the lease path with focus
// scoped to the spec itself.
#[test]
fn classify_target_routes_epic_vs_spec() {
    let epic = mk_req("EPIC-54", RequirementType::Epic);
    assert!(matches!(
        classify_worktree_target(Some(&epic), "EPIC-54"),
        WorktreeTarget::Epic
    ));

    let story = mk_req("STORY-742", RequirementType::Story);
    match classify_worktree_target(Some(&story), "story-742") {
        WorktreeTarget::Spec { display, focus } => {
            assert_eq!(display, "STORY-742", "canonical id resolved");
            assert_eq!(focus, "STORY-742", "focus scoped to the spec");
        }
        WorktreeTarget::Epic => panic!("a non-epic spec must take the spec path"),
    }

    // Unresolved arg → epic path (legacy behavior preserved).
    assert!(matches!(
        classify_worktree_target(None, "EPIC-99"),
        WorktreeTarget::Epic
    ));
}

// agreed_id wins over spec_id for the canonical display id.
#[test]
fn classify_target_prefers_agreed_id() {
    let mut story = mk_req("STORY-7-042", RequirementType::Story);
    story.agreed_id = Some("STORY-742".to_string());
    match classify_worktree_target(Some(&story), "STORY-7-042") {
        WorktreeTarget::Spec { display, focus } => {
            assert_eq!(display, "STORY-742");
            assert_eq!(focus, "STORY-742");
        }
        WorktreeTarget::Epic => panic!("resolved story must take the spec path"),
    }
}

// Fresh single-spec setup: mint runs, the outcome carries the epic-style
// path/branch and created=true, and focus is scoped to the spec. A second
// call with an existing lease re-enters WITHOUT minting.
#[test]
fn spec_worktree_core_mints_fresh_then_reenters_via_lease() {
    let home = tempfile::tempdir().unwrap();
    let expected_path = home.path().join("ai").join("aida-story742");

    let out = ensure_spec_worktree_core(
        home.path(),
        "STORY-742",
        "STORY-742",
        None,
        None,
        None,  // no existing lease
        false, // not a registered worktree
        |path, branch| {
            // The mint gets the epic-style path/branch the queue-work setup
            // would fork off origin/main.
            assert_eq!(path, home.path().join("ai").join("aida-story742"));
            assert_eq!(branch, "story-742-work");
            Ok((path.to_path_buf(), branch.to_string()))
        },
    )
    .expect("fresh spec setup succeeds");
    assert!(out.created, "fresh setup reports created");
    assert_eq!(out.path, expected_path);
    assert_eq!(out.branch, "story-742-work");
    assert_eq!(out.focus, "STORY-742");
    assert_eq!(
        crate::focus::read_focus_marker(&out.path).as_deref(),
        Some("STORY-742"),
        "focus marker written INSIDE the new worktree"
    );

    // Re-enter: a lease already covers the spec → mint MUST NOT run, the
    // outcome reports not-created at the lease's worktree path.
    let lease_path = home.path().join("ai").join("aida-story742");
    let again = ensure_spec_worktree_core(
        home.path(),
        "STORY-742",
        "STORY-742",
        None,
        None,
        Some((
            lease_path.clone(),
            "story-742-work".to_string(),
            "019f768atest".to_string(),
            true,
        )),
        false,
        |_p, _b| panic!("mint must NOT run when a lease already exists"),
    )
    .expect("re-enter succeeds");
    assert!(!again.created, "re-enter reports not-created");
    assert_eq!(again.path, lease_path);
    assert_eq!(again.branch, "story-742-work");
    assert_eq!(again.lease_id.as_deref(), Some("019f768atest"));
    assert!(again.has_session_env);
}

#[test]
fn enter_shell_payload_cd_then_sources_session_env() {
    let tree = tempfile::tempdir().unwrap();
    let aida_dir = tree.path().join(".aida");
    std::fs::create_dir_all(&aida_dir).unwrap();
    std::fs::write(
        aida_dir.join("session-env.sh"),
        "export CARGO_TARGET_DIR='/tmp/aida/target'\n",
    )
    .unwrap();

    let payload = enter_shell_payload(tree.path(), "STORY-742");
    assert!(payload.starts_with(&format!("{}\n", enter_cd_line(tree.path()))));
    assert!(payload.contains("export CARGO_TARGET_DIR='/tmp/aida/target'\n"));
    // TASK-1160: the payload also splices the ambient worktree PS1 segment.
    assert!(payload.contains("export AIDA_WT_PS1_PREFIX='(wt:STORY-742) '\n"));
    assert!(payload.contains("export PS1=\"$AIDA_WT_PS1_PREFIX$PS1\"\n"));
}

// TASK-1160: the enter payload splices the PS1 indicator even for a worktree
// with no session-env shim (epic-scoped trees), and the exit payload is the
// exact inverse contract: cd back + unset the session exports + strip PS1.
// trace:TASK-1160 | ai:claude
#[test]
fn enter_payload_without_session_env_still_splices_ps1() {
    let tree = tempfile::tempdir().unwrap();
    let payload = enter_shell_payload(tree.path(), "EPIC-54");
    assert!(payload.starts_with(&format!("{}\n", enter_cd_line(tree.path()))));
    assert!(payload.contains("export AIDA_WT_PS1_PREFIX='(wt:EPIC-54) '\n"));
}

// trace:TASK-1160 | ai:claude
#[test]
fn exit_payload_is_the_inverse_of_enter() {
    let main_root = std::path::Path::new("/home/joe/ai/aida");
    let payload = crate::worktree::exit_shell_payload(main_root, &["AIDA_AGENT_TYPE".to_string()]);
    assert!(payload.starts_with("cd '/home/joe/ai/aida'\n"));
    assert!(payload.contains("unset AIDA_SESSION_ID CARGO_TARGET_DIR AIDA_AGENT_TYPE\n"));
    assert!(payload.contains("unset AIDA_WT_PS1_PREFIX\n"));
    // Never re-exports anything: exit only removes state.
    assert!(!payload.contains("export "));
}

// A worktree already registered (git) at the default path but carrying no
// lease re-affirms focus without minting.
#[test]
fn spec_worktree_core_registered_without_lease_reaffirms() {
    let home = tempfile::tempdir().unwrap();
    let out = ensure_spec_worktree_core(
        home.path(),
        "TASK-100",
        "TASK-100",
        None,
        None,
        None, // no lease
        true, // but already a registered git worktree
        |_p, _b| panic!("mint must NOT run for an already-registered worktree"),
    )
    .expect("registered re-affirm succeeds");
    assert!(!out.created);
    assert_eq!(out.path, home.path().join("ai").join("aida-task100"));
    assert_eq!(out.branch, "task-100-work");
    assert_eq!(
        crate::focus::read_focus_marker(&out.path).as_deref(),
        Some("TASK-100")
    );
}

// --path / --branch overrides flow through to the mint and the outcome.
#[test]
fn spec_worktree_core_honors_path_and_branch_overrides() {
    let home = tempfile::tempdir().unwrap();
    let custom = home.path().join("custom-spec-tree");
    let out = ensure_spec_worktree_core(
        home.path(),
        "STORY-742",
        "STORY-742",
        Some(custom.to_str().unwrap()),
        Some("my-spec-branch"),
        None,
        false,
        |path, branch| {
            assert_eq!(path, custom.as_path());
            assert_eq!(branch, "my-spec-branch");
            Ok((path.to_path_buf(), branch.to_string()))
        },
    )
    .expect("override setup succeeds");
    assert!(out.created);
    assert_eq!(out.path, custom);
    assert_eq!(out.branch, "my-spec-branch");
    assert_eq!(out.focus, "STORY-742");
}
