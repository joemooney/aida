use super::*;

/// BUG-87: full composition matrix. `--for X` always wins over `--all`;
/// `--for any` selects unrouted; `--all` only suppresses the
// active-session-role default. trace:BUG-87 | ai:claude
// why: the inline case-matrix tuple documents the test's input/expected columns; an alias would scatter that legend away from the data.
#[allow(clippy::type_complexity)]
#[test]
fn resolve_role_filter_composition_matrix() {
    // (role, all, session_role) -> (role_filter, only_unrouted)
    let cases: &[(Option<&str>, bool, Option<&str>, Option<&str>, bool)] = &[
        // --for X (X != "any") wins over --all and session.
        (
            Some("reviewer"),
            true,
            Some("implementer"),
            Some("reviewer"),
            false,
        ),
        (
            Some("reviewer"),
            false,
            Some("implementer"),
            Some("reviewer"),
            false,
        ),
        (Some("reviewer"), true, None, Some("reviewer"), false),
        (Some("reviewer"), false, None, Some("reviewer"), false),
        // --for any → only_unrouted=true, regardless of --all/session.
        (Some("any"), true, Some("implementer"), None, true),
        (Some("any"), false, Some("implementer"), None, true),
        (Some("any"), false, None, None, true),
        // --for any is case-insensitive.
        (Some("ANY"), false, Some("implementer"), None, true),
        // --all alone: no filter (override session default).
        (None, true, Some("implementer"), None, false),
        (None, true, None, None, false),
        // No flags: inherit session role.
        (None, false, Some("implementer"), Some("implementer"), false),
        // No flags, no session: no filter.
        (None, false, None, None, false),
        // Empty session var treated as "no session role".
        (None, false, Some(""), None, false),
    ];
    for (role, all, session, want_role, want_unrouted) in cases {
        let (got_role, got_unrouted) = resolve_queue_role_filter(*role, *all, *session);
        assert_eq!(
            (got_role.as_deref(), got_unrouted),
            (*want_role, *want_unrouted),
            "case role={:?} all={} session={:?}",
            role,
            all,
            session
        );
    }
}

/// BUG-87: predicate honors only_unrouted before falling through
// to the role-equality check. trace:BUG-87 | ai:claude
#[test]
fn entry_matches_filter_branches() {
    // only_unrouted: only entries with for_role==None pass.
    assert!(entry_matches_role_filter(None, None, true));
    assert!(!entry_matches_role_filter(Some("reviewer"), None, true));
    assert!(!entry_matches_role_filter(Some("implementer"), None, true));

    // Routed filter: exact match required.
    assert!(entry_matches_role_filter(
        Some("reviewer"),
        Some("reviewer"),
        false
    ));
    assert!(!entry_matches_role_filter(
        Some("implementer"),
        Some("reviewer"),
        false
    ));
    assert!(!entry_matches_role_filter(None, Some("reviewer"), false));

    // No filter, no unrouted-only: everything passes.
    assert!(entry_matches_role_filter(None, None, false));
    assert!(entry_matches_role_filter(Some("anything"), None, false));

    // STORY-718: `queue list --role integrator` routes exactly like the
    // other agent-wired roles — an integrator-routed entry matches the
    // integrator filter and nothing else does.
    assert!(entry_matches_role_filter(
        Some("integrator"),
        Some("integrator"),
        false
    ));
    assert!(!entry_matches_role_filter(
        Some("implementer"),
        Some("integrator"),
        false
    ));
    assert!(!entry_matches_role_filter(
        Some("integrator"),
        Some("reviewer"),
        false
    ));
}

/// BUG-87: the original incident. `--all --for reviewer` against a
/// queue of mixed-role items returns ONLY reviewer-routed items.
// trace:BUG-87 | ai:claude
#[test]
fn all_and_for_reviewer_returns_only_reviewer_routed() {
    let entries: Vec<Option<&str>> = vec![
        Some("implementer"),
        Some("reviewer"),
        Some("implementer"),
        None,
        Some("reviewer"),
        Some("triage"),
    ];
    // Simulate `aida queue list --all --for reviewer` from an
    // implementer session.
    let (role_filter, only_unrouted) =
        resolve_queue_role_filter(Some("reviewer"), true, Some("implementer"));
    let kept: Vec<Option<&str>> = entries
        .iter()
        .copied()
        .filter(|fr| entry_matches_role_filter(*fr, role_filter.as_deref(), only_unrouted))
        .collect();
    assert_eq!(kept, vec![Some("reviewer"), Some("reviewer")]);
}

