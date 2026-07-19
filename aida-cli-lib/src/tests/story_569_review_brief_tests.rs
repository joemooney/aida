use super::*;

fn write_config(root: &std::path::Path, body: &str) {
    let dir = root.join(".aida");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.toml"), body).unwrap();
}

// trace:STORY-569 | ai:claude — default advisor; empty string disables;
/// explicit value wins; unparseable TOML falls back to the default.
#[test]
fn review_brief_agent_default_disable_and_override() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    // No config at all → the advisor default.
    assert_eq!(
        read_zen_review_brief_agent(root),
        Some("advisor".to_string())
    );

    // Config without the key → still the default.
    write_config(root, "[zen]\nauto_exit = true\n");
    assert_eq!(
        read_zen_review_brief_agent(root),
        Some("advisor".to_string())
    );

    // Explicit empty string → handoff disabled.
    write_config(root, "[zen]\nreview_brief_agent = \"\"\n");
    assert_eq!(read_zen_review_brief_agent(root), None);

    // Explicit target wins.
    write_config(root, "[zen]\nreview_brief_agent = \"codex\"\n");
    assert_eq!(read_zen_review_brief_agent(root), Some("codex".to_string()));

    // Unparseable TOML fails open to the default.
    write_config(root, "[zen\nnot toml");
    assert_eq!(
        read_zen_review_brief_agent(root),
        Some("advisor".to_string())
    );
}

// trace:STORY-569 | ai:claude — only an unacked `.md` counts as pending;
/// acked briefs (`.md.acked`) and other specs' briefs don't, and the
/// spec match is case-insensitive (brief filenames carry the canonical
/// uppercase id).
#[test]
fn pending_brief_exists_counts_only_unacked_md() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    // No mailbox dir at all.
    assert!(!pending_brief_exists(root, "advisor", "STORY-569"));

    let dir = root.join(".aida").join("agent-briefs").join("advisor");
    std::fs::create_dir_all(&dir).unwrap();

    // Acked brief only → not pending.
    std::fs::write(dir.join("STORY-569-20260612T000000Z.md.acked"), "x").unwrap();
    assert!(!pending_brief_exists(root, "advisor", "STORY-569"));

    // Another spec's pending brief → not ours.
    std::fs::write(dir.join("STORY-570-20260612T000000Z.md"), "x").unwrap();
    assert!(!pending_brief_exists(root, "advisor", "STORY-569"));

    // A pending brief for the spec → found, case-insensitively.
    std::fs::write(dir.join("STORY-569-20260612T000001Z.md"), "x").unwrap();
    assert!(pending_brief_exists(root, "advisor", "STORY-569"));
    assert!(pending_brief_exists(root, "advisor", "story-569"));
}

// trace:STORY-569 | ai:claude — a disabled target short-circuits before
/// any forge/store work, and an empty lease scope files nothing.
#[test]
fn file_zen_review_brief_short_circuits() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    let lease = |scope: &str| SessionLease {
        id: "zen001".into(),
        scope: scope.into(),
        slug: scope.to_lowercase(),
        owner: "tester".into(),
        worktree_path: root.join("wt"),
        branch: "story-569".into(),
        started_at: chrono::Utc::now(),
        hostname: "h".into(),
        role: Some("implementer".into()),
        creator_pid: None,
        active_pid: None,
        cargo_target_dir: None,
        parent_project_root: None,
        pr_head_sha: None,
        pr_base_sha: None,
        pr_base_ref: None,
        zen_intent_token: None,
        escalated_to_human: None,
        parent_branch: None,
        parent_branch_sha: None,
        review_verb: false,
        claim_verb: false,
    };

    // Handoff disabled → None, no side effects.
    write_config(root, "[zen]\nreview_brief_agent = \"\"\n");
    assert!(matches!(
        file_zen_review_brief(root, &lease("STORY-569")),
        Ok(None)
    ));

    // Empty scope → None (nothing to brief on).
    write_config(root, "[zen]\nreview_brief_agent = \"advisor\"\n");
    assert!(matches!(
        file_zen_review_brief(root, &lease("  ")),
        Ok(None)
    ));

    assert!(
        !root.join(".aida").join("agent-briefs").exists(),
        "short-circuit paths must not create mailbox state"
    );
}