/// BUG-87: `--all --for any` returns ONLY unrouted items.
// trace:BUG-87 | ai:claude
#[test]
fn all_and_for_any_returns_only_unrouted() {
    let entries: Vec<Option<&str>> = vec![
        Some("implementer"),
        None,
        Some("reviewer"),
        None,
        Some("triage"),
    ];
    let (role_filter, only_unrouted) =
        resolve_queue_role_filter(Some("any"), true, Some("implementer"));
    let kept: Vec<Option<&str>> = entries
        .iter()
        .copied()
        .filter(|fr| entry_matches_role_filter(*fr, role_filter.as_deref(), only_unrouted))
        .collect();
    assert_eq!(kept, vec![None, None]);
}

/// BUG-527: the `Queued:` value formatter — empty → None (line omitted),
/// unrouted → `general`, multiple memberships joined.
// trace:BUG-527 | ai:claude
#[test]
fn format_queue_membership_shapes() {
    assert_eq!(format_queue_membership(&[]), None);
    assert_eq!(
        format_queue_membership(&[(Some("implementer".to_string()), 2)]).as_deref(),
        Some("implementer (pos 2)")
    );
    assert_eq!(
        format_queue_membership(&[(None, 1)]).as_deref(),
        Some("general (pos 1)")
    );
    assert_eq!(
        format_queue_membership(&[
            (Some("implementer".to_string()), 2),
            (Some("reviewer".to_string()), 1),
        ])
        .as_deref(),
        Some("implementer (pos 2), reviewer (pos 1)")
    );
}

/// BUG-527: `queue_memberships_for` ranks per-role within a user's queue
/// by raw position (1-based), surfaces the matching spec only, and
// returns empty for a spec in no queue. trace:BUG-527 | ai:claude
#[test]
fn queue_memberships_for_ranks_and_filters() {
    let tmp = tempfile::tempdir().unwrap();
    let project_root = tmp.path();
    let qdir = project_root.join(".aida-store/registry/queues");
    std::fs::create_dir_all(&qdir).unwrap();

    let target = Uuid::new_v4();
    let other = Uuid::new_v4();
    let mk = |req: Uuid, pos: i64, role: Option<&str>| aida_core::QueueEntry {
        user_id: "alice".to_string(),
        requirement_id: req,
        position: pos,
        added_by: "alice".to_string(),
        note: None,
        added_at: chrono::Utc::now(),
        for_role: role.map(str::to_string),
        for_scope: None,
        for_session: None,
        added_by_machine: None,
    };
    // implementer queue: other(pos 1000) then target(pos 2000) → target rank 2.
    // reviewer queue: target(pos 500) alone → rank 1.
    let entries = vec![
        mk(other, 1000, Some("implementer")),
        mk(target, 2000, Some("implementer")),
        mk(target, 500, Some("reviewer")),
    ];
    std::fs::write(
        qdir.join("alice.yaml"),
        serde_yaml::to_string(&entries).unwrap(),
    )
    .unwrap();

    let memberships = queue_memberships_for(project_root, &target);
    // Sorted: routed roles alphabetically → implementer, reviewer.
    assert_eq!(
        memberships,
        vec![
            (Some("implementer".to_string()), 2),
            (Some("reviewer".to_string()), 1)
        ]
    );

    // A spec in no queue → empty (caller omits the line).
    let absent = Uuid::new_v4();
    assert!(queue_memberships_for(project_root, &absent).is_empty());
}

/// BUG-87: no regression — bare `aida queue list` in an implementer
/// session still filters to implementer-routed items.
// trace:BUG-87 | ai:claude
#[test]
fn default_session_role_filter_preserved() {
    let entries: Vec<Option<&str>> = vec![
        Some("implementer"),
        Some("reviewer"),
        None,
        Some("implementer"),
    ];
    let (role_filter, only_unrouted) = resolve_queue_role_filter(None, false, Some("implementer"));
    let kept: Vec<Option<&str>> = entries
        .iter()
        .copied()
        .filter(|fr| entry_matches_role_filter(*fr, role_filter.as_deref(), only_unrouted))
        .collect();
    assert_eq!(kept, vec![Some("implementer"), Some("implementer")]);
}

/// TASK-747: `human` is a first-class route target. `--role Human`
/// canonicalizes to lowercase `human` and matches `--for human` entries
/// (written canonical on add), symmetric with `dialog`→`advisor`.
// trace:TASK-747 | ai:claude
#[test]
fn human_route_filter_matches_canonical_entries() {
    let entries: Vec<Option<&str>> = vec![
        Some("human"),
        Some("implementer"),
        Some("human"),
        None,
        Some("advisor"),
    ];
    // `aida queue list --role Human` (mixed casing) → canonical `human`.
    let (role_filter, only_unrouted) = resolve_queue_role_filter(Some("Human"), true, None);
    assert_eq!(role_filter.as_deref(), Some("human"));
    let kept: Vec<Option<&str>> = entries
        .iter()
        .copied()
        .filter(|fr| entry_matches_role_filter(*fr, role_filter.as_deref(), only_unrouted))
        .collect();
    assert_eq!(kept, vec![Some("human"), Some("human")]);
}
